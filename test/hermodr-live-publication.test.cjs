"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const path = require("node:path");
const { createRequire } = require("node:module");
const { OdinLivePublicationSource } = require("../src/odin/live-publication-source.cjs");
const { createProviderSessionIngress } = require("../src/odin/provider-session-ingress.cjs");
const { HermodrStateStreamRegistry } = require("../src/hermodr-state-stream.cjs");
const { normalizeStateBindingSource } = require("../src/hermodr-daemon.cjs");

const selection = {
  providerId: "voidbot.swarm",
  sourceId: "voidbot.swarm_state_snapshot:voidbot-swarm",
  schemaId: "voidbot.swarm_state_snapshot.v1",
  recordKey: "voidbot-swarm",
};
const identity = { providerId: selection.providerId };

test("accepted provider publications drive ordered SSE state without polling", async () => {
  const source = new OdinLivePublicationSource(value => value);
  const streams = new HermodrStateStreamRegistry(source);
  const events = [];
  streams.acquire(selection, event => events.push(event));

  source.accept(identity, { schemaId: selection.schemaId, recordKey: selection.recordKey, payload: { summary: { globalHeat: 1.2 } } });
  source.accept(identity, { schemaId: selection.schemaId, recordKey: selection.recordKey, payload: { summary: { globalHeat: 0.15 } } });
  source.withdraw(identity, { schemaId: selection.schemaId, recordKey: selection.recordKey }, "lease-expired");
  source.accept(identity, { schemaId: selection.schemaId, recordKey: selection.recordKey, payload: { summary: { globalHeat: 0.2 } } });

  assert.deepEqual(events.map(event => [event.type, event.sequence]), [["snapshot", 1], ["update", 2], ["stale", 3], ["reconnected", 4]]);
  assert.equal(events[1].value.summary.globalHeat, 0.15);
  assert.equal((await source.forProvider(selection.providerId).latest(selection.schemaId, selection.recordKey)).summary.globalHeat, 0.2);
});

test("replayed accepted state is delivered immediately to a new typed source subscriber", () => {
  const source = new OdinLivePublicationSource(value => value);
  source.accept(identity, { schemaId: selection.schemaId, recordKey: selection.recordKey, payload: { heat: 1.2 } });
  const events = [];
  new HermodrStateStreamRegistry(source).acquire(selection, event => events.push(event));
  assert.deepEqual(events.map(event => event.type), ["snapshot"]);
  assert.deepEqual(events[0].value, { heat: 1.2 });
});

test("Eve source ids select record keys while pointer paths remain client-owned", () => {
  assert.deepEqual(normalizeStateBindingSource({ ...selection, pointerId: "summary.globalHeat" }), selection);
  assert.equal(normalizeStateBindingSource({ providerId: "voidbot.swarm", sourceId: "", schemaId: selection.schemaId }), null);
});

test("CultLib provider-session acceptance and withdrawal drive the live source", async () => {
  const cultLibRoot = process.env.CULTLIB_ROOT;
  assert.ok(cultLibRoot, "CULTLIB_ROOT must identify the CultLib reliability branch worktree");
  const requireCultMesh = createRequire(path.join(cultLibRoot, "packages", "cultmesh-ts", "package.json"));
  const runtime = requireCultMesh("./dist/index.js");
  const { CultNetDocumentRegistry } = requireCultMesh("cultnet-ts");
  const { decode } = requireCultMesh("@msgpack/msgpack");
  const source = new OdinLivePublicationSource(payload => decode(payload));
  const ingress = createProviderSessionIngress({
    runtime,
    CultNetDocumentRegistry,
    source,
    runtimeId: "odin-live-source-test",
    bindHost: "127.0.0.1",
    bindPort: 0,
    sessionToken: "test-provider-authority",
  });
  await ingress.start();
  const transport = new runtime.CultMeshProviderRudpTransport({
    endpoint: `rudp://127.0.0.1:${ingress.bind.port}`,
    runtimeId: "voidbot-worker-test",
    connectionId: 0x43554c54,
    sessionToken: "test-provider-authority",
  });
  const providerIdentity = {
    providerId: selection.providerId,
    serviceInstanceId: "voidbot-worker-test",
    endpointId: "voidbot-worker-test.rudp",
    verseId: "voidbot.local",
  };
  const connection = await transport.connect(providerIdentity, new AbortController().signal);
  try {
    const lease = await connection.register({ identity: providerIdentity, requestedLeaseDurationMs: 5_000 }, new AbortController().signal);
    await connection.publish({ publicationId: "swarm", schemaId: selection.schemaId, recordKey: selection.recordKey, value: { heat: 0.15 } }, lease);
    assert.deepEqual(await source.forProvider(selection.providerId).latest(selection.schemaId, selection.recordKey), { heat: 0.15 });
    await connection.withdrawPublication("swarm", lease);
    await assert.rejects(source.forProvider(selection.providerId).latest(selection.schemaId, selection.recordKey));
  } finally {
    connection.close();
    ingress.close();
  }
});

test("provider lease expiry withdraws accepted state and emits stale", async () => {
  const cultLibRoot = process.env.CULTLIB_ROOT;
  const requireCultMesh = createRequire(path.join(cultLibRoot, "packages", "cultmesh-ts", "package.json"));
  const runtime = requireCultMesh("./dist/index.js");
  const { CultNetDocumentRegistry } = requireCultMesh("cultnet-ts");
  const { decode } = requireCultMesh("@msgpack/msgpack");
  const source = new OdinLivePublicationSource(payload => decode(payload));
  const ingress = createProviderSessionIngress({
    runtime, CultNetDocumentRegistry, source,
    runtimeId: "odin-expiry-test", bindHost: "127.0.0.1", bindPort: 0,
    sessionToken: "expiry-token", expiryPollMs: 10,
  });
  await ingress.start();
  const identity = { providerId: selection.providerId, serviceInstanceId: "expiry-worker", endpointId: "expiry.rudp", verseId: "voidbot.local" };
  const connection = await new runtime.CultMeshProviderRudpTransport({
    endpoint: `rudp://127.0.0.1:${ingress.bind.port}`, runtimeId: "expiry-worker",
    connectionId: 0x43554c54, sessionToken: "expiry-token",
  }).connect(identity, new AbortController().signal);
  const events = [];
  const release = new HermodrStateStreamRegistry(source).acquire(selection, event => events.push(event));
  try {
    const lease = await connection.register({ identity, requestedLeaseDurationMs: 30 }, new AbortController().signal);
    await connection.publish({ publicationId: "swarm", schemaId: selection.schemaId, recordKey: selection.recordKey, value: { heat: 1.2 } }, lease);
    await new Promise(resolve => setTimeout(resolve, 80));
    assert.deepEqual(events.map(event => event.type), ["snapshot", "stale"]);
  } finally {
    release(); connection.close(); ingress.close();
  }
});

"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { HermodrStateStream, HermodrStateStreamRegistry } = require("../src/hermodr-state-stream.cjs");

const source = { providerId: "fixture", schemaId: "fixture.heat.v1", documentId: "swarm" };

test("state stream emits ordered provider snapshots and updates without rewriting documents", async () => {
  let now = 1_000;
  let document = { heat: 1.2, nested: { owner: "provider" } };
  const initialDocument = document;
  const stream = new HermodrStateStream(source, async () => document, {
    now: () => now,
    schedule: () => null,
  });
  const events = [];
  stream.on("event", event => events.push(event));
  stream.running = true;

  await stream.poll();
  await stream.poll();
  document = { heat: 0.15, nested: { owner: "provider" } };
  now += 1_000;
  await stream.poll();

  assert.deepEqual(events.map(event => [event.type, event.sequence]), [["snapshot", 1], ["update", 2]]);
  assert.strictEqual(events[0].value, initialDocument);
  assert.deepEqual(events[1].value, document);
});

test("state stream reports stale once and reconnected with the current provider document", async () => {
  let now = 5_000;
  let failure = false;
  const document = { heat: 1.2 };
  const stream = new HermodrStateStream(source, async () => {
    if (failure) throw new Error("provider unavailable");
    return document;
  }, { now: () => now, staleAfterMs: 100, schedule: () => null });
  const events = [];
  stream.on("event", event => events.push(event));
  stream.running = true;

  await stream.poll();
  failure = true;
  now += 100;
  await stream.poll();
  await stream.poll();
  failure = false;
  now += 100;
  await stream.poll();

  assert.deepEqual(events.map(event => [event.type, event.sequence]), [
    ["snapshot", 1], ["stale", 2], ["reconnected", 3],
  ]);
  assert.deepEqual(events[2].value, document);
});

test("registry multiplexes subscribers onto one provider stream and releases it after the last client", () => {
  const registry = new HermodrStateStreamRegistry({ schedule: () => null });
  const read = async () => ({ heat: 1.2 });
  const releaseFirst = registry.acquire(source, read, () => {});
  const releaseSecond = registry.acquire(source, read, () => {});

  assert.equal(registry.streams.size, 1);
  assert.equal([...registry.streams.values()][0].subscribers, 2);
  releaseFirst();
  assert.equal(registry.streams.size, 1);
  releaseFirst();
  releaseSecond();
  assert.equal(registry.streams.size, 0);
});

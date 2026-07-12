"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { createLiveProviderRegistry } = require("../src/odin/provider-ingress.cjs");

test("live provider registry unwraps Rust compatibility records", () => {
  const registry = createLiveProviderRegistry();
  registry.ingestDocument({
    schemaId: "gamecult.eve.provider_advertisement.v1",
    recordKey: "muninn.telemetry.nightwing",
    payload: [{
      value: {
        providerId: "muninn.telemetry.nightwing",
        title: "Muninn Nightwing Telemetry",
        status: "active",
        cultMeshAddress: "asgard.nightwing.muninn/telemetry",
        capabilities: ["muninn.move_evidence_stream"],
      },
    }],
  }, { address: "10.77.0.3", port: 40000 });

  const snapshot = registry.snapshot();
  assert.equal(snapshot.providerAdvertisements.length, 1);
  assert.equal(snapshot.providerAdvertisements[0].id, "muninn.telemetry.nightwing");
  assert.deepEqual(snapshot.providerAdvertisements[0].capabilities, ["muninn.move_evidence_stream"]);
  assert.equal(snapshot.providerAdvertisements[0].source, "cultnet-rudp://10.77.0.3:40000");
});

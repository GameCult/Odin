"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const {
  createBrowserCatalog,
  findProviderCdnRoute,
  normalizeProviderAdvertisement,
  normalizeStateBindingSource,
  surfaceRecordKeys,
} = require("../src/hermodr-daemon.cjs");

const provider = normalizeProviderAdvertisement({
  providerId: "fixture.game",
  title: "Fixture game",
  surfaces: [
    { surfaceId: "fixture.menu", surfaceKind: "menu", recordRef: "cultmesh://fixture/surfaces/menu" },
    { surfaceId: "fixture.world", surfaceKind: "interactive-world", recordRef: "cultmesh://fixture/surfaces/world" },
  ],
  routes: [{
    id: "fixture-cdn",
    uri: "rudp://127.0.0.1:3000",
    tags: ["cultmesh-cdn", "asset_blob"],
  }],
});

test("Hermodr preserves every advertised surface and its semantic kind", () => {
  const catalog = createBrowserCatalog([provider], { odinCultMeshUri: "cultmesh://odin/providers" });
  assert.deepEqual(catalog.surfaces.map(surface => surface.surfaceId), ["fixture.menu", "fixture.world"]);
  assert.equal(catalog.surfaces[1].surfaceKind, "interactive-world");
  assert.equal(catalog.providers[0].surfaces[1].recordRef, "cultmesh://fixture/surfaces/world");
});

test("surface reads prefer the provider-advertised record reference", () => {
  const catalog = createBrowserCatalog([provider]);
  assert.equal(surfaceRecordKeys(catalog, "fixture.game", "fixture.world")[0], "cultmesh://fixture/surfaces/world");
});

test("CDN routing follows the asset URI provider instead of a product name", () => {
  const catalog = createBrowserCatalog([provider]);
  const route = findProviderCdnRoute(catalog, "cultmesh://fixture.game/assets/player");
  assert.equal(route.providerId, "fixture.game");
  assert.equal(route.endpoint, "rudp://127.0.0.1:3000");
});

test("state binding sources select typed records without interpreting pointer paths", () => {
  assert.deepEqual(normalizeStateBindingSource({
    providerId: "voidbot.swarm",
    sourceId: "voidbot.swarm_state_snapshot:voidbot-swarm",
    schemaId: "voidbot.swarm_state_snapshot.v1",
    pointerId: "summary.globalHeat",
  }), {
    providerId: "voidbot.swarm",
    sourceId: "voidbot.swarm_state_snapshot:voidbot-swarm",
    schemaId: "voidbot.swarm_state_snapshot.v1",
    documentId: "voidbot-swarm",
  });
  assert.equal(normalizeStateBindingSource({ providerId: "voidbot.swarm", sourceId: "", schemaId: "schema.v1" }), null);
  assert.equal(normalizeStateBindingSource({ providerId: "voidbot.swarm", sourceId: "record", schemaId: "" }), null);
});

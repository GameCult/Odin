"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const {
  createBrowserCatalog,
  findProviderCdnRoute,
  hermodrRudpPeerOptions,
  normalizeProviderAdvertisement,
  normalizeSurfaceState,
  surfaceRecordKeys,
} = require("../src/hermodr-daemon.cjs");

test("Hermodr outbound RUDP peers bind beyond loopback", () => {
  const options = hermodrRudpPeerOptions();
  assert.equal(options.bindHost, "0.0.0.0");
  assert.equal(options.connectTimeoutMs, 2_000);
});

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

test("surface-state normalization lowers MessagePack array records", () => {
  const state = normalizeSurfaceState([
    "gjallar.overview",
    "Gjallar",
    7,
    "2026-08-25T00:00:00Z",
    [
      "gjallar.overview.surface",
      ["root", "dashboard", { title: "Gjallar" }, [], [], [], { mode: "weighted-bisect" }, {}],
      [],
    ],
  ], "gjallar.overview");

  assert.equal(state.surface.id, "gjallar.overview.surface");
  assert.equal(state.surface.root.kind, "dashboard");
  assert.equal(state.surface.root.layout.mode, "weighted-bisect");
});

test("CDN routing follows the asset URI provider instead of a product name", () => {
  const catalog = createBrowserCatalog([provider]);
  const route = findProviderCdnRoute(catalog, "cultmesh://fixture.game/assets/player");
  assert.equal(route.providerId, "fixture.game");
  assert.equal(route.endpoint, "rudp://127.0.0.1:3000");
});

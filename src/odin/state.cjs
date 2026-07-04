"use strict";

const fs = require("fs");
const { dashboardUnavailable } = require("./interfaces.cjs");
const {
  adbSnapshot,
  dockerSnapshot,
} = require("./probes.cjs");
const { buildMarqueeText } = require("./marquee.cjs");
const { buildPendingSurface, buildSurface } = require("./surface.cjs");
const { stableId } = require("./utils.cjs");

function createStateBuilder({ cachePath, gamecultTextDocumentStorePath, interfaceDiscovery, layoutStore, stonksBurstSize }) {
  let version = 0;

  function buildPendingState(message) {
    return {
      type: "dashboard-state",
      schema: "mimir.eve_dashboard_state.v1",
      providerId: "odin.providers",
      title: "Odin Provider Catalog",
      version,
      updatedAt: new Date().toISOString(),
      selectedNodeId: "coordinator-starting",
      lutPreset: "terminal",
      nodes: [],
      providerCatalog: [],
      surface: buildPendingSurface(message),
    };
  }

  async function buildState() {
    version += 1;
    const observedAt = new Date().toISOString();
    const [
      docker,
      adb,
      discovery,
    ] = await Promise.all([
      dockerSnapshot(),
      adbSnapshot(),
      interfaceDiscovery.discoverAll(),
    ]);
    const discoveredInterfaces = discovery.interfaces;
    const providerAdvertisements = discovery.providerAdvertisements;
    const interfaceById = new Map(discoveredInterfaces.map((entry) => [entry.providerId, entry]));
    const interfaces = [...interfaceById.values()];
    const marqueeText = await buildMarqueeText({ interfaces, textDocumentStorePath: gamecultTextDocumentStorePath, stonksBurstSize });
    const interfaceSummary = interfaces.map((entry) => `${entry.providerId}:${entry.state}`).join(", ");
    const layout = await layoutStore.readLayout();
    const verseEvidence = [...providerAdvertisements, ...interfaces];
    const voidBotDashboard = interfaceById.get("voidbot.swarm") || dashboardUnavailable("voidbot.swarm", "discovery", "not discovered");
    const mimirLiveStats = interfaceById.get("mimir.live.stats") || dashboardUnavailable("mimir.live.stats", "discovery", "not discovered");
    const periwinkleServices = [
      ...adbFaultServices(adb),
    ];

    const verses = [
      verse("starfire.local", "Starfire", "coordinator", "active", ["docker", "adb", "eve-provider"], [
        service("odin", "Odin all-seer", "active", "asgard.starfire.odin/eve/tui"),
        service("docker", "Docker", docker.state === "ok" ? "active" : "warn", `${docker.containers.length} running`),
        service("adb", "ADB transport", adbState(adb), adbDetail(adb)),
        service("cultcache", "Odin CultCache", fs.existsSync(cachePath) ? "active" : "waiting", require("path").basename(cachePath)),
        service("interfaces", "Eve interfaces", interfaces.some((entry) => entry.surface?.root) ? "active" : "waiting", interfaceSummary || "no provider surfaces"),
        service("voidbot-swarm", "VoidBot Swarm", voidBotDashboard.state, voidBotDashboard.detail),
        service("mimir-live-stats", "Mimir Live Stats", mimirLiveStats.state, mimirLiveStats.detail),
        ...docker.containers.map((container) => service(`docker-${container.name}`, container.name, "active", container.image)),
      ]),
      verse("nightwing.local", "Nightwing", "dashboard-renderer", bodyStatus(verseEvidence, "nightwing"), ["eve-tui", "gpu-worker"], bodyServices(verseEvidence, "nightwing")),
      verse("eve.ipad", "EVE", "ios-client", bodyStatus(verseEvidence, "eve"), ["native-eve"], bodyServices(verseEvidence, "eve")),
      periwinkleServices.length ? verse("periwinkle.android", "Periwinkle", "android-client", periwinkleStatus(periwinkleServices), ["sensor-edge", "adb-transport"], periwinkleServices) : null,
      verse("raven.local", "Raven", "local-peer", bodyStatus(verseEvidence, "raven"), ["cultmesh-provider"], bodyServices(verseEvidence, "raven")),
      verse("yggdrasil.ops", "Yggdrasil", "ops-host", bodyStatus(verseEvidence, "yggdrasil"), ["cultmesh-provider"], bodyServices(verseEvidence, "yggdrasil")),
    ].filter(Boolean);

    return {
      type: "dashboard-state",
      schema: "mimir.eve_dashboard_state.v1",
      providerId: "odin.providers",
      title: "Odin Provider Catalog",
      version,
      updatedAt: observedAt,
      selectedNodeId: "verse-starfire.local",
      lutPreset: "terminal",
      nodes: verses.map((entry, index) => ({
        id: `verse-${entry.verseId}`,
        label: `${entry.name}\n${entry.role}`,
        kind: "cultmesh-verse",
        visible: true,
        x: -0.8 + (index % 3) * 0.8,
        y: -0.45 + Math.floor(index / 3) * 0.55,
        z: 0,
        rotation: 0,
        scale: 1,
        width: 0.42,
        height: 0.2,
        health: entry.status,
        detail: entry.capabilities.join(", "),
      })),
      providerCatalog: providerAdvertisements,
      surface: buildSurface({ observedAt, docker, adb, verses, interfaces, layout, marqueeText }),
    };
  }

  return { buildPendingState, buildState };
}

function verse(verseId, name, role, status, capabilities, services = []) {
  return { verseId, name, role, status, capabilities, services };
}

function service(id, name, state, detail = "") {
  return { id: stableId(id), name, state, detail: String(detail || "") };
}

function bodyStatus(entries, body) {
  const matches = bodyEntries(entries, body);
  if (!matches.length) return "waiting";
  if (matches.some((entry) => providerState(entry) === "active")) return "active";
  if (matches.some((entry) => providerState(entry) === "warn")) return "warn";
  return "waiting";
}

function bodyServices(entries, body) {
  const matches = bodyEntries(entries, body);
  if (!matches.length) {
    return [service(`${body}-cultmesh-witnesses`, `${titleCase(body)} CultMesh witnesses`, "waiting", "no provider advertisement or interface discovered")];
  }

  return matches
    .slice(0, 8)
    .map((entry) => service(
      `${body}-${entry.providerId || entry.id || entry.title || "provider"}`,
      entry.title || entry.providerId || entry.id || `${titleCase(body)} provider`,
      providerState(entry),
      providerDetail(entry),
    ));
}

function bodyEntries(entries, body) {
  const token = String(body).toLowerCase();
  return entries.filter((entry) => providerBodyText(entry).includes(token));
}

function providerBodyText(entry) {
  return [
    entry.providerId,
    entry.id,
    entry.title,
    entry.canonicalService,
    entry.locatedService,
    entry.cultMeshAddress,
    entry.verseId,
    entry.source,
  ].filter(Boolean).join(" ").toLowerCase();
}

function providerState(entry) {
  const state = String(entry.state || entry.status || "").toLowerCase();
  if (["active", "ok", "ready", "running", "published"].includes(state)) return "active";
  if (["warn", "warning", "degraded"].includes(state)) return "warn";
  if (["failed", "error", "offline"].includes(state)) return "warn";
  return entry.surface?.root ? "active" : "waiting";
}

function providerDetail(entry) {
  return [
    entry.detail,
    entry.cultMeshAddress,
    entry.locatedService,
    entry.source,
  ].filter(Boolean).join(" / ") || "provider-owned CultMesh discovery";
}

function titleCase(value) {
  const text = String(value || "");
  return text ? text[0].toUpperCase() + text.slice(1) : "";
}

function adbState(adb) {
  if (adb.state !== "ok") return adb.state;
  return adbFaultDevices(adb).length ? "warn" : "ok";
}

function adbDetail(adb) {
  if (adb.error) return adb.error;
  if (adb.devices.length === 0) return "no devices";
  return adb.devices.map((device) => `${device.serial}:${device.state}`).join(", ");
}

function adbFaultDevices(adb) {
  return adb.devices.filter((device) => device.state !== "device");
}

function adbFaultServices(adb) {
  if (adb.state !== "ok") {
    return [service("adb-transport", "ADB transport", adb.state, adbDetail(adb))];
  }

  return adbFaultDevices(adb).map((device) =>
    service(`adb-${device.serial}`, "ADB device fault", device.state, `${device.serial}:${device.state}`),
  );
}

function periwinkleStatus(services) {
  if (services.some((entry) => entry.state === "active")) return "active";
  if (services.some((entry) => entry.state === "stale")) return "stale";
  if (services.some((entry) => entry.state === "offline" || entry.state === "unauthorized" || entry.state === "error" || entry.state === "missing")) return "warn";
  return "waiting";
}

module.exports = {
  createStateBuilder,
  service,
  verse,
};

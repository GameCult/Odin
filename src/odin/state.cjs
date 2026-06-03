"use strict";

const fs = require("fs");
const { dashboardUnavailable } = require("./interfaces.cjs");
const { observationServices, observationSnapshot } = require("./observations.cjs");
const {
  adbSnapshot,
  dockerSnapshot,
  hostChecks,
  hostServices,
  hostState,
  remoteGpu,
  remoteServices,
  systemdState,
} = require("./probes.cjs");
const { buildPendingSurface, buildSurface } = require("./surface.cjs");
const { stableId } = require("./utils.cjs");

function createStateBuilder({ cachePath, interfaceDiscovery, layoutStore, observationFreshSeconds, observationLogPath }) {
  let version = 0;

  function buildPendingState(message) {
    return {
      type: "dashboard-state",
      schema: "mimir.eve_dashboard_state.v1",
      providerId: "odin.allseer",
      title: "Odin All-Seer",
      version,
      updatedAt: new Date().toISOString(),
      selectedNodeId: "coordinator-starting",
      lutPreset: "terminal",
      nodes: [],
      surface: buildPendingSurface(message),
    };
  }

  async function buildState() {
    version += 1;
    const observedAt = new Date().toISOString();
    const [docker, adb, hosts, yggdrasilServices, nightwingServices, nightwingGpu, discoveredInterfaces, observations] = await Promise.all([
      dockerSnapshot(),
      adbSnapshot(),
      hostChecks(),
      remoteServices("ygg", ["nginx", "streampixels-web", "streampixels-service", "heimdall", "repixelizer-gui", "bifrost"]),
      remoteServices("nightwing", ["ssh", "nightwing-eve-dashboard", "nightwing-eve-browser-reference", "gamecult-visible-ops", "docker"]),
      remoteGpu("nightwing"),
      interfaceDiscovery.discoverInterfaces(),
      observationSnapshot(observationLogPath, observationFreshSeconds),
    ]);
    const interfaceById = new Map(discoveredInterfaces.map((entry) => [entry.providerId, entry]));
    const interfaces = [...interfaceById.values()];
    const interfaceSummary = interfaces.map((entry) => `${entry.providerId}:${entry.state}`).join(", ");
    const voidBotDashboard = interfaceById.get("voidbot.swarm") || dashboardUnavailable("voidbot.swarm", "discovery", "not discovered");
    const mimirLiveStats = interfaceById.get("mimir.live.stats") || dashboardUnavailable("mimir.live.stats", "discovery", "not discovered");
    const periwinkleServices = [
      ...adbFaultServices(adb),
      ...observationServices(observations, "periwinkle", service),
    ];

    const verses = [
      verse("starfire.local", "Starfire", "coordinator", "active", ["docker", "adb", "eve-provider"], [
        service("odin", "Odin all-seer", "active", "ws://0.0.0.0:8797/eve/deck"),
        service("docker", "Docker", docker.state === "ok" ? "active" : "warn", `${docker.containers.length} running`),
        service("adb", "ADB transport", adbState(adb), adbDetail(adb)),
        service("cultcache", "Odin CultCache", fs.existsSync(cachePath) ? "active" : "waiting", require("path").basename(cachePath)),
        service("interfaces", "Eve interfaces", interfaces.some((entry) => entry.surface?.root) ? "active" : "waiting", interfaceSummary || "no provider surfaces"),
        service("voidbot-swarm", "VoidBot Swarm", voidBotDashboard.state, voidBotDashboard.detail),
        service("mimir-live-stats", "Mimir Live Stats", mimirLiveStats.state, mimirLiveStats.detail),
        service("mimir-observation-ledger", "Mimir Observation Ledger", observations.state, observations.detail),
        ...docker.containers.map((container) => service(`docker-${container.name}`, container.name, "active", container.image)),
      ]),
      verse("nightwing.local", "Nightwing", "dashboard-renderer", hostState(hosts.Nightwing), ["eve-tui", "gpu-worker"], [
        ...hostServices("Nightwing", hosts.Nightwing),
        ...nightwingServices.map((entry) => service(`systemd-${entry.name}`, entry.name, systemdState(entry.state), entry.state)),
        service("gpu", "GTX 860M", nightwingGpu.state, nightwingGpu.detail),
      ]),
      verse("eve.ipad", "EVE", "ios-client", hostState(hosts.EVE), ["ssh", "native-eve"], hostServices("EVE", hosts.EVE)),
      periwinkleServices.length ? verse("periwinkle.android", "Periwinkle", "android-client", periwinkleStatus(periwinkleServices), ["sensor-edge", "adb-transport"], periwinkleServices) : null,
      verse("raven.local", "Raven", "local-peer", hostState(hosts.Raven), ["ssh"], hostServices("Raven", hosts.Raven)),
      verse("yggdrasil.ops", "Yggdrasil", "ops-host", hostState(hosts.Yggdrasil), ["ssh", "https", "services"], [
        ...hostServices("Yggdrasil", hosts.Yggdrasil),
        ...yggdrasilServices.map((entry) => service(`systemd-${entry.name}`, entry.name, systemdState(entry.state), entry.state)),
      ]),
    ].filter(Boolean);

    return {
      type: "dashboard-state",
      schema: "mimir.eve_dashboard_state.v1",
      providerId: "odin.allseer",
      title: "Odin All-Seer",
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
      surface: buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses, interfaces, observations, layout: layoutStore.readLayout() }),
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

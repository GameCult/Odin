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

    const verses = [
      verse("starfire.local", "Starfire", "coordinator", "active", ["docker", "adb", "eve-provider"], [
        service("odin", "Odin all-seer", "active", "ws://0.0.0.0:8797/eve/deck"),
        service("docker", "Docker", docker.state === "ok" ? "active" : "warn", `${docker.containers.length} running`),
        service("adb", "Periwinkle ADB", adb.devices.length ? "active" : "waiting", adb.devices.map((device) => `${device.serial}:${device.state}`).join(", ") || adb.error || "no devices"),
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
      verse("periwinkle.android", "Periwinkle", "android-client", adb.devices.length ? "connected" : "waiting", ["adb", "sensor-edge"], [
        service("adb-device", "ADB device", adb.devices.length ? "active" : "waiting", adb.devices.map((device) => `${device.serial}:${device.state}`).join(", ") || adb.error || "no devices"),
        ...observationServices(observations, "periwinkle", service),
      ]),
      verse("raven.local", "Raven", "local-peer", hostState(hosts.Raven), ["ssh"], hostServices("Raven", hosts.Raven)),
      verse("yggdrasil.ops", "Yggdrasil", "ops-host", hostState(hosts.Yggdrasil), ["ssh", "https", "services"], [
        ...hostServices("Yggdrasil", hosts.Yggdrasil),
        ...yggdrasilServices.map((entry) => service(`systemd-${entry.name}`, entry.name, systemdState(entry.state), entry.state)),
      ]),
    ];

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

module.exports = {
  createStateBuilder,
  service,
  verse,
};

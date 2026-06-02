#!/usr/bin/env node
"use strict";

const crypto = require("crypto");
const fs = require("fs");
const http = require("http");
const net = require("net");
const os = require("os");
const path = require("path");
const { execFile } = require("child_process");
const { promisify } = require("util");
const Module = require("module");

const execFileAsync = promisify(execFile);

const repoRoot = path.resolve(__dirname, "..");
process.env.NODE_PATH = [
  path.resolve(repoRoot, "..", "CultLib", "packages"),
  process.env.NODE_PATH || "",
].filter(Boolean).join(path.delimiter);
Module._initPaths();

let CultMesh;
let defineDocumentType;
try {
  ({ CultMesh } = require("cultmesh-ts/dist/index.js"));
  ({ defineDocumentType } = require("cultcache-ts/dist/index.js"));
} catch (error) {
  console.error("CultMesh runtime unavailable; durable mesh snapshot disabled:", error.message);
}

const args = parseArgs(process.argv.slice(2));
const port = Number(args.port || 8797);
const host = args.host || "0.0.0.0";
const intervalMs = Number(args.intervalMs || 5000);
const stateDir = args.stateDir || path.join(repoRoot, "scratch", "odin");
const cachePath = args.cachePath || path.join(stateDir, "odin.ccmp");
const surfaceKey = "surface:gamecult.network.status";
const layoutPath = args.layoutPath || path.join(stateDir, "interface-layout.json");
const seedDeckUrls = String(args.eveDeckUrl || "ws://127.0.0.1:8795/eve/deck,ws://192.168.1.75:8795/eve/deck")
  .split(",")
  .map((entry) => entry.trim())
  .filter(Boolean);
const observationLogPath = args.observationLogPath || path.join(repoRoot, "..", "Mimir", "artifacts", "runtime", "periwinkle-cultmesh-sensors.out.log");
const observationFreshSeconds = Number(args.observationFreshSeconds || 120);
const interfaceBindingStores = String(
  args.interfaceBindingStore ||
  path.join(repoRoot, "..", "VoidBot", ".voidbot", "status", "cultmesh", "voidbot-swarm-state.cc"),
)
  .split(",")
  .map((entry) => entry.trim())
  .filter(Boolean);

fs.mkdirSync(stateDir, { recursive: true });

const surfaceDefinition = defineDocumentType
  ? defineDocumentType({
      type: "gamecult.eve.surface_state",
      schemaName: "gamecult.eve.surface_state",
      schemaId: "gamecult.eve.surface_state.v1",
      schemaVersion: "gamecult.eve.surface_state.v1",
      global: true,
      name: (value) => value?.providerId || "surface",
      schema: { parse: (value) => value },
      members: [
        { slot: 0, memberName: "providerId", typeName: "string", isName: true },
        { slot: 1, memberName: "title", typeName: "string" },
        { slot: 2, memberName: "version", typeName: "long" },
        { slot: 3, memberName: "updatedAt", typeName: "string" },
        { slot: 4, memberName: "surface", typeName: "object" },
      ],
    })
  : null;
const interfaceBindingDefinition = defineDocumentType
  ? defineDocumentType({
      type: "gamecult.eve.interface_binding",
      schemaName: "gamecult.eve.interface_binding",
      schemaId: "gamecult.eve.interface_binding.v1",
      schemaVersion: "gamecult.eve.interface_binding.v1",
      global: true,
      name: (value) => value?.bindingId || value?.providerId || "interface",
      schema: parseObjectDocument("Eve interface binding"),
    })
  : null;
const providerAdvertisementDefinition = defineDocumentType
  ? defineDocumentType({
      type: "gamecult.eve.provider_advertisement",
      schemaName: "gamecult.eve.provider_advertisement",
      schemaId: "gamecult.eve.provider_advertisement.v1",
      schemaVersion: "gamecult.eve.provider_advertisement.v1",
      global: true,
      name: (value) => value?.providerId || "provider",
      schema: parseObjectDocument("Eve provider advertisement"),
    })
  : null;
const voidbotSwarmSnapshotDefinition = defineDocumentType
  ? defineDocumentType({
      type: "voidbot.swarm_state_snapshot",
      schemaName: "voidbot.swarm_state_snapshot",
      schemaId: "voidbot.swarm_state_snapshot.v1",
      schemaVersion: "voidbot.swarm_state_snapshot.v1",
      global: true,
      schema: parseObjectDocument("VoidBot swarm snapshot"),
    })
  : null;

let meshNodePromise = null;
let version = 0;
let currentState = buildPendingState("Coordinator starting");
let discoveredDeckUrls = [...seedDeckUrls];
let lastLanScanAt = 0;
const clients = new Set();

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

async function main() {
  if (CultMesh && surfaceDefinition) {
    meshNodePromise = CultMesh.createNode(cachePath, { documents: [surfaceDefinition] });
  }

  const server = http.createServer(handleHttp);
  server.on("upgrade", handleUpgrade);
  server.listen(port, host, () => {
    console.log(`Odin coordinator listening on ws://${host}:${port}/eve/deck`);
    console.log(`Durable surface cache: ${cachePath}`);
  });

  await refresh();
  setInterval(() => {
    refresh().catch((error) => console.error("refresh failed:", error));
  }, intervalMs);
}

async function refresh() {
  currentState = await buildState();
  await persistState(currentState);
  const payload = JSON.stringify(currentState);
  for (const client of [...clients]) {
    try {
      sendFrame(client, 0x1, Buffer.from(payload, "utf8"));
    } catch {
      clients.delete(client);
      client.destroy();
    }
  }
}

async function persistState(state) {
  fs.writeFileSync(path.join(stateDir, "latest-surface.json"), JSON.stringify(state, null, 2), "utf8");
  if (!meshNodePromise || !surfaceDefinition) {
    return;
  }

  try {
    const node = await meshNodePromise;
    await node.put(surfaceDefinition, surfaceKey, state);
    await node.flush?.(true);
  } catch (error) {
    console.error("CultMesh snapshot write failed:", error.message);
  }
}

async function handleHttp(req, res) {
  if (req.url === "/health") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      ok: true,
      providerId: currentState.providerId,
      version: currentState.version,
      clients: clients.size,
      cachePath,
    }));
    return;
  }

  if (req.url === "/eve/deck/providers") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      providers: [{
        id: currentState.providerId,
        title: currentState.title,
        description: "Odin central coordinator for GameCult Verse discovery, schema awareness, translation, and network status.",
        version: String(currentState.version),
        endpoint: "/eve/deck",
        capabilities: ["network-status", "cultmesh-verses", "cultui-surface"],
        usesCultMesh: true,
        transport: "CultMesh durable surface + Eve WebSocket",
      }],
    }));
    return;
  }

  res.writeHead(404, { "content-type": "text/plain" });
  res.end("not found");
}

function handleUpgrade(req, socket) {
  if (!req.url.startsWith("/eve/deck")) {
    socket.end("HTTP/1.1 404 Not Found\r\n\r\n");
    return;
  }

  const key = req.headers["sec-websocket-key"];
  if (!key) {
    socket.end("HTTP/1.1 400 Bad Request\r\n\r\n");
    return;
  }

  const accept = crypto
    .createHash("sha1")
    .update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
    .digest("base64");
  socket.write(
    "HTTP/1.1 101 Switching Protocols\r\n" +
    "Upgrade: websocket\r\n" +
    "Connection: Upgrade\r\n" +
    `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  clients.add(socket);
  sendFrame(socket, 0x1, Buffer.from(JSON.stringify(currentState), "utf8"));
  socket.on("data", (chunk) => handleClientFrame(socket, chunk));
  socket.on("close", () => clients.delete(socket));
  socket.on("error", () => clients.delete(socket));
}

function handleClientFrame(socket, chunk) {
  const frame = tryReadFrame(chunk);
  const opcode = frame?.opcode ?? (chunk[0] & 0x0f);
  if (opcode === 0x8) {
    clients.delete(socket);
    socket.end();
    return;
  }
  if (opcode !== 0x1 || !frame) {
    return;
  }
  try {
    const command = JSON.parse(frame.payload.toString("utf8"));
    applyClientCommand(command);
  } catch {
    // Renderer input is advisory. Bad client frames do not get to kill Odin.
  }
}

function sendFrame(socket, opcode, payload) {
  const header = [0x80 | opcode];
  if (payload.length < 126) {
    header.push(payload.length);
  } else if (payload.length <= 0xffff) {
    header.push(126, payload.length >> 8, payload.length & 0xff);
  } else {
    const length = Buffer.alloc(8);
    length.writeBigUInt64BE(BigInt(payload.length));
    header.push(127, ...length);
  }
  socket.write(Buffer.concat([Buffer.from(header), payload]));
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
    discoverInterfaces(),
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
      service("cultcache", "Odin CultCache", fs.existsSync(cachePath) ? "active" : "waiting", path.basename(cachePath)),
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
      ...observationServices(observations, "periwinkle"),
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
    surface: buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses, interfaces, observations }),
  };
}

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
    surface: {
      schema: "gamecult.eve.surface.v1",
      id: "gamecult.network.status.surface",
      title: "Odin",
      root: stack("root", [pane("Coordinator", [text("status", message)])]),
      assets: [],
    },
  };
}

function parseObjectDocument(label) {
  return {
    parse(value) {
      if (!value || typeof value !== "object") {
        throw new Error(`${label} must be an object`);
      }
      return value;
    },
  };
}

function buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses, interfaces, observations }) {
  const activeInterfaces = interfaces.filter((entry) => entry.surface?.root);
  const activeObservationStreams = observations.streams.filter((entry) => entry.state === "active");
  const layout = readLayout();
  return {
    schema: "gamecult.eve.surface.v1",
    id: "gamecult.network.status.surface",
    title: "Odin",
    root: {
      id: "network-root",
      kind: "dashboard",
      props: {
        title: "Odin All-Seer",
        observedAt,
        summary: `${verses.length} Verses / ${verses.reduce((sum, entry) => sum + entry.services.length, 0)} services / ${activeInterfaces.length} interfaces / ${activeObservationStreams.length} live streams`,
      },
      children: [
        pane("Coordinator", [
          text("observed", `observed ${observedAt}`),
          text("authority", "Odin owns Verse discovery, schema awareness, translation planning, and accepted surface publication. Renderers lower only."),
          metric("docker-count", "Docker containers", docker.containers.length, docker.state === "ok" ? "ok" : "warn"),
          metric("adb-count", "ADB devices", adb.devices.length, adb.devices.length ? "ok" : "warn"),
          metric("observation-stream-count", "Observation streams", observations.streams.length, activeObservationStreams.length ? "ok" : "warn"),
          text("observation-ledger", `observation ledger: ${observations.detail}`),
        ]),
        observationPane(observations),
        ...interfaces.map((entry) => ({
          id: `interface-${stableId(entry.providerId)}`,
          kind: "interface",
          props: {
            title: entry.title,
            providerId: entry.providerId,
            source: entry.source,
            status: entry.state,
            detail: entry.detail,
            version: entry.version,
            updatedAt: entry.updatedAt,
            layout: mergeLayoutIntent(layout.tiles?.[entry.providerId], entry, interfaces),
          },
          children: entry.surface?.root ? [entry.surface.root] : [
            text(`interface-${stableId(entry.providerId)}-missing`, entry.detail || "interface unavailable"),
          ],
        })),
        ...verses.map((entry) => ({
          id: `verse-${entry.verseId}`,
          kind: "verse",
          props: {
            title: entry.name,
            verseId: entry.verseId,
            role: entry.role,
            status: entry.status,
            capabilities: entry.capabilities,
            services: entry.services,
          },
          children: entry.services.map((item) => ({
            id: `service-${entry.verseId}-${item.id}`,
            kind: "service",
            props: item,
            children: [],
          })),
        })),
        pane("Verses", verses.map((entry) =>
          card(`verse-card-${entry.verseId}`, [
            text(`verse-title-${entry.verseId}`, `${entry.name} :: ${entry.verseId}`),
            text(`verse-status-${entry.verseId}`, `${entry.role} / ${entry.status}`),
            text(`verse-caps-${entry.verseId}`, entry.capabilities.join(", ")),
          ], entry.status),
        )),
        pane("Hosts", Object.entries(hosts).map(([name, checks]) =>
          card(`host-${name}`, [
            text(`host-name-${name}`, name),
            ...checks.map((check) => text(`host-${name}-${check.name}`, `${check.name}: ${check.state}${check.detail ? ` ${check.detail}` : ""}`)),
          ], checks.some((check) => check.state === "open" || check.state === "ok") ? "ok" : "warn"),
        )),
        pane("Yggdrasil Services", yggdrasilServices.map((service) =>
          text(`ygg-${service.name}`, `${service.name}: ${service.state}`),
        )),
        pane("Docker", docker.containers.length
          ? docker.containers.slice(0, 10).map((container) => text(`docker-${container.name}`, `${container.name}: ${container.status} (${container.image})`))
          : [text("docker-empty", docker.error || "no running containers")]),
        pane("Periwinkle / ADB", adb.devices.length
          ? adb.devices.map((device) => text(`adb-${device.serial}`, `${device.serial}: ${device.state}`))
          : [text("adb-empty", adb.error || "no devices")]),
      ],
    },
    assets: [],
  };
}

function observationPane(observations) {
  return {
    id: "observation-streams",
    kind: "pane",
    props: {
      title: "Device Observation Streams",
      source: observations.source,
      status: observations.state,
      detail: observations.detail,
    },
    children: observations.streams.length
      ? observations.streams.map(observationStreamNode)
      : [text("observation-streams-empty", observations.detail || "no observation streams")],
  };
}

function observationStreamNode(entry) {
  return {
    id: `observation-stream-${stableId(entry.deviceId)}-${stableId(entry.streamId)}-${stableId(entry.kind)}`,
    kind: "observation-stream",
    props: entry,
    children: [
      text(`observation-${stableId(entry.streamId)}-schema`, entry.document),
      text(`observation-${stableId(entry.streamId)}-latest`, `${entry.kind} seq ${entry.sequence} age ${entry.ageSeconds}s`),
      text(`observation-${stableId(entry.streamId)}-shape`, observationShape(entry)),
    ],
  };
}

function stack(id, children) {
  return { id, kind: "stack", props: {}, children };
}

function pane(title, children) {
  return { id: stableId(`pane-${title}`), kind: "pane", props: { title }, children };
}

function card(id, children, tone = "default") {
  return { id, kind: "card", props: { title: id }, style: { tone }, children };
}

function text(id, value) {
  return { id, kind: "text", props: { text: value }, children: [] };
}

function metric(id, label, value, tone) {
  return { id, kind: "metric", props: { label, text: `${label}: ${value}`, value, tone }, children: [] };
}

function verse(verseId, name, role, status, capabilities, services = []) {
  return { verseId, name, role, status, capabilities, services };
}

function service(id, name, state, detail = "") {
  return { id: stableId(id), name, state, detail: String(detail || "") };
}

function observationServices(observations, deviceId) {
  return observations.streams
    .filter((entry) => entry.deviceId === deviceId)
    .map((entry) => service(
      `observation-${entry.streamId}-${entry.kind}`,
      `${entry.kind} observation`,
      entry.state,
      observationShape(entry),
    ));
}

function hostServices(hostName, checks) {
  return (checks || []).map((check) =>
    service(`${hostName}-${check.name}`, `${hostName} ${check.name}`, check.state === "open" || check.state === "ok" ? "active" : check.state, check.detail),
  );
}

function systemdState(state) {
  if (state === "active") return "active";
  if (state === "inactive") return "inactive";
  if (state === "failed") return "failed";
  return state.startsWith("unknown") ? "unknown" : "warn";
}

function hostState(checks) {
  if (!checks) return "unknown";
  if (checks.some((check) => check.state === "open" || check.state === "ok")) return "reachable";
  if (checks.some((check) => check.state === "timeout")) return "timeout";
  return "offline";
}

async function hostChecks() {
  return {
    Starfire: [{ name: "local", state: "ok", detail: os.hostname() }],
    Nightwing: await checksFor("192.168.1.75", [["ssh", 22], ["eve-broker", 8795], ["eve-browser", 8891]]),
    Raven: await checksFor("192.168.1.84", [["ssh", 22]]),
    EVE: await checksFor("192.168.1.72", [["ssh", 22]]),
    Yggdrasil: await checksFor("yggdrasil.gamecult.org", [["ssh", 22], ["http", 80], ["https", 443]]),
  };
}

async function checksFor(host, specs) {
  return Promise.all(specs.map(async ([name, port]) => ({
    name,
    ...(await tcpCheck(host, port)),
  })));
}

function tcpCheck(host, port) {
  return new Promise((resolve) => {
    const socket = net.createConnection({ host, port, timeout: 900 });
    socket.on("connect", () => {
      socket.destroy();
      resolve({ state: "open", detail: `${host}:${port}` });
    });
    socket.on("timeout", () => {
      socket.destroy();
      resolve({ state: "timeout", detail: `${host}:${port}` });
    });
    socket.on("error", (error) => {
      resolve({ state: "closed", detail: error.code || error.message });
    });
  });
}

async function dockerSnapshot() {
  try {
    const { stdout } = await execFileAsync("docker", ["ps", "--format", "{{json .}}"], { timeout: 3000 });
    const containers = stdout.split(/\r?\n/)
      .filter(Boolean)
      .map((line) => JSON.parse(line))
      .map((item) => ({
        name: item.Names,
        image: item.Image,
        status: item.Status,
        ports: item.Ports,
      }));
    return { state: "ok", containers, error: "" };
  } catch (error) {
    return { state: "error", containers: [], error: error.message };
  }
}

async function adbSnapshot() {
  const adb = path.join(process.env.LOCALAPPDATA || "", "Android", "Sdk", "platform-tools", "adb.exe");
  if (!fs.existsSync(adb)) {
    return { state: "missing", devices: [], error: `missing ${adb}` };
  }
  try {
    const { stdout } = await execFileAsync(adb, ["devices"], { timeout: 3000 });
    const devices = stdout.split(/\r?\n/)
      .map((line) => line.match(/^(\S+)\s+(device|offline|unauthorized)$/))
      .filter(Boolean)
      .map((match) => ({ serial: match[1], state: match[2] }));
    return { state: "ok", devices, error: "" };
  } catch (error) {
    return { state: "error", devices: [], error: error.message };
  }
}

async function observationSnapshot(source, freshSeconds) {
  if (!fs.existsSync(source)) {
    return {
      state: "missing",
      source,
      detail: `missing ${source}`,
      streams: [],
    };
  }

  try {
    const lines = tailTextFile(source, 512 * 1024)
      .split(/\r?\n/)
      .filter(Boolean);
    const nowMs = Date.now();
    const byStream = new Map();
    let accepted = 0;
    for (const line of lines) {
      let item;
      try {
        item = JSON.parse(line);
      } catch {
        continue;
      }

      if (item.type !== "cultmesh-observation" && item.type !== "cultmesh-media-observation") {
        continue;
      }

      accepted += 1;
      const key = `${item.DeviceId || "unknown"}:${item.StreamId || "unknown"}:${item.Kind || "unknown"}`;
      byStream.set(key, item);
    }

    const streams = [...byStream.values()]
      .map((item) => observationStream(item, nowMs, freshSeconds))
      .sort((left, right) => left.deviceId.localeCompare(right.deviceId)
        || left.streamId.localeCompare(right.streamId)
        || left.kind.localeCompare(right.kind));

    const activeCount = streams.filter((entry) => entry.state === "active").length;
    return {
      state: streams.length ? (activeCount ? "active" : "stale") : "waiting",
      source,
      detail: `${streams.length} streams, ${activeCount} active, ${accepted} recent records`,
      streams,
    };
  } catch (error) {
    return {
      state: "error",
      source,
      detail: error.message,
      streams: [],
    };
  }
}

function tailTextFile(filePath, maxBytes) {
  const stat = fs.statSync(filePath);
  const length = Math.min(stat.size, maxBytes);
  const buffer = Buffer.alloc(length);
  const fd = fs.openSync(filePath, "r");
  try {
    fs.readSync(fd, buffer, 0, length, stat.size - length);
  } finally {
    fs.closeSync(fd);
  }

  return buffer.toString("utf8");
}

function observationStream(item, nowMs, freshSeconds) {
  const latestMs = Date.parse(item.WallClockUtc || "");
  const ageSeconds = Number.isFinite(latestMs) ? Math.max(0, Math.round((nowMs - latestMs) / 1000)) : null;
  return {
    deviceId: String(item.DeviceId || "unknown"),
    streamId: String(item.StreamId || "unknown"),
    kind: String(item.Kind || "unknown"),
    document: String(item.document || "unknown"),
    state: ageSeconds === null ? "unknown" : ageSeconds <= freshSeconds ? "active" : "stale",
    sequence: item.Sequence ?? null,
    latestAt: item.WallClockUtc || "",
    ageSeconds,
    clockDomainId: item.ClockDomainId || "",
    format: item.Format || "",
    width: item.Width ?? null,
    height: item.Height ?? null,
    sampleRate: item.SampleRate ?? null,
    channels: item.Channels ?? null,
    frameCount: item.FrameCount ?? null,
    payloadEncoding: item.PayloadEncoding || "",
    payloadBytes: item.PayloadBytes ?? null,
    values: Array.isArray(item.Values) ? item.Values : null,
    accuracy: item.Accuracy ?? null,
    action: item.Action ?? null,
    pointerCount: item.PointerCount ?? null,
    x: item.X ?? null,
    y: item.Y ?? null,
  };
}

function observationShape(entry) {
  if (entry.width && entry.height) {
    return `${entry.format || "media"} ${entry.width}x${entry.height}, ${entry.payloadBytes || 0} bytes`;
  }

  if (entry.sampleRate) {
    return `${entry.format || "audio"} ${entry.sampleRate} Hz x ${entry.channels || 1}, ${entry.frameCount || 0} frames, ${entry.payloadBytes || 0} bytes`;
  }

  if (entry.values) {
    return `${entry.values.map((value) => Number(value).toFixed(3)).join(", ")} accuracy ${entry.accuracy ?? "unknown"}`;
  }

  if (entry.action) {
    return `${entry.action} pointers ${entry.pointerCount ?? "unknown"} @ ${entry.x ?? "?"},${entry.y ?? "?"}`;
  }

  return entry.state;
}

async function remoteServices(target, services) {
  const script = `for s in ${services.join(" ")}; do printf '%s=' "$s"; systemctl is-active "$s" 2>/dev/null || true; done`;
  try {
    const { stdout } = await execFileAsync("ssh", ["-o", "BatchMode=yes", "-o", "ConnectTimeout=3", target, script], { timeout: 6000 });
    return stdout.split(/\r?\n/)
      .map((line) => line.match(/^([^=]+)=(.+)$/))
      .filter(Boolean)
      .map((match) => ({ name: match[1], state: match[2].trim() }));
  } catch (error) {
    return services.map((name) => ({ name, state: `unknown: ${error.message}` }));
  }
}

async function remoteGpu(target) {
  try {
    const { stdout } = await execFileAsync("ssh", [
      "-o", "BatchMode=yes",
      "-o", "ConnectTimeout=3",
      target,
      "nvidia-smi --query-gpu=name,driver_version,memory.total,utilization.gpu --format=csv,noheader,nounits 2>/dev/null | head -n 1",
    ], { timeout: 6000 });
    const detail = stdout.trim();
    return detail ? { state: "active", detail } : { state: "unknown", detail: "nvidia-smi returned no GPU" };
  } catch (error) {
    return { state: "unknown", detail: error.message };
  }
}

async function discoverInterfaces() {
  await refreshLanDeckDiscovery();
  const manifestsByProvider = new Map();
  for (const deckUrl of discoveredDeckUrls) {
    const manifestUrl = deckUrl.replace(/^ws:/, "http:").replace(/\/eve\/deck.*$/, "/eve/deck/providers");
    try {
      const catalog = JSON.parse(await httpGet(manifestUrl, 2500));
      for (const provider of catalog.providers || []) {
        if (!provider?.id || provider.id === "eve.dashboard.broker") continue;
        const existing = manifestsByProvider.get(provider.id);
        if (!existing || deckUrl.startsWith("ws://127.0.0.1")) {
          manifestsByProvider.set(provider.id, { provider, deckUrl });
        }
      }
    } catch {
      // Discovery is opportunistic. Failed endpoints fall out of the next pass.
    }
  }

  const interfaces = [];
  for (const { provider, deckUrl } of manifestsByProvider.values()) {
    interfaces.push(await fetchEveProvider(deckUrl, provider.id, provider));
  }
  for (const entry of await discoverCultMeshInterfaceBindings()) {
    const existingIndex = interfaces.findIndex((candidate) => candidate.providerId === entry.providerId);
    if (existingIndex >= 0) {
      interfaces[existingIndex] = entry;
    } else {
      interfaces.push(entry);
    }
  }
  interfaces.sort((left, right) => left.providerId.localeCompare(right.providerId));
  return interfaces;
}

async function discoverCultMeshInterfaceBindings() {
  if (!CultMesh || !interfaceBindingDefinition || !surfaceDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
    return [];
  }
  const interfaces = [];
  for (const storePath of interfaceBindingStores) {
    try {
      if (!fs.existsSync(storePath)) {
        continue;
      }
      const node = await CultMesh.createNode(storePath, {
        documents: [
          voidbotSwarmSnapshotDefinition,
          providerAdvertisementDefinition,
          interfaceBindingDefinition,
          surfaceDefinition,
        ],
      });
      const binding = node.get(interfaceBindingDefinition, "voidbot.swarm");
      if (!binding?.providerId) {
        continue;
      }
      const state = node.get(surfaceDefinition, binding.providerId);
      interfaces.push({
        providerId: binding.providerId,
        title: binding.title || state?.title || binding.providerId,
        state: "active",
        detail: `${state?.surface?.root?.kind || binding.kind || "surface"} ${state?.nodes?.length || 0} nodes via CultMesh`,
        version: state?.version || 0,
        updatedAt: state?.updatedAt || binding.updatedAt || new Date().toISOString(),
        source: `cultmesh:${storePath}`,
        manifest: binding.provider || null,
        surface: state?.surface || binding.surface || null,
      });
    } catch (error) {
      interfaces.push(dashboardUnavailable("voidbot.swarm", `cultmesh:${storePath}`, error.message));
    }
  }
  return interfaces;
}

async function refreshLanDeckDiscovery() {
  const now = Date.now();
  if (now - lastLanScanAt < 60000) return;
  lastLanScanAt = now;
  const urls = new Set(seedDeckUrls);
  const checks = [];
  for (const prefix of localIpv4Prefixes()) {
    for (let host = 1; host <= 254; host += 1) {
      const address = `${prefix}.${host}`;
      checks.push(tcpCheck(address, 8795).then((check) => {
        if (check.state === "open") urls.add(`ws://${address}:8795/eve/deck`);
      }));
    }
  }
  await Promise.allSettled(checks);
  discoveredDeckUrls = [...urls].sort();
}

function localIpv4Prefixes() {
  const prefixes = new Set();
  for (const entries of Object.values(os.networkInterfaces())) {
    for (const entry of entries || []) {
      if (entry.family !== "IPv4" || entry.internal) continue;
      const parts = entry.address.split(".");
      if (parts.length === 4) prefixes.add(parts.slice(0, 3).join("."));
    }
  }
  return [...prefixes];
}

function httpGet(url, timeoutMs) {
  return new Promise((resolve, reject) => {
    const req = http.get(url, { timeout: timeoutMs }, (res) => {
      if ((res.statusCode || 0) >= 400) {
        reject(new Error(`HTTP ${res.statusCode}`));
        res.resume();
        return;
      }
      const chunks = [];
      res.on("data", (chunk) => chunks.push(chunk));
      res.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    });
    req.on("timeout", () => req.destroy(new Error("HTTP request timed out")));
    req.on("error", reject);
  });
}

async function fetchEveProvider(url, providerId, manifest = null) {
  try {
    const socket = await openWebSocket(url);
    try {
      sendClientFrame(socket, JSON.stringify({ type: "open-provider", providerId }));
      for (let index = 0; index < 8; index += 1) {
        const message = await readServerTextFrame(socket, 2500);
        const state = JSON.parse(message);
        if (state?.providerId === providerId) {
          return {
            providerId,
            title: state.title || manifest?.title || providerId,
            state: "active",
            detail: `${state.surface?.root?.kind || "surface"} ${state.nodes?.length || 0} nodes`,
            version: state.version,
            updatedAt: state.updatedAt,
            source: url,
            manifest,
            surface: state.surface,
          };
        }
      }
      return dashboardUnavailable(providerId, url, "provider did not publish matching state");
    } finally {
      socket.destroy();
    }
  } catch (error) {
    return dashboardUnavailable(providerId, url, error.message);
  }
}

function dashboardUnavailable(providerId, source, detail) {
  return {
    providerId,
    title: providerId,
    state: "unavailable",
    detail,
    version: 0,
    updatedAt: new Date().toISOString(),
    source,
    manifest: null,
    surface: null,
  };
}

function openWebSocket(url) {
  return new Promise((resolve, reject) => {
    const parsed = new URL(url);
    const port = Number(parsed.port || (parsed.protocol === "wss:" ? 443 : 80));
    const socket = net.createConnection({ host: parsed.hostname, port, timeout: 2500 });
    const key = crypto.randomBytes(16).toString("base64");
    const pathName = `${parsed.pathname || "/"}${parsed.search || ""}`;
    let buffer = Buffer.alloc(0);
    socket.on("connect", () => {
      socket.write([
        `GET ${pathName} HTTP/1.1`,
        `Host: ${parsed.hostname}:${port}`,
        "Upgrade: websocket",
        "Connection: Upgrade",
        `Sec-WebSocket-Key: ${key}`,
        "Sec-WebSocket-Version: 13",
        "",
        "",
      ].join("\r\n"));
    });
    socket.on("data", function onHandshake(chunk) {
      buffer = Buffer.concat([buffer, chunk]);
      const marker = buffer.indexOf("\r\n\r\n");
      if (marker < 0) return;
      const header = buffer.subarray(0, marker).toString("latin1");
      if (!header.startsWith("HTTP/1.1 101")) {
        reject(new Error(header.split(/\r?\n/)[0] || "websocket handshake failed"));
        socket.destroy();
        return;
      }
      socket.off("data", onHandshake);
      socket.unshift(buffer.subarray(marker + 4));
      resolve(socket);
    });
    socket.on("timeout", () => {
      reject(new Error("websocket connection timed out"));
      socket.destroy();
    });
    socket.on("error", reject);
  });
}

function sendClientFrame(socket, textValue) {
  const payload = Buffer.from(textValue, "utf8");
  const mask = crypto.randomBytes(4);
  const header = [0x81];
  if (payload.length < 126) {
    header.push(0x80 | payload.length);
  } else if (payload.length <= 0xffff) {
    header.push(0x80 | 126, payload.length >> 8, payload.length & 0xff);
  } else {
    const length = Buffer.alloc(8);
    length.writeBigUInt64BE(BigInt(payload.length));
    header.push(0x80 | 127, ...length);
  }
  const masked = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]));
  socket.write(Buffer.concat([Buffer.from(header), mask, masked]));
}

function readServerTextFrame(socket, timeoutMs) {
  return new Promise((resolve, reject) => {
    let buffer = Buffer.alloc(0);
    const timer = setTimeout(() => cleanup(new Error("timed out waiting for dashboard frame")), timeoutMs);
    function cleanup(error, value) {
      clearTimeout(timer);
      socket.off("data", onData);
      socket.off("error", onError);
      if (error) reject(error);
      else resolve(value);
    }
    function onError(error) {
      cleanup(error);
    }
    function onData(chunk) {
      buffer = Buffer.concat([buffer, chunk]);
      const frame = tryReadFrame(buffer);
      if (!frame) return;
      buffer = buffer.subarray(frame.consumed);
      if (frame.opcode === 0x1) cleanup(null, frame.payload.toString("utf8"));
      if (frame.opcode === 0x8) cleanup(new Error("dashboard websocket closed"));
    }
    socket.on("data", onData);
    socket.on("error", onError);
  });
}

function tryReadFrame(buffer) {
  if (buffer.length < 2) return null;
  const opcode = buffer[0] & 0x0f;
  const masked = Boolean(buffer[1] & 0x80);
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < offset + 2) return null;
    length = buffer.readUInt16BE(offset);
    offset += 2;
  } else if (length === 127) {
    if (buffer.length < offset + 8) return null;
    length = Number(buffer.readBigUInt64BE(offset));
    offset += 8;
  }
  const mask = masked ? buffer.subarray(offset, offset + 4) : null;
  if (masked) offset += 4;
  if (buffer.length < offset + length) return null;
  let payload = buffer.subarray(offset, offset + length);
  if (mask) {
    payload = Buffer.from(payload.map((byte, index) => byte ^ mask[index % 4]));
  }
  return { opcode, payload, consumed: offset + length };
}

function readLayout() {
  try {
    return JSON.parse(fs.readFileSync(layoutPath, "utf8"));
  } catch {
    return { schema: "odin.interface_layout.v1", tiles: {} };
  }
}

function writeLayout(layout) {
  fs.writeFileSync(layoutPath, JSON.stringify(layout, null, 2), "utf8");
}

function defaultLayoutFor(entry, interfaces) {
  const index = Math.max(0, interfaces.findIndex((candidate) => candidate.providerId === entry.providerId));
  const intent = surfaceLayoutIntent(entry);
  const preferredHeight = positiveNumber(intent.preferredHeight, entry.providerId === "voidbot.swarm" ? 16 : 10);
  const minHeight = positiveNumber(intent.minHeight, 8);
  const preferredWidth = positiveNumber(intent.preferredWidth, 96);
  const minWidth = positiveNumber(intent.minWidth, 48);
  const priority = Number.isFinite(intent.priority) ? intent.priority : index;
  return {
    tileId: stableId(entry.providerId || "interface"),
    visible: true,
    priority,
    x: index % 2,
    y: Math.floor(index / 2),
    w: 1,
    h: Math.max(1, Math.ceil(preferredHeight / 12)),
    minWidth,
    minHeight,
    preferredWidth,
    preferredHeight,
    density: intent.density || "adaptive",
    viewportMode: intent.viewportMode || "adaptive",
  };
}

function mergeLayoutIntent(existing, entry, interfaces) {
  const base = existing && typeof existing === "object"
    ? { ...existing }
    : defaultLayoutFor(entry, interfaces);
  const intent = surfaceLayoutIntent(entry);
  const fallback = defaultLayoutFor(entry, interfaces);
  const minWidth = positiveNumber(intent.minWidth, positiveNumber(base.minWidth, fallback.minWidth));
  const minHeight = positiveNumber(intent.minHeight, positiveNumber(base.minHeight, fallback.minHeight));
  const preferredWidth = positiveNumber(intent.preferredWidth, positiveNumber(base.preferredWidth, fallback.preferredWidth));
  const preferredHeight = positiveNumber(intent.preferredHeight, positiveNumber(base.preferredHeight, fallback.preferredHeight));
  return {
    ...base,
    minWidth,
    minHeight,
    preferredWidth: Math.max(positiveNumber(base.preferredWidth, preferredWidth), preferredWidth),
    preferredHeight: Math.max(positiveNumber(base.preferredHeight, preferredHeight), preferredHeight),
    h: Math.max(positiveNumber(base.h, fallback.h), Math.ceil(preferredHeight / 12)),
    w: Math.max(positiveNumber(base.w, fallback.w), Math.ceil(preferredWidth / 96)),
    priority: Number.isFinite(intent.priority) ? Math.min(Number(base.priority ?? intent.priority), intent.priority) : base.priority,
    density: intent.density || base.density || fallback.density,
    viewportMode: intent.viewportMode || base.viewportMode || fallback.viewportMode,
  };
}

function surfaceLayoutIntent(entry) {
  const root = entry?.surface?.root;
  if (!root || typeof root !== "object") return {};
  const layout = root.layout && typeof root.layout === "object" ? root.layout : {};
  const props = root.props && typeof root.props === "object" ? root.props : {};
  const propLayout = props.layout && typeof props.layout === "object" ? props.layout : {};
  return { ...propLayout, ...layout };
}

function positiveNumber(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) && number > 0 ? number : fallback;
}

function applyClientCommand(command) {
  if (!command || command.type !== "odin-layout-intent" || !command.providerId) return;
  const layout = readLayout();
  layout.schema = "odin.interface_layout.v1";
  layout.updatedAt = new Date().toISOString();
  layout.tiles ||= {};
  const current = layout.tiles[command.providerId] || defaultLayoutFor({ providerId: command.providerId }, []);
  const next = { ...current };
  if (command.action === "focus") {
    next.visible = true;
    next.priority = -1;
  } else if (command.action === "resize") {
    next.w = clampNumber((next.w || 1) + Number(command.dw || 0), 1, 4);
    next.h = clampNumber((next.h || 1) + Number(command.dh || 0), 1, 4);
  } else if (command.action === "move") {
    next.x = clampNumber((next.x || 0) + Number(command.dx || 0), 0, 12);
    next.y = clampNumber((next.y || 0) + Number(command.dy || 0), 0, 12);
  } else if (command.action === "toggle") {
    next.visible = !next.visible;
  } else {
    return;
  }
  layout.tiles[command.providerId] = next;
  writeLayout(layout);
}

function clampNumber(value, min, max) {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(max, Math.round(value)));
}

function stableId(value) {
  return String(value).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
}

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) continue;
    const key = arg.slice(2);
    const next = argv[index + 1];
    if (!next || next.startsWith("--")) {
      parsed[key] = true;
    } else {
      parsed[key] = next;
      index += 1;
    }
  }
  return parsed;
}

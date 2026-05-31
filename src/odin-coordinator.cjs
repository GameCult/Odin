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

fs.mkdirSync(stateDir, { recursive: true });

const surfaceDefinition = defineDocumentType
  ? defineDocumentType({
      type: "gamecult.eve.surface_state",
      schemaName: "gamecult.eve.surface_state",
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

let meshNodePromise = null;
let version = 0;
let currentState = buildPendingState("Coordinator starting");
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
  const opcode = chunk[0] & 0x0f;
  if (opcode === 0x8) {
    clients.delete(socket);
    socket.end();
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
  const [docker, adb, hosts, yggdrasilServices] = await Promise.all([
    dockerSnapshot(),
    adbSnapshot(),
    hostChecks(),
    remoteServices("ygg", ["nginx", "streampixels-web", "streampixels-service", "heimdall", "repixelizer-gui", "bifrost"]),
  ]);

  const verses = [
    verse("starfire.local", "Starfire", "coordinator", "active", ["docker", "adb", "eve-provider"]),
    verse("nightwing.local", "Nightwing", "dashboard-renderer", hostState(hosts.Nightwing), ["eve-tui", "gpu-worker"]),
    verse("eve.ipad", "EVE", "ios-client", hostState(hosts.EVE), ["ssh", "native-eve"]),
    verse("periwinkle.android", "Periwinkle", "android-client", adb.devices.length ? "connected" : "waiting", ["adb", "sensor-edge"]),
    verse("raven.local", "Raven", "local-peer", hostState(hosts.Raven), ["ssh"]),
    verse("yggdrasil.ops", "Yggdrasil", "ops-host", hostState(hosts.Yggdrasil), ["ssh", "https", "services"]),
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
    surface: buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses }),
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

function buildSurface({ observedAt, docker, adb, hosts, yggdrasilServices, verses }) {
  return {
    schema: "gamecult.eve.surface.v1",
    id: "gamecult.network.status.surface",
    title: "Odin",
    root: {
      id: "network-root",
      kind: "stack",
      props: { title: "GameCult Network" },
      children: [
        pane("Coordinator", [
          text("observed", `observed ${observedAt}`),
          text("authority", "Odin owns Verse discovery, schema awareness, translation planning, and accepted surface publication. Renderers lower only."),
          metric("docker-count", "Docker containers", docker.containers.length, docker.state === "ok" ? "ok" : "warn"),
          metric("adb-count", "ADB devices", adb.devices.length, adb.devices.length ? "ok" : "warn"),
        ]),
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

function verse(verseId, name, role, status, capabilities) {
  return { verseId, name, role, status, capabilities };
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

function stableId(value) {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
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

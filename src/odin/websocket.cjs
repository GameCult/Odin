"use strict";

const crypto = require("crypto");
const http = require("http");
const net = require("net");

function createDashboardServer({ applyClientCommand, getCurrentState, getHealth, host, port }) {
  const clients = new Set();
  const server = http.createServer((req, res) => handleHttp(req, res, getCurrentState, getHealth));
  server.on("upgrade", (req, socket) => handleUpgrade(req, socket, clients, getCurrentState, applyClientCommand));
  server.listen(port, host, () => {
    console.log(`Odin coordinator listening on ws://${host}:${port}/eve/deck`);
  });
  return { clients, server };
}

function handleHttp(req, res, getCurrentState, getHealth) {
  const currentState = getCurrentState();
  if (req.url === "/health") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(getHealth()));
    return;
  }

  if (req.url === "/eve/deck/providers") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({
      marqueeText: String(currentState?.surface?.root?.props?.marqueeText || ""),
      providers: providerCatalogFromState(currentState),
    }));
    return;
  }

  const providerId = providerIdFromDeckUrl(req.url);
  if (providerId) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(stateForRequest(currentState, req.url)));
    return;
  }

  res.writeHead(404, { "content-type": "text/plain" });
  res.end("not found");
}

function providerCatalogFromState(state) {
  const providers = new Map();
  for (const provider of Array.isArray(state?.providerCatalog) ? state.providerCatalog : []) {
    if (provider?.id) {
      providers.set(String(provider.id), provider);
    }
  }
  const root = state?.surface?.root;
  const children = Array.isArray(root?.children) ? root.children : [];
  for (const child of children
    .filter((child) => child?.kind === "interface" && child.props?.providerId)
  ) {
      const props = child.props || {};
      const embeddedRoot = Array.isArray(child.children) ? child.children[0] : null;
      const providerId = String(props.providerId);
      providers.set(providerId, {
        id: providerId,
        title: String(props.title || props.providerId),
        description: String(props.detail || "Provider-owned Eve/CultUI interface discovered by Odin."),
        version: String(props.version || 0),
        endpoint: `/eve/deck/${encodeURIComponent(providerId)}`,
        canonicalService: props.canonicalService || null,
        locatedService: props.locatedService || null,
        cultMeshAddress: props.cultMeshAddress || null,
        endpoints: Array.isArray(props.endpoints) ? props.endpoints : [],
        routes: Array.isArray(props.routes) ? props.routes : [],
        transportEndpoint: `/eve/deck/${encodeURIComponent(providerId)}`,
        capabilities: providerCapabilities(embeddedRoot),
        usesCultMesh: String(props.source || "").startsWith("cultmesh:"),
        transport: String(props.source || "").startsWith("cultmesh:")
          ? "Odin provider proxy from CultMesh Eve interface binding"
          : "Odin provider proxy from Eve WebSocket provider",
        status: props.status || "unknown",
        updatedAt: props.updatedAt || state.updatedAt,
        source: props.source || "",
      });
  }
  return [...providers.values()].sort((left, right) => String(left.id).localeCompare(String(right.id)));
}

function transportEndpoint(props) {
  const routes = Array.isArray(props.routes) ? props.routes : [];
  return routes.find((route) => route?.transport === "cultnet")?.address
    || routes.find((route) => route?.transport === "compatibility-eve-deck")?.address
    || props.source
    || "";
}

function providerCapabilities(root) {
  const capabilities = new Set(["cultui-surface"]);
  if (hasCommand(root)) {
    capabilities.add("command-surface");
  }
  return [...capabilities];
}

function hasCommand(node) {
  if (!node || typeof node !== "object") {
    return false;
  }
  const props = node.props || {};
  if (props.command || props.action?.command || props.action?.type) {
    return true;
  }
  return Array.isArray(node.children) && node.children.some(hasCommand);
}

function handleUpgrade(req, socket, clients, getCurrentState, applyClientCommand) {
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
  socket.odinRequestUrl = req.url;
  clients.add(socket);
  sendFrame(socket, 0x1, Buffer.from(JSON.stringify(stateForRequest(getCurrentState(), req.url)), "utf8"));
  socket.on("data", (chunk) => handleClientFrame(socket, chunk, clients, applyClientCommand));
  socket.on("close", () => clients.delete(socket));
  socket.on("error", () => clients.delete(socket));
}

function stateForRequest(currentState, requestUrl) {
  const providerId = providerIdFromDeckUrl(requestUrl);
  if (!providerId) {
    return odinCatalogState(currentState);
  }
  const providerState = providerStateFromCurrentState(currentState, providerId);
  if (providerState) {
    return providerState;
  }
  const provider = providerCatalogFromState(currentState).find((entry) => entry.id === providerId);
  if (provider) {
    return unavailableProviderState(
      providerId,
      "provider advertisement is live, but no provider-owned Eve surface is published yet",
      provider.title,
    );
  }
  return unavailableProviderState(providerId, "provider not present in Odin catalog");
}

function providerIdFromDeckUrl(requestUrl) {
  const path = String(requestUrl || "").split("?")[0].replace(/\/+$/, "");
  if (path === "/eve/deck") {
    return "";
  }
  const prefix = "/eve/deck/";
  if (!path.startsWith(prefix)) {
    return "";
  }
  return decodeURIComponent(path.slice(prefix.length));
}

function odinCatalogState(currentState) {
  const providers = providerCatalogFromState(currentState);
  return {
    type: "dashboard-state",
    schema: "gamecult.odin.provider_catalog.v1",
    providerId: "odin.providers",
    title: "Odin Provider Catalog",
    version: currentState.version,
    updatedAt: currentState.updatedAt,
    providerCatalog: providers,
    surface: {
      schema: "gamecult.eve.surface.v1",
      id: "odin.provider-catalog.surface",
      title: "Odin Provider Catalog",
      root: {
        id: "odin-provider-catalog-root",
        kind: "dashboard",
        props: {
          title: "Odin Providers",
          summary: `${providers.length} provider surfaces discovered`,
          marqueeText: String(currentState?.surface?.root?.props?.marqueeText || ""),
        },
        children: providers.map((provider) => ({
          id: `provider-${stableSurfaceId(provider.id)}`,
          kind: "text",
          props: {
            text: `${provider.id} ${provider.status || "unknown"} ${provider.title || ""}`.trim(),
            providerId: provider.id,
          },
          children: [],
        })),
      },
      assets: [],
    },
  };
}

function providerStateFromCurrentState(currentState, providerId) {
  const child = providerInterfaceChild(currentState, providerId);
  if (!child) {
    return null;
  }
  const props = child.props || {};
  const root = Array.isArray(child.children) ? child.children[0] : null;
  if (!root) {
    return null;
  }
  return {
    type: "dashboard-state",
    schema: "gamecult.eve.provider_surface_proxy.v1",
    providerId,
    title: props.title || providerId,
    version: props.version || currentState.version,
    updatedAt: props.updatedAt || currentState.updatedAt,
    surface: {
      schema: "gamecult.eve.surface.v1",
      id: `proxied-${providerId}`,
      title: props.title || providerId,
      root,
      assets: [],
    },
  };
}

function providerInterfaceChild(currentState, providerId) {
  const root = currentState?.surface?.root;
  const children = Array.isArray(root?.children) ? root.children : [];
  return children.find((child) => child?.kind === "interface" && String(child.props?.providerId || "") === providerId);
}

function unavailableProviderState(providerId, detail, title = null) {
  return {
    type: "dashboard-state",
    schema: "gamecult.eve.provider_surface_proxy.v1",
    providerId,
    title: title || providerId,
    version: 0,
    updatedAt: new Date().toISOString(),
    surface: {
      schema: "gamecult.eve.surface.v1",
      id: `unavailable-${stableSurfaceId(providerId)}`,
      title: title || providerId,
      root: {
        id: `unavailable-${stableSurfaceId(providerId)}-root`,
        kind: "pane",
        props: {
          title: title || providerId,
          status: "unavailable",
        },
        children: [{
          id: `unavailable-${stableSurfaceId(providerId)}-detail`,
          kind: "text",
          props: { text: detail },
          children: [],
        }],
      },
      assets: [],
    },
  };
}

function stableSurfaceId(value) {
  return String(value || "provider")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "provider";
}

function handleClientFrame(socket, chunk, clients, applyClientCommand) {
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

function broadcastState(clients, state) {
  for (const client of [...clients]) {
    try {
      const payload = Buffer.from(JSON.stringify(stateForRequest(state, client.odinRequestUrl || "/eve/deck")), "utf8");
      sendFrame(client, 0x1, payload);
    } catch {
      clients.delete(client);
      client.destroy();
    }
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
      socket.odinPendingData = buffer.subarray(marker + 4);
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
    let buffer = socket.odinPendingData || Buffer.alloc(0);
    socket.odinPendingData = null;
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
    const frame = tryReadFrame(buffer);
    if (frame) {
      buffer = buffer.subarray(frame.consumed);
      socket.odinPendingData = buffer.length ? buffer : null;
      if (frame.opcode === 0x1) cleanup(null, frame.payload.toString("utf8"));
      if (frame.opcode === 0x8) cleanup(new Error("dashboard websocket closed"));
    }
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

module.exports = {
  broadcastState,
  createDashboardServer,
  openWebSocket,
  readServerTextFrame,
  sendClientFrame,
  sendFrame,
  tryReadFrame,
};

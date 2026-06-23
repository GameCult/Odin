"use strict";

const dgram = require("dgram");
const path = require("path");
const { createRequire } = require("module");

const requireCultNet = createRequire(path.resolve(
  __dirname,
  "..",
  "..",
  "..",
  "CultLib",
  "packages",
  "cultnet-ts",
  "package.json",
));

const {
  CultNetDocumentRegistry,
  CultNetRudpSession,
  cultNetBuiltinSchemaRegistry,
  decodeRudpPacket,
  defineCultNetDocumentBinding,
  encodeCultNetMessageForWire,
  encodeRudpPacket,
  parseCultNetMessage,
} = requireCultNet("./dist/index.js");
const { decode, encode } = requireCultNet("@msgpack/msgpack");

const ODIN_RUDP_CONNECTION_ID = 0x0d1d0002;
const DEFAULT_SESSION_TIMEOUT_MS = 30_000;

function createCultNetRudpSurfaceServer(options) {
  if (!options?.bind) return null;
  if (!options.documents?.surfaceDefinition) {
    throw new Error("Odin CultNet/RUDP surface server requires a surface document definition.");
  }
  if (typeof options.getCache !== "function") {
    throw new Error("Odin CultNet/RUDP surface server requires an async getCache function.");
  }

  const bind = parseEndpoint(options.bind);
  const documentRegistry = new CultNetDocumentRegistry([
    defineCultNetDocumentBinding({ definition: options.documents.surfaceDefinition }),
  ]);
  const sessions = new Map();
  const socket = dgram.createSocket(bind.host.includes(":") ? "udp6" : "udp4");
  const resendPollMs = Math.max(10, Number(options.resendPollMs || 25));
  const sessionTimeoutMs = Math.max(1_000, Number(options.sessionTimeoutMs || DEFAULT_SESSION_TIMEOUT_MS));
  let resendTimer = null;

  socket.on("message", (wire, remote) => {
    try {
      const packet = decodeRudpPacket(wire);
      handlePacket(packet, remote).catch((error) => {
        logError(options, `CultNet/RUDP packet handling failed for ${remote.address}:${remote.port}`, error);
      });
    } catch (error) {
      logError(options, "CultNet/RUDP packet decode failed", error);
    }
  });

  socket.on("error", (error) => {
    logError(options, "CultNet/RUDP socket error", error);
  });

  async function start() {
    await new Promise((resolve, reject) => {
      socket.once("error", reject);
      socket.bind(bind.port, bind.host, () => {
        socket.off("error", reject);
        resolve();
      });
    });
    resendTimer = setInterval(() => {
      try {
        const nowMs = Date.now();
        for (const [key, record] of sessions) {
          if (record.session.checkTimeout(nowMs, sessionTimeoutMs)) {
            sessions.delete(key);
            continue;
          }
          for (const packet of record.session.dueResends(nowMs)) {
            sendPacket(record.remote, packet);
          }
        }
      } catch (error) {
        logError(options, "CultNet/RUDP resend loop failed", error);
      }
    }, resendPollMs);
    resendTimer.unref?.();
  }

  function close() {
    if (resendTimer) {
      clearInterval(resendTimer);
      resendTimer = null;
    }
    sessions.clear();
    socket.close();
  }

  async function handlePacket(packet, remote) {
    const key = remoteKey(remote);
    const nowMs = Date.now();
    let record = sessions.get(key);

    if (packet.packetType === "connect") {
      record = {
        remote: { address: remote.address, family: remote.family, port: remote.port },
        session: new CultNetRudpSession({
          connectionId: ODIN_RUDP_CONNECTION_ID,
          initialSequence: 1,
          resendDelayMs: 100,
        }),
      };
      sessions.set(key, record);
      sendPacket(record.remote, record.session.acceptConnect(packet, nowMs, new Uint8Array()));
      return;
    }

    if (!record) {
      return;
    }

    const result = record.session.receive(packet, nowMs);
    if (result.reply) {
      sendPacket(record.remote, result.reply);
    }
    for (const frame of result.delivered) {
      if (frame.channelId !== "schema") {
        continue;
      }
      await handleSchemaFrame(record, frame.payload);
    }
    if (result.disconnected) {
      sessions.delete(key);
      return;
    }
    if (packet.packetType === "accept" || result.delivered.length > 0) {
      sendPacket(record.remote, record.session.createAck());
    }
  }

  async function handleSchemaFrame(record, payload) {
    const message = parseCultNetMessage(decode(payload), "cultnet.schema.v0");
    switch (message.schemaVersion) {
      case "cultnet.snapshot_request.v0": {
        try {
          const cache = await options.getCache();
          const response = documentRegistry.createRawSnapshotResponse(cache, message.messageId, message);
          sendSchemaMessage(record, response);
        } catch (error) {
          sendSchemaMessage(record, {
            schemaVersion: "cultnet.error.v0",
            error: error.message,
          });
        }
        return;
      }
      case "cultnet.schema_catalog_request.v0":
        sendSchemaMessage(record, cultNetBuiltinSchemaRegistry.createCatalogResponse(message));
        return;
      case "cultnet.document_put_raw.v0": {
        if (typeof options.onDocumentPutRaw === "function") {
          try {
            options.onDocumentPutRaw(normalizeRawDocumentPut(message, record.remote));
          } catch (error) {
            logError(options, "CultNet/RUDP raw document ingest failed", error);
          }
        }
        return;
      }
      default:
        sendSchemaMessage(record, {
          schemaVersion: "cultnet.error.v0",
          error: `Unsupported Odin CultNet/RUDP request ${message.schemaVersion}.`,
        });
    }
  }

  function sendSchemaMessage(record, message) {
    const payload = encode(encodeCultNetMessageForWire(message, "cultnet.schema.v0"));
    for (const packet of record.session.sendMany("schema", payload, {
      reliable: true,
      ordered: true,
      nowMs: Date.now(),
    })) {
      sendPacket(record.remote, packet);
    }
  }

  function sendPacket(remote, packet) {
    const wire = Buffer.from(encodeRudpPacket(packet));
    socket.send(wire, remote.port, remote.address);
  }

  return {
    bind,
    start,
    close,
  };
}

function normalizeRawDocumentPut(message, remote) {
  const document = message?.document;
  if (!document?.schemaId || !document?.recordKey) {
    throw new Error("raw document put is missing schemaId or recordKey");
  }
  if (document.payloadEncoding !== "messagepack") {
    throw new Error(`unsupported raw payload encoding ${document.payloadEncoding}`);
  }
  return {
    schemaId: document.schemaId,
    recordKey: document.recordKey,
    storedAt: document.storedAt || new Date().toISOString(),
    payload: decode(document.payload),
    sourceRuntimeId: document.sourceRuntimeId || null,
    sourceRole: document.sourceRole || null,
    tags: Array.isArray(document.tags) ? document.tags : [],
    remote,
  };
}

function parseEndpoint(value) {
  const text = String(value || "").trim();
  const ipv6 = text.match(/^\[([^\]]+)\]:(\d+)$/);
  if (ipv6) return { host: ipv6[1], port: parsePort(ipv6[2]) };
  const index = text.lastIndexOf(":");
  if (index <= 0) {
    throw new Error(`CultNet/RUDP bind must be host:port, got "${value}".`);
  }
  return { host: text.slice(0, index), port: parsePort(text.slice(index + 1)) };
}

function parsePort(value) {
  const port = Number(value);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`CultNet/RUDP port is invalid: ${value}`);
  }
  return port;
}

function remoteKey(remote) {
  return `${remote.address}:${remote.port}`;
}

function logError(options, label, error) {
  const message = error instanceof Error ? error.message : String(error);
  if (typeof options?.logError === "function") {
    options.logError(`${label}: ${message}`);
    return;
  }
  console.error(`${label}: ${message}`);
}

module.exports = {
  createCultNetRudpSurfaceServer,
};

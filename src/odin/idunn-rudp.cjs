"use strict";

const dgram = require("dgram");
const crypto = require("crypto");
const fs = require("fs");
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
  CultNetRudpSession,
  decodeRudpPacket,
  encodeCultNetMessageForWire,
  encodeRudpPacket,
} = requireCultNet("./dist/index.js");

const { encode } = requireCultNet("@msgpack/msgpack");

const CULTNET_RUDP_PROTOCOL_ID = "cultnet.transport.rudp.v0";
const IDUNN_HEALTH_RUDP_CONNECTION_ID = 0x1d0d0001;
const SIGNED_HEALTH_SCHEMA = "idunn.signed_daemon_health.v1";
const SIGNATURE_DOMAIN = Buffer.from("gamecult.provider-health.signature.v1\0", "utf8");
const ID_DOMAIN = Buffer.from("gamecult.provider-health.identity.v1\0", "utf8");
const SIGNING_PURPOSE = Buffer.from(SIGNED_HEALTH_SCHEMA, "utf8");

function createIdunnRudpHealthPublisher(options) {
  if (!options) return null;
  const endpoint = parseEndpoint(options.endpoint);
  const publisher = {
    daemonId: options.daemonId,
    endpoint,
    healthContract: options.healthContract,
    publisherIncarnationId: crypto.randomUUID(),
    publisherSequence: 0,
    sourceRuntimeId: options.sourceRuntimeId || "odin-coordinator",
  };
  if (options.privateKeyPath) {
    publisher.privateKey = crypto.createPrivateKey(fs.readFileSync(options.privateKeyPath));
    publisher.publicKey = rawEd25519PublicKey(publisher.privateKey);
    publisher.signerIdentityId = crypto
      .createHash("sha256")
      .update(ID_DOMAIN)
      .update(publisher.publicKey)
      .digest("hex");
  }
  return publisher;
}

async function publishIdunnRudpHealth(publisher, health) {
  if (!publisher) return;

  const socket = dgram.createSocket(endpointFamily(publisher.endpoint.host));
  await bindSocket(socket, publisher.endpoint);
  const receiver = createPacketReceiver(socket);
  const session = new CultNetRudpSession({
    connectionId: IDUNN_HEALTH_RUDP_CONNECTION_ID,
    initialSequence: 1,
    resendDelayMs: 100,
  });
  try {
    const connect = session.createConnect(Date.now(), new Uint8Array());
    await sendPacket(socket, publisher.endpoint, connect);
    await receiveUntil(
      receiver,
      session,
      publisher.endpoint,
      (packet) => packet.packetType === "accept",
      5000,
      "accept",
    );

    const observedAt = health.observedAt || new Date().toISOString();
    const signed = publisher.privateKey
      ? signedHealthPayload(publisher, health, observedAt)
      : null;
    const payload = signed?.payload || encode([
      publisher.daemonId,
      health.state,
      String(health.detail || "").slice(0, 512),
      observedAt,
      publisher.healthContract,
      "daemon-published",
      CULTNET_RUDP_PROTOCOL_ID,
    ]);
    const message = {
      schemaVersion: "cultnet.document_put_raw.v0",
      messageId: `odin-health:${publisher.daemonId}:${observedAt.replace(/[:.]/g, "-")}`,
      document: {
        schemaId: signed ? SIGNED_HEALTH_SCHEMA : "idunn.daemon_health",
        recordKey: publisher.daemonId,
        storedAt: observedAt,
        payloadEncoding: "messagepack",
        payload,
        sourceRuntimeId: publisher.sourceRuntimeId,
        sourceRole: "daemon-health-publisher",
        tags: [CULTNET_RUDP_PROTOCOL_ID],
      },
    };
    const wirePayload = encode(encodeCultNetMessageForWire(message, "cultnet.schema.v0"));
    const dataPackets = session.sendMany("schema", wirePayload, {
      reliable: true,
      ordered: true,
      nowMs: Date.now(),
    });
    const ack = receiveUntil(
      receiver,
      session,
      publisher.endpoint,
      (packet) => packet.packetType === "ack",
      500,
      "ack",
    ).catch(() => undefined);
    for (const packet of dataPackets) {
      await sendPacket(socket, publisher.endpoint, packet);
    }
    await ack;
  } finally {
    receiver.close();
    socket.close();
  }
}

function signedHealthPayload(publisher, health, observedAt) {
  publisher.publisherSequence += 1;
  const observedAtUnixMillis = Date.parse(observedAt);
  if (!Number.isSafeInteger(observedAtUnixMillis) || observedAtUnixMillis <= 0) {
    throw new Error("Idunn signed health observation time is invalid.");
  }
  const unsigned = [
    SIGNED_HEALTH_SCHEMA,
    publisher.daemonId,
    publisher.healthContract,
    publisher.sourceRuntimeId,
    health.state,
    String(health.detail || "").slice(0, 512),
    publisher.signerIdentityId,
    publisher.publisherIncarnationId,
    publisher.publisherSequence,
    observedAtUnixMillis,
    null,
    null,
    null,
    null,
    "ed25519",
    new Uint8Array(),
    false,
  ];
  const unsignedPayload = encode(unsigned);
  const signature = crypto.sign(null, signingMessage(unsignedPayload), publisher.privateKey);
  const statement = unsigned.slice();
  statement[15] = new Uint8Array(signature);
  return { payload: encode(statement), statement, unsignedPayload };
}

function signingMessage(payload) {
  const purposeLength = Buffer.alloc(8);
  purposeLength.writeBigUInt64BE(BigInt(SIGNING_PURPOSE.length));
  const payloadLength = Buffer.alloc(8);
  payloadLength.writeBigUInt64BE(BigInt(payload.length));
  return Buffer.concat([
    SIGNATURE_DOMAIN,
    purposeLength,
    SIGNING_PURPOSE,
    payloadLength,
    Buffer.from(payload),
  ]);
}

function rawEd25519PublicKey(privateKey) {
  const der = crypto.createPublicKey(privateKey).export({ type: "spki", format: "der" });
  if (der.length < 32) throw new Error("Ed25519 public key export is too short.");
  return Buffer.from(der.subarray(der.length - 32));
}

async function bindSocket(socket, endpoint) {
  await new Promise((resolve, reject) => {
    socket.once("error", reject);
    socket.bind(0, endpoint.host.includes(":") ? "::" : "0.0.0.0", () => {
      socket.off("error", reject);
      resolve();
    });
  });
}

function parseEndpoint(value) {
  const text = String(value || "").trim();
  const ipv6 = text.match(/^\[([^\]]+)\]:(\d+)$/);
  if (ipv6) return { host: ipv6[1], port: parsePort(ipv6[2]) };
  const index = text.lastIndexOf(":");
  if (index <= 0) {
    throw new Error(`Idunn RUDP endpoint must be host:port, got "${value}".`);
  }
  return { host: text.slice(0, index), port: parsePort(text.slice(index + 1)) };
}

function parsePort(value) {
  const port = Number(value);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`Idunn RUDP endpoint port is invalid: ${value}`);
  }
  return port;
}

function endpointFamily(host) {
  return host.includes(":") ? "udp6" : "udp4";
}

async function receiveUntil(receiver, session, endpoint, predicate, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const packet = await receiver.next(Math.min(100, deadline - Date.now()), label);
      const result = session.receive(packet, Date.now());
      if (result.reply) {
        throw new Error("Odin RUDP health publisher received an unexpected reply-required packet.");
      }
      if (predicate(packet)) {
        return;
      }
    } catch (error) {
      if (error.code !== "ETIMEDOUT") {
        throw error;
      }
    }
    for (const packet of session.dueResends(Date.now())) {
      await sendPacket(receiver.socket, endpoint, packet);
    }
  }
  throw new Error(`timed out waiting for Idunn RUDP ${label} response after ${timeoutMs}ms`);
}

function createPacketReceiver(socket) {
  const packets = [];
  const waiters = [];
  const errors = [];

  const resolveNext = () => {
    while (waiters.length > 0 && (packets.length > 0 || errors.length > 0)) {
      const waiter = waiters.shift();
      clearTimeout(waiter.timer);
      if (errors.length > 0) {
        waiter.reject(errors.shift());
      } else {
        waiter.resolve(packets.shift());
      }
    }
  };
  const onMessage = (wire) => {
    try {
      packets.push(decodeRudpPacket(wire));
    } catch (error) {
      errors.push(error);
    }
    resolveNext();
  };
  const onError = (error) => {
    errors.push(error);
    resolveNext();
  };

  socket.on("message", onMessage);
  socket.on("error", onError);

  return {
    socket,
    next(timeoutMs, label = "packet") {
      if (packets.length > 0) return Promise.resolve(packets.shift());
      if (errors.length > 0) return Promise.reject(errors.shift());
      return new Promise((resolve, reject) => {
        const waiter = {
          resolve,
          reject,
          timer: setTimeout(() => {
            const index = waiters.indexOf(waiter);
            if (index >= 0) waiters.splice(index, 1);
            const error = new Error(`timed out waiting for Idunn RUDP ${label}`);
            error.code = "ETIMEDOUT";
            reject(error);
          }, Math.max(1, timeoutMs)),
        };
        waiters.push(waiter);
      });
    },
    close() {
      socket.off("message", onMessage);
      socket.off("error", onError);
      while (waiters.length > 0) {
        const waiter = waiters.shift();
        clearTimeout(waiter.timer);
        const error = new Error("Odin RUDP health publisher closed.");
        error.code = "ECLOSED";
        waiter.reject(error);
      }
    },
  };
}

async function sendPacket(socket, endpoint, packet) {
  const wire = encodeRudpPacket(packet);
  await new Promise((resolve, reject) => {
    socket.send(wire, endpoint.port, endpoint.host, (error) => {
      if (error) reject(error);
      else resolve();
    });
  });
}

module.exports = {
  CULTNET_RUDP_PROTOCOL_ID,
  createIdunnRudpHealthPublisher,
  publishIdunnRudpHealth,
  signedHealthPayload,
  signingMessage,
};

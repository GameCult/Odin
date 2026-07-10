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
  CultNetRudpSession,
  decodeRudpPacket,
  encodeCultNetMessageForWire,
  encodeRudpPacket,
} = requireCultNet("./dist/index.js");

const { encode } = requireCultNet("@msgpack/msgpack");

const CULTNET_RUDP_PROTOCOL_ID = "cultnet.transport.rudp.v0";
const IDUNN_HEALTH_RUDP_CONNECTION_ID = 0x1d0d0001;

function createIdunnRudpHealthPublisher(options) {
  if (!options) return null;
  const endpoint = parseEndpoint(options.endpoint);
  return {
    daemonId: options.daemonId,
    endpoint,
    healthContract: options.healthContract,
  };
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
    const record = {
      daemonId: publisher.daemonId,
      state: health.state,
      detail: health.detail,
      observedAt,
      healthContract: publisher.healthContract,
      publicationSource: "daemon-published",
      transport: CULTNET_RUDP_PROTOCOL_ID,
    };
    const payload = encode([
      record.daemonId,
      record.state,
      record.detail,
      record.observedAt,
      record.healthContract,
      record.publicationSource,
      record.transport,
    ]);
    const message = {
      schemaVersion: "cultnet.document_put_raw.v0",
      messageId: `odin-health:${publisher.daemonId}:${observedAt.replace(/[:.]/g, "-")}`,
      document: {
        schemaId: "idunn.daemon_health",
        recordKey: publisher.daemonId,
        storedAt: observedAt,
        payloadEncoding: "messagepack",
        payload,
        sourceRuntimeId: "odin-coordinator",
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
};

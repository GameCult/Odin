"use strict";

// Idunn health publication, as Odin consumes it.
//
// The transport used to live here: 300-odd lines of socket handling, RUDP
// handshaking, Ed25519 signing, and packet receipt, of which almost nothing was
// about Idunn. That is generic CultNet client work and now lives in CultLib as
// cultnet-ts's signed-daemon-health module.
//
// What remains is what Odin actually needs to know: Idunn's wire contract, and
// the shape of the calls its consumers already make. The constants are defined
// by Idunn in docs/signed-daemon-health-authority.md. They are declared here,
// not decided here; if they change, that document changes first.

const path = require("path");
const { createRequire } = require("module");

const cultLibRoot = process.env.CULTLIB_ROOT
  ? path.resolve(process.env.CULTLIB_ROOT)
  : path.resolve(__dirname, "..", "..", "..", "CultLib");

const requireCultNet = createRequire(path.resolve(cultLibRoot, "packages", "cultnet-ts", "package.json"));

const {
  CULTNET_RUDP_PROTOCOL_ID,
  createSignedDaemonHealthPublisher,
  publishSignedDaemonHealth,
  signedDaemonHealthPayload,
  signedDaemonHealthSigningMessage,
} = requireCultNet("./dist/index.js");

// Idunn's contract. Owner: GameCult/Idunn, docs/signed-daemon-health-authority.md.
const IDUNN_HEALTH_CONTRACT = Object.freeze({
  connectionId: 0x1d0d0001,
  signedSchemaId: "idunn.signed_daemon_health.v1",
  unsignedSchemaId: "idunn.daemon_health",
  messageIdPrefix: "odin-health",
});

function createIdunnRudpHealthPublisher(options) {
  if (!options) return null;
  return createSignedDaemonHealthPublisher({
    sourceRuntimeId: "odin-coordinator",
    ...options,
    contract: IDUNN_HEALTH_CONTRACT,
  });
}

const publishIdunnRudpHealth = publishSignedDaemonHealth;
const signedHealthPayload = signedDaemonHealthPayload;

// The signing purpose is Idunn's signed schema id, so a statement signed for
// Idunn cannot verify as another service's health.
function signingMessage(payload) {
  return signedDaemonHealthSigningMessage(IDUNN_HEALTH_CONTRACT.signedSchemaId, payload);
}

module.exports = {
  CULTNET_RUDP_PROTOCOL_ID,
  IDUNN_HEALTH_CONTRACT,
  createIdunnRudpHealthPublisher,
  publishIdunnRudpHealth,
  signedHealthPayload,
  signingMessage,
};

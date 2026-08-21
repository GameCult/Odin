"use strict";

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const test = require("node:test");

const {
  signedHealthPayload,
  signingMessage,
} = require("../src/odin/idunn-rudp.cjs");

test("Odin signs the complete canonical provider-health statement", () => {
  const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
  const publicDer = publicKey.export({ type: "spki", format: "der" });
  const rawPublicKey = Buffer.from(publicDer.subarray(publicDer.length - 32));
  const signerIdentityId = crypto
    .createHash("sha256")
    .update(Buffer.from("gamecult.provider-health.identity.v1\0"))
    .update(rawPublicKey)
    .digest("hex");
  const publisher = {
    daemonId: "yggdrasil-odin",
    healthContract: "odin.cultnet-rudp-provider-health",
    privateKey,
    publisherIncarnationId: "24e0dc7f-6a92-4aa8-9f56-fb513d265e13",
    publisherSequence: 0,
    signerIdentityId,
    sourceRuntimeId: "odin-yggdrasil",
  };

  const signed = signedHealthPayload(
    publisher,
    { state: "active", detail: "catalog current" },
    "2026-08-21T12:34:56.789Z",
  );

  assert.equal(signed.statement[0], "idunn.signed_daemon_health.v1");
  assert.equal(signed.statement[1], "yggdrasil-odin");
  assert.equal(signed.statement[2], "odin.cultnet-rudp-provider-health");
  assert.equal(signed.statement[3], "odin-yggdrasil");
  assert.equal(signed.statement[8], 1);
  assert.equal(signed.statement[9], 1787315696789);
  assert.equal(signed.statement[15].byteLength, 64);
  assert.equal(
    crypto.verify(
      null,
      signingMessage(signed.unsignedPayload),
      publicKey,
      Buffer.from(signed.statement[15]),
    ),
    true,
  );

  const tampered = Buffer.from(signed.unsignedPayload);
  tampered[tampered.length - 1] ^= 1;
  assert.equal(
    crypto.verify(
      null,
      signingMessage(tampered),
      publicKey,
      Buffer.from(signed.statement[15]),
    ),
    false,
  );
});

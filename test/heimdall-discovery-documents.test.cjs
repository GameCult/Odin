"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { defineOdinDocuments } = require("../src/odin/documents.cjs");

test("Odin registers the typed Heimdall access discovery documents", () => {
  const documents = defineOdinDocuments((definition) => definition);

  assert.equal(
    documents.evePluginAdvertisementDefinition.schemaId,
    "gamecult.eve.plugin_advertisement.v1",
  );
  assert.equal(
    documents.heimdallCommandBoundaryDefinition.schemaId,
    "heimdall.command_boundary.v1",
  );
  assert.equal(
    documents.heimdallTransportProfileDefinition.schemaId,
    "heimdall.transport_profile.v1",
  );
});

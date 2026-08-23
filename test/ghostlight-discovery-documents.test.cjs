"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");

const { defineOdinDocuments } = require("../src/odin/documents.cjs");

test("Odin persists Ghostlight's public schema catalog envelope", () => {
  const documents = defineOdinDocuments((definition) => definition);
  const catalog = documents.ghostlightSchemaCatalogDefinition;

  assert.equal(catalog.type, "ghostlight.schema_catalog");
  assert.equal(catalog.schemaId, "ghostlight.schema_catalog.v1");
  assert.equal(catalog.schemaVersion, "ghostlight.schema_catalog.v1");
  assert.equal(catalog.name({ providerId: "gamecult.ghostlight.dungeon" }), "gamecult.ghostlight.dungeon");
});

test("Odin's unavailable-runtime projection includes the Ghostlight catalog slot", () => {
  const documents = defineOdinDocuments(null);
  assert.equal(documents.ghostlightSchemaCatalogDefinition, null);
});

"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const { loadCultRuntime } = require("../src/odin/config.cjs");
const {
  defineOdinDocuments,
  documentDefinitionForSchema,
} = require("../src/odin/documents.cjs");

test("Odin persists accepted model-atlas and Eve documents without synthesizing edges", async (t) => {
  const { CultMesh, defineDocumentType, error } = loadCultRuntime();
  assert.ifError(error);
  const documents = defineOdinDocuments(defineDocumentType);
  const definitions = Object.values(documents).filter(Boolean);
  const stateDir = fs.mkdtempSync(path.join(os.tmpdir(), "odin-model-atlas-"));
  t.after(() => fs.rmSync(stateDir, { recursive: true, force: true }));

  const atlasDefinition = documentDefinitionForSchema(
    documents,
    "gamecult.model.atlas_publication.v0",
  );
  const entanglementDefinition = documentDefinitionForSchema(
    documents,
    "gamecult.model.entanglement_projection.v0",
  );
  const surfaceDefinition = documentDefinitionForSchema(
    documents,
    "gamecult.eve.surface.v1",
  );
  assert.ok(atlasDefinition);
  assert.ok(entanglementDefinition);
  assert.ok(surfaceDefinition);
  assert.equal(documentDefinitionForSchema(documents, "gamecult.model.unknown.v0"), null);

  const node = await CultMesh.createNode(path.join(stateDir, "odin.ccmp"), {
    documents: definitions,
  });
  const atlas = {
    publicationId: "aetheria-atlas",
    repositoryId: "GameCult/Aetheria",
    nodes: [{ id: "aetheria.runtime" }],
  };
  const entanglement = {
    projectionId: "gamecult-entanglement",
    sourcePublicationIds: ["aetheria-atlas"],
    edges: [],
  };
  const surface = {
    schema: "gamecult.eve.surface.v1",
    id: "gamecult.model.atlas.surface",
    root: { id: "root", kind: "dashboard", children: [] },
  };

  await node.put(atlasDefinition, "atlas:aetheria", atlas);
  await node.put(entanglementDefinition, "entanglement:gamecult", entanglement);
  await node.put(surfaceDefinition, "surface:model-atlas", surface);
  await node.flush(true);

  assert.deepEqual(node.getRequired(atlasDefinition, "atlas:aetheria"), atlas);
  assert.equal(node.getRequired(atlasDefinition, "atlas:aetheria").edges, undefined);
  assert.deepEqual(
    node.getRequired(entanglementDefinition, "entanglement:gamecult").edges,
    [],
  );
  assert.deepEqual(node.getRequired(surfaceDefinition, "surface:model-atlas"), surface);
});

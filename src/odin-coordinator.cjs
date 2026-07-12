#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const { buildConfig, loadCultRuntime } = require("./odin/config.cjs");
const { createCultMeshRudpDocumentServer } = require("./odin/cultnet-rudp.cjs");
const { defineOdinDocuments } = require("./odin/documents.cjs");
const { createInterfaceDiscovery } = require("./odin/interfaces.cjs");
const { createIdunnRudpHealthPublisher, publishIdunnRudpHealth } = require("./odin/idunn-rudp.cjs");
const { createLayoutStore } = require("./odin/layout.cjs");
const { createLiveProviderRegistry } = require("./odin/provider-ingress.cjs");
const { createStateBuilder } = require("./odin/state.cjs");

const config = buildConfig(process.argv.slice(2));
fs.mkdirSync(config.stateDir, { recursive: true });
const idunnRudpHealthPublisher = createIdunnRudpHealthPublisher(config.idunnRudpHealth);

const cultRuntime = loadCultRuntime();
if (cultRuntime.error) {
  console.error("CultMesh runtime unavailable; durable mesh catalog disabled:", cultRuntime.error.message);
}

const documents = defineOdinDocuments(cultRuntime.defineDocumentType);
const allDocumentDefinitions = Object.values(documents).filter(Boolean);
const layoutStore = createLayoutStore({
  definition: documents.interfaceLayoutDefinition,
  getNode: async () => meshNodePromise ? await meshNodePromise : null,
  layoutPath: config.layoutPath,
});
const liveProviderRegistry = createLiveProviderRegistry();
const interfaceDiscovery = createInterfaceDiscovery({
  CultMesh: cultRuntime.CultMesh,
  documents,
  interfaceBindingStores: config.interfaceBindingStores,
  liveProviderRegistry,
});
const stateBuilder = createStateBuilder({
  cachePath: config.cachePath,
  gamecultTextDocumentStorePath: config.gamecultTextDocumentStorePath,
  interfaceDiscovery,
  layoutStore,
  stonksBurstSize: config.stonksBurstSize,
});

let meshNodePromise = null;
let cultMeshRudpDocumentServer = null;
let currentState = stateBuilder.buildPendingState("Coordinator starting");
let lastRefresh = {
  completedAt: null,
  durationMs: null,
  error: null,
  startedAt: null,
};
let lastIdunnRudpHealthPublishedAt = null;
let observedRudpDocuments = 0;

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

async function main() {
  if (cultRuntime.CultMesh && documents.surfaceDefinition) {
    meshNodePromise = createDurableSurfaceNode();
  }
  if (config.cultnetRudpBind) {
    cultMeshRudpDocumentServer = createCultMeshRudpDocumentServer({
      CultMesh: cultRuntime.CultMesh,
      bind: config.cultnetRudpBind,
      documents,
      getCache: async () => {
        if (!meshNodePromise) {
          throw new Error("CultMesh runtime is unavailable; Odin cannot serve a CultMesh/RUDP document catalog.");
        }
        return (await meshNodePromise).cache;
      },
      onDocumentPutRaw: (document) => {
        if (observedRudpDocuments < 40) {
          observedRudpDocuments += 1;
          console.error(
            `Odin RUDP document #${observedRudpDocuments} schema=${document.schemaId} key=${document.recordKey} source=${document.sourceRuntimeId || "unknown"} remote=${document.remote?.address}:${document.remote?.port}`,
          );
        }
        liveProviderRegistry.ingestDocument(document, document.remote);
        persistRudpDocumentPut(document).catch((error) => {
          console.error("CultMesh/RUDP document persist failed:", error.message);
        });
      },
    });
    await cultMeshRudpDocumentServer.start();
    console.log(`CultMesh/RUDP document catalog: ${config.cultnetRudpBind}`);
  }

  console.log(`Durable surface cache: ${config.cachePath}`);

  await refresh();
  scheduleRefresh();
}

async function persistRudpDocumentPut(document) {
  if (!meshNodePromise || !document?.schemaId || !document?.recordKey) return;
  const definition = documentDefinitionForSchema(document.schemaId);
  if (!definition) return;
  const node = await meshNodePromise;
  await node.put(definition, document.recordKey, normalizeRudpPayload(document.payload));
}

function documentDefinitionForSchema(schemaId) {
  return allDocumentDefinitions.find((definition) =>
    definition?.schemaId === schemaId
    || definition?.schemaVersion === schemaId
    || definition?.schemaName === schemaId
    || definition?.type === schemaId
  );
}

function normalizeRudpPayload(payload) {
  if (Array.isArray(payload) && payload.length === 1 && payload[0] && typeof payload[0] === "object" && !Array.isArray(payload[0])) {
    return payload[0];
  }
  return payload;
}

function scheduleRefresh() {
  setTimeout(() => {
    refresh()
      .catch((error) => console.error("refresh failed:", error))
      .finally(() => scheduleRefresh());
  }, config.intervalMs);
}

async function createDurableSurfaceNode() {
  try {
    return await cultRuntime.CultMesh.createNode(config.cachePath, { documents: allDocumentDefinitions });
  } catch (error) {
    if (!fs.existsSync(config.cachePath)) {
      throw error;
    }

    const corruptPath = `${config.cachePath}.corrupt-${new Date().toISOString().replace(/[:.]/g, "-")}`;
    fs.renameSync(config.cachePath, corruptPath);
    console.error(`CultMesh surface cache was unreadable and has been quarantined: ${corruptPath}`);
    console.error(`CultMesh surface cache read error: ${error.message}`);
    return cultRuntime.CultMesh.createNode(config.cachePath, { documents: allDocumentDefinitions });
  }
}

async function refresh() {
  const started = Date.now();
  lastRefresh = {
    completedAt: null,
    durationMs: null,
    error: null,
    startedAt: new Date(started).toISOString(),
  };
  try {
    currentState = await stateBuilder.buildState();
    await persistState(currentState);
    lastRefresh = {
      completedAt: new Date().toISOString(),
      durationMs: Date.now() - started,
      error: null,
      startedAt: new Date(started).toISOString(),
    };
    await publishOdinHealth("active", `Odin provider catalog refreshed in ${lastRefresh.durationMs}ms`);
  } catch (error) {
    lastRefresh = {
      completedAt: new Date().toISOString(),
      durationMs: Date.now() - started,
      error: error.message,
      startedAt: new Date(started).toISOString(),
    };
    await publishOdinHealth("failed", `Odin provider refresh failed: ${error.message}`);
    throw error;
  }
}

async function publishOdinHealth(state, detail) {
  if (!idunnRudpHealthPublisher) return;
  try {
    await publishIdunnRudpHealth(idunnRudpHealthPublisher, {
      state,
      detail,
      observedAt: new Date().toISOString(),
    });
    lastIdunnRudpHealthPublishedAt = Date.now();
  } catch (error) {
    const lastPublishedAgeMs = lastIdunnRudpHealthPublishedAt === null
      ? Number.POSITIVE_INFINITY
      : Date.now() - lastIdunnRudpHealthPublishedAt;
    if (lastPublishedAgeMs > Math.max(60_000, config.intervalMs * 4)) {
      console.error("Idunn RUDP health publish failed:", error.message);
    }
  }
}

async function persistState(state) {
  if (config.writeDebugSurfaceJson) {
    fs.writeFileSync(path.join(config.stateDir, "latest-surface.json"), JSON.stringify(state, null, 2), "utf8");
  }
  if (!meshNodePromise || !documents.surfaceDefinition) {
    return;
  }

  try {
    const node = await meshNodePromise;
    await node.put(documents.surfaceDefinition, config.surfaceKey, state);
    await node.flush?.(true);
  } catch (error) {
    console.error("CultMesh snapshot write failed:", error.message);
  }
}

#!/usr/bin/env node
"use strict";

const fs = require("fs");
const path = require("path");

const { buildConfig, loadCultRuntime } = require("./odin/config.cjs");
const { defineOdinDocuments } = require("./odin/documents.cjs");
const { createInterfaceDiscovery } = require("./odin/interfaces.cjs");
const { createLayoutStore } = require("./odin/layout.cjs");
const { createStateBuilder } = require("./odin/state.cjs");
const { broadcastState, createDashboardServer } = require("./odin/websocket.cjs");

const config = buildConfig(process.argv.slice(2));
fs.mkdirSync(config.stateDir, { recursive: true });

const cultRuntime = loadCultRuntime();
if (cultRuntime.error) {
  console.error("CultMesh runtime unavailable; durable mesh snapshot disabled:", cultRuntime.error.message);
}

const documents = defineOdinDocuments(cultRuntime.defineDocumentType);
const layoutStore = createLayoutStore(config.layoutPath);
const interfaceDiscovery = createInterfaceDiscovery({
  CultMesh: cultRuntime.CultMesh,
  documents,
  interfaceBindingStores: config.interfaceBindingStores,
  seedDeckUrls: config.seedDeckUrls,
});
const stateBuilder = createStateBuilder({
  cachePath: config.cachePath,
  gamecultTextDocumentStorePath: config.gamecultTextDocumentStorePath,
  interfaceDiscovery,
  layoutStore,
  observationFreshSeconds: config.observationFreshSeconds,
  observationLogPath: config.observationLogPath,
  stonksBurstSize: config.stonksBurstSize,
  stonksStateUrl: config.stonksStateUrl,
});

let meshNodePromise = null;
let currentState = stateBuilder.buildPendingState("Coordinator starting");
let lastRefresh = {
  completedAt: null,
  durationMs: null,
  error: null,
  startedAt: null,
};

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});

async function main() {
  if (cultRuntime.CultMesh && documents.surfaceDefinition) {
    meshNodePromise = cultRuntime.CultMesh.createNode(config.cachePath, { documents: [documents.surfaceDefinition] });
  }

  const dashboardServer = createDashboardServer({
    applyClientCommand: (command) => layoutStore.applyClientCommand(command),
    getCurrentState: () => currentState,
    getHealth: () => health(dashboardServer.clients),
    host: config.host,
    port: config.port,
  });
  console.log(`Durable surface cache: ${config.cachePath}`);

  await refresh(dashboardServer.clients);
  setInterval(() => {
    refresh(dashboardServer.clients).catch((error) => console.error("refresh failed:", error));
  }, config.intervalMs);
}

async function refresh(clients) {
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
    broadcastState(clients, currentState);
    lastRefresh = {
      completedAt: new Date().toISOString(),
      durationMs: Date.now() - started,
      error: null,
      startedAt: new Date(started).toISOString(),
    };
  } catch (error) {
    lastRefresh = {
      completedAt: new Date().toISOString(),
      durationMs: Date.now() - started,
      error: error.message,
      startedAt: new Date(started).toISOString(),
    };
    throw error;
  }
}

async function persistState(state) {
  fs.writeFileSync(path.join(config.stateDir, "latest-surface.json"), JSON.stringify(state, null, 2), "utf8");
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

function health(clients) {
  return {
    ok: !lastRefresh.error,
    providerId: currentState.providerId,
    version: currentState.version,
    clients: clients.size,
    cachePath: config.cachePath,
    stateDir: config.stateDir,
    layoutPath: config.layoutPath,
    intervalMs: config.intervalMs,
    cultMesh: {
      available: Boolean(cultRuntime.CultMesh && documents.surfaceDefinition),
      error: cultRuntime.error?.message || null,
    },
    discovery: {
      seedDeckUrls: config.seedDeckUrls,
      discoveredDeckUrls: interfaceDiscovery.getDiscoveredDeckUrls(),
      interfaceBindingStores: config.interfaceBindingStores,
    },
    refresh: lastRefresh,
  };
}

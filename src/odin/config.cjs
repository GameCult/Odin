"use strict";

const Module = require("module");
const path = require("path");
const { parseArgs } = require("./utils.cjs");

const repoRoot = path.resolve(__dirname, "..", "..");

process.env.NODE_PATH = [
  process.env.CULTLIB_PACKAGES || path.resolve(repoRoot, "..", "CultLib-dev-runtime", "packages"),
  process.env.NODE_PATH || "",
].filter(Boolean).join(path.delimiter);
Module._initPaths();

function buildConfig(argv) {
  const args = parseArgs(argv);
  const cultnetRudpBind = args["cultnet-rudp-bind"] ? String(args["cultnet-rudp-bind"]) : "";
  const intervalMs = Number(args.intervalMs || 5000);
  const stateDir = args.stateDir || path.join(repoRoot, "scratch", "odin");
  const cachePath = args.cachePath || path.join(stateDir, "odin.ccmp");
  const layoutPath = args.layoutPath || path.join(stateDir, "interface-layout.json");
  const writeDebugSurfaceJson = Boolean(args.writeDebugSurfaceJson || args["write-debug-surface-json"] || process.env.ODIN_WRITE_DEBUG_SURFACE_JSON === "1");
  if (args.observationLogPath || args.observationFreshSeconds) {
    throw new Error("Odin no longer tails Mimir observation logs. Publish observation records through CultMesh/Odin provider discovery instead.");
  }
  const gamecultTextDocumentStorePath = args.gamecultTextDocumentStorePath || path.join(repoRoot, "..", "VoidBot", ".voidbot", "private", "gamecult-text-documents.cc");
  const stonksBurstSize = Math.max(1, Number(args.stonksBurstSize || 8));
  const idunnRudpHealth = args["idunn-rudp-health"] ? {
    endpoint: String(args["idunn-rudp-health"]),
    daemonId: String(args["idunn-daemon"] || "odin"),
    healthContract: String(args["idunn-health-contract"] || "odin.cultnet-rudp-provider-health"),
  } : null;
  const interfaceBindingStores = String(
    args.interfaceBindingStore ||
    process.env.ODIN_INTERFACE_BINDING_STORES ||
    "",
  )
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);

  return {
    args,
    cachePath,
    cultnetRudpBind,
    gamecultTextDocumentStorePath,
    interfaceBindingStores,
    idunnRudpHealth,
    intervalMs,
    layoutPath,
    repoRoot,
    stateDir,
    stonksBurstSize,
    surfaceKey: "surface:gamecult.network.status",
    writeDebugSurfaceJson,
  };
}

function loadCultRuntime() {
  try {
    const { CultMesh } = require("cultmesh-ts");
    const { defineDocumentType } = require("cultcache-ts");
    return { CultMesh, defineDocumentType, error: null };
  } catch (error) {
    return { CultMesh: null, defineDocumentType: null, error };
  }
}

module.exports = { buildConfig, loadCultRuntime, repoRoot };

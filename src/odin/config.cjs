"use strict";

const Module = require("module");
const path = require("path");
const { parseArgs } = require("./utils.cjs");

const repoRoot = path.resolve(__dirname, "..", "..");

process.env.NODE_PATH = [
  path.resolve(repoRoot, "..", "CultLib", "packages"),
  process.env.NODE_PATH || "",
].filter(Boolean).join(path.delimiter);
Module._initPaths();

function buildConfig(argv) {
  const args = parseArgs(argv);
  const port = Number(args.port || 8797);
  const host = args.host || "0.0.0.0";
  const cultnetRudpBind = args["cultnet-rudp-bind"] ? String(args["cultnet-rudp-bind"]) : "";
  const intervalMs = Number(args.intervalMs || 5000);
  const stateDir = args.stateDir || path.join(repoRoot, "scratch", "odin");
  const cachePath = args.cachePath || path.join(stateDir, "odin.ccmp");
  const layoutPath = args.layoutPath || path.join(stateDir, "interface-layout.json");
  const seedDeckUrls = String(args.eveDeckUrl || "ws://127.0.0.1:8795/eve/deck,ws://127.0.0.1:8796/eve/deck,ws://127.0.0.1:8799/eve/deck,ws://127.0.0.1:8802/eve/deck,ws://192.168.1.75:8795/eve/deck,ws://10.77.0.4:8824/eve/deck")
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);
  const observationLogPath = args.observationLogPath || path.join(repoRoot, "..", "Mimir", "artifacts", "runtime", "periwinkle-cultmesh-sensors.out.log");
  const observationFreshSeconds = Number(args.observationFreshSeconds || 120);
  const gamecultTextDocumentStorePath = args.gamecultTextDocumentStorePath || path.join(repoRoot, "..", "VoidBot", ".voidbot", "private", "gamecult-text-documents.cc");
  const stonksStateUrl = args.stonksStateUrl || "http://127.0.0.1:8802/market/state";
  const stonksBurstSize = Math.max(1, Number(args.stonksBurstSize || 8));
  const idunnRudpHealth = args["idunn-rudp-health"] ? {
    endpoint: String(args["idunn-rudp-health"]),
    daemonId: String(args["idunn-daemon"] || "odin"),
    healthContract: String(args["idunn-health-contract"] || "odin.cultnet-rudp-provider-health"),
  } : null;
  const defaultInterfaceBindingStores = [
    path.join(repoRoot, "..", "VoidBot", ".voidbot", "status", "cultmesh", "voidbot-swarm-state.cc"),
    path.join(repoRoot, "..", "Bifrost", ".bifrost", "provider-advertisement.cc"),
    path.join(repoRoot, "..", "weksa", ".weksa", "provider-advertisement-store.cc"),
    "sftp://raven/E:/Projects/Vili/.vili/vili.service.cc",
    path.join(repoRoot, "..", "Stonks", "scratch", "stonks", "stonks-state.cc"),
    path.join(repoRoot, "..", "StreamPixels", ".streampixels-data", "cultcache", "streampixels.service.cc"),
    "sftp://raven/C:/Meta/Odin/state/muninn.telemetry.cc",
    "C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc",
    "sftp://nightwing/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc",
    "sftp://nightwing/var/lib/gamecult/gjallar/cultcache/gjallar.service.cc",
  ].join(",");
  const interfaceBindingStores = String(
    args.interfaceBindingStore ||
    defaultInterfaceBindingStores,
  )
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean);

  return {
    args,
    cachePath,
    cultnetRudpBind,
    gamecultTextDocumentStorePath,
    host,
    interfaceBindingStores,
    idunnRudpHealth,
    intervalMs,
    layoutPath,
    observationFreshSeconds,
    observationLogPath,
    port,
    repoRoot,
    seedDeckUrls,
    stateDir,
    stonksBurstSize,
    stonksStateUrl,
    surfaceKey: "surface:gamecult.network.status",
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

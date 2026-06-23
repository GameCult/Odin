"use strict";

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
  CultNetDocumentRegistry,
  defineCultNetDocumentBinding,
} = requireCultNet("./dist/index.js");

const ODIN_RUDP_CONNECTION_ID = 0x0d1d0002;

function createCultMeshRudpDocumentServer(options) {
  if (!options?.bind) return null;
  if (!options.CultMesh?.createRudpDocumentServer) {
    throw new Error("Odin requires a CultMesh runtime with createRudpDocumentServer.");
  }
  if (!options.documents?.surfaceDefinition) {
    throw new Error("Odin CultMesh/RUDP document server requires a surface document definition.");
  }
  if (typeof options.getCache !== "function") {
    throw new Error("Odin CultMesh/RUDP document server requires an async getCache function.");
  }

  const bind = parseEndpoint(options.bind);
  const registry = new CultNetDocumentRegistry([
    defineCultNetDocumentBinding({ definition: options.documents.surfaceDefinition }),
  ]);
  const server = options.CultMesh.createRudpDocumentServer(
    "odin-cultmesh-catalog",
    ODIN_RUDP_CONNECTION_ID,
    {
      bindHost: bind.host,
      bindPort: bind.port,
      documents: registry,
      getCache: options.getCache,
      onError: (error) => {
        const message = error instanceof Error ? error.message : String(error);
        if (typeof options.logError === "function") {
          options.logError(`CultMesh/RUDP document server error: ${message}`);
          return;
        }
        console.error(`CultMesh/RUDP document server error: ${message}`);
      },
      onDocumentPutRaw: (document) => {
        if (typeof options.onDocumentPutRaw === "function") {
          options.onDocumentPutRaw(document);
        }
      },
    },
  );

  return {
    bind,
    start: () => server.start(),
    close: () => server.close(),
  };
}

function parseEndpoint(value) {
  const text = String(value || "").trim();
  const ipv6 = text.match(/^\[([^\]]+)\]:(\d+)$/);
  if (ipv6) return { host: ipv6[1], port: parsePort(ipv6[2]) };
  const index = text.lastIndexOf(":");
  if (index <= 0) {
    throw new Error(`CultMesh/RUDP bind must be host:port, got "${value}".`);
  }
  return { host: text.slice(0, index), port: parsePort(text.slice(index + 1)) };
}

function parsePort(value) {
  const port = Number(value);
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`CultMesh/RUDP port is invalid: ${value}`);
  }
  return port;
}

module.exports = {
  createCultMeshRudpDocumentServer,
};

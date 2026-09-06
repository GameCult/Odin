"use strict";

// The swarm document-type catalog, as Odin consumes it.
//
// The 45 definitions used to live here. Almost none of them are Odin's: they
// describe documents that cross service boundaries, and every service that
// decodes another's documents needs the same ones. They lived here because Odin
// was written first. They now live in CultLib as cultcache-ts's
// defineSwarmDocuments, beside the defineDocumentType that builds them, so
// Hermodr can leave without forking a second definition of one wire format.

const path = require("path");
const { createRequire } = require("module");

const cultLibRoot = process.env.CULTLIB_ROOT
  ? path.resolve(process.env.CULTLIB_ROOT)
  : path.resolve(__dirname, "..", "..", "..", "CultLib");

const requireCultCache = createRequire(path.resolve(cultLibRoot, "packages", "cultcache-ts", "package.json"));

const { defineSwarmDocuments } = requireCultCache("./dist/index.js");

// Odin's consumers know this name. The catalog is not Odin's, but the call is.
const defineOdinDocuments = defineSwarmDocuments;

module.exports = { defineOdinDocuments, defineSwarmDocuments };

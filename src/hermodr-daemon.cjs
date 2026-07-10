#!/usr/bin/env node
"use strict";

const fs = require("fs");
const http = require("http");
const { createRequire } = require("module");
const path = require("path");

const { defineOdinDocuments } = require("./odin/documents.cjs");
const { parseArgs } = require("./odin/utils.cjs");

const repoRoot = path.resolve(__dirname, "..");
const projectsRoot = path.resolve(repoRoot, "..");
const defaultEveRepoRoot = path.resolve(projectsRoot, "Eve");
const defaultEveWebRoot = path.resolve(defaultEveRepoRoot, "web");
const defaultOdinCultMeshUri = "cultmesh://odin/rendezvous/provider-catalog";
const defaultOdinRudpEndpoint = "rudp://127.0.0.1:17871";
const odinRudpConnectionId = 0x0d1d0002;
const providerRudpConnectionId = 0x43554c54;
const providerRudpPeers = new Map();
const defaultProviderCatalogKeys = [];

const cultCacheRequire = createRequire(resolveCultCachePackagePath());
const cultMeshRequire = createRequire(resolveCultMeshPackagePath());
const { defineDocumentType } = cultCacheRequire(resolveCultCacheRuntimePath());
const { CultMesh } = cultMeshRequire(resolveCultMeshRuntimePath());
const { decode: decodeMessagePack } = cultMeshRequire("@msgpack/msgpack");

const documents = defineOdinDocuments(defineDocumentType);
const odinDocumentDefinitions = Object.values(documents).filter(Boolean);
const HermodrCommandDefinition = defineDocumentType({
  type: "gamecult.eve.command",
  schemaName: "gamecult.eve.command",
  schemaId: "gamecult.eve.command.v1",
  schemaVersion: "gamecult.eve.command.v1",
  global: false,
  name: (value) => value?.commandId || value?.command_id || "hermodr-command",
  schema: requireObject("Hermodr command document"),
});

async function main() {
  const options = parseOptions(process.argv.slice(2));
  rejectRemovedOptions(options);
  if (options.help) {
    printUsage();
    return;
  }

  const bridge = createHermodrBridge(options);
  const server = http.createServer((request, response) => {
    bridge.handle(request, response).catch((error) => {
      writeJson(response, error?.statusCode || 500, {
        ok: false,
        error: error instanceof Error ? error.message : String(error),
      });
    });
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(options.port, options.host, () => {
      server.off("error", reject);
      resolve();
    });
  });

  const address = server.address();
  const port = typeof address === "object" && address ? address.port : options.port;
  console.log(`Hermodr browser lowering listening on http://${options.host}:${port}`);
  console.log(`Hermodr Odin CultMesh URI: ${options.odinCultMeshUri}`);

  const shutdown = () => {
    server.close(() => {});
    closeProviderRudpPeers();
  };
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);
}

function parseOptions(argv) {
  const parsed = parseArgs(argv);
  return {
    help: Boolean(parsed.help),
    host: stringOption(parsed.host, process.env.HERMODR_BIND_HOST || "127.0.0.1"),
    port: numberOption(parsed.port, process.env.HERMODR_PORT || 8798),
    odinCultMeshUri: stringOption(
      parsed.odinCultMeshUri || parsed.odinUri || parsed["odin-cultmesh-uri"],
      process.env.HERMODR_ODIN_CULTMESH_URI || process.env.ODIN_CULTMESH_URI || defaultOdinCultMeshUri,
    ),
    odinRudpEndpoint: stringOption(
      parsed.odinRudpEndpoint || parsed["odin-rudp-endpoint"],
      process.env.HERMODR_ODIN_RUDP || process.env.CULTMESH_URI_ODIN_RUDP || defaultOdinRudpEndpoint,
    ),
    odinSurfaceKey: stringOption(parsed.odinSurfaceKey, process.env.HERMODR_ODIN_SURFACE_KEY || "surface:gamecult.network.status"),
    eveWebRoot: path.resolve(stringOption(parsed.eveWebRoot, process.env.HERMODR_EVE_WEB_ROOT || defaultEveWebRoot)),
    eveRepoRoot: path.resolve(stringOption(parsed.eveRepoRoot, process.env.HERMODR_EVE_REPO_ROOT || defaultEveRepoRoot)),
    providerCatalogKeys: stringListOption(
      parsed.providerCatalogKeys || parsed["provider-catalog-keys"],
      process.env.HERMODR_PROVIDER_CATALOG_KEYS,
      defaultProviderCatalogKeys,
    ),
    sleipnirProviderId: stringOption(parsed.sleipnirProviderId, process.env.HERMODR_SLEIPNIR_PROVIDER_ID || "sleipnir.input-mirror.starfire"),
    raw: parsed,
  };
}

function rejectRemovedOptions(options) {
  const removed = [
    "odinHttpBase",
    "odinWsBase",
    "odinStatePath",
    "sleipnirMappingPath",
    "sleipnirStorePath",
    "sleipnirCommandEndpoint",
    "sleipnirCommandRudp",
    "muninnRudp",
    "aetheriaCommandRudp",
    "hermodrCommandStorePath",
    "odinCachePath",
  ];
  for (const option of removed) {
    if (Object.hasOwn(options.raw, option)) {
      throw new Error(`--${option} was removed. Hermodr is a browser lowering over Odin/CultMesh state, not a fallback discovery or raw command transport.`);
    }
  }

  const removedEnv = [
    "HERMODR_ODIN_HTTP_BASE",
    "HERMODR_ODIN_WS_BASE",
    "HERMODR_ODIN_STATE_PATH",
    "HERMODR_SLEIPNIR_MAPPING_PATH",
    "HERMODR_SLEIPNIR_STORE_PATH",
    "HERMODR_SLEIPNIR_COMMAND_ENDPOINT",
    "HERMODR_SLEIPNIR_COMMAND_RUDP",
    "HERMODR_MUNINN_RUDP",
    "HERMODR_AETHERIA_COMMAND_RUDP",
    "HERMODR_ODIN_CULTMESH_STORE",
    "ODIN_CULTMESH_STORE",
  ];
  for (const name of removedEnv) {
    if (process.env[name]) {
      throw new Error(`${name} was removed. Hermodr must lower Odin/CultMesh state and publish typed CultMesh command documents.`);
    }
  }
}

function createHermodrBridge(options) {
  return {
    async handle(request, response) {
      const url = new URL(request.url || "/", `http://${request.headers.host || "localhost"}`);
      if (request.method === "OPTIONS") {
        writeCors(response, 204);
        response.end();
        return;
      }

      if (request.method === "GET" && url.pathname === "/health") {
        writeJson(response, 200, {
          ok: true,
          service: "hermodr-browser-lowering",
          authority: "lowering-only",
          visibleAuthority: "Odin sees provider-owned CultMesh documents; Odin does not own provider daemons.",
          odinCultMeshUri: options.odinCultMeshUri,
          eveWebRoot: options.eveWebRoot,
          assetBytes: "provider-cultmesh-cdn",
        });
        return;
      }

      if (request.method === "GET" && (url.pathname === "/" || url.pathname === "/index.html")) {
        await serveStaticFile(response, path.join(options.eveWebRoot, "index.html"), "text/html; charset=utf-8");
        return;
      }

      if (request.method === "GET" && url.pathname === "/cultmesh-cdn/catalog") {
        writeJson(response, 200, await readAssetCatalog(options));
        return;
      }

      if ((request.method === "GET" || request.method === "HEAD") && url.pathname.startsWith("/cultmesh-cdn/")) {
        await serveCultMeshAsset(
          options,
          response,
          decodeURIComponent(url.pathname.slice("/cultmesh-cdn/".length)),
          request.method === "HEAD",
        );
        return;
      }

      if (request.method === "GET" && url.pathname === "/hermodr/verse/catalog") {
        writeJson(response, 200, await readCatalog(options));
        return;
      }

      if (request.method === "GET" && url.pathname.startsWith("/hermodr/surface/")) {
        const providerId = decodeURIComponent(url.pathname.slice("/hermodr/surface/".length));
        const surfaceId = stringOption(url.searchParams.get("surfaceId"), "");
        const catalog = await readCatalog(options);
        const surface = await readProviderSurface(options, catalog, providerId, surfaceId);
        if (!surface) {
          writeJson(response, 404, { ok: false, error: `No surface found for provider '${providerId}'.` });
          return;
        }
        writeJson(response, 200, surface);
        return;
      }

      if (request.method === "GET" && url.pathname.startsWith("/hermodr/document/")) {
        const providerId = decodeURIComponent(url.pathname.slice("/hermodr/document/".length));
        const documentId = stringOption(url.searchParams.get("documentId"), "");
        const schemaId = stringOption(url.searchParams.get("schemaId"), "");
        const resolved = await readVisibleDocument(options, { providerId, documentId, schemaId });
        if (!resolved) {
          writeJson(response, 404, { ok: false, error: `No visible document '${documentId}' (${schemaId || "any schema"}) for provider '${providerId}'.` });
          return;
        }
        writeJson(response, 200, resolved);
        return;
      }

      if (request.method === "POST" && url.pathname === "/hermodr/commands/sleipnir/input-mapping") {
        const body = await readJsonBody(request);
        writeJson(response, 202, await publishSleipnirMapping(options, body));
        return;
      }

      if (request.method === "POST" && url.pathname === "/hermodr/commands/eve") {
        const body = await readJsonBody(request);
        writeJson(response, 202, await publishEveCommand(options, body));
        return;
      }

      if (request.method === "GET" && url.pathname.startsWith("/packages/")) {
        await serveStaticFile(response, safeJoin(path.join(options.eveRepoRoot, "packages"), url.pathname.slice("/packages/".length)));
        return;
      }

      if (request.method === "GET") {
        await serveStaticFile(response, safeJoin(options.eveWebRoot, url.pathname.slice(1)));
        return;
      }

      writeJson(response, 404, { ok: false, error: "not found" });
    },
  };
}

async function readCatalog(options) {
  const providerAdvertisements = mergeProviderAdvertisements([
    ...await readOdinProviderAdvertisements(options),
    ...await readOdinSurfaceProviderCatalog(options),
  ]);
  return createBrowserCatalog(providerAdvertisements, options);
}

function createBrowserCatalog(providerAdvertisements, options = {}) {
  const surfaces = providerAdvertisements.flatMap((provider) => {
    const advertised = provider.surfaces?.length ? provider.surfaces : [{}];
    return advertised.map((surface) => ({
      providerId: provider.id,
      surfaceId: surface.surfaceId || provider.id,
      surfaceKind: surface.surfaceKind || "",
      title: surface.title || provider.title || surface.surfaceId || provider.id,
      version: 0,
      updatedAt: provider.updatedAt || null,
      surface: { advertised: true },
    }));
  });

  return {
    schema: "gamecult.hermodr.browser_lowering_catalog.v1",
    service: "hermodr-browser-lowering",
    generatedAt: new Date().toISOString(),
    authority: {
      owner: "Odin/CultMesh",
      ownerNote: "Odin sees provider-owned daemon publications; it does not own provider daemon state.",
      hermodrRole: "browser lowering adapter",
      forbidden: ["provider catalog synthesis", "provider state ownership", "renderer-owned discovery"],
    },
    source: {
      odinCultMeshUri: options.odinCultMeshUri || "",
      odinSurfaceKey: options.odinSurfaceKey || "",
    },
    surface: null,
    surfaces,
    providers: providerAdvertisements,
  };
}

async function readOdinSurfaceProviderCatalog(options) {
  const peer = await CultMesh.createRudpPeer(
    "hermodr-browser-lowering-odin-surface",
    odinRudpConnectionId,
    options.odinCultMeshUri,
    {
      connectTimeoutMs: 2_000,
      maxFragmentBytes: 1200,
      maxPendingReliablePackets: 512,
      resolveCultMeshRudpEndpoint: (uri) => resolveHermodrRudpEndpoint(uri, options),
    },
  );
  try {
    const keys = [
      options.odinSurfaceKey,
      String(options.odinSurfaceKey || "").startsWith("surface:")
        ? String(options.odinSurfaceKey || "").slice("surface:".length)
        : `surface:${options.odinSurfaceKey}`,
    ];
    const document = await requestCultNetRawSnapshotFirstDocument(
      peer,
      "gamecult.eve.surface_state.v1",
      keys,
      { timeoutMs: 4_000, messageIdPrefix: "hermodr-odin-provider-catalog" },
    );
    const decoded = decodeMessagePack(bufferFromPayload(document.payload));
    const catalog = Array.isArray(decoded?.providerCatalog) ? decoded.providerCatalog : [];
    return catalog.map(normalizeCatalogProviderAdvertisement).filter(Boolean);
  } finally {
    if (typeof peer.close === "function") {
      peer.close();
    }
  }
}

async function readOdinProviderAdvertisements(options) {
  const peer = await CultMesh.createRudpPeer(
    "hermodr-browser-lowering-odin",
    odinRudpConnectionId,
    options.odinCultMeshUri,
    {
      connectTimeoutMs: 2_000,
      maxFragmentBytes: 1200,
      maxPendingReliablePackets: 512,
      resolveCultMeshRudpEndpoint: (uri) => resolveHermodrRudpEndpoint(uri, options),
    },
  );
  try {
    const response = await requestCultNetRawSnapshot(
      peer,
      options.providerCatalogKeys,
      {
        schemaIds: ["gamecult.eve.provider_advertisement.v1"],
        timeoutMs: 4_000,
        messageIdPrefix: "hermodr-odin-providers",
      },
    );
    return response.documents
      .filter((document) => document.schemaId === "gamecult.eve.provider_advertisement.v1")
      .map((document) => normalizeProviderAdvertisement(
        decodeMessagePack(bufferFromPayload(document.payload)),
        document.recordKey,
      ))
      .filter(Boolean);
  } finally {
    if (typeof peer.close === "function") {
      peer.close();
    }
  }
}

function normalizeCatalogProviderAdvertisement(value) {
  const providerId = value?.id || value?.providerId || value?.provider?.id;
  if (!providerId) return null;
  return {
    id: String(providerId),
    title: value.title || String(providerId),
    description: value.description || "",
    endpoint: value.endpoint || value.cultMeshAddress || value.provider?.endpoint || null,
    cultMeshAddress: value.cultMeshAddress || value.endpoint || value.provider?.endpoint || null,
    endpoints: arrayOfObjects(value.endpoints),
    routes: arrayOfObjects(value.routes),
    commandSurface: value.commandSurface || null,
    surfaces: normalizeAdvertisedSurfaces(value.surfaces),
    status: value.status || "unknown",
    updatedAt: value.updatedAt || null,
  };
}

function mergeProviderAdvertisements(providers) {
  const merged = new Map();
  for (const provider of providers) {
    if (!provider?.id) continue;
    const existing = merged.get(provider.id) || {};
    merged.set(provider.id, {
      ...existing,
      ...provider,
      endpoints: provider.endpoints?.length ? provider.endpoints : existing.endpoints || [],
      routes: provider.routes?.length ? provider.routes : existing.routes || [],
      surfaces: provider.surfaces?.length ? provider.surfaces : existing.surfaces || [],
      cultMeshAddress: provider.cultMeshAddress || existing.cultMeshAddress || null,
      endpoint: provider.endpoint || existing.endpoint || null,
    });
  }
  return [...merged.values()].sort((left, right) => String(left.id).localeCompare(String(right.id)));
}

function normalizeAdvertisedSurfaces(surfaces) {
  return arrayOfObjects(surfaces)
    .map((surface) => ({
      ...surface,
      surfaceId: String(surface.surfaceId || surface.id || "").trim(),
      surfaceKind: String(surface.surfaceKind || surface.kind || "").trim(),
      recordRef: String(surface.recordRef || surface.key || "").trim(),
    }))
    .filter((surface) => surface.surfaceId);
}

function surfaceRecordKeys(catalog, providerId, surfaceId) {
  const provider = (catalog.providers || []).find((candidate) => candidate.id === providerId);
  const surface = (provider?.surfaces || []).find((candidate) => candidate.surfaceId === surfaceId);
  return [...new Set([
    surface?.recordRef,
    surfaceId,
    `surface:${surfaceId}`,
    `eve:surface:${surfaceId}`,
    providerId,
    `surface:${providerId}`,
  ].filter(Boolean))];
}

function resolveHermodrRudpEndpoint(uri, options) {
  const text = String(uri || "").trim();
  if (text === defaultOdinCultMeshUri || text.startsWith("cultmesh://odin/")) {
    return normalizeRudpEndpoint(options.odinRudpEndpoint);
  }
  return undefined;
}

function normalizeRudpEndpoint(endpoint) {
  const text = String(endpoint || "").trim();
  if (!text) return text;
  return text.startsWith("rudp://") ? text : `rudp://${text}`;
}

async function readProviderSurface(options, catalog, providerId, surfaceId = "") {
  const selectedSurfaceId = surfaceId || providerId;
  const odinSurface = await readOdinProviderSurface(options, providerId, selectedSurfaceId);
  if (odinSurface) {
    return odinSurface;
  }

  const route = findProviderSnapshotRoute(catalog, providerId);
  if (!route) return null;
  try {
    const document = await requestProviderSnapshotFirstDocumentWithReconnect(
      route.endpoint,
      "gamecult.eve.surface_state.v1",
      surfaceRecordKeys(catalog, providerId, selectedSurfaceId),
      { timeoutMs: 4_000, messageIdPrefix: "hermodr-surface-state" },
    );
    const state = normalizeSurfaceState(decodeMessagePack(bufferFromPayload(document.payload)), document.recordKey);
    if (state?.surface?.root) {
      return {
        providerId,
        documentProviderId: state.providerId,
        providerKind: "provider-cultmesh-rudp",
        title: state.title || providerId,
        version: state.version ?? 0,
        updatedAt: state.updatedAt || null,
        surface: normalizeEveSurfaceTree(state.surface),
        commands: [],
      };
    }
  } catch {
    // Older providers may still expose gamecult.eve.surface.v1 directly.
  }
  const document = await requestProviderSnapshotFirstDocumentWithReconnect(
    route.endpoint,
    "gamecult.eve.surface.v1",
    surfaceRecordKeys(catalog, providerId, selectedSurfaceId),
    { timeoutMs: 4_000, messageIdPrefix: "hermodr-surface" },
  );
  return normalizeEveSurfaceDocument(decodeMessagePack(bufferFromPayload(document.payload)), providerId);
}

async function readOdinProviderSurface(options, providerId, surfaceId = providerId) {
  const peer = await CultMesh.createRudpPeer(
    "hermodr-browser-lowering-odin-provider-surface",
    odinRudpConnectionId,
    options.odinCultMeshUri,
    {
      connectTimeoutMs: 2_000,
      maxFragmentBytes: 1200,
      maxPendingReliablePackets: 512,
      resolveCultMeshRudpEndpoint: (uri) => resolveHermodrRudpEndpoint(uri, options),
    },
  );
  try {
    const document = await requestCultNetRawSnapshotFirstDocument(
      peer,
      "gamecult.eve.surface_state.v1",
      [surfaceId, `surface:${surfaceId}`, providerId, `surface:${providerId}`],
      { timeoutMs: 4_000, messageIdPrefix: "hermodr-odin-provider-surface" },
    );
    const state = normalizeSurfaceState(decodeMessagePack(bufferFromPayload(document.payload)), document.recordKey);
    if (!state?.surface?.root) {
      return null;
    }
    return {
      providerId,
      documentProviderId: state.providerId,
      providerKind: "odin-cultmesh-surface-state",
      title: state.title || providerId,
      version: state.version ?? 0,
      updatedAt: state.updatedAt || null,
      surface: normalizeEveSurfaceTree(state.surface),
      commands: [],
    };
  } catch {
    return null;
  } finally {
    if (typeof peer.close === "function") {
      peer.close();
    }
  }
}

function normalizeEveSurfaceDocument(value, fallbackProviderId) {
  if (Array.isArray(value)) {
    if (Array.isArray(value[5])) {
      return {
        providerId: String(fallbackProviderId || value[0] || ""),
        documentProviderId: String(value[0] || ""),
        providerKind: String(value[1] || ""),
        title: String(value[2] || value[0] || fallbackProviderId || ""),
        version: value[3] ?? 0,
        updatedAt: value[4] || null,
        surface: normalizeEveSurfaceTree(value[5]),
        commands: Array.isArray(value[6]) ? normalizeEveCommands(value[6]) : [],
      };
    }
    return {
      providerId: String(value[2] || fallbackProviderId || ""),
      providerKind: String(value[3] || ""),
      title: String(value[4] || value[2] || fallbackProviderId || ""),
      version: value[5] ?? 0,
      updatedAt: value[6] || null,
      surface: normalizeEveSurfaceTree(value[7]),
      commands: Array.isArray(value[8]) ? normalizeEveCommands(value[8]) : [],
    };
  }
  return {
    providerId: String(value?.providerId || fallbackProviderId || ""),
    providerKind: String(value?.providerKind || ""),
    title: String(value?.title || value?.surface?.title || value?.providerId || fallbackProviderId || ""),
    version: value?.version ?? 0,
    updatedAt: value?.updatedAtUtc || value?.updatedAt || null,
    surface: normalizeEveSurfaceTree(value?.surface),
    commands: Array.isArray(value?.commands) ? normalizeEveCommands(value.commands) : [],
  };
}

function normalizeEveSurfaceTree(value) {
  if (Array.isArray(value)) {
    return {
      id: String(value[0] || ""),
      root: normalizeEveSurfaceComponent(value[1]),
      styles: Array.isArray(value[2]) ? value[2] : [],
    };
  }
  return {
    id: String(value?.id || ""),
    title: value?.title,
    root: normalizeEveSurfaceComponent(value?.root),
    styles: Array.isArray(value?.styles) ? value.styles : [],
  };
}

function normalizeEveSurfaceComponent(value) {
  if (Array.isArray(value)) {
    const component = {
      id: String(value[0] || ""),
      kind: String(value[1] || ""),
      props: value[2] && typeof value[2] === "object" && !Array.isArray(value[2]) ? value[2] : {},
      children: Array.isArray(value[3]) ? value[3].map(normalizeEveSurfaceComponent) : [],
    };
    if (Array.isArray(value[5])) component.embeddedDocuments = normalizeEmbeddedDocuments(value[5]);
    if (value[6] && typeof value[6] === "object" && !Array.isArray(value[6])) component.layout = value[6];
    if (value[7] && typeof value[7] === "object" && !Array.isArray(value[7])) component.style = value[7];
    return component;
  }
  if (!value || typeof value !== "object") {
    return { id: "", kind: "surface", props: {}, children: [] };
  }
  return {
    ...value,
    props: value.props && typeof value.props === "object" && !Array.isArray(value.props) ? value.props : {},
    children: Array.isArray(value.children) ? value.children.map(normalizeEveSurfaceComponent) : [],
  };
}

function normalizeEveCommands(commands) {
  return commands.map((command) => {
    if (!Array.isArray(command)) return command;
    return {
      commandId: String(command[0] || ""),
      label: String(command[1] || ""),
      description: String(command[2] || ""),
      authority: String(command[3] || ""),
      transport: String(command[4] || ""),
    };
  });
}

function normalizeEmbeddedDocuments(documents) {
  return documents.map((document) => {
    if (!Array.isArray(document)) return document;
    return {
      slotId: String(document[0] || ""),
      documentId: String(document[1] || ""),
      schemaId: String(document[2] || ""),
      presentationKind: String(document[3] || ""),
      bindingMode: String(document[4] || ""),
      query: String(document[5] || ""),
    };
  });
}

async function publishSleipnirMapping(options, body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw new Error("Sleipnir input mapping command body must be an object.");
  }

  const catalog = await readCatalog(options);
  const route = findSleipnirCommandRoute(catalog, options.sleipnirProviderId);
  if (!route) {
    const error = new Error(`Odin does not advertise a cultmesh:// Sleipnir input mapping command route for ${options.sleipnirProviderId}.`);
    error.statusCode = 409;
    throw error;
  }

  const commandId = body.commandId || body.command_id || `hermodr:sleipnir:${Date.now()}:${Math.random().toString(16).slice(2)}`;
  const mapping = {
    ...body,
    schema: "sleipnir.input_mapping.v1",
    providerId: options.sleipnirProviderId,
    commandId,
    commandRoute: route.uri,
    commandRouteResolver: "odin-cultmesh",
    publishedBy: "hermodr-browser-lowering",
    publishedAt: new Date().toISOString(),
  };
  delete mapping.muninnRudp;
  delete mapping.muninn_rudp;
  const publication = await publishCommandDocument(options, documents.SleipnirInputMappingDefinition, options.sleipnirProviderId, mapping, route.uri);
  if (publication.routeError) {
    const error = new Error(`CultMesh route publish failed for ${route.uri}: ${publication.routeError}`);
    error.statusCode = 502;
    throw error;
  }
  return {
    ok: true,
    schema: "sleipnir.input_mapping.v1",
    commandId,
    providerId: options.sleipnirProviderId,
    commandRoute: route.uri,
    route: route.uri,
    routeError: publication.routeError || undefined,
  };
}

async function publishEveCommand(options, body) {
  if (!body || typeof body !== "object" || Array.isArray(body)) {
    throw new Error("Eve command body must be an object.");
  }

  const target = typeof body.target === "string" ? body.target.trim() : "";
  const catalog = await readCatalog(options);
  const route = target.startsWith("cultmesh://")
    ? target
    : findProviderCommandRoute(catalog, body.providerId || body.provider_id || "");
  if (!route) {
    const error = new Error("No provider-advertised cultmesh:// command route was visible.");
    error.statusCode = 409;
    throw error;
  }

  const commandId = body.commandId || body.command_id || `hermodr:eve:${Date.now()}:${Math.random().toString(16).slice(2)}`;
  const command = {
    ...body,
    schema: "gamecult.eve.command.v1",
    commandId,
    target: route || target || null,
    publishedBy: "hermodr-browser-lowering",
    publishedAt: new Date().toISOString(),
  };
  const publication = await publishCommandDocument(options, HermodrCommandDefinition, commandId, command, route);
  if (publication.routeError) {
    const error = new Error(`CultMesh route publish failed for ${route}: ${publication.routeError}`);
    error.statusCode = 502;
    throw error;
  }
  return {
    ok: true,
    schema: "gamecult.eve.command.v1",
    commandId,
    target: route,
  };
}

async function publishCommandDocument(options, definition, key, value, route) {
  let routeError = null;
  if (route && route.startsWith("cultmesh://")) {
    try {
      await CultMesh.publishRudpDocumentOnce(
        "hermodr-browser-lowering",
        0xe1e00001,
        route,
        { definition },
        key,
        value,
        {
          sourceRuntimeId: "hermodr-browser-lowering",
          sourceRole: "browser-lowering",
          tags: ["hermodr", "browser", "command"],
        },
      );
    } catch (error) {
      routeError = error instanceof Error ? error.message : String(error);
    }
  }
  return { routeError };
}

async function readVisibleDocument(options, request) {
  const catalog = await readCatalog(options);
  const resolved = await fetchProviderDocument(catalog, request);
  if (!resolved) return null;
  return {
    ok: true,
    schema: "gamecult.hermodr.resolved_document.v1",
    providerId: request.providerId,
    documentId: request.documentId,
    schemaId: resolved.schemaId,
    recordKey: resolved.recordKey,
    document: resolved.document,
    source: {
      visibility: "provider-cultmesh-rudp-snapshot",
      endpoint: resolved.endpoint,
      odinCultMeshUri: options.odinCultMeshUri,
    },
  };
}

function findSleipnirCommandRoute(catalog, providerId) {
  const providers = catalog.providers.filter((provider) => provider.id === providerId || String(provider.id).includes("sleipnir"));
  for (const provider of providers) {
    const endpoints = [...(provider.endpoints || []), ...(provider.routes || [])];
    const route = endpoints.find((endpoint) => {
      const uri = endpoint.uri || endpoint.endpoint || endpoint.address || "";
      const role = [endpoint.id, endpoint.role, ...(endpoint.tags || [])].join(" ").toLowerCase();
      return uri.startsWith("cultmesh://") && role.includes("command") && role.includes("sleipnir");
    }) || endpoints.find((endpoint) => {
      const uri = endpoint.uri || endpoint.endpoint || endpoint.address || "";
      return uri.startsWith("cultmesh://") && uri.includes("sleipnir") && uri.includes("input_mapping");
    });
    if (route) {
      return {
        ...route,
        uri: route.uri || route.endpoint || route.address,
      };
    }
  }
  return null;
}

function findProviderCommandRoute(catalog, providerId) {
  const normalized = String(providerId || "").trim();
  const providers = catalog.providers.filter((provider) => {
    if (!normalized) return true;
    return provider.id === normalized || provider.id.includes(normalized) || normalized.includes(provider.id);
  });
  for (const provider of providers) {
    const endpoints = [...(provider.endpoints || []), ...(provider.routes || [])];
    const route = endpoints.find((endpoint) => {
      const uri = endpoint.uri || endpoint.endpoint || endpoint.address || "";
      const role = [endpoint.id, endpoint.role, ...(endpoint.tags || [])].join(" ").toLowerCase();
      return uri.startsWith("cultmesh://") && role.includes("command");
    });
    if (route) return route.uri || route.endpoint || route.address;
  }
  return "";
}

function normalizeProviderAdvertisement(value, recordKey) {
  const providerId = value?.providerId || value?.provider?.id || value?.id || recordKey;
  if (!providerId) {
    return null;
  }
  const endpoints = arrayOfObjects(value?.endpoints);
  const routes = arrayOfObjects(value?.routes);
  return {
    id: String(providerId),
    title: value.title || value.provider?.title || String(providerId),
    description: value.description || "",
    cultMeshAddress: value.cultMeshAddress || value.provider?.endpoint || null,
    endpoints,
    routes,
    commandSurface: value.commandSurface || null,
    surfaces: normalizeAdvertisedSurfaces(value.surfaces),
    status: value.status || "unknown",
    updatedAt: value.updatedAt || value.generatedAt || null,
  };
}

function normalizeSurfaceState(value, recordKey) {
  if (Array.isArray(value)) {
    value = {
      providerId: value[0],
      title: value[1],
      version: value[2],
      updatedAt: value[3],
      surface: value[4],
    };
  }
  const providerId = value?.providerId || value?.provider_id || recordKey?.replace(/^surface:/, "");
  if (!providerId) {
    return null;
  }
  return {
    providerId: String(providerId),
    title: value.title || String(providerId),
    version: value.version || 0,
    updatedAt: value.updatedAt || value.updated_at || null,
    surface: value.surface || null,
  };
}

function decodeCultMeshDocumentId(documentId) {
  const text = String(documentId || "");
  if (!text.startsWith("cultmesh://")) return text;
  const parsed = new URL(text);
  return decodeURIComponent(parsed.pathname.replace(/^\/+/, ""));
}

function arrayOfObjects(value) {
  return Array.isArray(value) ? value.filter((item) => item && typeof item === "object" && !Array.isArray(item)) : [];
}

function requireObject(label) {
  return {
    parse(value) {
      if (!value || typeof value !== "object" || Array.isArray(value)) {
        throw new Error(`${label} must be an object.`);
      }
      return value;
    },
  };
}

function readJsonBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("error", reject);
    request.on("end", () => {
      try {
        const text = Buffer.concat(chunks).toString("utf8").trim();
        resolve(text ? JSON.parse(text) : {});
      } catch (error) {
        reject(new Error(`Invalid JSON body: ${error.message}`));
      }
    });
  });
}

function writeJson(response, statusCode, body) {
  writeCors(response, statusCode);
  response.setHeader("Content-Type", "application/json; charset=utf-8");
  response.end(`${JSON.stringify(body, null, 2)}\n`);
}

function writeCors(response, statusCode) {
  response.statusCode = statusCode;
  response.setHeader("Access-Control-Allow-Origin", "*");
  response.setHeader("Access-Control-Allow-Methods", "GET,POST,OPTIONS");
  response.setHeader("Access-Control-Allow-Headers", "content-type");
}

async function serveStaticFile(response, filePath, contentType, headOnly = false) {
  if (!filePath || !fs.existsSync(filePath) || fs.statSync(filePath).isDirectory()) {
    writeJson(response, 404, { ok: false, error: "not found" });
    return;
  }
  writeCors(response, 200);
  response.setHeader("Content-Type", contentType || contentTypeForPath(filePath));
  response.setHeader("Content-Length", fs.statSync(filePath).size);
  if (headOnly) {
    response.end();
    return;
  }
  response.end(await fs.promises.readFile(filePath));
}

function safeJoin(root, relativePath) {
  const resolved = path.resolve(root, relativePath || "");
  const normalizedRoot = path.resolve(root);
  if (resolved !== normalizedRoot && !resolved.startsWith(`${normalizedRoot}${path.sep}`)) {
    return "";
  }
  return resolved;
}

async function readAssetCatalog(options) {
  const catalog = await readCatalog(options);
  return {
    ok: true,
    schema: "gamecult.hermodr.asset_catalog.v1",
    source: "odin-cultmesh-provider-routes",
    providers: catalog.providers,
    manifests: [],
  };
}

async function serveCultMeshAsset(options, response, uri, headOnly = false) {
  const assetUri = normalizeCultMeshAssetUri(uri);
  try {
    const asset = await fetchCultMeshCdnAsset(options, assetUri);
    writeCors(response, 200);
    response.setHeader("Content-Type", asset.contentType || contentTypeForPath(assetUri));
    response.setHeader("Content-Length", asset.bytes.length);
    response.setHeader("X-CultMesh-CDN-Provider", asset.providerId);
    response.setHeader("X-CultMesh-CDN-Endpoint", asset.endpoint);
    if (headOnly) {
      response.end();
      return;
    }
    response.end(asset.bytes);
    return;
  } catch (error) {
    writeJson(response, 502, {
      ok: false,
      error: `Provider-owned CultMesh CDN fetch failed for ${assetUri}: ${error instanceof Error ? error.message : String(error)}`,
      note: "Hermodr resolves provider-advertised CultMesh CDN routes and never reads product filesystems.",
    });
    return;
  }
}

async function fetchCultMeshCdnAsset(options, assetUri) {
  const catalog = await readCatalog(options);
  const route = findProviderCdnRoute(catalog, assetUri);
  if (!route) {
    throw new Error(`No provider advertised a CultMesh CDN route for '${assetUri}'.`);
  }

  const peer = await getProviderRudpPeer(route.endpoint);
  const document = await requestCultNetRawSnapshotDocument(
    peer,
    "gamecult.cultmesh.cdn.asset_blob.v1",
    assetUri,
    { timeoutMs: 4_000, messageIdPrefix: "hermodr-cdn" },
  );
  return {
    providerId: route.providerId,
    endpoint: route.endpoint,
    contentType: contentTypeFromTags(document.tags) || contentTypeForPath(assetUri),
    bytes: bufferFromMessagePackedBytes(document.payload),
  };
}

async function fetchProviderDocument(catalog, request) {
  const route = findProviderSnapshotRoute(catalog, request.providerId);
  if (!route) return null;
  const recordKey = decodeCultMeshDocumentId(request.documentId);
  const document = await requestProviderSnapshotDocumentWithReconnect(
    route.endpoint,
    request.schemaId,
    recordKey,
    { timeoutMs: 4_000, messageIdPrefix: "hermodr-document" },
  );
  return {
    endpoint: route.endpoint,
    schemaId: document.schemaId,
    recordKey: document.recordKey,
    document: normalizeProviderDocumentPayload(document.schemaId, decodeMessagePack(bufferFromPayload(document.payload))),
  };
}

async function requestProviderSnapshotDocumentWithReconnect(endpoint, schemaId, recordKey, options = {}) {
  try {
    const peer = await getProviderRudpPeer(endpoint);
    return await requestCultNetRawSnapshotDocument(peer, schemaId, recordKey, options);
  } catch (firstError) {
    await dropProviderRudpPeer(endpoint);
    const peer = await getProviderRudpPeer(endpoint);
    try {
      return await requestCultNetRawSnapshotDocument(peer, schemaId, recordKey, options);
    } catch (secondError) {
      secondError.message = `${secondError.message} Provider route ${endpoint} was retried after dropping a stale Hermodr RUDP peer. First error: ${
        firstError instanceof Error ? firstError.message : String(firstError)
      }`;
      throw secondError;
    }
  }
}

async function requestProviderSnapshotFirstDocumentWithReconnect(endpoint, schemaId, recordKeys, options = {}) {
  try {
    const peer = await getProviderRudpPeer(endpoint);
    return await requestCultNetRawSnapshotFirstDocument(peer, schemaId, recordKeys, options);
  } catch (firstError) {
    await dropProviderRudpPeer(endpoint);
    const peer = await getProviderRudpPeer(endpoint);
    try {
      return await requestCultNetRawSnapshotFirstDocument(peer, schemaId, recordKeys, options);
    } catch (secondError) {
      secondError.message = `${secondError.message} Provider route ${endpoint} was retried after dropping a stale Hermodr RUDP peer. First error: ${
        firstError instanceof Error ? firstError.message : String(firstError)
      }`;
      throw secondError;
    }
  }
}

async function getProviderRudpPeer(endpoint) {
  const key = String(endpoint || "").trim();
  if (!key) throw new Error("Provider RUDP endpoint is empty.");
  const existing = providerRudpPeers.get(key);
  if (existing) return existing;

  const pending = CultMesh.createRudpPeer(
    "hermodr-browser-lowering-provider",
    providerRudpConnectionId,
    key,
    {
      connectTimeoutMs: 2_000,
      maxFragmentBytes: 1200,
      maxPendingReliablePackets: 512,
    },
  ).catch((error) => {
    providerRudpPeers.delete(key);
    throw error;
  });
  providerRudpPeers.set(key, pending);
  return pending;
}

async function dropProviderRudpPeer(endpoint) {
  const key = String(endpoint || "").trim();
  const peerOrPromise = providerRudpPeers.get(key);
  providerRudpPeers.delete(key);
  if (!peerOrPromise) return;
  try {
    const peer = await peerOrPromise;
    if (typeof peer?.close === "function") peer.close();
  } catch {
    // The peer is already unusable; removing it is the repair.
  }
}

function closeProviderRudpPeers() {
  for (const peerOrPromise of providerRudpPeers.values()) {
    Promise.resolve(peerOrPromise)
      .then((peer) => {
        if (typeof peer?.close === "function") peer.close();
      })
      .catch(() => {});
  }
  providerRudpPeers.clear();
}

function normalizeProviderDocumentPayload(schemaId, value) {
  const documentSchema = Array.isArray(value) ? value[0] : value?.schema;
  if (
    schemaMatches(schemaId, "gamecult.fields.splats.v1") ||
    schemaMatches(documentSchema, "gamecult.fields.splats.v1")
  ) {
    return normalizeRenderSplatsViewport(value);
  }
  if (
    schemaMatches(schemaId, "gamecult.fields.gravity.v1") ||
    schemaMatches(documentSchema, "gamecult.fields.gravity.v1")
  ) {
    return normalizeGravityViewport(value);
  }
  if (
    schemaMatches(schemaId, "gamecult.fields.objects.v1") ||
    schemaMatches(documentSchema, "gamecult.fields.objects.v1")
  ) {
    return normalizeObjectsViewport(value);
  }
  return value;
}

function schemaMatches(schemaId, expected) {
  return String(schemaId || "") === expected;
}

function normalizeRenderSplatsViewport(value) {
  if (!Array.isArray(value)) return value;
  const splats = Array.isArray(value[9]) ? value[9] : [];
  return {
    schema: value[0],
    frameId: value[1],
    publishedAtUtc: value[2],
    simulationTimeSeconds: value[3],
    runId: value[4],
    zoneIndex: value[5],
    zoneName: value[6],
    viewport: normalizeViewport(value[7]),
    layers: Array.isArray(value[8]) ? value[8].map(normalizeRenderSplatLayer) : [],
    splats: {
      count: splats[0] ?? 0,
      centerX: splats[1] || [],
      centerY: splats[2] || [],
      halfExtentX: splats[3] || [],
      halfExtentY: splats[4] || [],
      rotationCos: splats[5] || [],
      rotationSin: splats[6] || [],
      channel: splats[7] || [],
      falloff: splats[8] || [],
      valueR: splats[9] || [],
      valueG: splats[10] || [],
      valueB: splats[11] || [],
      valueA: splats[12] || [],
      sourceKey: splats[13] || [],
      layerIndex: splats[14] || [],
      sourceKind: splats[15] || [],
      frequencyX: splats[16] || [],
      frequencyY: splats[17] || [],
      phaseX: splats[18] || [],
      phaseY: splats[19] || [],
      animationSpeed: splats[20] || [],
      sourceFlags: splats[21] || [],
    },
  };
}

function normalizeViewport(value) {
  return Array.isArray(value)
    ? { minX: value[0], minY: value[1], maxX: value[2], maxY: value[3] }
    : value;
}

function normalizeRenderSplatLayer(value) {
  if (!Array.isArray(value)) return value;
  return {
    layerKey: value[0],
    displayName: value[1],
    channel: value[2],
    blendMode: value[3],
    graphicsFormat: value[4],
    clearBeforeDraw: value[5],
    clearR: value[6],
    clearG: value[7],
    clearB: value[8],
    clearA: value[9],
  };
}

function normalizeGravityViewport(value) {
  if (!Array.isArray(value)) return value;
  return {
    schema: value[0],
    frameId: value[1],
    publishedAtUtc: value[2],
    simulationTimeSeconds: value[3],
    runId: value[4],
    zoneIndex: value[5],
    zoneName: value[6],
    viewport: normalizeViewport(value[7]),
    gravityInfluences: Array.isArray(value[8]) ? value[8].map(normalizeGravityInfluence) : [],
    bodies: Array.isArray(value[9]) ? value[9].map(normalizeBodyView) : [],
    terrainRadius: value[10],
    terrainDepth: value[11],
    terrainDepthExponent: value[12],
    terrainWaveFrequency: value[13],
  };
}

function normalizeObjectsViewport(value) {
  if (!Array.isArray(value)) return value;
  return {
    schema: value[0],
    frameId: value[1],
    publishedAtUtc: value[2],
    simulationTimeSeconds: value[3],
    runId: value[4],
    zoneIndex: value[5],
    zoneName: value[6],
    currentEntityKey: value[7],
    viewport: normalizeViewport(value[8]),
    controlledEntityIndices: Array.isArray(value[9]) ? value[9] : [],
    objects: Array.isArray(value[10]) ? value[10].map(normalizeViewportObject) : [],
  };
}

function normalizeGravityInfluence(value) {
  if (!Array.isArray(value)) return value;
  return {
    bodyKey: value[0],
    orbitKey: value[1],
    kind: value[2],
    x: value[3],
    y: value[4],
    radius: value[5],
    gravityDepth: value[6],
    gravityDepthExponent: value[7],
    waveRadius: value[8],
    waveDepth: value[9],
    waveSpeed: value[10],
  };
}

function normalizeBodyView(value) {
  if (!Array.isArray(value)) return value;
  return {
    bodyKey: value[0],
    orbitKey: value[1],
    name: value[2],
    kind: value[3],
    x: value[4],
    y: value[5],
    radius: value[6],
    isAsteroidBelt: value[7],
    body: value[8],
    iconAsset: normalizeAssetRef(value[9]),
  };
}

function normalizeViewportObject(value) {
  if (!Array.isArray(value)) return value;
  return {
    entityIndex: value[0],
    entityKey: value[1],
    displayName: value[2],
    kind: value[3],
    factionKey: value[4],
    x: value[5],
    y: value[6],
    z: value[7],
    directionX: value[8],
    directionY: value[9],
    velocityX: value[10],
    velocityY: value[11],
    controlled: value[12],
    targetEntityIndex: value[13],
    isActive: value[14],
    visibility: value[15],
    status: normalizeEntityStatus(value[16]),
    inventory: Array.isArray(value[17]) ? value[17].map(normalizeInventoryItem) : [],
    iconAsset: normalizeAssetRef(value[18]),
  };
}

function normalizeEntityStatus(value) {
  if (!Array.isArray(value)) return value || {};
  return {
    hull: value[0],
    shield: value[1],
    heat: value[2],
  };
}

function normalizeInventoryItem(value) {
  if (!Array.isArray(value)) return value;
  return {
    source: value[0],
    itemKey: value[1],
    quantity: value[2],
    quality: value[3],
    durability: value[4],
    enabled: value[5],
    sourceIndex: value[6],
    x: value[7],
    y: value[8],
    iconAsset: normalizeAssetRef(value[9]),
  };
}

function normalizeAssetRef(value) {
  if (!Array.isArray(value)) return value || {};
  return {
    assetKey: value[0],
    kind: value[1],
    uri: value[2],
    transport: value[3],
    contentHash: value[4],
    mimeType: value[5],
    metadata: value[6] || {},
  };
}

function findProviderSnapshotRoute(catalog, providerId) {
  const normalized = String(providerId || "").trim();
  const providers = (catalog.providers || []).filter((provider) => {
    if (!normalized) return true;
    return provider.id === normalized ||
      provider.id.includes(normalized) ||
      normalized.includes(provider.id);
  });
  const candidates = normalized ? providers : (catalog.providers || []);
  for (const provider of candidates) {
    const endpoints = [...(provider.endpoints || []), ...(provider.routes || [])];
    const route = endpoints.find((endpoint) => {
      const address = String(endpoint.uri || endpoint.endpoint || endpoint.address || "");
      const role = [endpoint.id, endpoint.role, endpoint.resolver, ...(endpoint.tags || [])].join(" ").toLowerCase();
      return address.startsWith("rudp://") && (role.includes("snapshot") || role.includes("provider-cultmesh-rudp"));
    }) || endpoints.find((endpoint) => {
      const address = String(endpoint.uri || endpoint.endpoint || endpoint.address || "");
      return address.startsWith("rudp://");
    });
    if (route) {
      return {
        providerId: provider.id,
        endpoint: route.uri || route.endpoint || route.address,
      };
    }
  }
  return null;
}

function findProviderCdnRoute(catalog, assetUri = "") {
  const providerHint = providerIdFromCultMeshUri(assetUri);
  const providers = (catalog.providers || []).filter((provider) =>
    !providerHint || provider.id === providerHint || provider.id.startsWith(`${providerHint}.`) || providerHint.startsWith(`${provider.id}.`));
  for (const provider of providers) {
    const endpoints = [...(provider.endpoints || []), ...(provider.routes || [])];
    const route = endpoints.find((endpoint) => {
      const address = String(endpoint.uri || endpoint.endpoint || endpoint.address || "");
      const role = [endpoint.id, endpoint.role, endpoint.resolver, endpoint.schemaId, ...(endpoint.tags || [])].join(" ").toLowerCase();
      return (address.startsWith("cultmesh://") || address.startsWith("rudp://")) &&
        role.includes("cultmesh-cdn") &&
        (role.includes("asset_blob") || role.includes("cdn"));
    }) || endpoints.find((endpoint) => {
      const address = String(endpoint.uri || endpoint.endpoint || endpoint.address || "");
      const role = [endpoint.id, endpoint.role, endpoint.resolver, ...(endpoint.tags || [])].join(" ").toLowerCase();
      return (address.startsWith("cultmesh://") || address.startsWith("rudp://")) && role.includes("cdn");
    });
    if (route) {
      return {
        providerId: provider.id,
        endpoint: route.uri || route.endpoint || route.address,
      };
    }
  }
  return null;
}

async function requestCultNetRawSnapshotDocument(peer, schemaId, recordKey, options = {}) {
  const messageId = `${options.messageIdPrefix || "hermodr-snapshot"}:${Date.now()}:${Math.random().toString(16).slice(2)}`;
  const response = await requestCultNetRawSnapshot(peer, [recordKey], {
    schemaIds: [schemaId],
    messageId,
    timeoutMs: options.timeoutMs,
  });
  const candidates = response.documents.filter((candidate) => candidate.recordKey === recordKey);
  const document = candidates.find((candidate) => candidate.schemaId === schemaId) || candidates[0];
  if (!document) {
    throw new Error(`CultMesh peer snapshot did not return ${schemaId} at ${recordKey}.`);
  }
  return document;
}

async function requestCultNetRawSnapshotFirstDocument(peer, schemaId, recordKeys, options = {}) {
  const keys = [...new Set(recordKeys.map((key) => String(key || "").trim()).filter(Boolean))];
  const messageId = `${options.messageIdPrefix || "hermodr-snapshot"}:${Date.now()}:${Math.random().toString(16).slice(2)}`;
  const response = await requestCultNetRawSnapshot(peer, keys, {
    schemaIds: [schemaId],
    messageId,
    timeoutMs: options.timeoutMs,
  });
  const document = response.documents.find((candidate) => candidate.schemaId === schemaId) || response.documents[0];
  if (!document) {
    throw new Error(`CultMesh peer snapshot did not return ${schemaId} at ${keys.join(", ")}.`);
  }
  return document;
}

async function requestCultNetRawSnapshot(peer, recordKeys, options) {
  const messageId = options.messageId || `${options.messageIdPrefix || "hermodr-snapshot"}:${Date.now()}:${Math.random().toString(16).slice(2)}`;
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timer);
      peer.off("message", onMessage);
      peer.off("invalidMessage", onInvalidMessage);
      peer.off("error", onError);
      peer.off("close", onClose);
    };
    const rejectWith = (error) => {
      cleanup();
      reject(error);
    };
    const onMessage = (message) => {
      if (message?.schemaVersion === "cultnet.error.v0") {
        rejectWith(new Error(message.error || "CultNet peer returned an error."));
        return;
      }
      if (message?.schemaVersion !== "cultnet.snapshot_response_raw.v0" ||
          message.messageId !== messageId) {
        return;
      }
      cleanup();
      resolve(message);
    };
    const onInvalidMessage = (error) => rejectWith(error);
    const onError = (error) => rejectWith(error);
    const onClose = () => rejectWith(new Error("CultNet peer closed before snapshot response."));
    const timer = setTimeout(
      () => rejectWith(new Error(`Timed out waiting for CultNet snapshot ${messageId}.`)),
      options.timeoutMs || 4_000,
    );

    peer.on("message", onMessage);
    peer.on("invalidMessage", onInvalidMessage);
    peer.on("error", onError);
    peer.on("close", onClose);
    peer.sendSnapshotRequest({
      schemaVersion: "cultnet.snapshot_request.v0",
      messageId,
      ...(recordKeys.length > 0 ? { recordKeys: [...recordKeys] } : {}),
      schemaIds: Array.isArray(options.schemaIds) ? [...options.schemaIds] : undefined,
    });
  });
}

function bufferFromMessagePackedBytes(payload) {
  const decoded = decodeMessagePack(bufferFromPayload(payload));
  return bufferFromPayload(decoded);
}

function bufferFromPayload(payload) {
  if (Buffer.isBuffer(payload)) return payload;
  if (payload instanceof Uint8Array) return Buffer.from(payload);
  if (Array.isArray(payload)) return Buffer.from(payload);
  if (payload && typeof payload === "object") {
    const values = Object.keys(payload)
      .sort((left, right) => Number(left) - Number(right))
      .map((key) => payload[key]);
    if (values.every((value) => Number.isInteger(value) && value >= 0 && value <= 255)) {
      return Buffer.from(values);
    }
  }
  throw new Error("CultMesh CDN asset payload was not binary.");
}

function contentTypeFromTags(tags) {
  const tag = (Array.isArray(tags) ? tags : []).find((candidate) => String(candidate).startsWith("mime:"));
  return tag ? String(tag).slice("mime:".length) : "";
}

function normalizeCultMeshAssetUri(uri) {
  const text = String(uri || "").trim();
  if (!text) return "";
  if (text.startsWith("cultmesh://") || text.startsWith("resources://")) return text;
  throw new Error(`Asset URI '${text}' is not a provider-owned CultMesh URI.`);
}

function providerIdFromCultMeshUri(uri) {
  const match = /^cultmesh:\/\/([^/]+)/i.exec(String(uri || "").trim());
  return match ? decodeURIComponent(match[1]) : "";
}

function contentTypeForPath(filePath) {
  const ext = path.extname(filePath).toLowerCase();
  return {
    ".css": "text/css; charset=utf-8",
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".json": "application/json; charset=utf-8",
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".svg": "image/svg+xml",
  }[ext] || "application/octet-stream";
}

function stringOption(value, fallback) {
  return typeof value === "string" && value.trim() ? value.trim() : fallback;
}

function numberOption(value, fallback) {
  const parsed = Number.parseInt(String(value ?? ""), 10);
  return Number.isInteger(parsed) && parsed > 0 && parsed <= 65535 ? parsed : fallback;
}

function stringListOption(value, fallback, defaultValue = []) {
  const text = String(value ?? fallback ?? "").trim();
  if (!text) return [...defaultValue];
  return text.split(",").map((entry) => entry.trim()).filter(Boolean);
}

function resolveCultCachePackagePath() {
  const candidate = path.resolve(projectsRoot, "CultLib", "packages", "cultcache-ts", "package.json");
  if (!fs.existsSync(candidate)) {
    throw new Error(`CultCache TypeScript runtime is unavailable at ${candidate}`);
  }
  return candidate;
}

function resolveCultMeshPackagePath() {
  const candidate = path.resolve(projectsRoot, "CultLib", "packages", "cultmesh-ts", "package.json");
  if (!fs.existsSync(candidate)) {
    throw new Error(`CultMesh TypeScript runtime is unavailable at ${candidate}`);
  }
  return candidate;
}

function resolveCultCacheRuntimePath() {
  return path.resolve(projectsRoot, "CultLib", "packages", "cultcache-ts", "dist", "index.js");
}

function resolveCultMeshRuntimePath() {
  return path.resolve(projectsRoot, "CultLib", "packages", "cultmesh-ts", "dist", "index.js");
}

function printUsage() {
  console.log(`Usage:
  node src/hermodr-daemon.cjs [--host 127.0.0.1] [--port 8798] [--odin-cultmesh-uri cultmesh://odin/rendezvous/provider-catalog]

Hermodr is a browser lowering adapter over Odin/CultMesh state. It does not own
provider discovery, daemon health, or raw command transport.
`);
}

if (require.main === module) {
  main().catch((error) => {
    const statusCode = error?.statusCode || 1;
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = statusCode === 409 ? 1 : statusCode;
  });
}

module.exports = {
  createBrowserCatalog,
  findProviderCdnRoute,
  normalizeProviderAdvertisement,
  surfaceRecordKeys,
};

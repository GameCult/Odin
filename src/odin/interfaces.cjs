"use strict";

const fs = require("fs");
const { createRequire } = require("module");
const path = require("path");
const { fileURLToPath } = require("url");
const { stableId } = require("./utils.cjs");

const requireCultCacheInspection = createRequire(
  path.resolve(__dirname, "..", "..", "..", "CultLib", "packages", "cultcache-ts", "package.json"),
);
const { inspectCultCacheBytes } = requireCultCacheInspection("./dist/cult-cache-inspector.js");

function createInterfaceDiscovery({
  CultMesh,
  documents,
  interfaceBindingStores,
  liveProviderRegistry,
}) {
  async function discoverInterfaces() {
    const { interfaces } = await discoverAll();
    return interfaces;
  }

  async function discoverProviderAdvertisements() {
    const { providerAdvertisements } = await discoverAll();
    return providerAdvertisements;
  }

  async function discoverAll() {
    const { interfaces: cultMeshInterfaces, providerAdvertisements } = await discoverCultMeshEntries();
    const liveAnnouncements = liveProviderRegistry?.snapshot?.() || {
      interfaces: [],
      providerAdvertisements: [],
    };

    const interfaces = [];
    for (const entry of [...cultMeshInterfaces, ...liveAnnouncements.interfaces]) {
      const existingIndex = interfaces.findIndex((candidate) => candidate.providerId === entry.providerId);
      if (existingIndex >= 0) {
        interfaces[existingIndex] = entry;
      } else {
        interfaces.push(entry);
      }
    }
    interfaces.sort((left, right) => left.providerId.localeCompare(right.providerId));
    const allProviderAdvertisements = [...providerAdvertisements, ...liveAnnouncements.providerAdvertisements];
    const providersById = new Map();
    for (const provider of allProviderAdvertisements) {
      if (provider?.id) {
        const existing = providersById.get(String(provider.id));
        providersById.set(String(provider.id), mergeProviderAdvertisement(existing, provider));
      }
    }
    return {
      interfaces,
      providerAdvertisements: [...providersById.values()].sort((left, right) => String(left.id).localeCompare(String(right.id))),
    };
  }

  function cultMeshDocumentsAvailable() {
    const {
      interfaceBindingDefinition,
      idunnCommandBoundaryDefinition,
      idunnDaemonHealthDefinition,
      idunnTransportProfileDefinition,
      muninnCaptureStreamDefinition,
      muninnCommandBoundaryDefinition,
      muninnMoveControllerStateDefinition,
      muninnMoveIdentityDefinition,
      muninnMoveLightCommandDefinition,
      muninnMoveMarkerCandidateDefinition,
      muninnObsStreamCatalogDefinition,
      muninnQuestAccessDefinition,
      muninnTelemetrySurfaceDefinition,
      muninnTransportProfileDefinition,
      operatorStateDefinition,
      providerAdvertisementDefinition,
      voidbotProviderCatalogDefinition,
      stonksCommandBoundaryDefinition,
      stonksMarketSnapshotDefinition,
      stonksRequestEventDefinition,
      stonksTransportProfileDefinition,
      streamPixelsCommandBoundaryDefinition,
      streamPixelsTransportProfileDefinition,
      surfaceDefinition,
      viliCommandBoundaryDefinition,
      viliTransportProfileDefinition,
      weksaCommandBoundaryDefinition,
      weksaOperatorStateDefinition,
      weksaTransportProfileDefinition,
      voidbotSwarmSnapshotDefinition,
    } = documents;
    return Boolean(
      CultMesh &&
      interfaceBindingDefinition &&
      idunnCommandBoundaryDefinition &&
      idunnDaemonHealthDefinition &&
      idunnTransportProfileDefinition &&
      muninnCaptureStreamDefinition &&
      muninnCommandBoundaryDefinition &&
      muninnMoveControllerStateDefinition &&
      muninnMoveIdentityDefinition &&
      muninnMoveLightCommandDefinition &&
      muninnMoveMarkerCandidateDefinition &&
      muninnObsStreamCatalogDefinition &&
      muninnQuestAccessDefinition &&
      muninnTelemetrySurfaceDefinition &&
      muninnTransportProfileDefinition &&
      operatorStateDefinition &&
      providerAdvertisementDefinition &&
      voidbotProviderCatalogDefinition &&
      stonksCommandBoundaryDefinition &&
      stonksMarketSnapshotDefinition &&
      stonksRequestEventDefinition &&
      stonksTransportProfileDefinition &&
      streamPixelsCommandBoundaryDefinition &&
      streamPixelsTransportProfileDefinition &&
      surfaceDefinition &&
      viliCommandBoundaryDefinition &&
      viliTransportProfileDefinition &&
      weksaCommandBoundaryDefinition &&
      weksaOperatorStateDefinition &&
      weksaTransportProfileDefinition &&
      voidbotSwarmSnapshotDefinition
    );
  }

  async function discoverCultMeshEntries() {
    const {
      interfaceBindingDefinition,
      idunnCommandBoundaryDefinition,
      idunnDaemonHealthDefinition,
      idunnTransportProfileDefinition,
      muninnCaptureStreamDefinition,
      muninnCommandBoundaryDefinition,
      muninnMoveControllerStateDefinition,
      muninnMoveIdentityDefinition,
      muninnMoveLightCommandDefinition,
      muninnMoveMarkerCandidateDefinition,
      muninnObsStreamCatalogDefinition,
      muninnQuestAccessDefinition,
      muninnTelemetrySurfaceDefinition,
      muninnTransportProfileDefinition,
      operatorStateDefinition,
      providerAdvertisementDefinition,
      voidbotProviderCatalogDefinition,
      stonksCommandBoundaryDefinition,
      stonksMarketSnapshotDefinition,
      stonksRequestEventDefinition,
      stonksTransportProfileDefinition,
      streamPixelsCommandBoundaryDefinition,
      streamPixelsTransportProfileDefinition,
      surfaceDefinition,
      viliCommandBoundaryDefinition,
      viliTransportProfileDefinition,
      weksaCommandBoundaryDefinition,
      weksaOperatorStateDefinition,
      weksaTransportProfileDefinition,
      voidbotSwarmSnapshotDefinition,
    } = documents;
    if (!cultMeshDocumentsAvailable()) {
      return { interfaces: [], providerAdvertisements: [] };
    }

    const nodeDocuments = [
      voidbotSwarmSnapshotDefinition,
      stonksRequestEventDefinition,
      stonksMarketSnapshotDefinition,
      providerAdvertisementDefinition,
      voidbotProviderCatalogDefinition,
      interfaceBindingDefinition,
      surfaceDefinition,
      idunnCommandBoundaryDefinition,
      idunnTransportProfileDefinition,
      stonksCommandBoundaryDefinition,
      stonksTransportProfileDefinition,
      streamPixelsCommandBoundaryDefinition,
      streamPixelsTransportProfileDefinition,
      idunnDaemonHealthDefinition,
      muninnCaptureStreamDefinition,
      muninnCommandBoundaryDefinition,
      muninnMoveControllerStateDefinition,
      muninnMoveIdentityDefinition,
      muninnMoveLightCommandDefinition,
      muninnMoveMarkerCandidateDefinition,
      muninnObsStreamCatalogDefinition,
      muninnQuestAccessDefinition,
      muninnTelemetrySurfaceDefinition,
      muninnTransportProfileDefinition,
      operatorStateDefinition,
      viliCommandBoundaryDefinition,
      viliTransportProfileDefinition,
      weksaOperatorStateDefinition,
      weksaCommandBoundaryDefinition,
      weksaTransportProfileDefinition,
    ];

    const storeEntries = await Promise.all(interfaceBindingStores.map(async (storeSpec) => {
      let resolvedStore = null;
      try {
        resolvedStore = resolveInterfaceBindingStore(storeSpec);
        if (!resolvedStore) {
          return { interfaces: [], providerAdvertisements: [] };
        }
        const { localPath: storePath, sourceId } = resolvedStore;
        if (!fs.existsSync(storePath)) {
          return { interfaces: [], providerAdvertisements: [] };
        }
        const node = await CultMesh.createNode(storePath, { documents: nodeDocuments });
        const typed = inspectCultMeshStore({
          documents,
          interfaceBindingDefinition,
          muninnTelemetrySurfaceDefinition,
          node,
          providerAdvertisementDefinition,
          sourceId,
          storePath,
        });
        const inspected = inspectCultMeshStoreBytes(storePath, sourceId);
        return {
          interfaces: mergeInterfaces([...typed.interfaces, ...inspected.interfaces]),
          providerAdvertisements: mergeProviderAdvertisements([
            ...typed.providerAdvertisements,
            ...inspected.providerAdvertisements,
          ]),
        };
      } catch (error) {
        const interfaces = [];
        const providerAdvertisements = [];
        if (resolvedStore?.localPath && fs.existsSync(resolvedStore.localPath)) {
          try {
            const inspected = inspectCultMeshStoreBytes(resolvedStore.localPath, resolvedStore.sourceId);
            interfaces.push(...inspected.interfaces);
            providerAdvertisements.push(...inspected.providerAdvertisements);
          } catch (fallbackError) {
            if (fallbackError?.code !== "ENOENT") {
              throw fallbackError;
            }
          }
        }
        if (interfaces.length === 0) {
          interfaces.push(dashboardUnavailable(`cultmesh:${storeSpec}`, `cultmesh:${storeSpec}`, error.message));
        }
        return { interfaces, providerAdvertisements };
      } finally {
        resolvedStore?.cleanup?.();
      }
    }));

    return {
      interfaces: storeEntries.flatMap((entry) => entry.interfaces),
      providerAdvertisements: storeEntries.flatMap((entry) => entry.providerAdvertisements),
    };
  }

  return {
    discoverAll,
    discoverProviderAdvertisements,
    discoverInterfaces,
  };
}

function cultMeshProviderInterface({
  advertisement,
  state,
  storePath,
  operatorState = null,
  commandBoundary = null,
  transportProfile = null,
}) {
  const surface = state.surface || null;
  const providerId = advertisement.providerId || advertisement.provider?.id || advertisement.id || "unknown-provider";
  const title = state.title || advertisement.title || advertisement.provider?.title || providerId;
  const endpoints = sanitizeProviderEndpoints(advertisement.endpoints);
  const routes = sanitizeProviderEndpoints(advertisement.routes);
  return {
    providerId,
    title,
    state: "active",
    detail: `${surface?.root?.kind || surface?.root?.type || "surface"} ${countSurfaceNodes(surface, state)} nodes via provider advertisement`,
    version: state.version || 0,
    updatedAt: state.updatedAt || advertisement.updatedAt || new Date().toISOString(),
    source: `cultmesh:${storePath}`,
    manifest: {
      ...advertisement,
      endpoints,
      routes,
    },
    canonicalService: advertisement.canonicalService || null,
    locatedService: advertisement.locatedService || null,
    cultMeshAddress: advertisement.cultMeshAddress || null,
    endpoints,
    routes,
    operatorState,
    commandBoundary,
    transportProfile,
    surface,
  };
}

function inspectCultMeshStore({
  documents,
  interfaceBindingDefinition,
  muninnTelemetrySurfaceDefinition,
  node,
  providerAdvertisementDefinition,
  sourceId,
}) {
  const {
    idunnCommandBoundaryDefinition,
    idunnTransportProfileDefinition,
    operatorStateDefinition,
    stonksCommandBoundaryDefinition,
    stonksTransportProfileDefinition,
    streamPixelsCommandBoundaryDefinition,
    streamPixelsTransportProfileDefinition,
    surfaceDefinition,
    viliCommandBoundaryDefinition,
    viliTransportProfileDefinition,
    weksaCommandBoundaryDefinition,
    weksaOperatorStateDefinition,
    weksaTransportProfileDefinition,
    muninnCommandBoundaryDefinition,
    muninnTransportProfileDefinition,
  } = documents;

  const providerAdvertisements = [];
  const interfaces = [];
  const advertisements = typeof node.cache?.getAll === "function"
    ? node.cache.getAll(providerAdvertisementDefinition).map(unwrapDocumentRecord)
    : [];

  for (const advertisement of advertisements) {
    const providerId = advertisement?.providerId || advertisement?.provider?.id || advertisement?.id;
    if (!providerId) {
      continue;
    }
    const endpoints = sanitizeProviderEndpoints(advertisement.endpoints);
    const routes = sanitizeProviderEndpoints(advertisement.routes);
    const providerEndpoint = advertisement.cultMeshAddress
      || providerTypedEndpoint(routes)
      || providerTypedEndpoint(endpoints)
      || typedProviderEndpoint(advertisement.provider?.endpoint)
      || `cultmesh:${sourceId}`;
    providerAdvertisements.push({
      id: providerId,
      title: advertisement.title || advertisement.provider?.title || providerId,
      description: advertisement.description || "Provider-owned CultMesh advertisement.",
      version: String(advertisement.version || 0),
      endpoint: providerEndpoint,
      canonicalService: advertisement.canonicalService || null,
      locatedService: advertisement.locatedService || null,
      cultMeshAddress: advertisement.cultMeshAddress || null,
      endpoints,
      routes,
      inputStreams: Array.isArray(advertisement.inputStreams) ? advertisement.inputStreams : [],
      activeStreams: Array.isArray(advertisement.activeStreams) ? advertisement.activeStreams : [],
      availableSources: Array.isArray(advertisement.availableSources) ? advertisement.availableSources : [],
      transportEndpoint: providerTypedEndpoint(routes)
        || typedProviderEndpoint(advertisement.provider?.endpoint)
        || providerEndpoint,
      capabilities: advertisement.provider?.capabilities || advertisement.capabilities || [],
      operatorState: providerRecord(node, [
        operatorStateDefinition,
        weksaOperatorStateDefinition,
      ], advertisement) || null,
      commandBoundary: providerRecord(node, [
        idunnCommandBoundaryDefinition,
        muninnCommandBoundaryDefinition,
        viliCommandBoundaryDefinition,
        weksaCommandBoundaryDefinition,
        stonksCommandBoundaryDefinition,
        streamPixelsCommandBoundaryDefinition,
      ], advertisement) || null,
      transportProfile: providerRecord(node, [
        idunnTransportProfileDefinition,
        muninnTransportProfileDefinition,
        viliTransportProfileDefinition,
        weksaTransportProfileDefinition,
        stonksTransportProfileDefinition,
        streamPixelsTransportProfileDefinition,
      ], advertisement) || null,
      usesCultMesh: true,
      transport: advertisement.provider?.transport || "CultMesh provider advertisement",
      status: advertisement.status || "unknown",
      updatedAt: advertisement.updatedAt || new Date().toISOString(),
      source: `cultmesh:${sourceId}`,
      commandSurface: advertisement.commandSurface || null,
    });

    const state = providerSurfaceState(
      node,
      advertisement,
      surfaceDefinition,
      muninnTelemetrySurfaceDefinition,
    );
    if (state?.surface?.root) {
      interfaces.push(cultMeshProviderInterface({
        advertisement,
        state,
        storePath: sourceId,
        operatorState: providerRecord(node, [
          operatorStateDefinition,
          weksaOperatorStateDefinition,
        ], advertisement) || null,
        commandBoundary: providerRecord(node, [
          idunnCommandBoundaryDefinition,
          muninnCommandBoundaryDefinition,
          viliCommandBoundaryDefinition,
          weksaCommandBoundaryDefinition,
          stonksCommandBoundaryDefinition,
          streamPixelsCommandBoundaryDefinition,
        ], advertisement) || null,
        transportProfile: providerRecord(node, [
          idunnTransportProfileDefinition,
          muninnTransportProfileDefinition,
          viliTransportProfileDefinition,
          weksaTransportProfileDefinition,
          stonksTransportProfileDefinition,
          streamPixelsTransportProfileDefinition,
        ], advertisement) || null,
      }));
    }
  }

  const bindings = typeof node.cache?.getAll === "function"
    ? node.cache.getAll(interfaceBindingDefinition).map(unwrapDocumentRecord)
    : [unwrapDocumentRecord(node.get(interfaceBindingDefinition, "voidbot.swarm"))].filter(Boolean);
  for (const binding of bindings) {
    if (!binding?.providerId) {
      continue;
    }
    const state = unwrapDocumentRecord(node.get(surfaceDefinition, binding.providerId));
    const surface = state?.surface || binding.surface || null;
    interfaces.push({
      providerId: binding.providerId,
      title: binding.title || state?.title || binding.providerId,
      state: "active",
      detail: `${surface?.root?.kind || binding.kind || "surface"} ${countSurfaceNodes(surface, state)} nodes via CultMesh`,
      version: state?.version || 0,
      updatedAt: state?.updatedAt || binding.updatedAt || new Date().toISOString(),
      source: `cultmesh:${sourceId}`,
      manifest: binding.provider || null,
      canonicalService: binding.provider?.canonicalService || null,
      locatedService: binding.provider?.locatedService || null,
      cultMeshAddress: binding.provider?.cultMeshAddress || binding.provider?.endpoint || null,
      endpoints: sanitizeProviderEndpoints(binding.provider?.endpoints),
      routes: sanitizeProviderEndpoints(binding.provider?.routes),
      surface,
    });
  }

  return { interfaces, providerAdvertisements };
}

function providerSurfaceState(node, advertisement, surfaceDefinition, muninnTelemetrySurfaceDefinition) {
  const surfaceKeys = [
    advertisement.surfaceId,
    advertisement.surface_id,
    advertisement.providerId,
    advertisement.provider?.id,
  ].filter(Boolean);
  for (const surfaceKey of [...new Set(surfaceKeys.map(String))]) {
    const surfaceState = unwrapDocumentRecord(node.get?.(surfaceDefinition, surfaceKey));
    const providerId = surfaceState?.providerId || surfaceState?.provider_id;
    const updatedAt = surfaceState?.updatedAt || surfaceState?.updated_at;
    if (surfaceState?.surface?.root) {
      return {
        providerId: providerId || advertisement.providerId || advertisement.provider?.id || "unknown-provider",
        title: surfaceState.title || advertisement.title || advertisement.provider?.title || advertisement.providerId || "unknown-provider",
        version: Number(surfaceState.version || 0),
        updatedAt: updatedAt || advertisement.updatedAt || new Date().toISOString(),
        surface: surfaceState.surface,
      };
    }
  }

  const telemetryState = providerTelemetrySurfaceState(node, advertisement, muninnTelemetrySurfaceDefinition);
  if (telemetryState) {
    return telemetryState;
  }
  return null;
}

function providerTelemetrySurfaceState(node, advertisement, muninnTelemetrySurfaceDefinition) {
  if (!muninnTelemetrySurfaceDefinition) {
    return null;
  }
  const keys = [
    advertisement.surfaceId,
    advertisement.surface_id,
    "latest",
  ].filter(Boolean);
  for (const key of keys) {
    const record = unwrapDocumentRecord(node.get?.(muninnTelemetrySurfaceDefinition, key));
    const telemetryState = muninnTelemetryStateFromRecord(record, advertisement);
    if (telemetryState) {
      return telemetryState;
    }
  }
  return null;
}

function providerRecord(node, definitions, advertisement) {
  const keys = providerRecordKeys(advertisement);
  for (const definition of definitions) {
    if (!definition) continue;
    for (const key of keys) {
      const record = unwrapDocumentRecord(node.get?.(definition, key));
      if (record) return record;
    }
  }
  return null;
}

function unwrapDocumentRecord(record) {
  if (Array.isArray(record) && record.length === 1 && record[0] && typeof record[0] === "object") {
    return record[0];
  }
  return record;
}

function providerRecordKeys(advertisement) {
  const keys = [
    advertisement?.commandSurface?.commandBoundaryId,
    advertisement?.commandSurface?.transportProfileId,
    advertisement?.providerId,
    advertisement?.daemonId,
    advertisement?.daemon_id,
    advertisement?.serviceId,
    advertisement?.service_id,
    advertisement?.provider?.id,
    advertisement?.canonicalService,
    advertisement?.locatedService,
  ];
  if (advertisement?.providerId === "vili.animation") keys.push("vili");
  if (advertisement?.providerId === "weksa.intent.service") keys.push("weksa");
  if (advertisement?.providerId === "stonks.market") keys.push("stonks");
  if (advertisement?.provider?.id === "streampixels.service") keys.push("streampixels", "yggdrasil-streampixels");
  return [...new Set(keys.filter(Boolean).map(String))];
}

function inspectProviderAdvertisementsFromStore(storePath, sourceId) {
  const inspection = inspectCultCacheBytes(storePath, fs.readFileSync(storePath));
  const records = inspection.records || [];
  return records
    .filter((record) => record.schemaName === "gamecult.eve.provider_advertisement")
    .map((record) => buildInspectedProviderAdvertisement(record, records, sourceId))
    .filter(Boolean);
}

function buildInspectedProviderAdvertisement(record, records, sourceId) {
  const advertisement = normalizeInspectedProviderAdvertisement(record?.payloadPreview);
  const providerId = advertisement?.providerId;
  if (!providerId) {
    return null;
  }
  const commandBoundary = inspectProviderCommandBoundary(providerId, records);
  const transportProfile = inspectProviderTransportProfile(providerId, records);
  const endpoints = sanitizeProviderEndpoints(advertisement.endpoints);
  const routes = sanitizeProviderEndpoints(normalizeInspectedRoutes(advertisement));
  return {
    id: providerId,
    title: advertisement.title || providerId,
    description: advertisement.description || "Provider-owned CultMesh advertisement.",
    version: String(advertisement.version || 0),
    endpoint: advertisement.cultMeshAddress || providerTypedEndpoint(endpoints) || `cultmesh:${sourceId}`,
    canonicalService: advertisement.canonicalService || null,
    locatedService: advertisement.locatedService || null,
    cultMeshAddress: advertisement.cultMeshAddress || null,
    endpoints,
    routes,
    inputStreams: Array.isArray(advertisement.inputStreams) ? advertisement.inputStreams : [],
    activeStreams: Array.isArray(advertisement.activeStreams) ? advertisement.activeStreams : [],
    availableSources: Array.isArray(advertisement.availableSources) ? advertisement.availableSources : [],
    transportEndpoint: advertisement.cultMeshAddress || providerTypedEndpoint(routes) || providerTypedEndpoint(endpoints) || `cultmesh:${sourceId}`,
    capabilities: advertisement.capabilities || [],
    operatorState: null,
    commandBoundary,
    transportProfile,
    usesCultMesh: true,
    transport: "CultMesh provider advertisement",
    status: advertisement.status || "unknown",
    updatedAt: advertisement.updatedAt || new Date().toISOString(),
    source: `cultmesh:${sourceId}`,
    commandSurface: advertisement.commandSurface || null,
  };
}

function inspectCultMeshInterfacesFromStore(storePath, sourceId) {
  const inspection = inspectCultCacheBytes(storePath, fs.readFileSync(storePath));
  const records = inspection.records || [];
  const providersById = new Map(
    records
      .filter((record) => record.schemaName === "gamecult.eve.provider_advertisement")
      .map((record) => buildInspectedProviderAdvertisement(record, records, sourceId))
      .filter(Boolean)
      .map((provider) => [provider.id, provider]),
  );
  const surfaceInterfaces = records
    .filter((record) => record.schemaName === "gamecult.eve.surface_state")
    .map((record) => buildInspectedInterfaceFromSurfaceRecord(
      record,
      providersById,
      sourceId,
      records,
    ))
    .filter(Boolean);
  const telemetryInterfaces = records
    .filter((record) => record.schemaName === "muninn.telemetry_surface")
    .map((record) => buildInspectedInterfaceFromTelemetryRecord(
      record,
      providersById,
      sourceId,
      records,
    ))
    .filter(Boolean);
  return [...surfaceInterfaces, ...telemetryInterfaces];
}

function inspectCultMeshStoreBytes(storePath, sourceId) {
  return {
    interfaces: inspectCultMeshInterfacesFromStore(storePath, sourceId),
    providerAdvertisements: inspectProviderAdvertisementsFromStore(storePath, sourceId),
  };
}

function mergeInterfaces(entries) {
  const byProvider = new Map();
  for (const entry of entries) {
    if (!entry?.providerId) continue;
    const existing = byProvider.get(String(entry.providerId));
    if (!existing || (!existing.surface?.root && entry.surface?.root)) {
      byProvider.set(String(entry.providerId), entry);
    }
  }
  return [...byProvider.values()];
}

function mergeProviderAdvertisements(entries) {
  const byProvider = new Map();
  for (const entry of entries) {
    if (!entry?.id) continue;
    const existing = byProvider.get(String(entry.id));
    byProvider.set(String(entry.id), mergeProviderAdvertisement(existing, entry));
  }
  return [...byProvider.values()];
}

function mergeProviderAdvertisement(existing, entry) {
  if (!existing) return entry;
  return {
    ...existing,
    ...entry,
    endpoints: preferNonEmptyArray(entry.endpoints, existing.endpoints),
    routes: preferNonEmptyArray(entry.routes, existing.routes),
    inputStreams: preferNonEmptyArray(entry.inputStreams, existing.inputStreams),
    activeStreams: preferNonEmptyArray(entry.activeStreams, existing.activeStreams),
    availableSources: preferNonEmptyArray(entry.availableSources, existing.availableSources),
    capabilities: preferNonEmptyArray(entry.capabilities, existing.capabilities),
  };
}

function preferNonEmptyArray(candidate, fallback) {
  return Array.isArray(candidate) && candidate.length > 0
    ? candidate
    : (Array.isArray(fallback) ? fallback : []);
}

function buildInspectedInterfaceFromSurfaceRecord(record, providersById, sourceId, records) {
  const state = normalizeInspectedSurfaceState(record?.payloadPreview);
  if (!state?.providerId || !state.surface?.root) {
    return null;
  }
  const provider = providersById.get(state.providerId) || null;
  return {
    providerId: state.providerId,
    title: state.title || provider?.title || state.providerId,
    state: "active",
    detail: `${state.surface.root.kind || "surface"} ${countSurfaceNodes(state.surface)} nodes via CultMesh witness inspection`,
    version: state.version || 0,
    updatedAt: state.updatedAt || provider?.updatedAt || new Date().toISOString(),
    source: `cultmesh:${sourceId}`,
    manifest: provider ? {
      providerId: provider.id,
      title: provider.title,
      description: provider.description,
      canonicalService: provider.canonicalService,
      locatedService: provider.locatedService,
      cultMeshAddress: provider.cultMeshAddress,
      endpoints: sanitizeProviderEndpoints(provider.endpoints),
      routes: sanitizeProviderEndpoints(provider.routes),
    } : null,
    canonicalService: provider?.canonicalService || null,
    locatedService: provider?.locatedService || null,
    cultMeshAddress: provider?.cultMeshAddress || null,
    endpoints: sanitizeProviderEndpoints(provider?.endpoints),
    routes: sanitizeProviderEndpoints(provider?.routes),
    operatorState: null,
    commandBoundary: provider?.commandBoundary || inspectProviderCommandBoundary(state.providerId, records),
    transportProfile: provider?.transportProfile || inspectProviderTransportProfile(state.providerId, records),
    surface: state.surface,
  };
}

function buildInspectedInterfaceFromTelemetryRecord(record, providersById, sourceId, records) {
  const telemetry = normalizeInspectedMuninnTelemetrySurfaceRecord(record?.payloadPreview);
  if (!telemetry?.hostId) {
    return null;
  }
  const providerId = `muninn.telemetry.${telemetry.hostId}`;
  const provider = providersById.get(providerId) || null;
  const state = muninnTelemetryStateFromRecord(telemetry, provider);
  if (!state?.surface?.root) {
    return null;
  }
  return {
    providerId: state.providerId,
    title: state.title,
    state: "active",
    detail: `${state.surface.root.kind || "surface"} ${countSurfaceNodes(state.surface)} nodes via Muninn telemetry surface`,
    version: state.version || 0,
    updatedAt: state.updatedAt || provider?.updatedAt || new Date().toISOString(),
    source: `cultmesh:${sourceId}`,
    manifest: provider ? {
      providerId: provider.id,
      title: provider.title,
      description: provider.description,
      canonicalService: provider.canonicalService,
      locatedService: provider.locatedService,
      cultMeshAddress: provider.cultMeshAddress,
      endpoints: sanitizeProviderEndpoints(provider.endpoints),
      routes: sanitizeProviderEndpoints(provider.routes),
    } : null,
    canonicalService: provider?.canonicalService || null,
    locatedService: provider?.locatedService || null,
    cultMeshAddress: provider?.cultMeshAddress || null,
    endpoints: sanitizeProviderEndpoints(provider?.endpoints),
    routes: sanitizeProviderEndpoints(provider?.routes),
    operatorState: null,
    commandBoundary: provider?.commandBoundary || null,
    transportProfile: provider?.transportProfile || null,
    surface: state.surface,
  };
}

function muninnTelemetryStateFromRecord(record, advertisement = null) {
  const telemetry = normalizeMuninnTelemetrySurfaceRecord(record);
  if (!telemetry?.hostId) {
    return null;
  }
  const providerId = advertisement?.providerId || advertisement?.id || `muninn.telemetry.${telemetry.hostId}`;
  const title = advertisement?.title || advertisement?.provider?.title || `Muninn ${titleCase(telemetry.hostId)} Telemetry`;
  const updatedAt = telemetry.updatedAt || advertisement?.updatedAt || new Date().toISOString();
  const parsedVersion = Date.parse(updatedAt);
  return {
    providerId,
    title,
    version: Number.isFinite(parsedVersion) ? parsedVersion : 0,
    updatedAt,
    surface: buildMuninnTelemetrySurfaceDocument(providerId, title, telemetry),
  };
}

function normalizeMuninnTelemetrySurfaceRecord(record) {
  if (!record) {
    return null;
  }
  if (!Array.isArray(record) && typeof record === "object") {
    return {
      surfaceId: record.surface_id || record.surfaceId || null,
      hostId: record.host_id || record.hostId || null,
      state: record.state || "unknown",
      availableSources: Array.isArray(record.available_sources || record.availableSources)
        ? (record.available_sources || record.availableSources)
        : [],
      streamAffordances: Array.isArray(record.stream_affordances || record.streamAffordances)
        ? (record.stream_affordances || record.streamAffordances)
        : [],
      activeStreams: Array.isArray(record.active_streams || record.activeStreams)
        ? (record.active_streams || record.activeStreams)
        : [],
      activationAuthority: record.activation_authority || record.activationAuthority || "",
      detail: record.detail || "",
      updatedAt: record.updated_at || record.updatedAt || null,
    };
  }
  return normalizeInspectedMuninnTelemetrySurfaceRecord(record);
}

function normalizeInspectedMuninnTelemetrySurfaceRecord(preview) {
  if (Array.isArray(preview) && preview.length === 1 && preview[0] && typeof preview[0] === "object" && !Array.isArray(preview[0])) {
    return normalizeMuninnTelemetrySurfaceRecord(preview[0]);
  }
  if (!Array.isArray(preview) || preview.length < 9) {
    return null;
  }
  return {
    surfaceId: preview[0] || null,
    hostId: preview[1] || null,
    state: preview[2] || "unknown",
    availableSources: Array.isArray(preview[3]) ? preview[3] : [],
    streamAffordances: Array.isArray(preview[4]) ? preview[4] : [],
    activeStreams: Array.isArray(preview[5]) ? preview[5] : [],
    activationAuthority: preview[6] || "",
    detail: preview[7] || "",
    updatedAt: preview[8] || null,
  };
}

function buildMuninnTelemetrySurfaceDocument(providerId, title, telemetry) {
  const sourceSummary = telemetry.availableSources.length ? telemetry.availableSources.join(", ") : "none";
  const activeSummary = telemetry.activeStreams.length ? telemetry.activeStreams.join(", ") : "none";
  const affordanceSummary = telemetry.streamAffordances.length ? telemetry.streamAffordances.join(", ") : "none";
  return {
    schema: "gamecult.eve.surface.v1",
    id: telemetry.surfaceId || `${providerId}.surface`,
    title,
    root: {
      id: `${providerId}.root`,
      kind: "dashboard",
      props: {
        title,
        summary: telemetry.detail || `${telemetry.state} on ${telemetry.hostId}`,
        status: telemetry.state || "unknown",
      },
      children: [
        {
          id: `${providerId}.runtime`,
          kind: "card",
          props: { title: "Runtime" },
          children: [
            textElement(`${providerId}.state`, `state: ${telemetry.state || "unknown"}`),
            textElement(`${providerId}.host`, `host: ${telemetry.hostId}`),
            textElement(`${providerId}.authority`, `activation: ${telemetry.activationAuthority || "unknown"}`),
            textElement(`${providerId}.updated`, `updated: ${telemetry.updatedAt || "unknown"}`),
          ],
        },
        {
          id: `${providerId}.sources`,
          kind: "card",
          props: { title: "Sources" },
          children: [
            textElement(`${providerId}.available-sources`, `available: ${sourceSummary}`),
            textElement(`${providerId}.affordances`, `affordances: ${affordanceSummary}`),
          ],
        },
        {
          id: `${providerId}.streams`,
          kind: "card",
          props: { title: "Streams" },
          children: [
            textElement(`${providerId}.active-streams`, `active: ${activeSummary}`),
            textElement(`${providerId}.detail`, telemetry.detail || "detail: none"),
          ],
        },
      ],
    },
    assets: [],
  };
}

function normalizeInspectedProviderAdvertisement(preview) {
  if (Array.isArray(preview) && preview.length === 1 && preview[0] && typeof preview[0] === "object" && !Array.isArray(preview[0])) {
    const value = preview[0];
    return {
      providerId: value.providerId || value.ProviderId || null,
      title: value.title || value.Title || null,
      description: value.description || value.Description || null,
      canonicalService: value.canonicalService || value.CanonicalService || null,
      locatedService: value.locatedService || value.LocatedService || null,
      cultMeshAddress: value.cultMeshAddress || value.CultMeshAddress || null,
      status: value.status || value.Status || null,
      updatedAt: value.updatedAt || value.UpdatedAt || null,
      capabilities: value.capabilities || value.CapabilityIds || [],
      endpoints: value.endpoints || value.Endpoints || [],
      commandSurface: value.commandSurface || value.CommandSurface || null,
    };
  }
  if (!Array.isArray(preview) || preview.length < 11) {
    return null;
  }
  return {
    providerId: preview[1] || null,
    serviceId: preview[2] || null,
    verseId: preview[3] || null,
    title: preview[4] || null,
    description: preview[5] || null,
    canonicalService: preview[6] || null,
    locatedService: preview[7] || null,
    cultMeshAddress: preview[8] || null,
    status: preview[9] || null,
    updatedAt: preview[10] || null,
    capabilities: Array.isArray(preview[11]) ? preview[11] : [],
    endpoints: Array.isArray(preview[12]) ? preview[12] : [],
    schemas: Array.isArray(preview[13]) ? preview[13] : [],
    witnesses: Array.isArray(preview[14]) ? preview[14] : [],
    surfaces: Array.isArray(preview[15]) ? preview[15] : [],
  };
}

function normalizeInspectedSurfaceState(preview) {
  if (Array.isArray(preview) && preview.length === 1 && preview[0] && typeof preview[0] === "object" && !Array.isArray(preview[0])) {
    const value = preview[0];
    return {
      providerId: value.providerId || value.ProviderId || null,
      title: value.title || value.Title || null,
      version: Number(value.version || value.Version || 0),
      updatedAt: value.updatedAt || value.UpdatedAt || null,
      surface: normalizeInspectedSurface(value.surface || value.Surface || null),
    };
  }
  if (!Array.isArray(preview) || preview.length < 5) {
    return null;
  }
  if (preview.length >= 6) {
    return {
      providerId: preview[1] || null,
      title: preview[2] || null,
      version: Number(preview[3] || 0),
      updatedAt: preview[4] || null,
      surface: normalizeInspectedSurface(preview[5]),
    };
  }
  return {
    providerId: preview[0] || null,
    title: preview[1] || null,
    version: Number(preview[2] || 0),
    updatedAt: preview[3] || null,
    surface: normalizeInspectedSurface(preview[4]),
  };
}

function normalizeInspectedSurface(preview) {
  if (!preview) {
    return null;
  }
  if (!Array.isArray(preview) && typeof preview === "object") {
    return {
      ...preview,
      root: normalizeInspectedSurfaceNode(preview.root),
      assets: Array.isArray(preview.assets) ? preview.assets : [],
    };
  }
  if (!Array.isArray(preview) || preview.length < 4) {
    return null;
  }
  return {
    schema: preview[0] || "gamecult.eve.surface.v1",
    id: preview[1] || "surface",
    title: preview[2] || "",
    root: normalizeInspectedSurfaceNode(preview[3]),
    assets: Array.isArray(preview[4]) ? preview[4] : [],
  };
}

function normalizeInspectedSurfaceNode(preview) {
  if (!preview) {
    return null;
  }
  if (!Array.isArray(preview) && typeof preview === "object") {
    return {
      ...preview,
      props: normalizeInspectedSurfaceProps(preview.props),
      children: Array.isArray(preview.children)
        ? preview.children.map((child) => normalizeInspectedSurfaceNode(child)).filter(Boolean)
        : [],
    };
  }
  if (!Array.isArray(preview) || preview.length < 3) {
    return null;
  }
  return {
    id: preview[0] || "node",
    kind: preview[1] || "group",
    props: normalizeInspectedSurfaceProps(preview[2]),
    children: Array.isArray(preview[3])
      ? preview[3].map((child) => normalizeInspectedSurfaceNode(child)).filter(Boolean)
      : [],
  };
}

function normalizeInspectedSurfaceProps(preview) {
  if (!preview) {
    return {};
  }
  if (!Array.isArray(preview) && typeof preview === "object") {
    return preview;
  }
  if (!Array.isArray(preview)) {
    return {};
  }
  return {
    title: preview[0] || "",
    text: preview[1] || "",
    label: preview[2] || "",
    value: preview[3] || "",
    status: preview[4] || "",
    summary: preview[5] || "",
    detail: preview[6] || "",
  };
}

function inspectProviderCommandBoundary(providerId, records) {
  if (providerId !== "nightwing-gjallar") {
    return null;
  }
  const record = records.find((entry) => entry.schemaName === "gjallar.command_boundary");
  if (!record || !Array.isArray(record.payloadPreview)) {
    return null;
  }
  return {
    boundary_id: record.payloadPreview[0] || null,
    daemon_id: record.payloadPreview[1] || null,
    owner: record.payloadPreview[2] || null,
    lifecycle_authority: record.payloadPreview[5] || null,
    updated_at: record.payloadPreview[8] || null,
  };
}

function inspectProviderTransportProfile(providerId, records) {
  if (providerId !== "nightwing-gjallar") {
    return null;
  }
  const record = records.find((entry) => entry.schemaName === "gjallar.transport_profile");
  if (!record || !Array.isArray(record.payloadPreview)) {
    return null;
  }
  return {
    profile_id: record.payloadPreview[0] || null,
    daemon_id: record.payloadPreview[1] || null,
    state: record.payloadPreview[2] || null,
    debug_transport: record.payloadPreview[3] || null,
    current_transport: record.payloadPreview[4] || null,
    health_contract: record.payloadPreview[5] || null,
    cut_line: record.payloadPreview[9] || null,
    updated_at: record.payloadPreview[10] || null,
  };
}

function normalizeInspectedEndpoints(endpoints) {
  if (!Array.isArray(endpoints)) {
    return [];
  }
  return endpoints.map((entry) => {
    if (entry && typeof entry === "object" && !Array.isArray(entry)) {
      return entry;
    }
    return {
      transport: "witness",
      address: String(entry || ""),
    };
  });
}

function sanitizeProviderEndpoints(endpoints) {
  return normalizeInspectedEndpoints(endpoints).filter((entry) => {
    const transport = String(entry?.transport || "").toLowerCase();
    const address = String(entry?.address || "").trim();
    if (address.startsWith("cultmesh:")) {
      return true;
    }
    if (transport.includes("cultmesh") || transport.includes("cultcache")) {
      return true;
    }
    return false;
  });
}

function providerTypedEndpoint(endpoints) {
  const normalized = sanitizeProviderEndpoints(endpoints);
  return normalized.find((entry) => {
    const address = String(entry?.address || "");
    return address.startsWith("cultmesh:");
  })?.address || null;
}

function typedProviderEndpoint(endpoint) {
  const value = typeof endpoint === "string" ? endpoint.trim() : "";
  if (!value) {
    return null;
  }
  return value.startsWith("cultmesh:") ? value : null;
}

function normalizeInspectedRoutes(advertisement) {
  if (Array.isArray(advertisement?.routes)) {
    return advertisement.routes;
  }
  if (Array.isArray(advertisement?.endpoints)) {
    return normalizeInspectedEndpoints(advertisement.endpoints);
  }
  return [];
}

function resolveInterfaceBindingStore(storeSpec) {
  if (typeof storeSpec !== "string" || !storeSpec.trim()) {
    return null;
  }
  const text = storeSpec.trim();
  if (text.startsWith("cultmesh-store:file://")) {
    const localPath = fileURLToPath(text.slice("cultmesh-store:".length));
    return {
      localPath,
      sourceId: text,
      cleanup: () => {},
    };
  }
  if (text.startsWith("cultmesh://")) {
    throw new Error(`CultMesh rendezvous URI ${text} must announce through Odin; static interface store imports are not the rendezvous owner.`);
  }
  if (text.startsWith("sftp://") || text.startsWith("http://") || text.startsWith("https://") || text.startsWith("rudp://")) {
    throw new Error(`network interface binding stores are no longer supported by Odin discovery: ${text}`);
  }

  throw new Error(`raw interface binding store paths are no longer supported by Odin discovery: ${text}. Use CultMesh announcement through Odin, or an explicit cultmesh-store:file:// URI for a local debug import.`);
}

function dashboardUnavailable(providerId, source, detail) {
  return {
    providerId,
    title: providerId,
    state: "unavailable",
    detail,
    version: 0,
    updatedAt: new Date().toISOString(),
    source,
    manifest: null,
    surface: null,
  };
}

function countSurfaceNodes(surface, state = null) {
  if (Array.isArray(state?.nodes)) {
    return state.nodes.length;
  }
  if (!surface?.root) {
    return 0;
  }

  let count = 0;
  const stack = [surface.root];
  while (stack.length > 0) {
    const node = stack.pop();
    if (!node || typeof node !== "object") {
      continue;
    }
    count += 1;
    if (Array.isArray(node.children)) {
      for (const child of node.children) {
        stack.push(child);
      }
    }
  }
  return count;
}

function textElement(id, text) {
  return {
    id: String(id),
    kind: "text",
    props: { text: String(text) },
    children: [],
  };
}

function titleCase(value) {
  return String(value || "")
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

module.exports = {
  createInterfaceDiscovery,
  dashboardUnavailable,
};

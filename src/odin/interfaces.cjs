"use strict";

const childProcess = require("child_process");
const fs = require("fs");
const { createRequire } = require("module");
const os = require("os");
const path = require("path");
const { httpGet, stableId } = require("./utils.cjs");
const { openWebSocket, readServerTextFrame, sendClientFrame } = require("./websocket.cjs");
const { tcpCheck } = require("./probes.cjs");

const requireCultCacheInspection = createRequire(
  path.resolve(__dirname, "..", "..", "..", "CultLib", "packages", "cultcache-ts", "package.json"),
);
const { inspectCultCacheBytes } = requireCultCacheInspection("./dist/cult-cache-inspector.js");

function createInterfaceDiscovery({
  CultMesh,
  documents,
  interfaceBindingStores,
  seedDeckUrls,
}) {
  let discoveredDeckUrls = [...seedDeckUrls];
  let lastLanScanAt = 0;

  async function discoverInterfaces() {
    await refreshLanDeckDiscovery();
    const manifestsByProvider = new Map();
    for (const deckUrl of discoveredDeckUrls) {
      const manifestUrl = deckUrl.replace(/^ws:/, "http:").replace(/\/eve\/deck.*$/, "/eve/deck/providers");
      try {
        const catalog = JSON.parse(await httpGet(manifestUrl, 2500));
        for (const provider of catalog.providers || []) {
          if (!provider?.id || provider.id === "eve.dashboard.broker") continue;
          const existing = manifestsByProvider.get(provider.id);
          if (!existing || deckUrl.startsWith("ws://127.0.0.1")) {
            manifestsByProvider.set(provider.id, { provider, deckUrl });
          }
        }
      } catch {
        // Discovery is opportunistic. Failed endpoints fall out of the next pass.
      }
    }

    const interfaces = [];
    for (const { provider, deckUrl } of manifestsByProvider.values()) {
      interfaces.push(await fetchEveProvider(deckUrl, provider.id, provider));
    }
    for (const entry of await discoverCultMeshInterfaceBindings()) {
      const existingIndex = interfaces.findIndex((candidate) => candidate.providerId === entry.providerId);
      if (existingIndex >= 0) {
        interfaces[existingIndex] = entry;
      } else {
        interfaces.push(entry);
      }
    }
    interfaces.sort((left, right) => left.providerId.localeCompare(right.providerId));
    return interfaces;
  }

  async function discoverProviderAdvertisements() {
    const {
      interfaceBindingDefinition,
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
      providerAdvertisementDefinition,
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
    if (!CultMesh || !interfaceBindingDefinition || !idunnDaemonHealthDefinition || !muninnCaptureStreamDefinition || !muninnCommandBoundaryDefinition || !muninnMoveControllerStateDefinition || !muninnMoveIdentityDefinition || !muninnMoveLightCommandDefinition || !muninnMoveMarkerCandidateDefinition || !muninnObsStreamCatalogDefinition || !muninnQuestAccessDefinition || !muninnTelemetrySurfaceDefinition || !muninnTransportProfileDefinition || !operatorStateDefinition || !stonksCommandBoundaryDefinition || !stonksMarketSnapshotDefinition || !stonksRequestEventDefinition || !stonksTransportProfileDefinition || !streamPixelsCommandBoundaryDefinition || !streamPixelsTransportProfileDefinition || !surfaceDefinition || !viliCommandBoundaryDefinition || !viliTransportProfileDefinition || !weksaCommandBoundaryDefinition || !weksaOperatorStateDefinition || !weksaTransportProfileDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
      return [];
    }

    const providers = [];
    for (const storeSpec of interfaceBindingStores) {
      let resolvedStore = null;
      try {
        resolvedStore = resolveInterfaceBindingStore(storeSpec);
        if (!resolvedStore) {
          continue;
        }
        const { localPath: storePath, sourceId } = resolvedStore;
        if (!fs.existsSync(storePath)) {
          continue;
        }
        const node = await CultMesh.createNode(storePath, {
          documents: [
            voidbotSwarmSnapshotDefinition,
            stonksRequestEventDefinition,
            stonksMarketSnapshotDefinition,
            providerAdvertisementDefinition,
            interfaceBindingDefinition,
            surfaceDefinition,
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
          ],
        });
        const advertisements = typeof node.cache?.getAll === "function"
          ? node.cache.getAll(providerAdvertisementDefinition).map(unwrapDocumentRecord)
          : [];
        for (const advertisement of advertisements) {
          const providerId = advertisement?.providerId || advertisement?.provider?.id || advertisement?.id;
          if (!providerId) {
            continue;
          }
          providers.push({
            id: providerId,
            title: advertisement.title || advertisement.provider?.title || providerId,
            description: advertisement.description || "Provider-owned CultMesh advertisement.",
            version: String(advertisement.version || 0),
            endpoint: advertisement.cultMeshAddress || advertisement.endpoints?.[0]?.address || advertisement.provider?.endpoint || `cultmesh:${sourceId}`,
            canonicalService: advertisement.canonicalService || null,
            locatedService: advertisement.locatedService || null,
            cultMeshAddress: advertisement.cultMeshAddress || null,
            endpoints: advertisement.endpoints || [],
            routes: advertisement.routes || [],
            transportEndpoint: advertisement.routes?.find((route) => route.transport === "cultnet")?.address
              || advertisement.routes?.find((route) => route.transport === "compatibility-eve-deck")?.address
              || advertisement.provider?.endpoint
              || `cultmesh:${sourceId}`,
            capabilities: advertisement.provider?.capabilities || advertisement.capabilities || [],
            operatorState: providerRecord(node, [
              operatorStateDefinition,
              weksaOperatorStateDefinition,
            ], advertisement) || null,
            commandBoundary: providerRecord(node, [
              muninnCommandBoundaryDefinition,
              viliCommandBoundaryDefinition,
              weksaCommandBoundaryDefinition,
              stonksCommandBoundaryDefinition,
              streamPixelsCommandBoundaryDefinition,
            ], advertisement) || null,
            transportProfile: providerRecord(node, [
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
        }
      } catch {
        // Provider advertisements are optional discovery hints; broken stores
        // are still surfaced through interface discovery when possible.
        if (resolvedStore?.localPath && fs.existsSync(resolvedStore.localPath)) {
          providers.push(...inspectProviderAdvertisementsFromStore(
            resolvedStore.localPath,
            resolvedStore.sourceId,
          ));
        }
      } finally {
        resolvedStore?.cleanup?.();
      }
    }

    providers.sort((left, right) => String(left.id).localeCompare(String(right.id)));
    return providers;
  }

  async function discoverCultMeshInterfaceBindings() {
    const {
      interfaceBindingDefinition,
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
      providerAdvertisementDefinition,
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
    if (!CultMesh || !interfaceBindingDefinition || !idunnDaemonHealthDefinition || !muninnCaptureStreamDefinition || !muninnCommandBoundaryDefinition || !muninnMoveControllerStateDefinition || !muninnMoveIdentityDefinition || !muninnMoveLightCommandDefinition || !muninnMoveMarkerCandidateDefinition || !muninnObsStreamCatalogDefinition || !muninnQuestAccessDefinition || !muninnTelemetrySurfaceDefinition || !muninnTransportProfileDefinition || !operatorStateDefinition || !stonksCommandBoundaryDefinition || !stonksMarketSnapshotDefinition || !stonksRequestEventDefinition || !stonksTransportProfileDefinition || !streamPixelsCommandBoundaryDefinition || !streamPixelsTransportProfileDefinition || !surfaceDefinition || !viliCommandBoundaryDefinition || !viliTransportProfileDefinition || !weksaCommandBoundaryDefinition || !weksaOperatorStateDefinition || !weksaTransportProfileDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
      return [];
    }
    const interfaces = [];
    for (const storeSpec of interfaceBindingStores) {
      let resolvedStore = null;
      try {
        resolvedStore = resolveInterfaceBindingStore(storeSpec);
        if (!resolvedStore) {
          continue;
        }
        const { localPath: storePath, sourceId } = resolvedStore;
        if (!fs.existsSync(storePath)) {
          continue;
        }
        const node = await CultMesh.createNode(storePath, {
          documents: [
            voidbotSwarmSnapshotDefinition,
            stonksRequestEventDefinition,
            stonksMarketSnapshotDefinition,
            providerAdvertisementDefinition,
            interfaceBindingDefinition,
            surfaceDefinition,
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
          ],
        });
        const advertisements = typeof node.cache?.getAll === "function"
          ? node.cache.getAll(providerAdvertisementDefinition).map(unwrapDocumentRecord)
          : [];
        for (const advertisement of advertisements) {
          if (!advertisement?.providerId) {
            continue;
          }
          const state = providerSurfaceState(
            node,
            advertisement,
            surfaceDefinition,
            muninnTelemetrySurfaceDefinition,
          );
          if (!state?.surface?.root) {
            continue;
          }
          interfaces.push(cultMeshProviderInterface({
            advertisement,
            state,
            storePath: sourceId,
            operatorState: providerRecord(node, [
              operatorStateDefinition,
              weksaOperatorStateDefinition,
            ], advertisement) || null,
            commandBoundary: providerRecord(node, [
              muninnCommandBoundaryDefinition,
              viliCommandBoundaryDefinition,
              weksaCommandBoundaryDefinition,
              stonksCommandBoundaryDefinition,
              streamPixelsCommandBoundaryDefinition,
            ], advertisement) || null,
            transportProfile: providerRecord(node, [
              muninnTransportProfileDefinition,
              viliTransportProfileDefinition,
              weksaTransportProfileDefinition,
              stonksTransportProfileDefinition,
              streamPixelsTransportProfileDefinition,
            ], advertisement) || null,
          }));
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
            endpoints: binding.provider?.endpoints || [],
            routes: binding.provider?.routes || [],
            surface,
          });
        }
      } catch (error) {
        if (resolvedStore?.localPath && fs.existsSync(resolvedStore.localPath)) {
          const fallbackInterfaces = inspectCultMeshInterfacesFromStore(
            resolvedStore.localPath,
            resolvedStore.sourceId,
          );
          if (fallbackInterfaces.length > 0) {
            interfaces.push(...fallbackInterfaces);
            continue;
          }
        }
        interfaces.push(dashboardUnavailable(`cultmesh:${storeSpec}`, `cultmesh:${storeSpec}`, error.message));
      } finally {
        resolvedStore?.cleanup?.();
      }
    }
    return interfaces;
  }

  async function refreshLanDeckDiscovery() {
    const now = Date.now();
    if (now - lastLanScanAt < 60000) return;
    lastLanScanAt = now;
    const urls = new Set(seedDeckUrls);
    const checks = [];
    for (const prefix of localIpv4Prefixes()) {
      for (let host = 1; host <= 254; host += 1) {
        const address = `${prefix}.${host}`;
        checks.push(tcpCheck(address, 8795).then((check) => {
          if (check.state === "open") urls.add(`ws://${address}:8795/eve/deck`);
        }));
      }
    }
    await Promise.allSettled(checks);
    discoveredDeckUrls = [...urls].sort();
  }

  return {
    discoverProviderAdvertisements,
    discoverInterfaces,
    getDiscoveredDeckUrls: () => [...discoveredDeckUrls],
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
  return {
    providerId,
    title,
    state: "active",
    detail: `${surface?.root?.kind || surface?.root?.type || "surface"} ${countSurfaceNodes(surface, state)} nodes via provider advertisement`,
    version: state.version || 0,
    updatedAt: state.updatedAt || advertisement.updatedAt || new Date().toISOString(),
    source: `cultmesh:${storePath}`,
    manifest: advertisement,
    canonicalService: advertisement.canonicalService || null,
    locatedService: advertisement.locatedService || null,
    cultMeshAddress: advertisement.cultMeshAddress || null,
    endpoints: advertisement.endpoints || [],
    routes: advertisement.routes || [],
    operatorState,
    commandBoundary,
    transportProfile,
    surface,
  };
}

function providerSurfaceState(node, advertisement, surfaceDefinition, muninnTelemetrySurfaceDefinition) {
  const surfaceKey = advertisement.surfaceId || advertisement.surface_id || advertisement.providerId;
  const surfaceState = unwrapDocumentRecord(node.get?.(surfaceDefinition, surfaceKey));
  if (surfaceState?.surface?.root) {
    return {
      providerId: surfaceState.providerId || advertisement.providerId || advertisement.provider?.id || "unknown-provider",
      title: surfaceState.title || advertisement.title || advertisement.provider?.title || advertisement.providerId || "unknown-provider",
      version: Number(surfaceState.version || 0),
      updatedAt: surfaceState.updatedAt || advertisement.updatedAt || new Date().toISOString(),
      surface: surfaceState.surface,
    };
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
  const endpoints = normalizeInspectedEndpoints(advertisement.endpoints);
  return {
    id: providerId,
    title: advertisement.title || providerId,
    description: advertisement.description || "Provider-owned CultMesh advertisement.",
    version: String(advertisement.version || 0),
    endpoint: advertisement.cultMeshAddress || endpoints[0]?.address || `cultmesh:${sourceId}`,
    canonicalService: advertisement.canonicalService || null,
    locatedService: advertisement.locatedService || null,
    cultMeshAddress: advertisement.cultMeshAddress || null,
    endpoints,
    routes: normalizeInspectedRoutes(advertisement),
    transportEndpoint: advertisement.cultMeshAddress || endpoints.find((entry) => String(entry.address || "").startsWith("ws://"))?.address || endpoints[0]?.address || `cultmesh:${sourceId}`,
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
      endpoints: provider.endpoints,
      routes: provider.routes,
    } : null,
    canonicalService: provider?.canonicalService || null,
    locatedService: provider?.locatedService || null,
    cultMeshAddress: provider?.cultMeshAddress || null,
    endpoints: provider?.endpoints || [],
    routes: provider?.routes || [],
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
      endpoints: provider.endpoints,
      routes: provider.routes,
    } : null,
    canonicalService: provider?.canonicalService || null,
    locatedService: provider?.locatedService || null,
    cultMeshAddress: provider?.cultMeshAddress || null,
    endpoints: provider?.endpoints || [],
    routes: provider?.routes || [],
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
    compatibility_transport: record.payloadPreview[3] || null,
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
      transport: String(entry || "").startsWith("ws://") ? "compatibility-eve-deck" : "witness",
      address: String(entry || ""),
    };
  });
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
  if (!storeSpec.startsWith("sftp://")) {
    return {
      localPath: storeSpec,
      sourceId: storeSpec,
      cleanup: () => {},
    };
  }

  const url = new URL(storeSpec);
  const host = url.hostname;
  let remotePath = decodeURIComponent(url.pathname);
  if (/^\/[A-Za-z]:\//.test(remotePath)) {
    remotePath = remotePath.slice(1);
  }
  if (!host || !remotePath) {
    throw new Error(`invalid sftp interface binding store: ${storeSpec}`);
  }

  const cacheRoot = path.join(os.tmpdir(), "odin-cultmesh-stores");
  fs.mkdirSync(cacheRoot, { recursive: true });
  const suffix = path.extname(remotePath) || ".cc";
  const stem = `${stableId(host)}-${stableId(remotePath)}`;
  const transferId = `${process.pid}-${Date.now()}-${Math.random().toString(16).slice(2, 10)}`;
  const localPath = path.join(cacheRoot, `${stem}-${transferId}${suffix}`);
  const batchPath = path.join(cacheRoot, `${stem}-${transferId}.sftp`);
  const stagedRemotePath = stageRemoteStoreForRead(host, remotePath, `${stem}-${transferId}`, suffix);
  fs.writeFileSync(
    batchPath,
    `get "${stagedRemotePath}" "${localPath.replace(/\\/g, "/")}"\n`,
    "ascii",
  );
  try {
    const result = childProcess.spawnSync(
      "sftp.exe",
      ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-b", batchPath, host],
      {
        encoding: "utf8",
        timeout: 15000,
        windowsHide: true,
      },
    );
    if (result.status !== 0) {
      throw new Error((result.stderr || result.stdout || `sftp exited ${result.status}`).trim());
    }
  } finally {
    cleanupRemoteStagedStore(host, stagedRemotePath);
    try {
      fs.rmSync(batchPath, { force: true });
    } catch {
      // Best-effort temp cleanup. A later pass can reuse a fresh transfer id.
    }
  }

  return {
    localPath,
    sourceId: storeSpec,
    cleanup: () => {
      try {
        fs.rmSync(localPath, { force: true });
      } catch {
        // Best-effort temp cleanup. Live readers may still be releasing handles.
      }
    },
  };
}

function stageRemoteStoreForRead(host, remotePath, stem, suffix) {
  const stagedRemotePath = isWindowsRemoteStorePath(remotePath)
    ? `C:/Windows/Temp/odin-interface-${stem}${suffix}`
    : `/tmp/odin-interface-${stem}${suffix}`;
  if (isWindowsRemoteStorePath(remotePath)) {
    const script = [
      "$ErrorActionPreference = 'Stop'",
      `$sourcePath = ${quotePowerShellSingle(remotePath)}`,
      `$stagedPath = ${quotePowerShellSingle(stagedRemotePath)}`,
      "$lockPath = $sourcePath + '.lock'",
      "if (-not (Test-Path -LiteralPath $sourcePath)) { throw \"remote store missing at $sourcePath\" }",
      "New-Item -ItemType Directory -Force -Path (Split-Path -Parent $stagedPath) | Out-Null",
      "$lockStream = [System.IO.File]::Open($lockPath, [System.IO.FileMode]::OpenOrCreate, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::ReadWrite)",
      "try {",
      "  $lockStream.Lock(0, 1)",
      "  Copy-Item -LiteralPath $sourcePath -Destination $stagedPath -Force",
      "} finally {",
      "  try { $lockStream.Unlock(0, 1) } catch {}",
      "  $lockStream.Dispose()",
      "}",
    ].join("\n");
    runRemoteCommand(
      host,
      `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encodePowerShellCommand(script)}`,
    );
  } else {
    const command = [
      "set -eu",
      `src=${quotePosixSingle(remotePath)}`,
      `staged=${quotePosixSingle(stagedRemotePath)}`,
      'lock="${src}.lock"',
      'mkdir -p "$(dirname "$staged")"',
      'if [ -e "$lock" ]; then',
      '  exec 9<"$lock"',
      '  flock -s 9',
      'fi',
      'cp "$src" "$staged"',
    ].join("\n");
    runRemoteCommand(host, `sh -lc ${quotePosixSingle(command)}`);
  }
  return stagedRemotePath;
}

function cleanupRemoteStagedStore(host, stagedRemotePath) {
  try {
    if (isWindowsRemoteStorePath(stagedRemotePath)) {
      const script = [
        "$ErrorActionPreference = 'SilentlyContinue'",
        `$stagedPath = ${quotePowerShellSingle(stagedRemotePath)}`,
        "Remove-Item -LiteralPath $stagedPath -Force -ErrorAction SilentlyContinue",
      ].join("\n");
      runRemoteCommand(
        host,
        `powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ${encodePowerShellCommand(script)}`,
      );
      return;
    }
    runRemoteCommand(host, `sh -lc ${quotePosixSingle(`rm -f ${quotePosixSingle(stagedRemotePath)}`)}`);
  } catch {
    // Temp snapshot cleanup is opportunistic.
  }
}

function runRemoteCommand(host, command) {
  const result = childProcess.spawnSync(
    "ssh.exe",
    ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host, command],
    {
      encoding: "utf8",
      timeout: 15000,
      windowsHide: true,
    },
  );
  if (result.status !== 0) {
    throw new Error((result.stderr || result.stdout || `ssh exited ${result.status}`).trim());
  }
}

function isWindowsRemoteStorePath(remotePath) {
  return /^[A-Za-z]:\//.test(remotePath);
}

function encodePowerShellCommand(script) {
  return Buffer.from(script, "utf16le").toString("base64");
}

function quotePowerShellSingle(value) {
  return `'${String(value).replace(/'/g, "''")}'`;
}

function quotePosixSingle(value) {
  return `'${String(value).replace(/'/g, `'\\''`)}'`;
}

function localIpv4Prefixes() {
  const prefixes = new Set();
  for (const entries of Object.values(os.networkInterfaces())) {
    for (const entry of entries || []) {
      if (entry.family !== "IPv4" || entry.internal) continue;
      const parts = entry.address.split(".");
      if (parts.length === 4) prefixes.add(parts.slice(0, 3).join("."));
    }
  }
  return [...prefixes];
}

async function fetchEveProvider(url, providerId, manifest = null) {
  const direct = await fetchEveProviderDirect(providerDeckUrl(url, providerId), providerId, manifest);
  if (direct.state === "active") {
    return direct;
  }

  const broker = await fetchEveProviderViaBroker(url, providerId, manifest);
  if (broker.state === "active") {
    return broker;
  }

  return broker.detail ? broker : direct;
}

async function fetchEveProviderDirect(url, providerId, manifest = null) {
  try {
    const socket = await openWebSocket(url);
    try {
      for (let index = 0; index < 3; index += 1) {
        const message = await readServerTextFrame(socket, 1000);
        const state = JSON.parse(message);
        if (state?.providerId === providerId) {
          return activeProviderInterface(url, providerId, state, manifest);
        }
      }
      return dashboardUnavailable(providerId, url, "direct provider stream did not publish matching state");
    } finally {
      socket.destroy();
    }
  } catch (error) {
    return dashboardUnavailable(providerId, url, error.message);
  }
}

async function fetchEveProviderViaBroker(url, providerId, manifest = null) {
  try {
    const socket = await openWebSocket(url);
    try {
      sendClientFrame(socket, JSON.stringify({ type: "open-provider", providerId }));
      for (let index = 0; index < 8; index += 1) {
        const message = await readServerTextFrame(socket, 2500);
        const state = JSON.parse(message);
        if (state?.providerId === providerId) {
          return activeProviderInterface(url, providerId, state, manifest);
        }
      }
      return dashboardUnavailable(providerId, url, "provider did not publish matching state");
    } finally {
      socket.destroy();
    }
  } catch (error) {
    return dashboardUnavailable(providerId, url, error.message);
  }
}

function activeProviderInterface(source, providerId, state, manifest = null) {
  const surface = state.surface?.root ? state.surface : legacySurfaceFromNodes(state, manifest);
  return {
    providerId,
    title: state.title || manifest?.title || providerId,
    state: "active",
    detail: `${surface?.root?.kind || "surface"} ${countSurfaceNodes(surface, state)} nodes`,
    version: state.version,
    updatedAt: state.updatedAt,
    source,
    manifest,
    surface,
  };
}

function providerDeckUrl(url, providerId) {
  const deck = new URL(url);
  deck.pathname = `/eve/deck/${encodeURIComponent(providerId)}`;
  deck.search = "";
  return deck.toString();
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

function legacySurfaceFromNodes(state, manifest = null) {
  const nodes = Array.isArray(state?.nodes) ? state.nodes : [];
  if (!nodes.length) {
    return null;
  }

  const providerId = state.providerId || manifest?.id || "legacy-provider";
  const title = state.title || manifest?.title || providerId;
  return {
    schema: "gamecult.eve.surface.v1",
    id: `legacy-${providerId}`,
    title,
    root: {
      id: `legacy-root-${providerId}`,
      kind: "dashboard",
      props: {
        title,
        providerId,
        compatibility: "legacy-dashboard-nodes",
        layout: {
          density: "dense",
          viewportMode: "nested-scroll",
          layoutStrategy: "legacy-node-groups",
          preferredWidth: 96,
          preferredHeight: 24,
          minWidth: 36,
          minHeight: 8,
        },
      },
      children: legacyNodeGroups(nodes, providerId),
    },
    assets: [],
  };
}

function legacyNodeGroups(nodes, providerId) {
  const groups = new Map();
  for (const node of nodes) {
    const kind = String(node.kind || "node");
    const key = kind;
    if (!groups.has(key)) {
      groups.set(key, { kind, nodes: [] });
    }
    groups.get(key).nodes.push(node);
  }

  return [...groups.values()]
    .sort((left, right) => left.kind.localeCompare(right.kind))
    .map((group) => legacyKindGroup(group, providerId));
}

function legacyKindGroup(group, providerId) {
  const richNodes = [];
  const statusBuckets = new Map();
  for (const node of group.nodes) {
    if (hasLegacyDetails(node)) {
      richNodes.push(node);
      continue;
    }

    const health = String(node.health || "unknown");
    if (!statusBuckets.has(health)) {
      statusBuckets.set(health, []);
    }
    statusBuckets.get(health).push(String(node.label || node.id || "node"));
  }

  const summaries = [...statusBuckets.entries()]
    .sort((left, right) => left[0].localeCompare(right[0]))
    .map(([health, labels]) => textElement(
      `legacy-summary-${stableLegacyId(providerId, group.kind, health)}`,
      `${health}: ${labels.sort((left, right) => left.localeCompare(right)).join(", ")}`,
    ));

  return {
    id: `legacy-group-${stableLegacyId(providerId, group.kind)}`,
    kind: "group",
    props: {
      title: group.kind,
      count: group.nodes.length,
      density: "dense",
    },
    children: [
      ...summaries,
      ...richNodes
        .sort((left, right) => String(left.id || left.label || "").localeCompare(String(right.id || right.label || "")))
        .map((node) => legacyNodeElement(node, providerId)),
    ],
  };
}

function hasLegacyDetails(node) {
  return Boolean(node.endpoint || node.command || (node.providerId && node.providerId !== "unknown"));
}

function legacyNodeElement(node, providerId) {
  const id = String(node.id || node.label || "node");
  const title = String(node.label || node.id || "node");
  const facts = [
    compactFact(id, "kind", node.kind || "node"),
    compactFact(id, "health", node.health || "unknown"),
    node.providerId ? compactFact(id, "provider", node.providerId || providerId) : null,
    node.endpoint ? compactFact(id, "endpoint", node.endpoint) : null,
    node.command ? compactFact(id, "command", node.command) : null,
  ].filter(Boolean);
  return {
    id,
    kind: "card",
    props: {
      title,
      status: String(node.health || ""),
      providerId: node.providerId || providerId,
      command: node.command || "",
      endpoint: node.endpoint || "",
      density: "compact",
    },
    children: facts,
  };
}

function compactFact(ownerId, name, value) {
  return textElement(`fact-${stableLegacyId(ownerId, name, value)}`, `${name}: ${value}`);
}

function stableLegacyId(...parts) {
  return parts
    .map((part) => String(part || "x").trim().toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, ""))
    .filter(Boolean)
    .join("-") || "x";
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
  fetchEveProvider,
  legacySurfaceFromNodes,
};

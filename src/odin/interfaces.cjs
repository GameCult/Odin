"use strict";

const childProcess = require("child_process");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { httpGet, stableId } = require("./utils.cjs");
const { openWebSocket, readServerTextFrame, sendClientFrame } = require("./websocket.cjs");
const { tcpCheck } = require("./probes.cjs");

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
    if (!CultMesh || !interfaceBindingDefinition || !idunnDaemonHealthDefinition || !muninnCaptureStreamDefinition || !muninnCommandBoundaryDefinition || !muninnMoveControllerStateDefinition || !muninnMoveLightCommandDefinition || !muninnMoveMarkerCandidateDefinition || !muninnObsStreamCatalogDefinition || !muninnQuestAccessDefinition || !muninnTelemetrySurfaceDefinition || !muninnTransportProfileDefinition || !operatorStateDefinition || !stonksCommandBoundaryDefinition || !stonksMarketSnapshotDefinition || !stonksRequestEventDefinition || !stonksTransportProfileDefinition || !streamPixelsCommandBoundaryDefinition || !streamPixelsTransportProfileDefinition || !surfaceDefinition || !viliCommandBoundaryDefinition || !viliTransportProfileDefinition || !weksaCommandBoundaryDefinition || !weksaOperatorStateDefinition || !weksaTransportProfileDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
      return [];
    }

    const providers = [];
    for (const storeSpec of interfaceBindingStores) {
      try {
        const resolvedStore = resolveInterfaceBindingStore(storeSpec);
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
    if (!CultMesh || !interfaceBindingDefinition || !idunnDaemonHealthDefinition || !muninnCaptureStreamDefinition || !muninnCommandBoundaryDefinition || !muninnMoveControllerStateDefinition || !muninnMoveLightCommandDefinition || !muninnMoveMarkerCandidateDefinition || !muninnObsStreamCatalogDefinition || !muninnQuestAccessDefinition || !muninnTelemetrySurfaceDefinition || !muninnTransportProfileDefinition || !operatorStateDefinition || !stonksCommandBoundaryDefinition || !stonksMarketSnapshotDefinition || !stonksRequestEventDefinition || !stonksTransportProfileDefinition || !streamPixelsCommandBoundaryDefinition || !streamPixelsTransportProfileDefinition || !surfaceDefinition || !viliCommandBoundaryDefinition || !viliTransportProfileDefinition || !weksaCommandBoundaryDefinition || !weksaOperatorStateDefinition || !weksaTransportProfileDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
      return [];
    }
    const interfaces = [];
    for (const storeSpec of interfaceBindingStores) {
      try {
        const resolvedStore = resolveInterfaceBindingStore(storeSpec);
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
          const state = unwrapDocumentRecord(node.get(
            surfaceDefinition,
            advertisement.surfaceId || advertisement.surface_id || advertisement.providerId,
          ));
          if (!state?.surface) {
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
        interfaces.push(dashboardUnavailable(`cultmesh:${storeSpec}`, `cultmesh:${storeSpec}`, error.message));
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

function resolveInterfaceBindingStore(storeSpec) {
  if (typeof storeSpec !== "string" || !storeSpec.trim()) {
    return null;
  }
  if (!storeSpec.startsWith("sftp://")) {
    return {
      localPath: storeSpec,
      sourceId: storeSpec,
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
  const localPath = path.join(cacheRoot, `${stem}${suffix}`);
  const batchPath = path.join(cacheRoot, `${stem}.sftp`);
  fs.writeFileSync(
    batchPath,
    `get "${remotePath}" "${localPath.replace(/\\/g, "/")}"\n`,
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
    fs.rmSync(batchPath, { force: true });
  }

  return {
    localPath,
    sourceId: storeSpec,
  };
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

module.exports = {
  createInterfaceDiscovery,
  dashboardUnavailable,
  fetchEveProvider,
  legacySurfaceFromNodes,
};

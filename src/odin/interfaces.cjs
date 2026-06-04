"use strict";

const fs = require("fs");
const os = require("os");
const { httpGet } = require("./utils.cjs");
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
      providerAdvertisementDefinition,
      surfaceDefinition,
      voidbotSwarmSnapshotDefinition,
    } = documents;
    if (!CultMesh || !interfaceBindingDefinition || !surfaceDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
      return [];
    }

    const providers = [];
    for (const storePath of interfaceBindingStores) {
      try {
        if (!fs.existsSync(storePath)) {
          continue;
        }
        const node = await CultMesh.createNode(storePath, {
          documents: [
            voidbotSwarmSnapshotDefinition,
            providerAdvertisementDefinition,
            interfaceBindingDefinition,
            surfaceDefinition,
          ],
        });
        const advertisements = typeof node.cache?.getAll === "function"
          ? node.cache.getAll(providerAdvertisementDefinition)
          : [];
        for (const advertisement of advertisements) {
          if (!advertisement?.providerId) {
            continue;
          }
          providers.push({
            id: advertisement.providerId,
            title: advertisement.title || advertisement.providerId,
            description: advertisement.description || "Provider-owned CultMesh advertisement.",
            version: String(advertisement.version || 0),
            endpoint: advertisement.cultMeshAddress || advertisement.endpoints?.[0]?.address || advertisement.provider?.endpoint || `cultmesh:${storePath}`,
            canonicalService: advertisement.canonicalService || null,
            locatedService: advertisement.locatedService || null,
            cultMeshAddress: advertisement.cultMeshAddress || null,
            endpoints: advertisement.endpoints || [],
            routes: advertisement.routes || [],
            transportEndpoint: advertisement.routes?.find((route) => route.transport === "cultnet")?.address
              || advertisement.routes?.find((route) => route.transport === "compatibility-eve-deck")?.address
              || advertisement.provider?.endpoint
              || `cultmesh:${storePath}`,
            capabilities: advertisement.provider?.capabilities || advertisement.capabilities || [],
            usesCultMesh: true,
            transport: advertisement.provider?.transport || "CultMesh provider advertisement",
            status: advertisement.status || "unknown",
            updatedAt: advertisement.updatedAt || new Date().toISOString(),
            source: `cultmesh:${storePath}`,
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
      providerAdvertisementDefinition,
      surfaceDefinition,
      voidbotSwarmSnapshotDefinition,
    } = documents;
    if (!CultMesh || !interfaceBindingDefinition || !surfaceDefinition || !voidbotSwarmSnapshotDefinition || !providerAdvertisementDefinition) {
      return [];
    }
    const interfaces = [];
    for (const storePath of interfaceBindingStores) {
      try {
        if (!fs.existsSync(storePath)) {
          continue;
        }
        const node = await CultMesh.createNode(storePath, {
          documents: [
            voidbotSwarmSnapshotDefinition,
            providerAdvertisementDefinition,
            interfaceBindingDefinition,
            surfaceDefinition,
          ],
        });
        const bindings = typeof node.cache?.getAll === "function"
          ? node.cache.getAll(interfaceBindingDefinition)
          : [node.get(interfaceBindingDefinition, "voidbot.swarm")].filter(Boolean);
        for (const binding of bindings) {
          if (!binding?.providerId) {
            continue;
          }
          const state = node.get(surfaceDefinition, binding.providerId);
          const surface = state?.surface || binding.surface || null;
          interfaces.push({
            providerId: binding.providerId,
            title: binding.title || state?.title || binding.providerId,
            state: "active",
            detail: `${surface?.root?.kind || binding.kind || "surface"} ${countSurfaceNodes(surface, state)} nodes via CultMesh`,
            version: state?.version || 0,
            updatedAt: state?.updatedAt || binding.updatedAt || new Date().toISOString(),
            source: `cultmesh:${storePath}`,
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
        interfaces.push(dashboardUnavailable(`cultmesh:${storePath}`, `cultmesh:${storePath}`, error.message));
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
  try {
    const socket = await openWebSocket(url);
    try {
      sendClientFrame(socket, JSON.stringify({ type: "open-provider", providerId }));
      for (let index = 0; index < 8; index += 1) {
        const message = await readServerTextFrame(socket, 2500);
        const state = JSON.parse(message);
        if (state?.providerId === providerId) {
          const surface = state.surface?.root ? state.surface : legacySurfaceFromNodes(state, manifest);
          return {
            providerId,
            title: state.title || manifest?.title || providerId,
            state: "active",
            detail: `${surface?.root?.kind || "surface"} ${countSurfaceNodes(surface, state)} nodes`,
            version: state.version,
            updatedAt: state.updatedAt,
            source: url,
            manifest,
            surface,
          };
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

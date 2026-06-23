"use strict";

function createLiveProviderRegistry(options = {}) {
  const maxAgeMs = Math.max(1_000, Number(options.maxAgeMs || 120_000));
  const records = new Map();

  function ingestDocument(document, source = {}) {
    if (!document?.schemaId || !document?.recordKey) {
      return;
    }
    const now = Date.now();
    records.set(`${document.schemaId}:${document.recordKey}`, {
      ...document,
      receivedAtMs: now,
      source,
    });
    prune(now);
  }

  function snapshot() {
    const now = Date.now();
    prune(now);

    const advertisements = new Map();
    const surfaces = new Map();
    const commandBoundaries = new Map();
    const transportProfiles = new Map();

    for (const record of records.values()) {
      switch (record.schemaId) {
        case "gamecult.eve.provider_advertisement":
          if (record.payload?.providerId) {
            advertisements.set(String(record.payload.providerId), record);
          }
          break;
        case "gamecult.eve.surface_state":
          if (record.payload?.providerId) {
            surfaces.set(String(record.payload.providerId), record);
          }
          break;
        default: {
          const payload = record.payload || {};
          const providerId = payload.provider_id || payload.providerId;
          if (!providerId) break;
          if (String(record.schemaId).endsWith(".command_boundary")) {
            commandBoundaries.set(String(providerId), payload);
          } else if (String(record.schemaId).endsWith(".transport_profile")) {
            transportProfiles.set(String(providerId), payload);
          }
          break;
        }
      }
    }

    const providerAdvertisements = [...advertisements.values()]
      .map((record) => normalizeProviderAdvertisementRecord(record, commandBoundaries, transportProfiles))
      .filter(Boolean)
      .sort((left, right) => String(left.id).localeCompare(String(right.id)));

    const interfaces = providerAdvertisements
      .map((provider) => {
        const surfaceRecord = surfaces.get(String(provider.id));
        if (!surfaceRecord?.payload?.surface?.root) {
          return null;
        }
        return liveProviderInterface(provider, surfaceRecord.payload, surfaceRecord.source);
      })
      .filter(Boolean)
      .sort((left, right) => String(left.providerId).localeCompare(String(right.providerId)));

    return { interfaces, providerAdvertisements };
  }

  function prune(now = Date.now()) {
    for (const [key, record] of records) {
      if (now - record.receivedAtMs > maxAgeMs) {
        records.delete(key);
      }
    }
  }

  return {
    ingestDocument,
    snapshot,
  };
}

function normalizeProviderAdvertisementRecord(record, commandBoundaries, transportProfiles) {
  const advertisement = record.payload;
  const providerId = advertisement?.providerId || advertisement?.provider?.id || advertisement?.id;
  if (!providerId) {
    return null;
  }
  return {
    id: providerId,
    title: advertisement.title || advertisement.provider?.title || providerId,
    description: advertisement.description || "Provider-owned CultNet/RUDP advertisement.",
    version: String(advertisement.version || 0),
    endpoint: advertisement.cultMeshAddress || advertisement.endpoints?.[0]?.address || advertisement.provider?.endpoint || sourceLabel(record.source),
    canonicalService: advertisement.canonicalService || null,
    locatedService: advertisement.locatedService || null,
    cultMeshAddress: advertisement.cultMeshAddress || null,
    endpoints: Array.isArray(advertisement.endpoints) ? advertisement.endpoints : [],
    routes: Array.isArray(advertisement.routes) ? advertisement.routes : [],
    transportEndpoint: advertisement.routes?.find((route) => route.transport === "cultnet")?.address
      || advertisement.routes?.find((route) => route.transport === "compatibility-eve-deck")?.address
      || advertisement.provider?.endpoint
      || sourceLabel(record.source),
    capabilities: advertisement.provider?.capabilities || advertisement.capabilities || [],
    operatorState: null,
    commandBoundary: commandBoundaries.get(String(providerId)) || null,
    transportProfile: transportProfiles.get(String(providerId)) || null,
    usesCultMesh: false,
    transport: "CultNet/RUDP provider advertisement",
    status: advertisement.status || "unknown",
    updatedAt: advertisement.updatedAt || record.storedAt || new Date(record.receivedAtMs).toISOString(),
    source: sourceLabel(record.source),
    commandSurface: advertisement.commandSurface || null,
  };
}

function liveProviderInterface(provider, surfaceState, source) {
  const surface = surfaceState.surface || null;
  return {
    providerId: provider.id,
    title: surfaceState.title || provider.title || provider.id,
    state: "active",
    detail: `${surface?.root?.kind || surface?.root?.type || "surface"} ${countSurfaceNodes(surface, surfaceState)} nodes via live CultNet/RUDP announcement`,
    version: surfaceState.version || 0,
    updatedAt: surfaceState.updatedAt || provider.updatedAt || new Date().toISOString(),
    source: sourceLabel(source),
    manifest: provider,
    canonicalService: provider.canonicalService || null,
    locatedService: provider.locatedService || null,
    cultMeshAddress: provider.cultMeshAddress || null,
    endpoints: provider.endpoints || [],
    routes: provider.routes || [],
    commandBoundary: provider.commandBoundary || null,
    transportProfile: provider.transportProfile || null,
    surface,
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
    if (!node || typeof node !== "object") continue;
    count += 1;
    if (Array.isArray(node.children)) {
      for (const child of node.children) stack.push(child);
    }
  }
  return count;
}

function sourceLabel(source) {
  if (!source?.address) {
    return "cultnet-rudp:unknown";
  }
  return `cultnet-rudp://${source.address}:${source.port}`;
}

module.exports = {
  createLiveProviderRegistry,
};

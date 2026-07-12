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
      const payload = normalizeDocumentPayload(record);
      switch (record.schemaId) {
        case "gamecult.eve.provider_advertisement":
        case "gamecult.eve.provider_advertisement.v1":
          if (payload?.providerId) {
            advertisements.set(String(payload.providerId), { ...record, payload });
          }
          break;
        case "gamecult.eve.surface_state":
        case "gamecult.eve.surface_state.v0":
        case "gamecult.eve.surface_state.v1":
          if (surfaceProviderId(payload)) {
            surfaces.set(String(surfaceProviderId(payload)), { ...record, payload });
          }
          break;
        default: {
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

function normalizeDocumentPayload(record) {
  const payload = record?.payload;
  switch (record?.schemaId) {
    case "gamecult.eve.surface_state":
    case "gamecult.eve.surface_state.v0":
    case "gamecult.eve.surface_state.v1":
      if (Array.isArray(payload) && payload.length >= 5) {
        return {
          provider_id: payload[0],
          title: payload[1],
          version: payload[2],
          updated_at: payload[3],
          surface: payload[4],
        };
      }
      break;
    default:
      break;
  }
  let normalized = payload;
  for (let depth = 0; depth < 4; depth += 1) {
    if (Array.isArray(normalized) && normalized.length === 1 && normalized[0] && typeof normalized[0] === "object") {
      normalized = normalized[0];
      continue;
    }
    if (normalized?.value && typeof normalized.value === "object" && !Array.isArray(normalized.value)) {
      normalized = normalized.value;
      continue;
    }
    break;
  }
  return normalized || {};
}

function surfaceProviderId(surfaceState) {
  return surfaceState?.providerId || surfaceState?.provider_id || null;
}

function surfaceTitle(surfaceState) {
  return surfaceState?.title || surfaceState?.Title || null;
}

function surfaceVersion(surfaceState) {
  return surfaceState?.version || surfaceState?.Version || 0;
}

function surfaceUpdatedAt(surfaceState) {
  return surfaceState?.updatedAt || surfaceState?.updated_at || surfaceState?.UpdatedAt || null;
}

function normalizeProviderAdvertisementRecord(record, commandBoundaries, transportProfiles) {
  const advertisement = record.payload;
  const providerId = advertisement?.providerId || advertisement?.provider?.id || advertisement?.id;
  if (!providerId) {
    return null;
  }
  const endpoints = sanitizeProviderEndpoints(advertisement.endpoints);
  const routes = sanitizeProviderEndpoints(advertisement.routes);
  const providerEndpoint = advertisement.cultMeshAddress
    || providerTypedEndpoint(routes)
    || providerTypedEndpoint(endpoints)
    || typedProviderEndpoint(advertisement.provider?.endpoint)
    || `cultmesh:live:${providerId}`;
  return {
    id: providerId,
    title: advertisement.title || advertisement.provider?.title || providerId,
    description: advertisement.description || "Provider-owned CultNet/RUDP advertisement.",
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

function typedProviderEndpoint(endpoint) {
  const value = typeof endpoint === "string" ? endpoint.trim() : "";
  if (!value) {
    return null;
  }
  return value.startsWith("cultmesh:") ? value : null;
}

function normalizeProviderEndpoints(endpoints) {
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
  return normalizeProviderEndpoints(endpoints).filter((entry) => {
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
  return sanitizeProviderEndpoints(endpoints).find((entry) =>
    String(entry?.address || "").startsWith("cultmesh:")
  )?.address || null;
}

function liveProviderInterface(provider, surfaceState, source) {
  const surface = surfaceState.surface || null;
  return {
    providerId: provider.id,
    title: surfaceTitle(surfaceState) || provider.title || provider.id,
    state: "active",
    detail: `${surface?.root?.kind || surface?.root?.type || "surface"} ${countSurfaceNodes(surface, surfaceState)} nodes via live CultNet/RUDP announcement`,
    version: surfaceVersion(surfaceState),
    updatedAt: surfaceUpdatedAt(surfaceState) || provider.updatedAt || new Date().toISOString(),
    source: sourceLabel(source),
    manifest: provider,
    canonicalService: provider.canonicalService || null,
    locatedService: provider.locatedService || null,
    cultMeshAddress: provider.cultMeshAddress || null,
    endpoints: sanitizeProviderEndpoints(provider.endpoints),
    routes: sanitizeProviderEndpoints(provider.routes),
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

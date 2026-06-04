"use strict";

const { parseObjectDocument } = require("./utils.cjs");

function defineOdinDocuments(defineDocumentType) {
  if (!defineDocumentType) {
    return {
      interfaceBindingDefinition: null,
      providerAdvertisementDefinition: null,
      surfaceDefinition: null,
      voidbotSwarmSnapshotDefinition: null,
    };
  }

  const surfaceDefinition = defineDocumentType({
    type: "gamecult.eve.surface_state",
    schemaName: "gamecult.eve.surface_state",
    schemaId: "gamecult.eve.surface_state.v1",
    schemaVersion: "gamecult.eve.surface_state.v1",
    global: false,
    name: (value) => value?.providerId || "surface",
    schema: { parse: (value) => value },
    members: [
      { slot: 0, memberName: "providerId", typeName: "string", isName: true },
      { slot: 1, memberName: "title", typeName: "string" },
      { slot: 2, memberName: "version", typeName: "long" },
      { slot: 3, memberName: "updatedAt", typeName: "string" },
      { slot: 4, memberName: "surface", typeName: "object" },
    ],
  });
  const interfaceBindingDefinition = defineDocumentType({
    type: "gamecult.eve.interface_binding",
    schemaName: "gamecult.eve.interface_binding",
    schemaId: "gamecult.eve.interface_binding.v1",
    schemaVersion: "gamecult.eve.interface_binding.v1",
    global: false,
    name: (value) => value?.bindingId || value?.providerId || "interface",
    schema: parseObjectDocument("Eve interface binding"),
  });
  const providerAdvertisementDefinition = defineDocumentType({
    type: "gamecult.eve.provider_advertisement",
    schemaName: "gamecult.eve.provider_advertisement",
    schemaId: "gamecult.eve.provider_advertisement.v1",
    schemaVersion: "gamecult.eve.provider_advertisement.v1",
    global: false,
    name: (value) => value?.providerId || "provider",
    schema: parseObjectDocument("Eve provider advertisement"),
  });
  const voidbotSwarmSnapshotDefinition = defineDocumentType({
    type: "voidbot.swarm_state_snapshot",
    schemaName: "voidbot.swarm_state_snapshot",
    schemaId: "voidbot.swarm_state_snapshot.v1",
    schemaVersion: "voidbot.swarm_state_snapshot.v1",
    global: true,
    schema: parseObjectDocument("VoidBot swarm snapshot"),
  });

  return {
    interfaceBindingDefinition,
    providerAdvertisementDefinition,
    surfaceDefinition,
    voidbotSwarmSnapshotDefinition,
  };
}

module.exports = { defineOdinDocuments };

"use strict";

const { parseObjectDocument } = require("./utils.cjs");

function defineOdinDocuments(defineDocumentType) {
  if (!defineDocumentType) {
    return {
      interfaceBindingDefinition: null,
      operatorStateDefinition: null,
      providerAdvertisementDefinition: null,
      surfaceDefinition: null,
      viliCommandBoundaryDefinition: null,
      viliTransportProfileDefinition: null,
      weksaCommandBoundaryDefinition: null,
      weksaOperatorStateDefinition: null,
      weksaTransportProfileDefinition: null,
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
  const operatorStateDefinition = defineDocumentType({
    type: "gamecult.vili.operator_state",
    schemaName: "gamecult.vili.operator_state",
    schemaId: "gamecult.vili.operator_state.v1",
    schemaVersion: "gamecult.vili.operator_state.v1",
    global: false,
    name: (value) => value?.service || value?.providerId || "operator",
    schema: parseObjectDocument("Vili operator state"),
  });
  const viliCommandBoundaryDefinition = defineDocumentType({
    type: "gamecult.vili.command_boundary",
    schemaName: "gamecult.vili.command_boundary",
    schemaId: "gamecult.vili.command_boundary.v1",
    schemaVersion: "gamecult.vili.command_boundary.v1",
    global: false,
    name: (value) => value?.boundaryId || value?.daemonId || "vili",
    schema: parseObjectDocument("Vili command boundary"),
  });
  const viliTransportProfileDefinition = defineDocumentType({
    type: "gamecult.vili.transport_profile",
    schemaName: "gamecult.vili.transport_profile",
    schemaId: "gamecult.vili.transport_profile.v1",
    schemaVersion: "gamecult.vili.transport_profile.v1",
    global: false,
    name: (value) => value?.profileId || value?.daemonId || "vili",
    schema: parseObjectDocument("Vili transport profile"),
  });
  const weksaOperatorStateDefinition = defineDocumentType({
    type: "weksa.operator_state",
    schemaName: "weksa.operator_state",
    schemaId: "weksa.operator_state.v0",
    schemaVersion: "weksa.operator_state.v0",
    global: false,
    name: (value) => value?.daemon_id || value?.daemonId || value?.provider_id || value?.providerId || "weksa",
    schema: parseObjectDocument("Weksa operator state"),
  });
  const weksaCommandBoundaryDefinition = defineDocumentType({
    type: "weksa.command_boundary",
    schemaName: "weksa.command_boundary",
    schemaId: "weksa.command_boundary.v1",
    schemaVersion: "weksa.command_boundary.v1",
    global: false,
    name: (value) => value?.boundary_id || value?.boundaryId || value?.daemon_id || value?.daemonId || "weksa",
    schema: parseObjectDocument("Weksa command boundary"),
  });
  const weksaTransportProfileDefinition = defineDocumentType({
    type: "weksa.transport_profile",
    schemaName: "weksa.transport_profile",
    schemaId: "weksa.transport_profile.v1",
    schemaVersion: "weksa.transport_profile.v1",
    global: false,
    name: (value) => value?.profile_id || value?.profileId || value?.daemon_id || value?.daemonId || "weksa",
    schema: parseObjectDocument("Weksa transport profile"),
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
    operatorStateDefinition,
    providerAdvertisementDefinition,
    surfaceDefinition,
    viliCommandBoundaryDefinition,
    viliTransportProfileDefinition,
    weksaCommandBoundaryDefinition,
    weksaOperatorStateDefinition,
    weksaTransportProfileDefinition,
    voidbotSwarmSnapshotDefinition,
  };
}

module.exports = { defineOdinDocuments };

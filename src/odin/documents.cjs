"use strict";

const { parseObjectDocument } = require("./utils.cjs");

function defineOdinDocuments(defineDocumentType) {
  if (!defineDocumentType) {
    return {
      interfaceBindingDefinition: null,
      idunnDaemonHealthDefinition: null,
      muninnCaptureStreamCommandDefinition: null,
      muninnCaptureStreamDefinition: null,
      muninnCommandBoundaryDefinition: null,
      muninnMoveControllerStateDefinition: null,
      muninnMoveIdentityDefinition: null,
      muninnMoveLightCommandDefinition: null,
      muninnMoveMarkerCandidateDefinition: null,
      muninnObsStreamCatalogDefinition: null,
      muninnQuestAccessDefinition: null,
      muninnTelemetrySurfaceDefinition: null,
      muninnTransportProfileDefinition: null,
      operatorStateDefinition: null,
      providerAdvertisementDefinition: null,
      stonksCommandBoundaryDefinition: null,
      stonksMarketSnapshotDefinition: null,
      stonksRequestEventDefinition: null,
      stonksTransportProfileDefinition: null,
      streamPixelsCommandBoundaryDefinition: null,
      streamPixelsTransportProfileDefinition: null,
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
    name: (value) => value?.providerId || value?.provider?.id || "provider",
    schema: parseObjectDocument("Eve provider advertisement"),
  });
  const idunnDaemonHealthDefinition = defineDocumentType({
    type: "idunn.daemon_health",
    schemaName: "idunn.daemon_health",
    schemaId: "idunn.daemon_health",
    schemaVersion: "idunn.daemon_health.v1",
    global: false,
    name: (value) => value?.daemonId || value?.daemon_id || "daemon",
    schema: parseObjectDocument("Idunn daemon health"),
  });
  const muninnCaptureStreamDefinition = defineDocumentType({
    type: "muninn.capture_stream",
    schemaName: "muninn.capture_stream",
    schemaId: "muninn.capture_stream.v1",
    schemaVersion: "muninn.capture_stream.v1",
    global: false,
    name: (value) => value?.stream_id || value?.streamId || "capture-stream",
    schema: parseObjectDocument("Muninn capture stream"),
  });
  const muninnCaptureStreamCommandDefinition = defineDocumentType({
    type: "muninn.capture_stream_command",
    schemaName: "muninn.capture_stream_command",
    schemaId: "muninn.capture_stream_command.v1",
    schemaVersion: "muninn.capture_stream_command.v1",
    global: false,
    name: (value) => value?.command_id || value?.commandId || value?.stream_id || value?.streamId || "capture-stream-command",
    schema: parseObjectDocument("Muninn capture stream command"),
  });
  const muninnCommandBoundaryDefinition = defineDocumentType({
    type: "muninn.command_boundary",
    schemaName: "muninn.command_boundary",
    schemaId: "muninn.command_boundary.v1",
    schemaVersion: "muninn.command_boundary.v1",
    global: false,
    name: (value) => value?.boundary_id || value?.boundaryId || value?.daemon_id || value?.daemonId || "muninn",
    schema: parseObjectDocument("Muninn command boundary"),
  });
  const muninnMoveControllerStateDefinition = defineDocumentType({
    type: "muninn.move_controller_state",
    schemaName: "muninn.move_controller_state",
    schemaId: "muninn.move_controller_state.v1",
    schemaVersion: "muninn.move_controller_state.v1",
    global: false,
    name: (value) => value?.stream_id || value?.streamId || value?.move_id || value?.moveId || "move-controller-state",
    schema: parseObjectDocument("Muninn Move controller state"),
  });
  const muninnMoveIdentityDefinition = defineDocumentType({
    type: "muninn.move_identity",
    schemaName: "muninn.move_identity",
    schemaId: "muninn.move_identity.v1",
    schemaVersion: "muninn.move_identity.v1",
    global: false,
    name: (value) => value?.identity_id || value?.identityId || value?.move_id || value?.moveId || "move-identity",
    schema: parseObjectDocument("Muninn Move identity"),
  });
  const muninnMoveLightCommandDefinition = defineDocumentType({
    type: "muninn.move_light_command",
    schemaName: "muninn.move_light_command",
    schemaId: "muninn.move_light_command.v1",
    schemaVersion: "muninn.move_light_command.v1",
    global: false,
    name: (value) => value?.command_id || value?.commandId || value?.move_id || value?.moveId || "move-light-command",
    schema: parseObjectDocument("Muninn Move light command"),
  });
  const muninnMoveMarkerCandidateDefinition = defineDocumentType({
    type: "muninn.move_marker_candidate",
    schemaName: "muninn.move_marker_candidate",
    schemaId: "muninn.move_marker_candidate.v1",
    schemaVersion: "muninn.move_marker_candidate.v1",
    global: false,
    name: (value) => value?.stream_id || value?.streamId || value?.camera_id || value?.cameraId || "move-marker-candidate",
    schema: parseObjectDocument("Muninn Move marker candidate"),
  });
  const muninnObsStreamCatalogDefinition = defineDocumentType({
    type: "muninn.obs_stream_catalog",
    schemaName: "muninn.obs_stream_catalog",
    schemaId: "muninn.obs_stream_catalog.v1",
    schemaVersion: "muninn.obs_stream_catalog.v1",
    global: false,
    name: (value) => value?.catalog_id || value?.catalogId || value?.host_id || value?.hostId || "muninn",
    schema: parseObjectDocument("Muninn OBS stream catalog"),
  });
  const muninnQuestAccessDefinition = defineDocumentType({
    type: "muninn.quest_access",
    schemaName: "muninn.quest_access",
    schemaId: "muninn.quest_access.v1",
    schemaVersion: "muninn.quest_access.v1",
    global: false,
    name: (value) => value?.access_id || value?.accessId || value?.serial || "quest-access",
    schema: parseObjectDocument("Muninn Quest access"),
  });
  const muninnTelemetrySurfaceDefinition = defineDocumentType({
    type: "muninn.telemetry_surface",
    schemaName: "muninn.telemetry_surface",
    schemaId: "muninn.telemetry_surface.v1",
    schemaVersion: "muninn.telemetry_surface.v1",
    global: false,
    name: (value) => value?.surface_id || value?.surfaceId || value?.host_id || value?.hostId || "muninn-telemetry",
    schema: parseObjectDocument("Muninn telemetry surface"),
  });
  const muninnTransportProfileDefinition = defineDocumentType({
    type: "muninn.transport_profile",
    schemaName: "muninn.transport_profile",
    schemaId: "muninn.transport_profile.v1",
    schemaVersion: "muninn.transport_profile.v1",
    global: false,
    name: (value) => value?.profile_id || value?.profileId || value?.daemon_id || value?.daemonId || "muninn",
    schema: parseObjectDocument("Muninn transport profile"),
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
  const stonksRequestEventDefinition = defineDocumentType({
    type: "stonks.request_event",
    schemaName: "stonks.request_event",
    schemaId: "stonks.request_event.v1",
    schemaVersion: "stonks.request_event.v1",
    global: false,
    name: (value) => value?.id || "request",
    schema: parseObjectDocument("Stonks request event"),
  });
  const stonksMarketSnapshotDefinition = defineDocumentType({
    type: "stonks.market_snapshot",
    schemaName: "stonks.market_snapshot",
    schemaId: "stonks.market_snapshot.v1",
    schemaVersion: "stonks.market_snapshot.v1",
    global: true,
    schema: parseObjectDocument("Stonks market snapshot"),
  });
  const stonksCommandBoundaryDefinition = defineDocumentType({
    type: "stonks.command_boundary",
    schemaName: "stonks.command_boundary",
    schemaId: "stonks.command_boundary.v1",
    schemaVersion: "stonks.command_boundary.v1",
    global: false,
    name: (value) => value?.boundaryId || value?.daemonId || "stonks",
    schema: parseObjectDocument("Stonks command boundary"),
  });
  const stonksTransportProfileDefinition = defineDocumentType({
    type: "stonks.transport_profile",
    schemaName: "stonks.transport_profile",
    schemaId: "stonks.transport_profile.v1",
    schemaVersion: "stonks.transport_profile.v1",
    global: false,
    name: (value) => value?.profileId || value?.daemonId || "stonks",
    schema: parseObjectDocument("Stonks transport profile"),
  });
  const streamPixelsCommandBoundaryDefinition = defineDocumentType({
    type: "streampixels.command_boundary",
    schemaName: "streampixels.command_boundary",
    schemaId: "streampixels.command_boundary.v1",
    schemaVersion: "streampixels.command_boundary.v1",
    global: false,
    name: (value) => value?.boundaryId || value?.daemonId || "streampixels",
    schema: parseObjectDocument("StreamPixels command boundary"),
  });
  const streamPixelsTransportProfileDefinition = defineDocumentType({
    type: "streampixels.transport_profile",
    schemaName: "streampixels.transport_profile",
    schemaId: "streampixels.transport_profile.v1",
    schemaVersion: "streampixels.transport_profile.v1",
    global: false,
    name: (value) => value?.profileId || value?.daemonId || "streampixels",
    schema: parseObjectDocument("StreamPixels transport profile"),
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
    idunnDaemonHealthDefinition,
    muninnCaptureStreamCommandDefinition,
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
  };
}

module.exports = { defineOdinDocuments };

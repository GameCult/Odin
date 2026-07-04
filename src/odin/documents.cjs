"use strict";

const { parseObjectDocument } = require("./utils.cjs");

function defineOdinDocuments(defineDocumentType) {
  if (!defineDocumentType) {
    return {
      interfaceBindingDefinition: null,
      idunnCommandBoundaryDefinition: null,
      idunnDaemonHealthDefinition: null,
      idunnTransportProfileDefinition: null,
      interfaceLayoutDefinition: null,
      muninnCaptureStreamCommandDefinition: null,
      muninnCaptureStreamDefinition: null,
      muninnCommandBoundaryDefinition: null,
      muninnMediaAudioPacketDefinition: null,
      muninnMediaReceiverFeedbackDefinition: null,
      muninnMediaVideoAccessUnitDefinition: null,
      muninnHidControllerStateDefinition: null,
      muninnMoveControllerStateDefinition: null,
      muninnMoveIdentityDefinition: null,
      muninnMoveLightCommandDefinition: null,
      muninnMoveMarkerCandidateDefinition: null,
      muninnObsStreamCatalogDefinition: null,
      muninnQuestAccessDefinition: null,
      muninnTelemetrySurfaceDefinition: null,
      muninnTransportProfileDefinition: null,
      SleipnirInputMappingDefinition: null,
      aetheriaAssetManifestDefinition: null,
      aetheriaGravityViewportDefinition: null,
      aetheriaObjectsViewportDefinition: null,
      aetheriaRenderSplatsViewportDefinition: null,
      operatorStateDefinition: null,
      providerAdvertisementDefinition: null,
      voidbotProviderCatalogDefinition: null,
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
  const interfaceLayoutDefinition = defineDocumentType({
    type: "odin.interface_layout",
    schemaName: "odin.interface_layout",
    schemaId: "odin.interface_layout.v1",
    schemaVersion: "odin.interface_layout.v1",
    global: false,
    name: (value) => value?.layoutId || value?.providerId || "odin.providers",
    schema: parseObjectDocument("Odin interface layout"),
    members: [
      { slot: 0, memberName: "layoutId", typeName: "string", isName: true },
      { slot: 1, memberName: "updatedAt", typeName: "string" },
      { slot: 2, memberName: "tiles", typeName: "object" },
    ],
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
  const idunnCommandBoundaryDefinition = defineDocumentType({
    type: "idunn.command_boundary",
    schemaName: "idunn.command_boundary",
    schemaId: "idunn.command_boundary.v1",
    schemaVersion: "idunn.command_boundary.v1",
    global: false,
    name: (value) => value?.boundary_id || value?.boundaryId || value?.daemon_id || value?.daemonId || "daemon",
    schema: parseObjectDocument("Idunn command boundary"),
  });
  const idunnTransportProfileDefinition = defineDocumentType({
    type: "idunn.daemon_transport_profile",
    schemaName: "idunn.daemon_transport_profile",
    schemaId: "idunn.daemon_transport_profile.v1",
    schemaVersion: "idunn.daemon_transport_profile.v1",
    global: false,
    name: (value) => value?.profile_id || value?.profileId || value?.daemon_id || value?.daemonId || "daemon",
    schema: parseObjectDocument("Idunn daemon transport profile"),
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
  const muninnMediaVideoAccessUnitDefinition = defineDocumentType({
    type: "muninn.media_video_access_unit",
    schemaName: "muninn.media_video_access_unit",
    schemaId: "muninn.media_video_access_unit.v1",
    schemaVersion: "muninn.media_video_access_unit.v1",
    global: false,
    name: (value) => {
      const streamId = value?.stream_id || value?.streamId || "stream";
      const sessionId = value?.session_id || value?.sessionId || "session";
      const frameId = value?.frame_id ?? value?.frameId ?? "frame";
      const chunkIndex = value?.chunk_index ?? value?.chunkIndex ?? "chunk";
      return `${streamId}:${sessionId}:video:${frameId}:${chunkIndex}`;
    },
    schema: parseObjectDocument("Muninn media video access unit"),
  });
  const muninnMediaAudioPacketDefinition = defineDocumentType({
    type: "muninn.media_audio_packet",
    schemaName: "muninn.media_audio_packet",
    schemaId: "muninn.media_audio_packet.v1",
    schemaVersion: "muninn.media_audio_packet.v1",
    global: false,
    name: (value) => {
      const streamId = value?.stream_id || value?.streamId || "stream";
      const sessionId = value?.session_id || value?.sessionId || "session";
      const packetId = value?.packet_id ?? value?.packetId ?? "packet";
      return `${streamId}:${sessionId}:audio:${packetId}`;
    },
    schema: parseObjectDocument("Muninn media audio packet"),
  });
  const muninnMediaReceiverFeedbackDefinition = defineDocumentType({
    type: "muninn.media_receiver_feedback",
    schemaName: "muninn.media_receiver_feedback",
    schemaId: "muninn.media_receiver_feedback.v1",
    schemaVersion: "muninn.media_receiver_feedback.v1",
    global: false,
    name: (value) => {
      const streamId = value?.stream_id || value?.streamId || "stream";
      const sessionId = value?.session_id || value?.sessionId || "session";
      const receiverId = value?.receiver_id || value?.receiverId || "receiver";
      return `${streamId}:${sessionId}:feedback:${receiverId}`;
    },
    schema: parseObjectDocument("Muninn media receiver feedback"),
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
  const muninnHidControllerStateDefinition = defineDocumentType({
    type: "muninn.hid_controller_state",
    schemaName: "muninn.hid_controller_state",
    schemaId: "muninn.hid_controller_state.v1",
    schemaVersion: "muninn.hid_controller_state.v1",
    global: false,
    name: (value) => value?.stream_id || value?.streamId || value?.device_id || value?.deviceId || "hid-controller-state",
    schema: parseObjectDocument("Muninn HID controller state"),
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
  const SleipnirInputMappingDefinition = defineDocumentType({
    type: "sleipnir.input_mapping",
    schemaName: "sleipnir.input_mapping",
    schemaId: "sleipnir.input_mapping.v1",
    schemaVersion: "sleipnir.input_mapping.v1",
    global: false,
    name: (value) => value?.providerId || value?.provider_id || "sleipnir",
    schema: parseObjectDocument("Sleipnir input mapping"),
  });
  const aetheriaAssetManifestDefinition = defineDocumentType({
    type: "gamecult.aetheria.asset_manifest",
    schemaName: "gamecult.aetheria.asset_manifest",
    schemaId: "gamecult.aetheria.asset_manifest.v1",
    schemaVersion: "gamecult.aetheria.asset_manifest.v1",
    global: false,
    name: (value) => value?.runId || value?.run_id || "aetheria-assets",
    schema: parseObjectDocument("Aetheria asset manifest"),
  });
  const aetheriaGravityViewportDefinition = defineDocumentType({
    type: "gamecult.aetheria.gravity_viewport",
    schemaName: "gamecult.aetheria.gravity_viewport",
    schemaId: "gamecult.aetheria.gravity_viewport.v1",
    schemaVersion: "gamecult.aetheria.gravity_viewport.v1",
    global: false,
    name: (value) => value?.viewportId || value?.viewport_id || "aetheria-gravity",
    schema: parseObjectDocument("Aetheria gravity viewport"),
  });
  const aetheriaObjectsViewportDefinition = defineDocumentType({
    type: "gamecult.aetheria.objects_viewport",
    schemaName: "gamecult.aetheria.objects_viewport",
    schemaId: "gamecult.aetheria.objects_viewport.v1",
    schemaVersion: "gamecult.aetheria.objects_viewport.v1",
    global: false,
    name: (value) => value?.viewportId || value?.viewport_id || "aetheria-objects",
    schema: parseObjectDocument("Aetheria objects viewport"),
  });
  const aetheriaRenderSplatsViewportDefinition = defineDocumentType({
    type: "gamecult.aetheria.render_splats_viewport",
    schemaName: "gamecult.aetheria.render_splats_viewport",
    schemaId: "gamecult.aetheria.render_splats_viewport.v1",
    schemaVersion: "gamecult.aetheria.render_splats_viewport.v1",
    global: false,
    name: (value) => value?.viewportId || value?.viewport_id || "aetheria-render-splats",
    schema: parseObjectDocument("Aetheria render splats viewport"),
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
  const voidbotProviderCatalogDefinition = defineDocumentType({
    type: "voidbot.provider_advertisement_catalog",
    schemaName: "voidbot.provider_advertisement_catalog",
    schemaId: "voidbot.provider_advertisement_catalog.v0",
    schemaVersion: "voidbot.provider_advertisement_catalog.v0",
    global: true,
    schema: parseObjectDocument("VoidBot provider advertisement catalog"),
  });

  return {
    interfaceBindingDefinition,
    interfaceLayoutDefinition,
    idunnCommandBoundaryDefinition,
    idunnDaemonHealthDefinition,
    idunnTransportProfileDefinition,
    muninnCaptureStreamCommandDefinition,
    muninnCaptureStreamDefinition,
    muninnCommandBoundaryDefinition,
    muninnMediaAudioPacketDefinition,
    muninnMediaReceiverFeedbackDefinition,
    muninnMediaVideoAccessUnitDefinition,
    muninnHidControllerStateDefinition,
    muninnMoveControllerStateDefinition,
    muninnMoveIdentityDefinition,
    muninnMoveLightCommandDefinition,
    muninnMoveMarkerCandidateDefinition,
    muninnObsStreamCatalogDefinition,
    muninnQuestAccessDefinition,
    muninnTelemetrySurfaceDefinition,
    muninnTransportProfileDefinition,
    SleipnirInputMappingDefinition,
    aetheriaAssetManifestDefinition,
    aetheriaGravityViewportDefinition,
    aetheriaObjectsViewportDefinition,
    aetheriaRenderSplatsViewportDefinition,
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
    voidbotProviderCatalogDefinition,
    voidbotSwarmSnapshotDefinition,
  };
}

module.exports = { defineOdinDocuments };

use cultcache_rs::DatabaseEntry;
use serde_json::Value;

pub const ODIN_SNAPSHOT_SCHEMA: &str = "odin.snapshot.v1";
pub const ODIN_VERSE_SCHEMA: &str = "odin.verse.v1";
pub const ODIN_SERVICE_SCHEMA: &str = "odin.service.v1";
pub const ODIN_INTERFACE_SCHEMA: &str = "odin.interface.v1";
pub const ODIN_OBSERVATION_STREAM_SCHEMA: &str = "odin.observation_stream.v1";
pub const ODIN_TRANSLATION_ROUTE_SCHEMA: &str = "odin.translation_route.v1";
pub const EVE_SURFACE_STATE_SCHEMA: &str = "gamecult.eve.surface_state.v1";
pub const EVE_INTERFACE_BINDING_SCHEMA: &str = "gamecult.eve.interface_binding.v1";
pub const EVE_PROVIDER_ADVERTISEMENT_SCHEMA: &str = "gamecult.eve.provider_advertisement.v1";
pub const VOIDBOT_SWARM_STATE_SNAPSHOT_SCHEMA: &str = "voidbot.swarm_state_snapshot.v1";
pub const IDUNN_DESIRED_DAEMON_SCHEMA: &str = "idunn.desired_daemon.v1";
pub const IDUNN_DAEMON_HEALTH_SCHEMA: &str = "idunn.daemon_health.v1";
pub const IDUNN_KEEPALIVE_DECISION_SCHEMA: &str = "idunn.keepalive_decision.v1";
pub const IDUNN_RESTART_REQUEST_SCHEMA: &str = "idunn.restart_request.v1";
pub const IDUNN_RESTART_RESULT_SCHEMA: &str = "idunn.restart_result.v1";
pub const IDUNN_DEPLOYMENT_REQUEST_SCHEMA: &str = "idunn.deployment_request.v1";
pub const IDUNN_DEPLOYMENT_RESULT_SCHEMA: &str = "idunn.deployment_result.v1";
pub const IDUNN_LIFECYCLE_COMMAND_SCHEMA: &str = "idunn.lifecycle_command.v1";
pub const IDUNN_RELEASE_TARGET_SCHEMA: &str = "idunn.release_target.v1";
pub const IDUNN_DEPLOYMENT_ARTIFACT_SCHEMA: &str = "idunn.deployment_artifact.v1";
pub const IDUNN_STATE_MIGRATION_PLAN_SCHEMA: &str = "idunn.state_migration_plan.v1";
pub const IDUNN_STATE_MIGRATION_RESULT_SCHEMA: &str = "idunn.state_migration_result.v1";
pub const IDUNN_ROLLOUT_PLAN_SCHEMA: &str = "idunn.rollout_plan.v1";
pub const IDUNN_ROLLOUT_RESULT_SCHEMA: &str = "idunn.rollout_result.v1";
pub const IDUNN_OPERATOR_ALARM_SCHEMA: &str = "idunn.operator_alarm.v1";
pub const IDUNN_SWARM_SURGERY_PLAN_SCHEMA: &str = "idunn.swarm_surgery_plan.v1";
pub const IDUNN_DAEMON_SURGERY_PLAN_SCHEMA: &str = "idunn.daemon_surgery_plan.v1";
pub const IDUNN_DAEMON_TRANSPORT_PROFILE_SCHEMA: &str = "idunn.daemon_transport_profile.v1";
pub const IDUNN_COMMAND_BOUNDARY_SCHEMA: &str = "idunn.command_boundary.v1";
pub const IDUNN_RUNTIME_TRANSPORT_CHECK_SCHEMA: &str = "idunn.runtime_transport_check.v1";
pub const IDUNN_RUDP_HEALTH_INGRESS_SCHEMA: &str = "idunn.rudp_health_ingress.v1";
pub const MUNINN_TELEMETRY_SURFACE_SCHEMA: &str = "muninn.telemetry_surface.v1";
pub const MUNINN_CAPTURE_STREAM_SCHEMA: &str = "muninn.capture_stream.v1";
pub const MUNINN_CAPTURE_STREAM_COMMAND_SCHEMA: &str = "muninn.capture_stream_command.v1";
pub const MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA: &str = "muninn.media_video_access_unit.v1";
pub const MUNINN_MEDIA_VIDEO_PARITY_SHARD_SCHEMA: &str = "muninn.media_video_parity_shard.v2";
pub const MUNINN_MEDIA_AUDIO_PACKET_SCHEMA: &str = "muninn.media_audio_packet.v1";
pub const MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA: &str = "muninn.media_receiver_feedback.v1";
pub const MUNINN_OBS_STREAM_CATALOG_SCHEMA: &str = "muninn.obs_stream_catalog.v1";
pub const MUNINN_MOVE_MARKER_CANDIDATE_SCHEMA: &str = "muninn.move_marker_candidate.v1";
pub const MUNINN_MOVE_CONTROLLER_STATE_SCHEMA: &str = "muninn.move_controller_state.v1";
pub const MUNINN_HID_CONTROLLER_STATE_SCHEMA: &str = "muninn.hid_controller_state.v1";
pub const MUNINN_MOVE_IDENTITY_SCHEMA: &str = "muninn.move_identity.v1";
pub const MUNINN_MOVE_LIGHT_COMMAND_SCHEMA: &str = "muninn.move_light_command.v1";
pub const MUNINN_MOVE_HUE_PROGRAM_SCHEMA: &str = "muninn.move_hue_program.v1";
pub const MUNINN_MOVE_TRACKER_HEALTH_SCHEMA: &str = "muninn.move_tracker_health.v1";
pub const MUNINN_MOVE_EVIDENCE_TRANSPORT_HEALTH_SCHEMA: &str = "muninn.move_evidence_transport_health.v1";
pub const MUNINN_QUEST_ACCESS_SCHEMA: &str = "muninn.quest_access.v1";
pub const MUNINN_COMMAND_BOUNDARY_SCHEMA: &str = "muninn.command_boundary.v1";
pub const MUNINN_TRANSPORT_PROFILE_SCHEMA: &str = "muninn.transport_profile.v1";
pub const SLEIPNIR_INPUT_MAPPING_SCHEMA: &str = "sleipnir.input_mapping.v1";

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "odin.snapshot", schema = "odin.snapshot.v1")]
pub struct OdinSnapshotRecord {
    #[cultcache(key = 0)]
    pub snapshot_id: String,
    #[cultcache(key = 1)]
    pub observed_at: String,
    #[cultcache(key = 2)]
    pub verse_count: u32,
    #[cultcache(key = 3)]
    pub service_count: u32,
    #[cultcache(key = 4)]
    pub interface_count: u32,
    #[cultcache(key = 5)]
    pub observation_stream_count: u32,
    #[cultcache(key = 6)]
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "odin.verse", schema = "odin.verse.v1")]
pub struct OdinVerseRecord {
    #[cultcache(key = 0)]
    pub verse_id: String,
    #[cultcache(key = 1)]
    pub name: String,
    #[cultcache(key = 2)]
    pub role: String,
    #[cultcache(key = 3)]
    pub status: String,
    #[cultcache(key = 4)]
    pub capabilities: Vec<String>,
    #[cultcache(key = 5)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "odin.service", schema = "odin.service.v1")]
pub struct OdinServiceRecord {
    #[cultcache(key = 0)]
    pub service_id: String,
    #[cultcache(key = 1)]
    pub verse_id: String,
    #[cultcache(key = 2)]
    pub name: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub detail: String,
    #[cultcache(key = 5)]
    pub authority: String,
    #[cultcache(key = 6)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "odin.interface", schema = "odin.interface.v1")]
pub struct OdinInterfaceRecord {
    #[cultcache(key = 0)]
    pub provider_id: String,
    #[cultcache(key = 1)]
    pub title: String,
    #[cultcache(key = 2)]
    pub state: String,
    #[cultcache(key = 3)]
    pub source: String,
    #[cultcache(key = 4)]
    pub version: Option<String>,
    #[cultcache(key = 5)]
    pub updated_at: Option<String>,
    #[cultcache(key = 6)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "odin.observation_stream",
    schema = "odin.observation_stream.v1"
)]
pub struct OdinObservationStreamRecord {
    #[cultcache(key = 0)]
    pub stream_key: String,
    #[cultcache(key = 1)]
    pub device_id: String,
    #[cultcache(key = 2)]
    pub stream_id: String,
    #[cultcache(key = 3)]
    pub kind: String,
    #[cultcache(key = 4)]
    pub state: String,
    #[cultcache(key = 5)]
    pub detail: String,
    #[cultcache(key = 6)]
    pub owner: String,
    #[cultcache(key = 7)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "odin.translation_route", schema = "odin.translation_route.v1")]
pub struct OdinTranslationRouteRecord {
    #[cultcache(key = 0)]
    pub route_id: String,
    #[cultcache(key = 1)]
    pub source_schema: String,
    #[cultcache(key = 2)]
    pub target_schema: String,
    #[cultcache(key = 3)]
    pub translation_kind: String,
    #[cultcache(key = 4)]
    pub owner: String,
    #[cultcache(key = 5)]
    pub version: String,
    #[cultcache(key = 6)]
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "gamecult.eve.surface_state",
    schema = "gamecult.eve.surface_state.v1"
)]
pub struct EveSurfaceStateRecord {
    #[cultcache(key = 0)]
    pub provider_id: String,
    #[cultcache(key = 1)]
    pub title: String,
    #[cultcache(key = 2)]
    pub version: i64,
    #[cultcache(key = 3)]
    pub updated_at: String,
    #[cultcache(key = 4)]
    pub surface: Value,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "gamecult.eve.interface_binding",
    schema = "gamecult.eve.interface_binding.v1"
)]
pub struct EveInterfaceBindingCompatRecord {
    #[cultcache(key = 0)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "gamecult.eve.provider_advertisement",
    schema = "gamecult.eve.provider_advertisement.v1"
)]
pub struct EveProviderAdvertisementRecord {
    #[cultcache(key = 0)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "voidbot.swarm_state_snapshot",
    schema = "voidbot.swarm_state_snapshot.v1"
)]
pub struct VoidBotSwarmStateSnapshotCompatRecord {
    #[cultcache(key = 0)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.desired_daemon", schema = "idunn.desired_daemon.v1")]
pub struct IdunnDesiredDaemonRecord {
    #[cultcache(key = 0)]
    pub daemon_id: String,
    #[cultcache(key = 1)]
    pub verse_id: String,
    #[cultcache(key = 2)]
    pub name: String,
    #[cultcache(key = 3)]
    pub enabled: bool,
    #[cultcache(key = 4)]
    pub health_command: Option<String>,
    #[cultcache(key = 5)]
    pub restart_command: Option<String>,
    #[cultcache(key = 6)]
    pub authority: String,
    #[cultcache(key = 7)]
    pub max_silence_seconds: u32,
    #[cultcache(key = 8)]
    pub observed_at: String,
    #[cultcache(key = 9)]
    pub deploy_command: Option<String>,
    #[cultcache(key = 10)]
    pub health_contract: String,
    #[cultcache(key = 11, default)]
    pub transport_profile_id: String,
    #[cultcache(key = 12, default)]
    pub command_boundary_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.daemon_health", schema = "idunn.daemon_health.v1")]
pub struct IdunnDaemonHealthRecord {
    #[cultcache(key = 0)]
    pub daemon_id: String,
    #[cultcache(key = 1)]
    pub state: String,
    #[cultcache(key = 2)]
    pub detail: String,
    #[cultcache(key = 3)]
    pub observed_at: String,
    #[cultcache(key = 4)]
    pub health_contract: String,
    #[cultcache(key = 5, default)]
    pub publication_source: String,
    #[cultcache(key = 6, default)]
    pub transport: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.keepalive_decision",
    schema = "idunn.keepalive_decision.v1"
)]
pub struct IdunnKeepaliveDecisionRecord {
    #[cultcache(key = 0)]
    pub decision_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub action: String,
    #[cultcache(key = 3)]
    pub reason: String,
    #[cultcache(key = 4)]
    pub authority: String,
    #[cultcache(key = 5)]
    pub decided_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.restart_request", schema = "idunn.restart_request.v1")]
pub struct IdunnRestartRequestRecord {
    #[cultcache(key = 0)]
    pub request_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub command: String,
    #[cultcache(key = 3)]
    pub authority: String,
    #[cultcache(key = 4)]
    pub requested_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.restart_result", schema = "idunn.restart_result.v1")]
pub struct IdunnRestartResultRecord {
    #[cultcache(key = 0)]
    pub result_id: String,
    #[cultcache(key = 1)]
    pub request_id: String,
    #[cultcache(key = 2)]
    pub daemon_id: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub detail: String,
    #[cultcache(key = 5)]
    pub completed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.deployment_request",
    schema = "idunn.deployment_request.v1"
)]
pub struct IdunnDeploymentRequestRecord {
    #[cultcache(key = 0)]
    pub request_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub command: String,
    #[cultcache(key = 3)]
    pub authority: String,
    #[cultcache(key = 4)]
    pub requested_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.deployment_result",
    schema = "idunn.deployment_result.v1"
)]
pub struct IdunnDeploymentResultRecord {
    #[cultcache(key = 0)]
    pub result_id: String,
    #[cultcache(key = 1)]
    pub request_id: String,
    #[cultcache(key = 2)]
    pub daemon_id: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub detail: String,
    #[cultcache(key = 5)]
    pub completed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.lifecycle_command",
    schema = "idunn.lifecycle_command.v1"
)]
pub struct IdunnLifecycleCommandRecord {
    #[cultcache(key = 0)]
    pub command_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub action: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub requested_by: String,
    #[cultcache(key = 5)]
    pub requested_at: String,
    #[cultcache(key = 6)]
    pub detail: String,
    #[cultcache(key = 7, default)]
    pub claimed_at: String,
    #[cultcache(key = 8, default)]
    pub result_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.release_target", schema = "idunn.release_target.v1")]
pub struct IdunnReleaseTargetRecord {
    #[cultcache(key = 0)]
    pub target_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub repo: String,
    #[cultcache(key = 3)]
    pub repo_path: String,
    #[cultcache(key = 4)]
    pub upstream_remote: String,
    #[cultcache(key = 5)]
    pub upstream_branch: String,
    #[cultcache(key = 6)]
    pub desired_revision: String,
    #[cultcache(key = 7)]
    pub deployed_revision: String,
    #[cultcache(key = 8)]
    pub artifact_strategy: String,
    #[cultcache(key = 9)]
    pub rollout_strategy: String,
    #[cultcache(key = 10)]
    pub state_migration_authority: String,
    #[cultcache(key = 11)]
    pub zero_downtime_capability: String,
    #[cultcache(key = 12)]
    pub status: String,
    #[cultcache(key = 13)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.deployment_artifact",
    schema = "idunn.deployment_artifact.v1"
)]
pub struct IdunnDeploymentArtifactRecord {
    #[cultcache(key = 0)]
    pub artifact_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub source_revision: String,
    #[cultcache(key = 3)]
    pub source_branch: String,
    #[cultcache(key = 4)]
    pub source_remote: String,
    #[cultcache(key = 5)]
    pub artifact_kind: String,
    #[cultcache(key = 6)]
    pub artifact_uri: String,
    #[cultcache(key = 7)]
    pub sha256: String,
    #[cultcache(key = 8)]
    pub built_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.state_migration_plan",
    schema = "idunn.state_migration_plan.v1"
)]
pub struct IdunnStateMigrationPlanRecord {
    #[cultcache(key = 0)]
    pub plan_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub from_schema_version: String,
    #[cultcache(key = 3)]
    pub to_schema_version: String,
    #[cultcache(key = 4)]
    pub authority: String,
    #[cultcache(key = 5)]
    pub command: String,
    #[cultcache(key = 6)]
    pub strategy: String,
    #[cultcache(key = 7)]
    pub backup_required: bool,
    #[cultcache(key = 8)]
    pub zero_downtime_required: bool,
    #[cultcache(key = 9)]
    pub status: String,
    #[cultcache(key = 10)]
    pub planned_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.state_migration_result",
    schema = "idunn.state_migration_result.v1"
)]
pub struct IdunnStateMigrationResultRecord {
    #[cultcache(key = 0)]
    pub result_id: String,
    #[cultcache(key = 1)]
    pub plan_id: String,
    #[cultcache(key = 2)]
    pub daemon_id: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub detail: String,
    #[cultcache(key = 5)]
    pub completed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.rollout_plan", schema = "idunn.rollout_plan.v1")]
pub struct IdunnRolloutPlanRecord {
    #[cultcache(key = 0)]
    pub plan_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub desired_revision: String,
    #[cultcache(key = 3)]
    pub deployed_revision: String,
    #[cultcache(key = 4)]
    pub strategy: String,
    #[cultcache(key = 5)]
    pub phases: Vec<String>,
    #[cultcache(key = 6)]
    pub migration_plan_id: String,
    #[cultcache(key = 7)]
    pub artifact_id: String,
    #[cultcache(key = 8)]
    pub status: String,
    #[cultcache(key = 9)]
    pub planned_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.rollout_result", schema = "idunn.rollout_result.v1")]
pub struct IdunnRolloutResultRecord {
    #[cultcache(key = 0)]
    pub result_id: String,
    #[cultcache(key = 1)]
    pub plan_id: String,
    #[cultcache(key = 2)]
    pub daemon_id: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub detail: String,
    #[cultcache(key = 5)]
    pub completed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.operator_alarm", schema = "idunn.operator_alarm.v1")]
pub struct IdunnOperatorAlarmRecord {
    #[cultcache(key = 0)]
    pub alarm_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub severity: String,
    #[cultcache(key = 3)]
    pub reason: String,
    #[cultcache(key = 4)]
    pub escalation_target: String,
    #[cultcache(key = 5)]
    pub raised_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.swarm_surgery_plan",
    schema = "idunn.swarm_surgery_plan.v1"
)]
pub struct IdunnSwarmSurgeryPlanRecord {
    #[cultcache(key = 0)]
    pub plan_id: String,
    #[cultcache(key = 1)]
    pub profile: String,
    #[cultcache(key = 2)]
    pub status: String,
    #[cultcache(key = 3)]
    pub owner: String,
    #[cultcache(key = 4)]
    pub objective: String,
    #[cultcache(key = 5)]
    pub current_mechanism: String,
    #[cultcache(key = 6)]
    pub invariants: Vec<String>,
    #[cultcache(key = 7)]
    pub phases: Vec<String>,
    #[cultcache(key = 8)]
    pub current_phase: String,
    #[cultcache(key = 9)]
    pub next_target: String,
    #[cultcache(key = 10)]
    pub cut_line: String,
    #[cultcache(key = 11)]
    pub verification_layer: String,
    #[cultcache(key = 12)]
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.daemon_surgery_plan",
    schema = "idunn.daemon_surgery_plan.v1"
)]
pub struct IdunnDaemonSurgeryPlanRecord {
    #[cultcache(key = 0)]
    pub plan_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub severity: String,
    #[cultcache(key = 3)]
    pub status: String,
    #[cultcache(key = 4)]
    pub owner: String,
    #[cultcache(key = 5)]
    pub objective: String,
    #[cultcache(key = 6)]
    pub current_mechanism: String,
    #[cultcache(key = 7)]
    pub intended_authority: String,
    #[cultcache(key = 8)]
    pub cut_line: String,
    #[cultcache(key = 9)]
    pub steps: Vec<String>,
    #[cultcache(key = 10)]
    pub blockers: Vec<String>,
    #[cultcache(key = 11)]
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.daemon_transport_profile",
    schema = "idunn.daemon_transport_profile.v1"
)]
pub struct IdunnDaemonTransportProfileRecord {
    #[cultcache(key = 0)]
    pub profile_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub target_transport: String,
    #[cultcache(key = 3)]
    pub current_transport: String,
    #[cultcache(key = 4)]
    pub state: String,
    #[cultcache(key = 5)]
    pub health_contract: String,
    #[cultcache(key = 6)]
    pub publication_schema: String,
    #[cultcache(key = 7)]
    pub debug_mechanism: String,
    #[cultcache(key = 8)]
    pub cut_line: String,
    #[cultcache(key = 9)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "idunn.command_boundary", schema = "idunn.command_boundary.v1")]
pub struct IdunnCommandBoundaryRecord {
    #[cultcache(key = 0)]
    pub boundary_id: String,
    #[cultcache(key = 1)]
    pub daemon_id: String,
    #[cultcache(key = 2)]
    pub owner: String,
    #[cultcache(key = 3)]
    pub restart_authority: String,
    #[cultcache(key = 4)]
    pub deploy_authority: String,
    #[cultcache(key = 5)]
    pub health_authority: String,
    #[cultcache(key = 6)]
    pub alarm_authority: String,
    #[cultcache(key = 7)]
    pub command_lowerings: Vec<String>,
    #[cultcache(key = 8)]
    pub forbidden_authority: String,
    #[cultcache(key = 9)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.runtime_transport_check",
    schema = "idunn.runtime_transport_check.v1"
)]
pub struct IdunnRuntimeTransportCheckRecord {
    #[cultcache(key = 0)]
    pub check_id: String,
    #[cultcache(key = 1)]
    pub runtime_id: String,
    #[cultcache(key = 2)]
    pub transport: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub detail: String,
    #[cultcache(key = 5)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.rudp_health_ingress",
    schema = "idunn.rudp_health_ingress.v1"
)]
pub struct IdunnRudpHealthIngressRecord {
    #[cultcache(key = 0)]
    pub ingress_id: String,
    #[cultcache(key = 1)]
    pub bind_address: String,
    #[cultcache(key = 2)]
    pub transport: String,
    #[cultcache(key = 3)]
    pub accepted_schema: String,
    #[cultcache(key = 4)]
    pub state: String,
    #[cultcache(key = 5)]
    pub detail: String,
    #[cultcache(key = 6)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "muninn.capture_stream", schema = "muninn.capture_stream.v1")]
pub struct MuninnCaptureStreamRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub state: String,
    #[cultcache(key = 3)]
    pub video_source: String,
    #[cultcache(key = 4)]
    pub audio_source: String,
    #[cultcache(key = 5)]
    pub transport: String,
    #[cultcache(key = 6)]
    pub targets: Vec<String>,
    #[cultcache(key = 7)]
    pub command_witness: String,
    #[cultcache(key = 8)]
    pub supervisor_pid: Option<u32>,
    #[cultcache(key = 9)]
    pub mux_pid: Option<u32>,
    #[cultcache(key = 10)]
    pub restart_count: u32,
    #[cultcache(key = 11)]
    pub detail: String,
    #[cultcache(key = 12)]
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.capture_stream_command",
    schema = "muninn.capture_stream_command.v1"
)]
pub struct MuninnCaptureStreamCommandRecord {
    #[cultcache(key = 0)]
    pub command_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub stream_id: String,
    #[cultcache(key = 3)]
    pub state: String,
    #[cultcache(key = 4)]
    pub action: String,
    #[cultcache(key = 5)]
    pub target_host: String,
    #[cultcache(key = 6)]
    pub port: u16,
    #[cultcache(key = 7)]
    pub obs_target_host: Option<String>,
    #[cultcache(key = 8)]
    pub obs_port: u16,
    #[cultcache(key = 9)]
    pub media_transport: String,
    #[cultcache(key = 10)]
    pub media_packet_bytes: u32,
    #[cultcache(key = 11)]
    pub requested_by: String,
    #[cultcache(key = 12)]
    pub detail: String,
    #[cultcache(key = 13)]
    pub updated_at: String,
    #[cultcache(key = 14, default)]
    pub rudp_video_bitrate_kbps: u32,
    #[cultcache(key = 15, default)]
    pub rudp_latency_budget_ms: u32,
    #[cultcache(key = 16, default)]
    pub video_source_id: String,
    #[cultcache(key = 17, default)]
    pub audio_source_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.media_video_access_unit",
    schema = "muninn.media_video_access_unit.v1"
)]
pub struct MuninnMediaVideoAccessUnitRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub session_id: String,
    #[cultcache(key = 2)]
    pub frame_id: u64,
    #[cultcache(key = 3)]
    pub codec: String,
    #[cultcache(key = 4)]
    pub pts_ticks: i64,
    #[cultcache(key = 5)]
    pub duration_ticks: u32,
    #[cultcache(key = 6)]
    pub timebase_num: u32,
    #[cultcache(key = 7)]
    pub timebase_den: u32,
    #[cultcache(key = 8)]
    pub keyframe: bool,
    #[cultcache(key = 9)]
    pub dependency_frame_id: Option<u64>,
    #[cultcache(key = 10)]
    pub deadline_ticks: i64,
    #[cultcache(key = 11)]
    pub chunk_index: u16,
    #[cultcache(key = 12)]
    pub chunk_count: u16,
    #[cultcache(key = 13)]
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.media_video_parity_shard",
    schema = "muninn.media_video_parity_shard.v2"
)]
pub struct MuninnMediaVideoParityShardRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub session_id: String,
    #[cultcache(key = 2)]
    pub frame_id: u64,
    #[cultcache(key = 3)]
    pub codec: String,
    #[cultcache(key = 4)]
    pub pts_ticks: i64,
    #[cultcache(key = 5)]
    pub duration_ticks: u32,
    #[cultcache(key = 6)]
    pub timebase_num: u32,
    #[cultcache(key = 7)]
    pub timebase_den: u32,
    #[cultcache(key = 8)]
    pub keyframe: bool,
    #[cultcache(key = 9)]
    pub dependency_frame_id: Option<u64>,
    #[cultcache(key = 10)]
    pub deadline_ticks: i64,
    #[cultcache(key = 11)]
    pub chunk_count: u16,
    #[cultcache(key = 12)]
    pub parity_index: u16,
    #[cultcache(key = 13)]
    pub parity_count: u16,
    #[cultcache(key = 14)]
    pub chunk_payload_bytes: u32,
    #[cultcache(key = 15)]
    pub last_chunk_payload_bytes: u32,
    #[cultcache(key = 16)]
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.media_audio_packet",
    schema = "muninn.media_audio_packet.v1"
)]
pub struct MuninnMediaAudioPacketRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub session_id: String,
    #[cultcache(key = 2)]
    pub packet_id: u64,
    #[cultcache(key = 3)]
    pub codec: String,
    #[cultcache(key = 4)]
    pub pts_ticks: i64,
    #[cultcache(key = 5)]
    pub duration_ticks: u32,
    #[cultcache(key = 6)]
    pub timebase_num: u32,
    #[cultcache(key = 7)]
    pub timebase_den: u32,
    #[cultcache(key = 8)]
    pub deadline_ticks: i64,
    #[cultcache(key = 9)]
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.media_receiver_feedback",
    schema = "muninn.media_receiver_feedback.v1"
)]
pub struct MuninnMediaReceiverFeedbackRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub session_id: String,
    #[cultcache(key = 2)]
    pub receiver_id: String,
    #[cultcache(key = 3)]
    pub highest_decodable_frame_id: Option<u64>,
    #[cultcache(key = 4)]
    pub missing_frame_ids: Vec<u64>,
    #[cultcache(key = 5)]
    pub late_frame_ids: Vec<u64>,
    #[cultcache(key = 6)]
    pub requested_keyframe: bool,
    #[cultcache(key = 7)]
    pub jitter_us: i64,
    #[cultcache(key = 8)]
    pub decode_queue_us: i64,
    #[cultcache(key = 9)]
    pub observed_at: String,
    #[cultcache(key = 10, default)]
    pub missing_video_chunk_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.telemetry_surface",
    schema = "muninn.telemetry_surface.v1"
)]
pub struct MuninnTelemetrySurfaceRecord {
    #[cultcache(key = 0)]
    pub surface_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub state: String,
    #[cultcache(key = 3)]
    pub available_sources: Vec<String>,
    #[cultcache(key = 4)]
    pub stream_affordances: Vec<String>,
    #[cultcache(key = 5)]
    pub active_streams: Vec<String>,
    #[cultcache(key = 6)]
    pub activation_authority: String,
    #[cultcache(key = 7)]
    pub detail: String,
    #[cultcache(key = 8)]
    pub updated_at: String,
    #[cultcache(key = 9, default)]
    pub primary_stream_id: String,
    #[cultcache(key = 10, default)]
    pub primary_stream_label: String,
    #[cultcache(key = 11, default)]
    pub command_rudp_target: String,
    #[cultcache(key = 12, default)]
    pub media_target_host: String,
    #[cultcache(key = 13, default)]
    pub media_port: u16,
    #[cultcache(key = 14, default)]
    pub media_packet_bytes: u32,
    #[cultcache(key = 15, default)]
    pub rudp_video_bitrate_kbps: u32,
    #[cultcache(key = 16, default)]
    pub rudp_latency_budget_ms: u32,
    #[cultcache(key = 17, default)]
    pub video_source_ids: Vec<String>,
    #[cultcache(key = 18, default)]
    pub video_source_labels: Vec<String>,
    #[cultcache(key = 19, default)]
    pub audio_source_ids: Vec<String>,
    #[cultcache(key = 20, default)]
    pub audio_source_labels: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.obs_stream_catalog",
    schema = "muninn.obs_stream_catalog.v1"
)]
pub struct MuninnObsStreamCatalogRecord {
    #[cultcache(key = 0)]
    pub catalog_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub stream_ids: Vec<String>,
    #[cultcache(key = 3)]
    pub labels: Vec<String>,
    #[cultcache(key = 4)]
    pub urls: Vec<String>,
    #[cultcache(key = 5)]
    pub states: Vec<String>,
    #[cultcache(key = 6)]
    pub updated_at: String,
    #[cultcache(key = 7, default)]
    pub command_rudp_target: String,
    #[cultcache(key = 8, default)]
    pub media_target_host: String,
    #[cultcache(key = 9, default)]
    pub media_port: u16,
    #[cultcache(key = 10, default)]
    pub media_packet_bytes: u32,
    #[cultcache(key = 11, default)]
    pub rudp_video_bitrate_kbps: u32,
    #[cultcache(key = 12, default)]
    pub rudp_latency_budget_ms: u32,
    #[cultcache(key = 13, default)]
    pub video_source_ids: Vec<String>,
    #[cultcache(key = 14, default)]
    pub video_source_labels: Vec<String>,
    #[cultcache(key = 15, default)]
    pub audio_source_ids: Vec<String>,
    #[cultcache(key = 16, default)]
    pub audio_source_labels: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "muninn.move_marker_candidate",
    schema = "muninn.move_marker_candidate.v1"
)]
pub struct MuninnMoveMarkerCandidateRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub camera_id: String,
    #[cultcache(key = 3)]
    pub frame_sequence: u64,
    #[cultcache(key = 4)]
    pub source_id_hash: u64,
    #[cultcache(key = 5)]
    pub tile_x: u32,
    #[cultcache(key = 6)]
    pub tile_y: u32,
    #[cultcache(key = 7)]
    pub center_x_px: f32,
    #[cultcache(key = 8)]
    pub center_y_px: f32,
    #[cultcache(key = 9)]
    pub radius_px: f32,
    #[cultcache(key = 10)]
    pub area_px: u32,
    #[cultcache(key = 11)]
    pub mean_luma: f32,
    #[cultcache(key = 12)]
    pub peak_luma: u32,
    #[cultcache(key = 13)]
    pub score: f32,
    #[cultcache(key = 14)]
    pub observed_at: String,
    #[cultcache(key = 15, default)]
    pub move_id: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "muninn.move_controller_state",
    schema = "muninn.move_controller_state.v1"
)]
pub struct MuninnMoveControllerStateRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub move_id: String,
    #[cultcache(key = 3)]
    pub sequence: u64,
    #[cultcache(key = 4)]
    pub source_timestamp_ns: i64,
    #[cultcache(key = 5)]
    pub accelerometer_xyz: Vec<f32>,
    #[cultcache(key = 6)]
    pub gyroscope_xyz: Vec<f32>,
    #[cultcache(key = 7)]
    pub magnetometer_xyz: Vec<f32>,
    #[cultcache(key = 8)]
    pub trigger_value: f32,
    #[cultcache(key = 9)]
    pub buttons: Vec<String>,
    #[cultcache(key = 10)]
    pub battery01: f32,
    #[cultcache(key = 11)]
    pub observed_at: String,
    #[cultcache(key = 12, default)]
    pub source_path: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "muninn.hid_controller_state",
    schema = "muninn.hid_controller_state.v1"
)]
pub struct MuninnHidControllerStateRecord {
    #[cultcache(key = 0)]
    pub stream_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub device_id: String,
    #[cultcache(key = 3)]
    pub device_kind: String,
    #[cultcache(key = 4)]
    pub sequence: u64,
    #[cultcache(key = 5)]
    pub source_timestamp_ns: i64,
    #[cultcache(key = 6)]
    pub axes: Vec<f32>,
    #[cultcache(key = 7)]
    pub buttons: Vec<String>,
    #[cultcache(key = 8)]
    pub battery01: f32,
    #[cultcache(key = 9)]
    pub observed_at: String,
    #[cultcache(key = 10)]
    pub source_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "muninn.move_identity", schema = "muninn.move_identity.v1")]
pub struct MuninnMoveIdentityRecord {
    #[cultcache(key = 0)]
    pub identity_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub move_id: String,
    #[cultcache(key = 3)]
    pub source_path: String,
    #[cultcache(key = 4)]
    pub bluetooth_host_address: String,
    #[cultcache(key = 5)]
    pub state: String,
    #[cultcache(key = 6)]
    pub detail: String,
    #[cultcache(key = 7)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.move_light_command",
    schema = "muninn.move_light_command.v1"
)]
pub struct MuninnMoveLightCommandRecord {
    #[cultcache(key = 0)]
    pub command_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub move_id: String,
    #[cultcache(key = 3)]
    pub hidraw_path: String,
    #[cultcache(key = 4)]
    pub colors: Vec<String>,
    #[cultcache(key = 5)]
    pub durations_ms: Vec<u32>,
    #[cultcache(key = 6)]
    pub repeat_count: u32,
    #[cultcache(key = 7)]
    pub authority: String,
    #[cultcache(key = 8)]
    pub state: String,
    #[cultcache(key = 9)]
    pub detail: String,
    #[cultcache(key = 10)]
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "muninn.move_hue_program",
    schema = "muninn.move_hue_program.v1"
)]
pub struct MuninnMoveHueProgramRecord {
    #[cultcache(key = 0)]
    pub program_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub mode: String,
    #[cultcache(key = 3)]
    pub cycle_ms: u64,
    #[cultcache(key = 4)]
    pub epoch_ns: i64,
    #[cultcache(key = 5)]
    pub hold_at_ns: i64,
    #[cultcache(key = 6)]
    pub requested_by: String,
    #[cultcache(key = 7)]
    pub updated_at: String,
    #[cultcache(key = 8, default)]
    pub order_mode: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(type = "muninn.move_tracker_health", schema = "muninn.move_tracker_health.v1")]
pub struct MuninnMoveTrackerHealthRecord {
    #[cultcache(key = 0)] pub health_id: String,
    #[cultcache(key = 1)] pub host_id: String,
    #[cultcache(key = 2)] pub camera_id: String,
    #[cultcache(key = 3)] pub camera_index: i32,
    #[cultcache(key = 4)] pub state: String,
    #[cultcache(key = 5)] pub camera_name: String,
    #[cultcache(key = 6)] pub camera_api: String,
    #[cultcache(key = 7)] pub width: u32,
    #[cultcache(key = 8)] pub height: u32,
    #[cultcache(key = 9)] pub exposure: f32,
    #[cultcache(key = 10)] pub calibrated_controller_count: u32,
    #[cultcache(key = 11)] pub update_count: u64,
    #[cultcache(key = 12)] pub observation_count: u64,
    #[cultcache(key = 13)] pub latest_observation_count: u32,
    #[cultcache(key = 14)] pub last_observation_at: String,
    #[cultcache(key = 15)] pub detail: String,
    #[cultcache(key = 16)] pub updated_at: String,
    #[cultcache(key = 17, default)] pub image_mean_rgb: Vec<u32>,
    #[cultcache(key = 18, default)] pub image_peak_rgb: Vec<u32>,
    #[cultcache(key = 19, default)] pub color_evidence_move_ids: Vec<String>,
    #[cultcache(key = 20, default)] pub color_evidence_pixel_counts: Vec<u32>,
    #[cultcache(key = 21, default)] pub rejected_stale_count: u64,
    #[cultcache(key = 22, default)] pub rejected_radius_count: u64,
    #[cultcache(key = 23, default)] pub rejected_bounds_count: u64,
    #[cultcache(key = 24, default)] pub rejected_continuity_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "muninn.move_evidence_transport_health", schema = "muninn.move_evidence_transport_health.v1")]
pub struct MuninnMoveEvidenceTransportHealthRecord {
    #[cultcache(key = 0)] pub health_id: String,
    #[cultcache(key = 1)] pub host_id: String,
    #[cultcache(key = 2)] pub stream_id: String,
    #[cultcache(key = 3)] pub produced_frames: u64,
    #[cultcache(key = 4)] pub local_ring_admissions: u64,
    #[cultcache(key = 5)] pub remote_handoffs: u64,
    #[cultcache(key = 6)] pub remote_sends: u64,
    #[cultcache(key = 7)] pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "muninn.quest_access", schema = "muninn.quest_access.v1")]
pub struct MuninnQuestAccessRecord {
    #[cultcache(key = 0)]
    pub access_id: String,
    #[cultcache(key = 1)]
    pub host_id: String,
    #[cultcache(key = 2)]
    pub serial: String,
    #[cultcache(key = 3)]
    pub connection_state: String,
    #[cultcache(key = 4)]
    pub product: String,
    #[cultcache(key = 5)]
    pub model: String,
    #[cultcache(key = 6)]
    pub device: String,
    #[cultcache(key = 7)]
    pub transport_id: String,
    #[cultcache(key = 8)]
    pub input_stream_id: String,
    #[cultcache(key = 9)]
    pub pose_stream_id: String,
    #[cultcache(key = 10)]
    pub video_input_stream_id: String,
    #[cultcache(key = 11)]
    pub video_input_transport: String,
    #[cultcache(key = 12)]
    pub state: String,
    #[cultcache(key = 13)]
    pub detail: String,
    #[cultcache(key = 14)]
    pub observed_at: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "muninn.command_boundary",
    schema = "muninn.command_boundary.v1"
)]
pub struct MuninnCommandBoundaryCompatRecord {
    #[cultcache(key = 0)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "muninn.transport_profile",
    schema = "muninn.transport_profile.v1"
)]
pub struct MuninnTransportProfileCompatRecord {
    #[cultcache(key = 0)]
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(type = "sleipnir.input_mapping", schema = "sleipnir.input_mapping.v1")]
pub struct SleipnirInputMappingRecord {
    #[cultcache(key = 0)]
    pub provider_id: String,
    #[cultcache(key = 1)]
    pub enabled: bool,
    #[cultcache(key = 2)]
    pub device_filter: String,
    #[cultcache(key = 3)]
    pub stream_id: String,
    #[cultcache(key = 4)]
    pub presentation: String,
    #[cultcache(key = 5)]
    pub axis_map: Value,
    #[cultcache(key = 6)]
    pub button_map: Value,
    #[cultcache(key = 7)]
    pub pending_learn: Value,
    #[cultcache(key = 8)]
    pub updated_at: String,
    #[cultcache(key = 9)]
    pub source: String,
}

cultmesh_rs::cultmesh_documents!(OdinDocuments {
    OdinSnapshotRecord => ODIN_SNAPSHOT_SCHEMA,
    OdinVerseRecord => ODIN_VERSE_SCHEMA,
    OdinServiceRecord => ODIN_SERVICE_SCHEMA,
    OdinInterfaceRecord => ODIN_INTERFACE_SCHEMA,
    OdinObservationStreamRecord => ODIN_OBSERVATION_STREAM_SCHEMA,
    OdinTranslationRouteRecord => ODIN_TRANSLATION_ROUTE_SCHEMA,
    EveSurfaceStateRecord => EVE_SURFACE_STATE_SCHEMA,
    EveInterfaceBindingCompatRecord => EVE_INTERFACE_BINDING_SCHEMA,
    EveProviderAdvertisementRecord => EVE_PROVIDER_ADVERTISEMENT_SCHEMA,
    VoidBotSwarmStateSnapshotCompatRecord => VOIDBOT_SWARM_STATE_SNAPSHOT_SCHEMA,
    IdunnDesiredDaemonRecord => IDUNN_DESIRED_DAEMON_SCHEMA,
    IdunnDaemonHealthRecord => IDUNN_DAEMON_HEALTH_SCHEMA,
    IdunnKeepaliveDecisionRecord => IDUNN_KEEPALIVE_DECISION_SCHEMA,
    IdunnRestartRequestRecord => IDUNN_RESTART_REQUEST_SCHEMA,
    IdunnRestartResultRecord => IDUNN_RESTART_RESULT_SCHEMA,
    IdunnDeploymentRequestRecord => IDUNN_DEPLOYMENT_REQUEST_SCHEMA,
    IdunnDeploymentResultRecord => IDUNN_DEPLOYMENT_RESULT_SCHEMA,
    IdunnLifecycleCommandRecord => IDUNN_LIFECYCLE_COMMAND_SCHEMA,
    IdunnReleaseTargetRecord => IDUNN_RELEASE_TARGET_SCHEMA,
    IdunnDeploymentArtifactRecord => IDUNN_DEPLOYMENT_ARTIFACT_SCHEMA,
    IdunnStateMigrationPlanRecord => IDUNN_STATE_MIGRATION_PLAN_SCHEMA,
    IdunnStateMigrationResultRecord => IDUNN_STATE_MIGRATION_RESULT_SCHEMA,
    IdunnRolloutPlanRecord => IDUNN_ROLLOUT_PLAN_SCHEMA,
    IdunnRolloutResultRecord => IDUNN_ROLLOUT_RESULT_SCHEMA,
    IdunnOperatorAlarmRecord => IDUNN_OPERATOR_ALARM_SCHEMA,
    IdunnSwarmSurgeryPlanRecord => IDUNN_SWARM_SURGERY_PLAN_SCHEMA,
    IdunnDaemonSurgeryPlanRecord => IDUNN_DAEMON_SURGERY_PLAN_SCHEMA,
    IdunnDaemonTransportProfileRecord => IDUNN_DAEMON_TRANSPORT_PROFILE_SCHEMA,
    IdunnCommandBoundaryRecord => IDUNN_COMMAND_BOUNDARY_SCHEMA,
    IdunnRuntimeTransportCheckRecord => IDUNN_RUNTIME_TRANSPORT_CHECK_SCHEMA,
    IdunnRudpHealthIngressRecord => IDUNN_RUDP_HEALTH_INGRESS_SCHEMA,
    MuninnTelemetrySurfaceRecord => MUNINN_TELEMETRY_SURFACE_SCHEMA,
    MuninnCaptureStreamRecord => MUNINN_CAPTURE_STREAM_SCHEMA,
    MuninnCaptureStreamCommandRecord => MUNINN_CAPTURE_STREAM_COMMAND_SCHEMA,
    MuninnMediaVideoAccessUnitRecord => MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA,
    MuninnMediaVideoParityShardRecord => MUNINN_MEDIA_VIDEO_PARITY_SHARD_SCHEMA,
    MuninnMediaAudioPacketRecord => MUNINN_MEDIA_AUDIO_PACKET_SCHEMA,
    MuninnMediaReceiverFeedbackRecord => MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA,
    MuninnObsStreamCatalogRecord => MUNINN_OBS_STREAM_CATALOG_SCHEMA,
    MuninnMoveMarkerCandidateRecord => MUNINN_MOVE_MARKER_CANDIDATE_SCHEMA,
    MuninnMoveControllerStateRecord => MUNINN_MOVE_CONTROLLER_STATE_SCHEMA,
    MuninnHidControllerStateRecord => MUNINN_HID_CONTROLLER_STATE_SCHEMA,
    MuninnMoveIdentityRecord => MUNINN_MOVE_IDENTITY_SCHEMA,
    MuninnMoveLightCommandRecord => MUNINN_MOVE_LIGHT_COMMAND_SCHEMA,
    MuninnMoveHueProgramRecord => MUNINN_MOVE_HUE_PROGRAM_SCHEMA,
    MuninnMoveTrackerHealthRecord => MUNINN_MOVE_TRACKER_HEALTH_SCHEMA,
    MuninnMoveEvidenceTransportHealthRecord => MUNINN_MOVE_EVIDENCE_TRANSPORT_HEALTH_SCHEMA,
    MuninnQuestAccessRecord => MUNINN_QUEST_ACCESS_SCHEMA,
    MuninnCommandBoundaryCompatRecord => MUNINN_COMMAND_BOUNDARY_SCHEMA,
    MuninnTransportProfileCompatRecord => MUNINN_TRANSPORT_PROFILE_SCHEMA,
    SleipnirInputMappingRecord => SLEIPNIR_INPUT_MAPPING_SCHEMA,
});

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OdinRecords {
    pub snapshot: Option<OdinSnapshotRecord>,
    pub verses: Vec<OdinVerseRecord>,
    pub services: Vec<OdinServiceRecord>,
    pub interfaces: Vec<OdinInterfaceRecord>,
    pub observation_streams: Vec<OdinObservationStreamRecord>,
    pub translation_routes: Vec<OdinTranslationRouteRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use cultmesh_rs::{CultMesh, CultMeshNodeOptions};

    #[test]
    fn muninn_media_documents_round_trip_through_cultmesh() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("muninn-media.cc");
        let mut node = CultMesh::create_node(
            &store_path,
            OdinDocuments,
            CultMeshNodeOptions {
                runtime_id: "muninn-media-test".to_string(),
                pull_on_start: true,
            },
        )?;

        let video = MuninnMediaVideoAccessUnitRecord {
            stream_id: "muninn.raven.av.rudp".to_string(),
            session_id: "session-1".to_string(),
            frame_id: 42,
            codec: "h264".to_string(),
            pts_ticks: 126_000,
            duration_ticks: 3_000,
            timebase_num: 1,
            timebase_den: 90_000,
            keyframe: true,
            dependency_frame_id: None,
            deadline_ticks: 127_800,
            chunk_index: 0,
            chunk_count: 1,
            payload: vec![0, 0, 0, 1, 0x65],
        };
        let audio = MuninnMediaAudioPacketRecord {
            stream_id: video.stream_id.clone(),
            session_id: video.session_id.clone(),
            packet_id: 7,
            codec: "opus".to_string(),
            pts_ticks: 67_200,
            duration_ticks: 960,
            timebase_num: 1,
            timebase_den: 48_000,
            deadline_ticks: 68_160,
            payload: vec![0xf8, 0xff, 0xfe],
        };
        let feedback = MuninnMediaReceiverFeedbackRecord {
            stream_id: video.stream_id.clone(),
            session_id: video.session_id.clone(),
            receiver_id: "starfire.obs".to_string(),
            highest_decodable_frame_id: Some(41),
            missing_frame_ids: vec![42],
            late_frame_ids: vec![40],
            requested_keyframe: true,
            jitter_us: 750,
            decode_queue_us: 2_000,
            observed_at: "2026-06-18T00:00:00Z".to_string(),
            missing_video_chunk_keys: vec!["42:2".to_string()],
        };

        node.put("video:42:0", &video)?;
        node.put("audio:7", &audio)?;
        node.put("feedback:starfire.obs", &feedback)?;

        assert_eq!(
            node.documents()
                .binding("muninn.media_video_access_unit")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(MUNINN_MEDIA_VIDEO_ACCESS_UNIT_SCHEMA)
        );
        assert_eq!(
            node.documents()
                .binding("muninn.media_audio_packet")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(MUNINN_MEDIA_AUDIO_PACKET_SCHEMA)
        );
        assert_eq!(
            node.documents()
                .binding("muninn.media_receiver_feedback")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(MUNINN_MEDIA_RECEIVER_FEEDBACK_SCHEMA)
        );

        let reloaded = CultMesh::create_node(
            &store_path,
            OdinDocuments,
            CultMeshNodeOptions {
                runtime_id: "muninn-media-test-reloaded".to_string(),
                pull_on_start: true,
            },
        )?;
        assert_eq!(
            reloaded
                .get_required::<MuninnMediaVideoAccessUnitRecord>("video:42:0")?
                .payload,
            vec![0, 0, 0, 1, 0x65]
        );
        assert_eq!(
            reloaded
                .get_required::<MuninnMediaAudioPacketRecord>("audio:7")?
                .codec,
            "opus"
        );
        assert!(
            reloaded
                .get_required::<MuninnMediaReceiverFeedbackRecord>("feedback:starfire.obs")?
                .requested_keyframe
        );
        assert_eq!(
            reloaded
                .get_required::<MuninnMediaReceiverFeedbackRecord>("feedback:starfire.obs")?
                .missing_video_chunk_keys,
            vec!["42:2".to_string()]
        );
        Ok(())
    }
}

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
pub const IDUNN_OPERATOR_ALARM_SCHEMA: &str = "idunn.operator_alarm.v1";
pub const MUNINN_TELEMETRY_SURFACE_SCHEMA: &str = "muninn.telemetry_surface.v1";
pub const MUNINN_CAPTURE_STREAM_SCHEMA: &str = "muninn.capture_stream.v1";
pub const MUNINN_OBS_STREAM_CATALOG_SCHEMA: &str = "muninn.obs_stream_catalog.v1";
pub const MUNINN_MOVE_MARKER_CANDIDATE_SCHEMA: &str = "muninn.move_marker_candidate.v1";
pub const MUNINN_MOVE_CONTROLLER_STATE_SCHEMA: &str = "muninn.move_controller_state.v1";
pub const MUNINN_MOVE_LIGHT_COMMAND_SCHEMA: &str = "muninn.move_light_command.v1";

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
pub struct EveProviderAdvertisementCompatRecord {
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

cultmesh_rs::cultmesh_documents!(OdinDocuments {
    OdinSnapshotRecord => ODIN_SNAPSHOT_SCHEMA,
    OdinVerseRecord => ODIN_VERSE_SCHEMA,
    OdinServiceRecord => ODIN_SERVICE_SCHEMA,
    OdinInterfaceRecord => ODIN_INTERFACE_SCHEMA,
    OdinObservationStreamRecord => ODIN_OBSERVATION_STREAM_SCHEMA,
    OdinTranslationRouteRecord => ODIN_TRANSLATION_ROUTE_SCHEMA,
    EveSurfaceStateRecord => EVE_SURFACE_STATE_SCHEMA,
    EveInterfaceBindingCompatRecord => EVE_INTERFACE_BINDING_SCHEMA,
    EveProviderAdvertisementCompatRecord => EVE_PROVIDER_ADVERTISEMENT_SCHEMA,
    VoidBotSwarmStateSnapshotCompatRecord => VOIDBOT_SWARM_STATE_SNAPSHOT_SCHEMA,
    IdunnDesiredDaemonRecord => IDUNN_DESIRED_DAEMON_SCHEMA,
    IdunnDaemonHealthRecord => IDUNN_DAEMON_HEALTH_SCHEMA,
    IdunnKeepaliveDecisionRecord => IDUNN_KEEPALIVE_DECISION_SCHEMA,
    IdunnRestartRequestRecord => IDUNN_RESTART_REQUEST_SCHEMA,
    IdunnRestartResultRecord => IDUNN_RESTART_RESULT_SCHEMA,
    IdunnDeploymentRequestRecord => IDUNN_DEPLOYMENT_REQUEST_SCHEMA,
    IdunnDeploymentResultRecord => IDUNN_DEPLOYMENT_RESULT_SCHEMA,
    IdunnOperatorAlarmRecord => IDUNN_OPERATOR_ALARM_SCHEMA,
    MuninnTelemetrySurfaceRecord => MUNINN_TELEMETRY_SURFACE_SCHEMA,
    MuninnCaptureStreamRecord => MUNINN_CAPTURE_STREAM_SCHEMA,
    MuninnObsStreamCatalogRecord => MUNINN_OBS_STREAM_CATALOG_SCHEMA,
    MuninnMoveMarkerCandidateRecord => MUNINN_MOVE_MARKER_CANDIDATE_SCHEMA,
    MuninnMoveControllerStateRecord => MUNINN_MOVE_CONTROLLER_STATE_SCHEMA,
    MuninnMoveLightCommandRecord => MUNINN_MOVE_LIGHT_COMMAND_SCHEMA,
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

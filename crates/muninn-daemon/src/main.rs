mod media_packetizer;

use crate::media_packetizer::{
    AudioPcmStreamSendConfig, AudioPcmStreamSendState, MuninnMediaSendPayload,
    MuninnMediaWireRecord, VideoAnnexBStreamSendConfig, VideoAnnexBStreamSendState,
    decode_media_wire_record,
};
use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{
    CULTMESH_RUDP_DOCUMENT_CATALOG_CONNECTION_ID, CultMesh, CultMeshNodeOptions,
    CultMeshRudpDocumentPublishOptions, CultMeshRudpSnapshotOptions, CultMeshSharedMemoryFrameRing,
    CultMeshStreamBodyTransport, CultMeshStreamCatalog, CultMeshStreamClock,
    CultMeshStreamDescriptor, CultMeshStreamKind,
};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding,
    CultNetRudpSocketTransportConnection, CultNetRudpSocketTransportOptions, CultNetTransportFrame,
    CultNetWireContract, decode_cultnet_message_from_slice, encode_cultnet_message_to_vec,
};
use odin_core::{
    EVE_PROVIDER_ADVERTISEMENT_SCHEMA, EveProviderAdvertisementRecord, EveSurfaceStateRecord,
    IdunnDaemonHealthRecord,
    MUNINN_MOVE_HUE_PROGRAM_SCHEMA,
    MUNINN_OBS_STREAM_CATALOG_SCHEMA,
    MuninnCaptureStreamCommandRecord, MuninnCaptureStreamRecord, MuninnCommandBoundaryCompatRecord,
    MuninnHidControllerStateRecord, MuninnMediaReceiverFeedbackRecord,
    MuninnMoveControllerStateRecord, MuninnMoveEvidenceTransportHealthRecord, MuninnMoveHueProgramRecord, MuninnMoveIdentityRecord,
    MuninnMoveLightCommandRecord,
    MuninnMoveMarkerCandidateRecord, MuninnObsStreamCatalogRecord, MuninnQuestAccessRecord,
    MuninnTelemetrySurfaceRecord, MuninnTransportProfileCompatRecord, OdinDocuments,
    OdinEndpointQuery, discover_provider_endpoints,
};
#[cfg(feature = "psmoveapi-tracker")]
use odin_core::MuninnMoveTrackerHealthRecord;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
#[cfg(feature = "psmoveapi-tracker")]
use std::io::BufReader;
#[cfg(any(not(windows), feature = "psmoveapi-tracker"))]
use std::io::Write;
use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::XboxController::{XINPUT_GAMEPAD, XINPUT_STATE, XInputGetState};

const CULTNET_RUDP_PROTOCOL_ID: &str = "cultnet.transport.rudp.v0";
const MUNINN_MEDIA_RUDP_SCHEMA: &str = "muninn.media.rudp.v1";
const IDUNN_HEALTH_RUDP_CONNECTION_ID: u32 = 0x1d0d_0001;
const MUNINN_MEDIA_RUDP_CONNECTION_ID: u32 = 0x6d75_0001;
const MUNINN_AUDIO_RUDP_CONNECTION_ID: u32 = 0x6d75_0004;
const MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID: u32 = 0x6d75_0005;
const MUNINN_COMMAND_RUDP_CONNECTION_ID: u32 = 0x61e0_0001;
const MUNINN_RUDP_MEDIA_PROFILE_ID: &str = "muninn.rudp.low_latency_h264_lan.v1";
const MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS: u32 = 12_000;
const MUNINN_RUDP_MEDIA_VBV_FRAME_BUDGETS: u32 = 1;
const MUNINN_RUDP_MEDIA_LOW_DELAY_KEY_FRAME_SCALE: u32 = 4;
const MUNINN_RUDP_MEDIA_VIDEO_DPB_SIZE: u32 = 1;
const MUNINN_RUDP_MEDIA_PACKET_BYTES: usize = 848;
const MUNINN_RUDP_IPV4_UDP_PAYLOAD_BYTES: usize = 1_472;
const MUNINN_RUDP_FIXED_HEADER_BYTES: usize = 36;
const MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES: usize = MUNINN_RUDP_IPV4_UDP_PAYLOAD_BYTES
    - MUNINN_RUDP_FIXED_HEADER_BYTES
    - crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL.len();
const MUNINN_RUDP_MEDIA_RESEND_DELAY_MS: u64 = 8;
const MUNINN_RUDP_MEDIA_MAX_PENDING_RELIABLE_PACKETS: u32 = 512;
const MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS: u64 = 100;
const MUNINN_RUDP_MEDIA_PAYLOAD_CHANNEL_CAPACITY: usize = 1;
const MUNINN_RUDP_MEDIA_PENDING_AUDIO_CAPACITY: usize = 256;
const MUNINN_RUDP_MEDIA_PENDING_VIDEO_CAPACITY: usize = 512;
const MUNINN_RUDP_MEDIA_INGEST_BUDGET_PER_TURN: usize = 1;
const MUNINN_RUDP_MEDIA_RECEIVER_GAP_WAIT_MS: u64 = 16;
const MUNINN_RUDP_MEDIA_REPAIR_CACHE_CHUNKS: usize = 16_384;
const MUNINN_RUDP_MEDIA_REPAIR_BURST_CHUNKS: usize = 2_048;
const MUNINN_RUDP_MEDIA_REPAIR_INITIAL_CHUNKS_PER_SECOND: usize = 4_096;
const MUNINN_RUDP_MEDIA_REPAIR_MIN_CHUNKS_PER_SECOND: usize = 8;
const MUNINN_RUDP_MEDIA_REPAIR_MAX_CHUNKS_PER_SECOND: usize = 16_384;
const MUNINN_RUDP_MEDIA_REPAIR_ADD_CHUNKS_PER_SECOND: usize = 2_048;
const MUNINN_RUDP_MEDIA_REPAIR_RECOVERY_INTERVAL_MS: u64 = 2_000;
const MUNINN_RUDP_MEDIA_REPAIR_MAX_FEEDBACK_PER_POLL: usize = 32;
const MUNINN_RUDP_MEDIA_REPAIR_MAX_CHUNKS_PER_POLL: usize = 256;
const MUNINN_RUDP_MEDIA_SOCKET_BUFFER_BYTES: usize = 16 * 1024 * 1024;
const MUNINN_RUDP_MEDIA_SEND_PACE_EVERY_PAYLOADS: usize = 1;
const MUNINN_RUDP_MEDIA_SEND_PACE_SLEEP_US: u64 = 0;
const MUNINN_RUDP_ACTIVE_CATALOG_REPUBLISH_MS: u64 = 2_000;
const MUNINN_ODIN_PROVIDER_LEASE_REFRESH_SECONDS: u64 = 30;
const PS_MOVE_LED_REPORT_LEN: usize = 49;
const MUNINN_DISABLED_VIDEO_SOURCE_ID: &str = "video:none";
const MUNINN_DISABLED_AUDIO_SOURCE_ID: &str = "audio:none";
const MUNINN_DEFAULT_ACTIVATION_STORE_PATH: &str = "C:/Meta/Odin/state/muninn.activate.cc";
const WINDOWS_XINPUT_SOURCE_PREFIX: &str = "xinput://";
const XINPUT_GAMEPAD_DPAD_UP_MASK: u16 = 0x0001;
const XINPUT_GAMEPAD_DPAD_DOWN_MASK: u16 = 0x0002;
const XINPUT_GAMEPAD_DPAD_LEFT_MASK: u16 = 0x0004;
const XINPUT_GAMEPAD_DPAD_RIGHT_MASK: u16 = 0x0008;
const XINPUT_GAMEPAD_START_MASK: u16 = 0x0010;
const XINPUT_GAMEPAD_BACK_MASK: u16 = 0x0020;
const XINPUT_GAMEPAD_LEFT_THUMB_MASK: u16 = 0x0040;
const XINPUT_GAMEPAD_RIGHT_THUMB_MASK: u16 = 0x0080;
const XINPUT_GAMEPAD_LEFT_SHOULDER_MASK: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER_MASK: u16 = 0x0200;
const XINPUT_GAMEPAD_A_MASK: u16 = 0x1000;
const XINPUT_GAMEPAD_B_MASK: u16 = 0x2000;
const XINPUT_GAMEPAD_X_MASK: u16 = 0x4000;
const XINPUT_GAMEPAD_Y_MASK: u16 = 0x8000;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Serve,
    Activate,
    Health,
    DryRun,
    RequestStream,
    RequestMoveLight,
    SetMoveHueProgram,
    MoveHueProgramStatus,
    CaptureStreamStatus,
    ObsCatalogStatus,
    MoveLightStatus,
    MoveIdentityStatus,
    MoveSourceStatus,
    MoveStateStatus,
    ClaimMoveHost,
    QuestAccessStatus,
    MoveTrackerWorker,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Options {
    mode: Mode,
    store_path: PathBuf,
    activation_store_path: Option<PathBuf>,
    surface_id: String,
    stream_id: String,
    stream_filter_explicit: bool,
    stream_action: String,
    host_id: String,
    target_host: String,
    port: u16,
    obs_target_host: Option<String>,
    obs_port: u16,
    media_transport: MediaTransport,
    media_packet_bytes: usize,
    rudp_video_bitrate_kbps: u32,
    rudp_latency_budget_ms: u32,
    width: u32,
    height: u32,
    framerate: u32,
    ddagrab_output_index: u32,
    capture_video: bool,
    capture_audio: bool,
    audio_device: String,
    audio_source_id_override: Option<String>,
    video_sources: Vec<CatalogSource>,
    audio_sources: Vec<CatalogSource>,
    audio_sample_rate: u32,
    audio_channels: u32,
    ffmpeg_path: String,
    video_encoder_path: Option<PathBuf>,
    loopback_script: PathBuf,
    log_root: PathBuf,
    interval_seconds: Option<u64>,
    move_id: String,
    move_filter: Option<String>,
    hidraw_path: String,
    move_colors: Vec<String>,
    move_durations_ms: Vec<u32>,
    move_repeat_count: u32,
    move_hue_cycle_ms: u64,
    move_hue_cycle_ms_explicit: bool,
    move_hue_mode: Option<String>,
    move_hue_order_mode: Option<String>,
    command_id: Option<String>,
    move_host_address: Option<String>,
    move_state_sources: Vec<MoveStateSource>,
    move_marker_camera_sources: Vec<MoveMarkerCameraSource>,
    move_marker_width: u32,
    move_marker_height: u32,
    move_marker_stride_bytes: Option<u32>,
    move_marker_fps: u32,
    move_marker_threshold_min: u8,
    move_marker_min_area_px: u32,
    move_marker_max_candidates: u32,
    move_psmoveapi_tracker: bool,
    move_light_passive: bool,
    move_tracker_exposure_milli: u32,
    move_tracker_camera_exposure_milli: HashMap<String, u32>,
    move_evidence_stream_id: Option<String>,
    move_evidence_verse_id: String,
    move_evidence_ring_slots: usize,
    move_evidence_slot_bytes: usize,
    move_evidence_snapshot_path: Option<PathBuf>,
    quest_adb: bool,
    quest_serial: Option<String>,
    quest_input_stream_id: Option<String>,
    quest_pose_stream_id: Option<String>,
    quest_video_input_stream_id: Option<String>,
    idunn_rudp_health: Option<IdunnRudpHealthOptions>,
    odin_cultmesh_uri: Option<String>,
    hid_controller_rudp_target: Option<SocketAddr>,
    hid_controller_rudp_bind: Option<SocketAddr>,
    hid_controller_rudp_advertise: Option<String>,
    command_rudp_bind: Option<SocketAddr>,
    command_rudp_advertise: Option<String>,
    hid_controller_receipt_retention_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CatalogSource {
    id: String,
    label: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MoveMarkerCameraSource {
    camera_id: String,
    device_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioSourceKind {
    Loopback,
    Input,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AudioSourceSpec {
    kind: AudioSourceKind,
    device: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MediaTransport {
    Rudp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MuninnRudpMediaProfile {
    profile_id: &'static str,
    video_codec: &'static str,
    video_encoder: &'static str,
    video_preset: &'static str,
    video_tune: &'static str,
    video_bitrate_kbps: u32,
    media_packet_bytes: usize,
    max_fragment_bytes: usize,
    video_b_frames: u8,
    video_rc_lookahead: u8,
    sender_queue_deadline_ms: u64,
    sender_resend_delay_ms: u64,
    sender_delivery_deadline_ms: u64,
    sender_pace_every_payloads: usize,
    sender_pace_sleep_us: u64,
    receiver_assembly_deadline_ms: u64,
    receiver_gap_wait_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MoveStateSource {
    move_id: String,
    hidraw_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct IdunnRudpHealthOptions {
    endpoint: SocketAddr,
    daemon_id: String,
    health_contract: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MuxPlan {
    command_line: String,
    command_script: String,
    command_file: PathBuf,
    targets: Vec<String>,
}

struct ActiveMoveLightCommand {
    command: MuninnMoveLightCommandRecord,
    colors: Vec<(u8, u8, u8)>,
    step_index: usize,
    repeats_done: u32,
    next_write_at: Instant,
}

struct ActiveCaptureStreamCommand {
    command_id: String,
    stream_id: String,
    command: MuninnCaptureStreamCommandRecord,
    child: Child,
}

struct ActiveMoveStateSource {
    source: MoveStateSource,
    sequence: u64,
    joystick_axes: [i16; 16],
    joystick_buttons: [bool; 32],
    light_hidraw_path: Option<String>,
    latest_move_record: Option<MuninnMoveControllerStateRecord>,
}

struct ActiveMoveMarkerCameraSource {
    source: MoveMarkerCameraSource,
    frame_source: MoveMarkerFrameSource,
    sequence: u64,
    #[cfg(feature = "psmoveapi-tracker")]
    psmoveapi_observations: Option<Arc<Mutex<Option<Vec<muninn_psmoveapi_tracker::PsmoveApiObservation>>>>>,
    #[cfg(feature = "psmoveapi-tracker")]
    psmoveapi_health: Option<Arc<Mutex<Option<MuninnMoveTrackerHealthRecord>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DefaultMoveLightTarget {
    path: String,
    identity: String,
}

#[derive(Clone, Debug, PartialEq)]
struct MoveMarkerFrameSource {
    stream_id: String,
    host_id: String,
    camera_id: String,
    fps: u32,
    tracker_config: muninn_move_tracker::MoveTrackerConfig,
}

#[derive(Serialize)]
struct MuninnMoveEvidenceStreamFrame<'a>(
    &'a str,
    &'a str,
    i64,
    &'a [MuninnMoveMarkerCandidateRecord],
    &'a [MuninnMoveControllerStateRecord],
);

#[derive(Serialize)]
struct MimirMoveProofEvidenceFrameSnapshot<'a>(
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    i64,
    u64,
    #[serde(with = "serde_bytes")] &'a [u8],
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JoystickEvent {
    event_type: u8,
    number: u8,
    value: i16,
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;
    match options.mode {
        Mode::Serve => serve(options),
        Mode::Activate => activate(options),
        Mode::Health => health_check(&options),
        Mode::RequestStream => request_capture_stream(options),
        Mode::RequestMoveLight => request_move_light(options),
        Mode::SetMoveHueProgram => set_move_hue_program(options),
        Mode::MoveHueProgramStatus => move_hue_program_status(options),
        Mode::CaptureStreamStatus => capture_stream_status(options),
        Mode::ObsCatalogStatus => obs_catalog_status(options),
        Mode::MoveLightStatus => move_light_status(options),
        Mode::MoveIdentityStatus => move_identity_status(options),
        Mode::MoveSourceStatus => move_source_status(options),
        Mode::MoveStateStatus => move_state_status(options),
        Mode::ClaimMoveHost => claim_move_host(options),
        Mode::QuestAccessStatus => quest_access_status(options),
        Mode::MoveTrackerWorker => run_move_tracker_worker(options),
        Mode::DryRun => {
            require_media_target_uri(&options)?;
            let plan = build_mux_plan(&options, "dry-run".to_string());
            println!("{}", plan.command_line);
            Ok(())
        }
    }
}

fn serve(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let move_hue_program = load_or_initialize_move_hue_program(&options)?;
    let mut move_evidence_stream = create_move_evidence_stream(&options)?;
    let mut active_move_lights = Vec::new();
    let suppressed_default_move_light_paths = Arc::new(Mutex::new(HashSet::new()));
    let mut hid_controller_stream = create_hid_controller_stream(&options)?;
    let mut last_move_host_claim_attempt_at = None;
    let mut last_move_bluetooth_pickup_attempt_at = None;
    let move_runtime_enabled = serve_should_manage_move_runtime(&options);
    let mut active_move_states =
        active_move_state_sources(serve_move_state_sources(&options, move_runtime_enabled));
    start_daemon_health_worker(&options);
    let mut active_move_marker_cameras =
        active_move_marker_camera_sources(&options, Arc::clone(&move_hue_program));
    let mut active_capture_streams = Vec::new();
    let move_evidence_rudp_sender = start_hid_controller_rudp_ingress(
        &options,
        move_evidence_stream.as_ref().map(|stream| Arc::clone(&stream.counters)),
    )?;
    if let Some(stream) = move_evidence_stream.as_mut() {
        stream.rudp_sender = move_evidence_rudp_sender;
    }
    let move_evidence_transport_health = move_evidence_stream.as_ref().map(|stream| {
        MoveEvidenceTransportHealth {
            stream_id: stream.stream_id.clone(),
            counters: Arc::clone(&stream.counters),
        }
    });
    let latest_move_controller_states = Arc::new(Mutex::new(Vec::new()));
    if let Some(stream) = move_evidence_stream.take() {
        start_move_evidence_aggregator(
            stream,
            move_evidence_camera_inputs(&active_move_marker_cameras),
            Arc::clone(&latest_move_controller_states),
        );
    }
    if !options.move_light_passive {
        start_default_move_light_worker(
            &options,
            Arc::clone(&suppressed_default_move_light_paths),
            Arc::clone(&move_hue_program),
        );
    }
    start_move_hue_program_sync_worker(&options, Arc::clone(&move_hue_program));
    start_provider_command_ingress(&options, Arc::clone(&move_hue_program))?;
    start_odin_provider_lease_worker(&options);

    loop {
        let live_move_sources = serve_move_state_sources(&options, move_runtime_enabled);
        sync_active_move_state_sources(&mut active_move_states, live_move_sources.clone());
        let active_stream_ids =
            tick_capture_stream_commands(&options, &mut active_capture_streams)?;
        {
            let mut node = open_node(&options, "muninn-daemon")?;
            reconcile_move_identity_records(
                &mut node,
                &options,
                &live_move_sources,
                &active_move_states,
            )?;
            register_move_light_commands(&mut node, &options, &mut active_move_lights)?;
            update_suppressed_default_move_light_paths(
                &suppressed_default_move_light_paths,
                &active_move_lights,
            );
            tick_move_light_commands(&mut node, &mut active_move_lights, &mut HidMoveLightWriter)?;
            update_suppressed_default_move_light_paths(
                &suppressed_default_move_light_paths,
                &active_move_lights,
            );
            publish_move_controller_states(
                &mut node,
                &options,
                &mut active_move_states,
                &mut HidMoveControllerStateReader,
                None,
                hid_controller_stream.as_mut(),
            )?;
            let controller_states: Vec<MuninnMoveControllerStateRecord> =
                active_move_states
                    .iter()
                    .filter_map(|state| state.latest_move_record.clone())
                    .collect();
            if let Ok(mut latest) = latest_move_controller_states.lock() {
                *latest = controller_states;
            }
            publish_move_tracker_health(&mut node, &mut active_move_marker_cameras)?;
            publish_move_evidence_transport_health(
                &mut node,
                &options,
                move_evidence_transport_health.as_ref(),
            )?;
            publish_quest_access_if_requested(&mut node, &options)?;
            let state = if active_stream_ids.is_empty() {
                "idle"
            } else {
                "streaming"
            };
            publish_surface(&mut node, &options, state, &active_stream_ids)?;
            publish_runtime_boundary_records(
                &mut node,
                &options,
                state,
                &active_stream_ids,
                &live_move_sources,
            )?;
            publish_obs_catalog_idle(&mut node, &options)?;
        }
        claim_move_host_if_due(&options, &mut last_move_host_claim_attempt_at);
        pickup_bluetooth_moves_if_due(
            &active_move_states,
            &mut last_move_bluetooth_pickup_attempt_at,
        );
        let has_platform_default_move_lights = serve_should_manage_platform_move_lights(&options);
        if options.interval_seconds.is_none()
            && active_move_lights.is_empty()
            && active_move_states.is_empty()
            && active_move_marker_cameras.is_empty()
            && active_capture_streams.is_empty()
            && !has_platform_default_move_lights
        {
            return Ok(());
        }
        let sleep = if !active_move_lights.is_empty()
            || !active_move_states.is_empty()
            || !active_move_marker_cameras.is_empty()
            || !active_capture_streams.is_empty()
            || has_platform_default_move_lights
        {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(options.interval_seconds.unwrap_or(15))
        };
        thread::sleep(sleep);
    }
}

fn start_daemon_health_worker(options: &Options) {
    if options.idunn_rudp_health.is_none() {
        return;
    }
    let options = options.clone();
    thread::spawn(move || {
        loop {
            if let Err(error) = run_daemon_health_publisher(&options) {
                eprintln!("Muninn Idunn health publish failed: {error:#}");
            }
            thread::sleep(Duration::from_secs(5));
        }
    });
}

fn move_hue_program_key(host_id: &str) -> String {
    format!("muninn:{host_id}:move-hue-program")
}

fn move_hue_surface_key(host_id: &str) -> String {
    format!("surface:muninn.telemetry.{host_id}.move-hue")
}

fn load_or_initialize_move_hue_program(
    options: &Options,
) -> Result<Arc<Mutex<MuninnMoveHueProgramRecord>>> {
    let key = move_hue_program_key(&options.host_id);
    let mut node = open_node(options, "muninn-move-hue-program-bootstrap")?;
    let program = node.get::<MuninnMoveHueProgramRecord>(&key)?.unwrap_or_else(|| {
        bootstrap_move_hue_program(options)
    });
    validate_move_hue_program(&program)?;
    node.put(&key, &program)?;
    Ok(Arc::new(Mutex::new(program)))
}

fn bootstrap_move_hue_program(options: &Options) -> MuninnMoveHueProgramRecord {
    MuninnMoveHueProgramRecord {
        program_id: move_hue_program_key(&options.host_id),
        host_id: options.host_id.clone(),
        mode: "animated".to_string(),
        cycle_ms: options.move_hue_cycle_ms,
        epoch_ns: 0,
        hold_at_ns: 0,
        requested_by: "muninn-bootstrap".to_string(),
        updated_at: timestamp().unwrap_or_else(|_| "unix-0".to_string()),
        order_mode: "descending".to_string(),
        transition_percent: 100,
        transition_percent_explicit: true,
    }
}

fn validate_move_hue_program(program: &MuninnMoveHueProgramRecord) -> Result<()> {
    if !matches!(program.mode.as_str(), "animated" | "hold" | "static") {
        return Err(anyhow!("unsupported Move hue mode {}", program.mode));
    }
    if program.cycle_ms == 0 {
        return Err(anyhow!("Move hue cycle_ms must be greater than zero"));
    }
    if !matches!(
        program.order_mode.as_str(),
        "descending" | "ascending" | "bounce" | "rotating-lead" | "golden-permutation"
    ) {
        return Err(anyhow!("unsupported Move hue order mode {}", program.order_mode));
    }
    if program.transition_percent > 100 { return Err(anyhow!("Move hue transition_percent must be at most 100")); }
    Ok(())
}

fn move_hue_program_timestamp_ns(program: &MuninnMoveHueProgramRecord, now_ns: i128) -> i128 {
    match program.mode.as_str() {
        "hold" => i128::from(program.hold_at_ns.max(program.epoch_ns)),
        "static" => i128::from(program.epoch_ns),
        _ => now_ns,
    }
}

fn effective_transition_percent(program: &MuninnMoveHueProgramRecord) -> u8 {
    if program.transition_percent_explicit { program.transition_percent } else { 100 }
}

fn start_move_hue_program_sync_worker(
    options: &Options,
    runtime_program: Arc<Mutex<MuninnMoveHueProgramRecord>>,
) {
    let options = options.clone();
    let mut activation_options = options.clone();
    activation_options.store_path = options
        .activation_store_path
        .as_ref()
        .cloned()
        .unwrap_or_else(|| options.store_path.clone());
    thread::spawn(move || loop {
        let key = move_hue_program_key(&options.host_id);
        let main_program = open_node(&options, "muninn-move-hue-program-sync")
            .ok()
            .and_then(|node| node.get::<MuninnMoveHueProgramRecord>(&key).ok().flatten());
        let activation_program = open_node(
            &activation_options,
            "muninn-move-hue-program-activation-sync",
        )
        .ok()
        .and_then(|node| node.get::<MuninnMoveHueProgramRecord>(&key).ok().flatten());
        let program = [main_program, activation_program]
            .into_iter()
            .flatten()
            .filter(|program| validate_move_hue_program(program).is_ok())
            .max_by_key(|program| unix_timestamp_sort_key(&program.updated_at));
        if let Some(program) = program
            && let Ok(mut current) = runtime_program.lock()
            && *current != program
        {
            *current = program.clone();
            if let Ok(mut node) = open_node(&options, "muninn-move-hue-program-projection") {
                let _ = node.put(&key, &program);
            }
        }
        thread::sleep(Duration::from_millis(250));
    });
}

fn start_provider_command_ingress(
    options: &Options,
    runtime_program: Arc<Mutex<MuninnMoveHueProgramRecord>>,
) -> Result<()> {
    let Some(bind) = options.command_rudp_bind else {
        return Ok(());
    };
    let socket = UdpSocket::bind(bind)
        .with_context(|| format!("binding Muninn provider command route at {bind}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(5)))?;
    let options = options.clone();
    thread::spawn(move || {
        if let Err(error) = run_provider_command_ingress(socket, options, runtime_program) {
            eprintln!("Muninn provider command ingress stopped: {error:#}");
        }
    });
    Ok(())
}

fn run_provider_command_ingress(
    socket: UdpSocket,
    options: Options,
    runtime_program: Arc<Mutex<MuninnMoveHueProgramRecord>>,
) -> Result<()> {
    let mut transport = CultNetRudpSocketTransportConnection::new(
        CultNetRudpSocketTransportOptions {
            runtime_id: format!("muninn-{}-command-ingress", options.host_id),
            socket,
            mode: cultnet_rs::CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id: MUNINN_COMMAND_RUDP_CONNECTION_ID,
            initial_sequence: 1,
            resend_delay_ms: 15,
            transport_id: Some("muninn-provider-command-rudp".to_string()),
            max_payload_bytes: None,
            max_fragment_bytes: Some(1200),
            max_pending_reliable_packets: Some(64),
            reconnect_policy: None,
        },
    )?;
    loop {
        if let Some(frame) = transport.receive_once()?
            && frame.channel_id == "schema"
        {
            let message = decode_cultnet_message_from_slice(
                &frame.payload,
                CultNetWireContract::CultNetSchemaV0,
            )?;
            if let CultNetMessage::DocumentPutRaw { document, .. } = message {
                apply_provider_command_document(&options, &runtime_program, document)?;
            }
        }
        transport.poll_resends()?;
    }
}

fn apply_provider_command_document(
    options: &Options,
    runtime_program: &Arc<Mutex<MuninnMoveHueProgramRecord>>,
    document: CultNetRawDocumentRecord,
) -> Result<()> {
    if document.schema_id != "gamecult.eve.command.v1" {
        return Ok(());
    }
    let value: serde_json::Value = match document.payload_encoding {
        CultNetRawPayloadEncoding::Messagepack => rmp_serde::from_slice(&document.payload)?,
    };
    let command_type = value
        .get("command")
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if command_type != "muninn.set-move-hue-program" {
        return Ok(());
    }
    let payload = value
        .get("payload")
        .filter(|payload| payload.is_object())
        .unwrap_or(&value);
    let provider_id = value
        .get("providerId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if provider_id != muninn_provider_id(options) {
        return Err(anyhow!("Move hue command targets provider {provider_id}, not {}", muninn_provider_id(options)));
    }
    let mut program = runtime_program
        .lock()
        .map_err(|_| anyhow!("Move hue runtime program lock is poisoned"))?
        .clone();
    if let Some(mode) = payload.get("mode").and_then(serde_json::Value::as_str) {
        program.mode = mode.to_string();
        if mode == "hold" {
            program.hold_at_ns = timestamp_ns()?;
        }
    }
    if let Some(order) = payload.get("orderMode").and_then(serde_json::Value::as_str) {
        program.order_mode = order.to_string();
    }
    if let Some(cycle_ms) = payload.get("cycleMs").and_then(serde_json::Value::as_u64) {
        program.cycle_ms = cycle_ms;
    }
    if let Some(value) = payload.get("transitionPercent").and_then(serde_json::Value::as_u64) {
        program.transition_percent = u8::try_from(value).map_err(|_| anyhow!("Move hue transitionPercent must be at most 100"))?;
        program.transition_percent_explicit = true;
    }
    program.requested_by = value
        .get("publishedBy")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("eve-command")
        .to_string();
    program.updated_at = timestamp()?;
    validate_move_hue_program(&program)?;
    let key = move_hue_program_key(&options.host_id);
    open_node(options, "muninn-provider-command")?.put(&key, &program)?;
    *runtime_program
        .lock()
        .map_err(|_| anyhow!("Move hue runtime program lock is poisoned"))? = program;
    Ok(())
}

fn tick_capture_stream_commands(
    options: &Options,
    active: &mut Vec<ActiveCaptureStreamCommand>,
) -> Result<Vec<String>> {
    reap_capture_stream_children(options, active)?;
    let mut activation_options = options.clone();
    activation_options.store_path = options
        .activation_store_path
        .as_ref()
        .cloned()
        .unwrap_or_else(|| options.store_path.clone());
    let mut node = open_node(&activation_options, "muninn-activation-controller")?;
    let mut commands = node.cache().get_all::<MuninnCaptureStreamCommandRecord>()?;
    commands.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
    let latest_command_by_stream = latest_capture_stream_command_ids(&commands, &options.host_id);
    for command in commands {
        if command.host_id != options.host_id {
            continue;
        }
        let command_stream_key = canonical_muninn_stream_id(&command.stream_id);
        if let Some(latest_command_id) = latest_command_by_stream.get(&command_stream_key) {
            if latest_command_id != &command.command_id {
                supersede_capture_stream_command(&mut node, command, latest_command_id)?;
                continue;
            }
        }
        match (command.action.as_str(), command.state.as_str()) {
            ("start", "pending") => {
                start_capture_stream_command(options, &mut node, active, command)?;
            }
            ("stop", "pending") => {
                stop_capture_stream_command(&mut node, active, command)?;
            }
            ("start", "running") => {
                if !active
                    .iter()
                    .any(|session| session.command_id == command.command_id)
                {
                    start_capture_stream_command(options, &mut node, active, command)?;
                }
            }
            _ => {}
        }
    }
    Ok(active
        .iter()
        .map(|session| session.stream_id.clone())
        .collect())
}

fn pull_odin_obs_catalog_snapshot(node: &mut cultmesh_rs::CultMeshNode, options: &Options) {
    let Some(target) = resolve_odin_cultmesh_uri(options) else {
        return;
    };
    if let Err(error) = node.pull_rudp_catalog_snapshot(CultMeshRudpSnapshotOptions {
        target,
        runtime_id: format!("muninn-{}-obs-catalog-client", options.host_id),
        schema_ids: Some(vec![MUNINN_OBS_STREAM_CATALOG_SCHEMA.to_string()]),
        connect_timeout: Duration::from_millis(2000),
        response_timeout: Duration::from_millis(5000),
        resend_delay_ms: 15,
        connection_id: CULTMESH_RUDP_DOCUMENT_CATALOG_CONNECTION_ID,
        ..CultMeshRudpSnapshotOptions::default()
    }) {
        eprintln!("Muninn could not pull OBS catalog from Odin: {error:#}");
    }
}

fn obs_catalog_status(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let mut node = open_node(&options, "muninn-obs-catalog-client")?;
    pull_odin_obs_catalog_snapshot(&mut node, &options);
    let catalogs = node.cache().get_all::<MuninnObsStreamCatalogRecord>()?;
    if catalogs.is_empty() {
        return Err(anyhow!(
            "Odin did not return any {} records for OBS discovery",
            MUNINN_OBS_STREAM_CATALOG_SCHEMA
        ));
    }
    Ok(())
}

fn resolve_odin_cultmesh_uri(options: &Options) -> Option<SocketAddr> {
    let uri = options.odin_cultmesh_uri.as_deref()?.trim();
    if uri.is_empty() {
        return None;
    }
    match CultMesh::resolve_rudp_endpoint(uri) {
        Ok(target) => Some(target),
        Err(error) => {
            eprintln!("Muninn could not resolve Odin CultMesh URI {uri}: {error:#}");
            None
        }
    }
}

fn latest_capture_stream_command_ids(
    commands: &[MuninnCaptureStreamCommandRecord],
    host_id: &str,
) -> HashMap<String, String> {
    let mut latest_command_by_stream = HashMap::new();
    for command in commands.iter().filter(|command| command.host_id == host_id) {
        latest_command_by_stream.insert(
            canonical_muninn_stream_id(&command.stream_id),
            command.command_id.clone(),
        );
    }
    latest_command_by_stream
}

fn latest_capture_stream_command_key(host_id: &str, stream_id: &str) -> String {
    format!(
        "{}:{}:capture-stream:latest",
        host_id,
        canonical_muninn_stream_id(stream_id)
    )
}

fn supersede_capture_stream_command(
    node: &mut cultmesh_rs::CultMeshNode,
    command: MuninnCaptureStreamCommandRecord,
    latest_command_id: &str,
) -> Result<()> {
    if capture_stream_command_is_terminal(&command.state) {
        return Ok(());
    }
    let command_id = command.command_id.clone();
    node.put(
        &command_id,
        &MuninnCaptureStreamCommandRecord {
            state: "completed".to_string(),
            detail: format!("Superseded by newer command {latest_command_id}."),
            updated_at: timestamp()?,
            ..command
        },
    )?;
    Ok(())
}

fn capture_stream_command_is_terminal(state: &str) -> bool {
    matches!(state, "completed" | "failed")
}

fn reap_capture_stream_children(
    options: &Options,
    active: &mut Vec<ActiveCaptureStreamCommand>,
) -> Result<()> {
    let Some(activation_store_path) = options.activation_store_path.as_ref() else {
        return Ok(());
    };
    let mut index = 0;
    while index < active.len() {
        match active[index].child.try_wait()? {
            Some(status) => {
                let mut activation_options = options.clone();
                activation_options.store_path = activation_store_path.clone();
                let mut node = open_node(&activation_options, "muninn-activation-controller")?;
                if let Some(command) =
                    node.get::<MuninnCaptureStreamCommandRecord>(&active[index].command_id)?
                {
                    let command_id = command.command_id.clone();
                    let state = if status.success() {
                        "completed"
                    } else {
                        "failed"
                    };
                    node.put(
                        &command_id,
                        &MuninnCaptureStreamCommandRecord {
                            state: state.to_string(),
                            detail: format!("activation child exited with {status}"),
                            updated_at: timestamp()?,
                            ..command
                        },
                    )?;
                }
                active.remove(index);
            }
            None => index += 1,
        }
    }
    Ok(())
}

fn start_capture_stream_command(
    options: &Options,
    node: &mut cultmesh_rs::CultMeshNode,
    active: &mut Vec<ActiveCaptureStreamCommand>,
    command: MuninnCaptureStreamCommandRecord,
) -> Result<()> {
    let command_stream_key = canonical_muninn_stream_id(&command.stream_id);
    if let Some(session) = active
        .iter_mut()
        .find(|session| session.stream_id == command_stream_key)
    {
        if capture_stream_commands_start_equivalent(&session.command, &command) {
            eprintln!(
                "Muninn serve kept existing capture stream {} bitrate_kbps={} latency_budget_ms={}.",
                command.stream_id,
                command_rudp_video_bitrate_kbps(&command),
                command_rudp_latency_budget_ms(&command)
            );
            let running = MuninnCaptureStreamCommandRecord {
                state: "running".to_string(),
                detail: format!(
                    "Muninn serve kept existing activation child from command {}.",
                    session.command_id
                ),
                updated_at: timestamp()?,
                ..command.clone()
            };
            node.put(&running.command_id, &running)?;
            session.command_id = running.command_id.clone();
            session.command = running;
            return Ok(());
        }
    }

    for session in active.iter_mut() {
        if session.stream_id == command_stream_key {
            terminate_child_tree(&mut session.child);
            let _ = session.child.wait();
        }
    }
    active.retain(|session| session.stream_id != command_stream_key);

    eprintln!(
        "Muninn serve spawning capture stream {} bitrate_kbps={} latency_budget_ms={}.",
        command.stream_id,
        command_rudp_video_bitrate_kbps(&command),
        command_rudp_latency_budget_ms(&command)
    );
    let child = spawn_capture_stream_activation(options, &command)?;
    let running = MuninnCaptureStreamCommandRecord {
        state: "running".to_string(),
        detail: "Muninn serve spawned the local activation child.".to_string(),
        updated_at: timestamp()?,
        ..command.clone()
    };
    node.put(&running.command_id, &running)?;
    active.push(ActiveCaptureStreamCommand {
        command_id: command.command_id.clone(),
        stream_id: command_stream_key,
        command,
        child,
    });
    Ok(())
}

fn capture_stream_commands_start_equivalent(
    active: &MuninnCaptureStreamCommandRecord,
    command: &MuninnCaptureStreamCommandRecord,
) -> bool {
    active.host_id == command.host_id
        && canonical_muninn_stream_id(&active.stream_id)
            == canonical_muninn_stream_id(&command.stream_id)
        && active.action == "start"
        && command.action == "start"
        && active.target_host == command.target_host
        && active.port == command.port
        && active.obs_target_host == command.obs_target_host
        && active.obs_port == command.obs_port
        && active.media_transport == command.media_transport
        && active.media_packet_bytes == command.media_packet_bytes
        && command_rudp_video_bitrate_kbps(active) == command_rudp_video_bitrate_kbps(command)
        && command_rudp_latency_budget_ms(active) == command_rudp_latency_budget_ms(command)
        && command_video_source_id(active) == command_video_source_id(command)
        && command_audio_source_id(active) == command_audio_source_id(command)
}

fn command_rudp_video_bitrate_kbps(command: &MuninnCaptureStreamCommandRecord) -> u32 {
    if command.rudp_video_bitrate_kbps == 0 {
        MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS
    } else {
        command.rudp_video_bitrate_kbps
    }
}

fn command_rudp_latency_budget_ms(command: &MuninnCaptureStreamCommandRecord) -> u32 {
    if command.rudp_latency_budget_ms == 0 {
        MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS as u32
    } else {
        command.rudp_latency_budget_ms
    }
}

fn canonical_muninn_stream_id(stream_id: &str) -> String {
    for suffix in [".rudp"] {
        if let Some(prefix) = stream_id.strip_suffix(suffix) {
            return prefix.to_string();
        }
    }
    if let Some((prefix, suffix)) = stream_id.rsplit_once(':') {
        if !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            return prefix.to_string();
        }
    }
    stream_id.to_string()
}

fn command_video_source_id(command: &MuninnCaptureStreamCommandRecord) -> &str {
    if command.video_source_id.is_empty() {
        "display:0"
    } else {
        command.video_source_id.as_str()
    }
}

fn command_audio_source_id(command: &MuninnCaptureStreamCommandRecord) -> &str {
    if command.audio_source_id.is_empty() {
        "wasapi-loopback:Realtek"
    } else {
        command.audio_source_id.as_str()
    }
}

fn command_requests_video(command: &MuninnCaptureStreamCommandRecord) -> bool {
    command_video_source_id(command) != MUNINN_DISABLED_VIDEO_SOURCE_ID
}

fn command_requests_audio(command: &MuninnCaptureStreamCommandRecord) -> bool {
    command_audio_source_id(command) != MUNINN_DISABLED_AUDIO_SOURCE_ID
}

fn stop_capture_stream_command(
    node: &mut cultmesh_rs::CultMeshNode,
    active: &mut Vec<ActiveCaptureStreamCommand>,
    command: MuninnCaptureStreamCommandRecord,
) -> Result<()> {
    let mut stopped = false;
    let command_stream_key = canonical_muninn_stream_id(&command.stream_id);
    let mut index = 0;
    while index < active.len() {
        if active[index].stream_id == command_stream_key {
            terminate_child_tree(&mut active[index].child);
            let _ = active[index].child.wait();
            active.remove(index);
            stopped = true;
        } else {
            index += 1;
        }
    }
    let command_id = command.command_id.clone();
    node.put(
        &command_id,
        &MuninnCaptureStreamCommandRecord {
            state: "completed".to_string(),
            detail: if stopped {
                "Muninn serve stopped the active capture stream.".to_string()
            } else {
                "No active capture stream matched the stop request.".to_string()
            },
            updated_at: timestamp()?,
            ..command
        },
    )?;
    Ok(())
}

fn terminate_child_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status();
    }
    #[cfg(not(windows))]
    {
        let _ = child.kill();
    }
}

fn spawn_capture_stream_activation(
    options: &Options,
    command: &MuninnCaptureStreamCommandRecord,
) -> Result<Child> {
    let exe = env::current_exe().context("resolving current Muninn executable")?;
    let activation_stderr = options.log_root.join(format!(
        "muninn-activation-{}.err.log",
        safe_log_component(&command.command_id)
    ));
    let mut args = vec![
        "activate".to_string(),
        "--store".to_string(),
        options.store_path.display().to_string(),
        "--host".to_string(),
        options.host_id.clone(),
        "--stream".to_string(),
        command.stream_id.clone(),
        "--target-host".to_string(),
        command.target_host.clone(),
        "--port".to_string(),
        command.port.to_string(),
        "--media-transport".to_string(),
        command.media_transport.clone(),
        "--media-packet-bytes".to_string(),
        command.media_packet_bytes.to_string(),
        "--rudp-video-bitrate-kbps".to_string(),
        command_rudp_video_bitrate_kbps(command).to_string(),
        "--rudp-latency-budget-ms".to_string(),
        command_rudp_latency_budget_ms(command).to_string(),
    ];
    if command_requests_video(command) {
        let output_index = video_source_output_index(command_video_source_id(command))
            .unwrap_or(options.ddagrab_output_index);
        args.extend([
            "--ddagrab-output-index".to_string(),
            output_index.to_string(),
        ]);
    } else {
        args.push("--no-video".to_string());
    }
    if command_requests_audio(command) {
        args.extend([
            "--audio-source-id".to_string(),
            command_audio_source_id(command).to_string(),
        ]);
    } else {
        args.push("--no-audio".to_string());
    }
    if let Some(obs_target_host) = command.obs_target_host.as_deref()
        && !obs_target_host.trim().is_empty()
        && command.obs_port != 0
    {
        args.extend([
            "--obs-target-host".to_string(),
            obs_target_host.to_string(),
            "--obs-port".to_string(),
            command.obs_port.to_string(),
        ]);
    }
    args.extend([
        "--audio-sample-rate".to_string(),
        options.audio_sample_rate.to_string(),
        "--audio-channels".to_string(),
        options.audio_channels.to_string(),
        "--ffmpeg".to_string(),
        options.ffmpeg_path.clone(),
        "--loopback-script".to_string(),
        options.loopback_script.display().to_string(),
        "--log-root".to_string(),
        options.log_root.display().to_string(),
    ]);
    if let Some(path) = &options.video_encoder_path {
        args.push("--video-encoder".to_string());
        args.push(path.display().to_string());
    }
    Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(fs::File::create(&activation_stderr).with_context(|| {
            format!(
                "creating activation stderr log {}",
                activation_stderr.display()
            )
        })?)
        .spawn()
        .context("spawning daemon-owned Muninn capture activation child")
}

fn video_source_id_for_options(options: &Options) -> String {
    if options.capture_video {
        format!("display:{}", options.ddagrab_output_index)
    } else {
        MUNINN_DISABLED_VIDEO_SOURCE_ID.to_string()
    }
}

fn video_source_label_for_options(options: &Options) -> String {
    format!(
        "{} display {}",
        options.host_id,
        options.ddagrab_output_index + 1
    )
}

fn audio_source_id_for_options(options: &Options) -> String {
    if options.capture_audio {
        if let Some(source_id) = options.audio_source_id_override.as_ref() {
            if parse_audio_source_spec(source_id).is_some() {
                return source_id.clone();
            }
        }
        format!("wasapi-loopback:{}", options.audio_device)
    } else {
        MUNINN_DISABLED_AUDIO_SOURCE_ID.to_string()
    }
}

fn audio_source_label_for_options(options: &Options) -> String {
    let spec = audio_source_spec_for_options(options);
    match spec.kind {
        AudioSourceKind::Loopback => {
            format!("{} loopback ({})", options.host_id, spec.device)
        }
        AudioSourceKind::Input => format!("{} input ({})", options.host_id, spec.device),
    }
}

fn video_source_catalog(options: &Options) -> Vec<CatalogSource> {
    if options.video_sources.is_empty() {
        vec![CatalogSource {
            id: video_source_id_for_options(options),
            label: video_source_label_for_options(options),
        }]
    } else {
        options.video_sources.clone()
    }
}

fn audio_source_catalog(options: &Options) -> Vec<CatalogSource> {
    if options.audio_sources.is_empty() {
        vec![CatalogSource {
            id: audio_source_id_for_options(options),
            label: audio_source_label_for_options(options),
        }]
    } else {
        options.audio_sources.clone()
    }
}

fn video_source_output_index(source_id: &str) -> Option<u32> {
    source_id
        .strip_prefix("display:")
        .and_then(|value| value.parse::<u32>().ok())
}

fn parse_audio_source_spec(source_id: &str) -> Option<AudioSourceSpec> {
    let (kind, device) = if let Some(value) = source_id.strip_prefix("wasapi-loopback:") {
        (AudioSourceKind::Loopback, value)
    } else if let Some(value) = source_id.strip_prefix("wasapi-input:") {
        (AudioSourceKind::Input, value)
    } else {
        return None;
    };
    let device = device.trim();
    if device.is_empty() {
        return None;
    }
    Some(AudioSourceSpec {
        kind,
        device: device.to_string(),
    })
}

fn audio_source_spec_for_options(options: &Options) -> AudioSourceSpec {
    if let Some(source_id) = options.audio_source_id_override.as_ref() {
        if let Some(spec) = parse_audio_source_spec(source_id) {
            return spec;
        }
    }
    AudioSourceSpec {
        kind: AudioSourceKind::Loopback,
        device: options.audio_device.clone(),
    }
}

fn safe_log_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn active_move_state_sources(sources: Vec<MoveStateSource>) -> Vec<ActiveMoveStateSource> {
    sources.into_iter().map(active_move_state_source).collect()
}

fn active_move_state_source(source: MoveStateSource) -> ActiveMoveStateSource {
    ActiveMoveStateSource {
        light_hidraw_path: default_move_light_path(&source.hidraw_path),
        source,
        sequence: 0,
        joystick_axes: [0; 16],
        joystick_buttons: [false; 32],
        latest_move_record: None,
    }
}

fn sync_active_move_state_sources(
    active: &mut Vec<ActiveMoveStateSource>,
    desired: Vec<MoveStateSource>,
) {
    active.retain(|state| desired.iter().any(|source| source == &state.source));
    for source in desired {
        if active.iter().any(|state| state.source == source) {
            continue;
        }
        active.push(active_move_state_source(source));
    }
}

fn publish_move_identity_records(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    sources: &[MoveStateSource],
) -> Result<()> {
    let observed_at = timestamp()?;
    let bluetooth_host_address = options.move_host_address.clone().unwrap_or_default();
    for source in sources {
        let record = MuninnMoveIdentityRecord {
            identity_id: format!("{}:{}:move-identity", options.host_id, source.move_id),
            host_id: options.host_id.clone(),
            move_id: source.move_id.clone(),
            source_path: source.hidraw_path.clone(),
            bluetooth_host_address: bluetooth_host_address.clone(),
            state: "usb-visible".to_string(),
            detail: "Muninn discovered this PS Move on a local USB/HID input path; controller-state records may be absent when the platform HID input collection is silent.".to_string(),
            observed_at: observed_at.clone(),
        };
        node.put(&record.identity_id, &record)?;
    }
    Ok(())
}

fn reconcile_move_identity_records(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    sources: &[MoveStateSource],
    active: &[ActiveMoveStateSource],
) -> Result<()> {
    publish_move_identity_records(node, options, sources)?;
    publish_bluetooth_move_identity_records(node, options, active)?;

    let current_ids = current_move_identity_records_from_sources(options, sources)?
        .into_iter()
        .map(|identity| identity.identity_id)
        .collect::<Vec<_>>();
    let existing = node.cache().get_all::<MuninnMoveIdentityRecord>()?;
    for identity in existing {
        if identity.host_id == options.host_id
            && !current_ids
                .iter()
                .any(|current| current == &identity.identity_id)
        {
            node.delete::<MuninnMoveIdentityRecord>(&identity.identity_id)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn publish_bluetooth_move_identity_records(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    active: &[ActiveMoveStateSource],
) -> Result<()> {
    let observed_at = timestamp()?;
    let bluetooth_host_address = platform_default_bluetooth_host_address().unwrap_or_default();
    let devices = bluetoothctl_motion_controller_devices()?;
    for device in devices {
        let Some(move_id) = bluetooth_address_move_id(&device.address) else {
            continue;
        };
        if active.iter().any(|state| {
            state.source.move_id.eq_ignore_ascii_case(&move_id)
                && !is_joystick_path(&state.source.hidraw_path)
        }) {
            continue;
        }
        let Some(record) = build_bluetooth_move_identity_record(
            options,
            &device,
            bluetooth_host_address.clone(),
            observed_at.clone(),
        ) else {
            continue;
        };
        node.put(&record.identity_id, &record)?;
    }
    Ok(())
}

fn build_bluetooth_move_identity_record(
    options: &Options,
    device: &BluetoothMoveDevice,
    bluetooth_host_address: String,
    observed_at: String,
) -> Option<MuninnMoveIdentityRecord> {
    let move_id = bluetooth_address_move_id(&device.address)?;
    let state = if device.connected {
        "bluetooth-connected"
    } else if device.trusted {
        "bluetooth-waiting"
    } else {
        "bluetooth-known"
    };
    let detail = if device.connected {
        "Muninn sees this PS Move connected through BlueZ."
    } else if device.trusted {
        "Muninn sees this trusted PS Move in BlueZ and will attempt bounded pickup while disconnected."
    } else {
        "Muninn sees this PS Move in BlueZ, but it is not trusted for automatic pickup."
    };
    Some(MuninnMoveIdentityRecord {
        identity_id: format!("{}:{}:move-identity", options.host_id, move_id),
        host_id: options.host_id.clone(),
        move_id,
        source_path: format!("bluetooth:{}", device.address),
        bluetooth_host_address,
        state: state.to_string(),
        detail: detail.to_string(),
        observed_at,
    })
}

#[cfg(not(unix))]
fn publish_bluetooth_move_identity_records(
    _node: &mut cultmesh_rs::CultMeshNode,
    _options: &Options,
    _active: &[ActiveMoveStateSource],
) -> Result<()> {
    Ok(())
}

fn live_move_state_sources(options: &Options) -> Vec<MoveStateSource> {
    let discovered = platform_move_state_sources();
    if discovered.is_empty() {
        return options.move_state_sources.clone();
    }

    merge_move_state_sources(discovered, &options.move_state_sources)
}

fn merge_move_state_sources(
    discovered: Vec<MoveStateSource>,
    configured: &[MoveStateSource],
) -> Vec<MoveStateSource> {
    let mut sources = discovered;
    for source in configured {
        if is_joystick_path(&source.hidraw_path) {
            if let Some(discovered) = sources
                .iter_mut()
                .find(|discovered| discovered.hidraw_path == source.hidraw_path)
            {
                if discovered.move_id.starts_with("hid-") {
                    discovered.move_id = source.move_id.clone();
                }
                continue;
            }
            #[cfg(unix)]
            if !Path::new(&source.hidraw_path).exists() {
                continue;
            }
        }
        if !sources.iter().any(|discovered| {
            discovered == source || discovered.move_id == source.move_id
        }) {
            sources.push(source.clone());
        }
    }
    sources
}

fn claim_move_host_if_due(options: &Options, last_attempt_at: &mut Option<Instant>) {
    if !serve_should_claim_move_host(options) {
        return;
    }

    let cadence = Duration::from_secs(5);
    if last_attempt_at
        .as_ref()
        .is_some_and(|instant| instant.elapsed() < cadence)
    {
        return;
    }
    *last_attempt_at = Some(Instant::now());

    if let Err(error) = prepare_move_bluetooth_host_for_claim() {
        eprintln!("Muninn could not prepare Bluetooth host for USB PS Move claim: {error:#}");
    }

    let Some(host) = move_host_address_for_claim(options) else {
        return;
    };
    if let Err(error) = claim_ps_move_host(&host) {
        eprintln!("Muninn could not claim USB PS Move host {host}: {error:#}");
    }
}

fn serve_should_claim_move_host(options: &Options) -> bool {
    options
        .move_host_address
        .as_deref()
        .is_some_and(|host| !host.trim().is_empty())
}

fn serve_should_manage_move_runtime(options: &Options) -> bool {
    serve_should_claim_move_host(options)
        || !options.move_state_sources.is_empty()
        || options.move_evidence_stream_id.is_some()
}

fn serve_should_manage_platform_move_lights(options: &Options) -> bool {
    serve_should_manage_move_runtime(options)
}

fn serve_move_state_sources(options: &Options, move_runtime_enabled: bool) -> Vec<MoveStateSource> {
    if move_runtime_enabled {
        return live_move_state_sources(options);
    }
    Vec::new()
}

fn pickup_bluetooth_moves_if_due(
    active: &[ActiveMoveStateSource],
    last_attempt_at: &mut Option<Instant>,
) {
    let cadence = Duration::from_secs(5);
    if last_attempt_at
        .as_ref()
        .is_some_and(|instant| instant.elapsed() < cadence)
    {
        return;
    }
    *last_attempt_at = Some(Instant::now());

    if let Err(error) = pickup_bluetooth_moves(active) {
        eprintln!("Muninn could not attempt PS Move Bluetooth pickup: {error:#}");
    }
}

#[cfg(unix)]
fn prepare_move_bluetooth_host_for_claim() -> Result<()> {
    for args in [["pairable", "on"], ["discoverable", "on"]] {
        let output = Command::new("bluetoothctl")
            .args(args)
            .output()
            .with_context(|| format!("bluetoothctl {} {} could not run", args[0], args[1]))?;
        if output.status.success() {
            continue;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "bluetoothctl {} {} failed: {}",
            args[0],
            args[1],
            stderr.trim()
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_move_bluetooth_host_for_claim() -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn pickup_bluetooth_moves(_active: &[ActiveMoveStateSource]) -> Result<()> {
    let devices = bluetoothctl_motion_controller_devices()?;
    for device in devices {
        if !bluetooth_move_needs_pickup(&device) {
            continue;
        }
        match bluetoothctl_connect_device(&device.address) {
            Ok(true) => println!(
                "bluetooth_move_pickup address={} state=connected",
                device.address
            ),
            Ok(false) => {}
            Err(error) => eprintln!(
                "bluetooth_move_pickup address={} state=failed error={error:#}",
                device.address
            ),
        }
    }
    Ok(())
}

fn bluetooth_move_needs_pickup(device: &BluetoothMoveDevice) -> bool {
    device.trusted && !device.connected
}

#[cfg(not(unix))]
fn pickup_bluetooth_moves(_active: &[ActiveMoveStateSource]) -> Result<()> {
    Ok(())
}

fn bluetooth_address_move_id(address: &str) -> Option<String> {
    parse_bluetooth_address_little_endian(address)
        .ok()
        .map(|bytes| {
            format!(
                "move-{}",
                bytes
                    .iter()
                    .rev()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )
        })
}

fn move_host_address_for_claim(options: &Options) -> Option<String> {
    if let Some(host) = options.move_host_address.as_deref() {
        if !host.trim().is_empty() {
            return Some(host.to_string());
        }
    }
    platform_default_bluetooth_host_address()
}

struct ActiveMoveEvidenceStream {
    catalog: CultMeshStreamCatalog,
    stream_id: String,
    producer_peer_id: String,
    frame_counter: u64,
    snapshot_path: Option<PathBuf>,
    rudp_sender: Option<Arc<Mutex<Option<Vec<u8>>>>>,
    counters: Arc<MoveEvidenceTransportCounters>,
    local_ring_enabled: bool,
}

#[derive(Clone)]
struct MoveEvidenceTransportHealth {
    stream_id: String,
    counters: Arc<MoveEvidenceTransportCounters>,
}

#[derive(Default)]
struct MoveEvidenceTransportCounters {
    produced_frames: AtomicU64,
    local_ring_admissions: AtomicU64,
    remote_handoffs: AtomicU64,
    remote_sends: AtomicU64,
}

fn activate(options: Options) -> Result<()> {
    require_media_target_uri(&options)?;
    activate_rudp(options)
}

fn activate_rudp(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let mut node = open_node(&options, "muninn-activation")?;
    let plan = build_mux_plan(&options, timestamp()?);
    write_command_file(&plan)?;

    let supervisor_pid = std::process::id();
    let mut restart_count = 0;

    loop {
        publish_surface(
            &mut node,
            &options,
            "active",
            std::slice::from_ref(&options.stream_id),
        )?;
        publish_runtime_boundary_records(
            &mut node,
            &options,
            "active",
            std::slice::from_ref(&options.stream_id),
            &[],
        )?;

        match run_rudp_mux_once(&options, &plan, &mut node, supervisor_pid, restart_count) {
            Ok(restart) => {
                restart_count += 1;
                publish_stream(
                    &mut node,
                    &options,
                    &plan,
                    "restarting",
                    supervisor_pid,
                    None,
                    restart_count,
                    &restart.detail,
                )?;
                thread::sleep(restart.delay);
            }
            Err(error) => {
                restart_count += 1;
                publish_stream(
                    &mut node,
                    &options,
                    &plan,
                    "failed",
                    supervisor_pid,
                    None,
                    restart_count,
                    &format!("RUDP mux failed: {error:#}"),
                )?;
                return Err(error);
            }
        }
    }
}

struct RudpMuxRestart {
    detail: String,
    delay: Duration,
}

fn rudp_media_activity_detail(options: &Options) -> String {
    match (options.capture_video, options.capture_audio) {
        (true, true) => "typed video/audio access units are publishing over CultNet RUDP media",
        (true, false) => "typed video access units are publishing over CultNet RUDP media",
        (false, true) => "typed audio packets are publishing over CultNet RUDP media",
        (false, false) => "Muninn RUDP media is idle",
    }
    .to_string()
}

struct RudpCatalogPublisher {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl RudpCatalogPublisher {
    fn start(
        options: Options,
        plan: MuxPlan,
        supervisor_pid: u32,
        mux_pid: Option<u32>,
        restart_count: u32,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut node = match open_node(&options, "muninn-rudp-catalog-publisher") {
                Ok(node) => node,
                Err(error) => {
                    eprintln!("Muninn catalog publisher could not open CultMesh state: {error:#}");
                    return;
                }
            };
            let mut next_publish_at = Instant::now()
                + Duration::from_millis(MUNINN_RUDP_ACTIVE_CATALOG_REPUBLISH_MS);
            while !thread_stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                if now >= next_publish_at {
                    if let Err(error) = publish_stream(
                        &mut node,
                        &options,
                        &plan,
                        "running",
                        supervisor_pid,
                        mux_pid,
                        restart_count,
                        &rudp_media_activity_detail(&options),
                    ) {
                        eprintln!("Muninn catalog publisher failed: {error:#}");
                    }
                    next_publish_at = Instant::now()
                        + Duration::from_millis(MUNINN_RUDP_ACTIVE_CATALOG_REPUBLISH_MS);
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        Self { stop, handle: Some(handle) }
    }
}

impl Drop for RudpCatalogPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run_rudp_mux_once(
    options: &Options,
    plan: &MuxPlan,
    node: &mut cultmesh_rs::CultMeshNode,
    supervisor_pid: u32,
    restart_count: u32,
) -> Result<RudpMuxRestart> {
    let media_profile = muninn_rudp_media_profile_for_options(options);
    let mut video_transport = open_media_rudp_transport(
        options,
        node,
        MUNINN_MEDIA_RUDP_CONNECTION_ID,
        None,
        "video",
    )?;
    let mut audio_transport = open_media_rudp_transport(
        options,
        node,
        MUNINN_AUDIO_RUDP_CONNECTION_ID,
        Some(media_profile.receiver_assembly_deadline_ms),
        "audio",
    )?;
    let timestamp = timestamp()?;
    let loopback_stderr = options
        .log_root
        .join(format!("muninn-{timestamp}.loopback.err.log"));
    let video_ffmpeg_stderr = options
        .log_root
        .join(format!("muninn-{timestamp}.ffmpeg-video.err.log"));
    let audio_ffmpeg_stderr = options
        .log_root
        .join(format!("muninn-{timestamp}.ffmpeg-audio.err.log"));

    let mut loopback = if options.capture_audio {
        Some(
            Command::new("powershell.exe")
                .args(loopback_args(options))
                .stdout(Stdio::piped())
                .stderr(fs::File::create(&loopback_stderr)?)
                .spawn()
                .with_context(|| {
                    format!(
                        "starting loopback capture {}",
                        options.loopback_script.display()
                    )
                })?,
        )
    } else {
        None
    };
    let loopback_stdout = if let Some(process) = loopback.as_mut() {
        Some(
            process
                .stdout
                .take()
                .context("loopback stdout was not piped")?,
        )
    } else {
        None
    };

    let mut video_ffmpeg = if options.capture_video {
        let (program, args, controllable) = if let Some(path) = &options.video_encoder_path {
            (
                path.as_os_str(),
                muninn_controllable_video_encoder_args(options),
                true,
            )
        } else {
            (
                std::ffi::OsStr::new(&options.ffmpeg_path),
                rudp_video_ffmpeg_args(options),
                false,
            )
        };
        Some(
            Command::new(program)
                .args(args)
                .stdin(if controllable { Stdio::piped() } else { Stdio::null() })
                .stdout(Stdio::piped())
                .stderr(fs::File::create(&video_ffmpeg_stderr)?)
                .spawn()
                .with_context(|| format!("starting {} video encoder", program.to_string_lossy()))?,
        )
    } else {
        None
    };
    let mut video_encoder_control = if options.video_encoder_path.is_some() {
        video_ffmpeg.as_mut().and_then(|process| process.stdin.take())
    } else {
        None
    };
    let video_ffmpeg_stdout = if let Some(process) = video_ffmpeg.as_mut() {
        Some(
            process
                .stdout
                .take()
                .context("video ffmpeg stdout was not piped")?,
        )
    } else {
        None
    };
    let mut audio_ffmpeg = if options.capture_audio {
        Some(
            Command::new(&options.ffmpeg_path)
                .args(rudp_audio_ffmpeg_args(options))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(fs::File::create(&audio_ffmpeg_stderr)?)
                .spawn()
                .with_context(|| format!("starting {} audio encoder", options.ffmpeg_path))?,
        )
    } else {
        None
    };
    let audio_ffmpeg_stdout = if let Some(process) = audio_ffmpeg.as_mut() {
        Some(
            process
                .stdout
                .take()
                .context("audio ffmpeg stdout was not piped")?,
        )
    } else {
        None
    };
    let audio_pump = match (loopback_stdout, audio_ffmpeg.as_mut()) {
        (Some(mut reader), Some(process)) => {
            let mut audio_ffmpeg_stdin = process
                .stdin
                .take()
                .context("audio ffmpeg stdin was not piped")?;
            Some(thread::spawn(move || -> Result<()> {
                std::io::copy(&mut reader, &mut audio_ffmpeg_stdin)?;
                Ok(())
            }))
        }
        _ => None,
    };

    let mut video_sender = None;
    let mut audio_sender = None;
    let result = (|| -> Result<RudpMuxRestart> {
        publish_stream(
            node,
            options,
            plan,
            "running",
            supervisor_pid,
            video_ffmpeg
                .as_ref()
                .map(Child::id)
                .or_else(|| audio_ffmpeg.as_ref().map(Child::id)),
            restart_count,
            &rudp_media_activity_detail(options),
        )?;
        let mux_pid = video_ffmpeg
            .as_ref()
            .map(Child::id)
            .or_else(|| audio_ffmpeg.as_ref().map(Child::id));
        let _catalog_publisher = RudpCatalogPublisher::start(
            options.clone(),
            plan.clone(),
            supervisor_pid,
            mux_pid,
            restart_count,
        );

        let (payload_tx, payload_rx) =
            mpsc::sync_channel::<Result<Vec<QueuedMuninnMediaSendPayload>>>(
                MUNINN_RUDP_MEDIA_PAYLOAD_CHANNEL_CAPACITY,
            );
        video_sender = if let Some(stdout) = video_ffmpeg_stdout {
            Some(video_rudp_payload_reader(
                payload_tx.clone(),
                stdout,
                VideoAnnexBStreamSendConfig {
                    stream_id: options.stream_id.clone(),
                    session_id: format!("{}:{timestamp}:video", options.host_id),
                    codec: media_profile.video_codec.to_string(),
                    first_frame_id: 0,
                    first_pts_ticks: 0,
                    frame_duration_ticks: video_frame_duration_ticks(options)?,
                    timebase_num: 1,
                    timebase_den: 90_000,
                    deadline_delay_ticks: rudp_media_deadline_delay_ticks(&media_profile),
                    max_payload_bytes: options.media_packet_bytes.max(256),
                    max_pending_bytes: options.media_packet_bytes.max(256) * 4096,
                    source_runtime_id: options.host_id.clone(),
                    source_role: "muninn.rudp.video".to_string(),
                },
            ))
        } else {
            None
        };
        audio_sender = if let Some(stdout) = audio_ffmpeg_stdout {
            Some(audio_rudp_payload_reader(
                payload_tx.clone(),
                stdout,
                AudioPcmStreamSendConfig {
                    stream_id: options.stream_id.clone(),
                    session_id: format!("{}:{timestamp}:audio", options.host_id),
                    codec: "pcm-f32le-interleaved".to_string(),
                    first_packet_id: 0,
                    first_pts_ticks: 0,
                    packet_duration_ticks: 480,
                    timebase_num: 1,
                    timebase_den: options.audio_sample_rate,
                    deadline_delay_ticks: rudp_audio_deadline_delay_ticks(options, &media_profile),
                    channels: options.audio_channels,
                    bytes_per_sample: 4,
                    max_pending_bytes: 480usize
                        .saturating_mul(options.audio_channels as usize)
                        .saturating_mul(4)
                        .saturating_mul(128),
                    source_runtime_id: options.host_id.clone(),
                    source_role: "muninn.rudp.audio".to_string(),
                },
            ))
        } else {
            None
        };
        drop(payload_tx);

        let mut payloads_sent = 0_u64;
        let mut payloads_queue_expired = 0_u64;
        let mut payloads_send_expired = 0_u64;
        let mut receiver_feedback = MuninnRudpReceiverFeedbackStats::default();
        let mut handled_keyframe_requests = 0_u64;
        let mut video_bitrate_controller = MuninnVideoBitrateController::new(
            media_profile.video_bitrate_kbps,
            Instant::now(),
        );
        request_video_encoder_bitrate(
            video_encoder_control.as_mut(),
            video_bitrate_controller.current_kbps(),
        )?;
        let mut repair_cache =
            RecentVideoChunkRepairCache::new(MUNINN_RUDP_MEDIA_REPAIR_CACHE_CHUNKS);
        let mut repair_budget = MuninnRudpRepairBudget::new(
            MUNINN_RUDP_MEDIA_REPAIR_INITIAL_CHUNKS_PER_SECOND,
            MUNINN_RUDP_MEDIA_REPAIR_BURST_CHUNKS,
        );
        let mut video_send_pacer = MuninnRudpMediaSendPacer::new(
            media_profile.sender_pace_every_payloads,
            Duration::from_micros(media_profile.sender_pace_sleep_us),
        );
        let mut audio_send_pacer = MuninnRudpMediaSendPacer::new(0, Duration::ZERO);
        let mut pending_payloads = PendingMuninnMediaSendQueues::default();
        let mut payload_channel_disconnected = false;
        loop {
            if pending_payloads.is_empty() && !payload_channel_disconnected {
                payload_channel_disconnected = receive_pending_media_payloads(
                    &payload_rx,
                    &mut pending_payloads,
                    Duration::from_millis(5),
                )?;
            } else if !payload_channel_disconnected {
                payload_channel_disconnected =
                    drain_available_media_payloads(&payload_rx, &mut pending_payloads)?;
            }

            if let Some(queued) = pending_payloads.pop_next() {
                if media_payload_queue_age_exceeded(
                    queued.queued_at,
                    Instant::now(),
                    Duration::from_millis(media_profile.sender_queue_deadline_ms),
                ) {
                    payloads_queue_expired += 1;
                    let payloads_dropped = payloads_queue_expired + payloads_send_expired;
                    poll_rudp_media_receiver_feedback(
                        &mut video_transport,
                        &mut receiver_feedback,
                        &repair_cache,
                        &mut repair_budget,
                        &media_profile,
                        &mut video_send_pacer,
                        payloads_dropped,
                    )?;
                    apply_video_encoder_feedback_control(
                        &receiver_feedback,
                        &mut handled_keyframe_requests,
                        &mut video_bitrate_controller,
                        video_encoder_control.as_mut(),
                        payloads_dropped,
                        media_profile.receiver_assembly_deadline_ms,
                    )?;
                    poll_rudp_resends_with_backpressure(&mut video_transport)?;
                    poll_rudp_resends_with_backpressure(&mut audio_transport)?;
                    if payloads_dropped == 1 || payloads_dropped % 300 == 0 {
                        let expired = reliable_packets_expired(&video_transport, &audio_transport);
                        eprintln!(
                            "{}",
                            rudp_media_progress_detail(
                                payloads_sent,
                                payloads_dropped,
                                payloads_queue_expired,
                                payloads_send_expired,
                                expired,
                                &receiver_feedback
                            )
                        );
                    }
                    continue;
                }

                let payload_len = queued.payload.payload.len();
                let sent = match queued.kind {
                    QueuedMuninnMediaKind::Video => send_rudp_media_payload_with_backpressure(
                        &mut video_transport,
                        queued.payload.clone(),
                        queued.queued_at,
                        Duration::from_millis(media_profile.sender_queue_deadline_ms),
                        &mut video_send_pacer,
                    )?,
                    QueuedMuninnMediaKind::Audio => send_rudp_media_payload_with_backpressure(
                        &mut audio_transport,
                        queued.payload.clone(),
                        queued.queued_at,
                        Duration::from_millis(media_profile.sender_queue_deadline_ms),
                        &mut audio_send_pacer,
                    )?,
                };
                if !sent {
                    payloads_send_expired += 1;
                    continue;
                }
                let payloads_dropped = payloads_queue_expired + payloads_send_expired;
                if queued.kind == QueuedMuninnMediaKind::Video {
                    repair_cache.remember(
                        &queued.payload,
                        Instant::now(),
                        Duration::from_millis(media_profile.sender_queue_deadline_ms),
                    )?;
                }
                poll_rudp_media_receiver_feedback(
                    &mut video_transport,
                    &mut receiver_feedback,
                    &repair_cache,
                    &mut repair_budget,
                    &media_profile,
                    &mut video_send_pacer,
                    payloads_dropped,
                )?;
                apply_video_encoder_feedback_control(
                    &receiver_feedback,
                    &mut handled_keyframe_requests,
                    &mut video_bitrate_controller,
                    video_encoder_control.as_mut(),
                    payloads_dropped,
                    media_profile.receiver_assembly_deadline_ms,
                )?;
                poll_rudp_resends_with_backpressure(&mut video_transport)?;
                poll_rudp_resends_with_backpressure(&mut audio_transport)?;
                payloads_sent += 1;
                if payloads_sent == 1 || payloads_sent % 900 == 0 {
                    let expired = reliable_packets_expired(&video_transport, &audio_transport);
                    eprintln!(
                        "{}; pending_audio={} pending_video={}; latest {:?} payload was {payload_len} bytes.",
                        rudp_media_progress_detail(
                            payloads_sent,
                            payloads_dropped,
                            payloads_queue_expired,
                            payloads_send_expired,
                            expired,
                            &receiver_feedback
                        ),
                        pending_payloads.audio_len(),
                        pending_payloads.video_len(),
                        queued.kind
                    );
                }
                continue;
            }

            if payload_channel_disconnected {
                let expired = reliable_packets_expired(&video_transport, &audio_transport);
                let payloads_dropped = payloads_queue_expired + payloads_send_expired;
                break Ok(RudpMuxRestart {
                    detail: format!(
                        "encoder stdout ended; {}",
                        rudp_media_progress_detail(
                            payloads_sent,
                            payloads_dropped,
                            payloads_queue_expired,
                            payloads_send_expired,
                            expired,
                            &receiver_feedback
                        )
                    ),
                    delay: default_rudp_mux_restart_delay(restart_count),
                });
            }

            {
                let payloads_dropped = payloads_queue_expired + payloads_send_expired;
                poll_rudp_media_receiver_feedback(
                    &mut video_transport,
                    &mut receiver_feedback,
                    &repair_cache,
                    &mut repair_budget,
                    &media_profile,
                    &mut video_send_pacer,
                    payloads_dropped,
                )?;
                apply_video_encoder_feedback_control(
                    &receiver_feedback,
                    &mut handled_keyframe_requests,
                    &mut video_bitrate_controller,
                    video_encoder_control.as_mut(),
                    payloads_dropped,
                    media_profile.receiver_assembly_deadline_ms,
                )?;
                poll_rudp_resends_with_backpressure(&mut video_transport)?;
                poll_rudp_resends_with_backpressure(&mut audio_transport)?;
            }
        }
    })();

    if let Some(process) = video_ffmpeg.as_mut() {
        let _ = process.kill();
        let _ = process.wait();
    }
    if let Some(process) = audio_ffmpeg.as_mut() {
        let _ = process.kill();
        let _ = process.wait();
    }
    if let Some(process) = loopback.as_mut() {
        let _ = process.kill();
        let _ = process.wait();
    }
    if let Some(pump) = audio_pump {
        let _ = pump.join();
    }
    if let Some(sender) = video_sender {
        let _ = sender.join();
    }
    if let Some(sender) = audio_sender {
        let _ = sender.join();
    }
    result
}

struct QueuedMuninnMediaSendPayload {
    payload: MuninnMediaSendPayload,
    queued_at: Instant,
    kind: QueuedMuninnMediaKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueuedMuninnMediaKind {
    Video,
    Audio,
}

#[derive(Default)]
struct PendingMuninnMediaSendQueues {
    active_audio: VecDeque<QueuedMuninnMediaSendPayload>,
    audio: VecDeque<VecDeque<QueuedMuninnMediaSendPayload>>,
    active_video: VecDeque<QueuedMuninnMediaSendPayload>,
    video: VecDeque<VecDeque<QueuedMuninnMediaSendPayload>>,
}

impl PendingMuninnMediaSendQueues {
    fn push(&mut self, payload: QueuedMuninnMediaSendPayload) {
        self.push_group(vec![payload]);
    }

    fn push_group(&mut self, payloads: Vec<QueuedMuninnMediaSendPayload>) {
        let Some(kind) = payloads.first().map(|payload| payload.kind) else {
            return;
        };
        debug_assert!(payloads.iter().all(|payload| payload.kind == kind));
        let group = VecDeque::from(payloads);
        match kind {
            QueuedMuninnMediaKind::Audio => {
                while self.audio_len().saturating_add(group.len())
                    > MUNINN_RUDP_MEDIA_PENDING_AUDIO_CAPACITY
                    && !self.audio.is_empty()
                {
                    self.audio.pop_front();
                }
                self.audio.push_back(group);
            }
            QueuedMuninnMediaKind::Video => {
                while self.video_len().saturating_add(group.len())
                    > MUNINN_RUDP_MEDIA_PENDING_VIDEO_CAPACITY
                    && !self.video.is_empty()
                {
                    self.video.pop_front();
                }
                self.video.push_back(group);
            }
        }
    }

    fn pop_next(&mut self) -> Option<QueuedMuninnMediaSendPayload> {
        Self::pop_from(&mut self.active_audio, &mut self.audio)
            .or_else(|| Self::pop_from(&mut self.active_video, &mut self.video))
    }

    fn pop_from(
        active: &mut VecDeque<QueuedMuninnMediaSendPayload>,
        queued: &mut VecDeque<VecDeque<QueuedMuninnMediaSendPayload>>,
    ) -> Option<QueuedMuninnMediaSendPayload> {
        if active.is_empty()
            && let Some(group) = queued.pop_front()
        {
            *active = group;
        }
        active.pop_front()
    }

    fn is_empty(&self) -> bool {
        self.active_audio.is_empty()
            && self.audio.is_empty()
            && self.active_video.is_empty()
            && self.video.is_empty()
    }

    fn audio_len(&self) -> usize {
        self.active_audio.len() + self.audio.iter().map(VecDeque::len).sum::<usize>()
    }

    fn video_len(&self) -> usize {
        self.active_video.len() + self.video.iter().map(VecDeque::len).sum::<usize>()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MuninnRudpReceiverFeedbackStats {
    feedback_records: u64,
    requested_keyframes: u64,
    late_frames: u64,
    missing_video_chunks: u64,
    repaired_video_chunks: u64,
    deferred_repair_chunks: u64,
    repair_chunks_per_second: usize,
    highest_decodable_frame_id: Option<u64>,
    latest_jitter_us: u64,
    latest_decode_queue_us: u64,
}

#[derive(Debug)]
struct MuninnVideoBitrateController {
    current_kbps: u32,
    min_kbps: u32,
    max_kbps: u32,
    last_late_frames: u64,
    last_keyframes: u64,
    last_deferred_repairs: u64,
    last_queue_dropped: u64,
    last_sample_at: Instant,
    stable_since: Instant,
}

impl MuninnVideoBitrateController {
    fn new(max_kbps: u32, now: Instant) -> Self {
        let max_kbps = max_kbps.max(1_000);
        let current_kbps = (max_kbps.saturating_mul(2) / 3).max(1_000);
        Self {
            current_kbps,
            min_kbps: (max_kbps / 4).max(1_000),
            max_kbps,
            last_late_frames: 0,
            last_keyframes: 0,
            last_deferred_repairs: 0,
            last_queue_dropped: 0,
            last_sample_at: now,
            stable_since: now,
        }
    }

    fn observe(
        &mut self,
        feedback: &MuninnRudpReceiverFeedbackStats,
        queue_dropped: u64,
        deadline_us: u64,
        now: Instant,
    ) -> Option<u32> {
        if now.duration_since(self.last_sample_at) < Duration::from_millis(500) {
            return None;
        }
        self.last_sample_at = now;
        let receiver_pressure = feedback
            .latest_jitter_us
            .saturating_add(feedback.latest_decode_queue_us)
            >= deadline_us.saturating_mul(3) / 4;
        let damaged = feedback.late_frames > self.last_late_frames
            || feedback.requested_keyframes > self.last_keyframes
            || feedback.deferred_repair_chunks > self.last_deferred_repairs
            || queue_dropped > self.last_queue_dropped
            || receiver_pressure;
        self.last_late_frames = feedback.late_frames;
        self.last_keyframes = feedback.requested_keyframes;
        self.last_deferred_repairs = feedback.deferred_repair_chunks;
        self.last_queue_dropped = queue_dropped;

        let previous = self.current_kbps;
        if damaged {
            self.current_kbps = (self.current_kbps.saturating_mul(85) / 100)
                .max(self.min_kbps);
            self.stable_since = now;
        } else if now.duration_since(self.stable_since) >= Duration::from_secs(10) {
            self.current_kbps = self
                .current_kbps
                .saturating_add((self.max_kbps / 50).max(100))
                .min(self.max_kbps);
            self.stable_since = now;
        }
        (self.current_kbps != previous).then_some(self.current_kbps)
    }

    fn current_kbps(&self) -> u32 {
        self.current_kbps
    }
}

#[derive(Debug)]
struct MuninnRudpRepairBudget {
    chunks_per_second: usize,
    min_chunks_per_second: usize,
    max_chunks_per_second: usize,
    add_chunks_per_second: usize,
    recovery_interval: Duration,
    max_available_chunks: usize,
    available_chunks: usize,
    last_refill_at: Instant,
    last_rate_adjust_at: Instant,
    last_queue_dropped: u64,
}

impl MuninnRudpRepairBudget {
    fn new(chunks_per_second: usize, max_available_chunks: usize) -> Self {
        let now = Instant::now();
        let max_available_chunks = max_available_chunks.max(1);
        Self {
            chunks_per_second: chunks_per_second.max(1),
            min_chunks_per_second: MUNINN_RUDP_MEDIA_REPAIR_MIN_CHUNKS_PER_SECOND,
            max_chunks_per_second: MUNINN_RUDP_MEDIA_REPAIR_MAX_CHUNKS_PER_SECOND,
            add_chunks_per_second: MUNINN_RUDP_MEDIA_REPAIR_ADD_CHUNKS_PER_SECOND,
            recovery_interval: Duration::from_millis(MUNINN_RUDP_MEDIA_REPAIR_RECOVERY_INTERVAL_MS),
            max_available_chunks,
            available_chunks: max_available_chunks,
            last_refill_at: now,
            last_rate_adjust_at: now,
            last_queue_dropped: 0,
        }
    }

    fn chunks_per_second(&self) -> usize {
        self.chunks_per_second
    }

    fn take(&mut self, requested_chunks: usize, now: Instant, queue_dropped: u64) -> usize {
        self.adjust_rate(now, queue_dropped, requested_chunks);
        self.refill(now);
        let allowed = requested_chunks.min(self.available_chunks);
        self.available_chunks -= allowed;
        allowed
    }

    fn adjust_rate(&mut self, now: Instant, queue_dropped: u64, requested_chunks: usize) {
        if queue_dropped > self.last_queue_dropped {
            self.chunks_per_second = (self.chunks_per_second / 2).max(self.min_chunks_per_second);
            self.available_chunks = self.available_chunks.min(self.chunks_per_second);
            self.last_queue_dropped = queue_dropped;
            self.last_rate_adjust_at = now;
            return;
        }
        self.last_queue_dropped = queue_dropped;
        if requested_chunks == 0
            || now.saturating_duration_since(self.last_rate_adjust_at) < self.recovery_interval
        {
            return;
        }
        if self.chunks_per_second < self.max_chunks_per_second {
            self.chunks_per_second = self
                .chunks_per_second
                .saturating_add(self.add_chunks_per_second)
                .min(self.max_chunks_per_second);
            self.last_rate_adjust_at = now;
        }
    }

    fn refill(&mut self, now: Instant) {
        let elapsed_ms = now
            .saturating_duration_since(self.last_refill_at)
            .as_millis() as usize;
        if elapsed_ms == 0 {
            return;
        }
        let refill_chunks = elapsed_ms.saturating_mul(self.chunks_per_second) / 1_000;
        if refill_chunks == 0 {
            return;
        }
        self.available_chunks = self
            .available_chunks
            .saturating_add(refill_chunks)
            .min(self.max_available_chunks);
        self.last_refill_at = now;
    }
}

#[derive(Debug)]
struct RecentVideoChunkRepairCache {
    max_entries: usize,
    order: VecDeque<String>,
    entries: HashMap<String, CachedVideoRepair>,
}

#[derive(Debug)]
struct CachedVideoRepair {
    payload: MuninnMediaSendPayload,
    expires_at: Instant,
}

impl RecentVideoChunkRepairCache {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn remember(
        &mut self,
        payload: &MuninnMediaSendPayload,
        now: Instant,
        lifetime: Duration,
    ) -> Result<()> {
        let Some(key) = video_repair_cache_key_from_payload(payload)? else {
            return Ok(());
        };
        let cached = CachedVideoRepair {
            payload: payload.clone(),
            expires_at: now + lifetime,
        };
        if self.entries.contains_key(&key) {
            self.entries.insert(key, cached);
            return Ok(());
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, cached);
        while self.entries.len() > self.max_entries {
            let Some(expired) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&expired);
        }
        Ok(())
    }

    fn repair_payloads_for_feedback(
        &self,
        feedback: &MuninnMediaReceiverFeedbackRecord,
        now: Instant,
    ) -> Vec<MuninnMediaSendPayload> {
        feedback
            .missing_video_chunk_keys
            .iter()
            .filter(|chunk_key| {
                chunk_key
                    .split_once(':')
                    .and_then(|(frame_id, _)| frame_id.parse::<u64>().ok())
                    .is_none_or(|frame_id| !feedback.late_frame_ids.contains(&frame_id))
            })
            .filter_map(|chunk_key| {
                self.entries
                    .get(&video_repair_cache_key(
                        &feedback.stream_id,
                        &feedback.session_id,
                        chunk_key,
                    ))
                    .filter(|cached| cached.expires_at > now)
                    .map(|cached| cached.payload.clone())
            })
            .collect()
    }
}

fn video_repair_cache_key(stream_id: &str, session_id: &str, chunk_key: &str) -> String {
    format!("{stream_id}:{session_id}:video:{chunk_key}")
}

fn video_repair_cache_key_from_payload(payload: &MuninnMediaSendPayload) -> Result<Option<String>> {
    if payload.channel_id != crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL {
        return Ok(None);
    }
    let MuninnMediaWireRecord::Video(video) = decode_media_wire_record(&payload.payload)? else {
        return Ok(None);
    };
    Ok(Some(video_repair_cache_key(
        &video.stream_id,
        &video.session_id,
        &crate::media_packetizer::video_chunk_feedback_key(video.frame_id, video.chunk_index),
    )))
}

fn queue_muninn_media_payloads(
    tx: &mpsc::SyncSender<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    payloads: Vec<MuninnMediaSendPayload>,
    kind: QueuedMuninnMediaKind,
) -> Result<()> {
    if payloads.is_empty() {
        return Ok(());
    }
    let queued_at = Instant::now();
    tx.send(Ok(payloads
        .into_iter()
        .map(|payload| QueuedMuninnMediaSendPayload {
            payload,
            queued_at,
            kind,
        })
        .collect()))
        .context("queueing typed Muninn media payload group")
}

fn queue_muninn_video_payloads_by_access_unit(
    tx: &mpsc::SyncSender<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    payloads: Vec<MuninnMediaSendPayload>,
) -> Result<()> {
    let mut current_frame_id = None;
    let mut current = Vec::new();
    for payload in payloads {
        let frame_id = match decode_media_wire_record(&payload.payload)? {
            MuninnMediaWireRecord::Video(record) => record.frame_id,
            MuninnMediaWireRecord::VideoParity(record) => record.frame_id,
            _ => return Err(anyhow!("video packetizer emitted a non-video media record")),
        };
        if current_frame_id.is_some_and(|current_id| current_id != frame_id) {
            queue_muninn_media_payloads(tx, std::mem::take(&mut current), QueuedMuninnMediaKind::Video)?;
        }
        current_frame_id = Some(frame_id);
        current.push(payload);
    }
    queue_muninn_media_payloads(tx, current, QueuedMuninnMediaKind::Video)
}

fn receive_pending_media_payloads(
    rx: &mpsc::Receiver<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    pending: &mut PendingMuninnMediaSendQueues,
    timeout: Duration,
) -> Result<bool> {
    match rx.recv_timeout(timeout) {
        Ok(Ok(payloads)) => {
            pending.push_group(payloads);
            drain_available_media_payloads(rx, pending)
        }
        Ok(Err(error)) => Err(error),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
        Err(mpsc::RecvTimeoutError::Disconnected) => Ok(true),
    }
}

fn drain_available_media_payloads(
    rx: &mpsc::Receiver<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    pending: &mut PendingMuninnMediaSendQueues,
) -> Result<bool> {
    for _ in 0..MUNINN_RUDP_MEDIA_INGEST_BUDGET_PER_TURN {
        match rx.try_recv() {
            Ok(Ok(payloads)) => pending.push_group(payloads),
            Ok(Err(error)) => return Err(error),
            Err(mpsc::TryRecvError::Empty) => return Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => return Ok(true),
        }
    }
    Ok(false)
}

fn media_payload_queue_age_exceeded(queued_at: Instant, now: Instant, max_age: Duration) -> bool {
    now.saturating_duration_since(queued_at) > max_age
}

fn poll_rudp_resends_with_backpressure(
    transport: &mut CultNetRudpSocketTransportConnection,
) -> Result<()> {
    match transport.poll_resends() {
        Ok(()) => Ok(()),
        Err(error) if is_would_block_error(&error) => Ok(()),
        Err(error) => Err(error).context("polling Muninn RUDP media resends"),
    }
}

fn send_rudp_media_payload_with_backpressure(
    transport: &mut CultNetRudpSocketTransportConnection,
    payload: MuninnMediaSendPayload,
    queued_at: Instant,
    max_age: Duration,
    send_pacer: &mut MuninnRudpMediaSendPacer,
) -> Result<bool> {
    loop {
        match transport.send(payload.channel_id, payload.payload.clone()) {
            Ok(()) => {
                send_pacer.observe_sent_payload();
                return Ok(true);
            }
            Err(error) if is_would_block_error(&error) => {
                if media_payload_queue_age_exceeded(queued_at, Instant::now(), max_age) {
                    return Ok(false);
                }
                for _ in 0..MUNINN_RUDP_MEDIA_MAX_PENDING_RELIABLE_PACKETS {
                    if !transport.poll_receive_once()? {
                        break;
                    }
                }
                poll_rudp_resends_with_backpressure(transport)?;
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => {
                return Err(error).context("sending typed Muninn media payload over RUDP media");
            }
        }
    }
}

#[derive(Debug)]
struct MuninnRudpMediaSendPacer {
    payloads_since_pause: usize,
    every_payloads: usize,
    sleep_for: Duration,
}

impl MuninnRudpMediaSendPacer {
    fn new(every_payloads: usize, sleep_for: Duration) -> Self {
        Self {
            payloads_since_pause: 0,
            every_payloads: every_payloads.max(1),
            sleep_for,
        }
    }

    fn observe_sent_payload(&mut self) {
        if self.sleep_for.is_zero() {
            return;
        }
        self.payloads_since_pause = self.payloads_since_pause.saturating_add(1);
        if self.payloads_since_pause < self.every_payloads {
            return;
        }
        self.payloads_since_pause = 0;
        thread::sleep(self.sleep_for);
    }
}

fn is_would_block_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == ErrorKind::WouldBlock)
            || cause.to_string().contains("os error 10035")
    })
}

fn default_rudp_mux_restart_delay(restart_count: u32) -> Duration {
    Duration::from_secs((2 + restart_count).min(30) as u64)
}

fn record_receiver_keyframe_pressure(
    receiver_feedback: &MuninnRudpReceiverFeedbackStats,
    handled_keyframe_requests: &mut u64,
) -> bool {
    if receiver_feedback.requested_keyframes <= *handled_keyframe_requests {
        return false;
    }
    *handled_keyframe_requests = receiver_feedback.requested_keyframes;
    true
}

fn request_video_encoder_idr<W: std::io::Write + ?Sized>(control: Option<&mut W>) -> Result<()> {
    if let Some(control) = control {
        control.write_all(b"IDR\n")?;
        control.flush()?;
        eprintln!("Muninn RUDP receiver invalidated the decode chain; requested next-frame IDR.");
    } else {
        eprintln!(
            "Muninn RUDP receiver invalidated the decode chain; scheduled quarter-second IDR remains the recovery ceiling because the configured FFmpeg CLI has no live encoder command surface."
        );
    }
    Ok(())
}

fn request_video_encoder_bitrate<W: std::io::Write + ?Sized>(
    control: Option<&mut W>,
    bitrate_kbps: u32,
) -> Result<()> {
    if let Some(control) = control {
        writeln!(control, "BITRATE {bitrate_kbps}")?;
        control.flush()?;
        eprintln!("Muninn adjusted live NVENC bitrate to {bitrate_kbps} kbps.");
    }
    Ok(())
}

fn apply_video_encoder_feedback_control(
    feedback: &MuninnRudpReceiverFeedbackStats,
    handled_keyframe_requests: &mut u64,
    bitrate: &mut MuninnVideoBitrateController,
    mut control: Option<&mut std::process::ChildStdin>,
    queue_dropped: u64,
    deadline_ms: u64,
) -> Result<()> {
    if record_receiver_keyframe_pressure(feedback, handled_keyframe_requests) {
        request_video_encoder_idr(control.as_deref_mut())?;
    }
    if let Some(kbps) = bitrate.observe(
        feedback,
        queue_dropped,
        deadline_ms.saturating_mul(1_000),
        Instant::now(),
    ) {
        request_video_encoder_bitrate(control.as_deref_mut(), kbps)?;
    }
    Ok(())
}

fn rudp_media_progress_detail(
    sent: u64,
    queue_dropped: u64,
    queue_expired: u64,
    send_expired: u64,
    reliable_expired: u64,
    receiver_feedback: &MuninnRudpReceiverFeedbackStats,
) -> String {
    format!(
        "Muninn RUDP media progress: sent={sent} queue_dropped={queue_dropped} queue_expired={queue_expired} send_expired={send_expired} reliable_expired={reliable_expired} receiver_feedback={} receiver_keyframes={} receiver_late_frames={} receiver_missing_chunks={} receiver_repaired_chunks={} receiver_deferred_repairs={} repair_rate={} receiver_highest_decodable={}",
        receiver_feedback.feedback_records,
        receiver_feedback.requested_keyframes,
        receiver_feedback.late_frames,
        receiver_feedback.missing_video_chunks,
        receiver_feedback.repaired_video_chunks,
        receiver_feedback.deferred_repair_chunks,
        receiver_feedback.repair_chunks_per_second,
        receiver_feedback
            .highest_decodable_frame_id
            .map(|frame_id| frame_id.to_string())
            .unwrap_or_else(|| "none".to_string())
    )
}

fn poll_rudp_media_receiver_feedback(
    transport: &mut CultNetRudpSocketTransportConnection,
    stats: &mut MuninnRudpReceiverFeedbackStats,
    repair_cache: &RecentVideoChunkRepairCache,
    repair_budget: &mut MuninnRudpRepairBudget,
    media_profile: &MuninnRudpMediaProfile,
    send_pacer: &mut MuninnRudpMediaSendPacer,
    queue_dropped: u64,
) -> Result<()> {
    let mut feedback_processed = 0_usize;
    loop {
        match transport.receive_once() {
            Ok(Some(frame)) => {
                if feedback_processed >= MUNINN_RUDP_MEDIA_REPAIR_MAX_FEEDBACK_PER_POLL {
                    return Ok(());
                }
                feedback_processed += 1;
                let repair_payloads =
                    record_rudp_media_receiver_feedback(&frame, stats, repair_cache)?;
                let requested_repairs = repair_payloads.len();
                let poll_limited_repairs =
                    requested_repairs.min(MUNINN_RUDP_MEDIA_REPAIR_MAX_CHUNKS_PER_POLL);
                let allowed_repairs =
                    repair_budget.take(poll_limited_repairs, Instant::now(), queue_dropped);
                stats.repair_chunks_per_second = repair_budget.chunks_per_second();
                stats.deferred_repair_chunks = stats
                    .deferred_repair_chunks
                    .saturating_add(requested_repairs.saturating_sub(allowed_repairs) as u64);
                for payload in repair_payloads.into_iter().take(allowed_repairs) {
                    if send_rudp_media_payload_with_backpressure(
                        transport,
                        payload,
                        Instant::now(),
                        Duration::from_millis(media_profile.sender_queue_deadline_ms),
                        send_pacer,
                    )? {
                        stats.repaired_video_chunks = stats.repaired_video_chunks.saturating_add(1);
                    }
                }
            }
            Ok(None) => return Ok(()),
            Err(error) if is_would_block_error(&error) => return Ok(()),
            Err(error) => return Err(error).context("polling Muninn RUDP media feedback"),
        }
    }
}

fn record_rudp_media_receiver_feedback(
    frame: &CultNetTransportFrame,
    stats: &mut MuninnRudpReceiverFeedbackStats,
    repair_cache: &RecentVideoChunkRepairCache,
) -> Result<Vec<MuninnMediaSendPayload>> {
    if frame.channel_id != crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL {
        return Ok(Vec::new());
    }

    let MuninnMediaWireRecord::Feedback(feedback) = decode_media_wire_record(&frame.payload)?
    else {
        return Ok(Vec::new());
    };

    let repair_payloads = repair_cache.repair_payloads_for_feedback(&feedback, Instant::now());
    stats.feedback_records = stats.feedback_records.saturating_add(1);
    if feedback.requested_keyframe {
        stats.requested_keyframes = stats.requested_keyframes.saturating_add(1);
    }
    stats.late_frames = stats
        .late_frames
        .saturating_add(feedback.late_frame_ids.len() as u64);
    stats.missing_video_chunks = stats
        .missing_video_chunks
        .saturating_add(feedback.missing_video_chunk_keys.len() as u64);
    stats.latest_jitter_us = feedback.jitter_us.max(0) as u64;
    stats.latest_decode_queue_us = feedback.decode_queue_us.max(0) as u64;
    if let Some(frame_id) = feedback.highest_decodable_frame_id {
        stats.highest_decodable_frame_id = Some(
            stats
                .highest_decodable_frame_id
                .map(|current| current.max(frame_id))
                .unwrap_or(frame_id),
        );
    }
    Ok(repair_payloads)
}

fn open_media_rudp_transport(
    options: &Options,
    node: &mut cultmesh_rs::CultMeshNode,
    connection_id: u32,
    reliable_expire_after_ms: Option<u64>,
    role: &str,
) -> Result<CultNetRudpSocketTransportConnection> {
    let media_profile = muninn_rudp_media_profile_for_options(options);
    let endpoint = resolve_media_rudp_endpoint(options, node)?;
    let socket = UdpSocket::bind("0.0.0.0:0").context("binding Muninn media RUDP client socket")?;
    configure_media_rudp_socket_buffers(&socket)
        .context("configuring Muninn media RUDP socket buffers")?;
    socket
        .set_nonblocking(true)
        .context("setting Muninn media RUDP client nonblocking")?;
    let mut transport = CultNetRudpSocketTransportConnection::new(muninn_media_rudp_options(
        socket,
        endpoint,
        &media_profile,
        connection_id,
        reliable_expire_after_ms,
    ))?;
    transport.connect(options.stream_id.as_bytes().to_vec())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !transport.connected() {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out connecting Muninn {role} RUDP stream to {endpoint}"
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(transport)
}

fn resolve_media_rudp_endpoint(
    options: &Options,
    node: &mut cultmesh_rs::CultMeshNode,
) -> Result<SocketAddr> {
    require_media_target_uri(options)?;
    if let Some(obs_target_host) = options.obs_target_host.as_deref()
        && !obs_target_host.trim().is_empty()
        && options.obs_port != 0
    {
        let endpoint = format!("{}:{}", obs_target_host.trim(), options.obs_port);
        return endpoint.parse().with_context(|| {
            format!(
                "parsing command-owned OBS Muninn media RUDP endpoint {endpoint} for {}",
                options.target_host
            )
        });
    }
    pull_odin_media_catalog_snapshot(node, options);
    let endpoint = discover_provider_endpoints(
        node,
        OdinEndpointQuery {
            schema: Some(MUNINN_MEDIA_RUDP_SCHEMA),
            transport_contains: Some("rudp"),
            host_hint: Some(&options.target_host),
            device_filter: Some(&options.stream_id),
        },
    )
    .into_iter()
    .next()
    .or_else(|| {
        discover_provider_endpoints(
            node,
            OdinEndpointQuery {
                schema: Some(MUNINN_MEDIA_RUDP_SCHEMA),
                transport_contains: Some("rudp"),
                host_hint: Some(&options.target_host),
                device_filter: None,
            },
        )
        .into_iter()
        .next()
    })
    .ok_or_else(|| {
        anyhow!(
            "Odin provider catalog did not advertise a {} endpoint for {}",
            MUNINN_MEDIA_RUDP_SCHEMA,
            options.target_host
        )
    })?;
    endpoint.address.parse().with_context(|| {
        format!(
            "parsing Odin-discovered Muninn media RUDP endpoint {} for {}",
            endpoint.address, options.target_host
        )
    })
}

fn pull_odin_media_catalog_snapshot(node: &mut cultmesh_rs::CultMeshNode, options: &Options) {
    let Some(target) = resolve_odin_cultmesh_uri(options) else {
        return;
    };
    if let Err(error) = node.pull_rudp_catalog_snapshot(CultMeshRudpSnapshotOptions {
        target,
        runtime_id: format!("muninn-{}-media-target-catalog-client", options.host_id),
        schema_ids: Some(vec![EVE_PROVIDER_ADVERTISEMENT_SCHEMA.to_string()]),
        connect_timeout: Duration::from_millis(150),
        response_timeout: Duration::from_millis(150),
        resend_delay_ms: 15,
        ..CultMeshRudpSnapshotOptions::default()
    }) {
        eprintln!("Muninn Odin media target catalog pull failed from {target}: {error:#}");
    }
}

fn configure_media_rudp_socket_buffers(socket: &UdpSocket) -> Result<()> {
    let socket = socket2::SockRef::from(socket);
    socket
        .set_send_buffer_size(MUNINN_RUDP_MEDIA_SOCKET_BUFFER_BYTES)
        .context("setting Muninn media RUDP send buffer")?;
    socket
        .set_recv_buffer_size(MUNINN_RUDP_MEDIA_SOCKET_BUFFER_BYTES)
        .context("setting Muninn media RUDP receive buffer")?;
    Ok(())
}

fn muninn_media_rudp_options(
    socket: UdpSocket,
    endpoint: SocketAddr,
    media_profile: &MuninnRudpMediaProfile,
    connection_id: u32,
    _reliable_expire_after_ms: Option<u64>,
) -> CultNetRudpSocketTransportOptions {
    let mut options =
        CultNetRudpSocketTransportOptions::client("muninn-media", socket, endpoint, connection_id);
    options.resend_delay_ms = media_profile.sender_resend_delay_ms;
    options.max_fragment_bytes = Some(media_profile.max_fragment_bytes as u32);
    options.max_pending_reliable_packets = Some(MUNINN_RUDP_MEDIA_MAX_PENDING_RELIABLE_PACKETS);
    options
}

fn reliable_packets_expired(
    video_transport: &CultNetRudpSocketTransportConnection,
    audio_transport: &CultNetRudpSocketTransportConnection,
) -> u64 {
    let _ = (video_transport, audio_transport);
    0
}

fn video_rudp_payload_reader<R>(
    tx: mpsc::SyncSender<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    mut reader: R,
    config: VideoAnnexBStreamSendConfig,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        if let Err(error) = read_video_rudp_payloads(&tx, &mut reader, config) {
            let _ = tx.send(Err(error));
        }
    })
}

fn audio_rudp_payload_reader<R>(
    tx: mpsc::SyncSender<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    mut reader: R,
    config: AudioPcmStreamSendConfig,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        if let Err(error) = read_audio_rudp_payloads(&tx, &mut reader, config) {
            let _ = tx.send(Err(error));
        }
    })
}

fn read_video_rudp_payloads<R>(
    tx: &mpsc::SyncSender<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    reader: &mut R,
    config: VideoAnnexBStreamSendConfig,
) -> Result<()>
where
    R: Read,
{
    let mut sender = VideoAnnexBStreamSendState::new(config)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .context("reading encoded Annex B video from ffmpeg stdout")?;
        if read == 0 {
            queue_muninn_video_payloads_by_access_unit(
                tx,
                sender.finish(&timestamp()?)?,
            )
            .context("queueing final typed video media payload group")?;
            return Ok(());
        }
        queue_muninn_video_payloads_by_access_unit(
            tx,
            sender.push(&timestamp()?, &buffer[..read])?,
        )
        .context("queueing typed video media payload group")?;
    }
}

fn read_audio_rudp_payloads<R>(
    tx: &mpsc::SyncSender<Result<Vec<QueuedMuninnMediaSendPayload>>>,
    reader: &mut R,
    config: AudioPcmStreamSendConfig,
) -> Result<()>
where
    R: Read,
{
    let mut sender = AudioPcmStreamSendState::new(config)?;
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .context("reading PCM audio from ffmpeg stdout")?;
        if read == 0 {
            queue_muninn_media_payloads(
                tx,
                sender.finish(&timestamp()?)?,
                QueuedMuninnMediaKind::Audio,
            )
            .context("queueing final typed audio media payload group")?;
            return Ok(());
        }
        queue_muninn_media_payloads(
            tx,
            sender.push(&timestamp()?, &buffer[..read])?,
            QueuedMuninnMediaKind::Audio,
        )
        .context("queueing typed audio media payload group")?;
    }
}

fn video_frame_duration_ticks(options: &Options) -> Result<u32> {
    if options.framerate == 0 {
        return Err(anyhow!("framerate must be greater than zero"));
    }
    Ok((90_000_u32 / options.framerate).max(1))
}

fn rudp_media_deadline_delay_ticks(media_profile: &MuninnRudpMediaProfile) -> i64 {
    i64::try_from(
        media_profile
            .receiver_assembly_deadline_ms
            .saturating_mul(90),
    )
    .unwrap_or(i64::MAX)
}

fn rudp_audio_deadline_delay_ticks(
    options: &Options,
    media_profile: &MuninnRudpMediaProfile,
) -> i64 {
    if options.audio_sample_rate == 0 {
        return i64::from(1_024);
    }
    i64::try_from(
        media_profile
            .receiver_assembly_deadline_ms
            .saturating_mul(u64::from(options.audio_sample_rate))
            / 1_000,
    )
    .unwrap_or(i64::MAX)
    .max(i64::from(1_024))
}

fn publish_surface(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    state: &str,
    active_streams: &[String],
) -> Result<()> {
    let video_sources = video_source_catalog(options);
    let audio_sources = audio_source_catalog(options);
    let record = MuninnTelemetrySurfaceRecord {
        surface_id: options.surface_id.clone(),
        host_id: options.host_id.clone(),
        state: state.to_string(),
        available_sources: available_sources(options),
        stream_affordances: vec![
            "screen.capture.ddagrab".to_string(),
            "audio.loopback.wasapi".to_string(),
            "psmove.light.command".to_string(),
            "psmove.controller.state".to_string(),
            "quest.usb.access".to_string(),
            "quest.input.openxr.witness".to_string(),
            "quest.video.warp_corrected_input".to_string(),
            "audio.input.enumeration.pending".to_string(),
            "video.input.enumeration.pending".to_string(),
        ],
        active_streams: active_streams.to_vec(),
        activation_authority: "muninn.activate.explicit-request".to_string(),
        detail: if active_streams.is_empty() {
            "Muninn is idle; no capture streams are consuming resources.".to_string()
        } else {
            "Muninn has explicit active stream requests.".to_string()
        },
        updated_at: timestamp()?,
        primary_stream_id: options.stream_id.clone(),
        primary_stream_label: format!("{} screen and loopback A/V", options.host_id),
        command_rudp_target: String::new(),
        media_target_host: options.target_host.clone(),
        media_port: options.port,
        media_packet_bytes: options.media_packet_bytes as u32,
        rudp_video_bitrate_kbps: options.rudp_video_bitrate_kbps,
        rudp_latency_budget_ms: options.rudp_latency_budget_ms,
        video_source_ids: video_sources
            .iter()
            .map(|source| source.id.clone())
            .collect(),
        video_source_labels: video_sources
            .iter()
            .map(|source| source.label.clone())
            .collect(),
        audio_source_ids: audio_sources
            .iter()
            .map(|source| source.id.clone())
            .collect(),
        audio_source_labels: audio_sources
            .iter()
            .map(|source| source.label.clone())
            .collect(),
    };
    node.put("latest", &record)?;
    node.put(&record.surface_id, &record)?;
    publish_move_hue_eve_surface(node, options)?;
    Ok(())
}

fn publish_move_hue_eve_surface(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
) -> Result<()> {
    let program_key = move_hue_program_key(&options.host_id);
    let program = node
        .get::<MuninnMoveHueProgramRecord>(&program_key)?
        .unwrap_or_else(|| bootstrap_move_hue_program(options));
    let provider_id = muninn_provider_id(options);
    let surface_key = move_hue_surface_key(&options.host_id);
    let action = |id: &str, label: &str, mode: Option<&str>, order_mode: Option<&str>, cycle_ms: Option<u64>, transition_percent: Option<u8>| {
        json!({
            "id": format!("{provider_id}.move-hue.{id}"),
            "kind": "card",
            "props": {
                "title": label,
                "commandId": "muninn.set-move-hue-program",
                "action": {
                    "type": "muninn.set-move-hue-program",
                    "providerId": provider_id,
                    "programId": program_key,
                    "schema": MUNINN_MOVE_HUE_PROGRAM_SCHEMA,
                    "mode": mode,
                    "orderMode": order_mode,
                    "cycleMs": cycle_ms,
                    "transitionPercent": transition_percent
                }
            },
            "children": []
        })
    };
    let surface = EveSurfaceStateRecord {
        provider_id: provider_id.clone(),
        title: format!("Muninn {} Move Hue Program", options.host_id),
        version: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        updated_at: timestamp()?,
        surface: json!({
            "schema": "gamecult.eve.surface.v1",
            "id": format!("{provider_id}.move-hue.surface"),
            "title": "Move Hue Program",
            "root": {
                "id": format!("{provider_id}.move-hue.root"),
                "kind": "dashboard",
                "props": {
                    "title": "Move Hue Program",
                    "summary": format!("{} / {} / {} ms / {}% transition", program.mode, program.order_mode, program.cycle_ms, effective_transition_percent(&program))
                },
                "children": [
                    {
                        "id": format!("{provider_id}.move-hue.state"),
                        "kind": "pane",
                        "props": { "title": "State" },
                        "children": [
                            { "id": format!("{provider_id}.move-hue.state.mode"), "kind": "text", "props": { "text": format!("mode: {}", program.mode) } },
                            { "id": format!("{provider_id}.move-hue.state.order"), "kind": "text", "props": { "text": format!("order: {}", program.order_mode) } },
                            { "id": format!("{provider_id}.move-hue.state.rate"), "kind": "text", "props": { "text": format!("cycle: {} ms", program.cycle_ms) } },
                            { "id": format!("{provider_id}.move-hue.state.transition"), "kind": "text", "props": { "text": format!("transition: {}%", effective_transition_percent(&program)) } }
                        ]
                    },
                    {
                        "id": format!("{provider_id}.move-hue.mode"),
                        "kind": "pane",
                        "props": { "title": "Mode" },
                        "children": [
                            action("mode-animated", "Animate", Some("animated"), None, None, None),
                            action("mode-hold", "Hold Current Colors", Some("hold"), None, None, None),
                            action("mode-static", "Static Palette", Some("static"), None, None, None)
                        ]
                    },
                    {
                        "id": format!("{provider_id}.move-hue.order"),
                        "kind": "pane",
                        "props": { "title": "Update Order" },
                        "children": [
                            action("order-descending", "Descending", None, Some("descending"), None, None),
                            action("order-ascending", "Ascending", None, Some("ascending"), None, None),
                            action("order-bounce", "Bounce", None, Some("bounce"), None, None),
                            action("order-rotating", "Rotating Lead", None, Some("rotating-lead"), None, None),
                            action("order-golden", "Golden Permutation", None, Some("golden-permutation"), None, None)
                        ]
                    },
                    {
                        "id": format!("{provider_id}.move-hue.rate"),
                        "kind": "pane",
                        "props": { "title": "Cycle Rate" },
                        "children": [
                            action("rate-500", "2 Hz", None, None, Some(500), None),
                            action("rate-1000", "1 Hz", None, None, Some(1000), None),
                            action("rate-2000", "0.5 Hz", None, None, Some(2000), None)
                        ]
                    },
                    {
                        "id": format!("{provider_id}.move-hue.transition"),
                        "kind": "pane",
                        "props": { "title": "Transition Duration" },
                        "children": [
                            action("transition-0", "Off", None, None, None, Some(0)),
                            action("transition-10", "10%", None, None, None, Some(10)),
                            action("transition-25", "25%", None, None, None, Some(25)),
                            action("transition-50", "50%", None, None, None, Some(50)),
                            action("transition-75", "75%", None, None, None, Some(75)),
                            action("transition-100", "100%", None, None, None, Some(100))
                        ]
                    }
                ]
            }
        }),
    };
    node.put(&surface_key, &surface)?;
    if let Some(target) = resolve_odin_cultmesh_uri(options) {
        let _ = node.publish_document_to_rudp_catalog(
            &program_key,
            &program,
            CultMeshRudpDocumentPublishOptions {
                target,
                runtime_id: muninn_daemon_id(options),
                source_role: Some("muninn.move-hue-program-state".to_string()),
                tags: vec!["muninn".to_string(), "move-hue-program".to_string()],
                flush_timeout: Duration::from_millis(300),
                resend_delay_ms: 15,
                ..CultMeshRudpDocumentPublishOptions::default()
            },
        );
        let _ = node.publish_document_to_rudp_catalog(
            &surface_key,
            &surface,
            CultMeshRudpDocumentPublishOptions {
                target,
                runtime_id: muninn_daemon_id(options),
                source_role: Some("muninn.move-hue-eve-surface".to_string()),
                tags: vec!["muninn".to_string(), "eve-surface".to_string()],
                flush_timeout: Duration::from_millis(300),
                resend_delay_ms: 15,
                ..CultMeshRudpDocumentPublishOptions::default()
            },
        );
    }
    Ok(())
}

fn publish_runtime_boundary_records(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    state: &str,
    active_streams: &[String],
    live_move_sources: &[MoveStateSource],
) -> Result<()> {
    let provider_id = muninn_provider_id(options);
    let daemon_id = muninn_daemon_id(options);
    let updated_at = timestamp()?;
    let available_sources = available_sources(options);
    let command_boundary_key = format!("command-boundary:{daemon_id}");
    let transport_profile_key = format!("transport-profile:{daemon_id}");
    let store_path = options.store_path.display().to_string();
    let log_root = options.log_root.display().to_string();
    let stream_id = options.stream_id.clone();
    let activation_store_path = options
        .activation_store_path
        .as_deref()
        .unwrap_or(&options.store_path);
    let activation_command = if options.target_host.trim().is_empty() {
        None
    } else {
        let command = format!(
            "muninn request-stream --store {} --activate-store {} --host {} --stream {} --target-host {} --port {} --media-transport {}",
            store_path,
            activation_store_path.display(),
            options.host_id,
            options.stream_id,
            options.target_host,
            options.port,
            media_transport_cli(&options.media_transport)
        );
        Some(command)
    };
    let command_lowerings = activation_command
        .as_ref()
        .map(|command| json!([command]))
        .unwrap_or_else(|| json!([]));
    let current_transport = if options.idunn_rudp_health.is_some() {
        "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store"
    } else {
        "daemon-owned-cultcache-telemetry-store + missing-rudp-health-publication"
    };
    let transport_state = if options.idunn_rudp_health.is_some() {
        "rudp-health-and-provider-store-live"
    } else {
        "cultcache-provider-store-only"
    };
    let hid_controller_endpoint = options.hid_controller_rudp_bind.map(|bind| {
        options
            .hid_controller_rudp_advertise
            .clone()
            .unwrap_or_else(|| bind.to_string())
    });
    let mut provider_routes = vec![json!({
        "transport": "cultcache-store",
        "address": options.store_path.display().to_string()
    })];
    if let Some(bind) = options.command_rudp_bind {
        let address = options
            .command_rudp_advertise
            .clone()
            .unwrap_or_else(|| bind.to_string());
        provider_routes.push(json!({
            "id": "muninn.provider.command",
            "role": "muninn provider command ingress",
            "uri": format!("cultmesh://{address}/muninn/{}/commands", options.host_id),
            "transport": CULTNET_RUDP_PROTOCOL_ID,
            "address": address,
            "connectionId": MUNINN_COMMAND_RUDP_CONNECTION_ID,
            "channel": "schema",
            "schema": "gamecult.eve.command.v1",
            "tags": ["muninn", "command", "provider-owned"]
        }));
    }
    let hid_controller_devices_snake = live_move_sources
        .iter()
        .map(|source| {
            json!({
                "device_id": source.move_id,
                "device_kind": hid_controller_kind_from_source(source),
                "source_path": source.hidraw_path
            })
        })
        .collect::<Vec<_>>();
    let hid_controller_devices_camel = live_move_sources
        .iter()
        .map(|source| {
            json!({
                "deviceId": source.move_id,
                "deviceKind": hid_controller_kind_from_source(source),
                "sourcePath": source.hidraw_path
            })
        })
        .collect::<Vec<_>>();
    let input_stream_id = format!("muninn:{}:hid-controller-state", options.host_id);
    let transport_input_streams = hid_controller_endpoint
        .as_ref()
        .map(|endpoint| {
            let mut streams = vec![json!({
                "stream_id": input_stream_id,
                "schema": "muninn.hid_controller_state.v1",
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "address": endpoint,
                "connection_id": MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
                "channel_id": "latest",
                "producer": "Muninn IO daemon",
                "devices": hid_controller_devices_snake
            })];
            if let Some(stream_id) = options.move_evidence_stream_id.as_ref() {
                streams.push(json!({
                    "stream_id": stream_id,
                    "schema": "mimir.muninn_move_evidence_stream_frame.v1",
                    "transport": CULTNET_RUDP_PROTOCOL_ID,
                    "address": endpoint,
                    "connection_id": MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
                    "channel_id": "move-evidence",
                    "producer": "Muninn Move evidence runtime"
                }));
            }
            json!(streams)
        })
        .unwrap_or_else(|| json!([]));
    let provider_input_streams = hid_controller_endpoint
        .as_ref()
        .map(|endpoint| {
            let mut streams = vec![json!({
                "streamId": input_stream_id,
                "schema": "muninn.hid_controller_state.v1",
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "address": endpoint,
                "connectionId": MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
                "channel": "latest",
                "devices": hid_controller_devices_camel
            })];
            if let Some(stream_id) = options.move_evidence_stream_id.as_ref() {
                streams.push(json!({
                    "streamId": stream_id,
                    "schema": "mimir.muninn_move_evidence_stream_frame.v1",
                    "transport": CULTNET_RUDP_PROTOCOL_ID,
                    "address": endpoint,
                    "connectionId": MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
                    "channel": "move-evidence"
                }));
            }
            json!(streams)
        })
        .unwrap_or_else(|| json!([]));
    let mut provider_endpoints = vec![json!({
        "transport": "cultcache-store",
        "address": options.store_path.display().to_string()
    })];
    if let Some(endpoint) = hid_controller_endpoint.as_ref() {
        provider_endpoints.push(json!({
            "transport": CULTNET_RUDP_PROTOCOL_ID,
            "role": "muninn.hid_controller_state",
            "schema": "muninn.hid_controller_state.v1",
            "address": endpoint,
            "connectionId": MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
            "channel": "latest"
        }));
        if options.move_evidence_stream_id.is_some() {
            provider_endpoints.push(json!({
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "role": "muninn.move_evidence_stream",
                "schema": "mimir.muninn_move_evidence_stream_frame.v1",
                "address": endpoint,
                "connectionId": MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
                "channel": "move-evidence"
            }));
        }
    }
    let media_profile = muninn_rudp_media_profile_for_options(options);
    let command_boundary = MuninnCommandBoundaryCompatRecord {
        value: json!({
            "schema": "muninn.command_boundary.v1",
            "boundary_id": command_boundary_key,
            "daemon_id": daemon_id,
            "provider_id": provider_id,
            "service_id": provider_id,
            "host_id": options.host_id,
            "owner": "Muninn telemetry runtime",
            "updated_at": updated_at,
            "state_store": store_path,
            "log_root": log_root,
            "lifecycle_authority": "idunn-supervisor-command",
            "health_publication": options.idunn_rudp_health.as_ref().map(|idunn| json!({
                "contract": idunn.health_contract,
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "publication_source": "daemon-published",
                "endpoint": idunn.endpoint.to_string(),
                "state_owner": "Muninn serve process"
            })).unwrap_or_else(|| json!({
                "contract": serde_json::Value::Null,
                "transport": "unconfigured",
                "publication_source": "missing-daemon-publication",
                "state_owner": "Muninn local store"
            })),
            "commands": [
                {
                    "command": "muninn.capture_stream_command",
                    "ingress": "cultmesh-document",
                    "schema": "muninn.capture_stream_command.v1",
                    "invocation": activation_command,
                    "state": if activation_command.is_some() { "configured" } else { "missing-media-target" },
                    "owns": [
                        "explicit capture stream activation",
                        "ffmpeg/loopback process launch",
                        "active muninn.capture_stream receipts"
                    ]
                },
                {
                    "command": "muninn.move_light_command",
                    "ingress": "cultmesh-document",
                    "schema": "muninn.move_light_command.v1",
                    "owns": [
                        "PS Move light pulses for attached local controllers"
                    ]
                },
                {
                    "command": "muninn.set-move-hue-program",
                    "ingress": "cultmesh-document",
                    "schema": MUNINN_MOVE_HUE_PROGRAM_SCHEMA,
                    "record_key": move_hue_program_key(&options.host_id),
                    "owns": [
                        "live Move hue mode",
                        "cycle duration",
                        "deterministic update order",
                        "hold timestamp"
                    ]
                }
            ],
            "forbidden_authority": "Task Scheduler, HTTP probes, OBS consumers, and one-shot health commands do not own Muninn telemetry truth or stream activation state.",
        }),
    };
    let transport_profile = MuninnTransportProfileCompatRecord {
        value: json!({
            "schema": "muninn.transport_profile.v1",
            "profile_id": transport_profile_key,
            "daemon_id": daemon_id,
            "provider_id": provider_id,
            "host_id": options.host_id,
            "target_transport": CULTNET_RUDP_PROTOCOL_ID,
            "current_transport": current_transport,
            "state": transport_state,
            "health_contract": options.idunn_rudp_health.as_ref().map(|idunn| idunn.health_contract.clone()),
            "provider_advertisement_schema": "gamecult.eve.provider_advertisement.v1",
            "command_boundary_schema": "muninn.command_boundary.v1",
            "telemetry_surface_schema": "muninn.telemetry_surface.v1",
            "media_profile": {
                "profile_id": media_profile.profile_id,
                "owner": "Muninn capture runtime",
                "strategy": "hardware-codec-owned-prediction-over-fixed-budget-rudp",
                "visual_prediction_owner": "NVENC H.264 inter-frame prediction",
                "transport_owner": "CultNet RUDP media channel",
                "codec": media_profile.video_codec,
                "encoder": media_profile.video_encoder,
                "encoder_preset": media_profile.video_preset,
                "encoder_tune": media_profile.video_tune,
                "video_bitrate_kbps": media_profile.video_bitrate_kbps,
                "video_bitrate": muninn_rudp_video_bitrate_arg(&media_profile),
                "video_maxrate": muninn_rudp_video_bitrate_arg(&media_profile),
                "video_bufsize": muninn_rudp_video_vbv_buffer_arg(options, &media_profile),
                "video_low_delay_key_frame_scale": MUNINN_RUDP_MEDIA_LOW_DELAY_KEY_FRAME_SCALE,
                "media_packet_bytes": media_profile.media_packet_bytes,
                "max_fragment_bytes": media_profile.max_fragment_bytes,
                "video_b_frames": media_profile.video_b_frames,
                "video_gop_frames": muninn_rudp_video_gop_frames(options),
                "video_rate_control": "cbr",
                "video_rc_lookahead": media_profile.video_rc_lookahead,
                "sender_queue_deadline_ms": media_profile.sender_queue_deadline_ms,
                "sender_resend_delay_ms": media_profile.sender_resend_delay_ms,
                "sender_delivery_deadline_ms": media_profile.sender_delivery_deadline_ms,
                "receiver_assembly_deadline_ms": media_profile.receiver_assembly_deadline_ms,
                "receiver_gap_wait_ms": media_profile.receiver_gap_wait_ms,
                "late_media_policy": "drop expired queued media; do not repair frames outside the latency budget",
                "recovery": "fixed quarter-second IDR budget with receiver feedback pressure telemetry"
            },
            "input_streams": transport_input_streams,
            "debug_lowerings": [],
            "command_lowerings": command_lowerings,
            "cut_line": "Muninn's telemetry store owns provider advertisement, command boundary, transport profile, telemetry surface, and daemon health state. Local CLI activation is a command lowering; health is published by the serve process over CultNet/RUDP.",
            "updated_at": updated_at,
        }),
    };
    let provider_advertisement = EveProviderAdvertisementRecord {
        value: json!({
            "schema": "gamecult.eve.provider_advertisement.v1",
            "providerId": provider_id,
            "daemonId": daemon_id,
            "title": muninn_provider_title(options),
            "description": muninn_provider_description(options),
            "canonicalService": "asgard.muninn",
            "locatedService": format!("asgard.{}.muninn", options.host_id),
            "verseId": format!("{}.local", options.host_id),
            "cultMeshAddress": format!("asgard.{}.muninn/telemetry", options.host_id),
            "status": "active",
            "mode": "daemon-live",
            "updatedAt": updated_at,
            "stateStore": store_path,
            "surfaceId": options.surface_id,
            "streamId": stream_id,
            "surfaceState": state,
            "availableSources": available_sources,
            "activeStreams": active_streams,
            "capabilities": muninn_capabilities(options),
            "endpoints": provider_endpoints,
            "inputStreams": provider_input_streams,
            "routes": provider_routes,
            "surfaces": [{
                "surfaceId": format!("{provider_id}.move-hue.surface"),
                "surfaceKind": "gamecult.eve.surface_state.v1",
                "recordRef": move_hue_surface_key(&options.host_id),
                "title": "Move Hue Program"
            }],
            "commandSurface": {
                "commandBoundaryId": command_boundary_key,
                "transportProfileId": transport_profile_key
            }
        }),
    };
    node.put(&provider_id, &provider_advertisement)?;
    node.put(&command_boundary_key, &command_boundary)?;
    node.put(&transport_profile_key, &transport_profile)?;
    Ok(())
}

fn start_odin_provider_lease_worker(options: &Options) {
    if resolve_odin_cultmesh_uri(options).is_none() {
        return;
    }

    let options = options.clone();
    thread::spawn(move || {
        let mut has_published = false;
        loop {
            let result = open_node(&options, "muninn-odin-provider-lease")
                .and_then(|node| publish_odin_provider_lease(&node, &options));
            match result {
                Ok(()) => {
                    has_published = true;
                    thread::sleep(Duration::from_secs(
                        MUNINN_ODIN_PROVIDER_LEASE_REFRESH_SECONDS,
                    ));
                }
                Err(error) => {
                    eprintln!("Muninn could not renew its Odin provider lease: {error:#}");
                    thread::sleep(if has_published {
                        Duration::from_secs(MUNINN_ODIN_PROVIDER_LEASE_REFRESH_SECONDS)
                    } else {
                        Duration::from_secs(1)
                    });
                }
            }
        }
    });
}

fn publish_odin_provider_lease(node: &cultmesh_rs::CultMeshNode, options: &Options) -> Result<()> {
    let Some(target) = resolve_odin_cultmesh_uri(options) else {
        return Ok(());
    };
    let provider_id = muninn_provider_id(options);
    let provider = node.get_required::<EveProviderAdvertisementRecord>(&provider_id)?;
    node.publish_document_to_rudp_catalog(
        &provider_id,
        &provider,
        CultMeshRudpDocumentPublishOptions {
            target,
            runtime_id: muninn_daemon_id(options),
            source_role: Some("muninn.telemetry-provider".to_string()),
            tags: vec![
                "provider-lease".to_string(),
                "odin-verse-discovery".to_string(),
            ],
            ..CultMeshRudpDocumentPublishOptions::default()
        },
    )
}

fn muninn_provider_id(options: &Options) -> String {
    format!("muninn.telemetry.{}", options.host_id)
}

fn muninn_daemon_id(options: &Options) -> String {
    if let Some(idunn) = options.idunn_rudp_health.as_ref() {
        return idunn.daemon_id.clone();
    }
    match options.host_id.as_str() {
        "raven" => "muninn".to_string(),
        "starfire" => "starfire-muninn".to_string(),
        "nightwing" => "nightwing-muninn".to_string(),
        _ => format!("muninn-{}", options.host_id),
    }
}

fn muninn_provider_title(options: &Options) -> String {
    format!("Muninn {} Telemetry", title_case_host(&options.host_id))
}

fn muninn_provider_description(options: &Options) -> String {
    let mut detail = format!(
        "Muninn telemetry runtime on {} publishing local capture affordances and typed telemetry state.",
        title_case_host(&options.host_id)
    );
    if options.quest_adb {
        detail.push_str(" Quest USB access is enabled on this body.");
    }
    if !options.move_state_sources.is_empty() {
        detail.push_str(" Move HID/controller evidence is enabled on this body.");
    }
    detail
}

fn title_case_host(host_id: &str) -> String {
    let mut chars = host_id.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".to_string(),
    }
}

fn muninn_capabilities(options: &Options) -> Vec<String> {
    let mut capabilities = vec![
        "screen.capture.ddagrab".to_string(),
        "audio.loopback.wasapi".to_string(),
        "muninn.telemetry_surface".to_string(),
        "muninn.capture_stream".to_string(),
        "muninn.command_boundary".to_string(),
        "muninn.transport_profile".to_string(),
    ];
    if options.quest_adb {
        capabilities.push("quest.usb.access".to_string());
    }
    if !options.move_state_sources.is_empty() {
        capabilities.push("psmove.controller.state".to_string());
        capabilities.push("muninn.move_evidence_stream".to_string());
    }
    capabilities
}

fn available_sources(options: &Options) -> Vec<String> {
    let mut sources = vec![
        format!("screen:ddagrab:output_idx={}", options.ddagrab_output_index),
        format!(
            "audio-loopback:wasapi:{}:{}ch@{}",
            options.audio_device, options.audio_channels, options.audio_sample_rate
        ),
        "sensor:microphone:enumeration-pending".to_string(),
        "sensor:camera:enumeration-pending".to_string(),
    ];
    if options.quest_adb {
        sources.push(format!(
            "sensor:quest:adb:{}",
            options
                .quest_serial
                .as_deref()
                .unwrap_or("any-authorized-quest")
        ));
    } else {
        sources.push("sensor:quest:adb:activation-required".to_string());
    }
    sources
}

fn publish_stream(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    plan: &MuxPlan,
    state: &str,
    supervisor_pid: u32,
    mux_pid: Option<u32>,
    restart_count: u32,
    detail: &str,
) -> Result<()> {
    let record = MuninnCaptureStreamRecord {
        stream_id: options.stream_id.clone(),
        host_id: options.host_id.clone(),
        state: state.to_string(),
        video_source: if options.capture_video {
            format!(
                "ddagrab:output_idx={}:{}x{}@{}",
                options.ddagrab_output_index, options.width, options.height, options.framerate
            )
        } else {
            "disabled".to_string()
        },
        audio_source: if options.capture_audio {
            format!(
                "wasapi-loopback:{}:{}ch@{}",
                options.audio_device, options.audio_channels, options.audio_sample_rate
            )
        } else {
            "disabled".to_string()
        },
        transport: media_transport_id(&options.media_transport).to_string(),
        targets: plan.targets.clone(),
        command_witness: plan.command_file.display().to_string(),
        supervisor_pid: Some(supervisor_pid),
        mux_pid,
        restart_count,
        detail: detail.to_string(),
        updated_at: timestamp()?,
    };
    node.put("latest-stream", &record)?;
    node.put(&record.stream_id, &record)?;
    Ok(())
}

fn publish_obs_catalog_idle(node: &mut cultmesh_rs::CultMeshNode, options: &Options) -> Result<()> {
    let mut stream_ids = vec![options.stream_id.clone()];
    let mut labels = vec![format!("{} screen and loopback A/V", options.host_id)];
    let mut urls = vec![String::new()];
    let mut states = vec!["activation-required".to_string()];
    for source in available_sources(options) {
        stream_ids.push(format!("{}:{}", options.surface_id, source));
        labels.push(source);
        urls.push(String::new());
        states.push("affordance".to_string());
    }
    publish_obs_catalog(node, options, stream_ids, labels, urls, states)
}

fn publish_obs_catalog(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    stream_ids: Vec<String>,
    labels: Vec<String>,
    urls: Vec<String>,
    states: Vec<String>,
) -> Result<()> {
    let video_sources = video_source_catalog(options);
    let audio_sources = audio_source_catalog(options);
    let record = MuninnObsStreamCatalogRecord {
        catalog_id: "muninn.obs.streams".to_string(),
        host_id: options.host_id.clone(),
        stream_ids,
        labels,
        urls,
        states,
        updated_at: timestamp()?,
        command_rudp_target: String::new(),
        media_target_host: options.target_host.clone(),
        media_port: options.port,
        media_packet_bytes: options.media_packet_bytes as u32,
        rudp_video_bitrate_kbps: options.rudp_video_bitrate_kbps,
        rudp_latency_budget_ms: options.rudp_latency_budget_ms,
        video_source_ids: video_sources
            .iter()
            .map(|source| source.id.clone())
            .collect(),
        video_source_labels: video_sources
            .iter()
            .map(|source| source.label.clone())
            .collect(),
        audio_source_ids: audio_sources
            .iter()
            .map(|source| source.id.clone())
            .collect(),
        audio_source_labels: audio_sources
            .iter()
            .map(|source| source.label.clone())
            .collect(),
    };
    node.put("obs", &record)?;
    if let Some(target) = resolve_odin_cultmesh_uri(options)
        && let Err(error) = node.publish_document_to_rudp_catalog(
            "obs",
            &record,
            CultMeshRudpDocumentPublishOptions {
                target,
                runtime_id: muninn_daemon_id(options),
                source_role: Some("muninn.obs-catalog-provider".to_string()),
                tags: vec![
                    "obs-discovery".to_string(),
                    "odin-verse-discovery".to_string(),
                ],
                ..CultMeshRudpDocumentPublishOptions::default()
            },
        )
    {
        eprintln!("Muninn could not publish OBS catalog to Odin: {error:#}");
    }
    Ok(())
}

fn create_move_evidence_stream(options: &Options) -> Result<Option<ActiveMoveEvidenceStream>> {
    if options.move_state_sources.is_empty()
        && options.move_marker_camera_sources.is_empty()
        && options.move_evidence_stream_id.is_none()
    {
        return Ok(None);
    }

    let stream_id = options
        .move_evidence_stream_id
        .clone()
        .unwrap_or_else(|| format!("muninn:{}:move-evidence", options.host_id));
    let producer_peer_id = format!("muninn:{}", options.host_id);
    let clock_domain_id = format!("{}:clock", producer_peer_id);
    let descriptor = CultMeshStreamDescriptor::new(
        stream_id.clone(),
        options.move_evidence_verse_id.clone(),
        producer_peer_id.clone(),
        CultMeshStreamKind::Bytes,
        CultMeshStreamClock::new(clock_domain_id)?
            .source_id(stream_id.clone())?
            .confidence(1.0)
            .evidence_kind("muninn-move-evidence")?,
        vec![
            CultMeshStreamBodyTransport::SharedMemory,
            CultMeshStreamBodyTransport::CultCachePage,
        ],
    )?
    .label("Muninn Move evidence")?
    .max_in_flight_frames(options.move_evidence_ring_slots as u32)
    .metadata_schema_id("mimir.muninn_move_evidence_stream_frame.v1")?;

    let mut catalog = CultMesh::create_stream_catalog();
    catalog.declare(descriptor);
    catalog.create_shared_memory_ring(
        &stream_id,
        options.move_evidence_ring_slots,
        options.move_evidence_slot_bytes,
    )?;

    Ok(Some(ActiveMoveEvidenceStream {
        catalog,
        stream_id,
        producer_peer_id,
        frame_counter: 0,
        snapshot_path: options.move_evidence_snapshot_path.clone(),
        rudp_sender: None,
        counters: Arc::new(MoveEvidenceTransportCounters::default()),
        local_ring_enabled: options.hid_controller_rudp_bind.is_none(),
    }))
}

fn publish_move_evidence_stream_frame(
    stream: &mut ActiveMoveEvidenceStream,
    marker_candidates: &[MuninnMoveMarkerCandidateRecord],
    controller_states: &[MuninnMoveControllerStateRecord],
) -> Result<Option<cultmesh_rs::CultMeshStreamFrameHandle>> {
    if marker_candidates.is_empty() && controller_states.is_empty() {
        return Ok(None);
    }

    let published_at_ns = timestamp_ns()?;
    let frame_id = format!("{}:{}", stream.stream_id, stream.frame_counter);
    stream.frame_counter = stream.frame_counter.saturating_add(1);
    stream.counters.produced_frames.fetch_add(1, Ordering::Relaxed);
    let frame = MuninnMoveEvidenceStreamFrame(
        &frame_id,
        &stream.producer_peer_id,
        published_at_ns,
        marker_candidates,
        controller_states,
    );
    let payload =
        rmp_serde::to_vec(&frame).context("encoding Muninn Move evidence stream frame")?;
    if let Some(sender) = stream.rudp_sender.as_ref() {
        if let Ok(mut latest) = sender.lock() { *latest = Some(payload.clone()); }
        stream.counters.remote_handoffs.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(path) = stream.snapshot_path.as_deref() {
        write_move_evidence_snapshot(
            path,
            &stream.stream_id,
            &frame_id,
            &stream.producer_peer_id,
            published_at_ns,
            &payload,
        )?;
    }
    let handle = if stream.local_ring_enabled {
        let ring: &mut CultMeshSharedMemoryFrameRing = stream.catalog
            .ring_mut(&stream.stream_id)
            .ok_or_else(|| anyhow!("missing Muninn Move evidence ring"))?;
        ring.try_publish_copy(&payload, published_at_ns, 0)?
    } else {
        None
    };
    if let Some(handle) = handle {
        stream.counters.local_ring_admissions.fetch_add(1, Ordering::Relaxed);
        stream.catalog.publish_frame(handle.clone())?;
        Ok(Some(handle))
    } else {
        Ok(None)
    }
}

fn write_move_evidence_snapshot(
    path: &Path,
    stream_id: &str,
    frame_id: &str,
    producer_peer_id: &str,
    published_at_ns: i64,
    payload: &[u8],
) -> Result<()> {
    if payload.is_empty() {
        return Err(anyhow!("cannot write empty Move evidence snapshot"));
    }

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "creating Move evidence snapshot directory {}",
                parent.display()
            )
        })?;
    }

    let captured_at_ns = u64::try_from(timestamp_ns()?)
        .context("Move evidence snapshot capture timestamp does not fit u64")?;
    let snapshot_id = format!("{frame_id}:snapshot");
    let snapshot = MimirMoveProofEvidenceFrameSnapshot(
        &snapshot_id,
        stream_id,
        frame_id,
        producer_peer_id,
        published_at_ns,
        captured_at_ns,
        payload,
    );
    let bytes =
        rmp_serde::to_vec(&snapshot).context("encoding Mimir Move proof evidence snapshot")?;
    fs::write(path, bytes)
        .with_context(|| format!("writing Move proof evidence snapshot {}", path.display()))
}

fn extract_move_marker_candidates_from_luma_frame(
    source: &MoveMarkerFrameSource,
    y8_frame: &[u8],
    observed_at: String,
) -> Result<Vec<MuninnMoveMarkerCandidateRecord>> {
    let candidates = muninn_move_tracker::extract_luma_candidates(y8_frame, source.tracker_config)
        .ok_or_else(|| {
            anyhow!(
                "invalid Move marker Y8 frame for camera {}: expected width={} height={} stride={}",
                source.camera_id,
                source.tracker_config.width,
                source.tracker_config.height,
                source.tracker_config.stride_bytes
            )
        })?;
    Ok(candidates
        .into_iter()
        .map(|candidate| {
            build_move_marker_candidate_record(
                &source.stream_id,
                &source.host_id,
                &source.camera_id,
                candidate,
                observed_at.clone(),
            )
        })
        .collect())
}

fn build_move_marker_candidate_record(
    stream_id: &str,
    host_id: &str,
    camera_id: &str,
    candidate: muninn_move_tracker::MoveMarkerCandidate,
    observed_at: String,
) -> MuninnMoveMarkerCandidateRecord {
    MuninnMoveMarkerCandidateRecord {
        stream_id: stream_id.to_string(),
        host_id: host_id.to_string(),
        camera_id: camera_id.to_string(),
        frame_sequence: candidate.frame_sequence,
        source_id_hash: candidate.source_id_hash,
        tile_x: candidate.tile_x,
        tile_y: candidate.tile_y,
        center_x_px: candidate.center_x_px,
        center_y_px: candidate.center_y_px,
        radius_px: candidate.radius_px,
        area_px: candidate.area_px,
        mean_luma: candidate.mean_luma,
        peak_luma: candidate.peak_luma,
        score: candidate.score,
        observed_at,
        move_id: String::new(),
    }
}

trait MoveLightWriter {
    fn write_report(&mut self, hidraw_path: &str, report: &[u8]) -> Result<()>;
}

struct HidMoveLightWriter;

impl MoveLightWriter for HidMoveLightWriter {
    #[cfg(not(windows))]
    fn write_report(&mut self, hidraw_path: &str, report: &[u8]) -> Result<()> {
        let mut device = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(hidraw_path)
            .with_context(|| format!("opening PS Move HID path {hidraw_path}"))?;
        device
            .write_all(report)
            .with_context(|| format!("writing PS Move HID report to {hidraw_path}"))
    }

    #[cfg(windows)]
    fn write_report(&mut self, hidraw_path: &str, report: &[u8]) -> Result<()> {
        write_windows_hid_report(hidraw_path, report)
    }
}

trait MoveControllerStateReader {
    fn read_report(&mut self, hidraw_path: &str) -> Result<Option<Vec<u8>>>;
    fn read_joystick_events(&mut self, joystick_path: &str) -> Result<Vec<JoystickEvent>>;
}

trait MoveMarkerCameraFrameReader {
    fn read_luma_frame(
        &mut self,
        source: &MoveMarkerCameraSource,
        frame_source: &MoveMarkerFrameSource,
    ) -> Result<Option<Vec<u8>>>;
}

#[derive(Default)]
struct PlatformMoveMarkerCameraFrameReader;

impl MoveMarkerCameraFrameReader for PlatformMoveMarkerCameraFrameReader {
    #[cfg(unix)]
    fn read_luma_frame(
        &mut self,
        source: &MoveMarkerCameraSource,
        frame_source: &MoveMarkerFrameSource,
    ) -> Result<Option<Vec<u8>>> {
        read_v4l2_yuyv_luma_frame(source, frame_source)
    }

    #[cfg(not(unix))]
    fn read_luma_frame(
        &mut self,
        source: &MoveMarkerCameraSource,
        _frame_source: &MoveMarkerFrameSource,
    ) -> Result<Option<Vec<u8>>> {
        Err(anyhow!(
            "Move marker camera source {} at {} requires a Unix V4L2 runtime",
            source.camera_id,
            source.device_path.display()
        ))
    }
}

#[cfg(unix)]
const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
#[cfg(unix)]
const V4L2_FIELD_NONE: u32 = 1;
#[cfg(unix)]
const V4L2_MEMORY_MMAP: u32 = 1;
#[cfg(unix)]
const VIDIOC_G_FMT: libc::c_ulong = 0xC0D05604;
#[cfg(unix)]
const VIDIOC_S_FMT: libc::c_ulong = 0xC0D05605;
#[cfg(unix)]
const VIDIOC_REQBUFS: libc::c_ulong = 0xC0145608;
#[cfg(unix)]
const VIDIOC_QUERYBUF: libc::c_ulong = 0xC0585609;
#[cfg(unix)]
const VIDIOC_QBUF: libc::c_ulong = 0xC058560F;
#[cfg(unix)]
const VIDIOC_DQBUF: libc::c_ulong = 0xC0585611;
#[cfg(unix)]
const VIDIOC_STREAMON: libc::c_ulong = 0x40045612;
#[cfg(unix)]
const VIDIOC_STREAMOFF: libc::c_ulong = 0x40045613;
#[cfg(unix)]
const VIDIOC_G_PARM: libc::c_ulong = 0xC0CC5615;
#[cfg(unix)]
const VIDIOC_S_PARM: libc::c_ulong = 0xC0CC5616;

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    ycbcr_enc: u32,
    quantization: u32,
    xfer_func: u32,
}

#[cfg(unix)]
#[repr(C)]
union V4l2FormatUnion {
    pix: V4l2PixFormat,
    raw: [u8; 200],
    align: u64,
}

#[cfg(unix)]
#[repr(C)]
struct V4l2Format {
    type_: u32,
    fmt: V4l2FormatUnion,
}

#[cfg(unix)]
#[repr(C)]
struct V4l2RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2Fract {
    numerator: u32,
    denominator: u32,
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2CaptureParm {
    capability: u32,
    capturemode: u32,
    timeperframe: V4l2Fract,
    extendedmode: u32,
    readbuffers: u32,
    reserved: [u32; 4],
}

#[cfg(unix)]
#[repr(C)]
union V4l2StreamParmUnion {
    capture: V4l2CaptureParm,
    raw: [u8; 200],
}

#[cfg(unix)]
#[repr(C)]
struct V4l2StreamParm {
    type_: u32,
    parm: V4l2StreamParmUnion,
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct V4l2Timecode {
    type_: u32,
    flags: u32,
    frames: u8,
    seconds: u8,
    minutes: u8,
    hours: u8,
    userbits: [u8; 4],
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
union V4l2BufferUnion {
    offset: u32,
    userptr: libc::c_ulong,
    planes: *mut libc::c_void,
    fd: i32,
}

#[cfg(unix)]
#[repr(C)]
struct V4l2Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: V4l2Timecode,
    sequence: u32,
    memory: u32,
    m: V4l2BufferUnion,
    length: u32,
    reserved2: u32,
    request_fd: i32,
}

#[cfg(unix)]
struct V4l2MappedBuffer {
    ptr: *mut libc::c_void,
    length: usize,
}

#[cfg(unix)]
fn read_v4l2_yuyv_luma_frame(
    source: &MoveMarkerCameraSource,
    frame_source: &MoveMarkerFrameSource,
) -> Result<Option<Vec<u8>>> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let config = frame_source.tracker_config;
    if config.stride_bytes != config.width {
        return Err(anyhow!(
            "Move marker V4L2 reader emits compact Y8 frames, so stride {} must equal width {}",
            config.stride_bytes,
            config.width
        ));
    }

    let device = CString::new(source.device_path.as_os_str().as_bytes())
        .with_context(|| format!("invalid V4L2 device path {}", source.device_path.display()))?;
    let fd = unsafe { libc::open(device.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("opening V4L2 camera {}", source.device_path.display()));
    }

    let result = read_v4l2_yuyv_luma_frame_from_fd(fd, source, frame_source);
    let close_result = unsafe { libc::close(fd) };
    if close_result != 0 && result.is_ok() {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("closing V4L2 camera {}", source.device_path.display()));
    }
    result
}

#[cfg(unix)]
fn read_v4l2_yuyv_luma_frame_from_fd(
    fd: libc::c_int,
    source: &MoveMarkerCameraSource,
    frame_source: &MoveMarkerFrameSource,
) -> Result<Option<Vec<u8>>> {
    let config = frame_source.tracker_config;
    let yuyv = fourcc("YUYV");
    let mut stream_on = false;
    let mut maps = Vec::new();
    let read_result = (|| -> Result<Option<Vec<u8>>> {
        set_v4l2_frame_interval(fd, frame_source.fps)?;

        let mut fmt = zeroed_v4l2_format();
        fmt.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        ioctl_mut(fd, VIDIOC_G_FMT, &mut fmt).with_context(|| {
            format!("querying V4L2 format for {}", source.device_path.display())
        })?;
        fmt.fmt.pix.width = config.width;
        fmt.fmt.pix.height = config.height;
        fmt.fmt.pix.pixelformat = yuyv;
        fmt.fmt.pix.field = V4L2_FIELD_NONE;
        ioctl_mut(fd, VIDIOC_S_FMT, &mut fmt).with_context(|| {
            format!(
                "setting V4L2 YUYV format for {}",
                source.device_path.display()
            )
        })?;
        let actual = unsafe { fmt.fmt.pix };
        if actual.width != config.width || actual.height != config.height {
            return Err(anyhow!(
                "V4L2 camera {} returned {}x{}, expected {}x{}",
                source.device_path.display(),
                actual.width,
                actual.height,
                config.width,
                config.height
            ));
        }
        if actual.pixelformat != yuyv {
            return Err(anyhow!(
                "V4L2 camera {} did not accept YUYV format",
                source.device_path.display()
            ));
        }

        let mut req = V4l2RequestBuffers {
            count: 2,
            type_: V4L2_BUF_TYPE_VIDEO_CAPTURE,
            memory: V4L2_MEMORY_MMAP,
            capabilities: 0,
            flags: 0,
            reserved: [0; 3],
        };
        ioctl_mut(fd, VIDIOC_REQBUFS, &mut req).with_context(|| {
            format!(
                "requesting V4L2 buffers for {}",
                source.device_path.display()
            )
        })?;
        if req.count == 0 {
            return Err(anyhow!(
                "V4L2 camera {} returned no mmap buffers",
                source.device_path.display()
            ));
        }

        for index in 0..req.count {
            let mut buf = zeroed_v4l2_buffer();
            buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            buf.memory = V4L2_MEMORY_MMAP;
            buf.index = index;
            ioctl_mut(fd, VIDIOC_QUERYBUF, &mut buf).with_context(|| {
                format!(
                    "querying V4L2 buffer {index} for {}",
                    source.device_path.display()
                )
            })?;
            let length = usize::try_from(buf.length).context("V4L2 buffer length overflow")?;
            let offset = unsafe { buf.m.offset };
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    length,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd,
                    offset as libc::off_t,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(std::io::Error::last_os_error()).with_context(|| {
                    format!(
                        "mapping V4L2 buffer {index} for {}",
                        source.device_path.display()
                    )
                });
            }
            maps.push(V4l2MappedBuffer { ptr, length });
            ioctl_mut(fd, VIDIOC_QBUF, &mut buf).with_context(|| {
                format!(
                    "queueing V4L2 buffer {index} for {}",
                    source.device_path.display()
                )
            })?;
        }

        let mut capture_type = V4L2_BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
        ioctl_mut(fd, VIDIOC_STREAMON, &mut capture_type).with_context(|| {
            format!("starting V4L2 stream for {}", source.device_path.display())
        })?;
        stream_on = true;

        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll_fd, 1, 250) };
        if ready < 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("polling V4L2 camera {}", source.device_path.display()));
        }
        if ready == 0 {
            return Ok(None);
        }

        let mut buf = zeroed_v4l2_buffer();
        buf.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        ioctl_mut(fd, VIDIOC_DQBUF, &mut buf)
            .with_context(|| format!("reading V4L2 frame from {}", source.device_path.display()))?;
        let index = usize::try_from(buf.index).context("V4L2 buffer index overflow")?;
        let mapping = maps.get(index).ok_or_else(|| {
            anyhow!(
                "V4L2 camera {} returned unknown buffer index {}",
                source.device_path.display(),
                index
            )
        })?;
        let bytes_used = usize::try_from(buf.bytesused).context("V4L2 bytesused overflow")?;
        let raw_len = bytes_used.min(mapping.length);
        let raw = unsafe { std::slice::from_raw_parts(mapping.ptr as *const u8, raw_len) };
        Ok(Some(yuyv_to_compact_luma(
            raw,
            config.width,
            config.height,
            actual.bytesperline,
        )?))
    })();

    if stream_on {
        let mut capture_type = V4L2_BUF_TYPE_VIDEO_CAPTURE as libc::c_int;
        let _ = ioctl_mut(fd, VIDIOC_STREAMOFF, &mut capture_type);
    }
    for mapping in maps {
        unsafe {
            libc::munmap(mapping.ptr, mapping.length);
        }
    }
    read_result
}

#[cfg(unix)]
fn set_v4l2_frame_interval(fd: libc::c_int, fps: u32) -> Result<()> {
    let mut parm = zeroed_v4l2_stream_parm();
    parm.type_ = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    let _ = ioctl_mut(fd, VIDIOC_G_PARM, &mut parm);
    parm.parm.capture.timeperframe.numerator = 1;
    parm.parm.capture.timeperframe.denominator = fps.max(1);
    ioctl_mut(fd, VIDIOC_S_PARM, &mut parm)
}

#[cfg(unix)]
fn yuyv_to_compact_luma(raw: &[u8], width: u32, height: u32, stride_bytes: u32) -> Result<Vec<u8>> {
    let width = usize::try_from(width).context("YUYV width overflow")?;
    let height = usize::try_from(height).context("YUYV height overflow")?;
    let stride = usize::try_from(stride_bytes).context("YUYV stride overflow")?;
    let required = height
        .checked_sub(1)
        .and_then(|last_row| last_row.checked_mul(stride))
        .and_then(|last_row| last_row.checked_add(width.saturating_mul(2)))
        .unwrap_or(0);
    if raw.len() < required {
        return Err(anyhow!(
            "YUYV frame is too short: {} bytes, expected at least {}",
            raw.len(),
            required
        ));
    }
    let mut luma = Vec::with_capacity(width.saturating_mul(height));
    for row in 0..height {
        let start = row * stride;
        let row_bytes = &raw[start..start + width * 2];
        luma.extend(row_bytes.iter().step_by(2).copied());
    }
    Ok(luma)
}

#[cfg(unix)]
fn fourcc(value: &str) -> u32 {
    let bytes = value.as_bytes();
    u32::from(bytes[0])
        | (u32::from(bytes[1]) << 8)
        | (u32::from(bytes[2]) << 16)
        | (u32::from(bytes[3]) << 24)
}

#[cfg(unix)]
fn ioctl_mut<T>(fd: libc::c_int, request: libc::c_ulong, value: &mut T) -> Result<()> {
    let result = unsafe { libc::ioctl(fd, request, value as *mut T) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error()).context("V4L2 ioctl failed")
    }
}

#[cfg(unix)]
fn zeroed_v4l2_format() -> V4l2Format {
    unsafe { std::mem::zeroed() }
}

#[cfg(unix)]
fn zeroed_v4l2_stream_parm() -> V4l2StreamParm {
    unsafe { std::mem::zeroed() }
}

#[cfg(unix)]
fn zeroed_v4l2_buffer() -> V4l2Buffer {
    unsafe { std::mem::zeroed() }
}

struct HidMoveControllerStateReader;

impl MoveControllerStateReader for HidMoveControllerStateReader {
    #[cfg(unix)]
    fn read_report(&mut self, hidraw_path: &str) -> Result<Option<Vec<u8>>> {
        let mut device = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(hidraw_path)
            .with_context(|| format!("opening PS Move HID input path {hidraw_path}"))?;
        let mut report = vec![0u8; 64];
        match device.read(&mut report) {
            Ok(0) => Ok(None),
            Ok(count) => {
                report.truncate(count);
                Ok(Some(report))
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error)
                .with_context(|| format!("reading PS Move HID input report from {hidraw_path}")),
        }
    }

    #[cfg(all(not(unix), not(windows)))]
    fn read_report(&mut self, hidraw_path: &str) -> Result<Option<Vec<u8>>> {
        if hidraw_path.trim().is_empty() {
            return Ok(None);
        }
        Err(anyhow!(
            "PS Move controller state HID reads are currently implemented for Unix hidraw paths"
        ))
    }

    #[cfg(windows)]
    fn read_report(&mut self, hidraw_path: &str) -> Result<Option<Vec<u8>>> {
        if hidraw_path.trim().is_empty() {
            return Ok(None);
        }
        if is_windows_ps_move_source(hidraw_path) {
            return windows_ps_move_input_report(hidraw_path);
        }
        Err(anyhow!(
            "Windows PS Move controller state reads require the windows-psmove source path"
        ))
    }

    #[cfg(unix)]
    fn read_joystick_events(&mut self, joystick_path: &str) -> Result<Vec<JoystickEvent>> {
        let mut device = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(joystick_path)
            .with_context(|| format!("opening PS Move joystick input path {joystick_path}"))?;
        let mut events = Vec::new();
        loop {
            let mut report = [0u8; 8];
            match device.read_exact(&mut report) {
                Ok(()) => events.push(JoystickEvent {
                    event_type: report[6],
                    number: report[7],
                    value: i16::from_le_bytes([report[4], report[5]]),
                }),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::UnexpectedEof => break,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("reading PS Move joystick event from {joystick_path}")
                    });
                }
            }
        }
        Ok(events)
    }

    #[cfg(not(unix))]
    fn read_joystick_events(&mut self, joystick_path: &str) -> Result<Vec<JoystickEvent>> {
        if joystick_path.trim().is_empty() {
            return Ok(Vec::new());
        }
        Err(anyhow!(
            "PS Move controller state joystick reads are currently implemented for Unix input paths"
        ))
    }
}

fn publish_move_controller_states(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    active: &mut [ActiveMoveStateSource],
    reader: &mut impl MoveControllerStateReader,
    move_evidence_stream: Option<&mut ActiveMoveEvidenceStream>,
    mut hid_controller_stream: Option<&mut ActiveHidControllerStream>,
) -> Result<()> {
    let mut published_records = Vec::new();
    for state in active {
        let record = if is_joystick_path(&state.source.hidraw_path) {
            let events = match reader.read_joystick_events(&state.source.hidraw_path) {
                Ok(events) => events,
                Err(error) => {
                    eprintln!(
                        "Muninn skipped Move source {} at {}: {error:#}",
                        state.source.move_id, state.source.hidraw_path
                    );
                    continue;
                }
            };
            for event in events {
                match event.event_type & 0x7f {
                    0x01 => {
                        if let Some(button) = state.joystick_buttons.get_mut(event.number as usize)
                        {
                            *button = event.value != 0;
                        }
                    }
                    0x02 => {
                        if let Some(axis) = state.joystick_axes.get_mut(event.number as usize) {
                            *axis = event.value;
                        }
                    }
                    _ => {}
                }
            }
            state.sequence = state.sequence.saturating_add(1);
            let observed_at = timestamp()?;
            let source_timestamp_ns = timestamp_ns()?;
            let hid_record = build_hid_controller_state_record_from_joystick(
                options,
                &state.source,
                state.sequence,
                state.joystick_axes,
                state.joystick_buttons,
                source_timestamp_ns,
                observed_at.clone(),
            );
            put_hid_controller_state_receipt(node, options, &hid_record)?;
            publish_hid_controller_state_to_stream(
                hid_controller_stream.as_deref_mut(),
                &hid_record,
            );
            publish_hid_controller_state_to_odin(node, &hid_record);
            build_move_controller_state_record_from_joystick(
                options,
                &state.source,
                state.sequence,
                state.joystick_axes,
                state.joystick_buttons,
                source_timestamp_ns,
                observed_at,
            )
        } else if is_xinput_source_path(&state.source.hidraw_path) {
            let index = match xinput_index_from_source_path(&state.source.hidraw_path) {
                Some(index) => index,
                None => {
                    eprintln!(
                        "Muninn skipped XInput source {} at {}: invalid xinput source path",
                        state.source.move_id, state.source.hidraw_path
                    );
                    continue;
                }
            };
            let gamepad = match platform_xinput_gamepad(index) {
                Ok(Some(gamepad)) => gamepad,
                Ok(None) => continue,
                Err(error) => {
                    eprintln!(
                        "Muninn skipped XInput source {} at {}: {error:#}",
                        state.source.move_id, state.source.hidraw_path
                    );
                    continue;
                }
            };
            state.sequence = state.sequence.saturating_add(1);
            let observed_at = timestamp()?;
            let hid_record = build_hid_controller_state_record_from_xinput_gamepad(
                options,
                &state.source,
                state.sequence,
                &gamepad,
                timestamp_ns()?,
                observed_at,
            );
            put_hid_controller_state_receipt(node, options, &hid_record)?;
            publish_hid_controller_state_to_stream(
                hid_controller_stream.as_deref_mut(),
                &hid_record,
            );
            publish_hid_controller_state_to_odin(node, &hid_record);
            continue;
        } else {
            #[cfg(windows)]
            if options.hid_controller_rudp_bind.is_some()
                && is_windows_ps_move_source(&state.source.hidraw_path)
            {
                continue;
            }
            let report = match reader.read_report(&state.source.hidraw_path) {
                Ok(report) => report,
                Err(error) => {
                    eprintln!(
                        "Muninn skipped Move source {} at {}: {error:#}",
                        state.source.move_id, state.source.hidraw_path
                    );
                    continue;
                }
            };
            let Some(report) = report else {
                continue;
            };
            state.sequence = state.sequence.saturating_add(1);
            let observed_at = timestamp()?;
            let source_timestamp_ns = timestamp_ns()?;
            let hid_record = build_hid_controller_state_record_from_report(
                options,
                &state.source,
                state.sequence,
                &report,
                source_timestamp_ns,
                observed_at.clone(),
            );
            trace_hid_controller_record("report", &hid_record, &report);
            put_hid_controller_state_receipt(node, options, &hid_record)?;
            publish_hid_controller_state_to_stream(
                hid_controller_stream.as_deref_mut(),
                &hid_record,
            );
            publish_hid_controller_state_to_odin(node, &hid_record);
            build_move_controller_state_record(
                options,
                &state.source,
                state.sequence,
                &report,
                source_timestamp_ns,
                observed_at,
            )
        };
        put_move_controller_state_receipt(node, options, &record)?;
        state.latest_move_record = Some(record.clone());
        published_records.push(record);
    }
    if let Some(stream) = move_evidence_stream {
        publish_move_evidence_stream_frame(stream, &[], &published_records)?;
    }
    Ok(())
}

fn active_move_marker_camera_sources(
    options: &Options,
    _move_hue_program: Arc<Mutex<MuninnMoveHueProgramRecord>>,
) -> Vec<ActiveMoveMarkerCameraSource> {
    options
        .move_marker_camera_sources
        .iter()
        .map(|source| {
            let stream_id = format!(
                "muninn:{}:{}:move-marker-candidates",
                options.host_id, source.camera_id
            );
            #[cfg(feature = "psmoveapi-tracker")]
            let (psmoveapi_observations, psmoveapi_health) = if options.move_psmoveapi_tracker {
                video_device_index(&source.device_path).and_then(|camera_index| {
                    Some(start_psmoveapi_tracker_worker(
                        options.store_path.clone(),
                        options.host_id.clone(),
                        source.camera_id.clone(),
                        camera_index,
                        options.move_tracker_camera_exposure_milli.get(&source.camera_id)
                            .copied().unwrap_or(options.move_tracker_exposure_milli) as f32 / 1000.0,
                        serve_move_state_sources(options, true),
                        Arc::clone(&_move_hue_program),
                    ))
                }).map_or((None, None), |(observations, health)| (Some(observations), Some(health)))
            } else {
                (None, None)
            };
            ActiveMoveMarkerCameraSource {
                source: source.clone(),
                frame_source: MoveMarkerFrameSource {
                    stream_id,
                    host_id: options.host_id.clone(),
                    camera_id: source.camera_id.clone(),
                    fps: options.move_marker_fps,
                    tracker_config: muninn_move_tracker::MoveTrackerConfig {
                        width: options.move_marker_width,
                        height: options.move_marker_height,
                        stride_bytes: options
                            .move_marker_stride_bytes
                            .unwrap_or(options.move_marker_width),
                        tile_size: 16,
                        threshold_min: options.move_marker_threshold_min,
                        min_area_px: options.move_marker_min_area_px,
                        max_candidates: options.move_marker_max_candidates,
                        source_id_hash: 0,
                        frame_sequence: 0,
                    },
                },
                sequence: 0,
                #[cfg(feature = "psmoveapi-tracker")]
                psmoveapi_observations,
                #[cfg(feature = "psmoveapi-tracker")]
                psmoveapi_health,
            }
        })
        .collect()
}

fn move_evidence_camera_inputs(
    active: &[ActiveMoveMarkerCameraSource],
) -> Vec<ActiveMoveMarkerCameraSource> {
    active
        .iter()
        .map(|camera| ActiveMoveMarkerCameraSource {
            source: camera.source.clone(),
            frame_source: camera.frame_source.clone(),
            sequence: 0,
            #[cfg(feature = "psmoveapi-tracker")]
            psmoveapi_observations: camera.psmoveapi_observations.as_ref().map(Arc::clone),
            #[cfg(feature = "psmoveapi-tracker")]
            psmoveapi_health: None,
        })
        .collect()
}

fn start_move_evidence_aggregator(
    mut stream: ActiveMoveEvidenceStream,
    mut cameras: Vec<ActiveMoveMarkerCameraSource>,
    latest_move_controller_states: Arc<Mutex<Vec<MuninnMoveControllerStateRecord>>>,
) {
    thread::spawn(move || {
        let mut reader = PlatformMoveMarkerCameraFrameReader::default();
        let fps = cameras
            .iter()
            .map(|camera| camera.frame_source.fps)
            .max()
            .unwrap_or(60)
            .max(1);
        let cadence = Duration::from_secs_f64(1.0 / f64::from(fps));
        let mut next_tick_at = Instant::now();
        loop {
            let now = Instant::now();
            if now < next_tick_at {
                thread::sleep(next_tick_at - now);
            }
            next_tick_at += cadence;
            if next_tick_at < Instant::now() {
                next_tick_at = Instant::now() + cadence;
            }
            let controller_states = latest_move_controller_states
                .lock()
                .map(|states| states.clone())
                .unwrap_or_default();
            if let Err(error) = publish_move_marker_camera_frames(
                &mut cameras,
                &mut reader,
                &controller_states,
                Some(&mut stream),
            ) {
                eprintln!("Muninn Move evidence aggregator warning: {error:#}");
            }
        }
    });
}

fn publish_move_marker_camera_frames(
    active: &mut [ActiveMoveMarkerCameraSource],
    reader: &mut impl MoveMarkerCameraFrameReader,
    latest_move_controller_states: &[MuninnMoveControllerStateRecord],
    move_evidence_stream: Option<&mut ActiveMoveEvidenceStream>,
) -> Result<()> {
    let Some(stream) = move_evidence_stream else {
        return Ok(());
    };
    let mut marker_candidates = Vec::new();
    for camera in active {
        camera.sequence = camera.sequence.saturating_add(1);
        let mut frame_source = camera.frame_source.clone();
        frame_source.tracker_config.frame_sequence = camera.sequence;
        frame_source.tracker_config.source_id_hash =
            stable_marker_camera_source_hash(&frame_source);
        #[cfg(feature = "psmoveapi-tracker")]
        if let Some(latest) = camera.psmoveapi_observations.as_ref() {
            let observed_at = timestamp()?;
            let observations = latest.lock().ok().and_then(|mut value| value.take()).unwrap_or_default();
            marker_candidates.extend(observations.into_iter().map(|observation| {
                let radius = observation.radius_px.max(0.0);
                MuninnMoveMarkerCandidateRecord {
                    stream_id: frame_source.stream_id.clone(),
                    host_id: frame_source.host_id.clone(),
                    camera_id: frame_source.camera_id.clone(),
                    frame_sequence: camera.sequence,
                    source_id_hash: frame_source.tracker_config.source_id_hash,
                    tile_x: 0,
                    tile_y: 0,
                    center_x_px: observation.center_x_px,
                    center_y_px: observation.center_y_px,
                    radius_px: radius,
                    area_px: (std::f32::consts::PI * radius * radius).round() as u32,
                    mean_luma: 0.0,
                    peak_luma: 0,
                    // PSMoveAPI exposes position age, not optical fit quality.
                    score: 0.5,
                    observed_at: observed_at.clone(),
                    move_id: observation.move_id,
                }
            }));
            continue;
        }
        let Some(frame) = reader.read_luma_frame(&camera.source, &frame_source)? else {
            continue;
        };
        let observed_at = timestamp()?;
        marker_candidates.extend(extract_move_marker_candidates_from_luma_frame(
            &frame_source,
            &frame,
            observed_at,
        )?);
    }
    if !marker_candidates.is_empty() {
        publish_move_evidence_stream_frame(
            stream,
            &marker_candidates,
            latest_move_controller_states,
        )?;
    }
    Ok(())
}

#[cfg(feature = "psmoveapi-tracker")]
fn start_psmoveapi_tracker_worker(
    store_path: PathBuf,
    host_id: String,
    camera_id: String,
    camera_index: i32,
    exposure: f32,
    move_state_sources: Vec<MoveStateSource>,
    move_hue_program: Arc<Mutex<MuninnMoveHueProgramRecord>>,
) -> (Arc<Mutex<Option<Vec<muninn_psmoveapi_tracker::PsmoveApiObservation>>>>, Arc<Mutex<Option<MuninnMoveTrackerHealthRecord>>>) {
    let observations_latest = Arc::new(Mutex::new(None));
    let health_latest = Arc::new(Mutex::new(None));
    let observations_output = Arc::clone(&observations_latest);
    let health_output = Arc::clone(&health_latest);
    thread::spawn(move || {
        let mut command = Command::new(env::current_exe().unwrap_or_else(|_| PathBuf::from("muninn")));
        command.arg("move-tracker-worker")
            .arg("--store").arg(store_path)
            .arg("--host").arg(&host_id)
            .arg("--move-marker-camera").arg(format!("{camera_id}=/dev/video{camera_index}"))
            .arg("--move-tracker-exposure-milli").arg(((exposure * 1000.0).round() as u32).to_string());
        for source in move_state_sources {
            command.arg("--move-state").arg(format!("{}={}", source.move_id, source.hidraw_path));
        }
        let child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn();
        let Ok(mut child) = child else { return; };
        let Some(mut stdin) = child.stdin.take() else { return; };
        let program_source = Arc::clone(&move_hue_program);
        thread::spawn(move || {
            let mut previous = None;
            loop {
                let Some(program) = program_source.lock().ok().map(|value| MoveTrackerWorkerProgram::from(&*value)) else { return; };
                if previous.as_ref() != Some(&program) {
                    if write_length_framed_message(&mut stdin, &program).is_err() { return; }
                    previous = Some(program);
                }
                thread::sleep(Duration::from_millis(100));
            }
        });
        let Some(stdout) = child.stdout.take() else { return; };
        let mut reader = BufReader::new(stdout);
        loop {
            let frame = match read_move_tracker_worker_frame(&mut reader) {
                Ok(frame) => frame,
                Err(error) => {
                    eprintln!("Muninn Move tracker worker stream stopped camera={camera_id}: {error:#}");
                    break;
                }
            };
            let observations = frame.observations.into_iter().map(|value| muninn_psmoveapi_tracker::PsmoveApiObservation {
                move_id: value.move_id, center_x_px: value.center_x_px, center_y_px: value.center_y_px,
                radius_px: value.radius_px, age_ms: value.age_ms,
            }).collect();
            if let Ok(mut latest) = health_output.lock() { *latest = Some(frame.health); }
            if let Ok(mut latest) = observations_output.lock() { *latest = Some(observations); }
        }
        let _ = child.wait();
    });
    (observations_latest, health_latest)
}

#[cfg(feature = "psmoveapi-tracker")]
fn read_move_tracker_worker_frame(reader: &mut impl Read) -> Result<MoveTrackerWorkerFrame> {
    read_length_framed_message(reader)
}

#[cfg(feature = "psmoveapi-tracker")]
fn read_length_framed_message<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > 1024 * 1024 { return Err(anyhow!("Move tracker worker frame exceeds 1 MiB")); }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    rmp_serde::from_slice(&payload).context("decoding length-framed MessagePack")
}

#[cfg(feature = "psmoveapi-tracker")]
fn write_move_tracker_worker_frame(writer: &mut impl Write, frame: &MoveTrackerWorkerFrame) -> Result<()> {
    write_length_framed_message(writer, frame)
}

#[cfg(feature = "psmoveapi-tracker")]
fn write_length_framed_message(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let payload = rmp_serde::to_vec(value)?;
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

#[cfg(feature = "psmoveapi-tracker")]
impl From<&MuninnMoveHueProgramRecord> for MoveTrackerWorkerProgram {
    fn from(value: &MuninnMoveHueProgramRecord) -> Self {
        Self { mode: value.mode.clone(), cycle_ms: value.cycle_ms, epoch_ns: value.epoch_ns,
            hold_at_ns: value.hold_at_ns, order_mode: value.order_mode.clone(), transition_percent: value.transition_percent,
            transition_percent_explicit: value.transition_percent_explicit }
    }
}

#[cfg(feature = "psmoveapi-tracker")]
impl MoveTrackerWorkerProgram {
    fn as_record(&self, host_id: &str) -> MuninnMoveHueProgramRecord {
        MuninnMoveHueProgramRecord { program_id: move_hue_program_key(host_id), host_id: host_id.to_string(),
            mode: self.mode.clone(), cycle_ms: self.cycle_ms, epoch_ns: self.epoch_ns, hold_at_ns: self.hold_at_ns,
            requested_by: "parent-worker-pipe".to_string(), updated_at: "worker-live".to_string(), order_mode: self.order_mode.clone(),
            transition_percent: self.transition_percent, transition_percent_explicit: self.transition_percent_explicit }
    }
}

#[cfg(feature = "psmoveapi-tracker")]
fn run_move_tracker_worker(options: Options) -> Result<()> {
    let source = options.move_marker_camera_sources.first()
        .context("move-tracker-worker requires one --move-marker-camera")?;
    if options.move_marker_camera_sources.len() != 1 {
        return Err(anyhow!("move-tracker-worker accepts exactly one camera"));
    }
    let camera_index = video_device_index(&source.device_path)
        .context("move-tracker-worker camera must be /dev/videoN")?;
    let mut roster = serve_move_state_sources(&options, true).into_iter()
        .map(|source| source.move_id).collect::<Vec<_>>();
    roster.sort();
    roster.dedup();
    let mut input = BufReader::new(std::io::stdin());
    let initial: MoveTrackerWorkerProgram = read_length_framed_message(&mut input)?;
    let mut program = initial.as_record(&options.host_id);
    let (program_sender, program_receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        while let Ok(program) = read_length_framed_message::<MoveTrackerWorkerProgram>(&mut input) {
            match program_sender.try_send(program) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            }
        }
    });
    let exposure = options.move_tracker_exposure_milli as f32 / 1000.0;
    let colors = tracker_colors(&roster, &program);
    let mut tracker = muninn_psmoveapi_tracker::PsmoveApiTracker::open(camera_index, exposure, &colors)?;
    let camera_info = tracker.camera_info().clone();
    let mut update_count = 0u64;
    let mut observation_count = 0u64;
    let mut rejected_stale_count = 0u64;
    let mut rejected_radius_count = 0u64;
    let mut rejected_bounds_count = 0u64;
    let mut rejected_continuity_count = 0u64;
    let mut admitted = HashMap::<String, AdmittedMoveObservation>::new();
    let mut last_observation_at = String::new();
    let mut last_calibration = Instant::now();
    let mut last_image_evidence = Instant::now() - Duration::from_secs(1);
    let mut image_mean_rgb = Vec::new();
    let mut image_peak_rgb = Vec::new();
    let mut color_evidence_move_ids = Vec::new();
    let mut color_evidence_pixel_counts = Vec::new();
    let stdout = std::io::stdout();
    let mut writer = stdout.lock();
    loop {
        while let Ok(latest) = program_receiver.try_recv() { program = latest.as_record(&options.host_id); }
        if last_calibration.elapsed() >= Duration::from_secs(30) {
            tracker.observe_connected();
            last_calibration = Instant::now();
        }
        let expected_colors = tracker_colors(&roster, &program);
        for (identity, color) in &expected_colors {
            tracker.set_expected_color(identity, *color);
        }
        let observed_now = Instant::now();
        let observations = tracker.update().into_iter().filter(|observation| {
            match admit_psmoveapi_observation(observation, camera_info.width, camera_info.height,
                admitted.get(&observation.move_id), observed_now) {
                Ok(next) => { admitted.insert(observation.move_id.clone(), next); true }
                Err(PsmoveApiObservationRejection::Stale) => { rejected_stale_count = rejected_stale_count.saturating_add(1); false }
                Err(PsmoveApiObservationRejection::Radius) => { rejected_radius_count = rejected_radius_count.saturating_add(1); false }
                Err(PsmoveApiObservationRejection::Bounds) => { rejected_bounds_count = rejected_bounds_count.saturating_add(1); false }
                Err(PsmoveApiObservationRejection::Continuity) => { rejected_continuity_count = rejected_continuity_count.saturating_add(1); false }
            }
        }).collect::<Vec<_>>();
        update_count = update_count.saturating_add(1);
        observation_count = observation_count.saturating_add(observations.len() as u64);
        if !observations.is_empty() { last_observation_at = timestamp()?; }
        if last_image_evidence.elapsed() >= Duration::from_millis(250) {
            if let Some((_, _, rgb)) = tracker.rgb_image() {
                (image_mean_rgb, image_peak_rgb, color_evidence_move_ids, color_evidence_pixel_counts) =
                    summarize_tracker_rgb(&rgb, &expected_colors);
            }
            last_image_evidence = Instant::now();
        }
        let health = MuninnMoveTrackerHealthRecord {
            health_id: format!("muninn:{}:{}:move-tracker-health", options.host_id, source.camera_id),
            host_id: options.host_id.clone(), camera_id: source.camera_id.clone(), camera_index,
            state: "running".to_string(), camera_name: camera_info.name.clone(), camera_api: camera_info.api.clone(),
            width: camera_info.width, height: camera_info.height, exposure: camera_info.exposure,
            calibrated_controller_count: tracker.tracked_controller_count() as u32, update_count, observation_count,
            latest_observation_count: observations.len() as u32, last_observation_at: last_observation_at.clone(),
            detail: "private subprocess worker".to_string(), updated_at: timestamp()?,
            image_mean_rgb: image_mean_rgb.clone(), image_peak_rgb: image_peak_rgb.clone(),
            color_evidence_move_ids: color_evidence_move_ids.clone(),
            color_evidence_pixel_counts: color_evidence_pixel_counts.clone(),
            rejected_stale_count, rejected_radius_count, rejected_bounds_count, rejected_continuity_count,
        };
        let frame = MoveTrackerWorkerFrame {
            health,
            observations: observations.into_iter().map(|value| MoveTrackerWorkerObservation {
                move_id: value.move_id, center_x_px: value.center_x_px, center_y_px: value.center_y_px,
                radius_px: value.radius_px, age_ms: value.age_ms,
            }).collect(),
        };
        write_move_tracker_worker_frame(&mut writer, &frame)?;
        thread::sleep(Duration::from_millis(4));
    }
}

#[cfg(feature = "psmoveapi-tracker")]
#[derive(Clone, Copy, Debug, PartialEq)]
struct AdmittedMoveObservation { center_x_px: f32, center_y_px: f32, radius_px: f32, admitted_at: Instant }

#[cfg(feature = "psmoveapi-tracker")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PsmoveApiObservationRejection { Stale, Radius, Bounds, Continuity }

#[cfg(feature = "psmoveapi-tracker")]
fn admit_psmoveapi_observation(observation: &muninn_psmoveapi_tracker::PsmoveApiObservation, width: u32, height: u32,
    previous: Option<&AdmittedMoveObservation>, now: Instant) -> std::result::Result<AdmittedMoveObservation, PsmoveApiObservationRejection> {
    if observation.age_ms < 0 || observation.age_ms > 50 { return Err(PsmoveApiObservationRejection::Stale); }
    if !observation.radius_px.is_finite() || observation.radius_px < 2.0 { return Err(PsmoveApiObservationRejection::Radius); }
    if !observation.center_x_px.is_finite() || !observation.center_y_px.is_finite() || observation.center_x_px < 0.0
        || observation.center_x_px >= width as f32 || observation.center_y_px < 0.0 || observation.center_y_px >= height as f32 {
        return Err(PsmoveApiObservationRejection::Bounds);
    }
    if let Some(previous) = previous {
        let elapsed = now.saturating_duration_since(previous.admitted_at).as_secs_f32();
        let dx = observation.center_x_px - previous.center_x_px;
        let dy = observation.center_y_px - previous.center_y_px;
        let distance = (dx * dx + dy * dy).sqrt();
        let radius_ratio = observation.radius_px / previous.radius_px;
        if distance > 80.0 + 2_500.0 * elapsed.min(0.25) || !(0.25..=4.0).contains(&radius_ratio) {
            return Err(PsmoveApiObservationRejection::Continuity);
        }
    }
    Ok(AdmittedMoveObservation { center_x_px: observation.center_x_px, center_y_px: observation.center_y_px,
        radius_px: observation.radius_px, admitted_at: now })
}

#[cfg(all(test, feature = "psmoveapi-tracker"))]
mod psmoveapi_observation_admission_tests {
    use super::*;
    fn observation(x: f32, y: f32, radius: f32, age_ms: i32) -> muninn_psmoveapi_tracker::PsmoveApiObservation {
        muninn_psmoveapi_tracker::PsmoveApiObservation { move_id: "move-test".to_string(), center_x_px: x, center_y_px: y, radius_px: radius, age_ms }
    }
    #[test]
    fn rejects_stale_tiny_and_out_of_frame_positions() {
        let now = Instant::now();
        assert_eq!(Err(PsmoveApiObservationRejection::Stale), admit_psmoveapi_observation(&observation(100.0, 100.0, 8.0, 51), 640, 480, None, now));
        assert_eq!(Err(PsmoveApiObservationRejection::Radius), admit_psmoveapi_observation(&observation(100.0, 100.0, 0.7, 1), 640, 480, None, now));
        assert_eq!(Err(PsmoveApiObservationRejection::Bounds), admit_psmoveapi_observation(&observation(641.0, 100.0, 8.0, 1), 640, 480, None, now));
    }
    #[test]
    fn rejects_fresh_teleport_without_calling_age_confidence() {
        let now = Instant::now();
        let previous = admit_psmoveapi_observation(&observation(100.0, 100.0, 8.0, 1), 640, 480, None, now).unwrap();
        assert_eq!(Err(PsmoveApiObservationRejection::Continuity), admit_psmoveapi_observation(
            &observation(500.0, 400.0, 8.0, 1), 640, 480, Some(&previous), now + Duration::from_millis(4)));
        assert!(admit_psmoveapi_observation(&observation(108.0, 104.0, 8.3, 1), 640, 480, Some(&previous), now + Duration::from_millis(4)).is_ok());
    }
}

#[cfg(feature = "psmoveapi-tracker")]
fn summarize_tracker_rgb(rgb: &[u8], expected: &[(String, [u8; 3])]) -> (Vec<u32>, Vec<u32>, Vec<String>, Vec<u32>) {
    let mut sums = [0u64; 3];
    let mut peaks = [0u32; 3];
    let mut counts = vec![0u32; expected.len()];
    let pixels = rgb.len() / 3;
    for pixel in rgb.chunks_exact(3) {
        for channel in 0..3 { sums[channel] += u64::from(pixel[channel]); peaks[channel] = peaks[channel].max(u32::from(pixel[channel])); }
        let max = *pixel.iter().max().unwrap_or(&0);
        let min = *pixel.iter().min().unwrap_or(&0);
        if max < 40 || max.saturating_sub(min) < 30 { continue; }
        let pixel_norm = (pixel.iter().map(|value| f64::from(*value).powi(2)).sum::<f64>()).sqrt();
        for (index, (_, color)) in expected.iter().enumerate() {
            let color_norm = (color.iter().map(|value| f64::from(*value).powi(2)).sum::<f64>()).sqrt();
            let dot = pixel.iter().zip(color).map(|(left, right)| f64::from(*left) * f64::from(*right)).sum::<f64>();
            if pixel_norm > 0.0 && color_norm > 0.0 && dot / (pixel_norm * color_norm) >= 0.985 {
                counts[index] = counts[index].saturating_add(1);
            }
        }
    }
    let means = if pixels == 0 { vec![0, 0, 0] } else { sums.iter().map(|sum| (*sum / pixels as u64) as u32).collect() };
    (means, peaks.to_vec(), expected.iter().map(|(id, _)| id.clone()).collect(), counts)
}

#[cfg(feature = "psmoveapi-tracker")]
fn tracker_colors(roster: &[String], program: &MuninnMoveHueProgramRecord) -> Vec<(String, [u8; 3])> {
    let now_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as i128;
    let timestamp_ns = move_hue_program_timestamp_ns(program, now_ns);
    roster.iter().filter_map(|identity| scheduled_golden_move_color_with_order(
        identity, roster, i128::from(program.epoch_ns), i128::from(program.cycle_ms) * 1_000_000,
        timestamp_ns, &program.order_mode, effective_transition_percent(program),
    ).map(|(color, _, _)| (identity.clone(), [color.0, color.1, color.2]))).collect()
}

#[cfg(not(feature = "psmoveapi-tracker"))]
fn run_move_tracker_worker(_options: Options) -> Result<()> {
    Err(anyhow!("move-tracker-worker requires the psmoveapi-tracker feature"))
}

#[cfg(feature = "psmoveapi-tracker")]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MoveTrackerWorkerObservation {
    move_id: String,
    center_x_px: f32,
    center_y_px: f32,
    radius_px: f32,
    age_ms: i32,
}

#[cfg(feature = "psmoveapi-tracker")]
#[derive(Clone, Debug, Serialize, Deserialize)]
struct MoveTrackerWorkerFrame {
    health: MuninnMoveTrackerHealthRecord,
    observations: Vec<MoveTrackerWorkerObservation>,
}

#[cfg(feature = "psmoveapi-tracker")]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct MoveTrackerWorkerProgram {
    mode: String,
    cycle_ms: u64,
    epoch_ns: i64,
    hold_at_ns: i64,
    order_mode: String,
    transition_percent: u8,
    transition_percent_explicit: bool,
}

#[cfg(feature = "psmoveapi-tracker")]
fn publish_move_tracker_health(node: &mut cultmesh_rs::CultMeshNode, active: &mut [ActiveMoveMarkerCameraSource]) -> Result<()> {
    for camera in active {
        let Some(latest) = camera.psmoveapi_health.as_ref() else { continue; };
        let record = latest.lock().ok().and_then(|mut value| value.take());
        if let Some(record) = record { node.put(&record.health_id, &record)?; }
    }
    Ok(())
}

#[cfg(not(feature = "psmoveapi-tracker"))]
fn publish_move_tracker_health(_node: &mut cultmesh_rs::CultMeshNode, _active: &mut [ActiveMoveMarkerCameraSource]) -> Result<()> { Ok(()) }

fn publish_move_evidence_transport_health(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    stream: Option<&MoveEvidenceTransportHealth>,
) -> Result<()> {
    let Some(stream) = stream else { return Ok(()); };
    let record = MuninnMoveEvidenceTransportHealthRecord {
        health_id: format!("muninn:{}:move-evidence-transport-health", options.host_id),
        host_id: options.host_id.clone(),
        stream_id: stream.stream_id.clone(),
        produced_frames: stream.counters.produced_frames.load(Ordering::Relaxed),
        local_ring_admissions: stream.counters.local_ring_admissions.load(Ordering::Relaxed),
        remote_handoffs: stream.counters.remote_handoffs.load(Ordering::Relaxed),
        remote_sends: stream.counters.remote_sends.load(Ordering::Relaxed),
        updated_at: timestamp()?,
    };
    node.put(&record.health_id, &record)?;
    Ok(())
}

#[cfg(feature = "psmoveapi-tracker")]
fn video_device_index(path: &Path) -> Option<i32> {
    path.file_name()?.to_str()?.strip_prefix("video")?.parse().ok()
}

fn stable_marker_camera_source_hash(source: &MoveMarkerFrameSource) -> u64 {
    stable_u64_hash(&format!(
        "{}:{}:{}",
        source.host_id, source.camera_id, source.stream_id
    ))
}

fn put_hid_controller_state_receipt(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    record: &MuninnHidControllerStateRecord,
) -> Result<()> {
    node.put(&record.stream_id, record)?;
    node.put(
        &sequence_receipt_key(&record.stream_id, record.sequence),
        record,
    )?;
    prune_hid_controller_state_receipt(node, options, &record.stream_id, record.sequence)
}

fn put_move_controller_state_receipt(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    record: &MuninnMoveControllerStateRecord,
) -> Result<()> {
    node.put(&record.stream_id, record)?;
    node.put(
        &sequence_receipt_key(&record.stream_id, record.sequence),
        record,
    )?;
    prune_move_controller_state_receipt(node, options, &record.stream_id, record.sequence)
}

fn prune_hid_controller_state_receipt(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    stream_id: &str,
    current_sequence: u64,
) -> Result<()> {
    let Some(first_retained_sequence) = first_retained_receipt_sequence(options, current_sequence)
    else {
        return Ok(());
    };
    for key in stale_sequence_receipt_keys(node, stream_id, first_retained_sequence) {
        let _ = node.delete::<MuninnHidControllerStateRecord>(&key)?;
    }
    Ok(())
}

fn prune_move_controller_state_receipt(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    stream_id: &str,
    current_sequence: u64,
) -> Result<()> {
    let Some(first_retained_sequence) = first_retained_receipt_sequence(options, current_sequence)
    else {
        return Ok(());
    };
    for key in stale_sequence_receipt_keys(node, stream_id, first_retained_sequence) {
        let _ = node.delete::<MuninnMoveControllerStateRecord>(&key)?;
    }
    Ok(())
}

fn first_retained_receipt_sequence(options: &Options, current_sequence: u64) -> Option<u64> {
    let interval_seconds = options.interval_seconds.unwrap_or(15).max(1);
    let retained_sequences = (options.hid_controller_receipt_retention_seconds / interval_seconds)
        .saturating_add(2)
        .max(1);
    current_sequence.checked_sub(retained_sequences)
}

fn stale_sequence_receipt_keys(
    node: &cultmesh_rs::CultMeshNode,
    stream_id: &str,
    first_retained_sequence: u64,
) -> Vec<String> {
    node.cache()
        .snapshot()
        .into_iter()
        .filter_map(|envelope| {
            let sequence = sequence_from_receipt_key(stream_id, &envelope.key)?;
            (sequence < first_retained_sequence).then_some(envelope.key)
        })
        .collect()
}

fn sequence_from_receipt_key(stream_id: &str, key: &str) -> Option<u64> {
    let suffix = key.strip_prefix(stream_id)?.strip_prefix(':')?;
    suffix.parse().ok()
}

fn sequence_receipt_key(stream_id: &str, sequence: u64) -> String {
    format!("{stream_id}:{sequence}")
}

fn is_joystick_path(path: &str) -> bool {
    path.contains("/dev/input/js") || path.contains("-joystick")
}

fn is_xinput_source_path(path: &str) -> bool {
    path.to_ascii_lowercase()
        .starts_with(WINDOWS_XINPUT_SOURCE_PREFIX)
}

fn xinput_index_from_source_path(path: &str) -> Option<u32> {
    path.to_ascii_lowercase()
        .strip_prefix(WINDOWS_XINPUT_SOURCE_PREFIX)?
        .parse::<u32>()
        .ok()
        .filter(|index| *index < 4)
}

struct ActiveHidControllerStream {
    target: SocketAddr,
    transport: CultNetRudpSocketTransportConnection,
    last_connect_attempt_at: Option<Instant>,
    connected_logged: bool,
    sent_frames: u64,
    last_wait_log_at: Option<Instant>,
    last_sent_at: Option<Instant>,
    last_stale_log_at: Option<Instant>,
}

struct ActiveHidControllerRudpSource {
    source: MoveStateSource,
    epoch: u64,
    sequence: u64,
    next_edge_sequence: u64,
    pending_edges: VecDeque<HidButtonEdge>,
    edge_buttons: Vec<String>,
    last_edge_send_at: Option<Instant>,
    joystick_axes: [i16; 16],
    joystick_buttons: [bool; 32],
    latest_report: Option<LatestHidReport>,
    last_emitted_axes: Option<Vec<f32>>,
    last_emitted_buttons: Option<Vec<String>>,
    last_emitted_at: Option<Instant>,
    #[cfg(windows)]
    report_rx: Option<mpsc::Receiver<LatestHidReport>>,
    last_read_error_log_at: Option<Instant>,
}

const MUNINN_HID_MAX_PENDING_EDGES: usize = 256;

fn active_hid_controller_rudp_source(source: MoveStateSource) -> ActiveHidControllerRudpSource {
    let epoch = timestamp_ns().unwrap_or_default().max(0) as u64;
    ActiveHidControllerRudpSource {
        #[cfg(windows)]
        report_rx: start_windows_hid_report_reader_if_supported(&source.hidraw_path),
        source,
        epoch,
        sequence: 0,
        next_edge_sequence: 1,
        pending_edges: VecDeque::new(),
        edge_buttons: Vec::new(),
        last_edge_send_at: None,
        joystick_axes: [0; 16],
        joystick_buttons: [false; 32],
        latest_report: None,
        last_emitted_axes: None,
        last_emitted_buttons: None,
        last_emitted_at: None,
        last_read_error_log_at: None,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct HidButtonEdge {
    epoch: u64,
    device_id: String,
    edge_sequence: u64,
    button: String,
    pressed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct HidLatestStateFrame {
    epoch: u64,
    state_sequence: u64,
    record: MuninnHidControllerStateRecord,
}

#[derive(Clone, Debug, Deserialize)]
struct HidEdgeAck {
    epoch: u64,
    device_id: String,
    edge_sequence: u64,
}

fn capture_button_edges(active: &mut ActiveHidControllerRudpSource, buttons: &[String]) {
    let previous = active.edge_buttons.clone();
    for button in previous.iter().filter(|button| !buttons.contains(button)) {
        active.pending_edges.push_back(HidButtonEdge { epoch: active.epoch.clone(), device_id: active.source.move_id.clone(), edge_sequence: active.next_edge_sequence, button: button.clone(), pressed: false });
        active.next_edge_sequence = active.next_edge_sequence.saturating_add(1);
    }
    for button in buttons.iter().filter(|button| !previous.contains(button)) {
        active.pending_edges.push_back(HidButtonEdge { epoch: active.epoch.clone(), device_id: active.source.move_id.clone(), edge_sequence: active.next_edge_sequence, button: button.clone(), pressed: true });
        active.next_edge_sequence = active.next_edge_sequence.saturating_add(1);
    }
    active.edge_buttons = buttons.to_vec();
    if active.pending_edges.len() > MUNINN_HID_MAX_PENDING_EDGES {
        active.epoch = active.epoch.saturating_add(1).max(1);
        active.sequence = 0;
        active.next_edge_sequence = 1;
        active.pending_edges.clear();
        active.last_edge_send_at = None;
    }
}

fn sync_hid_controller_rudp_sources(
    active: &mut Vec<ActiveHidControllerRudpSource>,
    desired: Vec<MoveStateSource>,
) {
    active.retain(|state| desired.iter().any(|source| source == &state.source));
    for source in desired {
        if active.iter().any(|state| state.source == source) {
            continue;
        }
        active.push(active_hid_controller_rudp_source(source));
    }
}

#[derive(Clone, Debug)]
struct LatestHidReport {
    bytes: Vec<u8>,
    source_timestamp_ns: i64,
    observed_at: String,
}

const HID_CONTROLLER_RUDP_HEARTBEAT_AFTER: Duration = Duration::from_millis(50);
const HID_CONTROLLER_RUDP_AXIS_EPSILON: f32 = 0.01;
const HID_CONTROLLER_RUDP_MAX_FRAGMENT_BYTES: u32 = 1_200;
const HID_CONTROLLER_RUDP_MAX_REPORT_DRAIN: usize = 256;

#[derive(Clone, Debug, Deserialize)]
struct HidControllerRudpSubscription {
    #[serde(rename = "deviceFilter")]
    device_filter: Option<String>,
    #[serde(rename = "streamId")]
    stream_id: Option<String>,
}

fn start_hid_controller_rudp_ingress(
    options: &Options,
    counters: Option<Arc<MoveEvidenceTransportCounters>>,
) -> Result<Option<Arc<Mutex<Option<Vec<u8>>>>>> {
    let Some(bind_address) = options.hid_controller_rudp_bind else {
        return Ok(None);
    };
    if live_move_state_sources(options).is_empty() {
        return Ok(None);
    }
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Muninn HID controller RUDP stream at {bind_address}"))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(1)))
        .with_context(|| format!("setting Muninn HID controller RUDP timeout at {bind_address}"))?;
    let move_evidence_latest = Arc::new(Mutex::new(None));
    let ingress_evidence = Arc::clone(&move_evidence_latest);
    let runtime_options = options.clone();
    thread::spawn(move || {
        if let Err(error) =
            run_hid_controller_rudp_ingress(socket, runtime_options, ingress_evidence, counters)
        {
            eprintln!("Muninn HID controller RUDP stream stopped: {error:#}");
        }
    });
    Ok(Some(move_evidence_latest))
}

fn run_hid_controller_rudp_ingress(
    socket: UdpSocket,
    options: Options,
    move_evidence_latest: Arc<Mutex<Option<Vec<u8>>>>,
    counters: Option<Arc<MoveEvidenceTransportCounters>>,
) -> Result<()> {
    let local_addr = socket.local_addr()?;
    println!("Muninn HID controller RUDP stream listening at {local_addr}.");
    let mut transport =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "muninn-hid-controller-rudp".to_string(),
            socket,
            mode: cultnet_rs::CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id: MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
            initial_sequence: 1,
            resend_delay_ms: 5,
            transport_id: Some("muninn-hid-controller-rudp".to_string()),
            max_payload_bytes: None,
            max_fragment_bytes: Some(HID_CONTROLLER_RUDP_MAX_FRAGMENT_BYTES),
            max_pending_reliable_packets: Some(256),
            reconnect_policy: None,
        })?;
    let mut sources = live_move_state_sources(&options)
        .into_iter()
        .map(active_hid_controller_rudp_source)
        .collect::<Vec<_>>();
    let mut last_source_refresh_at = Instant::now();
    let mut reader = HidMoveControllerStateReader;
    let mut sent_frames = 0u64;
    let mut last_sent_at = None::<Instant>;
    let mut last_stale_log_at = None::<Instant>;
    let mut selected_device_filter = None::<String>;
    let mut selected_stream_id = None::<String>;
    let mut last_waiting_for_subscription_log_at = None::<Instant>;
    loop {
        if last_source_refresh_at.elapsed() >= Duration::from_millis(500) {
            sync_hid_controller_rudp_sources(&mut sources, live_move_state_sources(&options));
            last_source_refresh_at = Instant::now();
        }
        for _ in 0..16 {
            match transport.receive_once() {
                Ok(Some(frame)) if frame.channel_id == "hid.subscribe" => {
                    match serde_json::from_slice::<HidControllerRudpSubscription>(&frame.payload) {
                        Ok(subscription) => {
                            let next_device_filter = subscription
                                .device_filter
                                .filter(|filter| !filter.trim().is_empty());
                            let next_stream_id = subscription
                                .stream_id
                                .filter(|stream_id| !stream_id.trim().is_empty());
                            if selected_device_filter != next_device_filter
                                || selected_stream_id != next_stream_id
                            {
                                eprintln!(
                                    "Muninn HID controller RUDP subscription device_filter={:?} stream_id={:?}",
                                    next_device_filter, next_stream_id
                                );
                            }
                            selected_device_filter = next_device_filter;
                            selected_stream_id = next_stream_id;
                        }
                        Err(error) => {
                            eprintln!(
                                "Muninn ignored invalid HID controller RUDP subscription: {error:#}"
                            );
                        }
                    }
                }
                Ok(Some(frame)) if frame.channel_id == "hid.edge.ack" => {
                    if let Ok(ack) = serde_json::from_slice::<HidEdgeAck>(&frame.payload) {
                        if let Some(source) = sources.iter_mut().find(|source| source.source.move_id == ack.device_id && source.epoch == ack.epoch) {
                            while source.pending_edges.front().is_some_and(|edge| edge.edge_sequence <= ack.edge_sequence) {
                                source.pending_edges.pop_front();
                            }
                        }
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => {
                    eprintln!("Muninn HID controller RUDP receive warning: {error:#}");
                    break;
                }
            }
        }
        if transport.connected() && selected_device_filter.is_none() && selected_stream_id.is_none()
        {
            let should_log = last_waiting_for_subscription_log_at
                .is_none_or(|logged_at| logged_at.elapsed() >= Duration::from_secs(5));
            if should_log {
                eprintln!(
                    "Muninn HID controller RUDP has a consumer but no selected HID subscription; not broadcasting inputs"
                );
                last_waiting_for_subscription_log_at = Some(Instant::now());
            }
            thread::sleep(Duration::from_millis(8));
            continue;
        }
        if transport.connected() {
            let payload = move_evidence_latest.lock().ok().and_then(|mut latest| latest.take());
            if let Some(payload) = payload {
                if let Err(error) = transport.send("move-evidence", payload) {
                    eprintln!("Muninn Move evidence RUDP send warning: {error:#}");
                } else if let Some(counters) = counters.as_ref() {
                    counters.remote_sends.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        for source in &mut sources {
            if !hid_controller_source_matches_subscription(
                &options,
                &source.source,
                selected_device_filter.as_deref(),
                selected_stream_id.as_deref(),
            ) {
                continue;
            }
            let Some(record) = read_hid_controller_state_for_stream(&options, source, &mut reader)
            else {
                continue;
            };
            if !hid_controller_record_matches_subscription(
                &record,
                selected_device_filter.as_deref(),
                selected_stream_id.as_deref(),
            ) {
                continue;
            }
            if transport.connected() {
                let payload = serde_json::to_vec(&HidLatestStateFrame { epoch: source.epoch.clone(), state_sequence: record.sequence, record: sanitize_hid_controller_record(record.clone()) })
                    .context("encoding Muninn HID controller stream record")?;
                if let Err(error) = transport.send("latest", payload) {
                    eprintln!("Muninn HID controller RUDP send warning: {error:#}");
                    break;
                }
                if source.last_edge_send_at.is_none_or(|sent| sent.elapsed() >= Duration::from_millis(15)) {
                  if let Some(edge) = source.pending_edges.front() {
                    if let Err(error) = transport.send("hid.edge", serde_json::to_vec(edge)?) {
                        eprintln!("Muninn HID edge send warning: {error:#}");
                    } else {
                        source.last_edge_send_at = Some(Instant::now());
                    }
                  }
                }
                sent_frames = sent_frames.saturating_add(1);
                last_sent_at = Some(Instant::now());
                last_stale_log_at = None;
                if sent_frames % 600 == 0 {
                    eprintln!(
                        "Muninn HID controller RUDP served frame #{} device={} seq={}",
                        sent_frames, record.device_id, record.sequence
                    );
                }
            }
        }
        if let Err(error) = transport.poll_resends() {
            eprintln!("Muninn HID controller RUDP resend warning: {error:#}");
        }
        if transport.connected()
            && last_sent_at.is_some_and(|sent_at| sent_at.elapsed() >= Duration::from_secs(2))
        {
            let should_log = last_stale_log_at
                .is_none_or(|logged_at| logged_at.elapsed() >= Duration::from_secs(5));
            if should_log {
                eprintln!(
                    "Muninn HID controller RUDP has an active consumer but has sent no frames for {:?}",
                    last_sent_at.map(|sent_at| sent_at.elapsed())
                );
                last_stale_log_at = Some(Instant::now());
            }
        }
        thread::sleep(Duration::from_millis(8));
    }
}

fn hid_controller_source_matches_subscription(
    options: &Options,
    source: &MoveStateSource,
    device_filter: Option<&str>,
    stream_id: Option<&str>,
) -> bool {
    let source_stream_id = format!(
        "{}:{}:hid-controller-state",
        options.host_id, source.move_id
    );
    let stream_matches = stream_id.is_none_or(|stream_id| source_stream_id == stream_id);
    let device_matches = device_filter.is_none_or(|filter| {
        source.move_id == filter
            || source_stream_id == filter
            || source.hidraw_path.contains(filter)
            || hid_controller_kind_from_source(source) == filter
    });
    stream_matches && device_matches
}

fn hid_controller_record_matches_subscription(
    record: &MuninnHidControllerStateRecord,
    device_filter: Option<&str>,
    stream_id: Option<&str>,
) -> bool {
    let stream_matches = stream_id.is_none_or(|stream_id| record.stream_id == stream_id);
    let device_matches = device_filter.is_none_or(|filter| {
        record.device_id == filter
            || record.stream_id == filter
            || record.source_path.contains(filter)
            || record.device_kind == filter
    });
    stream_matches && device_matches
}

fn read_hid_controller_state_for_stream(
    options: &Options,
    active: &mut ActiveHidControllerRudpSource,
    reader: &mut impl MoveControllerStateReader,
) -> Option<MuninnHidControllerStateRecord> {
    let source = active.source.clone();
    if is_joystick_path(&source.hidraw_path) {
        let events = match reader.read_joystick_events(&source.hidraw_path) {
            Ok(events) => {
                active.last_read_error_log_at = None;
                events
            }
            Err(error) => {
                let should_log = active
                    .last_read_error_log_at
                    .is_none_or(|logged_at| logged_at.elapsed() >= Duration::from_secs(5));
                if should_log {
                    eprintln!(
                        "Muninn skipped HID controller stream source {} at {}: {error:#}",
                        source.move_id, source.hidraw_path
                    );
                    active.last_read_error_log_at = Some(Instant::now());
                }
                return None;
            }
        };
        for event in events {
            match event.event_type & 0x7f {
                0x01 => {
                    if let Some(button) = active.joystick_buttons.get_mut(event.number as usize) {
                        *button = event.value != 0;
                    }
                    let buttons = joystick_button_names(active.joystick_buttons);
                    capture_button_edges(active, &buttons);
                }
                0x02 => {
                    if let Some(axis) = active.joystick_axes.get_mut(event.number as usize) {
                        *axis = event.value;
                    }
                }
                _ => {}
            }
        }
        let record = build_hid_controller_state_record_from_joystick(
            options,
            &source,
            active.sequence.saturating_add(1),
            active.joystick_axes,
            active.joystick_buttons,
            timestamp_ns().ok()?,
            timestamp().ok()?,
        );
        return emit_hid_controller_record_if_due(active, record);
    }
    if !is_xinput_source_path(&source.hidraw_path) {
        #[cfg(windows)]
        if let Some(rx) = active.report_rx.as_ref() {
            let mut saw_report = false;
            let mut drained_reports = 0usize;
            let mut reports = Vec::new();
            for _ in 0..HID_CONTROLLER_RUDP_MAX_REPORT_DRAIN {
                let Ok(report) = rx.try_recv() else {
                    break;
                };
                reports.push(report);
                saw_report = true;
                drained_reports += 1;
            }
            for report in reports {
                let buttons = hid_controller_button_names(&active.source, &report.bytes);
                capture_button_edges(active, &buttons);
                active.latest_report = Some(report);
            }
            if drained_reports == HID_CONTROLLER_RUDP_MAX_REPORT_DRAIN {
                eprintln!(
                    "Muninn Windows HID reader drained {drained_reports} queued reports for {}; keeping latest",
                    source.move_id
                );
            }
            if saw_report {
                active.last_read_error_log_at = None;
            } else if active.latest_report.is_none() || !hid_controller_stream_heartbeat_due(active)
            {
                return None;
            }
            let report = active.latest_report.as_ref()?;
            let record = build_hid_controller_state_record_from_report(
                options,
                &source,
                active.sequence.saturating_add(1),
                &report.bytes,
                report.source_timestamp_ns,
                report.observed_at.clone(),
            );
            trace_hid_controller_record("rudp-report", &record, &report.bytes);
            return emit_hid_controller_record_if_due(active, record);
        }
        let report = {
            match reader.read_report(&source.hidraw_path) {
                Ok(Some(report)) => report,
                Ok(None) => return None,
                Err(error) => {
                    let should_log = active
                        .last_read_error_log_at
                        .is_none_or(|logged_at| logged_at.elapsed() >= Duration::from_secs(5));
                    if should_log {
                        eprintln!(
                            "Muninn skipped HID controller stream source {} at {}: {error:#}",
                            source.move_id, source.hidraw_path
                        );
                        active.last_read_error_log_at = Some(Instant::now());
                    }
                    return None;
                }
            }
        };
        let record = build_hid_controller_state_record_from_report(
            options,
            &source,
            active.sequence.saturating_add(1),
            &report,
            timestamp_ns().ok()?,
            timestamp().ok()?,
        );
        trace_hid_controller_record("rudp-report", &record, &report);
        return emit_hid_controller_record_if_due(active, record);
    }
    let Some(index) = xinput_index_from_source_path(&source.hidraw_path) else {
        return None;
    };
    let gamepad = match platform_xinput_gamepad(index) {
        Ok(Some(gamepad)) => {
            active.last_read_error_log_at = None;
            gamepad
        }
        Ok(None) => return None,
        Err(error) => {
            let should_log = active
                .last_read_error_log_at
                .is_none_or(|logged_at| logged_at.elapsed() >= Duration::from_secs(5));
            if should_log {
                eprintln!(
                    "Muninn skipped HID controller stream source {} at {}: {error:#}",
                    source.move_id, source.hidraw_path
                );
                active.last_read_error_log_at = Some(Instant::now());
            }
            return None;
        }
    };
    let record = build_hid_controller_state_record_from_xinput_gamepad(
        options,
        &source,
        active.sequence.saturating_add(1),
        &gamepad,
        timestamp_ns().ok()?,
        timestamp().ok()?,
    );
    emit_hid_controller_record_if_due(active, record)
}

fn emit_hid_controller_record_if_due(
    active: &mut ActiveHidControllerRudpSource,
    mut record: MuninnHidControllerStateRecord,
) -> Option<MuninnHidControllerStateRecord> {
    capture_button_edges(active, &record.buttons);
    let axes_changed = active
        .last_emitted_axes
        .as_ref()
        .is_none_or(|previous| hid_controller_axes_changed(previous, &record.axes));
    let buttons_changed = active
        .last_emitted_buttons
        .as_ref()
        .is_none_or(|previous| previous != &record.buttons);
    let heartbeat_due = active
        .last_emitted_at
        .is_none_or(|last| last.elapsed() >= HID_CONTROLLER_RUDP_HEARTBEAT_AFTER);
    if !(axes_changed || buttons_changed || heartbeat_due) {
        return None;
    }
    active.sequence = active.sequence.saturating_add(1);
    record.sequence = active.sequence;
    active.last_emitted_axes = Some(record.axes.clone());
    active.last_emitted_buttons = Some(record.buttons.clone());
    active.last_emitted_at = Some(Instant::now());
    Some(record)
}

fn hid_controller_stream_heartbeat_due(active: &ActiveHidControllerRudpSource) -> bool {
    active
        .last_emitted_at
        .is_none_or(|last| last.elapsed() >= HID_CONTROLLER_RUDP_HEARTBEAT_AFTER)
}

fn hid_controller_axes_changed(previous: &[f32], current: &[f32]) -> bool {
    previous.len() != current.len()
        || previous
            .iter()
            .zip(current.iter())
            .any(|(previous, current)| {
                (previous - current).abs() > HID_CONTROLLER_RUDP_AXIS_EPSILON
            })
}

fn build_hid_controller_state_record_from_report(
    options: &Options,
    source: &MoveStateSource,
    sequence: u64,
    report: &[u8],
    source_timestamp_ns: i64,
    observed_at: String,
) -> MuninnHidControllerStateRecord {
    MuninnHidControllerStateRecord {
        stream_id: format!(
            "{}:{}:hid-controller-state",
            options.host_id, source.move_id
        ),
        host_id: options.host_id.clone(),
        device_id: source.move_id.clone(),
        device_kind: hid_controller_kind_from_source(source),
        sequence,
        source_timestamp_ns,
        axes: hid_controller_axes_from_report(source, report),
        buttons: hid_controller_button_names(source, report),
        battery01: move_battery01(report.get(12).copied().unwrap_or_default()),
        observed_at,
        source_path: source.hidraw_path.clone(),
    }
}

fn trace_hid_controller_record(
    source: &str,
    record: &MuninnHidControllerStateRecord,
    report: &[u8],
) {
    if !muninn_hid_trace_enabled() {
        return;
    }
    let raw = report
        .iter()
        .take(24)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "Muninn HID trace source={source} device={} kind={} seq={} axes={:?} buttons={:?} raw24=[{}] path={}",
        record.device_id,
        record.device_kind,
        record.sequence,
        record.axes,
        record.buttons,
        raw,
        record.source_path
    );
}

fn muninn_hid_trace_enabled() -> bool {
    env::var("MUNINN_HID_TRACE")
        .map(|value| {
            let value = value.trim();
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
}

fn hid_controller_axes_from_report(source: &MoveStateSource, report: &[u8]) -> Vec<f32> {
    if hid_controller_kind_from_source(source) == "ps3-navigation" {
        return vec![
            byte_axis_to_signed_unit(report.get(6).copied()),
            byte_axis_to_signed_unit(report.get(7).copied()),
            trigger_to_signed_unit(report.get(18).copied().unwrap_or_default()),
        ];
    }
    vec![
        signed_report_axis_to_unit(report, 19),
        signed_report_axis_to_unit(report, 21),
        trigger_to_signed_unit(report.get(6).copied().unwrap_or_default()),
        signed_report_axis_to_unit(report, 25),
        signed_report_axis_to_unit(report, 27),
    ]
}

fn hid_controller_button_names(source: &MoveStateSource, report: &[u8]) -> Vec<String> {
    if hid_controller_kind_from_source(source) == "ps3-navigation" {
        return ps3_navigation_button_names(report);
    }
    move_button_names(report)
}

fn ps3_navigation_button_names(report: &[u8]) -> Vec<String> {
    let bits = report.get(1).copied().unwrap_or_default() as u32
        | ((report.get(2).copied().unwrap_or_default() as u32) << 8)
        | ((report.get(3).copied().unwrap_or_default() as u32) << 16);
    [
        (1 << 0, "up"),
        (1 << 1, "right"),
        (1 << 2, "down"),
        (1 << 3, "left"),
        (1 << 4, "triangle"),
        (1 << 5, "circle"),
        (1 << 6, "cross"),
        (1 << 7, "square"),
        (1 << 8, "select"),
        (1 << 9, "l3"),
        (1 << 10, "r3"),
        (1 << 11, "start"),
        (1 << 12, "up"),
        (1 << 13, "right"),
        (1 << 14, "down"),
        (1 << 15, "left"),
        (1 << 16, "ps"),
        (1 << 18, "l1"),
        (1 << 19, "l2"),
        (1 << 20, "trigger"),
    ]
    .iter()
    .filter_map(|(mask, name)| {
        if bits & mask != 0 {
            Some((*name).to_string())
        } else {
            None
        }
    })
    .fold(Vec::<String>::new(), |mut buttons, button| {
        if !buttons.iter().any(|existing| existing == &button) {
            buttons.push(button);
        }
        buttons
    })
}

fn byte_axis_to_signed_unit(value: Option<u8>) -> f32 {
    match value {
        Some(value) => ((value as f32 / 255.0) * 2.0 - 1.0).clamp(-1.0, 1.0),
        None => 0.0,
    }
}

fn signed_report_axis_to_unit(report: &[u8], offset: usize) -> f32 {
    axis_to_signed_unit(read_le_i16(report, offset))
}

fn sanitize_hid_controller_record(
    mut record: MuninnHidControllerStateRecord,
) -> MuninnHidControllerStateRecord {
    if !record.battery01.is_finite() {
        record.battery01 = -1.0;
    }
    for axis in &mut record.axes {
        if !axis.is_finite() {
            *axis = 0.0;
        }
    }
    record
}

fn create_hid_controller_stream(options: &Options) -> Result<Option<ActiveHidControllerStream>> {
    let Some(target) = options.hid_controller_rudp_target else {
        return Ok(None);
    };
    let bind_address = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Muninn HID RUDP sender at {bind_address}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(1)))?;
    eprintln!("Muninn HID fast stream targeting {target} over CultNet RUDP");
    let transport = CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
        runtime_id: "muninn-hid-controller-rudp".to_string(),
        socket,
        mode: cultnet_rs::CultNetRudpSocketMode::Client,
        remote_addr: Some(target),
        connection_id: MUNINN_HID_CONTROLLER_RUDP_CONNECTION_ID,
        initial_sequence: 1,
        resend_delay_ms: 5,
        transport_id: Some("muninn-hid-controller-rudp".to_string()),
        max_payload_bytes: None,
        max_fragment_bytes: Some(HID_CONTROLLER_RUDP_MAX_FRAGMENT_BYTES),
        max_pending_reliable_packets: Some(256),
        reconnect_policy: None,
    })?;
    Ok(Some(ActiveHidControllerStream {
        target,
        transport,
        last_connect_attempt_at: None,
        connected_logged: false,
        sent_frames: 0,
        last_wait_log_at: None,
        last_sent_at: None,
        last_stale_log_at: None,
    }))
}

fn publish_hid_controller_state_to_stream(
    stream: Option<&mut ActiveHidControllerStream>,
    record: &MuninnHidControllerStateRecord,
) {
    let Some(stream) = stream else {
        return;
    };
    if let Err(error) = publish_hid_controller_state_to_stream_inner(stream, record) {
        eprintln!(
            "Muninn could not publish HID controller {} to fast RUDP stream {}: {error:#}",
            record.device_id, stream.target
        );
    }
}

fn publish_hid_controller_state_to_stream_inner(
    stream: &mut ActiveHidControllerStream,
    record: &MuninnHidControllerStateRecord,
) -> Result<()> {
    if !stream.transport.connected() {
        if stream.connected_logged {
            eprintln!(
                "Muninn HID fast stream lost connection to {}; reconnecting",
                stream.target
            );
            stream.connected_logged = false;
        }
        let should_attempt = stream
            .last_connect_attempt_at
            .is_none_or(|attempt| attempt.elapsed() >= Duration::from_millis(250));
        if should_attempt {
            stream.last_connect_attempt_at = Some(Instant::now());
            eprintln!("Muninn HID fast stream connecting to {}", stream.target);
            stream.transport.connect(Vec::new())?;
        }
        for _ in 0..8 {
            if stream.transport.connected() {
                break;
            }
            let _ = stream.transport.receive_once()?;
            stream.transport.poll_resends()?;
        }
        if !stream.transport.connected() {
            let should_log_wait = stream
                .last_wait_log_at
                .is_none_or(|logged| logged.elapsed() >= Duration::from_secs(2));
            if should_log_wait {
                stream.last_wait_log_at = Some(Instant::now());
                eprintln!(
                    "Muninn HID fast stream waiting for RUDP accept from {}",
                    stream.target
                );
            }
            return Ok(());
        }
    }
    if !stream.connected_logged {
        eprintln!("Muninn HID fast stream connected to {}", stream.target);
        stream.connected_logged = true;
    }
    if stream.transport.check_timeout(2_000) {
        eprintln!(
            "Muninn HID fast stream timed out waiting for {}",
            stream.target
        );
        stream.connected_logged = false;
        return Ok(());
    }
    let mut frame = record.clone();
    if !frame.battery01.is_finite() {
        frame.battery01 = -1.0;
    }
    for axis in &mut frame.axes {
        if !axis.is_finite() {
            *axis = 0.0;
        }
    }
    let payload = serde_json::to_vec(&frame).context("encoding HID controller stream frame")?;
    stream.transport.send("latest", payload)?;
    stream.last_sent_at = Some(Instant::now());
    stream.last_stale_log_at = None;
    for _ in 0..4 {
        if stream.transport.receive_once()?.is_none() {
            break;
        }
    }
    stream.transport.poll_resends()?;
    stream.sent_frames = stream.sent_frames.saturating_add(1);
    if stream.sent_frames <= 5 || stream.sent_frames % 120 == 0 {
        eprintln!(
            "Muninn HID fast stream sent frame #{} device={} seq={} buttons=[{}]",
            stream.sent_frames,
            record.device_id,
            record.sequence,
            record.buttons.join(",")
        );
    }
    Ok(())
}

fn publish_hid_controller_state_to_odin(
    node: &mut cultmesh_rs::CultMeshNode,
    record: &MuninnHidControllerStateRecord,
) {
    if let Err(error) = node.put(&record.stream_id, record) {
        eprintln!(
            "Muninn could not store HID controller {} discovery record: {error:#}",
            record.device_id
        );
    }
}

#[cfg(windows)]
fn is_windows_ps_move_source(path: &str) -> bool {
    path.eq_ignore_ascii_case("windows-psmove")
        || path.eq_ignore_ascii_case("windows-psmove-col01")
        || path
            .to_ascii_lowercase()
            .starts_with(WINDOWS_PS_MOVE_SOURCE_PREFIX)
}

fn build_move_controller_state_record(
    options: &Options,
    source: &MoveStateSource,
    sequence: u64,
    report: &[u8],
    source_timestamp_ns: i64,
    observed_at: String,
) -> MuninnMoveControllerStateRecord {
    MuninnMoveControllerStateRecord {
        stream_id: format!(
            "{}:{}:move-controller-state",
            options.host_id, source.move_id
        ),
        host_id: options.host_id.clone(),
        move_id: source.move_id.clone(),
        sequence,
        source_timestamp_ns,
        accelerometer_xyz: vec![
            read_le_i16(report, 19) as f32,
            read_le_i16(report, 21) as f32,
            read_le_i16(report, 23) as f32,
        ],
        gyroscope_xyz: vec![
            read_le_i16(report, 25) as f32,
            read_le_i16(report, 27) as f32,
            read_le_i16(report, 29) as f32,
        ],
        magnetometer_xyz: vec![
            read_le_i16(report, 31) as f32,
            read_le_i16(report, 33) as f32,
            read_le_i16(report, 35) as f32,
        ],
        trigger_value: report.get(6).copied().unwrap_or_default() as f32 / 255.0,
        buttons: move_button_names(report),
        battery01: move_battery01(report.get(12).copied().unwrap_or_default()),
        observed_at,
        source_path: source.hidraw_path.clone(),
    }
}

fn build_move_controller_state_record_from_joystick(
    options: &Options,
    source: &MoveStateSource,
    sequence: u64,
    axes: [i16; 16],
    buttons: [bool; 32],
    source_timestamp_ns: i64,
    observed_at: String,
) -> MuninnMoveControllerStateRecord {
    MuninnMoveControllerStateRecord {
        stream_id: format!(
            "{}:{}:move-controller-state",
            options.host_id, source.move_id
        ),
        host_id: options.host_id.clone(),
        move_id: source.move_id.clone(),
        sequence,
        source_timestamp_ns,
        accelerometer_xyz: vec![axes[0] as f32, axes[1] as f32, axes[2] as f32],
        gyroscope_xyz: vec![axes[3] as f32, axes[4] as f32, axes[5] as f32],
        magnetometer_xyz: vec![axes[6] as f32, axes[7] as f32, axes[8] as f32],
        trigger_value: axis_to_unit(axes[2]),
        buttons: joystick_button_names(buttons),
        battery01: f32::NAN,
        observed_at,
        source_path: source.hidraw_path.clone(),
    }
}

fn build_hid_controller_state_record_from_joystick(
    options: &Options,
    source: &MoveStateSource,
    sequence: u64,
    axes: [i16; 16],
    buttons: [bool; 32],
    source_timestamp_ns: i64,
    observed_at: String,
) -> MuninnHidControllerStateRecord {
    MuninnHidControllerStateRecord {
        stream_id: format!(
            "{}:{}:hid-controller-state",
            options.host_id, source.move_id
        ),
        host_id: options.host_id.clone(),
        device_id: source.move_id.clone(),
        device_kind: hid_controller_kind_from_source(source),
        sequence,
        source_timestamp_ns,
        axes: axes.into_iter().map(axis_to_signed_unit).collect(),
        buttons: joystick_button_names(buttons),
        battery01: f32::NAN,
        observed_at,
        source_path: source.hidraw_path.clone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct XinputGamepadSnapshot {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
}

fn build_hid_controller_state_record_from_xinput_gamepad(
    options: &Options,
    source: &MoveStateSource,
    sequence: u64,
    gamepad: &XinputGamepadSnapshot,
    source_timestamp_ns: i64,
    observed_at: String,
) -> MuninnHidControllerStateRecord {
    MuninnHidControllerStateRecord {
        stream_id: format!(
            "{}:{}:hid-controller-state",
            options.host_id, source.move_id
        ),
        host_id: options.host_id.clone(),
        device_id: source.move_id.clone(),
        device_kind: "xinput-controller".to_string(),
        sequence,
        source_timestamp_ns,
        axes: vec![
            axis_to_signed_unit(gamepad.thumb_lx),
            axis_to_signed_unit(gamepad.thumb_ly),
            trigger_to_signed_unit(gamepad.left_trigger),
            axis_to_signed_unit(gamepad.thumb_rx),
            axis_to_signed_unit(gamepad.thumb_ry),
            trigger_to_signed_unit(gamepad.right_trigger),
        ],
        buttons: xinput_button_names(gamepad.buttons),
        battery01: f32::NAN,
        observed_at,
        source_path: source.hidraw_path.clone(),
    }
}

fn hid_controller_kind_from_source(source: &MoveStateSource) -> String {
    let haystack = format!(
        "{} {}",
        source.move_id.to_ascii_lowercase(),
        source.hidraw_path.to_ascii_lowercase()
    );
    if haystack.contains("nav") || haystack.contains("navigation") || haystack.contains("042f") {
        "ps3-navigation".to_string()
    } else if haystack.contains("xinput") || haystack.contains("xbox") {
        "xinput-controller".to_string()
    } else if haystack.contains("move") || haystack.contains("03d5") {
        "ps-move".to_string()
    } else {
        "generic-hid-controller".to_string()
    }
}

fn joystick_button_names(buttons: [bool; 32]) -> Vec<String> {
    [
        (0, "select"),
        (1, "l3"),
        (2, "r3"),
        (3, "start"),
        (4, "up"),
        (5, "right"),
        (6, "down"),
        (7, "left"),
        (8, "l2"),
        (9, "r2"),
        (10, "l1"),
        (11, "r1"),
        (12, "triangle"),
        (13, "circle"),
        (14, "cross"),
        (15, "square"),
        (16, "ps"),
        (17, "move"),
        (18, "trigger"),
    ]
    .iter()
    .filter_map(|(index, name)| {
        if buttons.get(*index).copied().unwrap_or(false) {
            Some((*name).to_string())
        } else {
            None
        }
    })
    .collect()
}

fn axis_to_unit(value: i16) -> f32 {
    ((value as f32 + 32768.0) / 65535.0).clamp(0.0, 1.0)
}

fn axis_to_signed_unit(value: i16) -> f32 {
    if value == i16::MIN {
        -1.0
    } else {
        (value as f32 / i16::MAX as f32).clamp(-1.0, 1.0)
    }
}

fn trigger_to_signed_unit(value: u8) -> f32 {
    ((value as f32 / 255.0) * 2.0 - 1.0).clamp(-1.0, 1.0)
}

fn xinput_button_names(buttons: u16) -> Vec<String> {
    [
        (XINPUT_GAMEPAD_DPAD_UP_MASK, "up"),
        (XINPUT_GAMEPAD_DPAD_DOWN_MASK, "down"),
        (XINPUT_GAMEPAD_DPAD_LEFT_MASK, "left"),
        (XINPUT_GAMEPAD_DPAD_RIGHT_MASK, "right"),
        (XINPUT_GAMEPAD_START_MASK, "start"),
        (XINPUT_GAMEPAD_BACK_MASK, "back"),
        (XINPUT_GAMEPAD_LEFT_THUMB_MASK, "l3"),
        (XINPUT_GAMEPAD_RIGHT_THUMB_MASK, "r3"),
        (XINPUT_GAMEPAD_LEFT_SHOULDER_MASK, "l1"),
        (XINPUT_GAMEPAD_RIGHT_SHOULDER_MASK, "r1"),
        (XINPUT_GAMEPAD_A_MASK, "a"),
        (XINPUT_GAMEPAD_B_MASK, "b"),
        (XINPUT_GAMEPAD_X_MASK, "x"),
        (XINPUT_GAMEPAD_Y_MASK, "y"),
    ]
    .iter()
    .filter_map(|(mask, name)| {
        if buttons & *mask != 0 {
            Some((*name).to_string())
        } else {
            None
        }
    })
    .collect()
}

#[cfg(windows)]
fn platform_xinput_gamepad(index: u32) -> Result<Option<XinputGamepadSnapshot>> {
    let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
    let result = unsafe { XInputGetState(index, &mut state) };
    if result != 0 {
        return Ok(None);
    }
    Ok(Some(xinput_snapshot_from_gamepad(state.Gamepad)))
}

#[cfg(not(windows))]
fn platform_xinput_gamepad(index: u32) -> Result<Option<XinputGamepadSnapshot>> {
    if index >= 4 {
        return Err(anyhow!(
            "XInput index {index} is outside the supported 0..3 range"
        ));
    }
    Ok(None)
}

#[cfg(windows)]
fn xinput_snapshot_from_gamepad(gamepad: XINPUT_GAMEPAD) -> XinputGamepadSnapshot {
    XinputGamepadSnapshot {
        buttons: gamepad.wButtons,
        left_trigger: gamepad.bLeftTrigger,
        right_trigger: gamepad.bRightTrigger,
        thumb_lx: gamepad.sThumbLX,
        thumb_ly: gamepad.sThumbLY,
        thumb_rx: gamepad.sThumbRX,
        thumb_ry: gamepad.sThumbRY,
    }
}

fn move_button_names(report: &[u8]) -> Vec<String> {
    let bits = report.get(1).copied().unwrap_or_default() as u32
        | ((report.get(2).copied().unwrap_or_default() as u32) << 8)
        | ((report.get(3).copied().unwrap_or_default() as u32) << 16);
    [
        (1 << 4, "triangle"),
        (1 << 5, "circle"),
        (1 << 6, "cross"),
        (1 << 7, "square"),
        (1 << 8, "select"),
        (1 << 11, "start"),
        (1 << 16, "ps"),
        (1 << 19, "move"),
        (1 << 20, "trigger"),
    ]
    .iter()
    .filter_map(|(mask, name)| {
        if bits & mask != 0 {
            Some((*name).to_string())
        } else {
            None
        }
    })
    .collect()
}

fn move_battery01(value: u8) -> f32 {
    match value {
        0x00..=0x05 => value as f32 / 5.0,
        0xee | 0xef => 1.0,
        _ => f32::NAN,
    }
}

fn read_le_i16(report: &[u8], offset: usize) -> i16 {
    let Some(lo) = report.get(offset).copied() else {
        return 0;
    };
    let hi = report.get(offset + 1).copied().unwrap_or_default();
    i16::from_le_bytes([lo, hi])
}

fn register_move_light_commands(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    active: &mut Vec<ActiveMoveLightCommand>,
) -> Result<()> {
    let commands = node.cache().get_all::<MuninnMoveLightCommandRecord>()?;
    for command in commands {
        if command.host_id != options.host_id {
            continue;
        }
        if command.state != "pending"
            && !(command.state == "running"
                && !active
                    .iter()
                    .any(|active| active.command.command_id == command.command_id))
        {
            continue;
        }

        let active_command = match prepare_move_light_command(command.clone()) {
            Ok(active_command) => active_command,
            Err(error) => {
                let command_id = command.command_id.clone();
                node.put(&command_id, &command_failed(command, &error.to_string())?)?;
                continue;
            }
        };
        let running = MuninnMoveLightCommandRecord {
            state: "running".to_string(),
            detail: "Muninn is refreshing local PS Move HID reports.".to_string(),
            updated_at: timestamp()?,
            ..command.clone()
        };
        node.put(&running.command_id, &running)?;
        active.push(ActiveMoveLightCommand {
            command: running,
            ..active_command
        });
    }
    Ok(())
}

fn prepare_move_light_command(
    command: MuninnMoveLightCommandRecord,
) -> Result<ActiveMoveLightCommand> {
    let colors = parse_move_colors(&command.colors)?;
    if command.hidraw_path.trim().is_empty() {
        return Err(anyhow!("hidraw_path is required"));
    }
    if command.repeat_count == 0 {
        return Err(anyhow!("repeat_count must be greater than zero"));
    }
    if command.repeat_count > 86_400 {
        return Err(anyhow!("repeat_count must be 86400 or less"));
    }
    if command.durations_ms.len() != colors.len() {
        return Err(anyhow!(
            "durations_ms must contain one duration for each color"
        ));
    }
    if command
        .durations_ms
        .iter()
        .any(|duration| *duration > 60_000)
    {
        return Err(anyhow!("durations_ms values must be 60000 or less"));
    }

    Ok(ActiveMoveLightCommand {
        command,
        colors,
        step_index: 0,
        repeats_done: 0,
        next_write_at: Instant::now(),
    })
}

fn tick_move_light_commands(
    node: &mut cultmesh_rs::CultMeshNode,
    active: &mut Vec<ActiveMoveLightCommand>,
    writer: &mut impl MoveLightWriter,
) -> Result<()> {
    let now = Instant::now();
    let mut index = 0;
    while index < active.len() {
        if active[index].next_write_at > now {
            index += 1;
            continue;
        }

        let command_id = active[index].command.command_id.clone();
        let (red, green, blue) = active[index].colors[active[index].step_index];
        let report = move_light_report(red, green, blue);
        if let Err(error) = writer.write_report(&active[index].command.hidraw_path, &report) {
            let failed = command_failed(active[index].command.clone(), &format!("{error:#}"))?;
            node.put(&command_id, &failed)?;
            active.remove(index);
            continue;
        }

        let duration_ms = active[index].command.durations_ms[active[index].step_index];
        active[index].step_index += 1;
        if active[index].step_index >= active[index].colors.len() {
            active[index].step_index = 0;
            active[index].repeats_done += 1;
        }

        if active[index].repeats_done >= active[index].command.repeat_count {
            let completed = MuninnMoveLightCommandRecord {
                state: "completed".to_string(),
                detail: format!(
                    "wrote {} PS Move light step(s) for {} repeat(s)",
                    active[index].colors.len(),
                    active[index].command.repeat_count
                ),
                updated_at: timestamp()?,
                ..active[index].command.clone()
            };
            node.put(&command_id, &completed)?;
            active.remove(index);
        } else {
            active[index].next_write_at =
                now + Duration::from_millis(u64::from(duration_ms.max(1)));
            index += 1;
        }
    }
    Ok(())
}

fn start_default_move_light_worker(
    options: &Options,
    suppressed_paths: Arc<Mutex<HashSet<String>>>,
    move_hue_program: Arc<Mutex<MuninnMoveHueProgramRecord>>,
) {
    if !serve_should_manage_platform_move_lights(options) {
        return;
    }

    let options = options.clone();
    thread::spawn(move || {
        let mut writer = HidMoveLightWriter;
        let mut last_error_log_at = None::<Instant>;
        let mut written_color_by_target = HashMap::<String, (u8, u8, u8)>::new();
        let mut targets = Vec::<DefaultMoveLightTarget>::new();
        let mut last_target_refresh_at = None::<Instant>;
        loop {
            if targets.is_empty()
                || last_target_refresh_at
                    .is_none_or(|refreshed_at| refreshed_at.elapsed() >= Duration::from_millis(500))
            {
                let states = active_move_state_sources(serve_move_state_sources(&options, true));
                targets = default_move_light_paths(&states, true);
                last_target_refresh_at = Some(Instant::now());
            }
            let mut roster = targets
                .iter()
                .map(|target| target.identity.clone())
                .collect::<Vec<_>>();
            roster.sort();
            roster.dedup();
            let suppressed = suppressed_paths
                .lock()
                .map(|paths| paths.clone())
                .unwrap_or_default();
            let now_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i128;
            let program = move_hue_program
                .lock()
                .map(|program| program.clone())
                .unwrap_or_else(|_| MuninnMoveHueProgramRecord {
                    program_id: move_hue_program_key(&options.host_id),
                    host_id: options.host_id.clone(),
                    mode: "static".to_string(),
                    cycle_ms: options.move_hue_cycle_ms,
                    epoch_ns: 0,
                    hold_at_ns: 0,
                    requested_by: "poisoned-runtime-fallback".to_string(),
                    updated_at: "unix-0".to_string(),
                    order_mode: "descending".to_string(),
                    transition_percent: 100,
                    transition_percent_explicit: true,
                });
            let program_timestamp_ns = move_hue_program_timestamp_ns(&program, now_ns);
            for target in &targets {
                if suppressed.contains(&target.path) {
                    continue;
                }
                let Some((color, _, _)) = scheduled_golden_move_color_with_order(
                    &target.identity,
                    &roster,
                    i128::from(program.epoch_ns),
                    i128::from(program.cycle_ms) * 1_000_000,
                    program_timestamp_ns,
                    &program.order_mode,
                    effective_transition_percent(&program),
                ) else {
                    continue;
                };
                let target_key = format!("{}:{}", target.identity, target.path);
                if written_color_by_target.get(&target_key) == Some(&color) {
                    continue;
                }
                let report = default_move_light_report(color);
                if let Err(error) = writer.write_report(&target.path, &report) {
                    let should_log = last_error_log_at
                        .is_none_or(|logged_at| logged_at.elapsed() >= Duration::from_secs(5));
                    if should_log {
                        eprintln!(
                            "Muninn default Move light refresh failed identity={} path={}: {error:#}",
                            target.identity, target.path
                        );
                        last_error_log_at = Some(Instant::now());
                    }
                } else {
                    written_color_by_target.insert(target_key, color);
                }
            }
            const MOVE_HUE_UPDATE_NS: u128 = 25_000_000;
            let current_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let until_boundary_ns = MOVE_HUE_UPDATE_NS - current_ns % MOVE_HUE_UPDATE_NS;
            thread::sleep(Duration::from_nanos(
                until_boundary_ns.min(u128::from(u64::MAX)) as u64,
            ));
        }
    });
}

fn update_suppressed_default_move_light_paths(
    suppressed_paths: &Arc<Mutex<HashSet<String>>>,
    active_commands: &[ActiveMoveLightCommand],
) {
    if let Ok(mut paths) = suppressed_paths.lock() {
        paths.clear();
        paths.extend(
            active_commands
                .iter()
                .map(|command| command.command.hidraw_path.clone()),
        );
    }
}

fn default_move_light_paths(
    states: &[ActiveMoveStateSource],
    include_platform_defaults: bool,
) -> Vec<DefaultMoveLightTarget> {
    let mut paths = Vec::new();
    for state in states {
        if let Some(path) = state.light_hidraw_path.as_ref() {
            push_unique_light_target(
                &mut paths,
                DefaultMoveLightTarget {
                    path: path.clone(),
                    identity: state.source.move_id.clone(),
                },
            );
        }
    }
    if include_platform_defaults {
        for path in platform_default_move_light_paths() {
            push_unique_light_target(&mut paths, path);
        }
    }
    paths
}

fn push_unique_light_target(
    paths: &mut Vec<DefaultMoveLightTarget>,
    target: DefaultMoveLightTarget,
) {
    if !paths.iter().any(|existing| existing.path == target.path) {
        paths.push(target);
    }
}

fn default_move_color_for_identity(identity: &str) -> (u8, u8, u8) {
    let hue = (stable_u64_hash(identity) % 360) as f64;
    hsv_to_rgb(hue, 0.82, 1.0)
}

fn golden_move_color_for_roster(identity: &str, roster: &[String]) -> (u8, u8, u8) {
    const GOLDEN_RATIO_CONJUGATE: f64 = 0.618_033_988_749_894_9;
    let Some(index) = roster.iter().position(|candidate| candidate == identity) else {
        return default_move_color_for_identity(identity);
    };
    let hue = ((index as f64 * GOLDEN_RATIO_CONJUGATE).fract() * 360.0 + 15.0) % 360.0;
    hsv_to_rgb(hue, 1.0, 1.0)
}

fn scheduled_golden_move_color(
    identity: &str,
    roster: &[String],
    epoch_ns: i128,
    cycle_ns: i128,
    timestamp_ns: i128,
) -> Option<((u8, u8, u8), i128, bool)> {
    scheduled_golden_move_color_with_order(
        identity,
        roster,
        epoch_ns,
        cycle_ns,
        timestamp_ns,
        "descending",
        100,
    )
}

fn scheduled_golden_move_color_with_order(
    identity: &str,
    roster: &[String],
    epoch_ns: i128,
    cycle_ns: i128,
    timestamp_ns: i128,
    order_mode: &str,
    transition_percent: u8,
) -> Option<((u8, u8, u8), i128, bool)> {
    const GOLDEN_RATIO_CONJUGATE: f64 = 0.618_033_988_749_894_9;
    if roster.is_empty() || cycle_ns <= 0 {
        return None;
    }
    let identity_index = roster.iter().position(|candidate| candidate == identity)? as i128;
    let roster_len = roster.len() as i128;
    let elapsed_ns = (timestamp_ns - epoch_ns).max(0);
    let completed_steps = elapsed_ns.saturating_mul(roster_len).div_euclid(cycle_ns);
    let completed_cycles = completed_steps.div_euclid(roster_len);
    let completed_in_cycle = completed_steps.rem_euclid(roster_len);
    let update_order = move_hue_update_order(roster.len(), completed_cycles, order_mode);
    let order_position = update_order.iter().position(|index| *index == identity_index as usize)? as i128;
    let advanced_in_cycle = i128::from(order_position < completed_in_cycle);
    let sequence_index = identity_index + completed_cycles + advanced_in_cycle;
    let source_hue = (sequence_index as f64 * GOLDEN_RATIO_CONJUGATE).fract();
    let hue = if order_position == completed_in_cycle {
        let subslot_ns = cycle_ns.div_euclid(roster_len).max(1);
        let subslot_elapsed_ns = elapsed_ns.rem_euclid(subslot_ns);
        let amount = if transition_percent == 0 {
            1.0
        } else {
            let transition_ns = subslot_ns.saturating_mul(i128::from(transition_percent.min(100))).div_euclid(100).max(1);
            smootherstep((subslot_elapsed_ns as f64 / transition_ns as f64).min(1.0))
        };
        let target_hue = ((sequence_index + 1) as f64 * GOLDEN_RATIO_CONJUGATE).fract();
        wrapped_hue_lerp(source_hue, target_hue, amount)
    } else {
        source_hue
    };
    Some((
        hsv_to_rgb(hue * 360.0, 1.0, 1.0),
        sequence_index,
        order_position == completed_in_cycle,
    ))
}

fn move_hue_update_order(move_count: usize, cycle: i128, mode: &str) -> Vec<usize> {
    let mut order = (0..move_count).collect::<Vec<_>>();
    match mode {
        "ascending" => order,
        "bounce" if cycle.rem_euclid(2) == 1 => order,
        "rotating-lead" => {
            order.reverse();
            if !order.is_empty() {
                let rotation = cycle.rem_euclid(order.len() as i128) as usize;
                order.rotate_left(rotation);
            }
            order
        }
        "golden-permutation" => {
            const GOLDEN_RATIO_CONJUGATE: f64 = 0.618_033_988_749_894_9;
            order.sort_by(|left, right| {
                let left_phase = (((cycle * move_count as i128 + *left as i128) as f64)
                    * GOLDEN_RATIO_CONJUGATE)
                    .fract();
                let right_phase = (((cycle * move_count as i128 + *right as i128) as f64)
                    * GOLDEN_RATIO_CONJUGATE)
                    .fract();
                left_phase.total_cmp(&right_phase)
            });
            order
        }
        _ => {
            order.reverse();
            order
        }
    }
}

fn hsv_to_rgb(hue_degrees: f64, saturation: f64, value: f64) -> (u8, u8, u8) {
    let chroma = value * saturation;
    let hue_sector = (hue_degrees.rem_euclid(360.0)) / 60.0;
    let secondary = chroma * (1.0 - ((hue_sector % 2.0) - 1.0).abs());
    let (red, green, blue) = match hue_sector.floor() as u8 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let floor = value - chroma;
    (
        ((red + floor) * 255.0).round() as u8,
        ((green + floor) * 255.0).round() as u8,
        ((blue + floor) * 255.0).round() as u8,
    )
}

fn stable_u64_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    if hash == 0 { 1 } else { hash }
}

fn default_move_light_report(color: (u8, u8, u8)) -> [u8; PS_MOVE_LED_REPORT_LEN] {
    move_light_report(color.0, color.1, color.2)
}

fn move_light_report(red: u8, green: u8, blue: u8) -> [u8; PS_MOVE_LED_REPORT_LEN] {
    let mut report = [0u8; PS_MOVE_LED_REPORT_LEN];
    report[0] = 0x06;
    report[2] = red;
    report[3] = green;
    report[4] = blue;
    report
}

fn default_move_light_path(source_path: &str) -> Option<String> {
    if source_path.contains("/dev/hidraw") {
        return Some(source_path.to_string());
    }
    if is_joystick_path(source_path) {
        return joystick_light_hidraw_path(source_path);
    }
    None
}

#[cfg(unix)]
fn joystick_light_hidraw_path(joystick_path: &str) -> Option<String> {
    let joystick_name = Path::new(joystick_path).file_name()?.to_str()?;
    let mut cursor = fs::canonicalize(format!("/sys/class/input/{joystick_name}/device")).ok()?;
    for _ in 0..12 {
        let hidraw_dir = cursor.join("hidraw");
        if let Ok(entries) = fs::read_dir(hidraw_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_str()?;
                if name.starts_with("hidraw") {
                    return Some(format!("/dev/{name}"));
                }
            }
        }
        if !cursor.pop() {
            break;
        }
    }
    None
}

#[cfg(unix)]
fn platform_move_state_sources() -> Vec<MoveStateSource> {
    let mut candidates: Vec<(MoveStateSource, u8)> = Vec::new();
    let Ok(entries) = fs::read_dir("/dev/input") else {
        return Vec::new();
    };

    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name.starts_with("js") {
            continue;
        }
        let joystick_path = format!("/dev/input/{name}");
        let Some(sysfs_device) = fs::canonicalize(format!("/sys/class/input/{name}/device")).ok()
        else {
            continue;
        };
        let uevent = read_move_uevent_chain(&sysfs_device);
        let input_name = read_sysfs_input_name(&sysfs_device);
        if !is_muninn_hid_controller_uevent(&uevent, input_name.as_deref()) {
            continue;
        }

        let hidraw_path = joystick_light_hidraw_path(&joystick_path);
        let move_id = hid_controller_unique_id_from_uevent(&uevent, input_name.as_deref())
            .or_else(|| {
                hidraw_path
                    .as_deref()
                    .and_then(controller_id_from_hidraw)
                    .map(|id| format!("move-{id}"))
            })
            .unwrap_or_else(|| format!("hid-{name}"));
        let score = if uevent.contains("HID_ID=0005:0000054C:000003D5") {
            2
        } else {
            1
        };

        candidates.push((
            MoveStateSource {
                move_id,
                hidraw_path: joystick_path,
            },
            score,
        ));
    }

    let mut selected: Vec<(MoveStateSource, u8)> = Vec::new();
    for candidate in candidates {
        if let Some(existing) = selected
            .iter_mut()
            .find(|existing| existing.0.move_id == candidate.0.move_id)
        {
            if candidate.1 >= existing.1 {
                *existing = candidate;
            }
            continue;
        }
        selected.push(candidate);
    }
    selected.into_iter().map(|(source, _)| source).collect()
}

#[cfg(windows)]
const WINDOWS_PS_MOVE_SOURCE_PREFIX: &str = "windows-psmove:";

#[cfg(windows)]
fn platform_move_state_sources() -> Vec<MoveStateSource> {
    let targets = windows_ps_move_state_paths().unwrap_or_default();
    let mut sources = targets
        .into_iter()
        .enumerate()
        .map(|(index, target)| {
            let path_lower = target.path.to_ascii_lowercase();
            let move_id = if target.identity.starts_with("move-")
                || target.identity.starts_with("nav-")
            {
                target.identity
            } else if path_lower.contains("pid_042f") || path_lower.contains("vid_054c&pid_042f") {
                format!("nav-windows-psnav-{index}")
            } else {
                format!("move-windows-psmove-{index}")
            };
            MoveStateSource {
                move_id,
                hidraw_path: windows_ps_move_source_token(&target.path),
            }
        })
        .collect::<Vec<_>>();
    for index in 0..4 {
        if matches!(platform_xinput_gamepad(index), Ok(Some(_))) {
            let source = MoveStateSource {
                move_id: format!("xbox-xinput-{index}"),
                hidraw_path: format!("{WINDOWS_XINPUT_SOURCE_PREFIX}{index}"),
            };
            if !sources.iter().any(|existing| {
                existing
                    .hidraw_path
                    .eq_ignore_ascii_case(&source.hidraw_path)
            }) {
                sources.push(source);
            }
        }
    }
    sources
}

#[cfg(windows)]
fn windows_ps_move_source_token(path: &str) -> String {
    format!("{WINDOWS_PS_MOVE_SOURCE_PREFIX}{path}")
}

#[cfg(not(unix))]
#[cfg(not(windows))]
fn platform_move_state_sources() -> Vec<MoveStateSource> {
    Vec::new()
}

#[cfg(unix)]
fn read_move_uevent_chain(start: &Path) -> String {
    let mut cursor = start.to_path_buf();
    let mut text = String::new();
    for _ in 0..6 {
        if let Ok(uevent) = fs::read_to_string(cursor.join("uevent")) {
            text.push_str(&uevent);
            text.push('\n');
        }
        if !cursor.pop() {
            break;
        }
    }
    text
}

#[cfg(unix)]
fn read_sysfs_input_name(sysfs_device: &Path) -> Option<String> {
    fs::read_to_string(sysfs_device.join("name"))
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

#[cfg(unix)]
fn is_muninn_hid_controller_uevent(uevent: &str, input_name: Option<&str>) -> bool {
    is_ps_move_uevent(uevent) || is_ps3_navigation_uevent(uevent, input_name)
}

#[cfg(unix)]
fn is_ps_move_uevent(uevent: &str) -> bool {
    uevent.contains("ID_VENDOR_ID=054c") && uevent.contains("ID_MODEL_ID=03d5")
        || uevent.contains("ID_MODEL=Motion_Controller")
        || uevent.contains("HID_ID=0005:0000054C:000003D5")
        || uevent.contains("HID_ID=0003:0000054C:000003D5")
}

#[cfg(unix)]
fn is_ps3_navigation_uevent(uevent: &str, input_name: Option<&str>) -> bool {
    uevent.contains("ID_VENDOR_ID=054c") && uevent.contains("ID_MODEL_ID=042f")
        || uevent.contains("HID_ID=0005:0000054C:0000042F")
        || uevent.contains("HID_ID=0003:0000054C:0000042F")
        || input_name.is_some_and(|name| {
            name.to_ascii_lowercase()
                .contains("sony navigation controller")
        })
}

#[cfg(unix)]
fn hid_controller_unique_id_from_uevent(uevent: &str, input_name: Option<&str>) -> Option<String> {
    value_from_uevent(uevent, "HID_UNIQ")
        .map(|value| value.replace(':', ""))
        .filter(|value| !value.is_empty())
        .map(|value| {
            if is_ps3_navigation_uevent(uevent, input_name) {
                format!("nav-{value}")
            } else {
                format!("move-{value}")
            }
        })
}

#[cfg(unix)]
fn value_from_uevent(uevent: &str, key: &str) -> Option<String> {
    uevent.lines().find_map(|line| {
        let (candidate_key, value) = line.split_once('=')?;
        (candidate_key == key).then(|| value.trim().to_string())
    })
}

#[cfg(unix)]
fn controller_id_from_hidraw(hidraw_path: &str) -> Option<String> {
    let device = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(hidraw_path)
        .ok()?;
    let mut report = [0u8; 16];
    report[0] = 4;
    let request = hid_iocgfeature(report.len());
    let ok = unsafe { libc::ioctl(device.as_raw_fd(), request, report.as_mut_ptr()) };
    if ok < 0 {
        return None;
    }
    Some(
        report[1..7]
            .iter()
            .rev()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

#[cfg(unix)]
fn hid_iocgfeature(length: usize) -> libc::c_ulong {
    const IOC_READ: libc::c_ulong = 2;
    const IOC_WRITE: libc::c_ulong = 1;
    const IOC_NRBITS: libc::c_ulong = 8;
    const IOC_TYPEBITS: libc::c_ulong = 8;
    const IOC_SIZEBITS: libc::c_ulong = 14;
    const IOC_NRSHIFT: libc::c_ulong = 0;
    const IOC_TYPESHIFT: libc::c_ulong = IOC_NRSHIFT + IOC_NRBITS;
    const IOC_SIZESHIFT: libc::c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: libc::c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;
    ((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
        | ((length as libc::c_ulong) << IOC_SIZESHIFT)
        | ((b'H' as libc::c_ulong) << IOC_TYPESHIFT)
        | (0x07 << IOC_NRSHIFT)
}

#[cfg(unix)]
fn hid_iocsfeature(length: usize) -> libc::c_ulong {
    const IOC_READ: libc::c_ulong = 2;
    const IOC_WRITE: libc::c_ulong = 1;
    const IOC_NRBITS: libc::c_ulong = 8;
    const IOC_TYPEBITS: libc::c_ulong = 8;
    const IOC_SIZEBITS: libc::c_ulong = 14;
    const IOC_NRSHIFT: libc::c_ulong = 0;
    const IOC_TYPESHIFT: libc::c_ulong = IOC_NRSHIFT + IOC_NRBITS;
    const IOC_SIZESHIFT: libc::c_ulong = IOC_TYPESHIFT + IOC_TYPEBITS;
    const IOC_DIRSHIFT: libc::c_ulong = IOC_SIZESHIFT + IOC_SIZEBITS;
    ((IOC_READ | IOC_WRITE) << IOC_DIRSHIFT)
        | ((length as libc::c_ulong) << IOC_SIZESHIFT)
        | ((b'H' as libc::c_ulong) << IOC_TYPESHIFT)
        | (0x06 << IOC_NRSHIFT)
}

#[cfg(unix)]
fn platform_default_bluetooth_host_address() -> Option<String> {
    let entries = fs::read_dir("/sys/class/bluetooth").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with("hci") {
            continue;
        }
        let Ok(address) = fs::read_to_string(entry.path().join("address")) else {
            continue;
        };
        let address = address.trim();
        if !address.is_empty() {
            return Some(address.to_string());
        }
    }
    bluetoothctl_host_address()
}

#[cfg(unix)]
fn bluetoothctl_host_address() -> Option<String> {
    let output = Command::new("bluetoothctl").arg("show").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(|line| {
        let trimmed = line.trim();
        let address = trimmed
            .strip_prefix("Controller ")?
            .split_whitespace()
            .next()?;
        parse_bluetooth_address_little_endian(address)
            .ok()
            .map(|_| address.to_string())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BluetoothMoveDevice {
    address: String,
    trusted: bool,
    connected: bool,
}

#[cfg(unix)]
fn bluetoothctl_motion_controller_devices() -> Result<Vec<BluetoothMoveDevice>> {
    let output = Command::new("bluetoothctl")
        .arg("devices")
        .output()
        .context("bluetoothctl devices could not run")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("bluetoothctl devices failed: {}", stderr.trim()));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();
    for line in stdout.lines() {
        let Some(address) = parse_bluetoothctl_motion_controller_device_line(line) else {
            continue;
        };
        if let Some(device) = bluetoothctl_device_info(&address) {
            devices.push(device);
        }
    }
    Ok(devices)
}

fn parse_bluetoothctl_motion_controller_device_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix("Device ")?;
    let mut parts = rest.split_whitespace();
    let address = parts.next()?;
    let name = parts.collect::<Vec<_>>().join(" ");
    if !name.eq_ignore_ascii_case("Motion Controller") {
        return None;
    }
    parse_bluetooth_address_little_endian(address)
        .ok()
        .map(|_| address.to_string())
}

#[cfg(unix)]
fn bluetoothctl_device_info(address: &str) -> Option<BluetoothMoveDevice> {
    let output = Command::new("bluetoothctl")
        .args(["info", address])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_bluetoothctl_device_info(
        address,
        &String::from_utf8_lossy(&output.stdout),
    ))
}

fn parse_bluetoothctl_device_info(address: &str, text: &str) -> BluetoothMoveDevice {
    BluetoothMoveDevice {
        address: address.to_string(),
        trusted: bluetoothctl_info_flag(text, "Trusted"),
        connected: bluetoothctl_info_flag(text, "Connected"),
    }
}

fn bluetoothctl_info_flag(text: &str, name: &str) -> bool {
    let prefix = format!("{name}:");
    text.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(&prefix)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("yes"))
    })
}

#[cfg(unix)]
fn bluetoothctl_connect_device(address: &str) -> Result<bool> {
    let mut child = Command::new("bluetoothctl")
        .args(["connect", address])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("bluetoothctl connect {address} could not start"))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("waiting for bluetoothctl connect {address}"))?
        {
            return Ok(status.success());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(unix))]
fn platform_default_bluetooth_host_address() -> Option<String> {
    None
}

#[cfg(unix)]
fn unix_usb_move_hidraw_paths() -> Vec<String> {
    let mut paths = Vec::new();
    let Ok(entries) = fs::read_dir("/sys/class/hidraw") else {
        return paths;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Ok(uevent) = fs::read_to_string(entry.path().join("device").join("uevent")) else {
            continue;
        };
        if uevent.contains("HID_ID=0003:0000054C:000003D5") {
            paths.push(format!("/dev/{name}"));
        }
    }
    paths.sort();
    paths
}

#[cfg(unix)]
fn unix_claim_ps_move_host(host: &[u8; 6]) -> Result<usize> {
    let mut claims = 0usize;
    let mut hard_errors = Vec::new();
    for path in unix_usb_move_hidraw_paths() {
        let device = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(device) => device,
            Err(error) => {
                hard_errors.push(format!("{path}: {error}"));
                continue;
            }
        };

        let before = controller_and_host_from_hidraw_device(&device);
        let mut report = [0u8; 23];
        report[0] = 0x05;
        report[1..7].copy_from_slice(host);
        let request = hid_iocsfeature(report.len());
        let ok = unsafe { libc::ioctl(device.as_raw_fd(), request, report.as_mut_ptr()) };
        if ok < 0 {
            eprintln!(
                "path={path} skipped error={}",
                std::io::Error::last_os_error()
            );
            continue;
        }
        let after = controller_and_host_from_hidraw_device(&device);
        let controller = after
            .as_ref()
            .or(before.as_ref())
            .map(|addresses| addresses.0.as_str())
            .unwrap_or("(unknown)");
        let host_before = before
            .as_ref()
            .map(|addresses| addresses.1.as_str())
            .unwrap_or("(unknown)");
        let host_after = after
            .as_ref()
            .map(|addresses| addresses.1.as_str())
            .unwrap_or("(unknown)");
        println!(
            "path={path} controller={controller} host_before={host_before} host_after={host_after}"
        );
        claims += 1;
    }

    if claims == 0 && !hard_errors.is_empty() {
        return Err(anyhow!(
            "no USB PS Move host claims succeeded: {}",
            hard_errors.join("; ")
        ));
    }
    Ok(claims)
}

#[cfg(unix)]
fn controller_and_host_from_hidraw_device(device: &fs::File) -> Option<(String, String)> {
    let mut report = [0u8; 16];
    report[0] = 4;
    let request = hid_iocgfeature(report.len());
    let ok = unsafe { libc::ioctl(device.as_raw_fd(), request, report.as_mut_ptr()) };
    if ok < 0 {
        return None;
    }
    Some((
        format_bluetooth_address_little_endian(&report[1..7]),
        format_bluetooth_address_little_endian(&report[10..16]),
    ))
}

#[cfg(not(unix))]
fn joystick_light_hidraw_path(_joystick_path: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn platform_default_move_light_paths() -> Vec<DefaultMoveLightTarget> {
    Vec::new()
}

#[cfg(windows)]
fn platform_default_move_light_paths() -> Vec<DefaultMoveLightTarget> {
    windows_ps_move_light_paths().unwrap_or_default()
}

#[cfg(windows)]
fn windows_ps_move_light_paths() -> Result<Vec<DefaultMoveLightTarget>> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData,
        HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let mut hid_guid = unsafe { std::mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut hid_guid) };
    let info_set = unsafe {
        SetupDiGetClassDevsW(
            &hid_guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if info_set == INVALID_HANDLE_VALUE as isize {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    let mut index = 0;
    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            InterfaceClassGuid: unsafe { std::mem::zeroed() },
            Flags: 0,
            Reserved: 0,
        };
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                info_set,
                std::ptr::null_mut(),
                &hid_guid,
                index,
                &mut interface_data,
            )
        };
        if ok == 0 {
            break;
        }
        index += 1;

        let Some(path) = (unsafe { windows_hid_interface_path(info_set, &mut interface_data) })
        else {
            continue;
        };
        if !path.to_ascii_lowercase().contains("&col01#") {
            continue;
        }

        let wide = wide_null(&path);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }

        let mut attributes = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        let mut preparsed = 0;
        let is_move_light = unsafe {
            HidD_GetAttributes(handle, &mut attributes) != 0
                && attributes.VendorID == 0x054c
                && attributes.ProductID == 0x03d5
                && HidD_GetPreparsedData(handle, &mut preparsed) != 0
        };
        if is_move_light {
            let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
            let caps_ok = unsafe { HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS };
            if caps_ok && caps.OutputReportByteLength > 0 {
                let identity = windows_ps_move_controller_identity(handle)
                    .map(|id| format!("move-{id}"))
                    .unwrap_or_else(|| path.clone());
                paths.push(DefaultMoveLightTarget { path, identity });
            }
        }
        if preparsed != 0 {
            unsafe { HidD_FreePreparsedData(preparsed) };
        }
        unsafe { CloseHandle(handle) };
    }

    unsafe { SetupDiDestroyDeviceInfoList(info_set) };
    paths.sort_by(|a, b| a.identity.cmp(&b.identity).then(a.path.cmp(&b.path)));
    Ok(paths)
}

#[cfg(windows)]
fn windows_ps_move_state_paths() -> Result<Vec<DefaultMoveLightTarget>> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData,
        HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let sibling_identities = windows_ps_move_identity_by_physical_key()?;
    let mut hid_guid = unsafe { std::mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut hid_guid) };
    let info_set = unsafe {
        SetupDiGetClassDevsW(
            &hid_guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if info_set == INVALID_HANDLE_VALUE as isize {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    let mut index = 0;
    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            InterfaceClassGuid: unsafe { std::mem::zeroed() },
            Flags: 0,
            Reserved: 0,
        };
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                info_set,
                std::ptr::null_mut(),
                &hid_guid,
                index,
                &mut interface_data,
            )
        };
        if ok == 0 {
            break;
        }
        index += 1;

        let Some(path) = (unsafe { windows_hid_interface_path(info_set, &mut interface_data) })
        else {
            continue;
        };
        let wide = wide_null(&path);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }

        let mut attributes = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        let mut preparsed = 0;
        let is_move = unsafe {
            HidD_GetAttributes(handle, &mut attributes) != 0
                && attributes.VendorID == 0x054c
                && matches!(attributes.ProductID, 0x03d5 | 0x042f)
                && HidD_GetPreparsedData(handle, &mut preparsed) != 0
        };
        if is_move {
            let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
            let caps_ok = unsafe { HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS };
            if caps_ok && caps.InputReportByteLength > 0 {
                let identity = windows_ps_move_controller_identity(handle)
                    .or_else(|| {
                        windows_ps_move_physical_key(&path)
                            .and_then(|key| sibling_identities.get(&key).cloned())
                    })
                    .map(|id| {
                        if attributes.ProductID == 0x042f {
                            format!("nav-{id}")
                        } else {
                            format!("move-{id}")
                        }
                    })
                    .unwrap_or_else(|| path.clone());
                paths.push(DefaultMoveLightTarget { path, identity });
            }
        }
        if preparsed != 0 {
            unsafe { HidD_FreePreparsedData(preparsed) };
        }
        unsafe { CloseHandle(handle) };
    }

    unsafe { SetupDiDestroyDeviceInfoList(info_set) };
    paths.sort_by(|a, b| a.identity.cmp(&b.identity).then(a.path.cmp(&b.path)));
    Ok(paths)
}

#[cfg(windows)]
fn windows_ps_move_identity_by_physical_key() -> Result<std::collections::HashMap<String, String>> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetHidGuid,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let mut hid_guid = unsafe { std::mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut hid_guid) };
    let info_set = unsafe {
        SetupDiGetClassDevsW(
            &hid_guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if info_set == INVALID_HANDLE_VALUE as isize {
        return Ok(std::collections::HashMap::new());
    }

    let mut identities = std::collections::HashMap::new();
    let mut index = 0;
    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            InterfaceClassGuid: unsafe { std::mem::zeroed() },
            Flags: 0,
            Reserved: 0,
        };
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                info_set,
                std::ptr::null_mut(),
                &hid_guid,
                index,
                &mut interface_data,
            )
        };
        if ok == 0 {
            break;
        }
        index += 1;

        let Some(path) = (unsafe { windows_hid_interface_path(info_set, &mut interface_data) })
        else {
            continue;
        };
        let Some(key) = windows_ps_move_physical_key(&path) else {
            continue;
        };
        let wide = wide_null(&path);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            continue;
        }

        let mut attributes = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        let is_move = unsafe {
            HidD_GetAttributes(handle, &mut attributes) != 0
                && attributes.VendorID == 0x054c
                && attributes.ProductID == 0x03d5
        };
        if is_move {
            if let Some(identity) = windows_ps_move_controller_identity(handle) {
                identities.insert(key, identity);
            }
        }
        unsafe { CloseHandle(handle) };
    }

    unsafe { SetupDiDestroyDeviceInfoList(info_set) };
    Ok(identities)
}

#[cfg(windows)]
fn windows_ps_move_physical_key(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase();
    if !lower.contains("vid_054c&pid_03d5") {
        return None;
    }
    let (before_guid, _) = lower.split_once("#{").unwrap_or((lower.as_str(), ""));
    let without_collection = before_guid
        .replace("&col01", "")
        .replace("&col02", "")
        .replace("&col03", "")
        .replace("&col04", "");
    let Some((prefix, suffix)) = without_collection.rsplit_once('&') else {
        return Some(without_collection);
    };
    if suffix.len() == 4
        && suffix
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Some(prefix.to_string());
    }
    Some(without_collection)
}

#[cfg(windows)]
fn windows_ps_move_controller_identity(handle: *mut std::ffi::c_void) -> Option<String> {
    use windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_GetFeature;

    let mut report = [0u8; 16];
    report[0] = 0x04;
    let ok = unsafe { HidD_GetFeature(handle, report.as_mut_ptr().cast(), report.len() as u32) };
    if ok == 0 {
        return None;
    }
    Some(
        report[1..7]
            .iter()
            .rev()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
}

fn parse_bluetooth_address_little_endian(value: &str) -> Result<[u8; 6]> {
    let parts = value.split([':', '-']).collect::<Vec<_>>();
    if parts.len() != 6 {
        return Err(anyhow!("Bluetooth address must have six hex bytes"));
    }
    let mut address = [0u8; 6];
    for (index, part) in parts.iter().enumerate() {
        let byte = u8::from_str_radix(part, 16)
            .with_context(|| format!("parsing Bluetooth address byte {part}"))?;
        address[5 - index] = byte;
    }
    Ok(address)
}

fn format_bluetooth_address_little_endian(address: &[u8]) -> String {
    address
        .iter()
        .rev()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(windows)]
fn windows_ps_move_bluetooth_addresses(handle: *mut std::ffi::c_void) -> Option<(String, String)> {
    use windows_sys::Win32::Devices::HumanInterfaceDevice::HidD_GetFeature;

    let mut report = [0u8; 16];
    report[0] = 0x04;
    let ok = unsafe { HidD_GetFeature(handle, report.as_mut_ptr().cast(), report.len() as u32) };
    if ok == 0 {
        return None;
    }
    Some((
        format_bluetooth_address_little_endian(&report[1..7]),
        format_bluetooth_address_little_endian(&report[10..16]),
    ))
}

#[cfg(windows)]
fn windows_claim_ps_move_host(host: &[u8; 6]) -> Result<usize> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    };
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData,
        HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData, HidD_SetFeature, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let mut hid_guid = unsafe { std::mem::zeroed() };
    unsafe { HidD_GetHidGuid(&mut hid_guid) };
    let info_set = unsafe {
        SetupDiGetClassDevsW(
            &hid_guid,
            std::ptr::null(),
            std::ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if info_set == INVALID_HANDLE_VALUE as isize {
        return Err(std::io::Error::last_os_error()).context("enumerating Windows HID devices");
    }

    let mut claims = 0usize;
    let mut index = 0;
    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            InterfaceClassGuid: unsafe { std::mem::zeroed() },
            Flags: 0,
            Reserved: 0,
        };
        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(
                info_set,
                std::ptr::null_mut(),
                &hid_guid,
                index,
                &mut interface_data,
            )
        };
        if ok == 0 {
            break;
        }
        index += 1;

        let Some(path) = (unsafe { windows_hid_interface_path(info_set, &mut interface_data) })
        else {
            continue;
        };
        if !path.to_ascii_lowercase().contains("&col02#") {
            continue;
        }

        let wide = wide_null(&path);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            eprintln!(
                "path={} open=failed error={}",
                path,
                std::io::Error::last_os_error()
            );
            continue;
        }

        let mut attributes = HIDD_ATTRIBUTES {
            Size: std::mem::size_of::<HIDD_ATTRIBUTES>() as u32,
            VendorID: 0,
            ProductID: 0,
            VersionNumber: 0,
        };
        let mut preparsed = 0;
        let is_move = unsafe {
            HidD_GetAttributes(handle, &mut attributes) != 0
                && attributes.VendorID == 0x054c
                && attributes.ProductID == 0x03d5
                && HidD_GetPreparsedData(handle, &mut preparsed) != 0
        };
        if !is_move {
            unsafe { CloseHandle(handle) };
            continue;
        }

        let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
        let caps_ok = unsafe { HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS };
        unsafe { HidD_FreePreparsedData(preparsed) };
        if !caps_ok || caps.FeatureReportByteLength < 23 {
            unsafe { CloseHandle(handle) };
            continue;
        }

        let before = windows_ps_move_bluetooth_addresses(handle);
        let mut report = [0u8; 23];
        report[0] = 0x05;
        report[1..7].copy_from_slice(host);
        let ok =
            unsafe { HidD_SetFeature(handle, report.as_mut_ptr().cast(), report.len() as u32) };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            eprintln!("path={} set_feature=failed error={}", path, error);
            continue;
        }
        let after = windows_ps_move_bluetooth_addresses(handle);
        unsafe { CloseHandle(handle) };

        let controller = after
            .as_ref()
            .or(before.as_ref())
            .map(|addresses| addresses.0.as_str())
            .unwrap_or("(unknown)");
        let host_before = before
            .as_ref()
            .map(|addresses| addresses.1.as_str())
            .unwrap_or("(unknown)");
        let host_after = after
            .as_ref()
            .map(|addresses| addresses.1.as_str())
            .unwrap_or("(unknown)");
        println!(
            "path={} controller={} host_before={} host_after={}",
            path, controller, host_before, host_after
        );
        claims += 1;
    }

    unsafe { SetupDiDestroyDeviceInfoList(info_set) };
    Ok(claims)
}

#[cfg(windows)]
fn windows_ps_move_input_report(source: &str) -> Result<Option<Vec<u8>>> {
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetInputReport,
        HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let Some(path) = windows_ps_move_input_path(source)? else {
        return Ok(None);
    };
    let wide = wide_null(&path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("opening Windows PS Move input HID path {path}"));
    }

    let mut preparsed = 0;
    if unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } == 0 {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(handle) };
        return Err(error).with_context(|| format!("reading HID preparsed data from {path}"));
    }

    let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
    let caps_ok = unsafe { HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS };
    unsafe { HidD_FreePreparsedData(preparsed) };
    if !caps_ok || caps.InputReportByteLength == 0 {
        unsafe { CloseHandle(handle) };
        return Ok(None);
    }

    let interrupt_report =
        windows_read_hid_interrupt_report(handle, caps.InputReportByteLength as usize, &path)?;
    if interrupt_report.is_some() {
        unsafe { CloseHandle(handle) };
        return Ok(interrupt_report);
    }

    let mut report = vec![0u8; caps.InputReportByteLength as usize];
    report[0] = 0x01;
    let ok =
        unsafe { HidD_GetInputReport(handle, report.as_mut_ptr().cast(), report.len() as u32) };
    unsafe { CloseHandle(handle) };
    if ok != 0 {
        return Ok(Some(report));
    }
    Ok(None)
}

#[cfg(windows)]
fn windows_ps_move_input_path(source: &str) -> Result<Option<String>> {
    if source.eq_ignore_ascii_case("windows-psmove")
        || source.eq_ignore_ascii_case("windows-psmove-col01")
    {
        return Ok(windows_ps_move_light_paths()?
            .into_iter()
            .next()
            .map(|target| target.path));
    }
    let Some(path) = source
        .to_ascii_lowercase()
        .strip_prefix(WINDOWS_PS_MOVE_SOURCE_PREFIX)
        .map(|_| &source[WINDOWS_PS_MOVE_SOURCE_PREFIX.len()..])
    else {
        return Ok(None);
    };
    if path.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(path.to_string()))
}

#[cfg(windows)]
fn start_windows_hid_report_reader_if_supported(
    source: &str,
) -> Option<mpsc::Receiver<LatestHidReport>> {
    if !is_windows_ps_move_source(source) {
        return None;
    }
    let path = match windows_ps_move_input_path(source) {
        Ok(Some(path)) => path,
        Ok(None) => return None,
        Err(error) => {
            eprintln!("Muninn could not resolve Windows HID input source {source}: {error:#}");
            return None;
        }
    };
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("muninn-windows-hid-reader".to_string())
        .spawn(move || run_windows_hid_report_reader(path, tx))
        .ok()?;
    Some(rx)
}

#[cfg(windows)]
fn run_windows_hid_report_reader(path: String, tx: mpsc::Sender<LatestHidReport>) {
    loop {
        match read_windows_hid_reports_until_error(&path, &tx) {
            Ok(()) => return,
            Err(error) => {
                eprintln!("Muninn Windows HID reader for {path} restarting after: {error:#}");
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(windows)]
fn read_windows_hid_reports_until_error(
    path: &str,
    tx: &mpsc::Sender<LatestHidReport>,
) -> Result<()> {
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        ReadFile,
    };

    let wide = wide_null(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("opening persistent Windows HID input path {path}"));
    }

    let mut preparsed = 0;
    if unsafe { HidD_GetPreparsedData(handle, &mut preparsed) } == 0 {
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(handle) };
        return Err(error).with_context(|| format!("reading HID preparsed data from {path}"));
    }

    let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
    let caps_ok = unsafe { HidP_GetCaps(preparsed, &mut caps) == HIDP_STATUS_SUCCESS };
    unsafe { HidD_FreePreparsedData(preparsed) };
    if !caps_ok || caps.InputReportByteLength == 0 {
        unsafe { CloseHandle(handle) };
        return Err(anyhow!(
            "Windows HID input path {path} has no input reports"
        ));
    }

    eprintln!(
        "Muninn Windows HID reader attached to {path} report_len={}",
        caps.InputReportByteLength
    );
    loop {
        let mut report = vec![0u8; caps.InputReportByteLength as usize];
        let mut bytes_read = 0;
        let ok = unsafe {
            ReadFile(
                handle,
                report.as_mut_ptr(),
                report.len() as u32,
                &mut bytes_read,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(handle) };
            return Err(error)
                .with_context(|| format!("reading persistent HID report from {path}"));
        }
        if bytes_read == 0 {
            continue;
        }
        report.truncate(bytes_read as usize);
        let latest = LatestHidReport {
            bytes: report,
            source_timestamp_ns: timestamp_ns().unwrap_or_default(),
            observed_at: timestamp().unwrap_or_else(|_| "unix:0".to_string()),
        };
        if tx.send(latest).is_err() {
            unsafe { CloseHandle(handle) };
            return Ok(());
        }
    }
}

#[cfg(windows)]
fn windows_read_hid_interrupt_report(
    handle: *mut std::ffi::c_void,
    report_len: usize,
    path: &str,
) -> Result<Option<Vec<u8>>> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, ERROR_SEM_TIMEOUT, ERROR_TIMEOUT,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(std::io::Error::last_os_error()).context("creating Windows HID read event");
    }

    let mut report = vec![0u8; report_len];
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
    overlapped.hEvent = event;
    let mut bytes_read = 0;
    let ok = unsafe {
        ReadFile(
            handle,
            report.as_mut_ptr(),
            report.len() as u32,
            &mut bytes_read,
            &mut overlapped,
        )
    };
    if ok != 0 {
        unsafe { CloseHandle(event) };
        report.truncate(bytes_read as usize);
        return Ok(Some(report));
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
        unsafe { CloseHandle(event) };
        if matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_TIMEOUT as i32
                    || code == ERROR_SEM_TIMEOUT as i32
                    || code == ERROR_OPERATION_ABORTED as i32
        ) {
            return Ok(None);
        }
        return Err(error)
            .with_context(|| format!("starting Windows PS Move input read from {path}"));
    }

    let wait = unsafe { WaitForSingleObject(event, 2) };
    if wait == WAIT_TIMEOUT {
        unsafe {
            CancelIoEx(handle, &mut overlapped);
            CloseHandle(event);
        }
        return Ok(None);
    }
    if wait != WAIT_OBJECT_0 {
        unsafe {
            CancelIoEx(handle, &mut overlapped);
            CloseHandle(event);
        }
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("waiting for Windows PS Move input read from {path}"));
    }

    let mut transferred = 0;
    let done = unsafe { GetOverlappedResult(handle, &mut overlapped, &mut transferred, 0) };
    unsafe { CloseHandle(event) };
    if done == 0 {
        let error = std::io::Error::last_os_error();
        if matches!(
            error.raw_os_error(),
            Some(code)
                if code == ERROR_OPERATION_ABORTED as i32
                    || code == ERROR_TIMEOUT as i32
                    || code == ERROR_SEM_TIMEOUT as i32
        ) {
            return Ok(None);
        }
        return Err(error)
            .with_context(|| format!("completing Windows PS Move input read from {path}"));
    }
    report.truncate(transferred as usize);
    Ok(Some(report))
}

#[cfg(windows)]
unsafe fn windows_hid_interface_path(
    info_set: isize,
    interface_data: &mut windows_sys::Win32::Devices::DeviceAndDriverInstallation::SP_DEVICE_INTERFACE_DATA,
) -> Option<String> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiGetDeviceInterfaceDetailW,
    };

    let mut required_size = 0;
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            info_set,
            interface_data,
            std::ptr::null_mut(),
            0,
            &mut required_size,
            std::ptr::null_mut(),
        );
    }
    if required_size == 0 {
        return None;
    }

    let mut buffer = vec![0u8; required_size as usize];
    let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
    unsafe {
        (*detail).cbSize = if cfg!(target_pointer_width = "64") {
            8
        } else {
            6
        };
    }
    let ok = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            info_set,
            interface_data,
            detail,
            required_size,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }

    let path_ptr = unsafe { (*detail).DevicePath.as_ptr() };
    let mut len = 0;
    unsafe {
        while *path_ptr.add(len) != 0 {
            len += 1;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(
            path_ptr, len,
        )))
    }
}

#[cfg(windows)]
fn write_windows_hid_report(path: &str, report: &[u8]) -> Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        WriteFile,
    };

    let wide = wide_null(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("opening Windows PS Move HID path {path}"));
    }

    let mut written = 0;
    let ok = unsafe {
        WriteFile(
            handle,
            report.as_ptr().cast(),
            report.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("writing Windows PS Move HID report to {path}"));
    }
    Ok(())
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
fn execute_move_light_command(
    command: MuninnMoveLightCommandRecord,
    writer: &mut impl MoveLightWriter,
) -> Result<MuninnMoveLightCommandRecord> {
    let active = match prepare_move_light_command(command.clone()) {
        Ok(active) => active,
        Err(error) => return command_failed(command, &error.to_string()),
    };

    for _ in 0..active.command.repeat_count {
        for (index, (red, green, blue)) in active.colors.iter().copied().enumerate() {
            let report = move_light_report(red, green, blue);
            if let Err(error) = writer.write_report(&active.command.hidraw_path, &report) {
                return command_failed(active.command, &format!("{error:#}"));
            }
            let duration_ms = active.command.durations_ms[index];
            if duration_ms > 0 {
                thread::sleep(Duration::from_millis(u64::from(duration_ms)));
            }
        }
    }

    Ok(MuninnMoveLightCommandRecord {
        state: "completed".to_string(),
        detail: format!(
            "wrote {} PS Move light step(s) for {} repeat(s)",
            active.colors.len(),
            active.command.repeat_count
        ),
        updated_at: timestamp()?,
        ..active.command
    })
}

fn command_failed(
    command: MuninnMoveLightCommandRecord,
    detail: &str,
) -> Result<MuninnMoveLightCommandRecord> {
    Ok(MuninnMoveLightCommandRecord {
        state: "failed".to_string(),
        detail: detail.to_string(),
        updated_at: timestamp()?,
        ..command
    })
}

fn parse_move_colors(colors: &[String]) -> Result<Vec<(u8, u8, u8)>> {
    if colors.is_empty() {
        return Err(anyhow!("colors must contain at least one #rrggbb value"));
    }
    colors.iter().map(|color| parse_move_color(color)).collect()
}

fn parse_move_color(color: &str) -> Result<(u8, u8, u8)> {
    let trimmed = color.trim();
    let text = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if text.len() != 6 || !text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(anyhow!("invalid color {color:?}; expected #rrggbb"));
    }
    let red = u8::from_str_radix(&text[0..2], 16)?;
    let green = u8::from_str_radix(&text[2..4], 16)?;
    let blue = u8::from_str_radix(&text[4..6], 16)?;
    Ok((red, green, blue))
}

fn publish_quest_access_if_requested(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
) -> Result<()> {
    if !options.quest_adb {
        return Ok(());
    }

    let record = probe_quest_access(options)?;
    node.put("quest-access", &record)?;
    node.put(&record.access_id, &record)?;
    Ok(())
}

fn probe_quest_access(options: &Options) -> Result<MuninnQuestAccessRecord> {
    let output = match Command::new("adb").args(["devices", "-l"]).output() {
        Ok(output) => output,
        Err(error) => {
            return build_quest_access_record(
                options,
                ParsedQuestDevice::default(),
                "unavailable",
                &format!("adb devices -l could not run: {error}"),
            );
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return build_quest_access_record(
            options,
            ParsedQuestDevice::default(),
            "unavailable",
            &format!("adb devices -l failed: {}", stderr.trim()),
        );
    }

    let parsed = parse_quest_device_from_adb(&stdout, options.quest_serial.as_deref());
    let parsed_device = parsed.unwrap_or_default();
    let (state, detail): (String, String) = match &parsed_device {
        device if device.connection_state == "device" => (
            "usb-authorized".to_string(),
            "Quest is authorized over USB. Muninn can publish access and route Quest input/video surfaces; tracked poses still require a Quest/OpenXR witness.".to_string(),
        ),
        device if device.serial.is_empty() => (
            "unavailable".to_string(),
            "No matching Quest device was found in adb devices -l.".to_string(),
        ),
        device => (
            device.connection_state.clone(),
            "Quest is visible over USB but not authorized for local access.".to_string(),
        ),
    };
    build_quest_access_record(options, parsed_device, &state, &detail)
}

fn build_quest_access_record(
    options: &Options,
    device: ParsedQuestDevice,
    state: &str,
    detail: &str,
) -> Result<MuninnQuestAccessRecord> {
    let serial = if device.serial.is_empty() {
        options
            .quest_serial
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        device.serial
    };
    Ok(MuninnQuestAccessRecord {
        access_id: format!("muninn:{}:quest-access:{}", options.host_id, serial),
        host_id: options.host_id.clone(),
        serial,
        connection_state: device.connection_state,
        product: device.product,
        model: device.model,
        device: device.device,
        transport_id: device.transport_id,
        input_stream_id: options
            .quest_input_stream_id
            .clone()
            .unwrap_or_else(|| format!("muninn:{}:quest-input", options.host_id)),
        pose_stream_id: options
            .quest_pose_stream_id
            .clone()
            .unwrap_or_else(|| format!("muninn:{}:quest-poses", options.host_id)),
        video_input_stream_id: options
            .quest_video_input_stream_id
            .clone()
            .unwrap_or_else(|| format!("muninn:{}:quest-warped-video-input", options.host_id)),
        video_input_transport: "brokkr-unity-editor-warped-frame-stream".to_string(),
        state: state.to_string(),
        detail: detail.to_string(),
        observed_at: timestamp()?,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParsedQuestDevice {
    serial: String,
    connection_state: String,
    product: String,
    model: String,
    device: String,
    transport_id: String,
}

fn parse_quest_device_from_adb(
    adb_devices_output: &str,
    requested_serial: Option<&str>,
) -> Option<ParsedQuestDevice> {
    adb_devices_output
        .lines()
        .filter_map(parse_adb_device_line)
        .find(|device| {
            if let Some(serial) = requested_serial {
                return device.serial == serial;
            }
            device.model.contains("Quest")
                || device.product == "hollywood"
                || device.device == "hollywood"
        })
}

fn parse_adb_device_line(line: &str) -> Option<ParsedQuestDevice> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.len() < 2 || tokens[0] == "List" {
        return None;
    }

    let mut device = ParsedQuestDevice {
        serial: tokens[0].to_string(),
        connection_state: tokens[1].to_string(),
        ..ParsedQuestDevice::default()
    };
    for token in tokens.iter().skip(2) {
        let Some((key, value)) = token.split_once(':') else {
            continue;
        };
        match key {
            "product" => device.product = value.to_string(),
            "model" => device.model = value.to_string(),
            "device" => device.device = value.to_string(),
            "transport_id" => device.transport_id = value.to_string(),
            _ => {}
        }
    }
    Some(device)
}

fn quest_access_status(options: Options) -> Result<()> {
    let node = open_node(&options, "muninn-quest-access-status")?;
    let record = node
        .get_required::<MuninnQuestAccessRecord>("quest-access")
        .context("Muninn Quest access record is unavailable")?;
    println!(
        "{} state={} model={} product={} input={} poses={} video={} detail={}",
        record.serial,
        record.state,
        record.model,
        record.product,
        record.input_stream_id,
        record.pose_stream_id,
        record.video_input_stream_id,
        record.detail
    );
    Ok(())
}

fn health_check(options: &Options) -> Result<()> {
    let observed_at = idunn_timestamp()?;
    let result = evaluate_health(options);
    let (state, detail) = match &result {
        Ok(detail) => ("active", detail.clone()),
        Err(error) => ("failed", error.to_string()),
    };
    if let Some(idunn) = options.idunn_rudp_health.as_ref() {
        let publish_result = publish_idunn_rudp_health(idunn, state, &detail, &observed_at);
        if let Err(publish_error) = publish_result {
            return match result {
                Ok(_) => Err(publish_error).context("publishing Muninn health to Idunn RUDP"),
                Err(health_error) => Err(anyhow!(
                    "{health_error}; also failed to publish Muninn health to Idunn RUDP: {publish_error}"
                )),
            };
        }
    }

    match result {
        Ok(detail) => {
            println!("{detail}");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn evaluate_health(options: &Options) -> Result<String> {
    let node = open_node(options, "muninn-health")?;
    evaluate_health_from_node(options, &node)
}

fn evaluate_health_from_node(
    options: &Options,
    node: &cultmesh_rs::CultMeshNode,
) -> Result<String> {
    let surface = node
        .get_required::<MuninnTelemetrySurfaceRecord>("latest")
        .context("Muninn telemetry surface is unavailable")?;
    if surface.state != "idle" && surface.state != "active" {
        return Err(anyhow!(
            "Muninn telemetry surface is {}: {}",
            surface.state,
            surface.detail
        ));
    }

    verify_move_sources_fresh(options, &node)?;

    Ok(format!(
        "Muninn healthy: {} on {} ({})",
        surface.surface_id, surface.host_id, surface.state
    ))
}

fn publish_idunn_rudp_health(
    options: &IdunnRudpHealthOptions,
    state: &str,
    detail: &str,
    observed_at: &str,
) -> Result<()> {
    let mut transport = connect_idunn_rudp_health(options)?;
    transport
        .send("schema", idunn_health_payload(options, state, detail, observed_at)?)
        .with_context(|| format!("sending Idunn health to {}", options.endpoint))?;
    Ok(())
}

fn smootherstep(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn wrapped_hue_lerp(source: f64, target: f64, amount: f64) -> f64 {
    let delta = (target - source + 0.5).rem_euclid(1.0) - 0.5;
    (source + delta * amount).rem_euclid(1.0)
}

fn run_daemon_health_publisher(options: &Options) -> Result<()> {
    let idunn = options
        .idunn_rudp_health
        .as_ref()
        .context("Muninn daemon health publisher requires Idunn RUDP options")?;
    let mut transport = connect_idunn_rudp_health(idunn)?;
    let cadence = Duration::from_secs(options.interval_seconds.unwrap_or(15).max(1));
    let mut next_publish_at = Instant::now();
    loop {
        if Instant::now() >= next_publish_at {
            let observed_at = idunn_timestamp()?;
            let (state, detail) = match evaluate_health(options) {
                Ok(detail) => ("active", detail),
                Err(error) => ("failed", error.to_string()),
            };
            transport
                .send("schema", idunn_health_payload(idunn, state, &detail, &observed_at)?)
                .with_context(|| format!("sending Idunn health to {}", idunn.endpoint))?;
            next_publish_at = Instant::now() + cadence;
        }
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
    }
}

fn idunn_health_payload(
    options: &IdunnRudpHealthOptions,
    state: &str,
    detail: &str,
    observed_at: &str,
) -> Result<Vec<u8>> {
    let health = IdunnDaemonHealthRecord {
        daemon_id: options.daemon_id.clone(),
        state: state.to_string(),
        detail: detail.to_string(),
        observed_at: observed_at.to_string(),
        health_contract: options.health_contract.clone(),
        publication_source: "daemon-published".to_string(),
        transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
    };
    let message = CultNetMessage::DocumentPutRaw {
        message_id: format!(
            "muninn-health:{}:{}",
            options.daemon_id,
            observed_at.replace(':', "-")
        ),
        document: CultNetRawDocumentRecord {
            schema_id: "idunn.daemon_health".to_string(),
            record_key: options.daemon_id.clone(),
            stored_at: observed_at.to_string(),
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload: rmp_serde::to_vec(&health).context("encoding Idunn daemon health")?,
            source_runtime_id: Some("muninn-daemon".to_string()),
            source_agent_id: None,
            source_role: Some("daemon-health-publisher".to_string()),
            tags: Some(vec![CULTNET_RUDP_PROTOCOL_ID.to_string()]),
        },
    };
    encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)
        .context("encoding Idunn health CultNet message")
}

fn connect_idunn_rudp_health(
    options: &IdunnRudpHealthOptions,
) -> Result<CultNetRudpSocketTransportConnection> {
    let bind_address = if options.endpoint.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Muninn RUDP sender at {bind_address}"))?;
    socket.set_read_timeout(Some(Duration::from_secs(1)))?;
    let mut transport =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::client(
            "muninn-daemon",
            socket,
            options.endpoint,
            IDUNN_HEALTH_RUDP_CONNECTION_ID,
        ))?;
    transport.connect(Vec::new())?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !transport.connected() {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out connecting Muninn RUDP sender to {}",
                options.endpoint
            ));
        }
    }
    Ok(transport)
}

fn verify_move_sources_fresh(options: &Options, node: &cultmesh_rs::CultMeshNode) -> Result<()> {
    let move_state_sources =
        serve_move_state_sources(options, serve_should_manage_move_runtime(options));
    if move_state_sources.is_empty() {
        return Ok(());
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    let max_age_seconds = options
        .interval_seconds
        .unwrap_or(15)
        .saturating_mul(4)
        .max(30);
    let states = node.cache().get_all::<MuninnMoveControllerStateRecord>()?;

    for source in &move_state_sources {
        let latest = states
            .iter()
            .filter(|state| {
                state.host_id == options.host_id
                    && state.move_id == source.move_id
                    && state.source_path == source.hidraw_path
            })
            .max_by_key(|state| unix_timestamp_sort_key(&state.observed_at))
            .ok_or_else(|| {
                anyhow!(
                    "Muninn Move source {} at {} has no controller-state records",
                    source.move_id,
                    source.hidraw_path
                )
            })?;
        let observed_seconds = parse_unix_timestamp(&latest.observed_at).with_context(|| {
            format!(
                "Muninn Move source {} has invalid observed_at",
                source.move_id
            )
        })?;
        let age = now.saturating_sub(observed_seconds);
        if age > max_age_seconds {
            return Err(anyhow!(
                "Muninn Move source {} is stale: observed {} seconds ago, max {}",
                source.move_id,
                age,
                max_age_seconds
            ));
        }
    }

    Ok(())
}

fn parse_unix_timestamp(value: &str) -> Result<u64> {
    value
        .strip_prefix("unix-")
        .ok_or_else(|| anyhow!("timestamp must start with unix-"))?
        .parse()
        .context("timestamp seconds must be an integer")
}

fn unix_timestamp_sort_key(value: &str) -> u64 {
    parse_unix_timestamp(value).unwrap_or(0)
}

fn request_move_light(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let mut node = open_node(&options, "muninn-move-light-request")?;
    let command = build_move_light_command(&options)?;
    node.put(&command.command_id, &command)?;
    println!(
        "Published Muninn Move light command {} for {} on {}.",
        command.command_id, command.move_id, command.host_id
    );
    Ok(())
}

fn request_capture_stream(options: Options) -> Result<()> {
    require_media_target_uri(&options)?;
    publish_obs_media_receiver_to_odin(&options)?;
    let command = build_capture_stream_command(&options)?;
    publish_capture_command_to_odin(&options, &command)?;
    println!(
        "Published Muninn capture stream command {} {} {} for {} through Odin/CultMesh.",
        command.command_id, command.action, command.stream_id, command.host_id
    );
    Ok(())
}

fn publish_obs_media_receiver_to_odin(options: &Options) -> Result<()> {
    let Some(address_host) = options.obs_target_host.as_deref() else {
        return Ok(());
    };
    if address_host.trim().is_empty() || options.obs_port == 0 {
        return Ok(());
    }
    let Some(target) = resolve_odin_cultmesh_uri(options) else {
        return Err(anyhow!(
            "muninn request-stream requires --odin-cultmesh-uri before OBS can advertise its media receiver"
        ));
    };
    let address = format!("{address_host}:{}", options.obs_port);
    let provider_id = format!("mimir.obs.media-receiver.{}", options.host_id);
    let advertisement = EveProviderAdvertisementRecord {
        value: json!({
            "schema": "gamecult.eve.provider_advertisement.v1",
            "providerId": provider_id,
            "title": "Mimir OBS Muninn media receiver",
            "description": "OBS-owned Muninn RUDP media receiver endpoint advertised for Odin-routed capture activation.",
            "canonicalService": "mimir.obs.media-receiver",
            "locatedService": options.target_host,
            "cultMeshAddress": options.target_host,
            "status": "active",
            "updatedAt": timestamp()?,
            "inputStreams": [{
                "streamId": format!("{}#{}", options.target_host, options.stream_id),
                "schema": MUNINN_MEDIA_RUDP_SCHEMA,
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "address": address,
                "connectionId": MUNINN_MEDIA_RUDP_CONNECTION_ID,
                "channel": "media",
                "producer": "Mimir OBS Muninn source"
            }],
            "routes": [{
                "schema": MUNINN_MEDIA_RUDP_SCHEMA,
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "address": address
            }],
        }),
    };
    let node = open_node(options, "muninn-obs-media-receiver-publisher")?;
    node.publish_document_to_rudp_catalog(
        &provider_id,
        &advertisement,
        CultMeshRudpDocumentPublishOptions {
            target,
            runtime_id: "muninn-request-stream".to_string(),
            source_role: Some("obs-media-receiver-provider".to_string()),
            tags: vec![
                "odin-media-receiver-route".to_string(),
                "muninn.media-rudp".to_string(),
            ],
            connection_id: CULTMESH_RUDP_DOCUMENT_CATALOG_CONNECTION_ID,
            ..CultMeshRudpDocumentPublishOptions::default()
        },
    )
}

fn publish_capture_command_to_odin(
    options: &Options,
    command: &MuninnCaptureStreamCommandRecord,
) -> Result<()> {
    let Some(target) = resolve_odin_cultmesh_uri(options) else {
        return Err(anyhow!(
            "muninn request-stream requires --odin-cultmesh-uri; direct activation-store writes and raw RUDP command targets have been removed"
        ));
    };
    let node = open_node(options, "muninn-capture-command-odin-publisher")?;
    node.publish_document_to_rudp_catalog(
        &latest_capture_stream_command_key(&command.host_id, &command.stream_id),
        command,
        CultMeshRudpDocumentPublishOptions {
            target,
            runtime_id: "muninn-request-stream".to_string(),
            source_role: Some("capture-command-publisher".to_string()),
            tags: vec![
                "odin-cultmesh-command-route".to_string(),
                "muninn.capture-stream".to_string(),
            ],
            connection_id: CULTMESH_RUDP_DOCUMENT_CATALOG_CONNECTION_ID,
            ..CultMeshRudpDocumentPublishOptions::default()
        },
    )
}

fn move_light_status(options: Options) -> Result<()> {
    let node = open_node(&options, "muninn-move-light-status")?;
    let mut commands = node.cache().get_all::<MuninnMoveLightCommandRecord>()?;
    commands.retain(|command| command.host_id == options.host_id);
    if let Some(command_id) = options.command_id.as_deref() {
        commands.retain(|command| command.command_id == command_id);
    }
    commands.sort_by(|left, right| left.command_id.cmp(&right.command_id));

    if commands.is_empty() {
        println!(
            "No Muninn Move light commands found for {}.",
            options.host_id
        );
        return Ok(());
    }

    for command in commands {
        println!(
            "{} {} {} {} {}",
            command.command_id, command.host_id, command.move_id, command.state, command.detail
        );
    }
    Ok(())
}

fn set_move_hue_program(options: Options) -> Result<()> {
    let key = move_hue_program_key(&options.host_id);
    let mut node = open_node(&options, "muninn-set-move-hue-program")?;
    let mut program = node
        .get::<MuninnMoveHueProgramRecord>(&key)?
        .unwrap_or_else(|| bootstrap_move_hue_program(&options));
    if let Some(mode) = options.move_hue_mode.as_deref() {
        program.mode = mode.to_string();
        if mode == "hold" {
            program.hold_at_ns = timestamp_ns()?;
        }
    }
    if let Some(order_mode) = options.move_hue_order_mode.as_deref() {
        program.order_mode = order_mode.to_string();
    }
    if options.move_hue_cycle_ms_explicit {
        program.cycle_ms = options.move_hue_cycle_ms;
    }
    program.requested_by = "muninn-cli-or-eve-lowering".to_string();
    program.updated_at = timestamp()?;
    validate_move_hue_program(&program)?;
    node.put(&key, &program)?;
    if let Some(target) = resolve_odin_cultmesh_uri(&options) {
        node.publish_document_to_rudp_catalog(
            &key,
            &program,
            CultMeshRudpDocumentPublishOptions {
                target,
                runtime_id: format!("muninn-{}-move-hue-command", options.host_id),
                source_role: Some("muninn.move-hue-program-command".to_string()),
                tags: vec!["muninn".to_string(), "move-hue-program".to_string()],
                ..CultMeshRudpDocumentPublishOptions::default()
            },
        )?;
    }
    println!(
        "{} mode={} order={} cycle_ms={} hold_at_ns={} updated={}",
        program.program_id,
        program.mode,
        program.order_mode,
        program.cycle_ms,
        program.hold_at_ns,
        program.updated_at
    );
    Ok(())
}

fn move_hue_program_status(options: Options) -> Result<()> {
    let key = move_hue_program_key(&options.host_id);
    let node = open_node(&options, "muninn-move-hue-program-status")?;
    let program = node
        .get_required::<MuninnMoveHueProgramRecord>(&key)
        .context("Muninn Move hue program is unavailable")?;
    println!(
        "{} mode={} order={} cycle_ms={} epoch_ns={} hold_at_ns={} requested_by={} updated={}",
        program.program_id,
        program.mode,
        program.order_mode,
        program.cycle_ms,
        program.epoch_ns,
        program.hold_at_ns,
        program.requested_by,
        program.updated_at
    );
    Ok(())
}

fn capture_stream_status(options: Options) -> Result<()> {
    let mut status_options = options.clone();
    if let Some(activation_store_path) = status_options.activation_store_path.as_ref() {
        status_options.store_path = activation_store_path.clone();
    }
    let node = open_node(&status_options, "muninn-capture-stream-status")?;
    let mut commands = node.cache().get_all::<MuninnCaptureStreamCommandRecord>()?;
    commands.retain(|command| command.host_id == status_options.host_id);
    if let Some(command_id) = status_options.command_id.as_deref() {
        commands.retain(|command| command.command_id == command_id);
    }
    if status_options.stream_filter_explicit && !status_options.stream_id.trim().is_empty() {
        commands.retain(|command| command.stream_id == status_options.stream_id);
    }
    commands.sort_by(|left, right| {
        left.stream_id
            .cmp(&right.stream_id)
            .then(left.command_id.cmp(&right.command_id))
    });

    if commands.is_empty() {
        println!(
            "No Muninn capture stream commands found for host {}{}{}.",
            status_options.host_id,
            if !status_options.stream_filter_explicit || status_options.stream_id.trim().is_empty()
            {
                ""
            } else {
                " stream "
            },
            if !status_options.stream_filter_explicit || status_options.stream_id.trim().is_empty()
            {
                ""
            } else {
                status_options.stream_id.as_str()
            }
        );
        return Ok(());
    }

    for command in commands {
        let target_display = if command.action == "start" {
            format!("{}:{}", command.target_host, command.port)
        } else {
            "n/a".to_string()
        };
        let video_source_display = if command.action == "start" {
            command_video_source_id(&command).to_string()
        } else {
            "n/a".to_string()
        };
        let audio_source_display = if command.action == "start" {
            command_audio_source_id(&command).to_string()
        } else {
            "n/a".to_string()
        };
        println!(
            "{} host={} stream={} action={} state={} target={} video_source={} audio_source={} detail={} updated={}",
            command.command_id,
            command.host_id,
            command.stream_id,
            command.action,
            command.state,
            target_display,
            video_source_display,
            audio_source_display,
            command.detail,
            command.updated_at
        );
    }
    Ok(())
}

fn move_state_status(options: Options) -> Result<()> {
    let node = open_node(&options, "muninn-move-state-status")?;
    let mut states = node.cache().get_all::<MuninnMoveControllerStateRecord>()?;
    states.retain(|state| state.host_id == options.host_id);
    if let Some(move_filter) = options.move_filter.as_deref() {
        states.retain(|state| state.move_id == move_filter);
    }
    for source in live_move_state_sources(&options) {
        if options
            .move_filter
            .as_deref()
            .is_some_and(|move_filter| source.move_id != move_filter)
        {
            continue;
        }
        let key = format!(
            "{}:{}:move-controller-state",
            options.host_id, source.move_id
        );
        if let Some(state) = node.get::<MuninnMoveControllerStateRecord>(&key)? {
            states.push(state);
        }
    }
    states.sort_by(|a, b| {
        a.stream_id
            .cmp(&b.stream_id)
            .then(
                unix_timestamp_sort_key(&a.observed_at)
                    .cmp(&unix_timestamp_sort_key(&b.observed_at)),
            )
            .then(a.sequence.cmp(&b.sequence))
    });
    states.dedup_by(|left, right| {
        left.stream_id == right.stream_id && left.sequence == right.sequence
    });
    if states.is_empty() {
        println!(
            "No Muninn Move controller state records found for host {}.",
            options.host_id
        );
        return Ok(());
    }
    for state in states {
        println!(
            "{} seq={} move={} source={} buttons=[{}] trigger={:.3} accel={:?} gyro={:?} battery={:.3} observed={}",
            state.stream_id,
            state.sequence,
            state.move_id,
            state.source_path,
            state.buttons.join(","),
            state.trigger_value,
            state.accelerometer_xyz,
            state.gyroscope_xyz,
            state.battery01,
            state.observed_at
        );
    }
    Ok(())
}

fn move_source_status(options: Options) -> Result<()> {
    let sources = live_move_state_sources(&options);
    if sources.is_empty() {
        println!(
            "No live Muninn Move state sources found for host {}.",
            options.host_id
        );
        return Ok(());
    }
    let mut reader = HidMoveControllerStateReader;
    for source in sources {
        let report_status = if is_joystick_path(&source.hidraw_path) {
            match reader.read_joystick_events(&source.hidraw_path) {
                Ok(events) => format!("joystick_events={}", events.len()),
                Err(error) => format!("read_error={error:#}"),
            }
        } else {
            match reader.read_report(&source.hidraw_path) {
                Ok(Some(report)) => format!("report_bytes={}", report.len()),
                Ok(None) => "report_bytes=none".to_string(),
                Err(error) => format!("read_error={error:#}"),
            }
        };
        println!(
            "{} move={} source={} {}",
            options.host_id, source.move_id, source.hidraw_path, report_status
        );
    }
    Ok(())
}

fn move_identity_status(options: Options) -> Result<()> {
    let mut identities = current_move_identity_records(&options)?;
    if let Some(move_filter) = options.move_filter.as_deref() {
        identities.retain(|identity| identity.move_id == move_filter);
    }
    identities.sort_by(|a, b| {
        a.identity_id.cmp(&b.identity_id).then(
            unix_timestamp_sort_key(&b.observed_at).cmp(&unix_timestamp_sort_key(&a.observed_at)),
        )
    });
    identities.dedup_by(|left, right| left.identity_id == right.identity_id);
    if identities.is_empty() {
        println!(
            "No Muninn Move identity records found for host {}.",
            options.host_id
        );
        return Ok(());
    }
    for identity in identities {
        println!(
            "{} move={} source={} bluetooth_host={} state={} observed={} detail={}",
            identity.identity_id,
            identity.move_id,
            identity.source_path,
            identity.bluetooth_host_address,
            identity.state,
            identity.observed_at,
            identity.detail
        );
    }
    Ok(())
}

fn current_move_identity_records(options: &Options) -> Result<Vec<MuninnMoveIdentityRecord>> {
    let sources = live_move_state_sources(options);
    current_move_identity_records_from_sources(options, &sources)
}

fn current_move_identity_records_from_sources(
    options: &Options,
    sources: &[MoveStateSource],
) -> Result<Vec<MuninnMoveIdentityRecord>> {
    let observed_at = timestamp()?;
    let bluetooth_host_address = move_host_address_for_claim(options).unwrap_or_default();
    let mut identities = sources
        .iter()
        .map(|source| MuninnMoveIdentityRecord {
            identity_id: format!("{}:{}:move-identity", options.host_id, source.move_id),
            host_id: options.host_id.clone(),
            move_id: source.move_id.clone(),
            source_path: source.hidraw_path.clone(),
            bluetooth_host_address: bluetooth_host_address.clone(),
            state: "usb-visible".to_string(),
            detail: "Muninn currently sees this PS Move on a local USB/HID input path.".to_string(),
            observed_at: observed_at.clone(),
        })
        .collect::<Vec<_>>();
    identities.extend(current_bluetooth_move_identity_records(
        options,
        &observed_at,
    ));
    identities.sort_by(|a, b| {
        a.identity_id.cmp(&b.identity_id).then(
            unix_timestamp_sort_key(&b.observed_at).cmp(&unix_timestamp_sort_key(&a.observed_at)),
        )
    });
    identities.dedup_by(|left, right| left.identity_id == right.identity_id);
    Ok(identities)
}

#[cfg(unix)]
fn current_bluetooth_move_identity_records(
    options: &Options,
    observed_at: &str,
) -> Vec<MuninnMoveIdentityRecord> {
    let bluetooth_host_address = platform_default_bluetooth_host_address().unwrap_or_default();
    bluetoothctl_motion_controller_devices()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|device| {
            build_bluetooth_move_identity_record(
                options,
                &device,
                bluetooth_host_address.clone(),
                observed_at.to_string(),
            )
        })
        .collect()
}

#[cfg(not(unix))]
fn current_bluetooth_move_identity_records(
    _options: &Options,
    _observed_at: &str,
) -> Vec<MuninnMoveIdentityRecord> {
    Vec::new()
}

fn claim_move_host(options: Options) -> Result<()> {
    let host = options
        .move_host_address
        .as_deref()
        .ok_or_else(|| anyhow!("--move-host is required for claim-move-host"))?;
    claim_ps_move_host(host)
}

#[cfg(unix)]
fn claim_ps_move_host(host_address: &str) -> Result<()> {
    let host = parse_bluetooth_address_little_endian(host_address)?;
    let claims = unix_claim_ps_move_host(&host)?;
    if claims == 0 {
        println!("no USB PS Move pairing collections found");
    }
    Ok(())
}

#[cfg(all(not(unix), not(windows)))]
fn claim_ps_move_host(_host_address: &str) -> Result<()> {
    Err(anyhow!(
        "claim-move-host is implemented in Muninn on Unix and Windows"
    ))
}

#[cfg(windows)]
fn claim_ps_move_host(host_address: &str) -> Result<()> {
    let host = parse_bluetooth_address_little_endian(host_address)?;
    let claims = windows_claim_ps_move_host(&host)?;
    if claims == 0 {
        println!("no USB PS Move pairing collections found");
    }
    Ok(())
}

fn build_move_light_command(options: &Options) -> Result<MuninnMoveLightCommandRecord> {
    parse_move_colors(&options.move_colors)?;
    if options.hidraw_path.trim().is_empty() {
        return Err(anyhow!("--hidraw is required for request-move-light"));
    }
    if options.move_repeat_count == 0 {
        return Err(anyhow!("--repeat-count must be greater than zero"));
    }
    if options.move_durations_ms.len() != options.move_colors.len() {
        return Err(anyhow!(
            "--duration-ms must be provided once for each --color"
        ));
    }

    let now = timestamp()?;
    Ok(MuninnMoveLightCommandRecord {
        command_id: format!("{}:{}:move-light:{}", options.host_id, options.move_id, now),
        host_id: options.host_id.clone(),
        move_id: options.move_id.clone(),
        hidraw_path: options.hidraw_path.clone(),
        colors: options.move_colors.clone(),
        durations_ms: options.move_durations_ms.clone(),
        repeat_count: options.move_repeat_count,
        authority: "muninn.operator-request".to_string(),
        state: "pending".to_string(),
        detail: "operator requested a typed Move light command".to_string(),
        updated_at: now,
    })
}

fn build_capture_stream_command(options: &Options) -> Result<MuninnCaptureStreamCommandRecord> {
    require_media_target_uri(options)?;
    if options.stream_action != "start" && options.stream_action != "stop" {
        return Err(anyhow!("--stream-action must be start or stop"));
    }
    let now = timestamp()?;
    let command_id = options.command_id.clone().unwrap_or_else(|| {
        format!(
            "{}:{}:capture-stream:{}:{}",
            options.host_id, options.stream_id, options.stream_action, now
        )
    });
    Ok(MuninnCaptureStreamCommandRecord {
        command_id,
        host_id: options.host_id.clone(),
        stream_id: options.stream_id.clone(),
        state: "pending".to_string(),
        action: options.stream_action.clone(),
        target_host: options.target_host.clone(),
        port: options.port,
        obs_target_host: options.obs_target_host.clone(),
        obs_port: options.obs_port,
        media_transport: media_transport_cli(&options.media_transport).to_string(),
        media_packet_bytes: options.media_packet_bytes as u32,
        requested_by: "muninn.request-stream".to_string(),
        detail: "operator requested a daemon-owned capture stream lifecycle change".to_string(),
        updated_at: now,
        rudp_video_bitrate_kbps: options.rudp_video_bitrate_kbps,
        rudp_latency_budget_ms: options.rudp_latency_budget_ms,
        video_source_id: video_source_id_for_options(options),
        audio_source_id: audio_source_id_for_options(options),
    })
}

fn build_mux_plan(options: &Options, timestamp: String) -> MuxPlan {
    let command_file = options.log_root.join(format!("muninn-{timestamp}.ps1"));
    let loopback_stderr = options
        .log_root
        .join(format!("muninn-{timestamp}.loopback.err.log"));
    let ffmpeg_stderr = options
        .log_root
        .join(format!("muninn-{timestamp}.ffmpeg.err.log"));
    let targets = build_targets(options);
    let loopback_args = loopback_args(options);
    let ffmpeg_args = ffmpeg_args(options);
    let loopback_args_literal = powershell_array_literal(&loopback_args);
    let ffmpeg_args_literal = powershell_array_literal(&ffmpeg_args);
    let command_line = format!(
        "powershell.exe {} | {} {}",
        loopback_args
            .iter()
            .map(|arg| quote_powershell(arg))
            .collect::<Vec<_>>()
            .join(" "),
        options.ffmpeg_path,
        ffmpeg_args
            .iter()
            .map(|arg| quote_powershell(arg))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let command_script = format!(
        concat!(
            "$ErrorActionPreference = 'Stop'\r\n",
            "$ProgressPreference = 'SilentlyContinue'\r\n",
            "$loopbackArgs = {0}\r\n",
            "$ffmpegArgs = {1}\r\n",
            "$loopbackErrorPath = {2}\r\n",
            "$ffmpegErrorPath = {3}\r\n",
            "function Quote-NativeArgument([string] $Value) {{\r\n",
            "  if ([string]::IsNullOrEmpty($Value)) {{ return '\"\"' }}\r\n",
            "  if ($Value -match '[\\s\"]') {{ return '\"' + $Value.Replace('\"', '\\\"') + '\"' }}\r\n",
            "  return $Value\r\n",
            "}}\r\n",
            "function Start-RedirectTask([System.IO.StreamReader] $Reader, [string] $Path) {{\r\n",
            "  return [System.Threading.Tasks.Task]::Run([Action]{{\r\n",
            "    $writer = [System.IO.StreamWriter]::new($Path, $false, [System.Text.Encoding]::ASCII)\r\n",
            "    try {{\r\n",
            "      while (($line = $Reader.ReadLine()) -ne $null) {{ $writer.WriteLine($line); $writer.Flush() }}\r\n",
            "    }} finally {{ $writer.Dispose() }}\r\n",
            "  }})\r\n",
            "}}\r\n",
            "$loopbackInfo = [System.Diagnostics.ProcessStartInfo]::new()\r\n",
            "$loopbackInfo.FileName = 'powershell.exe'\r\n",
            "$loopbackInfo.UseShellExecute = $false\r\n",
            "$loopbackInfo.RedirectStandardOutput = $true\r\n",
            "$loopbackInfo.RedirectStandardError = $true\r\n",
            "$loopbackInfo.CreateNoWindow = $true\r\n",
            "$loopbackInfo.Arguments = ($loopbackArgs | ForEach-Object {{ Quote-NativeArgument $_ }}) -join ' '\r\n",
            "$ffmpegInfo = [System.Diagnostics.ProcessStartInfo]::new()\r\n",
            "$ffmpegInfo.FileName = {4}\r\n",
            "$ffmpegInfo.UseShellExecute = $false\r\n",
            "$ffmpegInfo.RedirectStandardInput = $true\r\n",
            "$ffmpegInfo.RedirectStandardError = $true\r\n",
            "$ffmpegInfo.CreateNoWindow = $true\r\n",
            "$ffmpegInfo.Arguments = ($ffmpegArgs | ForEach-Object {{ Quote-NativeArgument $_ }}) -join ' '\r\n",
            "$loopback = [System.Diagnostics.Process]::Start($loopbackInfo)\r\n",
            "$ffmpeg = [System.Diagnostics.Process]::Start($ffmpegInfo)\r\n",
            "$loopbackErrTask = Start-RedirectTask $loopback.StandardError $loopbackErrorPath\r\n",
            "$ffmpegErrTask = Start-RedirectTask $ffmpeg.StandardError $ffmpegErrorPath\r\n",
            "$buffer = [byte[]]::new(65536)\r\n",
            "try {{\r\n",
            "  while (($read = $loopback.StandardOutput.BaseStream.Read($buffer, 0, $buffer.Length)) -gt 0) {{\r\n",
            "    $ffmpeg.StandardInput.BaseStream.Write($buffer, 0, $read)\r\n",
            "    $ffmpeg.StandardInput.BaseStream.Flush()\r\n",
            "  }}\r\n",
            "  $ffmpeg.StandardInput.Close()\r\n",
            "  $ffmpeg.WaitForExit()\r\n",
            "  $loopback.WaitForExit()\r\n",
            "  [System.Threading.Tasks.Task]::WaitAll(@($loopbackErrTask, $ffmpegErrTask))\r\n",
            "  if ($ffmpeg.ExitCode -ne 0) {{ throw ('ffmpeg exited with code ' + $ffmpeg.ExitCode) }}\r\n",
            "  if ($loopback.ExitCode -ne 0) {{ throw ('loopback exited with code ' + $loopback.ExitCode) }}\r\n",
            "}} finally {{\r\n",
            "  if (-not $ffmpeg.HasExited) {{ $ffmpeg.Kill() }}\r\n",
            "  if (-not $loopback.HasExited) {{ $loopback.Kill() }}\r\n",
            "}}\r\n"
        ),
        loopback_args_literal,
        ffmpeg_args_literal,
        quote_powershell(&loopback_stderr.display().to_string()),
        quote_powershell(&ffmpeg_stderr.display().to_string()),
        quote_powershell(&options.ffmpeg_path)
    );

    MuxPlan {
        command_line,
        command_script,
        command_file,
        targets,
    }
}

fn require_media_target_uri(options: &Options) -> Result<()> {
    if options.target_host.trim().is_empty() {
        return Err(anyhow!(
            "--target-host is required for Muninn media activation; provide an Odin/CultMesh media target URI"
        ));
    }
    if !options.target_host.starts_with("cultmesh://") {
        return Err(anyhow!(
            "Muninn media activation requires a cultmesh:// target URI resolved through Odin; raw hosts, IPs, and ports are not accepted"
        ));
    }
    Ok(())
}

fn build_targets(options: &Options) -> Vec<String> {
    vec![rudp_endpoint_for_options(options)]
}

fn rudp_endpoint_for_options(options: &Options) -> String {
    format!("{}#{}", options.target_host, options.stream_id)
}

fn muninn_rudp_media_profile() -> MuninnRudpMediaProfile {
    muninn_rudp_media_profile_for_bitrate(MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS)
}

fn muninn_rudp_media_profile_for_options(options: &Options) -> MuninnRudpMediaProfile {
    muninn_rudp_media_profile_for_bitrate_and_latency(
        options.rudp_video_bitrate_kbps,
        options.rudp_latency_budget_ms,
    )
}

fn muninn_rudp_media_profile_for_bitrate(video_bitrate_kbps: u32) -> MuninnRudpMediaProfile {
    muninn_rudp_media_profile_for_bitrate_and_latency(
        video_bitrate_kbps,
        MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS as u32,
    )
}

fn muninn_rudp_media_profile_for_bitrate_and_latency(
    video_bitrate_kbps: u32,
    latency_budget_ms: u32,
) -> MuninnRudpMediaProfile {
    let latency_budget_ms = u64::from(latency_budget_ms.max(1));
    MuninnRudpMediaProfile {
        profile_id: MUNINN_RUDP_MEDIA_PROFILE_ID,
        video_codec: "h264",
        video_encoder: "h264_nvenc",
        video_preset: "p5",
        video_tune: "ull",
        video_bitrate_kbps: video_bitrate_kbps.max(1),
        media_packet_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES,
        max_fragment_bytes: MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES,
        video_b_frames: 0,
        video_rc_lookahead: 0,
        sender_queue_deadline_ms: latency_budget_ms,
        sender_resend_delay_ms: MUNINN_RUDP_MEDIA_RESEND_DELAY_MS,
        sender_delivery_deadline_ms: latency_budget_ms,
        sender_pace_every_payloads: MUNINN_RUDP_MEDIA_SEND_PACE_EVERY_PAYLOADS,
        sender_pace_sleep_us: MUNINN_RUDP_MEDIA_SEND_PACE_SLEEP_US,
        receiver_assembly_deadline_ms: latency_budget_ms,
        receiver_gap_wait_ms: MUNINN_RUDP_MEDIA_RECEIVER_GAP_WAIT_MS,
    }
}

fn muninn_rudp_video_bitrate_arg(profile: &MuninnRudpMediaProfile) -> String {
    format!("{}k", profile.video_bitrate_kbps)
}

fn muninn_rudp_video_vbv_buffer_arg(options: &Options, profile: &MuninnRudpMediaProfile) -> String {
    let framerate = options.framerate.max(1);
    let frame_budget_kbits = profile.video_bitrate_kbps.div_ceil(framerate).max(1);
    let burst_budget_kbits = frame_budget_kbits
        .saturating_mul(MUNINN_RUDP_MEDIA_VBV_FRAME_BUDGETS)
        .max(frame_budget_kbits);
    format!("{burst_budget_kbits}k")
}

fn muninn_rudp_video_gop_frames(options: &Options) -> u32 {
    (options.framerate.max(1).div_ceil(4)).max(1)
}

fn loopback_args(options: &Options) -> Vec<String> {
    let audio_source = audio_source_spec_for_options(options);
    vec![
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        options.loopback_script.display().to_string(),
        "-Output".to_string(),
        "stdout".to_string(),
        "-SampleRate".to_string(),
        options.audio_sample_rate.to_string(),
        "-Channels".to_string(),
        options.audio_channels.to_string(),
        "-Device".to_string(),
        audio_source.device,
        "-DataFlow".to_string(),
        match audio_source.kind {
            AudioSourceKind::Loopback => "Render".to_string(),
            AudioSourceKind::Input => "Capture".to_string(),
        },
    ]
    .into_iter()
    .chain(match audio_source.kind {
        AudioSourceKind::Loopback => vec!["-Loopback".to_string()],
        AudioSourceKind::Input => Vec::new(),
    })
    .collect()
}

fn rudp_video_ffmpeg_args(options: &Options) -> Vec<String> {
    let profile = muninn_rudp_media_profile_for_options(options);
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-fflags".to_string(),
        "nobuffer".to_string(),
        "-flags".to_string(),
        "low_delay".to_string(),
        "-thread_queue_size".to_string(),
        "256".to_string(),
        "-f".to_string(),
        "lavfi".to_string(),
        "-i".to_string(),
        format!(
            "ddagrab=framerate={}:output_idx={}:draw_mouse=1",
            options.framerate, options.ddagrab_output_index
        ),
        "-map".to_string(),
        "0:v:0".to_string(),
        "-an".to_string(),
        "-c:v".to_string(),
        profile.video_encoder.to_string(),
        "-preset".to_string(),
        profile.video_preset.to_string(),
        "-tune".to_string(),
        profile.video_tune.to_string(),
        "-zerolatency".to_string(),
        "1".to_string(),
        "-bf".to_string(),
        profile.video_b_frames.to_string(),
        "-delay".to_string(),
        "0".to_string(),
        "-rc".to_string(),
        "cbr".to_string(),
        "-rc-lookahead".to_string(),
        profile.video_rc_lookahead.to_string(),
        "-multipass".to_string(),
        "disabled".to_string(),
        "-strict_gop".to_string(),
        "1".to_string(),
        "-ldkfs".to_string(),
        MUNINN_RUDP_MEDIA_LOW_DELAY_KEY_FRAME_SCALE.to_string(),
        "-spatial_aq".to_string(),
        "1".to_string(),
        "-temporal_aq".to_string(),
        "1".to_string(),
        "-aq-strength".to_string(),
        "8".to_string(),
        "-nonref_p".to_string(),
        "1".to_string(),
        "-b_ref_mode".to_string(),
        "disabled".to_string(),
        "-dpb_size".to_string(),
        MUNINN_RUDP_MEDIA_VIDEO_DPB_SIZE.to_string(),
        "-b:v".to_string(),
        muninn_rudp_video_bitrate_arg(&profile),
        "-maxrate".to_string(),
        muninn_rudp_video_bitrate_arg(&profile),
        "-bufsize".to_string(),
        muninn_rudp_video_vbv_buffer_arg(options, &profile),
        "-g".to_string(),
        muninn_rudp_video_gop_frames(options).to_string(),
        "-keyint_min".to_string(),
        muninn_rudp_video_gop_frames(options).to_string(),
        "-forced-idr".to_string(),
        "1".to_string(),
        "-aud".to_string(),
        "1".to_string(),
        "-fps_mode".to_string(),
        "cfr".to_string(),
        "-r".to_string(),
        options.framerate.max(1).to_string(),
        "-f".to_string(),
        profile.video_codec.to_string(),
        "pipe:1".to_string(),
    ]
}

fn muninn_controllable_video_encoder_args(options: &Options) -> Vec<String> {
    vec![
        "--input".to_string(),
        format!(
            "ddagrab=framerate={}:output_idx={}:draw_mouse=1",
            options.framerate, options.ddagrab_output_index
        ),
        "--framerate".to_string(),
        options.framerate.max(1).to_string(),
        "--bitrate-kbps".to_string(),
        options.rudp_video_bitrate_kbps.to_string(),
        "--gop-frames".to_string(),
        muninn_rudp_video_gop_frames(options).to_string(),
    ]
}

fn rudp_audio_ffmpeg_args(options: &Options) -> Vec<String> {
    vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-fflags".to_string(),
        "nobuffer".to_string(),
        "-flags".to_string(),
        "low_delay".to_string(),
        "-thread_queue_size".to_string(),
        "256".to_string(),
        "-f".to_string(),
        "f32le".to_string(),
        "-ar".to_string(),
        options.audio_sample_rate.to_string(),
        "-ac".to_string(),
        options.audio_channels.to_string(),
        "-i".to_string(),
        "pipe:0".to_string(),
        "-vn".to_string(),
        "-ar".to_string(),
        options.audio_sample_rate.to_string(),
        "-ac".to_string(),
        options.audio_channels.to_string(),
        "-f".to_string(),
        "f32le".to_string(),
        "pipe:1".to_string(),
    ]
}

fn ffmpeg_args(options: &Options) -> Vec<String> {
    let _ = options.media_transport;
    rudp_video_ffmpeg_args(options)
}

fn write_command_file(plan: &MuxPlan) -> Result<()> {
    fs::write(&plan.command_file, &plan.command_script)
        .with_context(|| format!("writing {}", plan.command_file.display()))
}

fn quote_powershell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell_array_literal(values: &[String]) -> String {
    let joined = values
        .iter()
        .map(|value| quote_powershell(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("@({joined})")
}

fn ensure_state_dirs(options: &Options) -> Result<()> {
    fs::create_dir_all(&options.log_root)
        .with_context(|| format!("creating {}", options.log_root.display()))?;
    if let Some(parent) = options.store_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    if let Some(parent) = options
        .activation_store_path
        .as_ref()
        .and_then(|path| path.parent())
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    Ok(())
}

fn open_node(options: &Options, runtime_id: &str) -> Result<cultmesh_rs::CultMeshNode> {
    CultMesh::create_node(
        &options.store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: runtime_id.to_string(),
            pull_on_start: true,
        },
    )
    .context("opening Muninn CultMesh store")
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Options {
            mode: Mode::Serve,
            store_path: PathBuf::from("C:/Meta/Odin/state/muninn.telemetry.cc"),
            activation_store_path: Some(PathBuf::from(MUNINN_DEFAULT_ACTIVATION_STORE_PATH)),
            surface_id: "muninn.telemetry.local".to_string(),
            stream_id: "muninn.raven.av.rudp".to_string(),
            stream_filter_explicit: false,
            stream_action: "start".to_string(),
            host_id: "raven".to_string(),
            target_host: String::new(),
            port: 5200,
            obs_target_host: None,
            obs_port: 5204,
            media_transport: MediaTransport::Rudp,
            media_packet_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES,
            rudp_video_bitrate_kbps: MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS,
            rudp_latency_budget_ms: MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS as u32,
            width: 1920,
            height: 1080,
            framerate: 30,
            ddagrab_output_index: 0,
            capture_video: true,
            capture_audio: true,
            audio_device: "Realtek".to_string(),
            audio_source_id_override: None,
            video_sources: Vec::new(),
            audio_sources: Vec::new(),
            audio_sample_rate: 48000,
            audio_channels: 2,
            ffmpeg_path: "ffmpeg".to_string(),
            video_encoder_path: None,
            loopback_script: PathBuf::from("scripts/wasapi-loopback-capture.ps1"),
            log_root: PathBuf::from("C:/Meta/Odin/logs/muninn"),
            interval_seconds: None,
            move_id: "move-usb".to_string(),
            move_filter: None,
            hidraw_path: String::new(),
            move_colors: Vec::new(),
            move_durations_ms: Vec::new(),
            move_repeat_count: 1,
            move_hue_cycle_ms: 1_000,
            move_hue_cycle_ms_explicit: false,
            move_hue_mode: None,
            move_hue_order_mode: None,
            command_id: None,
            move_host_address: None,
            move_state_sources: Vec::new(),
            move_marker_camera_sources: Vec::new(),
            move_marker_width: 320,
            move_marker_height: 240,
            move_marker_stride_bytes: None,
            move_marker_fps: 187,
            move_marker_threshold_min: 180,
            move_marker_min_area_px: 4,
            move_marker_max_candidates: 64,
            move_psmoveapi_tracker: false,
            move_light_passive: false,
            move_tracker_exposure_milli: 100,
            move_tracker_camera_exposure_milli: HashMap::new(),
            move_evidence_stream_id: None,
            move_evidence_verse_id: "mimir-live".to_string(),
            move_evidence_ring_slots: 4,
            move_evidence_slot_bytes: 8192,
            move_evidence_snapshot_path: None,
            quest_adb: false,
            quest_serial: None,
            quest_input_stream_id: None,
            quest_pose_stream_id: None,
            quest_video_input_stream_id: None,
            idunn_rudp_health: None,
            odin_cultmesh_uri: Some("cultmesh://odin/rendezvous/provider-catalog".to_string()),
            hid_controller_rudp_target: None,
            hid_controller_rudp_bind: None,
            hid_controller_rudp_advertise: None,
            command_rudp_bind: None,
            command_rudp_advertise: None,
            hid_controller_receipt_retention_seconds: 3_600,
        };

        let mut args = args.peekable();
        let mut idunn_rudp_health_endpoint = None;
        let mut idunn_daemon_id = None;
        let mut idunn_health_contract = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => options.mode = Mode::Serve,
                "activate" => options.mode = Mode::Activate,
                "request-stream" => options.mode = Mode::RequestStream,
                "request-move-light" => options.mode = Mode::RequestMoveLight,
                "set-move-hue-program" => options.mode = Mode::SetMoveHueProgram,
                "move-hue-program-status" => options.mode = Mode::MoveHueProgramStatus,
                "capture-stream-status" => options.mode = Mode::CaptureStreamStatus,
                "obs-catalog-status" => options.mode = Mode::ObsCatalogStatus,
                "move-light-status" => options.mode = Mode::MoveLightStatus,
                "move-identity-status" => options.mode = Mode::MoveIdentityStatus,
                "move-source-status" => options.mode = Mode::MoveSourceStatus,
                "move-state-status" => options.mode = Mode::MoveStateStatus,
                "claim-move-host" => options.mode = Mode::ClaimMoveHost,
                "quest-access-status" => options.mode = Mode::QuestAccessStatus,
                "move-tracker-worker" => options.mode = Mode::MoveTrackerWorker,
                "--health" => options.mode = Mode::Health,
                "--dry-run" => options.mode = Mode::DryRun,
                "--store" => options.store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--activate-store" => {
                    options.activation_store_path =
                        Some(PathBuf::from(take_value(&mut args, "--activate-store")?))
                }
                "--surface" => options.surface_id = take_value(&mut args, "--surface")?,
                "--stream" => {
                    options.stream_id = take_value(&mut args, "--stream")?;
                    options.stream_filter_explicit = true;
                }
                "--stream-action" => {
                    options.stream_action = take_value(&mut args, "--stream-action")?
                }
                "--host" => options.host_id = take_value(&mut args, "--host")?,
                "--target-host" => options.target_host = take_value(&mut args, "--target-host")?,
                "--port" => options.port = take_value(&mut args, "--port")?.parse()?,
                "--obs-target-host" => {
                    options.obs_target_host = Some(take_value(&mut args, "--obs-target-host")?)
                }
                "--obs-port" => options.obs_port = take_value(&mut args, "--obs-port")?.parse()?,
                "--media-transport" => {
                    options.media_transport =
                        parse_media_transport(&take_value(&mut args, "--media-transport")?)?
                }
                "--media-packet-bytes" => {
                    options.media_packet_bytes =
                        take_value(&mut args, "--media-packet-bytes")?.parse()?
                }
                "--rudp-video-bitrate-kbps" => {
                    let bitrate = take_value(&mut args, "--rudp-video-bitrate-kbps")?
                        .parse()
                        .context("--rudp-video-bitrate-kbps must be a positive integer")?;
                    if bitrate == 0 {
                        return Err(anyhow!(
                            "--rudp-video-bitrate-kbps must be greater than zero"
                        ));
                    }
                    options.rudp_video_bitrate_kbps = bitrate;
                }
                "--rudp-latency-budget-ms" => {
                    let latency_budget = take_value(&mut args, "--rudp-latency-budget-ms")?
                        .parse()
                        .context("--rudp-latency-budget-ms must be a positive integer")?;
                    if latency_budget == 0 {
                        return Err(anyhow!(
                            "--rudp-latency-budget-ms must be greater than zero"
                        ));
                    }
                    options.rudp_latency_budget_ms = latency_budget;
                }
                "--width" => options.width = take_value(&mut args, "--width")?.parse()?,
                "--height" => options.height = take_value(&mut args, "--height")?.parse()?,
                "--framerate" => {
                    options.framerate = take_value(&mut args, "--framerate")?.parse()?
                }
                "--ddagrab-output-index" => {
                    options.ddagrab_output_index =
                        take_value(&mut args, "--ddagrab-output-index")?.parse()?
                }
                "--no-video" => options.capture_video = false,
                "--no-audio" => options.capture_audio = false,
                "--audio-device" => options.audio_device = take_value(&mut args, "--audio-device")?,
                "--audio-source-id" => {
                    options.audio_source_id_override =
                        Some(take_value(&mut args, "--audio-source-id")?)
                }
                "--video-source" => options.video_sources.push(parse_catalog_source(&take_value(
                    &mut args,
                    "--video-source",
                )?)?),
                "--audio-source" => options.audio_sources.push(parse_catalog_source(&take_value(
                    &mut args,
                    "--audio-source",
                )?)?),
                "--audio-sample-rate" => {
                    options.audio_sample_rate =
                        take_value(&mut args, "--audio-sample-rate")?.parse()?
                }
                "--audio-channels" => {
                    options.audio_channels = take_value(&mut args, "--audio-channels")?.parse()?
                }
                "--ffmpeg" => options.ffmpeg_path = take_value(&mut args, "--ffmpeg")?,
                "--video-encoder" => {
                    options.video_encoder_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--video-encoder",
                    )?))
                }
                "--loopback-script" => {
                    options.loopback_script =
                        PathBuf::from(take_value(&mut args, "--loopback-script")?)
                }
                "--log-root" => {
                    options.log_root = PathBuf::from(take_value(&mut args, "--log-root")?)
                }
                "--interval-seconds" => {
                    options.interval_seconds = Some(
                        take_value(&mut args, "--interval-seconds")?
                            .parse()
                            .context("--interval-seconds must be a positive integer")?,
                    )
                }
                "--move" => {
                    let value = take_value(&mut args, "--move")?;
                    options.move_id = value.clone();
                    options.move_filter = Some(value);
                }
                "--hidraw" => options.hidraw_path = take_value(&mut args, "--hidraw")?,
                "--move-state" => {
                    let value = take_value(&mut args, "--move-state")?;
                    options
                        .move_state_sources
                        .push(parse_move_state_source(&value)?);
                }
                "--move-marker-camera" => {
                    let value = take_value(&mut args, "--move-marker-camera")?;
                    options
                        .move_marker_camera_sources
                        .push(parse_move_marker_camera_source(&value)?);
                }
                "--move-marker-width" => {
                    options.move_marker_width =
                        take_value(&mut args, "--move-marker-width")?.parse()?
                }
                "--move-marker-height" => {
                    options.move_marker_height =
                        take_value(&mut args, "--move-marker-height")?.parse()?
                }
                "--move-marker-stride-bytes" => {
                    options.move_marker_stride_bytes =
                        Some(take_value(&mut args, "--move-marker-stride-bytes")?.parse()?);
                }
                "--move-marker-fps" => {
                    options.move_marker_fps = take_value(&mut args, "--move-marker-fps")?.parse()?
                }
                "--move-marker-threshold-min" => {
                    options.move_marker_threshold_min =
                        take_value(&mut args, "--move-marker-threshold-min")?.parse()?
                }
                "--move-marker-min-area-px" => {
                    options.move_marker_min_area_px =
                        take_value(&mut args, "--move-marker-min-area-px")?.parse()?
                }
                "--move-marker-max-candidates" => {
                    options.move_marker_max_candidates =
                        take_value(&mut args, "--move-marker-max-candidates")?.parse()?
                }
                "--move-psmoveapi-tracker" => options.move_psmoveapi_tracker = true,
                "--move-light-passive" => options.move_light_passive = true,
                "--move-tracker-exposure-milli" => {
                    options.move_tracker_exposure_milli =
                        take_value(&mut args, "--move-tracker-exposure-milli")?.parse()?
                }
                "--move-tracker-camera-exposure" => {
                    let value = take_value(&mut args, "--move-tracker-camera-exposure")?;
                    let (camera_id, exposure) = value.split_once('=')
                        .context("--move-tracker-camera-exposure must be camera-id=0..1000")?;
                    let exposure = exposure.parse::<u32>()
                        .context("--move-tracker-camera-exposure must be camera-id=0..1000")?;
                    if camera_id.trim().is_empty() || exposure > 1000 {
                        return Err(anyhow!("--move-tracker-camera-exposure must be camera-id=0..1000"));
                    }
                    options.move_tracker_camera_exposure_milli.insert(camera_id.to_string(), exposure);
                }
                "--move-evidence-stream" => {
                    options.move_evidence_stream_id =
                        Some(take_value(&mut args, "--move-evidence-stream")?)
                }
                "--move-evidence-verse" => {
                    options.move_evidence_verse_id = take_value(&mut args, "--move-evidence-verse")?
                }
                "--move-evidence-ring-slots" => {
                    options.move_evidence_ring_slots =
                        take_value(&mut args, "--move-evidence-ring-slots")?
                            .parse()
                            .context("--move-evidence-ring-slots must be a positive integer")?
                }
                "--move-evidence-slot-bytes" => {
                    options.move_evidence_slot_bytes =
                        take_value(&mut args, "--move-evidence-slot-bytes")?
                            .parse()
                            .context("--move-evidence-slot-bytes must be a positive integer")?
                }
                "--move-evidence-snapshot" => {
                    let value = take_value(&mut args, "--move-evidence-snapshot")?;
                    if value.trim().is_empty() {
                        return Err(anyhow!("--move-evidence-snapshot must be non-empty"));
                    }

                    options.move_evidence_snapshot_path = Some(PathBuf::from(value));
                }
                "--quest-adb" => options.quest_adb = true,
                "--quest-serial" => {
                    options.quest_serial = Some(take_value(&mut args, "--quest-serial")?)
                }
                "--quest-input-stream" => {
                    options.quest_input_stream_id =
                        Some(take_value(&mut args, "--quest-input-stream")?)
                }
                "--quest-pose-stream" => {
                    options.quest_pose_stream_id =
                        Some(take_value(&mut args, "--quest-pose-stream")?)
                }
                "--quest-video-input-stream" => {
                    options.quest_video_input_stream_id =
                        Some(take_value(&mut args, "--quest-video-input-stream")?)
                }
                "--idunn-rudp-health" => {
                    idunn_rudp_health_endpoint = Some(
                        take_value(&mut args, "--idunn-rudp-health")?
                            .parse()
                            .context("--idunn-rudp-health must be a socket address")?,
                    )
                }
                "--idunn-daemon" => {
                    idunn_daemon_id = Some(take_value(&mut args, "--idunn-daemon")?)
                }
                "--idunn-health-contract" => {
                    idunn_health_contract = Some(take_value(&mut args, "--idunn-health-contract")?)
                }
                "--odin-cultmesh-rudp" => {
                    let _ = take_value(&mut args, "--odin-cultmesh-rudp")?;
                    return Err(anyhow!(
                        "--odin-cultmesh-rudp has been removed; use --odin-cultmesh-uri cultmesh://odin/rendezvous/provider-catalog and let CultMesh resolve Odin's transport"
                    ));
                }
                "--odin-cultmesh-uri" => {
                    options.odin_cultmesh_uri = Some(take_value(&mut args, "--odin-cultmesh-uri")?)
                }
                "--hid-controller-rudp-target" | "--hid-controller-udp-target" => {
                    options.hid_controller_rudp_target = Some(
                        take_value(&mut args, &arg)?
                            .parse()
                            .context("--hid-controller-rudp-target must be a socket address")?,
                    )
                }
                "--hid-controller-rudp-bind" => {
                    options.hid_controller_rudp_bind = Some(
                        take_value(&mut args, "--hid-controller-rudp-bind")?
                            .parse()
                            .context("--hid-controller-rudp-bind must be a socket address")?,
                    )
                }
                "--hid-controller-rudp-advertise" => {
                    options.hid_controller_rudp_advertise =
                        Some(take_value(&mut args, "--hid-controller-rudp-advertise")?)
                }
                "--command-rudp-bind" => {
                    options.command_rudp_bind = Some(
                        take_value(&mut args, "--command-rudp-bind")?
                            .parse()
                            .context("--command-rudp-bind must be a socket address")?,
                    )
                }
                "--command-rudp-advertise" => {
                    options.command_rudp_advertise =
                        Some(take_value(&mut args, "--command-rudp-advertise")?)
                }
                "--hid-controller-receipt-retention-seconds" => {
                    options.hid_controller_receipt_retention_seconds = take_value(
                        &mut args,
                        "--hid-controller-receipt-retention-seconds",
                    )?
                    .parse()
                    .context(
                        "--hid-controller-receipt-retention-seconds must be a positive integer",
                    )?;
                    if options.hid_controller_receipt_retention_seconds == 0 {
                        return Err(anyhow!(
                            "--hid-controller-receipt-retention-seconds must be greater than zero"
                        ));
                    }
                }
                "--capture-command-rudp-bind" => {
                    let _ = take_value(&mut args, "--capture-command-rudp-bind")?;
                    return Err(anyhow!(
                        "--capture-command-rudp-bind has been removed; Muninn serve reads capture commands from Odin/CultMesh discovery"
                    ));
                }
                "--capture-command-rudp-target" => {
                    let _ = take_value(&mut args, "--capture-command-rudp-target")?;
                    return Err(anyhow!(
                        "--capture-command-rudp-target has been removed; use --odin-cultmesh-uri so request-stream publishes through Odin/CultMesh"
                    ));
                }
                "--color" => options.move_colors.push(take_value(&mut args, "--color")?),
                "--duration-ms" => options
                    .move_durations_ms
                    .push(take_value(&mut args, "--duration-ms")?.parse()?),
                "--repeat-count" => {
                    options.move_repeat_count = take_value(&mut args, "--repeat-count")?
                        .parse()
                        .context("--repeat-count must be a positive integer")?
                }
                "--move-hue-cycle-ms" => {
                    options.move_hue_cycle_ms = take_value(&mut args, "--move-hue-cycle-ms")?
                        .parse()
                        .context("--move-hue-cycle-ms must be a positive integer")?;
                    options.move_hue_cycle_ms_explicit = true;
                }
                "--move-hue-mode" => {
                    options.move_hue_mode = Some(take_value(&mut args, "--move-hue-mode")?)
                }
                "--move-hue-order" => {
                    options.move_hue_order_mode =
                        Some(take_value(&mut args, "--move-hue-order")?)
                }
                "--command" => options.command_id = Some(take_value(&mut args, "--command")?),
                "--move-host" => {
                    options.move_host_address = Some(take_value(&mut args, "--move-host")?)
                }
                "--help" | "-h" => return Err(anyhow!(help_text())),
                other => {
                    return Err(anyhow!(
                        "unknown Muninn argument: {other}\n\n{}",
                        help_text()
                    ));
                }
            }
        }

        if options.interval_seconds == Some(0) {
            return Err(anyhow!("--interval-seconds must be greater than zero"));
        }
        if options.move_hue_cycle_ms == 0 {
            return Err(anyhow!("--move-hue-cycle-ms must be greater than zero"));
        }
        if !options.capture_video && !options.capture_audio {
            return Err(anyhow!(
                "Muninn activation must keep at least one leg alive; do not pass both --no-video and --no-audio"
            ));
        }
        if options.move_evidence_verse_id.trim().is_empty() {
            return Err(anyhow!("--move-evidence-verse must be non-empty"));
        }
        if options
            .move_evidence_stream_id
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(anyhow!("--move-evidence-stream must be non-empty"));
        }
        if options.move_evidence_ring_slots == 0 {
            return Err(anyhow!(
                "--move-evidence-ring-slots must be greater than zero"
            ));
        }
        if options.move_evidence_slot_bytes == 0 {
            return Err(anyhow!(
                "--move-evidence-slot-bytes must be greater than zero"
            ));
        }
        if options
            .move_evidence_snapshot_path
            .as_ref()
            .is_some_and(|path| path.as_os_str().is_empty())
        {
            return Err(anyhow!("--move-evidence-snapshot must be non-empty"));
        }
        if options.move_marker_width == 0 {
            return Err(anyhow!("--move-marker-width must be greater than zero"));
        }
        if options.move_marker_height == 0 {
            return Err(anyhow!("--move-marker-height must be greater than zero"));
        }
        if options
            .move_marker_stride_bytes
            .is_some_and(|stride| stride < options.move_marker_width)
        {
            return Err(anyhow!(
                "--move-marker-stride-bytes must be at least --move-marker-width"
            ));
        }
        if options.move_marker_fps == 0 {
            return Err(anyhow!("--move-marker-fps must be greater than zero"));
        }
        if options.move_marker_min_area_px == 0 {
            return Err(anyhow!(
                "--move-marker-min-area-px must be greater than zero"
            ));
        }
        if options.move_marker_max_candidates == 0 {
            return Err(anyhow!(
                "--move-marker-max-candidates must be greater than zero"
            ));
        }
        if options.move_tracker_exposure_milli > 1000 {
            return Err(anyhow!("--move-tracker-exposure-milli must be within 0..=1000"));
        }
        if options.move_psmoveapi_tracker && !cfg!(feature = "psmoveapi-tracker") {
            return Err(anyhow!(
                "--move-psmoveapi-tracker requires a Muninn build with the psmoveapi-tracker feature"
            ));
        }
        for (name, value) in [
            ("--quest-serial", options.quest_serial.as_ref()),
            (
                "--quest-input-stream",
                options.quest_input_stream_id.as_ref(),
            ),
            ("--quest-pose-stream", options.quest_pose_stream_id.as_ref()),
            (
                "--quest-video-input-stream",
                options.quest_video_input_stream_id.as_ref(),
            ),
            ("--move-host", options.move_host_address.as_ref()),
            ("--idunn-daemon", idunn_daemon_id.as_ref()),
            ("--idunn-health-contract", idunn_health_contract.as_ref()),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(anyhow!("{name} must be non-empty"));
            }
        }
        options.idunn_rudp_health = match (
            idunn_rudp_health_endpoint,
            idunn_daemon_id,
            idunn_health_contract,
        ) {
            (None, None, None) => None,
            (Some(endpoint), Some(daemon_id), Some(health_contract)) => {
                Some(IdunnRudpHealthOptions {
                    endpoint,
                    daemon_id,
                    health_contract,
                })
            }
            _ => {
                return Err(anyhow!(
                    "--idunn-rudp-health, --idunn-daemon, and --idunn-health-contract must be provided together"
                ));
            }
        };
        Ok(options)
    }
}

fn take_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn timestamp() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    Ok(format!("unix-{seconds}"))
}

fn idunn_timestamp() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    Ok(format!("unix:{seconds}"))
}

fn timestamp_ns() -> Result<i64> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    i64::try_from(nanos).context("system timestamp does not fit i64 nanoseconds")
}

fn media_transport_id(transport: &MediaTransport) -> &'static str {
    match transport {
        MediaTransport::Rudp => CULTNET_RUDP_PROTOCOL_ID,
    }
}

fn media_transport_cli(transport: &MediaTransport) -> &'static str {
    match transport {
        MediaTransport::Rudp => "rudp",
    }
}

fn parse_media_transport(value: &str) -> Result<MediaTransport> {
    match value.to_ascii_lowercase().as_str() {
        "rudp" | "cultnet-rudp" | "cultmesh-rudp" | CULTNET_RUDP_PROTOCOL_ID => {
            Ok(MediaTransport::Rudp)
        }
        _ => Err(anyhow!(
            "--media-transport must be one of: rudp, cultnet-rudp"
        )),
    }
}

fn parse_catalog_source(value: &str) -> Result<CatalogSource> {
    let Some((id, label)) = value.split_once('=') else {
        return Err(anyhow!(
            "source catalog entries must be formatted as <source-id>=<display-label>"
        ));
    };
    let id = id.trim();
    let label = label.trim();
    if id.is_empty() || label.is_empty() {
        return Err(anyhow!(
            "source catalog entries must include a non-empty source id and display label"
        ));
    }
    Ok(CatalogSource {
        id: id.to_string(),
        label: label.to_string(),
    })
}

fn parse_move_marker_camera_source(value: &str) -> Result<MoveMarkerCameraSource> {
    let Some((camera_id, device_path)) = value.split_once('=') else {
        return Err(anyhow!(
            "--move-marker-camera must be formatted as <camera-id>=<device-path>"
        ));
    };
    if camera_id.trim().is_empty() || device_path.trim().is_empty() {
        return Err(anyhow!(
            "--move-marker-camera requires non-empty camera id and device path"
        ));
    }
    Ok(MoveMarkerCameraSource {
        camera_id: camera_id.to_string(),
        device_path: PathBuf::from(device_path),
    })
}

fn help_text() -> &'static str {
    "Usage: muninn [serve|activate|request-stream|capture-stream-status|obs-catalog-status|request-move-light|move-light-status|move-identity-status|move-source-status|move-state-status|claim-move-host|quest-access-status] [--store <path>] [--activate-store <path>] [--stream-action <start|stop>] [--target-host <cultmesh-uri>] [--media-transport <rudp>] [--media-packet-bytes <bytes>] [--rudp-video-bitrate-kbps <kbps>] [--rudp-latency-budget-ms <ms>] [--video-source <source-id=label>] [--audio-source <source-id=label>] [--audio-source-id <source-id>] [--no-video] [--no-audio] [--loopback-script <path>] [--ffmpeg <path>] [--odin-cultmesh-uri <cultmesh-uri>] [--move-state <move-id>=<hidraw-path>] [--move-marker-camera <camera-id>=<device-path>] [--move-psmoveapi-tracker] [--move-tracker-exposure-milli <0..1000>] [--move-marker-width <px>] [--move-marker-height <px>] [--move-marker-fps <fps>] [--move-host <bt-addr>] [--move-evidence-stream <stream-id>] [--move-evidence-verse <verse-id>] [--move-evidence-ring-slots <slots>] [--move-evidence-slot-bytes <bytes>] [--move-evidence-snapshot <path>] [--quest-adb] [--quest-serial <serial>] [--quest-input-stream <stream-id>] [--quest-pose-stream <stream-id>] [--quest-video-input-stream <stream-id>] [--idunn-rudp-health <addr>] [--idunn-daemon <id>] [--idunn-health-contract <contract>] [--dry-run] [--health]\n\nMuninn is Odin's portable telemetry Verse assembler. serve publishes cheap typed telemetry affordances, optional Quest USB access surfaces, and the explicitly configured Move runtime; when serve receives --move-state, --move-marker-camera, --move-host, or --move-evidence-stream it may publish source-local Move controller state, source-local optical marker candidates, typed Move identity records, a CultMesh Move evidence stream, optionally write a latest one-copy Move proof evidence snapshot for Mimir field capture/replay, and keep USB-attached PS Moves claimed to that explicit Bluetooth host; --move-psmoveapi-tracker delegates camera exposure and optical extraction to the reference PSMoveAPI backend while Muninn retains stable-ID light actuation; serve consumes typed capture stream commands from its provider-owned activation store and owns the local ffmpeg/loopback activation child lifecycle, resolves media targets from Odin/CultMesh provider advertisements, and publishes its discovery advertisement through --odin-cultmesh-uri; activate starts an explicitly requested local CultNet RUDP stream as a daemon child after resolving the cultmesh:// media target URI through Odin; request-stream discovers the provider through Odin and sends its typed command to that provider; obs-catalog-status pulls Odin-owned muninn.obs_stream_catalog discovery into the local compatibility store for OBS; capture-stream-status reads typed capture stream command receipts; use --no-video or --no-audio to request one leg over the CultNet RUDP media lane; request-move-light publishes a typed Move light command for Muninn serve to execute; move-light-status reads typed command receipts; move-identity-status reads typed Move identity records; move-source-status prints live Move source discovery; move-state-status reads typed controller-state records; claim-move-host assigns USB-attached PS Moves to a Bluetooth host; quest-access-status reads typed Quest access state. In --health mode, the Idunn RUDP flags publish the same typed daemon health document to Idunn for explicit diagnostics."
}

fn parse_move_state_source(value: &str) -> Result<MoveStateSource> {
    let Some((move_id, hidraw_path)) = value.split_once('=') else {
        return Err(anyhow!(
            "--move-state must be formatted as <move-id>=<hidraw-path>"
        ));
    };
    if move_id.trim().is_empty() || hidraw_path.trim().is_empty() {
        return Err(anyhow!(
            "--move-state requires non-empty move id and hidraw path"
        ));
    }
    Ok(MoveStateSource {
        move_id: move_id.to_string(),
        hidraw_path: hidraw_path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Default)]
    struct RecordingMoveLightWriter {
        writes: Vec<(String, Vec<u8>)>,
    }

    impl MoveLightWriter for RecordingMoveLightWriter {
        fn write_report(&mut self, hidraw_path: &str, report: &[u8]) -> Result<()> {
            self.writes.push((hidraw_path.to_string(), report.to_vec()));
            Ok(())
        }
    }

    fn pending_move_light_command() -> MuninnMoveLightCommandRecord {
        MuninnMoveLightCommandRecord {
            command_id: "cmd-1".to_string(),
            host_id: "raven".to_string(),
            move_id: "move-1".to_string(),
            hidraw_path: "/dev/hidraw3".to_string(),
            colors: vec!["#ff4008".to_string()],
            durations_ms: vec![0],
            repeat_count: 1,
            authority: "mimir.structured-light".to_string(),
            state: "pending".to_string(),
            detail: "test".to_string(),
            updated_at: "unix-0".to_string(),
        }
    }

    #[test]
    fn serve_is_default_and_does_not_activate_streams() {
        let options = Options::parse([].into_iter()).unwrap();

        assert_eq!(options.mode, Mode::Serve);
        assert!(options.interval_seconds.is_none());
        assert_eq!(
            options.activation_store_path,
            Some(PathBuf::from(MUNINN_DEFAULT_ACTIVATION_STORE_PATH))
        );
        assert_eq!(
            options.rudp_video_bitrate_kbps,
            MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS
        );
    }

    #[test]
    fn serve_can_publish_distinct_activation_store_path() {
        let options = Options::parse(
            [
                "serve",
                "--store",
                "C:/Meta/Odin/state/muninn.telemetry.cc",
                "--activate-store",
                "C:/Meta/Odin/state/muninn.activate.cc",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            options.activation_store_path,
            Some(PathBuf::from("C:/Meta/Odin/state/muninn.activate.cc"))
        );
    }

    #[test]
    fn parse_accepts_independent_obs_source_catalog_entries() {
        let options = Options::parse(
            [
                "serve",
                "--video-source",
                "display:0=Raven display 1",
                "--video-source",
                "display:1=Raven display 2",
                "--audio-source",
                "wasapi-loopback:Realtek=Raven Realtek loopback",
                "--audio-source",
                "wasapi-loopback:Headphones=Raven headphones loopback",
                "--audio-source",
                "wasapi-input:Microphone=Raven microphone input",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            video_source_catalog(&options),
            vec![
                CatalogSource {
                    id: "display:0".to_string(),
                    label: "Raven display 1".to_string(),
                },
                CatalogSource {
                    id: "display:1".to_string(),
                    label: "Raven display 2".to_string(),
                }
            ]
        );
        assert_eq!(
            audio_source_catalog(&options),
            vec![
                CatalogSource {
                    id: "wasapi-loopback:Realtek".to_string(),
                    label: "Raven Realtek loopback".to_string(),
                },
                CatalogSource {
                    id: "wasapi-loopback:Headphones".to_string(),
                    label: "Raven headphones loopback".to_string(),
                },
                CatalogSource {
                    id: "wasapi-input:Microphone".to_string(),
                    label: "Raven microphone input".to_string(),
                }
            ]
        );
    }

    #[test]
    fn parse_audio_source_spec_accepts_loopback_and_input_sources() {
        assert_eq!(
            parse_audio_source_spec("wasapi-loopback:Realtek"),
            Some(AudioSourceSpec {
                kind: AudioSourceKind::Loopback,
                device: "Realtek".to_string(),
            })
        );
        assert_eq!(
            parse_audio_source_spec("wasapi-input:Microphone"),
            Some(AudioSourceSpec {
                kind: AudioSourceKind::Input,
                device: "Microphone".to_string(),
            })
        );
    }

    #[test]
    fn obs_source_catalog_falls_back_to_configured_capture_pair() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "raven",
                "--ddagrab-output-index",
                "1",
                "--audio-device",
                "Realtek",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            video_source_catalog(&options),
            vec![CatalogSource {
                id: "display:1".to_string(),
                label: "raven display 2".to_string(),
            }]
        );
        assert_eq!(
            audio_source_catalog(&options),
            vec![CatalogSource {
                id: "wasapi-loopback:Realtek".to_string(),
                label: "raven loopback (Realtek)".to_string(),
            }]
        );
    }

    #[test]
    fn parse_accepts_single_leg_rudp_activation_requests() {
        let video_only = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--no-audio",
                "--stream",
                "muninn.raven.video.rudp",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert!(video_only.capture_video);
        assert!(!video_only.capture_audio);
        assert_eq!(
            video_source_id_for_options(&video_only),
            "display:0".to_string()
        );
        assert_eq!(
            audio_source_id_for_options(&video_only),
            MUNINN_DISABLED_AUDIO_SOURCE_ID.to_string()
        );

        let audio_only = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--no-video",
                "--stream",
                "muninn.raven.audio.rudp",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert!(!audio_only.capture_video);
        assert!(audio_only.capture_audio);
        assert_eq!(
            video_source_id_for_options(&audio_only),
            MUNINN_DISABLED_VIDEO_SOURCE_ID.to_string()
        );
        assert_eq!(
            audio_source_id_for_options(&audio_only),
            "wasapi-loopback:Realtek".to_string()
        );
    }

    #[test]
    fn loopback_args_switch_to_capture_mode_for_input_sources() {
        let options = Options::parse(
            [
                "activate",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--audio-source-id",
                "wasapi-input:Microphone (USB Audio)",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        let args = loopback_args(&options);
        assert!(args.contains(&"Capture".to_string()));
        assert!(!args.contains(&"-Loopback".to_string()));
        assert!(args.contains(&"Microphone (USB Audio)".to_string()));
    }

    #[test]
    fn parse_rejects_activation_when_both_media_legs_are_disabled() {
        let error = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--no-video",
                "--no-audio",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("at least one leg alive"));
    }

    #[test]
    fn media_activation_requires_cultmesh_target_uri() {
        let options = Options::parse(["activate"].into_iter().map(String::from)).unwrap();

        let error = require_media_target_uri(&options).unwrap_err().to_string();
        assert!(error.contains("--target-host is required"));

        let options = Options::parse(
            ["activate", "--target-host", "198.51.100.66"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let error = require_media_target_uri(&options).unwrap_err().to_string();
        assert!(error.contains("cultmesh://"));

        let options = Options::parse(["request-stream"].into_iter().map(String::from)).unwrap();

        let error = build_capture_stream_command(&options)
            .unwrap_err()
            .to_string();
        assert!(error.contains("--target-host is required"));
    }

    #[test]
    fn parse_rejects_blank_obs_source_catalog_entries() {
        let error = Options::parse(
            ["serve", "--video-source", "display:0="]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err();

        assert!(error.to_string().contains("non-empty source id"));
    }

    #[test]
    fn health_idunn_rudp_options_are_explicit_bundle() {
        let options = Options::parse(
            [
                "--health",
                "--idunn-rudp-health",
                "127.0.0.1:17870",
                "--idunn-daemon",
                "starfire-muninn",
                "--idunn-health-contract",
                "muninn.cultnet-rudp-local-telemetry-and-quest-access",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        let idunn = options.idunn_rudp_health.unwrap();
        assert_eq!(options.mode, Mode::Health);
        assert_eq!(idunn.endpoint, "127.0.0.1:17870".parse().unwrap());
        assert_eq!(idunn.daemon_id, "starfire-muninn");
        assert_eq!(
            idunn.health_contract,
            "muninn.cultnet-rudp-local-telemetry-and-quest-access"
        );

        let error = Options::parse(
            ["--health", "--idunn-rudp-health", "127.0.0.1:17870"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("must be provided together"));
    }

    #[test]
    fn serve_accepts_odin_cultmesh_uri_startup_target() {
        let options = Options::parse(
            [
                "serve",
                "--odin-cultmesh-uri",
                "cultmesh://odin/rendezvous/provider-catalog",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            options.odin_cultmesh_uri.as_deref(),
            Some("cultmesh://odin/rendezvous/provider-catalog")
        );
    }

    #[test]
    fn serve_rejects_removed_odin_cultmesh_rudp_startup_target() {
        let error = Options::parse(
            ["serve", "--odin-cultmesh-rudp", "203.0.113.10:17871"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--odin-cultmesh-rudp has been removed"));
    }

    #[test]
    fn request_stream_rejects_raw_capture_command_rudp_target() {
        let error = Options::parse(
            [
                "request-stream",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--capture-command-rudp-target",
                "127.0.0.1:17872",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--capture-command-rudp-target has been removed"));
    }

    #[test]
    fn serve_rejects_removed_capture_command_rudp_bind() {
        let error = Options::parse(
            ["serve", "--capture-command-rudp-bind", "127.0.0.1:17884"]
                .into_iter()
                .map(String::from),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--capture-command-rudp-bind has been removed"));
    }

    #[test]
    fn request_stream_requires_odin_cultmesh_command_route() {
        let options = Options::parse(
            [
                "request-stream",
                "--host",
                "raven",
                "--stream",
                "muninn.raven.av.rudp",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let error = request_capture_stream(options).unwrap_err().to_string();

        assert!(error.contains("request-stream requires --odin-cultmesh-uri"));
    }

    #[test]
    fn idunn_rudp_health_publisher_sends_raw_daemon_health_document() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut transport = CultNetRudpSocketTransportConnection::new(
                CultNetRudpSocketTransportOptions::server(
                    "idunn-test",
                    receiver,
                    IDUNN_HEALTH_RUDP_CONNECTION_ID,
                ),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(frame) = transport.receive_once().unwrap() {
                    return cultnet_rs::decode_cultnet_message_from_slice(
                        &frame.payload,
                        CultNetWireContract::CultNetSchemaV0,
                    )
                    .unwrap();
                }
                transport.poll_resends().unwrap();
                if Instant::now() >= deadline {
                    panic!("timed out waiting for Muninn RUDP health frame");
                }
            }
        });
        let options = IdunnRudpHealthOptions {
            endpoint: receiver_addr,
            daemon_id: "starfire-muninn".to_string(),
            health_contract: "muninn.cultnet-rudp-local-telemetry-and-quest-access".to_string(),
        };

        publish_idunn_rudp_health(&options, "active", "Muninn healthy", "unix:100").unwrap();

        let message = handle.join().unwrap();
        let CultNetMessage::DocumentPutRaw { document, .. } = message else {
            panic!("expected raw document put");
        };
        assert_eq!(document.schema_id, "idunn.daemon_health");
        assert_eq!(document.record_key, "starfire-muninn");
        assert_eq!(
            document.payload_encoding,
            CultNetRawPayloadEncoding::Messagepack
        );
        let health: IdunnDaemonHealthRecord = rmp_serde::from_slice(&document.payload).unwrap();
        assert_eq!(health.daemon_id, "starfire-muninn");
        assert_eq!(health.state, "active");
        assert_eq!(
            health.health_contract,
            "muninn.cultnet-rudp-local-telemetry-and-quest-access"
        );
        assert_eq!(health.publication_source, "daemon-published");
        assert_eq!(health.transport, CULTNET_RUDP_PROTOCOL_ID);
        assert_eq!(health.observed_at, "unix:100");
    }

    #[test]
    fn repeated_capture_stream_start_is_equivalent_when_media_shape_matches() {
        let options = Options::parse(
            [
                "request-stream",
                "--host",
                "raven",
                "--stream",
                "muninn.raven.av.rudp",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--port",
                "5204",
                "--media-transport",
                "rudp",
                "--media-packet-bytes",
                "800",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut active = build_capture_stream_command(&options).unwrap();
        active.command_id = "first-command".to_string();
        active.state = "running".to_string();
        let mut retry = build_capture_stream_command(&options).unwrap();
        retry.command_id = "retry-command".to_string();

        assert!(capture_stream_commands_start_equivalent(&active, &retry));

        retry.media_packet_bytes = 940;
        assert!(!capture_stream_commands_start_equivalent(&active, &retry));
    }

    #[test]
    fn repeated_capture_stream_start_is_not_equivalent_when_split_leg_changes() {
        let mut video_only = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--media-transport",
                    "rudp",
                    "--stream",
                    "muninn.raven.video.rudp",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                    "--no-audio",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        video_only.command_id = "video-only".to_string();
        video_only.state = "running".to_string();

        let mut combined = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--media-transport",
                    "rudp",
                    "--stream",
                    "muninn.raven.video.rudp",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        combined.command_id = "combined".to_string();

        assert!(!capture_stream_commands_start_equivalent(
            &video_only,
            &combined
        ));
        assert_eq!(
            command_audio_source_id(&video_only),
            MUNINN_DISABLED_AUDIO_SOURCE_ID
        );
        assert!(command_requests_video(&video_only));
        assert!(!command_requests_audio(&video_only));
    }

    #[test]
    fn repeated_capture_stream_start_is_equivalent_across_transport_specific_stream_ids() {
        let mut active = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--host",
                    "raven",
                    "--stream",
                    "muninn.raven.av.rudp",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                    "--port",
                    "5204",
                    "--media-transport",
                    "rudp",
                    "--audio-source-id",
                    "wasapi-loopback:Speakers (Realtek(R) Audio)",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        active.command_id = "active-command".to_string();
        active.state = "running".to_string();

        let mut retry = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--host",
                    "raven",
                    "--stream",
                    "muninn.raven.av.rudp",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                    "--port",
                    "5204",
                    "--media-transport",
                    "rudp",
                    "--audio-source-id",
                    "wasapi-loopback:Speakers (Realtek(R) Audio)",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        retry.command_id = "retry-command".to_string();

        assert_eq!(
            canonical_muninn_stream_id(&active.stream_id),
            canonical_muninn_stream_id(&retry.stream_id)
        );
        assert!(capture_stream_commands_start_equivalent(&active, &retry));
    }

    #[test]
    fn latest_capture_stream_command_ids_canonicalizes_transport_specific_variants() {
        let mut older = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--host",
                    "raven",
                    "--stream",
                    "muninn.raven.av.rudp",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                    "--port",
                    "5204",
                    "--media-transport",
                    "rudp",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        older.command_id = "older-command".to_string();
        older.updated_at = "unix-1000".to_string();

        let mut newer = older.clone();
        newer.stream_id = "muninn.raven.av.rudp".to_string();
        newer.command_id = "newer-command".to_string();
        newer.updated_at = "unix-2000".to_string();

        let latest = latest_capture_stream_command_ids(&[older, newer], "raven");

        assert_eq!(latest.len(), 1);
        assert_eq!(
            latest.get("muninn.raven.av").map(String::as_str),
            Some("newer-command")
        );
    }

    #[test]
    fn media_target_contract_accepts_only_cultmesh_rudp() {
        let options = Options::parse(
            [
                "activate",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--media-transport",
                "rudp",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(options.media_transport, MediaTransport::Rudp);
        assert!(require_media_target_uri(&options).is_ok());

        let raw_target_options = Options::parse(
            ["activate", "--target-host", "198.51.100.66"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let error = require_media_target_uri(&raw_target_options)
            .unwrap_err()
            .to_string();
        assert!(error.contains("cultmesh://"));
    }

    #[test]
    fn builds_rudp_media_target_for_obs_plugin_activation() {
        let options = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--stream",
                "muninn.raven.av.rudp",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--port",
                "5204",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(options.media_packet_bytes, MUNINN_RUDP_MEDIA_PACKET_BYTES);
        let plan = build_mux_plan(&options, "test".to_string());

        assert_eq!(
            plan.targets,
            vec!["cultmesh://odin/media/muninn-raven-av#muninn.raven.av.rudp"]
        );
        assert!(!plan.command_line.contains("tee"));
    }

    #[test]
    fn build_capture_stream_command_marks_disabled_legs_explicitly() {
        let video_only = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--media-transport",
                    "rudp",
                    "--stream",
                    "muninn.raven.video.rudp",
                    "--no-audio",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                    "--port",
                    "5204",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(video_only.video_source_id, "display:0");
        assert_eq!(video_only.audio_source_id, MUNINN_DISABLED_AUDIO_SOURCE_ID);

        let audio_only = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--media-transport",
                    "rudp",
                    "--stream",
                    "muninn.raven.audio.rudp",
                    "--no-video",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                    "--port",
                    "5204",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(audio_only.video_source_id, MUNINN_DISABLED_VIDEO_SOURCE_ID);
        assert_eq!(audio_only.audio_source_id, "wasapi-loopback:Realtek");
    }

    #[test]
    fn rudp_video_encoder_outputs_annex_b_h264_for_packetizer() {
        let options = Options::parse(
            ["activate", "--media-transport", "rudp", "--framerate", "60"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        let args = rudp_video_ffmpeg_args(&options);
        let profile = muninn_rudp_media_profile();

        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-f" && pair[1] == "h264")
        );
        assert_eq!(profile.profile_id, MUNINN_RUDP_MEDIA_PROFILE_ID);
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-tune" && pair[1] == "ull")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-bf" && pair[1] == "0")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-rc" && pair[1] == "cbr")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-rc-lookahead" && pair[1] == "0")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-multipass" && pair[1] == "disabled")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-strict_gop" && pair[1] == "1")
        );
        assert!(args.windows(2).any(|pair| pair[0] == "-ldkfs"
            && pair[1] == MUNINN_RUDP_MEDIA_LOW_DELAY_KEY_FRAME_SCALE.to_string()));
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-spatial_aq" && pair[1] == "1")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-temporal_aq" && pair[1] == "1")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-aq-strength" && pair[1] == "8")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-nonref_p" && pair[1] == "1")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-b_ref_mode" && pair[1] == "disabled")
        );
        assert!(args.windows(2).any(|pair| pair[0] == "-dpb_size"
            && pair[1] == MUNINN_RUDP_MEDIA_VIDEO_DPB_SIZE.to_string()));
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-preset" && pair[1] == "p5")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-b:v" && pair[1] == "12000k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-maxrate" && pair[1] == "12000k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-bufsize" && pair[1] == "200k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-g" && pair[1] == "15")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-keyint_min" && pair[1] == "15")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-forced-idr" && pair[1] == "1")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-aud" && pair[1] == "1")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-fps_mode" && pair[1] == "cfr")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-r" && pair[1] == "60")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-an" && pair[1] == "-c:v")
        );
        assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
    }

    #[test]
    fn rudp_video_encoder_uses_explicit_bitrate_budget() {
        let options = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--framerate",
                "30",
                "--rudp-video-bitrate-kbps",
                "8000",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        let args = rudp_video_ffmpeg_args(&options);
        let profile = muninn_rudp_media_profile_for_options(&options);

        assert_eq!(profile.video_bitrate_kbps, 8000);
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-b:v" && pair[1] == "8000k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-maxrate" && pair[1] == "8000k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-bufsize" && pair[1] == "267k")
        );
    }

    #[test]
    fn rudp_video_vbv_buffer_tracks_lan_burst_budget() {
        let thirty_fps = Options::parse(
            ["activate", "--media-transport", "rudp", "--framerate", "30"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let sixty_fps = Options::parse(
            ["activate", "--media-transport", "rudp", "--framerate", "60"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let profile = muninn_rudp_media_profile();

        assert_eq!(
            muninn_rudp_video_vbv_buffer_arg(&thirty_fps, &profile),
            "400k"
        );
        assert_eq!(
            muninn_rudp_video_vbv_buffer_arg(&sixty_fps, &profile),
            "200k"
        );
    }

    #[test]
    fn rudp_video_gop_tracks_quarter_second_recovery_budget() {
        let thirty_fps = Options::parse(
            ["activate", "--media-transport", "rudp", "--framerate", "30"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let sixty_fps = Options::parse(
            ["activate", "--media-transport", "rudp", "--framerate", "60"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        assert_eq!(muninn_rudp_video_gop_frames(&thirty_fps), 8);
        assert_eq!(muninn_rudp_video_gop_frames(&sixty_fps), 15);
    }

    #[test]
    fn controllable_video_encoder_owns_capture_and_recovery_shape() {
        let options = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--framerate",
                "60",
                "--rudp-video-bitrate-kbps",
                "16000",
                "--ddagrab-output-index",
                "1",
                "--video-encoder",
                "C:/GameCult/muninn-video-encoder.exe",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            options.video_encoder_path,
            Some(PathBuf::from("C:/GameCult/muninn-video-encoder.exe"))
        );
        assert_eq!(
            muninn_controllable_video_encoder_args(&options),
            vec![
                "--input",
                "ddagrab=framerate=60:output_idx=1:draw_mouse=1",
                "--framerate",
                "60",
                "--bitrate-kbps",
                "16000",
                "--gop-frames",
                "15",
            ]
        );
    }

    #[test]
    fn default_rudp_media_packet_size_keeps_late_typed_video_wire_under_udp_mtu() {
        let mut access_unit = Vec::new();
        access_unit.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        access_unit.resize(MUNINN_RUDP_MEDIA_PACKET_BYTES, 0x80);

        let payloads = crate::media_packetizer::video_annex_b_stream_send_payloads(
            crate::media_packetizer::VideoAnnexBStreamWireOptions {
                packetize: crate::media_packetizer::VideoAnnexBStreamPacketizeOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "session-1",
                    codec: "h264",
                    first_frame_id: 1_000_000,
                    first_pts_ticks: 3_000_000_000,
                    frame_duration_ticks: 3_000,
                    timebase_num: 1,
                    timebase_den: 90_000,
                    deadline_delay_ticks: 1_800,
                    max_payload_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES,
                },
                stored_at: "2026-06-18T00:00:00Z",
                source_runtime_id: "muninn-test",
                source_role: "media-test",
            },
            &access_unit,
        )
        .unwrap();

        assert_eq!(payloads.len(), 1);
        assert!(
            payloads[0].payload.len() <= MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES,
            "typed media payload was {} bytes",
            payloads[0].payload.len()
        );
        assert!(
            payloads[0].payload.len()
                + MUNINN_RUDP_FIXED_HEADER_BYTES
                + crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL.len()
                <= MUNINN_RUDP_IPV4_UDP_PAYLOAD_BYTES
        );
    }

    #[test]
    fn default_rudp_media_packet_size_keeps_chunked_video_wire_under_rudp_fragment_limit() {
        let mut access_unit = Vec::new();
        access_unit.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        access_unit.resize(MUNINN_RUDP_MEDIA_PACKET_BYTES * 64, 0x80);

        let payloads = crate::media_packetizer::video_annex_b_stream_send_payloads(
            crate::media_packetizer::VideoAnnexBStreamWireOptions {
                packetize: crate::media_packetizer::VideoAnnexBStreamPacketizeOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "raven:2026-06-19T14-56-23Z:video",
                    codec: "h264",
                    first_frame_id: 1_000_000,
                    first_pts_ticks: 3_000_000_000,
                    frame_duration_ticks: 1_500,
                    timebase_num: 1,
                    timebase_den: 90_000,
                    deadline_delay_ticks: 1_500,
                    max_payload_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES,
                },
                stored_at: "2026-06-19T14:56:23Z",
                source_runtime_id: "raven",
                source_role: "muninn.rudp.video",
            },
            &access_unit,
        )
        .unwrap();

        assert!(payloads.len() > 1);
        let largest = payloads
            .iter()
            .map(|payload| payload.payload.len())
            .max()
            .unwrap();
        assert!(
            largest <= MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES,
            "largest typed media payload was {largest} bytes"
        );
    }

    #[test]
    fn rudp_media_transport_options_follow_low_latency_profile() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint: SocketAddr = "127.0.0.1:5204".parse().unwrap();
        let profile = muninn_rudp_media_profile();

        let options = muninn_media_rudp_options(
            socket,
            endpoint,
            &profile,
            MUNINN_MEDIA_RUDP_CONNECTION_ID,
            None,
        );

        assert_eq!(options.runtime_id, "muninn-media");
        assert_eq!(options.remote_addr, Some(endpoint));
        assert_eq!(options.connection_id, MUNINN_MEDIA_RUDP_CONNECTION_ID);
        assert_eq!(options.resend_delay_ms, MUNINN_RUDP_MEDIA_RESEND_DELAY_MS);
        assert_eq!(
            options.max_fragment_bytes,
            Some(MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES as u32)
        );
        assert_eq!(
            options.max_pending_reliable_packets,
            Some(MUNINN_RUDP_MEDIA_MAX_PENDING_RELIABLE_PACKETS)
        );
        assert_eq!(options.reconnect_policy, None);
        let transport = CultNetRudpSocketTransportConnection::new(options).unwrap();
        let channels = transport
            .profile
            .transports
            .first()
            .unwrap()
            .channels
            .iter();
        let media_channel = channels
            .clone()
            .find(|channel| {
                channel.channel_id == crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL
            })
            .unwrap();
        assert_eq!(
            media_channel.delivery,
            cultnet_rs::CultNetTransportDelivery::Unreliable
        );
    }

    #[test]
    fn rudp_audio_transport_uses_separate_realtime_connection() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint: SocketAddr = "127.0.0.1:5204".parse().unwrap();
        let profile = muninn_rudp_media_profile();

        let options = muninn_media_rudp_options(
            socket,
            endpoint,
            &profile,
            MUNINN_AUDIO_RUDP_CONNECTION_ID,
            Some(MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS),
        );

        assert_eq!(options.connection_id, MUNINN_AUDIO_RUDP_CONNECTION_ID);
        assert_eq!(options.reconnect_policy, None);
    }

    #[test]
    fn rudp_audio_encoder_outputs_pcm_for_packetizer() {
        let options = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--audio-sample-rate",
                "48000",
                "--audio-channels",
                "2",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        let args = rudp_audio_ffmpeg_args(&options);

        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-f" && pair[1] == "f32le")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-vn" && pair[1] == "-ar")
        );
        assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
    }

    #[test]
    fn media_payload_queue_deadline_is_strictly_bounded() {
        let queued_at = Instant::now();
        let deadline = Duration::from_millis(muninn_rudp_media_profile().sender_queue_deadline_ms);

        assert!(!media_payload_queue_age_exceeded(
            queued_at,
            queued_at + deadline,
            deadline
        ));
        assert!(media_payload_queue_age_exceeded(
            queued_at,
            queued_at + deadline + Duration::from_millis(1),
            deadline
        ));
    }

    #[test]
    fn rudp_media_sender_prioritizes_pending_audio_over_video() {
        let queued_at = Instant::now();
        let mut pending = PendingMuninnMediaSendQueues::default();
        pending.push(QueuedMuninnMediaSendPayload {
            payload: MuninnMediaSendPayload {
                channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                payload: vec![0x76],
            },
            queued_at,
            kind: QueuedMuninnMediaKind::Video,
        });
        pending.push(QueuedMuninnMediaSendPayload {
            payload: MuninnMediaSendPayload {
                channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                payload: vec![0x61],
            },
            queued_at,
            kind: QueuedMuninnMediaKind::Audio,
        });

        let first = pending.pop_next().expect("audio payload should be queued");
        let second = pending
            .pop_next()
            .expect("video payload should remain queued");

        assert_eq!(first.kind, QueuedMuninnMediaKind::Audio);
        assert_eq!(first.payload.payload, vec![0x61]);
        assert_eq!(second.kind, QueuedMuninnMediaKind::Video);
        assert_eq!(second.payload.payload, vec![0x76]);
        assert!(pending.is_empty());
    }

    #[test]
    fn rudp_media_pending_queues_are_structurally_bounded() {
        let queued_at = Instant::now();
        let mut pending = PendingMuninnMediaSendQueues::default();
        for index in 0..(MUNINN_RUDP_MEDIA_PENDING_AUDIO_CAPACITY + 32) {
            pending.push(QueuedMuninnMediaSendPayload {
                payload: MuninnMediaSendPayload {
                    channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                    payload: vec![(index % 251) as u8],
                },
                queued_at,
                kind: QueuedMuninnMediaKind::Audio,
            });
        }
        for index in 0..(MUNINN_RUDP_MEDIA_PENDING_VIDEO_CAPACITY + 32) {
            pending.push(QueuedMuninnMediaSendPayload {
                payload: MuninnMediaSendPayload {
                    channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                    payload: vec![(index % 251) as u8],
                },
                queued_at,
                kind: QueuedMuninnMediaKind::Video,
            });
        }

        assert_eq!(
            pending.audio_len(),
            MUNINN_RUDP_MEDIA_PENDING_AUDIO_CAPACITY
        );
        assert_eq!(
            pending.video_len(),
            MUNINN_RUDP_MEDIA_PENDING_VIDEO_CAPACITY
        );
    }

    #[test]
    fn started_video_access_unit_cannot_be_partially_evicted() {
        let queued_at = Instant::now();
        let video_group = |marker: u8, count: usize| {
            (0..count)
                .map(|_| QueuedMuninnMediaSendPayload {
                    payload: MuninnMediaSendPayload {
                        channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                        payload: vec![marker],
                    },
                    queued_at,
                    kind: QueuedMuninnMediaKind::Video,
                })
                .collect::<Vec<_>>()
        };
        let mut pending = PendingMuninnMediaSendQueues::default();
        pending.push_group(video_group(1, 64));
        assert_eq!(pending.pop_next().unwrap().payload.payload, vec![1]);

        pending.push_group(video_group(2, MUNINN_RUDP_MEDIA_PENDING_VIDEO_CAPACITY));

        for _ in 1..64 {
            assert_eq!(pending.pop_next().unwrap().payload.payload, vec![1]);
        }
        assert_eq!(pending.pop_next().unwrap().payload.payload, vec![2]);
    }

    #[test]
    fn rudp_media_ingest_yields_to_send_after_a_bounded_turn() {
        let (tx, rx) = mpsc::sync_channel(MUNINN_RUDP_MEDIA_INGEST_BUDGET_PER_TURN * 2);
        for index in 0..(MUNINN_RUDP_MEDIA_INGEST_BUDGET_PER_TURN * 2) {
            tx.try_send(Ok(vec![QueuedMuninnMediaSendPayload {
                payload: MuninnMediaSendPayload {
                    channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                    payload: vec![(index % 251) as u8],
                },
                queued_at: Instant::now(),
                kind: QueuedMuninnMediaKind::Video,
            }]))
            .expect("test payload should fit the producer channel");
        }
        let mut pending = PendingMuninnMediaSendQueues::default();

        let disconnected = drain_available_media_payloads(&rx, &mut pending).unwrap();

        assert!(!disconnected);
        assert_eq!(pending.video_len(), MUNINN_RUDP_MEDIA_INGEST_BUDGET_PER_TURN);
        assert!(rx.try_recv().is_ok(), "producer backlog should remain for the next turn");
    }

    #[test]
    fn video_access_unit_enters_handoff_as_one_bounded_group() {
        let (tx, rx) = mpsc::sync_channel(MUNINN_RUDP_MEDIA_PAYLOAD_CHANNEL_CAPACITY);
        let payloads = (0..64)
            .map(|index| MuninnMediaSendPayload {
                channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                payload: vec![index],
            })
            .collect();

        queue_muninn_media_payloads(&tx, payloads, QueuedMuninnMediaKind::Video).unwrap();
        let group = rx.try_recv().unwrap().unwrap();

        assert_eq!(group.len(), 64);
        assert!(group.iter().all(|payload| payload.kind == QueuedMuninnMediaKind::Video));
        assert!(group.windows(2).all(|pair| pair[0].queued_at == pair[1].queued_at));
    }

    #[test]
    fn video_handoff_splits_access_units_returned_by_one_encoder_read() {
        let (tx, rx) = mpsc::sync_channel(2);
        let payload = |frame_id, chunk_index| {
            let record = odin_core::MuninnMediaVideoAccessUnitRecord {
                stream_id: "video".to_string(),
                session_id: "session".to_string(),
                frame_id,
                codec: "h264".to_string(),
                pts_ticks: frame_id as i64 * 3_000,
                duration_ticks: 3_000,
                timebase_num: 1,
                timebase_den: 90_000,
                keyframe: frame_id == 1,
                dependency_frame_id: frame_id.checked_sub(1),
                deadline_ticks: frame_id as i64 * 3_000 + 180_000,
                chunk_index,
                chunk_count: 2,
                payload: vec![frame_id as u8, chunk_index as u8],
            };
            MuninnMediaSendPayload {
                channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
                payload: crate::media_packetizer::encode_media_wire_record(
                    &MuninnMediaWireRecord::Video(record),
                    "2026-07-17T00:00:00Z",
                    "muninn-test",
                    "video",
                )
                .unwrap(),
            }
        };

        queue_muninn_video_payloads_by_access_unit(
            &tx,
            vec![payload(1, 0), payload(1, 1), payload(2, 0), payload(2, 1)],
        )
        .unwrap();

        let first = rx.try_recv().unwrap().unwrap();
        let second = rx.try_recv().unwrap().unwrap();
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);
        assert!(rx.try_recv().is_err());
        for (expected_frame_id, group) in [(1, first), (2, second)] {
            assert!(group.iter().all(|payload| {
                matches!(
                    decode_media_wire_record(&payload.payload.payload).unwrap(),
                    MuninnMediaWireRecord::Video(record) if record.frame_id == expected_frame_id
                )
            }));
            assert!(group.windows(2).all(|pair| pair[0].queued_at == pair[1].queued_at));
        }
    }

    #[test]
    fn rudp_latency_budget_owns_sender_and_record_deadlines() {
        let options = Options::parse(
            [
                "activate",
                "--media-transport",
                "rudp",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--framerate",
                "30",
                "--audio-sample-rate",
                "48000",
                "--rudp-latency-budget-ms",
                "2000",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let profile = muninn_rudp_media_profile_for_options(&options);

        assert_eq!(profile.sender_queue_deadline_ms, 2000);
        assert_eq!(profile.sender_delivery_deadline_ms, 2000);
        assert_eq!(
            profile.sender_pace_every_payloads,
            MUNINN_RUDP_MEDIA_SEND_PACE_EVERY_PAYLOADS
        );
        assert_eq!(
            profile.sender_pace_sleep_us,
            MUNINN_RUDP_MEDIA_SEND_PACE_SLEEP_US
        );
        assert_eq!(profile.receiver_assembly_deadline_ms, 2000);
        assert_eq!(rudp_media_deadline_delay_ticks(&profile), 180_000);
        assert_eq!(rudp_audio_deadline_delay_ticks(&options, &profile), 96_000);
        assert_eq!(
            rudp_endpoint_for_options(&options),
            "cultmesh://odin/media/muninn-raven-av#muninn.raven.av.rudp"
        );
    }

    #[test]
    fn rudp_media_progress_detail_reports_queue_and_transport_pressure() {
        let receiver_feedback = MuninnRudpReceiverFeedbackStats {
            feedback_records: 2,
            requested_keyframes: 1,
            late_frames: 3,
            missing_video_chunks: 4,
            repaired_video_chunks: 0,
            deferred_repair_chunks: 5,
            repair_chunks_per_second: 64,
            highest_decodable_frame_id: Some(88),
            ..Default::default()
        };

        assert_eq!(
            rudp_media_progress_detail(120, 3, 2, 1, 9, &receiver_feedback),
            "Muninn RUDP media progress: sent=120 queue_dropped=3 queue_expired=2 send_expired=1 reliable_expired=9 receiver_feedback=2 receiver_keyframes=1 receiver_late_frames=3 receiver_missing_chunks=4 receiver_repaired_chunks=0 receiver_deferred_repairs=5 repair_rate=64 receiver_highest_decodable=88"
        );
    }

    #[test]
    fn rudp_repair_budget_backs_off_on_media_drops_and_recovers_when_stable() {
        let start = Instant::now();
        let mut budget = MuninnRudpRepairBudget {
            chunks_per_second: 64,
            min_chunks_per_second: 8,
            max_chunks_per_second: 128,
            add_chunks_per_second: 8,
            recovery_interval: Duration::from_secs(2),
            max_available_chunks: 4,
            available_chunks: 4,
            last_refill_at: start,
            last_rate_adjust_at: start,
            last_queue_dropped: 0,
        };

        assert_eq!(budget.take(4, start, 0), 4);
        assert_eq!(budget.chunks_per_second(), 64);
        assert_eq!(budget.take(3, start + Duration::from_millis(10), 1), 0);
        assert_eq!(budget.chunks_per_second(), 32);
        assert_eq!(budget.take(3, start + Duration::from_secs(1), 1), 3);
        assert_eq!(budget.chunks_per_second(), 32);
        assert_eq!(budget.take(3, start + Duration::from_secs(3), 1), 3);
        assert_eq!(budget.chunks_per_second(), 40);
    }

    #[test]
    fn default_rudp_repair_budget_has_lan_stream_headroom() {
        let mut budget = MuninnRudpRepairBudget::new(
            MUNINN_RUDP_MEDIA_REPAIR_INITIAL_CHUNKS_PER_SECOND,
            MUNINN_RUDP_MEDIA_REPAIR_BURST_CHUNKS,
        );
        let start = budget.last_refill_at;

        assert_eq!(budget.chunks_per_second(), 4_096);
        assert_eq!(budget.take(2_048, start, 0), 2_048);
        assert_eq!(budget.take(8_192, start + Duration::from_secs(1), 0), 2_048);
        assert_eq!(budget.take(8_192, start + Duration::from_secs(3), 0), 2_048);
        assert_eq!(budget.chunks_per_second(), 6_144);
        assert_eq!(budget.take(512, start + Duration::from_secs(4), 1), 512);
        assert_eq!(budget.chunks_per_second(), 3_072);
    }

    #[test]
    fn receiver_feedback_keyframe_requests_are_edge_triggered() {
        let mut handled = 0;
        let mut receiver_feedback = MuninnRudpReceiverFeedbackStats::default();

        assert!(!record_receiver_keyframe_pressure(&receiver_feedback, &mut handled));
        assert_eq!(handled, 0);

        receiver_feedback.requested_keyframes = 1;
        assert!(record_receiver_keyframe_pressure(&receiver_feedback, &mut handled));
        assert_eq!(handled, 1);
        assert!(!record_receiver_keyframe_pressure(&receiver_feedback, &mut handled));
        assert_eq!(handled, 1);

        receiver_feedback.requested_keyframes = 2;
        assert!(record_receiver_keyframe_pressure(&receiver_feedback, &mut handled));
        assert_eq!(handled, 2);
    }

    #[test]
    fn receiver_keyframe_pressure_writes_live_encoder_idr_command() {
        let mut commands = Vec::new();
        request_video_encoder_idr(Some(&mut commands)).unwrap();
        assert_eq!(commands, b"IDR\n");
    }

    #[test]
    fn video_bitrate_controller_backs_off_fast_and_recovers_slowly() {
        let start = Instant::now();
        let mut controller = MuninnVideoBitrateController::new(12_000, start);
        let mut feedback = MuninnRudpReceiverFeedbackStats::default();
        feedback.late_frames = 1;
        assert_eq!(
            controller.observe(&feedback, 0, 100_000, start + Duration::from_millis(500)),
            Some(6_800)
        );
        assert_eq!(
            controller.observe(&feedback, 0, 100_000, start + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            controller.observe(&feedback, 0, 100_000, start + Duration::from_millis(2500)),
            None
        );
        assert_eq!(
            controller.observe(&feedback, 0, 100_000, start + Duration::from_millis(10_500)),
            Some(7_040)
        );
    }

    #[test]
    fn receiver_decode_pressure_drives_bitrate_and_command_shape() {
        let start = Instant::now();
        let mut controller = MuninnVideoBitrateController::new(16_000, start);
        let feedback = MuninnRudpReceiverFeedbackStats {
            latest_jitter_us: 30_000,
            latest_decode_queue_us: 50_000,
            ..Default::default()
        };
        let adjusted = controller
            .observe(&feedback, 0, 100_000, start + Duration::from_millis(500))
            .unwrap();
        assert_eq!(adjusted, 9_066);
        let mut commands = Vec::new();
        request_video_encoder_bitrate(Some(&mut commands), adjusted).unwrap();
        assert_eq!(commands, b"BITRATE 9066\n");
    }

    #[test]
    fn rudp_media_receiver_feedback_updates_sender_pressure_stats() {
        let feedback = crate::media_packetizer::build_receiver_feedback(
            crate::media_packetizer::ReceiverFeedbackOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "raven:session:video",
                receiver_id: "starfire.obs",
                highest_decodable_frame_id: Some(41),
                missing_frame_ids: Vec::new(),
                missing_video_chunk_keys: vec!["42:1".to_string(), "42:3".to_string()],
                late_frame_ids: vec![42, 43],
                requested_keyframe: true,
                jitter_us: 500,
                decode_queue_us: 2_000,
                observed_at: "unix:1000",
            },
        )
        .unwrap();
        let payload = crate::media_packetizer::encode_media_wire_record(
            &crate::media_packetizer::MuninnMediaWireRecord::Feedback(feedback),
            "unix:1000",
            "starfire",
            "mimir.obs",
        )
        .unwrap();
        let frame = CultNetTransportFrame {
            channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL.to_string(),
            payload,
        };
        let mut stats = MuninnRudpReceiverFeedbackStats::default();
        let repair_cache = RecentVideoChunkRepairCache::new(16);

        let repairs =
            record_rudp_media_receiver_feedback(&frame, &mut stats, &repair_cache).unwrap();

        assert_eq!(stats.feedback_records, 1);
        assert_eq!(stats.requested_keyframes, 1);
        assert_eq!(stats.late_frames, 2);
        assert_eq!(stats.missing_video_chunks, 2);
        assert_eq!(stats.repaired_video_chunks, 0);
        assert_eq!(stats.highest_decodable_frame_id, Some(41));
        assert!(repairs.is_empty());
    }

    #[test]
    fn repair_cache_returns_recent_missing_video_chunks_from_feedback() {
        let video = odin_core::MuninnMediaVideoAccessUnitRecord {
            stream_id: "muninn.raven.av.rudp".to_string(),
            session_id: "raven:session:video".to_string(),
            frame_id: 42,
            codec: "h264".to_string(),
            pts_ticks: 126_000,
            duration_ticks: 3_000,
            timebase_num: 1,
            timebase_den: 90_000,
            keyframe: false,
            dependency_frame_id: Some(41),
            deadline_ticks: 127_800,
            chunk_index: 3,
            chunk_count: 5,
            payload: vec![1, 2, 3, 4],
        };
        let payload = MuninnMediaSendPayload {
            channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
            payload: crate::media_packetizer::encode_media_wire_record(
                &crate::media_packetizer::MuninnMediaWireRecord::Video(video),
                "unix:1000",
                "muninn-test",
                "repair-cache-test",
            )
            .unwrap(),
        };
        let feedback = crate::media_packetizer::build_receiver_feedback(
            crate::media_packetizer::ReceiverFeedbackOptions {
                stream_id: "muninn.raven.av.rudp",
                session_id: "raven:session:video",
                receiver_id: "starfire.obs",
                highest_decodable_frame_id: Some(41),
                missing_frame_ids: Vec::new(),
                missing_video_chunk_keys: vec!["42:3".to_string(), "42:4".to_string()],
                late_frame_ids: Vec::new(),
                requested_keyframe: true,
                jitter_us: 500,
                decode_queue_us: 2_000,
                observed_at: "unix:1000",
            },
        )
        .unwrap();

        let mut cache = RecentVideoChunkRepairCache::new(16);
        let now = Instant::now();
        cache
            .remember(&payload, now, Duration::from_millis(100))
            .unwrap();
        let repairs = cache.repair_payloads_for_feedback(&feedback, now);

        assert_eq!(repairs, vec![payload]);
    }

    #[test]
    fn repair_cache_refuses_expired_and_receiver_late_chunks() {
        let video = odin_core::MuninnMediaVideoAccessUnitRecord {
            stream_id: "muninn.raven.av.rudp".to_string(),
            session_id: "raven:session:video".to_string(),
            frame_id: 42,
            codec: "h264".to_string(),
            pts_ticks: 126_000,
            duration_ticks: 3_000,
            timebase_num: 1,
            timebase_den: 90_000,
            keyframe: false,
            dependency_frame_id: Some(41),
            deadline_ticks: 127_800,
            chunk_index: 3,
            chunk_count: 5,
            payload: vec![1, 2, 3, 4],
        };
        let payload = MuninnMediaSendPayload {
            channel_id: crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL,
            payload: crate::media_packetizer::encode_media_wire_record(
                &crate::media_packetizer::MuninnMediaWireRecord::Video(video),
                "unix:1000",
                "muninn-test",
                "repair-cache-expiry-test",
            )
            .unwrap(),
        };
        let feedback = |late_frame_ids| {
            crate::media_packetizer::build_receiver_feedback(
                crate::media_packetizer::ReceiverFeedbackOptions {
                    stream_id: "muninn.raven.av.rudp",
                    session_id: "raven:session:video",
                    receiver_id: "starfire.obs",
                    highest_decodable_frame_id: Some(41),
                    missing_frame_ids: Vec::new(),
                    missing_video_chunk_keys: vec!["42:3".to_string()],
                    late_frame_ids,
                    requested_keyframe: false,
                    jitter_us: 500,
                    decode_queue_us: 2_000,
                    observed_at: "unix:1000",
                },
            )
            .unwrap()
        };
        let now = Instant::now();
        let mut cache = RecentVideoChunkRepairCache::new(16);
        cache
            .remember(&payload, now, Duration::from_millis(100))
            .unwrap();

        assert!(
            cache
                .repair_payloads_for_feedback(&feedback(vec![42]), now)
                .is_empty()
        );
        assert!(
            cache
                .repair_payloads_for_feedback(
                    &feedback(Vec::new()),
                    now + Duration::from_millis(101),
                )
                .is_empty()
        );
    }

    #[test]
    fn idle_surface_advertises_affordances_without_active_streams() {
        let options = Options::parse([].into_iter()).unwrap();
        let sources = available_sources(&options);

        assert!(sources.iter().any(|source| source.starts_with("screen:")));
        assert!(
            sources
                .iter()
                .any(|source| source.starts_with("audio-loopback:"))
        );
    }

    #[test]
    fn muninn_eve_surface_exposes_live_move_hue_program_controls() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-move-hue-eve-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--store",
                store_path.to_str().unwrap(),
                "--host",
                "nightwing",
                "--odin-cultmesh-uri",
                "none",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let _ = load_or_initialize_move_hue_program(&options).unwrap();
        let mut node = open_node(&options, "muninn-move-hue-eve-test").unwrap();
        publish_surface(&mut node, &options, "idle", &[]).unwrap();
        let surface = node
            .get_required::<EveSurfaceStateRecord>(&move_hue_surface_key("nightwing"))
            .unwrap();
        let encoded = surface.surface.to_string();
        assert!(encoded.contains("Hold Current Colors"));
        assert!(encoded.contains("golden-permutation"));
        assert!(encoded.contains("Transition Duration"));
        assert!(encoded.contains("transitionPercent"));
        assert!(encoded.contains(MUNINN_MOVE_HUE_PROGRAM_SCHEMA));
        drop(node);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn move_hue_program_command_updates_typed_live_state() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-move-hue-command-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "set-move-hue-program",
                "--store",
                store_path.to_str().unwrap(),
                "--host",
                "nightwing",
                "--move-hue-mode",
                "hold",
                "--move-hue-order",
                "bounce",
                "--move-hue-cycle-ms",
                "1500",
                "--odin-cultmesh-uri",
                "none",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        set_move_hue_program(options.clone()).unwrap();
        let node = open_node(&options, "muninn-move-hue-command-test").unwrap();
        let program = node
            .get_required::<MuninnMoveHueProgramRecord>(&move_hue_program_key("nightwing"))
            .unwrap();
        assert_eq!(program.mode, "hold");
        assert_eq!(program.order_mode, "bounce");
        assert_eq!(program.cycle_ms, 1500);
        assert!(program.hold_at_ns > 0);
        drop(node);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn provider_command_rudp_ingress_applies_move_hue_command() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-provider-command-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--store",
                store_path.to_str().unwrap(),
                "--host",
                "nightwing",
                "--odin-cultmesh-uri",
                "none",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let runtime_program = load_or_initialize_move_hue_program(&options).unwrap();
        let server_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        server_socket
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let server_address = server_socket.local_addr().unwrap();
        let server_options = options.clone();
        let server_program = Arc::clone(&runtime_program);
        thread::spawn(move || {
            run_provider_command_ingress(server_socket, server_options, server_program).unwrap()
        });

        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        client_socket
            .set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let mut client = CultNetRudpSocketTransportConnection::new(
            CultNetRudpSocketTransportOptions {
                runtime_id: "muninn-provider-command-test-client".to_string(),
                socket: client_socket,
                mode: cultnet_rs::CultNetRudpSocketMode::Client,
                remote_addr: Some(server_address),
                connection_id: MUNINN_COMMAND_RUDP_CONNECTION_ID,
                initial_sequence: 1,
                resend_delay_ms: 15,
                transport_id: Some("muninn-provider-command-test".to_string()),
                max_payload_bytes: None,
                max_fragment_bytes: Some(1200),
                max_pending_reliable_packets: Some(64),
                reconnect_policy: None,
            },
        )
        .unwrap();
        client.connect(Vec::new()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !client.connected() && Instant::now() < deadline {
            let _ = client.receive_once().unwrap();
            client.poll_resends().unwrap();
            thread::sleep(Duration::from_millis(1));
        }
        assert!(client.connected(), "provider command RUDP handshake");
        let command = json!({
            "type": "surface-command",
            "schema": "gamecult.eve.command.v1",
            "providerId": "muninn.telemetry.nightwing",
            "command": "muninn.set-move-hue-program",
            "payload": {
                "type": "muninn.set-move-hue-program",
                "mode": "hold",
                "orderMode": "bounce",
                "cycleMs": 1500,
                "transitionPercent": 25
            },
            "publishedBy": "provider-command-test"
        });
        let message = CultNetMessage::DocumentPutRaw {
            message_id: "provider-command-test".to_string(),
            document: CultNetRawDocumentRecord {
                schema_id: "gamecult.eve.command.v1".to_string(),
                record_key: "provider-command-test".to_string(),
                stored_at: timestamp().unwrap(),
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: rmp_serde::to_vec_named(&command).unwrap(),
                source_runtime_id: Some("provider-command-test".to_string()),
                source_agent_id: None,
                source_role: Some("test".to_string()),
                tags: None,
            },
        };
        client
            .send(
                "schema",
                encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)
                    .unwrap(),
            )
            .unwrap();
        let apply_deadline = Instant::now() + Duration::from_secs(2);
        while runtime_program.lock().unwrap().mode != "hold" && Instant::now() < apply_deadline {
            client.poll_resends().unwrap();
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(runtime_program.lock().unwrap().mode, "hold");
        assert_eq!(runtime_program.lock().unwrap().order_mode, "bounce");
        assert_eq!(runtime_program.lock().unwrap().cycle_ms, 1500);
        assert_eq!(runtime_program.lock().unwrap().transition_percent, 25);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn move_light_command_writes_ps_move_led_report() {
        let command = pending_move_light_command();
        let mut writer = RecordingMoveLightWriter::default();

        let result = execute_move_light_command(command, &mut writer).unwrap();

        assert_eq!(result.state, "completed");
        assert_eq!(writer.writes.len(), 1);
        assert_eq!(writer.writes[0].0, "/dev/hidraw3");
        assert_eq!(writer.writes[0].1.len(), PS_MOVE_LED_REPORT_LEN);
        assert_eq!(&writer.writes[0].1[..5], &[0x06, 0, 255, 64, 8]);
        assert!(writer.writes[0].1[5..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn default_move_light_report_holds_full_identity_color() {
        let full = default_move_light_report((100, 80, 60));
        assert_eq!(full.len(), PS_MOVE_LED_REPORT_LEN);
        assert_eq!(&full[..5], &[0x06, 0, 100, 80, 60]);
        assert!(full[5..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn default_move_color_is_stable_for_physical_identity() {
        assert_eq!(
            default_move_color_for_identity("move-0006f523e2d1"),
            default_move_color_for_identity("move-0006f523e2d1")
        );
        assert_ne!(
            default_move_color_for_identity("move-0006f523e2d1"),
            default_move_color_for_identity("move-000704a6be5f")
        );
        let roster = [
            "move-0006f523e2d1",
            "move-000704a39772",
            "move-000704a6be5f",
            "move-000704a800d0",
        ];
        let colors = roster.map(default_move_color_for_identity);
        assert_eq!(colors.into_iter().collect::<HashSet<_>>().len(), roster.len());
        assert!(colors.into_iter().all(|color| color.0.max(color.1).max(color.2) == 255));
    }

    #[test]
    fn golden_move_roster_spreads_neighboring_stable_ids_across_hue_space() {
        let roster = vec![
            "move-0006f523e2d1".to_string(),
            "move-000704a39772".to_string(),
            "move-000704a6be5f".to_string(),
            "move-000704a800d0".to_string(),
        ];
        let colors = roster
            .iter()
            .map(|identity| golden_move_color_for_roster(identity, &roster))
            .collect::<Vec<_>>();
        assert_eq!(colors.iter().copied().collect::<HashSet<_>>().len(), 4);
        assert!(colors.iter().all(|color| color.0.max(color.1).max(color.2) == 255));
        assert!(colors.iter().any(|color| color.0 == 255 && color.1 < 100));
        assert!(colors.iter().any(|color| color.1 == 255 && color.0 < 100));
        assert!(colors.iter().any(|color| color.2 == 255 && color.0 < 100));
    }

    #[test]
    fn scheduled_golden_colors_shift_window_in_descending_move_order() {
        let roster = (0..4).map(|index| format!("move-{index}")).collect::<Vec<_>>();
        let state = |millis: i128| {
            roster
                .iter()
                .map(|identity| {
                    scheduled_golden_move_color(
                        identity,
                        &roster,
                        0,
                        1_000_000_000,
                        millis * 1_000_000,
                    )
                    .map(|(_, sequence, next)| (sequence, next))
                    .unwrap()
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(state(0), vec![(0, false), (1, false), (2, false), (3, true)]);
        assert_eq!(state(249), vec![(0, false), (1, false), (2, false), (3, true)]);
        assert_eq!(state(250), vec![(0, false), (1, false), (2, true), (4, false)]);
        assert_eq!(state(500), vec![(0, false), (1, true), (3, false), (4, false)]);
        assert_eq!(state(750), vec![(0, true), (2, false), (3, false), (4, false)]);
        assert_eq!(state(1_000), vec![(1, false), (2, false), (3, false), (4, true)]);
    }

    #[test]
    fn scheduled_golden_color_smoothersteps_across_the_full_subslot() {
        let roster = (0..4).map(|index| format!("move-{index}")).collect::<Vec<_>>();
        let color_at = |millis: i128| {
            scheduled_golden_move_color(
                "move-3",
                &roster,
                0,
                1_000_000_000,
                millis * 1_000_000,
            )
            .unwrap()
            .0
        };

        assert_eq!(color_at(0), hsv_to_rgb((3.0 * 0.618_033_988_749_894_9_f64).fract() * 360.0, 1.0, 1.0));
        assert_eq!(color_at(125), hsv_to_rgb(wrapped_hue_lerp(
            (3.0 * 0.618_033_988_749_894_9_f64).fract(),
            (4.0 * 0.618_033_988_749_894_9_f64).fract(),
            0.5,
        ) * 360.0, 1.0, 1.0));
        assert_eq!(color_at(250), hsv_to_rgb((4.0 * 0.618_033_988_749_894_9_f64).fract() * 360.0, 1.0, 1.0));
    }

    #[test]
    fn scheduled_golden_color_transition_uses_selected_subslot_percentage() {
        let roster = (0..4).map(|index| format!("move-{index}")).collect::<Vec<_>>();
        let color_at = |millis: i128| scheduled_golden_move_color_with_order(
            "move-3", &roster, 0, 1_000_000_000, millis * 1_000_000, "descending", 25,
        ).unwrap().0;
        let source = (3.0 * 0.618_033_988_749_894_9_f64).fract();
        let target = (4.0 * 0.618_033_988_749_894_9_f64).fract();
        assert_ne!(color_at(31), hsv_to_rgb(source * 360.0, 1.0, 1.0));
        assert_ne!(color_at(31), hsv_to_rgb(target * 360.0, 1.0, 1.0));
        assert_eq!(color_at(63), hsv_to_rgb(target * 360.0, 1.0, 1.0));
        assert_eq!(color_at(200), hsv_to_rgb(target * 360.0, 1.0, 1.0));
    }

    #[test]
    fn zero_percent_transition_hard_steps_at_subslot_boundary() {
        let roster = (0..4).map(|index| format!("move-{index}")).collect::<Vec<_>>();
        let color = scheduled_golden_move_color_with_order(
            "move-3", &roster, 0, 1_000_000_000, 0, "descending", 0,
        ).unwrap().0;
        let target = (4.0 * 0.618_033_988_749_894_9_f64).fract();
        assert_eq!(color, hsv_to_rgb(target * 360.0, 1.0, 1.0));
    }

    #[test]
    fn hue_update_order_modes_are_deterministic_per_cycle() {
        assert_eq!(move_hue_update_order(4, 0, "descending"), vec![3, 2, 1, 0]);
        assert_eq!(move_hue_update_order(4, 0, "ascending"), vec![0, 1, 2, 3]);
        assert_eq!(move_hue_update_order(4, 0, "bounce"), vec![3, 2, 1, 0]);
        assert_eq!(move_hue_update_order(4, 1, "bounce"), vec![0, 1, 2, 3]);
        assert_eq!(move_hue_update_order(4, 0, "rotating-lead"), vec![3, 2, 1, 0]);
        assert_eq!(move_hue_update_order(4, 1, "rotating-lead"), vec![2, 1, 0, 3]);
        let first = move_hue_update_order(4, 23, "golden-permutation");
        assert_eq!(first, move_hue_update_order(4, 23, "golden-permutation"));
        assert_eq!(first.iter().copied().collect::<HashSet<_>>().len(), 4);
    }

    #[test]
    fn move_light_command_rejects_duration_shape_mismatch() {
        let command = MuninnMoveLightCommandRecord {
            colors: vec!["#ff0000".to_string(), "#00ff00".to_string()],
            durations_ms: vec![0],
            ..pending_move_light_command()
        };
        let mut writer = RecordingMoveLightWriter::default();

        let result = execute_move_light_command(command, &mut writer).unwrap();

        assert_eq!(result.state, "failed");
        assert!(result.detail.contains("durations_ms"));
        assert!(writer.writes.is_empty());
    }

    #[test]
    fn move_light_command_rejects_invalid_color() {
        let command = MuninnMoveLightCommandRecord {
            colors: vec!["blue".to_string()],
            ..pending_move_light_command()
        };
        let mut writer = RecordingMoveLightWriter::default();

        let result = execute_move_light_command(command, &mut writer).unwrap();

        assert_eq!(result.state, "failed");
        assert!(result.detail.contains("expected #rrggbb"));
        assert!(writer.writes.is_empty());
    }

    #[test]
    fn request_move_light_builds_pending_typed_command() {
        let options = Options::parse(
            [
                "request-move-light",
                "--host",
                "nightwing",
                "--move",
                "move-usb",
                "--hidraw",
                "/dev/hidraw1",
                "--color",
                "#35ff6c",
                "--duration-ms",
                "0",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        let command = build_move_light_command(&options).unwrap();

        assert_eq!(command.host_id, "nightwing");
        assert_eq!(command.move_id, "move-usb");
        assert_eq!(command.hidraw_path, "/dev/hidraw1");
        assert_eq!(command.colors, vec!["#35ff6c"]);
        assert_eq!(command.durations_ms, vec![0]);
        assert_eq!(command.state, "pending");
    }

    #[test]
    fn move_light_status_accepts_command_filter() {
        let options = Options::parse(
            [
                "move-light-status",
                "--host",
                "nightwing",
                "--command",
                "cmd-1",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::MoveLightStatus);
        assert_eq!(options.host_id, "nightwing");
        assert_eq!(options.command_id.as_deref(), Some("cmd-1"));
    }

    #[test]
    fn capture_stream_status_accepts_stream_and_command_filters() {
        let options = Options::parse(
            [
                "capture-stream-status",
                "--host",
                "raven",
                "--stream",
                "muninn.probe.video.rudp",
                "--command",
                "cmd-1",
                "--activate-store",
                "C:/Meta/Odin/state/muninn.activate.cc",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::CaptureStreamStatus);
        assert_eq!(options.host_id, "raven");
        assert_eq!(options.stream_id, "muninn.probe.video.rudp");
        assert!(options.stream_filter_explicit);
        assert_eq!(options.command_id.as_deref(), Some("cmd-1"));
        assert_eq!(
            options.activation_store_path,
            Some(PathBuf::from("C:/Meta/Odin/state/muninn.activate.cc"))
        );
    }

    #[test]
    fn capture_stream_status_uses_default_activation_store_path() {
        let options = Options::parse(
            ["capture-stream-status", "--host", "raven"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        assert_eq!(
            options.activation_store_path,
            Some(PathBuf::from(MUNINN_DEFAULT_ACTIVATION_STORE_PATH))
        );
    }

    #[test]
    fn capture_stream_status_does_not_filter_on_default_stream_id() {
        let options = Options::parse(
            ["capture-stream-status", "--host", "raven"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::CaptureStreamStatus);
        assert_eq!(options.host_id, "raven");
        assert_eq!(options.stream_id, "muninn.raven.av.rudp");
        assert!(!options.stream_filter_explicit);
    }

    #[test]
    fn tick_capture_stream_commands_supersedes_older_nonterminal_receipts() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-capture-command-supersede-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "raven",
                "--store",
                store_path.to_str().unwrap(),
                "--activate-store",
                store_path.to_str().unwrap(),
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut older = build_capture_stream_command(
            &Options::parse(
                [
                    "request-stream",
                    "--host",
                    "raven",
                    "--stream",
                    "muninn.probe.video.rudp",
                    "--target-host",
                    "cultmesh://odin/media/muninn-raven-av",
                ]
                .into_iter()
                .map(String::from),
            )
            .unwrap(),
        )
        .unwrap();
        older.command_id = "older-command".to_string();
        older.updated_at = "unix-1000".to_string();
        older.state = "pending".to_string();

        let mut newer = older.clone();
        newer.command_id = "newer-command".to_string();
        newer.updated_at = "unix-2000".to_string();
        newer.state = "completed".to_string();
        newer.detail = "already handled elsewhere".to_string();

        let mut node = open_node(&options, "muninn-capture-command-supersede-test").unwrap();
        node.put(&older.command_id, &older).unwrap();
        node.put(&newer.command_id, &newer).unwrap();
        drop(node);

        let active = &mut Vec::new();
        let active_stream_ids = tick_capture_stream_commands(&options, active).unwrap();

        assert!(active_stream_ids.is_empty());

        let node = open_node(&options, "muninn-capture-command-supersede-status").unwrap();
        let superseded = node
            .get_required::<MuninnCaptureStreamCommandRecord>("older-command")
            .unwrap();
        let latest = node
            .get_required::<MuninnCaptureStreamCommandRecord>("newer-command")
            .unwrap();
        assert_eq!(superseded.state, "completed");
        assert_eq!(
            superseded.detail,
            "Superseded by newer command newer-command."
        );
        assert_eq!(latest.state, "completed");
        assert_eq!(latest.detail, "already handled elsewhere");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn move_state_status_accepts_move_filter() {
        let options = Options::parse(
            [
                "move-state-status",
                "--host",
                "nightwing",
                "--move",
                "move-usb",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::MoveStateStatus);
        assert_eq!(options.host_id, "nightwing");
        assert_eq!(options.move_id, "move-usb");
    }

    #[test]
    fn move_source_status_is_a_read_only_diagnostic_mode() {
        let options = Options::parse(
            ["move-source-status", "--host", "starfire"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::MoveSourceStatus);
        assert_eq!(options.host_id, "starfire");
    }

    #[test]
    fn move_identity_status_accepts_move_filter() {
        let options = Options::parse(
            [
                "move-identity-status",
                "--host",
                "starfire",
                "--move",
                "move-0006f523e2d1",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::MoveIdentityStatus);
        assert_eq!(options.host_id, "starfire");
        assert_eq!(options.move_id, "move-0006f523e2d1");
    }

    #[test]
    fn claim_move_host_accepts_target_bluetooth_address() {
        let options = Options::parse(
            ["claim-move-host", "--move-host", "5C:93:A2:9C:A8:A8"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();

        assert_eq!(options.mode, Mode::ClaimMoveHost);
        assert_eq!(
            options.move_host_address.as_deref(),
            Some("5C:93:A2:9C:A8:A8")
        );
    }

    #[test]
    fn serve_only_claims_move_host_when_explicitly_configured() {
        let implicit =
            Options::parse(["serve", "--host", "raven"].into_iter().map(String::from)).unwrap();
        let explicit = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--move-host",
                "5C:93:A2:9C:A8:A8",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert!(!serve_should_claim_move_host(&implicit));
        assert!(serve_should_claim_move_host(&explicit));
    }

    #[test]
    fn serve_only_manages_platform_move_lights_when_move_runtime_is_explicit() {
        let plain =
            Options::parse(["serve", "--host", "raven"].into_iter().map(String::from)).unwrap();
        let move_host = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--move-host",
                "5C:93:A2:9C:A8:A8",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let move_state = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--move-state",
                "move-usb=windows-psmove",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert!(!serve_should_manage_move_runtime(&plain));
        assert!(serve_should_manage_move_runtime(&move_host));
        assert!(serve_should_manage_move_runtime(&move_state));
        assert!(!serve_should_manage_platform_move_lights(&plain));
        assert!(serve_should_manage_platform_move_lights(&move_host));
        assert!(serve_should_manage_platform_move_lights(&move_state));
    }

    #[test]
    fn parses_bluetoothctl_motion_controller_device_line() {
        assert_eq!(
            parse_bluetoothctl_motion_controller_device_line(
                "Device 00:07:04:A8:00:D0 Motion Controller"
            )
            .as_deref(),
            Some("00:07:04:A8:00:D0")
        );
        assert!(
            parse_bluetoothctl_motion_controller_device_line("Device 00:11:22:33:44:55 Keyboard")
                .is_none()
        );
    }

    #[test]
    fn parses_bluetoothctl_motion_controller_flags() {
        let info = "\
Device 00:07:04:A8:00:D0 (public)
\tTrusted: yes
\tConnected: no
";
        let device = parse_bluetoothctl_device_info("00:07:04:A8:00:D0", info);

        assert_eq!(device.address, "00:07:04:A8:00:D0");
        assert!(device.trusted);
        assert!(!device.connected);
    }

    #[test]
    fn bluetooth_address_maps_to_move_id() {
        assert_eq!(
            bluetooth_address_move_id("00:07:04:A8:00:D0").as_deref(),
            Some("move-000704a800d0")
        );
    }

    #[test]
    fn bluetooth_move_identity_record_describes_waiting_pickup() {
        let options = Options::parse(
            ["serve", "--host", "nightwing"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let device = BluetoothMoveDevice {
            address: "00:07:04:A8:00:D0".to_string(),
            trusted: true,
            connected: false,
        };

        let record = build_bluetooth_move_identity_record(
            &options,
            &device,
            "5C:93:A2:9C:A8:A8".to_string(),
            "unix-1781667000".to_string(),
        )
        .unwrap();

        assert_eq!(
            record.identity_id,
            "nightwing:move-000704a800d0:move-identity"
        );
        assert_eq!(record.source_path, "bluetooth:00:07:04:A8:00:D0");
        assert_eq!(record.bluetooth_host_address, "5C:93:A2:9C:A8:A8");
        assert_eq!(record.state, "bluetooth-waiting");
        assert!(bluetooth_move_needs_pickup(&device));
    }

    #[test]
    fn bluetooth_pickup_uses_bluez_state_not_stale_usb_sources() {
        let disconnected_trusted = BluetoothMoveDevice {
            address: "00:07:04:A8:00:D0".to_string(),
            trusted: true,
            connected: false,
        };
        let connected = BluetoothMoveDevice {
            connected: true,
            ..disconnected_trusted.clone()
        };
        let untrusted = BluetoothMoveDevice {
            trusted: false,
            ..disconnected_trusted.clone()
        };

        assert!(bluetooth_move_needs_pickup(&disconnected_trusted));
        assert!(!bluetooth_move_needs_pickup(&connected));
        assert!(!bluetooth_move_needs_pickup(&untrusted));
    }

    #[cfg(windows)]
    #[test]
    fn windows_ps_move_source_token_preserves_concrete_hid_path() {
        let path = r"\\?\hid#vid_054c&pid_03d5&col01#example#{guid}";
        let token = windows_ps_move_source_token(path);

        assert!(is_windows_ps_move_source(&token));
        assert_eq!(
            windows_ps_move_input_path(&token).unwrap().as_deref(),
            Some(path)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_ps_move_physical_key_groups_sibling_collections() {
        let col01 = r"\\?\hid#vid_054c&pid_03d5&col01#a&976df89&0&0000#{guid}";
        let col02 = r"\\?\hid#vid_054c&pid_03d5&col02#a&976df89&0&0001#{guid}";

        assert_eq!(
            windows_ps_move_physical_key(col01),
            windows_ps_move_physical_key(col02)
        );
    }

    #[test]
    fn serve_accepts_move_state_sources() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-state",
                "move-usb=/dev/hidraw1",
                "--move-evidence-stream",
                "muninn:nightwing:move-evidence",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(
            options.move_state_sources,
            vec![MoveStateSource {
                move_id: "move-usb".to_string(),
                hidraw_path: "/dev/hidraw1".to_string(),
            }]
        );
        assert_eq!(
            options.move_evidence_stream_id.as_deref(),
            Some("muninn:nightwing:move-evidence")
        );
        assert_eq!(options.move_evidence_verse_id, "mimir-live");
    }

    #[test]
    fn serve_accepts_quest_access_streams() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--quest-adb",
                "--quest-serial",
                "1WMHHB68PG1515",
                "--quest-input-stream",
                "muninn:starfire:quest-input",
                "--quest-pose-stream",
                "muninn:starfire:quest-poses",
                "--quest-video-input-stream",
                "muninn:starfire:quest-warped-video-input",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert!(options.quest_adb);
        assert_eq!(options.quest_serial.as_deref(), Some("1WMHHB68PG1515"));
        assert_eq!(
            options.quest_video_input_stream_id.as_deref(),
            Some("muninn:starfire:quest-warped-video-input")
        );
    }

    #[test]
    fn parses_authorized_quest_from_adb_devices() {
        let output = "List of devices attached\n1WMHHB68PG1515         device product:hollywood model:Quest_2 device:hollywood transport_id:5\n";
        let device = parse_quest_device_from_adb(output, None).expect("Quest should parse");

        assert_eq!(device.serial, "1WMHHB68PG1515");
        assert_eq!(device.connection_state, "device");
        assert_eq!(device.product, "hollywood");
        assert_eq!(device.model, "Quest_2");
        assert_eq!(device.transport_id, "5");
    }

    #[test]
    fn quest_access_record_defaults_streams_to_host_owned_muninn_surfaces() {
        let options = Options::parse(
            ["serve", "--host", "starfire", "--quest-adb"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let record = build_quest_access_record(
            &options,
            ParsedQuestDevice {
                serial: "1WMHHB68PG1515".to_string(),
                connection_state: "device".to_string(),
                product: "hollywood".to_string(),
                model: "Quest_2".to_string(),
                device: "hollywood".to_string(),
                transport_id: "5".to_string(),
            },
            "usb-authorized",
            "test",
        )
        .unwrap();

        assert_eq!(
            record.access_id,
            "muninn:starfire:quest-access:1WMHHB68PG1515"
        );
        assert_eq!(record.input_stream_id, "muninn:starfire:quest-input");
        assert_eq!(record.pose_stream_id, "muninn:starfire:quest-poses");
        assert_eq!(
            record.video_input_stream_id,
            "muninn:starfire:quest-warped-video-input"
        );
        assert_eq!(
            record.video_input_transport,
            "brokkr-unity-editor-warped-frame-stream"
        );
    }

    #[derive(Deserialize)]
    struct DecodedMoveEvidenceStreamFrame(
        String,
        String,
        i64,
        Vec<DecodedMarkerCandidate>,
        Vec<MuninnMoveControllerStateRecord>,
    );

    #[derive(Deserialize)]
    struct DecodedMimirMoveProofEvidenceFrameSnapshot(
        String,
        String,
        String,
        String,
        i64,
        u64,
        Vec<u8>,
    );

    #[derive(Deserialize)]
    struct DecodedMarkerCandidate {
        stream_id: String,
        host_id: String,
        camera_id: String,
        frame_sequence: u64,
        source_id_hash: u64,
        tile_x: u32,
        tile_y: u32,
        center_x_px: f32,
        center_y_px: f32,
        radius_px: f32,
        area_px: u32,
        mean_luma: f32,
        peak_luma: u32,
        score: f32,
        observed_at: String,
        move_id: String,
    }

    struct RecordingMoveStateReader {
        reports: Vec<Vec<u8>>,
        joystick_events: Vec<JoystickEvent>,
        failing_joystick_path: Option<String>,
    }

    #[derive(Default)]
    struct RecordingMoveMarkerCameraReader {
        frames: Vec<Vec<u8>>,
        configs: Vec<muninn_move_tracker::MoveTrackerConfig>,
    }

    impl MoveMarkerCameraFrameReader for RecordingMoveMarkerCameraReader {
        fn read_luma_frame(
            &mut self,
            _source: &MoveMarkerCameraSource,
            frame_source: &MoveMarkerFrameSource,
        ) -> Result<Option<Vec<u8>>> {
            self.configs.push(frame_source.tracker_config);
            if self.frames.is_empty() {
                return Ok(None);
            }
            Ok(Some(self.frames.remove(0)))
        }
    }

    impl MoveControllerStateReader for RecordingMoveStateReader {
        fn read_report(&mut self, _hidraw_path: &str) -> Result<Option<Vec<u8>>> {
            if self.reports.is_empty() {
                return Ok(None);
            }
            Ok(Some(self.reports.remove(0)))
        }

        fn read_joystick_events(&mut self, joystick_path: &str) -> Result<Vec<JoystickEvent>> {
            if self
                .failing_joystick_path
                .as_deref()
                .is_some_and(|path| path == joystick_path)
            {
                return Err(anyhow!("simulated missing joystick"));
            }
            Ok(std::mem::take(&mut self.joystick_events))
        }
    }

    #[test]
    fn move_identity_publishes_without_controller_state_report() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-move-identity-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--store",
                store_path.to_str().unwrap(),
                "--move-host",
                "5C:93:A2:9C:A8:A8",
                "--move-state",
                "move-0006f523e2d1=windows-psmove://silent",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut node = open_node(&options, "muninn-move-identity-test").unwrap();

        publish_move_identity_records(&mut node, &options, &options.move_state_sources).unwrap();

        let record = node
            .get_required::<MuninnMoveIdentityRecord>("starfire:move-0006f523e2d1:move-identity")
            .unwrap();
        assert_eq!(record.host_id, "starfire");
        assert_eq!(record.move_id, "move-0006f523e2d1");
        assert_eq!(record.bluetooth_host_address, "5C:93:A2:9C:A8:A8");
        assert_eq!(record.state, "usb-visible");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn runtime_boundary_records_use_explicit_activation_store_path() {
        let store_path =
            std::env::temp_dir().join(format!("muninn-boundary-{}.cc", timestamp_ns().unwrap()));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "raven",
                "--store",
                store_path.to_str().unwrap(),
                "--activate-store",
                "C:/Meta/Odin/state/muninn.activate.cc",
                "--target-host",
                "cultmesh://odin/media/muninn-raven-av",
                "--idunn-rudp-health",
                "198.51.100.10:17870",
                "--idunn-daemon",
                "muninn",
                "--idunn-health-contract",
                "muninn.cultnet-rudp-remote-telemetry-health",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut node = open_node(&options, "muninn-boundary-test").unwrap();

        publish_runtime_boundary_records(&mut node, &options, "idle", &[], &[]).unwrap();

        let boundary = node
            .get_required::<MuninnCommandBoundaryCompatRecord>("command-boundary:muninn")
            .unwrap();
        let transport = node
            .get_required::<MuninnTransportProfileCompatRecord>("transport-profile:muninn")
            .unwrap();
        let provider = node
            .get_required::<EveProviderAdvertisementRecord>("muninn.telemetry.raven")
            .unwrap();

        let invocation = boundary
            .value
            .get("commands")
            .and_then(|commands| commands.as_array())
            .and_then(|commands| commands.first())
            .and_then(|command| command.get("invocation"))
            .and_then(|value| value.as_str())
            .unwrap();
        assert!(invocation.contains("C:/Meta/Odin/state/muninn.activate.cc"));
        assert!(invocation.contains(store_path.to_str().unwrap()));
        assert!(invocation.contains("request-stream"));
        assert!(invocation.contains("--target-host cultmesh://odin/media/muninn-raven-av"));

        let command_lowerings = transport
            .value
            .get("command_lowerings")
            .and_then(|value| value.as_array())
            .unwrap();
        assert!(command_lowerings.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|entry| entry.contains("C:/Meta/Odin/state/muninn.activate.cc"))
        }));

        let media_profile = transport
            .value
            .get("media_profile")
            .and_then(|value| value.as_object())
            .unwrap();
        assert_eq!(
            media_profile
                .get("profile_id")
                .and_then(|value| value.as_str()),
            Some(MUNINN_RUDP_MEDIA_PROFILE_ID)
        );
        assert_eq!(
            media_profile
                .get("strategy")
                .and_then(|value| value.as_str()),
            Some("hardware-codec-owned-prediction-over-fixed-budget-rudp")
        );
        assert_eq!(
            media_profile
                .get("visual_prediction_owner")
                .and_then(|value| value.as_str()),
            Some("NVENC H.264 inter-frame prediction")
        );
        assert_eq!(
            media_profile
                .get("transport_owner")
                .and_then(|value| value.as_str()),
            Some("CultNet RUDP media channel")
        );
        assert_eq!(
            media_profile
                .get("video_rate_control")
                .and_then(|value| value.as_str()),
            Some("cbr")
        );
        assert_eq!(
            media_profile
                .get("video_rc_lookahead")
                .and_then(|value| value.as_u64()),
            Some(0)
        );
        assert_eq!(
            media_profile
                .get("video_bitrate")
                .and_then(|value| value.as_str()),
            Some("12000k")
        );
        assert_eq!(
            media_profile
                .get("video_bufsize")
                .and_then(|value| value.as_str()),
            Some("400k")
        );
        assert_eq!(
            media_profile
                .get("video_low_delay_key_frame_scale")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_LOW_DELAY_KEY_FRAME_SCALE as u64)
        );
        assert_eq!(
            media_profile
                .get("media_packet_bytes")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_PACKET_BYTES as u64)
        );
        assert_eq!(
            media_profile
                .get("max_fragment_bytes")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES as u64)
        );
        assert_eq!(
            media_profile
                .get("receiver_gap_wait_ms")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_RECEIVER_GAP_WAIT_MS)
        );
        assert_eq!(
            media_profile
                .get("sender_delivery_deadline_ms")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS)
        );
        assert_eq!(
            media_profile
                .get("sender_resend_delay_ms")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_RESEND_DELAY_MS)
        );
        assert_eq!(
            media_profile
                .get("late_media_policy")
                .and_then(|value| value.as_str()),
            Some("drop expired queued media; do not repair frames outside the latency budget")
        );

        let routes = provider
            .value
            .get("routes")
            .and_then(|value| value.as_array())
            .unwrap();
        assert!(!routes.iter().any(|route| {
            route
                .get("address")
                .and_then(|value| value.as_str())
                .is_some_and(|entry| entry.contains("C:/Meta/Odin/state/muninn.activate.cc"))
        }));
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn runtime_boundary_advertises_hid_and_move_evidence_streams() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-boundary-hid-sources-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--store",
                store_path.to_str().unwrap(),
                "--move-evidence-stream",
                "muninn:nightwing:move-evidence",
                "--hid-controller-rudp-bind",
                "0.0.0.0:17888",
                "--hid-controller-rudp-advertise",
                "198.51.100.66:17888",
                "--command-rudp-bind",
                "0.0.0.0:17889",
                "--command-rudp-advertise",
                "198.51.100.66:17889",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut node = open_node(&options, "muninn-boundary-hid-sources-test").unwrap();
        let live_sources = vec![MoveStateSource {
            move_id: "nav-windows-psnav-0".to_string(),
            hidraw_path: "windows-psmove://nav".to_string(),
        }];

        publish_runtime_boundary_records(&mut node, &options, "idle", &[], &live_sources).unwrap();

        let provider = node
            .get_required::<EveProviderAdvertisementRecord>("muninn.telemetry.starfire")
            .unwrap();
        let command_route = provider
            .value
            .get("routes")
            .and_then(|value| value.as_array())
            .and_then(|routes| routes.iter().find(|route| {
                route.get("role").and_then(|value| value.as_str())
                    == Some("muninn provider command ingress")
            }))
            .expect("provider-owned command route");
        assert_eq!(
            command_route.get("uri").and_then(|value| value.as_str()),
            Some("cultmesh://198.51.100.66:17889/muninn/starfire/commands")
        );
        let streams = provider
            .value
            .get("inputStreams")
            .and_then(|value| value.as_array())
            .unwrap();
        let hid_stream = streams.first().unwrap();
        assert_eq!(streams.len(), 2);
        assert_eq!(
            hid_stream.get("streamId").and_then(|value| value.as_str()),
            Some("muninn:starfire:hid-controller-state")
        );
        assert_eq!(
            hid_stream.get("address").and_then(|value| value.as_str()),
            Some("198.51.100.66:17888")
        );
        let devices = hid_stream
            .get("devices")
            .and_then(|value| value.as_array())
            .unwrap();
        assert_eq!(
            devices
                .first()
                .and_then(|device| device.get("deviceId"))
                .and_then(|value| value.as_str()),
            Some("nav-windows-psnav-0")
        );
        assert_eq!(
            devices
                .first()
                .and_then(|device| device.get("deviceKind"))
                .and_then(|value| value.as_str()),
            Some("ps3-navigation")
        );
        let evidence_stream = &streams[1];
        assert_eq!(
            evidence_stream
                .get("streamId")
                .and_then(|value| value.as_str()),
            Some("muninn:nightwing:move-evidence")
        );
        assert_eq!(
            evidence_stream
                .get("channel")
                .and_then(|value| value.as_str()),
            Some("move-evidence")
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn ps3_navigation_hid_report_uses_live_stick_bytes() {
        let source = MoveStateSource {
            move_id: "nav-windows-psnav-0".to_string(),
            hidraw_path: "windows-psmove://vid_054c&pid_042f".to_string(),
        };
        let mut report = vec![0u8; 49];
        report[0] = 0x01;
        report[6] = 0x7d;
        report[7] = 0x80;
        report[18] = 0x00;

        let axes = hid_controller_axes_from_report(&source, &report);

        assert!(
            axes[0].abs() < 0.02,
            "x axis should be near neutral: {}",
            axes[0]
        );
        assert!(
            axes[1].abs() < 0.02,
            "y axis should be near neutral: {}",
            axes[1]
        );
        assert_eq!(axes[2], -1.0);

        report[6] = 0xff;
        report[7] = 0x00;
        report[18] = 0xff;
        let axes = hid_controller_axes_from_report(&source, &report);

        assert!(axes[0] > 0.95);
        assert!(axes[1] < -0.95);
        assert_eq!(axes[2], 1.0);
    }

    #[test]
    fn runtime_boundary_reports_explicit_rudp_video_bitrate() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-boundary-bitrate-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--store",
                store_path.to_str().unwrap(),
                "--host",
                "raven",
                "--media-transport",
                "rudp",
                "--rudp-video-bitrate-kbps",
                "8000",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut node = open_node(&options, "muninn-boundary-bitrate-test").unwrap();

        publish_runtime_boundary_records(&mut node, &options, "idle", &[], &[]).unwrap();

        let transport = node
            .get_required::<MuninnTransportProfileCompatRecord>("transport-profile:muninn")
            .unwrap();
        let media_profile = transport
            .value
            .get("media_profile")
            .and_then(|value| value.as_object())
            .unwrap();
        assert_eq!(
            media_profile
                .get("video_bitrate")
                .and_then(|value| value.as_str()),
            Some("8000k")
        );
        assert_eq!(
            media_profile
                .get("video_bufsize")
                .and_then(|value| value.as_str()),
            Some("267k")
        );
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn move_identity_status_reads_reopened_store() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-move-identity-reopen-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--store",
                store_path.to_str().unwrap(),
                "--move-host",
                "5C:93:A2:9C:A8:A8",
                "--move-state",
                "move-000704a39772=/dev/input/js0",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut node = open_node(&options, "muninn-move-identity-reopen-test").unwrap();

        publish_move_identity_records(&mut node, &options, &options.move_state_sources).unwrap();
        drop(node);

        let reopened = open_node(&options, "muninn-move-identity-reopen-status").unwrap();
        let identities = reopened
            .cache()
            .get_all::<MuninnMoveIdentityRecord>()
            .unwrap();

        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].move_id, "move-000704a39772");
        assert_eq!(identities[0].source_path, "/dev/input/js0");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn move_identity_reconcile_deletes_stale_host_records() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-move-identity-reconcile-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--store",
                store_path.to_str().unwrap(),
                "--move-host",
                "5C:93:A2:9C:A8:A8",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut node = open_node(&options, "muninn-move-identity-reconcile-test").unwrap();
        let stale = MuninnMoveIdentityRecord {
            identity_id: "starfire:move-0006f523e2d1:move-identity".to_string(),
            host_id: "starfire".to_string(),
            move_id: "move-0006f523e2d1".to_string(),
            source_path: "windows-psmove://stale".to_string(),
            bluetooth_host_address: "5C:93:A2:9C:A8:A8".to_string(),
            state: "usb-visible".to_string(),
            detail: "stale test record".to_string(),
            observed_at: "unix-1".to_string(),
        };
        node.put(&stale.identity_id, &stale).unwrap();
        let current = vec![MoveStateSource {
            move_id: "move-000704a800d0".to_string(),
            hidraw_path: "windows-psmove://current".to_string(),
        }];

        reconcile_move_identity_records(&mut node, &options, &current, &[]).unwrap();

        assert!(
            node.get::<MuninnMoveIdentityRecord>("starfire:move-0006f523e2d1:move-identity")
                .unwrap()
                .is_none()
        );
        let current_record = node
            .get_required::<MuninnMoveIdentityRecord>("starfire:move-000704a800d0:move-identity")
            .unwrap();
        assert_eq!(current_record.source_path, "windows-psmove://current");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn joystick_move_state_publishes_fresh_record_without_new_events() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-empty-joystick-events-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--store",
                store_path.to_str().unwrap(),
                "--move-state",
                "move-usb=/dev/input/js0",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let source = options.move_state_sources[0].clone();
        let mut active = vec![ActiveMoveStateSource {
            source,
            sequence: 0,
            joystick_axes: [0; 16],
            joystick_buttons: [false; 32],
            light_hidraw_path: None,
            latest_move_record: None,
        }];
        let mut reader = RecordingMoveStateReader {
            reports: Vec::new(),
            joystick_events: Vec::new(),
            failing_joystick_path: None,
        };
        let mut node = open_node(&options, "muninn-empty-joystick-test").unwrap();

        publish_move_controller_states(&mut node, &options, &mut active, &mut reader, None, None)
            .unwrap();

        let record = node
            .get_required::<MuninnMoveControllerStateRecord>(
                "nightwing:move-usb:move-controller-state",
            )
            .unwrap();
        assert_eq!(
            active[0]
                .latest_move_record
                .as_ref()
                .map(|record| record.sequence),
            Some(1)
        );
        assert_eq!(record.sequence, 1);
        assert_eq!(record.host_id, "nightwing");
        assert_eq!(record.move_id, "move-usb");
        assert_eq!(record.accelerometer_xyz, vec![0.0, 0.0, 0.0]);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn windows_hid_move_state_publishes_hid_controller_record() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-windows-hid-controller-state-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "starfire",
                "--store",
                store_path.to_str().unwrap(),
                "--move-state",
                "nav-windows-psnav-0=windows-psmove://vid_054c&pid_042f",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut active = active_move_state_sources(options.move_state_sources.clone());
        let mut report = vec![0u8; 49];
        report[0] = 0x01;
        report[6] = 0xff;
        report[7] = 0x00;
        report[18] = 0xff;
        let mut reader = RecordingMoveStateReader {
            reports: vec![report],
            joystick_events: Vec::new(),
            failing_joystick_path: None,
        };
        let mut node = open_node(&options, "muninn-windows-hid-controller-state-test").unwrap();

        publish_move_controller_states(&mut node, &options, &mut active, &mut reader, None, None)
            .unwrap();

        let record = node
            .get_required::<MuninnHidControllerStateRecord>(
                "starfire:nav-windows-psnav-0:hid-controller-state",
            )
            .unwrap();
        assert_eq!(record.device_kind, "ps3-navigation");
        assert_eq!(record.sequence, 1);
        assert!(record.axes[0] > 0.95);
        assert!(record.axes[1] < -0.95);
        assert_eq!(record.axes[2], 1.0);
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn missing_move_state_source_does_not_block_other_sources() {
        let store_path = std::env::temp_dir().join(format!(
            "muninn-missing-joystick-events-{}.cc",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--store",
                store_path.to_str().unwrap(),
                "--move-state",
                "missing=/dev/input/js-missing",
                "--move-state",
                "present=/dev/input/js-present",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut active = active_move_state_sources(options.move_state_sources.clone());
        let mut reader = RecordingMoveStateReader {
            reports: Vec::new(),
            joystick_events: Vec::new(),
            failing_joystick_path: Some("/dev/input/js-missing".to_string()),
        };
        let mut node = open_node(&options, "muninn-missing-joystick-test").unwrap();

        publish_move_controller_states(&mut node, &options, &mut active, &mut reader, None, None)
            .unwrap();

        assert!(
            node.get_required::<MuninnMoveControllerStateRecord>(
                "nightwing:missing:move-controller-state"
            )
            .is_err()
        );
        let record = node
            .get_required::<MuninnMoveControllerStateRecord>(
                "nightwing:present:move-controller-state",
            )
            .unwrap();
        assert_eq!(record.move_id, "present");
        let _ = fs::remove_file(store_path);
    }

    #[test]
    fn xinput_gamepad_snapshot_maps_to_generic_hid_controller_record() {
        let options =
            Options::parse(["serve", "--host", "raven"].into_iter().map(String::from)).unwrap();
        let source = MoveStateSource {
            move_id: "xbox-raven".to_string(),
            hidraw_path: "xinput://0".to_string(),
        };
        let gamepad = XinputGamepadSnapshot {
            buttons: XINPUT_GAMEPAD_A_MASK
                | XINPUT_GAMEPAD_B_MASK
                | XINPUT_GAMEPAD_X_MASK
                | XINPUT_GAMEPAD_Y_MASK
                | XINPUT_GAMEPAD_DPAD_UP_MASK,
            left_trigger: 0,
            right_trigger: 255,
            thumb_lx: i16::MAX,
            thumb_ly: i16::MIN,
            thumb_rx: 0,
            thumb_ry: 0,
        };

        let record = build_hid_controller_state_record_from_xinput_gamepad(
            &options,
            &source,
            12,
            &gamepad,
            34,
            "unix-56".to_string(),
        );

        assert_eq!(record.stream_id, "raven:xbox-raven:hid-controller-state");
        assert_eq!(record.device_kind, "xinput-controller");
        assert_eq!(record.axes[0], 1.0);
        assert_eq!(record.axes[1], -1.0);
        assert_eq!(record.axes[2], -1.0);
        assert_eq!(record.axes[5], 1.0);
        assert_eq!(record.buttons, vec!["up", "a", "b", "x", "y"]);
    }

    #[test]
    fn hid_edge_capture_preserves_quick_tap_before_state_collapse() {
        let mut active = active_hid_controller_rudp_source(MoveStateSource {
            move_id: "pad".to_string(),
            hidraw_path: "xinput://0".to_string(),
        });
        capture_button_edges(&mut active, &["a".to_string()]);
        capture_button_edges(&mut active, &[]);
        let edges = active.pending_edges.into_iter().collect::<Vec<_>>();
        assert_eq!(edges.len(), 2);
        assert_eq!((edges[0].button.as_str(), edges[0].pressed, edges[0].edge_sequence), ("a", true, 1));
        assert_eq!((edges[1].button.as_str(), edges[1].pressed, edges[1].edge_sequence), ("a", false, 2));
        assert_eq!(edges[0].epoch, edges[1].epoch);
    }

    #[test]
    fn hid_edge_backlog_rotates_epoch_and_rebases_on_latest_state() {
        let mut active = active_hid_controller_rudp_source(MoveStateSource {
            move_id: "pad-1".to_string(),
            hidraw_path: "test".to_string(),
        });
        let initial_epoch = active.epoch;
        for index in 0..=MUNINN_HID_MAX_PENDING_EDGES {
            let buttons = if index % 2 == 0 {
                vec!["a".to_string()]
            } else {
                Vec::new()
            };
            capture_button_edges(&mut active, &buttons);
        }
        assert!(active.epoch > initial_epoch);
        assert!(active.pending_edges.len() <= MUNINN_HID_MAX_PENDING_EDGES);
        assert_eq!(active.next_edge_sequence, 1);
        assert_eq!(active.edge_buttons, vec!["a".to_string()]);
    }

    #[test]
    fn active_move_state_sync_replaces_stale_hotplug_identity() {
        let mut active = active_move_state_sources(vec![MoveStateSource {
            move_id: "move-old".to_string(),
            hidraw_path: "/dev/input/js0".to_string(),
        }]);

        sync_active_move_state_sources(
            &mut active,
            vec![
                MoveStateSource {
                    move_id: "move-new".to_string(),
                    hidraw_path: "/dev/input/js0".to_string(),
                },
                MoveStateSource {
                    move_id: "move-bt".to_string(),
                    hidraw_path: "/dev/input/js1".to_string(),
                },
            ],
        );

        assert_eq!(active.len(), 2);
        assert!(
            active
                .iter()
                .any(|state| state.source.move_id == "move-new")
        );
        assert!(active.iter().any(|state| state.source.move_id == "move-bt"));
        assert!(
            !active
                .iter()
                .any(|state| state.source.move_id == "move-old")
        );
    }

    #[test]
    fn configured_move_identity_canonicalizes_discovered_joystick_path() {
        let discovered = vec![MoveStateSource {
            move_id: "hid-js0".to_string(),
            hidraw_path: "/dev/input/js0".to_string(),
        }];
        let configured = vec![MoveStateSource {
            move_id: "move-000704a800d0".to_string(),
            hidraw_path: "/dev/input/js0".to_string(),
        }];

        let merged = merge_move_state_sources(discovered, &configured);

        assert_eq!(
            merged,
            vec![MoveStateSource {
                move_id: "move-000704a800d0".to_string(),
                hidraw_path: "/dev/input/js0".to_string(),
            }]
        );
    }

    #[test]
    fn physical_move_identity_survives_configured_path_reuse() {
        let discovered = vec![MoveStateSource {
            move_id: "move-000704a6be5f".to_string(),
            hidraw_path: "/dev/input/js2".to_string(),
        }];
        let configured = vec![MoveStateSource {
            move_id: "move-000704a39772".to_string(),
            hidraw_path: "/dev/input/js2".to_string(),
        }];

        assert_eq!(merge_move_state_sources(discovered.clone(), &configured), discovered);
    }

    #[test]
    fn rudp_sources_reconcile_hotplugged_physical_identity() {
        let mut active = vec![active_hid_controller_rudp_source(MoveStateSource {
            move_id: "move-000704a39772".to_string(),
            hidraw_path: "/dev/input/js2".to_string(),
        })];
        let desired = vec![MoveStateSource {
            move_id: "move-000704a39772".to_string(),
            hidraw_path: "/dev/input/js0".to_string(),
        }];

        sync_hid_controller_rudp_sources(&mut active, desired.clone());

        assert_eq!(active.len(), 1);
        assert_eq!(active[0].source, desired[0]);
    }

    #[test]
    fn move_controller_state_publishes_mimir_compatible_cultmesh_frame() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-state",
                "move-usb=/dev/input/js0",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let source = options.move_state_sources[0].clone();
        let record = build_move_controller_state_record_from_joystick(
            &options,
            &source,
            7,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0, 0],
            {
                let mut buttons = [false; 32];
                buttons[16] = true;
                buttons[17] = true;
                buttons
            },
            123_456_789,
            "unix-1".to_string(),
        );
        let mut stream = create_move_evidence_stream(&options)
            .unwrap()
            .expect("move state source should create a stream");
        let rudp_latest = Arc::new(Mutex::new(None));
        stream.rudp_sender = Some(Arc::clone(&rudp_latest));

        let handle = publish_move_evidence_stream_frame(&mut stream, &[], &[record.clone()])
            .unwrap()
            .expect("controller state should publish a frame");

        assert_eq!(handle.stream_id, "muninn:nightwing:move-evidence");
        assert_eq!(
            stream
                .catalog
                .latest_frame("muninn:nightwing:move-evidence")
                .unwrap()
                .sequence,
            0
        );
        assert_eq!(
            stream
                .catalog
                .get("muninn:nightwing:move-evidence")
                .unwrap()
                .metadata_schema_id
                .as_deref(),
            Some("mimir.muninn_move_evidence_stream_frame.v1")
        );
        let lease = stream
            .catalog
            .ring("muninn:nightwing:move-evidence")
            .and_then(CultMeshSharedMemoryFrameRing::try_acquire_latest_read)
            .expect("latest frame should be readable");
        let decoded: DecodedMoveEvidenceStreamFrame = rmp_serde::from_slice(lease.bytes()).unwrap();
        let remote_payload = rudp_latest.lock().unwrap().take().expect("RUDP evidence copy");
        assert_eq!(remote_payload, lease.bytes());

        assert_eq!(decoded.0, "muninn:nightwing:move-evidence:0");
        assert_eq!(decoded.1, "muninn:nightwing");
        assert!(decoded.2 > 0);
        assert!(decoded.3.is_empty());
        assert_eq!(decoded.4.len(), 1);
        assert_eq!(decoded.4[0].stream_id, record.stream_id);
        assert_eq!(decoded.4[0].host_id, record.host_id);
        assert_eq!(decoded.4[0].move_id, record.move_id);
        assert_eq!(decoded.4[0].sequence, record.sequence);
        assert_eq!(decoded.4[0].source_timestamp_ns, record.source_timestamp_ns);
        assert_eq!(decoded.4[0].accelerometer_xyz, record.accelerometer_xyz);
        assert_eq!(decoded.4[0].gyroscope_xyz, record.gyroscope_xyz);
        assert_eq!(decoded.4[0].magnetometer_xyz, record.magnetometer_xyz);
        assert_eq!(decoded.4[0].buttons, record.buttons);
        assert!(decoded.4[0].battery01.is_nan());
    }

    #[test]
    fn move_marker_candidates_publish_in_mimir_compatible_cultmesh_frame() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-evidence-stream",
                "muninn:nightwing:move-evidence",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut y8 = vec![0u8; 32 * 32];
        for y in 8..12 {
            for x in 10..14 {
                y8[y * 32 + x] = 255;
            }
        }
        let frame_source = MoveMarkerFrameSource {
            stream_id: "muninn:nightwing:ps3eye0:move-markers".to_string(),
            host_id: "nightwing".to_string(),
            camera_id: "ps3eye0".to_string(),
            fps: 187,
            tracker_config: muninn_move_tracker::MoveTrackerConfig {
                width: 32,
                height: 32,
                stride_bytes: 32,
                tile_size: 16,
                threshold_min: 180,
                min_area_px: 4,
                max_candidates: 8,
                source_id_hash: 42,
                frame_sequence: 7,
            },
        };
        let mut stream = create_move_evidence_stream(&options)
            .unwrap()
            .expect("move evidence option should create a stream");

        let marker_candidates = extract_move_marker_candidates_from_luma_frame(
            &frame_source,
            &y8,
            "unix-1".to_string(),
        )
        .unwrap();
        let handle = publish_move_evidence_stream_frame(&mut stream, &marker_candidates, &[])
            .unwrap()
            .expect("marker candidates should publish a frame");

        assert_eq!(handle.stream_id, "muninn:nightwing:move-evidence");
        let lease = stream
            .catalog
            .ring("muninn:nightwing:move-evidence")
            .and_then(CultMeshSharedMemoryFrameRing::try_acquire_latest_read)
            .expect("latest frame should be readable");
        let decoded: DecodedMoveEvidenceStreamFrame = rmp_serde::from_slice(lease.bytes()).unwrap();

        assert_eq!(decoded.0, "muninn:nightwing:move-evidence:0");
        assert_eq!(decoded.1, "muninn:nightwing");
        assert!(decoded.2 > 0);
        assert_eq!(decoded.3.len(), 1);
        assert!(decoded.4.is_empty());
        let marker = &decoded.3[0];
        assert_eq!(marker.stream_id, "muninn:nightwing:ps3eye0:move-markers");
        assert_eq!(marker.host_id, "nightwing");
        assert_eq!(marker.camera_id, "ps3eye0");
        assert_eq!(marker.frame_sequence, 7);
        assert_eq!(marker.source_id_hash, 42);
        assert_eq!(marker.tile_x, 0);
        assert_eq!(marker.tile_y, 0);
        assert!(marker.center_x_px > 11.0 && marker.center_x_px < 12.0);
        assert!(marker.center_y_px > 9.0 && marker.center_y_px < 10.0);
        assert!(marker.radius_px > 2.0);
        assert_eq!(marker.area_px, 16);
        assert_eq!(marker.peak_luma, 255);
        assert!(marker.mean_luma > 250.0);
        assert!(marker.score > 0.65);
        assert_eq!(marker.observed_at, "unix-1");
        assert!(marker.move_id.is_empty());
    }

    #[test]
    fn move_marker_camera_option_builds_owned_evidence_source() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-marker-camera",
                "ps3eye0=/dev/video0",
                "--move-marker-width",
                "32",
                "--move-marker-height",
                "32",
                "--move-marker-fps",
                "120",
                "--move-marker-threshold-min",
                "170",
                "--move-marker-min-area-px",
                "3",
                "--move-marker-max-candidates",
                "9",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        assert_eq!(options.move_marker_camera_sources.len(), 1);
        assert_eq!(options.move_marker_camera_sources[0].camera_id, "ps3eye0");
        assert_eq!(
            options.move_marker_camera_sources[0].device_path,
            PathBuf::from("/dev/video0")
        );
        let active = active_move_marker_camera_sources(
            &options,
            Arc::new(Mutex::new(bootstrap_move_hue_program(&options))),
        );
        assert_eq!(active.len(), 1);
        assert_eq!(
            active[0].frame_source.stream_id,
            "muninn:nightwing:ps3eye0:move-marker-candidates"
        );
        assert_eq!(active[0].frame_source.fps, 120);
        assert_eq!(active[0].frame_source.tracker_config.width, 32);
        assert_eq!(active[0].frame_source.tracker_config.height, 32);
        assert_eq!(active[0].frame_source.tracker_config.threshold_min, 170);
        assert_eq!(active[0].frame_source.tracker_config.min_area_px, 3);
        assert_eq!(active[0].frame_source.tracker_config.max_candidates, 9);
        assert!(
            create_move_evidence_stream(&options).unwrap().is_some(),
            "marker camera alone should create the evidence stream"
        );
    }

    #[test]
    fn move_marker_camera_tick_publishes_extracted_evidence_frame() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-marker-camera",
                "ps3eye0=/dev/video0",
                "--move-evidence-stream",
                "muninn:nightwing:move-evidence",
                "--move-marker-width",
                "32",
                "--move-marker-height",
                "32",
                "--move-marker-threshold-min",
                "180",
                "--move-marker-min-area-px",
                "4",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut y8 = vec![0u8; 32 * 32];
        for y in 8..12 {
            for x in 10..14 {
                y8[y * 32 + x] = 255;
            }
        }
        let mut active = active_move_marker_camera_sources(
            &options,
            Arc::new(Mutex::new(bootstrap_move_hue_program(&options))),
        );
        let mut reader = RecordingMoveMarkerCameraReader {
            frames: vec![y8],
            configs: Vec::new(),
        };
        let mut stream = create_move_evidence_stream(&options)
            .unwrap()
            .expect("marker camera should create stream");

        publish_move_marker_camera_frames(&mut active, &mut reader, &[], Some(&mut stream))
            .unwrap();

        assert_eq!(active[0].sequence, 1);
        assert_eq!(reader.configs.len(), 1);
        assert_eq!(reader.configs[0].frame_sequence, 1);
        assert_ne!(reader.configs[0].source_id_hash, 0);
        let lease = stream
            .catalog
            .ring("muninn:nightwing:move-evidence")
            .and_then(CultMeshSharedMemoryFrameRing::try_acquire_latest_read)
            .expect("latest marker evidence frame should be readable");
        let decoded: DecodedMoveEvidenceStreamFrame = rmp_serde::from_slice(lease.bytes()).unwrap();

        assert_eq!(decoded.0, "muninn:nightwing:move-evidence:0");
        assert_eq!(decoded.1, "muninn:nightwing");
        assert_eq!(decoded.3.len(), 1);
        assert!(decoded.4.is_empty());
        assert_eq!(
            decoded.3[0].stream_id,
            "muninn:nightwing:ps3eye0:move-marker-candidates"
        );
        assert_eq!(decoded.3[0].camera_id, "ps3eye0");
        assert_eq!(decoded.3[0].frame_sequence, 1);
        assert_eq!(
            decoded.3[0].source_id_hash,
            reader.configs[0].source_id_hash
        );
    }

    #[test]
    fn move_marker_camera_tick_bundles_latest_controller_state() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-state",
                "move-usb=/dev/input/js0",
                "--move-marker-camera",
                "ps3eye0=/dev/video0",
                "--move-marker-camera",
                "ps3eye1=/dev/video1",
                "--move-evidence-stream",
                "muninn:nightwing:move-evidence",
                "--move-marker-width",
                "32",
                "--move-marker-height",
                "32",
                "--move-marker-threshold-min",
                "180",
                "--move-marker-min-area-px",
                "4",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut y8 = vec![0u8; 32 * 32];
        for y in 8..12 {
            for x in 10..14 {
                y8[y * 32 + x] = 255;
            }
        }
        let mut active_cameras = active_move_marker_camera_sources(
            &options,
            Arc::new(Mutex::new(bootstrap_move_hue_program(&options))),
        );
        let mut reader = RecordingMoveMarkerCameraReader {
            frames: vec![y8.clone(), y8],
            configs: Vec::new(),
        };
        let source = options.move_state_sources[0].clone();
        let controller = build_move_controller_state_record_from_joystick(
            &options,
            &source,
            7,
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0, 0, 0, 0, 0],
            [false; 32],
            123_456_789,
            "unix-123".to_string(),
        );
        let mut stream = create_move_evidence_stream(&options)
            .unwrap()
            .expect("marker camera should create stream");

        publish_move_marker_camera_frames(
            &mut active_cameras,
            &mut reader,
            std::slice::from_ref(&controller),
            Some(&mut stream),
        )
        .unwrap();

        let lease = stream
            .catalog
            .ring("muninn:nightwing:move-evidence")
            .and_then(CultMeshSharedMemoryFrameRing::try_acquire_latest_read)
            .expect("latest bundled evidence frame should be readable");
        let decoded: DecodedMoveEvidenceStreamFrame = rmp_serde::from_slice(lease.bytes()).unwrap();

        assert_eq!(decoded.3.len(), 2);
        assert_eq!(decoded.4.len(), 1);
        assert_ne!(decoded.3[0].source_id_hash, decoded.3[1].source_id_hash);
        assert_eq!(decoded.4[0].move_id, "move-usb");
        assert_eq!(decoded.4[0].sequence, 7);
        assert_eq!(decoded.4[0].source_path, "/dev/input/js0");
    }

    #[test]
    fn move_evidence_snapshot_writes_mimir_compatible_frame_artifact() {
        let snapshot_path = std::env::temp_dir().join(format!(
            "muninn-move-evidence-snapshot-{}.mpack",
            timestamp_ns().unwrap()
        ));
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-marker-camera",
                "ps3eye0=/dev/video0",
                "--move-evidence-stream",
                "muninn:nightwing:move-evidence",
                "--move-evidence-snapshot",
                snapshot_path.to_str().unwrap(),
                "--move-marker-width",
                "32",
                "--move-marker-height",
                "32",
                "--move-marker-threshold-min",
                "180",
                "--move-marker-min-area-px",
                "4",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let mut y8 = vec![0u8; 32 * 32];
        for y in 8..12 {
            for x in 10..14 {
                y8[y * 32 + x] = 255;
            }
        }
        let mut active = active_move_marker_camera_sources(
            &options,
            Arc::new(Mutex::new(bootstrap_move_hue_program(&options))),
        );
        let mut reader = RecordingMoveMarkerCameraReader {
            frames: vec![y8],
            configs: Vec::new(),
        };
        let mut stream = create_move_evidence_stream(&options)
            .unwrap()
            .expect("marker camera should create stream");

        publish_move_marker_camera_frames(&mut active, &mut reader, &[], Some(&mut stream))
            .unwrap();

        let snapshot_bytes = fs::read(&snapshot_path).expect("snapshot should be written");
        let snapshot: DecodedMimirMoveProofEvidenceFrameSnapshot =
            rmp_serde::from_slice(&snapshot_bytes).unwrap();
        let payload_frame: DecodedMoveEvidenceStreamFrame =
            rmp_serde::from_slice(&snapshot.6).unwrap();
        let payload_start = snapshot_bytes
            .len()
            .checked_sub(snapshot.6.len())
            .expect("payload should be contained in snapshot bytes");
        if snapshot.6.len() <= u8::MAX as usize {
            assert_eq!(snapshot_bytes[payload_start - 2], 0xc4);
            assert_eq!(snapshot_bytes[payload_start - 1] as usize, snapshot.6.len());
        } else {
            assert_eq!(snapshot_bytes[payload_start - 3], 0xc5);
            assert_eq!(
                u16::from_be_bytes([
                    snapshot_bytes[payload_start - 2],
                    snapshot_bytes[payload_start - 1]
                ]) as usize,
                snapshot.6.len()
            );
        }

        assert_eq!(snapshot.0, "muninn:nightwing:move-evidence:0:snapshot");
        assert_eq!(snapshot.1, "muninn:nightwing:move-evidence");
        assert_eq!(snapshot.2, "muninn:nightwing:move-evidence:0");
        assert_eq!(snapshot.3, "muninn:nightwing");
        assert!(snapshot.4 > 0);
        assert!(snapshot.5 > 0);
        assert_eq!(payload_frame.0, snapshot.2);
        assert_eq!(payload_frame.1, snapshot.3);
        assert_eq!(payload_frame.2, snapshot.4);
        assert_eq!(payload_frame.3.len(), 1);
        assert!(payload_frame.4.is_empty());
        assert_eq!(
            payload_frame.3[0].stream_id,
            "muninn:nightwing:ps3eye0:move-marker-candidates"
        );
        let _ = fs::remove_file(snapshot_path);
    }

    #[test]
    fn move_controller_state_projects_raw_hid_report() {
        let options = Options::parse(
            ["serve", "--host", "nightwing"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let source = MoveStateSource {
            move_id: "move-usb".to_string(),
            hidraw_path: "/dev/hidraw1".to_string(),
        };
        let mut report = vec![0u8; 64];
        report[1] = 0b1111_0000;
        report[2] = 0b0000_1001;
        report[3] = 0b0001_1000;
        report[6] = 128;
        report[12] = 4;
        report[19..21].copy_from_slice(&123i16.to_le_bytes());
        report[21..23].copy_from_slice(&(-456i16).to_le_bytes());
        report[23..25].copy_from_slice(&789i16.to_le_bytes());
        report[25..27].copy_from_slice(&11i16.to_le_bytes());
        report[27..29].copy_from_slice(&22i16.to_le_bytes());
        report[29..31].copy_from_slice(&33i16.to_le_bytes());

        let record = build_move_controller_state_record(
            &options,
            &source,
            42,
            &report,
            123_456,
            "unix-1".to_string(),
        );

        assert_eq!(record.stream_id, "nightwing:move-usb:move-controller-state");
        assert_eq!(record.sequence, 42);
        assert_eq!(record.accelerometer_xyz, vec![123.0, -456.0, 789.0]);
        assert_eq!(record.gyroscope_xyz, vec![11.0, 22.0, 33.0]);
        assert!(record.buttons.contains(&"triangle".to_string()));
        assert!(record.buttons.contains(&"trigger".to_string()));
        assert!((record.trigger_value - (128.0 / 255.0)).abs() < f32::EPSILON);
        assert!((record.battery01 - 0.8).abs() < f32::EPSILON);
    }
}

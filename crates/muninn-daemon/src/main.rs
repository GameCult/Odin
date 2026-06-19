mod media_packetizer;

use crate::media_packetizer::{
    AudioAdtsStreamSendConfig, AudioAdtsStreamSendState, MuninnMediaSendPayload,
    MuninnMediaWireRecord, VideoAnnexBStreamSendConfig, VideoAnnexBStreamSendState,
    decode_media_wire_record,
};
use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{
    CultMesh, CultMeshNodeOptions, CultMeshSharedMemoryFrameRing, CultMeshStreamBodyTransport,
    CultMeshStreamCatalog, CultMeshStreamClock, CultMeshStreamDescriptor, CultMeshStreamKind,
};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding,
    CultNetRudpSocketTransportConnection, CultNetRudpSocketTransportOptions, CultNetTransportFrame,
    CultNetWireContract, decode_cultnet_message_from_slice, encode_cultnet_message_to_vec,
};
use odin_core::{
    EveProviderAdvertisementCompatRecord, IdunnDaemonHealthRecord,
    MuninnCaptureStreamCommandRecord, MuninnCaptureStreamRecord, MuninnCommandBoundaryCompatRecord,
    MuninnMediaReceiverFeedbackRecord, MuninnMoveControllerStateRecord, MuninnMoveIdentityRecord,
    MuninnMoveLightCommandRecord, MuninnObsStreamCatalogRecord, MuninnQuestAccessRecord,
    MuninnTelemetrySurfaceRecord, MuninnTransportProfileCompatRecord, OdinDocuments,
};
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::fs;
#[cfg(not(windows))]
use std::io::Write;
use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, UdpSocket};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
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

const CULTNET_RUDP_PROTOCOL_ID: &str = "cultnet.transport.rudp.v0";
const IDUNN_HEALTH_RUDP_CONNECTION_ID: u32 = 0x1d0d_0001;
const MUNINN_COMMAND_RUDP_CONNECTION_ID: u32 = 0x6d75_0002;
const MUNINN_MEDIA_RUDP_CONNECTION_ID: u32 = 0x6d75_0001;
const MUNINN_OBS_CATALOG_RUDP_CONNECTION_ID: u32 = 0x6d75_0003;
const MUNINN_MEDIA_SEND_QUEUE_DEADLINE_MS: u64 = 75;
const MUNINN_RUDP_MEDIA_PROFILE_ID: &str = "muninn.rudp.low_latency_h264_lan.v1";
const MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS: u32 = 48_000;
const MUNINN_RUDP_MEDIA_VBV_FRAME_BUDGETS: u32 = 1;
const MUNINN_RUDP_MEDIA_LOW_DELAY_KEY_FRAME_SCALE: u32 = 4;
const MUNINN_RUDP_MEDIA_PACKET_BYTES: usize = 800;
const MUNINN_RUDP_IPV4_UDP_PAYLOAD_BYTES: usize = 1_472;
const MUNINN_RUDP_FIXED_HEADER_BYTES: usize = 36;
const MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES: usize = MUNINN_RUDP_IPV4_UDP_PAYLOAD_BYTES
    - MUNINN_RUDP_FIXED_HEADER_BYTES
    - crate::media_packetizer::MUNINN_MEDIA_RUDP_CHANNEL.len();
const MUNINN_RUDP_MEDIA_RESEND_DELAY_MS: u64 = 5;
const MUNINN_RUDP_MEDIA_RELIABLE_EXPIRE_AFTER_MS: u64 = 600;
const MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS: u64 = 400;
const MUNINN_RUDP_MEDIA_RECEIVER_GAP_WAIT_MS: u64 = 16;
const MUNINN_RUDP_MEDIA_REPAIR_CACHE_CHUNKS: usize = 16_384;
const MUNINN_RUDP_MEDIA_REPAIR_BURST_CHUNKS: usize = 96;
const PS_MOVE_LED_REPORT_LEN: usize = 49;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Serve,
    Activate,
    Health,
    DryRun,
    RequestStream,
    RequestMoveLight,
    MoveLightStatus,
    MoveIdentityStatus,
    MoveSourceStatus,
    MoveStateStatus,
    ClaimMoveHost,
    QuestAccessStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Options {
    mode: Mode,
    store_path: PathBuf,
    activation_store_path: Option<PathBuf>,
    surface_id: String,
    stream_id: String,
    stream_action: String,
    host_id: String,
    target_host: String,
    port: u16,
    obs_target_host: Option<String>,
    obs_port: u16,
    media_transport: MediaTransport,
    media_packet_bytes: usize,
    width: u32,
    height: u32,
    framerate: u32,
    ddagrab_output_index: u32,
    audio_device: String,
    audio_sample_rate: u32,
    audio_channels: u32,
    ffmpeg_path: String,
    loopback_script: PathBuf,
    log_root: PathBuf,
    interval_seconds: Option<u64>,
    move_id: String,
    move_filter: Option<String>,
    hidraw_path: String,
    move_colors: Vec<String>,
    move_durations_ms: Vec<u32>,
    move_repeat_count: u32,
    command_id: Option<String>,
    move_host_address: Option<String>,
    move_state_sources: Vec<MoveStateSource>,
    move_evidence_stream_id: Option<String>,
    move_evidence_verse_id: String,
    move_evidence_ring_slots: usize,
    move_evidence_slot_bytes: usize,
    quest_adb: bool,
    quest_serial: Option<String>,
    quest_input_stream_id: Option<String>,
    quest_pose_stream_id: Option<String>,
    quest_video_input_stream_id: Option<String>,
    idunn_rudp_health: Option<IdunnRudpHealthOptions>,
    capture_command_rudp_bind: Option<SocketAddr>,
    capture_command_rudp_target: Option<SocketAddr>,
    obs_catalog_rudp_target: Option<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MediaTransport {
    Srt,
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
    sender_reliable_expire_after_ms: u64,
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
    child: Child,
}

struct ActiveMoveStateSource {
    source: MoveStateSource,
    sequence: u64,
    joystick_axes: [i16; 16],
    joystick_buttons: [bool; 32],
    light_hidraw_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DefaultMoveLightTarget {
    path: String,
    identity: String,
}

#[derive(Serialize)]
struct MuninnMoveEvidenceStreamFrame<'a>(
    &'a str,
    &'a str,
    i64,
    &'a [MuninnMoveMarkerCandidateWire],
    &'a [MuninnMoveControllerStateRecord],
);

#[derive(Serialize)]
struct MuninnMoveMarkerCandidateWire;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JoystickEvent {
    event_type: u8,
    number: u8,
    value: i16,
}

trait ProcessSpawner {
    fn spawn_mux(&self, plan: &MuxPlan) -> Result<Child>;
}

struct CmdSpawner;

impl ProcessSpawner for CmdSpawner {
    fn spawn_mux(&self, plan: &MuxPlan) -> Result<Child> {
        Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&plan.command_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting mux command {}", plan.command_file.display()))
    }
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;
    match options.mode {
        Mode::Serve => serve(options),
        Mode::Activate => activate(options, CmdSpawner),
        Mode::Health => health_check(&options),
        Mode::RequestStream => request_capture_stream(options),
        Mode::RequestMoveLight => request_move_light(options),
        Mode::MoveLightStatus => move_light_status(options),
        Mode::MoveIdentityStatus => move_identity_status(options),
        Mode::MoveSourceStatus => move_source_status(options),
        Mode::MoveStateStatus => move_state_status(options),
        Mode::ClaimMoveHost => claim_move_host(options),
        Mode::QuestAccessStatus => quest_access_status(options),
        Mode::DryRun => {
            let plan = build_mux_plan(&options, "dry-run".to_string());
            println!("{}", plan.command_line);
            Ok(())
        }
    }
}

fn serve(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let mut move_evidence_stream = create_move_evidence_stream(&options)?;
    let mut active_move_lights = Vec::new();
    let mut last_default_move_light_write_at = None;
    let mut last_idunn_health_publish_attempt_at = None;
    let mut last_move_host_claim_attempt_at = None;
    let mut last_move_bluetooth_pickup_attempt_at = None;
    let move_runtime_enabled = serve_should_manage_move_runtime(&options);
    let mut active_move_states =
        active_move_state_sources(serve_move_state_sources(&options, move_runtime_enabled));
    let mut active_capture_streams = Vec::new();
    start_capture_command_rudp_ingress(&options)?;

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
            tick_move_light_commands(&mut node, &mut active_move_lights, &mut HidMoveLightWriter)?;
            tick_default_move_light_pulse(
                &mut active_move_states,
                &active_move_lights,
                &mut last_default_move_light_write_at,
                serve_should_manage_platform_move_lights(&options),
                &mut HidMoveLightWriter,
            );
            publish_move_controller_states(
                &mut node,
                &options,
                &mut active_move_states,
                &mut HidMoveControllerStateReader,
                move_evidence_stream.as_mut(),
            )?;
            publish_quest_access_if_requested(&mut node, &options)?;
            let state = if active_stream_ids.is_empty() {
                "idle"
            } else {
                "streaming"
            };
            publish_surface(&mut node, &options, state, &active_stream_ids)?;
            publish_runtime_boundary_records(&mut node, &options, state, &active_stream_ids)?;
        }
        claim_move_host_if_due(&options, &mut last_move_host_claim_attempt_at);
        pickup_bluetooth_moves_if_due(
            &active_move_states,
            &mut last_move_bluetooth_pickup_attempt_at,
        );
        publish_daemon_health_if_configured(&options, &mut last_idunn_health_publish_attempt_at)?;
        let has_platform_default_move_lights = serve_should_manage_platform_move_lights(&options);
        if options.interval_seconds.is_none()
            && active_move_lights.is_empty()
            && active_move_states.is_empty()
            && active_capture_streams.is_empty()
            && !has_platform_default_move_lights
        {
            return Ok(());
        }
        let sleep = if !active_move_lights.is_empty()
            || !active_move_states.is_empty()
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

fn tick_capture_stream_commands(
    options: &Options,
    active: &mut Vec<ActiveCaptureStreamCommand>,
) -> Result<Vec<String>> {
    reap_capture_stream_children(options, active)?;
    let Some(activation_store_path) = options.activation_store_path.as_ref() else {
        return Ok(active
            .iter()
            .map(|session| session.stream_id.clone())
            .collect());
    };
    let mut activation_options = options.clone();
    activation_options.store_path = activation_store_path.clone();
    let mut node = open_node(&activation_options, "muninn-activation-controller")?;
    let mut commands = node.cache().get_all::<MuninnCaptureStreamCommandRecord>()?;
    commands.sort_by(|left, right| left.updated_at.cmp(&right.updated_at));
    let mut latest_command_by_stream = HashMap::new();
    for command in commands
        .iter()
        .filter(|command| command.host_id == options.host_id)
    {
        latest_command_by_stream.insert(command.stream_id.clone(), command.command_id.clone());
    }
    for command in commands {
        if command.host_id != options.host_id {
            continue;
        }
        if latest_command_by_stream
            .get(&command.stream_id)
            .is_some_and(|command_id| command_id != &command.command_id)
        {
            continue;
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

fn start_capture_command_rudp_ingress(options: &Options) -> Result<()> {
    let Some(bind_address) = options.capture_command_rudp_bind else {
        return Ok(());
    };
    let Some(command_store_path) = options
        .activation_store_path
        .as_ref()
        .or(Some(&options.store_path))
        .cloned()
    else {
        return Ok(());
    };

    let socket = UdpSocket::bind(bind_address).with_context(|| {
        format!("binding Muninn capture command RUDP ingress at {bind_address}")
    })?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let runtime_options = options.clone();
    thread::spawn(move || {
        if let Err(error) =
            run_capture_command_rudp_ingress(socket, runtime_options, command_store_path)
        {
            eprintln!("Muninn capture command RUDP ingress stopped: {error:#}");
        }
    });
    Ok(())
}

fn run_capture_command_rudp_ingress(
    socket: UdpSocket,
    options: Options,
    command_store_path: PathBuf,
) -> Result<()> {
    let local_addr = socket.local_addr()?;
    println!("Muninn capture command RUDP ingress listening at {local_addr}.");
    let mut command_options = options.clone();
    command_options.store_path = command_store_path;
    ensure_state_dirs(&command_options)?;

    loop {
        let mut transport =
            CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::server(
                "muninn-capture-command-ingress",
                socket
                    .try_clone()
                    .context("cloning Muninn capture command RUDP socket")?,
                MUNINN_COMMAND_RUDP_CONNECTION_ID,
            ))?;
        loop {
            if let Some(frame) = transport.receive_once()? {
                transport.poll_resends()?;
                if frame.channel_id != "schema" {
                    continue;
                }
                match capture_command_from_rudp_frame(&frame.payload) {
                    Ok(command) => {
                        let mut node = open_node(&command_options, "muninn-capture-command-rudp")?;
                        node.put(&command.command_id, &command)?;
                        println!(
                            "Muninn accepted RUDP capture command {} {} {} for {}.",
                            command.command_id, command.action, command.stream_id, command.host_id
                        );
                    }
                    Err(error) => {
                        eprintln!("Muninn rejected RUDP capture command frame: {error:#}");
                    }
                }
                break;
            } else {
                transport.poll_resends()?;
                if transport.connected() && transport.check_timeout(2_000) {
                    break;
                }
            }
        }
    }
}

fn capture_command_from_rudp_frame(payload: &[u8]) -> Result<MuninnCaptureStreamCommandRecord> {
    let message = decode_cultnet_message_from_slice(payload, CultNetWireContract::CultNetSchemaV0)
        .context("decoding Muninn capture command CultNet message")?;
    let CultNetMessage::DocumentPutRaw { document, .. } = message else {
        return Err(anyhow!("expected cultnet.document_put_raw.v0"));
    };
    if document.schema_id != "muninn.capture_stream_command" {
        return Err(anyhow!(
            "expected muninn.capture_stream_command schema, received {}",
            document.schema_id
        ));
    }
    if document.payload_encoding != CultNetRawPayloadEncoding::Messagepack {
        return Err(anyhow!("expected MessagePack raw payload encoding"));
    }
    let command: MuninnCaptureStreamCommandRecord = rmp_serde::from_slice(&document.payload)
        .context("decoding Muninn capture command payload")?;
    if document.record_key != command.command_id {
        return Err(anyhow!(
            "record key {} does not match command_id {}",
            document.record_key,
            command.command_id
        ));
    }
    Ok(command)
}

fn start_capture_stream_command(
    options: &Options,
    node: &mut cultmesh_rs::CultMeshNode,
    active: &mut Vec<ActiveCaptureStreamCommand>,
    command: MuninnCaptureStreamCommandRecord,
) -> Result<()> {
    for session in active.iter_mut() {
        if session.stream_id == command.stream_id {
            terminate_child_tree(&mut session.child);
            let _ = session.child.wait();
        }
    }
    active.retain(|session| session.stream_id != command.stream_id);

    let child = spawn_capture_stream_activation(options, &command)?;
    let running = MuninnCaptureStreamCommandRecord {
        state: "running".to_string(),
        detail: "Muninn serve spawned the local activation child.".to_string(),
        updated_at: timestamp()?,
        ..command.clone()
    };
    node.put(&running.command_id, &running)?;
    active.push(ActiveCaptureStreamCommand {
        command_id: command.command_id,
        stream_id: command.stream_id,
        child,
    });
    Ok(())
}

fn stop_capture_stream_command(
    node: &mut cultmesh_rs::CultMeshNode,
    active: &mut Vec<ActiveCaptureStreamCommand>,
    command: MuninnCaptureStreamCommandRecord,
) -> Result<()> {
    let mut stopped = false;
    let mut index = 0;
    while index < active.len() {
        if active[index].stream_id == command.stream_id {
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
    ];
    if let Some(obs_target_host) = command.obs_target_host.as_ref() {
        args.extend([
            "--obs-target-host".to_string(),
            obs_target_host.clone(),
            "--obs-port".to_string(),
            command.obs_port.to_string(),
        ]);
    } else {
        args.push("--no-obs-target".to_string());
    }
    args.extend([
        "--audio-device".to_string(),
        options.audio_device.clone(),
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

    let mut sources = discovered;
    for source in &options.move_state_sources {
        if is_joystick_path(&source.hidraw_path)
            && sources
                .iter()
                .any(|discovered| discovered.hidraw_path == source.hidraw_path)
        {
            continue;
        }
        if !sources.iter().any(|discovered| discovered == source) {
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
fn pickup_bluetooth_moves(active: &[ActiveMoveStateSource]) -> Result<()> {
    let devices = bluetoothctl_motion_controller_devices()?;
    for device in devices {
        if device.connected
            || !device.trusted
            || active_move_source_has_bluetooth_address(active, &device.address)
        {
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

#[cfg(not(unix))]
fn pickup_bluetooth_moves(_active: &[ActiveMoveStateSource]) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn active_move_source_has_bluetooth_address(
    active: &[ActiveMoveStateSource],
    address: &str,
) -> bool {
    bluetooth_address_move_id(address).is_some_and(|move_id| {
        active
            .iter()
            .any(|state| state.source.move_id.eq_ignore_ascii_case(&move_id))
    })
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
}

fn activate(options: Options, spawner: impl ProcessSpawner) -> Result<()> {
    if options.media_transport == MediaTransport::Rudp {
        return activate_rudp(options);
    }

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
        )?;

        let mut child = match spawner.spawn_mux(&plan) {
            Ok(child) => child,
            Err(error) => {
                publish_stream(
                    &mut node,
                    &options,
                    &plan,
                    "failed",
                    supervisor_pid,
                    None,
                    restart_count,
                    &format!("could not start mux: {error:#}"),
                )?;
                return Err(error);
            }
        };

        publish_stream(
            &mut node,
            &options,
            &plan,
            "running",
            supervisor_pid,
            Some(child.id()),
            restart_count,
            "requested stream is active",
        )?;

        let status = child.wait().context("waiting for mux process")?;
        restart_count += 1;
        publish_stream(
            &mut node,
            &options,
            &plan,
            "restarting",
            supervisor_pid,
            None,
            restart_count,
            &format!("mux exited with {status}"),
        )?;
        thread::sleep(Duration::from_secs((2 + restart_count).min(30) as u64));
    }
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

fn run_rudp_mux_once(
    options: &Options,
    plan: &MuxPlan,
    node: &mut cultmesh_rs::CultMeshNode,
    supervisor_pid: u32,
    restart_count: u32,
) -> Result<RudpMuxRestart> {
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

    let mut loopback = Command::new("powershell.exe")
        .args(loopback_args(options))
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&loopback_stderr)?)
        .spawn()
        .with_context(|| {
            format!(
                "starting loopback capture {}",
                options.loopback_script.display()
            )
        })?;
    let loopback_stdout = loopback
        .stdout
        .take()
        .context("loopback stdout was not piped")?;

    let mut video_ffmpeg = Command::new(&options.ffmpeg_path)
        .args(rudp_video_ffmpeg_args(options))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&video_ffmpeg_stderr)?)
        .spawn()
        .with_context(|| format!("starting {} video encoder", options.ffmpeg_path))?;
    let video_ffmpeg_stdout = video_ffmpeg
        .stdout
        .take()
        .context("video ffmpeg stdout was not piped")?;
    let video_ffmpeg_pid = video_ffmpeg.id();

    let mut audio_ffmpeg = Command::new(&options.ffmpeg_path)
        .args(rudp_audio_ffmpeg_args(options))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(fs::File::create(&audio_ffmpeg_stderr)?)
        .spawn()
        .with_context(|| format!("starting {} audio encoder", options.ffmpeg_path))?;
    let mut audio_ffmpeg_stdin = audio_ffmpeg
        .stdin
        .take()
        .context("audio ffmpeg stdin was not piped")?;
    let audio_ffmpeg_stdout = audio_ffmpeg
        .stdout
        .take()
        .context("audio ffmpeg stdout was not piped")?;

    let audio_pump = thread::spawn(move || -> Result<()> {
        let mut reader = loopback_stdout;
        std::io::copy(&mut reader, &mut audio_ffmpeg_stdin)?;
        Ok(())
    });

    let mut transport = open_media_rudp_transport(options)?;
    publish_stream(
        node,
        options,
        plan,
        "running",
        supervisor_pid,
        Some(video_ffmpeg_pid),
        restart_count,
        "typed video/audio access units are publishing over CultNet RUDP media",
    )?;

    let media_profile = muninn_rudp_media_profile();
    let (payload_tx, payload_rx) = mpsc::channel::<Result<QueuedMuninnMediaSendPayload>>();
    let video_sender = video_rudp_payload_reader(
        payload_tx.clone(),
        video_ffmpeg_stdout,
        VideoAnnexBStreamSendConfig {
            stream_id: options.stream_id.clone(),
            session_id: format!("{}:{timestamp}:video", options.host_id),
            codec: media_profile.video_codec.to_string(),
            first_frame_id: 0,
            first_pts_ticks: 0,
            frame_duration_ticks: video_frame_duration_ticks(options)?,
            timebase_num: 1,
            timebase_den: 90_000,
            deadline_delay_ticks: i64::from(video_frame_duration_ticks(options)?),
            max_payload_bytes: options.media_packet_bytes.max(256),
            max_pending_bytes: options.media_packet_bytes.max(256) * 4096,
            source_runtime_id: options.host_id.clone(),
            source_role: "muninn.rudp.video".to_string(),
        },
    );
    let audio_sender = audio_rudp_payload_reader(
        payload_tx.clone(),
        audio_ffmpeg_stdout,
        AudioAdtsStreamSendConfig {
            stream_id: options.stream_id.clone(),
            session_id: format!("{}:{timestamp}:audio", options.host_id),
            codec: "aac-adts".to_string(),
            first_packet_id: 0,
            first_pts_ticks: 0,
            packet_duration_ticks: 1_024,
            timebase_num: 1,
            timebase_den: options.audio_sample_rate,
            deadline_delay_ticks: 1_024,
            max_pending_bytes: options.media_packet_bytes.max(256) * 64,
            source_runtime_id: options.host_id.clone(),
            source_role: "muninn.rudp.audio".to_string(),
        },
    );
    drop(payload_tx);

    let mut payloads_sent = 0_u64;
    let mut payloads_dropped = 0_u64;
    let mut receiver_feedback = MuninnRudpReceiverFeedbackStats::default();
    let mut handled_keyframe_requests = 0_u64;
    let mut repair_cache = RecentVideoChunkRepairCache::new(MUNINN_RUDP_MEDIA_REPAIR_CACHE_CHUNKS);
    let result = loop {
        match payload_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(Ok(queued)) => {
                if media_payload_queue_age_exceeded(
                    queued.queued_at,
                    Instant::now(),
                    Duration::from_millis(media_profile.sender_queue_deadline_ms),
                ) {
                    payloads_dropped += 1;
                    poll_rudp_media_receiver_feedback(
                        &mut transport,
                        &mut receiver_feedback,
                        &repair_cache,
                    )?;
                    record_receiver_keyframe_pressure(
                        &receiver_feedback,
                        &mut handled_keyframe_requests,
                    );
                    poll_rudp_resends_with_backpressure(&mut transport)?;
                    if payloads_dropped == 1 || payloads_dropped % 300 == 0 {
                        let expired = transport.stats().reliable_packets_expired;
                        eprintln!(
                            "{}",
                            rudp_media_progress_detail(
                                payloads_sent,
                                payloads_dropped,
                                expired,
                                &receiver_feedback
                            )
                        );
                    }
                    continue;
                }

                let payload_len = queued.payload.payload.len();
                if !send_rudp_media_payload_with_backpressure(
                    &mut transport,
                    queued.payload.clone(),
                    queued.queued_at,
                    Duration::from_millis(media_profile.sender_queue_deadline_ms),
                )? {
                    payloads_dropped += 1;
                    continue;
                }
                repair_cache.remember(&queued.payload)?;
                poll_rudp_media_receiver_feedback(
                    &mut transport,
                    &mut receiver_feedback,
                    &repair_cache,
                )?;
                record_receiver_keyframe_pressure(
                    &receiver_feedback,
                    &mut handled_keyframe_requests,
                );
                poll_rudp_resends_with_backpressure(&mut transport)?;
                payloads_sent += 1;
                if payloads_sent == 1 || payloads_sent % 900 == 0 {
                    let expired = transport.stats().reliable_packets_expired;
                    eprintln!(
                        "{}; latest payload was {payload_len} bytes.",
                        rudp_media_progress_detail(
                            payloads_sent,
                            payloads_dropped,
                            expired,
                            &receiver_feedback
                        )
                    );
                }
            }
            Ok(Err(error)) => break Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                poll_rudp_media_receiver_feedback(
                    &mut transport,
                    &mut receiver_feedback,
                    &repair_cache,
                )?;
                record_receiver_keyframe_pressure(
                    &receiver_feedback,
                    &mut handled_keyframe_requests,
                );
                poll_rudp_resends_with_backpressure(&mut transport)?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let expired = transport.stats().reliable_packets_expired;
                break Ok(RudpMuxRestart {
                    detail: format!(
                        "encoder stdout ended; {}",
                        rudp_media_progress_detail(
                            payloads_sent,
                            payloads_dropped,
                            expired,
                            &receiver_feedback
                        )
                    ),
                    delay: default_rudp_mux_restart_delay(restart_count),
                });
            }
        }
    };

    let _ = video_ffmpeg.kill();
    let _ = audio_ffmpeg.kill();
    let _ = loopback.kill();
    let _ = video_ffmpeg.wait();
    let _ = audio_ffmpeg.wait();
    let _ = loopback.wait();
    let _ = audio_pump.join();
    let _ = video_sender.join();
    let _ = audio_sender.join();
    result
}

struct QueuedMuninnMediaSendPayload {
    payload: MuninnMediaSendPayload,
    queued_at: Instant,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct MuninnRudpReceiverFeedbackStats {
    feedback_records: u64,
    requested_keyframes: u64,
    late_frames: u64,
    missing_video_chunks: u64,
    repaired_video_chunks: u64,
    highest_decodable_frame_id: Option<u64>,
}

#[derive(Debug)]
struct RecentVideoChunkRepairCache {
    max_entries: usize,
    order: VecDeque<String>,
    entries: HashMap<String, MuninnMediaSendPayload>,
}

impl RecentVideoChunkRepairCache {
    fn new(max_entries: usize) -> Self {
        Self {
            max_entries,
            order: VecDeque::new(),
            entries: HashMap::new(),
        }
    }

    fn remember(&mut self, payload: &MuninnMediaSendPayload) -> Result<()> {
        let Some(key) = video_repair_cache_key_from_payload(payload)? else {
            return Ok(());
        };
        if self.entries.contains_key(&key) {
            self.entries.insert(key, payload.clone());
            return Ok(());
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, payload.clone());
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
    ) -> Vec<MuninnMediaSendPayload> {
        feedback
            .missing_video_chunk_keys
            .iter()
            .filter_map(|chunk_key| {
                self.entries
                    .get(&video_repair_cache_key(
                        &feedback.stream_id,
                        &feedback.session_id,
                        chunk_key,
                    ))
                    .cloned()
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

fn queue_muninn_media_payload(
    tx: &mpsc::Sender<Result<QueuedMuninnMediaSendPayload>>,
    payload: MuninnMediaSendPayload,
) -> Result<()> {
    tx.send(Ok(QueuedMuninnMediaSendPayload {
        payload,
        queued_at: Instant::now(),
    }))
    .context("queueing typed Muninn media payload")
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
) -> Result<bool> {
    loop {
        match transport.send(payload.channel_id, payload.payload.clone()) {
            Ok(()) => return Ok(true),
            Err(error) if is_would_block_error(&error) => {
                if media_payload_queue_age_exceeded(queued_at, Instant::now(), max_age) {
                    return Ok(false);
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
) {
    if receiver_feedback.requested_keyframes <= *handled_keyframe_requests {
        return;
    }
    *handled_keyframe_requests = receiver_feedback.requested_keyframes;
    eprintln!(
        "Muninn RUDP receiver requested a fresh keyframe; continuing current low-latency encoder session until explicit encoder control exists."
    );
}

fn rudp_media_progress_detail(
    sent: u64,
    queue_dropped: u64,
    reliable_expired: u64,
    receiver_feedback: &MuninnRudpReceiverFeedbackStats,
) -> String {
    format!(
        "Muninn RUDP media progress: sent={sent} queue_dropped={queue_dropped} reliable_expired={reliable_expired} receiver_feedback={} receiver_keyframes={} receiver_late_frames={} receiver_missing_chunks={} receiver_repaired_chunks={} receiver_highest_decodable={}",
        receiver_feedback.feedback_records,
        receiver_feedback.requested_keyframes,
        receiver_feedback.late_frames,
        receiver_feedback.missing_video_chunks,
        receiver_feedback.repaired_video_chunks,
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
) -> Result<()> {
    loop {
        match transport.receive_once() {
            Ok(Some(frame)) => {
                let repair_payloads =
                    record_rudp_media_receiver_feedback(&frame, stats, repair_cache)?;
                for payload in repair_payloads
                    .into_iter()
                    .take(MUNINN_RUDP_MEDIA_REPAIR_BURST_CHUNKS)
                {
                    if send_rudp_media_payload_with_backpressure(
                        transport,
                        payload,
                        Instant::now(),
                        Duration::from_millis(MUNINN_MEDIA_SEND_QUEUE_DEADLINE_MS),
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

    let repair_payloads = repair_cache.repair_payloads_for_feedback(&feedback);
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

fn open_media_rudp_transport(options: &Options) -> Result<CultNetRudpSocketTransportConnection> {
    let media_profile = muninn_rudp_media_profile();
    let endpoint: SocketAddr = format!("{}:{}", options.target_host, options.port)
        .parse()
        .with_context(|| {
            format!(
                "parsing RUDP media endpoint {}:{}",
                options.target_host, options.port
            )
        })?;
    let socket = UdpSocket::bind("0.0.0.0:0").context("binding Muninn media RUDP client socket")?;
    socket
        .set_nonblocking(true)
        .context("setting Muninn media RUDP client nonblocking")?;
    let mut transport = CultNetRudpSocketTransportConnection::new(muninn_media_rudp_options(
        socket,
        endpoint,
        &media_profile,
    ))?;
    transport.connect(options.stream_id.as_bytes().to_vec())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !transport.connected() {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out connecting Muninn media RUDP stream to {endpoint}"
            ));
        }
        thread::sleep(Duration::from_millis(2));
    }
    Ok(transport)
}

fn muninn_media_rudp_options(
    socket: UdpSocket,
    endpoint: SocketAddr,
    media_profile: &MuninnRudpMediaProfile,
) -> CultNetRudpSocketTransportOptions {
    let mut options = CultNetRudpSocketTransportOptions::client(
        "muninn-media",
        socket,
        endpoint,
        MUNINN_MEDIA_RUDP_CONNECTION_ID,
    );
    options.resend_delay_ms = media_profile.sender_resend_delay_ms;
    options.max_fragment_bytes = Some(media_profile.max_fragment_bytes as u32);
    options.media_reliable_expire_after_ms = None;
    options
}

fn video_rudp_payload_reader<R>(
    tx: mpsc::Sender<Result<QueuedMuninnMediaSendPayload>>,
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
    tx: mpsc::Sender<Result<QueuedMuninnMediaSendPayload>>,
    mut reader: R,
    config: AudioAdtsStreamSendConfig,
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
    tx: &mpsc::Sender<Result<QueuedMuninnMediaSendPayload>>,
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
            for payload in sender.finish(&timestamp()?)? {
                queue_muninn_media_payload(tx, payload)
                    .context("queueing final typed video media payload")?;
            }
            return Ok(());
        }
        for payload in sender.push(&timestamp()?, &buffer[..read])? {
            queue_muninn_media_payload(tx, payload)
                .context("queueing typed video media payload")?;
        }
    }
}

fn read_audio_rudp_payloads<R>(
    tx: &mpsc::Sender<Result<QueuedMuninnMediaSendPayload>>,
    reader: &mut R,
    config: AudioAdtsStreamSendConfig,
) -> Result<()>
where
    R: Read,
{
    let mut sender = AudioAdtsStreamSendState::new(config)?;
    let mut buffer = vec![0_u8; 16 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .context("reading encoded ADTS audio from ffmpeg stdout")?;
        if read == 0 {
            for payload in sender.finish(&timestamp()?)? {
                queue_muninn_media_payload(tx, payload)
                    .context("queueing final typed audio media payload")?;
            }
            return Ok(());
        }
        for payload in sender.push(&timestamp()?, &buffer[..read])? {
            queue_muninn_media_payload(tx, payload)
                .context("queueing typed audio media payload")?;
        }
    }
}

fn video_frame_duration_ticks(options: &Options) -> Result<u32> {
    if options.framerate == 0 {
        return Err(anyhow!("framerate must be greater than zero"));
    }
    Ok((90_000_u32 / options.framerate).max(1))
}

fn publish_surface(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    state: &str,
    active_streams: &[String],
) -> Result<()> {
    publish_obs_catalog_idle(node, options)?;
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
    };
    node.put("latest", &record)?;
    node.put(&record.surface_id, &record)?;
    Ok(())
}

fn publish_runtime_boundary_records(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    state: &str,
    active_streams: &[String],
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
    let activation_command = format!(
        "muninn request-stream --store {} --activate-store {} --host {} --stream {}",
        store_path,
        activation_store_path.display(),
        options.host_id,
        options.stream_id
    );
    let health_command = format!(
        "muninn --health --store {} --host {}",
        options.store_path.display(),
        options.host_id
    );
    let current_transport = if options.idunn_rudp_health.is_some() {
        "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store + compatibility.local-cli fallback"
    } else {
        "daemon-owned-cultcache-telemetry-store + compatibility.local-cli fallback"
    };
    let transport_state = if options.idunn_rudp_health.is_some() {
        "rudp-health-and-provider-store-live"
    } else {
        "cultcache-provider-store-only"
    };
    let compatibility_mechanisms = if options.idunn_rudp_health.is_some() {
        vec![health_command.clone(), activation_command.clone()]
    } else {
        vec![activation_command.clone()]
    };
    let media_profile = muninn_rudp_media_profile();
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
            "lifecycle_authority": "idunn.local-command",
            "health_publication": options.idunn_rudp_health.as_ref().map(|idunn| json!({
                "contract": idunn.health_contract,
                "transport": CULTNET_RUDP_PROTOCOL_ID,
                "publication_source": "daemon-published",
                "endpoint": idunn.endpoint.to_string(),
                "state_owner": "Muninn serve process"
            })).unwrap_or_else(|| json!({
                "contract": serde_json::Value::Null,
                "transport": "unconfigured",
                "publication_source": "compatibility-command-only",
                "state_owner": "Muninn local store"
            })),
            "commands": [
                {
                    "command": "muninn.capture_stream_command",
                    "ingress": "cultmesh-document",
                    "schema": "muninn.capture_stream_command.v1",
                    "invocation": activation_command,
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
                "sender_reliable_expire_after_ms": media_profile.sender_reliable_expire_after_ms,
                "receiver_assembly_deadline_ms": media_profile.receiver_assembly_deadline_ms,
                "receiver_gap_wait_ms": media_profile.receiver_gap_wait_ms,
                "late_media_policy": "drop expired queued media; do not repair frames outside the latency budget",
                "recovery": "fixed quarter-second IDR budget with receiver feedback pressure telemetry"
            },
            "compatibility_mechanisms": compatibility_mechanisms,
            "cut_line": "Muninn's telemetry store owns provider advertisement, command boundary, transport profile, telemetry surface, and daemon health state. Local CLI activation and health commands are compatibility/ops witnesses only.",
            "updated_at": updated_at,
        }),
    };
    let provider_advertisement = EveProviderAdvertisementCompatRecord {
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
            "endpoints": [
                {
                    "transport": "cultcache-store",
                    "address": options.store_path.display().to_string()
                }
            ],
            "routes": [
                {
                    "transport": "cultcache-store",
                    "address": options.store_path.display().to_string()
                },
                {
                    "transport": "compatibility-local-cli",
                    "address": activation_command
                }
            ],
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
    publish_obs_catalog_active(node, options, plan, state)?;
    let record = MuninnCaptureStreamRecord {
        stream_id: options.stream_id.clone(),
        host_id: options.host_id.clone(),
        state: state.to_string(),
        video_source: format!(
            "ddagrab:output_idx={}:{}x{}@{}",
            options.ddagrab_output_index, options.width, options.height, options.framerate
        ),
        audio_source: format!(
            "wasapi-loopback:{}:{}ch@{}",
            options.audio_device, options.audio_channels, options.audio_sample_rate
        ),
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

fn publish_obs_catalog_active(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    plan: &MuxPlan,
    state: &str,
) -> Result<()> {
    let mut stream_ids = Vec::new();
    let mut labels = Vec::new();
    let mut urls = Vec::new();
    let mut states = Vec::new();
    for (index, target) in plan.targets.iter().enumerate() {
        stream_ids.push(if plan.targets.len() == 1 {
            options.stream_id.clone()
        } else {
            format!("{}:{}", options.stream_id, index)
        });
        labels.push(format!("{} A/V target {}", options.host_id, index + 1));
        urls.push(target.clone());
        states.push(state.to_string());
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
    let record = MuninnObsStreamCatalogRecord {
        catalog_id: "muninn.obs.streams".to_string(),
        host_id: options.host_id.clone(),
        stream_ids,
        labels,
        urls,
        states,
        updated_at: timestamp()?,
        command_rudp_target: options
            .capture_command_rudp_bind
            .map(|addr| addr.to_string())
            .unwrap_or_default(),
        media_target_host: options.target_host.clone(),
        media_port: options.port,
        media_packet_bytes: options.media_packet_bytes as u32,
    };
    node.put("obs", &record)?;
    if let Some(target) = options.obs_catalog_rudp_target {
        if let Err(error) = publish_obs_catalog_rudp(target, &record) {
            eprintln!("Muninn could not publish OBS stream catalog to {target}: {error:#}");
        }
    }
    Ok(())
}

fn publish_obs_catalog_rudp(
    target: SocketAddr,
    record: &MuninnObsStreamCatalogRecord,
) -> Result<()> {
    publish_rudp_schema_payload(
        target,
        MUNINN_OBS_CATALOG_RUDP_CONNECTION_ID,
        "muninn-obs-catalog",
        "muninn.obs_stream_catalog",
        rmp_serde::to_vec(record).context("encoding Muninn OBS stream catalog")?,
    )
}

fn create_move_evidence_stream(options: &Options) -> Result<Option<ActiveMoveEvidenceStream>> {
    if options.move_state_sources.is_empty() {
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
    }))
}

fn publish_move_evidence_stream_frame(
    stream: &mut ActiveMoveEvidenceStream,
    controller_states: &[MuninnMoveControllerStateRecord],
) -> Result<Option<cultmesh_rs::CultMeshStreamFrameHandle>> {
    if controller_states.is_empty() {
        return Ok(None);
    }

    let published_at_ns = timestamp_ns()?;
    let frame_id = format!("{}:{}", stream.stream_id, stream.frame_counter);
    stream.frame_counter = stream.frame_counter.saturating_add(1);
    let marker_candidates: &[MuninnMoveMarkerCandidateWire] = &[];
    let frame = MuninnMoveEvidenceStreamFrame(
        &frame_id,
        &stream.producer_peer_id,
        published_at_ns,
        marker_candidates,
        controller_states,
    );
    let payload =
        rmp_serde::to_vec(&frame).context("encoding Muninn Move evidence stream frame")?;
    let handle = {
        let ring: &mut CultMeshSharedMemoryFrameRing =
            stream
                .catalog
                .ring_mut(&stream.stream_id)
                .ok_or_else(|| anyhow!("missing Muninn Move evidence ring"))?;
        ring.try_publish_copy(&payload, published_at_ns, 0)?
    };
    if let Some(handle) = handle {
        stream.catalog.publish_frame(handle.clone())?;
        Ok(Some(handle))
    } else {
        Ok(None)
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
            .write(true)
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
            build_move_controller_state_record_from_joystick(
                options,
                &state.source,
                state.sequence,
                state.joystick_axes,
                state.joystick_buttons,
                timestamp_ns()?,
                timestamp()?,
            )
        } else {
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
            build_move_controller_state_record(
                options,
                &state.source,
                state.sequence,
                &report,
                timestamp_ns()?,
                timestamp()?,
            )
        };
        node.put(&record.stream_id, &record)?;
        node.put(
            &format!("{}:{}", record.stream_id, record.sequence),
            &record,
        )?;
        published_records.push(record);
    }
    if let Some(stream) = move_evidence_stream {
        publish_move_evidence_stream_frame(stream, &published_records)?;
    }
    Ok(())
}

fn is_joystick_path(path: &str) -> bool {
    path.contains("/dev/input/js") || path.contains("-joystick")
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

const DEFAULT_MOVE_LIGHT_COLORS: &[(u8, u8, u8)] = &[
    (255, 48, 64),
    (32, 160, 255),
    (80, 255, 120),
    (255, 208, 48),
    (200, 80, 255),
    (255, 128, 32),
];

fn tick_default_move_light_pulse(
    states: &mut [ActiveMoveStateSource],
    active_commands: &[ActiveMoveLightCommand],
    last_write_at: &mut Option<Instant>,
    include_platform_defaults: bool,
    writer: &mut impl MoveLightWriter,
) {
    let now = Instant::now();
    if last_write_at.is_some_and(|last| now.duration_since(last) < Duration::from_millis(200)) {
        return;
    }

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let paths = default_move_light_paths(states, include_platform_defaults);

    for target in paths.iter() {
        if active_commands
            .iter()
            .any(|command| command.command.hidraw_path == target.path)
        {
            continue;
        }

        let color = default_move_color_for_identity(&target.identity);
        let report = default_move_light_report(color, seconds);
        let _ = writer.write_report(&target.path, &report);
    }
    *last_write_at = Some(now);
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
    let mut hash = 0xcbf29ce484222325u64;
    for byte in identity.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    DEFAULT_MOVE_LIGHT_COLORS[hash as usize % DEFAULT_MOVE_LIGHT_COLORS.len()]
}

fn default_move_light_report(color: (u8, u8, u8), seconds: f64) -> [u8; PS_MOVE_LED_REPORT_LEN] {
    let intensity = seconds.sin().abs() * 0.5 + 0.5;
    move_light_report(
        scale_color_channel(color.0, intensity),
        scale_color_channel(color.1, intensity),
        scale_color_channel(color.2, intensity),
    )
}

fn move_light_report(red: u8, green: u8, blue: u8) -> [u8; PS_MOVE_LED_REPORT_LEN] {
    let mut report = [0u8; PS_MOVE_LED_REPORT_LEN];
    report[0] = 0x06;
    report[2] = red;
    report[3] = green;
    report[4] = blue;
    report
}

fn scale_color_channel(channel: u8, intensity: f64) -> u8 {
    ((f64::from(channel) * intensity).round()).clamp(0.0, 255.0) as u8
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
        if !is_ps_move_uevent(&uevent) {
            continue;
        }

        let hidraw_path = joystick_light_hidraw_path(&joystick_path);
        let move_id = hidraw_path
            .as_deref()
            .and_then(controller_id_from_hidraw)
            .map(|id| format!("move-{id}"))
            .or_else(|| move_unique_id_from_uevent(&uevent))
            .unwrap_or_else(|| format!("move-{name}"));
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
    let Ok(targets) = windows_ps_move_state_paths() else {
        return Vec::new();
    };
    targets
        .into_iter()
        .enumerate()
        .map(|(index, target)| {
            let move_id = if target.identity.starts_with("move-") {
                target.identity
            } else {
                format!("move-windows-psmove-{index}")
            };
            MoveStateSource {
                move_id,
                hidraw_path: windows_ps_move_source_token(&target.path),
            }
        })
        .collect()
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
fn is_ps_move_uevent(uevent: &str) -> bool {
    uevent.contains("ID_VENDOR_ID=054c") && uevent.contains("ID_MODEL_ID=03d5")
        || uevent.contains("ID_MODEL=Motion_Controller")
        || uevent.contains("HID_ID=0005:0000054C:000003D5")
        || uevent.contains("HID_ID=0003:0000054C:000003D5")
}

#[cfg(unix)]
fn move_unique_id_from_uevent(uevent: &str) -> Option<String> {
    value_from_uevent(uevent, "HID_UNIQ")
        .map(|value| value.replace(':', ""))
        .filter(|value| !value.is_empty())
        .map(|value| format!("move-{value}"))
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
                && attributes.ProductID == 0x03d5
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

    let mut report = vec![0u8; caps.InputReportByteLength as usize];
    report[0] = 0x01;
    let ok =
        unsafe { HidD_GetInputReport(handle, report.as_mut_ptr().cast(), report.len() as u32) };
    if ok != 0 {
        unsafe { CloseHandle(handle) };
        return Ok(Some(report));
    }

    let interrupt_report = windows_read_hid_interrupt_report(handle, report.len(), &path)?;
    unsafe { CloseHandle(handle) };
    Ok(interrupt_report)
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

    let wait = unsafe { WaitForSingleObject(event, 25) };
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

fn publish_daemon_health_if_configured(
    options: &Options,
    last_attempt_at: &mut Option<Instant>,
) -> Result<()> {
    let Some(idunn) = options.idunn_rudp_health.as_ref() else {
        return Ok(());
    };

    let cadence = Duration::from_secs(options.interval_seconds.unwrap_or(15).max(1));
    if last_attempt_at
        .as_ref()
        .is_some_and(|instant| instant.elapsed() < cadence)
    {
        return Ok(());
    }

    let observed_at = idunn_timestamp()?;
    let health = evaluate_health(options);
    let (state, detail) = match health {
        Ok(detail) => ("active", detail),
        Err(error) => ("failed", error.to_string()),
    };
    *last_attempt_at = Some(Instant::now());
    if let Err(error) = publish_idunn_rudp_health(idunn, state, &detail, &observed_at) {
        eprintln!(
            "Muninn could not publish Idunn RUDP health for {} at {}: {error:#}",
            options.host_id, observed_at
        );
    }
    Ok(())
}

fn publish_idunn_rudp_health(
    options: &IdunnRudpHealthOptions,
    state: &str,
    detail: &str,
    observed_at: &str,
) -> Result<()> {
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
    let bind_address = if options.endpoint.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Muninn RUDP sender at {bind_address}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
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
    let payload = encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)
        .context("encoding Idunn health CultNet message")?;
    transport
        .send("schema", payload)
        .with_context(|| format!("sending Idunn health to {}", options.endpoint))?;
    Ok(())
}

fn publish_rudp_schema_payload(
    target: SocketAddr,
    connection_id: u32,
    peer_id: &str,
    context: &str,
    payload: Vec<u8>,
) -> Result<()> {
    let bind_address = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Muninn RUDP sender at {bind_address}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut transport = CultNetRudpSocketTransportConnection::new(
        CultNetRudpSocketTransportOptions::client(peer_id, socket, target, connection_id),
    )?;
    transport.connect(Vec::new())?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !transport.connected() {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out connecting Muninn RUDP sender to {target}"
            ));
        }
    }
    transport
        .send("schema", payload)
        .with_context(|| format!("sending {context} to {target}"))?;
    let ack_deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < ack_deadline {
        match transport.receive_once() {
            Ok(_) => {}
            Err(error) if is_would_block_error(&error) => {}
            Err(error) => return Err(error).with_context(|| format!("receiving {context} ACK")),
        }
        transport
            .poll_resends()
            .with_context(|| format!("polling {context} RUDP ACK"))?;
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn verify_move_sources_fresh(options: &Options, node: &cultmesh_rs::CultMeshNode) -> Result<()> {
    let move_state_sources = live_move_state_sources(options);
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
    let command = build_capture_stream_command(&options)?;
    if let Some(target) = options.capture_command_rudp_target {
        publish_capture_command_rudp(target, &command)?;
        println!(
            "Published Muninn capture stream command {} {} {} for {} over RUDP to {}.",
            command.command_id, command.action, command.stream_id, command.host_id, target
        );
        return Ok(());
    }

    let mut command_options = options.clone();
    if let Some(activation_store_path) = options.activation_store_path.as_ref() {
        command_options.store_path = activation_store_path.clone();
    }
    ensure_state_dirs(&command_options)?;
    let mut node = open_node(&command_options, "muninn-capture-stream-request")?;
    node.put(&command.command_id, &command)?;
    println!(
        "Published Muninn capture stream command {} {} {} for {}.",
        command.command_id, command.action, command.stream_id, command.host_id
    );
    Ok(())
}

fn publish_capture_command_rudp(
    target: SocketAddr,
    command: &MuninnCaptureStreamCommandRecord,
) -> Result<()> {
    let message = CultNetMessage::DocumentPutRaw {
        message_id: format!(
            "muninn-capture-command:{}:{}",
            command.command_id,
            command.updated_at.replace(':', "-")
        ),
        document: CultNetRawDocumentRecord {
            schema_id: "muninn.capture_stream_command".to_string(),
            record_key: command.command_id.clone(),
            stored_at: command.updated_at.clone(),
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload: rmp_serde::to_vec(command).context("encoding Muninn capture command")?,
            source_runtime_id: Some("muninn-request-stream".to_string()),
            source_agent_id: None,
            source_role: Some("capture-command-publisher".to_string()),
            tags: Some(vec![CULTNET_RUDP_PROTOCOL_ID.to_string()]),
        },
    };
    let bind_address = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding Muninn capture command RUDP sender at {bind_address}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut transport =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::client(
            "muninn-capture-command-request",
            socket,
            target,
            MUNINN_COMMAND_RUDP_CONNECTION_ID,
        ))?;
    transport.connect(Vec::new())?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !transport.connected() {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "timed out connecting Muninn capture command RUDP sender to {target}"
            ));
        }
    }
    let payload = encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)
        .context("encoding Muninn capture command CultNet message")?;
    transport
        .send("schema", payload)
        .with_context(|| format!("sending Muninn capture command to {target}"))?;
    Ok(())
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

fn build_targets(options: &Options) -> Vec<String> {
    match options.media_transport {
        MediaTransport::Srt => {
            let mut targets = vec![srt_endpoint(&options.target_host, options.port)];
            if let Some(host) = &options.obs_target_host {
                targets.push(srt_endpoint(host, options.obs_port));
            }
            targets
        }
        MediaTransport::Rudp => {
            vec![rudp_endpoint(
                &options.target_host,
                options.port,
                &options.stream_id,
            )]
        }
    }
}

fn srt_endpoint(host: &str, port: u16) -> String {
    format!("srt://{host}:{port}?mode=caller&latency=120000&timeout=30000000")
}

fn rudp_endpoint(host: &str, port: u16, stream_id: &str) -> String {
    let profile = muninn_rudp_media_profile();
    format!(
        "rudp://{host}:{port}/{stream_id}?channel=media&format=muninn-typed-media&connection=0x{MUNINN_MEDIA_RUDP_CONNECTION_ID:08x}&profile={}&delivery=unreliable&sender_resend_delay_ms={}&assembly_deadline_ms={}&gap_wait_ms={}",
        profile.profile_id,
        profile.sender_resend_delay_ms,
        profile.receiver_assembly_deadline_ms,
        profile.receiver_gap_wait_ms
    )
}

fn muninn_rudp_media_profile() -> MuninnRudpMediaProfile {
    MuninnRudpMediaProfile {
        profile_id: MUNINN_RUDP_MEDIA_PROFILE_ID,
        video_codec: "h264",
        video_encoder: "h264_nvenc",
        video_preset: "p5",
        video_tune: "ull",
        video_bitrate_kbps: MUNINN_RUDP_MEDIA_VIDEO_BITRATE_KBPS,
        media_packet_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES,
        max_fragment_bytes: MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES,
        video_b_frames: 0,
        video_rc_lookahead: 0,
        sender_queue_deadline_ms: MUNINN_MEDIA_SEND_QUEUE_DEADLINE_MS,
        sender_resend_delay_ms: MUNINN_RUDP_MEDIA_RESEND_DELAY_MS,
        sender_reliable_expire_after_ms: MUNINN_RUDP_MEDIA_RELIABLE_EXPIRE_AFTER_MS,
        receiver_assembly_deadline_ms: MUNINN_RUDP_MEDIA_RECEIVER_ASSEMBLY_DEADLINE_MS,
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
        options.audio_device.clone(),
    ]
}

fn rudp_video_ffmpeg_args(options: &Options) -> Vec<String> {
    let profile = muninn_rudp_media_profile();
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
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-ar".to_string(),
        options.audio_sample_rate.to_string(),
        "-ac".to_string(),
        options.audio_channels.to_string(),
        "-f".to_string(),
        "adts".to_string(),
        "pipe:1".to_string(),
    ]
}

fn ffmpeg_args(options: &Options) -> Vec<String> {
    if options.media_transport == MediaTransport::Rudp {
        return rudp_video_ffmpeg_args(options);
    }

    let mut args = vec![
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
        "-map".to_string(),
        "0:v:0".to_string(),
        "-map".to_string(),
        "1:a:0".to_string(),
        "-c:v".to_string(),
        "h264_nvenc".to_string(),
        "-preset".to_string(),
        "p1".to_string(),
        "-tune".to_string(),
        "ull".to_string(),
        "-zerolatency".to_string(),
        "1".to_string(),
        "-bf".to_string(),
        "0".to_string(),
        "-delay".to_string(),
        "0".to_string(),
        "-b:v".to_string(),
        "12000k".to_string(),
        "-maxrate".to_string(),
        "12000k".to_string(),
        "-bufsize".to_string(),
        "6000k".to_string(),
        "-g".to_string(),
        "30".to_string(),
        "-forced-idr".to_string(),
        "1".to_string(),
        "-fps_mode".to_string(),
        "cfr".to_string(),
        "-r".to_string(),
        options.framerate.max(1).to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-ar".to_string(),
        options.audio_sample_rate.to_string(),
        "-ac".to_string(),
        options.audio_channels.to_string(),
    ];

    match options.media_transport {
        MediaTransport::Srt => {
            let tee_targets = build_targets(options)
                .iter()
                .map(|target| format!("[f=mpegts]{target}"))
                .collect::<Vec<_>>()
                .join("|");
            args.extend(["-f".to_string(), "tee".to_string(), tee_targets]);
        }
        MediaTransport::Rudp => {
            args.extend([
                "-flush_packets".to_string(),
                "1".to_string(),
                "-muxdelay".to_string(),
                "0".to_string(),
                "-muxpreload".to_string(),
                "0".to_string(),
                "-mpegts_flags".to_string(),
                "resend_headers".to_string(),
                "-f".to_string(),
                "mpegts".to_string(),
                "pipe:1".to_string(),
            ]);
        }
    }

    args
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
            activation_store_path: None,
            surface_id: "muninn.telemetry.local".to_string(),
            stream_id: "muninn.raven.av.srt".to_string(),
            stream_action: "start".to_string(),
            host_id: "raven".to_string(),
            target_host: "10.77.0.2".to_string(),
            port: 5200,
            obs_target_host: Some("10.77.0.2".to_string()),
            obs_port: 5204,
            media_transport: MediaTransport::Srt,
            media_packet_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES,
            width: 1920,
            height: 1080,
            framerate: 30,
            ddagrab_output_index: 0,
            audio_device: "Realtek".to_string(),
            audio_sample_rate: 48000,
            audio_channels: 2,
            ffmpeg_path: "ffmpeg".to_string(),
            loopback_script: PathBuf::from("scripts/wasapi-loopback-capture.ps1"),
            log_root: PathBuf::from("C:/Meta/Odin/logs/muninn"),
            interval_seconds: None,
            move_id: "move-usb".to_string(),
            move_filter: None,
            hidraw_path: String::new(),
            move_colors: Vec::new(),
            move_durations_ms: Vec::new(),
            move_repeat_count: 1,
            command_id: None,
            move_host_address: None,
            move_state_sources: Vec::new(),
            move_evidence_stream_id: None,
            move_evidence_verse_id: "mimir-live".to_string(),
            move_evidence_ring_slots: 4,
            move_evidence_slot_bytes: 8192,
            quest_adb: false,
            quest_serial: None,
            quest_input_stream_id: None,
            quest_pose_stream_id: None,
            quest_video_input_stream_id: None,
            idunn_rudp_health: None,
            capture_command_rudp_bind: None,
            capture_command_rudp_target: None,
            obs_catalog_rudp_target: None,
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
                "move-light-status" => options.mode = Mode::MoveLightStatus,
                "move-identity-status" => options.mode = Mode::MoveIdentityStatus,
                "move-source-status" => options.mode = Mode::MoveSourceStatus,
                "move-state-status" => options.mode = Mode::MoveStateStatus,
                "claim-move-host" => options.mode = Mode::ClaimMoveHost,
                "quest-access-status" => options.mode = Mode::QuestAccessStatus,
                "--health" => options.mode = Mode::Health,
                "--dry-run" => options.mode = Mode::DryRun,
                "--store" => options.store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--activate-store" => {
                    options.activation_store_path =
                        Some(PathBuf::from(take_value(&mut args, "--activate-store")?))
                }
                "--surface" => options.surface_id = take_value(&mut args, "--surface")?,
                "--stream" => options.stream_id = take_value(&mut args, "--stream")?,
                "--stream-action" => {
                    options.stream_action = take_value(&mut args, "--stream-action")?
                }
                "--host" => options.host_id = take_value(&mut args, "--host")?,
                "--target-host" => options.target_host = take_value(&mut args, "--target-host")?,
                "--port" => options.port = take_value(&mut args, "--port")?.parse()?,
                "--obs-target-host" => {
                    options.obs_target_host = Some(take_value(&mut args, "--obs-target-host")?)
                }
                "--no-obs-target" => options.obs_target_host = None,
                "--obs-port" => options.obs_port = take_value(&mut args, "--obs-port")?.parse()?,
                "--media-transport" => {
                    options.media_transport =
                        parse_media_transport(&take_value(&mut args, "--media-transport")?)?
                }
                "--media-packet-bytes" => {
                    options.media_packet_bytes =
                        take_value(&mut args, "--media-packet-bytes")?.parse()?
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
                "--audio-device" => options.audio_device = take_value(&mut args, "--audio-device")?,
                "--audio-sample-rate" => {
                    options.audio_sample_rate =
                        take_value(&mut args, "--audio-sample-rate")?.parse()?
                }
                "--audio-channels" => {
                    options.audio_channels = take_value(&mut args, "--audio-channels")?.parse()?
                }
                "--ffmpeg" => options.ffmpeg_path = take_value(&mut args, "--ffmpeg")?,
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
                "--capture-command-rudp-bind" => {
                    options.capture_command_rudp_bind = Some(
                        take_value(&mut args, "--capture-command-rudp-bind")?
                            .parse()
                            .context("--capture-command-rudp-bind must be a socket address")?,
                    )
                }
                "--capture-command-rudp-target" => {
                    options.capture_command_rudp_target = Some(
                        take_value(&mut args, "--capture-command-rudp-target")?
                            .parse()
                            .context("--capture-command-rudp-target must be a socket address")?,
                    )
                }
                "--obs-catalog-rudp-target" => {
                    options.obs_catalog_rudp_target = Some(
                        take_value(&mut args, "--obs-catalog-rudp-target")?
                            .parse()
                            .context("--obs-catalog-rudp-target must be a socket address")?,
                    )
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
        MediaTransport::Srt => "srt",
        MediaTransport::Rudp => CULTNET_RUDP_PROTOCOL_ID,
    }
}

fn media_transport_cli(transport: &MediaTransport) -> &'static str {
    match transport {
        MediaTransport::Srt => "srt",
        MediaTransport::Rudp => "rudp",
    }
}

fn parse_media_transport(value: &str) -> Result<MediaTransport> {
    match value.to_ascii_lowercase().as_str() {
        "srt" => Ok(MediaTransport::Srt),
        "rudp" | "cultnet-rudp" | "cultmesh-rudp" | CULTNET_RUDP_PROTOCOL_ID => {
            Ok(MediaTransport::Rudp)
        }
        _ => Err(anyhow!(
            "--media-transport must be one of: srt, rudp, cultnet-rudp"
        )),
    }
}

fn help_text() -> &'static str {
    "Usage: muninn [serve|activate|request-stream|request-move-light|move-light-status|move-identity-status|move-source-status|move-state-status|claim-move-host|quest-access-status] [--store <path>] [--activate-store <path>] [--stream-action <start|stop>] [--target-host <host>] [--port <port>] [--obs-target-host <host>] [--obs-port <port>] [--media-transport <srt|rudp>] [--media-packet-bytes <bytes>] [--loopback-script <path>] [--ffmpeg <path>] [--capture-command-rudp-bind <addr>] [--capture-command-rudp-target <addr>] [--obs-catalog-rudp-target <addr>] [--move-state <move-id>=<hidraw-path>] [--move-host <bt-addr>] [--move-evidence-stream <stream-id>] [--move-evidence-verse <verse-id>] [--quest-adb] [--quest-serial <serial>] [--quest-input-stream <stream-id>] [--quest-pose-stream <stream-id>] [--quest-video-input-stream <stream-id>] [--idunn-rudp-health <addr>] [--idunn-daemon <id>] [--idunn-health-contract <contract>] [--dry-run] [--health]\n\nMuninn is Odin's portable telemetry Verse assembler. serve publishes cheap typed telemetry affordances, optional Quest USB access surfaces, and the explicitly configured Move runtime; when serve receives --move-state, --move-host, or --move-evidence-stream it may publish source-local Move controller state, typed Move identity records, a CultMesh Move evidence stream, and keep USB-attached PS Moves claimed to that explicit Bluetooth host; serve also consumes typed capture stream commands from --activate-store or --capture-command-rudp-bind and owns the local ffmpeg/loopback activation child lifecycle, and may project the OBS stream catalog to a local OBS plugin over --obs-catalog-rudp-target; activate starts an explicitly requested local stream over SRT or CultNet RUDP as a daemon child; request-stream publishes a typed capture stream command for Muninn serve to execute, either into the activation store or over --capture-command-rudp-target; request-move-light publishes a typed Move light command for Muninn serve to execute; move-light-status reads typed command receipts; move-identity-status reads typed Move identity records; move-source-status prints live Move source discovery; move-state-status reads typed controller-state records; claim-move-host assigns USB-attached PS Moves to a Bluetooth host; quest-access-status reads typed Quest access state. In --health mode, the Idunn RUDP flags publish typed daemon health to Idunn while preserving command-probe exit semantics."
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
        assert!(options.activation_store_path.is_none());
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
    fn obs_catalog_rudp_publisher_sends_typed_catalog_payload() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut transport = CultNetRudpSocketTransportConnection::new(
                CultNetRudpSocketTransportOptions::server(
                    "muninn-obs-catalog-test",
                    receiver,
                    MUNINN_OBS_CATALOG_RUDP_CONNECTION_ID,
                ),
            )
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if let Some(frame) = transport.receive_once().unwrap() {
                    return frame.payload;
                }
                transport.poll_resends().unwrap();
                if Instant::now() >= deadline {
                    panic!("timed out waiting for Muninn OBS catalog frame");
                }
            }
        });
        let catalog = MuninnObsStreamCatalogRecord {
            catalog_id: "muninn.obs.streams".to_string(),
            host_id: "raven".to_string(),
            stream_ids: vec!["muninn.raven.av.rudp".to_string()],
            labels: vec!["raven screen and loopback A/V".to_string()],
            urls: vec![rudp_endpoint("192.168.1.66", 5204, "muninn.raven.av.rudp")],
            states: vec!["activation-ready".to_string()],
            updated_at: "unix:100".to_string(),
            command_rudp_target: "192.168.1.84:17873".to_string(),
            media_target_host: "192.168.1.66".to_string(),
            media_port: 5204,
            media_packet_bytes: MUNINN_RUDP_MEDIA_PACKET_BYTES as u32,
        };

        publish_obs_catalog_rudp(receiver_addr, &catalog).unwrap();

        let payload = handle.join().unwrap();
        let decoded: MuninnObsStreamCatalogRecord = rmp_serde::from_slice(&payload).unwrap();
        assert_eq!(decoded.catalog_id, "muninn.obs.streams");
        assert_eq!(decoded.host_id, "raven");
        assert_eq!(decoded.stream_ids, vec!["muninn.raven.av.rudp"]);
        assert_eq!(decoded.states, vec!["activation-ready"]);
        assert_eq!(decoded.command_rudp_target, "192.168.1.84:17873");
        assert_eq!(decoded.media_target_host, "192.168.1.66");
        assert_eq!(decoded.media_port, 5204);
        assert_eq!(
            decoded.media_packet_bytes,
            MUNINN_RUDP_MEDIA_PACKET_BYTES as u32
        );
        assert!(decoded.urls[0].contains("delivery=unreliable"));
        assert!(!decoded.urls[0].contains("reliable_expire_after_ms"));
        assert!(decoded.urls[0].contains("assembly_deadline_ms=400"));
    }

    #[test]
    fn capture_command_rudp_publisher_sends_raw_command_document() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let receiver_addr = receiver.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let mut transport = CultNetRudpSocketTransportConnection::new(
                CultNetRudpSocketTransportOptions::server(
                    "muninn-command-test",
                    receiver,
                    MUNINN_COMMAND_RUDP_CONNECTION_ID,
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
                    panic!("timed out waiting for Muninn capture command frame");
                }
            }
        });
        let options = Options::parse(
            [
                "request-stream",
                "--host",
                "raven",
                "--stream",
                "muninn.raven.av.rudp",
                "--target-host",
                "10.77.0.2",
                "--port",
                "5204",
                "--media-transport",
                "rudp",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        let command = build_capture_stream_command(&options).unwrap();

        publish_capture_command_rudp(receiver_addr, &command).unwrap();

        let message = handle.join().unwrap();
        let CultNetMessage::DocumentPutRaw { document, .. } = message else {
            panic!("expected raw document put");
        };
        assert_eq!(document.schema_id, "muninn.capture_stream_command");
        assert_eq!(document.record_key, command.command_id);
        assert_eq!(
            document.payload_encoding,
            CultNetRawPayloadEncoding::Messagepack
        );
        let decoded: MuninnCaptureStreamCommandRecord =
            rmp_serde::from_slice(&document.payload).unwrap();
        assert_eq!(decoded.host_id, "raven");
        assert_eq!(decoded.stream_id, "muninn.raven.av.rudp");
        assert_eq!(decoded.media_transport, "rudp");
        assert_eq!(decoded.port, 5204);
    }

    #[test]
    fn builds_two_srt_targets_for_explicit_activation() {
        let options = Options::parse(
            [
                "activate",
                "--target-host",
                "10.77.0.2",
                "--port",
                "5200",
                "--obs-target-host",
                "10.77.0.2",
                "--obs-port",
                "5204",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();

        let plan = build_mux_plan(&options, "test".to_string());

        assert_eq!(plan.targets.len(), 2);
        assert!(plan.command_file.to_string_lossy().ends_with(".ps1"));
        assert!(plan.command_line.contains("ddagrab=framerate=30"));
        assert!(plan.command_line.contains(" | ffmpeg "));
        assert!(plan.command_line.contains("srt://10.77.0.2:5200"));
        assert!(plan.command_line.contains("srt://10.77.0.2:5204"));
        assert!(plan.command_line.contains(
            "5200?mode=caller&latency=120000&timeout=30000000|[f=mpegts]srt://10.77.0.2:5204"
        ));
        assert!(
            plan.command_script
                .contains("RedirectStandardInput = $true")
        );
        assert!(
            plan.command_script
                .contains("BaseStream.Write($buffer, 0, $read)")
        );
    }

    #[test]
    fn can_disable_obs_target_for_partial_activation_smoke() {
        let options = Options::parse(
            ["activate", "--no-obs-target"]
                .into_iter()
                .map(String::from),
        )
        .unwrap();
        let plan = build_mux_plan(&options, "test".to_string());

        assert_eq!(
            plan.targets,
            vec!["srt://10.77.0.2:5200?mode=caller&latency=120000&timeout=30000000"]
        );
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
                "10.77.0.2",
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
            vec![
                "rudp://10.77.0.2:5204/muninn.raven.av.rudp?channel=media&format=muninn-typed-media&connection=0x6d750001&profile=muninn.rudp.low_latency_h264_lan.v1&delivery=unreliable&sender_resend_delay_ms=5&assembly_deadline_ms=400&gap_wait_ms=16"
            ]
        );
        assert!(!plan.command_line.contains("tee"));
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
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-preset" && pair[1] == "p5")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-b:v" && pair[1] == "48000k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-maxrate" && pair[1] == "48000k")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-bufsize" && pair[1] == "800k")
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
            "1600k"
        );
        assert_eq!(
            muninn_rudp_video_vbv_buffer_arg(&sixty_fps, &profile),
            "800k"
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
    fn rudp_media_transport_options_follow_low_latency_profile() {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let endpoint: SocketAddr = "127.0.0.1:5204".parse().unwrap();
        let profile = muninn_rudp_media_profile();

        let options = muninn_media_rudp_options(socket, endpoint, &profile);

        assert_eq!(options.runtime_id, "muninn-media");
        assert_eq!(options.remote_addr, Some(endpoint));
        assert_eq!(options.connection_id, MUNINN_MEDIA_RUDP_CONNECTION_ID);
        assert_eq!(options.resend_delay_ms, MUNINN_RUDP_MEDIA_RESEND_DELAY_MS);
        assert_eq!(
            options.max_fragment_bytes,
            Some(MUNINN_RUDP_MEDIA_MAX_FRAGMENT_BYTES as u32)
        );
        assert_eq!(options.media_reliable_expire_after_ms, None);
        let transport = CultNetRudpSocketTransportConnection::new(options).unwrap();
        let media_channel = transport
            .profile
            .transports
            .first()
            .unwrap()
            .channels
            .iter()
            .find(|channel| channel.channel_id == "media")
            .unwrap();
        assert_eq!(
            media_channel.delivery,
            cultnet_rs::CultNetTransportDelivery::Unreliable
        );
        assert_eq!(media_channel.reliable_expire_after_ms, None);
    }

    #[test]
    fn rudp_audio_encoder_outputs_adts_for_packetizer() {
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
                .any(|pair| pair[0] == "-f" && pair[1] == "adts")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-c:a" && pair[1] == "aac")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair[0] == "-vn" && pair[1] == "-c:a")
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
    fn rudp_media_progress_detail_reports_queue_and_transport_pressure() {
        let receiver_feedback = MuninnRudpReceiverFeedbackStats {
            feedback_records: 2,
            requested_keyframes: 1,
            late_frames: 3,
            missing_video_chunks: 4,
            repaired_video_chunks: 0,
            highest_decodable_frame_id: Some(88),
        };

        assert_eq!(
            rudp_media_progress_detail(120, 3, 9, &receiver_feedback),
            "Muninn RUDP media progress: sent=120 queue_dropped=3 reliable_expired=9 receiver_feedback=2 receiver_keyframes=1 receiver_late_frames=3 receiver_missing_chunks=4 receiver_repaired_chunks=0 receiver_highest_decodable=88"
        );
    }

    #[test]
    fn receiver_feedback_keyframe_requests_are_recorded_without_encoder_restart() {
        let mut handled = 0;
        let mut receiver_feedback = MuninnRudpReceiverFeedbackStats::default();

        record_receiver_keyframe_pressure(&receiver_feedback, &mut handled);
        assert_eq!(handled, 0);

        receiver_feedback.requested_keyframes = 1;
        record_receiver_keyframe_pressure(&receiver_feedback, &mut handled);
        assert_eq!(handled, 1);
        record_receiver_keyframe_pressure(&receiver_feedback, &mut handled);
        assert_eq!(handled, 1);

        receiver_feedback.requested_keyframes = 2;
        record_receiver_keyframe_pressure(&receiver_feedback, &mut handled);
        assert_eq!(handled, 2);
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
                late_frame_ids: vec![42],
                requested_keyframe: true,
                jitter_us: 500,
                decode_queue_us: 2_000,
                observed_at: "unix:1000",
            },
        )
        .unwrap();

        let mut cache = RecentVideoChunkRepairCache::new(16);
        cache.remember(&payload).unwrap();
        let repairs = cache.repair_payloads_for_feedback(&feedback);

        assert_eq!(repairs, vec![payload]);
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
    fn default_move_light_report_pulses_between_half_and_full_brightness() {
        let half = default_move_light_report((100, 80, 60), 0.0);
        assert_eq!(half.len(), PS_MOVE_LED_REPORT_LEN);
        assert_eq!(&half[..5], &[0x06, 0, 50, 40, 30]);
        assert!(half[5..].iter().all(|byte| *byte == 0));

        let full = default_move_light_report((100, 80, 60), std::f64::consts::FRAC_PI_2);
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
    struct DecodedMarkerCandidate;

    struct RecordingMoveStateReader {
        joystick_events: Vec<JoystickEvent>,
        failing_joystick_path: Option<String>,
    }

    impl MoveControllerStateReader for RecordingMoveStateReader {
        fn read_report(&mut self, _hidraw_path: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
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
                "--idunn-rudp-health",
                "10.77.0.2:17870",
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

        publish_runtime_boundary_records(&mut node, &options, "idle", &[]).unwrap();

        let boundary = node
            .get_required::<MuninnCommandBoundaryCompatRecord>("command-boundary:muninn")
            .unwrap();
        let transport = node
            .get_required::<MuninnTransportProfileCompatRecord>("transport-profile:muninn")
            .unwrap();
        let provider = node
            .get_required::<EveProviderAdvertisementCompatRecord>("muninn.telemetry.raven")
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

        let compatibility_mechanisms = transport
            .value
            .get("compatibility_mechanisms")
            .and_then(|value| value.as_array())
            .unwrap();
        assert!(compatibility_mechanisms.iter().any(|value| {
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
                .get("video_bufsize")
                .and_then(|value| value.as_str()),
            Some("1600k")
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
                .get("sender_reliable_expire_after_ms")
                .and_then(|value| value.as_u64()),
            Some(MUNINN_RUDP_MEDIA_RELIABLE_EXPIRE_AFTER_MS)
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
        assert!(routes.iter().any(|route| {
            route
                .get("address")
                .and_then(|value| value.as_str())
                .is_some_and(|entry| entry.contains("C:/Meta/Odin/state/muninn.activate.cc"))
        }));
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
        }];
        let mut reader = RecordingMoveStateReader {
            joystick_events: Vec::new(),
            failing_joystick_path: None,
        };
        let mut node = open_node(&options, "muninn-empty-joystick-test").unwrap();

        publish_move_controller_states(&mut node, &options, &mut active, &mut reader, None)
            .unwrap();

        let record = node
            .get_required::<MuninnMoveControllerStateRecord>(
                "nightwing:move-usb:move-controller-state",
            )
            .unwrap();
        assert_eq!(record.sequence, 1);
        assert_eq!(record.host_id, "nightwing");
        assert_eq!(record.move_id, "move-usb");
        assert_eq!(record.accelerometer_xyz, vec![0.0, 0.0, 0.0]);
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
            joystick_events: Vec::new(),
            failing_joystick_path: Some("/dev/input/js-missing".to_string()),
        };
        let mut node = open_node(&options, "muninn-missing-joystick-test").unwrap();

        publish_move_controller_states(&mut node, &options, &mut active, &mut reader, None)
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

        let handle = publish_move_evidence_stream_frame(&mut stream, &[record.clone()])
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

use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{
    CultMesh, CultMeshNodeOptions, CultMeshSharedMemoryFrameRing, CultMeshStreamBodyTransport,
    CultMeshStreamCatalog, CultMeshStreamClock, CultMeshStreamDescriptor, CultMeshStreamKind,
};
use odin_core::{
    MuninnCaptureStreamRecord, MuninnMoveControllerStateRecord, MuninnMoveLightCommandRecord,
    MuninnObsStreamCatalogRecord, MuninnQuestAccessRecord, MuninnTelemetrySurfaceRecord,
    OdinDocuments,
};
use serde::Serialize;
use std::env;
use std::fs;
#[cfg(not(windows))]
use std::io::Write;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::io::{ErrorKind, Read};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Serve,
    Activate,
    Health,
    DryRun,
    RequestMoveLight,
    MoveLightStatus,
    MoveStateStatus,
    ClaimMoveHost,
    QuestAccessStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Options {
    mode: Mode,
    store_path: PathBuf,
    surface_id: String,
    stream_id: String,
    host_id: String,
    target_host: String,
    port: u16,
    obs_target_host: Option<String>,
    obs_port: u16,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MoveStateSource {
    move_id: String,
    hidraw_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MuxPlan {
    command_line: String,
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
        Command::new("cmd.exe")
            .arg("/d")
            .arg("/c")
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
        Mode::RequestMoveLight => request_move_light(options),
        Mode::MoveLightStatus => move_light_status(options),
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
    let mut active_move_states: Vec<ActiveMoveStateSource> = options
        .move_state_sources
        .iter()
        .cloned()
        .map(|source| ActiveMoveStateSource {
            light_hidraw_path: default_move_light_path(&source.hidraw_path),
            source,
            sequence: 0,
            joystick_axes: [0; 16],
            joystick_buttons: [false; 32],
        })
        .collect();

    loop {
        let mut node = open_node(&options, "muninn-daemon")?;
        register_move_light_commands(&mut node, &options, &mut active_move_lights)?;
        tick_move_light_commands(&mut node, &mut active_move_lights, &mut HidMoveLightWriter)?;
        tick_default_move_light_pulse(
            &mut active_move_states,
            &active_move_lights,
            &mut last_default_move_light_write_at,
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
        publish_surface(&mut node, &options, "idle", &[])?;
        let has_platform_default_move_lights = platform_default_move_lights_enabled();
        if options.interval_seconds.is_none()
            && active_move_lights.is_empty()
            && active_move_states.is_empty()
            && !has_platform_default_move_lights
        {
            return Ok(());
        }
        let sleep = if !active_move_lights.is_empty()
            || !active_move_states.is_empty()
            || has_platform_default_move_lights
        {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(options.interval_seconds.unwrap_or(15))
        };
        thread::sleep(sleep);
    }
}

struct ActiveMoveEvidenceStream {
    catalog: CultMeshStreamCatalog,
    stream_id: String,
    producer_peer_id: String,
    frame_counter: u64,
}

fn activate(options: Options, spawner: impl ProcessSpawner) -> Result<()> {
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
        transport: "srt".to_string(),
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
        stream_ids.push(format!("{}:{}", options.stream_id, index));
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
    };
    node.put("obs", &record)?;
    Ok(())
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
            return windows_ps_move_input_report();
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
            let events = reader.read_joystick_events(&state.source.hidraw_path)?;
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
            let Some(report) = reader.read_report(&state.source.hidraw_path)? else {
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
    path.eq_ignore_ascii_case("windows-psmove") || path.eq_ignore_ascii_case("windows-psmove-col01")
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
        let report = [0x06, 0, red, green, blue, 0, 0, 0, 0];
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
    let paths = default_move_light_paths(states);

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

fn default_move_light_paths(states: &[ActiveMoveStateSource]) -> Vec<DefaultMoveLightTarget> {
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
    for path in platform_default_move_light_paths() {
        push_unique_light_target(&mut paths, path);
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

fn default_move_light_report(color: (u8, u8, u8), seconds: f64) -> [u8; 9] {
    let intensity = seconds.sin().abs() * 0.5 + 0.5;
    [
        0x06,
        0,
        scale_color_channel(color.0, intensity),
        scale_color_channel(color.1, intensity),
        scale_color_channel(color.2, intensity),
        0,
        0,
        0,
        0,
    ]
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
    for _ in 0..4 {
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

#[cfg(not(unix))]
fn joystick_light_hidraw_path(_joystick_path: &str) -> Option<String> {
    None
}

#[cfg(not(windows))]
fn platform_default_move_light_paths() -> Vec<DefaultMoveLightTarget> {
    Vec::new()
}

#[cfg(not(windows))]
fn platform_default_move_lights_enabled() -> bool {
    false
}

#[cfg(windows)]
fn platform_default_move_light_paths() -> Vec<DefaultMoveLightTarget> {
    windows_ps_move_light_paths().unwrap_or_default()
}

#[cfg(windows)]
fn platform_default_move_lights_enabled() -> bool {
    true
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

#[cfg(windows)]
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

#[cfg(windows)]
fn format_bluetooth_address_little_endian(address: &[u8]) -> String {
    address
        .iter()
        .rev()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(windows)]
fn windows_ps_move_bluetooth_addresses(
    handle: *mut std::ffi::c_void,
) -> Option<(String, String)> {
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
        let ok = unsafe { HidD_SetFeature(handle, report.as_mut_ptr().cast(), report.len() as u32) };
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
fn windows_ps_move_input_report() -> Result<Option<Vec<u8>>> {
    use windows_sys::Win32::Devices::HumanInterfaceDevice::{
        HIDP_CAPS, HIDP_STATUS_SUCCESS, HidD_FreePreparsedData, HidD_GetInputReport,
        HidD_GetPreparsedData, HidP_GetCaps,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let Some(target) = windows_ps_move_light_paths()?.into_iter().next() else {
        return Ok(None);
    };
    let path = target.path;
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
            let report = [0x06, 0, red, green, blue, 0, 0, 0, 0];
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
    let output = Command::new("adb")
        .args(["devices", "-l"])
        .output()
        .context("running adb devices -l for Quest access")?;
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
    let node = open_node(options, "muninn-health")?;
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

    println!(
        "Muninn healthy: {} on {} ({})",
        surface.surface_id, surface.host_id, surface.state
    );
    Ok(())
}

fn verify_move_sources_fresh(options: &Options, node: &cultmesh_rs::CultMeshNode) -> Result<()> {
    if options.move_state_sources.is_empty() {
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

    for source in &options.move_state_sources {
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
    if !options.move_id.trim().is_empty() {
        states.retain(|state| state.move_id == options.move_id);
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

fn claim_move_host(options: Options) -> Result<()> {
    let host = options
        .move_host_address
        .as_deref()
        .ok_or_else(|| anyhow!("--move-host is required for claim-move-host"))?;
    claim_ps_move_host(host)
}

#[cfg(not(windows))]
fn claim_ps_move_host(_host_address: &str) -> Result<()> {
    Err(anyhow!(
        "claim-move-host is implemented in Muninn on Windows; use scripts/nightwing-claim-usb-moves.sh on Linux"
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

fn build_mux_plan(options: &Options, timestamp: String) -> MuxPlan {
    let command_file = options.log_root.join(format!("muninn-{timestamp}.cmd"));
    let targets = build_targets(options);

    let loopback = vec![
        "powershell.exe".to_string(),
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        quote_cmd(&options.loopback_script.display().to_string()),
        "-Output".to_string(),
        quote_cmd("stdout"),
        "-SampleRate".to_string(),
        options.audio_sample_rate.to_string(),
        "-Channels".to_string(),
        options.audio_channels.to_string(),
        "-Device".to_string(),
        quote_cmd(&options.audio_device),
    ]
    .join(" ");

    let tee_targets = targets
        .iter()
        .map(|target| format!("[f=mpegts]{target}"))
        .collect::<Vec<_>>()
        .join("|");

    let ffmpeg_args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-thread_queue_size".to_string(),
        "1024".to_string(),
        "-f".to_string(),
        "lavfi".to_string(),
        "-i".to_string(),
        format!(
            "ddagrab=framerate={}:output_idx={}:draw_mouse=1",
            options.framerate, options.ddagrab_output_index
        ),
        "-thread_queue_size".to_string(),
        "1024".to_string(),
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
        "p4".to_string(),
        "-tune".to_string(),
        "ll".to_string(),
        "-b:v".to_string(),
        "12000k".to_string(),
        "-maxrate".to_string(),
        "12000k".to_string(),
        "-bufsize".to_string(),
        "24000k".to_string(),
        "-g".to_string(),
        "60".to_string(),
        "-c:a".to_string(),
        "aac".to_string(),
        "-b:a".to_string(),
        "192k".to_string(),
        "-ar".to_string(),
        options.audio_sample_rate.to_string(),
        "-ac".to_string(),
        options.audio_channels.to_string(),
        "-f".to_string(),
        "tee".to_string(),
        tee_targets,
    ];
    let command_line = format!(
        "{} | {} {}",
        loopback,
        quote_cmd(&options.ffmpeg_path),
        ffmpeg_args
            .into_iter()
            .map(|arg| quote_cmd(&arg))
            .collect::<Vec<_>>()
            .join(" ")
    );

    MuxPlan {
        command_line,
        command_file,
        targets,
    }
}

fn build_targets(options: &Options) -> Vec<String> {
    let mut targets = vec![srt_endpoint(&options.target_host, options.port)];
    if let Some(host) = &options.obs_target_host {
        targets.push(srt_endpoint(host, options.obs_port));
    }
    targets
}

fn srt_endpoint(host: &str, port: u16) -> String {
    format!("srt://{host}:{port}?mode=caller&latency=120000&timeout=30000000")
}

fn write_command_file(plan: &MuxPlan) -> Result<()> {
    fs::write(
        &plan.command_file,
        format!("@echo off\r\n{}\r\n", plan.command_line),
    )
    .with_context(|| format!("writing {}", plan.command_file.display()))
}

fn quote_cmd(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn ensure_state_dirs(options: &Options) -> Result<()> {
    fs::create_dir_all(&options.log_root)
        .with_context(|| format!("creating {}", options.log_root.display()))?;
    if let Some(parent) = options.store_path.parent() {
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
            surface_id: "muninn.telemetry.local".to_string(),
            stream_id: "muninn.raven.av.srt".to_string(),
            host_id: "raven".to_string(),
            target_host: "10.77.0.2".to_string(),
            port: 5200,
            obs_target_host: Some("10.77.0.2".to_string()),
            obs_port: 5204,
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
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => options.mode = Mode::Serve,
                "activate" => options.mode = Mode::Activate,
                "request-move-light" => options.mode = Mode::RequestMoveLight,
                "move-light-status" => options.mode = Mode::MoveLightStatus,
                "move-state-status" => options.mode = Mode::MoveStateStatus,
                "claim-move-host" => options.mode = Mode::ClaimMoveHost,
                "quest-access-status" => options.mode = Mode::QuestAccessStatus,
                "--health" => options.mode = Mode::Health,
                "--dry-run" => options.mode = Mode::DryRun,
                "--store" => options.store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--surface" => options.surface_id = take_value(&mut args, "--surface")?,
                "--stream" => options.stream_id = take_value(&mut args, "--stream")?,
                "--host" => options.host_id = take_value(&mut args, "--host")?,
                "--target-host" => options.target_host = take_value(&mut args, "--target-host")?,
                "--port" => options.port = take_value(&mut args, "--port")?.parse()?,
                "--obs-target-host" => {
                    options.obs_target_host = Some(take_value(&mut args, "--obs-target-host")?)
                }
                "--no-obs-target" => options.obs_target_host = None,
                "--obs-port" => options.obs_port = take_value(&mut args, "--obs-port")?.parse()?,
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
                "--move" => options.move_id = take_value(&mut args, "--move")?,
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
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(anyhow!("{name} must be non-empty"));
            }
        }
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

fn timestamp_ns() -> Result<i64> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_nanos();
    i64::try_from(nanos).context("system timestamp does not fit i64 nanoseconds")
}

fn help_text() -> &'static str {
    "Usage: muninn [serve|activate|request-move-light|move-light-status|move-state-status|claim-move-host|quest-access-status] [--store <path>] [--target-host <host>] [--port <port>] [--obs-target-host <host>] [--obs-port <port>] [--loopback-script <path>] [--ffmpeg <path>] [--move-state <move-id>=<hidraw-path>] [--move-host <bt-addr>] [--move-evidence-stream <stream-id>] [--move-evidence-verse <verse-id>] [--quest-adb] [--quest-serial <serial>] [--quest-input-stream <stream-id>] [--quest-pose-stream <stream-id>] [--quest-video-input-stream <stream-id>] [--dry-run] [--health]\n\nMuninn is Odin's portable telemetry Verse assembler. serve publishes cheap typed telemetry affordances, optional source-local Move controller state, optional Quest USB access surfaces, and a CultMesh Move evidence stream when Move state sources are attached; activate starts an explicitly requested local stream; request-move-light publishes a typed Move light command for Muninn serve to execute; move-light-status reads typed command receipts; move-state-status reads typed controller-state records; claim-move-host assigns USB-attached PS Moves to a Bluetooth host; quest-access-status reads typed Quest access state."
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
        assert!(plan.command_line.contains("ddagrab=framerate=30"));
        assert!(plan.command_line.contains("srt://10.77.0.2:5200"));
        assert!(plan.command_line.contains("srt://10.77.0.2:5204"));
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
        assert_eq!(writer.writes[0].1, vec![0x06, 0, 255, 64, 8, 0, 0, 0, 0]);
    }

    #[test]
    fn default_move_light_report_pulses_between_half_and_full_brightness() {
        assert_eq!(
            default_move_light_report((100, 80, 60), 0.0),
            [0x06, 0, 50, 40, 30, 0, 0, 0, 0]
        );
        assert_eq!(
            default_move_light_report((100, 80, 60), std::f64::consts::FRAC_PI_2),
            [0x06, 0, 100, 80, 60, 0, 0, 0, 0]
        );
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
    fn claim_move_host_accepts_target_bluetooth_address() {
        let options = Options::parse(
            [
                "claim-move-host",
                "--move-host",
                "5C:93:A2:9C:A8:A8",
            ]
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
    }

    impl MoveControllerStateReader for RecordingMoveStateReader {
        fn read_report(&mut self, _hidraw_path: &str) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }

        fn read_joystick_events(&mut self, _joystick_path: &str) -> Result<Vec<JoystickEvent>> {
            Ok(std::mem::take(&mut self.joystick_events))
        }
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

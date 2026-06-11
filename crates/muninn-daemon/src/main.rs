use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{
    MuninnCaptureStreamRecord, MuninnMoveControllerStateRecord, MuninnMoveLightCommandRecord,
    MuninnObsStreamCatalogRecord, MuninnTelemetrySurfaceRecord, OdinDocuments,
};
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::io::{ErrorKind, Read};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Serve,
    Activate,
    Health,
    DryRun,
    RequestMoveLight,
    MoveLightStatus,
    MoveStateStatus,
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
    move_state_sources: Vec<MoveStateSource>,
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
        Mode::DryRun => {
            let plan = build_mux_plan(&options, "dry-run".to_string());
            println!("{}", plan.command_line);
            Ok(())
        }
    }
}

fn serve(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let mut active_move_lights = Vec::new();
    let mut active_move_states: Vec<ActiveMoveStateSource> = options
        .move_state_sources
        .iter()
        .cloned()
        .map(|source| ActiveMoveStateSource {
            source,
            sequence: 0,
        })
        .collect();

    loop {
        let mut node = open_node(&options, "muninn-daemon")?;
        register_move_light_commands(&mut node, &options, &mut active_move_lights)?;
        tick_move_light_commands(&mut node, &mut active_move_lights, &mut HidMoveLightWriter)?;
        publish_move_controller_states(
            &mut node,
            &options,
            &mut active_move_states,
            &mut HidMoveControllerStateReader,
        )?;
        publish_surface(&mut node, &options, "idle", &[])?;
        if options.interval_seconds.is_none()
            && active_move_lights.is_empty()
            && active_move_states.is_empty()
        {
            return Ok(());
        }
        let sleep = if !active_move_lights.is_empty() || !active_move_states.is_empty() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(options.interval_seconds.unwrap_or(15))
        };
        thread::sleep(sleep);
    }
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
    vec![
        format!("screen:ddagrab:output_idx={}", options.ddagrab_output_index),
        format!(
            "audio-loopback:wasapi:{}:{}ch@{}",
            options.audio_device, options.audio_channels, options.audio_sample_rate
        ),
        "sensor:microphone:enumeration-pending".to_string(),
        "sensor:camera:enumeration-pending".to_string(),
    ]
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

trait MoveLightWriter {
    fn write_report(&mut self, hidraw_path: &str, report: &[u8]) -> Result<()>;
}

struct HidMoveLightWriter;

impl MoveLightWriter for HidMoveLightWriter {
    fn write_report(&mut self, hidraw_path: &str, report: &[u8]) -> Result<()> {
        let mut device = fs::OpenOptions::new()
            .write(true)
            .open(hidraw_path)
            .with_context(|| format!("opening PS Move HID path {hidraw_path}"))?;
        device
            .write_all(report)
            .with_context(|| format!("writing PS Move HID report to {hidraw_path}"))
    }
}

trait MoveControllerStateReader {
    fn read_report(&mut self, hidraw_path: &str) -> Result<Option<Vec<u8>>>;
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

    #[cfg(not(unix))]
    fn read_report(&mut self, hidraw_path: &str) -> Result<Option<Vec<u8>>> {
        if hidraw_path.trim().is_empty() {
            return Ok(None);
        }
        Err(anyhow!(
            "PS Move controller state HID reads are currently implemented for Unix hidraw paths"
        ))
    }
}

fn publish_move_controller_states(
    node: &mut cultmesh_rs::CultMeshNode,
    options: &Options,
    active: &mut [ActiveMoveStateSource],
    reader: &mut impl MoveControllerStateReader,
) -> Result<()> {
    for state in active {
        let Some(report) = reader.read_report(&state.source.hidraw_path)? else {
            continue;
        };
        state.sequence = state.sequence.saturating_add(1);
        let record = build_move_controller_state_record(
            options,
            &state.source,
            state.sequence,
            &report,
            timestamp_ns()?,
            timestamp()?,
        );
        node.put(&record.stream_id, &record)?;
        node.put(
            &format!("{}:{}", record.stream_id, record.sequence),
            &record,
        )?;
    }
    Ok(())
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

fn health_check(options: &Options) -> Result<()> {
    let node = open_node(options, "muninn-health")?;
    let surface = node
        .get_required::<MuninnTelemetrySurfaceRecord>("latest")
        .context("Muninn telemetry surface is unavailable")?;
    if surface.state == "idle" || surface.state == "active" {
        println!(
            "Muninn healthy: {} on {} ({})",
            surface.surface_id, surface.host_id, surface.state
        );
        Ok(())
    } else {
        Err(anyhow!(
            "Muninn telemetry surface is {}: {}",
            surface.state,
            surface.detail
        ))
    }
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
            "{} seq={} move={} buttons=[{}] trigger={:.3} accel={:?} gyro={:?} battery={:.3} observed={}",
            state.stream_id,
            state.sequence,
            state.move_id,
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
            move_state_sources: Vec::new(),
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => options.mode = Mode::Serve,
                "activate" => options.mode = Mode::Activate,
                "request-move-light" => options.mode = Mode::RequestMoveLight,
                "move-light-status" => options.mode = Mode::MoveLightStatus,
                "move-state-status" => options.mode = Mode::MoveStateStatus,
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
    "Usage: muninn [serve|activate|request-move-light|move-light-status|move-state-status] [--store <path>] [--target-host <host>] [--port <port>] [--obs-target-host <host>] [--obs-port <port>] [--loopback-script <path>] [--ffmpeg <path>] [--move-state <move-id>=<hidraw-path>] [--dry-run] [--health]\n\nMuninn is Odin's portable telemetry Verse assembler. serve publishes cheap typed telemetry affordances and optional source-local Move controller state; activate starts an explicitly requested local stream; request-move-light publishes a typed Move light command for Muninn serve to execute; move-light-status reads typed command receipts; move-state-status reads typed controller-state records."
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
    fn serve_accepts_move_state_sources() {
        let options = Options::parse(
            [
                "serve",
                "--host",
                "nightwing",
                "--move-state",
                "move-usb=/dev/hidraw1",
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

use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{MuninnCaptureStreamRecord, MuninnTelemetrySurfaceRecord, OdinDocuments};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Serve,
    Activate,
    Health,
    DryRun,
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
    obs_catalog_path: PathBuf,
    interval_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MuxPlan {
    command_line: String,
    command_file: PathBuf,
    targets: Vec<String>,
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
        Mode::DryRun => {
            let plan = build_mux_plan(&options, "dry-run".to_string());
            println!("{}", plan.command_line);
            Ok(())
        }
    }
}

fn serve(options: Options) -> Result<()> {
    ensure_state_dirs(&options)?;
    let mut node = open_node(&options, "muninn-daemon")?;

    loop {
        publish_surface(&mut node, &options, "idle", &[])?;
        if let Some(interval) = options.interval_seconds {
            thread::sleep(Duration::from_secs(interval));
        } else {
            return Ok(());
        }
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
    write_obs_catalog_idle(options)?;
    let record = MuninnTelemetrySurfaceRecord {
        surface_id: options.surface_id.clone(),
        host_id: options.host_id.clone(),
        state: state.to_string(),
        available_sources: available_sources(options),
        stream_affordances: vec![
            "screen.capture.ddagrab".to_string(),
            "audio.loopback.wasapi".to_string(),
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
    write_obs_catalog_active(options, plan, state)?;
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

fn write_obs_catalog_idle(options: &Options) -> Result<()> {
    if let Some(parent) = options.obs_catalog_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut lines = vec![
        "# stream_id\tlabel\turl\tstate".to_string(),
        format!(
            "{}\t{} screen and loopback A/V\t\tactivation-required",
            options.stream_id, options.host_id
        ),
    ];
    for source in available_sources(options) {
        lines.push(format!(
            "{}:{}\t{}\t\taffordance",
            options.surface_id, source, source
        ));
    }
    fs::write(&options.obs_catalog_path, lines.join("\r\n") + "\r\n")
        .with_context(|| format!("writing {}", options.obs_catalog_path.display()))
}

fn write_obs_catalog_active(options: &Options, plan: &MuxPlan, state: &str) -> Result<()> {
    if let Some(parent) = options.obs_catalog_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut lines = vec!["# stream_id\tlabel\turl\tstate".to_string()];
    for (index, target) in plan.targets.iter().enumerate() {
        lines.push(format!(
            "{}:{}\t{} A/V target {}\t{}\t{}",
            options.stream_id,
            index,
            options.host_id,
            index + 1,
            target,
            state
        ));
    }
    fs::write(&options.obs_catalog_path, lines.join("\r\n") + "\r\n")
        .with_context(|| format!("writing {}", options.obs_catalog_path.display()))
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
            obs_catalog_path: PathBuf::from("C:/Meta/Odin/state/muninn-obs-streams.tsv"),
            interval_seconds: None,
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "serve" => options.mode = Mode::Serve,
                "activate" => options.mode = Mode::Activate,
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
                "--obs-catalog" => {
                    options.obs_catalog_path =
                        PathBuf::from(take_value(&mut args, "--obs-catalog")?)
                }
                "--interval-seconds" => {
                    options.interval_seconds = Some(
                        take_value(&mut args, "--interval-seconds")?
                            .parse()
                            .context("--interval-seconds must be a positive integer")?,
                    )
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

fn help_text() -> &'static str {
    "Usage: muninn [serve|activate] [--store <path>] [--target-host <host>] [--port <port>] [--obs-target-host <host>] [--obs-port <port>] [--loopback-script <path>] [--ffmpeg <path>] [--dry-run] [--health]\n\nMuninn is Odin's portable telemetry Verse assembler. serve publishes cheap typed telemetry affordances; activate starts an explicitly requested local stream."
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

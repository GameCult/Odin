use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{
    IdunnDaemonHealthRecord, IdunnDesiredDaemonRecord, IdunnRestartResultRecord, OdinDocuments,
    plan_keepalive,
};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
struct Options {
    store_path: PathBuf,
    daemon_id: String,
    verse_id: String,
    name: String,
    health_command: Option<String>,
    restart_command: Option<String>,
    enabled: bool,
    execute: bool,
    interval_seconds: Option<u64>,
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;

    if let Some(parent) = options.store_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    if let Some(interval_seconds) = options.interval_seconds {
        loop {
            run_cycle(&options)?;
            thread::sleep(Duration::from_secs(interval_seconds));
        }
    }

    run_cycle(&options)
}

fn run_cycle(options: &Options) -> Result<()> {
    let now = timestamp()?;

    let desired = IdunnDesiredDaemonRecord {
        daemon_id: options.daemon_id.clone(),
        verse_id: options.verse_id.clone(),
        name: options.name.clone(),
        enabled: options.enabled,
        health_command: options.health_command.clone(),
        restart_command: options.restart_command.clone(),
        authority: "idunn.local-command".to_string(),
        max_silence_seconds: 60,
        observed_at: now.clone(),
    };

    let health = probe_health(&options, &now);
    let plan = plan_keepalive(&desired, &health, now.clone());

    let mut node = CultMesh::create_node(
        &options.store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "idunn-daemon".to_string(),
            pull_on_start: true,
        },
    )?;

    node.put(&desired.daemon_id, &desired)?;
    node.put(&health.daemon_id, &health)?;
    node.put(&plan.decision.decision_id, &plan.decision)?;

    if let Some(request) = &plan.restart_request {
        node.put(&request.request_id, request)?;
        if options.execute {
            let result = run_restart(request, &now);
            node.put(&result.result_id, &result)?;
            println!(
                "Idunn restart {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
        } else {
            println!(
                "Idunn requested restart for {} but did not execute it. Pass --execute to actuate.",
                request.daemon_id
            );
        }
    }

    if let Some(alarm) = &plan.operator_alarm {
        node.put(&alarm.alarm_id, alarm)?;
        println!(
            "Idunn raised operator alarm for {} through {}: {}",
            alarm.daemon_id, alarm.escalation_target, alarm.reason
        );
    }

    println!(
        "Idunn decision for {}: {} ({})",
        plan.decision.daemon_id, plan.decision.action, plan.decision.reason
    );
    println!("CultMesh store: {}", options.store_path.display());
    Ok(())
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Options {
            store_path: PathBuf::from("scratch/idunn/idunn.keepalive.cc"),
            daemon_id: String::new(),
            verse_id: "local".to_string(),
            name: String::new(),
            health_command: None,
            restart_command: None,
            enabled: true,
            execute: false,
            interval_seconds: None,
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--store" => options.store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--daemon" => options.daemon_id = take_value(&mut args, "--daemon")?,
                "--verse" => options.verse_id = take_value(&mut args, "--verse")?,
                "--name" => options.name = take_value(&mut args, "--name")?,
                "--health-command" => {
                    options.health_command = Some(take_value(&mut args, "--health-command")?)
                }
                "--restart-command" => {
                    options.restart_command = Some(take_value(&mut args, "--restart-command")?)
                }
                "--disabled" => options.enabled = false,
                "--execute" => options.execute = true,
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
                        "unknown Idunn argument: {other}\n\n{}",
                        help_text()
                    ));
                }
            }
        }

        if options.daemon_id.trim().is_empty() {
            return Err(anyhow!("--daemon is required\n\n{}", help_text()));
        }
        if options.name.trim().is_empty() {
            options.name = options.daemon_id.clone();
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

fn probe_health(options: &Options, observed_at: &str) -> IdunnDaemonHealthRecord {
    match &options.health_command {
        Some(command) => match run_shell(command) {
            Ok(output) if output.status.success() => IdunnDaemonHealthRecord {
                daemon_id: options.daemon_id.clone(),
                state: "active".to_string(),
                detail: "health command exited successfully".to_string(),
                observed_at: observed_at.to_string(),
            },
            Ok(output) => IdunnDaemonHealthRecord {
                daemon_id: options.daemon_id.clone(),
                state: "failed".to_string(),
                detail: format!("health command exited with {}", output.status),
                observed_at: observed_at.to_string(),
            },
            Err(error) => IdunnDaemonHealthRecord {
                daemon_id: options.daemon_id.clone(),
                state: "failed".to_string(),
                detail: format!("health command could not run: {error}"),
                observed_at: observed_at.to_string(),
            },
        },
        None => IdunnDaemonHealthRecord {
            daemon_id: options.daemon_id.clone(),
            state: "unknown".to_string(),
            detail: "no health command was provided".to_string(),
            observed_at: observed_at.to_string(),
        },
    }
}

fn run_restart(
    request: &odin_core::IdunnRestartRequestRecord,
    requested_at: &str,
) -> IdunnRestartResultRecord {
    let result_id = format!("result:{}", request.request_id);
    match run_shell(&request.command) {
        Ok(output) if output.status.success() => IdunnRestartResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "succeeded".to_string(),
            detail: "restart command exited successfully".to_string(),
            completed_at: requested_at.to_string(),
        },
        Ok(output) => IdunnRestartResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: format!("restart command exited with {}", output.status),
            completed_at: requested_at.to_string(),
        },
        Err(error) => IdunnRestartResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: format!("restart command could not run: {error}"),
            completed_at: requested_at.to_string(),
        },
    }
}

fn run_shell(command: &str) -> Result<std::process::Output> {
    if cfg!(windows) {
        Command::new("cmd").arg("/C").arg(command).output()
    } else {
        Command::new("sh").arg("-c").arg(command).output()
    }
    .with_context(|| format!("running command {command:?}"))
}

fn timestamp() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    Ok(format!("unix:{seconds}"))
}

fn help_text() -> &'static str {
    "Usage: idunn --daemon <id> [--name <name>] [--verse <verse>] [--store <path>] [--health-command <command>] [--restart-command <command>] [--execute] [--interval-seconds <seconds>]\n\nIdunn probes one daemon, writes typed CultMesh records, and executes restart only when --execute is present."
}

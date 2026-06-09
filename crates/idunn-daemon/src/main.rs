use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{
    IdunnDaemonHealthRecord, IdunnDeploymentResultRecord, IdunnDesiredDaemonRecord,
    IdunnOperatorAlarmRecord, IdunnRestartResultRecord, OdinDocuments, plan_keepalive,
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
    deploy_command: Option<String>,
    restart_command: Option<String>,
    operator_alarm_command: Option<String>,
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
        deploy_command: options.deploy_command.clone(),
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

    if let Some(request) = &plan.deployment_request {
        node.put(&request.request_id, request)?;
        if options.execute {
            let result = run_deployment(request, &now);
            node.put(&result.result_id, &result)?;
            println!(
                "Idunn deployment {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
            if result.state != "succeeded" {
                let alarm = deployment_failure_alarm(&result, &now);
                node.put(&alarm.alarm_id, &alarm)?;
                println!(
                    "Idunn raised operator alarm for {} through {}: {}",
                    alarm.daemon_id, alarm.escalation_target, alarm.reason
                );
                run_operator_alarm_command(&options, &alarm);
            }
        } else {
            println!(
                "Idunn requested deployment for {} but did not execute it. Pass --execute to actuate.",
                request.daemon_id
            );
        }
    }

    if let Some(request) = &plan.restart_request {
        node.put(&request.request_id, request)?;
        if options.execute {
            let result = run_restart(request, &now);
            node.put(&result.result_id, &result)?;
            println!(
                "Idunn restart {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
            if result.state != "succeeded" {
                let alarm = restart_failure_alarm(&result, &now);
                node.put(&alarm.alarm_id, &alarm)?;
                println!(
                    "Idunn raised operator alarm for {} through {}: {}",
                    alarm.daemon_id, alarm.escalation_target, alarm.reason
                );
                run_operator_alarm_command(&options, &alarm);
            }
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
        run_operator_alarm_command(options, alarm);
    }

    println!(
        "Idunn decision for {}: {} ({})",
        plan.decision.daemon_id, plan.decision.action, plan.decision.reason
    );
    println!("CultMesh store: {}", options.store_path.display());
    Ok(())
}

fn restart_failure_alarm(
    result: &IdunnRestartResultRecord,
    raised_at: &str,
) -> IdunnOperatorAlarmRecord {
    IdunnOperatorAlarmRecord {
        alarm_id: format!("alarm:restart-failed:{}:{}", result.daemon_id, raised_at),
        daemon_id: result.daemon_id.clone(),
        severity: "operator-action-required".to_string(),
        reason: format!(
            "restart command failed for request {}: {}",
            result.request_id, result.detail
        ),
        escalation_target: "bifrost.operator-notification".to_string(),
        raised_at: raised_at.to_string(),
    }
}

fn deployment_failure_alarm(
    result: &IdunnDeploymentResultRecord,
    raised_at: &str,
) -> IdunnOperatorAlarmRecord {
    IdunnOperatorAlarmRecord {
        alarm_id: format!("alarm:deployment-failed:{}:{}", result.daemon_id, raised_at),
        daemon_id: result.daemon_id.clone(),
        severity: "operator-action-required".to_string(),
        reason: format!(
            "deployment command failed for request {}: {}",
            result.request_id, result.detail
        ),
        escalation_target: "bifrost.operator-notification".to_string(),
        raised_at: raised_at.to_string(),
    }
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Options {
            store_path: PathBuf::from("scratch/idunn/idunn.keepalive.cc"),
            daemon_id: String::new(),
            verse_id: "local".to_string(),
            name: String::new(),
            health_command: None,
            deploy_command: None,
            restart_command: None,
            operator_alarm_command: None,
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
                "--deploy-command" => {
                    options.deploy_command = Some(take_value(&mut args, "--deploy-command")?)
                }
                "--restart-command" => {
                    options.restart_command = Some(take_value(&mut args, "--restart-command")?)
                }
                "--operator-alarm-command" => {
                    options.operator_alarm_command =
                        Some(take_value(&mut args, "--operator-alarm-command")?)
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

fn run_deployment(
    request: &odin_core::IdunnDeploymentRequestRecord,
    requested_at: &str,
) -> IdunnDeploymentResultRecord {
    let result_id = format!("result:{}", request.request_id);
    match run_shell(&request.command) {
        Ok(output) if output.status.success() => IdunnDeploymentResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "succeeded".to_string(),
            detail: "deployment command exited successfully".to_string(),
            completed_at: requested_at.to_string(),
        },
        Ok(output) => IdunnDeploymentResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: format!("deployment command exited with {}", output.status),
            completed_at: requested_at.to_string(),
        },
        Err(error) => IdunnDeploymentResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: format!("deployment command could not run: {error}"),
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

fn run_operator_alarm_command(options: &Options, alarm: &IdunnOperatorAlarmRecord) {
    let Some(command) = options.operator_alarm_command.as_deref() else {
        return;
    };
    if command.trim().is_empty() {
        return;
    }

    let mut process = if cfg!(windows) {
        let mut process = Command::new("cmd");
        process.arg("/C").arg(command);
        process
    } else {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process
    };

    let output = process
        .env("IDUNN_ALARM_ID", &alarm.alarm_id)
        .env("IDUNN_ALARM_DAEMON_ID", &alarm.daemon_id)
        .env("IDUNN_ALARM_SEVERITY", &alarm.severity)
        .env("IDUNN_ALARM_REASON", &alarm.reason)
        .env("IDUNN_ALARM_ESCALATION_TARGET", &alarm.escalation_target)
        .env("IDUNN_ALARM_RAISED_AT", &alarm.raised_at)
        .output();

    match output {
        Ok(output) if output.status.success() => {
            println!(
                "Idunn operator alarm command completed for {}.",
                alarm.daemon_id
            );
        }
        Ok(output) => {
            eprintln!(
                "Idunn operator alarm command failed for {} with {}.",
                alarm.daemon_id, output.status
            );
        }
        Err(error) => {
            eprintln!(
                "Idunn operator alarm command could not run for {}: {}",
                alarm.daemon_id, error
            );
        }
    }
}

fn timestamp() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    Ok(format!("unix:{seconds}"))
}

fn help_text() -> &'static str {
    "Usage: idunn --daemon <id> [--name <name>] [--verse <verse>] [--store <path>] [--health-command <command>] [--deploy-command <command>] [--restart-command <command>] [--operator-alarm-command <command>] [--execute] [--interval-seconds <seconds>]\n\nIdunn probes one daemon, writes typed CultMesh records, executes deployment or restart only when --execute is present, and may invoke an operator alarm bridge command only after an alarm is raised."
}

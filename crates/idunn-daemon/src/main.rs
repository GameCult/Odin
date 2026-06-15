use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNode, CultMeshNodeOptions};
use odin_core::{
    IdunnDaemonHealthRecord, IdunnDeploymentResultRecord, IdunnDesiredDaemonRecord,
    IdunnOperatorAlarmRecord, IdunnRestartResultRecord, OdinDocuments, plan_keepalive,
};
use std::env;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
struct DaemonTarget {
    daemon_id: String,
    verse_id: String,
    name: String,
    health_contract: HealthContract,
    health_command: Option<String>,
    deploy_command: Option<String>,
    restart_command: Option<String>,
    enabled: bool,
    interval_seconds: u64,
}

#[derive(Clone, Debug)]
struct HealthContract {
    id: String,
    default_failure_state: String,
}

fn health_contract(id: &str, default_failure_state: &str) -> HealthContract {
    HealthContract {
        id: id.to_string(),
        default_failure_state: default_failure_state.to_string(),
    }
}

#[derive(Clone, Debug)]
struct CommonOptions {
    store_path: PathBuf,
    operator_alarm_command: Option<String>,
    execute: bool,
    command_timeout_seconds: u64,
}

#[derive(Clone, Debug)]
enum Mode {
    Single(DaemonTarget),
    Swarm(SwarmOptions),
}

#[derive(Clone, Debug)]
struct SwarmOptions {
    profile: String,
    repo_root: PathBuf,
}

#[derive(Clone, Debug)]
struct Options {
    common: CommonOptions,
    mode: Mode,
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;

    if let Some(parent) = options.common.store_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    match &options.mode {
        Mode::Single(target) => {
            let store_lock = Arc::new(Mutex::new(()));
            run_target_cycle(target, &options.common, &store_lock)
        }
        Mode::Swarm(swarm) => run_swarm(swarm, &options.common),
    }
}

fn run_swarm(options: &SwarmOptions, common: &CommonOptions) -> Result<()> {
    let targets = swarm_targets(options)?;
    if targets.is_empty() {
        return Err(anyhow!(
            "Idunn swarm profile {} resolved to no targets",
            options.profile
        ));
    }
    validate_targets(&targets)?;

    println!(
        "Idunn swarm profile {} starting with {} targets.",
        options.profile,
        targets.len()
    );
    println!("CultMesh store: {}", common.store_path.display());

    let store_lock = Arc::new(Mutex::new(()));
    let mut workers = Vec::with_capacity(targets.len());
    for target in targets {
        let worker_common = common.clone();
        let worker_store_lock = Arc::clone(&store_lock);
        workers.push(thread::spawn(move || {
            run_target_loop(target, worker_common, worker_store_lock)
        }));
    }

    for worker in workers {
        match worker.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(_) => return Err(anyhow!("Idunn swarm worker thread panicked")),
        }
    }

    Ok(())
}

fn validate_targets(targets: &[DaemonTarget]) -> Result<()> {
    let mut issues = Vec::new();
    for target in targets {
        if target.health_contract.id.trim().is_empty() {
            issues.push(format!("{} has no health contract", target.daemon_id));
        }
        if target.health_command.is_none() {
            issues.push(format!("{} has no health command", target.daemon_id));
        }
        if target.health_contract.default_failure_state == "stale-deployment"
            && target.deploy_command.is_none()
        {
            issues.push(format!(
                "{} treats probe failure as stale deployment but has no deploy command",
                target.daemon_id
            ));
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "Idunn target catalog is incoherent: {}",
            issues.join("; ")
        ))
    }
}

fn run_target_loop(
    target: DaemonTarget,
    options: CommonOptions,
    store_lock: Arc<Mutex<()>>,
) -> Result<()> {
    loop {
        if let Err(error) = run_target_cycle(&target, &options, &store_lock) {
            eprintln!(
                "Idunn swarm target {} cycle failed: {}",
                target.daemon_id, error
            );
        }
        thread::sleep(Duration::from_secs(target.interval_seconds));
    }
}

fn run_target_cycle(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
) -> Result<()> {
    let now = timestamp()?;

    let desired = IdunnDesiredDaemonRecord {
        daemon_id: target.daemon_id.clone(),
        verse_id: target.verse_id.clone(),
        name: target.name.clone(),
        enabled: target.enabled,
        health_command: target.health_command.clone(),
        restart_command: target.restart_command.clone(),
        deploy_command: target.deploy_command.clone(),
        health_contract: target.health_contract.id.clone(),
        authority: "idunn.local-command".to_string(),
        max_silence_seconds: 60,
        observed_at: now.clone(),
    };

    let health = probe_health(target, options.command_timeout_seconds, &now);
    let plan = plan_keepalive(&desired, &health, now.clone());

    with_store_node(options, store_lock, |node| {
        node.put(&desired.daemon_id, &desired)?;
        node.put(&health.daemon_id, &health)?;
        node.put(&plan.decision.decision_id, &plan.decision)?;
        if let Some(request) = &plan.deployment_request {
            node.put(&request.request_id, request)?;
        }
        if let Some(request) = &plan.restart_request {
            node.put(&request.request_id, request)?;
        }
        if let Some(alarm) = &plan.operator_alarm {
            node.put(&alarm.alarm_id, alarm)?;
        }
        Ok(())
    })?;

    if let Some(request) = &plan.deployment_request {
        if options.execute {
            let result = run_deployment(request, &now, options.command_timeout_seconds);
            let alarm = if result.state != "succeeded" {
                Some(deployment_failure_alarm(&result, &now))
            } else {
                None
            };
            with_store_node(options, store_lock, |node| {
                node.put(&result.result_id, &result)?;
                if let Some(alarm) = &alarm {
                    node.put(&alarm.alarm_id, alarm)?;
                }
                Ok(())
            })?;
            println!(
                "Idunn deployment {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
            if let Some(alarm) = alarm {
                println!(
                    "Idunn raised operator alarm for {} through {}: {}",
                    alarm.daemon_id, alarm.escalation_target, alarm.reason
                );
                run_operator_alarm_command(options, &alarm);
            }
        } else {
            println!(
                "Idunn requested deployment for {} but did not execute it. Pass --execute to actuate.",
                request.daemon_id
            );
        }
    }

    if let Some(request) = &plan.restart_request {
        if options.execute {
            let result = run_restart(request, &now, options.command_timeout_seconds);
            let alarm = if result.state != "succeeded" {
                Some(restart_failure_alarm(&result, &now))
            } else {
                None
            };
            with_store_node(options, store_lock, |node| {
                node.put(&result.result_id, &result)?;
                if let Some(alarm) = &alarm {
                    node.put(&alarm.alarm_id, alarm)?;
                }
                Ok(())
            })?;
            println!(
                "Idunn restart {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
            if let Some(alarm) = alarm {
                println!(
                    "Idunn raised operator alarm for {} through {}: {}",
                    alarm.daemon_id, alarm.escalation_target, alarm.reason
                );
                run_operator_alarm_command(options, &alarm);
            }
        } else {
            println!(
                "Idunn requested restart for {} but did not execute it. Pass --execute to actuate.",
                request.daemon_id
            );
        }
    }

    if let Some(alarm) = &plan.operator_alarm {
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
    Ok(())
}

fn with_store_node<F>(options: &CommonOptions, store_lock: &Arc<Mutex<()>>, write: F) -> Result<()>
where
    F: FnOnce(&mut CultMeshNode) -> Result<()>,
{
    let _store_guard = store_lock
        .lock()
        .map_err(|_| anyhow!("Idunn store lock is poisoned"))?;
    let mut node = CultMesh::create_node(
        &options.store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "idunn-daemon".to_string(),
            pull_on_start: true,
        },
    )?;
    write(&mut node)
}

fn swarm_targets(options: &SwarmOptions) -> Result<Vec<DaemonTarget>> {
    let repo_root = options.repo_root.display().to_string();
    let script = |name: &str| format!(r"{}\scripts\{}", repo_root, name);

    match options.profile.as_str() {
        "starfire-local" => Ok(vec![
            DaemonTarget {
                daemon_id: "odin".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Odin all-seer".to_string(),
                health_contract: health_contract("odin.http-health-and-deck-publication", "failed"),
                health_command: Some(script("health-odin.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-odin.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "mimir-eve-dashboard".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Mimir Eve dashboard".to_string(),
                health_contract: health_contract("mimir.http-dashboard-health", "failed"),
                health_command: Some(script("health-mimir-eve-dashboard.cmd")),
                deploy_command: None,
                restart_command: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "stonks".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Stonks market pulse".to_string(),
                health_contract: health_contract("stonks.http-health", "failed"),
                health_command: Some(script("health-stonks.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-stonks.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "voidbot".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "VoidBot local stack".to_string(),
                health_contract: health_contract("voidbot.stack-health", "failed"),
                health_command: Some(script("health-voidbot.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-voidbot.cmd")),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "weksa".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Weksa intent and utterance lowering service".to_string(),
                health_contract: health_contract("weksa.provider-health", "failed"),
                health_command: Some(script("health-weksa.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-weksa.cmd")),
                enabled: true,
                interval_seconds: 60,
            },
            DaemonTarget {
                daemon_id: "starfire-muninn".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Starfire Muninn telemetry and Quest access daemon".to_string(),
                health_contract: health_contract(
                    "muninn.local-telemetry-and-quest-access",
                    "failed",
                ),
                health_command: Some(script("health-starfire-muninn.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-starfire-muninn.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "muninn".to_string(),
                verse_id: "raven.local".to_string(),
                name: "Muninn telemetry Verse assembler".to_string(),
                health_contract: health_contract("muninn.remote-telemetry-health", "failed"),
                health_command: Some(script("health-muninn.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-muninn.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "vili".to_string(),
                verse_id: "raven.local".to_string(),
                name: "Vili Persona animation daemon".to_string(),
                health_contract: health_contract("vili.http-animation-health", "failed"),
                health_command: Some(script("health-vili.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-vili.cmd")),
                enabled: true,
                interval_seconds: 60,
            },
            DaemonTarget {
                daemon_id: "idunn-swarm-deployment-coverage".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Idunn swarm deployment coverage".to_string(),
                health_contract: health_contract("idunn.deployment-catalog-coherence", "degraded"),
                health_command: Some(script("health-idunn-swarm-deployment-coverage.cmd")),
                deploy_command: None,
                restart_command: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-heimdall".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Heimdall".to_string(),
                health_contract: health_contract(
                    "yggdrasil.source-deployment-freshness",
                    "stale-deployment",
                ),
                health_command: Some(script("health-yggdrasil-heimdall.cmd")),
                deploy_command: Some(script("deploy-yggdrasil-heimdall.cmd")),
                restart_command: None,
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-repixelizer".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Repixelizer".to_string(),
                health_contract: health_contract(
                    "yggdrasil.source-deployment-freshness",
                    "stale-deployment",
                ),
                health_command: Some(script("health-yggdrasil-repixelizer.cmd")),
                deploy_command: Some(script("deploy-yggdrasil-repixelizer.cmd")),
                restart_command: None,
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-streampixels".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil StreamPixels".to_string(),
                health_contract: health_contract(
                    "yggdrasil.source-deployment-freshness",
                    "stale-deployment",
                ),
                health_command: Some(script("health-yggdrasil-streampixels.cmd")),
                deploy_command: Some(script("deploy-yggdrasil-streampixels.cmd")),
                restart_command: None,
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "nightwing-gjallar".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Gjallar framebuffer compositor".to_string(),
                health_contract: health_contract(
                    "gjallar.framebuffer-composition-witness",
                    "dependency-unavailable",
                ),
                health_command: Some(script("health-nightwing-gjallar.cmd")),
                deploy_command: Some(script("deploy-nightwing-gjallar.cmd")),
                restart_command: Some(script("restart-nightwing-gjallar.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "nightwing-muninn".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Muninn telemetry and Move HID daemon".to_string(),
                health_contract: health_contract("muninn.remote-telemetry-and-move-hid", "failed"),
                health_command: Some(script("health-nightwing-muninn.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-muninn.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "nightwing-eve-dashboard".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Eve dashboard broker".to_string(),
                health_contract: health_contract("nightwing.systemd-service-liveness", "failed"),
                health_command: Some(script("health-nightwing-eve-dashboard.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-eve-dashboard.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "nightwing-eve-browser-reference".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Eve browser reference".to_string(),
                health_contract: health_contract("nightwing.systemd-service-liveness", "failed"),
                health_command: Some(script("health-nightwing-eve-browser-reference.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-eve-browser-reference.cmd")),
                enabled: true,
                interval_seconds: 30,
            },
        ]),
        other => Err(anyhow!("unknown Idunn swarm profile: {other}")),
    }
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
        let mut store_path = PathBuf::from("scratch/idunn/idunn.keepalive.cc");
        let mut operator_alarm_command = None;
        let mut execute = false;
        let mut command_timeout_seconds = 30;
        let mut daemon_id = None;
        let mut verse_id = "local".to_string();
        let mut name = None;
        let mut health_command = None;
        let mut deploy_command = None;
        let mut restart_command = None;
        let mut enabled = true;
        let mut interval_seconds = None;
        let mut swarm_profile = None;
        let mut repo_root = env::current_dir().context("determining current directory")?;

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--store" => store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--daemon" => daemon_id = Some(take_value(&mut args, "--daemon")?),
                "--verse" => verse_id = take_value(&mut args, "--verse")?,
                "--name" => name = Some(take_value(&mut args, "--name")?),
                "--health-command" => {
                    health_command = Some(take_value(&mut args, "--health-command")?)
                }
                "--deploy-command" => {
                    deploy_command = Some(take_value(&mut args, "--deploy-command")?)
                }
                "--restart-command" => {
                    restart_command = Some(take_value(&mut args, "--restart-command")?)
                }
                "--operator-alarm-command" => {
                    operator_alarm_command =
                        Some(take_value(&mut args, "--operator-alarm-command")?)
                }
                "--disabled" => enabled = false,
                "--execute" => execute = true,
                "--interval-seconds" => {
                    interval_seconds = Some(
                        take_value(&mut args, "--interval-seconds")?
                            .parse()
                            .context("--interval-seconds must be a positive integer")?,
                    )
                }
                "--command-timeout-seconds" => {
                    command_timeout_seconds = take_value(&mut args, "--command-timeout-seconds")?
                        .parse()
                        .context("--command-timeout-seconds must be a positive integer")?
                }
                "--swarm-profile" => {
                    swarm_profile = Some(take_value(&mut args, "--swarm-profile")?)
                }
                "--repo-root" => repo_root = PathBuf::from(take_value(&mut args, "--repo-root")?),
                "--help" | "-h" => return Err(anyhow!(help_text())),
                other => {
                    return Err(anyhow!(
                        "unknown Idunn argument: {other}\n\n{}",
                        help_text()
                    ));
                }
            }
        }

        if command_timeout_seconds == 0 {
            return Err(anyhow!(
                "--command-timeout-seconds must be greater than zero"
            ));
        }

        let common = CommonOptions {
            store_path,
            operator_alarm_command,
            execute,
            command_timeout_seconds,
        };

        let mode = match (swarm_profile, daemon_id) {
            (Some(profile), None) => Mode::Swarm(SwarmOptions { profile, repo_root }),
            (Some(_), Some(_)) => {
                return Err(anyhow!(
                    "use either --swarm-profile or --daemon, not both\n\n{}",
                    help_text()
                ));
            }
            (None, Some(daemon_id)) => {
                let interval_seconds = interval_seconds.unwrap_or(30);
                if interval_seconds == 0 {
                    return Err(anyhow!("--interval-seconds must be greater than zero"));
                }
                Mode::Single(DaemonTarget {
                    daemon_id: daemon_id.clone(),
                    verse_id,
                    name: name.unwrap_or(daemon_id),
                    health_contract: health_contract("manual.command-health", "failed"),
                    health_command,
                    deploy_command,
                    restart_command,
                    enabled,
                    interval_seconds,
                })
            }
            (None, None) => {
                return Err(anyhow!(
                    "either --daemon or --swarm-profile is required\n\n{}",
                    help_text()
                ));
            }
        };

        Ok(Self { common, mode })
    }
}

fn take_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn probe_health(
    target: &DaemonTarget,
    command_timeout_seconds: u64,
    observed_at: &str,
) -> IdunnDaemonHealthRecord {
    match &target.health_command {
        Some(command) => match run_shell(command, command_timeout_seconds) {
            Ok(output) if output.status.success() => IdunnDaemonHealthRecord {
                daemon_id: target.daemon_id.clone(),
                state: "active".to_string(),
                detail: command_output_detail("health command exited successfully", &output),
                health_contract: target.health_contract.id.clone(),
                observed_at: observed_at.to_string(),
            },
            Ok(output) => {
                let detail = command_output_detail(
                    &format!("health command exited with {}", output.status),
                    &output,
                );
                IdunnDaemonHealthRecord {
                    daemon_id: target.daemon_id.clone(),
                    state: health_state_from_detail(&detail, target).to_string(),
                    detail,
                    health_contract: target.health_contract.id.clone(),
                    observed_at: observed_at.to_string(),
                }
            }
            Err(error) => IdunnDaemonHealthRecord {
                daemon_id: target.daemon_id.clone(),
                state: "failed".to_string(),
                detail: format!("health command could not run: {error}"),
                health_contract: target.health_contract.id.clone(),
                observed_at: observed_at.to_string(),
            },
        },
        None => IdunnDaemonHealthRecord {
            daemon_id: target.daemon_id.clone(),
            state: "unknown".to_string(),
            detail: "no health command was provided".to_string(),
            health_contract: target.health_contract.id.clone(),
            observed_at: observed_at.to_string(),
        },
    }
}

fn health_state_from_detail<'a>(detail: &'a str, target: &'a DaemonTarget) -> &'a str {
    for state in [
        "stale-deployment",
        "dependency-unavailable",
        "degraded",
        "failed",
    ] {
        let marker = format!("idunn.health.state={state}");
        if detail.contains(&marker) {
            return state;
        }
    }

    &target.health_contract.default_failure_state
}

fn run_restart(
    request: &odin_core::IdunnRestartRequestRecord,
    requested_at: &str,
    command_timeout_seconds: u64,
) -> IdunnRestartResultRecord {
    let result_id = format!("result:{}", request.request_id);
    match run_shell(&request.command, command_timeout_seconds) {
        Ok(output) if output.status.success() => IdunnRestartResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "succeeded".to_string(),
            detail: command_output_detail("restart command exited successfully", &output),
            completed_at: requested_at.to_string(),
        },
        Ok(output) => IdunnRestartResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: command_output_detail(
                &format!("restart command exited with {}", output.status),
                &output,
            ),
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
    command_timeout_seconds: u64,
) -> IdunnDeploymentResultRecord {
    let result_id = format!("result:{}", request.request_id);
    match run_shell(&request.command, command_timeout_seconds) {
        Ok(output) if output.status.success() => IdunnDeploymentResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "succeeded".to_string(),
            detail: command_output_detail("deployment command exited successfully", &output),
            completed_at: requested_at.to_string(),
        },
        Ok(output) => IdunnDeploymentResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: command_output_detail(
                &format!("deployment command exited with {}", output.status),
                &output,
            ),
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

fn command_output_detail(prefix: &str, output: &std::process::Output) -> String {
    let mut detail = prefix.to_string();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    if !stdout.is_empty() {
        detail.push_str("; stdout: ");
        detail.push_str(&truncate_detail(stdout, 600));
    }
    if !stderr.is_empty() {
        detail.push_str("; stderr: ");
        detail.push_str(&truncate_detail(stderr, 600));
    }
    detail
}

fn truncate_detail(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn run_shell(command: &str, timeout_seconds: u64) -> Result<std::process::Output> {
    let mut process = if cfg!(windows) {
        let path = windows_command_path();
        let command = format!(r#"set "PATH={path}" && set "Path={path}" && {command}"#);
        let mut process = Command::new("cmd");
        process.arg("/D").arg("/S").arg("/C").arg(command);
        apply_windows_command_environment(&mut process, &path);
        process
    } else {
        let mut process = Command::new("sh");
        process.arg("-c").arg(command);
        process
    };

    let child = process
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running command {command:?}"))?;
    wait_for_child_with_timeout(child, Duration::from_secs(timeout_seconds), command)
}

fn run_operator_alarm_command(options: &CommonOptions, alarm: &IdunnOperatorAlarmRecord) {
    let Some(command) = options.operator_alarm_command.as_deref() else {
        return;
    };
    if command.trim().is_empty() {
        return;
    }

    let mut process = if cfg!(windows) {
        let path = windows_command_path();
        let command = format!(r#"set "PATH={path}" && set "Path={path}" && {command}"#);
        let mut process = Command::new("cmd");
        process.arg("/D").arg("/S").arg("/C").arg(command);
        apply_windows_command_environment(&mut process, &path);
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    match output {
        Ok(child) => match wait_for_child_with_timeout(
            child,
            Duration::from_secs(options.command_timeout_seconds),
            command,
        ) {
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
        },
        Err(error) => {
            eprintln!(
                "Idunn operator alarm command could not run for {}: {}",
                alarm.daemon_id, error
            );
        }
    }
}

fn windows_command_path() -> String {
    [
        r"C:\WINDOWS\system32",
        r"C:\WINDOWS",
        r"C:\WINDOWS\System32\Wbem",
        r"C:\WINDOWS\System32\WindowsPowerShell\v1.0",
        r"C:\WINDOWS\System32\OpenSSH",
        r"C:\Program Files\Git\cmd",
        r"C:\Program Files\nodejs",
        r"C:\Program Files\Docker\Docker\resources\bin",
        r"C:\Users\Meta\AppData\Local\Programs\Ollama",
        r"C:\Program Files\dotnet",
        r"C:\Users\Meta\.cargo\bin",
    ]
    .join(";")
}

fn apply_windows_command_environment(process: &mut Command, path: &str) {
    if !cfg!(windows) {
        return;
    }

    process.env("PATH", path);
    process.env("Path", path);
}

fn wait_for_child_with_timeout(
    mut child: Child,
    timeout: Duration,
    command: &str,
) -> Result<std::process::Output> {
    let started_at = Instant::now();

    loop {
        if child
            .try_wait()
            .with_context(|| format!("waiting on command {command:?}"))?
            .is_some()
        {
            return child
                .wait_with_output()
                .with_context(|| format!("collecting output for command {command:?}"));
        }

        if started_at.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait_with_output();
            return Err(anyhow!(
                "command timed out after {} seconds: {command:?}",
                timeout.as_secs()
            ));
        }

        thread::sleep(Duration::from_millis(100));
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
    "Usage: idunn --daemon <id> [--name <name>] [--verse <verse>] [--store <path>] [--health-command <command>] [--deploy-command <command>] [--restart-command <command>] [--operator-alarm-command <command>] [--execute] [--interval-seconds <seconds>] [--command-timeout-seconds <seconds>] [--repo-root <path>] [--swarm-profile <profile>]\n\nIdunn can run one manual daemon probe lane with --daemon, or one built-in swarm supervisor with --swarm-profile starfire-local."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(default_failure_state: &str, deploy_command: Option<&str>) -> DaemonTarget {
        DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.contract", default_failure_state),
            health_command: Some("exit 0".to_string()),
            deploy_command: deploy_command.map(ToString::to_string),
            restart_command: Some("restart test".to_string()),
            enabled: true,
            interval_seconds: 30,
        }
    }

    #[test]
    fn target_catalog_requires_health_contract() {
        let mut target = target("failed", None);
        target.health_contract.id = String::new();

        let error = validate_targets(&[target]).unwrap_err().to_string();

        assert!(error.contains("has no health contract"));
    }

    #[test]
    fn stale_deployment_contract_requires_deploy_authority() {
        let error = validate_targets(&[target("stale-deployment", None)])
            .unwrap_err()
            .to_string();

        assert!(error.contains("has no deploy command"));
    }

    #[test]
    fn health_state_uses_contract_default_when_probe_has_no_marker() {
        let target = target("dependency-unavailable", None);

        assert_eq!(
            health_state_from_detail("plain command failure", &target),
            "dependency-unavailable"
        );
    }
}

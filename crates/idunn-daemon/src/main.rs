use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNode, CultMeshNodeOptions};
use cultnet_rs::{
    CultNetMessage, CultNetRawPayloadEncoding, CultNetRudpPacketType, CultNetRudpSession,
    CultNetRudpSessionOptions, CultNetRudpSocketTransportConnection,
    CultNetRudpSocketTransportOptions, CultNetWireContract, decode_cultnet_message_from_slice,
    decode_rudp_packet, encode_cultnet_message_to_vec, encode_rudp_packet,
};
use odin_core::{
    IdunnCommandBoundaryRecord, IdunnDaemonHealthRecord, IdunnDaemonSurgeryPlanRecord,
    IdunnDaemonTransportProfileRecord, IdunnDeploymentArtifactRecord, IdunnDeploymentResultRecord,
    IdunnDesiredDaemonRecord, IdunnOperatorAlarmRecord, IdunnReleaseTargetRecord,
    IdunnRestartResultRecord, IdunnRolloutPlanRecord, IdunnRolloutResultRecord,
    IdunnRudpHealthIngressRecord, IdunnRuntimeTransportCheckRecord, IdunnStateMigrationPlanRecord,
    IdunnStateMigrationResultRecord, IdunnSwarmSurgeryPlanRecord, OdinDocuments, plan_keepalive,
};
use std::collections::HashMap;
use std::env;
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const CULTNET_RUDP_PROTOCOL_ID: &str = "cultnet.transport.rudp.v0";
const IDUNN_HEALTH_RUDP_CONNECTION_ID: u32 = 0x1d0d_0001;

#[derive(Clone, Debug)]
struct DaemonTarget {
    daemon_id: String,
    verse_id: String,
    name: String,
    health_contract: HealthContract,
    health_command: Option<String>,
    deploy_command: Option<String>,
    restart_command: Option<String>,
    release: Option<ReleaseTarget>,
    enabled: bool,
    interval_seconds: u64,
}

#[derive(Clone, Debug)]
struct ReleaseTarget {
    repo: String,
    repo_path: PathBuf,
    upstream_remote: String,
    upstream_branch: String,
    rollout_strategy: String,
    state_migration_command: Option<String>,
    zero_downtime_capability: String,
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
    rudp_health_bind: Option<SocketAddr>,
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
            let now = timestamp()?;
            publish_runtime_transport_check(&options.common, &store_lock, &now)?;
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
    let now = timestamp()?;
    publish_runtime_transport_check(common, &store_lock, &now)?;
    start_rudp_health_ingress(common, &store_lock, &now)?;
    publish_surgery_plans(&options.profile, &targets, common, &store_lock, &now)?;

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
        transport_profile_id: transport_profile_id(target),
        command_boundary_id: command_boundary_id(target),
        authority: "idunn.local-command".to_string(),
        max_silence_seconds: 60,
        observed_at: now.clone(),
    };

    let (mut health_key, mut health) =
        evaluate_target_health(target, options, store_lock, &desired, &now)?;
    let mut plan = plan_keepalive(&desired, &health, now.clone());
    if plan.restart_request.is_some() {
        let veto_now = timestamp()?;
        if let Some(fresh_health) =
            read_fresh_daemon_published_health(options, store_lock, &desired, &veto_now)?
        {
            if health_state_is_healthy(&fresh_health.state) {
                health_key = desired.daemon_id.clone();
                health = fresh_health;
                plan = plan_keepalive(&desired, &health, veto_now);
            }
        }
    }

    with_store_node(options, store_lock, |node| {
        let transport_profile = daemon_transport_profile(target, &now);
        let command_boundary = command_boundary(target, &now);
        node.put(&transport_profile.profile_id, &transport_profile)?;
        node.put(&command_boundary.boundary_id, &command_boundary)?;
        node.put(&desired.daemon_id, &desired)?;
        node.put(&health_key, &health)?;
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
            let migration_result = target.release.as_ref().and_then(|release| {
                run_state_migration(target, release, &now, options.command_timeout_seconds)
            });
            if let Some(result) = &migration_result {
                with_store_node(options, store_lock, |node| {
                    node.put(&result.result_id, result)?;
                    Ok(())
                })?;
            }
            let migration_failed = migration_result
                .as_ref()
                .is_some_and(|result| result.state != "succeeded" && result.state != "noop");
            let result = if migration_failed {
                IdunnDeploymentResultRecord {
                    result_id: format!("result:{}", request.request_id),
                    request_id: request.request_id.clone(),
                    daemon_id: request.daemon_id.clone(),
                    state: "failed".to_string(),
                    detail: "state migration failed; deployment command was not run".to_string(),
                    completed_at: now.clone(),
                }
            } else {
                run_deployment(request, &now, options.command_timeout_seconds)
            };
            let rollout_result = target.release.as_ref().map(|release| {
                rollout_result_record(target, release, &result, migration_result.as_ref(), &now)
            });
            let alarm = if result.state != "succeeded" {
                Some(deployment_failure_alarm(&result, &now))
            } else {
                None
            };
            with_store_node(options, store_lock, |node| {
                node.put(&result.result_id, &result)?;
                if let Some(result) = &rollout_result {
                    node.put(&result.result_id, result)?;
                }
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

fn with_store_node<T, F>(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    write: F,
) -> Result<T>
where
    F: FnOnce(&mut CultMeshNode) -> Result<T>,
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

fn read_fresh_daemon_published_health(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    desired: &IdunnDesiredDaemonRecord,
    now: &str,
) -> Result<Option<IdunnDaemonHealthRecord>> {
    with_store_node(options, store_lock, |node| {
        let Some(health) = node.get::<IdunnDaemonHealthRecord>(&desired.daemon_id)? else {
            return Ok(None);
        };
        if is_fresh_daemon_published_health(&health, desired, now) {
            Ok(Some(health))
        } else {
            Ok(None)
        }
    })
}

fn evaluate_target_health(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    desired: &IdunnDesiredDaemonRecord,
    now: &str,
) -> Result<(String, IdunnDaemonHealthRecord)> {
    if let Some(health) = read_fresh_daemon_published_health(
        options,
        store_lock,
        desired,
        &timestamp()?,
    )? {
        return Ok((desired.daemon_id.clone(), health));
    }

    let probe = probe_health(target, options.command_timeout_seconds, now);

    if let Some(health) = read_fresh_daemon_published_health(
        options,
        store_lock,
        desired,
        &timestamp()?,
    )? {
        return Ok((desired.daemon_id.clone(), health));
    }

    Ok((compatibility_health_key(&desired.daemon_id), probe))
}

fn compatibility_health_key(daemon_id: &str) -> String {
    format!("compat-health:{daemon_id}")
}

fn health_state_is_healthy(state: &str) -> bool {
    matches!(state, "active" | "healthy" | "ok" | "running")
}

fn is_fresh_daemon_published_health(
    health: &IdunnDaemonHealthRecord,
    desired: &IdunnDesiredDaemonRecord,
    now: &str,
) -> bool {
    if health.daemon_id != desired.daemon_id {
        return false;
    }
    if health.health_contract != desired.health_contract {
        return false;
    }
    if health.publication_source != "daemon-published" {
        return false;
    }
    if health.transport != CULTNET_RUDP_PROTOCOL_ID {
        return false;
    }
    let Some(now_seconds) = parse_timestamp_seconds(now) else {
        return false;
    };
    let Some(observed_seconds) = parse_timestamp_seconds(&health.observed_at) else {
        return false;
    };
    if observed_seconds > now_seconds {
        return false;
    }
    now_seconds.saturating_sub(observed_seconds) <= u64::from(desired.max_silence_seconds)
}

fn parse_unix_timestamp(value: &str) -> Option<u64> {
    value.strip_prefix("unix:")?.parse().ok()
}

fn parse_timestamp_seconds(value: &str) -> Option<u64> {
    parse_unix_timestamp(value).or_else(|| parse_utc_iso_timestamp_seconds(value))
}

fn parse_utc_iso_timestamp_seconds(value: &str) -> Option<u64> {
    if value.len() < 19 {
        return None;
    }
    if !value.ends_with('Z') && !value.contains("+00:00") {
        return None;
    }

    let year = value.get(0..4)?.parse::<i32>().ok()?;
    let month = value.get(5..7)?.parse::<u32>().ok()?;
    let day = value.get(8..10)?.parse::<u32>().ok()?;
    let hour = value.get(11..13)?.parse::<u32>().ok()?;
    let minute = value.get(14..16)?.parse::<u32>().ok()?;
    let second = value.get(17..19)?.parse::<u32>().ok()?;
    if value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
        || value.as_bytes().get(10) != Some(&b'T')
        || value.as_bytes().get(13) != Some(&b':')
        || value.as_bytes().get(16) != Some(&b':')
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600)?
        .checked_add(i64::from(minute) * 60)?
        .checked_add(i64::from(second))?;
    u64::try_from(seconds).ok()
}

fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    let year = i64::from(year) - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    if !(0..=365).contains(&day_of_year) {
        return None;
    }
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn publish_runtime_transport_check(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    observed_at: &str,
) -> Result<()> {
    let check = runtime_transport_check(observed_at);
    with_store_node(options, store_lock, |node| {
        node.put(&check.check_id, &check)?;
        Ok(())
    })
}

fn runtime_transport_check(observed_at: &str) -> IdunnRuntimeTransportCheckRecord {
    match verify_rudp_loopback() {
        Ok(transfer_id) => IdunnRuntimeTransportCheckRecord {
            check_id: "idunn-runtime-rudp-loopback".to_string(),
            runtime_id: "idunn-daemon".to_string(),
            transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
            state: "available".to_string(),
            detail: format!("loopback CultNet RUDP message acknowledged as transfer {transfer_id}"),
            observed_at: observed_at.to_string(),
        },
        Err(error) => IdunnRuntimeTransportCheckRecord {
            check_id: "idunn-runtime-rudp-loopback".to_string(),
            runtime_id: "idunn-daemon".to_string(),
            transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
            state: "failed".to_string(),
            detail: format!("loopback CultNet RUDP check failed: {error}"),
            observed_at: observed_at.to_string(),
        },
    }
}

fn verify_rudp_loopback() -> Result<String> {
    let server_socket = UdpSocket::bind("127.0.0.1:0")?;
    server_socket.set_read_timeout(Some(Duration::from_millis(100)))?;
    let server_addr = server_socket.local_addr()?;
    let client_socket = UdpSocket::bind("127.0.0.1:0")?;
    client_socket.set_read_timeout(Some(Duration::from_millis(100)))?;

    let handle = thread::spawn(move || -> Result<()> {
        let mut server =
            CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::server(
                "idunn-rudp-loopback-server",
                server_socket,
                IDUNN_HEALTH_RUDP_CONNECTION_ID,
            ))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(frame) = server.receive_once()? {
                let message = decode_cultnet_message_from_slice(
                    &frame.payload,
                    CultNetWireContract::CultNetSchemaV0,
                )?;
                return match message {
                    CultNetMessage::Hello { runtime_id, .. } if runtime_id == "idunn-daemon" => {
                        Ok(())
                    }
                    other => Err(anyhow!("unexpected loopback CultNet message: {other:?}")),
                };
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "Idunn RUDP loopback timed out waiting for schema frame"
                ));
            }
        }
    });

    let mut client =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::client(
            "idunn-rudp-loopback-client",
            client_socket,
            server_addr,
            IDUNN_HEALTH_RUDP_CONNECTION_ID,
        ))?;
    client.connect(Vec::new())?;
    let deadline = Instant::now() + Duration::from_secs(2);
    while !client.connected() {
        let _ = client.receive_once()?;
        client.poll_resends()?;
        if Instant::now() >= deadline {
            return Err(anyhow!("Idunn RUDP loopback timed out during handshake"));
        }
    }
    let message = CultNetMessage::Hello {
        runtime_id: "idunn-daemon".to_string(),
        runtime_kind: "keepalive".to_string(),
        agent_id: None,
        role: Some("idunn.runtime-transport-self-check".to_string()),
        display_name: Some("Idunn RUDP self-check".to_string()),
        supported_document_types: Some(vec!["idunn.runtime_transport_check".to_string()]),
        supported_mutation_contracts: None,
        supported_message_versions: Some(vec!["cultnet.hello.v0".to_string()]),
        transport_profiles: Some(vec![client.profile.clone()]),
        supports_schema_catalog: Some(false),
    };
    let payload = encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)?;
    client.send("schema", payload)?;
    handle
        .join()
        .map_err(|_| anyhow!("Idunn RUDP loopback receiver thread panicked"))??;
    Ok(format!("connection:{IDUNN_HEALTH_RUDP_CONNECTION_ID:08x}"))
}

fn start_rudp_health_ingress(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    observed_at: &str,
) -> Result<()> {
    let Some(bind_address) = options.rudp_health_bind else {
        return Ok(());
    };

    let ingress_id = "idunn-rudp-health-ingress".to_string();
    let socket = match UdpSocket::bind(bind_address) {
        Ok(socket) => socket,
        Err(error) => {
            let failed = rudp_health_ingress_record(
                &ingress_id,
                bind_address,
                "failed",
                &format!("failed to bind RUDP health ingress: {error}"),
                observed_at,
            );
            with_store_node(options, store_lock, |node| {
                node.put(&failed.ingress_id, &failed)?;
                Ok(())
            })?;
            return Err(error.into());
        }
    };
    let local_addr = socket.local_addr()?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;

    let active = rudp_health_ingress_record(
        &ingress_id,
        local_addr,
        "active",
        "listening for idunn.daemon_health document frames over CultNet RUDP schema channel",
        observed_at,
    );
    with_store_node(options, store_lock, |node| {
        node.put(&active.ingress_id, &active)?;
        Ok(())
    })?;

    let worker_options = options.clone();
    let worker_store_lock = Arc::clone(store_lock);
    thread::spawn(move || {
        if let Err(error) = run_rudp_health_ingress_loop(socket, worker_options, worker_store_lock)
        {
            eprintln!("Idunn RUDP health ingress stopped: {error}");
        }
    });
    Ok(())
}

fn rudp_health_ingress_record(
    ingress_id: &str,
    bind_address: SocketAddr,
    state: &str,
    detail: &str,
    observed_at: &str,
) -> IdunnRudpHealthIngressRecord {
    IdunnRudpHealthIngressRecord {
        ingress_id: ingress_id.to_string(),
        bind_address: bind_address.to_string(),
        transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
        accepted_schema: "idunn.daemon_health".to_string(),
        state: state.to_string(),
        detail: detail.to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn run_rudp_health_ingress_loop(
    socket: UdpSocket,
    options: CommonOptions,
    store_lock: Arc<Mutex<()>>,
) -> Result<()> {
    let local_addr = socket.local_addr()?;
    let mut sessions: HashMap<SocketAddr, CultNetRudpSession> = HashMap::new();
    let mut buffer = vec![0_u8; 65_535];
    let trace_ingress = env::var("IDUNN_RUDP_HEALTH_TRACE")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    loop {
        match socket.recv_from(&mut buffer) {
            Ok((size, source)) => {
                let observed_at = timestamp()?;
                if trace_ingress {
                    println!("Idunn RUDP health ingress received {size} bytes from {source}.");
                }
                match handle_rudp_health_datagram(
                    &socket,
                    &mut sessions,
                    &buffer[..size],
                    source,
                    trace_ingress,
                ) {
                    Ok(frames) => {
                        for frame in frames {
                            let message = match decode_cultnet_message_from_slice(
                                &frame.payload,
                                CultNetWireContract::CultNetSchemaV0,
                            ) {
                                Ok(message) => message,
                                Err(error) => {
                                    if trace_ingress {
                                        println!(
                                            "Idunn RUDP health ingress rejected schema bytes from {source}: {error}"
                                        );
                                    }
                                    publish_rudp_ingress_failure(
                                        &options,
                                        &store_lock,
                                        local_addr,
                                        &format!(
                                            "rejected RUDP health schema bytes from {source}: {error}"
                                        ),
                                        &observed_at,
                                    );
                                    continue;
                                }
                            };
                            match health_from_rudp_message(&message) {
                                Ok(mut health) => {
                                    health.publication_source = "daemon-published".to_string();
                                    health.transport = CULTNET_RUDP_PROTOCOL_ID.to_string();
                                    if let Err(error) =
                                        with_store_node(&options, &store_lock, |node| {
                                            node.put(&health.daemon_id, &health)?;
                                            Ok(())
                                        })
                                    {
                                        eprintln!(
                                            "Idunn RUDP health ingress failed to persist {} from {}: {}",
                                            health.daemon_id, source, error
                                        );
                                    } else {
                                        println!(
                                            "Idunn accepted RUDP health for {} from {} over {}.",
                                            health.daemon_id, source, frame.channel_id
                                        );
                                    }
                                }
                                Err(error) => {
                                    if trace_ingress {
                                        println!(
                                            "Idunn RUDP health ingress rejected schema frame from {source}: {error}"
                                        );
                                    }
                                    publish_rudp_ingress_failure(
                                        &options,
                                        &store_lock,
                                        local_addr,
                                        &format!(
                                            "rejected RUDP health schema frame from {source}: {error}"
                                        ),
                                        &observed_at,
                                    );
                                }
                            }
                        }
                    }
                    Err(error) => {
                        if trace_ingress {
                            println!(
                                "Idunn RUDP health ingress rejected datagram from {source}: {error}"
                            );
                        }
                        publish_rudp_ingress_failure(
                            &options,
                            &store_lock,
                            local_addr,
                            &format!("rejected RUDP health datagram from {source}: {error}"),
                            &observed_at,
                        );
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
}

fn publish_rudp_ingress_failure(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    local_addr: SocketAddr,
    detail: &str,
    observed_at: &str,
) {
    let failed = rudp_health_ingress_record(
        "idunn-rudp-health-ingress",
        local_addr,
        "degraded",
        detail,
        observed_at,
    );
    if let Err(error) = with_store_node(options, store_lock, |node| {
        node.put(&failed.ingress_id, &failed)?;
        Ok(())
    }) {
        eprintln!("Idunn RUDP health ingress failed to persist ingress failure: {error}");
    }
}

fn handle_rudp_health_datagram(
    socket: &UdpSocket,
    sessions: &mut HashMap<SocketAddr, CultNetRudpSession>,
    wire: &[u8],
    source: SocketAddr,
    trace_ingress: bool,
) -> Result<Vec<cultnet_rs::CultNetTransportFrame>> {
    let packet = decode_rudp_packet(wire)?;
    if packet.connection_id != IDUNN_HEALTH_RUDP_CONNECTION_ID {
        return Err(anyhow!(
            "unexpected RUDP connection id {:08x}",
            packet.connection_id
        ));
    }

    let now = unix_epoch_millis()?;

    if packet.packet_type == CultNetRudpPacketType::Connect {
        let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
            connection_id: IDUNN_HEALTH_RUDP_CONNECTION_ID,
            initial_sequence: 1,
            resend_delay_ms: 100,
            media_reliable_expire_after_ms: None,
            max_pending_reliable_packets: None,
        });
        let accept = session.accept_connect(&packet, now, Vec::new())?;
        let accept_wire = encode_rudp_packet(&accept)?;
        socket.send_to(&accept_wire, source)?;
        if trace_ingress {
            println!(
                "Idunn RUDP health ingress accepted connect from {source} with sequence {}.",
                packet.sequence
            );
        }
        sessions.insert(source, session);
        return Ok(Vec::new());
    }

    let session = sessions.entry(source).or_insert_with(|| {
        CultNetRudpSession::new(CultNetRudpSessionOptions {
            connection_id: IDUNN_HEALTH_RUDP_CONNECTION_ID,
            initial_sequence: 1,
            resend_delay_ms: 100,
            media_reliable_expire_after_ms: None,
            max_pending_reliable_packets: None,
        })
    });

    let result = session.receive(&packet, now)?;
    if trace_ingress && packet.packet_type == CultNetRudpPacketType::Data {
        println!(
            "Idunn RUDP health ingress data from {source}: sequence {} delivered {} frame(s).",
            packet.sequence,
            result.delivered.len()
        );
    }
    if let Some(reply) = result.reply {
        socket.send_to(&encode_rudp_packet(&reply)?, source)?;
    }
    let frames = result
        .delivered
        .into_iter()
        .map(|frame| cultnet_rs::CultNetTransportFrame {
            channel_id: frame.channel_id,
            payload: frame.payload,
        })
        .collect::<Vec<_>>();
    if packet.packet_type == CultNetRudpPacketType::Accept || !frames.is_empty() {
        let ack = session.create_ack();
        socket.send_to(&encode_rudp_packet(&ack)?, source)?;
    }
    if result.disconnected || !frames.is_empty() {
        sessions.remove(&source);
    }
    Ok(frames)
}

fn health_from_rudp_message(message: &CultNetMessage) -> Result<IdunnDaemonHealthRecord> {
    let CultNetMessage::DocumentPutRaw { document, .. } = message else {
        return Err(anyhow!("expected cultnet.document_put_raw.v0"));
    };
    if document.schema_id != "idunn.daemon_health" {
        return Err(anyhow!(
            "expected idunn.daemon_health schema, received {}",
            document.schema_id
        ));
    }
    if document.payload_encoding != CultNetRawPayloadEncoding::Messagepack {
        return Err(anyhow!("expected MessagePack raw payload encoding"));
    }
    let health: IdunnDaemonHealthRecord = rmp_serde::from_slice(&document.payload)?;
    if document.record_key != health.daemon_id {
        return Err(anyhow!(
            "record key {} does not match health daemon_id {}",
            document.record_key,
            health.daemon_id
        ));
    }
    Ok(health)
}

fn publish_surgery_plans(
    profile: &str,
    targets: &[DaemonTarget],
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    updated_at: &str,
) -> Result<()> {
    let plans = daemon_surgery_plans(targets, updated_at);
    let swarm_plan = swarm_surgery_plan(profile, targets, &plans, updated_at);
    with_store_node(options, store_lock, |node| {
        node.put(&swarm_plan.plan_id, &swarm_plan)?;
        for target in targets {
            let transport_profile = daemon_transport_profile(target, updated_at);
            let command_boundary = command_boundary(target, updated_at);
            node.put(&transport_profile.profile_id, &transport_profile)?;
            node.put(&command_boundary.boundary_id, &command_boundary)?;
            if let Some(release) = &target.release {
                let release_record = release_target_record(target, release, updated_at);
                let artifact = deployment_artifact_record(target, &release_record, updated_at);
                let migration_plan =
                    state_migration_plan_record(target, release, &release_record, updated_at);
                let rollout_plan = rollout_plan_record(
                    target,
                    release,
                    &release_record,
                    &artifact,
                    &migration_plan,
                    updated_at,
                );
                node.put(&release_record.target_id, &release_record)?;
                node.put(&artifact.artifact_id, &artifact)?;
                node.put(&migration_plan.plan_id, &migration_plan)?;
                node.put(&rollout_plan.plan_id, &rollout_plan)?;
            }
        }
        for plan in &plans {
            node.put(&plan.plan_id, plan)?;
        }
        Ok(())
    })
}

fn swarm_surgery_plan(
    profile: &str,
    targets: &[DaemonTarget],
    plans: &[IdunnDaemonSurgeryPlanRecord],
    updated_at: &str,
) -> IdunnSwarmSurgeryPlanRecord {
    let next_target = preferred_next_target(targets, plans).unwrap_or("none");

    IdunnSwarmSurgeryPlanRecord {
        plan_id: format!("swarm-surgery:{profile}"),
        profile: profile.to_string(),
        status: "active-transport-migration".to_string(),
        owner: "Idunn swarm supervisor".to_string(),
        objective:
            "Move daemon awareness from compatibility probes to daemon-published typed CultNet/RUDP state."
                .to_string(),
        current_mechanism:
            "Idunn publishes per-daemon desired state, surgery plans, transport profiles, command boundaries, runtime RUDP self-checks, and RUDP health ingress; compatibility command probes remain fallback evidence until each daemon publishes its own health."
                .to_string(),
        invariants: vec![
            "Daemon truth is typed CultCache/CultMesh state carried over cultnet.transport.rudp.v0.".to_string(),
            "Compatibility commands, HTTP, WebSocket, SSH, and systemd probes are evidence only and must not own daemon health.".to_string(),
            "Idunn consumes daemon-published RUDP health before compatibility probes and actuates only advertised lifecycle authority.".to_string(),
            "Each migrated daemon must publish the same health contract that Idunn expects for its target.".to_string(),
            "Shared operator hosts such as Raven must be actuated by background-only launch paths that do not create visible terminal or interactive windows.".to_string(),
            "Raven Task Scheduler actions must execute hidden launchers directly, not visible .cmd trampolines.".to_string(),
        ],
        phases: vec![
            "1. Publish Idunn's own RUDP substrate and ingress state.".to_string(),
            "2. Install daemon-published RUDP health in one Rust daemon and prove Idunn consumes it live.".to_string(),
            "3. Extend CultLib RUDP publication support across TypeScript, C#, and remaining daemon runtimes.".to_string(),
            "4. Promote provider advertisements, command boundaries, and transport profiles to daemon-owned CultNet/RUDP records.".to_string(),
            "5. Delete or demote compatibility probes once every target has daemon-owned publication and advertised lifecycle authority.".to_string(),
        ],
        current_phase:
            "Phase 25: Nightwing Gjallar now consumes Odin's accepted gamecult.eve.surface_state snapshot over CultNet/RUDP, so every active Starfire-local daemon target publishes daemon-owned RUDP health and typed boundary state; the remaining debt is demoting compatibility probes and bridge-only lowerings without letting them retake ownership."
                .to_string(),
        next_target: next_target.to_string(),
        cut_line:
            "Muninn, Vili, Idunn, Odin, Stonks, Weksa, VoidBot, Nightwing Gjallar, Mimir Eve dashboard, Nightwing Eve dashboard, Nightwing Eve browser reference, yggdrasil-streampixels, yggdrasil-heimdall, and yggdrasil-repixelizer now exercise daemon-owned RUDP health. Raven GameCult\\Vili has been refreshed from Odin, the hidden scheduled task now launches start-vili-daemon.ps1 with Idunn RUDP arguments, and live Idunn accepts vili.cultnet-rudp-animation-health from 10.77.0.4. VoidBot now publishes a daemon-owned provider catalog, provider advertisements, command_boundary, and transport_profile records from E:\\Projects\\VoidBot\\.voidbot\\status\\cultmesh\\voidbot-swarm-state.cc while health-voidbot.cmd is demoted to a fast compatibility witness. Raven Muninn now publishes provider advertisement, command_boundary, and transport_profile records from C:\\Meta\\Odin\\state\\muninn.telemetry.cc, keeps activation commands in C:\\Meta\\Odin\\state\\muninn.activate.cc, and no longer lets plain serve infer ambient Move runtime authority from platform defaults. Starfire Muninn now publishes the same daemon-owned witness records from C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc, Nightwing Muninn now publishes the same daemon-owned witness records from /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc, and live Idunn accepts muninn from 10.77.0.4, starfire-muninn from 127.0.0.1, and nightwing-muninn from 10.77.0.3. Nightwing's restart wrapper now makes Move runtime authority explicit with -DiscoverMoveState -ClaimUsbMoves instead of silently adding Move evidence on plain serve. Raven's GameCult-Muninn, GameCult-Muninn-Activate, and GameCult-Muninn-VideoProof tasks now execute wscript.exe actions whose hidden VBS launchers call noninteractive hidden PowerShell entrypoints directly; .cmd wrappers, where present, are manual compatibility entrypoints only. Live yggdrasil-streampixels publishes streampixels.cultnet-rudp-service-health from 10.77.0.1, live yggdrasil-heimdall publishes heimdall.cultnet-rudp-provider-health from 10.77.0.1 with a boundary store at /srv/heimdall/cultcache/heimdall.service.cc, live yggdrasil-repixelizer publishes repixelizer.cultnet-rudp-service-health from 10.77.0.1 with a boundary store at /srv/repixelizer/cultcache/repixelizer.service.cc, live Nightwing Gjallar publishes a boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc plus a daemon-owned gamecult.eve.surface_state witness, live Gjallar now consumes Odin's accepted surface:gamecult.network.status snapshot over CultNet/RUDP from 192.168.1.66:17871, live Odin now ingests remote Raven Vili and Nightwing Gjallar witness stores into the accepted provider catalog, and /eve/deck/nightwing-gjallar now proxies a renderer-only lowering of the live remote store. Live Mimir Eve dashboard now publishes CultMesh state at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp plus a boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc while health-nightwing-eve-dashboard.ps1 inspects those witnesses directly instead of trusting /health for daemon truth, and live Nightwing Eve browser reference now publishes a boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc while health-nightwing-eve-browser-reference.ps1 inspects that store directly instead of trusting /health for daemon truth. The remaining debt is demoting compatibility probes and bridge-only lowerings without letting them retake daemon truth."
                .to_string(),
        verification_layer:
            "CultMesh keepalive store records plus live Idunn decision cycles, not process exit codes or chat summaries."
                .to_string(),
        updated_at: updated_at.to_string(),
    }
}

fn preferred_next_target<'a>(
    targets: &'a [DaemonTarget],
    plans: &'a [IdunnDaemonSurgeryPlanRecord],
) -> Option<&'a str> {
    let plan_by_daemon = plans
        .iter()
        .map(|plan| (plan.daemon_id.as_str(), plan.status.as_str()))
        .collect::<std::collections::HashMap<_, _>>();
    let priority = |status: &str| match status {
        "transport-surgery-required" => 0,
        "partial-rudp-health-live" => 1,
        "partial-rudp-health-and-provider-store-live" => 2,
        "catalog-coherence-probe" => 3,
        _ => 4,
    };
    targets
        .iter()
        .filter(|target| target.enabled)
        .filter(|target| target.daemon_id != "idunn-swarm-deployment-coverage")
        .filter_map(|target| {
            let status = *plan_by_daemon.get(target.daemon_id.as_str())?;
            Some((target.daemon_id.as_str(), status, priority(status)))
        })
        .filter(|(_, status, _)| *status != "partial-rudp-health-and-provider-store-live")
        .min_by_key(|(_, _, rank)| *rank)
        .map(|(daemon_id, _, _)| daemon_id)
}

fn transport_profile_id(target: &DaemonTarget) -> String {
    format!("transport:{}", target.daemon_id)
}

fn command_boundary_id(target: &DaemonTarget) -> String {
    format!("command-boundary:{}", target.daemon_id)
}

fn release_target_id(target: &DaemonTarget) -> String {
    format!("release-target:{}", target.daemon_id)
}

fn rollout_plan_id(target: &DaemonTarget) -> String {
    format!("rollout:{}:main", target.daemon_id)
}

fn migration_plan_id(target: &DaemonTarget) -> String {
    format!("migration:{}:main", target.daemon_id)
}

fn artifact_id(target: &DaemonTarget) -> String {
    format!("artifact:{}:main", target.daemon_id)
}

fn release_target(
    repo: &str,
    repo_path: PathBuf,
    rollout_strategy: &str,
    state_migration_command: Option<&str>,
    zero_downtime_capability: &str,
) -> ReleaseTarget {
    ReleaseTarget {
        repo: repo.to_string(),
        repo_path,
        upstream_remote: "origin".to_string(),
        upstream_branch: "main".to_string(),
        rollout_strategy: rollout_strategy.to_string(),
        state_migration_command: state_migration_command.map(ToString::to_string),
        zero_downtime_capability: zero_downtime_capability.to_string(),
    }
}

fn release_target_record(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    observed_at: &str,
) -> IdunnReleaseTargetRecord {
    let desired_revision = git_revision(
        &release.repo_path,
        &format!("{}/{}", release.upstream_remote, release.upstream_branch),
    )
    .unwrap_or_else(|| "unknown".to_string());

    IdunnReleaseTargetRecord {
        target_id: release_target_id(target),
        daemon_id: target.daemon_id.clone(),
        repo: release.repo.clone(),
        repo_path: release.repo_path.display().to_string(),
        upstream_remote: release.upstream_remote.clone(),
        upstream_branch: release.upstream_branch.clone(),
        desired_revision,
        deployed_revision: "from-deployment-manifest".to_string(),
        artifact_strategy: "source-archive-from-upstream-main".to_string(),
        rollout_strategy: release.rollout_strategy.clone(),
        state_migration_authority: if release.state_migration_command.is_some() {
            "daemon-owned-command"
        } else {
            "daemon-declares-no-migration-required"
        }
        .to_string(),
        zero_downtime_capability: release.zero_downtime_capability.clone(),
        status: "tracked".to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn deployment_artifact_record(
    target: &DaemonTarget,
    release: &IdunnReleaseTargetRecord,
    built_at: &str,
) -> IdunnDeploymentArtifactRecord {
    IdunnDeploymentArtifactRecord {
        artifact_id: artifact_id(target),
        daemon_id: target.daemon_id.clone(),
        source_revision: release.desired_revision.clone(),
        source_branch: release.upstream_branch.clone(),
        source_remote: release.upstream_remote.clone(),
        artifact_kind: release.artifact_strategy.clone(),
        artifact_uri: "built-by-deploy-command".to_string(),
        sha256: "pending-deploy-command".to_string(),
        built_at: built_at.to_string(),
    }
}

fn state_migration_plan_record(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    _release_record: &IdunnReleaseTargetRecord,
    planned_at: &str,
) -> IdunnStateMigrationPlanRecord {
    let command = release
        .state_migration_command
        .clone()
        .unwrap_or_else(|| "none".to_string());
    IdunnStateMigrationPlanRecord {
        plan_id: migration_plan_id(target),
        daemon_id: target.daemon_id.clone(),
        from_schema_version: "deployed-state".to_string(),
        to_schema_version: "target-revision-state".to_string(),
        authority: if release.state_migration_command.is_some() {
            "daemon-owned-migrator"
        } else {
            "daemon-declared-noop"
        }
        .to_string(),
        command,
        strategy: if release.state_migration_command.is_some() {
            "backup-then-daemon-migrator"
        } else {
            "noop"
        }
        .to_string(),
        backup_required: release.state_migration_command.is_some(),
        zero_downtime_required: release.zero_downtime_capability == "zero-downtime",
        status: "planned".to_string(),
        planned_at: planned_at.to_string(),
    }
}

fn rollout_plan_record(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    release_record: &IdunnReleaseTargetRecord,
    artifact: &IdunnDeploymentArtifactRecord,
    migration_plan: &IdunnStateMigrationPlanRecord,
    planned_at: &str,
) -> IdunnRolloutPlanRecord {
    let mut phases = vec![
        "fetch upstream main into a managed release view".to_string(),
        "build source artifact from the desired upstream revision".to_string(),
        "snapshot state before any non-noop migration".to_string(),
        "run daemon-owned migration command when declared".to_string(),
        "deploy through the advertised Idunn command boundary".to_string(),
        "verify daemon-published RUDP health and deployment manifest".to_string(),
    ];
    if release.zero_downtime_capability == "zero-downtime" {
        phases.insert(
            4,
            "shift traffic or reload in place without dropping the active service".to_string(),
        );
    } else {
        phases.insert(
            4,
            "record restart-required because this daemon has no zero-downtime swap authority"
                .to_string(),
        );
    }

    IdunnRolloutPlanRecord {
        plan_id: rollout_plan_id(target),
        daemon_id: target.daemon_id.clone(),
        desired_revision: release_record.desired_revision.clone(),
        deployed_revision: release_record.deployed_revision.clone(),
        strategy: release.rollout_strategy.clone(),
        phases,
        migration_plan_id: migration_plan.plan_id.clone(),
        artifact_id: artifact.artifact_id.clone(),
        status: "planned".to_string(),
        planned_at: planned_at.to_string(),
    }
}

fn git_revision(repo_path: &PathBuf, rev: &str) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("rev-parse")
        .arg(rev)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn daemon_transport_profile(
    target: &DaemonTarget,
    observed_at: &str,
) -> IdunnDaemonTransportProfileRecord {
    let (current_transport, state, cut_line) = match target.daemon_id.as_str() {
        "stonks" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-provider-store + daemon-owned-rudp-command-ingress",
            "partial-rudp-health-and-provider-store-live",
            "Stonks daemon health is published over CultNet/RUDP, and provider advertisement, market snapshot, Eve surface, command_boundary, and transport_profile records are in the daemon-owned CultCache store; HTTP/WebSocket are renderer/debug lowerings.",
        ),
        "weksa" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-provider-store + daemon-owned-rudp-command-ingress",
            "partial-rudp-health-and-provider-store-live",
            "Weksa daemon health is published over CultNet/RUDP, provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records are in the daemon-owned CultCache store, and speech_provider.mimo.voicedesign now arrives over CultNet/RUDP as weksa.mimo_voicedesign_command.v0 instead of an HTTP route.",
        ),
        "voidbot" => (
            "daemon-published-rudp-health + daemon-owned-cultmesh-provider-store + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "VoidBot stack health is published over CultNet/RUDP from the local orchestrator pulse, and the daemon-owned CultMesh witness store at E:\\Projects\\VoidBot\\.voidbot\\status\\cultmesh\\voidbot-swarm-state.cc now carries provider advertisement catalog, provider advertisement, command_boundary, and transport_profile records. The operations probe remains fallback evidence only.",
        ),
        "nightwing-gjallar" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary + native-odin-cultnet-rudp-snapshot-input + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "Gjallar framebuffer composition health is published over CultNet/RUDP from Nightwing, the daemon-owned boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc carries provider advertisement, runtime_config, frame_status, gamecult.eve.surface_state, command_boundary, transport_profile, and daemon-health summary state, and live composition now consumes Odin's accepted surface:gamecult.network.status snapshot over CultNet/RUDP. The service/status probe is fallback evidence only.",
        ),
        "nightwing-eve-dashboard" => (
            "daemon-published-rudp-health + daemon-owned-cultmesh-state + daemon-owned-cultcache-boundary-store + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "Nightwing Eve dashboard service health is published over CultNet/RUDP from the Mimir.EveDashboard systemd process, and the live broker now publishes CultMesh state at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp plus a daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc carrying provider manifest, command_boundary, transport_profile, and daemon-health summary state. HTTP/WebSocket remain client lowerings, and health-nightwing-eve-dashboard.ps1 now inspects the typed witnesses directly while the .cmd wrapper stays a compatibility entrypoint only.",
        ),
        "nightwing-eve-browser-reference" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-boundary-store + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "Nightwing Eve browser reference health is published over CultNet/RUDP from the Mimir.EveBrowserReference service process, and the runtime now publishes a daemon-owned boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc carrying manifest, static-surface, command_boundary, transport_profile, and daemon-health summary state. HTTP remains a browser lowering, and health-nightwing-eve-browser-reference.ps1 now inspects the daemon-owned witness directly while the .cmd wrapper stays a compatibility entrypoint only.",
        ),
        "muninn" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store + background-only hidden task launch + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "Raven Muninn now publishes muninn.cultnet-rudp-remote-telemetry-health over CultNet/RUDP from the long-running hidden GameCult-Muninn serve task, and the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, and telemetry surface records. HTTP-free background launch remains an ops invariant and the command probe is fallback evidence only.",
        ),
        "starfire-muninn" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "Starfire Muninn now publishes muninn.cultnet-rudp-local-telemetry-and-quest-access over CultNet/RUDP from the long-running local serve process, and the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Quest access, and telemetry surface records. Quest ADB availability stays telemetry state, not daemon liveness.",
        ),
        "nightwing-muninn" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store + compatibility.local-command fallback",
            "partial-rudp-health-and-provider-store-live",
            "Nightwing Muninn now publishes muninn.cultnet-rudp-remote-telemetry-and-move-hid over CultNet/RUDP from the long-running serve process, and the daemon-owned telemetry store at /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Move HID evidence, and telemetry surface records.",
        ),
        "vili" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary + compatibility.http-websocket fallback",
            "partial-rudp-health-and-provider-store-live",
            "Vili now publishes vili.cultnet-rudp-animation-health over CultNet/RUDP from the hidden Raven GameCult\\Vili task, and its daemon-owned vili.service.cc store contains provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records; HTTP and WebSocket remain operator/debug lowerings.",
        ),
        "yggdrasil-streampixels" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary + compatibility.ssh-systemd-http fallback",
            "partial-rudp-health-and-provider-store-live",
            "StreamPixels now publishes daemon-owned RUDP health and a daemon-owned CultCache boundary from the live Yggdrasil service runtime; Idunn accepts yggdrasil-streampixels from 10.77.0.1 and the boundary store at /srv/streampixels/app/.streampixels-data/cultcache/streampixels.service.cc remains the service-owned witness behind the compatibility checks.",
        ),
        "yggdrasil-heimdall" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-boundary-store + compatibility.ssh-systemd-http fallback",
            "partial-rudp-health-and-provider-store-live",
            "Heimdall now publishes daemon-owned Idunn health over CultNet/RUDP from the live Yggdrasil service runtime, and its daemon-owned boundary store at /srv/heimdall/cultcache/heimdall.service.cc carries provider advertisement, command_boundary, transport_profile, and daemon-health summary state. /healthz, JWKS, discovery, systemd, and nginx remain compatibility witnesses.",
        ),
        "yggdrasil-repixelizer" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-boundary-store + compatibility.ssh-systemd-http fallback",
            "partial-rudp-health-and-provider-store-live",
            "Repixelizer now publishes daemon-owned Idunn health over CultNet/RUDP from the live Yggdrasil GUI runtime, and its daemon-owned boundary store at /srv/repixelizer/cultcache/repixelizer.service.cc carries provider advertisement, Eve surface state, queue/auth/runtime projection, command_boundary, transport_profile, and daemon-health summary state. /api/health, /api/config, systemd, and nginx remain compatibility witnesses.",
        ),
        _ => (
            "compatibility.local-command",
            "migration-required",
            "Compatibility command probes are evidence only; daemon truth moves to CultNet/RUDP health publication and advertised command boundaries.",
        ),
    };
    IdunnDaemonTransportProfileRecord {
        profile_id: transport_profile_id(target),
        daemon_id: target.daemon_id.clone(),
        target_transport: "cultnet.transport.rudp.v0".to_string(),
        current_transport: current_transport.to_string(),
        state: state.to_string(),
        health_contract: target.health_contract.id.clone(),
        publication_schema: "idunn.daemon_health.v1".to_string(),
        compatibility_mechanism: target
            .health_command
            .clone()
            .unwrap_or_else(|| "none".to_string()),
        cut_line: cut_line.to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn command_boundary(target: &DaemonTarget, observed_at: &str) -> IdunnCommandBoundaryRecord {
    let restart_authority = target
        .restart_command
        .as_ref()
        .map(|_| "idunn.local-command.restart")
        .unwrap_or("none")
        .to_string();
    let deploy_authority = target
        .deploy_command
        .as_ref()
        .map(|_| "idunn.local-command.deploy")
        .unwrap_or("none")
        .to_string();
    let compatibility_commands = [
        target.health_command.as_ref(),
        target.restart_command.as_ref(),
        target.deploy_command.as_ref(),
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();

    IdunnCommandBoundaryRecord {
        boundary_id: command_boundary_id(target),
        daemon_id: target.daemon_id.clone(),
        owner: "idunn.local-command-compatibility".to_string(),
        restart_authority,
        deploy_authority,
        health_authority: "compatibility.probe-only".to_string(),
        alarm_authority: "bifrost.operator-notification".to_string(),
        compatibility_commands,
        forbidden_authority:
            "Health commands, HTTP endpoints, WebSocket decks, SSH probes, and systemd status do not own daemon truth."
                .to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn daemon_surgery_plans(
    targets: &[DaemonTarget],
    updated_at: &str,
) -> Vec<IdunnDaemonSurgeryPlanRecord> {
    targets
        .iter()
        .map(|target| daemon_surgery_plan(target, updated_at))
        .collect()
}

fn daemon_surgery_plan(target: &DaemonTarget, updated_at: &str) -> IdunnDaemonSurgeryPlanRecord {
    let mut severity = "high";
    let mut status = "transport-surgery-required";
    let mut owner = "daemon-owned CultLib update";
    let mut objective = format!(
        "{} publishes daemon-owned health, provider state, command boundary, and transport profile through CultNet/RUDP.",
        target.name
    );
    let mut current_mechanism = format!(
        "Idunn target {} currently uses local compatibility health command {:?} under contract {}.",
        target.daemon_id, target.health_command, target.health_contract.id
    );
    let mut intended_authority = "Daemon publishes typed CultMesh/CultNet documents over cultnet.transport.rudp.v0; Idunn consumes those records and only actuates advertised lifecycle commands.".to_string();
    let mut cut_line = "Cut HTTP, WebSocket, SSH, systemd, and command-exit probes as sources of daemon truth once the daemon's CultLib can publish the RUDP health contract.".to_string();
    let mut steps = vec![
        "Update the daemon's runtime CultLib dependency to a build that can speak CultNet over RUDP.".to_string(),
        "Publish daemon_health, provider_advertisement, command_boundary, and transport_profile typed records over cultnet.transport.rudp.v0.".to_string(),
        "Teach Odin to accept the daemon's RUDP provider records into the service/catalog surface.".to_string(),
        "Switch Idunn from the compatibility health command to the daemon-owned RUDP health record.".to_string(),
        "Delete or demote the old probe to a xenos-boundary compatibility check with no lifecycle authority.".to_string(),
    ];
    let blockers = Vec::new();

    match target.daemon_id.as_str() {
        "odin" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "critical";
            owner = "Odin core";
            current_mechanism =
                "Odin now ingests remote Raven Vili, Raven Muninn, Starfire Muninn, Nightwing Muninn, and Nightwing Gjallar witness stores into the accepted provider catalog while still exposing compatibility HTTP/WebSocket lowerings and relying on xenos-boundary hashed-store inspection for C# witness bodies."
                    .to_string();
            intended_authority =
                "Odin owns accepted Verse discovery and provider catalog truth as typed CultMesh/CultNet records over cultnet.transport.rudp.v0."
                    .to_string();
            cut_line =
                "HTTP/WebSocket deck and health endpoints become browser/lowering bridges only; Odin's remaining compatibility debt is deleting hashed-store inspection shims once remote daemon runtimes publish directly interoperable provider surfaces and records."
                    .to_string();
            steps[2] =
                "Make Odin ingest RUDP provider advertisements directly into its accepted service/catalog surface."
                    .to_string();
        }
        "nightwing-gjallar" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "critical";
            owner = "Gjallar C# runtime plus Odin accepted snapshot surface";
            current_mechanism =
                "Nightwing Gjallar publishes gjallar.cultnet-rudp-framebuffer-composition-health over CultNet/RUDP from the C# runtime, the daemon-owned boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc now contains provider advertisement, gamecult.eve.surface_state, runtime_config, frame_status, command_boundary, transport_profile, and daemon-health summary records, and the live compositor now polls Odin's accepted surface:gamecult.network.status snapshot over cultnet.transport.rudp.v0 from 192.168.1.66:17871. Live Odin ingests that witness store and /eve/deck/nightwing-gjallar remains a renderer-only lowering."
                    .to_string();
            intended_authority =
                "Gjallar consumes Odin's accepted provider surface snapshot over CultNet/RUDP and publishes framebuffer composition health, provider advertisement, provider-owned Eve surface, runtime config, command boundary, and transport profile as typed CultMesh/CultNet state."
                    .to_string();
            cut_line =
                "Keep the service/status probe as fallback only; the daemon-owned witness now owns provider advertisement, gamecult.eve.surface_state, command_boundary, and transport_profile truth, live input now comes from Odin's accepted CultNet/RUDP snapshot surface, and live Gjallar daemon health now keys off snapshot freshness through receive.status, lastAttemptStatus, lastSuccessfulAtUtc, staleAfterSeconds, and consecutiveFailures so retained panels cannot masquerade as healthy after input goes stale."
                    .to_string();
            steps = vec![
                "Keep live gjallar.cultnet-rudp-framebuffer-composition-health publication running from Nightwing's Gjallar service.".to_string(),
                "Keep Gjallar's daemon-owned boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc publishing provider advertisement, gamecult.eve.surface_state, runtime_config, frame_status, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                "Keep the native C# CultNet/RUDP snapshot polling path pointed at Odin's accepted surface:gamecult.network.status surface on 192.168.1.66:17871.".to_string(),
                "Keep Gjallar's receive-freshness daemon health live on Nightwing so daemon-published health degrades when Odin snapshot success goes stale instead of keying only off rendered frames.".to_string(),
                "Keep /eve/deck/nightwing-gjallar as a renderer-only lowering; it must not become Gjallar's live input path again.".to_string(),
                "Keep the .ps1 lifecycle bodies authoritative and leave the .cmd wrappers as manual compatibility entrypoints only.".to_string(),
            ];
        }
        "stonks" => {
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Stonks TypeScript runtime";
            current_mechanism =
                "Stonks publishes daemon health over CultNet/RUDP after each serialized market refresh, and its daemon-owned CultCache store contains provider advertisement, market snapshot, Eve surface, command_boundary, and transport_profile records that Odin can ingest. HTTP and WebSocket remain renderer/debug lowerings."
                    .to_string();
            intended_authority =
                "Stonks publishes daemon health, provider advertisement, market snapshot, Eve surface, command boundary, and transport profile as typed CultMesh/CultNet records over cultnet.transport.rudp.v0; HTTP/WebSocket remain renderer/debug lowerings."
                    .to_string();
            cut_line =
                "Keep HTTP/WebSocket as lowerings over Stonks CultCache records; demote health-stonks.cmd to a manual compatibility probe with no lifecycle truth once Idunn and Odin consistently consume the typed store."
                    .to_string();
            steps = vec![
                "Keep live stonks.cultnet-rudp-market-health publication running from the Stonks daemon.".to_string(),
                "Keep Stonks provider advertisement, market snapshot, Eve surface, command_boundary, and transport_profile records in the daemon-owned CultCache store.".to_string(),
                "Keep Odin provider discovery accepting Stonks' typed store instead of relying on HTTP manifest ingestion.".to_string(),
                "Delete or demote health-stonks.cmd to a manual compatibility probe with no lifecycle truth.".to_string(),
            ];
        }
        "mimir-eve-dashboard" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "high";
            owner = "Mimir dashboard runtime";
            current_mechanism =
                "Mimir Eve dashboard publishes mimir.cultnet-rudp-provider-health over CultNet/RUDP from the live Nightwing broker, publishes retained dashboard state through CultMesh at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp, and writes a daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc."
                    .to_string();
            intended_authority =
                "Mimir dashboard publishes daemon health over cultnet.transport.rudp.v0, publishes retained Eve dashboard state through CultMesh, and keeps provider manifest, command boundary, and transport profile in its daemon-owned boundary witness; HTTP/WebSocket remain lowerings for clients only."
                    .to_string();
            cut_line =
                "Keep the local HTTP probe as fallback only until Odin and Idunn consume the daemon-owned Mimir witness paths directly instead of treating the HTTP surface as the primary proof."
                    .to_string();
            steps = vec![
                "Keep live mimir.cultnet-rudp-provider-health publication running from the Nightwing Eve dashboard broker.".to_string(),
                "Keep the live CultMesh state witness at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp publishing retained dashboard state.".to_string(),
                "Keep the daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc publishing provider manifest, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                "Teach Odin to prefer Mimir CultMesh/CultCache witness paths over compatibility HTTP catalog ingestion.".to_string(),
                "Delete or demote health-mimir-eve-dashboard.cmd to a manual compatibility probe with no lifecycle truth.".to_string(),
            ];
        }
        "weksa" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "medium-high";
            owner = "Weksa provider runtime";
            current_mechanism =
                "Weksa publishes weksa.cultnet-rudp-provider-health over CultNet/RUDP after each serialized witness refresh, its daemon-owned provider store contains provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records that Odin can ingest, and speech_provider.mimo.voicedesign now arrives on rudp://127.0.0.1:8813 as weksa.mimo_voicedesign_command.v0."
                    .to_string();
            intended_authority =
                "Weksa publishes daemon health, provider advertisement, operator state, Eve surfaces, command boundary, transport profile, and MiMo VoiceDesign command ingress as typed CultMesh/CultNet records over cultnet.transport.rudp.v0; no HTTP endpoint owns daemon truth."
                    .to_string();
            cut_line =
                "Keep health-weksa.cmd as fallback evidence only; Odin should consume Weksa's typed provider store, command boundary, and transport profile directly, and no HTTP endpoint may retake command authority."
                    .to_string();
            steps = vec![
                "Keep live weksa.cultnet-rudp-provider-health publication running from the Weksa daemon.".to_string(),
                "Keep Weksa provider advertisement, operator-state, Eve surface, command_boundary, and transport_profile records in the daemon-owned provider store.".to_string(),
                "Keep Odin provider discovery accepting Weksa's typed provider store instead of relying on compatibility HTTP ingestion.".to_string(),
                "Keep speech_provider.mimo.voicedesign on CultNet/RUDP as weksa.mimo_voicedesign_command.v0 instead of an HTTP route.".to_string(),
                "Keep health-weksa.cmd as a compatibility witness over daemon-owned .cc state and process shape, not daemon truth.".to_string(),
            ];
        }
        "voidbot" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "medium-high";
            owner = "VoidBot internal provider stack";
            current_mechanism =
                "VoidBot publishes voidbot.cultnet-rudp-stack-health over CultNet/RUDP after each local orchestrator pulse, and the always-run voidbot-swarm-surface organ writes daemon-owned provider catalog, provider advertisement, command_boundary, and transport_profile records into E:\\Projects\\VoidBot\\.voidbot\\status\\cultmesh\\voidbot-swarm-state.cc."
                    .to_string();
            intended_authority =
                "VoidBot publishes internal swarm, repo-face, and provider health over CultNet/RUDP; Discord delivery remains a boundary adapter, never daemon truth."
                    .to_string();
            cut_line =
                "Keep the operations probe as fallback only while it verifies the daemon-owned CultMesh witness store; it no longer owns VoidBot lifecycle truth."
                    .to_string();
            steps = vec![
                "Keep live voidbot.cultnet-rudp-stack-health publication running from the GameCult Local Orchestrator pulse.".to_string(),
                "Keep VoidBot swarm, Discord, archive, source, and repo-face provider records published from the daemon-owned CultMesh witness store.".to_string(),
                "Keep VoidBot command_boundary and transport_profile records published from the provider runtime.".to_string(),
                "Teach Odin to prefer the daemon-owned VoidBot CultMesh witness store over compatibility status ingestion.".to_string(),
                "Delete or demote health-voidbot.cmd to a manual compatibility probe with no lifecycle truth.".to_string(),
            ];
        }
        "muninn" => {
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Muninn Rust runtime plus Raven background-only launcher surface";
            current_mechanism =
                "Raven Muninn now runs from the hidden GameCult-Muninn scheduled task with --idunn-rudp-health 10.77.0.2:17870, daemon id muninn, and contract muninn.cultnet-rudp-remote-telemetry-health on the long-running serve process. Live Idunn accepts that daemon-published health from 10.77.0.4, the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, and telemetry surface records, activation commands now route through the explicit C:\\Meta\\Odin\\state\\muninn.activate.cc store, the serve body no longer auto-claims ambient Move runtime state from platform defaults, and GameCult-Muninn, GameCult-Muninn-Activate, and GameCult-Muninn-VideoProof now execute hidden VBS launchers whose bodies call noninteractive hidden PowerShell entrypoints directly; .cmd wrappers, where present, are manual compatibility entrypoints only."
                    .to_string();
            intended_authority =
                "Muninn publishes telemetry, provider advertisement, command_boundary, transport_profile, explicit activation routing, and daemon health over CultNet/RUDP/CultCache; Raven Task Scheduler owns only background launch of hidden WScript/PowerShell launchers and never visible .cmd trampoline execution or ambient Move runtime inference."
                    .to_string();
            cut_line =
                "Keep Raven's hidden task launch invariant structural by verifying that scheduled-task actions and hidden VBS launchers never route through cmdPath trampolines, that activation commands stay pointed at C:\\Meta\\Odin\\state\\muninn.activate.cc, and that plain serve cannot infer Move runtime authority from platform defaults; Odin and Idunn should consume C:\\Meta\\Odin\\state\\muninn.telemetry.cc as the daemon-owned witness while health-muninn.cmd remains fallback evidence only."
                    .to_string();
            steps = vec![
                "Keep scripts/repair-raven-muninn-task-actions.ps1 using sftp plus a tiny remote runner so Windows command-line length does not block future hidden-task repair.".to_string(),
                "Keep GameCult-Muninn action executing wscript.exe with start-muninn-serve-hidden.vbs arguments.".to_string(),
                "Keep the Raven Muninn serve process command line carrying --idunn-rudp-health 10.77.0.2:17870, --idunn-daemon muninn, and --idunn-health-contract muninn.cultnet-rudp-remote-telemetry-health.".to_string(),
                "Keep the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\muninn.telemetry.cc publishing provider advertisement, command_boundary, transport_profile, and telemetry surface records.".to_string(),
                "Keep the activation path publishing through C:\\Meta\\Odin\\state\\muninn.activate.cc instead of reusing the telemetry store for activate routing.".to_string(),
                "Keep plain Muninn serve from auto-claiming PS Move hosts, Move state, or platform-default Move lights unless explicit Move flags request that runtime authority.".to_string(),
                "Keep GameCult-Muninn-Activate action executing wscript.exe with activate-raven-av-srt-hidden.vbs arguments.".to_string(),
                "Keep GameCult-Muninn-VideoProof action executing wscript.exe with muninn-raven-video-to-starfire-obs-hidden.vbs arguments.".to_string(),
                "Keep the Activate and VideoProof hidden VBS launchers calling PowerShell entrypoints directly instead of cmdPath trampolines.".to_string(),
                "Keep Raven health/restart actuators background-only; no visible terminals or interactive windows on the shared host.".to_string(),
            ];
        }
        "starfire-muninn" | "nightwing-muninn" => {
            owner = "Muninn Rust runtime";
            intended_authority =
                "Muninn publishes telemetry, Quest/Move access, and daemon health over CultNet/RUDP; activation commands remain separate from keepalive."
                    .to_string();
            if target.daemon_id == "starfire-muninn" {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Starfire Muninn now runs as a long-lived local serve process with --quest-adb, --idunn-rudp-health 127.0.0.1:17870, daemon id starfire-muninn, and contract muninn.cultnet-rudp-local-telemetry-and-quest-access. Live Idunn accepts that daemon-published health from 127.0.0.1, the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Quest access, and telemetry surface records, and restart-starfire-muninn.ps1 now archives a corrupt CultCache store before relaunch instead of leaving the daemon dead on decode faults."
                        .to_string();
                cut_line =
                    "Keep health-starfire-muninn.cmd as fallback evidence only; Odin and Idunn should consume C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc as the daemon-owned witness while Quest availability remains telemetry state."
                        .to_string();
                steps = vec![
                    "Keep the Starfire Muninn serve process command line carrying --idunn-rudp-health 127.0.0.1:17870, --idunn-daemon starfire-muninn, and --idunn-health-contract muninn.cultnet-rudp-local-telemetry-and-quest-access.".to_string(),
                    "Keep restart-starfire-muninn.ps1 clearing stale .lock files and archiving corrupt starfire.muninn.telemetry.cc before relaunch.".to_string(),
                    "Keep the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc publishing provider advertisement, command_boundary, transport_profile, Quest access, and telemetry surface records.".to_string(),
                    "Keep health-starfire-muninn.cmd as a compatibility witness for Quest telemetry and process shape, not daemon truth.".to_string(),
                ];
            } else {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Nightwing Muninn now runs as a long-lived serve process with explicit Move HID input, --idunn-rudp-health 10.77.0.2:17870, daemon id nightwing-muninn, and contract muninn.cultnet-rudp-remote-telemetry-and-move-hid. Live Idunn accepts that daemon-published health from 10.77.0.3, the daemon-owned telemetry store at /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Move HID evidence, and telemetry surface records, and restart-nightwing-muninn.ps1 now only claims or discovers Move runtime authority when -DiscoverMoveState, -ClaimUsbMoves, or explicit -MoveState values are present."
                        .to_string();
                cut_line =
                    "Keep health-nightwing-muninn.cmd as fallback evidence only; Odin and Idunn should consume /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc as the daemon-owned witness while the serve process owns muninn.cultnet-rudp-remote-telemetry-and-move-hid publication."
                        .to_string();
                steps = vec![
                    "Keep the Nightwing Muninn serve process command line carrying --idunn-rudp-health 10.77.0.2:17870, --idunn-daemon nightwing-muninn, and --idunn-health-contract muninn.cultnet-rudp-remote-telemetry-and-move-hid.".to_string(),
                    "Keep the daemon-owned telemetry store at /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc publishing provider advertisement, command_boundary, transport_profile, Move HID evidence, and telemetry surface records.".to_string(),
                    "Keep restart-nightwing-muninn.ps1 launching the long-running serve body instead of one-shot health publication.".to_string(),
                    "Keep restart-nightwing-muninn.ps1 requiring explicit -DiscoverMoveState/-ClaimUsbMoves or explicit -MoveState values before it adds Move evidence/runtime arguments.".to_string(),
                    "Keep health-nightwing-muninn.cmd as a compatibility witness for Move HID freshness, not daemon truth.".to_string(),
                ];
            }
        }
        "vili" => {
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Vili animation runtime";
            current_mechanism =
                "Vili now runs on Raven through the hidden GameCult\\Vili scheduled task, which launches start-vili-daemon.ps1 with --idunn-rudp-health 10.77.0.2:17870 and contract vili.cultnet-rudp-animation-health. Live Idunn accepts that daemon-published health from 10.77.0.4, and Vili writes a daemon-owned vili.service.cc CultCache store containing provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records."
                    .to_string();
            intended_authority =
                "Vili publishes animation daemon health, provider advertisement, operator state, command boundary, and transport profile as typed CultMesh/CultNet records over cultnet.transport.rudp.v0; HTTP and WebSocket remain renderer/operator lowerings only."
                    .to_string();
            cut_line =
                "Keep health-vili.cmd and the HTTP/WebSocket deck checks as fallback/operator lowerings only until Odin consumes Vili provider records natively and the compatibility health endpoint is demoted to a manual witness."
                    .to_string();
            steps = vec![
                "Keep the in-process Vili idunn.daemon_health RUDP publisher wired through scripts/vili-daemon.mjs.".to_string(),
                "Keep Vili's provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records in the daemon-owned vili.service.cc store.".to_string(),
                "Keep scripts/restart-vili.ps1 syncing the authoritative Vili runtime plus flattened CultLib node_modules into Raven before reinstalling the hidden task.".to_string(),
                "Keep GameCult\\Vili executing wscript.exe hidden launcher arguments that pass --idunn-rudp-health 10.77.0.2:17870, --idunn-daemon vili, and --idunn-health-contract vili.cultnet-rudp-animation-health.".to_string(),
                "Keep Idunn bound on 0.0.0.0:17870 so WireGuard peers such as Raven can publish daemon-owned health over cultnet.transport.rudp.v0.".to_string(),
                "Keep Odin provider discovery pointed at Vili's daemon-owned vili.service.cc store; then demote health-vili.cmd and HTTP/WebSocket deck probes to fallback witnesses only.".to_string(),
            ];
        }
        "nightwing-eve-dashboard" | "nightwing-eve-browser-reference" => {
            severity = "medium";
            owner = "Eve lowering/runtime owner";
            intended_authority =
                "Eve runtimes subscribe to provider-owned CultMesh/CultNet state; any browser/WebSocket bridge is display-only and does not own daemon health."
                    .to_string();
            if target.daemon_id == "nightwing-eve-dashboard" {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Nightwing Eve dashboard publishes nightwing.cultnet-rudp-eve-dashboard-health over CultNet/RUDP from the Mimir.EveDashboard systemd process, with retained dashboard state in /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp and a daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc."
                        .to_string();
                cut_line =
                    "HTTP/WebSocket stay client lowerings only. health-nightwing-eve-dashboard.ps1 now inspects the dashboard CultMesh/CultCache witnesses directly, the .cmd wrapper is a compatibility entrypoint only, and the remaining debt is teaching Nightwing projections and Odin to consume the typed witnesses without any service probe reclaiming daemon truth."
                        .to_string();
                steps = vec![
                    "Keep live nightwing.cultnet-rudp-eve-dashboard-health publication running from the Mimir.EveDashboard systemd process.".to_string(),
                    "Keep the live CultMesh state witness at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp publishing retained dashboard state.".to_string(),
                    "Keep the daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc publishing provider manifest, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                    "Teach Odin and Nightwing projections to prefer the dashboard CultMesh/CultCache witnesses over service compatibility probes.".to_string(),
                    "Keep health-nightwing-eve-dashboard.cmd as a compatibility wrapper over witness-first PowerShell checks with no daemon-truth of its own.".to_string(),
                ];
            } else {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Nightwing Eve browser reference publishes nightwing.cultnet-rudp-browser-reference-health over CultNet/RUDP from the Mimir.EveBrowserReference service process, and the runtime now writes a daemon-owned boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc."
                        .to_string();
                cut_line =
                    "HTTP stays a browser lowering only. health-nightwing-eve-browser-reference.ps1 now inspects the browser reference CultCache witness directly, the .cmd wrapper is a compatibility entrypoint only, and the remaining debt is teaching Nightwing projections and Odin to consume the typed witness without any service probe reclaiming daemon truth."
                        .to_string();
                steps = vec![
                    "Keep live nightwing.cultnet-rudp-browser-reference-health publication running from the Mimir.EveBrowserReference service process.".to_string(),
                    "Keep the daemon-owned boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc publishing manifest, static-surface, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                    "Teach Odin and Nightwing projections to prefer the browser reference CultCache witness over service compatibility probes.".to_string(),
                    "Keep health-nightwing-eve-browser-reference.cmd as a compatibility wrapper over witness-first PowerShell checks with no daemon-truth of its own.".to_string(),
                ];
            }
        }
        "yggdrasil-streampixels" => {
            severity = "medium";
            status = "partial-rudp-health-and-provider-store-live";
            owner = "StreamPixels service runtime plus gamecult-ops deploy lane";
            current_mechanism =
                "StreamPixels publishes a daemon-owned CultCache boundary store with provider advertisement, command_boundary, transport_profile, and Idunn health summary from the live Yggdrasil service runtime. The deployed service keeps STREAMPIXELS_IDUNN_RUDP_HEALTH=10.77.0.2:17870 and contract streampixels.cultnet-rudp-service-health in /srv/streampixels/env/service.env, the source-artifact lane now ships the required CultLib snapshot beside the app artifact, and live Idunn accepts yggdrasil-streampixels from 10.77.0.1."
                    .to_string();
            intended_authority =
                "StreamPixels publishes service health, provider state, command boundary, and transport profile over cultnet.transport.rudp.v0, with the service-owned CultCache boundary as durable local state and HTTP/SSH/systemd as fallback witnesses."
                    .to_string();
            cut_line =
                "Keep the StreamPixels service boundary store and in-process RUDP publisher live on Yggdrasil; SSH/systemd/HTTP checks and deployment-manifest freshness remain deployment/debug witnesses only until Odin consumes the typed store and Idunn no longer needs compatibility proof."
                    .to_string();
            steps = vec![
                "Keep apps/service/src/verse-state.ts publishing streampixels.service.cc from the StreamPixels service runtime.".to_string(),
                "Teach Odin to ingest StreamPixels provider advertisement, command_boundary, and transport_profile records from the service-owned CultCache boundary store.".to_string(),
                "Keep the StreamPixels in-process Idunn RUDP health publisher using contract streampixels.cultnet-rudp-service-health.".to_string(),
                "Keep the Yggdrasil source-artifact lane shipping the StreamPixels app artifact plus the CultLib cultnet-ts/cultcache-ts snapshot through a tiny remote runner script instead of brittle inline SSH quoting.".to_string(),
                "Keep the Yggdrasil deploy lane using a serial pnpm workspace build and the deployment-manifest freshness check so failed builds cannot masquerade as fresh restarts.".to_string(),
                "Keep live Idunn acceptance proof for StreamPixels RUDP health on Yggdrasil; the publisher sends the health document after a short accept grace period so one-shot pulses do not depend on receiving the accept reply.".to_string(),
                "Demote health-yggdrasil-streampixels.cmd, SSH/systemd, and HTTP checks to deployment/debug witnesses once Odin and Idunn consume the typed store and daemon-published RUDP health without compatibility help.".to_string(),
            ];
        }
        "yggdrasil-heimdall" => {
            severity = "medium";
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Heimdall service runtime plus Yggdrasil source-artifact lane";
            current_mechanism =
                "Heimdall now publishes heimdall.cultnet-rudp-provider-health over CultNet/RUDP from the live Yggdrasil service runtime, and its daemon-owned boundary store at /srv/heimdall/cultcache/heimdall.service.cc contains provider advertisement, command_boundary, transport_profile, and daemon-health summary state. The Yggdrasil source-artifact lane now ships the required CultLib snapshot beside the app artifact, while check-heimdall.sh still verifies systemd, /healthz, JWKS, discovery, nginx, and the boundary witness."
                    .to_string();
            intended_authority =
                "Heimdall keeps public HTTP as the product boundary while publishing daemon health, provider advertisement, command boundary, and transport profile as internal CultNet/RUDP state, with full redacted auth-document witness export following as a separate auth-safe pass."
                    .to_string();
            cut_line =
                "Keep the Heimdall boundary store and in-process RUDP publisher live on Yggdrasil; systemd, /healthz, JWKS, discovery, and nginx routing are deployment/debug witnesses only once Odin ingests the typed store and Idunn no longer needs compatibility proof."
                    .to_string();
            steps = vec![
                "Keep Heimdall's src/verse-witness.ts advertisement published through the runtime-owned boundary store with daemon-owned update timestamps.".to_string(),
                "Keep Heimdall daemon health published over CultNet/RUDP from the live Yggdrasil service runtime with contract heimdall.cultnet-rudp-provider-health.".to_string(),
                "Keep Heimdall command_boundary and transport_profile records in the runtime-owned boundary store so deploy/restart authority is inspectable without the compatibility scripts.".to_string(),
                "Keep the Yggdrasil Heimdall source-artifact lane shipping the app artifact plus the CultLib cultnet-ts/cultcache-ts snapshot through a tiny remote runner script instead of brittle inline SSH quoting.".to_string(),
                "Teach Odin to ingest Heimdall's provider advertisement and future redacted witness surfaces as typed state instead of treating HTTP discovery and JWKS as daemon truth.".to_string(),
                "Demote check-heimdall.sh, systemd, /healthz, JWKS, discovery, and nginx routing to deployment/debug witnesses once Odin and Idunn consume the typed store without compatibility help.".to_string(),
            ];
        }
        "yggdrasil-repixelizer" => {
            severity = "medium";
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Repixelizer GUI runtime plus Yggdrasil source-artifact lane";
            current_mechanism =
                "Repixelizer now publishes repixelizer.cultnet-rudp-service-health from the live Yggdrasil GUI runtime, and its daemon-owned boundary store at /srv/repixelizer/cultcache/repixelizer.service.cc carries provider advertisement, Eve surface state, queue/auth/runtime projection, command_boundary, transport_profile, and daemon-health summary state while check-repixelizer-gui.sh and systemd remain compatibility witnesses."
                    .to_string();
            intended_authority =
                "Repixelizer keeps HTTP GUI delivery as the product boundary while publishing internal daemon health, queue/provider state, command boundary, and transport profile over CultNet/RUDP for Idunn and Odin."
                    .to_string();
            cut_line =
                "Keep Repixelizer's runtime-owned RUDP health and boundary store live until Odin ingests the typed queue/provider state directly and Idunn no longer needs /api/health, /api/config, systemd, or nginx as compatibility witnesses."
                    .to_string();
            steps = vec![
                "Keep Repixelizer daemon health published over CultNet/RUDP from the live GUI/service runtime on Yggdrasil with contract repixelizer.cultnet-rudp-service-health.".to_string(),
                "Keep Repixelizer command_boundary and transport_profile records in the runtime-owned boundary store so deploy authority is inspectable typed state instead of an ops-only assumption.".to_string(),
                "Keep the Yggdrasil Repixelizer source-artifact lane shipping the app artifact plus the CultLib cultcache-py snapshot through a tiny remote runner script instead of brittle inline SSH quoting.".to_string(),
                "Teach Odin to ingest Repixelizer queue/auth/runtime state from the daemon-owned witness store instead of treating /api/health and /api/config as daemon truth.".to_string(),
                "Keep Heimdall-backed browser auth as a product boundary adapter, not the keepalive truth surface.".to_string(),
                "Demote check-repixelizer-gui.sh, systemd, /api/health, /api/config, and nginx routing to deployment/debug witnesses once Odin and Idunn consume the typed store without compatibility help.".to_string(),
            ];
        }
        "idunn-swarm-deployment-coverage" => {
            severity = "medium";
            status = "catalog-coherence-probe";
            owner = "Idunn";
            objective =
                "Keep Idunn's deployment target catalog honest while daemon-owned RUDP publication is being installed."
                    .to_string();
            current_mechanism =
                "A local coverage command verifies deployment authority categories for the swarm target catalog."
                    .to_string();
            intended_authority =
                "Daemon provider advertisements and command_boundary records make deploy/restart ownership inspectable without a local coverage shim."
                    .to_string();
            cut_line =
                "Delete this probe after every deployable daemon publishes command_boundary and transport_profile over CultNet/RUDP."
                    .to_string();
        }
        _ => {}
    }

    IdunnDaemonSurgeryPlanRecord {
        plan_id: format!("surgery:{}", target.daemon_id),
        daemon_id: target.daemon_id.clone(),
        severity: severity.to_string(),
        status: status.to_string(),
        owner: owner.to_string(),
        objective,
        current_mechanism,
        intended_authority,
        cut_line,
        steps,
        blockers,
        updated_at: updated_at.to_string(),
    }
}

fn swarm_targets(options: &SwarmOptions) -> Result<Vec<DaemonTarget>> {
    let repo_root = options.repo_root.display().to_string();
    let script = |name: &str| format!(r"{}\scripts\{}", repo_root, name);
    let project = |name: &str| PathBuf::from(format!(r"E:\Projects\{name}"));

    match options.profile.as_str() {
        "starfire-local" => Ok(vec![
            DaemonTarget {
                daemon_id: "odin".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Odin all-seer".to_string(),
                health_contract: health_contract("odin.cultnet-rudp-provider-health", "failed"),
                health_command: Some(script("health-odin.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-odin.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "mimir-eve-dashboard".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Mimir Eve dashboard".to_string(),
                health_contract: health_contract("mimir.cultnet-rudp-provider-health", "failed"),
                health_command: Some(script("health-mimir-eve-dashboard.cmd")),
                deploy_command: None,
                restart_command: None,
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "stonks".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Stonks market pulse".to_string(),
                health_contract: health_contract("stonks.cultnet-rudp-market-health", "failed"),
                health_command: Some(script("health-stonks.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-stonks.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "voidbot".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "VoidBot local stack".to_string(),
                health_contract: health_contract("voidbot.cultnet-rudp-stack-health", "failed"),
                health_command: Some(script("health-voidbot.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-voidbot.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "weksa".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Weksa intent and utterance lowering service".to_string(),
                health_contract: health_contract("weksa.cultnet-rudp-provider-health", "failed"),
                health_command: Some(script("health-weksa.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-weksa.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 60,
            },
            DaemonTarget {
                daemon_id: "starfire-muninn".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Starfire Muninn telemetry and Quest access daemon".to_string(),
                health_contract: health_contract(
                    "muninn.cultnet-rudp-local-telemetry-and-quest-access",
                    "failed",
                ),
                health_command: Some(script("health-starfire-muninn.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-starfire-muninn.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "muninn".to_string(),
                verse_id: "raven.local".to_string(),
                name: "Muninn telemetry Verse assembler".to_string(),
                health_contract: health_contract(
                    "muninn.cultnet-rudp-remote-telemetry-health",
                    "failed",
                ),
                health_command: Some(script("health-muninn.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-muninn.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "vili".to_string(),
                verse_id: "raven.local".to_string(),
                name: "Vili Persona animation daemon".to_string(),
                health_contract: health_contract("vili.cultnet-rudp-animation-health", "failed"),
                health_command: Some(script("health-vili.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-vili.cmd")),
                release: None,
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
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-heimdall".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Heimdall".to_string(),
                health_contract: health_contract("heimdall.cultnet-rudp-provider-health", "failed"),
                health_command: Some(script("health-yggdrasil-heimdall.cmd")),
                deploy_command: Some(script("deploy-yggdrasil-heimdall.cmd")),
                restart_command: None,
                release: Some(release_target(
                    "Heimdall",
                    project("Heimdall"),
                    "restart-after-verified-build",
                    None,
                    "restart-required",
                )),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-repixelizer".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Repixelizer".to_string(),
                health_contract: health_contract(
                    "repixelizer.cultnet-rudp-service-health",
                    "failed",
                ),
                health_command: Some(script("health-yggdrasil-repixelizer.cmd")),
                deploy_command: Some(script("deploy-yggdrasil-repixelizer.cmd")),
                restart_command: None,
                release: Some(release_target(
                    "repixelizer",
                    project("repixelizer"),
                    "restart-after-verified-build",
                    None,
                    "restart-required",
                )),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-streampixels".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil StreamPixels".to_string(),
                health_contract: health_contract(
                    "streampixels.cultnet-rudp-service-health",
                    "failed",
                ),
                health_command: Some(script("health-yggdrasil-streampixels.cmd")),
                deploy_command: Some(script("deploy-yggdrasil-streampixels.cmd")),
                restart_command: None,
                release: Some(release_target(
                    "StreamPixels",
                    project("StreamPixels"),
                    "restart-after-verified-build",
                    None,
                    "restart-required",
                )),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "nightwing-gjallar".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Gjallar framebuffer compositor".to_string(),
                health_contract: health_contract(
                    "gjallar.cultnet-rudp-framebuffer-composition-health",
                    "dependency-unavailable",
                ),
                health_command: Some(script("health-nightwing-gjallar.ps1")),
                deploy_command: Some(script("deploy-nightwing-gjallar.ps1")),
                restart_command: Some(script("restart-nightwing-gjallar.ps1")),
                release: Some(release_target(
                    "Gjallar",
                    project("Gjallar"),
                    "restart-after-verified-build",
                    None,
                    "restart-required",
                )),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "nightwing-muninn".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Muninn telemetry and Move HID daemon".to_string(),
                health_contract: health_contract(
                    "muninn.cultnet-rudp-remote-telemetry-and-move-hid",
                    "failed",
                ),
                health_command: Some(script("health-nightwing-muninn.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-muninn.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "nightwing-eve-dashboard".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Eve dashboard broker".to_string(),
                health_contract: health_contract(
                    "nightwing.cultnet-rudp-eve-dashboard-health",
                    "failed",
                ),
                health_command: Some(script("health-nightwing-eve-dashboard.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-eve-dashboard.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "nightwing-eve-browser-reference".to_string(),
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing Eve browser reference".to_string(),
                health_contract: health_contract(
                    "nightwing.cultnet-rudp-browser-reference-health",
                    "failed",
                ),
                health_command: Some(script("health-nightwing-eve-browser-reference.cmd")),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-eve-browser-reference.cmd")),
                release: None,
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
        let mut rudp_health_bind = Some("127.0.0.1:17870".parse::<SocketAddr>()?);
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
                "--rudp-health-bind" => {
                    let value = take_value(&mut args, "--rudp-health-bind")?;
                    rudp_health_bind = if value.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(
                            value
                                .parse()
                                .with_context(|| "--rudp-health-bind must be a socket address")?,
                        )
                    };
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
            rudp_health_bind,
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
                    release: None,
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
                publication_source: "compatibility-command".to_string(),
                transport: "compatibility.local-command".to_string(),
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
                    publication_source: "compatibility-command".to_string(),
                    transport: "compatibility.local-command".to_string(),
                    observed_at: observed_at.to_string(),
                }
            }
            Err(error) => IdunnDaemonHealthRecord {
                daemon_id: target.daemon_id.clone(),
                state: "failed".to_string(),
                detail: format!("health command could not run: {error}"),
                health_contract: target.health_contract.id.clone(),
                publication_source: "compatibility-command".to_string(),
                transport: "compatibility.local-command".to_string(),
                observed_at: observed_at.to_string(),
            },
        },
        None => IdunnDaemonHealthRecord {
            daemon_id: target.daemon_id.clone(),
            state: "unknown".to_string(),
            detail: "no health command was provided".to_string(),
            health_contract: target.health_contract.id.clone(),
            publication_source: "compatibility-command".to_string(),
            transport: "compatibility.local-command".to_string(),
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

fn run_state_migration(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    requested_at: &str,
    command_timeout_seconds: u64,
) -> Option<IdunnStateMigrationResultRecord> {
    let plan_id = migration_plan_id(target);
    let result_id = format!("result:{plan_id}:{requested_at}");
    let Some(command) = release.state_migration_command.as_deref() else {
        return Some(IdunnStateMigrationResultRecord {
            result_id,
            plan_id,
            daemon_id: target.daemon_id.clone(),
            state: "noop".to_string(),
            detail: "daemon declared no state migration command".to_string(),
            completed_at: requested_at.to_string(),
        });
    };
    match run_shell(command, command_timeout_seconds) {
        Ok(output) if output.status.success() => Some(IdunnStateMigrationResultRecord {
            result_id,
            plan_id,
            daemon_id: target.daemon_id.clone(),
            state: "succeeded".to_string(),
            detail: command_output_detail("state migration command exited successfully", &output),
            completed_at: requested_at.to_string(),
        }),
        Ok(output) => Some(IdunnStateMigrationResultRecord {
            result_id,
            plan_id,
            daemon_id: target.daemon_id.clone(),
            state: "failed".to_string(),
            detail: command_output_detail(
                &format!("state migration command exited with {}", output.status),
                &output,
            ),
            completed_at: requested_at.to_string(),
        }),
        Err(error) => Some(IdunnStateMigrationResultRecord {
            result_id,
            plan_id,
            daemon_id: target.daemon_id.clone(),
            state: "failed".to_string(),
            detail: format!("state migration command could not run: {error}"),
            completed_at: requested_at.to_string(),
        }),
    }
}

fn rollout_result_record(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    deployment: &IdunnDeploymentResultRecord,
    migration: Option<&IdunnStateMigrationResultRecord>,
    completed_at: &str,
) -> IdunnRolloutResultRecord {
    let migration_detail = migration
        .map(|result| format!("migration {}: {}", result.state, result.detail))
        .unwrap_or_else(|| "migration not declared".to_string());
    let downtime_detail = if release.zero_downtime_capability == "zero-downtime" {
        "zero-downtime strategy declared"
    } else {
        "restart-required strategy; zero downtime not claimed"
    };
    IdunnRolloutResultRecord {
        result_id: format!("result:{}:{completed_at}", rollout_plan_id(target)),
        plan_id: rollout_plan_id(target),
        daemon_id: target.daemon_id.clone(),
        state: deployment.state.clone(),
        detail: format!(
            "{migration_detail}; deployment {}: {}; {downtime_detail}",
            deployment.state, deployment.detail
        ),
        completed_at: completed_at.to_string(),
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
        .env("IDUNN_ACTUATOR", "1")
        .env("IDUNN_COMMAND_AUTHORITY", "idunn-daemon")
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

fn unix_epoch_millis() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_millis();
    u64::try_from(millis).context("system clock milliseconds exceeded u64")
}

fn help_text() -> &'static str {
    "Usage: idunn --daemon <id> [--name <name>] [--verse <verse>] [--store <path>] [--health-command <command>] [--deploy-command <command>] [--restart-command <command>] [--operator-alarm-command <command>] [--rudp-health-bind <addr|none>] [--execute] [--interval-seconds <seconds>] [--command-timeout-seconds <seconds>] [--repo-root <path>] [--swarm-profile <profile>]\n\nIdunn can run one manual daemon probe lane with --daemon, or one built-in swarm supervisor with --swarm-profile starfire-local."
}

#[cfg(test)]
mod tests {
    use super::*;
    use cultnet_rs::{CultNetRawDocumentRecord, CultNetRawPayloadEncoding};

    fn target(default_failure_state: &str, deploy_command: Option<&str>) -> DaemonTarget {
        DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.contract", default_failure_state),
            health_command: Some("exit 0".to_string()),
            deploy_command: deploy_command.map(ToString::to_string),
            restart_command: Some("restart test".to_string()),
            release: None,
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

    #[test]
    fn swarm_surgery_plan_moves_on_after_muninn_family_is_upgraded() {
        let starfire_muninn = DaemonTarget {
            daemon_id: "starfire-muninn".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Starfire Muninn".to_string(),
            health_contract: health_contract(
                "muninn.cultnet-rudp-local-telemetry-and-quest-access",
                "degraded",
            ),
            health_command: Some("health-starfire-muninn.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-starfire-muninn.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let nightwing_muninn = DaemonTarget {
            daemon_id: "nightwing-muninn".to_string(),
            verse_id: "nightwing.local".to_string(),
            name: "Nightwing Muninn".to_string(),
            health_contract: health_contract(
                "muninn.cultnet-rudp-remote-telemetry-and-move-hid",
                "failed",
            ),
            health_command: Some("health-nightwing-muninn.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-nightwing-muninn.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let raven_muninn = DaemonTarget {
            daemon_id: "muninn".to_string(),
            verse_id: "raven.local".to_string(),
            name: "Raven Muninn".to_string(),
            health_contract: health_contract(
                "muninn.cultnet-rudp-remote-telemetry-health",
                "failed",
            ),
            health_command: Some("health-muninn.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-muninn.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let odin = DaemonTarget {
            daemon_id: "odin".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Odin".to_string(),
            health_contract: health_contract("odin.cultnet-rudp-provider-health", "failed"),
            health_command: Some("health-odin.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-odin.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let stonks = DaemonTarget {
            daemon_id: "stonks".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Stonks".to_string(),
            health_contract: health_contract("stonks.cultnet-rudp-market-health", "failed"),
            health_command: Some("health-stonks.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-stonks.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let weksa = DaemonTarget {
            daemon_id: "weksa".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Weksa".to_string(),
            health_contract: health_contract("weksa.cultnet-rudp-provider-health", "failed"),
            health_command: Some("health-weksa.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-weksa.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 60,
        };
        let voidbot = DaemonTarget {
            daemon_id: "voidbot".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "VoidBot local stack".to_string(),
            health_contract: health_contract("voidbot.cultnet-rudp-stack-health", "failed"),
            health_command: Some("health-voidbot.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-voidbot.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 60,
        };
        let nightwing_gjallar = DaemonTarget {
            daemon_id: "nightwing-gjallar".to_string(),
            verse_id: "nightwing.local".to_string(),
            name: "Nightwing Gjallar framebuffer compositor".to_string(),
            health_contract: health_contract(
                "gjallar.cultnet-rudp-framebuffer-composition-health",
                "failed",
            ),
            health_command: Some("health-nightwing-gjallar.ps1".to_string()),
            deploy_command: Some("deploy-nightwing-gjallar.ps1".to_string()),
            restart_command: Some("restart-nightwing-gjallar.ps1".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let mimir = DaemonTarget {
            daemon_id: "mimir-eve-dashboard".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Mimir Eve dashboard".to_string(),
            health_contract: health_contract("mimir.cultnet-rudp-provider-health", "failed"),
            health_command: Some("health-mimir-eve-dashboard.cmd".to_string()),
            deploy_command: None,
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let nightwing_eve_dashboard = DaemonTarget {
            daemon_id: "nightwing-eve-dashboard".to_string(),
            verse_id: "nightwing.local".to_string(),
            name: "Nightwing Eve dashboard".to_string(),
            health_contract: health_contract(
                "nightwing.cultnet-rudp-eve-dashboard-health",
                "failed",
            ),
            health_command: Some("health-nightwing-eve-dashboard.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-nightwing-eve-dashboard.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let nightwing_eve_browser_reference = DaemonTarget {
            daemon_id: "nightwing-eve-browser-reference".to_string(),
            verse_id: "nightwing.local".to_string(),
            name: "Nightwing Eve browser reference".to_string(),
            health_contract: health_contract(
                "nightwing.cultnet-rudp-browser-reference-health",
                "failed",
            ),
            health_command: Some("health-nightwing-eve-browser-reference.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-nightwing-eve-browser-reference.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let vili = DaemonTarget {
            daemon_id: "vili".to_string(),
            verse_id: "raven.local".to_string(),
            name: "Vili".to_string(),
            health_contract: health_contract("vili.cultnet-rudp-animation-health", "failed"),
            health_command: Some("health-vili.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-vili.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let yggdrasil_streampixels = DaemonTarget {
            daemon_id: "yggdrasil-streampixels".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil StreamPixels".to_string(),
            health_contract: health_contract("streampixels.cultnet-rudp-service-health", "failed"),
            health_command: Some("health-yggdrasil-streampixels.cmd".to_string()),
            deploy_command: Some("deploy-yggdrasil-streampixels.cmd".to_string()),
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 300,
        };
        let yggdrasil_heimdall = DaemonTarget {
            daemon_id: "yggdrasil-heimdall".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil Heimdall".to_string(),
            health_contract: health_contract("heimdall.cultnet-rudp-provider-health", "failed"),
            health_command: Some("health-yggdrasil-heimdall.cmd".to_string()),
            deploy_command: Some("deploy-yggdrasil-heimdall.cmd".to_string()),
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 300,
        };
        let yggdrasil_repixelizer = DaemonTarget {
            daemon_id: "yggdrasil-repixelizer".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil repixelizer".to_string(),
            health_contract: health_contract("repixelizer.cultnet-rudp-service-health", "failed"),
            health_command: Some("health-yggdrasil-repixelizer.cmd".to_string()),
            deploy_command: Some("deploy-yggdrasil-repixelizer.cmd".to_string()),
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 300,
        };

        let targets = vec![
            odin,
            mimir,
            nightwing_eve_dashboard.clone(),
            nightwing_eve_browser_reference.clone(),
            vili.clone(),
            stonks,
            weksa.clone(),
            voidbot.clone(),
            nightwing_gjallar.clone(),
            starfire_muninn.clone(),
            nightwing_muninn.clone(),
            raven_muninn.clone(),
            yggdrasil_streampixels.clone(),
            yggdrasil_heimdall.clone(),
            yggdrasil_repixelizer.clone(),
        ];
        let plans = daemon_surgery_plans(&targets, "unix:100");
        let plan = swarm_surgery_plan("starfire-local", &targets, &plans, "unix:100");

        assert_eq!(plan.plan_id, "swarm-surgery:starfire-local");
        assert_eq!(plan.status, "active-transport-migration");
        assert_eq!(plan.next_target, "none");
        assert!(plan.current_phase.contains(
            "Nightwing Gjallar now consumes Odin's accepted gamecult.eve.surface_state snapshot over CultNet/RUDP"
        ));
        assert!(
            plan.cut_line.contains(
                "live yggdrasil-heimdall publishes heimdall.cultnet-rudp-provider-health"
            )
        );
        assert!(plan.cut_line.contains("GameCult\\Vili"));
        assert!(plan.cut_line.contains("10.77.0.4"));
        assert!(plan.cut_line.contains("Nightwing Muninn"));
        assert!(plan.cut_line.contains("voidbot-swarm-state.cc"));
        assert!(
            plan.cut_line
                .contains("/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc")
        );
        assert!(plan.cut_line.contains("starfire-muninn"));
        assert!(plan.cut_line.contains("127.0.0.1"));
        assert!(plan.cut_line.contains("10.77.0.3"));
        assert!(plan.cut_line.contains("GameCult-Muninn-Activate"));
        assert!(plan.cut_line.contains("GameCult-Muninn-VideoProof"));
        assert!(
            plan.cut_line
                .contains("hidden PowerShell entrypoints directly")
        );
        assert!(
            plan.cut_line
                .contains("manual compatibility entrypoints only")
        );
        assert!(plan.cut_line.contains("yggdrasil-streampixels"));
        assert!(plan.cut_line.contains("10.77.0.1"));
        assert!(
            plan.cut_line
                .contains("heimdall.cultnet-rudp-provider-health")
        );
        assert!(
            plan.cut_line
                .contains("/srv/heimdall/cultcache/heimdall.service.cc")
        );
        assert!(
            plan.cut_line
                .contains("repixelizer.cultnet-rudp-service-health")
        );
        assert!(
            plan.cut_line
                .contains("/srv/repixelizer/cultcache/repixelizer.service.cc")
        );
        assert!(
            plan.cut_line
                .contains("live Nightwing Gjallar publishes a boundary store")
        );
        assert!(
            plan.cut_line
                .contains("/var/lib/gamecult/gjallar/cultcache/gjallar.service.cc")
        );
        assert!(
            plan.cut_line
                .contains("Live Mimir Eve dashboard now publishes CultMesh state")
        );
        assert!(
            plan.cut_line
                .contains("/var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp")
        );
        assert!(
            plan.cut_line
                .contains("/var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc")
        );
        assert!(
            plan.cut_line
                .contains("live Nightwing Eve browser reference now publishes a boundary store")
        );
        assert!(plan.cut_line.contains(
            "/var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc"
        ));
        assert!(plan.cut_line.contains("surface:gamecult.network.status"));
        assert!(plan.cut_line.contains("192.168.1.66:17871"));
        assert!(plan.cut_line.contains("renderer-only lowering"));
        assert!(plan.verification_layer.contains("CultMesh keepalive store"));
        assert!(
            plan.invariants
                .iter()
                .any(|invariant| invariant.contains("cultnet.transport.rudp.v0"))
        );
        assert!(
            plan.invariants
                .iter()
                .any(|invariant| invariant.contains("Raven")
                    && invariant.contains("background-only")
                    && invariant.contains("visible terminal"))
        );
        assert!(
            plan.invariants
                .iter()
                .any(|invariant| invariant.contains("Raven")
                    && invariant.contains("Task Scheduler")
                    && invariant.contains(".cmd"))
        );

        let raven_plan = daemon_surgery_plan(&raven_muninn, "unix:100");
        assert_eq!(
            raven_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(raven_plan.current_mechanism.contains("10.77.0.2:17870"));
        assert!(raven_plan.current_mechanism.contains("GameCult-Muninn"));
        assert!(
            raven_plan
                .current_mechanism
                .contains("C:\\Meta\\Odin\\state\\muninn.telemetry.cc")
        );
        assert!(
            raven_plan
                .current_mechanism
                .contains("C:\\Meta\\Odin\\state\\muninn.activate.cc")
        );
        assert!(
            raven_plan
                .current_mechanism
                .contains("platform defaults")
        );
        assert!(raven_plan.cut_line.contains("fallback evidence only"));
        assert!(raven_plan.steps.iter().any(
            |step| step.contains("GameCult-Muninn-VideoProof") && step.contains("wscript.exe")
        ));
        assert!(raven_plan.steps.iter().any(
            |step| step.contains("PowerShell entrypoints directly") && step.contains("cmdPath")
        ));
        assert!(raven_plan.steps.iter().any(|step| {
            step.contains("muninn.activate.cc") && step.contains("activation path")
        }));
        assert!(raven_plan.steps.iter().any(|step| {
            step.contains("auto-claiming PS Move hosts") && step.contains("explicit Move flags")
        }));
        assert!(
            raven_plan
                .steps
                .iter()
                .any(|step| step.contains("--idunn-rudp-health 10.77.0.2:17870")
                    && step.contains("muninn.cultnet-rudp-remote-telemetry-health"))
        );
        assert!(raven_plan.blockers.is_empty());

        let starfire_plan = daemon_surgery_plan(&starfire_muninn, "unix:100");
        assert_eq!(
            starfire_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(starfire_plan.current_mechanism.contains("127.0.0.1:17870"));
        assert!(
            starfire_plan
                .current_mechanism
                .contains("archives a corrupt CultCache store")
        );
        assert!(
            starfire_plan
                .current_mechanism
                .contains("C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc")
        );
        assert!(starfire_plan.cut_line.contains("fallback evidence only"));
        assert!(starfire_plan.steps.iter().any(
            |step| step.contains("starfire.muninn.telemetry.cc") && step.contains(".lock")
        ));

        let nightwing_plan = daemon_surgery_plan(&nightwing_muninn, "unix:100");
        assert_eq!(
            nightwing_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(nightwing_plan.current_mechanism.contains("10.77.0.2:17870"));
        assert!(nightwing_plan.current_mechanism.contains("10.77.0.3"));
        assert!(
            nightwing_plan
                .current_mechanism
                .contains("/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc")
        );
        assert!(
            nightwing_plan
                .current_mechanism
                .contains("-DiscoverMoveState, -ClaimUsbMoves, or explicit -MoveState values")
        );
        assert!(nightwing_plan.cut_line.contains("fallback evidence only"));
        assert!(nightwing_plan.steps.iter().any(|step| {
            step.contains("/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc")
                && step.contains("provider advertisement")
        }));
        assert!(
            nightwing_plan
                .steps
                .iter()
                .any(|step| step.contains("restart-nightwing-muninn.ps1")
                    && step.contains("long-running serve body"))
        );
        assert!(nightwing_plan.steps.iter().any(|step| {
            step.contains("explicit -DiscoverMoveState/-ClaimUsbMoves")
                && step.contains("Move evidence/runtime arguments")
        }));

        let weksa_plan = daemon_surgery_plan(&weksa, "unix:100");
        assert_eq!(
            weksa_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(weksa_plan.current_mechanism.contains("command_boundary"));
        assert!(weksa_plan.current_mechanism.contains("weksa.mimo_voicedesign_command.v0"));
        assert!(weksa_plan.cut_line.contains("no HTTP endpoint may retake command authority"));
        assert!(weksa_plan.steps.iter().any(|step| {
            step.contains("speech_provider.mimo.voicedesign")
                && step.contains("CultNet/RUDP")
        }));

        let voidbot_plan = daemon_surgery_plan(&voidbot, "unix:100");
        assert_eq!(
            voidbot_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            voidbot_plan
                .current_mechanism
                .contains("voidbot.cultnet-rudp-stack-health")
        );
        assert!(
            voidbot_plan
                .current_mechanism
                .contains("voidbot-swarm-state.cc")
        );
        assert!(
            voidbot_plan
                .cut_line
                .contains("operations probe as fallback")
        );

        let gjallar_plan = daemon_surgery_plan(&nightwing_gjallar, "unix:100");
        assert_eq!(
            gjallar_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            gjallar_plan
                .current_mechanism
                .contains("gjallar.cultnet-rudp-framebuffer-composition-health")
        );
        assert!(gjallar_plan.cut_line.contains("gamecult.eve.surface_state"));
        assert!(
            gjallar_plan
                .current_mechanism
                .contains("surface:gamecult.network.status")
        );

        let nightwing_eve_dashboard_plan =
            daemon_surgery_plan(&nightwing_eve_dashboard, "unix:100");
        assert_eq!(
            nightwing_eve_dashboard_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            nightwing_eve_dashboard_plan
                .current_mechanism
                .contains("nightwing.cultnet-rudp-eve-dashboard-health")
        );
        assert!(
            nightwing_eve_dashboard_plan
                .current_mechanism
                .contains("/var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp")
        );
        assert!(
            nightwing_eve_dashboard_plan
                .current_mechanism
                .contains("/var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc")
        );
        assert!(
            nightwing_eve_dashboard_plan
                .steps
                .iter()
                .any(|step| step.contains("Mimir.EveDashboard systemd process"))
        );

        let nightwing_eve_browser_reference_plan =
            daemon_surgery_plan(&nightwing_eve_browser_reference, "unix:100");
        assert_eq!(
            nightwing_eve_browser_reference_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            nightwing_eve_browser_reference_plan
                .current_mechanism
                .contains("nightwing.cultnet-rudp-browser-reference-health")
        );
        assert!(
            nightwing_eve_browser_reference_plan
                .current_mechanism
                .contains("/var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc")
        );
        assert!(
            nightwing_eve_browser_reference_plan
                .steps
                .iter()
                .any(|step| step.contains("Mimir.EveBrowserReference service process"))
        );

        let vili_plan = daemon_surgery_plan(&vili, "unix:100");
        assert_eq!(
            vili_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(vili_plan.current_mechanism.contains("vili.service.cc"));
        assert!(vili_plan.current_mechanism.contains("GameCult\\Vili"));
        assert!(
            vili_plan
                .steps
                .iter()
                .any(|step| step.contains("command_boundary"))
        );
        assert!(
            vili_plan
                .steps
                .iter()
                .any(|step| step.contains("0.0.0.0:17870") && step.contains("Raven"))
        );
        assert!(vili_plan.blockers.is_empty());

        let streampixels_plan = daemon_surgery_plan(&yggdrasil_streampixels, "unix:100");
        assert_eq!(
            streampixels_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            streampixels_plan
                .current_mechanism
                .contains("10.77.0.2:17870")
        );
        assert!(streampixels_plan.current_mechanism.contains("10.77.0.1"));
        assert!(
            streampixels_plan
                .current_mechanism
                .contains("/srv/streampixels/env/service.env")
        );
        assert!(
            streampixels_plan
                .cut_line
                .contains("deployment/debug witnesses")
        );
        assert!(
            streampixels_plan
                .steps
                .iter()
                .any(|step| step.contains("CultLib cultnet-ts/cultcache-ts snapshot"))
        );
        assert!(
            streampixels_plan
                .steps
                .iter()
                .any(|step| step.contains("serial pnpm workspace build"))
        );

        let heimdall_plan = daemon_surgery_plan(&yggdrasil_heimdall, "unix:100");
        assert_eq!(
            heimdall_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            heimdall_plan
                .current_mechanism
                .contains("/srv/heimdall/cultcache/heimdall.service.cc")
        );
        assert!(
            heimdall_plan
                .steps
                .iter()
                .any(|step| step.contains("CultLib cultnet-ts/cultcache-ts snapshot"))
        );
        assert!(
            heimdall_plan
                .steps
                .iter()
                .any(|step| step.contains("JWKS") && step.contains("discovery"))
        );

        let repixelizer_plan = daemon_surgery_plan(&yggdrasil_repixelizer, "unix:100");
        assert_eq!(
            repixelizer_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            repixelizer_plan
                .current_mechanism
                .contains("/srv/repixelizer/cultcache/repixelizer.service.cc")
        );
        assert!(
            repixelizer_plan
                .steps
                .iter()
                .any(|step| step.contains("CultLib cultcache-py snapshot"))
        );
    }

    #[test]
    fn fresh_daemon_published_rudp_health_can_replace_probe_health() {
        let desired = IdunnDesiredDaemonRecord {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            enabled: true,
            health_command: Some("exit 1".to_string()),
            restart_command: Some("restart test".to_string()),
            authority: "idunn.local-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
            deploy_command: None,
            health_contract: "test.cultnet-rudp-health".to_string(),
            transport_profile_id: "transport:test-daemon".to_string(),
            command_boundary_id: "command-boundary:test-daemon".to_string(),
        };
        let mut health = IdunnDaemonHealthRecord {
            daemon_id: "test-daemon".to_string(),
            state: "active".to_string(),
            detail: "daemon published".to_string(),
            observed_at: "unix:95".to_string(),
            health_contract: "test.cultnet-rudp-health".to_string(),
            publication_source: "daemon-published".to_string(),
            transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
        };

        assert!(is_fresh_daemon_published_health(
            &health, &desired, "unix:100"
        ));

        health.observed_at = "1970-01-01T00:01:35.0000000+00:00".to_string();
        assert!(is_fresh_daemon_published_health(
            &health, &desired, "unix:100"
        ));

        health.observed_at = "unix:1".to_string();
        assert!(!is_fresh_daemon_published_health(
            &health, &desired, "unix:100"
        ));

        health.observed_at = "unix:95".to_string();
        health.publication_source = "compatibility-command".to_string();
        assert!(!is_fresh_daemon_published_health(
            &health, &desired, "unix:100"
        ));
    }

    #[test]
    fn evaluate_target_health_keeps_fresh_daemon_published_record_on_owner_key() {
        let now_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let now = format!("unix:{now_seconds}");
        let store_path = std::env::temp_dir().join(format!(
            "idunn-test-store-{}.cc",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = CommonOptions {
            store_path: store_path.clone(),
            operator_alarm_command: None,
            rudp_health_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        let store_lock = Arc::new(Mutex::new(()));
        let target = DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.cultnet-rudp-health", "failed"),
            health_command: Some("__idunn_probe_should_not_run__".to_string()),
            deploy_command: None,
            restart_command: Some("restart test".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let desired = IdunnDesiredDaemonRecord {
            daemon_id: target.daemon_id.clone(),
            verse_id: target.verse_id.clone(),
            name: target.name.clone(),
            enabled: target.enabled,
            health_command: target.health_command.clone(),
            restart_command: target.restart_command.clone(),
            deploy_command: target.deploy_command.clone(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
            authority: "idunn.local-command".to_string(),
            max_silence_seconds: 60,
            observed_at: now.clone(),
        };
        let health = IdunnDaemonHealthRecord {
            daemon_id: target.daemon_id.clone(),
            state: "active".to_string(),
            detail: "daemon published".to_string(),
            observed_at: now.clone(),
            health_contract: target.health_contract.id.clone(),
            publication_source: "daemon-published".to_string(),
            transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
        };

        with_store_node(&options, &store_lock, |node| {
            node.put(&health.daemon_id, &health)?;
            Ok(())
        })
        .unwrap();

        let (health_key, selected) =
            evaluate_target_health(&target, &options, &store_lock, &desired, &now).unwrap();

        assert_eq!(health_key, "test-daemon");
        assert_eq!(selected.publication_source, "daemon-published");

        let _ = std::fs::remove_file(store_path);
    }

    #[test]
    fn evaluate_target_health_uses_compatibility_lane_when_no_fresh_daemon_health_exists() {
        let store_path = std::env::temp_dir().join(format!(
            "idunn-test-store-{}.cc",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = CommonOptions {
            store_path: store_path.clone(),
            operator_alarm_command: None,
            rudp_health_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        let store_lock = Arc::new(Mutex::new(()));
        let target = DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.cultnet-rudp-health", "failed"),
            health_command: Some("__idunn_probe_should_fail__".to_string()),
            deploy_command: None,
            restart_command: Some("restart test".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let desired = IdunnDesiredDaemonRecord {
            daemon_id: target.daemon_id.clone(),
            verse_id: target.verse_id.clone(),
            name: target.name.clone(),
            enabled: target.enabled,
            health_command: target.health_command.clone(),
            restart_command: target.restart_command.clone(),
            deploy_command: target.deploy_command.clone(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
            authority: "idunn.local-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
        };

        let (health_key, selected) =
            evaluate_target_health(&target, &options, &store_lock, &desired, "unix:100").unwrap();

        assert_eq!(health_key, "compat-health:test-daemon");
        assert_eq!(selected.publication_source, "compatibility-command");

        let _ = std::fs::remove_file(store_path);
    }

    #[test]
    fn stonks_transport_profile_marks_provider_store_live() {
        let stonks = DaemonTarget {
            daemon_id: "stonks".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Stonks".to_string(),
            health_contract: health_contract("stonks.cultnet-rudp-market-health", "failed"),
            health_command: Some("health-stonks.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-stonks.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };

        let profile = daemon_transport_profile(&stonks, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert!(
            profile
                .current_transport
                .contains("daemon-owned-cultcache-provider-store")
        );
        assert!(profile.cut_line.contains("HTTP/WebSocket"));
    }

    #[test]
    fn weksa_transport_profile_marks_provider_store_live() {
        let weksa = DaemonTarget {
            daemon_id: "weksa".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Weksa".to_string(),
            health_contract: health_contract("weksa.cultnet-rudp-provider-health", "failed"),
            health_command: Some("health-weksa.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-weksa.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 60,
        };

        let profile = daemon_transport_profile(&weksa, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert_eq!(
            profile.current_transport,
            "daemon-published-rudp-health + daemon-owned-cultcache-provider-store + daemon-owned-rudp-command-ingress"
        );
        assert!(profile.cut_line.contains("weksa.mimo_voicedesign_command.v0"));
    }

    #[test]
    fn streampixels_transport_profile_marks_provider_store_live() {
        let streampixels = DaemonTarget {
            daemon_id: "yggdrasil-streampixels".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil StreamPixels".to_string(),
            health_contract: health_contract("streampixels.cultnet-rudp-service-health", "failed"),
            health_command: Some("health-yggdrasil-streampixels.cmd".to_string()),
            deploy_command: Some("deploy-yggdrasil-streampixels.cmd".to_string()),
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 300,
        };

        let profile = daemon_transport_profile(&streampixels, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert!(
            profile
                .current_transport
                .contains("daemon-owned-cultcache-service-boundary")
        );
        assert!(!profile.current_transport.contains("local-proof"));
        assert!(profile.cut_line.contains("10.77.0.1"));
        assert!(profile.cut_line.contains("service-owned witness"));
    }

    #[test]
    fn heimdall_transport_profile_marks_boundary_store_live() {
        let heimdall = DaemonTarget {
            daemon_id: "yggdrasil-heimdall".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil Heimdall".to_string(),
            health_contract: health_contract("heimdall.cultnet-rudp-provider-health", "failed"),
            health_command: Some("health-yggdrasil-heimdall.cmd".to_string()),
            deploy_command: Some("deploy-yggdrasil-heimdall.cmd".to_string()),
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 300,
        };

        let profile = daemon_transport_profile(&heimdall, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert!(
            profile
                .current_transport
                .contains("daemon-owned-cultcache-boundary-store")
        );
        assert!(
            profile
                .cut_line
                .contains("/srv/heimdall/cultcache/heimdall.service.cc")
        );
    }

    #[test]
    fn repixelizer_transport_profile_marks_rudp_boundary_store_live() {
        let repixelizer = DaemonTarget {
            daemon_id: "yggdrasil-repixelizer".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil repixelizer".to_string(),
            health_contract: health_contract("repixelizer.cultnet-rudp-service-health", "failed"),
            health_command: Some("health-yggdrasil-repixelizer.cmd".to_string()),
            deploy_command: Some("deploy-yggdrasil-repixelizer.cmd".to_string()),
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 300,
        };

        let profile = daemon_transport_profile(&repixelizer, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert!(
            profile
                .current_transport
                .contains("daemon-published-rudp-health")
        );
        assert!(
            profile
                .cut_line
                .contains("/srv/repixelizer/cultcache/repixelizer.service.cc")
        );
    }

    #[test]
    fn voidbot_transport_profile_marks_provider_store_live() {
        let voidbot = DaemonTarget {
            daemon_id: "voidbot".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "VoidBot local stack".to_string(),
            health_contract: health_contract("voidbot.cultnet-rudp-stack-health", "failed"),
            health_command: Some("health-voidbot.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-voidbot.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 60,
        };

        let profile = daemon_transport_profile(&voidbot, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert_eq!(
            profile.current_transport,
            "daemon-published-rudp-health + daemon-owned-cultmesh-provider-store + compatibility.local-command fallback"
        );
        assert!(profile.cut_line.contains("voidbot-swarm-state.cc"));
    }

    #[test]
    fn gjallar_transport_profile_marks_partial_rudp_health() {
        let gjallar = DaemonTarget {
            daemon_id: "nightwing-gjallar".to_string(),
            verse_id: "nightwing.local".to_string(),
            name: "Nightwing Gjallar framebuffer compositor".to_string(),
            health_contract: health_contract(
                "gjallar.cultnet-rudp-framebuffer-composition-health",
                "failed",
            ),
            health_command: Some("health-nightwing-gjallar.ps1".to_string()),
            deploy_command: Some("deploy-nightwing-gjallar.ps1".to_string()),
            restart_command: Some("restart-nightwing-gjallar.ps1".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };

        let profile = daemon_transport_profile(&gjallar, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert_eq!(
            profile.current_transport,
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary + native-odin-cultnet-rudp-snapshot-input + compatibility.local-command fallback"
        );
        assert!(
            profile
                .cut_line
                .contains("Gjallar framebuffer composition health is published over CultNet/RUDP")
        );
        assert!(profile.cut_line.contains("surface:gamecult.network.status"));
    }

    #[test]
    fn muninn_transport_profiles_mark_live_rudp_health() {
        let raven = DaemonTarget {
            daemon_id: "muninn".to_string(),
            verse_id: "raven.local".to_string(),
            name: "Raven Muninn".to_string(),
            health_contract: health_contract(
                "muninn.cultnet-rudp-remote-telemetry-health",
                "failed",
            ),
            health_command: Some("health-muninn.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-muninn.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let starfire = DaemonTarget {
            daemon_id: "starfire-muninn".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Starfire Muninn".to_string(),
            health_contract: health_contract(
                "muninn.cultnet-rudp-local-telemetry-and-quest-access",
                "failed",
            ),
            health_command: Some("health-starfire-muninn.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-starfire-muninn.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let nightwing = DaemonTarget {
            daemon_id: "nightwing-muninn".to_string(),
            verse_id: "nightwing.local".to_string(),
            name: "Nightwing Muninn".to_string(),
            health_contract: health_contract(
                "muninn.cultnet-rudp-remote-telemetry-and-move-hid",
                "failed",
            ),
            health_command: Some("health-nightwing-muninn.cmd".to_string()),
            deploy_command: None,
            restart_command: Some("restart-nightwing-muninn.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };

        let raven_profile = daemon_transport_profile(&raven, "unix:100");
        assert_eq!(
            raven_profile.state,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            raven_profile
                .current_transport
                .contains("daemon-owned-cultcache-telemetry-store")
        );
        assert!(
            raven_profile
                .current_transport
                .contains("background-only hidden task launch")
        );
        assert!(
            raven_profile
                .cut_line
                .contains("C:\\Meta\\Odin\\state\\muninn.telemetry.cc")
        );

        let starfire_profile = daemon_transport_profile(&starfire, "unix:100");
        assert_eq!(
            starfire_profile.state,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            starfire_profile
                .current_transport
                .contains("daemon-owned-cultcache-telemetry-store")
        );
        assert!(starfire_profile.cut_line.contains("Quest ADB availability"));
        assert!(
            starfire_profile
                .cut_line
                .contains("C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc")
        );

        let nightwing_profile = daemon_transport_profile(&nightwing, "unix:100");
        assert_eq!(
            nightwing_profile.state,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            nightwing_profile
                .current_transport
                .contains("daemon-owned-cultcache-telemetry-store")
        );
        assert!(nightwing_profile.cut_line.contains("Move HID evidence"));
        assert!(
            nightwing_profile
                .cut_line
                .contains("/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc")
        );
    }

    #[test]
    fn runtime_transport_check_proves_rudp_loopback() {
        let check = runtime_transport_check("2026-06-15T00:00:00Z");

        assert_eq!(check.check_id, "idunn-runtime-rudp-loopback");
        assert_eq!(check.transport, CULTNET_RUDP_PROTOCOL_ID);
        assert_eq!(check.state, "available");
    }

    #[test]
    fn rudp_health_ingress_accepts_raw_daemon_health_document() {
        let health = IdunnDaemonHealthRecord {
            daemon_id: "test-daemon".to_string(),
            state: "active".to_string(),
            detail: "published by daemon".to_string(),
            observed_at: "2026-06-15T00:00:00Z".to_string(),
            health_contract: "test.cultnet-rudp-health".to_string(),
            publication_source: "daemon-published".to_string(),
            transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
        };
        let message = CultNetMessage::DocumentPutRaw {
            message_id: "test-health".to_string(),
            document: CultNetRawDocumentRecord {
                schema_id: "idunn.daemon_health".to_string(),
                record_key: health.daemon_id.clone(),
                stored_at: "2026-06-15T00:00:00Z".to_string(),
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: rmp_serde::to_vec(&health).unwrap(),
                source_runtime_id: Some("test-daemon".to_string()),
                source_agent_id: None,
                source_role: Some("daemon-health-publisher".to_string()),
                tags: Some(vec!["cultnet.transport.rudp.v0".to_string()]),
            },
        };

        assert_eq!(health_from_rudp_message(&message).unwrap(), health);
    }
}

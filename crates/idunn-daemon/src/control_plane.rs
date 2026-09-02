use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cultcache_rs::{
    CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore,
};
use uuid::Uuid;

use crate::deployment::OperatorBinding;
use crate::drivers::ProcessIdentity;

const DEPLOYMENT_COMMAND_SCHEMA: &str = "idunn.deployment_command.v1";

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.deployment_command",
    schema = "idunn.deployment_command.v1"
)]
struct DeploymentCommandRecord {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    command_id: String,
    #[cultcache(key = 2)]
    selector: String,
    #[cultcache(key = 3)]
    requested_by: String,
    #[cultcache(key = 4)]
    requested_at_unix_millis: u64,
    #[cultcache(key = 5)]
    state: String,
    #[cultcache(key = 6)]
    owner_instance_id: Option<String>,
    #[cultcache(key = 7)]
    started_at_unix_millis: Option<u64>,
    #[cultcache(key = 8)]
    completed_at_unix_millis: Option<u64>,
    #[cultcache(key = 9)]
    detail: String,
}

impl DeploymentCommandRecord {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == DEPLOYMENT_COMMAND_SCHEMA,
            "deployment command schema is unsupported"
        );
        require_id(&self.command_id, "deployment command id")?;
        require_selector(&self.selector)?;
        require_value(&self.requested_by, "deployment requester")?;
        ensure!(
            self.requested_at_unix_millis > 0,
            "deployment command has no request time"
        );
        ensure!(
            matches!(
                self.state.as_str(),
                "pending" | "running" | "succeeded" | "failed"
            ),
            "deployment command state is invalid"
        );
        ensure!(
            self.detail.len() <= 2048 && !self.detail.contains('\0'),
            "deployment command detail is invalid"
        );
        match self.state.as_str() {
            "pending" => {
                ensure!(
                    self.owner_instance_id.is_none()
                        && self.started_at_unix_millis.is_none()
                        && self.completed_at_unix_millis.is_none(),
                    "pending deployment command has execution state"
                );
            }
            "running" => {
                ensure!(
                    self.owner_instance_id.is_some()
                        && self.started_at_unix_millis.is_some()
                        && self.completed_at_unix_millis.is_none(),
                    "running deployment command has partial ownership"
                );
            }
            "succeeded" | "failed" => {
                ensure!(
                    self.owner_instance_id.is_some()
                        && self.started_at_unix_millis.is_some()
                        && self.completed_at_unix_millis.is_some(),
                    "terminal deployment command has partial receipt state"
                );
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RuntimeOptions {
    state_store: PathBuf,
    bindings_dir: PathBuf,
    source_root: PathBuf,
    staging_root: PathBuf,
    topology_store: PathBuf,
    odin_correlation_store: PathBuf,
    odin_trust_anchor: PathBuf,
    source_identity: Option<ProcessIdentity>,
    poll_millis: u64,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            state_store: PathBuf::from("/var/lib/gamecult/idunn/control.cc"),
            bindings_dir: PathBuf::from("/etc/gamecult/idunn/bindings"),
            source_root: PathBuf::from("/var/lib/gamecult/idunn/sources"),
            staging_root: PathBuf::from("/var/lib/gamecult/idunn/staging"),
            topology_store: PathBuf::from("/var/lib/gamecult/idunn/topology.cc"),
            odin_correlation_store: PathBuf::from(
                "/var/lib/gamecult/odin/idunn-runtime-topology.cc",
            ),
            odin_trust_anchor: PathBuf::from("/etc/gamecult/idunn/odin-topology-anchor.cc"),
            source_identity: None,
            poll_millis: 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Command {
    Serve(RuntimeOptions),
    Up {
        selector: String,
        requested_by: String,
        state_store: PathBuf,
        wait: bool,
        timeout_seconds: u64,
    },
    Status {
        state_store: PathBuf,
        command_id: Option<String>,
    },
}

pub fn run(args: impl Iterator<Item = String>) -> Result<()> {
    match parse(args)? {
        Command::Serve(options) => serve(options),
        Command::Up {
            selector,
            requested_by,
            state_store,
            wait,
            timeout_seconds,
        } => submit(
            &state_store,
            &selector,
            &requested_by,
            wait,
            timeout_seconds,
        ),
        Command::Status {
            state_store,
            command_id,
        } => status(&state_store, command_id.as_deref()),
    }
}

fn parse(args: impl Iterator<Item = String>) -> Result<Command> {
    let mut args = args.peekable();
    let command = args.next().ok_or_else(|| anyhow!(usage()))?;
    match command.as_str() {
        "serve" => parse_serve(args),
        "up" => parse_up(args),
        "status" => parse_status(args),
        "--help" | "-h" | "help" => bail!(usage()),
        _ => bail!("unknown Idunn command {command:?}\n\n{}", usage()),
    }
}

fn parse_serve(mut args: impl Iterator<Item = String>) -> Result<Command> {
    let mut options = RuntimeOptions::default();
    let mut source_uid = None;
    let mut source_gid = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--state-store" => options.state_store = path_value(&mut args, &argument)?,
            "--bindings-dir" => options.bindings_dir = path_value(&mut args, &argument)?,
            "--source-root" => options.source_root = path_value(&mut args, &argument)?,
            "--staging-root" => options.staging_root = path_value(&mut args, &argument)?,
            "--topology-store" => options.topology_store = path_value(&mut args, &argument)?,
            "--odin-correlation-store" => {
                options.odin_correlation_store = path_value(&mut args, &argument)?
            }
            "--odin-trust-anchor" => options.odin_trust_anchor = path_value(&mut args, &argument)?,
            "--source-uid" => source_uid = Some(u32_value(&mut args, &argument)?),
            "--source-gid" => source_gid = Some(u32_value(&mut args, &argument)?),
            "--poll-millis" => options.poll_millis = u64_value(&mut args, &argument)?,
            "--help" | "-h" => bail!(usage()),
            _ => bail!("unknown Idunn serve option {argument:?}"),
        }
    }
    ensure!(options.poll_millis > 0, "--poll-millis must be positive");
    options.source_identity = match (source_uid, source_gid) {
        (Some(uid), Some(gid)) => {
            ensure!(
                uid > 0 && gid > 0,
                "source UID and GID must be unprivileged"
            );
            Some(ProcessIdentity { uid, gid })
        }
        (None, None) => None,
        _ => bail!("--source-uid and --source-gid must be supplied together"),
    };
    Ok(Command::Serve(options))
}

fn parse_up(mut args: impl Iterator<Item = String>) -> Result<Command> {
    let selector = args
        .next()
        .ok_or_else(|| anyhow!("idunn up requires a service or profile selector"))?;
    require_selector(&selector)?;
    let mut state_store = RuntimeOptions::default().state_store;
    let mut requested_by = env::var("SUDO_USER")
        .or_else(|_| env::var("USER"))
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "operator".into());
    let mut wait = true;
    let mut timeout_seconds = 1800;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--state-store" => state_store = path_value(&mut args, &argument)?,
            "--requested-by" => requested_by = string_value(&mut args, &argument)?,
            "--no-wait" => wait = false,
            "--timeout-seconds" => timeout_seconds = u64_value(&mut args, &argument)?,
            "--help" | "-h" => bail!(usage()),
            _ => bail!("unknown Idunn up option {argument:?}"),
        }
    }
    ensure!(timeout_seconds > 0, "--timeout-seconds must be positive");
    require_value(&requested_by, "deployment requester")?;
    Ok(Command::Up {
        selector,
        requested_by,
        state_store,
        wait,
        timeout_seconds,
    })
}

fn parse_status(mut args: impl Iterator<Item = String>) -> Result<Command> {
    let mut state_store = RuntimeOptions::default().state_store;
    let mut command_id = None;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--state-store" => state_store = path_value(&mut args, &argument)?,
            "--command" => command_id = Some(string_value(&mut args, &argument)?),
            "--help" | "-h" => bail!(usage()),
            _ => bail!("unknown Idunn status option {argument:?}"),
        }
    }
    Ok(Command::Status {
        state_store,
        command_id,
    })
}

fn serve(options: RuntimeOptions) -> Result<()> {
    for path in [
        &options.state_store,
        &options.topology_store,
        &options.staging_root,
    ] {
        let directory = if path.extension().is_some() {
            path.parent()
                .context("configured Idunn path has no parent")?
        } else {
            path.as_path()
        };
        fs::create_dir_all(directory)
            .with_context(|| format!("creating Idunn directory {}", directory.display()))?;
    }
    let instance_id = format!("idunn-{}", Uuid::new_v4());
    loop {
        if let Some(command) = claim_next(&options.state_store, &instance_id)? {
            let result = execute(&options, &command);
            complete(&options.state_store, &command, result)?;
        } else {
            thread::sleep(Duration::from_millis(options.poll_millis));
        }
    }
}

fn execute(options: &RuntimeOptions, command: &DeploymentCommandRecord) -> Result<String> {
    let bindings = load_bindings(&options.bindings_dir)?;
    let targets = resolve_selector(&bindings, &command.selector)?;
    ensure!(
        !targets.is_empty(),
        "deployment selector resolved to no targets"
    );
    bail!(
        "typed deployment drivers are not yet connected for {}",
        targets.join(",")
    )
}

fn load_bindings(directory: &Path) -> Result<BTreeMap<String, OperatorBinding>> {
    let mut paths = fs::read_dir(directory)
        .with_context(|| format!("reading Idunn binding directory {}", directory.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.retain(|path| path.extension() == Some(std::ffi::OsStr::new("toml")));
    paths.sort();
    let mut bindings = BTreeMap::new();
    for path in paths {
        let bytes = fs::read(&path)
            .with_context(|| format!("reading operator binding {}", path.display()))?;
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("operator binding {} is not UTF-8", path.display()))?;
        let binding = OperatorBinding::parse(text)
            .with_context(|| format!("validating operator binding {}", path.display()))?;
        let target = binding.target.clone();
        ensure!(
            bindings.insert(target.clone(), binding).is_none(),
            "operator binding target {target} is duplicated"
        );
    }
    ensure!(!bindings.is_empty(), "Idunn binding directory is empty");
    Ok(bindings)
}

fn resolve_selector(
    bindings: &BTreeMap<String, OperatorBinding>,
    selector: &str,
) -> Result<Vec<String>> {
    let targets = if let Some(profile) = selector.strip_prefix("profile:") {
        bindings
            .values()
            .filter(|binding| binding.profiles.contains(profile))
            .map(|binding| binding.target.clone())
            .collect::<Vec<_>>()
    } else {
        ensure!(
            bindings.contains_key(selector),
            "unknown deployment target {selector}"
        );
        vec![selector.to_owned()]
    };
    ensure!(
        !targets.is_empty(),
        "unknown or empty deployment profile {selector}"
    );
    Ok(targets)
}

fn submit(
    store_path: &Path,
    selector: &str,
    requested_by: &str,
    wait: bool,
    timeout_seconds: u64,
) -> Result<()> {
    if let Some(parent) = store_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let now = now_millis()?;
    let record = DeploymentCommandRecord {
        schema_version: DEPLOYMENT_COMMAND_SCHEMA.into(),
        command_id: format!("up-{}", Uuid::new_v4()),
        selector: selector.into(),
        requested_by: requested_by.into(),
        requested_at_unix_millis: now,
        state: "pending".into(),
        owner_instance_id: None,
        started_at_unix_millis: None,
        completed_at_unix_millis: None,
        detail: String::new(),
    };
    record.validate()?;
    let store = SingleFileMessagePackBackingStore::new(store_path);
    let next = typed_envelope(&record, now)?;
    if !store.compare_exchange(
        &[CultCacheExpectedEnvelope {
            r#type: DeploymentCommandRecord::TYPE.into(),
            key: record.command_id.clone(),
            current: None,
        }],
        &[next],
    )? {
        bail!("deployment command id collided")
    }
    println!("{}", record.command_id);
    if !wait {
        return Ok(());
    }
    let deadline = now.saturating_add(timeout_seconds.saturating_mul(1000));
    loop {
        let current = read_command(store_path, &record.command_id)?
            .context("submitted deployment command disappeared")?;
        match current.state.as_str() {
            "succeeded" => {
                println!("succeeded {}", current.detail);
                return Ok(());
            }
            "failed" => bail!("deployment failed: {}", current.detail),
            _ if now_millis()? >= deadline => bail!("deployment command timed out"),
            _ => thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn status(store_path: &Path, command_id: Option<&str>) -> Result<()> {
    let mut commands = read_commands(store_path)?;
    if let Some(command_id) = command_id {
        commands.retain(|command| command.command_id == command_id);
        ensure!(!commands.is_empty(), "deployment command is unknown");
    }
    commands.sort_by_key(|command| command.requested_at_unix_millis);
    for command in commands {
        println!(
            "{} {} {} {}",
            command.command_id, command.selector, command.state, command.detail
        );
    }
    Ok(())
}

fn claim_next(store_path: &Path, instance_id: &str) -> Result<Option<DeploymentCommandRecord>> {
    let store = SingleFileMessagePackBackingStore::new(store_path);
    let mut candidates = read_command_envelopes(store_path)?
        .into_iter()
        .filter(|(_, record)| record.state == "pending")
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(_, command)| command.requested_at_unix_millis);
    for (current, mut command) in candidates {
        let now = now_millis()?;
        command.state = "running".into();
        command.owner_instance_id = Some(instance_id.into());
        command.started_at_unix_millis = Some(now);
        command.validate()?;
        if store.compare_exchange(
            &[CultCacheExpectedEnvelope {
                r#type: DeploymentCommandRecord::TYPE.into(),
                key: command.command_id.clone(),
                current: Some(current),
            }],
            &[typed_envelope(&command, now)?],
        )? {
            return Ok(Some(command));
        }
    }
    Ok(None)
}

fn complete(
    store_path: &Path,
    running: &DeploymentCommandRecord,
    result: Result<String>,
) -> Result<()> {
    let store = SingleFileMessagePackBackingStore::new(store_path);
    let current = read_command_envelopes(store_path)?
        .into_iter()
        .find(|(_, command)| command.command_id == running.command_id)
        .context("running deployment command disappeared")?;
    ensure!(
        current.1 == *running,
        "deployment command changed while its owner was executing"
    );
    let now = now_millis()?;
    let mut terminal = running.clone();
    terminal.completed_at_unix_millis = Some(now);
    match result {
        Ok(detail) => {
            terminal.state = "succeeded".into();
            terminal.detail = detail;
        }
        Err(error) => {
            terminal.state = "failed".into();
            terminal.detail = truncate(&format!("{error:#}"), 2048);
        }
    }
    terminal.validate()?;
    ensure!(
        store.compare_exchange(
            &[CultCacheExpectedEnvelope {
                r#type: DeploymentCommandRecord::TYPE.into(),
                key: terminal.command_id.clone(),
                current: Some(current.0),
            }],
            &[typed_envelope(&terminal, now)?],
        )?,
        "deployment command changed before its terminal receipt"
    );
    Ok(())
}

fn read_commands(path: &Path) -> Result<Vec<DeploymentCommandRecord>> {
    Ok(read_command_envelopes(path)?
        .into_iter()
        .map(|(_, record)| record)
        .collect())
}

fn read_command(path: &Path, command_id: &str) -> Result<Option<DeploymentCommandRecord>> {
    Ok(read_commands(path)?
        .into_iter()
        .find(|command| command.command_id == command_id))
}

fn read_command_envelopes(
    path: &Path,
) -> Result<Vec<(CultCacheEnvelope, DeploymentCommandRecord)>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    SingleFileMessagePackBackingStore::new(path)
        .pull_all_read_only_snapshot()?
        .into_iter()
        .map(|envelope| {
            ensure!(
                envelope.r#type == DeploymentCommandRecord::TYPE
                    && envelope.schema_id.as_deref() == Some(DEPLOYMENT_COMMAND_SCHEMA),
                "Idunn control store contains a foreign document"
            );
            let record: DeploymentCommandRecord = rmp_serde::from_slice(&envelope.payload)?;
            ensure!(
                rmp_serde::to_vec(&record)? == envelope.payload
                    && envelope.key == record.command_id,
                "Idunn control store contains a noncanonical command"
            );
            record.validate()?;
            Ok((envelope, record))
        })
        .collect()
}

fn typed_envelope(record: &DeploymentCommandRecord, millis: u64) -> Result<CultCacheEnvelope> {
    record.validate()?;
    let stored_at = chrono::DateTime::from_timestamp_millis(i64::try_from(millis)?)
        .context("deployment command timestamp is out of range")?
        .to_rfc3339();
    Ok(CultCacheEnvelope {
        key: record.command_id.clone(),
        r#type: DeploymentCommandRecord::TYPE.into(),
        payload: rmp_serde::to_vec(record)?,
        stored_at,
        schema_id: Some(DEPLOYMENT_COMMAND_SCHEMA.into()),
    })
}

fn now_millis() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_millis()
        .try_into()?)
}

fn string_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn path_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(string_value(args, name)?))
}

fn u32_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<u32> {
    string_value(args, name)?
        .parse()
        .with_context(|| format!("{name} requires a u32"))
}

fn u64_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<u64> {
    string_value(args, name)?
        .parse()
        .with_context(|| format!("{name} requires a u64"))
}

fn require_selector(value: &str) -> Result<()> {
    let value = value.strip_prefix("profile:").unwrap_or(value);
    require_id(value, "deployment selector")
}

fn require_id(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
            }),
        "{label} is invalid"
    );
    Ok(())
}

fn require_value(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty()
            && value == value.trim()
            && value.len() <= 1024
            && !value.contains('\0'),
        "{label} is invalid"
    );
    Ok(())
}

fn truncate(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

fn usage() -> &'static str {
    "Idunn deployment and continuity control plane\n\n\
     idunn serve [runtime options]\n\
     idunn up <service|profile:name> [--state-store PATH] [--no-wait]\n\
     idunn status [--state-store PATH] [--command ID]\n\n\
     Recipes declare build, package, launch, health, state, and capability semantics.\n\
     Operator bindings select source, runner, workload, route, brakes, and placement."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_has_no_raw_command_or_compiled_profile_authority() {
        for arguments in [
            vec!["up", "ghostlight", "--deploy-command", "sh -c bad"],
            vec!["serve", "--swarm-profile", "yggdrasil-local"],
            vec!["serve", "--restart-command", "bad"],
            vec!["validate-runtime-admission"],
        ] {
            assert!(parse(arguments.into_iter().map(str::to_owned)).is_err());
        }
    }

    #[test]
    fn garden_path_accepts_service_and_profile_selectors() {
        for selector in ["ghostlight", "profile:aetheria", "profile:full-gamecult"] {
            let parsed =
                parse(["up", selector, "--no-wait"].into_iter().map(str::to_owned)).unwrap();
            let Command::Up {
                selector: actual, ..
            } = parsed
            else {
                panic!("expected up command")
            };
            assert_eq!(actual, selector);
        }
    }

    #[test]
    fn source_identity_is_explicit_and_atomic() {
        let partial = parse(
            ["serve", "--source-uid", "1001"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap_err()
        .to_string();
        assert!(partial.contains("supplied together"));
        let Command::Serve(options) = parse(
            ["serve", "--source-uid", "1001", "--source-gid", "1002"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap() else {
            panic!("expected serve")
        };
        assert_eq!(
            options.source_identity,
            Some(ProcessIdentity {
                uid: 1001,
                gid: 1002
            })
        );
    }

    #[test]
    fn deployment_command_receipt_is_positional_and_fail_closed() -> Result<()> {
        let record = DeploymentCommandRecord {
            schema_version: DEPLOYMENT_COMMAND_SCHEMA.into(),
            command_id: "up-test".into(),
            selector: "ghostlight".into(),
            requested_by: "operator".into(),
            requested_at_unix_millis: 100,
            state: "pending".into(),
            owner_instance_id: None,
            started_at_unix_millis: None,
            completed_at_unix_millis: None,
            detail: String::new(),
        };
        record.validate()?;
        let payload = rmp_serde::to_vec(&record)?;
        assert_eq!(payload[0], 0x9a);
        let mut invalid = record;
        invalid.state = "succeeded".into();
        assert!(invalid.validate().is_err());
        Ok(())
    }
}

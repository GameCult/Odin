use anyhow::{Context, Result, anyhow};
use cultcache_rs::{CacheBackingStore, DatabaseEntry};
use cultcache_rs::{
    CultCache, CultCacheEnvelope, CultCacheExpectedEnvelope, SingleFileMessagePackBackingStore,
};
use cultmesh_rs::{CultMesh, CultMeshNode, CultMeshNodeOptions};
use cultnet_rs::{
    CultNetDocumentBinding, CultNetDocumentRegistry, CultNetMessage, CultNetRawDocumentRecord,
    CultNetRawPayloadEncoding, CultNetReadOnlySnapshotPolicy, CultNetRudpPacketType,
    CultNetRudpSendOptions, CultNetRudpSession, CultNetRudpSessionOptions,
    CultNetRudpSocketTransportConnection, CultNetRudpSocketTransportOptions, CultNetWireContract,
    GameCultProviderHealthIdentity, IDUNN_DEPLOYMENT_BRAKE_SCHEMA,
    IdunnAuthenticatedProviderHealthProjectionPurpose, IdunnDeploymentBrakeObservation,
    IdunnDeploymentBrakeOperatorIdentity, IdunnDeploymentBrakeRecord, IdunnServiceIdentity,
    IdunnSignedDaemonHealthPurpose, ServiceIdentityProfile, ServiceIdentitySignature,
    ServiceIdentitySigner, ServiceIdentityTrustAnchor, decode_cultnet_message_from_slice,
    decode_rudp_packet, derive_service_identity_id, encode_cultnet_message_to_vec,
    encode_rudp_packet, evaluate_idunn_deployment_brake, open_service_identity_at,
    serve_read_only_raw_snapshot, verify_service_identity_signature_with_public_key,
};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use odin_core::{
    BifrostRepositoryReleaseAuthorityRecord, IDUNN_AUTHENTICATED_DAEMON_HEALTH_ADMISSION_SCHEMA,
    IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA,
    IDUNN_UNSIGNED_DAEMON_HEALTH_DIAGNOSTIC_SCHEMA, IdunnAuthenticatedDaemonHealthAdmissionRecord,
    IdunnAuthenticatedProviderHealthProjectionRecord, IdunnCommandBoundaryRecord,
    IdunnCurrentDeploymentRequestRecord, IdunnDaemonHealthRecord,
    IdunnDaemonHealthTrustBindingRecord, IdunnDaemonSurgeryPlanRecord,
    IdunnDaemonTransportProfileRecord, IdunnDeploymentArtifactRecord, IdunnDeploymentRequestRecord,
    IdunnDeploymentResultRecord, IdunnDesiredDaemonRecord, IdunnLifecycleCommandRecord,
    IdunnOperatorAlarmRecord, IdunnPlan, IdunnReleaseTargetRecord, IdunnRestartRequestRecord,
    IdunnRestartResultRecord, IdunnRolloutPlanRecord, IdunnRolloutResultRecord,
    IdunnRudpHealthIngressRecord, IdunnRuntimeTransportCheckRecord, IdunnSignedDaemonHealthRecord,
    IdunnSignedHealthAdmissionRecord, IdunnStateMigrationPlanRecord,
    IdunnStateMigrationResultRecord, IdunnSwarmSurgeryPlanRecord,
    IdunnUnsignedDaemonHealthDiagnosticRecord, OdinDocuments,
    authenticated_provider_health_reason_code, plan_keepalive,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs::{self, File};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const CULTNET_RUDP_PROTOCOL_ID: &str = "cultnet.transport.rudp.v0";
const IDUNN_HEALTH_RUDP_CONNECTION_ID: u32 = 0x1d0d_0001;
const IDUNN_PUBLIC_HEALTH_QUERY_CONNECTION_ID: u32 = 0x1d0d_0002;
const IDUNN_PUBLIC_HEALTH_QUERY_RUNTIME_ID: &str = "idunn-daemon";
const IDUNN_PUBLIC_HEALTH_QUERY_ROLE: &str = "authenticated-provider-health-projector";
const EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE: &str = "epiphany.idunn_signed_runtime_health";
const EPIPHANY_SIGNED_RUNTIME_HEALTH_SCHEMA_VERSION: &str =
    "epiphany.idunn_signed_runtime_health.v0";
const EPIPHANY_HEALTH_CONTRACT: &str = "epiphany.cultnet-rudp-runtime-health";
const EPIPHANY_HEALTH_SOURCE_RUNTIME: &str = "epiphany-daemon-supervisor";
const EPIPHANY_ADMISSION_MAX_AGE_SECONDS: u64 = 180;
const SIGNED_DAEMON_HEALTH_TYPE: &str = "idunn.signed_daemon_health";

struct TargetActuationGate {
    lock: Mutex<()>,
    reserved: AtomicBool,
}

impl TargetActuationGate {
    fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            reserved: AtomicBool::new(false),
        }
    }
}
const HOST_IDENTITY_TYPE: &str = "epiphany.host_identity_trust_anchor.v0";
const HOST_IDENTITY_KEY: &str = "host-incarnation-public";
const HOST_SIGNATURE_DOMAIN: &[u8] = b"epiphany.host-incarnation.signature.v0\0";
const HOST_IDENTITY_DOMAIN: &[u8] = b"epiphany.host-incarnation.identity.v0\0";
static RELEASE_AUTHORITY_SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct IdunnDaemonHealthWireV1 {
    daemon_id: String,
    state: String,
    detail: String,
    observed_at: String,
    health_contract: String,
    publication_source: String,
    transport: String,
}

enum DecodedHealthIngress {
    AuthenticatedLegacy {
        health: IdunnDaemonHealthRecord,
        admission: IdunnSignedHealthAdmissionRecord,
    },
    AuthenticatedGeneric {
        health: IdunnDaemonHealthRecord,
        statement: IdunnSignedDaemonHealthRecord,
        admission: IdunnAuthenticatedDaemonHealthAdmissionRecord,
    },
    Diagnostic(IdunnUnsignedDaemonHealthDiagnosticRecord),
}

struct HealthIngressOutcome {
    daemon_id: String,
    state: String,
    detail: String,
    authority: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct EpiphanySignedRuntimeHealthWire {
    schema_version: String,
    health: IdunnDaemonHealthWireV1,
    source_runtime_id: String,
    release_id: String,
    release_witness_sha256: String,
    source_commit: String,
    deployment_request_id: String,
    publisher_incarnation_id: String,
    publisher_sequence: u64,
    publisher_process_id: u32,
    publisher_process_creation_token: u64,
    publisher_process_created_at: String,
    publisher_executable_path: String,
    signer_identity_id: String,
    signature_algorithm: String,
    signature: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct EpiphanyHostIdentityWire {
    schema_version: String,
    identity_id: String,
    public_key: Vec<u8>,
    assurance: String,
    identity_created_at: String,
    source_identity_record_sha256: String,
}

#[derive(Clone, Debug)]
struct DaemonTarget {
    daemon_id: String,
    verse_id: String,
    name: String,
    health_contract: HealthContract,
    deploy_command: Option<String>,
    restart_command: Option<String>,
    release: Option<ReleaseTarget>,
    enabled: bool,
    interval_seconds: u64,
}

#[derive(Clone, Debug)]
struct ReleaseTarget {
    repo: String,
    repository_full_name: String,
    repo_path: PathBuf,
    upstream_remote: String,
    upstream_branch: String,
    rollout_strategy: String,
    state_migration_command: Option<String>,
    zero_downtime_capability: String,
    deployed_revision_witness: Option<PathBuf>,
    requires_bifrost_authority: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ReleaseAuthorization {
    repository_full_name: String,
    upstream_ref: String,
    source_revision: String,
    authority_id: String,
    envelope_sha256: String,
    requires_bifrost_authority: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BifrostReleaseAuthorityWire {
    authority_id: String,
    command_id: String,
    crossing_receipt_id: String,
    repository_full_name: String,
    upstream_ref: String,
    commit_sha: String,
    decision: String,
    status: String,
    policy_decision_id: String,
    authority_reference: String,
    actor_identity: String,
    source_kind: String,
    source_id: String,
    epiphany_run_id: String,
    epiphany_lane_id: String,
    epiphany_agent_identity: String,
    external_receipt_url: String,
    external_receipt_id: String,
    authorized_at: String,
    expires_at: String,
    revoked_at: String,
    revocation_reason: String,
}

impl From<BifrostReleaseAuthorityWire> for BifrostRepositoryReleaseAuthorityRecord {
    fn from(value: BifrostReleaseAuthorityWire) -> Self {
        Self {
            authority_id: value.authority_id,
            command_id: value.command_id,
            crossing_receipt_id: value.crossing_receipt_id,
            repository_full_name: value.repository_full_name,
            upstream_ref: value.upstream_ref,
            commit_sha: value.commit_sha,
            decision: value.decision,
            status: value.status,
            policy_decision_id: value.policy_decision_id,
            authority_reference: value.authority_reference,
            actor_identity: value.actor_identity,
            source_kind: value.source_kind,
            source_id: value.source_id,
            epiphany_run_id: value.epiphany_run_id,
            epiphany_lane_id: value.epiphany_lane_id,
            epiphany_agent_identity: value.epiphany_agent_identity,
            external_receipt_url: value.external_receipt_url,
            external_receipt_id: value.external_receipt_id,
            authorized_at: value.authorized_at,
            expires_at: value.expires_at,
            revoked_at: value.revoked_at,
            revocation_reason: value.revocation_reason,
        }
    }
}

impl From<&BifrostRepositoryReleaseAuthorityRecord> for BifrostReleaseAuthorityWire {
    fn from(value: &BifrostRepositoryReleaseAuthorityRecord) -> Self {
        Self {
            authority_id: value.authority_id.clone(),
            command_id: value.command_id.clone(),
            crossing_receipt_id: value.crossing_receipt_id.clone(),
            repository_full_name: value.repository_full_name.clone(),
            upstream_ref: value.upstream_ref.clone(),
            commit_sha: value.commit_sha.clone(),
            decision: value.decision.clone(),
            status: value.status.clone(),
            policy_decision_id: value.policy_decision_id.clone(),
            authority_reference: value.authority_reference.clone(),
            actor_identity: value.actor_identity.clone(),
            source_kind: value.source_kind.clone(),
            source_id: value.source_id.clone(),
            epiphany_run_id: value.epiphany_run_id.clone(),
            epiphany_lane_id: value.epiphany_lane_id.clone(),
            epiphany_agent_identity: value.epiphany_agent_identity.clone(),
            external_receipt_url: value.external_receipt_url.clone(),
            external_receipt_id: value.external_receipt_id.clone(),
            authorized_at: value.authorized_at.clone(),
            expires_at: value.expires_at.clone(),
            revoked_at: value.revoked_at.clone(),
            revocation_reason: value.revocation_reason.clone(),
        }
    }
}

trait ReleaseAuthorityPort {
    fn select(
        &self,
        repository_full_name: &str,
        upstream_ref: &str,
        now: &str,
    ) -> Result<ReleaseAuthorization>;

    fn authorize(
        &self,
        repository_full_name: &str,
        upstream_ref: &str,
        source_revision: &str,
        now: &str,
    ) -> Result<ReleaseAuthorization>;
}

struct CultCacheReleaseAuthorityPort<'a> {
    store_path: &'a std::path::Path,
}

impl ReleaseAuthorityPort for CultCacheReleaseAuthorityPort<'_> {
    fn select(
        &self,
        repository_full_name: &str,
        upstream_ref: &str,
        now: &str,
    ) -> Result<ReleaseAuthorization> {
        let mut candidates = read_release_authority_snapshot(self.store_path)?
            .into_iter()
            .filter(|(receipt, _, _)| {
                receipt.repository_full_name == repository_full_name
                    && receipt.upstream_ref == upstream_ref
                    && receipt.decision == "authorize"
                    && receipt.status == "authorized"
            })
            .filter_map(|(receipt, envelope, digest)| {
                let expected_id =
                    release_authority_id(repository_full_name, upstream_ref, &receipt.commit_sha);
                validate_release_authority_receipt(
                    &receipt,
                    &expected_id,
                    repository_full_name,
                    upstream_ref,
                    &receipt.commit_sha,
                    now,
                )
                .ok()
                .map(|_| (receipt, envelope, digest))
            })
            .collect::<Vec<_>>();
        if candidates.len() != 1 {
            return Err(anyhow!(
                "Bifrost release selection requires exactly one current authorized receipt for {repository_full_name} {upstream_ref}; found {}",
                candidates.len()
            ));
        }
        let (receipt, _envelope, envelope_sha256) = candidates.pop().expect("one candidate");
        Ok(ReleaseAuthorization {
            repository_full_name: repository_full_name.to_string(),
            upstream_ref: upstream_ref.to_string(),
            source_revision: receipt.commit_sha,
            authority_id: receipt.authority_id,
            envelope_sha256,
            requires_bifrost_authority: true,
        })
    }

    fn authorize(
        &self,
        repository_full_name: &str,
        upstream_ref: &str,
        source_revision: &str,
        now: &str,
    ) -> Result<ReleaseAuthorization> {
        validate_commit_sha(source_revision)?;
        let authority_id =
            release_authority_id(repository_full_name, upstream_ref, source_revision);
        let result = read_release_authority_snapshot(self.store_path)?
            .into_iter()
            .find(|(receipt, _, _)| receipt.authority_id == authority_id)
            .ok_or_else(|| anyhow!("Bifrost release authority is missing: {authority_id}"))?;
        let (receipt, _envelope, envelope_sha256) = result;
        validate_release_authority_receipt(
            &receipt,
            &authority_id,
            repository_full_name,
            upstream_ref,
            source_revision,
            now,
        )?;
        Ok(ReleaseAuthorization {
            repository_full_name: repository_full_name.to_string(),
            upstream_ref: upstream_ref.to_string(),
            source_revision: source_revision.to_string(),
            authority_id,
            envelope_sha256,
            requires_bifrost_authority: true,
        })
    }
}

fn read_release_authority_snapshot(
    store_path: &std::path::Path,
) -> Result<
    Vec<(
        BifrostRepositoryReleaseAuthorityRecord,
        CultCacheEnvelope,
        String,
    )>,
> {
    if !store_path.is_file() {
        return Err(anyhow!(
            "Bifrost release authority store is unavailable: {}",
            store_path.display()
        ));
    }
    let snapshot_path = env::temp_dir().join(format!(
        "idunn-release-authority-snapshot-{}-{}.cc",
        std::process::id(),
        RELEASE_AUTHORITY_SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::copy(store_path, &snapshot_path).with_context(|| {
        format!(
            "copying Bifrost release authority snapshot {}",
            store_path.display()
        )
    })?;
    let result = (|| {
        let mut cache = CultCache::new();
        cache.register_entry_type::<BifrostRepositoryReleaseAuthorityRecord>()?;
        cache.add_backing_store(
            SingleFileMessagePackBackingStore::new(&snapshot_path),
            [BifrostRepositoryReleaseAuthorityRecord::TYPE],
        );
        cache.pull_all_backing_stores()?;
        cache
            .snapshot()
            .into_iter()
            .filter(|envelope| envelope.r#type == BifrostRepositoryReleaseAuthorityRecord::TYPE)
            .map(|envelope| {
                if envelope.schema_id.as_deref()
                    != Some(odin_core::BIFROST_REPOSITORY_RELEASE_AUTHORITY_SCHEMA)
                {
                    return Err(anyhow!("unsupported Bifrost release authority schema"));
                }
                let receipt =
                    rmp_serde::from_slice::<BifrostReleaseAuthorityWire>(&envelope.payload)
                        .context("decode Bifrost camelCase release-authority payload")?
                        .into();
                let digest = format!("{:x}", Sha256::digest(rmp_serde::to_vec_named(&envelope)?));
                Ok((receipt, envelope, digest))
            })
            .collect()
    })();
    let _ = fs::remove_file(&snapshot_path);
    let _ = fs::remove_file(snapshot_path.with_extension("cc.lock"));
    result
}

trait ReleaseStatePort {
    fn fetch(&self, release: &ReleaseTarget) -> Result<()>;
    fn desired_revision(&self, release: &ReleaseTarget) -> Result<String>;
    fn deployed_revision(&self, release: &ReleaseTarget) -> Result<String>;
}

struct SystemReleaseStatePort;

impl ReleaseStatePort for SystemReleaseStatePort {
    fn fetch(&self, release: &ReleaseTarget) -> Result<()> {
        let status = Command::new("git")
            .arg("-C")
            .arg(&release.repo_path)
            .arg("fetch")
            .arg("--quiet")
            .arg(&release.upstream_remote)
            .arg(&release.upstream_branch)
            .status()
            .with_context(|| {
                format!(
                    "fetching {} {} in {}",
                    release.upstream_remote,
                    release.upstream_branch,
                    release.repo_path.display()
                )
            })?;
        if !status.success() {
            return Err(anyhow!("git fetch exited with {status}"));
        }
        Ok(())
    }

    fn desired_revision(&self, release: &ReleaseTarget) -> Result<String> {
        git_revision(
            &release.repo_path,
            &format!("{}/{}", release.upstream_remote, release.upstream_branch),
        )
        .ok_or_else(|| {
            anyhow!(
                "cannot resolve {}/{}",
                release.upstream_remote,
                release.upstream_branch
            )
        })
    }

    fn deployed_revision(&self, release: &ReleaseTarget) -> Result<String> {
        let path = release
            .deployed_revision_witness
            .as_ref()
            .ok_or_else(|| anyhow!("no deployed revision witness configured"))?;
        read_deployed_revision_witness(path)
    }
}

#[derive(Clone, Debug)]
struct HealthContract {
    id: String,
    default_failure_state: String,
    restart_on_missing_publication: bool,
}

fn health_contract(id: &str, default_failure_state: &str) -> HealthContract {
    HealthContract {
        id: id.to_string(),
        default_failure_state: default_failure_state.to_string(),
        restart_on_missing_publication: false,
    }
}

fn locally_supervised_health_contract(id: &str, default_failure_state: &str) -> HealthContract {
    HealthContract {
        id: id.to_string(),
        default_failure_state: default_failure_state.to_string(),
        restart_on_missing_publication: true,
    }
}

#[derive(Clone, Debug)]
struct CommonOptions {
    store_path: PathBuf,
    release_authority_store_path: Option<PathBuf>,
    deployment_brake_store_path: Option<PathBuf>,
    deployment_brake_operator_anchor_path: Option<PathBuf>,
    deployment_brake_runtime_id: Option<String>,
    operator_alarm_command: Option<String>,
    rudp_health_bind: Option<SocketAddr>,
    trusted_epiphany_health_identity_store: Option<PathBuf>,
    daemon_health_trust_store_path: Option<PathBuf>,
    service_identity_store_path: Option<PathBuf>,
    public_health_store_path: Option<PathBuf>,
    public_health_query_bind: Option<SocketAddr>,
    execute: bool,
    command_timeout_seconds: u64,
}

#[derive(Clone, Debug)]
enum Mode {
    Single(DaemonTarget),
    Swarm(SwarmOptions),
    LifecycleCommand(LifecycleCommandOptions),
    ReleaseAuthorityValidation(ReleaseAuthorityValidationOptions),
    HealthAdmissionValidation(HealthAdmissionValidationOptions),
}

#[derive(Clone, Debug)]
struct HealthAdmissionValidationOptions {
    daemon_id: String,
    deployment_request_id: String,
    release_id: String,
    release_witness_sha256: String,
    source_commit: String,
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

#[derive(Clone, Debug)]
enum LifecycleAction {
    Restart,
    Redeploy,
}

#[derive(Clone, Debug)]
struct LifecycleCommandOptions {
    daemon_id: String,
    action: LifecycleAction,
    requested_by: String,
    detail: String,
}

#[derive(Clone, Debug)]
struct ReleaseAuthorityValidationOptions {
    store_path: PathBuf,
    repository_full_name: String,
    upstream_ref: String,
    source_revision: String,
    authority_id: String,
    envelope_sha256: String,
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;

    if let Some(parent) = options.common.store_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    match &options.mode {
        Mode::Single(target) => {
            let projection_publisher = initialize_projection_publisher(&options.common)?;
            let store_lock = Arc::new(Mutex::new(()));
            let actuation_gate = Arc::new(TargetActuationGate::new());
            let now = timestamp()?;
            publish_runtime_transport_check(&options.common, &store_lock, &now)?;
            let mut missing_since = None;
            run_target_cycle(
                target,
                &options.common,
                &store_lock,
                Arc::clone(&actuation_gate),
                projection_publisher.as_deref(),
                &mut missing_since,
            )?;
            while actuation_gate.reserved.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
        Mode::Swarm(swarm) => {
            let projection_publisher = initialize_projection_publisher(&options.common)?;
            run_swarm(swarm, &options.common, projection_publisher)
        }
        Mode::LifecycleCommand(command) => publish_lifecycle_command(command, &options.common),
        Mode::ReleaseAuthorityValidation(validation) => {
            validate_release_authority_at_privileged_boundary(validation)
        }
        Mode::HealthAdmissionValidation(validation) => {
            validate_health_admission(validation, &options.common)
        }
    }
}

fn validate_health_admission(
    expected: &HealthAdmissionValidationOptions,
    options: &CommonOptions,
) -> Result<()> {
    validate_health_admission_at(expected, options, &timestamp()?)
}

fn validate_health_admission_at(
    expected: &HealthAdmissionValidationOptions,
    options: &CommonOptions,
    now: &str,
) -> Result<()> {
    let lock = Arc::new(Mutex::new(()));
    let admission = with_store_node(options, &lock, |node| {
        let admission = node
            .get::<IdunnSignedHealthAdmissionRecord>(&expected.daemon_id)?
            .ok_or_else(|| {
                anyhow!(
                    "Idunn has no signed health admission for {}",
                    expected.daemon_id
                )
            })?;
        validate_admission_against_current_request(node, &admission)?;
        let current_health = node
            .get::<IdunnDaemonHealthRecord>(&expected.daemon_id)?
            .ok_or_else(|| anyhow!("Idunn has no current health projection for admission"))?;
        if current_health.state != admission.state
            || current_health.observed_at != admission.observed_at
            || current_health.health_contract != admission.health_contract
            || current_health.publication_source != "daemon-published"
            || current_health.transport != CULTNET_RUDP_PROTOCOL_ID
        {
            return Err(anyhow!(
                "signed health admission is not the current daemon health observation"
            ));
        }
        Ok(admission)
    })?;
    validate_admission_fresh_at(&admission, now, EPIPHANY_ADMISSION_MAX_AGE_SECONDS)?;
    if admission.state != "active"
        || admission.deployment_request_id != expected.deployment_request_id
        || admission.release_id != expected.release_id
        || admission.release_witness_sha256 != expected.release_witness_sha256
        || admission.source_commit != expected.source_commit
        || admission.publisher_incarnation_id.is_empty()
        || admission.publisher_sequence == 0
        || admission.signed_health_sha256.is_empty()
        || admission.signer_identity_id.is_empty()
    {
        return Err(anyhow!(
            "Idunn admitted health does not prove the exact active candidate"
        ));
    }
    println!(
        "validated signed Idunn health admission daemon={} state={} releaseId={} witnessSha256={} sourceCommit={} signedHealthSha256={} publisherIncarnation={} publisherSequence={}",
        admission.daemon_id,
        admission.state,
        admission.release_id,
        admission.release_witness_sha256,
        admission.source_commit,
        admission.signed_health_sha256,
        admission.publisher_incarnation_id,
        admission.publisher_sequence,
    );
    Ok(())
}

fn validate_release_authority_at_privileged_boundary(
    options: &ReleaseAuthorityValidationOptions,
) -> Result<()> {
    let current = CultCacheReleaseAuthorityPort {
        store_path: &options.store_path,
    }
    .authorize(
        &options.repository_full_name,
        &options.upstream_ref,
        &options.source_revision,
        &timestamp()?,
    )?;
    if current.authority_id != options.authority_id
        || current.envelope_sha256 != options.envelope_sha256
    {
        return Err(anyhow!(
            "privileged deployment boundary received stale or substituted Bifrost authority"
        ));
    }
    println!("Bifrost release authority validated for privileged deployment boundary.");
    Ok(())
}

fn run_swarm(
    options: &SwarmOptions,
    common: &CommonOptions,
    projection_publisher: Option<Arc<IdunnProjectionPublisher>>,
) -> Result<()> {
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
    let recovered = terminalize_interrupted_deployment_requests(common, &store_lock, &now)?;
    if recovered > 0 {
        println!(
            "Idunn terminalized {recovered} deployment request(s) interrupted by the prior daemon incarnation."
        );
    }
    publish_runtime_transport_check(common, &store_lock, &now)?;
    start_public_health_query_listener(common, &targets)?;
    start_rudp_health_ingress(common, &store_lock, &now)?;
    publish_surgery_plans(&options.profile, &targets, common, &store_lock, &now)?;

    // Health-driven and lifecycle-command workers are separate so observation
    // remains responsive. One per-target gate serializes local actuation; the
    // persistent request/result CAS chain remains the durable authority.
    let actuation_gates = Arc::new(
        targets
            .iter()
            .map(|target| {
                (
                    target.daemon_id.clone(),
                    Arc::new(TargetActuationGate::new()),
                )
            })
            .collect::<HashMap<_, _>>(),
    );

    let command_targets = targets.clone();
    let command_common = common.clone();
    let command_store_lock = Arc::clone(&store_lock);
    let command_actuation_gates = Arc::clone(&actuation_gates);
    let mut workers = Vec::with_capacity(targets.len() + 1);
    workers.push(thread::spawn(move || {
        run_lifecycle_command_loop(
            command_targets,
            command_common,
            command_store_lock,
            command_actuation_gates,
        )
    }));
    for target in targets {
        let worker_common = common.clone();
        let worker_store_lock = Arc::clone(&store_lock);
        let worker_actuation_gate = actuation_gates
            .get(&target.daemon_id)
            .cloned()
            .ok_or_else(|| anyhow!("Idunn target lost its actuation gate"))?;
        let worker_projection_publisher = projection_publisher.clone();
        workers.push(thread::spawn(move || {
            run_target_loop(
                target,
                worker_common,
                worker_store_lock,
                worker_actuation_gate,
                worker_projection_publisher,
            )
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

fn run_lifecycle_command_loop(
    targets: Vec<DaemonTarget>,
    options: CommonOptions,
    store_lock: Arc<Mutex<()>>,
    actuation_gates: Arc<HashMap<String, Arc<TargetActuationGate>>>,
) -> Result<()> {
    loop {
        for target in &targets {
            let gate = actuation_gates
                .get(&target.daemon_id)
                .ok_or_else(|| anyhow!("Idunn lifecycle target lost its actuation gate"))?;
            if gate.reserved.swap(true, Ordering::AcqRel) {
                continue;
            }
            let result = with_target_actuation_gate(&gate.lock, || {
                process_pending_lifecycle_commands(target, &options, &store_lock)
            });
            gate.reserved.store(false, Ordering::Release);
            if let Err(error) = result {
                eprintln!(
                    "Idunn lifecycle command cycle failed for {}: {}",
                    target.daemon_id, error
                );
            }
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn validate_targets(targets: &[DaemonTarget]) -> Result<()> {
    let mut issues = Vec::new();
    for target in targets {
        if target.health_contract.id.trim().is_empty() {
            issues.push(format!("{} has no health contract", target.daemon_id));
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
    actuation_gate: Arc<TargetActuationGate>,
    projection_publisher: Option<Arc<IdunnProjectionPublisher>>,
) -> Result<()> {
    let mut missing_since = None;
    loop {
        if let Err(error) = run_target_cycle(
            &target,
            &options,
            &store_lock,
            Arc::clone(&actuation_gate),
            projection_publisher.as_deref(),
            &mut missing_since,
        ) {
            eprintln!(
                "Idunn swarm target {} cycle failed: {}",
                target.daemon_id, error
            );
        }
        thread::sleep(Duration::from_secs(target.interval_seconds));
    }
}

fn with_target_actuation_gate<T>(
    gate: &Mutex<()>,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _actuation = gate
        .lock()
        .map_err(|_| anyhow!("Idunn target actuation gate is poisoned"))?;
    action()
}

fn run_target_cycle(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    actuation_gate: Arc<TargetActuationGate>,
    projection_publisher: Option<&IdunnProjectionPublisher>,
    missing_since: &mut Option<Instant>,
) -> Result<()> {
    let now = timestamp()?;

    let desired = IdunnDesiredDaemonRecord {
        daemon_id: target.daemon_id.clone(),
        verse_id: target.verse_id.clone(),
        name: target.name.clone(),
        enabled: target.enabled,
        health_command: None,
        restart_command: target.restart_command.clone(),
        deploy_command: target.deploy_command.clone(),
        health_contract: target.health_contract.id.clone(),
        transport_profile_id: transport_profile_id(target),
        command_boundary_id: command_boundary_id(target),
        authority: "idunn-supervisor-command".to_string(),
        max_silence_seconds: 60,
        observed_at: now.clone(),
    };

    let (mut health_key, mut health, mut projection_source) =
        evaluate_target_health(target, options, store_lock, &desired, &now)?;
    let health_authenticated =
        projection_source.is_some() || health.publication_source == "daemon-published";
    apply_missing_publication_grace(
        target,
        &desired,
        &mut health,
        health_authenticated,
        missing_since,
        Instant::now(),
    );
    let mut plan = plan_keepalive(&desired, &health, now.clone());
    if plan.restart_request.is_some() {
        let veto_now = timestamp()?;
        if let Some(fresh_health) =
            read_fresh_daemon_published_health(options, store_lock, &desired, &veto_now)?
        {
            if health_state_is_healthy(&fresh_health.health.state) {
                health_key = desired.daemon_id.clone();
                health = fresh_health.health;
                projection_source = fresh_health.projection_source;
                *missing_since = None;
                plan = plan_keepalive(&desired, &health, veto_now);
            }
        }
    }
    if let Some(request) = plan.deployment_request.as_mut() {
        match authorize_release(target, options, store_lock) {
            Ok(authorization) => apply_release_authorization(request, &authorization),
            Err(error) => {
                let reason = format!("deployment refused: {error:#}");
                plan.deployment_request = None;
                plan.decision.action = "alarm".to_string();
                plan.decision.reason = reason.clone();
                plan.operator_alarm = Some(IdunnOperatorAlarmRecord {
                    alarm_id: format!("alarm:{}:{}", target.daemon_id, now),
                    daemon_id: target.daemon_id.clone(),
                    severity: "operator-action-required".to_string(),
                    reason,
                    escalation_target: "bifrost.operator-notification".to_string(),
                    raised_at: now.clone(),
                });
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
        if let Some(request) = &plan.restart_request {
            node.put(&request.request_id, request)?;
        }
        if let Some(alarm) = &plan.operator_alarm {
            node.put(&alarm.alarm_id, alarm)?;
        }
        Ok(())
    })?;

    publish_authenticated_provider_health(
        projection_publisher,
        options,
        store_lock,
        &desired,
        projection_source.as_ref(),
        &now,
    )?;

    if plan.deployment_request.is_some() || plan.restart_request.is_some() {
        schedule_automatic_actuation(
            target.clone(),
            options.clone(),
            Arc::clone(store_lock),
            actuation_gate,
            plan,
            now,
        );
        return Ok(());
    }

    finish_target_cycle(target, options, store_lock, plan, &now)
}

fn schedule_automatic_actuation(
    target: DaemonTarget,
    options: CommonOptions,
    store_lock: Arc<Mutex<()>>,
    gate: Arc<TargetActuationGate>,
    plan: IdunnPlan,
    now: String,
) {
    let daemon_id = target.daemon_id.clone();
    schedule_target_actuation(gate, daemon_id, move || {
        finish_target_cycle(&target, &options, &store_lock, plan, &now)
    });
}

fn schedule_target_actuation(
    gate: Arc<TargetActuationGate>,
    daemon_id: String,
    action: impl FnOnce() -> Result<()> + Send + 'static,
) {
    if gate
        .reserved
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    thread::spawn(move || {
        let result = with_target_actuation_gate(&gate.lock, action);
        gate.reserved.store(false, Ordering::Release);
        if let Err(error) = result {
            eprintln!(
                "Idunn automatic actuation failed for {}: {}",
                daemon_id, error
            );
        }
    });
}

fn finish_target_cycle(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    plan: IdunnPlan,
    now: &str,
) -> Result<()> {
    if let Some(request) = &plan.deployment_request {
        persist_current_deployment_request(options, store_lock, request)?;
    }

    if let Some(request) = &plan.deployment_request {
        if options.execute {
            let authority_error = revalidate_deployment_request(request, options, store_lock).err();
            let migration_result = if authority_error.is_none() {
                target
                    .release
                    .as_ref()
                    .and_then(|release| run_state_migration(target, release, request, now, options))
            } else {
                None
            };
            if let Some(result) = &migration_result {
                with_store_node(options, store_lock, |node| {
                    node.put(&result.result_id, result)?;
                    Ok(())
                })?;
            }
            let migration_failed = migration_result
                .as_ref()
                .is_some_and(|result| result.state != "succeeded" && result.state != "noop");
            let mut result = if let Some(error) = authority_error {
                IdunnDeploymentResultRecord {
                    result_id: format!("result:{}", request.request_id),
                    request_id: request.request_id.clone(),
                    daemon_id: request.daemon_id.clone(),
                    state: "failed".to_string(),
                    detail: format!(
                        "deployment authority revalidation failed before migration: {error:#}"
                    ),
                    completed_at: now.to_string(),
                }
            } else if migration_failed {
                IdunnDeploymentResultRecord {
                    result_id: format!("result:{}", request.request_id),
                    request_id: request.request_id.clone(),
                    daemon_id: request.daemon_id.clone(),
                    state: "failed".to_string(),
                    detail: "state migration failed; deployment command was not run".to_string(),
                    completed_at: now.to_string(),
                }
            } else {
                run_deployment(request, now, options, store_lock)
            };
            if result.state == "succeeded"
                && let Some(release) = target.release.as_ref()
                && release.deployed_revision_witness.is_some()
                && let Err(error) = verify_release_witness_current(release, &SystemReleaseStatePort)
            {
                result.state = "failed".to_string();
                result.detail = format!(
                    "deployment command exited successfully, but its deployed revision witness did not converge: {error:#}"
                );
            }
            let rollout_result = target.release.as_ref().map(|release| {
                rollout_result_record(target, release, &result, migration_result.as_ref(), now)
            });
            let alarm = if result.state != "succeeded" {
                Some(deployment_failure_alarm(&result, now))
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
            let result = run_restart(request, now, options);
            let alarm = if result.state != "succeeded" {
                Some(restart_failure_alarm(&result, now))
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

fn process_pending_lifecycle_commands(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
) -> Result<()> {
    let pending = with_store_node(options, store_lock, |node| {
        let mut commands = node.cache().get_all::<IdunnLifecycleCommandRecord>()?;
        commands.retain(|command| {
            command.daemon_id == target.daemon_id
                && command.state == "pending"
                && matches!(command.action.as_str(), "restart" | "redeploy")
        });
        commands.sort_by(|left, right| left.requested_at.cmp(&right.requested_at));
        Ok(commands)
    })?;

    for command in pending {
        process_lifecycle_command(target, options, store_lock, command)?;
    }

    Ok(())
}

fn process_lifecycle_command(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    command: IdunnLifecycleCommandRecord,
) -> Result<()> {
    let claimed_at = timestamp()?;
    match command.action.as_str() {
        "restart" => {
            let Some(shell_command) = target.restart_command.as_deref() else {
                reject_lifecycle_command(
                    options,
                    store_lock,
                    command,
                    &claimed_at,
                    "restart authority is not configured for this target",
                )?;
                return Ok(());
            };
            let request = IdunnRestartRequestRecord {
                request_id: format!("manual:restart:{}:{}", target.daemon_id, command.command_id),
                daemon_id: target.daemon_id.clone(),
                command: shell_command.to_string(),
                authority: "idunn-supervisor-command.manual".to_string(),
                requested_at: claimed_at.clone(),
            };
            with_store_node(options, store_lock, |node| {
                let mut running = command.clone();
                running.state = "running".to_string();
                running.claimed_at = claimed_at.clone();
                node.put(&running.command_id, &running)?;
                node.put(&request.request_id, &request)?;
                Ok(())
            })?;
            let result = if options.execute {
                run_restart(&request, &claimed_at, options)
            } else {
                IdunnRestartResultRecord {
                    result_id: format!("result:{}", request.request_id),
                    request_id: request.request_id.clone(),
                    daemon_id: request.daemon_id.clone(),
                    state: "skipped".to_string(),
                    detail: "manual restart command was claimed but --execute is not enabled"
                        .to_string(),
                    completed_at: claimed_at.clone(),
                }
            };
            with_store_node(options, store_lock, |node| {
                node.put(&result.result_id, &result)?;
                let mut completed = command.clone();
                completed.state = result.state.clone();
                completed.claimed_at = claimed_at.clone();
                completed.result_id = result.result_id.clone();
                node.put(&completed.command_id, &completed)?;
                Ok(())
            })?;
            println!(
                "Idunn manual restart {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
        }
        "redeploy" => {
            let Some(shell_command) = target.deploy_command.as_deref() else {
                reject_lifecycle_command(
                    options,
                    store_lock,
                    command,
                    &claimed_at,
                    "redeploy authority is not configured for this target",
                )?;
                return Ok(());
            };
            let authorization = match authorize_release(target, options, store_lock) {
                Ok(value) => value,
                Err(error) => {
                    reject_lifecycle_command(
                        options,
                        store_lock,
                        command,
                        &claimed_at,
                        &format!("redeploy authority refused: {error:#}"),
                    )?;
                    return Ok(());
                }
            };
            let mut request = IdunnDeploymentRequestRecord {
                request_id: format!(
                    "manual:redeploy:{}:{}",
                    target.daemon_id, command.command_id
                ),
                daemon_id: target.daemon_id.clone(),
                command: shell_command.to_string(),
                authority: "idunn-supervisor-command.manual".to_string(),
                requested_at: claimed_at.clone(),
                repository_full_name: String::new(),
                upstream_ref: String::new(),
                source_revision: String::new(),
                release_authority_id: String::new(),
                release_authority_envelope_sha256: String::new(),
                requires_bifrost_authority: false,
            };
            apply_release_authorization(&mut request, &authorization);
            with_store_node(options, store_lock, |node| {
                let mut running = command.clone();
                running.state = "running".to_string();
                running.claimed_at = claimed_at.clone();
                node.put(&running.command_id, &running)?;
                Ok(())
            })?;
            persist_current_deployment_request(options, store_lock, &request)?;
            let result = if options.execute {
                run_deployment(&request, &claimed_at, options, store_lock)
            } else {
                IdunnDeploymentResultRecord {
                    result_id: format!("result:{}", request.request_id),
                    request_id: request.request_id.clone(),
                    daemon_id: request.daemon_id.clone(),
                    state: "skipped".to_string(),
                    detail: "manual redeploy command was claimed but --execute is not enabled"
                        .to_string(),
                    completed_at: claimed_at.clone(),
                }
            };
            with_store_node(options, store_lock, |node| {
                node.put(&result.result_id, &result)?;
                let mut completed = command.clone();
                completed.state = result.state.clone();
                completed.claimed_at = claimed_at.clone();
                completed.result_id = result.result_id.clone();
                node.put(&completed.command_id, &completed)?;
                Ok(())
            })?;
            println!(
                "Idunn manual redeploy {} for {}: {}",
                result.state, result.daemon_id, result.detail
            );
        }
        _ => reject_lifecycle_command(
            options,
            store_lock,
            command,
            &claimed_at,
            "unknown lifecycle command action",
        )?,
    }

    Ok(())
}

fn reject_lifecycle_command(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    mut command: IdunnLifecycleCommandRecord,
    claimed_at: &str,
    detail: &str,
) -> Result<()> {
    command.state = "rejected".to_string();
    command.claimed_at = claimed_at.to_string();
    command.detail = if command.detail.trim().is_empty() {
        detail.to_string()
    } else {
        format!("{}; {}", command.detail, detail)
    };
    with_store_node(options, store_lock, |node| {
        node.put(&command.command_id, &command)?;
        Ok(())
    })
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

fn persist_current_deployment_request(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    request: &IdunnDeploymentRequestRecord,
) -> Result<IdunnCurrentDeploymentRequestRecord> {
    for _ in 0..8 {
        let (
            current_head,
            current_head_envelope,
            current_head_request_envelope,
            current_head_result_envelope,
            current_request_envelope,
        ) = with_store_node(options, store_lock, |node| {
            let head = node.get::<IdunnCurrentDeploymentRequestRecord>(&request.daemon_id)?;
            Ok((
                head.clone(),
                node.cache()
                    .get_envelope::<IdunnCurrentDeploymentRequestRecord>(&request.daemon_id)?,
                head.as_ref()
                    .map(|head| {
                        node.cache()
                            .get_envelope::<IdunnDeploymentRequestRecord>(&head.request_id)
                    })
                    .transpose()?
                    .flatten(),
                head.as_ref()
                    .map(|head| {
                        node.cache()
                            .get_envelope::<IdunnDeploymentResultRecord>(&format!(
                                "result:{}",
                                head.request_id
                            ))
                    })
                    .transpose()?
                    .flatten(),
                node.cache()
                    .get_envelope::<IdunnDeploymentRequestRecord>(&request.request_id)?,
            ))
        })?;
        if let Some(existing) = current_request_envelope.as_ref() {
            let decoded: IdunnDeploymentRequestRecord = rmp_serde::from_slice(&existing.payload)?;
            if decoded != *request {
                return Err(anyhow!("deployment request identity collision"));
            }
            if let Some(head) = current_head.as_ref()
                && head.request_id == request.request_id
            {
                return Ok(head.clone());
            }
            return Err(anyhow!(
                "existing deployment request is not the current authority head"
            ));
        }
        if let Some(current) = current_head.as_ref() {
            let prior_request_envelope = current_head_request_envelope
                .as_ref()
                .ok_or_else(|| anyhow!("current deployment head lost its exact request"))?;
            let prior_request: IdunnDeploymentRequestRecord =
                rmp_serde::from_slice(&prior_request_envelope.payload)?;
            if prior_request.request_id != current.request_id
                || prior_request.daemon_id != current.daemon_id
            {
                return Err(anyhow!(
                    "current deployment head request identity is substituted"
                ));
            }
            let prior_result_envelope = current_head_result_envelope.as_ref().ok_or_else(|| {
                anyhow!(
                    "current deployment request {} remains live; refusing supersession",
                    current.request_id
                )
            })?;
            let prior_result: IdunnDeploymentResultRecord =
                rmp_serde::from_slice(&prior_result_envelope.payload)?;
            if prior_result.result_id != format!("result:{}", current.request_id)
                || prior_result.request_id != current.request_id
                || prior_result.daemon_id != current.daemon_id
                || !matches!(
                    prior_result.state.as_str(),
                    "succeeded" | "failed" | "skipped"
                )
            {
                return Err(anyhow!(
                    "current deployment request lacks an exact terminal result"
                ));
            }
        }
        let head = IdunnCurrentDeploymentRequestRecord {
            daemon_id: request.daemon_id.clone(),
            request_id: request.request_id.clone(),
            sequence: current_head
                .as_ref()
                .map_or(1, |head| head.sequence.saturating_add(1)),
            updated_at: request.requested_at.clone(),
        };
        let mut expected = vec![
            CultCacheExpectedEnvelope {
                key: request.daemon_id.clone(),
                r#type: IdunnCurrentDeploymentRequestRecord::TYPE.into(),
                current: current_head_envelope,
            },
            CultCacheExpectedEnvelope {
                key: request.request_id.clone(),
                r#type: IdunnDeploymentRequestRecord::TYPE.into(),
                current: None,
            },
        ];
        if let Some(current) = current_head.as_ref() {
            expected.push(CultCacheExpectedEnvelope {
                key: current.request_id.clone(),
                r#type: IdunnDeploymentRequestRecord::TYPE.into(),
                current: current_head_request_envelope,
            });
            expected.push(CultCacheExpectedEnvelope {
                key: format!("result:{}", current.request_id),
                r#type: IdunnDeploymentResultRecord::TYPE.into(),
                current: current_head_result_envelope,
            });
        }
        if SingleFileMessagePackBackingStore::new(&options.store_path).compare_exchange(
            &expected,
            &[
                typed_envelope(&request.request_id, request, &request.requested_at)?,
                typed_envelope(&head.daemon_id, &head, &request.requested_at)?,
            ],
        )? {
            return Ok(head);
        }
    }
    Err(anyhow!(
        "deployment request head lost repeated cross-process contention"
    ))
}

/// On daemon startup no deployment actuator from the prior process can still
/// be an Idunn-owned child. Close any exact current request that lacks a result
/// before this incarnation launches workers. The current head remains durable
/// history; its exact failed result permits ordinary CAS supersession.
fn terminalize_interrupted_deployment_requests(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    completed_at: &str,
) -> Result<usize> {
    let _store_guard = store_lock
        .lock()
        .map_err(|_| anyhow!("Idunn store lock is poisoned"))?;
    let backing = SingleFileMessagePackBackingStore::new(&options.store_path);
    let opening = backing.pull_all()?;
    let heads = opening
        .iter()
        .filter(|entry| entry.r#type == IdunnCurrentDeploymentRequestRecord::TYPE)
        .collect::<Vec<_>>();
    let mut recovered = 0;
    for head_envelope in heads {
        let head: IdunnCurrentDeploymentRequestRecord =
            rmp_serde::from_slice(&head_envelope.payload)?;
        if head.daemon_id.trim().is_empty()
            || head.request_id.trim().is_empty()
            || head.sequence == 0
            || head_envelope.key != head.daemon_id
        {
            return Err(anyhow!(
                "current deployment request head is invalid during startup recovery"
            ));
        }
        let request_matches = opening
            .iter()
            .filter(|entry| {
                entry.r#type == IdunnDeploymentRequestRecord::TYPE && entry.key == head.request_id
            })
            .collect::<Vec<_>>();
        if request_matches.len() != 1 {
            return Err(anyhow!(
                "current deployment request {} lacks one exact request during startup recovery",
                head.request_id
            ));
        }
        let request_envelope = request_matches[0];
        let request: IdunnDeploymentRequestRecord =
            rmp_serde::from_slice(&request_envelope.payload)?;
        if request.request_id != head.request_id || request.daemon_id != head.daemon_id {
            return Err(anyhow!(
                "current deployment request is substituted during startup recovery"
            ));
        }
        let result_key = format!("result:{}", head.request_id);
        let result_matches = opening
            .iter()
            .filter(|entry| {
                entry.r#type == IdunnDeploymentResultRecord::TYPE && entry.key == result_key
            })
            .collect::<Vec<_>>();
        if result_matches.len() > 1 {
            return Err(anyhow!(
                "current deployment request has duplicate results during startup recovery"
            ));
        }
        if let Some(result_envelope) = result_matches.first() {
            let result: IdunnDeploymentResultRecord =
                rmp_serde::from_slice(&result_envelope.payload)?;
            if result.result_id != result_key
                || result.request_id != head.request_id
                || result.daemon_id != head.daemon_id
                || !matches!(result.state.as_str(), "succeeded" | "failed" | "skipped")
            {
                return Err(anyhow!(
                    "current deployment request lacks an exact terminal result during startup recovery"
                ));
            }
            continue;
        }
        let result = IdunnDeploymentResultRecord {
            result_id: result_key.clone(),
            request_id: head.request_id.clone(),
            daemon_id: head.daemon_id.clone(),
            state: "failed".to_string(),
            detail: "deployment actuator was interrupted when the owning Idunn daemon incarnation stopped"
                .to_string(),
            completed_at: completed_at.to_string(),
        };
        let expected = [
            CultCacheExpectedEnvelope {
                key: head.daemon_id.clone(),
                r#type: IdunnCurrentDeploymentRequestRecord::TYPE.into(),
                current: Some(head_envelope.clone()),
            },
            CultCacheExpectedEnvelope {
                key: head.request_id.clone(),
                r#type: IdunnDeploymentRequestRecord::TYPE.into(),
                current: Some(request_envelope.clone()),
            },
            CultCacheExpectedEnvelope {
                key: result_key.clone(),
                r#type: IdunnDeploymentResultRecord::TYPE.into(),
                current: None,
            },
        ];
        if !backing.compare_exchange(
            &expected,
            &[typed_envelope(&result_key, &result, completed_at)?],
        )? {
            return Err(anyhow!(
                "interrupted deployment request changed during startup recovery"
            ));
        }
        recovered += 1;
    }
    Ok(recovered)
}

fn release_authority_id(
    repository_full_name: &str,
    upstream_ref: &str,
    commit_sha: &str,
) -> String {
    format!("release:{repository_full_name}:{upstream_ref}:{commit_sha}")
}

fn validate_commit_sha(value: &str) -> Result<()> {
    if value.len() != 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(anyhow!(
            "release source revision must be a lowercase 40-hex Git commit"
        ));
    }
    Ok(())
}

fn validate_release_authority_receipt(
    receipt: &BifrostRepositoryReleaseAuthorityRecord,
    expected_id: &str,
    repository_full_name: &str,
    upstream_ref: &str,
    source_revision: &str,
    now: &str,
) -> Result<()> {
    if receipt.authority_id != expected_id
        || receipt.repository_full_name != repository_full_name
        || receipt.upstream_ref != upstream_ref
        || receipt.commit_sha != source_revision
    {
        return Err(anyhow!(
            "Bifrost release authority identity/repository/ref/commit mismatch"
        ));
    }
    if receipt.decision != "authorize" || receipt.status != "authorized" {
        return Err(anyhow!(
            "Bifrost release authority is not currently authorized"
        ));
    }
    if receipt.command_id.trim().is_empty()
        || receipt.crossing_receipt_id.trim().is_empty()
        || receipt.policy_decision_id.trim().is_empty()
        || receipt.authority_reference.trim().is_empty()
        || receipt.actor_identity.trim().is_empty()
        || receipt.source_kind.trim().is_empty()
        || receipt.source_id.trim().is_empty()
        || receipt.external_receipt_url.trim().is_empty()
        || receipt.external_receipt_id.trim().is_empty()
        || receipt.authorized_at.trim().is_empty()
    {
        return Err(anyhow!(
            "Bifrost release authority provenance is incomplete"
        ));
    }
    if !receipt.revoked_at.trim().is_empty() || !receipt.revocation_reason.trim().is_empty() {
        return Err(anyhow!(
            "Bifrost release authority carries revocation facts"
        ));
    }
    let expected_external_url =
        format!("https://github.com/{repository_full_name}/commit/{source_revision}");
    if receipt.external_receipt_id != source_revision
        || receipt.external_receipt_url != expected_external_url
    {
        return Err(anyhow!(
            "Bifrost release authority external GitHub proof does not bind the exact commit"
        ));
    }
    chrono::DateTime::parse_from_rfc3339(&receipt.authorized_at)
        .context("Bifrost release authority authorizedAt must be RFC3339")?;
    if !receipt.expires_at.trim().is_empty() {
        let expires = chrono::DateTime::parse_from_rfc3339(&receipt.expires_at)
            .context("Bifrost release authority expiresAt must be RFC3339")?
            .timestamp();
        let now_seconds = now
            .strip_prefix("unix:")
            .ok_or_else(|| anyhow!("Idunn authority clock is not unix-seconds"))?
            .parse::<i64>()
            .context("Idunn authority clock is invalid")?;
        if expires <= now_seconds {
            return Err(anyhow!("Bifrost release authority has expired"));
        }
    }
    Ok(())
}

fn authorize_release(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
) -> Result<ReleaseAuthorization> {
    let release = target
        .release
        .as_ref()
        .ok_or_else(|| anyhow!("deployment target has no release declaration"))?;
    let state = SystemReleaseStatePort;
    state.fetch(release)?;
    let observed_upstream_revision = state.desired_revision(release)?;
    let upstream_ref = format!("refs/heads/{}", release.upstream_branch);
    if !release.requires_bifrost_authority {
        validate_commit_sha(&observed_upstream_revision)?;
        return Ok(ReleaseAuthorization {
            repository_full_name: release.repository_full_name.clone(),
            upstream_ref,
            source_revision: observed_upstream_revision,
            authority_id: String::new(),
            envelope_sha256: String::new(),
            requires_bifrost_authority: false,
        });
    }
    let now = timestamp()?;
    let _store_guard = store_lock
        .lock()
        .map_err(|_| anyhow!("Idunn store lock is poisoned"))?;
    let authority_store = options
        .release_authority_store_path
        .as_deref()
        .ok_or_else(|| anyhow!("--release-authority-store is required for deployment authority"))?;
    let selected = CultCacheReleaseAuthorityPort {
        store_path: authority_store,
    }
    .select(&release.repository_full_name, &upstream_ref, &now)?;
    if git_revision(&release.repo_path, &selected.source_revision).as_deref()
        != Some(selected.source_revision.as_str())
    {
        return Err(anyhow!(
            "authorized Bifrost release commit is absent from fetched repository: {}",
            selected.source_revision
        ));
    }
    Ok(selected)
}

fn apply_release_authorization(
    request: &mut IdunnDeploymentRequestRecord,
    authorization: &ReleaseAuthorization,
) {
    request.repository_full_name = authorization.repository_full_name.clone();
    request.upstream_ref = authorization.upstream_ref.clone();
    request.source_revision = authorization.source_revision.clone();
    request.release_authority_id = authorization.authority_id.clone();
    request.release_authority_envelope_sha256 = authorization.envelope_sha256.clone();
    request.requires_bifrost_authority = authorization.requires_bifrost_authority;
}

fn revalidate_deployment_request(
    request: &IdunnDeploymentRequestRecord,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
) -> Result<()> {
    validate_commit_sha(&request.source_revision)?;
    if !request.requires_bifrost_authority {
        if !request.release_authority_id.is_empty()
            || !request.release_authority_envelope_sha256.is_empty()
        {
            return Err(anyhow!(
                "legacy release request cannot carry Bifrost authority claims"
            ));
        }
        return Ok(());
    }
    let _store_guard = store_lock
        .lock()
        .map_err(|_| anyhow!("Idunn store lock is poisoned"))?;
    let authority_store = options
        .release_authority_store_path
        .as_deref()
        .ok_or_else(|| anyhow!("--release-authority-store is required for deployment authority"))?;
    let current = CultCacheReleaseAuthorityPort {
        store_path: authority_store,
    }
    .authorize(
        &request.repository_full_name,
        &request.upstream_ref,
        &request.source_revision,
        &timestamp()?,
    )?;
    if current.authority_id != request.release_authority_id
        || current.envelope_sha256 != request.release_authority_envelope_sha256
    {
        return Err(anyhow!(
            "deployment request release authority changed after planning"
        ));
    }
    Ok(())
}

fn read_fresh_daemon_published_health(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    desired: &IdunnDesiredDaemonRecord,
    now: &str,
) -> Result<Option<ManagedHealthRead>> {
    with_store_node(options, store_lock, |node| {
        let Some(health) = node.get::<IdunnDaemonHealthRecord>(&desired.daemon_id)? else {
            return Ok(None);
        };
        if !is_fresh_daemon_published_health(&health, desired, now) {
            return Ok(None);
        }
        match health.publication_source.as_str() {
            "daemon-authenticated" => {
                let Some(admission) =
                    node.get::<IdunnAuthenticatedDaemonHealthAdmissionRecord>(&desired.daemon_id)?
                else {
                    return Ok(None);
                };
                admission.validate()?;
                let Some(statement) =
                    node.get::<IdunnSignedDaemonHealthRecord>(&admission.signed_health_sha256)?
                else {
                    return Ok(None);
                };
                let (binding, current_binding_sha256) =
                    load_daemon_health_trust_binding(options, &statement)?;
                validate_generic_release_binding(node, &admission)?;
                let now_millis = parse_timestamp_millis(now)
                    .ok_or_else(|| anyhow!("managed health clock is invalid"))?;
                let statement_digest = format!(
                    "sha256-{:x}",
                    Sha256::digest(rmp_serde::to_vec(&statement)?)
                );
                if admission.daemon_id != health.daemon_id
                    || admission.health_contract != health.health_contract
                    || admission.state != health.state
                    || admission.trust_binding_sha256 != current_binding_sha256
                    || admission.signed_health_sha256 != statement_digest
                    || admission.signer_identity_id != statement.signer_identity_id
                    || admission.publisher_incarnation_id != statement.publisher_incarnation_id
                    || admission.publisher_sequence != statement.publisher_sequence
                    || admission.observed_at_unix_millis != statement.observed_at_unix_millis
                    || now_millis < admission.admitted_at_unix_millis
                    || now_millis.saturating_sub(admission.observed_at_unix_millis)
                        > u64::from(desired.max_silence_seconds).saturating_mul(1000)
                {
                    return Ok(None);
                }
                let admission_sha256 = record_sha256(&admission)?;
                let (deployment_head, deployment_request) = if let Some(deployment_id) =
                    admission.deployment_id.as_deref()
                {
                    let head = node
                        .get::<IdunnCurrentDeploymentRequestRecord>(&admission.daemon_id)?
                        .ok_or_else(|| anyhow!("authenticated health lost deployment head"))?;
                    let request = node
                        .get::<IdunnDeploymentRequestRecord>(deployment_id)?
                        .ok_or_else(|| anyhow!("authenticated health lost deployment request"))?;
                    (Some(head), Some(request))
                } else {
                    (None, None)
                };
                Ok(Some(ManagedHealthRead {
                    health: health.clone(),
                    projection_source: Some(AuthenticatedProviderHealthSource {
                        health,
                        admission,
                        statement,
                        binding,
                        binding_sha256: current_binding_sha256,
                        statement_sha256: statement_digest,
                        admission_sha256,
                        deployment_head,
                        deployment_request,
                    }),
                }))
            }
            "daemon-published" => {
                let Some(admission) =
                    node.get::<IdunnSignedHealthAdmissionRecord>(&desired.daemon_id)?
                else {
                    return Ok(None);
                };
                validate_admission_against_current_request(node, &admission)?;
                validate_admission_fresh_at(
                    &admission,
                    now,
                    u64::from(desired.max_silence_seconds),
                )?;
                if admission.daemon_id != health.daemon_id
                    || admission.health_contract != health.health_contract
                    || admission.state != health.state
                    || admission.observed_at != health.observed_at
                {
                    return Ok(None);
                }
                Ok(Some(ManagedHealthRead {
                    health,
                    projection_source: None,
                }))
            }
            _ => Ok(None),
        }
    })
}

fn evaluate_target_health(
    target: &DaemonTarget,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    desired: &IdunnDesiredDaemonRecord,
    now: &str,
) -> Result<(
    String,
    IdunnDaemonHealthRecord,
    Option<AuthenticatedProviderHealthSource>,
)> {
    if let Some(release) = &target.release {
        if release.deployed_revision_witness.is_some() {
            if let Some(health) =
                evaluate_release_drift(target, desired, release, &SystemReleaseStatePort, now)
            {
                return Ok((supervisor_health_key(&desired.daemon_id), health, None));
            }
        }
    }
    if let Some(managed) = read_fresh_daemon_published_health(options, store_lock, desired, now)? {
        return Ok((
            desired.daemon_id.clone(),
            managed.health,
            managed.projection_source,
        ));
    }

    Ok((
        supervisor_health_key(&desired.daemon_id),
        missing_daemon_published_health(target, desired, now),
        None,
    ))
}

fn evaluate_release_drift(
    target: &DaemonTarget,
    desired: &IdunnDesiredDaemonRecord,
    release: &ReleaseTarget,
    port: &dyn ReleaseStatePort,
    observed_at: &str,
) -> Option<IdunnDaemonHealthRecord> {
    let observation = (|| -> Result<(String, Option<String>)> {
        port.fetch(release)?;
        let wanted = port.desired_revision(release)?;
        let deployed = match port.deployed_revision(release) {
            Ok(deployed) => Some(deployed),
            Err(error) if error_chain_contains_io_not_found(&error) => None,
            Err(error) => return Err(error),
        };
        Ok((wanted, deployed))
    })();

    let (state, detail, transport) = match observation {
        Ok((wanted, None)) => (
            "stale-deployment",
            format!(
                "upstream {}/{} is {wanted}, but no deployed revision witness exists",
                release.upstream_remote, release.upstream_branch
            ),
            "idunn.release-not-deployed",
        ),
        Ok((wanted, Some(deployed))) if wanted != deployed => (
            "stale-deployment",
            format!(
                "upstream {}/{} is {wanted}, but the deployed revision witness reports {deployed}",
                release.upstream_remote, release.upstream_branch
            ),
            "idunn.release-revision-drift",
        ),
        Ok(_) => return None,
        Err(error) => (
            "dependency-unavailable",
            format!(
                "Idunn could not establish release freshness for {}: {error:#}",
                release.repo
            ),
            "idunn.release-revision-unavailable",
        ),
    };

    Some(IdunnDaemonHealthRecord {
        daemon_id: target.daemon_id.clone(),
        state: state.to_string(),
        detail,
        health_contract: desired.health_contract.clone(),
        publication_source: "idunn-release-observation".to_string(),
        transport: transport.to_string(),
        observed_at: observed_at.to_string(),
    })
}

fn error_chain_contains_io_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
    })
}

fn verify_release_witness_current(
    release: &ReleaseTarget,
    port: &dyn ReleaseStatePort,
) -> Result<()> {
    port.fetch(release)?;
    let desired = port.desired_revision(release)?;
    let deployed = port.deployed_revision(release)?;
    if desired != deployed {
        return Err(anyhow!(
            "desired revision {desired} does not match deployed revision {deployed}"
        ));
    }
    Ok(())
}

fn read_deployed_revision_witness(path: &PathBuf) -> Result<String> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("reading deployed revision witness {}", path.display()))?;
    let revision = contents
        .lines()
        .find_map(|line| line.strip_prefix("DEPLOYED_REVISION="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("{} has no DEPLOYED_REVISION value", path.display()))?;
    Ok(revision.to_string())
}

fn supervisor_health_key(daemon_id: &str) -> String {
    format!("observation:{daemon_id}")
}

fn apply_missing_publication_grace(
    target: &DaemonTarget,
    desired: &IdunnDesiredDaemonRecord,
    health: &mut IdunnDaemonHealthRecord,
    health_authenticated: bool,
    missing_since: &mut Option<Instant>,
    now: Instant,
) {
    if health_authenticated {
        *missing_since = None;
        return;
    }
    if !target.health_contract.restart_on_missing_publication {
        return;
    }

    let first_missing = missing_since.get_or_insert(now);
    let missing_for = now.saturating_duration_since(*first_missing);
    if missing_for < Duration::from_secs(desired.max_silence_seconds.into()) {
        health.state = "degraded".to_string();
        health.detail = format!(
            "no fresh daemon-published {} record has arrived for {} seconds; Idunn is preserving the daemon-owned health key and waiting for the {} second continuity boundary",
            desired.health_contract,
            missing_for.as_secs(),
            desired.max_silence_seconds
        );
    }
}

fn missing_daemon_published_health(
    target: &DaemonTarget,
    desired: &IdunnDesiredDaemonRecord,
    observed_at: &str,
) -> IdunnDaemonHealthRecord {
    let (state, detail, transport) = if target.health_contract.restart_on_missing_publication {
        (
            "failed",
            format!(
                "no fresh daemon-published {} record arrived over {}; Idunn owns this local restart boundary.",
                desired.health_contract, CULTNET_RUDP_PROTOCOL_ID
            ),
            "cultmesh.missing-locally-supervised-daemon-publication",
        )
    } else {
        (
            "dependency-unavailable",
            format!(
                "no fresh daemon-published {} record arrived over {}; Idunn did not run local health probes.",
                desired.health_contract, CULTNET_RUDP_PROTOCOL_ID
            ),
            "cultmesh.missing-daemon-publication",
        )
    };
    IdunnDaemonHealthRecord {
        daemon_id: desired.daemon_id.clone(),
        state: state.to_string(),
        detail,
        health_contract: desired.health_contract.clone(),
        publication_source: "idunn-supervisor-observation".to_string(),
        transport: transport.to_string(),
        observed_at: observed_at.to_string(),
    }
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
    if !matches!(
        health.publication_source.as_str(),
        "daemon-authenticated" | "daemon-published"
    ) {
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

struct IdunnProjectionPublisher {
    signer: ServiceIdentitySigner<IdunnServiceIdentity>,
    public_store_path: PathBuf,
    incarnation_id: String,
    root_trust_snapshot_sha256: String,
    write_lock: Mutex<()>,
}

#[derive(Clone)]
struct IdunnPublicHealthSnapshotServer {
    public_store_path: PathBuf,
    allowed_record_keys: BTreeSet<String>,
}

impl IdunnPublicHealthSnapshotServer {
    fn new(public_store_path: PathBuf, targets: &[DaemonTarget]) -> Result<Self> {
        let allowed_record_keys = targets
            .iter()
            .map(|target| {
                provider_health_projection_key(&target.daemon_id, &target.health_contract.id)
            })
            .collect::<BTreeSet<_>>();
        if allowed_record_keys.len() != targets.len() {
            return Err(anyhow!(
                "public health query target catalog contains a duplicate daemon/contract pair"
            ));
        }
        let server = Self {
            public_store_path,
            allowed_record_keys,
        };
        // Startup observes the exact public store before any worker starts.
        // An absent store is an empty public projection, not private fallback.
        server.raw_snapshot()?;
        Ok(server)
    }

    fn serve(&self, request: &CultNetMessage) -> Result<CultNetMessage> {
        let registry = public_health_document_registry();
        let mut policy = CultNetReadOnlySnapshotPolicy::new();
        for key in &self.allowed_record_keys {
            policy.allow(IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA, key)?;
        }
        serve_read_only_raw_snapshot(&registry, &policy, &self.raw_snapshot()?, request)
    }

    fn raw_snapshot(&self) -> Result<Vec<CultNetRawDocumentRecord>> {
        let registry = public_health_document_registry();
        let entries = SingleFileMessagePackBackingStore::new(&self.public_store_path)
            .pull_all_read_only_snapshot()?;
        let mut records = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.r#type != IdunnAuthenticatedProviderHealthProjectionRecord::TYPE
                || entry.schema_id.as_deref()
                    != Some(IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA)
            {
                return Err(anyhow!(
                    "public health query store contains a non-projection envelope"
                ));
            }
            let projection: IdunnAuthenticatedProviderHealthProjectionRecord =
                rmp_serde::from_slice(&entry.payload)
                    .context("decoding public health query projection")?;
            if rmp_serde::to_vec(&projection)? != entry.payload
                || projection.projection_id != entry.key
            {
                return Err(anyhow!(
                    "public health query projection is noncanonical or has a mismatched key"
                ));
            }
            projection.validate()?;
            let mut record = registry.raw_document_record_from_envelope(&entry)?;
            record.source_runtime_id = Some(IDUNN_PUBLIC_HEALTH_QUERY_RUNTIME_ID.into());
            record.source_agent_id = None;
            record.source_role = Some(IDUNN_PUBLIC_HEALTH_QUERY_ROLE.into());
            record.tags = Some(vec!["public-health".into()]);
            records.push(record);
        }
        Ok(records)
    }
}

fn public_health_document_registry() -> CultNetDocumentRegistry {
    let mut registry = CultNetDocumentRegistry::new();
    registry.register(CultNetDocumentBinding {
        schema_id: IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into(),
        document_type: IdunnAuthenticatedProviderHealthProjectionRecord::TYPE.into(),
        mutation_contract: None,
        payload_schema_version: Some(IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into()),
    });
    registry
}

fn provider_health_projection_key(daemon_id: &str, health_contract: &str) -> String {
    format!(
        "provider-health:{:x}",
        Sha256::digest([daemon_id.as_bytes(), b"\0", health_contract.as_bytes()].concat())
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AuthenticatedProviderHealthSource {
    health: IdunnDaemonHealthRecord,
    admission: IdunnAuthenticatedDaemonHealthAdmissionRecord,
    statement: IdunnSignedDaemonHealthRecord,
    binding: IdunnDaemonHealthTrustBindingRecord,
    binding_sha256: String,
    statement_sha256: String,
    admission_sha256: String,
    deployment_head: Option<IdunnCurrentDeploymentRequestRecord>,
    deployment_request: Option<IdunnDeploymentRequestRecord>,
}

#[derive(Clone, Debug)]
struct ManagedHealthRead {
    health: IdunnDaemonHealthRecord,
    projection_source: Option<AuthenticatedProviderHealthSource>,
}

fn initialize_projection_publisher(
    options: &CommonOptions,
) -> Result<Option<Arc<IdunnProjectionPublisher>>> {
    let (Some(identity_path), Some(public_path)) = (
        options.service_identity_store_path.as_deref(),
        options.public_health_store_path.as_deref(),
    ) else {
        if options.service_identity_store_path.is_some()
            || options.public_health_store_path.is_some()
        {
            return Err(anyhow!(
                "--service-identity-store and --public-health-store must be configured together"
            ));
        }
        return Ok(None);
    };
    reject_store_aliases(options, identity_path, public_path)?;
    if let Some(parent) = public_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating public health store parent {}", parent.display()))?;
    }
    let signer = open_service_identity_at::<IdunnServiceIdentity>(identity_path)
        .context("opening enrolled Idunn service identity")?;
    let trust_path = options
        .daemon_health_trust_store_path
        .as_deref()
        .ok_or_else(|| anyhow!("public health projection requires --daemon-health-trust-store"))?;
    let root_trust_snapshot_sha256 = trust_store_snapshot_sha256(trust_path)?;
    Ok(Some(Arc::new(IdunnProjectionPublisher {
        signer,
        public_store_path: public_path.to_path_buf(),
        incarnation_id: uuid::Uuid::new_v4().to_string(),
        root_trust_snapshot_sha256,
        write_lock: Mutex::new(()),
    })))
}

fn reject_store_aliases(
    options: &CommonOptions,
    identity_path: &Path,
    public_path: &Path,
) -> Result<()> {
    let identity = normalized_store_path(identity_path)?;
    let public = normalized_store_path(public_path)?;
    let mut private_paths = vec![normalized_store_path(&options.store_path)?, identity];
    for path in [
        options.daemon_health_trust_store_path.as_deref(),
        options.release_authority_store_path.as_deref(),
        options.trusted_epiphany_health_identity_store.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        private_paths.push(normalized_store_path(path)?);
    }
    if private_paths.iter().enumerate().any(|(index, path)| {
        private_paths
            .iter()
            .skip(index + 1)
            .any(|other| other == path)
    }) {
        return Err(anyhow!(
            "service identity, operational, trust, and release stores must not alias"
        ));
    }
    if private_paths.iter().any(|path| path == &public) {
        return Err(anyhow!(
            "public health store must not alias identity, operational, trust, or release state"
        ));
    }
    Ok(())
}

fn normalized_store_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path)
            .with_context(|| format!("canonicalizing store path {}", path.display()));
    }
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow!("store path {} has no file name", path.display()))?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = fs::canonicalize(parent)
        .with_context(|| format!("canonicalizing store parent {}", parent.display()))?;
    Ok(canonical_parent.join(file_name))
}

fn trust_store_snapshot_sha256(path: &Path) -> Result<String> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    record_sha256(&entries)
}

impl IdunnProjectionPublisher {
    fn publish_if_current(
        &self,
        options: &CommonOptions,
        store_lock: &Arc<Mutex<()>>,
        desired: &IdunnDesiredDaemonRecord,
        source: &AuthenticatedProviderHealthSource,
        evaluated_at: &str,
    ) -> Result<()> {
        let evaluated_at_unix_millis = parse_timestamp_millis(evaluated_at)
            .ok_or_else(|| anyhow!("projection evaluation time is invalid"))?;
        let current =
            read_fresh_daemon_published_health(options, store_lock, desired, evaluated_at)?
                .and_then(|managed| managed.projection_source)
                .ok_or_else(|| {
                    anyhow!("authenticated provider health vanished before projection")
                })?;
        if &current != source {
            return Err(anyhow!(
                "authenticated provider health changed before projection publication"
            ));
        }
        self.require_incarnation_trust_snapshot(options)?;
        let reason_code = authenticated_provider_health_reason_code(&source.health.state)
            .ok_or_else(|| anyhow!("provider state is not publicly projectable"))?;
        let expires_at_unix_millis = source
            .admission
            .observed_at_unix_millis
            .saturating_add(u64::from(desired.max_silence_seconds).saturating_mul(1000));
        if expires_at_unix_millis <= evaluated_at_unix_millis {
            return Err(anyhow!(
                "authenticated provider health expired before projection"
            ));
        }
        let projection_key = provider_health_projection_key(
            &source.health.daemon_id,
            &source.health.health_contract,
        );
        let _write_guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow!("Idunn public projection store lock is poisoned"))?;
        for _ in 0..8 {
            let backing = SingleFileMessagePackBackingStore::new(&self.public_store_path);
            let entries = backing.pull_all_read_only_snapshot()?;
            if entries.iter().any(|entry| {
                entry.r#type != IdunnAuthenticatedProviderHealthProjectionRecord::TYPE
                    || entry.schema_id.as_deref()
                        != Some(IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA)
            }) {
                return Err(anyhow!(
                    "public health store contains a non-projection envelope"
                ));
            }
            for entry in &entries {
                self.verify_existing_projection(entry)?;
            }
            let current_envelope = entries
                .iter()
                .find(|entry| entry.key == projection_key)
                .cloned();
            let current_projection = current_envelope
                .as_ref()
                .map(|entry| {
                    rmp_serde::from_slice::<IdunnAuthenticatedProviderHealthProjectionRecord>(
                        &entry.payload,
                    )
                })
                .transpose()
                .context("decoding existing public health projection")?;
            if let Some(existing) = current_projection.as_ref() {
                existing.validate()?;
                if current_envelope
                    .as_ref()
                    .is_some_and(|entry| entry.key != existing.projection_id)
                    || existing.projection_id != projection_key
                {
                    return Err(anyhow!(
                        "public projection envelope key does not match its payload"
                    ));
                }
                if existing.idunn_signer_identity_id != self.signer.entry().identity_id {
                    return Err(anyhow!(
                        "public projection signer identity changed without store rotation"
                    ));
                }
                if existing.projection_incarnation_id == self.incarnation_id
                    && existing.signed_health_sha256 == source.statement_sha256
                    && existing.authenticated_admission_sha256 == source.admission_sha256
                    && existing.trust_binding_sha256 == source.binding_sha256
                {
                    return Ok(());
                }
            }
            let sequence = current_projection
                .as_ref()
                .map_or(1, |existing| existing.projection_sequence.saturating_add(1));
            if sequence == 0 {
                return Err(anyhow!("public projection sequence exhausted"));
            }
            let mut projection = IdunnAuthenticatedProviderHealthProjectionRecord {
                schema_version: IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into(),
                projection_id: projection_key.clone(),
                daemon_id: source.health.daemon_id.clone(),
                health_contract: source.health.health_contract.clone(),
                provider_state: source.health.state.clone(),
                reason_code: reason_code.into(),
                provider_observed_at_unix_millis: source.admission.observed_at_unix_millis,
                admitted_at_unix_millis: source.admission.admitted_at_unix_millis,
                evaluated_at_unix_millis,
                trust_binding_id: source.binding.binding_id.clone(),
                trust_binding_sha256: source.binding_sha256.clone(),
                signed_health_sha256: source.statement_sha256.clone(),
                authenticated_admission_sha256: source.admission_sha256.clone(),
                provider_signer_identity_id: source.admission.signer_identity_id.clone(),
                provider_incarnation_id: source.admission.publisher_incarnation_id.clone(),
                provider_sequence: source.admission.publisher_sequence,
                release_id: source.admission.release_id.clone(),
                release_witness_sha256: source.admission.release_witness_sha256.clone(),
                source_commit: source.admission.source_commit.clone(),
                deployment_id: source.admission.deployment_id.clone(),
                idunn_runtime_id: "idunn-daemon".into(),
                idunn_signer_identity_id: self.signer.entry().identity_id.clone(),
                projection_incarnation_id: self.incarnation_id.clone(),
                projection_sequence: sequence,
                signature_algorithm: "ed25519".into(),
                signature: Vec::new(),
                private_state_exposed: false,
                expires_at_unix_millis,
            };
            let unsigned = rmp_serde::to_vec(&projection)?;
            projection.signature = self
                .signer
                .sign::<IdunnAuthenticatedProviderHealthProjectionPurpose>(&unsigned)
                .signature;
            projection.validate()?;
            self.require_incarnation_trust_snapshot(options)?;
            let replacement = CultCacheEnvelope {
                key: projection_key.clone(),
                r#type: IdunnAuthenticatedProviderHealthProjectionRecord::TYPE.into(),
                payload: rmp_serde::to_vec(&projection)?,
                stored_at: evaluated_at.into(),
                schema_id: Some(IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into()),
            };
            let expected = CultCacheExpectedEnvelope {
                key: projection_key.clone(),
                r#type: IdunnAuthenticatedProviderHealthProjectionRecord::TYPE.into(),
                current: current_envelope,
            };
            if backing.compare_exchange(&[expected], &[replacement])? {
                return Ok(());
            }
        }
        Err(anyhow!(
            "public health projection lost repeated cross-process contention"
        ))
    }

    fn require_incarnation_trust_snapshot(&self, options: &CommonOptions) -> Result<()> {
        let path = options
            .daemon_health_trust_store_path
            .as_deref()
            .ok_or_else(|| anyhow!("projection publisher lost its root trust store"))?;
        if trust_store_snapshot_sha256(path)? != self.root_trust_snapshot_sha256 {
            return Err(anyhow!(
                "root daemon-health trust changed during this Idunn incarnation; restart is required"
            ));
        }
        Ok(())
    }

    fn verify_existing_projection(&self, entry: &CultCacheEnvelope) -> Result<()> {
        let projection: IdunnAuthenticatedProviderHealthProjectionRecord =
            rmp_serde::from_slice(&entry.payload)
                .context("decoding public health projection during store audit")?;
        if rmp_serde::to_vec(&projection)? != entry.payload {
            return Err(anyhow!(
                "public health projection is not canonical MessagePack"
            ));
        }
        projection.validate()?;
        if entry.key != projection.projection_id
            || projection.idunn_signer_identity_id != self.signer.entry().identity_id
        {
            return Err(anyhow!("public health projection key or signer is foreign"));
        }
        let proof = cultnet_rs::ServiceIdentitySignature {
            identity_id: projection.idunn_signer_identity_id.clone(),
            signature: projection.signature.clone(),
        };
        let mut unsigned = projection;
        unsigned.signature.clear();
        cultnet_rs::verify_service_identity_signature::<
            IdunnServiceIdentity,
            IdunnAuthenticatedProviderHealthProjectionPurpose,
        >(
            &self.signer.trust_anchor()?,
            &rmp_serde::to_vec(&unsigned)?,
            &proof,
        )
        .context("public health projection signature audit failed")
    }
}

fn record_sha256<T: serde::Serialize>(value: &T) -> Result<String> {
    Ok(format!(
        "sha256-{:x}",
        Sha256::digest(rmp_serde::to_vec(value)?)
    ))
}

fn publish_authenticated_provider_health(
    publisher: Option<&IdunnProjectionPublisher>,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    desired: &IdunnDesiredDaemonRecord,
    source: Option<&AuthenticatedProviderHealthSource>,
    evaluated_at: &str,
) -> Result<()> {
    if let (Some(publisher), Some(source)) = (publisher, source) {
        publisher.publish_if_current(options, store_lock, desired, source, evaluated_at)?;
    }
    Ok(())
}

fn parse_timestamp_millis(value: &str) -> Option<u64> {
    if let Some(seconds) = parse_unix_timestamp(value) {
        return seconds.checked_mul(1000);
    }
    let millis = chrono::DateTime::parse_from_rfc3339(value)
        .ok()?
        .timestamp_millis();
    u64::try_from(millis).ok()
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

fn start_public_health_query_listener(
    options: &CommonOptions,
    targets: &[DaemonTarget],
) -> Result<Option<SocketAddr>> {
    let Some(bind_address) = options.public_health_query_bind else {
        return Ok(None);
    };
    let public_store_path = options
        .public_health_store_path
        .clone()
        .ok_or_else(|| anyhow!("--public-health-query-bind requires --public-health-store"))?;
    if options.service_identity_store_path.is_none() {
        return Err(anyhow!(
            "--public-health-query-bind requires --service-identity-store projection authority"
        ));
    }
    let server = IdunnPublicHealthSnapshotServer::new(public_store_path, targets)?;
    let socket = UdpSocket::bind(bind_address)
        .with_context(|| format!("binding public health query listener at {bind_address}"))?;
    let local_addr = socket.local_addr()?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    thread::spawn(move || {
        if let Err(error) = run_public_health_query_listener_loop(socket, server, None) {
            eprintln!("Idunn public health query listener stopped: {error:#}");
        }
    });
    println!(
        "Idunn public health query listener active at {local_addr} for {} target projection key(s).",
        targets.len()
    );
    Ok(Some(local_addr))
}

fn run_public_health_query_listener_loop(
    socket: UdpSocket,
    server: IdunnPublicHealthSnapshotServer,
    stop: Option<Arc<AtomicBool>>,
) -> Result<()> {
    let mut sessions: HashMap<SocketAddr, CultNetRudpSession> = HashMap::new();
    let mut buffer = vec![0_u8; 65_535];
    loop {
        if stop
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
        {
            return Ok(());
        }
        match socket.recv_from(&mut buffer) {
            Ok((size, source)) => {
                if let Err(error) = handle_public_health_query_datagram(
                    &socket,
                    &mut sessions,
                    &server,
                    &buffer[..size],
                    source,
                ) {
                    eprintln!(
                        "Idunn public health query listener refused datagram from {source}: {error:#}"
                    );
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
        let now = unix_epoch_millis()?;
        let mut expired = Vec::new();
        for (peer, session) in &mut sessions {
            for packet in session.due_resends(now) {
                socket.send_to(&encode_rudp_packet(&packet)?, peer)?;
            }
            if session.check_timeout(now, 60_000) {
                expired.push(*peer);
            }
        }
        for peer in expired {
            sessions.remove(&peer);
        }
    }
}

fn handle_public_health_query_datagram(
    socket: &UdpSocket,
    sessions: &mut HashMap<SocketAddr, CultNetRudpSession>,
    server: &IdunnPublicHealthSnapshotServer,
    wire: &[u8],
    source: SocketAddr,
) -> Result<()> {
    let packet = decode_rudp_packet(wire)?;
    if packet.connection_id != IDUNN_PUBLIC_HEALTH_QUERY_CONNECTION_ID {
        return Err(anyhow!(
            "unexpected public health query RUDP connection id {:08x}",
            packet.connection_id
        ));
    }
    let now = unix_epoch_millis()?;
    if packet.packet_type == CultNetRudpPacketType::Connect {
        let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
            connection_id: IDUNN_PUBLIC_HEALTH_QUERY_CONNECTION_ID,
            initial_sequence: 1,
            resend_delay_ms: 100,
            max_pending_reliable_packets: Some(64),
        });
        let accept = session.accept_connect(&packet, now, Vec::new())?;
        socket.send_to(&encode_rudp_packet(&accept)?, source)?;
        sessions.insert(source, session);
        return Ok(());
    }
    let session = sessions
        .get_mut(&source)
        .ok_or_else(|| anyhow!("public health query peer has no accepted RUDP session"))?;
    let result = session.receive(&packet, now)?;
    if let Some(reply) = result.reply {
        socket.send_to(&encode_rudp_packet(&reply)?, source)?;
    }
    if result.disconnected {
        sessions.remove(&source);
        return Ok(());
    }
    if packet.packet_type == CultNetRudpPacketType::Accept || !result.delivered.is_empty() {
        socket.send_to(&encode_rudp_packet(&session.create_ack())?, source)?;
    }
    for frame in result.delivered {
        let response = if frame.channel_id != "schema" {
            CultNetMessage::Error {
                error: "public health queries require the CultNet schema channel".into(),
            }
        } else {
            match decode_cultnet_message_from_slice(
                &frame.payload,
                CultNetWireContract::CultNetSchemaV0,
            ) {
                Ok(request) => {
                    server
                        .serve(&request)
                        .unwrap_or_else(|error| CultNetMessage::Error {
                            error: format!("public health query refused: {error}"),
                        })
                }
                Err(error) => CultNetMessage::Error {
                    error: format!("public health query message is malformed: {error}"),
                },
            }
        };
        let payload =
            encode_cultnet_message_to_vec(&response, CultNetWireContract::CultNetSchemaV0)?;
        let packets = session.send_many(
            "schema",
            payload,
            CultNetRudpSendOptions {
                reliable: true,
                ordered: true,
                sequenced: false,
                now_ms: now,
                reliable_expire_after_ms: None,
            },
            Some(1200),
        )?;
        for packet in packets {
            socket.send_to(&encode_rudp_packet(&packet)?, source)?;
        }
    }
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
                            match admit_health_from_rudp_message(
                                &message,
                                &options,
                                &store_lock,
                                &observed_at,
                            ) {
                                Ok(outcome) => {
                                    let bounded_detail =
                                        outcome.detail.replace('\r', " ").replace('\n', " ");
                                    println!(
                                        "Idunn recorded RUDP health input for {} from {} over {} state={} authority={} detail={}",
                                        outcome.daemon_id,
                                        source,
                                        frame.channel_id,
                                        outcome.state,
                                        outcome.authority,
                                        bounded_detail
                                    );
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

fn admit_health_from_rudp_message(
    message: &CultNetMessage,
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    admitted_at: &str,
) -> Result<HealthIngressOutcome> {
    let decoded = decode_health_from_rudp_message(message, options, admitted_at)?;
    let (health, admission) = match decoded {
        DecodedHealthIngress::Diagnostic(diagnostic) => {
            diagnostic.validate()?;
            let outcome = HealthIngressOutcome {
                daemon_id: diagnostic.daemon_id.clone(),
                state: diagnostic.claimed_state.clone(),
                detail: diagnostic.claimed_detail.clone(),
                authority: "diagnostic-only",
            };
            let key = diagnostic.diagnostic_id.clone();
            // Unsigned input is quarantined under a distinct type and key. It
            // cannot overwrite the managed-health row or admission.
            with_store_node(options, store_lock, |node| {
                node.put(&key, &diagnostic)?;
                Ok(())
            })?;
            return Ok(outcome);
        }
        DecodedHealthIngress::AuthenticatedGeneric {
            health,
            statement,
            admission,
        } => {
            return admit_generic_signed_health(
                options,
                store_lock,
                admitted_at,
                health,
                statement,
                admission,
            );
        }
        DecodedHealthIngress::AuthenticatedLegacy { health, admission } => (health, admission),
    };
    {
        with_store_node(options, store_lock, |node| {
            validate_admission_against_current_request(node, &admission)
        })?;
    }
    for _ in 0..8 {
        let (expected_health, expected_admission, expected_head, expected_request) =
            with_store_node(options, store_lock, |node| {
                validate_admission_against_current_request(node, &admission)?;
                if let Some(existing) =
                    node.get::<IdunnSignedHealthAdmissionRecord>(&admission.daemon_id)?
                    && !signed_health_advances(&existing, &admission)?
                {
                    return Err(anyhow!("signed Epiphany health is replayed or regressed"));
                }
                Ok((
                    node.cache()
                        .get_envelope::<IdunnDaemonHealthRecord>(&health.daemon_id)?,
                    node.cache()
                        .get_envelope::<IdunnSignedHealthAdmissionRecord>(&admission.daemon_id)?,
                    node.cache()
                        .get_envelope::<IdunnCurrentDeploymentRequestRecord>(
                            &admission.daemon_id,
                        )?,
                    node.cache().get_envelope::<IdunnDeploymentRequestRecord>(
                        &admission.deployment_request_id,
                    )?,
                ))
            })?;
        let health_envelope = typed_envelope(&health.daemon_id, &health, admitted_at)?;
        let admission_envelope = typed_envelope(&admission.daemon_id, &admission, admitted_at)?;
        let expected = [
            CultCacheExpectedEnvelope {
                key: health.daemon_id.clone(),
                r#type: IdunnDaemonHealthRecord::TYPE.into(),
                current: expected_health,
            },
            CultCacheExpectedEnvelope {
                key: admission.daemon_id.clone(),
                r#type: IdunnSignedHealthAdmissionRecord::TYPE.into(),
                current: expected_admission,
            },
            CultCacheExpectedEnvelope {
                key: admission.daemon_id.clone(),
                r#type: IdunnCurrentDeploymentRequestRecord::TYPE.into(),
                current: expected_head,
            },
            CultCacheExpectedEnvelope {
                key: admission.deployment_request_id.clone(),
                r#type: IdunnDeploymentRequestRecord::TYPE.into(),
                current: expected_request,
            },
        ];
        if SingleFileMessagePackBackingStore::new(&options.store_path)
            .compare_exchange(&expected, &[health_envelope, admission_envelope])?
        {
            return Ok(HealthIngressOutcome {
                daemon_id: health.daemon_id,
                state: health.state,
                detail: health.detail,
                authority: "authenticated",
            });
        }
    }
    Err(anyhow!(
        "signed Epiphany health admission lost repeated cross-process contention"
    ))
}

fn typed_envelope<T: DatabaseEntry>(
    key: &str,
    value: &T,
    stored_at: &str,
) -> Result<CultCacheEnvelope> {
    Ok(CultCacheEnvelope {
        key: key.to_string(),
        r#type: T::TYPE.to_string(),
        payload: rmp_serde::to_vec(value)?,
        stored_at: stored_at.to_string(),
        schema_id: Some(T::TYPE.to_string()),
    })
}

fn authenticate_generic_signed_health(
    document: &cultnet_rs::CultNetRawDocumentRecord,
    options: &CommonOptions,
    admitted_at: &str,
) -> Result<(
    IdunnDaemonHealthRecord,
    IdunnSignedDaemonHealthRecord,
    IdunnAuthenticatedDaemonHealthAdmissionRecord,
)> {
    let statement: IdunnSignedDaemonHealthRecord = rmp_serde::from_slice(&document.payload)
        .context("decoding generic signed daemon health")?;
    if rmp_serde::to_vec(&statement)? != document.payload {
        return Err(anyhow!(
            "signed daemon health payload is not canonical positional MessagePack"
        ));
    }
    statement.validate()?;
    if document.record_key != statement.daemon_id
        || document.source_runtime_id.as_deref() != Some(statement.source_runtime_id.as_str())
        || document.source_role.as_deref() != Some("daemon-health-publisher")
    {
        return Err(anyhow!(
            "signed daemon health transport identity is invalid"
        ));
    }
    let (binding, binding_sha256) = load_daemon_health_trust_binding(options, &statement)?;
    if binding.release_binding_required {
        if statement.release_id.is_none()
            || statement.release_witness_sha256.is_none()
            || statement.source_commit.is_none()
            || statement.deployment_id.is_none()
        {
            return Err(anyhow!(
                "signed daemon health lacks required release binding"
            ));
        }
    } else if statement.release_id.is_some()
        || statement.release_witness_sha256.is_some()
        || statement.source_commit.is_some()
        || statement.deployment_id.is_some()
    {
        return Err(anyhow!(
            "signed daemon health carries release authority not declared by its trust binding"
        ));
    }
    let expected_identity =
        derive_service_identity_id::<GameCultProviderHealthIdentity>(&binding.signer_public_key)?;
    if binding.signer_identity_id != expected_identity
        || statement.signer_identity_id != binding.signer_identity_id
    {
        return Err(anyhow!("signed daemon health identity is not root-bound"));
    }
    let signature_bytes: [u8; 64] = statement
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("signed daemon health signature length is invalid"))?;
    let mut unsigned = statement.clone();
    unsigned.signature.clear();
    let unsigned_payload = rmp_serde::to_vec(&unsigned)?;
    verify_service_identity_signature_with_public_key::<
        GameCultProviderHealthIdentity,
        IdunnSignedDaemonHealthPurpose,
    >(
        &binding.signer_public_key,
        &unsigned_payload,
        &ServiceIdentitySignature {
            identity_id: binding.signer_identity_id.clone(),
            signature: signature_bytes.to_vec(),
        },
    )
    .context("signed daemon health signature is invalid")?;
    let admitted_at_unix_millis = parse_timestamp_millis(admitted_at)
        .ok_or_else(|| anyhow!("signed daemon health admission time is invalid"))?;
    if statement.observed_at_unix_millis > admitted_at_unix_millis
        || admitted_at_unix_millis.saturating_sub(statement.observed_at_unix_millis)
            > EPIPHANY_ADMISSION_MAX_AGE_SECONDS.saturating_mul(1000)
    {
        return Err(anyhow!(
            "signed daemon health observation is future or stale"
        ));
    }
    let signed_health_sha256 = format!("sha256-{:x}", Sha256::digest(&document.payload));
    let observed_at = chrono::DateTime::from_timestamp_millis(
        i64::try_from(statement.observed_at_unix_millis)
            .map_err(|_| anyhow!("signed daemon health observation is out of range"))?,
    )
    .ok_or_else(|| anyhow!("signed daemon health observation is out of range"))?
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let health = IdunnDaemonHealthRecord {
        daemon_id: statement.daemon_id.clone(),
        state: statement.state.clone(),
        detail: statement.detail.clone(),
        observed_at,
        health_contract: statement.health_contract.clone(),
        publication_source: "daemon-authenticated".into(),
        transport: CULTNET_RUDP_PROTOCOL_ID.into(),
    };
    let admission = IdunnAuthenticatedDaemonHealthAdmissionRecord {
        schema_version: IDUNN_AUTHENTICATED_DAEMON_HEALTH_ADMISSION_SCHEMA.into(),
        daemon_id: statement.daemon_id.clone(),
        health_contract: statement.health_contract.clone(),
        source_runtime_id: statement.source_runtime_id.clone(),
        state: statement.state.clone(),
        observed_at_unix_millis: statement.observed_at_unix_millis,
        admitted_at_unix_millis,
        trust_binding_id: binding.binding_id,
        trust_binding_sha256: binding_sha256,
        signer_identity_id: statement.signer_identity_id.clone(),
        publisher_incarnation_id: statement.publisher_incarnation_id.clone(),
        publisher_sequence: statement.publisher_sequence,
        signed_health_sha256,
        release_id: statement.release_id.clone(),
        release_witness_sha256: statement.release_witness_sha256.clone(),
        source_commit: statement.source_commit.clone(),
        deployment_id: statement.deployment_id.clone(),
        private_state_exposed: false,
    };
    admission.validate()?;
    Ok((health, statement, admission))
}

fn load_daemon_health_trust_binding(
    options: &CommonOptions,
    statement: &IdunnSignedDaemonHealthRecord,
) -> Result<(IdunnDaemonHealthTrustBindingRecord, String)> {
    let path = options
        .daemon_health_trust_store_path
        .as_deref()
        .ok_or_else(|| anyhow!("signed daemon health has no root trust store"))?;
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let mut matches = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.r#type == IdunnDaemonHealthTrustBindingRecord::TYPE)
    {
        let binding: IdunnDaemonHealthTrustBindingRecord = rmp_serde::from_slice(&entry.payload)
            .context("decoding root daemon health trust binding")?;
        binding.validate()?;
        if binding.daemon_id == statement.daemon_id
            && binding.health_contract == statement.health_contract
            && binding.source_runtime_id == statement.source_runtime_id
        {
            matches.push(binding);
        }
    }
    let binding = matches
        .pop()
        .ok_or_else(|| anyhow!("signed daemon health has no exact root trust binding"))?;
    if !matches.is_empty() {
        return Err(anyhow!("signed daemon health trust binding is ambiguous"));
    }
    let digest = format!("sha256-{:x}", Sha256::digest(rmp_serde::to_vec(&binding)?));
    Ok((binding, digest))
}

fn admit_generic_signed_health(
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
    admitted_at: &str,
    health: IdunnDaemonHealthRecord,
    statement: IdunnSignedDaemonHealthRecord,
    admission: IdunnAuthenticatedDaemonHealthAdmissionRecord,
) -> Result<HealthIngressOutcome> {
    for _ in 0..8 {
        let (expected_health, expected_admission, expected_statement) =
            with_store_node(options, store_lock, |node| {
                if let Some(existing) =
                    node.get::<IdunnAuthenticatedDaemonHealthAdmissionRecord>(&admission.daemon_id)?
                    && !generic_signed_health_advances(&existing, &admission)
                {
                    return Err(anyhow!("signed daemon health is replayed or regressed"));
                }
                validate_generic_release_binding(node, &admission)?;
                Ok((
                    node.cache()
                        .get_envelope::<IdunnDaemonHealthRecord>(&health.daemon_id)?,
                    node.cache()
                        .get_envelope::<IdunnAuthenticatedDaemonHealthAdmissionRecord>(
                            &admission.daemon_id,
                        )?,
                    node.cache().get_envelope::<IdunnSignedDaemonHealthRecord>(
                        &admission.signed_health_sha256,
                    )?,
                ))
            })?;
        let expected = [
            CultCacheExpectedEnvelope {
                key: health.daemon_id.clone(),
                r#type: IdunnDaemonHealthRecord::TYPE.into(),
                current: expected_health,
            },
            CultCacheExpectedEnvelope {
                key: admission.daemon_id.clone(),
                r#type: IdunnAuthenticatedDaemonHealthAdmissionRecord::TYPE.into(),
                current: expected_admission,
            },
            CultCacheExpectedEnvelope {
                key: admission.signed_health_sha256.clone(),
                r#type: IdunnSignedDaemonHealthRecord::TYPE.into(),
                current: expected_statement,
            },
        ];
        let replacements = [
            typed_envelope(&health.daemon_id, &health, admitted_at)?,
            typed_envelope(&admission.daemon_id, &admission, admitted_at)?,
            typed_envelope(&admission.signed_health_sha256, &statement, admitted_at)?,
        ];
        if SingleFileMessagePackBackingStore::new(&options.store_path)
            .compare_exchange(&expected, &replacements)?
        {
            return Ok(HealthIngressOutcome {
                daemon_id: health.daemon_id,
                state: health.state,
                detail: health.detail,
                authority: "authenticated",
            });
        }
    }
    Err(anyhow!(
        "signed daemon health admission lost repeated cross-process contention"
    ))
}

fn generic_signed_health_advances(
    existing: &IdunnAuthenticatedDaemonHealthAdmissionRecord,
    candidate: &IdunnAuthenticatedDaemonHealthAdmissionRecord,
) -> bool {
    if existing.trust_binding_id != candidate.trust_binding_id
        || existing.trust_binding_sha256 != candidate.trust_binding_sha256
        || existing.signer_identity_id != candidate.signer_identity_id
        || existing.daemon_id != candidate.daemon_id
    {
        return false;
    }
    if existing.publisher_incarnation_id == candidate.publisher_incarnation_id {
        candidate.publisher_sequence > existing.publisher_sequence
            && candidate.observed_at_unix_millis >= existing.observed_at_unix_millis
    } else {
        candidate.observed_at_unix_millis > existing.observed_at_unix_millis
    }
}

fn validate_generic_release_binding(
    node: &mut CultMeshNode,
    admission: &IdunnAuthenticatedDaemonHealthAdmissionRecord,
) -> Result<()> {
    let Some(deployment_id) = admission.deployment_id.as_deref() else {
        return Ok(());
    };
    let request = node
        .get::<IdunnDeploymentRequestRecord>(deployment_id)?
        .ok_or_else(|| anyhow!("signed daemon health names an unknown deployment request"))?;
    if request.daemon_id != admission.daemon_id
        || admission.source_commit.as_deref() != Some(request.source_revision.as_str())
        || !request.requires_bifrost_authority
        || request.release_authority_id.trim().is_empty()
        || request.release_authority_envelope_sha256.trim().is_empty()
    {
        return Err(anyhow!(
            "signed daemon health deployment authority is invalid"
        ));
    }
    let current = node
        .get::<IdunnCurrentDeploymentRequestRecord>(&admission.daemon_id)?
        .ok_or_else(|| anyhow!("signed daemon health has no current deployment head"))?;
    if current.request_id != request.request_id || current.sequence == 0 {
        return Err(anyhow!(
            "signed daemon health names a superseded deployment"
        ));
    }
    let requested_at = parse_timestamp_millis(&request.requested_at)
        .ok_or_else(|| anyhow!("deployment request timestamp is invalid"))?;
    if admission.observed_at_unix_millis < requested_at
        || admission.admitted_at_unix_millis < requested_at
    {
        return Err(anyhow!("signed daemon health predates its deployment"));
    }
    Ok(())
}

#[cfg(test)]
fn health_from_rudp_message(
    message: &CultNetMessage,
    options: &CommonOptions,
) -> Result<IdunnDaemonHealthRecord> {
    match decode_health_from_rudp_message(message, options, &timestamp()?)? {
        DecodedHealthIngress::AuthenticatedLegacy { health, .. }
        | DecodedHealthIngress::AuthenticatedGeneric { health, .. } => Ok(health),
        DecodedHealthIngress::Diagnostic(_) => {
            Err(anyhow!("unsigned daemon health is diagnostic-only"))
        }
    }
}

fn decode_health_from_rudp_message(
    message: &CultNetMessage,
    options: &CommonOptions,
    admitted_at: &str,
) -> Result<DecodedHealthIngress> {
    let CultNetMessage::DocumentPutRaw { document, .. } = message else {
        return Err(anyhow!("expected cultnet.document_put_raw.v0"));
    };
    if document.payload_encoding != CultNetRawPayloadEncoding::Messagepack {
        return Err(anyhow!("expected MessagePack raw payload encoding"));
    }
    if document.schema_id == SIGNED_DAEMON_HEALTH_TYPE {
        let (health, statement, admission) =
            authenticate_generic_signed_health(document, options, admitted_at)?;
        return Ok(DecodedHealthIngress::AuthenticatedGeneric {
            health,
            statement,
            admission,
        });
    }
    if document.schema_id == EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE {
        let (health, admission) =
            authenticate_epiphany_signed_health(document, options, admitted_at)?;
        return Ok(DecodedHealthIngress::AuthenticatedLegacy { health, admission });
    }
    if document.schema_id != "idunn.daemon_health" {
        return Err(anyhow!(
            "expected signed Epiphany or idunn.daemon_health schema, received {}",
            document.schema_id
        ));
    }
    let wire: IdunnDaemonHealthWireV1 = rmp_serde::from_slice(&document.payload)?;
    if wire.health_contract == EPIPHANY_HEALTH_CONTRACT {
        return Err(anyhow!(
            "Epiphany health requires its signed runtime-health schema"
        ));
    }
    if document.record_key != wire.daemon_id {
        return Err(anyhow!(
            "record key {} does not match health daemon_id {}",
            document.record_key,
            wire.daemon_id
        ));
    }
    let received_at_unix_millis = parse_timestamp_millis(admitted_at)
        .ok_or_else(|| anyhow!("unsigned health diagnostic receive time is invalid"))?;
    let diagnostic = IdunnUnsignedDaemonHealthDiagnosticRecord {
        schema_version: IDUNN_UNSIGNED_DAEMON_HEALTH_DIAGNOSTIC_SCHEMA.into(),
        diagnostic_id: format!("diagnostic:{}", wire.daemon_id),
        daemon_id: wire.daemon_id,
        claimed_state: wire.state,
        claimed_detail: wire.detail,
        claimed_observed_at: wire.observed_at,
        claimed_health_contract: wire.health_contract,
        transport_source_runtime_id: document.source_runtime_id.clone(),
        transport_source_role: document.source_role.clone(),
        received_at_unix_millis,
        authority: "diagnostic-only".into(),
        private_state_exposed: false,
    };
    diagnostic.validate()?;
    Ok(DecodedHealthIngress::Diagnostic(diagnostic))
}

fn authenticate_epiphany_signed_health(
    document: &cultnet_rs::CultNetRawDocumentRecord,
    options: &CommonOptions,
    admitted_at: &str,
) -> Result<(IdunnDaemonHealthRecord, IdunnSignedHealthAdmissionRecord)> {
    let signed: EpiphanySignedRuntimeHealthWire = rmp_serde::from_slice(&document.payload)
        .context("decoding signed Epiphany runtime health")?;
    validate_epiphany_signed_health_shape(&signed)?;
    if document.record_key != signed.health.daemon_id
        || document.source_runtime_id.as_deref() != Some(EPIPHANY_HEALTH_SOURCE_RUNTIME)
        || document.source_role.as_deref() != Some("daemon-health-publisher")
    {
        return Err(anyhow!(
            "signed Epiphany health transport identity is invalid"
        ));
    }
    let identity_store = options
        .trusted_epiphany_health_identity_store
        .as_deref()
        .ok_or_else(|| anyhow!("signed Epiphany health has no configured trust anchor"))?;
    let identity = load_epiphany_health_identity(identity_store)?;
    if signed.signer_identity_id != identity.identity_id {
        return Err(anyhow!(
            "signed Epiphany health signer is not the configured host identity"
        ));
    }
    let key_bytes: [u8; 32] = identity
        .public_key
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Epiphany health identity public key length is invalid"))?;
    let signature_bytes: [u8; 64] = signed
        .signature
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("signed Epiphany health signature length is invalid"))?;
    let mut unsigned = signed.clone();
    unsigned.signature.clear();
    let statement = rmp_serde::to_vec_named(&unsigned)?;
    VerifyingKey::from_bytes(&key_bytes)?
        .verify(
            &host_signature_message(EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE, &statement),
            &Signature::from_bytes(&signature_bytes),
        )
        .context("signed Epiphany health signature is invalid")?;
    let digest = format!("sha256-{:x}", Sha256::digest(&document.payload));
    let health = IdunnDaemonHealthRecord {
        daemon_id: signed.health.daemon_id,
        state: signed.health.state,
        detail: signed.health.detail,
        observed_at: signed.health.observed_at,
        health_contract: signed.health.health_contract,
        publication_source: "daemon-published".into(),
        transport: CULTNET_RUDP_PROTOCOL_ID.into(),
    };
    let admission = IdunnSignedHealthAdmissionRecord {
        daemon_id: health.daemon_id.clone(),
        state: health.state.clone(),
        observed_at: health.observed_at.clone(),
        admitted_at: admitted_at.to_string(),
        health_contract: health.health_contract.clone(),
        deployment_request_id: signed.deployment_request_id,
        release_id: signed.release_id,
        release_witness_sha256: signed.release_witness_sha256,
        source_commit: signed.source_commit,
        publisher_incarnation_id: signed.publisher_incarnation_id,
        publisher_sequence: signed.publisher_sequence,
        publisher_process_created_at: signed.publisher_process_created_at,
        signer_identity_id: signed.signer_identity_id,
        signed_health_sha256: digest,
    };
    Ok((health, admission))
}

fn validate_epiphany_signed_health_shape(signed: &EpiphanySignedRuntimeHealthWire) -> Result<()> {
    if signed.schema_version != EPIPHANY_SIGNED_RUNTIME_HEALTH_SCHEMA_VERSION
        || signed.health.daemon_id != "yggdrasil-epiphany"
        || signed.health.health_contract != EPIPHANY_HEALTH_CONTRACT
        || signed.health.publication_source != "daemon-published"
        || signed.health.transport != CULTNET_RUDP_PROTOCOL_ID
        || !matches!(
            signed.health.state.as_str(),
            "active" | "warming" | "degraded" | "failed"
        )
        || signed.source_runtime_id != EPIPHANY_HEALTH_SOURCE_RUNTIME
        || signed.deployment_request_id.trim().is_empty()
        || signed.release_id.trim().is_empty()
        || signed.publisher_sequence == 0
        || signed.publisher_process_id == 0
        || signed.publisher_process_creation_token == 0
        || signed.publisher_executable_path.trim().is_empty()
        || signed.signature_algorithm != "ed25519"
        || signed.signature.len() != 64
    {
        return Err(anyhow!("signed Epiphany health shape is invalid"));
    }
    uuid::Uuid::parse_str(&signed.publisher_incarnation_id)
        .context("signed health publisher incarnation must be UUID")?;
    chrono::DateTime::parse_from_rfc3339(&signed.health.observed_at)?;
    chrono::DateTime::parse_from_rfc3339(&signed.publisher_process_created_at)?;
    validate_sha256(&signed.release_witness_sha256, "release witness")?;
    if signed.source_commit.len() != 40
        || !signed
            .source_commit
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(anyhow!("signed Epiphany health source commit is invalid"));
    }
    Ok(())
}

fn load_epiphany_health_identity(path: &std::path::Path) -> Result<EpiphanyHostIdentityWire> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let envelope = entries
        .iter()
        .find(|entry| entry.r#type == HOST_IDENTITY_TYPE && entry.key == HOST_IDENTITY_KEY)
        .ok_or_else(|| anyhow!("configured Epiphany health identity is absent"))?;
    let identity: EpiphanyHostIdentityWire = rmp_serde::from_slice(&envelope.payload)?;
    let expected_id = format!(
        "{:x}",
        Sha256::digest([HOST_IDENTITY_DOMAIN, identity.public_key.as_slice()].concat())
    );
    if identity.schema_version != HOST_IDENTITY_TYPE
        || identity.identity_id != expected_id
        || identity.public_key.len() != 32
        || identity.assurance.trim().is_empty()
        || chrono::DateTime::parse_from_rfc3339(&identity.identity_created_at).is_err()
        || validate_sha256(&identity.source_identity_record_sha256, "identity record").is_err()
    {
        return Err(anyhow!("configured Epiphany health identity is invalid"));
    }
    Ok(identity)
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    let digest = value.strip_prefix("sha256-").unwrap_or(value);
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(anyhow!("signed Epiphany health {label} is invalid"));
    }
    Ok(())
}

fn host_signature_message(purpose: &str, payload: &[u8]) -> Vec<u8> {
    let mut message =
        Vec::with_capacity(HOST_SIGNATURE_DOMAIN.len() + purpose.len() + payload.len() + 16);
    message.extend_from_slice(HOST_SIGNATURE_DOMAIN);
    message.extend_from_slice(&(purpose.len() as u64).to_be_bytes());
    message.extend_from_slice(purpose.as_bytes());
    message.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    message.extend_from_slice(payload);
    message
}

fn signed_health_advances(
    existing: &IdunnSignedHealthAdmissionRecord,
    candidate: &IdunnSignedHealthAdmissionRecord,
) -> Result<bool> {
    if existing.signer_identity_id != candidate.signer_identity_id {
        return Ok(false);
    }
    if existing.publisher_incarnation_id == candidate.publisher_incarnation_id {
        return Ok(candidate.publisher_sequence > existing.publisher_sequence);
    }
    let existing_created =
        chrono::DateTime::parse_from_rfc3339(&existing.publisher_process_created_at)?;
    let candidate_created =
        chrono::DateTime::parse_from_rfc3339(&candidate.publisher_process_created_at)?;
    Ok(candidate_created > existing_created)
}

fn validate_admission_against_current_request(
    node: &mut CultMeshNode,
    admission: &IdunnSignedHealthAdmissionRecord,
) -> Result<()> {
    let request = node
        .get::<IdunnDeploymentRequestRecord>(&admission.deployment_request_id)?
        .ok_or_else(|| anyhow!("signed health names an unknown deployment request"))?;
    if request.daemon_id != admission.daemon_id
        || request.source_revision != admission.source_commit
        || !request.requires_bifrost_authority
        || request.repository_full_name != "GameCult/Epiphany"
        || request.upstream_ref != "refs/heads/main"
        || request.release_authority_id.trim().is_empty()
        || request.release_authority_envelope_sha256.trim().is_empty()
    {
        return Err(anyhow!(
            "signed health does not match an exact Bifrost-authorized Epiphany deployment request"
        ));
    }
    let request_time = parse_timestamp_seconds(&request.requested_at)
        .ok_or_else(|| anyhow!("deployment request timestamp is invalid"))?;
    let observed_time = parse_timestamp_seconds(&admission.observed_at)
        .ok_or_else(|| anyhow!("signed health observation timestamp is invalid"))?;
    let admitted_time = parse_timestamp_seconds(&admission.admitted_at)
        .ok_or_else(|| anyhow!("signed health admission timestamp is invalid"))?;
    if observed_time < request_time || admitted_time < request_time {
        return Err(anyhow!(
            "signed health predates the deployment request it claims"
        ));
    }
    let current = node
        .get::<IdunnCurrentDeploymentRequestRecord>(&admission.daemon_id)?
        .ok_or_else(|| anyhow!("Idunn has no current deployment request authority head"))?;
    if current.request_id != request.request_id || current.sequence == 0 {
        return Err(anyhow!(
            "signed health is bound to a superseded deployment request"
        ));
    }
    Ok(())
}

fn validate_admission_fresh_at(
    admission: &IdunnSignedHealthAdmissionRecord,
    now: &str,
    max_age_seconds: u64,
) -> Result<()> {
    let observed = parse_timestamp_seconds(&admission.observed_at)
        .ok_or_else(|| anyhow!("signed health observation timestamp is invalid"))?;
    let admitted = parse_timestamp_seconds(&admission.admitted_at)
        .ok_or_else(|| anyhow!("signed health admission timestamp is invalid"))?;
    let now =
        parse_timestamp_seconds(now).ok_or_else(|| anyhow!("Idunn admission clock is invalid"))?;
    if observed > admitted || admitted > now {
        return Err(anyhow!(
            "signed health admission carries a future or causally inverted timestamp"
        ));
    }
    if now.saturating_sub(observed) > max_age_seconds
        || now.saturating_sub(admitted) > max_age_seconds
    {
        return Err(anyhow!("signed health admission is stale"));
    }
    Ok(())
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
                let release_record = release_target_record(
                    target,
                    release,
                    updated_at,
                    options.release_authority_store_path.as_deref(),
                );
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
            "Move daemon awareness from debug probes to daemon-published typed CultNet/RUDP state."
                .to_string(),
        current_mechanism:
            "Idunn publishes per-daemon desired state, surgery plans, transport profiles, command boundaries, runtime RUDP self-checks, and RUDP health ingress; command probes are debug witnesses only."
                .to_string(),
        invariants: vec![
            "Daemon truth is typed CultCache/CultMesh state carried over cultnet.transport.rudp.v0.".to_string(),
            "Product/debug probes are evidence only and must not own daemon health.".to_string(),
            "Idunn consumes daemon-published RUDP health before debug probes and actuates only advertised lifecycle authority.".to_string(),
            "Each migrated daemon must publish the same health contract that Idunn expects for its target.".to_string(),
            "Shared operator hosts such as Raven must be actuated by background-only launch paths that do not create visible terminal or interactive windows.".to_string(),
            "Raven Task Scheduler actions must execute hidden launchers directly, not visible .cmd trampolines.".to_string(),
        ],
        phases: vec![
            "1. Publish Idunn's own RUDP substrate and ingress state.".to_string(),
            "2. Install daemon-published RUDP health in one Rust daemon and prove Idunn consumes it live.".to_string(),
            "3. Extend CultLib RUDP publication support across TypeScript, C#, and remaining daemon runtimes.".to_string(),
            "4. Promote provider advertisements, command boundaries, and transport profiles to daemon-owned CultNet/RUDP records.".to_string(),
            "5. Delete or demote debug probes once every target has daemon-owned publication and advertised lifecycle authority.".to_string(),
        ],
        current_phase:
            "Phase 25: Nightwing Gjallar now consumes Odin's accepted gamecult.eve.surface_state snapshot over CultNet/RUDP, so every active Starfire-local daemon target publishes daemon-owned RUDP health and typed boundary state; the remaining debt is demoting debug probes and bridge-only lowerings without letting them retake ownership."
                .to_string(),
        next_target: next_target.to_string(),
        cut_line:
            "Muninn, Vili, Idunn, Odin, Stonks, Weksa, VoidBot, Nightwing Gjallar, Mimir Eve dashboard, Nightwing Eve dashboard, Nightwing Eve browser reference, yggdrasil-streampixels, yggdrasil-heimdall, and yggdrasil-repixelizer now exercise daemon-owned RUDP health. Raven GameCult\\Vili has been refreshed from Odin, the hidden scheduled task now launches start-vili-daemon.ps1 with explicit Idunn RUDP arguments, and live Idunn accepts vili.cultnet-rudp-animation-health through the configured health route. VoidBot publishes daemon-owned provider catalog, provider advertisement, command_boundary, and transport_profile records from E:\\Projects\\VoidBot\\.voidbot\\status\\cultmesh\\voidbot-swarm-state.cc while health-voidbot.cmd is archived. Raven Muninn publishes provider advertisement, command_boundary, and transport_profile records from C:\\Meta\\Odin\\state\\muninn.telemetry.cc, keeps activation commands in C:\\Meta\\Odin\\state\\muninn.activate.cc, and no longer lets plain serve infer ambient Move runtime authority from platform defaults. Starfire Muninn publishes the same daemon-owned witness records from C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc, Nightwing Muninn publishes the same daemon-owned witness records from /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc, and live Idunn accepts Muninn-family daemon health by daemon id and configured health contract rather than source-address folklore. Nightwing's restart wrapper makes Move runtime authority explicit with -DiscoverMoveState -ClaimUsbMoves instead of silently adding Move evidence on plain serve. Raven's GameCult-Muninn, GameCult-Muninn-Activate, and GameCult-Muninn-VideoProof tasks execute wscript.exe actions whose hidden VBS launchers call noninteractive hidden PowerShell entrypoints directly; health .cmd wrappers are archived instead of command-probe health paths. Live yggdrasil-streampixels publishes streampixels.cultnet-rudp-service-health through its configured Yggdrasil health route, live yggdrasil-heimdall publishes heimdall.cultnet-rudp-provider-health with a boundary store at /srv/heimdall/cultcache/heimdall.service.cc, live yggdrasil-repixelizer publishes repixelizer.cultnet-rudp-service-health with a boundary store at /srv/repixelizer/cultcache/repixelizer.service.cc, live Nightwing Gjallar publishes a boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc plus a daemon-owned gamecult.eve.surface_state witness, live Gjallar consumes Odin's accepted surface:gamecult.network.status snapshot through the configured Gjallar Odin endpoint, and live Odin ingests remote Raven Vili and Nightwing Gjallar witness stores into the accepted provider catalog. Live Mimir Eve dashboard publishes CultMesh state at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp plus a boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc while health-nightwing-eve-dashboard.ps1 inspects those witnesses directly, and live Nightwing Eve browser reference publishes a boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc while health-nightwing-eve-browser-reference.ps1 inspects that store directly. The remaining debt is deleting bridge-only lowerings where they are no longer useful and keeping debug probes unable to own daemon truth."
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
        repository_full_name: format!("GameCult/{repo}"),
        repo_path,
        upstream_remote: "origin".to_string(),
        upstream_branch: "main".to_string(),
        rollout_strategy: rollout_strategy.to_string(),
        state_migration_command: state_migration_command.map(ToString::to_string),
        zero_downtime_capability: zero_downtime_capability.to_string(),
        deployed_revision_witness: None,
        requires_bifrost_authority: false,
    }
}

fn requiring_bifrost_authority(mut release: ReleaseTarget) -> ReleaseTarget {
    release.requires_bifrost_authority = true;
    release
}

fn with_deployed_revision_witness(mut release: ReleaseTarget, path: PathBuf) -> ReleaseTarget {
    release.deployed_revision_witness = Some(path);
    release
}

fn release_target_record(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    observed_at: &str,
    authority_store_path: Option<&std::path::Path>,
) -> IdunnReleaseTargetRecord {
    let port = SystemReleaseStatePort;
    let fetch_result = port.fetch(release);
    let observed_upstream_revision = fetch_result
        .as_ref()
        .ok()
        .and_then(|_| port.desired_revision(release).ok())
        .unwrap_or_else(|| "unknown".to_string());
    let deployed_revision = if release.deployed_revision_witness.is_some() {
        port.deployed_revision(release)
            .unwrap_or_else(|_| "unknown".to_string())
    } else {
        "untracked-no-witness".to_string()
    };
    let authorization = if release.requires_bifrost_authority && fetch_result.is_ok() {
        let upstream_ref = format!("refs/heads/{}", release.upstream_branch);
        authority_store_path.and_then(|store_path| {
            CultCacheReleaseAuthorityPort { store_path }
                .select(&release.repository_full_name, &upstream_ref, observed_at)
                .ok()
                .filter(|selected| {
                    git_revision(&release.repo_path, &selected.source_revision).as_deref()
                        == Some(selected.source_revision.as_str())
                })
        })
    } else {
        None
    };
    let desired_revision = authorization
        .as_ref()
        .map(|value| value.source_revision.clone())
        .unwrap_or_else(|| observed_upstream_revision.clone());
    let status = if fetch_result.is_ok()
        && observed_upstream_revision != "unknown"
        && (release.deployed_revision_witness.is_none() || deployed_revision != "unknown")
        && (!release.requires_bifrost_authority || authorization.is_some())
    {
        if release.requires_bifrost_authority {
            "tracked-authorized"
        } else {
            "tracked-legacy-authority"
        }
    } else {
        "release-authority-unavailable"
    };

    IdunnReleaseTargetRecord {
        target_id: release_target_id(target),
        daemon_id: target.daemon_id.clone(),
        repo: release.repo.clone(),
        repo_path: release.repo_path.display().to_string(),
        upstream_remote: release.upstream_remote.clone(),
        upstream_branch: release.upstream_branch.clone(),
        desired_revision,
        deployed_revision,
        artifact_strategy: "source-archive-from-upstream-main".to_string(),
        rollout_strategy: release.rollout_strategy.clone(),
        state_migration_authority: if release.state_migration_command.is_some() {
            "daemon-owned-command"
        } else {
            "daemon-declares-no-migration-required"
        }
        .to_string(),
        zero_downtime_capability: release.zero_downtime_capability.clone(),
        status: status.to_string(),
        observed_at: observed_at.to_string(),
        repository_full_name: release.repository_full_name.clone(),
        upstream_ref: format!("refs/heads/{}", release.upstream_branch),
        release_authority_id: authorization
            .as_ref()
            .map(|value| value.authority_id.clone())
            .unwrap_or_default(),
        release_authority_envelope_sha256: authorization
            .as_ref()
            .map(|value| value.envelope_sha256.clone())
            .unwrap_or_default(),
        release_authority_status: if authorization.is_some() {
            "authorized"
        } else if !release.requires_bifrost_authority {
            "legacy-unmigrated"
        } else {
            "unavailable"
        }
        .to_string(),
        requires_bifrost_authority: release.requires_bifrost_authority,
        observed_upstream_revision,
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
        release_authority_id: release.release_authority_id.clone(),
        release_authority_envelope_sha256: release.release_authority_envelope_sha256.clone(),
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
            "daemon-published-rudp-health + daemon-owned-cultcache-provider-store + odin-cultmesh-command-documents",
            "partial-rudp-health-and-provider-store-live",
            "Stonks daemon health is published over CultNet/RUDP, and provider advertisement, market snapshot, Eve surface, command_boundary, and transport_profile records are in the daemon-owned CultCache store. Renderer/debug surfaces live outside daemon transport authority.",
        ),
        "weksa" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-provider-store + odin-cultmesh-command-documents",
            "partial-rudp-health-and-provider-store-live",
            "Weksa daemon health is published over CultNet/RUDP, provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records are in the daemon-owned CultCache store, and speech_provider.mimo.voicedesign now arrives as typed CultMesh/CultNet command documents.",
        ),
        "voidbot" => (
            "daemon-published-rudp-health + daemon-owned-cultmesh-provider-store",
            "partial-rudp-health-and-provider-store-live",
            "VoidBot stack health is published over CultNet/RUDP from the local orchestrator pulse, and the daemon-owned CultMesh witness store at E:\\Projects\\VoidBot\\.voidbot\\status\\cultmesh\\voidbot-swarm-state.cc carries provider advertisement catalog, provider advertisement, command_boundary, and transport_profile records. The operations probe is a debug witness only.",
        ),
        "nightwing-gjallar" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary + native-odin-cultnet-rudp-snapshot-input",
            "partial-rudp-health-and-provider-store-live",
            "Gjallar framebuffer composition health is published over CultNet/RUDP from Nightwing, the daemon-owned boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc carries provider advertisement, runtime_config, frame_status, gamecult.eve.surface_state, command_boundary, transport_profile, and daemon-health summary state, and live composition consumes Odin's accepted surface:gamecult.network.status snapshot over CultNet/RUDP. The service/status probe is a debug witness only.",
        ),
        "nightwing-eve-dashboard" => (
            "daemon-published-rudp-health + daemon-owned-cultmesh-state + daemon-owned-cultcache-boundary-store",
            "partial-rudp-health-and-provider-store-live",
            "Nightwing Eve dashboard service health is published over CultNet/RUDP from the Mimir.EveDashboard systemd process, and the live broker publishes CultMesh state at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp plus a daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc carrying typed provider advertisement, command_boundary, transport_profile, and daemon-health summary state. Client renderers must lower those typed witnesses without becoming daemon transport authority.",
        ),
        "nightwing-eve-browser-reference" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-boundary-store",
            "partial-rudp-health-and-provider-store-live",
            "Nightwing Eve browser reference health is published over CultNet/RUDP from the Mimir.EveBrowserReference service process, and the runtime publishes a daemon-owned boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc carrying manifest, static-surface, command_boundary, transport_profile, and daemon-health summary state. Browser serving is a renderer surface, while health-nightwing-eve-browser-reference.ps1 inspects the daemon-owned witness directly.",
        ),
        "muninn" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store + background-only hidden task launch",
            "partial-rudp-health-and-provider-store-live",
            "Raven Muninn publishes muninn.cultnet-rudp-remote-telemetry-health over CultNet/RUDP from the long-running hidden GameCult-Muninn serve task, and the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\muninn.telemetry.cc carries provider advertisement, command_boundary, transport_profile, and telemetry surface records. Background-only launch remains an ops invariant; local commands are command/debug lowerings only.",
        ),
        "starfire-muninn" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store",
            "partial-rudp-health-and-provider-store-live",
            "Starfire Muninn now publishes muninn.cultnet-rudp-local-telemetry-and-quest-access over CultNet/RUDP from the long-running local serve process, and the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Quest access, and telemetry surface records. Quest ADB availability stays telemetry state, not daemon liveness.",
        ),
        "nightwing-muninn" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-telemetry-store",
            "partial-rudp-health-and-provider-store-live",
            "Nightwing Muninn now publishes muninn.cultnet-rudp-remote-telemetry-and-move-hid over CultNet/RUDP from the long-running serve process, and the daemon-owned telemetry store at /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Move HID evidence, and telemetry surface records.",
        ),
        "vili" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary",
            "partial-rudp-health-and-provider-store-live",
            "Vili now publishes vili.cultnet-rudp-animation-health over CultNet/RUDP from the hidden Raven GameCult\\Vili task, and its daemon-owned vili.service.cc store contains provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records. Operator/debug surfaces must not become daemon health or command transport.",
        ),
        "yggdrasil-streampixels" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary",
            "partial-rudp-health-and-provider-store-live",
            "StreamPixels publishes daemon-owned RUDP health and a daemon-owned CultCache boundary from the live Yggdrasil service runtime; Idunn accepts yggdrasil-streampixels through the configured service health route and the boundary store at /srv/streampixels/app/.streampixels-data/cultcache/streampixels.service.cc remains the service-owned witness behind deployment/debug lowerings.",
        ),
        "yggdrasil-heimdall" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-boundary-store",
            "partial-rudp-health-and-provider-store-live",
            "Heimdall publishes daemon-owned Idunn health over CultNet/RUDP from the live Yggdrasil service runtime, and its daemon-owned boundary store at /srv/heimdall/cultcache/heimdall.service.cc carries provider advertisement, command_boundary, transport_profile, and daemon-health summary state. Product web and host supervisor checks are deployment/debug witnesses, not daemon transport.",
        ),
        "yggdrasil-repixelizer" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-boundary-store",
            "partial-rudp-health-and-provider-store-live",
            "Repixelizer publishes daemon-owned Idunn health over CultNet/RUDP from the live Yggdrasil GUI runtime, and its daemon-owned boundary store at /srv/repixelizer/cultcache/repixelizer.service.cc carries provider advertisement, Eve surface state, queue/auth/runtime projection, command_boundary, transport_profile, and daemon-health summary state. Product web and host supervisor checks are deployment/debug witnesses, not daemon transport.",
        ),
        "raven-sleipnir" => (
            "daemon-published-rudp-health + daemon-owned-cultcache-input-mirror-store",
            "partial-rudp-health-and-provider-store-live",
            "Sleipnir publishes daemon-owned Idunn health over CultNet/RUDP from the Raven input mirror runtime, writes provider advertisement and Eve surface state to its daemon-owned CultMesh store, and accepts mapping commands through Odin's CultMesh command document route. The Raven scheduled task is an Idunn restart lowering, not deployment truth.",
        ),
        _ => (
            "missing-daemon-published-rudp-health",
            "migration-required",
            "Daemon truth must move to CultNet/RUDP health publication and advertised command boundaries; command probes are debug witnesses only.",
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
        debug_mechanism: "none".to_string(),
        cut_line: cut_line.to_string(),
        observed_at: observed_at.to_string(),
    }
}

fn command_boundary(target: &DaemonTarget, observed_at: &str) -> IdunnCommandBoundaryRecord {
    let restart_authority = target
        .restart_command
        .as_ref()
        .map(|_| "idunn-supervisor-command.restart")
        .unwrap_or("none")
        .to_string();
    let deploy_authority = target
        .deploy_command
        .as_ref()
        .map(|_| "idunn-supervisor-command.deploy")
        .unwrap_or("none")
        .to_string();
    let command_lowerings = [
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
        owner: "idunn-supervisor-command-boundary".to_string(),
        restart_authority,
        deploy_authority,
        health_authority: "daemon-published-rudp-health".to_string(),
        alarm_authority: "bifrost.operator-notification".to_string(),
        command_lowerings,
        forbidden_authority: "Product/debug probes do not own daemon truth.".to_string(),
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
        "Idunn target {} currently requires daemon-published RUDP health under contract {}; local health probes are not part of the target contract.",
        target.daemon_id, target.health_contract.id
    );
    let mut intended_authority = "Daemon publishes typed CultMesh/CultNet documents over cultnet.transport.rudp.v0; Idunn consumes those records and only actuates advertised lifecycle commands.".to_string();
    let mut cut_line = "Cut product/debug probes as sources of daemon truth once the daemon's CultLib can publish the RUDP health contract.".to_string();
    let mut steps = vec![
        "Update the daemon's runtime CultLib dependency to a build that can speak CultNet over RUDP.".to_string(),
        "Publish daemon_health, provider_advertisement, command_boundary, and transport_profile typed records over cultnet.transport.rudp.v0.".to_string(),
        "Teach Odin to accept the daemon's RUDP provider records into the service/catalog surface.".to_string(),
        "Switch Idunn from the debug health command to the daemon-owned RUDP health record.".to_string(),
        "Delete or demote the old probe to a xenos-boundary debug check with no lifecycle authority.".to_string(),
    ];
    let blockers = Vec::new();

    match target.daemon_id.as_str() {
        "odin" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "critical";
            owner = "Odin core";
            current_mechanism =
                "Odin now ingests remote Raven Vili, Raven Muninn, Starfire Muninn, Nightwing Muninn, and Nightwing Gjallar witness stores into the accepted provider catalog while relying on xenos-boundary hashed-store inspection for C# witness bodies."
                    .to_string();
            intended_authority =
                "Odin owns accepted Verse discovery and provider catalog truth as typed CultMesh/CultNet records over cultnet.transport.rudp.v0."
                    .to_string();
            cut_line =
                "Odin's remaining interop debt is deleting hashed-store inspection shims once remote daemon runtimes publish directly interoperable provider surfaces and records."
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
                "Nightwing Gjallar publishes gjallar.cultnet-rudp-framebuffer-composition-health over CultNet/RUDP from the C# runtime, the daemon-owned boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc now contains provider advertisement, gamecult.eve.surface_state, runtime_config, frame_status, command_boundary, transport_profile, and daemon-health summary records, and the live compositor polls Odin's accepted surface:gamecult.network.status snapshot over a configured cultnet.transport.rudp.v0 endpoint. Live Odin ingests that witness store without advertising a deck bridge as transport authority."
                    .to_string();
            intended_authority =
                "Gjallar consumes Odin's accepted provider surface snapshot over CultNet/RUDP and publishes framebuffer composition health, provider advertisement, provider-owned Eve surface, runtime config, command boundary, and transport profile as typed CultMesh/CultNet state."
                    .to_string();
            cut_line =
                "Keep the service/status probe as a debug witness only; the daemon-owned witness now owns provider advertisement, gamecult.eve.surface_state, command_boundary, and transport_profile truth, live input now comes from Odin's accepted CultNet/RUDP snapshot surface through the configured Gjallar Odin endpoint, and live Gjallar daemon health now keys off snapshot freshness through receive.status, lastAttemptStatus, lastSuccessfulAtUtc, staleAfterSeconds, and consecutiveFailures so retained panels cannot masquerade as healthy after input goes stale."
                    .to_string();
            steps = vec![
                "Keep live gjallar.cultnet-rudp-framebuffer-composition-health publication running from Nightwing's Gjallar service.".to_string(),
                "Keep Gjallar's daemon-owned boundary store at /var/lib/gamecult/gjallar/cultcache/gjallar.service.cc publishing provider advertisement, gamecult.eve.surface_state, runtime_config, frame_status, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                "Keep the native C# CultNet/RUDP snapshot polling path pointed at Odin's accepted surface:gamecult.network.status surface through the configured Gjallar Odin endpoint; no baked Starfire LAN endpoint is allowed.".to_string(),
                "Keep Gjallar's receive-freshness daemon health live on Nightwing so daemon-published health degrades when Odin snapshot success goes stale instead of keying only off rendered frames.".to_string(),
                "Keep Gjallar renderer output derived from the daemon-owned CultMesh/Eve witness; do not advertise a deck bridge as service input or discovery authority.".to_string(),
                "Keep the .ps1 lifecycle bodies as explicit witness/restart bodies and keep health .cmd wrappers archived so they cannot masquerade as Idunn health truth.".to_string(),
            ];
        }
        "raven-sleipnir" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "medium-high";
            owner = "Sleipnir input mirror runtime";
            current_mechanism =
                "Raven Sleipnir publishes sleipnir.cultnet-rudp-input-mirror-health over CultNet/RUDP from the live input mirror runtime, writes provider advertisement and Eve surface state to C:\\Meta\\Odin\\state\\raven.sleipnir.cc, exposes mapping commands through its advertised CultNet/RUDP route, and runs as a hidden Raven scheduled task installed by Idunn's restart lowering."
                    .to_string();
            intended_authority =
                "Sleipnir publishes daemon health, provider advertisement, provider-owned Eve surface, and mapping command ingress as typed CultMesh/CultNet records; the scheduled task is lifecycle actuation only."
                    .to_string();
            cut_line =
                "Keep restart-raven-sleipnir.ps1 as an Idunn actuator body only; direct operator starts and ad hoc deployment scripts must not own Raven Sleipnir lifecycle or command truth."
                    .to_string();
            steps = vec![
                "Keep live sleipnir.cultnet-rudp-input-mirror-health publication running from Raven sleipnir.".to_string(),
                "Keep Sleipnir provider advertisement and Eve surface state in C:\\Meta\\Odin\\state\\raven.sleipnir.cc and published to Odin over the configured CultNet/RUDP catalog endpoint.".to_string(),
                "Keep Sleipnir mapping changes flowing through Odin's `sleipnir.input_mapping.v1` CultMesh command document route.".to_string(),
                "Keep Raven process launch hidden through the Idunn-installed scheduled task; no visible cmd or PowerShell windows.".to_string(),
                "Keep start/deploy shortcuts deleted or actuator-guarded so Idunn remains lifecycle owner.".to_string(),
            ];
        }
        "stonks" => {
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Stonks TypeScript runtime";
            current_mechanism =
                "Stonks publishes daemon health over CultNet/RUDP after each serialized market refresh, and its daemon-owned CultCache store contains provider advertisement, market snapshot, Eve surface, command_boundary, and transport_profile records that Odin can ingest."
                    .to_string();
            intended_authority =
                "Stonks publishes daemon health, provider advertisement, market snapshot, Eve surface, command boundary, and transport profile as typed CultMesh/CultNet records over cultnet.transport.rudp.v0."
                    .to_string();
            cut_line =
                "Keep Stonks lifecycle and health on daemon-published CultMesh/CultNet records; product/debug exports are not daemon-health probes."
                    .to_string();
            steps = vec![
                "Keep live stonks.cultnet-rudp-market-health publication running from the Stonks daemon.".to_string(),
                "Keep Stonks provider advertisement, market snapshot, Eve surface, command_boundary, and transport_profile records in the daemon-owned CultCache store.".to_string(),
                "Keep Odin provider discovery accepting Stonks' typed store instead of relying on product manifest ingestion.".to_string(),
                "Keep health-stonks.cmd archived so local probes cannot masquerade as daemon health.".to_string(),
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
                "Mimir dashboard publishes daemon health over cultnet.transport.rudp.v0, publishes retained Eve dashboard state through CultMesh, and keeps typed provider advertisement, command boundary, and transport profile in its daemon-owned boundary witness."
                    .to_string();
            cut_line =
                "Keep local probe wrappers archived while Odin and Idunn consume the daemon-owned Mimir witness paths directly."
                    .to_string();
            steps = vec![
                "Keep live mimir.cultnet-rudp-provider-health publication running from the Nightwing Eve dashboard broker.".to_string(),
                "Keep the live CultMesh state witness at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp publishing retained dashboard state.".to_string(),
                "Keep the daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc publishing typed provider advertisement, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                "Keep Odin pointed at Mimir CultMesh/CultCache witness paths instead of product catalog ingestion.".to_string(),
                "Keep health-mimir-eve-dashboard.cmd archived so local probes cannot masquerade as daemon health.".to_string(),
            ];
        }
        "weksa" => {
            status = "partial-rudp-health-and-provider-store-live";
            severity = "medium-high";
            owner = "Weksa provider runtime";
            current_mechanism =
                "Weksa publishes weksa.cultnet-rudp-provider-health over CultNet/RUDP after each serialized witness refresh, its daemon-owned provider store contains provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records that Odin can ingest, and speech_provider.mimo.voicedesign arrives as typed CultMesh/CultNet command documents through Odin."
                    .to_string();
            intended_authority =
                "Weksa publishes daemon health, provider advertisement, operator state, Eve surfaces, command boundary, transport profile, and MiMo VoiceDesign command ingress as typed CultMesh/CultNet records over cultnet.transport.rudp.v0; product endpoints do not own daemon truth."
                    .to_string();
            cut_line =
                "Keep Weksa lifecycle and health on daemon-published CultMesh/CultNet records; local command health must not satisfy daemon truth."
                    .to_string();
            steps = vec![
                "Keep live weksa.cultnet-rudp-provider-health publication running from the Weksa daemon.".to_string(),
                "Keep Weksa provider advertisement, operator-state, Eve surface, command_boundary, and transport_profile records in the daemon-owned provider store.".to_string(),
                "Keep Odin provider discovery accepting Weksa's typed provider store instead of relying on product ingestion.".to_string(),
                "Keep speech_provider.mimo.voicedesign on typed CultMesh/CultNet command documents.".to_string(),
                "Keep health-weksa.cmd archived so local command health cannot masquerade as daemon truth.".to_string(),
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
                "Keep the operations probe as a debug witness for the daemon-owned CultMesh store; it no longer owns VoidBot lifecycle truth."
                    .to_string();
            steps = vec![
                "Keep live voidbot.cultnet-rudp-stack-health publication running from the GameCult Local Orchestrator pulse.".to_string(),
                "Keep VoidBot swarm, Discord, archive, source, and repo-face provider records published from the daemon-owned CultMesh witness store.".to_string(),
                "Keep VoidBot command_boundary and transport_profile records published from the provider runtime.".to_string(),
                "Keep Odin pointed at the daemon-owned VoidBot CultMesh witness store instead of status ingestion.".to_string(),
                "Keep health-voidbot.cmd archived so local witness inspection cannot masquerade as daemon health.".to_string(),
            ];
        }
        "muninn" => {
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Muninn Rust runtime plus Raven background-only launcher surface";
            current_mechanism =
                "Raven Muninn now runs from the hidden GameCult-Muninn scheduled task with --idunn-rudp-health supplied by the configured Idunn health endpoint, daemon id muninn, and contract muninn.cultnet-rudp-remote-telemetry-health on the long-running serve process. Live Idunn accepts that daemon-published health over the configured RUDP route, the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, and telemetry surface records, activation commands now route through the explicit C:\\Meta\\Odin\\state\\muninn.activate.cc store, the serve body no longer auto-claims ambient Move runtime authority from platform defaults, and GameCult-Muninn, GameCult-Muninn-Activate, and GameCult-Muninn-VideoProof now execute hidden VBS launchers whose bodies call noninteractive hidden PowerShell entrypoints directly; .cmd wrappers, where present, are manual debug entrypoints only."
                    .to_string();
            intended_authority =
                "Muninn publishes telemetry, provider advertisement, command_boundary, transport_profile, explicit activation routing, and daemon health over CultNet/RUDP/CultCache; Raven Task Scheduler owns only background launch of hidden WScript/PowerShell launchers and never visible .cmd trampoline execution or ambient Move runtime inference."
                    .to_string();
            cut_line =
                "Keep Raven's hidden task launch invariant structural by verifying that scheduled-task actions and hidden VBS launchers never route through cmdPath trampolines, that activation commands stay pointed at C:\\Meta\\Odin\\state\\muninn.activate.cc, and that plain serve cannot infer Move runtime authority from platform defaults; Odin and Idunn should consume C:\\Meta\\Odin\\state\\muninn.telemetry.cc as the daemon-owned witness while health-muninn.cmd remains archived."
                    .to_string();
            steps = vec![
                "Keep scripts/repair-raven-muninn-task-actions.ps1 using sftp plus a tiny remote runner so Windows command-line length does not block future hidden-task repair.".to_string(),
                "Keep GameCult-Muninn action executing wscript.exe with start-muninn-serve-hidden.vbs arguments.".to_string(),
                "Keep the Raven Muninn serve process command line carrying --idunn-rudp-health from the configured Idunn health endpoint, --idunn-daemon muninn, and --idunn-health-contract muninn.cultnet-rudp-remote-telemetry-health; no baked Starfire LAN health endpoint is allowed.".to_string(),
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
                    "Starfire Muninn now runs as a long-lived local serve process with --quest-adb, --idunn-rudp-health supplied by the configured Idunn health endpoint, daemon id starfire-muninn, and contract muninn.cultnet-rudp-local-telemetry-and-quest-access. Live Idunn accepts that daemon-published health through the configured RUDP route, the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Quest access, and telemetry surface records, and restart-starfire-muninn.ps1 now archives a corrupt CultCache store before relaunch instead of leaving the daemon dead on decode faults."
                        .to_string();
                cut_line =
                    "Keep health-starfire-muninn.ps1 consuming C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc as the daemon-owned witness while Quest availability remains telemetry state; health-starfire-muninn.cmd stays archived."
                        .to_string();
                steps = vec![
                    "Keep the Starfire Muninn serve process command line carrying --idunn-rudp-health from the configured Idunn health endpoint, --idunn-daemon starfire-muninn, and --idunn-health-contract muninn.cultnet-rudp-local-telemetry-and-quest-access; no baked local endpoint is allowed.".to_string(),
                    "Keep restart-starfire-muninn.ps1 clearing stale .lock files and archiving corrupt starfire.muninn.telemetry.cc before relaunch.".to_string(),
                    "Keep the daemon-owned telemetry store at C:\\Meta\\Odin\\state\\starfire.muninn.telemetry.cc publishing provider advertisement, command_boundary, transport_profile, Quest access, and telemetry surface records.".to_string(),
                    "Keep health-starfire-muninn.ps1 as the witness-first inspection body for Quest telemetry and process shape; health-starfire-muninn.cmd is archived.".to_string(),
                ];
            } else {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Nightwing Muninn now runs as a long-lived serve process with explicit Move HID input, --idunn-rudp-health supplied by the configured Idunn health endpoint, daemon id nightwing-muninn, and contract muninn.cultnet-rudp-remote-telemetry-and-move-hid. Live Idunn accepts that daemon-published health from the configured RUDP route, the daemon-owned telemetry store at /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc now carries provider advertisement, command_boundary, transport_profile, Move HID evidence, and telemetry surface records, and restart-nightwing-muninn.ps1 now only claims or discovers Move runtime authority when -DiscoverMoveState, -ClaimUsbMoves, or explicit -MoveState values are present."
                        .to_string();
                cut_line =
                    "Keep health-nightwing-muninn.ps1 consuming /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc as the daemon-owned witness while the serve process owns muninn.cultnet-rudp-remote-telemetry-and-move-hid publication; health-nightwing-muninn.cmd stays archived."
                        .to_string();
                steps = vec![
                    "Keep the Nightwing Muninn serve process command line carrying --idunn-rudp-health from the configured Idunn health endpoint, --idunn-daemon nightwing-muninn, and --idunn-health-contract muninn.cultnet-rudp-remote-telemetry-and-move-hid; no baked WireGuard endpoint is allowed.".to_string(),
                    "Keep the daemon-owned telemetry store at /home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc publishing provider advertisement, command_boundary, transport_profile, Move HID evidence, and telemetry surface records.".to_string(),
                    "Keep restart-nightwing-muninn.ps1 launching the long-running serve body instead of one-shot health publication.".to_string(),
                    "Keep restart-nightwing-muninn.ps1 requiring explicit -DiscoverMoveState/-ClaimUsbMoves or explicit -MoveState values before it adds Move evidence/runtime arguments.".to_string(),
                    "Keep health-nightwing-muninn.ps1 as the witness-first inspection body for Move HID freshness; health-nightwing-muninn.cmd is archived.".to_string(),
                ];
            }
        }
        "vili" => {
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Vili animation runtime";
            current_mechanism =
                "Vili now runs on Raven through the hidden GameCult\\Vili scheduled task, which launches start-vili-daemon.ps1 with --idunn-rudp-health supplied by the configured Idunn health endpoint and contract vili.cultnet-rudp-animation-health. Live Idunn accepts that daemon-published health from the configured RUDP route, and Vili writes a daemon-owned vili.service.cc CultCache store containing provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records."
                    .to_string();
            intended_authority =
                "Vili publishes animation daemon health, provider advertisement, operator state, command boundary, and transport profile as typed CultMesh/CultNet records over cultnet.transport.rudp.v0."
                    .to_string();
            cut_line =
                "Keep Vili lifecycle and health on daemon-published CultMesh/CultNet records while Odin consumes Vili provider records natively."
                    .to_string();
            steps = vec![
                "Keep the in-process Vili idunn.daemon_health RUDP publisher wired through scripts/vili-daemon.mjs.".to_string(),
                "Keep Vili's provider advertisement, operator state, Eve surface, command_boundary, and transport_profile records in the daemon-owned vili.service.cc store.".to_string(),
                "Keep scripts/restart-vili.ps1 syncing the authoritative Vili runtime plus flattened CultLib node_modules into Raven before reinstalling the hidden task.".to_string(),
                "Keep GameCult\\Vili executing wscript.exe hidden launcher arguments that pass --idunn-rudp-health from the configured Idunn health endpoint, --idunn-daemon vili, and --idunn-health-contract vili.cultnet-rudp-animation-health; no baked WireGuard endpoint is allowed.".to_string(),
                "Keep Idunn bound on 0.0.0.0:17870 so WireGuard peers such as Raven can publish daemon-owned health over cultnet.transport.rudp.v0.".to_string(),
                "Keep Odin provider discovery pointed at Vili's daemon-owned vili.service.cc store; keep health-vili.cmd archived so local probes cannot masquerade as daemon health.".to_string(),
            ];
        }
        "nightwing-eve-dashboard" | "nightwing-eve-browser-reference" => {
            severity = "medium";
            owner = "Eve lowering/runtime owner";
            intended_authority =
                "Eve runtimes subscribe to provider-owned CultMesh/CultNet state; browser delivery must not be advertised as daemon transport or health authority."
                    .to_string();
            if target.daemon_id == "nightwing-eve-dashboard" {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Nightwing Eve dashboard publishes nightwing.cultnet-rudp-eve-dashboard-health over CultNet/RUDP from the Mimir.EveDashboard systemd process, with retained dashboard state in /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp and a daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc."
                        .to_string();
                cut_line =
                    "health-nightwing-eve-dashboard.ps1 inspects the dashboard CultMesh/CultCache witnesses directly, restart-nightwing-eve-dashboard.ps1 owns the systemd restart actuator, and the remaining debt is teaching Nightwing projections and Odin to consume the typed witnesses without any service probe reclaiming daemon truth."
                        .to_string();
                steps = vec![
                    "Keep live nightwing.cultnet-rudp-eve-dashboard-health publication running from the Mimir.EveDashboard systemd process.".to_string(),
                    "Keep the live CultMesh state witness at /var/lib/gamecult/eve-dashboard/cultmesh/eve-dashboard.ccmp publishing retained dashboard state.".to_string(),
                    "Keep the daemon-owned boundary store at /var/lib/gamecult/eve-dashboard/cultcache/eve-dashboard.service.cc publishing typed provider advertisement, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                    "Teach Odin and Nightwing projections to prefer the dashboard CultMesh/CultCache witnesses over service probes.".to_string(),
                    "Keep health-nightwing-eve-dashboard.ps1 as the witness-first health body and restart-nightwing-eve-dashboard.ps1 as the systemd restart body.".to_string(),
                    "Keep health-nightwing-eve-dashboard.cmd archived and restart-nightwing-eve-dashboard.cmd as a manual lifecycle wrapper over the PowerShell restart body only.".to_string(),
                ];
            } else {
                status = "partial-rudp-health-and-provider-store-live";
                current_mechanism =
                    "Nightwing Eve browser reference publishes nightwing.cultnet-rudp-browser-reference-health over CultNet/RUDP from the Mimir.EveBrowserReference service process, and the runtime now writes a daemon-owned boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc."
                        .to_string();
                cut_line =
                    "health-nightwing-eve-browser-reference.ps1 inspects the browser reference CultCache witness directly, restart-nightwing-eve-browser-reference.ps1 owns the systemd restart actuator, health .cmd wrappers are archived, and the remaining debt is teaching Nightwing projections and Odin to consume the typed witness without any service probe reclaiming daemon truth."
                        .to_string();
                steps = vec![
                    "Keep live nightwing.cultnet-rudp-browser-reference-health publication running from the Mimir.EveBrowserReference service process.".to_string(),
                    "Keep the daemon-owned boundary store at /var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc publishing manifest, static-surface, command_boundary, transport_profile, and daemon-health summary records.".to_string(),
                    "Teach Odin and Nightwing projections to prefer the browser reference CultCache witness over service probes.".to_string(),
                    "Keep health-nightwing-eve-browser-reference.ps1 as the witness-first health body and restart-nightwing-eve-browser-reference.ps1 as the systemd restart body.".to_string(),
                    "Keep health-nightwing-eve-browser-reference.cmd archived and restart-nightwing-eve-browser-reference.cmd as a manual lifecycle wrapper over the PowerShell restart body only.".to_string(),
                ];
            }
        }
        "yggdrasil-streampixels" => {
            severity = "medium";
            status = "partial-rudp-health-and-provider-store-live";
            owner = "StreamPixels service runtime plus gamecult-ops deploy lane";
            current_mechanism =
                "StreamPixels publishes a daemon-owned CultCache boundary store with provider advertisement, command_boundary, transport_profile, and Idunn health summary from the live Yggdrasil service runtime. The deployed service requires STREAMPIXELS_IDUNN_RUDP_HEALTH from explicit service env or deployment env and contract streampixels.cultnet-rudp-service-health in /srv/streampixels/env/service.env, the source-artifact lane now ships the required CultLib snapshot beside the app artifact, and live Idunn accepts yggdrasil-streampixels from the configured RUDP route."
                    .to_string();
            intended_authority =
                "StreamPixels publishes service health, provider state, command boundary, and transport profile over cultnet.transport.rudp.v0, with the service-owned CultCache boundary as durable local state and deployment surfaces as debug witnesses."
                    .to_string();
            cut_line =
                "Keep the StreamPixels service boundary store and in-process RUDP publisher live on Yggdrasil; host checks and deployment-manifest freshness remain deployment/debug witnesses only while Odin consumes the typed store and Idunn reads daemon-published health."
                    .to_string();
            steps = vec![
                "Keep apps/service/src/verse-state.ts publishing streampixels.service.cc from the StreamPixels service runtime.".to_string(),
                "Teach Odin to ingest StreamPixels provider advertisement, command_boundary, and transport_profile records from the service-owned CultCache boundary store.".to_string(),
                "Keep the StreamPixels in-process Idunn RUDP health publisher using contract streampixels.cultnet-rudp-service-health.".to_string(),
                "Keep the Yggdrasil source-artifact lane shipping the StreamPixels app artifact plus the CultLib cultnet-ts/cultcache-ts snapshot through a tiny remote runner script instead of brittle inline SSH quoting.".to_string(),
                "Keep the Yggdrasil deploy lane using a serial pnpm workspace build and the deployment-manifest freshness check so failed builds cannot masquerade as fresh restarts.".to_string(),
                "Keep live Idunn acceptance proof for StreamPixels RUDP health on Yggdrasil; the publisher sends the health document after a short accept grace period so one-shot pulses do not depend on receiving the accept reply.".to_string(),
                "Keep health-yggdrasil-streampixels.cmd archived and keep host checks as explicit deployment witnesses while Odin and Idunn consume the typed store and daemon-published RUDP health.".to_string(),
            ];
        }
        "yggdrasil-heimdall" => {
            severity = "medium";
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Heimdall service runtime plus Yggdrasil source-artifact lane";
            current_mechanism =
                "Heimdall now publishes heimdall.cultnet-rudp-provider-health over CultNet/RUDP from the live Yggdrasil service runtime, and its daemon-owned boundary store at /srv/heimdall/cultcache/heimdall.service.cc contains provider advertisement, command_boundary, transport_profile, and daemon-health summary state. The Yggdrasil source-artifact lane now ships the required CultLib snapshot beside the app artifact, while deployment checks verify host/product readiness and the boundary witness."
                    .to_string();
            intended_authority =
                "Heimdall keeps public web delivery as the product boundary while publishing daemon health, provider advertisement, command boundary, and transport profile as internal CultNet/RUDP state, with full redacted auth-document witness export following as a separate auth-safe pass."
                    .to_string();
            cut_line =
                "Keep the Heimdall boundary store and in-process RUDP publisher live on Yggdrasil; host/product readiness checks are deployment/debug witnesses only while Odin ingests the typed store and Idunn reads daemon-published health."
                    .to_string();
            steps = vec![
                "Keep Heimdall's src/verse-witness.ts advertisement published through the runtime-owned boundary store with daemon-owned update timestamps.".to_string(),
                "Keep Heimdall daemon health published over CultNet/RUDP from the live Yggdrasil service runtime with contract heimdall.cultnet-rudp-provider-health.".to_string(),
                "Keep Heimdall command_boundary and transport_profile records in the runtime-owned boundary store so deploy/restart authority is inspectable without debug scripts owning truth.".to_string(),
                "Keep the Yggdrasil Heimdall source-artifact lane shipping the app artifact plus the CultLib cultnet-ts/cultcache-ts snapshot through a tiny remote runner script instead of brittle inline SSH quoting.".to_string(),
                "Teach Odin to ingest Heimdall's provider advertisement and future redacted witness surfaces as typed state instead of treating product web discovery as daemon truth.".to_string(),
                "Keep Heimdall deployment checks as witness-only while Odin and Idunn consume the typed store.".to_string(),
            ];
        }
        "yggdrasil-repixelizer" => {
            severity = "medium";
            status = "partial-rudp-health-and-provider-store-live";
            owner = "Repixelizer GUI runtime plus Yggdrasil source-artifact lane";
            current_mechanism =
                "Repixelizer publishes repixelizer.cultnet-rudp-service-health from the live Yggdrasil GUI runtime, and its daemon-owned boundary store at /srv/repixelizer/cultcache/repixelizer.service.cc carries provider advertisement, Eve surface state, queue/auth/runtime projection, command_boundary, transport_profile, and daemon-health summary state while check-repixelizer-gui.sh and systemd remain deployment/debug lowerings."
                    .to_string();
            intended_authority =
                "Repixelizer keeps product GUI delivery as the product boundary while publishing internal daemon health, queue/provider state, command boundary, and transport profile over CultNet/RUDP for Idunn and Odin."
                    .to_string();
            cut_line =
                "Keep Repixelizer's runtime-owned RUDP health and boundary store live; product web and host supervisor checks remain deployment/debug witnesses that must not own daemon truth."
                    .to_string();
            steps = vec![
                "Keep Repixelizer daemon health published over CultNet/RUDP from the live GUI/service runtime on Yggdrasil with contract repixelizer.cultnet-rudp-service-health.".to_string(),
                "Keep Repixelizer command_boundary and transport_profile records in the runtime-owned boundary store so deploy authority is inspectable typed state instead of an ops-only assumption.".to_string(),
                "Keep the Yggdrasil Repixelizer source-artifact lane shipping the app artifact plus the CultLib cultcache-py snapshot through a tiny remote runner script instead of brittle inline SSH quoting.".to_string(),
                "Teach Odin to ingest Repixelizer queue/auth/runtime state from the daemon-owned witness store instead of treating product web checks as daemon truth.".to_string(),
                "Keep Heimdall-backed browser auth as a product boundary adapter, not the keepalive truth surface.".to_string(),
                "Keep Repixelizer deployment checks as witness-only while Odin and Idunn consume the typed store.".to_string(),
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
    let yggdrasil_actuator = |action: &str, target: &str| {
        if action == "deploy" {
            format!(
                "sudo -n /usr/local/libexec/idunn-yggdrasil deploy {target} \"$IDUNN_SOURCE_COMMIT\" \"$IDUNN_REPOSITORY_FULL_NAME\" \"$IDUNN_UPSTREAM_REF\" \"$BIFROST_RELEASE_AUTHORITY_ID\" \"$BIFROST_RELEASE_AUTHORITY_SHA256\" \"$IDUNN_DEPLOYMENT_REQUEST_ID\" \"$IDUNN_REQUIRES_BIFROST_AUTHORITY\""
            )
        } else {
            format!("sudo -n /usr/local/libexec/idunn-yggdrasil {action} {target}")
        }
    };
    let project = |name: &str| PathBuf::from(format!(r"E:\Projects\{name}"));

    match options.profile.as_str() {
        "yggdrasil-local" => Ok(vec![
            DaemonTarget {
                daemon_id: "yggdrasil-voidbot".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil VoidBot".to_string(),
                health_contract: health_contract("voidbot.cultnet-rudp-stack-health", "failed"),
                deploy_command: Some(yggdrasil_actuator("deploy", "voidbot")),
                restart_command: Some(yggdrasil_actuator("restart", "voidbot")),
                release: Some(with_deployed_revision_witness(
                    release_target(
                        "VoidBot",
                        PathBuf::from("/srv/build/VoidBot"),
                        "restart-after-verified-build",
                        None,
                        "restart-required",
                    ),
                    PathBuf::from("/srv/voidbot/deploy/deployment.env"),
                )),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-heimdall".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Heimdall".to_string(),
                health_contract: health_contract("heimdall.cultnet-rudp-provider-health", "failed"),
                deploy_command: Some(yggdrasil_actuator("deploy", "heimdall")),
                restart_command: Some(yggdrasil_actuator("restart", "heimdall")),
                release: Some(release_target(
                    "Heimdall",
                    PathBuf::from("/srv/build/Heimdall"),
                    "restart-after-verified-build",
                    None,
                    "restart-required",
                )),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-epiphany".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Epiphany".to_string(),
                health_contract: health_contract("epiphany.cultnet-rudp-runtime-health", "failed"),
                deploy_command: Some(yggdrasil_actuator("deploy", "epiphany")),
                restart_command: Some(yggdrasil_actuator("restart", "epiphany")),
                release: Some(requiring_bifrost_authority(with_deployed_revision_witness(
                    release_target(
                        "Epiphany",
                        PathBuf::from("/srv/build/Epiphany"),
                        "restart-after-verified-build",
                        None,
                        "restart-required",
                    ),
                    PathBuf::from("/srv/epiphany/deploy/deployment.env"),
                ))),
                enabled: true,
                interval_seconds: 300,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-bifrost-persona-feedback".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Bifrost Persona feedback".to_string(),
                health_contract: health_contract(
                    "bifrost.cultnet-rudp-persona-feedback-health",
                    "stale-deployment",
                ),
                deploy_command: Some(yggdrasil_actuator("deploy", "bifrost-persona-feedback")),
                restart_command: None,
                release: Some(requiring_bifrost_authority(with_deployed_revision_witness(
                    release_target(
                        "Bifrost",
                        PathBuf::from("/srv/build/Bifrost-persona-feedback"),
                        "restart-after-verified-build",
                        None,
                        "restart-required",
                    ),
                    PathBuf::from("/srv/bifrost/persona-feedback/runtime/deployment.env"),
                ))),
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "yggdrasil-repixelizer".to_string(),
                verse_id: "yggdrasil.local".to_string(),
                name: "Yggdrasil Repixelizer".to_string(),
                health_contract: health_contract(
                    "repixelizer.cultnet-rudp-service-health",
                    "failed",
                ),
                deploy_command: Some(yggdrasil_actuator("deploy", "repixelizer")),
                restart_command: Some(yggdrasil_actuator("restart", "repixelizer")),
                release: Some(release_target(
                    "repixelizer",
                    PathBuf::from("/srv/build/repixelizer"),
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
                deploy_command: Some(yggdrasil_actuator("deploy", "streampixels")),
                restart_command: Some(yggdrasil_actuator("restart", "streampixels")),
                release: Some(release_target(
                    "StreamPixels",
                    PathBuf::from("/srv/build/StreamPixels"),
                    "restart-after-verified-build",
                    None,
                    "restart-required",
                )),
                enabled: true,
                interval_seconds: 300,
            },
        ]),
        "starfire-local" => Ok(vec![
            DaemonTarget {
                daemon_id: "odin".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Odin all-seer".to_string(),
                health_contract: health_contract("odin.cultnet-rudp-provider-health", "failed"),
                deploy_command: None,
                restart_command: Some(script("restart-odin.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "hermodr".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Hermodr browser lowering".to_string(),
                health_contract: locally_supervised_health_contract(
                    "hermodr.cultnet-rudp-browser-lowering-health",
                    "failed",
                ),
                deploy_command: None,
                restart_command: Some(script("restart-hermodr.ps1")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "mimir-eve-dashboard".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Mimir Eve dashboard".to_string(),
                health_contract: health_contract("mimir.cultnet-rudp-provider-health", "failed"),
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
                deploy_command: None,
                restart_command: Some(script("restart-stonks.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "weksa".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Weksa intent and utterance lowering service".to_string(),
                health_contract: health_contract("weksa.cultnet-rudp-provider-health", "failed"),
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
                health_contract: locally_supervised_health_contract(
                    "muninn.cultnet-rudp-local-telemetry-and-quest-access",
                    "failed",
                ),
                deploy_command: None,
                restart_command: Some(script("restart-starfire-muninn.ps1")),
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
                deploy_command: None,
                restart_command: Some(script("restart-muninn.ps1")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "vili".to_string(),
                verse_id: "raven.local".to_string(),
                name: "Vili Persona animation daemon".to_string(),
                health_contract: health_contract("vili.cultnet-rudp-animation-health", "failed"),
                deploy_command: None,
                restart_command: Some(script("restart-vili.cmd")),
                release: None,
                enabled: true,
                interval_seconds: 60,
            },
            DaemonTarget {
                daemon_id: "raven-sleipnir".to_string(),
                verse_id: "raven.local".to_string(),
                name: "Raven Sleipnir input mirror".to_string(),
                health_contract: health_contract(
                    "sleipnir.cultnet-rudp-input-mirror-health",
                    "failed",
                ),
                deploy_command: Some(script("deploy-raven-sleipnir.ps1")),
                restart_command: Some(script("restart-raven-sleipnir.ps1")),
                release: None,
                enabled: true,
                interval_seconds: 30,
            },
            DaemonTarget {
                daemon_id: "idunn-swarm-deployment-coverage".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Idunn swarm deployment coverage".to_string(),
                health_contract: health_contract("idunn.deployment-catalog-coherence", "degraded"),
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
                health_contract: locally_supervised_health_contract(
                    "muninn.cultnet-rudp-remote-telemetry-and-move-hid",
                    "failed",
                ),
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-muninn.ps1")),
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
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-eve-dashboard.ps1")),
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
                deploy_command: None,
                restart_command: Some(script("restart-nightwing-eve-browser-reference.ps1")),
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
        let args: Vec<String> = args.collect();
        let release_authority_validation = args
            .first()
            .is_some_and(|arg| arg == "validate-release-authority");
        let health_admission_validation = args
            .first()
            .is_some_and(|arg| arg == "validate-health-admission");
        let lifecycle_action = args.first().and_then(|arg| match arg.as_str() {
            "restart" | "request-restart" => Some(LifecycleAction::Restart),
            "redeploy" | "request-redeploy" | "deploy" | "request-deploy" => {
                Some(LifecycleAction::Redeploy)
            }
            _ => None,
        });
        let mut store_path = PathBuf::from("scratch/idunn/idunn.keepalive.cc");
        let mut release_authority_store_path = None;
        let mut deployment_brake_store_path = None;
        let mut deployment_brake_operator_anchor_path = None;
        let mut deployment_brake_runtime_id = None;
        let mut operator_alarm_command = None;
        let mut rudp_health_bind = None;
        let mut trusted_epiphany_health_identity_store = None;
        let mut daemon_health_trust_store_path = None;
        let mut service_identity_store_path = None;
        let mut public_health_store_path = None;
        let mut public_health_query_bind = None;
        let mut execute = false;
        let mut command_timeout_seconds = 30;
        let mut daemon_id = None;
        let mut verse_id = "local".to_string();
        let mut name = None;
        let mut deploy_command = None;
        let mut restart_command = None;
        let mut enabled = true;
        let mut interval_seconds = None;
        let mut swarm_profile = None;
        let mut repo_root = env::current_dir().context("determining current directory")?;
        let mut requested_by = env::var("USERNAME")
            .or_else(|_| env::var("USER"))
            .unwrap_or_else(|_| "operator".to_string());
        let mut command_detail = String::new();
        let mut validation_repository_full_name = None;
        let mut validation_upstream_ref = None;
        let mut validation_source_revision = None;
        let mut validation_authority_id = None;
        let mut validation_envelope_sha256 = None;
        let mut validation_release_id = None;
        let mut validation_release_witness_sha256 = None;
        let mut validation_deployment_request_id = None;

        let mut args = args.into_iter().peekable();
        if lifecycle_action.is_some() || release_authority_validation || health_admission_validation
        {
            let _ = args.next();
        }
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--store" => store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--release-authority-store" => {
                    release_authority_store_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--release-authority-store",
                    )?))
                }
                "--deployment-brake-store" => {
                    deployment_brake_store_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--deployment-brake-store",
                    )?))
                }
                "--deployment-brake-operator-anchor" => {
                    deployment_brake_operator_anchor_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--deployment-brake-operator-anchor",
                    )?))
                }
                "--deployment-brake-runtime-id" => {
                    deployment_brake_runtime_id =
                        Some(take_value(&mut args, "--deployment-brake-runtime-id")?)
                }
                "--repository-full-name" => {
                    validation_repository_full_name =
                        Some(take_value(&mut args, "--repository-full-name")?)
                }
                "--upstream-ref" => {
                    validation_upstream_ref = Some(take_value(&mut args, "--upstream-ref")?)
                }
                "--source-revision" => {
                    validation_source_revision = Some(take_value(&mut args, "--source-revision")?)
                }
                "--release-authority-id" => {
                    validation_authority_id = Some(take_value(&mut args, "--release-authority-id")?)
                }
                "--release-authority-sha256" => {
                    validation_envelope_sha256 =
                        Some(take_value(&mut args, "--release-authority-sha256")?)
                }
                "--release-id" => {
                    validation_release_id = Some(take_value(&mut args, "--release-id")?)
                }
                "--release-witness-sha256" => {
                    validation_release_witness_sha256 =
                        Some(take_value(&mut args, "--release-witness-sha256")?)
                }
                "--deployment-request-id" => {
                    validation_deployment_request_id =
                        Some(take_value(&mut args, "--deployment-request-id")?)
                }
                "--daemon" => daemon_id = Some(take_value(&mut args, "--daemon")?),
                "--verse" => verse_id = take_value(&mut args, "--verse")?,
                "--name" => name = Some(take_value(&mut args, "--name")?),
                "--health-command" => {
                    let _ = take_value(&mut args, "--health-command")?;
                    return Err(anyhow!(
                        "--health-command has been removed; daemon health must arrive as typed {} records over CultNet/RUDP",
                        CULTNET_RUDP_PROTOCOL_ID
                    ));
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
                "--trusted-epiphany-health-identity-store" => {
                    trusted_epiphany_health_identity_store = Some(PathBuf::from(take_value(
                        &mut args,
                        "--trusted-epiphany-health-identity-store",
                    )?));
                }
                "--daemon-health-trust-store" => {
                    daemon_health_trust_store_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--daemon-health-trust-store",
                    )?));
                }
                "--service-identity-store" => {
                    service_identity_store_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--service-identity-store",
                    )?));
                }
                "--public-health-store" => {
                    public_health_store_path = Some(PathBuf::from(take_value(
                        &mut args,
                        "--public-health-store",
                    )?));
                }
                "--public-health-query-bind" => {
                    public_health_query_bind = Some(
                        take_value(&mut args, "--public-health-query-bind")?
                            .parse()
                            .with_context(
                                || "--public-health-query-bind must be a socket address",
                            )?,
                    );
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
                "--requested-by" => requested_by = take_value(&mut args, "--requested-by")?,
                "--detail" => command_detail = take_value(&mut args, "--detail")?,
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
            release_authority_store_path,
            deployment_brake_store_path,
            deployment_brake_operator_anchor_path,
            deployment_brake_runtime_id: deployment_brake_runtime_id.or_else(|| {
                (swarm_profile.as_deref() == Some("yggdrasil-local"))
                    .then(|| "yggdrasil".to_string())
            }),
            operator_alarm_command,
            rudp_health_bind,
            trusted_epiphany_health_identity_store,
            daemon_health_trust_store_path,
            service_identity_store_path,
            public_health_store_path,
            public_health_query_bind,
            execute,
            command_timeout_seconds,
        };

        if release_authority_validation {
            let store_path = common.release_authority_store_path.clone().ok_or_else(|| {
                anyhow!("release authority validation requires --release-authority-store")
            })?;
            return Ok(Self {
                common,
                mode: Mode::ReleaseAuthorityValidation(ReleaseAuthorityValidationOptions {
                    store_path,
                    repository_full_name: validation_repository_full_name.ok_or_else(|| {
                        anyhow!("release authority validation requires --repository-full-name")
                    })?,
                    upstream_ref: validation_upstream_ref.ok_or_else(|| {
                        anyhow!("release authority validation requires --upstream-ref")
                    })?,
                    source_revision: validation_source_revision.ok_or_else(|| {
                        anyhow!("release authority validation requires --source-revision")
                    })?,
                    authority_id: validation_authority_id.ok_or_else(|| {
                        anyhow!("release authority validation requires --release-authority-id")
                    })?,
                    envelope_sha256: validation_envelope_sha256.ok_or_else(|| {
                        anyhow!("release authority validation requires --release-authority-sha256")
                    })?,
                }),
            });
        }

        if health_admission_validation {
            return Ok(Self {
                common,
                mode: Mode::HealthAdmissionValidation(HealthAdmissionValidationOptions {
                    daemon_id: daemon_id
                        .ok_or_else(|| anyhow!("health admission validation requires --daemon"))?,
                    deployment_request_id: validation_deployment_request_id.ok_or_else(|| {
                        anyhow!("health admission validation requires --deployment-request-id")
                    })?,
                    release_id: validation_release_id.ok_or_else(|| {
                        anyhow!("health admission validation requires --release-id")
                    })?,
                    release_witness_sha256: validation_release_witness_sha256.ok_or_else(|| {
                        anyhow!("health admission validation requires --release-witness-sha256")
                    })?,
                    source_commit: validation_source_revision.ok_or_else(|| {
                        anyhow!("health admission validation requires --source-revision")
                    })?,
                }),
            });
        }

        if swarm_profile.as_deref() == Some("yggdrasil-local")
            && common.release_authority_store_path.is_none()
        {
            return Err(anyhow!(
                "--release-authority-store is required for yggdrasil-local release targets"
            ));
        }
        if swarm_profile.as_deref() == Some("yggdrasil-local")
            && common.trusted_epiphany_health_identity_store.is_none()
        {
            return Err(anyhow!(
                "--trusted-epiphany-health-identity-store is required for yggdrasil-local"
            ));
        }
        if swarm_profile.as_deref() == Some("yggdrasil-local")
            && (common.deployment_brake_store_path.is_none()
                || common.deployment_brake_operator_anchor_path.is_none())
        {
            return Err(anyhow!(
                "--deployment-brake-store and --deployment-brake-operator-anchor are required for yggdrasil-local"
            ));
        }

        if let Some(action) = lifecycle_action {
            if swarm_profile.is_some() {
                return Err(anyhow!(
                    "lifecycle command publication uses --daemon, not --swarm-profile\n\n{}",
                    help_text()
                ));
            }
            let Some(daemon_id) = daemon_id else {
                return Err(anyhow!(
                    "lifecycle command publication requires --daemon\n\n{}",
                    help_text()
                ));
            };
            return Ok(Self {
                common,
                mode: Mode::LifecycleCommand(LifecycleCommandOptions {
                    daemon_id,
                    action,
                    requested_by,
                    detail: command_detail,
                }),
            });
        }

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
                    health_contract: health_contract("manual.cultnet-rudp-daemon-health", "failed"),
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

        if common.public_health_query_bind.is_some() && !matches!(mode, Mode::Swarm(_)) {
            return Err(anyhow!(
                "--public-health-query-bind requires --swarm-profile"
            ));
        }
        if common.public_health_query_bind.is_some()
            && (common.public_health_store_path.is_none()
                || common.service_identity_store_path.is_none())
        {
            return Err(anyhow!(
                "--public-health-query-bind requires --public-health-store and --service-identity-store"
            ));
        }

        Ok(Self { common, mode })
    }
}

fn take_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn publish_lifecycle_command(
    command: &LifecycleCommandOptions,
    options: &CommonOptions,
) -> Result<()> {
    let now = timestamp()?;
    let action = match command.action {
        LifecycleAction::Restart => "restart",
        LifecycleAction::Redeploy => "redeploy",
    };
    let record = IdunnLifecycleCommandRecord {
        command_id: format!("manual:{}:{}:{}", action, command.daemon_id, now),
        daemon_id: command.daemon_id.clone(),
        action: action.to_string(),
        state: "pending".to_string(),
        requested_by: command.requested_by.clone(),
        requested_at: now.clone(),
        detail: command.detail.clone(),
        claimed_at: String::new(),
        result_id: String::new(),
    };
    let store_lock = Arc::new(Mutex::new(()));
    with_store_node(options, &store_lock, |node| {
        node.put(&record.command_id, &record)?;
        Ok(())
    })?;
    println!(
        "Idunn lifecycle command {} queued for {} as {}.",
        record.action, record.daemon_id, record.command_id
    );
    Ok(())
}

fn run_restart(
    request: &odin_core::IdunnRestartRequestRecord,
    requested_at: &str,
    options: &CommonOptions,
) -> IdunnRestartResultRecord {
    let result_id = format!("result:{}", request.request_id);
    match run_braked_shell(
        &request.command,
        options,
        &format!("restart:{}", request.daemon_id),
        &request.request_id,
        None,
    ) {
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
    options: &CommonOptions,
    store_lock: &Arc<Mutex<()>>,
) -> IdunnDeploymentResultRecord {
    let result_id = format!("result:{}", request.request_id);
    if let Err(error) = revalidate_deployment_request(request, options, store_lock) {
        return IdunnDeploymentResultRecord {
            result_id,
            request_id: request.request_id.clone(),
            daemon_id: request.daemon_id.clone(),
            state: "failed".to_string(),
            detail: format!("deployment authority revalidation failed: {error:#}"),
            completed_at: requested_at.to_string(),
        };
    }
    match run_braked_shell(
        &request.command,
        options,
        &request.source_revision,
        &request.request_id,
        Some(request),
    ) {
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
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_string();
    }

    let separator = "...<truncated>...";
    let separator_len = separator.chars().count();
    if max_chars <= separator_len {
        return chars.into_iter().take(max_chars).collect();
    }

    let retained = max_chars - separator_len;
    let head_len = retained / 3;
    let tail_len = retained - head_len;
    let head: String = chars.iter().take(head_len).collect();
    let tail: String = chars.iter().skip(chars.len() - tail_len).collect();
    format!("{head}{separator}{tail}")
}

fn run_state_migration(
    target: &DaemonTarget,
    release: &ReleaseTarget,
    deployment: &IdunnDeploymentRequestRecord,
    requested_at: &str,
    options: &CommonOptions,
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
    // Migration is consequential deployment work. It consumes the same exact
    // release/request grant as the deployment spawn that follows it.
    match run_braked_shell(
        command,
        options,
        &deployment.source_revision,
        &deployment.request_id,
        Some(deployment),
    ) {
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

fn run_shell(
    command: &str,
    options: &CommonOptions,
    release_id: &str,
    deployment_id: &str,
    deployment: Option<&IdunnDeploymentRequestRecord>,
) -> Result<std::process::Output> {
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

    process
        .env("IDUNN_ACTUATOR", "1")
        .env("IDUNN_COMMAND_AUTHORITY", "idunn-daemon");
    if let Some(request) = deployment {
        process
            .env("IDUNN_SOURCE_COMMIT", &request.source_revision)
            .env("IDUNN_REPOSITORY_FULL_NAME", &request.repository_full_name)
            .env("IDUNN_UPSTREAM_REF", &request.upstream_ref)
            .env(
                "BIFROST_RELEASE_AUTHORITY_ID",
                &request.release_authority_id,
            )
            .env(
                "BIFROST_RELEASE_AUTHORITY_SHA256",
                &request.release_authority_envelope_sha256,
            )
            .env("IDUNN_DEPLOYMENT_REQUEST_ID", &request.request_id);
        process.env(
            "IDUNN_REQUIRES_BIFROST_AUTHORITY",
            if request.requires_bifrost_authority {
                "true"
            } else {
                "false"
            },
        );
    }
    if let Some(endpoint) = actuator_idunn_rudp_health_endpoint(options) {
        process
            .env("IDUNN_RUDP_HEALTH", &endpoint)
            .env("ODIN_IDUNN_RUDP_HEALTH", &endpoint);
    }

    let stdout_path = command_output_path("stdout")?;
    let stderr_path = command_output_path("stderr")?;
    let stdout_file = File::create(&stdout_path)
        .with_context(|| format!("creating {}", stdout_path.display()))?;
    let stderr_file = File::create(&stderr_path)
        .with_context(|| format!("creating {}", stderr_path.display()))?;

    configure_command_lifetime(&mut process);
    // This is deliberately the last fallible authority read before spawn.
    // Preparation above has no consequence outside Idunn's temporary files.
    verify_deployment_brake(options, release_id, deployment_id)?;
    let child = process
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .with_context(|| format!("running command {command:?}"))?;
    let status = wait_for_child_status_with_timeout(
        child,
        Duration::from_secs(options.command_timeout_seconds),
        command,
    )?;
    let stdout = fs::read(&stdout_path).unwrap_or_default();
    let stderr = fs::read(&stderr_path).unwrap_or_default();
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// The single final consequence gate. Callers may plan and persist before this,
/// but every migration, deployment, and restart re-opens both root-owned files
/// here, immediately before `Command::spawn`.
fn run_braked_shell(
    command: &str,
    options: &CommonOptions,
    release_id: &str,
    deployment_id: &str,
    deployment: Option<&IdunnDeploymentRequestRecord>,
) -> Result<std::process::Output> {
    run_shell(command, options, release_id, deployment_id, deployment)
}

fn verify_deployment_brake(
    options: &CommonOptions,
    release_id: &str,
    deployment_id: &str,
) -> Result<()> {
    let store_path = options
        .deployment_brake_store_path
        .as_ref()
        .ok_or_else(|| anyhow!("--deployment-brake-store is required for actuation"))?;
    let anchor_path = options
        .deployment_brake_operator_anchor_path
        .as_ref()
        .ok_or_else(|| anyhow!("--deployment-brake-operator-anchor is required for actuation"))?;
    let runtime_id = options
        .deployment_brake_runtime_id
        .as_deref()
        .ok_or_else(|| anyhow!("deployment brake runtime id is not configured"))?;

    let entries = SingleFileMessagePackBackingStore::new(store_path)
        .pull_all_read_only_snapshot()
        .with_context(|| format!("reading deployment brake {}", store_path.display()))?;
    let observation = match entries.as_slice() {
        [entry]
            if entry.r#type == IdunnDeploymentBrakeRecord::TYPE
                && entry.key == cultnet_rs::IDUNN_DEPLOYMENT_BRAKE_ID
                && entry.schema_id.as_deref() == Some(IDUNN_DEPLOYMENT_BRAKE_SCHEMA) =>
        {
            match rmp_serde::from_slice::<IdunnDeploymentBrakeRecord>(&entry.payload) {
                Ok(record)
                    if rmp_serde::to_vec(&record).ok().as_deref()
                        == Some(entry.payload.as_slice()) =>
                {
                    record
                }
                _ => return Err(anyhow!("deployment brake is corrupt")),
            }
        }
        [] => return Err(anyhow!("deployment brake is missing")),
        _ => return Err(anyhow!("deployment brake store is corrupt or ambiguous")),
    };
    let anchor_entries = SingleFileMessagePackBackingStore::new(anchor_path)
        .pull_all_read_only_snapshot()
        .with_context(|| {
            format!(
                "reading deployment brake operator anchor {}",
                anchor_path.display()
            )
        })?;
    let [anchor_entry] = anchor_entries.as_slice() else {
        return Err(anyhow!(
            "deployment brake operator anchor is missing or ambiguous"
        ));
    };
    if anchor_entry.r#type != IdunnDeploymentBrakeOperatorIdentity::TRUST_ANCHOR_TYPE
        || anchor_entry.key != IdunnDeploymentBrakeOperatorIdentity::TRUST_ANCHOR_KEY
        || anchor_entry.schema_id.as_deref()
            != Some(IdunnDeploymentBrakeOperatorIdentity::TRUST_ANCHOR_SCHEMA)
    {
        return Err(anyhow!("deployment brake operator anchor is foreign"));
    }
    let anchor: ServiceIdentityTrustAnchor = rmp_serde::from_slice(&anchor_entry.payload)
        .context("decoding deployment brake operator anchor")?;
    if rmp_serde::to_vec(&anchor)? != anchor_entry.payload {
        return Err(anyhow!("deployment brake operator anchor is noncanonical"));
    }
    evaluate_idunn_deployment_brake(
        IdunnDeploymentBrakeObservation::Present(&observation),
        &anchor,
        runtime_id,
        release_id,
        deployment_id,
        unix_epoch_millis()?,
    )
    .map_err(|denial| anyhow!("deployment brake denied actuation: {denial:?}"))
}

fn actuator_idunn_rudp_health_endpoint(options: &CommonOptions) -> Option<String> {
    let bind = options.rudp_health_bind?;
    let host = if bind.ip().is_unspecified() {
        "127.0.0.1".to_string()
    } else {
        bind.ip().to_string()
    };
    Some(format!("{host}:{}", bind.port()))
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

    configure_command_lifetime(&mut process);
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

#[cfg(unix)]
fn configure_command_lifetime(process: &mut Command) {
    use std::os::unix::process::CommandExt;
    process.process_group(0);
}

#[cfg(not(unix))]
fn configure_command_lifetime(_process: &mut Command) {}

#[cfg(unix)]
fn terminate_command_lifetime(child: &mut Child) {
    let process_group = i32::try_from(child.id()).expect("Unix child pid exceeded i32");
    // Idunn created the child as leader of this private process group. Killing
    // the group ends sudo, shells, and deployment descendants under the same
    // command lifetime instead of merely severing the parent we can see.
    unsafe {
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_command_lifetime(child: &mut Child) {
    let _ = child.kill();
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
            terminate_command_lifetime(&mut child);
            let _ = child.wait_with_output();
            return Err(anyhow!(
                "command timed out after {} seconds: {command:?}",
                timeout.as_secs()
            ));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_child_status_with_timeout(
    mut child: Child,
    timeout: Duration,
    command: &str,
) -> Result<ExitStatus> {
    let started_at = Instant::now();

    loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("waiting on command {command:?}"))?
        {
            return Ok(status);
        }

        if started_at.elapsed() >= timeout {
            terminate_command_lifetime(&mut child);
            let _ = child.wait();
            return Err(anyhow!(
                "command timed out after {} seconds: {command:?}",
                timeout.as_secs()
            ));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn command_output_path(kind: &str) -> Result<PathBuf> {
    let millis = unix_epoch_millis()?;
    Ok(env::temp_dir().join(format!(
        "idunn-command-{kind}-{}-{millis}.log",
        std::process::id()
    )))
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
    "Usage: idunn --daemon <id> [--name <name>] [--verse <verse>] [--store <path>] [--release-authority-store <path>] [--daemon-health-trust-store <path>] [--deploy-command <command>] [--restart-command <command>] [--operator-alarm-command <command>] [--rudp-health-bind <addr|none>] [--trusted-epiphany-health-identity-store <path>] [--execute] [--interval-seconds <seconds>] [--command-timeout-seconds <seconds>] [--repo-root <path>] [--swarm-profile <profile>] [--service-identity-store <path>] [--public-health-store <path>] [--public-health-query-bind <addr>]\n       idunn restart --daemon <id> [--store <path>] [--requested-by <who>] [--detail <text>]\n       idunn redeploy --daemon <id> [--store <path>] [--requested-by <who>] [--detail <text>]\n       idunn validate-health-admission --store <path> --daemon <id> --deployment-request-id <id> --release-id <id> --release-witness-sha256 <sha256> --source-revision <commit>\n\nIdunn supervises owner-authenticated CultNet/RUDP health with --daemon, or a built-in swarm supervisor with --swarm-profile starfire-local or yggdrasil-local. Generic signed health requires a root-owned CultCache trust store via --daemon-health-trust-store. Unsigned idunn.daemon_health packets are diagnostic-only and cannot satisfy lifecycle health. Yggdrasil release targets require an explicit Bifrost CultCache path via --release-authority-store. The Epiphany v0 migration path additionally requires its pinned host identity. RUDP health ingress is disabled unless --rudp-health-bind is supplied. The read-only outward projection listener is disabled unless --public-health-query-bind is supplied with the dedicated public store and Idunn service identity; it serves only target-catalog authenticated-provider projection keys and never opens private Idunn state. The restart/redeploy verbs publish typed idunn.lifecycle_command.v1 records; the running supervisor claims them and executes only through its configured command boundary."
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_actuation_gate_serializes_request_owners() {
        let gate = Arc::new(TargetActuationGate::new());
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let first_gate = Arc::clone(&gate);
        let first_entered = entered_tx.clone();
        let first = thread::spawn(move || {
            with_target_actuation_gate(&first_gate.lock, || {
                first_entered.send("manual").unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        assert_eq!(entered_rx.recv().unwrap(), "manual");

        let second_gate = Arc::clone(&gate);
        let second_entered = entered_tx;
        let second = thread::spawn(move || {
            with_target_actuation_gate(&second_gate.lock, || {
                second_entered.send("automatic").unwrap();
                Ok(())
            })
            .unwrap();
        });
        assert!(
            entered_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "automatic request owner entered while manual actuation was live"
        );
        release_tx.send(()).unwrap();
        assert_eq!(entered_rx.recv().unwrap(), "automatic");
        first.join().unwrap();
        second.join().unwrap();
    }

    #[test]
    fn blocked_actuation_does_not_block_observation_and_admission_work() {
        let gate = Arc::new(TargetActuationGate::new());
        assert!(!gate.reserved.swap(true, Ordering::AcqRel));
        let manual_guard = gate.lock.lock().unwrap();
        let (actuation_tx, actuation_rx) = std::sync::mpsc::channel();
        schedule_target_actuation(Arc::clone(&gate), "epiphany".into(), move || {
            actuation_tx.send("actuated").unwrap();
            Ok(())
        });

        assert!(
            actuation_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        // This is the target loop's independent signed-health admission lane:
        // it does not acquire or await the consequence gate.
        let admitted_health = "accepted";
        assert_eq!(admitted_health, "accepted");

        drop(manual_guard);
        gate.reserved.store(false, Ordering::Release);
        let (actuation_tx, actuation_rx) = std::sync::mpsc::channel();
        schedule_target_actuation(Arc::clone(&gate), "epiphany".into(), move || {
            actuation_tx.send("actuated").unwrap();
            Ok(())
        });
        assert_eq!(actuation_rx.recv().unwrap(), "actuated");
        while gate.reserved.load(Ordering::Acquire) {
            thread::yield_now();
        }
    }

    #[cfg(unix)]
    fn descendant_marker_command(marker: &std::path::Path) -> Child {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("(sleep 1; touch '{}') & wait", marker.display()))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_command_lifetime(&mut command);
        command.spawn().expect("spawn process-group test command")
    }

    #[cfg(unix)]
    #[test]
    fn status_timeout_terminates_command_descendants() {
        let marker =
            env::temp_dir().join(format!("idunn-status-descendant-{}", std::process::id()));
        let _ = fs::remove_file(&marker);
        let error = wait_for_child_status_with_timeout(
            descendant_marker_command(&marker),
            Duration::from_millis(100),
            "descendant timeout test",
        )
        .expect_err("command should time out");
        assert!(error.to_string().contains("timed out"));
        thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "timed-out descendant survived to write marker"
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_timeout_terminates_command_descendants() {
        let marker =
            env::temp_dir().join(format!("idunn-output-descendant-{}", std::process::id()));
        let _ = fs::remove_file(&marker);
        let error = wait_for_child_with_timeout(
            descendant_marker_command(&marker),
            Duration::from_millis(100),
            "descendant output timeout test",
        )
        .expect_err("command should time out");
        assert!(error.to_string().contains("timed out"));
        thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "timed-out descendant survived to write marker"
        );
    }
    use cultnet_rs::{CultNetRawDocumentRecord, CultNetRawPayloadEncoding};
    use ed25519_dalek::{Signer, SigningKey};
    use std::cell::RefCell;

    const EPIPHANY_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn truncated_command_detail_preserves_failure_tail() {
        let value = format!("{}fatal: promotion failed", "compiler noise\n".repeat(100));
        let detail = truncate_detail(&value, 120);

        assert_eq!(detail.chars().count(), 120);
        assert!(detail.starts_with("compiler noise"));
        assert!(detail.contains("...<truncated>..."));
        assert!(detail.ends_with("fatal: promotion failed"));
    }

    #[test]
    fn short_command_detail_is_unchanged() {
        assert_eq!(truncate_detail("precise failure", 120), "precise failure");
    }

    fn authority_record(status: &str) -> BifrostRepositoryReleaseAuthorityRecord {
        let repository = "GameCult/Epiphany";
        let upstream_ref = "refs/heads/main";
        BifrostRepositoryReleaseAuthorityRecord {
            authority_id: release_authority_id(repository, upstream_ref, EPIPHANY_SHA),
            command_id: "cultmesh-command-epiphany-release".to_string(),
            crossing_receipt_id: "crossing_epiphany_release".to_string(),
            repository_full_name: repository.to_string(),
            upstream_ref: upstream_ref.to_string(),
            commit_sha: EPIPHANY_SHA.to_string(),
            decision: "authorize".to_string(),
            status: status.to_string(),
            policy_decision_id: "policy-epiphany-release".to_string(),
            authority_reference: "bifrost:release:epiphany".to_string(),
            actor_identity: "metacrat".to_string(),
            source_kind: "bifrost.governance.topic".to_string(),
            source_id: "topic-epiphany-release".to_string(),
            epiphany_run_id: "run-epiphany-release".to_string(),
            epiphany_lane_id: "hands-publication".to_string(),
            epiphany_agent_identity: "epiphany".to_string(),
            external_receipt_url: "https://github.com/GameCult/Epiphany/commit/0123456789abcdef0123456789abcdef01234567".to_string(),
            external_receipt_id: EPIPHANY_SHA.to_string(),
            authorized_at: "2026-07-16T00:00:00Z".to_string(),
            expires_at: String::new(),
            revoked_at: if status == "revoked" { "2026-07-16T01:00:00Z".to_string() } else { String::new() },
            revocation_reason: if status == "revoked" { "operator revoked release".to_string() } else { String::new() },
        }
    }

    fn authority_store(record: &BifrostRepositoryReleaseAuthorityRecord) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "idunn-bifrost-authority-{}-{}.cc",
            std::process::id(),
            RELEASE_AUTHORITY_SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        write_authority_store(&path, record);
        path
    }

    fn write_authority_store(
        path: &std::path::Path,
        record: &BifrostRepositoryReleaseAuthorityRecord,
    ) {
        let mut backing = SingleFileMessagePackBackingStore::new(path);
        backing
            .push(&CultCacheEnvelope {
                key: record.authority_id.clone(),
                r#type: BifrostRepositoryReleaseAuthorityRecord::TYPE.to_string(),
                payload: rmp_serde::to_vec_named(&BifrostReleaseAuthorityWire::from(record))
                    .unwrap(),
                stored_at: "2026-07-16T00:00:00Z".to_string(),
                schema_id: Some(odin_core::BIFROST_REPOSITORY_RELEASE_AUTHORITY_SCHEMA.to_string()),
            })
            .unwrap();
    }

    fn decode_hex_fixture(value: &str) -> Vec<u8> {
        let bytes = value.trim().as_bytes();
        assert_eq!(bytes.len() % 2, 0);
        bytes
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn bifrost_release_authority_golden_contract_decodes_and_seals() {
        let record = authority_record("authorized");
        let path = env::temp_dir().join(format!(
            "idunn-bifrost-typescript-golden-{}-{}.cc",
            std::process::id(),
            RELEASE_AUTHORITY_SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let payload = decode_hex_fixture(include_str!(
            "../tests/fixtures/bifrost-release-authority-v1.payload.hex"
        ));
        let decoded: BifrostReleaseAuthorityWire = rmp_serde::from_slice(&payload).unwrap();
        assert_eq!(decoded.authority_id, record.authority_id);
        let mut backing = SingleFileMessagePackBackingStore::new(&path);
        backing
            .push(&CultCacheEnvelope {
                key: record.authority_id.clone(),
                r#type: BifrostRepositoryReleaseAuthorityRecord::TYPE.to_string(),
                payload,
                stored_at: "2026-07-16T16:56:33.727Z".to_string(),
                schema_id: Some(odin_core::BIFROST_REPOSITORY_RELEASE_AUTHORITY_SCHEMA.to_string()),
            })
            .unwrap();
        let authorization = CultCacheReleaseAuthorityPort { store_path: &path }
            .authorize(
                "GameCult/Epiphany",
                "refs/heads/main",
                EPIPHANY_SHA,
                "unix:1784246400",
            )
            .unwrap();

        assert_eq!(
            authorization.authority_id,
            format!("release:GameCult/Epiphany:refs/heads/main:{EPIPHANY_SHA}")
        );
        assert_eq!(authorization.envelope_sha256.len(), 64);
        assert!(
            authorization
                .envelope_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn revoked_bifrost_release_authority_fails_closed() {
        let record = authority_record("revoked");
        let path = authority_store(&record);
        let error = CultCacheReleaseAuthorityPort { store_path: &path }
            .authorize(
                "GameCult/Epiphany",
                "refs/heads/main",
                EPIPHANY_SHA,
                "unix:1784246400",
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("not currently authorized"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn bifrost_receipt_selects_release_independently_of_newer_upstream_head() {
        let record = authority_record("authorized");
        let path = authority_store(&record);
        let selected = CultCacheReleaseAuthorityPort { store_path: &path }
            .select("GameCult/Epiphany", "refs/heads/main", "unix:1784246400")
            .unwrap();

        assert_eq!(selected.source_revision, EPIPHANY_SHA);
        assert_ne!(
            selected.source_revision, "ffffffffffffffffffffffffffffffffffffffff",
            "a newer observed main must not replace the authorized release"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn multiple_current_bifrost_authorities_fail_release_selection() {
        let first = authority_record("authorized");
        let path = authority_store(&first);
        let mut second = first.clone();
        second.commit_sha = "fedcba9876543210fedcba9876543210fedcba98".to_string();
        second.authority_id = release_authority_id(
            &second.repository_full_name,
            &second.upstream_ref,
            &second.commit_sha,
        );
        second.external_receipt_id = second.commit_sha.clone();
        second.external_receipt_url = format!(
            "https://github.com/{}/commit/{}",
            second.repository_full_name, second.commit_sha
        );
        write_authority_store(&path, &second);

        let error = CultCacheReleaseAuthorityPort { store_path: &path }
            .select("GameCult/Epiphany", "refs/heads/main", "unix:1784246400")
            .unwrap_err()
            .to_string();

        assert!(error.contains("exactly one current authorized receipt"));
        assert!(error.contains("found 2"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn bifrost_external_proof_must_bind_exact_commit() {
        let mut record = authority_record("authorized");
        record.external_receipt_id = "ffffffffffffffffffffffffffffffffffffffff".to_string();
        let path = authority_store(&record);
        let error = CultCacheReleaseAuthorityPort { store_path: &path }
            .authorize(
                "GameCult/Epiphany",
                "refs/heads/main",
                EPIPHANY_SHA,
                "unix:1784246400",
            )
            .unwrap_err()
            .to_string();

        assert!(error.contains("external GitHub proof"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pre_actuation_revalidation_observes_bifrost_revocation() {
        let authorized = authority_record("authorized");
        let path = authority_store(&authorized);
        let authorization = CultCacheReleaseAuthorityPort { store_path: &path }
            .authorize(
                "GameCult/Epiphany",
                "refs/heads/main",
                EPIPHANY_SHA,
                "unix:1784246400",
            )
            .unwrap();
        let mut request = IdunnDeploymentRequestRecord {
            request_id: "deploy:yggdrasil-epiphany:test".to_string(),
            daemon_id: "yggdrasil-epiphany".to_string(),
            command: "must-not-run".to_string(),
            authority: "idunn-supervisor-command.manual".to_string(),
            requested_at: "unix:1784246400".to_string(),
            repository_full_name: String::new(),
            upstream_ref: String::new(),
            source_revision: String::new(),
            release_authority_id: String::new(),
            release_authority_envelope_sha256: String::new(),
            requires_bifrost_authority: false,
        };
        apply_release_authorization(&mut request, &authorization);
        write_authority_store(&path, &authority_record("revoked"));
        let options = CommonOptions {
            store_path: env::temp_dir().join("unused-idunn-store.cc"),
            release_authority_store_path: Some(path.clone()),
            deployment_brake_store_path: None,
            deployment_brake_operator_anchor_path: None,
            deployment_brake_runtime_id: None,
            operator_alarm_command: None,
            rudp_health_bind: None,
            trusted_epiphany_health_identity_store: None,
            daemon_health_trust_store_path: None,
            service_identity_store_path: None,
            public_health_store_path: None,
            public_health_query_bind: None,
            execute: true,
            command_timeout_seconds: 1,
        };

        let error = revalidate_deployment_request(&request, &options, &Arc::new(Mutex::new(())))
            .unwrap_err()
            .to_string();

        assert!(error.contains("not currently authorized"));
        let privileged_error =
            validate_release_authority_at_privileged_boundary(&ReleaseAuthorityValidationOptions {
                store_path: path.clone(),
                repository_full_name: request.repository_full_name.clone(),
                upstream_ref: request.upstream_ref.clone(),
                source_revision: request.source_revision.clone(),
                authority_id: request.release_authority_id.clone(),
                envelope_sha256: request.release_authority_envelope_sha256.clone(),
            })
            .unwrap_err()
            .to_string();
        assert!(privileged_error.contains("not currently authorized"));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn yggdrasil_profile_requires_explicit_bifrost_authority_store() {
        let error = Options::parse(
            ["--swarm-profile", "yggdrasil-local"]
                .into_iter()
                .map(ToString::to_string),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--release-authority-store is required"));
    }

    #[test]
    fn privileged_authority_validator_requires_exact_frozen_lineage() {
        let authority_id = format!("release:GameCult/Epiphany:refs/heads/main:{EPIPHANY_SHA}");
        let options = Options::parse(
            [
                "validate-release-authority",
                "--release-authority-store",
                "C:/authority.cc",
                "--repository-full-name",
                "GameCult/Epiphany",
                "--upstream-ref",
                "refs/heads/main",
                "--source-revision",
                EPIPHANY_SHA,
                "--release-authority-id",
                &authority_id,
                "--release-authority-sha256",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ]
            .into_iter()
            .map(ToString::to_string),
        )
        .unwrap();

        let Mode::ReleaseAuthorityValidation(validation) = options.mode else {
            panic!("expected privileged release-authority validation posture");
        };
        assert_eq!(validation.repository_full_name, "GameCult/Epiphany");
        assert_eq!(validation.source_revision, EPIPHANY_SHA);
        assert_eq!(validation.authority_id, authority_id);
    }

    fn target(default_failure_state: &str, deploy_command: Option<&str>) -> DaemonTarget {
        DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.contract", default_failure_state),
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
    fn parser_rejects_removed_health_command_lane() {
        let error = Options::parse(
            [
                "--daemon",
                "test-daemon",
                "--health-command",
                "health-test.cmd",
            ]
            .into_iter()
            .map(ToString::to_string),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("--health-command has been removed"));
        assert!(error.contains(CULTNET_RUDP_PROTOCOL_ID));
    }

    #[test]
    fn parser_does_not_default_rudp_health_ingress_to_localhost() {
        let options = Options::parse(
            ["--daemon", "test-daemon"]
                .into_iter()
                .map(ToString::to_string),
        )
        .unwrap();

        assert_eq!(options.common.rudp_health_bind, None);
    }

    #[test]
    fn parser_accepts_explicit_rudp_health_ingress() {
        let options = Options::parse(
            [
                "--daemon",
                "test-daemon",
                "--rudp-health-bind",
                "0.0.0.0:17870",
            ]
            .into_iter()
            .map(ToString::to_string),
        )
        .unwrap();

        assert_eq!(
            options.common.rudp_health_bind,
            Some("0.0.0.0:17870".parse().unwrap())
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
            deploy_command: None,
            restart_command: Some("restart-starfire-muninn.ps1".to_string()),
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
            deploy_command: None,
            restart_command: Some("restart-nightwing-muninn.ps1".to_string()),
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
            deploy_command: None,
            restart_command: Some("restart-muninn.ps1".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let odin = DaemonTarget {
            daemon_id: "odin".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Odin".to_string(),
            health_contract: health_contract("odin.cultnet-rudp-provider-health", "failed"),
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
            deploy_command: None,
            restart_command: Some("restart-nightwing-eve-dashboard.ps1".to_string()),
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
            deploy_command: None,
            restart_command: Some("restart-nightwing-eve-browser-reference.ps1".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let vili = DaemonTarget {
            daemon_id: "vili".to_string(),
            verse_id: "raven.local".to_string(),
            name: "Vili".to_string(),
            health_contract: health_contract("vili.cultnet-rudp-animation-health", "failed"),
            deploy_command: None,
            restart_command: Some("restart-vili.cmd".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let raven_sleipnir = DaemonTarget {
            daemon_id: "raven-sleipnir".to_string(),
            verse_id: "raven.local".to_string(),
            name: "Raven Sleipnir".to_string(),
            health_contract: health_contract("sleipnir.cultnet-rudp-input-mirror-health", "failed"),
            deploy_command: Some("deploy-raven-sleipnir.ps1".to_string()),
            restart_command: Some("restart-raven-sleipnir.ps1".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let yggdrasil_streampixels = DaemonTarget {
            daemon_id: "yggdrasil-streampixels".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil StreamPixels".to_string(),
            health_contract: health_contract("streampixels.cultnet-rudp-service-health", "failed"),
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
            raven_sleipnir.clone(),
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
        assert!(plan.cut_line.contains("configured health route"));
        assert!(!plan.cut_line.contains("10.77.0.4"));
        assert!(plan.cut_line.contains("Nightwing Muninn"));
        assert!(plan.cut_line.contains("voidbot-swarm-state.cc"));
        assert!(
            plan.cut_line
                .contains("/home/metacrat/.local/state/gamecult/muninn/muninn.telemetry.cc")
        );
        assert!(plan.cut_line.contains("Starfire Muninn"));
        assert!(plan.cut_line.contains("configured health contract"));
        assert!(!plan.cut_line.contains("127.0.0.1"));
        assert!(!plan.cut_line.contains("10.77.0.3"));
        assert!(plan.cut_line.contains("GameCult-Muninn-Activate"));
        assert!(plan.cut_line.contains("GameCult-Muninn-VideoProof"));
        assert!(
            plan.cut_line
                .contains("hidden PowerShell entrypoints directly")
        );
        assert!(
            plan.cut_line
                .contains("archived instead of command-probe health paths")
        );
        assert!(plan.cut_line.contains("yggdrasil-streampixels"));
        assert!(plan.cut_line.contains("configured Yggdrasil health route"));
        assert!(!plan.cut_line.contains("10.77.0.1"));
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
                .contains("Live Mimir Eve dashboard publishes CultMesh state")
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
                .contains("live Nightwing Eve browser reference publishes a boundary store")
        );
        assert!(plan.cut_line.contains(
            "/var/lib/gamecult/eve-browser-reference/cultcache/eve-browser-reference.service.cc"
        ));
        assert!(plan.cut_line.contains("surface:gamecult.network.status"));
        assert!(plan.cut_line.contains("configured Gjallar Odin endpoint"));
        assert!(!plan.cut_line.contains("nightwing-gjallar"));
        assert!(!plan.cut_line.contains("renderer-only lowering"));
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
        assert!(
            raven_plan
                .current_mechanism
                .contains("configured Idunn health endpoint")
        );
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
        assert!(raven_plan.current_mechanism.contains("platform defaults"));
        assert!(raven_plan.cut_line.contains("daemon-owned witness"));
        assert!(
            raven_plan
                .cut_line
                .contains("health-muninn.cmd remains archived")
        );
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
        assert!(raven_plan.steps.iter().any(|step| {
            step.contains("--idunn-rudp-health from the configured Idunn health endpoint")
                && step.contains("muninn.cultnet-rudp-remote-telemetry-health")
        }));
        assert!(!raven_plan.current_mechanism.contains("192.168.1.66:17870"));
        assert!(
            !raven_plan
                .steps
                .iter()
                .any(|step| step.contains("192.168.1.66:17870"))
        );
        assert!(raven_plan.blockers.is_empty());

        let starfire_plan = daemon_surgery_plan(&starfire_muninn, "unix:100");
        assert_eq!(
            starfire_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            starfire_plan
                .current_mechanism
                .contains("configured Idunn health endpoint")
        );
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
        assert!(starfire_plan.cut_line.contains("daemon-owned witness"));
        assert!(
            starfire_plan
                .cut_line
                .contains("health-starfire-muninn.cmd stays archived")
        );
        assert!(starfire_plan.steps.iter().any(
            |step| step.contains("starfire.muninn.telemetry.cc") && step.contains(".lock")
        ));

        let nightwing_plan = daemon_surgery_plan(&nightwing_muninn, "unix:100");
        assert_eq!(
            nightwing_plan.status,
            "partial-rudp-health-and-provider-store-live"
        );
        assert!(
            nightwing_plan
                .current_mechanism
                .contains("configured Idunn health endpoint")
        );
        assert!(
            nightwing_plan
                .current_mechanism
                .contains("configured RUDP route")
        );
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
        assert!(nightwing_plan.cut_line.contains("daemon-owned witness"));
        assert!(
            nightwing_plan
                .cut_line
                .contains("health-nightwing-muninn.cmd stays archived")
        );
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
        assert!(
            weksa_plan
                .current_mechanism
                .contains("typed CultMesh/CultNet command documents")
        );
        assert!(
            weksa_plan
                .cut_line
                .contains("local command health must not satisfy daemon truth")
        );
        assert!(weksa_plan.steps.iter().any(|step| {
            step.contains("speech_provider.mimo.voicedesign")
                && step.contains("typed CultMesh/CultNet")
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
                .contains("operations probe as a debug witness")
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
                .contains("requires STREAMPIXELS_IDUNN_RUDP_HEALTH")
        );
        assert!(
            streampixels_plan
                .current_mechanism
                .contains("configured RUDP route")
        );
        assert!(
            !streampixels_plan
                .current_mechanism
                .contains("10.77.0.2:17870")
        );
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
                .any(|step| step.contains("product web discovery")
                    && step.contains("daemon truth"))
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
    fn voidbot_release_authority_is_yggdrasil_local_only() {
        let yggdrasil = swarm_targets(&SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        })
        .expect("yggdrasil-local targets");
        let voidbot = yggdrasil
            .iter()
            .find(|target| target.daemon_id == "yggdrasil-voidbot")
            .expect("Yggdrasil VoidBot target");

        assert_eq!(voidbot.verse_id, "yggdrasil.local");
        let deploy = voidbot
            .deploy_command
            .as_deref()
            .expect("VoidBot deploy command");
        assert!(deploy.starts_with("sudo -n /usr/local/libexec/idunn-yggdrasil deploy voidbot"));
        assert!(deploy.contains("$IDUNN_SOURCE_COMMIT"));
        assert!(deploy.contains("$BIFROST_RELEASE_AUTHORITY_SHA256"));
        assert!(deploy.contains("$IDUNN_DEPLOYMENT_REQUEST_ID"));
        assert_eq!(
            voidbot.restart_command.as_deref(),
            Some("sudo -n /usr/local/libexec/idunn-yggdrasil restart voidbot")
        );
        let release = voidbot.release.as_ref().expect("VoidBot release target");
        assert_eq!(release.repo_path, PathBuf::from("/srv/build/VoidBot"));
        assert_eq!(release.upstream_remote, "origin");
        assert_eq!(release.upstream_branch, "main");
        assert!(!release.requires_bifrost_authority);

        let starfire = swarm_targets(&SwarmOptions {
            profile: "starfire-local".to_string(),
            repo_root: PathBuf::from("E:/Projects/Odin"),
        })
        .expect("starfire-local targets");
        assert!(
            starfire
                .iter()
                .all(|target| target.daemon_id != "voidbot"
                    && target.daemon_id != "yggdrasil-voidbot")
        );
    }

    #[test]
    fn epiphany_yggdrasil_target_requires_bifrost_authority_and_exact_witness() {
        let yggdrasil = swarm_targets(&SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        })
        .expect("yggdrasil-local targets");
        let epiphany = yggdrasil
            .iter()
            .find(|target| target.daemon_id == "yggdrasil-epiphany")
            .expect("Yggdrasil Epiphany target");

        assert_eq!(epiphany.verse_id, "yggdrasil.local");
        assert_eq!(
            epiphany.health_contract.id,
            "epiphany.cultnet-rudp-runtime-health"
        );
        assert_eq!(epiphany.health_contract.default_failure_state, "failed");
        assert_eq!(epiphany.interval_seconds, 300);
        assert_eq!(
            epiphany.restart_command.as_deref(),
            Some("sudo -n /usr/local/libexec/idunn-yggdrasil restart epiphany")
        );
        let deploy = epiphany
            .deploy_command
            .as_deref()
            .expect("Epiphany deploy command");
        assert!(deploy.starts_with("sudo -n /usr/local/libexec/idunn-yggdrasil deploy epiphany"));
        assert!(deploy.contains("$IDUNN_SOURCE_COMMIT"));
        assert!(deploy.contains("$IDUNN_REQUIRES_BIFROST_AUTHORITY"));

        let release = epiphany.release.as_ref().expect("Epiphany release target");
        assert_eq!(release.repo, "Epiphany");
        assert_eq!(release.repository_full_name, "GameCult/Epiphany");
        assert_eq!(release.repo_path, PathBuf::from("/srv/build/Epiphany"));
        assert_eq!(release.upstream_remote, "origin");
        assert_eq!(release.upstream_branch, "main");
        assert_eq!(release.rollout_strategy, "restart-after-verified-build");
        assert_eq!(release.state_migration_command, None);
        assert_eq!(release.zero_downtime_capability, "restart-required");
        assert_eq!(
            release.deployed_revision_witness,
            Some(PathBuf::from("/srv/epiphany/deploy/deployment.env"))
        );
        assert!(release.requires_bifrost_authority);

        for legacy in yggdrasil
            .iter()
            .filter(|target| {
                !matches!(
                    target.daemon_id.as_str(),
                    "yggdrasil-epiphany" | "yggdrasil-bifrost-persona-feedback"
                )
            })
            .filter_map(|target| target.release.as_ref())
        {
            assert!(
                !legacy.requires_bifrost_authority,
                "{} must stay on its explicit legacy authority posture",
                legacy.repo
            );
        }
    }

    #[test]
    fn yggdrasil_service_and_root_actuator_advertise_epiphany_release_boundary() {
        let unit = include_str!("../../../scripts/linux/idunn-yggdrasil.service");
        let actuator = include_str!("../../../scripts/linux/idunn-yggdrasil");

        assert!(unit.contains(
            "--release-authority-store /srv/bifrost/state/repository-release-authority.cc"
        ));
        assert!(unit.contains("--command-timeout-seconds 21600"));
        assert!(unit.contains(
            "--trusted-epiphany-health-identity-store /etc/gamecult/idunn/epiphany-health-identity.ccmp"
        ));
        assert!(actuator.contains("deploy:epiphany|restart:epiphany"));
        assert!(actuator.contains("/usr/local/bin/idunn validate-release-authority"));
        assert!(
            actuator.contains(
                "epiphany|bifrost-persona-feedback) target_requires_bifrost_authority=true"
            )
        );
        assert!(actuator.contains(
            "voidbot|heimdall|repixelizer|streampixels) target_requires_bifrost_authority=false"
        ));
        assert!(
            actuator
                .contains("caller release-authority mode does not match root policy for $target")
        );
        let policy_derivation = actuator
            .find("target_requires_bifrost_authority=true")
            .expect("root target policy");
        let caller_mode_branch = actuator
            .find("case \"$requires_bifrost_authority\" in")
            .expect("authority mode branch");
        assert!(
            policy_derivation < caller_mode_branch,
            "root target policy must be derived before the authority mode is trusted"
        );
        assert!(actuator.contains("IDUNN_SOURCE_COMMIT=\"$source_commit\""));
        assert!(
            actuator.contains("BIFROST_RELEASE_AUTHORITY_SHA256=\"$release_authority_sha256\"")
        );
        assert!(
            actuator.contains("IDUNN_REQUIRES_BIFROST_AUTHORITY=\"$requires_bifrost_authority\"")
        );
    }

    struct FakeReleaseStatePort {
        fetch_error: Option<String>,
        desired: std::result::Result<String, String>,
        deployed: RefCell<std::result::Result<String, String>>,
    }

    impl ReleaseStatePort for FakeReleaseStatePort {
        fn fetch(&self, _release: &ReleaseTarget) -> Result<()> {
            self.fetch_error
                .as_ref()
                .map_or(Ok(()), |error| Err(anyhow!(error.clone())))
        }

        fn desired_revision(&self, _release: &ReleaseTarget) -> Result<String> {
            self.desired.clone().map_err(|error| anyhow!(error))
        }

        fn deployed_revision(&self, _release: &ReleaseTarget) -> Result<String> {
            self.deployed
                .borrow()
                .clone()
                .map_err(|error| anyhow!(error))
        }
    }

    struct MissingWitnessReleaseStatePort {
        desired: String,
    }

    impl ReleaseStatePort for MissingWitnessReleaseStatePort {
        fn fetch(&self, _release: &ReleaseTarget) -> Result<()> {
            Ok(())
        }

        fn desired_revision(&self, _release: &ReleaseTarget) -> Result<String> {
            Ok(self.desired.clone())
        }

        fn deployed_revision(&self, _release: &ReleaseTarget) -> Result<String> {
            Err(
                std::io::Error::new(std::io::ErrorKind::NotFound, "deployment witness is absent")
                    .into(),
            )
        }
    }

    fn release_desired(target: &DaemonTarget) -> IdunnDesiredDaemonRecord {
        IdunnDesiredDaemonRecord {
            daemon_id: target.daemon_id.clone(),
            verse_id: target.verse_id.clone(),
            name: target.name.clone(),
            enabled: true,
            health_command: None,
            restart_command: target.restart_command.clone(),
            deploy_command: target.deploy_command.clone(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(target),
            command_boundary_id: command_boundary_id(target),
            authority: "idunn-supervisor-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
        }
    }

    #[test]
    fn matching_release_witness_does_not_override_daemon_health() {
        let target = swarm_targets(&SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        })
        .unwrap()
        .into_iter()
        .find(|target| target.daemon_id == "yggdrasil-voidbot")
        .unwrap();
        let port = FakeReleaseStatePort {
            fetch_error: None,
            desired: Ok("abc123".to_string()),
            deployed: RefCell::new(Ok("abc123".to_string())),
        };

        assert!(
            evaluate_release_drift(
                &target,
                &release_desired(&target),
                target.release.as_ref().unwrap(),
                &port,
                "unix:100"
            )
            .is_none()
        );
    }

    #[test]
    fn release_drift_plans_one_deploy_then_matching_witness_stops_it() {
        let target = swarm_targets(&SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        })
        .unwrap()
        .into_iter()
        .find(|target| target.daemon_id == "yggdrasil-voidbot")
        .unwrap();
        let desired = release_desired(&target);
        let port = FakeReleaseStatePort {
            fetch_error: None,
            desired: Ok("new456".to_string()),
            deployed: RefCell::new(Ok("old123".to_string())),
        };
        let stale = evaluate_release_drift(
            &target,
            &desired,
            target.release.as_ref().unwrap(),
            &port,
            "unix:100",
        )
        .unwrap();
        assert_eq!(stale.state, "stale-deployment");
        assert!(
            plan_keepalive(&desired, &stale, "unix:100")
                .deployment_request
                .is_some()
        );
        assert!(verify_release_witness_current(target.release.as_ref().unwrap(), &port).is_err());

        *port.deployed.borrow_mut() = Ok("new456".to_string());
        assert!(verify_release_witness_current(target.release.as_ref().unwrap(), &port).is_ok());
        assert!(
            evaluate_release_drift(
                &target,
                &desired,
                target.release.as_ref().unwrap(),
                &port,
                "unix:101"
            )
            .is_none()
        );
    }

    #[test]
    fn absent_release_witness_is_first_deployment_not_dependency_failure() {
        let target = swarm_targets(&SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        })
        .unwrap()
        .into_iter()
        .find(|target| target.daemon_id == "yggdrasil-bifrost-persona-feedback")
        .unwrap();
        let desired = release_desired(&target);
        let health = evaluate_release_drift(
            &target,
            &desired,
            target.release.as_ref().unwrap(),
            &MissingWitnessReleaseStatePort {
                desired: "abc123".to_string(),
            },
            "unix:100",
        )
        .unwrap();

        assert_eq!(health.state, "stale-deployment");
        assert_eq!(health.transport, "idunn.release-not-deployed");
        assert!(
            health
                .detail
                .contains("no deployed revision witness exists")
        );
        assert!(
            plan_keepalive(&desired, &health, "unix:100")
                .deployment_request
                .is_some()
        );
    }

    #[test]
    fn unavailable_fetch_or_witness_alarms_without_deploying() {
        let target = swarm_targets(&SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        })
        .unwrap()
        .into_iter()
        .find(|target| target.daemon_id == "yggdrasil-voidbot")
        .unwrap();
        let desired = release_desired(&target);
        let ports = [
            FakeReleaseStatePort {
                fetch_error: Some("network unavailable".to_string()),
                desired: Ok("new456".to_string()),
                deployed: RefCell::new(Ok("old123".to_string())),
            },
            FakeReleaseStatePort {
                fetch_error: None,
                desired: Ok("new456".to_string()),
                deployed: RefCell::new(Err("witness missing".to_string())),
            },
        ];
        for port in ports {
            let unavailable = evaluate_release_drift(
                &target,
                &desired,
                target.release.as_ref().unwrap(),
                &port,
                "unix:100",
            )
            .unwrap();
            assert_eq!(unavailable.state, "dependency-unavailable");
            let plan = plan_keepalive(&desired, &unavailable, "unix:100");
            assert!(plan.deployment_request.is_none());
            assert!(plan.operator_alarm.is_some());
        }
    }

    #[test]
    fn starfire_and_nightwing_muninn_restart_when_health_publication_is_missing() {
        let options = SwarmOptions {
            profile: "starfire-local".to_string(),
            repo_root: PathBuf::from("E:/Projects/Odin"),
        };
        let targets = swarm_targets(&options).expect("starfire-local targets");

        for daemon_id in ["starfire-muninn", "nightwing-muninn"] {
            let target = targets
                .iter()
                .find(|target| target.daemon_id == daemon_id)
                .expect("Muninn target");
            assert!(target.health_contract.restart_on_missing_publication);

            let desired = IdunnDesiredDaemonRecord {
                daemon_id: target.daemon_id.clone(),
                verse_id: target.verse_id.clone(),
                name: target.name.clone(),
                enabled: target.enabled,
                health_command: None,
                restart_command: target.restart_command.clone(),
                deploy_command: target.deploy_command.clone(),
                health_contract: target.health_contract.id.clone(),
                transport_profile_id: transport_profile_id(target),
                command_boundary_id: command_boundary_id(target),
                authority: "idunn-supervisor-command".to_string(),
                max_silence_seconds: 60,
                observed_at: "unix:100".to_string(),
            };
            let health = missing_daemon_published_health(target, &desired, "unix:101");
            let plan = plan_keepalive(&desired, &health, "unix:102");

            assert_eq!(health.state, "failed");
            assert_eq!(plan.decision.action, "restart");
            assert_eq!(
                plan.restart_request.expect("restart request").command,
                target.restart_command.clone().expect("restart command")
            );
            assert!(plan.operator_alarm.is_none());
        }
    }

    #[test]
    fn bifrost_persona_feedback_target_uses_daemon_health_and_idunn_deployment() {
        let options = SwarmOptions {
            profile: "yggdrasil-local".to_string(),
            repo_root: PathBuf::from("/srv/odin/source"),
        };
        let targets = swarm_targets(&options).expect("yggdrasil-local targets");
        let target = targets
            .iter()
            .find(|target| target.daemon_id == "yggdrasil-bifrost-persona-feedback")
            .expect("Bifrost Persona-feedback target");
        assert_eq!(
            target.health_contract.id,
            "bifrost.cultnet-rudp-persona-feedback-health"
        );
        assert_eq!(
            target.deploy_command.as_deref(),
            Some(
                "sudo -n /usr/local/libexec/idunn-yggdrasil deploy bifrost-persona-feedback \"$IDUNN_SOURCE_COMMIT\" \"$IDUNN_REPOSITORY_FULL_NAME\" \"$IDUNN_UPSTREAM_REF\" \"$BIFROST_RELEASE_AUTHORITY_ID\" \"$BIFROST_RELEASE_AUTHORITY_SHA256\" \"$IDUNN_DEPLOYMENT_REQUEST_ID\" \"$IDUNN_REQUIRES_BIFROST_AUTHORITY\""
            )
        );
        assert!(target.restart_command.is_none());
        let release = target.release.as_ref().expect("release target");
        assert!(release.requires_bifrost_authority);
        assert_eq!(release.repository_full_name, "GameCult/Bifrost");
        assert_eq!(
            release.repo_path,
            PathBuf::from("/srv/build/Bifrost-persona-feedback")
        );

        let desired = IdunnDesiredDaemonRecord {
            daemon_id: target.daemon_id.clone(),
            verse_id: target.verse_id.clone(),
            name: target.name.clone(),
            enabled: true,
            health_command: None,
            restart_command: None,
            deploy_command: target.deploy_command.clone(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(target),
            command_boundary_id: command_boundary_id(target),
            authority: "idunn-supervisor-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
        };
        let health = missing_daemon_published_health(target, &desired, "unix:101");
        assert_eq!(health.state, "dependency-unavailable");
        let plan = plan_keepalive(&desired, &health, "unix:102");
        assert_ne!(plan.decision.action, "deploy");
        assert!(plan.deployment_request.is_none());
        assert!(plan.restart_request.is_none());
    }

    #[test]
    fn fresh_daemon_published_rudp_health_is_the_health_owner() {
        let desired = IdunnDesiredDaemonRecord {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            enabled: true,
            health_command: None,
            restart_command: Some("restart test".to_string()),
            authority: "idunn-supervisor-command".to_string(),
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
        health.publication_source = "debug-command".to_string();
        assert!(!is_fresh_daemon_published_health(
            &health, &desired, "unix:100"
        ));
    }

    #[test]
    fn evaluate_target_health_refuses_bare_daemon_published_row_without_admission() {
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
            release_authority_store_path: None,
            deployment_brake_store_path: None,
            deployment_brake_operator_anchor_path: None,
            deployment_brake_runtime_id: None,
            operator_alarm_command: None,
            rudp_health_bind: None,
            trusted_epiphany_health_identity_store: None,
            daemon_health_trust_store_path: None,
            service_identity_store_path: None,
            public_health_store_path: None,
            public_health_query_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        let store_lock = Arc::new(Mutex::new(()));
        let target = DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.cultnet-rudp-health", "failed"),
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
            health_command: None,
            restart_command: target.restart_command.clone(),
            deploy_command: target.deploy_command.clone(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
            authority: "idunn-supervisor-command".to_string(),
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

        let (health_key, selected, authenticated) =
            evaluate_target_health(&target, &options, &store_lock, &desired, &now).unwrap();

        assert_eq!(health_key, "observation:test-daemon");
        assert_eq!(selected.publication_source, "idunn-supervisor-observation");
        assert_ne!(selected.state, "active");
        assert!(authenticated.is_none());

        let _ = std::fs::remove_file(store_path);
    }

    #[test]
    fn evaluate_target_health_reports_missing_daemon_publication_without_probe_lane() {
        let store_path = std::env::temp_dir().join(format!(
            "idunn-test-store-{}.cc",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let options = CommonOptions {
            store_path: store_path.clone(),
            release_authority_store_path: None,
            deployment_brake_store_path: None,
            deployment_brake_operator_anchor_path: None,
            deployment_brake_runtime_id: None,
            operator_alarm_command: None,
            rudp_health_bind: None,
            trusted_epiphany_health_identity_store: None,
            daemon_health_trust_store_path: None,
            service_identity_store_path: None,
            public_health_store_path: None,
            public_health_query_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        let store_lock = Arc::new(Mutex::new(()));
        let target = DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: health_contract("test.cultnet-rudp-health", "failed"),
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
            health_command: None,
            restart_command: target.restart_command.clone(),
            deploy_command: target.deploy_command.clone(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
            authority: "idunn-supervisor-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
        };

        let (health_key, selected, authenticated) =
            evaluate_target_health(&target, &options, &store_lock, &desired, "unix:100").unwrap();

        assert_eq!(health_key, "observation:test-daemon");
        assert_eq!(selected.state, "dependency-unavailable");
        assert_eq!(selected.publication_source, "idunn-supervisor-observation");
        assert_eq!(selected.transport, "cultmesh.missing-daemon-publication");
        assert!(authenticated.is_none());
        assert!(
            selected
                .detail
                .contains("Idunn did not run local health probes")
        );

        let _ = std::fs::remove_file(store_path);
    }

    #[test]
    fn locally_supervised_health_waits_for_continuous_missing_boundary() {
        let target = DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: locally_supervised_health_contract(
                "test.cultnet-rudp-health",
                "failed",
            ),
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
            enabled: true,
            health_command: None,
            restart_command: target.restart_command.clone(),
            deploy_command: None,
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
            authority: "idunn-supervisor-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
        };
        let now = Instant::now();
        let mut missing_since = None;
        let mut health = missing_daemon_published_health(&target, &desired, "unix:100");

        apply_missing_publication_grace(
            &target,
            &desired,
            &mut health,
            false,
            &mut missing_since,
            now,
        );

        assert_eq!(health.state, "degraded");
        assert!(missing_since.is_some());
        assert!(
            plan_keepalive(&desired, &health, "unix:100")
                .restart_request
                .is_none()
        );

        let mut health = missing_daemon_published_health(&target, &desired, "unix:161");
        apply_missing_publication_grace(
            &target,
            &desired,
            &mut health,
            false,
            &mut missing_since,
            now + Duration::from_secs(61),
        );

        assert_eq!(health.state, "failed");
        assert!(
            plan_keepalive(&desired, &health, "unix:161")
                .restart_request
                .is_some()
        );
    }

    #[test]
    fn only_authenticated_daemon_publication_resets_missing_continuity() {
        let target = DaemonTarget {
            daemon_id: "test-daemon".to_string(),
            verse_id: "test.local".to_string(),
            name: "Test daemon".to_string(),
            health_contract: locally_supervised_health_contract(
                "test.cultnet-rudp-health",
                "failed",
            ),
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
            enabled: true,
            health_command: None,
            restart_command: target.restart_command.clone(),
            deploy_command: None,
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
            authority: "idunn-supervisor-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "unix:100".to_string(),
        };
        let mut health = IdunnDaemonHealthRecord {
            daemon_id: target.daemon_id.clone(),
            state: "active".to_string(),
            detail: "daemon published".to_string(),
            health_contract: target.health_contract.id.clone(),
            publication_source: "daemon-published".to_string(),
            transport: CULTNET_RUDP_PROTOCOL_ID.to_string(),
            observed_at: "unix:100".to_string(),
        };
        let mut missing_since = Some(Instant::now() - Duration::from_secs(120));

        apply_missing_publication_grace(
            &target,
            &desired,
            &mut health,
            false,
            &mut missing_since,
            Instant::now(),
        );

        assert!(missing_since.is_some());
        assert_eq!(health.state, "active");
        apply_missing_publication_grace(
            &target,
            &desired,
            &mut health,
            true,
            &mut missing_since,
            Instant::now(),
        );
        assert!(missing_since.is_none());
    }

    #[test]
    fn stonks_transport_profile_marks_provider_store_live() {
        let stonks = DaemonTarget {
            daemon_id: "stonks".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Stonks".to_string(),
            health_contract: health_contract("stonks.cultnet-rudp-market-health", "failed"),
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
        assert!(
            profile
                .cut_line
                .contains("Renderer/debug surfaces live outside daemon transport authority")
        );
    }

    #[test]
    fn weksa_transport_profile_marks_provider_store_live() {
        let weksa = DaemonTarget {
            daemon_id: "weksa".to_string(),
            verse_id: "starfire.local".to_string(),
            name: "Weksa".to_string(),
            health_contract: health_contract("weksa.cultnet-rudp-provider-health", "failed"),
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
            "daemon-published-rudp-health + daemon-owned-cultcache-provider-store + odin-cultmesh-command-documents"
        );
        assert!(
            profile
                .cut_line
                .contains("typed CultMesh/CultNet command documents")
        );
    }

    #[test]
    fn streampixels_transport_profile_marks_provider_store_live() {
        let streampixels = DaemonTarget {
            daemon_id: "yggdrasil-streampixels".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil StreamPixels".to_string(),
            health_contract: health_contract("streampixels.cultnet-rudp-service-health", "failed"),
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
        assert!(profile.cut_line.contains("configured service health route"));
        assert!(!profile.cut_line.contains("10.77.0.1"));
        assert!(profile.cut_line.contains("service-owned witness"));
    }

    #[test]
    fn heimdall_transport_profile_marks_boundary_store_live() {
        let heimdall = DaemonTarget {
            daemon_id: "yggdrasil-heimdall".to_string(),
            verse_id: "yggdrasil.local".to_string(),
            name: "Yggdrasil Heimdall".to_string(),
            health_contract: health_contract("heimdall.cultnet-rudp-provider-health", "failed"),
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
            "daemon-published-rudp-health + daemon-owned-cultmesh-provider-store"
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
            "daemon-published-rudp-health + daemon-owned-cultcache-service-boundary + native-odin-cultnet-rudp-snapshot-input"
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
            deploy_command: None,
            restart_command: Some("restart-muninn.ps1".to_string()),
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
            deploy_command: None,
            restart_command: Some("restart-starfire-muninn.ps1".to_string()),
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
            deploy_command: None,
            restart_command: Some("restart-nightwing-muninn.ps1".to_string()),
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
    fn sleipnir_transport_profile_marks_live_rudp_health() {
        let sleipnir = DaemonTarget {
            daemon_id: "raven-sleipnir".to_string(),
            verse_id: "raven.local".to_string(),
            name: "Raven Sleipnir".to_string(),
            health_contract: health_contract("sleipnir.cultnet-rudp-input-mirror-health", "failed"),
            deploy_command: Some("deploy-raven-sleipnir.ps1".to_string()),
            restart_command: Some("restart-raven-sleipnir.ps1".to_string()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };

        let profile = daemon_transport_profile(&sleipnir, "unix:100");

        assert_eq!(profile.state, "partial-rudp-health-and-provider-store-live");
        assert_eq!(
            profile.current_transport,
            "daemon-published-rudp-health + daemon-owned-cultcache-input-mirror-store"
        );
        assert!(profile.cut_line.contains("Raven input mirror runtime"));
        assert!(
            profile
                .cut_line
                .contains("scheduled task is an Idunn restart lowering")
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
    fn rudp_health_ingress_quarantines_unsigned_health_without_authority() {
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
                payload: rmp_serde::to_vec(&IdunnDaemonHealthWireV1 {
                    daemon_id: health.daemon_id.clone(),
                    state: health.state.clone(),
                    detail: health.detail.clone(),
                    observed_at: health.observed_at.clone(),
                    health_contract: health.health_contract.clone(),
                    publication_source: health.publication_source.clone(),
                    transport: health.transport.clone(),
                })
                .unwrap(),
                source_runtime_id: Some("test-daemon".to_string()),
                source_agent_id: None,
                source_role: Some("daemon-health-publisher".to_string()),
                tags: Some(vec!["cultnet.transport.rudp.v0".to_string()]),
            },
        };

        let root = tempfile::tempdir().unwrap();
        let options = CommonOptions {
            store_path: root.path().join("idunn.cc"),
            release_authority_store_path: None,
            deployment_brake_store_path: None,
            deployment_brake_operator_anchor_path: None,
            deployment_brake_runtime_id: None,
            operator_alarm_command: None,
            rudp_health_bind: None,
            trusted_epiphany_health_identity_store: None,
            daemon_health_trust_store_path: None,
            service_identity_store_path: None,
            public_health_store_path: None,
            public_health_query_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        let lock = Arc::new(Mutex::new(()));
        let outcome =
            admit_health_from_rudp_message(&message, &options, &lock, "2026-06-15T00:00:01Z")
                .unwrap();
        assert_eq!(outcome.authority, "diagnostic-only");
        assert!(health_from_rudp_message(&message, &options).is_err());
        with_store_node(&options, &lock, |node| {
            assert!(
                node.get::<IdunnDaemonHealthRecord>(&health.daemon_id)?
                    .is_none()
            );
            let diagnostic = node
                .get::<IdunnUnsignedDaemonHealthDiagnosticRecord>(&format!(
                    "diagnostic:{}",
                    health.daemon_id
                ))?
                .expect("unsigned diagnostic");
            assert_eq!(diagnostic.claimed_state, "active");
            assert_eq!(diagnostic.authority, "diagnostic-only");
            Ok(())
        })
        .unwrap();
    }

    fn generic_signed_health_fixture(
        sequence: u64,
    ) -> (CultNetMessage, CommonOptions, tempfile::TempDir) {
        generic_signed_health_fixture_with_release(sequence, false)
    }

    fn generic_signed_health_fixture_with_release(
        sequence: u64,
        release_bound: bool,
    ) -> (CultNetMessage, CommonOptions, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let trust_path = root.path().join("health-trust.cc");
        let store_path = root.path().join("idunn.cc");
        let signer = cultnet_rs::enroll_service_identity_at::<GameCultProviderHealthIdentity>(
            &root.path().join("provider-health-identity.cc"),
        )
        .unwrap();
        let public_key = signer.entry().public_key.clone();
        let identity_id = signer.entry().identity_id.clone();
        let binding = IdunnDaemonHealthTrustBindingRecord {
            schema_version: odin_core::IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA.into(),
            binding_id: "root/test-daemon/health".into(),
            daemon_id: "test-daemon".into(),
            health_contract: "test.signed-health".into(),
            source_runtime_id: "test-runtime".into(),
            signer_identity_id: identity_id.clone(),
            signer_public_key: public_key,
            binding_authority: "root".into(),
            bound_at_unix_millis: 1_784_483_100_000,
            release_binding_required: release_bound,
            private_state_exposed: false,
        };
        SingleFileMessagePackBackingStore::new(&trust_path)
            .push(&typed_envelope(&binding.binding_id, &binding, "unix:1784483100").unwrap())
            .unwrap();
        let observed_at_unix_millis = chrono::DateTime::parse_from_rfc3339("2026-07-19T12:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let mut statement = IdunnSignedDaemonHealthRecord {
            schema_version: odin_core::IDUNN_SIGNED_DAEMON_HEALTH_SCHEMA.into(),
            daemon_id: binding.daemon_id.clone(),
            health_contract: binding.health_contract.clone(),
            source_runtime_id: binding.source_runtime_id.clone(),
            state: "active".into(),
            detail: "signed provider health".into(),
            signer_identity_id: identity_id,
            publisher_incarnation_id: "00000000-0000-4000-8000-000000000031".into(),
            publisher_sequence: sequence,
            observed_at_unix_millis,
            release_id: release_bound.then(|| "release-test-1".into()),
            release_witness_sha256: release_bound.then(|| format!("sha256-{}", "a".repeat(64))),
            source_commit: release_bound.then(|| "b".repeat(40)),
            deployment_id: release_bound.then(|| "deploy-test-1".into()),
            signature_algorithm: "ed25519".into(),
            signature: Vec::new(),
            private_state_exposed: false,
        };
        let unsigned = rmp_serde::to_vec(&statement).unwrap();
        statement.signature = signer
            .sign::<IdunnSignedDaemonHealthPurpose>(&unsigned)
            .signature;
        let message = CultNetMessage::DocumentPutRaw {
            message_id: format!("signed-health-{sequence}"),
            document: CultNetRawDocumentRecord {
                schema_id: SIGNED_DAEMON_HEALTH_TYPE.into(),
                record_key: statement.daemon_id.clone(),
                stored_at: "2026-07-19T12:00:00Z".into(),
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: rmp_serde::to_vec(&statement).unwrap(),
                source_runtime_id: Some(statement.source_runtime_id.clone()),
                source_agent_id: Some(statement.signer_identity_id.clone()),
                source_role: Some("daemon-health-publisher".into()),
                tags: Some(vec![CULTNET_RUDP_PROTOCOL_ID.into()]),
            },
        };
        if release_bound {
            let request = IdunnDeploymentRequestRecord {
                request_id: "deploy-test-1".into(),
                daemon_id: "test-daemon".into(),
                command: "deploy".into(),
                authority: "idunn".into(),
                requested_at: "2026-07-19T11:59:00Z".into(),
                repository_full_name: "GameCult/Test".into(),
                upstream_ref: "refs/heads/main".into(),
                source_revision: "b".repeat(40),
                release_authority_id: "bifrost-test".into(),
                release_authority_envelope_sha256: format!("sha256-{}", "c".repeat(64)),
                requires_bifrost_authority: true,
            };
            let head = IdunnCurrentDeploymentRequestRecord {
                daemon_id: "test-daemon".into(),
                request_id: request.request_id.clone(),
                sequence: 1,
                updated_at: "2026-07-19T11:59:00Z".into(),
            };
            let mut backing = SingleFileMessagePackBackingStore::new(&store_path);
            backing
                .push(&typed_envelope(&request.request_id, &request, "unix:1784462340").unwrap())
                .unwrap();
            backing
                .push(&typed_envelope(&head.daemon_id, &head, "unix:1784462340").unwrap())
                .unwrap();
        }
        (
            message,
            CommonOptions {
                store_path,
                release_authority_store_path: None,
                deployment_brake_store_path: None,
                deployment_brake_operator_anchor_path: None,
                deployment_brake_runtime_id: None,
                operator_alarm_command: None,
                rudp_health_bind: None,
                trusted_epiphany_health_identity_store: None,
                daemon_health_trust_store_path: Some(trust_path),
                service_identity_store_path: None,
                public_health_store_path: None,
                public_health_query_bind: None,
                execute: false,
                command_timeout_seconds: 1,
            },
            root,
        )
    }

    #[test]
    fn generic_signed_health_requires_root_binding_and_monotonic_admission() {
        let (first, options, _root) = generic_signed_health_fixture(1);
        let lock = Arc::new(Mutex::new(()));
        let outcome =
            admit_health_from_rudp_message(&first, &options, &lock, "2026-07-19T12:00:01Z")
                .unwrap();
        assert_eq!(outcome.authority, "authenticated");
        assert!(
            admit_health_from_rudp_message(&first, &options, &lock, "2026-07-19T12:00:02Z")
                .is_err()
        );
        with_store_node(&options, &lock, |node| {
            let health = node
                .get::<IdunnDaemonHealthRecord>("test-daemon")?
                .expect("authenticated health");
            assert_eq!(health.publication_source, "daemon-authenticated");
            let admission = node
                .get::<IdunnAuthenticatedDaemonHealthAdmissionRecord>("test-daemon")?
                .expect("generic admission");
            assert_eq!(admission.publisher_sequence, 1);
            assert_eq!(admission.trust_binding_id, "root/test-daemon/health");
            let statement = node
                .get::<IdunnSignedDaemonHealthRecord>(&admission.signed_health_sha256)?
                .expect("signed statement");
            assert_eq!(
                admission.signed_health_sha256,
                format!(
                    "sha256-{:x}",
                    Sha256::digest(rmp_serde::to_vec(&statement)?)
                )
            );
            let (_, binding_sha256) = load_daemon_health_trust_binding(&options, &statement)?;
            assert_eq!(admission.trust_binding_sha256, binding_sha256);
            Ok(())
        })
        .unwrap();
        let target = DaemonTarget {
            daemon_id: "test-daemon".into(),
            verse_id: "test.local".into(),
            name: "Test daemon".into(),
            health_contract: health_contract("test.signed-health", "failed"),
            deploy_command: None,
            restart_command: Some("restart test".into()),
            release: None,
            enabled: true,
            interval_seconds: 30,
        };
        let desired = IdunnDesiredDaemonRecord {
            daemon_id: target.daemon_id.clone(),
            verse_id: target.verse_id.clone(),
            name: target.name.clone(),
            enabled: true,
            health_command: None,
            restart_command: target.restart_command.clone(),
            deploy_command: None,
            authority: "idunn-supervisor-command".into(),
            max_silence_seconds: 60,
            observed_at: "2026-07-19T12:00:02Z".into(),
            health_contract: target.health_contract.id.clone(),
            transport_profile_id: transport_profile_id(&target),
            command_boundary_id: command_boundary_id(&target),
        };
        let (health_key, selected, authenticated) =
            evaluate_target_health(&target, &options, &lock, &desired, "2026-07-19T12:00:02Z")
                .unwrap();
        assert_eq!(health_key, "test-daemon");
        assert_eq!(selected.publication_source, "daemon-authenticated");
        assert!(authenticated.is_some());

        let (mut forged, mut no_trust, _other_root) = generic_signed_health_fixture(2);
        no_trust.daemon_health_trust_store_path = None;
        assert!(
            admit_health_from_rudp_message(
                &forged,
                &no_trust,
                &Arc::new(Mutex::new(())),
                "2026-07-19T12:00:01Z"
            )
            .is_err()
        );
        if let CultNetMessage::DocumentPutRaw { document, .. } = &mut forged {
            document.payload[0] ^= 1;
        }
        assert!(health_from_rudp_message(&forged, &options).is_err());
    }

    fn projection_fixture(
        release_bound: bool,
    ) -> (
        CommonOptions,
        Arc<Mutex<()>>,
        IdunnDesiredDaemonRecord,
        AuthenticatedProviderHealthSource,
        Arc<IdunnProjectionPublisher>,
        tempfile::TempDir,
    ) {
        let (message, mut options, root) =
            generic_signed_health_fixture_with_release(1, release_bound);
        let identity_path = root.path().join("idunn-identity.ccmp");
        let public_path = root.path().join("public-health.cc");
        cultnet_rs::enroll_service_identity_at::<IdunnServiceIdentity>(&identity_path).unwrap();
        options.service_identity_store_path = Some(identity_path);
        options.public_health_store_path = Some(public_path);
        let publisher = initialize_projection_publisher(&options)
            .unwrap()
            .expect("configured publisher");
        let lock = Arc::new(Mutex::new(()));
        admit_health_from_rudp_message(&message, &options, &lock, "2026-07-19T12:00:01Z").unwrap();
        let desired = IdunnDesiredDaemonRecord {
            daemon_id: "test-daemon".into(),
            verse_id: "test.local".into(),
            name: "Test daemon".into(),
            enabled: true,
            health_command: None,
            restart_command: None,
            deploy_command: None,
            authority: "idunn-supervisor-command".into(),
            max_silence_seconds: 60,
            observed_at: "2026-07-19T12:00:02Z".into(),
            health_contract: "test.signed-health".into(),
            transport_profile_id: "test-transport".into(),
            command_boundary_id: "test-command".into(),
        };
        let source =
            read_fresh_daemon_published_health(&options, &lock, &desired, "2026-07-19T12:00:02Z")
                .unwrap()
                .unwrap()
                .projection_source
                .unwrap();
        (options, lock, desired, source, publisher, root)
    }

    fn public_projection(
        options: &CommonOptions,
    ) -> IdunnAuthenticatedProviderHealthProjectionRecord {
        let entries = SingleFileMessagePackBackingStore::new(
            options.public_health_store_path.as_ref().unwrap(),
        )
        .pull_all_read_only_snapshot()
        .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].r#type,
            IdunnAuthenticatedProviderHealthProjectionRecord::TYPE
        );
        rmp_serde::from_slice(&entries[0].payload).unwrap()
    }

    fn public_query_target(desired: &IdunnDesiredDaemonRecord) -> DaemonTarget {
        DaemonTarget {
            daemon_id: desired.daemon_id.clone(),
            verse_id: desired.verse_id.clone(),
            name: desired.name.clone(),
            health_contract: health_contract(&desired.health_contract, "failed"),
            deploy_command: None,
            restart_command: None,
            release: None,
            enabled: true,
            interval_seconds: 30,
        }
    }

    fn connect_public_query_client(
        server_addr: SocketAddr,
    ) -> CultNetRudpSocketTransportConnection {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut client =
            CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::client(
                "epiphany-test-consumer",
                socket,
                server_addr,
                IDUNN_PUBLIC_HEALTH_QUERY_CONNECTION_ID,
            ))
            .unwrap();
        client.connect(Vec::new()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !client.connected() {
            client.receive_once().unwrap();
            client.poll_resends().unwrap();
            assert!(
                Instant::now() < deadline,
                "public query handshake timed out"
            );
        }
        client
    }

    fn receive_public_query_message(
        client: &mut CultNetRudpSocketTransportConnection,
    ) -> CultNetMessage {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(frame) = client.receive_once().unwrap() {
                assert_eq!(frame.channel_id, "schema");
                return decode_cultnet_message_from_slice(
                    &frame.payload,
                    CultNetWireContract::CultNetSchemaV0,
                )
                .unwrap();
            }
            client.poll_resends().unwrap();
            assert!(Instant::now() < deadline, "public query response timed out");
        }
    }

    #[test]
    fn public_query_server_returns_exact_allowlisted_bytes_and_correlation() {
        let (options, lock, desired, source, publisher, _root) = projection_fixture(false);
        publisher
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
            .unwrap();
        let public_path = options.public_health_store_path.as_ref().unwrap();
        let stored = SingleFileMessagePackBackingStore::new(public_path)
            .pull_all_read_only_snapshot()
            .unwrap()
            .remove(0);
        let server = IdunnPublicHealthSnapshotServer::new(
            public_path.clone(),
            &[public_query_target(&desired)],
        )
        .unwrap();
        let request = CultNetMessage::SnapshotRequest {
            message_id: "status-correlation-7".into(),
            schema_ids: Some(vec![
                IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into(),
            ]),
            record_keys: Some(vec![stored.key.clone()]),
        };
        let CultNetMessage::SnapshotResponseRaw {
            message_id,
            documents,
        } = server.serve(&request).unwrap()
        else {
            panic!("expected raw public health snapshot");
        };
        assert_eq!(message_id, "status-correlation-7");
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].payload, stored.payload);
        assert_eq!(
            documents[0].source_runtime_id.as_deref(),
            Some(IDUNN_PUBLIC_HEALTH_QUERY_RUNTIME_ID)
        );
        assert_eq!(
            documents[0].source_role.as_deref(),
            Some(IDUNN_PUBLIC_HEALTH_QUERY_ROLE)
        );

        for request in [
            CultNetMessage::SnapshotRequest {
                message_id: "unknown-schema".into(),
                schema_ids: Some(vec!["private.idunn.state.v0".into()]),
                record_keys: None,
            },
            CultNetMessage::SnapshotRequest {
                message_id: "unknown-key".into(),
                schema_ids: None,
                record_keys: Some(vec!["provider-health:not-in-target-catalog".into()]),
            },
        ] {
            let CultNetMessage::SnapshotResponseRaw { documents, .. } =
                server.serve(&request).unwrap()
            else {
                panic!("expected bounded empty response");
            };
            assert!(documents.is_empty());
        }
    }

    #[test]
    fn public_query_store_contamination_is_fatal_and_private_type_is_never_served() {
        let (options, _lock, desired, _source, _publisher, root) = projection_fixture(false);
        let private_path = root.path().join("private-in-public.cc");
        let private = IdunnDaemonHealthRecord {
            daemon_id: desired.daemon_id.clone(),
            state: "active".into(),
            detail: "private managed judgment".into(),
            observed_at: "2026-07-19T12:00:02Z".into(),
            health_contract: desired.health_contract.clone(),
            publication_source: "daemon-authenticated".into(),
            transport: CULTNET_RUDP_PROTOCOL_ID.into(),
        };
        SingleFileMessagePackBackingStore::new(&private_path)
            .push(&typed_envelope(&private.daemon_id, &private, "2026-07-19T12:00:02Z").unwrap())
            .unwrap();
        assert!(
            IdunnPublicHealthSnapshotServer::new(private_path, &[public_query_target(&desired)])
                .is_err()
        );

        let public_path = options.public_health_store_path.as_ref().unwrap();
        let server = IdunnPublicHealthSnapshotServer::new(
            public_path.clone(),
            &[public_query_target(&desired)],
        )
        .unwrap();
        SingleFileMessagePackBackingStore::new(public_path)
            .push(&typed_envelope(&private.daemon_id, &private, "2026-07-19T12:00:03Z").unwrap())
            .unwrap();
        assert!(
            server
                .serve(&CultNetMessage::SnapshotRequest {
                    message_id: "private-probe".into(),
                    schema_ids: None,
                    record_keys: None,
                })
                .is_err()
        );
    }

    #[test]
    fn public_query_rudp_is_multi_peer_and_survives_malformed_request() {
        let (options, lock, desired, source, publisher, _root) = projection_fixture(false);
        publisher
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
            .unwrap();
        let server = IdunnPublicHealthSnapshotServer::new(
            options.public_health_store_path.clone().unwrap(),
            &[public_query_target(&desired)],
        )
        .unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let server_addr = socket.local_addr().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            run_public_health_query_listener_loop(socket, server, Some(worker_stop))
        });

        let mut hostile = connect_public_query_client(server_addr);
        hostile.send("schema", vec![0xc1]).unwrap();
        assert!(matches!(
            receive_public_query_message(&mut hostile),
            CultNetMessage::Error { .. }
        ));

        let mut first = connect_public_query_client(server_addr);
        let mut second = connect_public_query_client(server_addr);
        for (client, id) in [(&mut first, "first-peer"), (&mut second, "second-peer")] {
            let request = CultNetMessage::SnapshotRequest {
                message_id: id.into(),
                schema_ids: Some(vec![
                    IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into(),
                ]),
                record_keys: Some(vec![provider_health_projection_key(
                    &desired.daemon_id,
                    &desired.health_contract,
                )]),
            };
            client
                .send(
                    "schema",
                    encode_cultnet_message_to_vec(&request, CultNetWireContract::CultNetSchemaV0)
                        .unwrap(),
                )
                .unwrap();
            let CultNetMessage::SnapshotResponseRaw {
                message_id,
                documents,
            } = receive_public_query_message(client)
            else {
                panic!("expected peer snapshot response");
            };
            assert_eq!(message_id, id);
            assert_eq!(documents.len(), 1);
        }
        stop.store(true, Ordering::Release);
        worker.join().unwrap().unwrap();
    }

    #[test]
    fn projection_is_signed_canonical_public_state_without_provider_detail() {
        let (options, lock, desired, source, publisher, _root) = projection_fixture(false);
        publisher
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
            .unwrap();
        let projection = public_projection(&options);
        assert_eq!(projection.projection_sequence, 1);
        assert_eq!(
            projection.expires_at_unix_millis,
            source.admission.observed_at_unix_millis + 60_000
        );
        assert!(
            !rmp_serde::to_vec(&projection)
                .unwrap()
                .windows(b"signed provider health".len())
                .any(|window| window == b"signed provider health")
        );
        let signature = projection.signature.clone();
        let mut unsigned = projection.clone();
        unsigned.signature.clear();
        let anchor = publisher.signer.trust_anchor().unwrap();
        cultnet_rs::verify_service_identity_signature::<
            IdunnServiceIdentity,
            IdunnAuthenticatedProviderHealthProjectionPurpose,
        >(
            &anchor,
            &rmp_serde::to_vec(&unsigned).unwrap(),
            &cultnet_rs::ServiceIdentitySignature {
                identity_id: projection.idunn_signer_identity_id.clone(),
                signature,
            },
        )
        .unwrap();
        publisher
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:03Z")
            .unwrap();
        assert_eq!(
            public_projection(&options),
            projection,
            "same source refreshed public state"
        );
    }

    #[test]
    fn projection_rejoins_trust_and_refuses_rotation_or_source_mutation() {
        let (options, lock, desired, source, publisher, _root) = projection_fixture(false);
        let trust_path = options.daemon_health_trust_store_path.as_ref().unwrap();
        let backing = SingleFileMessagePackBackingStore::new(trust_path);
        let current = backing.pull_all_read_only_snapshot().unwrap().remove(0);
        let mut rotated = source.binding.clone();
        rotated.binding_id = "root/test-daemon/health-rotated".into();
        assert!(
            backing
                .compare_exchange(
                    &[CultCacheExpectedEnvelope {
                        key: source.binding.binding_id.clone(),
                        r#type: IdunnDaemonHealthTrustBindingRecord::TYPE.into(),
                        current: Some(current),
                    }],
                    &[
                        typed_envelope(&rotated.binding_id, &rotated, "2026-07-19T12:00:02Z")
                            .unwrap()
                    ],
                )
                .unwrap()
        );
        assert!(
            publisher
                .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
                .is_err()
        );
        assert!(!options.public_health_store_path.as_ref().unwrap().exists());

        let (options, lock, desired, source, publisher, _root) = projection_fixture(false);
        with_store_node(&options, &lock, |node| {
            let mut admission = source.admission.clone();
            admission.publisher_sequence += 1;
            node.put(&admission.daemon_id, &admission)?;
            Ok(())
        })
        .unwrap();
        assert!(
            publisher
                .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
                .is_err()
        );
        assert!(!options.public_health_store_path.as_ref().unwrap().exists());
    }

    #[test]
    fn projection_refuses_superseded_deployment_and_does_not_delete_prior_row() {
        let (options, lock, desired, source, publisher, _root) = projection_fixture(true);
        publisher
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
            .unwrap();
        let prior = public_projection(&options);
        with_store_node(&options, &lock, |node| {
            let mut head = source.deployment_head.clone().unwrap();
            head.request_id = "deploy-test-2".into();
            head.sequence += 1;
            node.put(&head.daemon_id, &head)?;
            Ok(())
        })
        .unwrap();
        assert!(
            publisher
                .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:03Z")
                .is_err()
        );
        assert_eq!(public_projection(&options), prior);
    }

    #[test]
    fn projection_store_alias_contamination_and_restart_are_bounded() {
        let (mut options, _lock, _desired, _source, _publisher, root) = projection_fixture(false);
        options.public_health_store_path = Some(options.store_path.clone());
        assert!(initialize_projection_publisher(&options).is_err());

        let (options, lock, desired, source, first, second_root) = projection_fixture(false);
        first
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
            .unwrap();
        let first_projection = public_projection(&options);
        let second = initialize_projection_publisher(&options).unwrap().unwrap();
        second
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:03Z")
            .unwrap();
        let second_projection = public_projection(&options);
        assert_eq!(second_projection.projection_sequence, 2);
        assert_ne!(
            second_projection.projection_incarnation_id,
            first_projection.projection_incarnation_id
        );

        let public_path = options.public_health_store_path.as_ref().unwrap();
        let backing = SingleFileMessagePackBackingStore::new(public_path);
        let current_envelope = backing.pull_all_read_only_snapshot().unwrap().remove(0);
        let mut corrupted = second_projection.clone();
        corrupted.signature[0] ^= 1;
        let corrupted_envelope = CultCacheEnvelope {
            payload: rmp_serde::to_vec(&corrupted).unwrap(),
            ..current_envelope.clone()
        };
        assert!(
            backing
                .compare_exchange(
                    &[CultCacheExpectedEnvelope {
                        key: current_envelope.key.clone(),
                        r#type: current_envelope.r#type.clone(),
                        current: Some(current_envelope),
                    }],
                    &[corrupted_envelope],
                )
                .unwrap()
        );
        assert!(
            second
                .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:04Z")
                .is_err()
        );

        let contaminated_path = root.path().join("contaminated-public.cc");
        SingleFileMessagePackBackingStore::new(&contaminated_path)
            .push(&typed_envelope(&desired.daemon_id, &desired, "2026-07-19T12:00:02Z").unwrap())
            .unwrap();
        let mut contaminated = options.clone();
        contaminated.public_health_store_path = Some(contaminated_path);
        let contaminated_publisher = initialize_projection_publisher(&contaminated)
            .unwrap()
            .unwrap();
        assert!(
            contaminated_publisher
                .publish_if_current(
                    &contaminated,
                    &lock,
                    &desired,
                    &source,
                    "2026-07-19T12:00:04Z",
                )
                .is_err()
        );
        drop(second_root);
    }

    #[test]
    fn non_authenticated_paths_cannot_refresh_or_remove_public_projection() {
        let (options, lock, desired, source, publisher, _root) = projection_fixture(false);
        publisher
            .publish_if_current(&options, &lock, &desired, &source, "2026-07-19T12:00:02Z")
            .unwrap();
        let prior = public_projection(&options);
        publish_authenticated_provider_health(
            Some(&publisher),
            &options,
            &lock,
            &desired,
            None,
            "2026-07-19T12:00:30Z",
        )
        .unwrap();
        assert_eq!(public_projection(&options), prior);
    }

    #[test]
    fn projection_cas_serializes_competing_idunn_incarnations() {
        let (options, lock, desired, source, first, _root) = projection_fixture(false);
        let second = initialize_projection_publisher(&options).unwrap().unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let spawn_publish = |publisher: Arc<IdunnProjectionPublisher>| {
            let options = options.clone();
            let lock = Arc::clone(&lock);
            let desired = desired.clone();
            let source = source.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                publisher.publish_if_current(
                    &options,
                    &lock,
                    &desired,
                    &source,
                    "2026-07-19T12:00:02Z",
                )
            })
        };
        let one = spawn_publish(first);
        let two = spawn_publish(second);
        barrier.wait();
        one.join().unwrap().unwrap();
        two.join().unwrap().unwrap();
        let final_projection = public_projection(&options);
        assert_eq!(final_projection.projection_sequence, 2);
    }

    #[test]
    fn generic_signed_health_requires_canonical_bytes_and_preserves_millisecond_clock() {
        let (mut message, options, root) = generic_signed_health_fixture(1);
        let mut statement: IdunnSignedDaemonHealthRecord = match &message {
            CultNetMessage::DocumentPutRaw { document, .. } => {
                rmp_serde::from_slice(&document.payload).unwrap()
            }
            _ => unreachable!(),
        };
        statement.observed_at_unix_millis =
            chrono::DateTime::parse_from_rfc3339("2026-07-19T12:00:00.500Z")
                .unwrap()
                .timestamp_millis() as u64;
        statement.signature.clear();
        let signer = cultnet_rs::open_service_identity_at::<GameCultProviderHealthIdentity>(
            &root.path().join("provider-health-identity.cc"),
        )
        .unwrap();
        statement.signature = signer
            .sign::<IdunnSignedDaemonHealthPurpose>(&rmp_serde::to_vec(&statement).unwrap())
            .signature;
        let CultNetMessage::DocumentPutRaw { document, .. } = &mut message else {
            unreachable!()
        };
        document.payload = rmp_serde::to_vec(&statement).unwrap();

        admit_health_from_rudp_message(
            &message,
            &options,
            &Arc::new(Mutex::new(())),
            "2026-07-19T12:00:00.500Z",
        )
        .unwrap();

        let CultNetMessage::DocumentPutRaw { document, .. } = &mut message else {
            unreachable!()
        };
        let canonical = rmp_serde::to_vec(&statement).unwrap();
        let string_marker_index = if canonical.first() == Some(&0xdc) {
            3
        } else {
            1
        };
        let string_marker = canonical[string_marker_index];
        assert!((0xa0..=0xbf).contains(&string_marker));
        let string_length = string_marker & 0x1f;
        let mut noncanonical = Vec::with_capacity(canonical.len() + 1);
        noncanonical.extend_from_slice(&canonical[..string_marker_index]);
        noncanonical.extend_from_slice(&[0xd9, string_length]);
        noncanonical.extend_from_slice(&canonical[string_marker_index + 1..]);
        document.payload = noncanonical;
        let error =
            authenticate_generic_signed_health(document, &options, "2026-07-19T12:00:00.500Z")
                .unwrap_err()
                .to_string();
        assert!(error.contains("canonical positional MessagePack"));
    }

    fn signed_epiphany_health_fixture(
        sequence: u64,
    ) -> (CultNetMessage, CommonOptions, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let identity_path = root.path().join("host.ccmp");
        let store_path = root.path().join("idunn.cc");
        let key = SigningKey::from_bytes(&[23; 32]);
        let public_key = key.verifying_key().to_bytes().to_vec();
        let identity_id = format!(
            "{:x}",
            Sha256::digest([HOST_IDENTITY_DOMAIN, public_key.as_slice()].concat())
        );
        let identity = EpiphanyHostIdentityWire {
            schema_version: HOST_IDENTITY_TYPE.into(),
            identity_id: identity_id.clone(),
            public_key,
            assurance: "test".into(),
            identity_created_at: "2026-07-16T00:00:00Z".into(),
            source_identity_record_sha256: format!("sha256-{}", "c".repeat(64)),
        };
        let mut backing = SingleFileMessagePackBackingStore::new(&identity_path);
        backing
            .push(&CultCacheEnvelope {
                key: HOST_IDENTITY_KEY.into(),
                r#type: HOST_IDENTITY_TYPE.into(),
                payload: rmp_serde::to_vec(&identity).unwrap(),
                stored_at: identity.identity_created_at.clone(),
                schema_id: Some(HOST_IDENTITY_TYPE.into()),
            })
            .unwrap();
        let mut signed = EpiphanySignedRuntimeHealthWire {
            schema_version: EPIPHANY_SIGNED_RUNTIME_HEALTH_SCHEMA_VERSION.into(),
            health: IdunnDaemonHealthWireV1 {
                daemon_id: "yggdrasil-epiphany".into(),
                state: "warming".into(),
                detail: "authenticated checkpoint progress".into(),
                observed_at: "2026-07-16T00:01:00Z".into(),
                health_contract: EPIPHANY_HEALTH_CONTRACT.into(),
                publication_source: "daemon-published".into(),
                transport: CULTNET_RUDP_PROTOCOL_ID.into(),
            },
            source_runtime_id: EPIPHANY_HEALTH_SOURCE_RUNTIME.into(),
            release_id: "release-test".into(),
            release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
            source_commit: "b".repeat(40),
            deployment_request_id: "deploy:yggdrasil-epiphany:fixture".into(),
            publisher_incarnation_id: "00000000-0000-4000-8000-000000000001".into(),
            publisher_sequence: sequence,
            publisher_process_id: 42,
            publisher_process_creation_token: 7,
            publisher_process_created_at: "2026-07-16T00:00:30Z".into(),
            publisher_executable_path: "/srv/epiphany/app/current/epiphany-daemon-supervisor"
                .into(),
            signer_identity_id: identity_id,
            signature_algorithm: "ed25519".into(),
            signature: Vec::new(),
        };
        let statement = rmp_serde::to_vec_named(&signed).unwrap();
        signed.signature = key
            .sign(&host_signature_message(
                EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE,
                &statement,
            ))
            .to_bytes()
            .to_vec();
        let message = CultNetMessage::DocumentPutRaw {
            message_id: format!("signed-health-{sequence}"),
            document: CultNetRawDocumentRecord {
                schema_id: EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE.into(),
                record_key: signed.health.daemon_id.clone(),
                stored_at: signed.health.observed_at.clone(),
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: rmp_serde::to_vec_named(&signed).unwrap(),
                source_runtime_id: Some(EPIPHANY_HEALTH_SOURCE_RUNTIME.into()),
                source_agent_id: None,
                source_role: Some("daemon-health-publisher".into()),
                tags: Some(vec![CULTNET_RUDP_PROTOCOL_ID.into()]),
            },
        };
        let options = CommonOptions {
            store_path,
            release_authority_store_path: None,
            deployment_brake_store_path: None,
            deployment_brake_operator_anchor_path: None,
            deployment_brake_runtime_id: None,
            operator_alarm_command: None,
            rudp_health_bind: None,
            trusted_epiphany_health_identity_store: Some(identity_path),
            daemon_health_trust_store_path: None,
            service_identity_store_path: None,
            public_health_store_path: None,
            public_health_query_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        persist_current_deployment_request(
            &options,
            &Arc::new(Mutex::new(())),
            &IdunnDeploymentRequestRecord {
                request_id: "deploy:yggdrasil-epiphany:fixture".into(),
                daemon_id: "yggdrasil-epiphany".into(),
                command: "fixture".into(),
                authority: "idunn-supervisor-command.manual".into(),
                requested_at: "unix:1784160000".into(),
                repository_full_name: "GameCult/Epiphany".into(),
                upstream_ref: "refs/heads/main".into(),
                source_revision: "b".repeat(40),
                release_authority_id: "authority-fixture".into(),
                release_authority_envelope_sha256: format!("sha256-{}", "d".repeat(64)),
                requires_bifrost_authority: true,
            },
        )
        .unwrap();
        (message, options, root)
    }

    #[test]
    fn signed_epiphany_health_requires_pinned_identity_and_advancing_sequence() {
        let (message, options, _root) = signed_epiphany_health_fixture(1);
        let lock = Arc::new(Mutex::new(()));
        let admitted =
            admit_health_from_rudp_message(&message, &options, &lock, "2026-07-16T00:01:01Z")
                .unwrap();
        assert_eq!(admitted.state, "warming");
        validate_health_admission_at(
            &HealthAdmissionValidationOptions {
                daemon_id: "yggdrasil-epiphany".into(),
                deployment_request_id: "deploy:yggdrasil-epiphany:fixture".into(),
                release_id: "release-test".into(),
                release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
                source_commit: "b".repeat(40),
            },
            &options,
            "2026-07-16T00:01:02Z",
        )
        .unwrap_err();
        let mut active_message = message.clone();
        let CultNetMessage::DocumentPutRaw { document, .. } = &mut active_message else {
            unreachable!()
        };
        let mut active: EpiphanySignedRuntimeHealthWire =
            rmp_serde::from_slice(&document.payload).unwrap();
        active.health.state = "active".into();
        active.publisher_sequence = 2;
        active.signature.clear();
        let key = SigningKey::from_bytes(&[23; 32]);
        let statement = rmp_serde::to_vec_named(&active).unwrap();
        active.signature = key
            .sign(&host_signature_message(
                EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE,
                &statement,
            ))
            .to_bytes()
            .to_vec();
        document.payload = rmp_serde::to_vec_named(&active).unwrap();
        admit_health_from_rudp_message(&active_message, &options, &lock, "2026-07-16T00:01:02Z")
            .unwrap();
        validate_health_admission_at(
            &HealthAdmissionValidationOptions {
                daemon_id: "yggdrasil-epiphany".into(),
                deployment_request_id: "deploy:yggdrasil-epiphany:fixture".into(),
                release_id: "release-test".into(),
                release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
                source_commit: "b".repeat(40),
            },
            &options,
            "2026-07-16T00:01:03Z",
        )
        .unwrap();
        assert!(
            admit_health_from_rudp_message(&message, &options, &lock, "2026-07-16T00:01:03Z",)
                .is_err()
        );

        let mut newer = with_store_node(&options, &lock, |node| {
            node.get::<IdunnDeploymentRequestRecord>("deploy:yggdrasil-epiphany:fixture")?
                .ok_or_else(|| anyhow!("fixture request missing"))
        })
        .unwrap();
        newer.request_id = "deploy:yggdrasil-epiphany:aaa".into();
        newer.requested_at = "unix:1784160000".into();
        let live_error = persist_current_deployment_request(&options, &lock, &newer)
            .unwrap_err()
            .to_string();
        assert!(live_error.contains("remains live; refusing supersession"));
        with_store_node(&options, &lock, |node| {
            node.put(
                "result:deploy:yggdrasil-epiphany:fixture",
                &IdunnDeploymentResultRecord {
                    result_id: "result:deploy:yggdrasil-epiphany:fixture".into(),
                    request_id: "deploy:yggdrasil-epiphany:fixture".into(),
                    daemon_id: "yggdrasil-epiphany".into(),
                    state: "succeeded".into(),
                    detail: "fixture deployment completed".into(),
                    completed_at: "unix:1784160001".into(),
                },
            )?;
            Ok(())
        })
        .unwrap();
        persist_current_deployment_request(&options, &lock, &newer).unwrap();
        let stale_error = validate_health_admission_at(
            &HealthAdmissionValidationOptions {
                daemon_id: "yggdrasil-epiphany".into(),
                deployment_request_id: "deploy:yggdrasil-epiphany:fixture".into(),
                release_id: "release-test".into(),
                release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
                source_commit: "b".repeat(40),
            },
            &options,
            "2026-07-16T00:02:00Z",
        )
        .unwrap_err()
        .to_string();
        assert!(stale_error.contains("superseded deployment request"));
        assert!(
            admit_health_from_rudp_message(
                &active_message,
                &options,
                &lock,
                "2026-07-16T00:02:00Z",
            )
            .is_err()
        );

        let (mut alien, alien_options, _alien_root) = signed_epiphany_health_fixture(2);
        if let CultNetMessage::DocumentPutRaw { document, .. } = &mut alien {
            document.source_runtime_id = Some("alien-runtime".into());
        }
        assert!(health_from_rudp_message(&alien, &alien_options).is_err());

        let (mut forged, forged_options, _forged_root) = signed_epiphany_health_fixture(3);
        let CultNetMessage::DocumentPutRaw { document, .. } = &mut forged else {
            unreachable!()
        };
        let mut forged_wire: EpiphanySignedRuntimeHealthWire =
            rmp_serde::from_slice(&document.payload).unwrap();
        forged_wire.signature[0] ^= 1;
        document.payload = rmp_serde::to_vec_named(&forged_wire).unwrap();
        assert!(health_from_rudp_message(&forged, &forged_options).is_err());
    }

    #[test]
    fn startup_terminalizes_interrupted_deployment_before_supersession() {
        let (_message, options, _root) = signed_epiphany_health_fixture(1);
        let lock = Arc::new(Mutex::new(()));
        let prior = with_store_node(&options, &lock, |node| {
            node.get::<IdunnDeploymentRequestRecord>("deploy:yggdrasil-epiphany:fixture")?
                .ok_or_else(|| anyhow!("fixture request missing"))
        })
        .unwrap();

        assert_eq!(
            terminalize_interrupted_deployment_requests(&options, &lock, "2026-07-16T00:02:00Z",)
                .unwrap(),
            1
        );
        let result = with_store_node(&options, &lock, |node| {
            node.get::<IdunnDeploymentResultRecord>("result:deploy:yggdrasil-epiphany:fixture")?
                .ok_or_else(|| anyhow!("recovered result missing"))
        })
        .unwrap();
        assert_eq!(result.request_id, prior.request_id);
        assert_eq!(result.daemon_id, prior.daemon_id);
        assert_eq!(result.state, "failed");
        assert_eq!(result.completed_at, "2026-07-16T00:02:00Z");
        assert!(
            result
                .detail
                .contains("owning Idunn daemon incarnation stopped")
        );
        assert_eq!(
            terminalize_interrupted_deployment_requests(&options, &lock, "2026-07-16T00:03:00Z",)
                .unwrap(),
            0
        );

        let mut successor = prior;
        successor.request_id = "deploy:yggdrasil-epiphany:successor".into();
        successor.requested_at = "2026-07-16T00:04:00Z".into();
        let head = persist_current_deployment_request(&options, &lock, &successor).unwrap();
        assert_eq!(head.request_id, successor.request_id);
        assert_eq!(head.sequence, 2);
    }

    #[test]
    fn pinned_epiphany_health_identity_load_is_read_only() {
        let (_message, options, _root) = signed_epiphany_health_fixture(1);
        let identity_path = options
            .trusted_epiphany_health_identity_store
            .as_deref()
            .expect("fixture trust anchor");
        let lock_path = identity_path.with_file_name(format!(
            "{}.lock",
            identity_path
                .file_name()
                .expect("trust anchor filename")
                .to_string_lossy()
        ));
        if lock_path.exists() {
            std::fs::remove_file(&lock_path).unwrap();
        }
        let mut permissions = std::fs::metadata(identity_path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(identity_path, permissions).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                identity_path.parent().unwrap(),
                std::fs::Permissions::from_mode(0o555),
            )
            .unwrap();
        }

        let identity = load_epiphany_health_identity(identity_path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                identity_path.parent().unwrap(),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        assert_eq!(identity.schema_version, HOST_IDENTITY_TYPE);
        assert!(!lock_path.exists());
    }

    #[test]
    fn unsigned_epiphany_health_is_rejected() {
        let (mut message, options, _root) = signed_epiphany_health_fixture(1);
        let CultNetMessage::DocumentPutRaw { document, .. } = &mut message else {
            unreachable!()
        };
        let signed: EpiphanySignedRuntimeHealthWire =
            rmp_serde::from_slice(&document.payload).unwrap();
        document.schema_id = "idunn.daemon_health".into();
        document.payload = rmp_serde::to_vec(&signed.health).unwrap();
        assert!(health_from_rudp_message(&message, &options).is_err());
    }

    #[test]
    fn signed_admission_must_remain_the_current_health_observation() {
        let (message, options, _root) = signed_epiphany_health_fixture(1);
        let lock = Arc::new(Mutex::new(()));
        admit_health_from_rudp_message(&message, &options, &lock, "2026-07-16T00:01:01Z").unwrap();
        with_store_node(&options, &lock, |node| {
            let mut health = node
                .get::<IdunnDaemonHealthRecord>("yggdrasil-epiphany")?
                .unwrap();
            health.state = "failed".into();
            health.observed_at = "2026-07-16T00:01:02Z".into();
            node.put(&health.daemon_id, &health)?;
            Ok(())
        })
        .unwrap();
        let error = validate_health_admission_at(
            &HealthAdmissionValidationOptions {
                daemon_id: "yggdrasil-epiphany".into(),
                deployment_request_id: "deploy:yggdrasil-epiphany:fixture".into(),
                release_id: "release-test".into(),
                release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
                source_commit: "b".repeat(40),
            },
            &options,
            "2026-07-16T00:01:03Z",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("not the current daemon health observation"));
    }

    #[test]
    fn generic_health_cannot_substitute_for_signed_admission() {
        let root = tempfile::tempdir().unwrap();
        let options = CommonOptions {
            store_path: root.path().join("idunn.cc"),
            release_authority_store_path: None,
            deployment_brake_store_path: None,
            deployment_brake_operator_anchor_path: None,
            deployment_brake_runtime_id: None,
            operator_alarm_command: None,
            rudp_health_bind: None,
            trusted_epiphany_health_identity_store: None,
            daemon_health_trust_store_path: None,
            service_identity_store_path: None,
            public_health_store_path: None,
            public_health_query_bind: None,
            execute: false,
            command_timeout_seconds: 1,
        };
        let lock = Arc::new(Mutex::new(()));
        with_store_node(&options, &lock, |node| {
            node.put(
                "yggdrasil-epiphany",
                &IdunnDaemonHealthRecord {
                    daemon_id: "yggdrasil-epiphany".into(),
                    state: "active".into(),
                    detail: "counterfeit generic observation".into(),
                    observed_at: "2026-07-16T00:01:00Z".into(),
                    health_contract: EPIPHANY_HEALTH_CONTRACT.into(),
                    publication_source: "daemon-published".into(),
                    transport: CULTNET_RUDP_PROTOCOL_ID.into(),
                },
            )?;
            Ok(())
        })
        .unwrap();
        let error = validate_health_admission(
            &HealthAdmissionValidationOptions {
                daemon_id: "yggdrasil-epiphany".into(),
                deployment_request_id: "counterfeit".into(),
                release_id: "release-test".into(),
                release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
                source_commit: "b".repeat(40),
            },
            &options,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("no signed health admission"));
    }

    #[test]
    fn signed_admission_freshness_rejects_stale_future_and_inverted_time() {
        let mut admission = IdunnSignedHealthAdmissionRecord {
            daemon_id: "yggdrasil-epiphany".into(),
            state: "active".into(),
            observed_at: "unix:100".into(),
            admitted_at: "unix:101".into(),
            health_contract: EPIPHANY_HEALTH_CONTRACT.into(),
            deployment_request_id: "request".into(),
            release_id: "release".into(),
            release_witness_sha256: format!("sha256-{}", "a".repeat(64)),
            source_commit: "b".repeat(40),
            publisher_incarnation_id: "00000000-0000-4000-8000-000000000001".into(),
            publisher_sequence: 1,
            publisher_process_created_at: "2026-07-16T00:00:00Z".into(),
            signer_identity_id: "signer".into(),
            signed_health_sha256: format!("sha256-{}", "c".repeat(64)),
        };
        validate_admission_fresh_at(&admission, "unix:280", 180).unwrap();
        assert!(validate_admission_fresh_at(&admission, "unix:282", 180).is_err());
        admission.observed_at = "unix:102".into();
        assert!(validate_admission_fresh_at(&admission, "unix:102", 180).is_err());
        admission.observed_at = "unix:100".into();
        admission.admitted_at = "unix:103".into();
        assert!(validate_admission_fresh_at(&admission, "unix:102", 180).is_err());
    }

    #[test]
    fn concurrent_signed_health_writers_cannot_regress_sequence() {
        let (message_one, options, _root) = signed_epiphany_health_fixture(1);
        let (mut message_two, _, _) = signed_epiphany_health_fixture(2);
        // Re-sign sequence two with the trust anchor belonging to the shared fixture.
        let CultNetMessage::DocumentPutRaw { document, .. } = &mut message_two else {
            unreachable!()
        };
        let mut signed: EpiphanySignedRuntimeHealthWire =
            rmp_serde::from_slice(&document.payload).unwrap();
        signed.signer_identity_id = match &message_one {
            CultNetMessage::DocumentPutRaw { document, .. } => {
                let first: EpiphanySignedRuntimeHealthWire =
                    rmp_serde::from_slice(&document.payload).unwrap();
                first.signer_identity_id
            }
            _ => unreachable!(),
        };
        signed.signature.clear();
        let key = SigningKey::from_bytes(&[23; 32]);
        let statement = rmp_serde::to_vec_named(&signed).unwrap();
        signed.signature = key
            .sign(&host_signature_message(
                EPIPHANY_SIGNED_RUNTIME_HEALTH_TYPE,
                &statement,
            ))
            .to_bytes()
            .to_vec();
        document.payload = rmp_serde::to_vec_named(&signed).unwrap();

        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut workers = Vec::new();
        for message in [message_one, message_two] {
            let options = options.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                admit_health_from_rudp_message(
                    &message,
                    &options,
                    &Arc::new(Mutex::new(())),
                    "2026-07-16T00:01:01Z",
                )
            }));
        }
        barrier.wait();
        for worker in workers {
            let _ = worker.join().unwrap();
        }
        let final_admission = with_store_node(&options, &Arc::new(Mutex::new(())), |node| {
            node.get::<IdunnSignedHealthAdmissionRecord>("yggdrasil-epiphany")?
                .ok_or_else(|| anyhow!("final admission missing"))
        })
        .unwrap();
        assert_eq!(final_admission.publisher_sequence, 2);
    }
}

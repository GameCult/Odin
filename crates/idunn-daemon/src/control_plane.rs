use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cultcache_rs::{
    CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore,
};
use cultnet_rs::{
    AuthenticatedOdinRuntimeTopologyCorrelation, GameCultProviderHealthIdentity,
    IDUNN_DEPLOYMENT_BRAKE_SCHEMA, IDUNN_LIFECYCLE_BRAKE_SCHEMA, IDUNN_PROCESS_WRITE_LEASE_SCHEMA,
    IdunnDeploymentBrakeObservation, IdunnDeploymentBrakeOperatorIdentity,
    IdunnDeploymentBrakeRecord, IdunnExpectedIncarnationRecord, IdunnLifecycleBrakeObservation,
    IdunnLifecycleBrakeRecord, IdunnProcessWriteLeaseRecord, IdunnRuntimeActivationLaunch,
    IdunnRuntimeActivationRecord, IdunnServiceIdentity, OdinTopologyAuthenticationContext,
    OdinTopologyDisagreement, OdinTopologyIdentity, RuntimePresenceAuthenticationContext,
    ServiceIdentityProfile, ServiceIdentitySigner, ServiceIdentityTrustAnchor,
    authenticate_odin_runtime_topology_correlation, authenticate_runtime_presence_claim,
    correlate_runtime_presence_claim, derive_service_identity_id,
    evaluate_idunn_continuity_restart, evaluate_idunn_deployment_brake, open_service_identity_at,
    verify_idunn_deployment_brake_authorization, verify_runtime_authority,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::deployment::{DependencyKind, OperatorBinding, RouteBinding, capability_compatible};
use crate::deployment_plan::{
    CompiledDeploymentPlan, DependencyProviderAuthority, SealedRelease, compile_deployment_plan,
};
use crate::drivers::{
    CultCacheTopologyDriver, CultCacheWriteLeaseDriver, DockerRunnerDriver, FrozenSourceReceipt,
    GitSourceDriver, InstalledReleaseObservation, NginxRouteDriver, ProcessIdentity,
    RouteObservation, RoutePreflightReceipt, RunnerPort, SourcePort,
    SystemdTransientWorkloadDriver, TopologyPort, WorkloadObservation, WorkloadPort,
    WriteLeasePort,
};

const DEPLOYMENT_COMMAND_SCHEMA: &str = "idunn.deployment_command.v2";
const DEPLOYMENT_TRANSACTION_SCHEMA: &str = "idunn.deployment_transaction.v2";
const ADMITTED_GENERATION_SCHEMA: &str = "idunn.admitted_generation.v2";
const DEFAULT_TOPOLOGY_MAXIMUM_AGE_MILLIS: u64 = 30_000;
const DEFAULT_TOPOLOGY_MAXIMUM_FUTURE_SKEW_MILLIS: u64 = 2_000;

/// An immutable request. Execution state deliberately does not fit in this
/// record; status is derived from the transactions created for it.
#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.deployment_command",
    schema = "idunn.deployment_command.v2"
)]
struct DeploymentCommand {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    command_id: String,
    #[cultcache(key = 2)]
    kind: CommandKind,
    #[cultcache(key = 3)]
    selector: String,
    #[cultcache(key = 4)]
    requested_by: String,
    #[cultcache(key = 5)]
    requested_at_unix_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CommandKind {
    Deploy,
    Continuity,
}

impl DeploymentCommand {
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
        match self.kind {
            CommandKind::Deploy => ensure!(
                !self.command_id.starts_with("continuity-"),
                "operator deployment uses the continuity command namespace"
            ),
            CommandKind::Continuity => ensure!(
                self.command_id.starts_with("continuity-")
                    && !self.selector.starts_with("profile:"),
                "continuity command identity or selector is invalid"
            ),
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
enum DeploymentPhase {
    Sealing,
    Starting,
    Warming,
    Fencing,
    Leasing,
    AwaitingReady,
    Routing,
    Committing,
    Complete,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeploymentAuthorization {
    authorization_id: String,
    brake_sha256: String,
    canonical_brake_bytes: Vec<u8>,
    authorized_at_unix_millis: u64,
}

impl DeploymentAuthorization {
    fn validate_shape(&self) -> Result<()> {
        require_id(&self.authorization_id, "deployment authorization id")?;
        ensure!(
            !self.canonical_brake_bytes.is_empty()
                && self.brake_sha256 == sha256_id(&self.canonical_brake_bytes)
                && self.authorized_at_unix_millis > 0,
            "deployment authorization receipt is incomplete"
        );
        let record: IdunnDeploymentBrakeRecord =
            rmp_serde::from_slice(&self.canonical_brake_bytes)?;
        record.validate()?;
        ensure!(
            rmp_serde::to_vec(&record)? == self.canonical_brake_bytes
                && record.authorization_id.as_deref() == Some(&self.authorization_id),
            "deployment authorization receipt is noncanonical or mismatched"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TopologyEvidence {
    canonical_bytes: Vec<u8>,
    canonical_sha256: String,
    signer_identity_id: String,
    publisher_sequence: u64,
    admitted_at_unix_millis: u64,
}

impl TopologyEvidence {
    fn from_authenticated(
        topology: &AuthenticatedOdinRuntimeTopologyCorrelation,
        admitted_at_unix_millis: u64,
    ) -> Result<Self> {
        let record = topology.record();
        let evidence = Self {
            canonical_bytes: topology.canonical_bytes().to_vec(),
            canonical_sha256: sha256_id(topology.canonical_bytes()),
            signer_identity_id: record.signer_identity_id.clone(),
            publisher_sequence: record.publisher_sequence,
            admitted_at_unix_millis,
        };
        evidence.validate_shape()?;
        Ok(evidence)
    }

    fn validate_shape(&self) -> Result<()> {
        ensure!(
            !self.canonical_bytes.is_empty()
                && self.canonical_sha256 == sha256_id(&self.canonical_bytes),
            "topology evidence bytes or digest are invalid"
        );
        require_id(&self.signer_identity_id, "Odin topology signer")?;
        ensure!(
            self.publisher_sequence > 0 && self.admitted_at_unix_millis > 0,
            "topology evidence sequence or admission time is invalid"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimePresenceEvidence {
    canonical_bytes: Vec<u8>,
    canonical_sha256: String,
    message_id: String,
    challenged_at_unix_millis: u64,
    admitted_at_unix_millis: u64,
}

impl RuntimePresenceEvidence {
    fn from_present(
        present: &cultnet_rs::VerifiedRuntimePresence,
        message_id: String,
        challenged_at_unix_millis: u64,
        admitted_at_unix_millis: u64,
    ) -> Result<Self> {
        let evidence = Self {
            canonical_bytes: present.canonical_bytes().to_vec(),
            canonical_sha256: present.signed_presence_sha256().to_owned(),
            message_id,
            challenged_at_unix_millis,
            admitted_at_unix_millis,
        };
        evidence.validate_shape()?;
        Ok(evidence)
    }

    fn validate_shape(&self) -> Result<()> {
        ensure!(
            !self.canonical_bytes.is_empty()
                && self.canonical_sha256 == sha256_id(&self.canonical_bytes),
            "runtime presence evidence bytes or digest are invalid"
        );
        require_id(&self.message_id, "runtime presence challenge")?;
        ensure!(
            self.challenged_at_unix_millis > 0
                && self.admitted_at_unix_millis >= self.challenged_at_unix_millis,
            "runtime presence evidence timeline is invalid"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "kebab-case", deny_unknown_fields)]
enum WarmingEvidence {
    OdinTopology { evidence: TopologyEvidence },
    FirstOdinDirect { evidence: RuntimePresenceEvidence },
}

impl WarmingEvidence {
    fn validate_shape(&self) -> Result<()> {
        match self {
            Self::OdinTopology { evidence } => evidence.validate_shape(),
            Self::FirstOdinDirect { evidence } => evidence.validate_shape(),
        }
    }

    fn canonical_sha256(&self) -> &str {
        match self {
            Self::OdinTopology { evidence } => &evidence.canonical_sha256,
            Self::FirstOdinDirect { evidence } => &evidence.canonical_sha256,
        }
    }
}

/// Opaque capability minted only after the transaction CAS stores exact
/// Warming evidence. Adjacent drivers receive only the candidate identity and
/// signed-presence digest needed to bind the write lease; they cannot inspect
/// or construct semantic admission.
#[derive(Clone, Debug)]
pub struct SequenceAdmittedWarming {
    transaction_id: String,
    evidence: WarmingEvidence,
    signed_presence_sha256: String,
    runtime_instance_id: String,
}

impl SequenceAdmittedWarming {
    fn from_topology(
        transaction_id: String,
        evidence: TopologyEvidence,
        authenticated: AuthenticatedOdinRuntimeTopologyCorrelation,
    ) -> Result<Self> {
        let record = authenticated.record();
        let signed_presence_sha256 = record
            .signed_presence_sha256
            .clone()
            .context("Warming topology has no signed presence")?;
        let runtime_instance_id = record
            .runtime_instance_id
            .clone()
            .context("Warming topology has no runtime instance")?;
        Ok(Self {
            transaction_id,
            evidence: WarmingEvidence::OdinTopology { evidence },
            signed_presence_sha256,
            runtime_instance_id,
        })
    }

    fn from_first_odin_presence(
        transaction_id: String,
        evidence: RuntimePresenceEvidence,
        present: cultnet_rs::VerifiedRuntimePresence,
    ) -> Result<Self> {
        ensure!(
            present.canonical_bytes() == evidence.canonical_bytes
                && present.signed_presence_sha256() == evidence.canonical_sha256,
            "direct first-Odin Warming evidence differs from its authenticated presence"
        );
        let runtime_instance_id = present.record().runtime_instance_id.clone();
        Ok(Self {
            transaction_id,
            signed_presence_sha256: evidence.canonical_sha256.clone(),
            evidence: WarmingEvidence::FirstOdinDirect { evidence },
            runtime_instance_id,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        transaction_id: impl Into<String>,
        authenticated: AuthenticatedOdinRuntimeTopologyCorrelation,
        admitted_at_unix_millis: u64,
    ) -> Result<Self> {
        let record = authenticated.record();
        ensure!(
            record.present
                && !record.ready
                && record.observed_presence_state.as_deref() == Some("warming")
                && record.observed_write_lease_sha256.is_none()
                && record.disagreements.is_empty(),
            "test receipt is not exact Warming evidence"
        );
        let transaction_id = transaction_id.into();
        require_id(&transaction_id, "test transaction id")?;
        let evidence =
            TopologyEvidence::from_authenticated(&authenticated, admitted_at_unix_millis)?;
        let signed_presence_sha256 = record
            .signed_presence_sha256
            .clone()
            .context("test Warming receipt has no signed presence")?;
        let runtime_instance_id = record
            .runtime_instance_id
            .clone()
            .context("test Warming receipt has no runtime instance")?;
        let token = Self::from_topology(transaction_id, evidence, authenticated)?;
        ensure!(
            token.signed_presence_sha256 == signed_presence_sha256
                && token.runtime_instance_id == runtime_instance_id,
            "test Warming token extraction changed"
        );
        Ok(token)
    }

    pub(crate) fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    pub(crate) fn evidence_sha256(&self) -> &str {
        self.evidence.canonical_sha256()
    }

    pub(crate) fn signed_presence_sha256(&self) -> &str {
        &self.signed_presence_sha256
    }

    pub(crate) fn runtime_instance_id(&self) -> &str {
        &self.runtime_instance_id
    }
}

/// Ready proof with the same construction boundary. Dependency planning and
/// promotion receive this token, never caller-supplied digest/sequence tuples.
#[derive(Clone, Debug)]
pub(crate) struct SequenceAdmittedReady {
    transaction_id: String,
    evidence: TopologyEvidence,
    expected: IdunnExpectedIncarnationRecord,
    authenticated: AuthenticatedOdinRuntimeTopologyCorrelation,
}

impl SequenceAdmittedReady {
    #[cfg(test)]
    pub(crate) fn for_test(
        expected: &IdunnExpectedIncarnationRecord,
        authenticated: AuthenticatedOdinRuntimeTopologyCorrelation,
        admitted_at_unix_millis: u64,
    ) -> Result<Self> {
        expected.validate()?;
        let record = authenticated.record();
        ensure!(
            record.expected_projection_sha256 == expected.canonical_sha256()?
                && record.target == expected.target
                && record.runtime_id == expected.runtime_id
                && record.present
                && record.ready
                && record.observed_presence_state.as_deref() == Some("active")
                && record.disagreements.is_empty(),
            "test receipt is not exact Ready evidence"
        );
        let evidence =
            TopologyEvidence::from_authenticated(&authenticated, admitted_at_unix_millis)?;
        Ok(Self {
            transaction_id: "test-sequence-admission".into(),
            evidence,
            expected: expected.clone(),
            authenticated,
        })
    }

    pub(crate) fn authenticated(&self) -> &AuthenticatedOdinRuntimeTopologyCorrelation {
        &self.authenticated
    }

    pub(crate) fn expected(&self) -> &IdunnExpectedIncarnationRecord {
        &self.expected
    }

    pub(crate) fn transaction_id(&self) -> &str {
        &self.transaction_id
    }

    pub(crate) fn evidence_sha256(&self) -> &str {
        &self.evidence.canonical_sha256
    }

    pub(crate) fn publisher_sequence(&self) -> u64 {
        self.evidence.publisher_sequence
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case", deny_unknown_fields)]
enum FencingEvidence {
    SkippedStateless,
    Revoked {
        incumbent_lease_sha256: Option<String>,
        candidate_lease_path_verified_empty: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case", deny_unknown_fields)]
enum LeasingEvidence {
    SkippedStateless,
    Prepared {
        lease: IdunnProcessWriteLeaseRecord,
        lease_sha256: String,
    },
    Granted {
        lease: IdunnProcessWriteLeaseRecord,
        lease_sha256: String,
    },
}

impl LeasingEvidence {
    fn lease(&self) -> Option<&IdunnProcessWriteLeaseRecord> {
        match self {
            Self::SkippedStateless | Self::Prepared { .. } => None,
            Self::Granted { lease, .. } => Some(lease),
        }
    }

    fn prepared_lease(&self) -> Option<(&IdunnProcessWriteLeaseRecord, &str)> {
        match self {
            Self::Prepared {
                lease,
                lease_sha256,
            } => Some((lease, lease_sha256)),
            Self::SkippedStateless | Self::Granted { .. } => None,
        }
    }

    fn lease_sha256(&self) -> Option<&str> {
        match self {
            Self::SkippedStateless | Self::Prepared { .. } => None,
            Self::Granted { lease_sha256, .. } => Some(lease_sha256),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case", deny_unknown_fields)]
enum RoutingEvidence {
    SkippedUnrouted,
    Promoted {
        observation: RouteObservation,
        promoted_at_unix_millis: u64,
    },
}

impl RoutingEvidence {
    fn observation(&self) -> Option<&RouteObservation> {
        match self {
            Self::SkippedUnrouted => None,
            Self::Promoted { observation, .. } => Some(observation),
        }
    }

    fn promoted_at_unix_millis(&self) -> Option<u64> {
        match self {
            Self::SkippedUnrouted => None,
            Self::Promoted {
                promoted_at_unix_millis,
                ..
            } => Some(*promoted_at_unix_millis),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IsolationEvidence {
    candidate_uid: u32,
    candidate_pid_namespace_id: u64,
    candidate_mount_namespace_id: u64,
    incumbent_uid: Option<u32>,
    incumbent_pid_namespace_id: Option<u64>,
    incumbent_mount_namespace_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case", deny_unknown_fields)]
enum TransactionCompletion {
    Admitted { generation_id: String },
    FailedBeforeFencing { error: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CleanupEvidence {
    Pending,
    Skipped,
    Complete,
}

impl CleanupEvidence {
    fn is_complete(self) -> bool {
        matches!(self, Self::Skipped | Self::Complete)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreFencingAbort {
    error: String,
    candidate_cleanup: CleanupEvidence,
    topology_reconciliation: CleanupEvidence,
    source_cleanup: CleanupEvidence,
}

impl PreFencingAbort {
    fn is_complete(&self) -> bool {
        self.candidate_cleanup.is_complete()
            && self.topology_reconciliation.is_complete()
            && self.source_cleanup.is_complete()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case", deny_unknown_fields)]
enum IncumbentCleanupEvidence {
    SkippedNoIncumbent,
    Pending {
        generation_id: String,
        workload: WorkloadObservation,
    },
    Complete {
        generation_id: String,
    },
}

impl IncumbentCleanupEvidence {
    fn is_complete(&self) -> bool {
        matches!(self, Self::SkippedNoIncumbent | Self::Complete { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SourceCleanupEvidence {
    SkippedContinuity,
    Pending,
    Complete,
}

impl SourceCleanupEvidence {
    fn is_complete(self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PostCommitCleanup {
    incumbent: IncumbentCleanupEvidence,
    source: SourceCleanupEvidence,
}

impl PostCommitCleanup {
    fn is_complete(&self) -> bool {
        self.incumbent.is_complete() && self.source.is_complete()
    }
}

/// The sole owner of an in-flight deployment decision. Every actuator result
/// is durable here before a later phase is entered.
#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.deployment_transaction",
    schema = "idunn.deployment_transaction.v2"
)]
struct DeploymentTransaction {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    transaction_id: String,
    #[cultcache(key = 2)]
    command_id: String,
    #[cultcache(key = 3)]
    command_kind: CommandKind,
    #[cultcache(key = 4)]
    target: String,
    #[cultcache(key = 5)]
    ordinal: u32,
    #[cultcache(key = 6)]
    phase: DeploymentPhase,
    #[cultcache(key = 7)]
    created_at_unix_millis: u64,
    #[cultcache(key = 8)]
    updated_at_unix_millis: u64,
    #[cultcache(key = 9)]
    incumbent_generation_id: Option<String>,
    #[cultcache(key = 10)]
    plan: Option<CompiledDeploymentPlan>,
    #[cultcache(key = 11)]
    frozen_source: Option<FrozenSourceReceipt>,
    #[cultcache(key = 12)]
    sealed_release: Option<SealedRelease>,
    #[cultcache(key = 13)]
    installed_release: Option<InstalledReleaseObservation>,
    #[cultcache(key = 14)]
    expected: Option<IdunnExpectedIncarnationRecord>,
    #[cultcache(key = 15)]
    expected_publication_sha256: Option<String>,
    #[cultcache(key = 16)]
    deployment_authorization: Option<DeploymentAuthorization>,
    #[cultcache(key = 17)]
    lifecycle_authorized_at_unix_millis: Option<u64>,
    #[cultcache(key = 18)]
    activation: Option<IdunnRuntimeActivationRecord>,
    #[cultcache(key = 19)]
    workload: Option<WorkloadObservation>,
    #[cultcache(key = 20)]
    activation_publication_sha256: Option<String>,
    #[cultcache(key = 21)]
    latest_odin_observation: Option<TopologyEvidence>,
    #[cultcache(key = 22)]
    warming: Option<WarmingEvidence>,
    #[cultcache(key = 23)]
    route_preflight: Option<RoutePreflightReceipt>,
    #[cultcache(key = 24)]
    isolation: Option<IsolationEvidence>,
    #[cultcache(key = 25)]
    fencing: Option<FencingEvidence>,
    #[cultcache(key = 26)]
    leasing: Option<LeasingEvidence>,
    #[cultcache(key = 27)]
    ready: Option<TopologyEvidence>,
    #[cultcache(key = 28)]
    routing: Option<RoutingEvidence>,
    #[cultcache(key = 29)]
    odin_publisher_sequence_cursor: u64,
    #[cultcache(key = 30)]
    last_error: Option<String>,
    #[cultcache(key = 31)]
    completion: Option<TransactionCompletion>,
    #[cultcache(key = 32)]
    pre_fencing_abort: Option<PreFencingAbort>,
    #[cultcache(key = 33)]
    post_commit_cleanup: Option<PostCommitCleanup>,
}

impl DeploymentTransaction {
    fn new(
        command: &DeploymentCommand,
        target: String,
        ordinal: u32,
        incumbent: Option<&AdmittedGeneration>,
        now: u64,
    ) -> Result<Self> {
        let transaction = Self {
            schema_version: DEPLOYMENT_TRANSACTION_SCHEMA.into(),
            transaction_id: format!("tx-{}", Uuid::new_v4()),
            command_id: command.command_id.clone(),
            command_kind: command.kind,
            target,
            ordinal,
            phase: DeploymentPhase::Sealing,
            created_at_unix_millis: now,
            updated_at_unix_millis: now,
            incumbent_generation_id: incumbent.map(|value| value.generation_id.clone()),
            plan: None,
            frozen_source: None,
            sealed_release: None,
            installed_release: None,
            expected: None,
            expected_publication_sha256: None,
            deployment_authorization: None,
            lifecycle_authorized_at_unix_millis: None,
            activation: None,
            workload: None,
            activation_publication_sha256: None,
            latest_odin_observation: None,
            warming: None,
            route_preflight: None,
            isolation: None,
            fencing: None,
            leasing: None,
            ready: None,
            routing: None,
            odin_publisher_sequence_cursor: incumbent
                .map_or(0, |value| value.odin_publisher_sequence_cursor),
            last_error: None,
            completion: None,
            pre_fencing_abort: None,
            post_commit_cleanup: None,
        };
        transaction.validate()?;
        Ok(transaction)
    }

    fn from_continuity(
        command: &DeploymentCommand,
        incumbent: &AdmittedGeneration,
        now: u64,
    ) -> Result<Self> {
        let mut transaction =
            Self::new(command, incumbent.target.clone(), 0, Some(incumbent), now)?;
        transaction.plan = Some(incumbent.plan.clone());
        transaction.sealed_release = Some(incumbent.sealed_release.clone());
        transaction.installed_release = Some(incumbent.installed_release.clone());
        transaction.expected = Some(incumbent.expected.clone());
        transaction.validate()?;
        Ok(transaction)
    }

    fn rejected(command: &DeploymentCommand, error: anyhow::Error, now: u64) -> Result<Self> {
        let mut transaction = Self::new(command, command.selector.clone(), 0, None, now)?;
        let detail = truncate(&format!("{error:#}"), 2048);
        transaction.phase = DeploymentPhase::Complete;
        transaction.updated_at_unix_millis = now;
        transaction.last_error = Some(detail.clone());
        transaction.pre_fencing_abort = Some(PreFencingAbort {
            error: detail.clone(),
            candidate_cleanup: CleanupEvidence::Skipped,
            topology_reconciliation: CleanupEvidence::Skipped,
            source_cleanup: CleanupEvidence::Skipped,
        });
        transaction.completion = Some(TransactionCompletion::FailedBeforeFencing { error: detail });
        transaction.validate()?;
        Ok(transaction)
    }

    fn is_terminal(&self) -> bool {
        if self.phase != DeploymentPhase::Complete {
            return false;
        }
        match &self.completion {
            Some(TransactionCompletion::FailedBeforeFencing { .. }) => self
                .pre_fencing_abort
                .as_ref()
                .is_some_and(PreFencingAbort::is_complete),
            Some(TransactionCompletion::Admitted { .. }) => self
                .post_commit_cleanup
                .as_ref()
                .is_some_and(PostCommitCleanup::is_complete),
            None => false,
        }
    }

    /// Complete transactions retain retryable cleanup work, but admission has
    /// already transferred current-incarnation authority to AdmittedGeneration.
    fn owns_target_authority(&self) -> bool {
        self.phase != DeploymentPhase::Complete
    }

    /// Admission has moved to the new generation at Complete, but an exact
    /// draining incumbent still reserves its process and candidate endpoint.
    /// No later mutation for the same target may overlap that cleanup.
    fn blocks_new_target_mutation(&self) -> bool {
        self.owns_target_authority()
            || self
                .post_commit_cleanup
                .as_ref()
                .is_some_and(|cleanup| !cleanup.is_complete())
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == DEPLOYMENT_TRANSACTION_SCHEMA,
            "deployment transaction schema is unsupported"
        );
        require_id(&self.transaction_id, "deployment transaction id")?;
        require_id(&self.command_id, "transaction command id")?;
        require_id(&self.target, "transaction target")?;
        ensure!(
            self.created_at_unix_millis > 0
                && self.updated_at_unix_millis >= self.created_at_unix_millis,
            "deployment transaction timestamps are invalid"
        );
        if let Some(generation) = &self.incumbent_generation_id {
            require_id(generation, "incumbent generation id")?;
        }
        if let Some(plan) = &self.plan {
            plan.validate()?;
            ensure!(
                plan.parsed_inputs()?.1.target == self.target,
                "transaction plan belongs to another target"
            );
        }
        if let (Some(plan), Some(frozen)) = (&self.plan, &self.frozen_source) {
            frozen.validate_against(plan)?;
        }
        if let (Some(plan), Some(release)) = (&self.plan, &self.sealed_release) {
            release.validate_against(plan)?;
        }
        if let Some(expected) = &self.expected {
            expected.validate()?;
            ensure!(expected.target == self.target, "Expected target differs");
            if let Some(plan) = &self.plan {
                ensure!(expected.plan_id == plan.plan_id, "Expected plan differs");
            }
            if let Some(release) = &self.sealed_release {
                ensure!(
                    expected.sealed_release_id == release.sealed_release_id,
                    "Expected release differs"
                );
            }
            if let Some(digest) = &self.expected_publication_sha256 {
                ensure!(
                    digest == &expected.canonical_sha256()?,
                    "Expected publication receipt differs"
                );
            }
        }
        if let (Some(expected), Some(activation)) = (&self.expected, &self.activation) {
            activation.validate()?;
            ensure!(
                activation.expected_projection_sha256 == expected.canonical_sha256()?,
                "activation belongs to another Expected projection"
            );
            if let Some(workload) = &self.workload {
                ensure!(
                    workload.runtime_instance_id == activation.runtime_instance_id,
                    "workload belongs to another activation"
                );
            }
            if let Some(digest) = &self.activation_publication_sha256 {
                ensure!(
                    digest == &activation.canonical_sha256()?,
                    "activation publication receipt differs"
                );
            }
        }
        ensure!(
            self.workload.is_none() || self.activation.is_some(),
            "workload observation exists without its prepared activation"
        );
        for evidence in [&self.latest_odin_observation, &self.ready]
            .into_iter()
            .flatten()
        {
            evidence.validate_shape()?;
            ensure!(
                evidence.publisher_sequence <= self.odin_publisher_sequence_cursor,
                "topology evidence exceeds the transaction replay cursor"
            );
        }
        if let Some(warming) = &self.warming {
            warming.validate_shape()?;
            match warming {
                WarmingEvidence::OdinTopology { evidence } => ensure!(
                    evidence.publisher_sequence <= self.odin_publisher_sequence_cursor,
                    "Warming topology evidence exceeds the transaction replay cursor"
                ),
                WarmingEvidence::FirstOdinDirect { .. } => ensure!(
                    self.target == "odin" && self.incumbent_generation_id.is_none(),
                    "direct Warming evidence is reserved for first Odin bootstrap"
                ),
            }
        }
        if let Some(leasing) = &self.leasing {
            if let Some(expected) = &self.expected {
                ensure!(
                    matches!(
                        (expected.write_lease_required, leasing),
                        (true, LeasingEvidence::Prepared { .. })
                            | (true, LeasingEvidence::Granted { .. })
                            | (false, LeasingEvidence::SkippedStateless)
                    ),
                    "lease evidence differs from the Expected state contract"
                );
            }
            let lease_and_sha256 = match leasing {
                LeasingEvidence::SkippedStateless => None,
                LeasingEvidence::Prepared {
                    lease,
                    lease_sha256,
                }
                | LeasingEvidence::Granted {
                    lease,
                    lease_sha256,
                } => Some((lease, lease_sha256)),
            };
            if let Some((lease, lease_sha256)) = lease_and_sha256 {
                lease.validate()?;
                ensure!(
                    lease.canonical_sha256()? == *lease_sha256,
                    "lease evidence digest differs"
                );
            }
        }
        if let (Some(expected), Some(fencing)) = (&self.expected, &self.fencing) {
            match fencing {
                FencingEvidence::SkippedStateless => ensure!(
                    !expected.write_lease_required,
                    "stateful candidate claims stateless fencing"
                ),
                FencingEvidence::Revoked {
                    candidate_lease_path_verified_empty,
                    ..
                } => ensure!(
                    *candidate_lease_path_verified_empty == expected.write_lease_required,
                    "candidate lease-path fencing differs from Expected"
                ),
            }
        }
        if let Some(error) = &self.last_error {
            require_detail(error, "transaction error")?;
        }
        if let Some(authorization) = &self.deployment_authorization {
            authorization.validate_shape()?;
        }
        if let Some(abort) = &self.pre_fencing_abort {
            require_detail(&abort.error, "pre-fencing abort error")?;
            ensure!(
                self.post_commit_cleanup.is_none(),
                "aborted transaction also carries post-commit cleanup"
            );
            ensure!(
                matches!(
                    (
                        self.workload.is_some() || self.activation.is_some(),
                        abort.candidate_cleanup
                    ),
                    (true, CleanupEvidence::Pending | CleanupEvidence::Complete)
                        | (false, CleanupEvidence::Skipped)
                ),
                "abort candidate cleanup differs from its prepared activation"
            );
            let topology_cleanup_required = self.command_kind == CommandKind::Deploy
                && self.expected_publication_sha256.is_some();
            ensure!(
                matches!(
                    (topology_cleanup_required, abort.topology_reconciliation),
                    (true, CleanupEvidence::Pending | CleanupEvidence::Complete)
                        | (false, CleanupEvidence::Skipped)
                ),
                "abort topology cleanup differs from published deployment Expected"
            );
            ensure!(
                matches!(
                    (self.command_kind, abort.source_cleanup),
                    (
                        CommandKind::Deploy,
                        CleanupEvidence::Pending | CleanupEvidence::Complete
                    ) | (CommandKind::Continuity, CleanupEvidence::Skipped)
                ) || (self.command_kind == CommandKind::Deploy
                    && abort.source_cleanup == CleanupEvidence::Skipped
                    && self.plan.is_none()
                    && self.frozen_source.is_none()
                    && self.expected.is_none()
                    && self.phase == DeploymentPhase::Complete),
                "abort source cleanup differs from command work"
            );
            ensure!(
                self.phase < DeploymentPhase::Fencing || self.phase == DeploymentPhase::Complete,
                "pre-fencing abort crossed the fencing boundary"
            );
        }

        let failed = matches!(
            self.completion,
            Some(TransactionCompletion::FailedBeforeFencing { .. })
        );
        if failed {
            ensure!(
                self.phase == DeploymentPhase::Complete,
                "terminal failure is not Complete"
            );
            let abort = required(&self.pre_fencing_abort, "terminal abort evidence")?;
            let TransactionCompletion::FailedBeforeFencing { error } =
                self.completion.as_ref().unwrap()
            else {
                unreachable!()
            };
            ensure!(
                abort.is_complete() && abort.error == *error,
                "terminal failure lacks complete matching abort evidence"
            );
        } else {
            ensure!(
                !matches!(
                    self.completion,
                    Some(TransactionCompletion::FailedBeforeFencing { .. })
                ),
                "non-failure branch carries failure completion"
            );
            if self.phase >= DeploymentPhase::Starting {
                ensure!(
                    self.plan.is_some()
                        && self.sealed_release.is_some()
                        && self.installed_release.is_some()
                        && self.expected.is_some()
                        && self.expected_publication_sha256.is_some(),
                    "Starting transaction lacks sealed release evidence"
                );
                match self.command_kind {
                    CommandKind::Deploy => ensure!(
                        self.frozen_source.is_some() && self.deployment_authorization.is_some(),
                        "deployment entered Starting without frozen source and brake consumption"
                    ),
                    CommandKind::Continuity => ensure!(
                        self.lifecycle_authorized_at_unix_millis.is_some()
                            && self.deployment_authorization.is_none(),
                        "continuity entered Starting without its lifecycle gate"
                    ),
                }
            }
            if self.phase >= DeploymentPhase::Warming {
                ensure!(
                    self.activation.is_some()
                        && self.workload.is_some()
                        && self.activation_publication_sha256.is_some(),
                    "Warming transaction lacks observed activation"
                );
            }
            if self.phase >= DeploymentPhase::Fencing {
                ensure!(
                    self.warming.is_some() && self.isolation.is_some(),
                    "Fencing transaction lacks warming or isolation evidence"
                );
                ensure!(
                    self.expected
                        .as_ref()
                        .and_then(|expected| expected.route.as_ref())
                        .is_some()
                        == self.route_preflight.is_some(),
                    "route preflight does not match routed Expected"
                );
                if let Some(preflight) = &self.route_preflight {
                    preflight.validate()?;
                }
            }
            if self.phase >= DeploymentPhase::Leasing {
                ensure!(self.fencing.is_some(), "Leasing lacks fencing evidence");
            }
            if self.phase >= DeploymentPhase::AwaitingReady {
                ensure!(
                    matches!(
                        self.leasing.as_ref(),
                        Some(LeasingEvidence::SkippedStateless)
                            | Some(LeasingEvidence::Granted { .. })
                    ),
                    "AwaitingReady lacks finalized lease evidence"
                );
            }
            if self.phase >= DeploymentPhase::Routing {
                ensure!(self.ready.is_some(), "Routing lacks Ready evidence");
            }
            if self.phase >= DeploymentPhase::Committing {
                ensure!(self.routing.is_some(), "Committing lacks routing evidence");
            }
            if let Some(RoutingEvidence::Promoted {
                observation,
                promoted_at_unix_millis,
            }) = &self.routing
            {
                observation.validate()?;
                ensure!(
                    *promoted_at_unix_millis > 0
                        && *promoted_at_unix_millis <= self.updated_at_unix_millis,
                    "route promotion time is outside the durable transaction timeline"
                );
            }
            if self.phase == DeploymentPhase::Complete {
                ensure!(
                    matches!(
                        self.completion,
                        Some(TransactionCompletion::Admitted { .. })
                    ),
                    "successful Complete transaction lacks admission receipt"
                );
                ensure!(
                    self.pre_fencing_abort.is_none() && self.post_commit_cleanup.is_some(),
                    "admitted transaction lacks exclusive post-commit cleanup evidence"
                );
                let TransactionCompletion::Admitted { generation_id } =
                    self.completion.as_ref().unwrap()
                else {
                    unreachable!()
                };
                ensure!(
                    generation_id == &format!("generation-{}", self.transaction_id),
                    "admission completion names another transaction generation"
                );
            } else {
                ensure!(
                    self.completion.is_none() && self.post_commit_cleanup.is_none(),
                    "non-Complete transaction has completion or post-commit state"
                );
            }
        }
        if let Some(cleanup) = &self.post_commit_cleanup {
            ensure!(
                self.phase == DeploymentPhase::Complete
                    && matches!(
                        self.completion,
                        Some(TransactionCompletion::Admitted { .. })
                    ),
                "post-commit cleanup exists without an admitted Complete transaction"
            );
            match (&self.incumbent_generation_id, &cleanup.incumbent) {
                (None, IncumbentCleanupEvidence::SkippedNoIncumbent) => {}
                (
                    Some(expected),
                    IncumbentCleanupEvidence::Pending { generation_id, .. }
                    | IncumbentCleanupEvidence::Complete { generation_id },
                ) if expected == generation_id => {}
                _ => bail!("post-commit incumbent cleanup names another generation"),
            }
            ensure!(
                matches!(
                    (self.command_kind, cleanup.source),
                    (CommandKind::Deploy, SourceCleanupEvidence::Pending)
                        | (CommandKind::Deploy, SourceCleanupEvidence::Complete)
                        | (
                            CommandKind::Continuity,
                            SourceCleanupEvidence::SkippedContinuity
                        )
                ),
                "post-commit source cleanup differs from command kind"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmittedOdinAuthority {
    signer_identity_id: String,
    signer_public_key: Vec<u8>,
}

impl AdmittedOdinAuthority {
    fn from_anchor(anchor: &ServiceIdentityTrustAnchor) -> Result<Self> {
        ensure!(
            derive_service_identity_id::<OdinTopologyIdentity>(&anchor.public_key)?
                == anchor.identity_id,
            "Odin topology anchor identity differs from its key"
        );
        Ok(Self {
            signer_identity_id: anchor.identity_id.clone(),
            signer_public_key: anchor.public_key.clone(),
        })
    }

    fn validate(&self) -> Result<()> {
        require_id(&self.signer_identity_id, "admitted Odin signer")?;
        ensure!(
            derive_service_identity_id::<OdinTopologyIdentity>(&self.signer_public_key)?
                == self.signer_identity_id,
            "admitted Odin signer identity differs from its key"
        );
        Ok(())
    }
}

/// The only current-incarnation owner. Unit state, route state, topology
/// projections, and CLI output are observations of this record, never peers.
#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "idunn.admitted_generation",
    schema = "idunn.admitted_generation.v2"
)]
struct AdmittedGeneration {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    target: String,
    #[cultcache(key = 2)]
    generation_id: String,
    #[cultcache(key = 3)]
    command_id: String,
    #[cultcache(key = 4)]
    transaction_id: String,
    #[cultcache(key = 5)]
    admitted_at_unix_millis: u64,
    #[cultcache(key = 6)]
    plan: CompiledDeploymentPlan,
    #[cultcache(key = 7)]
    sealed_release: SealedRelease,
    #[cultcache(key = 8)]
    installed_release: InstalledReleaseObservation,
    #[cultcache(key = 9)]
    expected: IdunnExpectedIncarnationRecord,
    #[cultcache(key = 10)]
    activation: IdunnRuntimeActivationRecord,
    #[cultcache(key = 11)]
    workload: WorkloadObservation,
    #[cultcache(key = 12)]
    leasing: LeasingEvidence,
    #[cultcache(key = 13)]
    ready: TopologyEvidence,
    #[cultcache(key = 14)]
    latest_odin_observation: TopologyEvidence,
    #[cultcache(key = 15)]
    routing: RoutingEvidence,
    #[cultcache(key = 16)]
    odin_authority: AdmittedOdinAuthority,
    #[cultcache(key = 17)]
    odin_publisher_sequence_cursor: u64,
    #[cultcache(key = 18)]
    route_repair_started_at_unix_millis: Option<u64>,
}

impl AdmittedGeneration {
    fn from_transaction(
        transaction: &DeploymentTransaction,
        odin_authority: AdmittedOdinAuthority,
        now: u64,
    ) -> Result<Self> {
        ensure!(
            transaction.phase == DeploymentPhase::Committing,
            "only Committing can create an admitted generation"
        );
        let generation = Self {
            schema_version: ADMITTED_GENERATION_SCHEMA.into(),
            target: transaction.target.clone(),
            generation_id: format!("generation-{}", transaction.transaction_id),
            command_id: transaction.command_id.clone(),
            transaction_id: transaction.transaction_id.clone(),
            admitted_at_unix_millis: now,
            plan: required(&transaction.plan, "transaction plan")?.clone(),
            sealed_release: required(&transaction.sealed_release, "sealed release")?.clone(),
            installed_release: required(&transaction.installed_release, "installed release")?
                .clone(),
            expected: required(&transaction.expected, "Expected projection")?.clone(),
            activation: required(&transaction.activation, "activation")?.clone(),
            workload: required(&transaction.workload, "workload")?.clone(),
            leasing: required(&transaction.leasing, "lease disposition")?.clone(),
            ready: required(&transaction.ready, "Ready receipt")?.clone(),
            latest_odin_observation: required(
                &transaction.latest_odin_observation,
                "latest Odin receipt",
            )?
            .clone(),
            routing: required(&transaction.routing, "route disposition")?.clone(),
            odin_authority,
            odin_publisher_sequence_cursor: transaction.odin_publisher_sequence_cursor,
            route_repair_started_at_unix_millis: None,
        };
        generation.validate()?;
        Ok(generation)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == ADMITTED_GENERATION_SCHEMA,
            "admitted generation schema is unsupported"
        );
        require_id(&self.target, "admitted target")?;
        require_id(&self.generation_id, "admitted generation id")?;
        require_id(&self.command_id, "admitted command id")?;
        require_id(&self.transaction_id, "admitted transaction id")?;
        ensure!(self.admitted_at_unix_millis > 0, "admission has no time");
        self.plan.validate()?;
        self.sealed_release.validate_against(&self.plan)?;
        self.expected.validate()?;
        self.activation.validate()?;
        self.ready.validate_shape()?;
        self.latest_odin_observation.validate_shape()?;
        self.odin_authority.validate()?;
        ensure!(
            self.target == self.expected.target
                && self.expected.plan_id == self.plan.plan_id
                && self.expected.sealed_release_id == self.sealed_release.sealed_release_id
                && self.activation.expected_projection_sha256
                    == self.expected.canonical_sha256()?
                && self.activation.runtime_instance_id == self.workload.runtime_instance_id
                && self.ready.publisher_sequence <= self.odin_publisher_sequence_cursor
                && self.latest_odin_observation.publisher_sequence
                    == self.odin_publisher_sequence_cursor,
            "admitted generation evidence does not describe one incarnation"
        );
        ensure!(
            self.expected.write_lease_required == self.leasing.lease().is_some(),
            "admitted write-lease disposition differs from Expected"
        );
        ensure!(
            self.expected.route.is_some() == self.routing.observation().is_some(),
            "admitted route disposition differs from Expected"
        );
        if let Some(route) = self.routing.observation() {
            route.validate()?;
            ensure!(
                route.runtime_instance_id == self.activation.runtime_instance_id,
                "admitted route observation belongs to another runtime instance"
            );
        }
        if let Some(started_at) = self.route_repair_started_at_unix_millis {
            ensure!(started_at > 0, "admitted route repair has no start time");
            ensure!(
                matches!(&self.routing, RoutingEvidence::Promoted { .. }),
                "only a promoted routed generation can own route repair intent"
            );
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
    idunn_identity_store: PathBuf,
    deployment_brake_operator_anchor: PathBuf,
    source_identity: Option<ProcessIdentity>,
    topology_maximum_age_millis: u64,
    topology_maximum_future_skew_millis: u64,
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
            idunn_identity_store: PathBuf::from(
                "/var/lib/gamecult/idunn/idunn-service-identity.cc",
            ),
            deployment_brake_operator_anchor: PathBuf::from(
                "/etc/gamecult/idunn/deployment-brake-operator-anchor.cc",
            ),
            source_identity: None,
            topology_maximum_age_millis: DEFAULT_TOPOLOGY_MAXIMUM_AGE_MILLIS,
            topology_maximum_future_skew_millis: DEFAULT_TOPOLOGY_MAXIMUM_FUTURE_SKEW_MILLIS,
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
            "--idunn-identity-store" => {
                options.idunn_identity_store = path_value(&mut args, &argument)?
            }
            "--deployment-brake-operator-anchor" => {
                options.deployment_brake_operator_anchor = path_value(&mut args, &argument)?
            }
            "--topology-maximum-age-millis" => {
                options.topology_maximum_age_millis = u64_value(&mut args, &argument)?
            }
            "--topology-maximum-future-skew-millis" => {
                options.topology_maximum_future_skew_millis = u64_value(&mut args, &argument)?
            }
            "--source-uid" => source_uid = Some(u32_value(&mut args, &argument)?),
            "--source-gid" => source_gid = Some(u32_value(&mut args, &argument)?),
            "--poll-millis" => options.poll_millis = u64_value(&mut args, &argument)?,
            "--help" | "-h" => bail!(usage()),
            _ => bail!("unknown Idunn serve option {argument:?}"),
        }
    }
    ensure!(
        options.poll_millis > 0 && options.topology_maximum_age_millis > 0,
        "poll and topology maximum age must be positive"
    );
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

#[derive(Clone)]
struct Stored<T> {
    envelope: CultCacheEnvelope,
    value: T,
}

#[derive(Default)]
struct ControlSnapshot {
    commands: Vec<Stored<DeploymentCommand>>,
    transactions: Vec<Stored<DeploymentTransaction>>,
    admitted: Vec<Stored<AdmittedGeneration>>,
}

impl ControlSnapshot {
    fn read(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let mut snapshot = Self::default();
        for envelope in SingleFileMessagePackBackingStore::new(path)
            .pull_all_read_only_snapshot()
            .context("reading Idunn control snapshot")?
        {
            match envelope.r#type.as_str() {
                DeploymentCommand::TYPE => {
                    ensure!(
                        envelope.schema_id.as_deref() == Some(DEPLOYMENT_COMMAND_SCHEMA),
                        "Idunn control store contains an unsupported command"
                    );
                    let value: DeploymentCommand = decode_record(&envelope)?;
                    value.validate()?;
                    ensure!(
                        envelope.key == value.command_id,
                        "deployment command key differs from its identity"
                    );
                    snapshot.commands.push(Stored { envelope, value });
                }
                DeploymentTransaction::TYPE => {
                    ensure!(
                        envelope.schema_id.as_deref() == Some(DEPLOYMENT_TRANSACTION_SCHEMA),
                        "Idunn control store contains an unsupported transaction"
                    );
                    let value: DeploymentTransaction = decode_record(&envelope)?;
                    value.validate()?;
                    ensure!(
                        envelope.key == value.transaction_id,
                        "deployment transaction key differs from its identity"
                    );
                    snapshot.transactions.push(Stored { envelope, value });
                }
                AdmittedGeneration::TYPE => {
                    ensure!(
                        envelope.schema_id.as_deref() == Some(ADMITTED_GENERATION_SCHEMA),
                        "Idunn control store contains an unsupported admitted generation"
                    );
                    let value: AdmittedGeneration = decode_record(&envelope)?;
                    value.validate()?;
                    ensure!(
                        envelope.key == value.target,
                        "admitted generation key is not its target"
                    );
                    snapshot.admitted.push(Stored { envelope, value });
                }
                _ => bail!("Idunn control store contains a foreign document"),
            }
        }
        snapshot.validate_relations()?;
        Ok(snapshot)
    }

    fn validate_relations(&self) -> Result<()> {
        let command_ids = self
            .commands
            .iter()
            .map(|stored| stored.value.command_id.as_str())
            .collect::<BTreeSet<_>>();
        ensure!(
            command_ids.len() == self.commands.len(),
            "Idunn control store contains duplicate commands"
        );
        let transaction_ids = self
            .transactions
            .iter()
            .map(|stored| stored.value.transaction_id.as_str())
            .collect::<BTreeSet<_>>();
        ensure!(
            transaction_ids.len() == self.transactions.len(),
            "Idunn control store contains duplicate transactions"
        );
        let admitted_targets = self
            .admitted
            .iter()
            .map(|stored| stored.value.target.as_str())
            .collect::<BTreeSet<_>>();
        ensure!(
            admitted_targets.len() == self.admitted.len(),
            "Idunn control store contains duplicate current generations"
        );
        for transaction in &self.transactions {
            let command = self
                .commands
                .iter()
                .find(|candidate| candidate.value.command_id == transaction.value.command_id)
                .context("deployment transaction has no immutable command")?;
            ensure!(
                command.value.kind == transaction.value.command_kind,
                "transaction kind differs from its immutable command"
            );
        }
        let mut live_targets = BTreeSet::new();
        for transaction in self
            .transactions
            .iter()
            .filter(|stored| stored.value.owns_target_authority())
        {
            ensure!(
                live_targets.insert(transaction.value.target.as_str()),
                "multiple transactions claim current-incarnation authority for one target"
            );
            if let Some(fencing) = transaction
                .value
                .fencing
                .as_ref()
                .filter(|_| transaction.value.phase < DeploymentPhase::Complete)
            {
                let incumbent = self.admitted_for(&transaction.value.target);
                let incumbent_matches =
                    match (&transaction.value.incumbent_generation_id, incumbent) {
                        (None, None) => true,
                        (Some(expected), Some(current)) => current.value.generation_id == *expected,
                        _ => false,
                    };
                ensure!(
                    incumbent_matches,
                    "fenced transaction incumbent differs from current admission"
                );
                let incumbent_lease_sha256 = incumbent
                    .and_then(|stored| stored.value.leasing.lease())
                    .map(IdunnProcessWriteLeaseRecord::canonical_sha256)
                    .transpose()?;
                match fencing {
                    FencingEvidence::SkippedStateless => ensure!(
                        incumbent_lease_sha256.is_none(),
                        "stateless fencing omitted the admitted incumbent lease"
                    ),
                    FencingEvidence::Revoked {
                        incumbent_lease_sha256: recorded,
                        ..
                    } => {
                        ensure!(
                            recorded == &incumbent_lease_sha256,
                            "fencing evidence names another incumbent lease"
                        );
                        ensure!(
                            incumbent_lease_sha256.is_some()
                                || transaction
                                    .value
                                    .expected
                                    .as_ref()
                                    .is_some_and(|expected| expected.write_lease_required),
                            "stateless transaction did not use its explicit fencing skip"
                        );
                    }
                }
            }
        }
        let mut authorizations = BTreeSet::new();
        for authorization in self
            .transactions
            .iter()
            .filter_map(|stored| stored.value.deployment_authorization.as_ref())
        {
            ensure!(
                authorizations.insert(authorization.authorization_id.as_str()),
                "deployment authorization was consumed more than once"
            );
        }
        Ok(())
    }

    fn admitted_for(&self, target: &str) -> Option<&Stored<AdmittedGeneration>> {
        self.admitted
            .iter()
            .find(|stored| stored.value.target == target)
    }

    fn transaction_for_command(&self, command_id: &str) -> Vec<&DeploymentTransaction> {
        let mut values = self
            .transactions
            .iter()
            .filter(|stored| stored.value.command_id == command_id)
            .map(|stored| &stored.value)
            .collect::<Vec<_>>();
        values.sort_by_key(|value| value.ordinal);
        values
    }

    fn has_earlier_authority_sibling(&self, transaction: &DeploymentTransaction) -> bool {
        self.transactions.iter().any(|stored| {
            stored.value.owns_target_authority()
                && stored.value.command_id == transaction.command_id
                && stored.value.ordinal < transaction.ordinal
        })
    }

    fn max_odin_sequence(&self, target: &str, signer_identity_id: &str) -> u64 {
        let transaction_max = self
            .transactions
            .iter()
            .filter_map(|stored| {
                stored
                    .value
                    .latest_odin_observation
                    .as_ref()
                    .filter(|evidence| {
                        stored.value.target == target
                            && evidence.signer_identity_id == signer_identity_id
                    })
                    .map(|evidence| evidence.publisher_sequence)
            })
            .max()
            .unwrap_or(0);
        let admitted_max = self
            .admitted
            .iter()
            .filter(|stored| {
                stored.value.target == target
                    && stored.value.odin_authority.signer_identity_id == signer_identity_id
            })
            .map(|stored| stored.value.odin_publisher_sequence_cursor)
            .max()
            .unwrap_or(0);
        transaction_max.max(admitted_max)
    }
}

fn decode_record<T>(envelope: &CultCacheEnvelope) -> Result<T>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let value: T = rmp_serde::from_slice(&envelope.payload)?;
    ensure!(
        rmp_serde::to_vec(&value)? == envelope.payload,
        "Idunn control store contains a noncanonical record"
    );
    Ok(value)
}

fn command_envelope(value: &DeploymentCommand, now: u64) -> Result<CultCacheEnvelope> {
    value.validate()?;
    typed_envelope(
        &value.command_id,
        DeploymentCommand::TYPE,
        DEPLOYMENT_COMMAND_SCHEMA,
        value,
        now,
    )
}

fn transaction_envelope(value: &DeploymentTransaction, now: u64) -> Result<CultCacheEnvelope> {
    value.validate()?;
    typed_envelope(
        &value.transaction_id,
        DeploymentTransaction::TYPE,
        DEPLOYMENT_TRANSACTION_SCHEMA,
        value,
        now,
    )
}

fn admitted_envelope(value: &AdmittedGeneration, now: u64) -> Result<CultCacheEnvelope> {
    value.validate()?;
    typed_envelope(
        &value.target,
        AdmittedGeneration::TYPE,
        ADMITTED_GENERATION_SCHEMA,
        value,
        now,
    )
}

fn typed_envelope<T: Serialize>(
    key: &str,
    record_type: &str,
    schema: &str,
    value: &T,
    now: u64,
) -> Result<CultCacheEnvelope> {
    Ok(CultCacheEnvelope {
        key: key.into(),
        r#type: record_type.into(),
        payload: rmp_serde::to_vec(value)?,
        stored_at: rfc3339_millis(now)?,
        schema_id: Some(schema.into()),
    })
}

fn replace_transaction(
    store_path: &Path,
    current: &Stored<DeploymentTransaction>,
    next: &DeploymentTransaction,
) -> Result<()> {
    next.validate()?;
    ensure!(
        current.value.transaction_id == next.transaction_id,
        "transaction replacement changes identity"
    );
    ensure!(
        SingleFileMessagePackBackingStore::new(store_path).compare_exchange(
            &[CultCacheExpectedEnvelope {
                r#type: DeploymentTransaction::TYPE.into(),
                key: current.value.transaction_id.clone(),
                current: Some(current.envelope.clone()),
            }],
            &[transaction_envelope(next, next.updated_at_unix_millis)?],
        )?,
        "deployment transaction changed before its compare-exchange"
    );
    Ok(())
}

#[derive(Clone)]
struct LoadedBinding {
    binding: OperatorBinding,
    bytes: Vec<u8>,
}

fn load_bindings(directory: &Path) -> Result<BTreeMap<String, LoadedBinding>> {
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
            bindings
                .insert(target.clone(), LoadedBinding { binding, bytes })
                .is_none(),
            "operator binding target {target} is duplicated"
        );
    }
    ensure!(!bindings.is_empty(), "Idunn binding directory is empty");
    validate_route_bindings(&bindings)?;
    Ok(bindings)
}

fn validate_route_bindings(bindings: &BTreeMap<String, LoadedBinding>) -> Result<()> {
    let routes = bindings
        .iter()
        .filter_map(|(target, loaded)| {
            loaded
                .binding
                .route
                .as_ref()
                .map(|route| (target.as_str(), route))
        })
        .collect::<Vec<_>>();
    validate_route_binding_set(&routes)
}

fn validate_route_binding_set(routes: &[(&str, &RouteBinding)]) -> Result<()> {
    for (index, (target, route)) in routes.iter().enumerate() {
        let (stable_host, stable_port) = route.stable_socket()?;
        let private_host = route
            .private_host
            .parse::<std::net::IpAddr>()
            .context("validated route private host stopped being an IP address")?;
        ensure!(
            stable_host != private_host
                || !(route.private_port_start..=route.private_port_end).contains(&stable_port),
            "route {target} stable socket overlaps its candidate port range"
        );

        for (other_target, other) in &routes[index + 1..] {
            ensure!(
                route.route_id != other.route_id,
                "route id {} is shared by targets {target} and {other_target}",
                route.route_id
            );
            ensure!(
                route.config_path != other.config_path,
                "route fragment {} is shared by targets {target} and {other_target}",
                route.config_path.display()
            );
            let other_stable = other.stable_socket()?;
            ensure!(
                route.driver != other.driver || (stable_host, stable_port) != other_stable,
                "stable route socket {}:{} is shared by targets {target} and {other_target}",
                stable_host,
                stable_port
            );
            if route.driver != other.driver {
                continue;
            }
            let other_private_host = other
                .private_host
                .parse::<std::net::IpAddr>()
                .context("validated route private host stopped being an IP address")?;
            let ranges_overlap = route.private_host == other.private_host
                && route.private_port_start <= other.private_port_end
                && other.private_port_start <= route.private_port_end;
            ensure!(
                !ranges_overlap,
                "candidate port ranges overlap for targets {target} and {other_target}"
            );
            ensure!(
                stable_host != other_private_host
                    || !(other.private_port_start..=other.private_port_end).contains(&stable_port),
                "stable route socket for {target} overlaps {other_target}'s candidate range"
            );
            ensure!(
                other_stable.0 != private_host
                    || !(route.private_port_start..=route.private_port_end)
                        .contains(&other_stable.1),
                "stable route socket for {other_target} overlaps {target}'s candidate range"
            );
        }
    }
    Ok(())
}

fn resolve_selector(
    bindings: &BTreeMap<String, LoadedBinding>,
    selector: &str,
) -> Result<Vec<String>> {
    let mut targets = if let Some(profile) = selector.strip_prefix("profile:") {
        bindings
            .values()
            .filter(|binding| binding.binding.profiles.contains(profile))
            .map(|binding| binding.binding.target.clone())
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
    targets.sort_by(|left, right| {
        (left != "odin")
            .cmp(&(right != "odin"))
            .then_with(|| left.cmp(right))
    });
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
    let command = DeploymentCommand {
        schema_version: DEPLOYMENT_COMMAND_SCHEMA.into(),
        command_id: format!("up-{}", Uuid::new_v4()),
        kind: CommandKind::Deploy,
        selector: selector.into(),
        requested_by: requested_by.into(),
        requested_at_unix_millis: now,
    };
    command.validate()?;
    ensure!(
        SingleFileMessagePackBackingStore::new(store_path).compare_exchange(
            &[CultCacheExpectedEnvelope {
                r#type: DeploymentCommand::TYPE.into(),
                key: command.command_id.clone(),
                current: None,
            }],
            &[command_envelope(&command, now)?],
        )?,
        "deployment command id collided"
    );
    println!("{}", command.command_id);
    if !wait {
        return Ok(());
    }
    let deadline = now.saturating_add(timeout_seconds.saturating_mul(1000));
    loop {
        let snapshot = ControlSnapshot::read(store_path)?;
        ensure!(
            snapshot
                .commands
                .iter()
                .any(|stored| stored.value.command_id == command.command_id),
            "submitted deployment command disappeared"
        );
        let transactions = snapshot.transaction_for_command(&command.command_id);
        if !transactions.is_empty() && transactions.iter().all(|value| value.is_terminal()) {
            if let Some(error) =
                transactions
                    .iter()
                    .find_map(|transaction| match &transaction.completion {
                        Some(TransactionCompletion::FailedBeforeFencing { error }) => {
                            Some(error.as_str())
                        }
                        _ => None,
                    })
            {
                bail!("deployment failed: {error}")
            }
            println!("succeeded {} target(s)", transactions.len());
            return Ok(());
        }
        if now_millis()? >= deadline {
            bail!("deployment command timed out")
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn status(store_path: &Path, command_id: Option<&str>) -> Result<()> {
    let snapshot = ControlSnapshot::read(store_path)?;
    let mut commands = snapshot.commands.iter().collect::<Vec<_>>();
    commands.sort_by_key(|stored| stored.value.requested_at_unix_millis);
    if let Some(command_id) = command_id {
        commands.retain(|stored| stored.value.command_id == command_id);
        ensure!(!commands.is_empty(), "deployment command is unknown");
    }
    for stored in commands {
        let command = &stored.value;
        let transactions = snapshot.transaction_for_command(&command.command_id);
        let (state, detail) = derived_command_status(&transactions);
        println!(
            "{} {} {} {}",
            command.command_id, command.selector, state, detail
        );
    }
    Ok(())
}

fn derived_command_status(transactions: &[&DeploymentTransaction]) -> (&'static str, String) {
    if transactions.is_empty() {
        return ("queued", String::new());
    }
    if let Some(error) = transactions
        .iter()
        .find_map(|transaction| match &transaction.completion {
            Some(TransactionCompletion::FailedBeforeFencing { error }) => Some(error.clone()),
            _ => None,
        })
    {
        return ("failed", error);
    }
    if transactions.iter().all(|transaction| {
        transaction.is_terminal()
            && matches!(
                transaction.completion,
                Some(TransactionCompletion::Admitted { .. })
            )
    }) {
        return (
            "succeeded",
            transactions
                .iter()
                .map(|transaction| transaction.target.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let current = transactions
        .iter()
        .filter(|transaction| !transaction.is_terminal())
        .map(|transaction| format!("{}:{:?}", transaction.target, transaction.phase))
        .collect::<Vec<_>>()
        .join(",");
    ("running", current)
}

struct ProcessLock {
    file: File,
    #[cfg(not(unix))]
    path: PathBuf,
}

impl ProcessLock {
    fn acquire(store_path: &Path) -> Result<Self> {
        let path = sibling_path(store_path, ".daemon.lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let file = OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("opening Idunn daemon lock {}", path.display()))?;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            ensure!(result == 0, "another Idunn daemon owns this control store");
            Ok(Self { file })
        }
        #[cfg(not(unix))]
        {
            let file = OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(&path)
                .with_context(|| format!("another Idunn daemon owns {}", path.display()))?;
            Ok(Self { file, path })
        }
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
        #[cfg(not(unix))]
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct Engine {
    options: RuntimeOptions,
    idunn_signer: ServiceIdentitySigner<IdunnServiceIdentity>,
    idunn_anchor: ServiceIdentityTrustAnchor,
    bootstrap_odin_authority: AdmittedOdinAuthority,
    source: GitSourceDriver,
    runner: DockerRunnerDriver,
    workload: SystemdTransientWorkloadDriver,
}

impl Engine {
    fn open(options: RuntimeOptions) -> Result<Self> {
        let idunn_signer =
            open_service_identity_at::<IdunnServiceIdentity>(&options.idunn_identity_store)
                .context("opening Idunn activation identity")?;
        let idunn_anchor = idunn_signer.trust_anchor()?;
        let odin_anchor = read_trust_anchor::<OdinTopologyIdentity>(&options.odin_trust_anchor)
            .context("reading bootstrap Odin topology anchor")?;
        let bootstrap_odin_authority = AdmittedOdinAuthority::from_anchor(&odin_anchor)?;
        let source = GitSourceDriver::new(
            &options.source_root,
            options.staging_root.join("frozen-sources"),
            options.source_identity,
        );
        Ok(Self {
            options,
            idunn_signer,
            idunn_anchor,
            bootstrap_odin_authority,
            source,
            runner: DockerRunnerDriver::default(),
            workload: SystemdTransientWorkloadDriver::default(),
        })
    }

    fn topology(&self) -> CultCacheTopologyDriver {
        CultCacheTopologyDriver {
            projection_store: self.options.topology_store.clone(),
            correlation_store: self.options.odin_correlation_store.clone(),
        }
    }

    fn current_odin_authority(&self, snapshot: &ControlSnapshot) -> Result<AdmittedOdinAuthority> {
        let authority = snapshot
            .admitted_for("odin")
            .map(|stored| stored.value.odin_authority.clone())
            .unwrap_or_else(|| self.bootstrap_odin_authority.clone());
        authority.validate()?;
        Ok(authority)
    }

    fn trusted_topology_context(&self, now: u64) -> OdinTopologyAuthenticationContext {
        OdinTopologyAuthenticationContext {
            trusted_received_at_unix_millis: now,
            maximum_age_millis: self.options.topology_maximum_age_millis,
            maximum_future_skew_millis: self.options.topology_maximum_future_skew_millis,
        }
    }

    fn validate_durable_authority(&self, snapshot: &ControlSnapshot) -> Result<()> {
        let operator_anchor = read_trust_anchor::<IdunnDeploymentBrakeOperatorIdentity>(
            &self.options.deployment_brake_operator_anchor,
        )?;
        for stored in &snapshot.transactions {
            let transaction = &stored.value;
            if let Some(authorization) = &transaction.deployment_authorization {
                authorization.validate_shape()?;
                let record: IdunnDeploymentBrakeRecord =
                    rmp_serde::from_slice(&authorization.canonical_brake_bytes)?;
                verify_idunn_deployment_brake_authorization(&record, &operator_anchor)?;
                let expected = required(&transaction.expected, "authorized Expected projection")?;
                ensure!(
                    record.authorized_release_id.as_deref()
                        == Some(expected.sealed_release_id.as_str())
                        && record.authorized_deployment_id.as_deref()
                            == Some(transaction.transaction_id.as_str())
                        && record.runtime_id == expected.runtime_id,
                    "durable deployment authorization names another release or transaction"
                );
            }
            let lease = transaction
                .leasing
                .as_ref()
                .and_then(LeasingEvidence::lease_sha256);
            if let Some(evidence) = &transaction.latest_odin_observation {
                let authenticated = self.authenticate_topology_bytes(
                    snapshot,
                    transaction,
                    &evidence.canonical_bytes,
                    lease,
                    evidence.admitted_at_unix_millis,
                )?;
                validate_authenticated_evidence(evidence, &authenticated)?;
            }
            if let Some(evidence) = &transaction.warming {
                match evidence {
                    WarmingEvidence::OdinTopology { evidence } => {
                        let authenticated = self.authenticate_topology_bytes(
                            snapshot,
                            transaction,
                            &evidence.canonical_bytes,
                            None,
                            evidence.admitted_at_unix_millis,
                        )?;
                        validate_authenticated_evidence(evidence, &authenticated)?;
                        let incumbent_lease_sha256 =
                            self.incumbent_lease_sha256_for_warming(snapshot, transaction)?;
                        ensure!(
                            is_semantic_warming(
                                required(&transaction.expected, "Warming Expected projection",)?,
                                required(&transaction.activation, "Warming activation")?,
                                incumbent_lease_sha256.as_deref(),
                                &authenticated,
                            )?,
                            "durable Warming gate is not supported by current runtime evidence"
                        );
                    }
                    WarmingEvidence::FirstOdinDirect { evidence } => {
                        self.authenticate_first_odin_warming_presence(
                            transaction,
                            &evidence.message_id,
                            evidence.challenged_at_unix_millis,
                            evidence.admitted_at_unix_millis,
                            &evidence.canonical_bytes,
                        )?;
                    }
                }
            }
            if let Some(evidence) = &transaction.ready {
                let authenticated = self.authenticate_topology_bytes(
                    snapshot,
                    transaction,
                    &evidence.canonical_bytes,
                    lease,
                    evidence.admitted_at_unix_millis,
                )?;
                validate_authenticated_evidence(evidence, &authenticated)?;
                ensure!(
                    is_semantic_ready(&authenticated),
                    "durable Ready label is not exact semantic Ready"
                );
            }
        }
        let odin_authority = self.current_odin_authority(snapshot)?;
        for stored in &snapshot.admitted {
            let generation = &stored.value;
            let authority = self.runtime_authority_parts(
                &generation.plan,
                &generation.expected,
                &generation.activation,
            )?;
            let latest = authenticate_odin_runtime_topology_correlation(
                &generation.latest_odin_observation.canonical_bytes,
                &authority,
                generation.leasing.lease_sha256(),
                &odin_authority.signer_public_key,
                self.trusted_topology_context(
                    generation.latest_odin_observation.admitted_at_unix_millis,
                ),
            )?;
            validate_authenticated_evidence(&generation.latest_odin_observation, &latest)?;
            let ready = authenticate_odin_runtime_topology_correlation(
                &generation.ready.canonical_bytes,
                &authority,
                generation.leasing.lease_sha256(),
                &odin_authority.signer_public_key,
                self.trusted_topology_context(generation.ready.admitted_at_unix_millis),
            )?;
            validate_authenticated_evidence(&generation.ready, &ready)?;
            ensure!(
                is_semantic_ready(&ready),
                "admitted generation Ready label is not exact semantic Ready"
            );
        }
        Ok(())
    }
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
    let _lock = ProcessLock::acquire(&options.state_store)?;
    ControlSnapshot::read(&options.state_store).context("validating all Idunn records")?;
    let engine = Engine::open(options)?;
    engine.validate_durable_authority(&ControlSnapshot::read(&engine.options.state_store)?)?;

    loop {
        let transaction_progress = engine.resume_one_transaction()?;
        let continuity_progress = engine.supervise_one_admitted_generation()?;
        if !transaction_progress && !continuity_progress && engine.freeze_one_queued_command()? {
            continue;
        }
        thread::sleep(Duration::from_millis(engine.options.poll_millis));
    }
}

impl Engine {
    /// Startup and every later loop use the same order: unfinished ownership
    /// work first, admitted-body continuity second, new commands last. A
    /// waiting transaction yields without relaxing ordinal order inside its
    /// own command, so one target brake cannot suspend unrelated continuity.
    fn resume_one_transaction(&self) -> Result<bool> {
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let mut progressed = false;
        let mut candidates = snapshot
            .transactions
            .iter()
            .filter(|stored| !stored.value.is_terminal())
            .collect::<Vec<_>>();
        candidates.sort_by_key(|stored| {
            (
                stored.value.created_at_unix_millis,
                stored.value.ordinal,
                stored.value.transaction_id.as_str(),
            )
        });
        for current in candidates {
            if snapshot.has_earlier_authority_sibling(&current.value) {
                continue;
            }
            if let Err(error) = self.advance_transaction(current) {
                let latest_snapshot = ControlSnapshot::read(&self.options.state_store)?;
                let latest = latest_snapshot
                    .transactions
                    .iter()
                    .find(|stored| stored.value.transaction_id == current.value.transaction_id)
                    .context("transaction disappeared while recording an execution error")?;
                if latest.value.is_terminal() {
                    progressed = true;
                } else if latest.value.phase < DeploymentPhase::Fencing
                    && latest.value.pre_fencing_abort.is_none()
                {
                    self.begin_pre_fencing_abort(latest, error)?;
                } else {
                    self.record_resumable_error(latest, &error)?;
                }
            }
            let after = ControlSnapshot::read(&self.options.state_store)?;
            let live = after
                .transactions
                .iter()
                .find(|stored| stored.value.transaction_id == current.value.transaction_id)
                .context("transaction disappeared while checking scheduler progress")?;
            if live.envelope != current.envelope {
                progressed = true;
            }
        }
        Ok(progressed)
    }

    fn freeze_one_queued_command(&self) -> Result<bool> {
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let transaction_commands = snapshot
            .transactions
            .iter()
            .map(|stored| stored.value.command_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut queued = snapshot
            .commands
            .iter()
            .filter(|stored| {
                stored.value.kind == CommandKind::Deploy
                    && !transaction_commands.contains(stored.value.command_id.as_str())
            })
            .collect::<Vec<_>>();
        queued.sort_by_key(|stored| stored.value.requested_at_unix_millis);
        let Some(command) = queued.first() else {
            return Ok(false);
        };
        let bindings = match load_bindings(&self.options.bindings_dir) {
            Ok(bindings) => bindings,
            Err(error) => {
                eprintln!(
                    "Idunn left command {} queued because operator bindings are invalid: {error:#}",
                    command.value.command_id
                );
                return Ok(false);
            }
        };
        let targets = match resolve_selector(&bindings, &command.value.selector) {
            Ok(targets) => targets,
            Err(error) => {
                let now = now_millis()?;
                let rejected = DeploymentTransaction::rejected(&command.value, error, now)?;
                ensure!(
                    SingleFileMessagePackBackingStore::new(&self.options.state_store)
                        .compare_exchange(
                            &[CultCacheExpectedEnvelope {
                                r#type: DeploymentTransaction::TYPE.into(),
                                key: rejected.transaction_id.clone(),
                                current: None,
                            }],
                            &[transaction_envelope(&rejected, now)?],
                        )?,
                    "bad selector changed before refusal was recorded"
                );
                return Ok(true);
            }
        };
        let busy = snapshot
            .transactions
            .iter()
            .filter(|stored| stored.value.blocks_new_target_mutation())
            .map(|stored| stored.value.target.as_str())
            .collect::<BTreeSet<_>>();
        if targets.iter().any(|target| busy.contains(target.as_str())) {
            return Ok(false);
        }
        let now = now_millis()?;
        let transactions = targets
            .into_iter()
            .enumerate()
            .map(|(ordinal, target)| {
                DeploymentTransaction::new(
                    &command.value,
                    target.clone(),
                    u32::try_from(ordinal)?,
                    snapshot.admitted_for(&target).map(|stored| &stored.value),
                    now,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let expected = transactions
            .iter()
            .map(|transaction| CultCacheExpectedEnvelope {
                r#type: DeploymentTransaction::TYPE.into(),
                key: transaction.transaction_id.clone(),
                current: None,
            })
            .collect::<Vec<_>>();
        let next = transactions
            .iter()
            .map(|transaction| transaction_envelope(transaction, now))
            .collect::<Result<Vec<_>>>()?;
        ensure!(
            SingleFileMessagePackBackingStore::new(&self.options.state_store)
                .compare_exchange(&expected, &next)?,
            "queued command lost its ordered transaction creation CAS"
        );
        Ok(true)
    }

    fn supervise_one_admitted_generation(&self) -> Result<bool> {
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let mut progressed = false;
        let mut admitted = snapshot.admitted.iter().collect::<Vec<_>>();
        admitted.sort_by_key(|stored| stored.value.target.as_str());
        for current in admitted {
            let blocker = snapshot.transactions.iter().find(|stored| {
                stored.value.target == current.value.target
                    && stored.value.blocks_new_target_mutation()
            });
            if blocker.is_some_and(|stored| {
                stored.value.phase >= DeploymentPhase::Fencing
                    && stored.value.phase < DeploymentPhase::Complete
            }) {
                continue;
            }
            match self.supervise_admitted_route(current) {
                Ok(true) => {
                    progressed = true;
                    continue;
                }
                Ok(false) => {}
                Err(error) => eprintln!(
                    "Idunn rejected admitted {} route continuity: {error:#}",
                    current.value.target
                ),
            }
            match self.refresh_admitted_topology(&snapshot, current) {
                Ok(true) => {
                    progressed = true;
                    continue;
                }
                Ok(false) => {}
                Err(error) => eprintln!(
                    "Idunn preserved admitted {} after rejecting topology observation: {error:#}",
                    current.value.target
                ),
            }
            let mut operational_error = None;
            let observation = match self.workload.observe(
                &current.value.expected,
                &current.value.activation,
                &current.value.workload,
            ) {
                Ok(observation) => {
                    let lease_is_missing = if let Some(lease) = current.value.leasing.lease() {
                        let lease_health = (|| -> Result<bool> {
                            let lease_path = current
                                .value
                                .plan
                                .parsed_inputs()?
                                .1
                                .process_write_lease
                                .context("admitted lease has no operator binding")?
                                .record_path;
                            let driver =
                                CultCacheWriteLeaseDriver::new(&current.value.target, lease_path);
                            if driver.observe_exact(lease)? {
                                return Ok(true);
                            }
                            if driver.observe_empty()? {
                                return Ok(false);
                            }
                            bail!("write-lease store contains unexpected authority")
                        })();
                        match lease_health {
                            Ok(true) => false,
                            Ok(false) => {
                                operational_error =
                                    Some(anyhow!("admitted physical write lease is missing"));
                                true
                            }
                            Err(error) => {
                                eprintln!(
                                    "Idunn preserved admitted {} after refusing an unsafe write-lease mutation: {error:#}",
                                    current.value.target
                                );
                                continue;
                            }
                        }
                    } else {
                        false
                    };
                    if lease_is_missing {
                        None
                    } else {
                        Some(observation)
                    }
                }
                Err(error) => {
                    operational_error = Some(error);
                    None
                }
            };
            if let Some(observation) = observation {
                if blocker.is_none()
                    || blocker.is_some_and(|stored| stored.value.phase == DeploymentPhase::Complete)
                {
                    let repair = (|| -> Result<()> {
                        let topology = self.topology();
                        let provider_anchor = self.provider_anchor_for_plan(&current.value.plan)?;
                        if !topology.admitted_runtime_projection_is_exact(
                            &current.value.expected,
                            &provider_anchor,
                            &current.value.activation,
                            current.value.leasing.lease(),
                        )? {
                            ensure!(
                                topology
                                    .publish_expected(&current.value.expected, &provider_anchor,)?
                                    == current.value.expected.canonical_sha256()?,
                                "admitted Expected projection repair differs"
                            );
                            ensure!(
                                topology.publish_observed_activation(
                                    &current.value.expected,
                                    &current.value.activation,
                                    &observation,
                                )? == current.value.activation.canonical_sha256()?,
                                "admitted activation projection repair differs"
                            );
                            if let Some(lease) = current.value.leasing.lease() {
                                ensure!(
                                    topology.publish_process_write_lease(
                                        &current.value.expected,
                                        &current.value.activation,
                                        lease,
                                    )? == lease.canonical_sha256()?,
                                    "admitted write-lease projection repair differs"
                                );
                            }
                        }
                        Ok(())
                    })();
                    if let Err(error) = repair {
                        eprintln!(
                            "Idunn preserved admitted {} after refusing topology projection repair: {error:#}",
                            current.value.target
                        );
                    }
                }
                continue;
            }
            let workload_error = operational_error
                .context("admitted operational state has neither observation nor error")?;
            if let Some(blocker) = blocker {
                if blocker.value.phase < DeploymentPhase::Fencing
                    && blocker.value.pre_fencing_abort.is_none()
                {
                    self.begin_pre_fencing_abort(
                        blocker,
                        anyhow!(
                            "admitted incumbent failed before candidate fencing; deployment yielded to continuity: {workload_error:#}"
                        ),
                    )?;
                    progressed = true;
                }
                continue;
            }
            let now = now_millis()?;
            let command = DeploymentCommand {
                schema_version: DEPLOYMENT_COMMAND_SCHEMA.into(),
                command_id: format!("continuity-{}", Uuid::new_v4()),
                kind: CommandKind::Continuity,
                selector: current.value.target.clone(),
                requested_by: "idunn-continuity".into(),
                requested_at_unix_millis: now,
            };
            command.validate()?;
            let transaction =
                DeploymentTransaction::from_continuity(&command, &current.value, now)?;
            ensure!(
                SingleFileMessagePackBackingStore::new(&self.options.state_store)
                    .compare_exchange(
                        &[
                            CultCacheExpectedEnvelope {
                                r#type: DeploymentCommand::TYPE.into(),
                                key: command.command_id.clone(),
                                current: None,
                            },
                            CultCacheExpectedEnvelope {
                                r#type: DeploymentTransaction::TYPE.into(),
                                key: transaction.transaction_id.clone(),
                                current: None,
                            },
                        ],
                        &[
                            command_envelope(&command, now)?,
                            transaction_envelope(&transaction, now)?,
                        ],
                    )?,
                "continuity scheduling lost its command/transaction CAS"
            );
            progressed = true;
        }
        Ok(progressed)
    }

    fn supervise_admitted_route(&self, current: &Stored<AdmittedGeneration>) -> Result<bool> {
        let Some(expected_route) = current.value.expected.route.as_ref() else {
            ensure!(
                matches!(&current.value.routing, RoutingEvidence::SkippedUnrouted)
                    && current.value.route_repair_started_at_unix_millis.is_none(),
                "unrouted admitted generation carries route authority"
            );
            return Ok(false);
        };
        let RoutingEvidence::Promoted {
            observation,
            promoted_at_unix_millis,
        } = &current.value.routing
        else {
            bail!("routed admitted generation has no promoted route receipt")
        };
        ensure!(
            observation.route_id == expected_route.route_id
                && observation.runtime_instance_id == current.value.activation.runtime_instance_id,
            "admitted route receipt names another incarnation"
        );
        let binding = current.value.plan.parsed_inputs()?.1;
        let lease_driver = if let Some(lease) = current.value.leasing.lease() {
            let lease_path = &binding
                .process_write_lease
                .as_ref()
                .context("stateful admitted generation has no write-lease binding")?
                .record_path;
            let driver = CultCacheWriteLeaseDriver::new(&current.value.target, lease_path);
            ensure!(
                driver.observe_exact(lease)?,
                "admitted process write lease is no longer exact"
            );
            Some(driver)
        } else {
            None
        };
        let route_binding = binding
            .route
            .context("routed admitted generation has no operator route binding")?;
        let driver = NginxRouteDriver::new(route_binding);

        let membership_is_exact =
            driver.observe_membership(&current.value.expected, &observation.membership_sha256)?;
        let now = now_millis()?;
        if membership_is_exact
            && route_observation_is_current(
                observation.observed_at_unix_millis,
                now,
                self.options.topology_maximum_age_millis,
                self.options.topology_maximum_future_skew_millis,
            )
            && current.value.route_repair_started_at_unix_millis.is_none()
        {
            return Ok(false);
        }
        if current.value.route_repair_started_at_unix_millis.is_none() {
            let mut next = current.value.clone();
            next.route_repair_started_at_unix_millis = Some(now);
            next.validate()?;
            ensure!(
                SingleFileMessagePackBackingStore::new(&self.options.state_store)
                    .compare_exchange(
                        &[CultCacheExpectedEnvelope {
                            r#type: AdmittedGeneration::TYPE.into(),
                            key: current.value.target.clone(),
                            current: Some(current.envelope.clone()),
                        }],
                        &[admitted_envelope(&next, now)?],
                    )?,
                "admitted generation changed before route repair intent CAS"
            );
            return Ok(true);
        }
        driver
            .restore_admitted_membership(&current.value.expected, &observation.membership_sha256)?;
        let authority = self.runtime_authority_parts(
            &current.value.plan,
            &current.value.expected,
            &current.value.activation,
        )?;
        let refreshed = self.prove_stable_route_against(
            &current.value.expected,
            &current.value.activation,
            &authority,
            current.value.leasing.lease_sha256(),
            &driver,
            observation.membership_sha256.clone(),
        )?;
        ensure!(
            driver.observe_membership(&current.value.expected, &refreshed.membership_sha256)?,
            "admitted route membership changed during its continuity challenge"
        );
        if let (Some(lease_driver), Some(lease)) = (&lease_driver, current.value.leasing.lease()) {
            ensure!(
                lease_driver.observe_exact(lease)?,
                "admitted process write lease changed during its continuity challenge"
            );
        }
        let mut next = current.value.clone();
        next.routing = RoutingEvidence::Promoted {
            observation: refreshed,
            promoted_at_unix_millis: *promoted_at_unix_millis,
        };
        next.route_repair_started_at_unix_millis = None;
        next.validate()?;
        ensure!(
            SingleFileMessagePackBackingStore::new(&self.options.state_store).compare_exchange(
                &[CultCacheExpectedEnvelope {
                    r#type: AdmittedGeneration::TYPE.into(),
                    key: current.value.target.clone(),
                    current: Some(current.envelope.clone()),
                }],
                &[admitted_envelope(&next, now)?],
            )?,
            "admitted generation changed before route continuity receipt CAS"
        );
        Ok(true)
    }

    fn refresh_admitted_topology(
        &self,
        snapshot: &ControlSnapshot,
        current: &Stored<AdmittedGeneration>,
    ) -> Result<bool> {
        let Some(received) = self.topology().receive(&current.value.target)? else {
            return Ok(false);
        };
        ensure!(
            received.target == current.value.target,
            "topology transport substituted admitted target"
        );
        let now = now_millis()?;
        let authority = self.runtime_authority_parts(
            &current.value.plan,
            &current.value.expected,
            &current.value.activation,
        )?;
        let odin_authority = self.current_odin_authority(snapshot)?;
        let authenticated = authenticate_odin_runtime_topology_correlation(
            &received.canonical_bytes,
            &authority,
            current.value.leasing.lease_sha256(),
            &odin_authority.signer_public_key,
            self.trusted_topology_context(now),
        )?;
        let evidence = TopologyEvidence::from_authenticated(&authenticated, now)?;
        if !sequence_requires_admission(
            Some(&current.value.latest_odin_observation),
            snapshot.max_odin_sequence(&current.value.target, &evidence.signer_identity_id),
            &evidence,
        )? {
            return Ok(false);
        }
        let mut next = current.value.clone();
        next.latest_odin_observation = evidence.clone();
        next.odin_publisher_sequence_cursor = evidence.publisher_sequence;
        if is_semantic_ready(&authenticated) {
            next.ready = evidence;
        }
        next.validate()?;
        ensure!(
            SingleFileMessagePackBackingStore::new(&self.options.state_store).compare_exchange(
                &[CultCacheExpectedEnvelope {
                    r#type: AdmittedGeneration::TYPE.into(),
                    key: current.value.target.clone(),
                    current: Some(current.envelope.clone()),
                }],
                &[admitted_envelope(&next, now)?],
            )?,
            "admitted generation changed before topology sequence CAS"
        );
        Ok(true)
    }

    fn advance_transaction(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        if current.value.pre_fencing_abort.is_some() && current.value.completion.is_none() {
            return self.advance_pre_fencing_abort(current);
        }
        match current.value.phase {
            DeploymentPhase::Sealing => self.advance_sealing(current),
            DeploymentPhase::Starting => self.advance_starting(current),
            DeploymentPhase::Warming => self.advance_warming(current),
            DeploymentPhase::Fencing => self.advance_fencing(current),
            DeploymentPhase::Leasing => self.advance_leasing(current),
            DeploymentPhase::AwaitingReady => self.advance_awaiting_ready(current),
            DeploymentPhase::Routing => self.advance_routing(current),
            DeploymentPhase::Committing => self.advance_committing(current),
            DeploymentPhase::Complete => self.advance_post_commit_cleanup(current),
        }
    }

    fn advance_sealing(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        if current.value.plan.is_none() {
            ensure!(
                current.value.command_kind == CommandKind::Deploy,
                "continuity transaction lost its admitted plan"
            );
            let bindings = load_bindings(&self.options.bindings_dir)?;
            let loaded = bindings
                .get(&current.value.target)
                .context("operator binding disappeared before sealing")?;
            let now = now_millis()?;
            let resolved =
                self.source
                    .resolve(&loaded.binding, &current.value.transaction_id, now)?;
            let providers = self.current_ready_provider_tokens(
                &ControlSnapshot::read(&self.options.state_store)?,
                now,
            )?;
            let candidate_port = self.select_candidate_port(&loaded.binding)?;
            let plan = compile_deployment_plan(
                &resolved.recipe_bytes,
                &loaded.bytes,
                resolved.facts,
                format!("incarnation-{}", current.value.transaction_id),
                candidate_port,
                now,
                &providers,
            )?;
            return self.persist_same_phase(current, |next| {
                next.plan = Some(plan);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }

        if current.value.command_kind == CommandKind::Deploy
            && current.value.frozen_source.is_none()
        {
            let now = now_millis()?;
            let plan = required(&current.value.plan, "transaction plan")?;
            let receipt = self.source.freeze(&current.value.transaction_id, plan)?;
            return self.persist_same_phase(current, |next| {
                next.frozen_source = Some(receipt);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }

        if current.value.sealed_release.is_none()
            || current.value.installed_release.is_none()
            || current.value.expected.is_none()
        {
            ensure!(
                current.value.command_kind == CommandKind::Deploy,
                "continuity transaction lost its admitted release evidence"
            );
            let now = now_millis()?;
            let plan = required(&current.value.plan, "transaction plan")?;
            let frozen_receipt = required(&current.value.frozen_source, "frozen source")?;
            let frozen = self.source.observe_frozen(plan, frozen_receipt)?;
            let materialized =
                self.runner
                    .materialize(&frozen, plan, &self.options.staging_root, now)?;
            let installed = self.workload.install(plan, &materialized)?;
            let expected = materialized.release.expected_projection(plan)?;
            return self.persist_same_phase(current, |next| {
                next.sealed_release = Some(materialized.release);
                next.installed_release = Some(installed);
                next.expected = Some(expected);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }

        if current.value.expected_publication_sha256.is_none() {
            let now = now_millis()?;
            let plan = required(&current.value.plan, "transaction plan")?;
            let expected = required(&current.value.expected, "Expected projection")?;
            let provider_anchor = self.provider_anchor_for_plan(plan)?;
            let digest = self
                .topology()
                .publish_expected(expected, &provider_anchor)?;
            return self.persist_same_phase(current, |next| {
                next.expected_publication_sha256 = Some(digest);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }

        let now = now_millis()?;
        let mut next = current.value.clone();
        match current.value.command_kind {
            CommandKind::Deploy => {
                let Some(authorization) = self.deployment_authorization(current, now)? else {
                    return self.record_gate_wait(
                        current,
                        "deployment brake has not released this exact transaction",
                    );
                };
                next.deployment_authorization = Some(authorization);
            }
            CommandKind::Continuity => {
                if !self.lifecycle_allows(current, now)? {
                    return self
                        .record_gate_wait(current, "lifecycle brake denies continuity restart");
                }
                next.lifecycle_authorized_at_unix_millis = Some(now);
            }
        }
        validate_live_providers_for_deploy(current.value.command_kind, || {
            self.validate_selected_providers_current(
                required(&current.value.plan, "transaction plan")?,
                now,
            )
        })?;
        next.phase = DeploymentPhase::Starting;
        next.updated_at_unix_millis = now;
        next.last_error = None;
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn advance_starting(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let expected = required(&current.value.expected, "Expected projection")?;
        let plan = required(&current.value.plan, "transaction plan")?;
        let release = required(&current.value.sealed_release, "sealed release")?;
        let installed = required(&current.value.installed_release, "installed release")?;
        if current.value.activation.is_none() {
            validate_live_providers_for_deploy(current.value.command_kind, || {
                self.validate_selected_providers_current(plan, now_millis()?)
            })?;
            let now = now_millis()?;
            let runtime_instance_id = runtime_instance_id(&current.value.transaction_id)?;
            let launch = IdunnRuntimeActivationLaunch::issue(
                expected,
                runtime_instance_id,
                now,
                &self.idunn_signer,
            )?;
            let activation = self.workload.prepare_activation(plan, expected, launch)?;
            return self.persist_same_phase(current, |next| {
                next.activation = Some(activation);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }
        if current.value.workload.is_none() {
            let now = now_millis()?;
            let activation = required(&current.value.activation, "activation")?;
            let observation = self
                .workload
                .start_prepared(plan, release, installed, expected, activation)?;
            return self.persist_same_phase(current, |next| {
                next.workload = Some(observation);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }
        if current.value.activation_publication_sha256.is_none() {
            let now = now_millis()?;
            let activation = required(&current.value.activation, "activation")?;
            let workload = required(&current.value.workload, "workload")?;
            self.workload.observe(expected, activation, workload)?;
            let digest = self
                .topology()
                .publish_observed_activation(expected, activation, workload)?;
            return self.persist_same_phase(current, |next| {
                next.activation_publication_sha256 = Some(digest);
                next.updated_at_unix_millis = now;
                next.last_error = None;
                Ok(())
            });
        }
        let mut next = current.value.clone();
        next.phase = DeploymentPhase::Warming;
        next.updated_at_unix_millis = now_millis()?;
        next.last_error = None;
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn advance_warming(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let expected = required(&current.value.expected, "Expected projection")?;
        let activation = required(&current.value.activation, "activation")?;
        let workload = required(&current.value.workload, "workload")?;
        self.workload.observe(expected, activation, workload)?;
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let incumbent_lease_sha256 =
            self.incumbent_lease_sha256_for_warming(&snapshot, &current.value)?;
        if current.value.warming.is_none() {
            if current.value.target == "odin" && snapshot.admitted_for("odin").is_none() {
                ensure!(
                    current.value.incumbent_generation_id.is_none()
                        && expected.write_lease_required,
                    "first Odin bootstrap must be a stateful first incarnation"
                );
                let (evidence, present) = self.observe_first_odin_warming(&current.value)?;
                let _token = SequenceAdmittedWarming::from_first_odin_presence(
                    current.value.transaction_id.clone(),
                    evidence.clone(),
                    present,
                )?;
                return self.persist_same_phase(current, |next| {
                    next.warming = Some(WarmingEvidence::FirstOdinDirect { evidence });
                    Ok(())
                });
            }
            let Some((admitted, authenticated)) = self.admit_latest_topology(current, None)? else {
                return Ok(());
            };
            if admitted.envelope != current.envelope {
                return Ok(());
            }
            let semantic_warming = is_semantic_warming(
                expected,
                activation,
                incumbent_lease_sha256.as_deref(),
                &authenticated,
            )?;
            let latest = required(
                &admitted.value.latest_odin_observation,
                "sequence-admitted warming evidence",
            )?;
            if semantic_warming {
                let evidence = latest.clone();
                let _token = SequenceAdmittedWarming::from_topology(
                    admitted.value.transaction_id.clone(),
                    evidence.clone(),
                    authenticated,
                )?;
                return self.persist_same_phase(&admitted, |next| {
                    next.warming = Some(WarmingEvidence::OdinTopology { evidence });
                    Ok(())
                });
            } else {
                return Ok(());
            }
        }
        let warming = self.rehydrate_warming_token(&current.value, now_millis()?, false)?;
        ensure!(
            warming.transaction_id() == current.value.transaction_id
                && warming.runtime_instance_id() == activation.runtime_instance_id.as_str(),
            "durable Warming evidence belongs to another candidate"
        );

        if expected.route.is_some() && current.value.route_preflight.is_none() {
            let now = now_millis()?;
            let snapshot = ControlSnapshot::read(&self.options.state_store)?;
            let incumbent = self.exact_incumbent(&snapshot, &current.value)?;
            let incumbent_route =
                incumbent.and_then(|generation| generation.value.routing.observation().cloned());
            let binding = current.value.plan.as_ref().unwrap().parsed_inputs()?.1;
            let route_binding = binding
                .route
                .context("routed Expected lost route binding")?;
            let driver = NginxRouteDriver::new(route_binding);
            let receipt = driver.preflight(
                expected,
                &activation.runtime_instance_id,
                incumbent_route.as_ref(),
            )?;
            return self.persist_same_phase(current, |next| {
                next.route_preflight = Some(receipt);
                next.updated_at_unix_millis = now;
                Ok(())
            });
        }

        if current.value.isolation.is_none() {
            let now = now_millis()?;
            let snapshot = ControlSnapshot::read(&self.options.state_store)?;
            let incumbent = self.exact_incumbent(&snapshot, &current.value)?;
            let isolation =
                prove_isolation(workload, incumbent.map(|value| &value.value.workload))?;
            return self.persist_same_phase(current, |next| {
                next.isolation = Some(isolation);
                next.updated_at_unix_millis = now;
                Ok(())
            });
        }

        self.transition(current, DeploymentPhase::Fencing)
    }

    fn advance_fencing(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        if current.value.fencing.is_none() {
            let now = now_millis()?;
            let snapshot = ControlSnapshot::read(&self.options.state_store)?;
            let incumbent = self.exact_incumbent(&snapshot, &current.value)?;
            let incumbent_lease = incumbent.and_then(|generation| generation.value.leasing.lease());
            let expected = required(&current.value.expected, "Expected projection")?;
            let evidence = if expected.write_lease_required || incumbent_lease.is_some() {
                let incumbent_lease_path = incumbent_lease
                    .map(|_| -> Result<PathBuf> {
                        Ok(incumbent
                            .context("incumbent lease has no admitted generation")?
                            .value
                            .plan
                            .parsed_inputs()?
                            .1
                            .process_write_lease
                            .context("incumbent lease has no admitted binding")?
                            .record_path)
                    })
                    .transpose()?;
                if let (Some(lease), Some(path)) = (incumbent_lease, &incumbent_lease_path) {
                    let incumbent = incumbent.context("incumbent lease lost its generation")?;
                    self.workload
                        .stop(&incumbent.value.workload)
                        .context("stopping the exact incumbent before revoking its lifetime-held write lease")?;
                    let driver = CultCacheWriteLeaseDriver::new(&current.value.target, path);
                    driver.revoke_exact(Some(lease))?;
                    ensure!(
                        driver.observe_empty()?,
                        "incumbent write lease remained after exact fencing"
                    );
                    self.topology().withdraw_process_write_lease(
                        &incumbent.value.expected,
                        &incumbent.value.activation,
                        Some(lease),
                    )?;
                }
                let candidate_lease_path = if expected.write_lease_required {
                    Some(
                        current
                            .value
                            .plan
                            .as_ref()
                            .unwrap()
                            .parsed_inputs()?
                            .1
                            .process_write_lease
                            .context("stateful candidate has no write-lease binding")?
                            .record_path,
                    )
                } else {
                    None
                };
                if let Some(path) = &candidate_lease_path {
                    if incumbent_lease_path.as_ref() != Some(path) {
                        let driver = CultCacheWriteLeaseDriver::new(&current.value.target, path);
                        driver.revoke_exact(None)?;
                        ensure!(
                            driver.observe_empty()?,
                            "candidate write-lease path was not empty before grant"
                        );
                    }
                }
                self.topology().withdraw_process_write_lease(
                    expected,
                    required(&current.value.activation, "candidate activation")?,
                    None,
                )?;
                FencingEvidence::Revoked {
                    incumbent_lease_sha256: incumbent_lease
                        .map(IdunnProcessWriteLeaseRecord::canonical_sha256)
                        .transpose()?,
                    candidate_lease_path_verified_empty: candidate_lease_path.is_some(),
                }
            } else {
                FencingEvidence::SkippedStateless
            };
            return self.persist_same_phase(current, |next| {
                next.fencing = Some(evidence);
                next.updated_at_unix_millis = now;
                Ok(())
            });
        }
        self.transition(current, DeploymentPhase::Leasing)
    }

    fn advance_leasing(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let expected = required(&current.value.expected, "Expected projection")?;
        if !expected.write_lease_required {
            if current.value.leasing.is_none() {
                let now = now_millis()?;
                return self.persist_same_phase(current, |next| {
                    next.leasing = Some(LeasingEvidence::SkippedStateless);
                    next.updated_at_unix_millis = now;
                    Ok(())
                });
            }
            return self.transition(current, DeploymentPhase::AwaitingReady);
        }

        let binding = current.value.plan.as_ref().unwrap().parsed_inputs()?.1;
        let lease_path = binding
            .process_write_lease
            .context("stateful target has no write-lease binding")?
            .record_path;
        let driver = CultCacheWriteLeaseDriver::new(&current.value.target, lease_path);

        if matches!(
            current.value.leasing.as_ref(),
            Some(LeasingEvidence::Granted { .. })
        ) {
            let activation = required(&current.value.activation, "activation")?;
            let warming = self.rehydrate_warming_token(&current.value, now_millis()?, false)?;
            let lease = current
                .value
                .leasing
                .as_ref()
                .and_then(LeasingEvidence::lease)
                .context("granted Leasing evidence has no write lease")?;
            let recorded_sha256 = current
                .value
                .leasing
                .as_ref()
                .and_then(LeasingEvidence::lease_sha256)
                .context("granted Leasing evidence has no lease digest")?;
            ensure!(
                driver.observe_exact(lease)?,
                "physical write lease disappeared after Granted became durable"
            );
            self.workload.observe(
                expected,
                activation,
                required(&current.value.workload, "candidate workload")?,
            )?;
            ensure!(
                driver.grant(expected, activation, &warming, lease)? == recorded_sha256,
                "replayed physical write lease differs from Granted evidence"
            );
            ensure!(
                self.topology()
                    .publish_process_write_lease(expected, activation, lease)?
                    == recorded_sha256,
                "replayed write-lease projection differs from Granted evidence"
            );
            return self.transition(current, DeploymentPhase::AwaitingReady);
        }

        if let Some((lease, prepared_sha256)) = current
            .value
            .leasing
            .as_ref()
            .and_then(LeasingEvidence::prepared_lease)
        {
            let now = now_millis()?;
            let activation = required(&current.value.activation, "activation")?;
            let historical_warming = self.rehydrate_warming_token(&current.value, now, false)?;
            let physical_is_exact = driver.observe_exact(lease)?;
            let warming = if physical_is_exact {
                historical_warming
            } else {
                ensure!(
                    driver.observe_empty()?,
                    "candidate write-lease store contains authority other than its durable prepared lease"
                );
                match self.rehydrate_warming_token(&current.value, now, true) {
                    Ok(warming) => warming,
                    Err(_) => {
                        let Some((admitted, fresh_evidence, fresh_warming)) =
                            self.fresh_warming_for_lease(current, now)?
                        else {
                            return Ok(());
                        };
                        let replacement = self.prepare_candidate_write_lease(
                            &admitted.value,
                            &fresh_warming,
                            now,
                        )?;
                        let replacement_sha256 = replacement.canonical_sha256()?;
                        return self.persist_same_phase(&admitted, |next| {
                            next.warming = Some(fresh_evidence);
                            next.leasing = Some(LeasingEvidence::Prepared {
                                lease: replacement,
                                lease_sha256: replacement_sha256,
                            });
                            next.updated_at_unix_millis = now;
                            Ok(())
                        });
                    }
                }
            };
            self.workload.observe(
                expected,
                activation,
                required(&current.value.workload, "candidate workload")?,
            )?;
            if !physical_is_exact {
                ensure!(
                    driver.observe_empty()?,
                    "candidate write-lease store changed before physical grant"
                );
            }
            let granted_sha256 = driver.grant(expected, activation, &warming, lease)?;
            ensure!(
                granted_sha256 == prepared_sha256,
                "granted write lease differs from the durable prepared lease"
            );
            let projected_sha256 = self
                .topology()
                .publish_process_write_lease(expected, activation, lease)?;
            ensure!(
                projected_sha256 == granted_sha256,
                "projected write lease differs from the granted process authority"
            );
            let lease = lease.clone();
            return self.persist_same_phase(current, |next| {
                next.leasing = Some(LeasingEvidence::Granted {
                    lease,
                    lease_sha256: projected_sha256,
                });
                next.updated_at_unix_millis = now;
                Ok(())
            });
        }

        ensure!(
            current.value.leasing.is_none(),
            "stateful Leasing phase carries invalid lease evidence"
        );
        ensure!(
            driver.observe_empty()?,
            "candidate write-lease store contains authority before lease preparation"
        );
        let now = now_millis()?;
        let Some((admitted, fresh_evidence, fresh_warming)) =
            self.fresh_warming_for_lease(current, now)?
        else {
            return Ok(());
        };
        let lease = self.prepare_candidate_write_lease(&admitted.value, &fresh_warming, now)?;
        let lease_sha256 = lease.canonical_sha256()?;
        self.persist_same_phase(&admitted, |next| {
            next.warming = Some(fresh_evidence);
            next.leasing = Some(LeasingEvidence::Prepared {
                lease,
                lease_sha256,
            });
            next.updated_at_unix_millis = now;
            Ok(())
        })
    }

    fn advance_awaiting_ready(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let now = now_millis()?;
        let expected = required(&current.value.expected, "Expected projection")?;
        if expected.write_lease_required {
            let activation = required(&current.value.activation, "activation")?;
            let warming = self.rehydrate_warming_token(&current.value, now, false)?;
            let lease = current
                .value
                .leasing
                .as_ref()
                .and_then(LeasingEvidence::lease)
                .context("stateful transaction has no process write lease")?;
            let binding = current.value.plan.as_ref().unwrap().parsed_inputs()?.1;
            let path = binding
                .process_write_lease
                .context("stateful target has no write-lease binding")?
                .record_path;
            ensure!(
                CultCacheWriteLeaseDriver::new(&current.value.target, path)
                    .observe(expected, activation, &warming, lease)?,
                "candidate process write lease is no longer exact"
            );
        }
        {
            let current_lease = current
                .value
                .leasing
                .as_ref()
                .and_then(LeasingEvidence::lease_sha256);
            let Some((admitted, authenticated)) =
                self.admit_latest_topology(current, current_lease)?
            else {
                return Ok(());
            };
            if admitted.envelope != current.envelope {
                return Ok(());
            }
            let semantic_ready = is_semantic_ready(&authenticated);
            let latest = required(
                &admitted.value.latest_odin_observation,
                "sequence-admitted Ready evidence",
            )?;
            if current.value.ready.as_ref() == Some(latest) {
                ensure!(semantic_ready, "stored Ready receipt changed meaning");
            } else if semantic_ready {
                let evidence = latest.clone();
                let _token = SequenceAdmittedReady {
                    transaction_id: admitted.value.transaction_id.clone(),
                    evidence: evidence.clone(),
                    expected: expected.clone(),
                    authenticated,
                };
                return self.persist_same_phase(&admitted, |next| {
                    next.ready = Some(evidence);
                    Ok(())
                });
            } else {
                return Ok(());
            }
        }
        self.transition(current, DeploymentPhase::Routing)
    }

    fn advance_routing(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let expected = required(&current.value.expected, "Expected projection")?;
        let activation = required(&current.value.activation, "activation")?;
        if current.value.routing.is_none() {
            let current_lease = current
                .value
                .leasing
                .as_ref()
                .and_then(LeasingEvidence::lease_sha256);
            let Some((admitted, authenticated)) =
                self.admit_latest_topology(current, current_lease)?
            else {
                return Ok(());
            };
            if admitted.envelope != current.envelope {
                return Ok(());
            }
            ensure!(
                is_semantic_ready(&authenticated),
                "latest Odin observation is not Ready at route admission"
            );
            let latest = required(
                &admitted.value.latest_odin_observation,
                "current route-admission topology evidence",
            )?;
            if admitted.value.ready.as_ref() != Some(latest) {
                let latest = latest.clone();
                return self.persist_same_phase(&admitted, |next| {
                    next.ready = Some(latest);
                    Ok(())
                });
            }
            let ready = self.rehydrate_ready_token(&admitted.value, now_millis()?, true)?;
            ensure!(
                ready.transaction_id() == admitted.value.transaction_id,
                "Ready token belongs to another transaction"
            );
            validate_live_providers_for_deploy(admitted.value.command_kind, || {
                self.validate_selected_providers_current(
                    required(&admitted.value.plan, "transaction plan")?,
                    now_millis()?,
                )
            })?;
            self.ensure_transaction_write_lease_current(&admitted.value, now_millis()?)?;
            let evidence = if expected.route.is_some() {
                let binding = admitted.value.plan.as_ref().unwrap().parsed_inputs()?.1;
                let route_binding = binding.route.context("routed plan has no route binding")?;
                let preflight = required(&admitted.value.route_preflight, "route preflight")?;
                let driver = NginxRouteDriver::new(route_binding);
                ensure!(
                    preflight.candidate_runtime_instance_id == activation.runtime_instance_id,
                    "route preflight belongs to another runtime instance"
                );
                let rollback_allowed = may_rollback_route_after_failed_proof(required(
                    &admitted.value.fencing,
                    "route fencing evidence",
                )?);
                let membership_sha256 = driver.install(
                    expected,
                    &activation.runtime_instance_id,
                    preflight,
                    rollback_allowed,
                )?;
                let observation = match self.prove_stable_route(
                    &admitted.value,
                    &driver,
                    membership_sha256,
                ) {
                    Ok(observation) => observation,
                    Err(proof_error) if !rollback_allowed => {
                        return Err(proof_error).context(
                            "incumbent authority was fenced; candidate route remains installed for fail-closed retry",
                        );
                    }
                    Err(proof_error) => {
                        return match driver.rollback(
                            expected,
                            &activation.runtime_instance_id,
                            preflight,
                        ) {
                            Ok(()) => Err(proof_error)
                                .context("candidate did not answer its stable route challenge"),
                            Err(rollback_error) => Err(proof_error).context(format!(
                                "candidate did not answer its stable route challenge; exact route rollback also failed: {rollback_error:#}"
                            )),
                        };
                    }
                };
                ensure!(
                    driver.observe_membership(expected, &observation.membership_sha256)?,
                    "route membership changed during its signed stable-listener observation"
                );
                self.ensure_transaction_write_lease_current(&admitted.value, now_millis()?)?;
                let promoted_at_unix_millis = observation.observed_at_unix_millis;
                RoutingEvidence::Promoted {
                    observation,
                    promoted_at_unix_millis,
                }
            } else {
                RoutingEvidence::SkippedUnrouted
            };
            let now = now_millis()?;
            return self.persist_same_phase(&admitted, |next| {
                next.routing = Some(evidence);
                next.updated_at_unix_millis = now;
                Ok(())
            });
        }
        self.transition(current, DeploymentPhase::Committing)
    }

    fn advance_committing(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let current_lease_sha256 = current
            .value
            .leasing
            .as_ref()
            .and_then(LeasingEvidence::lease_sha256)
            .map(str::to_owned);
        let Some((ready_current, authenticated)) =
            self.admit_latest_topology(current, current_lease_sha256.as_deref())?
        else {
            return Ok(());
        };
        if ready_current.envelope != current.envelope {
            return Ok(());
        }
        ensure!(
            is_semantic_ready(&authenticated),
            "latest Odin observation is not Ready at admission commit"
        );
        ensure!(
            ready_current.value.latest_odin_observation.as_ref()
                == ready_current.value.ready.as_ref(),
            "latest Odin observation differs from the durable Ready receipt at admission commit"
        );
        self.rehydrate_ready_token(&ready_current.value, now_millis()?, true)?;
        validate_live_providers_for_deploy(ready_current.value.command_kind, || {
            self.validate_selected_providers_current(
                required(&ready_current.value.plan, "transaction plan")?,
                now_millis()?,
            )
        })?;

        let expected = required(&ready_current.value.expected, "Expected projection")?;
        let activation = required(&ready_current.value.activation, "activation")?;
        let workload = required(&ready_current.value.workload, "workload")?;
        self.workload.observe(expected, activation, workload)?;
        self.ensure_transaction_write_lease_current(&ready_current.value, now_millis()?)?;
        if let Some(route) = ready_current
            .value
            .routing
            .as_ref()
            .and_then(RoutingEvidence::observation)
        {
            let binding = ready_current
                .value
                .plan
                .as_ref()
                .unwrap()
                .parsed_inputs()?
                .1;
            let driver =
                NginxRouteDriver::new(binding.route.context("routed plan has no route binding")?);
            ensure!(
                driver.observe_membership(expected, &route.membership_sha256)?,
                "route membership changed before admission commit"
            );
            let current_route = self.prove_stable_route(
                &ready_current.value,
                &driver,
                route.membership_sha256.clone(),
            )?;
            ensure!(
                current_route.route_id == route.route_id
                    && current_route.runtime_instance_id == route.runtime_instance_id,
                "stable route changed incarnation before admission commit"
            );
            ensure!(
                driver.observe_membership(expected, &route.membership_sha256)?,
                "route membership changed during the final signed admission challenge"
            );
            self.ensure_transaction_write_lease_current(&ready_current.value, now_millis()?)?;
        }

        let current_lease_sha256 = ready_current
            .value
            .leasing
            .as_ref()
            .and_then(LeasingEvidence::lease_sha256)
            .map(str::to_owned);
        let Some((commit_current, authenticated)) =
            self.admit_latest_topology(&ready_current, current_lease_sha256.as_deref())?
        else {
            return Ok(());
        };
        if commit_current.envelope != ready_current.envelope {
            return Ok(());
        }
        ensure!(
            is_semantic_ready(&authenticated),
            "latest Odin observation is not Ready after the final admission challenge"
        );
        ensure!(
            commit_current.value.latest_odin_observation.as_ref()
                == commit_current.value.ready.as_ref(),
            "latest Odin observation differs from the durable Ready receipt after the final admission challenge"
        );
        let now = now_millis()?;
        self.rehydrate_ready_token(&commit_current.value, now, true)?;
        validate_live_providers_for_deploy(commit_current.value.command_kind, || {
            self.validate_selected_providers_current(
                required(&commit_current.value.plan, "transaction plan")?,
                now,
            )
        })?;
        self.workload.observe(
            required(&commit_current.value.expected, "Expected projection")?,
            required(&commit_current.value.activation, "activation")?,
            required(&commit_current.value.workload, "workload")?,
        )?;
        self.ensure_transaction_write_lease_current(&commit_current.value, now)?;

        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let incumbent = self.exact_incumbent(&snapshot, &commit_current.value)?;
        let odin_authority = self.current_odin_authority(&snapshot)?;
        let generation =
            AdmittedGeneration::from_transaction(&commit_current.value, odin_authority, now)?;
        let post_commit_cleanup = PostCommitCleanup {
            incumbent: match incumbent {
                Some(incumbent)
                    if incumbent_was_stopped_during_fencing(required(
                        &commit_current.value.fencing,
                        "commit fencing evidence",
                    )?) =>
                {
                    IncumbentCleanupEvidence::Complete {
                        generation_id: incumbent.value.generation_id.clone(),
                    }
                }
                Some(incumbent) => IncumbentCleanupEvidence::Pending {
                    generation_id: incumbent.value.generation_id.clone(),
                    workload: incumbent.value.workload.clone(),
                },
                None => IncumbentCleanupEvidence::SkippedNoIncumbent,
            },
            source: match commit_current.value.command_kind {
                CommandKind::Deploy => SourceCleanupEvidence::Pending,
                CommandKind::Continuity => SourceCleanupEvidence::SkippedContinuity,
            },
        };
        let mut complete = commit_current.value.clone();
        complete.phase = DeploymentPhase::Complete;
        complete.updated_at_unix_millis = now;
        complete.last_error = None;
        complete.completion = Some(TransactionCompletion::Admitted {
            generation_id: generation.generation_id.clone(),
        });
        complete.post_commit_cleanup = Some(post_commit_cleanup);
        complete.validate()?;

        let admitted_expected = CultCacheExpectedEnvelope {
            r#type: AdmittedGeneration::TYPE.into(),
            key: generation.target.clone(),
            current: incumbent.map(|stored| stored.envelope.clone()),
        };
        ensure!(
            SingleFileMessagePackBackingStore::new(&self.options.state_store).compare_exchange(
                &[
                    CultCacheExpectedEnvelope {
                        r#type: DeploymentTransaction::TYPE.into(),
                        key: commit_current.value.transaction_id.clone(),
                        current: Some(commit_current.envelope.clone()),
                    },
                    admitted_expected,
                ],
                &[
                    transaction_envelope(&complete, now)?,
                    admitted_envelope(&generation, now)?,
                ],
            )?,
            "incumbent or transaction changed before atomic admission commit"
        );
        Ok(())
    }

    fn exact_incumbent<'a>(
        &self,
        snapshot: &'a ControlSnapshot,
        transaction: &DeploymentTransaction,
    ) -> Result<Option<&'a Stored<AdmittedGeneration>>> {
        let current = snapshot.admitted_for(&transaction.target);
        match (&transaction.incumbent_generation_id, current) {
            (None, None) => Ok(None),
            (Some(expected), Some(current)) if current.value.generation_id == *expected => {
                Ok(Some(current))
            }
            _ => bail!("target incumbent changed after transaction creation"),
        }
    }

    fn incumbent_lease_sha256_for_warming(
        &self,
        snapshot: &ControlSnapshot,
        transaction: &DeploymentTransaction,
    ) -> Result<Option<String>> {
        if let Some(fencing) = &transaction.fencing {
            return match fencing {
                FencingEvidence::SkippedStateless => Ok(None),
                FencingEvidence::Revoked {
                    incumbent_lease_sha256,
                    ..
                } => Ok(incumbent_lease_sha256.clone()),
            };
        }
        self.exact_incumbent(snapshot, transaction)?
            .and_then(|incumbent| incumbent.value.leasing.lease())
            .map(IdunnProcessWriteLeaseRecord::canonical_sha256)
            .transpose()
    }

    fn fresh_warming_for_lease(
        &self,
        current: &Stored<DeploymentTransaction>,
        now: u64,
    ) -> Result<
        Option<(
            Stored<DeploymentTransaction>,
            WarmingEvidence,
            SequenceAdmittedWarming,
        )>,
    > {
        let prior = self.rehydrate_warming_token(&current.value, now, false)?;
        match required(&current.value.warming, "pre-fence Warming evidence")?.clone() {
            WarmingEvidence::FirstOdinDirect { .. } => {
                let snapshot = ControlSnapshot::read(&self.options.state_store)?;
                ensure!(
                    snapshot.admitted_for("odin").is_none()
                        && self.exact_incumbent(&snapshot, &current.value)?.is_none(),
                    "direct first-Odin Warming refresh found an admitted Odin"
                );
                let (evidence, present) = self.observe_first_odin_warming(&current.value)?;
                let token = SequenceAdmittedWarming::from_first_odin_presence(
                    current.value.transaction_id.clone(),
                    evidence.clone(),
                    present,
                )?;
                ensure!(
                    token.signed_presence_sha256() != prior.signed_presence_sha256(),
                    "first Odin replayed its pre-fence Warming presence"
                );
                Ok(Some((
                    current.clone(),
                    WarmingEvidence::FirstOdinDirect { evidence },
                    token,
                )))
            }
            WarmingEvidence::OdinTopology {
                evidence: prior_evidence,
            } => {
                let Some((admitted, authenticated)) = self.admit_latest_topology(current, None)?
                else {
                    return Ok(None);
                };
                let fresh_evidence = required(
                    &admitted.value.latest_odin_observation,
                    "post-fence Odin Warming observation",
                )?
                .clone();
                let snapshot = ControlSnapshot::read(&self.options.state_store)?;
                let incumbent_lease_sha256 =
                    self.incumbent_lease_sha256_for_warming(&snapshot, &admitted.value)?;
                if !is_semantic_warming(
                    required(&admitted.value.expected, "Warming Expected projection")?,
                    required(&admitted.value.activation, "Warming activation")?,
                    incumbent_lease_sha256.as_deref(),
                    &authenticated,
                )? {
                    return Ok(None);
                }
                let token = SequenceAdmittedWarming::from_topology(
                    admitted.value.transaction_id.clone(),
                    fresh_evidence.clone(),
                    authenticated,
                )?;
                if !provider_warming_advanced(
                    prior_evidence.publisher_sequence,
                    prior.signed_presence_sha256(),
                    fresh_evidence.publisher_sequence,
                    token.signed_presence_sha256(),
                ) {
                    return Ok(None);
                }
                Ok(Some((
                    admitted,
                    WarmingEvidence::OdinTopology {
                        evidence: fresh_evidence,
                    },
                    token,
                )))
            }
        }
    }

    fn prepare_candidate_write_lease(
        &self,
        transaction: &DeploymentTransaction,
        warming: &SequenceAdmittedWarming,
        now: u64,
    ) -> Result<IdunnProcessWriteLeaseRecord> {
        let expected = required(&transaction.expected, "Expected projection")?;
        let activation = required(&transaction.activation, "activation")?;
        ensure!(
            expected.write_lease_required
                && warming.transaction_id() == transaction.transaction_id
                && warming.runtime_instance_id() == activation.runtime_instance_id,
            "fresh Warming token does not own this stateful candidate"
        );
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let epoch = self
            .exact_incumbent(&snapshot, transaction)?
            .and_then(|generation| generation.value.leasing.lease())
            .map_or(1, |lease| lease.lease_epoch.saturating_add(1));
        let lease = IdunnProcessWriteLeaseRecord {
            schema_version: IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into(),
            target: expected.target.clone(),
            expected_projection_sha256: expected.canonical_sha256()?,
            plan_id: expected.plan_id.clone(),
            incarnation_id: expected.incarnation_id.clone(),
            sealed_release_id: expected.sealed_release_id.clone(),
            activation_witness_sha256: activation.canonical_sha256()?,
            state_schema_generation: expected
                .state_schema_generation
                .clone()
                .context("stateful Expected has no schema generation")?,
            state_contract_sha256: expected
                .state_contract_sha256
                .clone()
                .context("stateful Expected has no state contract")?,
            runtime_id: expected.runtime_id.clone(),
            runtime_instance_id: activation.runtime_instance_id.clone(),
            warming_presence_sha256: warming.signed_presence_sha256().to_owned(),
            lease_epoch: epoch,
            issued_at_unix_millis: now,
        };
        lease.validate()?;
        Ok(lease)
    }

    fn ensure_transaction_write_lease_current(
        &self,
        transaction: &DeploymentTransaction,
        now: u64,
    ) -> Result<()> {
        let expected = required(&transaction.expected, "Expected projection")?;
        let lease = transaction
            .leasing
            .as_ref()
            .and_then(LeasingEvidence::lease);
        ensure!(
            expected.write_lease_required == lease.is_some(),
            "transaction write-lease disposition differs from Expected"
        );
        let Some(lease) = lease else {
            return Ok(());
        };
        let activation = required(&transaction.activation, "activation")?;
        let warming = self.rehydrate_warming_token(transaction, now, false)?;
        let lease_path = required(&transaction.plan, "transaction plan")?
            .parsed_inputs()?
            .1
            .process_write_lease
            .context("stateful target has no write-lease binding")?
            .record_path;
        ensure!(
            CultCacheWriteLeaseDriver::new(&transaction.target, lease_path)
                .observe(expected, activation, &warming, lease,)?,
            "process write lease changed across the authority boundary"
        );
        Ok(())
    }

    fn select_candidate_port(&self, binding: &OperatorBinding) -> Result<Option<u16>> {
        let Some(route) = &binding.route else {
            return Ok(None);
        };
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let mut used = BTreeSet::new();
        for plan in snapshot
            .admitted
            .iter()
            .map(|stored| &stored.value.plan)
            .chain(
                snapshot
                    .transactions
                    .iter()
                    .filter(|stored| stored.value.blocks_new_target_mutation())
                    .filter_map(|stored| stored.value.plan.as_ref()),
            )
        {
            if let Some(port) = plan.candidate_port {
                used.insert(port);
            }
        }
        (route.private_port_start..=route.private_port_end)
            .find(|port| !used.contains(port))
            .map(Some)
            .context("no private candidate port remains in the operator range")
    }

    fn deployment_authorization(
        &self,
        current: &Stored<DeploymentTransaction>,
        now: u64,
    ) -> Result<Option<DeploymentAuthorization>> {
        let expected = required(&current.value.expected, "Expected projection")?;
        let plan = required(&current.value.plan, "transaction plan")?;
        let binding = plan.parsed_inputs()?.1;
        let Some((record, canonical_bytes)) =
            read_deployment_brake(&binding.brakes.deployment_store)?
        else {
            return Ok(None);
        };
        let anchor = read_trust_anchor::<IdunnDeploymentBrakeOperatorIdentity>(
            &self.options.deployment_brake_operator_anchor,
        )?;
        if evaluate_idunn_deployment_brake(
            IdunnDeploymentBrakeObservation::Present(&record),
            &anchor,
            &expected.runtime_id,
            &expected.sealed_release_id,
            &current.value.transaction_id,
            now,
        )
        .is_err()
        {
            return Ok(None);
        }
        let authorization_id = record
            .authorization_id
            .clone()
            .context("released brake has no authorization id")?;
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        ensure!(
            !snapshot.transactions.iter().any(|stored| {
                stored
                    .value
                    .deployment_authorization
                    .as_ref()
                    .is_some_and(|used| used.authorization_id == authorization_id)
            }),
            "deployment authorization was already consumed"
        );
        Ok(Some(DeploymentAuthorization {
            authorization_id,
            brake_sha256: sha256_id(&canonical_bytes),
            canonical_brake_bytes: canonical_bytes,
            authorized_at_unix_millis: now,
        }))
    }

    fn lifecycle_allows(&self, current: &Stored<DeploymentTransaction>, now: u64) -> Result<bool> {
        let plan = required(&current.value.plan, "continuity plan")?;
        let binding = plan.parsed_inputs()?.1;
        let expected = required(&current.value.expected, "continuity Expected")?;
        match read_lifecycle_brake(&binding.brakes.lifecycle_store) {
            Ok(Some(record)) => Ok(evaluate_idunn_continuity_restart(
                IdunnLifecycleBrakeObservation::Present(&record),
                &expected.runtime_id,
                &current.value.target,
                now,
            )
            .is_ok()),
            Ok(None) => Ok(evaluate_idunn_continuity_restart(
                IdunnLifecycleBrakeObservation::Missing,
                &expected.runtime_id,
                &current.value.target,
                now,
            )
            .is_ok()),
            Err(_) => Ok(false),
        }
    }

    fn record_gate_wait(
        &self,
        current: &Stored<DeploymentTransaction>,
        detail: &str,
    ) -> Result<()> {
        if current.value.last_error.as_deref() == Some(detail) {
            return Ok(());
        }
        let mut next = current.value.clone();
        next.last_error = Some(detail.into());
        next.updated_at_unix_millis = now_millis()?;
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn runtime_authority(
        &self,
        transaction: &DeploymentTransaction,
    ) -> Result<cultnet_rs::VerifiedRuntimeAuthority> {
        self.runtime_authority_parts(
            required(&transaction.plan, "transaction plan")?,
            required(&transaction.expected, "Expected projection")?,
            required(&transaction.activation, "activation")?,
        )
    }

    fn authenticate_routed_presence(
        &self,
        authority: &cultnet_rs::VerifiedRuntimeAuthority,
        current_write_lease_sha256: Option<&str>,
        message_id: &str,
        challenged_at_unix_millis: u64,
        received_at_unix_millis: u64,
        canonical_presence: &[u8],
    ) -> Result<(String, u64)> {
        ensure!(
            received_at_unix_millis >= challenged_at_unix_millis,
            "route observation predates its challenge"
        );
        let authenticated = authenticate_runtime_presence_claim(
            canonical_presence,
            authority,
            RuntimePresenceAuthenticationContext {
                trusted_received_at_unix_millis: received_at_unix_millis,
                maximum_age_millis: self.options.topology_maximum_age_millis,
                maximum_future_skew_millis: self.options.topology_maximum_future_skew_millis,
            },
        )?;
        let signed_presence_sha256 = authenticated.signed_presence_sha256().to_owned();
        let correlation = correlate_runtime_presence_claim(authenticated, authority)?;
        ensure!(
            correlation.disagreements().is_empty(),
            "stable route answered with a runtime that disagrees with current authority"
        );
        let present = correlation.into_undisputed_present()?;
        let presence = present.record();
        ensure!(
            presence.observed_at_unix_millis >= challenged_at_unix_millis,
            "stable route returned a presence minted before the route challenge"
        );
        ensure!(
            presence.state == "active",
            "stable route runtime is not Active"
        );
        ensure!(
            presence.detail == format!("route-observation:{message_id}"),
            "stable route response is not bound to the exact challenge"
        );
        ensure!(
            presence.write_lease_sha256.as_deref() == current_write_lease_sha256,
            "stable route runtime does not hold the exact current process write lease"
        );
        Ok((signed_presence_sha256, received_at_unix_millis))
    }

    fn authenticate_first_odin_warming_presence(
        &self,
        transaction: &DeploymentTransaction,
        message_id: &str,
        challenged_at_unix_millis: u64,
        received_at_unix_millis: u64,
        canonical_presence: &[u8],
    ) -> Result<cultnet_rs::VerifiedRuntimePresence> {
        ensure!(
            transaction.target == "odin"
                && transaction.incumbent_generation_id.is_none()
                && required(&transaction.expected, "first Odin Expected")?.write_lease_required,
            "direct Warming presence is reserved for stateful first Odin bootstrap"
        );
        ensure!(
            received_at_unix_millis >= challenged_at_unix_millis,
            "first Odin Warming observation predates its challenge"
        );
        let authority = self.runtime_authority(transaction)?;
        let authenticated = authenticate_runtime_presence_claim(
            canonical_presence,
            &authority,
            RuntimePresenceAuthenticationContext {
                trusted_received_at_unix_millis: received_at_unix_millis,
                maximum_age_millis: self.options.topology_maximum_age_millis,
                maximum_future_skew_millis: self.options.topology_maximum_future_skew_millis,
            },
        )?;
        let correlation = correlate_runtime_presence_claim(authenticated, &authority)?;
        ensure!(
            correlation.disagreements().is_empty(),
            "first Odin Warming presence disagrees with its Expected activation"
        );
        let present = correlation.into_undisputed_present()?;
        let presence = present.record();
        ensure!(
            presence.observed_at_unix_millis >= challenged_at_unix_millis
                && presence.state == "warming"
                && presence.write_lease_sha256.is_none()
                && presence.detail == format!("idunn-warming:{message_id}"),
            "first Odin candidate did not return exact fresh pre-lease Warming evidence"
        );
        Ok(present)
    }

    fn observe_first_odin_warming(
        &self,
        transaction: &DeploymentTransaction,
    ) -> Result<(RuntimePresenceEvidence, cultnet_rs::VerifiedRuntimePresence)> {
        let expected = required(&transaction.expected, "first Odin Expected")?;
        let binding = required(&transaction.plan, "first Odin plan")?
            .parsed_inputs()?
            .1;
        let driver = NginxRouteDriver::new(
            binding
                .route
                .context("first Odin bootstrap has no candidate route binding")?,
        );
        let message_id = format!("warming-{}", Uuid::new_v4().simple());
        let challenged_at_unix_millis = now_millis()?;
        let response = driver.request_candidate_runtime_presence(expected, &message_id)?;
        ensure!(
            response.message_id == message_id,
            "first Odin candidate transport substituted its challenge identity"
        );
        let admitted_at_unix_millis = now_millis()?;
        let present = self.authenticate_first_odin_warming_presence(
            transaction,
            &message_id,
            challenged_at_unix_millis,
            admitted_at_unix_millis,
            &response.canonical_presence,
        )?;
        let evidence = RuntimePresenceEvidence::from_present(
            &present,
            message_id,
            challenged_at_unix_millis,
            admitted_at_unix_millis,
        )?;
        Ok((evidence, present))
    }

    fn prove_stable_route(
        &self,
        transaction: &DeploymentTransaction,
        driver: &NginxRouteDriver,
        membership_sha256: String,
    ) -> Result<RouteObservation> {
        let expected = required(&transaction.expected, "route Expected projection")?;
        let activation = required(&transaction.activation, "route activation")?;
        let authority = self.runtime_authority(transaction)?;
        let current_write_lease_sha256 = transaction
            .leasing
            .as_ref()
            .and_then(LeasingEvidence::lease_sha256);
        self.prove_stable_route_against(
            expected,
            activation,
            &authority,
            current_write_lease_sha256,
            driver,
            membership_sha256,
        )
    }

    fn prove_stable_route_against(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        authority: &cultnet_rs::VerifiedRuntimeAuthority,
        current_write_lease_sha256: Option<&str>,
        driver: &NginxRouteDriver,
        membership_sha256: String,
    ) -> Result<RouteObservation> {
        let message_id = format!("route-{}", Uuid::new_v4().simple());
        let challenged_at_unix_millis = now_millis()?;
        let response = driver.request_runtime_presence(expected, &message_id)?;
        ensure!(
            response.message_id == message_id,
            "route transport substituted its challenge identity"
        );
        let received_at_unix_millis = now_millis()?;
        let (signed_presence_sha256, observed_at_unix_millis) = self.authenticate_routed_presence(
            authority,
            current_write_lease_sha256,
            &message_id,
            challenged_at_unix_millis,
            received_at_unix_millis,
            &response.canonical_presence,
        )?;
        let observation = RouteObservation {
            route_id: driver.binding.route_id.clone(),
            runtime_instance_id: activation.runtime_instance_id.clone(),
            membership_sha256,
            signed_presence_sha256,
            observed_at_unix_millis,
        };
        observation.validate()?;
        Ok(observation)
    }

    fn runtime_authority_parts(
        &self,
        plan: &CompiledDeploymentPlan,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<cultnet_rs::VerifiedRuntimeAuthority> {
        let provider_anchor = self.provider_anchor_for_plan(plan)?;
        verify_runtime_authority(
            expected,
            activation,
            &self.idunn_anchor,
            &provider_anchor.public_key,
        )
    }

    fn provider_anchor_for_plan(
        &self,
        plan: &CompiledDeploymentPlan,
    ) -> Result<ServiceIdentityTrustAnchor> {
        let binding = plan.parsed_inputs()?.1;
        let provider_anchor = read_trust_anchor::<GameCultProviderHealthIdentity>(
            &binding.runtime_identity.trust_anchor_store,
        )?;
        ensure!(
            provider_anchor.schema_version
                == <GameCultProviderHealthIdentity as ServiceIdentityProfile>::TRUST_ANCHOR_SCHEMA,
            "provider runtime presence trust anchor schema is unsupported"
        );
        Ok(provider_anchor)
    }

    fn authenticate_topology_bytes(
        &self,
        snapshot: &ControlSnapshot,
        transaction: &DeploymentTransaction,
        canonical_bytes: &[u8],
        current_write_lease_sha256: Option<&str>,
        trusted_received_at: u64,
    ) -> Result<AuthenticatedOdinRuntimeTopologyCorrelation> {
        let authority = self.runtime_authority(transaction)?;
        let odin_authority = self.current_odin_authority(snapshot)?;
        authenticate_odin_runtime_topology_correlation(
            canonical_bytes,
            &authority,
            current_write_lease_sha256,
            &odin_authority.signer_public_key,
            self.trusted_topology_context(trusted_received_at),
        )
    }

    /// Persist every newly authenticated publisher sequence before deciding
    /// whether it means Warming, Ready, degraded, or disagreement.
    fn admit_latest_topology(
        &self,
        current: &Stored<DeploymentTransaction>,
        current_write_lease_sha256: Option<&str>,
    ) -> Result<
        Option<(
            Stored<DeploymentTransaction>,
            AuthenticatedOdinRuntimeTopologyCorrelation,
        )>,
    > {
        let Some(received) = self.topology().receive(&current.value.target)? else {
            return Ok(None);
        };
        ensure!(
            received.target == current.value.target,
            "topology transport substituted target"
        );
        let now = now_millis()?;
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let live = snapshot
            .transactions
            .iter()
            .find(|stored| stored.value.transaction_id == current.value.transaction_id)
            .context("transaction disappeared before topology admission")?;
        ensure!(
            live.envelope == current.envelope,
            "transaction changed before topology admission"
        );
        let authenticated = self.authenticate_topology_bytes(
            &snapshot,
            &live.value,
            &received.canonical_bytes,
            current_write_lease_sha256,
            now,
        )?;
        let evidence = TopologyEvidence::from_authenticated(&authenticated, now)?;
        if !sequence_requires_admission(
            live.value.latest_odin_observation.as_ref(),
            snapshot.max_odin_sequence(&live.value.target, &evidence.signer_identity_id),
            &evidence,
        )? {
            return Ok(Some((live.clone(), authenticated)));
        }
        let mut next = live.value.clone();
        next.latest_odin_observation = Some(evidence.clone());
        next.odin_publisher_sequence_cursor = evidence.publisher_sequence;
        next.updated_at_unix_millis = now;
        next.last_error = None;
        replace_transaction(&self.options.state_store, live, &next)?;
        let admitted = ControlSnapshot::read(&self.options.state_store)?
            .transactions
            .into_iter()
            .find(|stored| stored.value.transaction_id == next.transaction_id)
            .context("sequence-admitted transaction disappeared")?;
        ensure!(
            admitted.value.latest_odin_observation.as_ref() == Some(&evidence)
                && admitted.value.odin_publisher_sequence_cursor == evidence.publisher_sequence,
            "topology sequence was not durably admitted"
        );
        Ok(Some((admitted, authenticated)))
    }

    fn rehydrate_warming_token(
        &self,
        transaction: &DeploymentTransaction,
        now: u64,
        require_current: bool,
    ) -> Result<SequenceAdmittedWarming> {
        let evidence = required(&transaction.warming, "durable warming evidence")?.clone();
        match evidence {
            WarmingEvidence::OdinTopology { evidence } => {
                let snapshot = ControlSnapshot::read(&self.options.state_store)?;
                let authenticated = self.authenticate_topology_bytes(
                    &snapshot,
                    transaction,
                    &evidence.canonical_bytes,
                    None,
                    if require_current {
                        now
                    } else {
                        evidence.admitted_at_unix_millis
                    },
                )?;
                validate_authenticated_evidence(&evidence, &authenticated)?;
                let incumbent_lease_sha256 =
                    self.incumbent_lease_sha256_for_warming(&snapshot, transaction)?;
                ensure!(
                    is_semantic_warming(
                        required(&transaction.expected, "Warming Expected projection")?,
                        required(&transaction.activation, "Warming activation")?,
                        incumbent_lease_sha256.as_deref(),
                        &authenticated,
                    )?,
                    "durable warming evidence no longer satisfies the Warming gate"
                );
                SequenceAdmittedWarming::from_topology(
                    transaction.transaction_id.clone(),
                    evidence,
                    authenticated,
                )
            }
            WarmingEvidence::FirstOdinDirect { evidence } => {
                let present = self.authenticate_first_odin_warming_presence(
                    transaction,
                    &evidence.message_id,
                    evidence.challenged_at_unix_millis,
                    if require_current {
                        now
                    } else {
                        evidence.admitted_at_unix_millis
                    },
                    &evidence.canonical_bytes,
                )?;
                SequenceAdmittedWarming::from_first_odin_presence(
                    transaction.transaction_id.clone(),
                    evidence,
                    present,
                )
            }
        }
    }

    fn rehydrate_ready_token(
        &self,
        transaction: &DeploymentTransaction,
        now: u64,
        require_current: bool,
    ) -> Result<SequenceAdmittedReady> {
        let evidence = required(&transaction.ready, "durable Ready evidence")?.clone();
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        let current_lease = transaction
            .leasing
            .as_ref()
            .and_then(LeasingEvidence::lease_sha256);
        let authenticated = self.authenticate_topology_bytes(
            &snapshot,
            transaction,
            &evidence.canonical_bytes,
            current_lease,
            if require_current {
                now
            } else {
                evidence.admitted_at_unix_millis
            },
        )?;
        validate_authenticated_evidence(&evidence, &authenticated)?;
        ensure!(
            is_semantic_ready(&authenticated),
            "durable Ready evidence no longer authenticates as Ready"
        );
        Ok(SequenceAdmittedReady {
            transaction_id: transaction.transaction_id.clone(),
            evidence,
            expected: required(&transaction.expected, "Expected projection")?.clone(),
            authenticated,
        })
    }

    fn current_ready_provider_tokens(
        &self,
        snapshot: &ControlSnapshot,
        now: u64,
    ) -> Result<Vec<SequenceAdmittedReady>> {
        let mut providers = Vec::new();
        for stored in &snapshot.admitted {
            if stored.value.latest_odin_observation != stored.value.ready {
                continue;
            }
            match self.rehydrate_admitted_ready(snapshot, &stored.value, now) {
                Ok(provider) => providers.push(provider),
                Err(error) => eprintln!(
                    "Idunn excluded non-current provider {}: {error:#}",
                    stored.value.target
                ),
            }
        }
        Ok(providers)
    }

    fn rehydrate_admitted_ready(
        &self,
        snapshot: &ControlSnapshot,
        generation: &AdmittedGeneration,
        now: u64,
    ) -> Result<SequenceAdmittedReady> {
        ensure!(
            generation.latest_odin_observation == generation.ready,
            "admitted provider latest topology is not Ready"
        );
        let authority = self.runtime_authority_parts(
            &generation.plan,
            &generation.expected,
            &generation.activation,
        )?;
        let odin_authority = self.current_odin_authority(snapshot)?;
        let current_lease = generation.leasing.lease_sha256();
        let authenticated = authenticate_odin_runtime_topology_correlation(
            &generation.ready.canonical_bytes,
            &authority,
            current_lease,
            &odin_authority.signer_public_key,
            self.trusted_topology_context(now),
        )?;
        validate_authenticated_evidence(&generation.ready, &authenticated)?;
        ensure!(
            is_semantic_ready(&authenticated),
            "admitted provider no longer has current exact Ready evidence"
        );
        Ok(SequenceAdmittedReady {
            transaction_id: generation.transaction_id.clone(),
            evidence: generation.ready.clone(),
            expected: generation.expected.clone(),
            authenticated,
        })
    }

    fn validate_selected_providers_current(
        &self,
        plan: &CompiledDeploymentPlan,
        now: u64,
    ) -> Result<()> {
        let snapshot = ControlSnapshot::read(&self.options.state_store)?;
        for selection in &plan.dependencies {
            let Some(provider) = &selection.provider else {
                ensure!(
                    selection.requirement.kind == DependencyKind::Optional,
                    "required dependency has no provider"
                );
                continue;
            };
            let DependencyProviderAuthority::ManagedReady {
                target,
                incarnation_id,
                plan_id,
                sealed_release_id,
                expected_projection_sha256,
                odin_topology_correlation_sha256,
                odin_topology_publisher_sequence,
            } = &provider.authority
            else {
                continue;
            };
            let admitted = snapshot
                .admitted_for(target)
                .context("selected managed dependency is no longer admitted")?;
            let token = self.rehydrate_admitted_ready(&snapshot, &admitted.value, now)?;
            ensure!(
                admitted.value.expected.incarnation_id == *incarnation_id
                    && admitted.value.plan.plan_id == *plan_id
                    && admitted.value.sealed_release.sealed_release_id == *sealed_release_id
                    && admitted.value.expected.canonical_sha256()? == *expected_projection_sha256
                    && token.publisher_sequence() >= *odin_topology_publisher_sequence,
                "selected dependency authority changed before actuation"
            );
            ensure!(
                token.publisher_sequence() != *odin_topology_publisher_sequence
                    || token.evidence_sha256() == odin_topology_correlation_sha256,
                "selected dependency Odin sequence changed evidence"
            );
            ensure!(
                token
                    .authenticated()
                    .record()
                    .observed_capabilities
                    .iter()
                    .any(|capability| {
                        capability_compatible(
                            &selection.requirement.capability,
                            &selection.requirement.schema,
                            &selection.requirement.compatibility,
                            &capability.capability,
                            &capability.schema,
                            &capability.compatibility,
                        ) && capability.capacity >= selection.requirement.minimum_capacity
                    }),
                "selected dependency no longer provides its required capability"
            );
        }
        Ok(())
    }

    fn persist_same_phase<F>(
        &self,
        current: &Stored<DeploymentTransaction>,
        mutation: F,
    ) -> Result<()>
    where
        F: FnOnce(&mut DeploymentTransaction) -> Result<()>,
    {
        let mut next = current.value.clone();
        mutation(&mut next)?;
        ensure!(
            next.phase == current.value.phase,
            "same-phase evidence update changed phase"
        );
        next.updated_at_unix_millis = now_millis()?;
        next.last_error = None;
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn transition(
        &self,
        current: &Stored<DeploymentTransaction>,
        next_phase: DeploymentPhase,
    ) -> Result<()> {
        ensure!(
            next_phase as u8 == current.value.phase as u8 + 1,
            "deployment phase transition is not adjacent"
        );
        let mut next = current.value.clone();
        next.phase = next_phase;
        next.updated_at_unix_millis = now_millis()?;
        next.last_error = None;
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn record_resumable_error(
        &self,
        current: &Stored<DeploymentTransaction>,
        error: &anyhow::Error,
    ) -> Result<()> {
        let detail = truncate(&format!("{error:#}"), 2048);
        if current.value.last_error.as_deref() == Some(detail.as_str()) {
            return Ok(());
        }
        let mut next = current.value.clone();
        next.last_error = Some(detail);
        next.updated_at_unix_millis = now_millis()?;
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn begin_pre_fencing_abort(
        &self,
        current: &Stored<DeploymentTransaction>,
        error: anyhow::Error,
    ) -> Result<()> {
        ensure!(
            current.value.phase < DeploymentPhase::Fencing,
            "post-fence transaction cannot terminal-fail"
        );
        ensure!(
            current.value.pre_fencing_abort.is_none(),
            "pre-fencing abort intent is already durable"
        );
        let abort = PreFencingAbort {
            error: truncate(&format!("{error:#}"), 2048),
            candidate_cleanup: candidate_cleanup_requirement(
                current.value.activation.is_some(),
                current.value.workload.is_some(),
            ),
            topology_reconciliation: if current.value.command_kind == CommandKind::Deploy
                && current.value.expected_publication_sha256.is_some()
            {
                CleanupEvidence::Pending
            } else {
                CleanupEvidence::Skipped
            },
            source_cleanup: if current.value.command_kind == CommandKind::Deploy {
                CleanupEvidence::Pending
            } else {
                CleanupEvidence::Skipped
            },
        };
        self.persist_same_phase(current, |next| {
            next.pre_fencing_abort = Some(abort);
            Ok(())
        })
    }

    fn advance_pre_fencing_abort(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        ensure!(
            current.value.phase < DeploymentPhase::Fencing,
            "pre-fencing abort crossed the fencing boundary"
        );
        let abort = required(&current.value.pre_fencing_abort, "pre-fencing abort intent")?;
        if abort.candidate_cleanup == CleanupEvidence::Pending {
            if let Some(workload) = &current.value.workload {
                self.workload
                    .stop(workload)
                    .context("stopping exact pre-fence candidate")?;
            }
            self.workload
                .discard_prepared(
                    required(&current.value.plan, "pre-fencing candidate plan")?,
                    required(&current.value.expected, "pre-fencing Expected projection")?,
                    required(&current.value.activation, "pre-fencing activation")?,
                )
                .context("discarding exact pre-fence activation material")?;
            return self.persist_same_phase(current, |next| {
                next.pre_fencing_abort.as_mut().unwrap().candidate_cleanup =
                    CleanupEvidence::Complete;
                Ok(())
            });
        }
        if abort.topology_reconciliation == CleanupEvidence::Pending {
            let expected = required(&current.value.expected, "failed Expected projection")?;
            let snapshot = ControlSnapshot::read(&self.options.state_store)?;
            if let Some(incumbent) = self.exact_incumbent(&snapshot, &current.value)? {
                let topology = self.topology();
                let failed_provider_anchor = self.provider_anchor_for_plan(required(
                    &current.value.plan,
                    "failed transaction plan",
                )?)?;
                let admitted_provider_anchor =
                    self.provider_anchor_for_plan(&incumbent.value.plan)?;
                let expected_sha256 = topology.restore_admitted_expected_only(
                    expected,
                    &failed_provider_anchor,
                    current.value.activation.as_ref(),
                    &incumbent.value.expected,
                    &admitted_provider_anchor,
                    &incumbent.value.activation,
                    incumbent.value.leasing.lease(),
                )?;
                ensure!(
                    expected_sha256 == incumbent.value.expected.canonical_sha256()?,
                    "restored incumbent Expected receipt differs"
                );
            } else {
                let plan = required(&current.value.plan, "failed transaction plan")?;
                let provider_anchor = self.provider_anchor_for_plan(plan)?;
                self.topology()
                    .withdraw_expected(
                        expected,
                        &provider_anchor,
                        current.value.activation.as_ref(),
                        None,
                    )
                    .context("withdrawing exact failed Expected projection")?;
            }
            return self.persist_same_phase(current, |next| {
                next.pre_fencing_abort
                    .as_mut()
                    .unwrap()
                    .topology_reconciliation = CleanupEvidence::Complete;
                Ok(())
            });
        }
        if abort.source_cleanup == CleanupEvidence::Pending {
            self.source
                .cleanup(
                    &current.value.transaction_id,
                    current.value.frozen_source.as_ref(),
                )
                .context("cleaning failed pre-fence source")?;
            return self.persist_same_phase(current, |next| {
                next.pre_fencing_abort.as_mut().unwrap().source_cleanup = CleanupEvidence::Complete;
                Ok(())
            });
        }
        ensure!(
            abort.is_complete(),
            "pre-fencing abort cleanup is incomplete"
        );
        let mut next = current.value.clone();
        next.phase = DeploymentPhase::Complete;
        next.updated_at_unix_millis = now_millis()?;
        next.last_error = Some(abort.error.clone());
        next.completion = Some(TransactionCompletion::FailedBeforeFencing {
            error: abort.error.clone(),
        });
        replace_transaction(&self.options.state_store, current, &next)
    }

    fn advance_post_commit_cleanup(&self, current: &Stored<DeploymentTransaction>) -> Result<()> {
        let cleanup = required(
            &current.value.post_commit_cleanup,
            "post-commit cleanup evidence",
        )?;
        if let IncumbentCleanupEvidence::Pending {
            generation_id,
            workload,
        } = &cleanup.incumbent
        {
            let binding = required(&current.value.plan, "committed transaction plan")?
                .parsed_inputs()?
                .1;
            if let Some(promoted_at_unix_millis) = required(
                &current.value.routing,
                "committed transaction routing evidence",
            )?
            .promoted_at_unix_millis()
            {
                let retire_not_before =
                    route_drain_deadline(promoted_at_unix_millis, binding.rollout.drain_seconds)?;
                if now_millis()? < retire_not_before {
                    return Ok(());
                }
            }
            self.workload
                .stop(workload)
                .with_context(|| format!("retiring admitted incumbent {generation_id}"))?;
            let generation_id = generation_id.clone();
            return self.persist_same_phase(current, |next| {
                next.post_commit_cleanup.as_mut().unwrap().incumbent =
                    IncumbentCleanupEvidence::Complete { generation_id };
                Ok(())
            });
        }
        if cleanup.source == SourceCleanupEvidence::Pending {
            self.source
                .cleanup(
                    &current.value.transaction_id,
                    current.value.frozen_source.as_ref(),
                )
                .context("cleaning committed deployment source")?;
            return self.persist_same_phase(current, |next| {
                next.post_commit_cleanup.as_mut().unwrap().source = SourceCleanupEvidence::Complete;
                Ok(())
            });
        }
        ensure!(cleanup.is_complete(), "post-commit cleanup is incomplete");
        Ok(())
    }
}

fn route_drain_deadline(promoted_at_unix_millis: u64, drain_seconds: u32) -> Result<u64> {
    promoted_at_unix_millis
        .checked_add(
            u64::from(drain_seconds)
                .checked_mul(1_000)
                .context("route drain duration overflows milliseconds")?,
        )
        .context("route drain deadline overflows Unix milliseconds")
}

fn route_observation_is_current(
    observed_at_unix_millis: u64,
    now_unix_millis: u64,
    maximum_age_millis: u64,
    maximum_future_skew_millis: u64,
) -> bool {
    observed_at_unix_millis <= now_unix_millis.saturating_add(maximum_future_skew_millis)
        && now_unix_millis.saturating_sub(observed_at_unix_millis) <= maximum_age_millis
}

fn provider_warming_advanced(
    prior_odin_sequence: u64,
    prior_signed_presence_sha256: &str,
    candidate_odin_sequence: u64,
    candidate_signed_presence_sha256: &str,
) -> bool {
    candidate_odin_sequence > prior_odin_sequence
        && candidate_signed_presence_sha256 != prior_signed_presence_sha256
}

fn may_rollback_route_after_failed_proof(fencing: &FencingEvidence) -> bool {
    matches!(fencing, FencingEvidence::SkippedStateless)
}

fn incumbent_was_stopped_during_fencing(fencing: &FencingEvidence) -> bool {
    matches!(
        fencing,
        FencingEvidence::Revoked {
            incumbent_lease_sha256: Some(_),
            ..
        }
    )
}

fn candidate_cleanup_requirement(
    has_prepared_activation: bool,
    has_workload_observation: bool,
) -> CleanupEvidence {
    if has_prepared_activation || has_workload_observation {
        CleanupEvidence::Pending
    } else {
        CleanupEvidence::Skipped
    }
}

fn prove_isolation(
    candidate: &WorkloadObservation,
    incumbent: Option<&WorkloadObservation>,
) -> Result<IsolationEvidence> {
    ensure!(
        candidate.dynamic_user
            && candidate.private_pids
            && candidate.private_mounts
            && candidate.process_uids[0] > 0
            && candidate.pid_namespace_id > 0
            && candidate.mount_namespace_id > 0,
        "candidate lacks dynamic identity or private namespaces"
    );
    let evidence = if let Some(incumbent) = incumbent {
        ensure!(
            incumbent.dynamic_user
                && incumbent.private_pids
                && incumbent.private_mounts
                && incumbent.process_uids[0] > 0
                && incumbent.pid_namespace_id > 0
                && incumbent.mount_namespace_id > 0,
            "incumbent lacks admitted dynamic identity or private namespaces"
        );
        ensure!(
            candidate.process_uids[0] != incumbent.process_uids[0]
                && candidate.pid_namespace_id != incumbent.pid_namespace_id
                && candidate.mount_namespace_id != incumbent.mount_namespace_id,
            "candidate and incumbent are not distinct by UID, PID namespace, and mount namespace"
        );
        IsolationEvidence {
            candidate_uid: candidate.process_uids[0],
            candidate_pid_namespace_id: candidate.pid_namespace_id,
            candidate_mount_namespace_id: candidate.mount_namespace_id,
            incumbent_uid: Some(incumbent.process_uids[0]),
            incumbent_pid_namespace_id: Some(incumbent.pid_namespace_id),
            incumbent_mount_namespace_id: Some(incumbent.mount_namespace_id),
        }
    } else {
        IsolationEvidence {
            candidate_uid: candidate.process_uids[0],
            candidate_pid_namespace_id: candidate.pid_namespace_id,
            candidate_mount_namespace_id: candidate.mount_namespace_id,
            incumbent_uid: None,
            incumbent_pid_namespace_id: None,
            incumbent_mount_namespace_id: None,
        }
    };
    Ok(evidence)
}

fn sequence_requires_admission(
    latest: Option<&TopologyEvidence>,
    maximum_admitted_sequence: u64,
    candidate: &TopologyEvidence,
) -> Result<bool> {
    candidate.validate_shape()?;
    if latest.is_some_and(|existing| {
        existing.canonical_bytes == candidate.canonical_bytes
            && existing.canonical_sha256 == candidate.canonical_sha256
            && existing.signer_identity_id == candidate.signer_identity_id
            && existing.publisher_sequence == candidate.publisher_sequence
    }) {
        return Ok(false);
    }
    ensure!(
        candidate.publisher_sequence > maximum_admitted_sequence,
        "Odin topology publisher sequence was replayed or reordered"
    );
    Ok(true)
}

fn validate_live_providers_for_deploy<F>(command_kind: CommandKind, validation: F) -> Result<()>
where
    F: FnOnce() -> Result<()>,
{
    match command_kind {
        CommandKind::Deploy => validation(),
        CommandKind::Continuity => Ok(()),
    }
}

fn validate_authenticated_evidence(
    evidence: &TopologyEvidence,
    authenticated: &AuthenticatedOdinRuntimeTopologyCorrelation,
) -> Result<()> {
    let record = authenticated.record();
    ensure!(
        authenticated.canonical_bytes() == evidence.canonical_bytes.as_slice()
            && sha256_id(authenticated.canonical_bytes()) == evidence.canonical_sha256
            && record.signer_identity_id == evidence.signer_identity_id
            && record.publisher_sequence == evidence.publisher_sequence,
        "durable topology evidence differs from its authenticated receipt"
    );
    Ok(())
}

fn is_semantic_warming(
    expected: &IdunnExpectedIncarnationRecord,
    activation: &IdunnRuntimeActivationRecord,
    incumbent_lease_sha256: Option<&str>,
    authenticated: &AuthenticatedOdinRuntimeTopologyCorrelation,
) -> Result<bool> {
    let record = authenticated.record();
    if !record.present || record.observed_write_lease_sha256.is_some() {
        return Ok(false);
    }
    let expected_projection_detail = format!(
        "expected:{};activation:{}",
        expected.canonical_sha256()?,
        activation.canonical_sha256()?
    );
    if !warming_disagreements_match_incumbent(
        &expected_projection_detail,
        incumbent_lease_sha256,
        &record.disagreements,
    ) {
        return Ok(false);
    }
    Ok(if expected.write_lease_required {
        !record.ready && record.observed_presence_state.as_deref() == Some("warming")
    } else {
        matches!(
            record.observed_presence_state.as_deref(),
            Some("warming" | "active")
        )
    })
}

fn warming_disagreements_match_incumbent(
    expected_projection_detail: &str,
    incumbent_lease_sha256: Option<&str>,
    disagreements: &[OdinTopologyDisagreement],
) -> bool {
    if disagreements.is_empty() {
        true
    } else if let (Some(incumbent_lease_sha256), [disagreement]) =
        (incumbent_lease_sha256, disagreements)
    {
        disagreement.code == "projected-write-lease"
            && disagreement.expected.as_deref() == Some(expected_projection_detail)
            && disagreement.observed.as_deref() == Some(incumbent_lease_sha256)
    } else {
        false
    }
}

fn is_semantic_ready(authenticated: &AuthenticatedOdinRuntimeTopologyCorrelation) -> bool {
    let record = authenticated.record();
    record.present
        && record.ready
        && record.observed_presence_state.as_deref() == Some("active")
        && record.disagreements.is_empty()
}

fn read_deployment_brake(path: &Path) -> Result<Option<(IdunnDeploymentBrakeRecord, Vec<u8>)>> {
    let Some(envelope) = read_single_envelope(path)? else {
        return Ok(None);
    };
    ensure!(
        envelope.r#type == IdunnDeploymentBrakeRecord::TYPE
            && envelope.schema_id.as_deref() == Some(IDUNN_DEPLOYMENT_BRAKE_SCHEMA),
        "deployment brake store contains a foreign record"
    );
    let record: IdunnDeploymentBrakeRecord = rmp_serde::from_slice(&envelope.payload)?;
    record.validate()?;
    ensure!(
        rmp_serde::to_vec(&record)? == envelope.payload && envelope.key == record.brake_id,
        "deployment brake is noncanonical or keyed by another authority"
    );
    Ok(Some((record, envelope.payload)))
}

fn read_lifecycle_brake(path: &Path) -> Result<Option<IdunnLifecycleBrakeRecord>> {
    let Some(envelope) = read_single_envelope(path)? else {
        return Ok(None);
    };
    ensure!(
        envelope.r#type == IdunnLifecycleBrakeRecord::TYPE
            && envelope.schema_id.as_deref() == Some(IDUNN_LIFECYCLE_BRAKE_SCHEMA),
        "lifecycle brake store contains a foreign record"
    );
    IdunnLifecycleBrakeRecord::decode_canonical(&envelope.payload).map(Some)
}

fn read_single_envelope(path: &Path) -> Result<Option<CultCacheEnvelope>> {
    if !path.exists() {
        return Ok(None);
    }
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    match entries.as_slice() {
        [] => Ok(None),
        [entry] => Ok(Some(entry.clone())),
        _ => bail!("single-record authority store is ambiguous"),
    }
}

fn read_trust_anchor<P: ServiceIdentityProfile>(path: &Path) -> Result<ServiceIdentityTrustAnchor> {
    let envelope = read_single_envelope(path)?
        .with_context(|| format!("service identity trust anchor {} is absent", path.display()))?;
    ensure!(
        envelope.r#type == P::TRUST_ANCHOR_TYPE
            && envelope.key == P::TRUST_ANCHOR_KEY
            && envelope.schema_id.as_deref() == Some(P::TRUST_ANCHOR_SCHEMA),
        "service identity trust anchor belongs to another profile"
    );
    let anchor: ServiceIdentityTrustAnchor = rmp_serde::from_slice(&envelope.payload)?;
    ensure!(
        rmp_serde::to_vec(&anchor)? == envelope.payload
            && derive_service_identity_id::<P>(&anchor.public_key)? == anchor.identity_id,
        "service identity trust anchor is noncanonical or self-inconsistent"
    );
    Ok(anchor)
}

fn required<'a, T>(value: &'a Option<T>, label: &str) -> Result<&'a T> {
    value.as_ref().with_context(|| format!("{label} is absent"))
}

fn now_millis() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_millis()
        .try_into()?)
}

fn rfc3339_millis(millis: u64) -> Result<String> {
    chrono::DateTime::from_timestamp_millis(i64::try_from(millis)?)
        .context("timestamp is out of range")
        .map(|value| value.to_rfc3339())
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
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

fn require_detail(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 2048 && !value.contains('\0'),
        "{label} is invalid"
    );
    Ok(())
}

fn truncate(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

fn sha256_id(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(71);
    encoded.push_str("sha256-");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn runtime_instance_id(transaction_id: &str) -> Result<String> {
    require_id(transaction_id, "runtime transaction id")?;
    Ok(sha256_id(
        format!("gamecult.idunn.runtime-instance.v1:{transaction_id}").as_bytes(),
    ))
}

fn usage() -> &'static str {
    "Idunn deployment, admission, and continuity control plane\n\n\
     idunn serve [runtime options]\n\
     idunn up <service|profile:name> [--state-store PATH] [--no-wait]\n\
     idunn status [--state-store PATH] [--command ID]\n\n\
     Recipes describe capability and process requirements. Idunn seals exact\n\
     source and artifacts, admits one incarnation, and delegates execution to\n\
     systemd and routing mechanics to the configured proxy."
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        byte.to_string().repeat(64)
    }

    fn route_binding(
        route_id: &str,
        stable_port: u16,
        private_port_start: u16,
        private_port_end: u16,
    ) -> RouteBinding {
        RouteBinding {
            driver: crate::deployment::RouteDriver::NginxStreamTcp,
            route_id: route_id.into(),
            stable_endpoint: format!("tcp://127.0.0.1:{stable_port}"),
            private_host: "127.0.0.1".into(),
            private_port_start,
            private_port_end,
            config_path: PathBuf::from(format!("/etc/nginx/idunn-stream-routes/{route_id}.conf")),
            reload_unit: "nginx.service".into(),
        }
    }

    fn command(kind: CommandKind) -> DeploymentCommand {
        DeploymentCommand {
            schema_version: DEPLOYMENT_COMMAND_SCHEMA.into(),
            command_id: match kind {
                CommandKind::Deploy => "up-test".into(),
                CommandKind::Continuity => "continuity-test".into(),
            },
            kind,
            selector: "ghostlight".into(),
            requested_by: "operator".into(),
            requested_at_unix_millis: 100,
        }
    }

    fn topology(sequence: u64, byte: u8) -> TopologyEvidence {
        let canonical_bytes = vec![byte];
        TopologyEvidence {
            canonical_sha256: sha256_id(&canonical_bytes),
            canonical_bytes,
            signer_identity_id: "odin-signer".into(),
            publisher_sequence: sequence,
            admitted_at_unix_millis: 100,
        }
    }

    fn workload(uid: u32, pid_namespace_id: u64, mount_namespace_id: u64) -> WorkloadObservation {
        WorkloadObservation {
            unit: format!("idunn-{uid}.service"),
            unit_description: format!("Idunn test {uid}"),
            invocation_id: format!("invocation-{uid}"),
            exec_main_start_timestamp_monotonic: 1,
            service_type: "exec".into(),
            restart_policy: "always".into(),
            kill_mode: "control-group".into(),
            dynamic_user: true,
            systemd_user: format!("u{uid}"),
            systemd_group: format!("u{uid}"),
            supplementary_groups: String::new(),
            capability_bounding_set: String::new(),
            ambient_capabilities: String::new(),
            private_mounts: true,
            private_pids: true,
            protect_proc: "invisible".into(),
            proc_subset: "pid".into(),
            no_new_privileges: true,
            umask: "0077".into(),
            inaccessible_paths: String::new(),
            load_credential: String::new(),
            main_pid: uid,
            process_start_time: 1,
            process_uids: [uid; 4],
            process_gids: [uid; 4],
            process_groups: vec![uid],
            process_cap_inheritable: 0,
            process_cap_permitted: 0,
            process_cap_effective: 0,
            process_cap_bounding: 0,
            process_cap_ambient: 0,
            process_no_new_privileges: true,
            process_namespace_pids: vec![1],
            mount_namespace_id,
            pid_namespace_id,
            executable: PathBuf::from("/opt/test/bin/service"),
            executable_device: 1,
            executable_inode: 1,
            executable_sha256: sha256_id(&[1]),
            runtime_instance_id: sha256_id(&uid.to_be_bytes()),
            working_directory: PathBuf::from("/opt/test"),
            runtime_bundle: PathBuf::from("/run/test"),
            command_line_sha256: sha256_id(&[2]),
            environment_names: Vec::new(),
            environment_contract_sha256: sha256_id(&[3]),
            control_group: format!("/system.slice/idunn-{uid}.service"),
            credentials_directory: None,
            parent_only_file_descriptors: Vec::new(),
            activation_signer_identity_id: "activation".into(),
            activation_signer_public_key: vec![1; 32],
            service_credentials: Vec::new(),
        }
    }

    fn write_lease() -> IdunnProcessWriteLeaseRecord {
        IdunnProcessWriteLeaseRecord {
            schema_version: IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into(),
            target: "ghostlight".into(),
            expected_projection_sha256: sha256_id(&[1]),
            plan_id: sha256_id(&[2]),
            incarnation_id: "incarnation-test".into(),
            sealed_release_id: sha256_id(&[3]),
            activation_witness_sha256: sha256_id(&[4]),
            state_schema_generation: "ghostlight-state-v1".into(),
            state_contract_sha256: sha256_id(&[5]),
            runtime_id: "ghostlight".into(),
            runtime_instance_id: sha256_id(&[6]),
            warming_presence_sha256: sha256_id(&[7]),
            lease_epoch: 1,
            issued_at_unix_millis: 100,
        }
    }

    #[test]
    fn cli_exposes_only_declarative_commands() {
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
    fn route_bindings_are_one_global_socket_and_candidate_authority_map() {
        let first = route_binding("first", 4103, 14103, 14111);
        let second = route_binding("second", 8831, 18831, 18839);
        validate_route_binding_set(&[("first", &first), ("second", &second)]).unwrap();

        let mut duplicate_id = second.clone();
        duplicate_id.route_id = first.route_id.clone();
        assert!(
            validate_route_binding_set(&[("first", &first), ("second", &duplicate_id)]).is_err()
        );

        let mut overlapping_candidates = second.clone();
        overlapping_candidates.private_port_start = 14111;
        overlapping_candidates.private_port_end = 14120;
        assert!(
            validate_route_binding_set(&[("first", &first), ("second", &overlapping_candidates),])
                .is_err()
        );

        let mut stable_inside_other_range = second;
        stable_inside_other_range.stable_endpoint = "tcp://127.0.0.1:14105".into();
        assert!(
            validate_route_binding_set(&[
                ("first", &first),
                ("second", &stable_inside_other_range),
            ])
            .is_err()
        );
    }

    #[test]
    fn deployment_command_is_immutable_positional_fact() -> Result<()> {
        let value = command(CommandKind::Deploy);
        value.validate()?;
        let encoded = rmp_serde::to_vec(&value)?;
        assert_eq!(encoded[0], 0x96);
        assert!(!encoded.windows(7).any(|window| window == b"running"));
        assert!(!encoded.windows(5).any(|window| window == b"owner"));
        Ok(())
    }

    #[test]
    fn phases_are_one_exact_forward_chain() {
        let phases = [
            DeploymentPhase::Sealing,
            DeploymentPhase::Starting,
            DeploymentPhase::Warming,
            DeploymentPhase::Fencing,
            DeploymentPhase::Leasing,
            DeploymentPhase::AwaitingReady,
            DeploymentPhase::Routing,
            DeploymentPhase::Committing,
            DeploymentPhase::Complete,
        ];
        assert!(
            phases
                .windows(2)
                .all(|pair| pair[1] as u8 == pair[0] as u8 + 1)
        );
    }

    #[test]
    fn every_new_odin_sequence_must_advance_and_exact_retry_is_idempotent() -> Result<()> {
        let first = topology(7, 1);
        assert!(sequence_requires_admission(None, 6, &first)?);
        assert!(!sequence_requires_admission(
            Some(&first),
            7,
            &topology(7, 1)
        )?);
        assert!(sequence_requires_admission(
            Some(&first),
            7,
            &topology(8, 2)
        )?);
        assert!(sequence_requires_admission(Some(&first), 7, &topology(7, 2)).is_err());
        assert!(sequence_requires_admission(Some(&first), 7, &topology(6, 3)).is_err());
        Ok(())
    }

    #[test]
    fn odin_sequence_cursors_are_scoped_by_target_and_signer() -> Result<()> {
        let command = command(CommandKind::Deploy);
        let mut ghostlight =
            DeploymentTransaction::new(&command, "ghostlight".into(), 0, None, 100)?;
        ghostlight.latest_odin_observation = Some(topology(7, 1));
        ghostlight.odin_publisher_sequence_cursor = 7;
        let mut odin = DeploymentTransaction::new(&command, "odin".into(), 1, None, 100)?;
        odin.latest_odin_observation = Some(topology(41, 2));
        odin.odin_publisher_sequence_cursor = 41;
        let envelope = |key: &str| CultCacheEnvelope {
            key: key.into(),
            r#type: DeploymentTransaction::TYPE.into(),
            payload: Vec::new(),
            stored_at: "1970-01-01T00:00:00.100Z".into(),
            schema_id: Some(DEPLOYMENT_TRANSACTION_SCHEMA.into()),
        };
        let snapshot = ControlSnapshot {
            commands: Vec::new(),
            transactions: vec![
                Stored {
                    envelope: envelope("ghostlight"),
                    value: ghostlight,
                },
                Stored {
                    envelope: envelope("odin"),
                    value: odin,
                },
            ],
            admitted: Vec::new(),
        };

        assert_eq!(snapshot.max_odin_sequence("ghostlight", "odin-signer"), 7);
        assert_eq!(snapshot.max_odin_sequence("odin", "odin-signer"), 41);
        assert_eq!(snapshot.max_odin_sequence("ghostlight", "other-signer"), 0);
        Ok(())
    }

    #[test]
    fn stale_odin_provider_receipt_cannot_gate_continuity_restart() -> Result<()> {
        let touched = std::cell::Cell::new(false);
        validate_live_providers_for_deploy(CommandKind::Continuity, || {
            touched.set(true);
            bail!("Odin provider receipt is stale")
        })?;
        assert!(!touched.get());
        assert!(
            validate_live_providers_for_deploy(CommandKind::Deploy, || {
                bail!("Odin provider receipt is stale")
            })
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn prepared_write_lease_is_durable_but_not_admitted_authority() -> Result<()> {
        let lease = write_lease();
        lease.validate()?;
        let prepared = LeasingEvidence::Prepared {
            lease: lease.clone(),
            lease_sha256: lease.canonical_sha256()?,
        };
        let encoded = rmp_serde::to_vec(&prepared)?;
        let decoded: LeasingEvidence = rmp_serde::from_slice(&encoded)?;
        assert_eq!(decoded, prepared);
        assert!(decoded.lease().is_none());
        assert!(decoded.lease_sha256().is_none());
        assert_eq!(decoded.prepared_lease().unwrap().0, &lease);
        Ok(())
    }

    #[test]
    fn runtime_instance_identity_is_stable_for_activation_prepare_replay() -> Result<()> {
        let first = runtime_instance_id("tx-one")?;
        assert_eq!(runtime_instance_id("tx-one")?, first);
        assert_ne!(runtime_instance_id("tx-two")?, first);
        Ok(())
    }

    #[test]
    fn pre_fencing_abort_is_durable_before_cleanup_and_cannot_resume_as_success() -> Result<()> {
        let command = command(CommandKind::Deploy);
        let mut transaction =
            DeploymentTransaction::new(&command, "ghostlight".into(), 0, None, 100)?;
        transaction.pre_fencing_abort = Some(PreFencingAbort {
            error: "sealed source was rejected".into(),
            candidate_cleanup: CleanupEvidence::Skipped,
            topology_reconciliation: CleanupEvidence::Skipped,
            source_cleanup: CleanupEvidence::Pending,
        });
        transaction.validate()?;
        assert!(!transaction.is_terminal());

        transaction
            .pre_fencing_abort
            .as_mut()
            .unwrap()
            .source_cleanup = CleanupEvidence::Complete;
        transaction.phase = DeploymentPhase::Complete;
        transaction.last_error = Some("sealed source was rejected".into());
        transaction.completion = Some(TransactionCompletion::FailedBeforeFencing {
            error: "sealed source was rejected".into(),
        });
        transaction.validate()?;
        assert!(transaction.is_terminal());
        Ok(())
    }

    #[test]
    fn activation_without_workload_still_requires_durable_candidate_cleanup() {
        assert_eq!(
            candidate_cleanup_requirement(true, false),
            CleanupEvidence::Pending
        );
        assert_eq!(
            candidate_cleanup_requirement(true, true),
            CleanupEvidence::Pending
        );
        assert_eq!(
            candidate_cleanup_requirement(false, false),
            CleanupEvidence::Skipped
        );
    }

    #[test]
    fn bad_selector_becomes_one_terminal_refusal_record() -> Result<()> {
        let command = command(CommandKind::Deploy);
        let transaction = DeploymentTransaction::rejected(
            &command,
            anyhow!("deployment selector is unknown"),
            100,
        )?;
        assert!(transaction.is_terminal());
        assert_eq!(transaction.target, command.selector);
        assert!(matches!(
            transaction.completion,
            Some(TransactionCompletion::FailedBeforeFencing { .. })
        ));
        Ok(())
    }

    #[test]
    fn post_commit_cleanup_keeps_exact_incumbent_work_owned_until_retired() -> Result<()> {
        let incumbent = workload(1001, 40, 50);
        let cleanup = PostCommitCleanup {
            incumbent: IncumbentCleanupEvidence::Pending {
                generation_id: "generation-old".into(),
                workload: incumbent.clone(),
            },
            source: SourceCleanupEvidence::Pending,
        };
        let encoded = rmp_serde::to_vec(&cleanup)?;
        let mut replayed: PostCommitCleanup = rmp_serde::from_slice(&encoded)?;
        assert_eq!(replayed, cleanup);
        assert!(!replayed.is_complete());
        replayed.incumbent = IncumbentCleanupEvidence::Complete {
            generation_id: "generation-old".into(),
        };
        assert!(!replayed.is_complete());
        replayed.source = SourceCleanupEvidence::Complete;
        assert!(replayed.is_complete());
        Ok(())
    }

    #[test]
    fn routed_incumbent_retirement_waits_for_the_declared_drain_deadline() -> Result<()> {
        assert_eq!(route_drain_deadline(1_000, 30)?, 31_000);
        assert!(route_drain_deadline(u64::MAX, 1).is_err());
        Ok(())
    }

    #[test]
    fn admitted_route_receipt_refreshes_when_stale_or_implausibly_future_dated() {
        assert!(route_observation_is_current(900, 1_000, 100, 10));
        assert!(route_observation_is_current(1_010, 1_000, 100, 10));
        assert!(!route_observation_is_current(899, 1_000, 100, 10));
        assert!(!route_observation_is_current(1_011, 1_000, 100, 10));
    }

    #[test]
    fn lease_warming_requires_both_new_odin_and_new_provider_evidence() {
        let old = digest('1');
        let fresh = digest('2');
        assert!(!provider_warming_advanced(7, &old, 7, &fresh));
        assert!(!provider_warming_advanced(7, &old, 8, &old));
        assert!(provider_warming_advanced(7, &old, 8, &fresh));
    }

    #[test]
    fn failed_route_proof_never_restores_any_fenced_incumbent() {
        assert!(!may_rollback_route_after_failed_proof(
            &FencingEvidence::Revoked {
                incumbent_lease_sha256: Some(digest('1')),
                candidate_lease_path_verified_empty: false,
            }
        ));
        assert!(!may_rollback_route_after_failed_proof(
            &FencingEvidence::Revoked {
                incumbent_lease_sha256: None,
                candidate_lease_path_verified_empty: true,
            }
        ));
        assert!(may_rollback_route_after_failed_proof(
            &FencingEvidence::SkippedStateless
        ));
    }

    #[test]
    fn fenced_writer_is_already_retired_before_post_commit_cleanup() {
        assert!(incumbent_was_stopped_during_fencing(
            &FencingEvidence::Revoked {
                incumbent_lease_sha256: Some(digest('1')),
                candidate_lease_path_verified_empty: false,
            }
        ));
        assert!(!incumbent_was_stopped_during_fencing(
            &FencingEvidence::Revoked {
                incumbent_lease_sha256: None,
                candidate_lease_path_verified_empty: true,
            }
        ));
        assert!(!incumbent_was_stopped_during_fencing(
            &FencingEvidence::SkippedStateless
        ));
    }

    #[test]
    fn warming_accepts_only_the_exact_projected_incumbent_lease_disagreement() {
        let expected_detail = format!("expected:{};activation:{}", digest('1'), digest('2'));
        let incumbent = digest('3');
        let exact = OdinTopologyDisagreement {
            code: "projected-write-lease".into(),
            expected: Some(expected_detail.clone()),
            observed: Some(incumbent.clone()),
        };

        assert!(warming_disagreements_match_incumbent(
            &expected_detail,
            Some(&incumbent),
            &[exact.clone()],
        ));
        assert!(!warming_disagreements_match_incumbent(
            &expected_detail,
            None,
            &[exact.clone()],
        ));

        let mut substituted = exact.clone();
        substituted.observed = Some(digest('4'));
        assert!(!warming_disagreements_match_incumbent(
            &expected_detail,
            Some(&incumbent),
            &[substituted],
        ));
        assert!(!warming_disagreements_match_incumbent(
            &expected_detail,
            Some(&incumbent),
            &[exact.clone(), exact],
        ));
    }

    #[test]
    fn candidate_and_incumbent_must_differ_in_all_three_native_boundaries() {
        let incumbent = workload(1001, 40, 50);
        assert!(prove_isolation(&workload(1002, 41, 51), Some(&incumbent)).is_ok());
        assert!(prove_isolation(&workload(1001, 41, 51), Some(&incumbent)).is_err());
        assert!(prove_isolation(&workload(1002, 40, 51), Some(&incumbent)).is_err());
        assert!(prove_isolation(&workload(1002, 41, 50), Some(&incumbent)).is_err());
    }

    #[test]
    fn exactly_one_inflight_transaction_may_own_a_target() -> Result<()> {
        let command = command(CommandKind::Deploy);
        let one = DeploymentTransaction::new(&command, "ghostlight".into(), 0, None, 100)?;
        let two = DeploymentTransaction::new(&command, "ghostlight".into(), 1, None, 100)?;
        let snapshot = ControlSnapshot {
            commands: vec![Stored {
                envelope: command_envelope(&command, 100)?,
                value: command,
            }],
            transactions: vec![
                Stored {
                    envelope: transaction_envelope(&one, 100)?,
                    value: one,
                },
                Stored {
                    envelope: transaction_envelope(&two, 100)?,
                    value: two,
                },
            ],
            admitted: Vec::new(),
        };
        assert!(snapshot.validate_relations().is_err());
        Ok(())
    }

    #[test]
    fn command_ordinals_do_not_turn_one_target_wait_into_a_global_brake() -> Result<()> {
        let first_command = command(CommandKind::Deploy);
        let first = DeploymentTransaction::new(&first_command, "odin".into(), 0, None, 100)?;
        let second = DeploymentTransaction::new(&first_command, "ghostlight".into(), 1, None, 100)?;
        let mut unrelated_command = command(CommandKind::Deploy);
        unrelated_command.command_id = "up-unrelated".into();
        let unrelated =
            DeploymentTransaction::new(&unrelated_command, "huginn".into(), 0, None, 101)?;
        let snapshot = ControlSnapshot {
            commands: Vec::new(),
            transactions: [&first, &second, &unrelated]
                .into_iter()
                .map(|value| Stored {
                    envelope: transaction_envelope(value, 100).unwrap(),
                    value: value.clone(),
                })
                .collect(),
            admitted: Vec::new(),
        };
        assert!(!snapshot.has_earlier_authority_sibling(&first));
        assert!(snapshot.has_earlier_authority_sibling(&second));
        assert!(!snapshot.has_earlier_authority_sibling(&unrelated));
        Ok(())
    }

    #[test]
    fn complete_cleanup_remains_retryable_without_owning_the_current_incarnation() -> Result<()> {
        let command = command(CommandKind::Deploy);
        let mut transaction =
            DeploymentTransaction::new(&command, "ghostlight".into(), 0, None, 100)?;
        transaction.phase = DeploymentPhase::Complete;
        transaction.completion = Some(TransactionCompletion::Admitted {
            generation_id: format!("generation-{}", transaction.transaction_id),
        });
        transaction.post_commit_cleanup = Some(PostCommitCleanup {
            incumbent: IncumbentCleanupEvidence::SkippedNoIncumbent,
            source: SourceCleanupEvidence::Pending,
        });

        assert!(!transaction.is_terminal());
        assert!(!transaction.owns_target_authority());
        assert!(transaction.blocks_new_target_mutation());
        Ok(())
    }

    #[test]
    fn legacy_mutable_command_schema_is_rejected() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("control.cc");
        let legacy = CultCacheEnvelope {
            key: "up-legacy".into(),
            r#type: DeploymentCommand::TYPE.into(),
            payload: rmp_serde::to_vec(&(DEPLOYMENT_COMMAND_SCHEMA, "up-legacy", "running"))?,
            stored_at: rfc3339_millis(100)?,
            schema_id: Some("idunn.deployment_command.v1".into()),
        };
        assert!(
            SingleFileMessagePackBackingStore::new(&path).compare_exchange(
                &[CultCacheExpectedEnvelope {
                    r#type: DeploymentCommand::TYPE.into(),
                    key: "up-legacy".into(),
                    current: None,
                }],
                &[legacy],
            )?
        );
        assert!(ControlSnapshot::read(&path).is_err());
        Ok(())
    }

    #[test]
    fn loadcredential_era_control_schemas_are_rejected_before_rehydration() -> Result<()> {
        for (record_type, key, schema) in [
            (
                DeploymentTransaction::TYPE,
                "tx-legacy-workload-observation",
                "idunn.deployment_transaction.v1",
            ),
            (
                AdmittedGeneration::TYPE,
                "legacy-admitted-target",
                "idunn.admitted_generation.v1",
            ),
        ] {
            let temporary = tempfile::tempdir()?;
            let path = temporary.path().join("control.cc");
            let legacy = CultCacheEnvelope {
                key: key.into(),
                r#type: record_type.into(),
                payload: Vec::new(),
                stored_at: rfc3339_millis(100)?,
                schema_id: Some(schema.into()),
            };
            assert!(
                SingleFileMessagePackBackingStore::new(&path).compare_exchange(
                    &[CultCacheExpectedEnvelope {
                        r#type: record_type.into(),
                        key: key.into(),
                        current: None,
                    }],
                    &[legacy],
                )?
            );
            assert!(ControlSnapshot::read(&path).is_err());
        }
        Ok(())
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
}

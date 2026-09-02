use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const TARGET_DECLARATION_SCHEMA: &str = "gamecult.idunn.target_declaration.v1";
pub const OPERATOR_BINDING_SCHEMA: &str = "gamecult.idunn.operator_binding.v1";
pub const PINNED_TAR_MEMBER_SCHEMA: &str = "gamecult.idunn.pinned_tar_member.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetDeclaration {
    pub schema: String,
    pub target: String,
    pub source_stamp_environment: String,
    #[serde(default)]
    pub required_gitlinks: BTreeSet<PathBuf>,
    #[serde(default)]
    pub steps: Vec<RecipeStep>,
    #[serde(default)]
    pub external_artifacts: Vec<ExternalArtifact>,
    pub artifacts: Vec<ArtifactOutput>,
    pub service: ServiceDeclaration,
    pub state: StateDeclaration,
    #[serde(default)]
    pub provides: Vec<ProvidedCapability>,
    #[serde(default)]
    pub dependencies: Vec<CapabilityDependency>,
    #[serde(default)]
    pub conflicts: Vec<CapabilityConflict>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerAffordance {
    SourceRead,
    ArtifactWrite,
    DependencyNetwork,
    BuildCache,
    PrivateCapabilityAccess,
    SecretRead,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeStep {
    pub id: String,
    pub phase: RecipePhase,
    pub runner: String,
    pub argv: Vec<String>,
    #[serde(default = "default_working_directory")]
    pub working_directory: PathBuf,
    #[serde(default)]
    pub required_environment: BTreeSet<String>,
}

fn default_working_directory() -> PathBuf {
    PathBuf::from(".")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecipePhase {
    Prepare,
    Test,
    Build,
    Acceptance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalArtifact {
    pub id: String,
    pub runner: String,
    pub manifest: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedTarMemberManifest {
    pub schema: String,
    pub url: String,
    pub archive_sha256: String,
    pub member: PathBuf,
    pub member_sha256: String,
}

impl PinnedTarMemberManifest {
    pub fn parse(input: &str) -> Result<Self> {
        let manifest: Self =
            toml::from_str(input).context("decoding strict pinned tar-member manifest")?;
        ensure!(
            manifest.schema == PINNED_TAR_MEMBER_SCHEMA,
            "unsupported pinned artifact schema"
        );
        https_host(&manifest.url)?;
        require_sha256(&manifest.archive_sha256, "archive sha256")?;
        require_relative_path(&manifest.member, "archive member", false)?;
        require_sha256(&manifest.member_sha256, "member sha256")?;
        Ok(manifest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactOutput {
    pub id: String,
    pub source_kind: ArtifactSource,
    pub runner: Option<String>,
    pub source: PathBuf,
    pub destination: PathBuf,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactSource {
    RunnerOutput,
    WorktreeTree,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDeclaration {
    pub executable_artifact: String,
    #[serde(default)]
    pub required_adjacent_artifacts: BTreeSet<String>,
    #[serde(default)]
    pub arguments: Vec<LaunchArgument>,
    pub transport: ServiceTransport,
    #[serde(default)]
    pub route_required: bool,
    pub health: HealthDeclaration,
    #[serde(default)]
    pub required_environment: BTreeSet<String>,
    #[serde(default)]
    pub optional_environment: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthDeclaration {
    pub contract: String,
    pub staged: HealthState,
    pub ready: HealthState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthState {
    pub state: String,
    pub detail: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceTransport {
    Http,
    Tcp,
    Rudp,
    Private,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LaunchArgument {
    Literal { value: String },
    Binding { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateDeclaration {
    pub generation: String,
    pub slots: Vec<StateSlot>,
    pub migration: Option<MigrationDeclaration>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateSlot {
    pub id: String,
    pub relative_path: PathBuf,
    pub kind: StateKind,
    pub schema: String,
    pub writer: StateWriter,
    pub recovery: StateRecovery,
    pub startup: StateStartup,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MigrationDeclaration {
    pub executable_artifact: String,
    pub from_generations: BTreeSet<String>,
    pub to_generation: String,
    #[serde(default)]
    pub arguments: Vec<LaunchArgument>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateKind {
    CultcacheFile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateWriter {
    ProcessBoundSingleWriter,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateRecovery {
    Preserve,
    Rebuildable,
    ExternalAuthority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateStartup {
    CreateOrOpenAfterAdmission,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidedCapability {
    pub capability: String,
    pub schema: String,
    pub compatibility: String,
    #[serde(default = "one")]
    pub capacity: u32,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDependency {
    pub kind: DependencyKind,
    pub capability: String,
    pub schema: String,
    pub compatibility: String,
    #[serde(default = "one")]
    pub minimum_capacity: u32,
    #[serde(default)]
    pub startup: StartupOrder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    Bootstrap,
    Required,
    Optional,
    SharedInfrastructure,
    Private,
    ExternalOperatorBinding,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupOrder {
    #[default]
    BeforePromotion,
    BeforeStart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityConflict {
    pub capability: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorBinding {
    pub schema: String,
    pub target: String,
    pub repository: RepositoryBinding,
    pub runners: BTreeMap<String, RunnerBinding>,
    pub workload: WorkloadBinding,
    pub route: Option<RouteBinding>,
    pub admission: AdmissionBinding,
    pub brakes: BrakeBinding,
    pub state_transition: StateTransitionBinding,
    pub rollout: RolloutBinding,
    pub placement: PlacementBinding,
    #[serde(default)]
    pub external_capabilities: Vec<ExternalCapabilityBinding>,
    #[serde(default)]
    pub profiles: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryBinding {
    pub origin: String,
    pub admitted_ref: String,
    pub minimum_revision: String,
    pub selection: SourceSelectionPolicy,
    pub pinned_revision: Option<String>,
    pub release_authority_store: Option<PathBuf>,
    pub checkout: PathBuf,
    pub recipe_path: PathBuf,
    #[serde(default)]
    pub gitlinks: BTreeMap<PathBuf, GitlinkBinding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceSelectionPolicy {
    PinnedObject,
    RefHead,
    SignedRelease,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitlinkBinding {
    pub origin: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerBinding {
    pub driver: RunnerDriver,
    pub image: String,
    #[serde(default)]
    pub affordances: BTreeSet<RunnerAffordance>,
    pub cache_root: Option<PathBuf>,
    #[serde(default)]
    pub allowed_https_hosts: BTreeSet<String>,
    #[serde(default)]
    pub allowed_programs: BTreeSet<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_files: BTreeMap<String, PathBuf>,
    pub network: RunnerNetwork,
    pub memory_mebibytes: u32,
    pub cpu_quota_percent: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerDriver {
    Docker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerNetwork {
    None,
    DependencyEgress,
    HostPrivate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadBinding {
    pub driver: WorkloadDriver,
    pub user: String,
    pub group: String,
    pub unit_prefix: String,
    pub release_root: PathBuf,
    pub state_root: PathBuf,
    pub runtime_root: PathBuf,
    pub network: WorkloadNetwork,
    pub hardening: WorkloadHardening,
    #[serde(default)]
    pub read_only_paths: BTreeSet<PathBuf>,
    #[serde(default)]
    pub read_write_paths: BTreeSet<PathBuf>,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
    #[serde(default)]
    pub devices: BTreeSet<PathBuf>,
    pub memory_mebibytes: u32,
    pub cpu_quota_percent: u32,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_files: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub argument_bindings: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkloadDriver {
    SystemdTransient,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkloadNetwork {
    HostPrivate,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkloadHardening {
    Strict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionBinding {
    pub driver: AdmissionDriver,
    pub record_path: PathBuf,
    pub lock_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionDriver {
    AtomicFile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrakeBinding {
    pub deployment_store: PathBuf,
    pub lifecycle_store: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateTransitionBinding {
    pub policy: StateTransitionPolicy,
    pub archive_root: Option<PathBuf>,
    pub backup_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateTransitionPolicy {
    Preserve,
    FencedMigration,
    FreshRoot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteBinding {
    pub driver: RouteDriver,
    pub route_id: String,
    pub stable_endpoint: String,
    pub private_host: String,
    pub private_port_start: u16,
    pub private_port_end: u16,
    pub config_path: PathBuf,
    pub reload_unit: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteDriver {
    NginxHttp,
    NginxStreamTcp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloutBinding {
    pub strategy: RolloutStrategy,
    pub drain_seconds: u32,
    pub retain_releases: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RolloutStrategy {
    CandidateThenPromote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacementBinding {
    pub desired_replicas: u32,
    pub nodes: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalCapabilityBinding {
    pub capability: String,
    pub schema: String,
    pub compatibility: String,
    pub endpoint: String,
}

impl TargetDeclaration {
    pub fn parse(input: &str) -> Result<Self> {
        let declaration: Self =
            toml::from_str(input).context("decoding strict Idunn target declaration")?;
        declaration.validate()?;
        Ok(declaration)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == TARGET_DECLARATION_SCHEMA,
            "unsupported target declaration schema"
        );
        require_id(&self.target, "target")?;
        require_environment_name(&self.source_stamp_environment)?;
        for path in &self.required_gitlinks {
            require_relative_path(path, "required gitlink", false)?;
        }
        ensure!(!self.steps.is_empty(), "no recipe steps declared");
        ensure!(!self.artifacts.is_empty(), "no artifact outputs declared");

        let mut runner_ids = BTreeSet::new();
        unique_ids(
            self.steps.iter().map(|step| step.id.as_str()),
            "recipe step",
        )?;
        for step in &self.steps {
            require_id(&step.runner, "runner class")?;
            runner_ids.insert(step.runner.as_str());
            ensure!(!step.argv.is_empty(), "step {} has empty argv", step.id);
            for argument in &step.argv {
                require_value(argument, "step argv")?;
            }
            require_program(&step.argv[0], "step program")?;
            require_relative_path(&step.working_directory, "step working directory", true)?;
            for name in &step.required_environment {
                require_environment_name(name)?;
            }
        }

        let mut artifact_ids = BTreeSet::new();
        let mut destinations = BTreeSet::new();
        for artifact in &self.artifacts {
            require_id(&artifact.id, "artifact")?;
            ensure!(
                artifact_ids.insert(artifact.id.as_str()),
                "artifact id {} is declared twice",
                artifact.id
            );
            require_relative_path(&artifact.source, "artifact source", false)?;
            require_leaf_path(&artifact.destination, "artifact destination")?;
            ensure!(
                destinations.insert(artifact.destination.as_path()),
                "artifact destination {} is declared twice",
                artifact.destination.display()
            );
            match (artifact.source_kind, artifact.runner.as_deref()) {
                (ArtifactSource::RunnerOutput, Some(runner)) => {
                    require_id(runner, "artifact runner class")?;
                    runner_ids.insert(runner);
                }
                (ArtifactSource::WorktreeTree, None) => {}
                (ArtifactSource::RunnerOutput, None) => {
                    bail!("runner-output artifact {} has no runner class", artifact.id)
                }
                (ArtifactSource::WorktreeTree, Some(_)) => bail!(
                    "worktree-tree artifact {} cannot name a runner class",
                    artifact.id
                ),
            }
        }
        for artifact in &self.external_artifacts {
            require_id(&artifact.id, "external artifact")?;
            ensure!(
                artifact_ids.insert(artifact.id.as_str()),
                "artifact id {} is declared twice",
                artifact.id
            );
            require_id(&artifact.runner, "external artifact runner class")?;
            runner_ids.insert(artifact.runner.as_str());
            require_relative_path(&artifact.manifest, "external artifact manifest", false)?;
            require_leaf_path(&artifact.destination, "external artifact destination")?;
            ensure!(
                destinations.insert(artifact.destination.as_path()),
                "artifact destination {} is declared twice",
                artifact.destination.display()
            );
        }
        ensure!(
            !runner_ids.is_empty(),
            "target declaration uses no runner classes"
        );

        ensure!(
            artifact_ids.contains(self.service.executable_artifact.as_str()),
            "service executable artifact is not declared"
        );
        for artifact in &self.service.required_adjacent_artifacts {
            ensure!(
                artifact_ids.contains(artifact.as_str()),
                "adjacent service artifact {artifact} is not declared"
            );
        }
        for argument in &self.service.arguments {
            validate_launch_argument(argument)?;
        }
        require_capability(&self.service.health.contract, "health contract")?;
        for (label, health) in [
            ("staged health", &self.service.health.staged),
            ("ready health", &self.service.health.ready),
        ] {
            require_value(&health.state, label)?;
            require_value(&health.detail, label)?;
        }
        ensure!(
            self.service.health.staged != self.service.health.ready,
            "staged and ready health states are identical"
        );
        for name in &self.service.required_environment {
            require_environment_name(name)?;
        }
        for name in &self.service.optional_environment {
            require_environment_name(name)?;
            ensure!(
                !self.service.required_environment.contains(name),
                "service environment {name} is both required and optional"
            );
        }

        require_capability(&self.state.generation, "state generation")?;
        let slot_ids = unique_ids(
            self.state.slots.iter().map(|slot| slot.id.as_str()),
            "state slot",
        )?;
        let mut slot_paths = BTreeSet::new();
        for slot in &self.state.slots {
            require_relative_path(&slot.relative_path, "state slot path", false)?;
            ensure!(
                slot_paths.insert(slot.relative_path.as_path()),
                "state slot path {} is declared twice",
                slot.relative_path.display()
            );
            require_capability(&slot.schema, "state slot schema")?;
            if slot.writer == StateWriter::ProcessBoundSingleWriter {
                ensure!(
                    slot.startup == StateStartup::CreateOrOpenAfterAdmission,
                    "writable state slot {} must open after write admission",
                    slot.id
                );
            }
            if slot.recovery == StateRecovery::ExternalAuthority {
                ensure!(
                    slot.writer == StateWriter::None,
                    "externally authoritative state slot {} cannot name the service as writer",
                    slot.id
                );
            }
        }
        if let Some(migration) = self.state.migration.as_ref() {
            ensure!(
                artifact_ids.contains(migration.executable_artifact.as_str()),
                "migration executable artifact is not declared"
            );
            ensure!(
                !migration.from_generations.is_empty(),
                "migration has no source generations"
            );
            for generation in &migration.from_generations {
                require_capability(generation, "migration source generation")?;
            }
            ensure!(
                migration.to_generation == self.state.generation,
                "migration destination differs from state generation"
            );
            for argument in &migration.arguments {
                validate_launch_argument(argument)?;
            }
        }
        ensure!(
            slot_ids.len() == self.state.slots.len(),
            "duplicate state slot"
        );

        let mut provided = BTreeSet::new();
        for capability in &self.provides {
            require_capability_contract(
                &capability.capability,
                &capability.schema,
                &capability.compatibility,
            )?;
            ensure!(
                capability.capacity > 0,
                "provided capability capacity must be positive"
            );
            ensure!(
                provided.insert(capability.capability.as_str()),
                "capability {} is provided twice",
                capability.capability
            );
        }
        let mut required = BTreeSet::new();
        for dependency in &self.dependencies {
            require_capability_contract(
                &dependency.capability,
                &dependency.schema,
                &dependency.compatibility,
            )?;
            ensure!(
                dependency.minimum_capacity > 0,
                "dependency capacity must be positive"
            );
            ensure!(
                required.insert((dependency.kind, dependency.capability.as_str())),
                "dependency {} is declared twice for the same kind",
                dependency.capability
            );
        }
        for conflict in &self.conflicts {
            require_capability(&conflict.capability, "conflicting capability")?;
            require_value(&conflict.reason, "conflict reason")?;
        }
        Ok(())
    }
}

impl OperatorBinding {
    pub fn parse(input: &str) -> Result<Self> {
        let binding: Self =
            toml::from_str(input).context("decoding strict Idunn operator binding")?;
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == OPERATOR_BINDING_SCHEMA,
            "unsupported operator binding schema"
        );
        require_id(&self.target, "target")?;
        ensure!(
            self.repository.origin.starts_with("https://"),
            "repository origin must use HTTPS"
        );
        ensure!(
            self.repository.origin.ends_with(".git"),
            "repository origin must name a Git repository"
        );
        ensure!(
            self.repository.admitted_ref.starts_with("refs/heads/"),
            "admitted ref must be a full branch ref"
        );
        require_sha1(&self.repository.minimum_revision, "minimum revision")?;
        match self.repository.selection {
            SourceSelectionPolicy::PinnedObject => {
                let revision = self.repository.pinned_revision.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("pinned-object selection has no pinned revision")
                })?;
                require_sha1(revision, "pinned revision")?;
                ensure!(
                    self.repository.release_authority_store.is_none(),
                    "pinned-object selection cannot name a release-authority store"
                );
            }
            SourceSelectionPolicy::RefHead => {
                ensure!(
                    self.repository.pinned_revision.is_none(),
                    "ref-head selection cannot name a pinned revision"
                );
                ensure!(
                    self.repository.release_authority_store.is_none(),
                    "ref-head selection cannot name a release-authority store"
                );
            }
            SourceSelectionPolicy::SignedRelease => {
                ensure!(
                    self.repository.pinned_revision.is_none(),
                    "signed-release selection cannot name a pinned revision"
                );
                let store = self
                    .repository
                    .release_authority_store
                    .as_ref()
                    .ok_or_else(|| {
                        anyhow::anyhow!("signed-release selection has no authority store")
                    })?;
                require_absolute_path(store, "release authority store")?;
            }
        }
        require_absolute_path(&self.repository.checkout, "repository checkout")?;
        require_relative_path(&self.repository.recipe_path, "recipe path", false)?;
        for (path, gitlink) in &self.repository.gitlinks {
            require_relative_path(path, "gitlink binding", false)?;
            require_git_origin(&gitlink.origin, "gitlink origin")?;
        }
        ensure!(!self.runners.is_empty(), "operator binding has no runners");
        for (id, runner) in &self.runners {
            require_id(id, "runner binding")?;
            require_pinned_image(&runner.image)?;
            ensure!(
                runner.memory_mebibytes > 0,
                "runner memory must be positive"
            );
            ensure!(
                (1..=100_000).contains(&runner.cpu_quota_percent),
                "runner CPU quota is outside 1..=100000"
            );
            if let Some(cache_root) = runner.cache_root.as_ref() {
                require_absolute_path(cache_root, "runner cache root")?;
            }
            for program in &runner.allowed_programs {
                require_program(program, "allowed runner program")?;
            }
            ensure!(
                !runner.allowed_programs.is_empty(),
                "runner {id} has no allowed programs"
            );
            for host in &runner.allowed_https_hosts {
                require_host(host)?;
            }
            match runner.network {
                RunnerNetwork::None => ensure!(
                    runner.allowed_https_hosts.is_empty(),
                    "network-none runner {id} has HTTPS hosts"
                ),
                RunnerNetwork::DependencyEgress => {
                    ensure!(
                        runner
                            .affordances
                            .contains(&RunnerAffordance::DependencyNetwork),
                        "dependency-egress runner {id} does not grant dependency-network"
                    );
                    ensure!(
                        !runner.allowed_https_hosts.is_empty(),
                        "dependency-egress runner {id} has no allowed hosts"
                    );
                }
                RunnerNetwork::HostPrivate => ensure!(
                    runner
                        .affordances
                        .contains(&RunnerAffordance::PrivateCapabilityAccess),
                    "host-private runner {id} does not grant private-capability-access"
                ),
            }
            for name in runner.environment.keys().chain(runner.secret_files.keys()) {
                require_environment_name(name)?;
            }
            for path in runner.secret_files.values() {
                require_absolute_path(path, "runner secret file")?;
            }
            if runner.affordances.contains(&RunnerAffordance::BuildCache) {
                ensure!(
                    runner.cache_root.is_some(),
                    "build-cache runner {id} has no cache root"
                );
            }
        }
        require_identity(&self.workload.user, "workload user")?;
        require_identity(&self.workload.group, "workload group")?;
        require_id(&self.workload.unit_prefix, "workload unit prefix")?;
        for (label, path) in [
            ("release root", &self.workload.release_root),
            ("state root", &self.workload.state_root),
            ("runtime root", &self.workload.runtime_root),
            ("admission record", &self.admission.record_path),
            ("admission lock", &self.admission.lock_path),
            ("deployment brake", &self.brakes.deployment_store),
            ("lifecycle brake", &self.brakes.lifecycle_store),
        ] {
            require_absolute_path(path, label)?;
        }
        ensure!(
            self.admission.record_path != self.admission.lock_path,
            "admission record and lock paths are identical"
        );
        ensure!(
            self.brakes.deployment_store != self.brakes.lifecycle_store,
            "deployment and lifecycle brakes are identical"
        );
        ensure!(
            self.workload.memory_mebibytes > 0,
            "workload memory must be positive"
        );
        ensure!(
            (1..=100_000).contains(&self.workload.cpu_quota_percent),
            "workload CPU quota is outside 1..=100000"
        );
        for path in self
            .workload
            .read_only_paths
            .iter()
            .chain(self.workload.read_write_paths.iter())
            .chain(self.workload.devices.iter())
        {
            require_absolute_path(path, "workload path")?;
        }
        for capability in &self.workload.capabilities {
            require_value(capability, "workload capability")?;
        }
        for name in self
            .workload
            .environment
            .keys()
            .chain(self.workload.secret_files.keys())
        {
            require_environment_name(name)?;
        }
        for name in self.workload.argument_bindings.keys() {
            require_id(name, "argument binding")?;
        }
        for value in self.workload.argument_bindings.values() {
            require_value(value, "argument binding value")?;
        }
        for path in self.workload.secret_files.values() {
            require_absolute_path(path, "secret file")?;
        }
        if let Some(route) = self.route.as_ref() {
            require_id(&route.route_id, "route id")?;
            require_value(&route.stable_endpoint, "stable endpoint")?;
            require_host(&route.private_host)?;
            ensure!(
                route.private_port_start > 0,
                "private port range starts at zero"
            );
            ensure!(
                route.private_port_start <= route.private_port_end,
                "private port range is inverted"
            );
            require_absolute_path(&route.config_path, "route config")?;
            require_unit(&route.reload_unit, "route reload unit")?;
        }
        match self.state_transition.policy {
            StateTransitionPolicy::Preserve => {}
            StateTransitionPolicy::FencedMigration => ensure!(
                self.state_transition.backup_root.is_some(),
                "fenced migration has no backup root"
            ),
            StateTransitionPolicy::FreshRoot => ensure!(
                self.state_transition.archive_root.is_some(),
                "fresh-root transition has no whole-root archive"
            ),
        }
        if let Some(path) = self.state_transition.archive_root.as_ref() {
            require_absolute_path(path, "state archive root")?;
        }
        if let Some(path) = self.state_transition.backup_root.as_ref() {
            require_absolute_path(path, "state backup root")?;
        }
        ensure!(
            self.rollout.retain_releases >= 2,
            "candidate rollout must retain at least current and prior releases"
        );
        ensure!(
            self.placement.desired_replicas > 0,
            "desired replicas must be positive"
        );
        ensure!(!self.placement.nodes.is_empty(), "placement has no nodes");
        for node in &self.placement.nodes {
            require_id(node, "placement node")?;
        }
        for capability in &self.external_capabilities {
            require_capability_contract(
                &capability.capability,
                &capability.schema,
                &capability.compatibility,
            )?;
            require_value(&capability.endpoint, "external capability endpoint")?;
        }
        for profile in &self.profiles {
            require_capability(profile, "profile")?;
        }
        Ok(())
    }

    pub fn admit(&self, declaration: &TargetDeclaration) -> Result<()> {
        ensure!(
            self.target == declaration.target,
            "binding target does not match declaration"
        );
        let bound_gitlinks: BTreeSet<_> = self.repository.gitlinks.keys().cloned().collect();
        ensure!(
            bound_gitlinks == declaration.required_gitlinks,
            "operator gitlink bindings do not exactly match required gitlinks"
        );
        let declared_runners: BTreeSet<_> = declaration
            .steps
            .iter()
            .map(|step| step.runner.as_str())
            .chain(
                declaration
                    .external_artifacts
                    .iter()
                    .map(|artifact| artifact.runner.as_str()),
            )
            .chain(
                declaration
                    .artifacts
                    .iter()
                    .filter_map(|artifact| artifact.runner.as_deref()),
            )
            .collect();
        let bound_runners: BTreeSet<_> = self.runners.keys().map(String::as_str).collect();
        ensure!(
            declared_runners == bound_runners,
            "operator runner classes do not exactly match the target recipe"
        );
        for step in &declaration.steps {
            let binding = &self.runners[&step.runner];
            ensure!(
                binding.affordances.contains(&RunnerAffordance::SourceRead),
                "runner {} cannot read the frozen source",
                step.runner
            );
            ensure!(
                binding.allowed_programs.contains(&step.argv[0]),
                "runner {} does not admit program {}",
                step.runner,
                step.argv[0]
            );
            let available: BTreeSet<_> = binding
                .environment
                .keys()
                .chain(binding.secret_files.keys())
                .cloned()
                .collect();
            ensure!(
                step.required_environment.is_subset(&available),
                "step {} lacks required runner environment",
                step.id
            );
        }
        for artifact in declaration
            .artifacts
            .iter()
            .filter(|artifact| artifact.source_kind == ArtifactSource::RunnerOutput)
        {
            let runner_id = artifact
                .runner
                .as_deref()
                .expect("validated runner-output artifact");
            let binding = &self.runners[runner_id];
            ensure!(
                binding
                    .affordances
                    .contains(&RunnerAffordance::ArtifactWrite),
                "runner {runner_id} cannot publish artifact {}",
                artifact.id
            );
        }
        for artifact in &declaration.external_artifacts {
            let binding = &self.runners[&artifact.runner];
            ensure!(
                binding.affordances.contains(&RunnerAffordance::SourceRead),
                "external artifact {} runner cannot read the frozen source",
                artifact.id
            );
            ensure!(
                binding
                    .affordances
                    .contains(&RunnerAffordance::ArtifactWrite),
                "external artifact {} runner cannot publish artifacts",
                artifact.id
            );
            ensure!(
                binding
                    .affordances
                    .contains(&RunnerAffordance::DependencyNetwork),
                "external artifact {} runner has no dependency-network affordance",
                artifact.id
            );
            ensure!(
                binding.network == RunnerNetwork::DependencyEgress
                    && !binding.allowed_https_hosts.is_empty(),
                "external artifact {} runner has no admitted egress hosts",
                artifact.id
            );
        }
        let available_environment: BTreeSet<_> = self
            .workload
            .environment
            .keys()
            .chain(self.workload.secret_files.keys())
            .cloned()
            .collect();
        ensure!(
            declaration
                .service
                .required_environment
                .is_subset(&available_environment),
            "operator service environment omits a required launch input"
        );
        let declared_environment: BTreeSet<_> = declaration
            .service
            .required_environment
            .union(&declaration.service.optional_environment)
            .cloned()
            .collect();
        ensure!(
            available_environment.is_subset(&declared_environment),
            "operator service environment contains an undeclared launch input"
        );
        let required_arguments: BTreeSet<_> = declaration
            .service
            .arguments
            .iter()
            .filter_map(|argument| match argument {
                LaunchArgument::Binding { name } => Some(name.clone()),
                LaunchArgument::Literal { .. } => None,
            })
            .collect();
        let bound_arguments: BTreeSet<_> =
            self.workload.argument_bindings.keys().cloned().collect();
        ensure!(
            required_arguments == bound_arguments,
            "operator argument bindings do not exactly match the launch contract"
        );
        match (
            declaration.service.route_required,
            declaration.service.transport,
            self.route.as_ref().map(|route| route.driver),
        ) {
            (true, ServiceTransport::Http, Some(RouteDriver::NginxHttp))
            | (true, ServiceTransport::Tcp, Some(RouteDriver::NginxStreamTcp))
            | (false, _, None) => {}
            _ => bail!("route binding does not match the launch contract"),
        }
        match self.state_transition.policy {
            StateTransitionPolicy::Preserve => {}
            StateTransitionPolicy::FencedMigration => ensure!(
                declaration.state.migration.is_some(),
                "binding requests migration but target declares no migrator"
            ),
            StateTransitionPolicy::FreshRoot => {}
        }
        for dependency in declaration
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind == DependencyKind::ExternalOperatorBinding)
        {
            ensure!(
                self.external_capabilities.iter().any(|binding| {
                    capability_compatible(
                        &dependency.capability,
                        &dependency.schema,
                        &dependency.compatibility,
                        &binding.capability,
                        &binding.schema,
                        &binding.compatibility,
                    )
                }),
                "external capability {} has no compatible operator binding",
                dependency.capability
            );
        }
        Ok(())
    }

    pub fn admit_external_artifact_manifest(
        &self,
        declaration: &TargetDeclaration,
        artifact_id: &str,
        manifest_input: &str,
    ) -> Result<PinnedTarMemberManifest> {
        let artifact = declaration
            .external_artifacts
            .iter()
            .find(|artifact| artifact.id == artifact_id)
            .with_context(|| format!("target declares no external artifact {artifact_id}"))?;
        let runner = self.runners.get(&artifact.runner).with_context(|| {
            format!("external artifact runner {} is not bound", artifact.runner)
        })?;
        ensure!(
            runner.network == RunnerNetwork::DependencyEgress,
            "external artifact runner has no dependency egress"
        );
        let manifest = PinnedTarMemberManifest::parse(manifest_input)?;
        let host = https_host(&manifest.url)?;
        ensure!(
            runner.allowed_https_hosts.contains(host),
            "external artifact host {host} is not admitted"
        );
        Ok(manifest)
    }
}

pub fn capability_compatible(
    required_capability: &str,
    required_schema: &str,
    required_compatibility: &str,
    provided_capability: &str,
    provided_schema: &str,
    provided_compatibility: &str,
) -> bool {
    required_capability == provided_capability
        && required_schema == provided_schema
        && required_compatibility == provided_compatibility
}

fn unique_ids<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> Result<BTreeSet<&'a str>> {
    let mut seen = BTreeSet::new();
    for value in values {
        require_id(value, label)?;
        ensure!(seen.insert(value), "duplicate {label} id {value}");
    }
    Ok(seen)
}

fn require_id(value: &str, label: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{label} is empty");
    ensure!(value.len() <= 96, "{label} is too long");
    ensure!(
        value.bytes().all(|byte| byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || matches!(byte, b'-' | b'_' | b'.' | b':')),
        "{label} contains an invalid character"
    );
    Ok(())
}

fn require_capability(value: &str, label: &str) -> Result<()> {
    require_id(value, label)
}

fn require_capability_contract(capability: &str, schema: &str, compatibility: &str) -> Result<()> {
    require_capability(capability, "capability")?;
    require_capability(schema, "capability schema")?;
    require_capability(compatibility, "capability compatibility")
}

fn require_value(value: &str, label: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{label} is empty");
    ensure!(!value.contains('\0'), "{label} contains NUL");
    ensure!(value.len() <= 4_096, "{label} is too long");
    Ok(())
}

fn validate_launch_argument(argument: &LaunchArgument) -> Result<()> {
    match argument {
        LaunchArgument::Literal { value } => require_value(value, "literal launch argument"),
        LaunchArgument::Binding { name } => require_id(name, "launch argument binding"),
    }
}

fn require_program(value: &str, label: &str) -> Result<()> {
    require_value(value, label)?;
    ensure!(
        !value.contains('/') && !value.contains('\\') && !value.chars().any(char::is_whitespace),
        "{label} must be one executable name"
    );
    Ok(())
}

fn require_git_origin(value: &str, label: &str) -> Result<()> {
    ensure!(value.starts_with("https://"), "{label} must use HTTPS");
    ensure!(
        value.ends_with(".git"),
        "{label} must name a Git repository"
    );
    require_value(value, label)
}

fn https_host(value: &str) -> Result<&str> {
    let remainder = value
        .strip_prefix("https://")
        .ok_or_else(|| anyhow::anyhow!("artifact URL must use HTTPS"))?;
    let authority = remainder.split('/').next().unwrap_or_default();
    ensure!(!authority.is_empty(), "artifact URL has no host");
    ensure!(
        !authority.contains('@')
            && !authority.contains(':')
            && !authority.contains('?')
            && !authority.contains('#'),
        "artifact URL authority is not a bare host"
    );
    require_host(authority)?;
    Ok(authority)
}

fn require_environment_name(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 128,
        "environment name is empty or too long"
    );
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_'),
        "environment name contains an invalid character"
    );
    Ok(())
}

fn require_relative_path(path: &Path, label: &str, allow_current: bool) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "{label} is empty");
    ensure!(!path.is_absolute(), "{label} must be relative");
    let mut saw_normal = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal = true,
            Component::CurDir if allow_current => {}
            _ => bail!("{label} contains a forbidden path component"),
        }
    }
    ensure!(allow_current || saw_normal, "{label} must name a path");
    Ok(())
}

fn require_leaf_path(path: &Path, label: &str) -> Result<()> {
    require_relative_path(path, label, false)?;
    ensure!(
        path.components().count() == 1,
        "{label} must be a single filename"
    );
    Ok(())
}

fn require_absolute_path(path: &Path, label: &str) -> Result<()> {
    ensure!(path.is_absolute(), "{label} must be absolute");
    ensure!(
        path.components()
            .all(|component| !matches!(component, Component::ParentDir)),
        "{label} contains a parent traversal"
    );
    Ok(())
}

fn require_sha1(value: &str, label: &str) -> Result<()> {
    require_lower_hex(value, 40, label)
}

fn require_sha256(value: &str, label: &str) -> Result<()> {
    require_lower_hex(value, 64, label)
}

fn require_lower_hex(value: &str, length: usize, label: &str) -> Result<()> {
    ensure!(value.len() == length, "{label} has the wrong length");
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} must be lowercase hexadecimal"
    );
    Ok(())
}

fn require_pinned_image(value: &str) -> Result<()> {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        bail!("runner image is not pinned by sha256 digest");
    };
    require_value(name, "runner image name")?;
    require_sha256(digest, "runner image digest")
}

fn require_identity(value: &str, label: &str) -> Result<()> {
    require_id(value, label)
}

fn require_unit(value: &str, label: &str) -> Result<()> {
    require_value(value, label)?;
    ensure!(
        value.ends_with(".service"),
        "{label} must be a systemd service unit"
    );
    ensure!(
        !value.contains('/') && !value.contains('\\'),
        "{label} must not contain a path"
    );
    Ok(())
}

fn require_host(value: &str) -> Result<()> {
    require_value(value, "host")?;
    ensure!(
        !value.contains('/') && !value.contains('@'),
        "host contains an invalid delimiter"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECIPE: &str = r#"
schema = "gamecult.idunn.target_declaration.v1"
target = "test-service"
source_stamp_environment = "TEST_SERVICE_BUILD_COMMIT"

[[steps]]
id = "test"
phase = "test"
runner = "rust"
argv = ["cargo", "test", "--locked", "-p", "test-service"]

[[steps]]
id = "build"
phase = "build"
runner = "rust"
argv = ["cargo", "build", "--locked", "--release", "-p", "test-service"]

[[external_artifacts]]
id = "tool"
runner = "rust"
manifest = "deployment/tool.toml"
destination = "tool"
executable = true

[[artifacts]]
id = "daemon"
source_kind = "runner-output"
runner = "rust"
source = "target/release/test-service"
destination = "test-service"
executable = true

[service]
executable_artifact = "daemon"
required_adjacent_artifacts = []
arguments = [
  { kind = "literal", value = "--state-root" },
  { kind = "binding", name = "state_root" },
]
transport = "http"
route_required = true
required_environment = ["TEST_SERVICE_BIND"]

[service.health]
contract = "test-service.cultnet-service-health"

[service.health.staged]
state = "warming"
detail = "traffic-admission-pending"

[service.health.ready]
state = "active"
detail = "serving"

[state]
generation = "v1"

[[state.slots]]
id = "world"
relative_path = "world.cc"
kind = "cultcache-file"
schema = "test-service.state.v1"
writer = "process-bound-single-writer"
recovery = "preserve"
startup = "create-or-open-after-admission"

[[provides]]
capability = "test-service.runtime"
schema = "test-service.state.v1"
compatibility = "v1"

[[dependencies]]
kind = "shared-infrastructure"
capability = "odin.verse-rendezvous"
schema = "odin.verse-topology.v1"
compatibility = "v1"
startup = "before-promotion"
"#;

    const BINDING: &str = r#"
schema = "gamecult.idunn.operator_binding.v1"
target = "test-service"
profiles = ["aetheria", "full-gamecult"]

[repository]
origin = "https://github.com/GameCult/TestService.git"
admitted_ref = "refs/heads/main"
minimum_revision = "13d5136b69bcd3a52c400d66a940f94a47122e48"
selection = "ref-head"
checkout = "/srv/build/TestService"
recipe_path = "deployment/idunn/recipe.toml"

[runners.rust]
driver = "docker"
image = "rust@sha256:4c2fd73ef19c5ef9d54bee03b06b2839a392604fbfcd578ed948b71b37c1d7fb"
affordances = ["source-read", "artifact-write", "dependency-network", "build-cache"]
cache_root = "/srv/ghostlight/build-cache"
allowed_https_hosts = ["crates.io", "github.com"]
allowed_programs = ["cargo"]
network = "dependency-egress"
memory_mebibytes = 8192
cpu_quota_percent = 400

[workload]
driver = "systemd-transient"
user = "test-service"
group = "test-service"
unit_prefix = "idunn-test-service"
release_root = "/srv/test-service/releases"
state_root = "/var/lib/gamecult/test-service"
runtime_root = "/etc/gamecult/test-service/runtime"
network = "host-private"
hardening = "strict"
memory_mebibytes = 2048
cpu_quota_percent = 200

[workload.environment]
TEST_SERVICE_BIND = "idunn.private_endpoint"

[workload.argument_bindings]
state_root = "/var/lib/gamecult/test-service"

[route]
driver = "nginx-http"
route_id = "test-service-public"
stable_endpoint = "https://yggdrasil.gamecult.org/test-service/"
private_host = "127.0.0.1"
private_port_start = 18831
private_port_end = 18839
config_path = "/etc/nginx/idunn-routes/test-service.conf"
reload_unit = "nginx.service"

[admission]
driver = "atomic-file"
record_path = "/etc/gamecult/test-service/runtime/process-admission.cc"
lock_path = "/etc/gamecult/test-service/runtime/process-admission.cc.lock"

[brakes]
deployment_store = "/var/lib/gamecult/idunn-authority/test-service-deployment-brake.cc"
lifecycle_store = "/var/lib/gamecult/idunn-authority/test-service-lifecycle-brake.cc"

[state_transition]
policy = "fresh-root"
archive_root = "/var/lib/gamecult/test-service-cold-archive"

[rollout]
strategy = "candidate-then-promote"
drain_seconds = 30
retain_releases = 2

[placement]
desired_replicas = 1
nodes = ["yggdrasil"]
"#;

    #[test]
    fn strict_recipe_and_binding_admit_without_host_privilege_in_recipe() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let binding = OperatorBinding::parse(BINDING).unwrap();
        binding.admit(&recipe).unwrap();
        assert_eq!(recipe.target, "test-service");
        assert_eq!(binding.route.as_ref().unwrap().private_port_start, 18831);
    }

    #[test]
    fn unknown_recipe_field_is_rejected() {
        let input = RECIPE.replace(
            "target = \"test-service\"",
            "target = \"test-service\"\nroot = true",
        );
        assert!(TargetDeclaration::parse(&input).is_err());
    }

    #[test]
    fn operator_cannot_omit_an_affordance_derived_from_the_recipe() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let input = BINDING.replace(
            "affordances = [\"source-read\", \"artifact-write\", \"dependency-network\", \"build-cache\"]",
            "affordances = [\"source-read\", \"dependency-network\", \"build-cache\"]",
        );
        let binding = OperatorBinding::parse(&input).unwrap();
        assert!(binding.admit(&recipe).is_err());
    }

    #[test]
    fn route_transport_mismatch_is_rejected() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let binding =
            OperatorBinding::parse(&BINDING.replace("nginx-http", "nginx-stream-tcp")).unwrap();
        assert!(binding.admit(&recipe).is_err());
    }

    #[test]
    fn raw_unit_template_is_rejected() {
        let input = RECIPE.replace(
            "executable_artifact = \"daemon\"",
            "executable_artifact = \"daemon\"\nunit_template = \"deployment/root.service\"",
        );
        assert!(TargetDeclaration::parse(&input).is_err());
    }

    #[test]
    fn extra_operator_environment_is_rejected() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let input = BINDING.replace(
            "TEST_SERVICE_BIND = \"idunn.private_endpoint\"",
            "TEST_SERVICE_BIND = \"idunn.private_endpoint\"\nUNDECLARED = \"no\"",
        );
        let binding = OperatorBinding::parse(&input).unwrap();
        assert!(binding.admit(&recipe).is_err());
    }

    #[test]
    fn fresh_root_keeps_target_preservation_semantics_inside_the_new_generation() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let binding = OperatorBinding::parse(BINDING).unwrap();
        binding.admit(&recipe).unwrap();
        assert_eq!(recipe.state.slots[0].recovery, StateRecovery::Preserve);
        assert_eq!(
            binding.state_transition.policy,
            StateTransitionPolicy::FreshRoot
        );
    }

    #[test]
    fn state_slot_path_cannot_escape_the_operator_owned_root() {
        let input = RECIPE.replace(
            "relative_path = \"world.cc\"",
            "relative_path = \"../world.cc\"",
        );
        assert!(TargetDeclaration::parse(&input).is_err());
    }

    #[test]
    fn pinned_artifact_host_must_be_operator_admitted() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let binding = OperatorBinding::parse(BINDING).unwrap();
        let manifest = r#"
schema = "gamecult.idunn.pinned_tar_member.v1"
url = "https://registry.npmjs.org/tool.tgz"
archive_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
member = "package/bin/tool"
member_sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
"#;
        assert!(
            binding
                .admit_external_artifact_manifest(&recipe, "tool", manifest)
                .is_err()
        );
        let admitted = BINDING.replace(
            "allowed_https_hosts = [\"crates.io\", \"github.com\"]",
            "allowed_https_hosts = [\"crates.io\", \"github.com\", \"registry.npmjs.org\"]",
        );
        let binding = OperatorBinding::parse(&admitted).unwrap();
        binding
            .admit_external_artifact_manifest(&recipe, "tool", manifest)
            .unwrap();
    }

    #[test]
    fn dependency_compatibility_requires_exact_typed_contract() {
        assert!(capability_compatible(
            "odin.verse-rendezvous",
            "odin.verse-topology.v1",
            "v1",
            "odin.verse-rendezvous",
            "odin.verse-topology.v1",
            "v1",
        ));
        assert!(!capability_compatible(
            "odin.verse-rendezvous",
            "odin.verse-topology.v1",
            "v1",
            "odin.verse-rendezvous",
            "odin.verse-topology.v2",
            "v2",
        ));
    }
}

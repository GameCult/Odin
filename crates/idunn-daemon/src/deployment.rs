use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

pub const TARGET_DECLARATION_SCHEMA: &str = "gamecult.idunn.target_declaration.v1";
pub const OPERATOR_BINDING_SCHEMA: &str = "gamecult.idunn.operator_binding.v1";
pub const IDUNN_RUNTIME_BUNDLE_ENVIRONMENT: &str = "GAMECULT_IDUNN_RUNTIME_BUNDLE";

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
    pub external_inputs: Vec<ExternalInput>,
    pub artifacts: Vec<ArtifactOutput>,
    pub service: ServiceDeclaration,
    #[serde(default)]
    pub state: Option<StateDeclaration>,
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
    BuildCache,
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
pub struct ExternalInput {
    pub id: String,
    pub url: String,
    pub sha256: String,
    pub runner: String,
    pub destination: PathBuf,
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
    pub expected_sha256: Option<String>,
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
    pub schema_generation: String,
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
    CreateOrOpenAfterWriteLease,
    OpenAtStart,
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
    pub runtime_identity: RuntimeIdentityBinding,
    pub route: Option<RouteBinding>,
    #[serde(default)]
    pub process_write_lease: Option<ProcessWriteLeaseBinding>,
    pub brakes: BrakeBinding,
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
    pub user: String,
    #[serde(default)]
    pub affordances: BTreeSet<RunnerAffordance>,
    pub cache_root: Option<PathBuf>,
    #[serde(default)]
    pub allowed_programs: BTreeSet<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub secret_files: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub network_profile: Option<String>,
    pub memory_mebibytes: u32,
    pub cpu_quota_percent: u32,
    pub pids_limit: u32,
    pub tmpfs_mebibytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunnerDriver {
    Docker,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadBinding {
    pub driver: WorkloadDriver,
    pub user: String,
    pub group: String,
    pub unit_prefix: String,
    pub release_root: PathBuf,
    #[serde(default)]
    pub state_root: Option<PathBuf>,
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
pub struct RuntimeIdentityBinding {
    pub runtime_id: String,
    pub expected_signer_identity_id: String,
    pub trust_anchor_store: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessWriteLeaseBinding {
    pub record_path: PathBuf,
}

impl ProcessWriteLeaseBinding {
    pub fn lock_path(&self) -> PathBuf {
        let mut lock_path = self.record_path.as_os_str().to_os_string();
        lock_path.push(".lock");
        PathBuf::from(lock_path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrakeBinding {
    pub deployment_store: PathBuf,
    pub lifecycle_store: PathBuf,
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
    pub provider_id: String,
    pub capability: String,
    pub schema: String,
    pub compatibility: String,
    #[serde(default = "one")]
    pub capacity: u32,
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
            if let Some(expected_sha256) = &artifact.expected_sha256 {
                require_sha256(expected_sha256, "expected artifact sha256")?;
            }
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
        let mut external_input_ids = BTreeSet::new();
        let mut external_input_destinations = BTreeSet::new();
        for input in &self.external_inputs {
            require_id(&input.id, "external input")?;
            ensure!(
                external_input_ids.insert(input.id.as_str()),
                "external input id {} is declared twice",
                input.id
            );
            ensure!(
                !artifact_ids.contains(input.id.as_str()),
                "external input id {} collides with an artifact id",
                input.id
            );
            require_value(&input.url, "external input URL")?;
            https_host(&input.url)?;
            require_sha256(&input.sha256, "external input sha256")?;
            require_id(&input.runner, "external input runner class")?;
            runner_ids.insert(input.runner.as_str());
            require_relative_path(&input.destination, "external input destination", false)?;
            ensure!(
                external_input_destinations
                    .insert((input.runner.as_str(), input.destination.as_path())),
                "external input destination {} is declared twice for runner {}",
                input.destination.display(),
                input.runner
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
        for name in &self.service.required_environment {
            require_environment_name(name)?;
        }
        ensure!(
            self.service
                .required_environment
                .contains(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT),
            "service does not require the standard Idunn runtime bundle"
        );
        for name in &self.service.optional_environment {
            require_environment_name(name)?;
            ensure!(
                !self.service.required_environment.contains(name),
                "service environment {name} is both required and optional"
            );
        }

        if let Some(state) = &self.state {
            require_capability(&state.schema_generation, "state schema generation")?;
            ensure!(!state.slots.is_empty(), "state contract has no slots");
            let slot_ids = unique_ids(
                state.slots.iter().map(|slot| slot.id.as_str()),
                "state slot",
            )?;
            let mut slot_paths = BTreeSet::<PathBuf>::new();
            for slot in &state.slots {
                require_relative_path(&slot.relative_path, "state slot path", false)?;
                ensure!(
                    slot_paths
                        .iter()
                        .all(|other| !paths_overlap(other, &slot.relative_path)),
                    "state slot path {} overlaps another declared slot",
                    slot.relative_path.display()
                );
                ensure!(
                    slot_paths.insert(slot.relative_path.clone()),
                    "state slot path {} is declared twice",
                    slot.relative_path.display()
                );
                require_capability(&slot.schema, "state slot schema")?;
                match slot.writer {
                    StateWriter::ProcessBoundSingleWriter => ensure!(
                        slot.startup == StateStartup::CreateOrOpenAfterWriteLease,
                        "writable state slot {} must open after its write lease",
                        slot.id
                    ),
                    StateWriter::None => ensure!(
                        slot.startup == StateStartup::OpenAtStart,
                        "non-writable state slot {} cannot wait for a write lease",
                        slot.id
                    ),
                }
                if slot.recovery == StateRecovery::ExternalAuthority {
                    ensure!(
                        slot.writer == StateWriter::None,
                        "externally authoritative state slot {} cannot name the service as writer",
                        slot.id
                    );
                }
            }
            if let Some(migration) = state.migration.as_ref() {
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
                    migration.to_generation == state.schema_generation,
                    "migration destination differs from state generation"
                );
                for argument in &migration.arguments {
                    validate_launch_argument(argument)?;
                }
            }
            ensure!(slot_ids.len() == state.slots.len(), "duplicate state slot");
        }

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
            if dependency.kind == DependencyKind::Bootstrap {
                ensure!(
                    dependency.startup == StartupOrder::BeforeStart,
                    "bootstrap dependency {} must be available before start",
                    dependency.capability
                );
            }
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

    pub fn write_lease_required(&self) -> bool {
        self.state.as_ref().is_some_and(|state| {
            state
                .slots
                .iter()
                .any(|slot| slot.writer == StateWriter::ProcessBoundSingleWriter)
        })
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
            require_container_identity(&runner.user)?;
            ensure!(
                runner.memory_mebibytes > 0,
                "runner memory must be positive"
            );
            ensure!(
                (1..=100_000).contains(&runner.cpu_quota_percent),
                "runner CPU quota is outside 1..=100000"
            );
            ensure!(
                runner.pids_limit > 0 && runner.tmpfs_mebibytes > 0,
                "runner process or temporary-filesystem limit is zero"
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
            if let Some(profile) = &runner.network_profile {
                require_capability(profile, "runner network profile")?;
            }
            for name in runner.environment.keys().chain(runner.secret_files.keys()) {
                require_environment_name(name)?;
            }
            for value in runner.environment.values() {
                require_value(value, "runner environment value")?;
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
            ensure!(
                runner.affordances.contains(&RunnerAffordance::BuildCache)
                    == runner.cache_root.is_some(),
                "runner {id} cache root and build-cache affordance disagree"
            );
            ensure!(
                runner.affordances.contains(&RunnerAffordance::SecretRead)
                    == !runner.secret_files.is_empty(),
                "runner {id} secret files and secret-read affordance disagree"
            );
        }
        require_identity(&self.workload.user, "workload user")?;
        require_identity(&self.workload.group, "workload group")?;
        ensure!(
            !matches!(self.workload.user.as_str(), "root" | "0")
                && !matches!(self.workload.group.as_str(), "root" | "0"),
            "ordinary Idunn workloads cannot run as root"
        );
        require_id(&self.workload.unit_prefix, "workload unit prefix")?;
        require_id(&self.runtime_identity.runtime_id, "runtime id")?;
        require_id(
            &self.runtime_identity.expected_signer_identity_id,
            "expected signer identity id",
        )?;
        require_absolute_path(
            &self.runtime_identity.trust_anchor_store,
            "trust anchor store",
        )?;
        for (label, path) in [
            ("release root", &self.workload.release_root),
            ("runtime root", &self.workload.runtime_root),
            ("deployment brake", &self.brakes.deployment_store),
            ("lifecycle brake", &self.brakes.lifecycle_store),
        ] {
            require_absolute_path(path, label)?;
        }
        if let Some(state_root) = &self.workload.state_root {
            require_absolute_path(state_root, "state root")?;
        }
        if let Some(write_lease) = &self.process_write_lease {
            require_absolute_path(&write_lease.record_path, "process write-lease record")?;
            require_absolute_path(&write_lease.lock_path(), "process write-lease lock")?;
        }
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
        for path in &self.workload.read_write_paths {
            if let Some(state_root) = &self.workload.state_root {
                ensure!(
                    !paths_overlap(path, state_root),
                    "workload read-write path {} overlaps the write-lease-controlled state root",
                    path.display()
                );
            }
        }
        let mut authority_paths = vec![
            self.brakes.deployment_store.clone(),
            self.brakes.lifecycle_store.clone(),
        ];
        if let Some(write_lease) = &self.process_write_lease {
            authority_paths.push(write_lease.record_path.clone());
            authority_paths.push(write_lease.lock_path());
        }
        ensure!(
            authority_paths.iter().enumerate().all(|(index, path)| {
                authority_paths[index + 1..]
                    .iter()
                    .all(|other| !paths_overlap(path, other))
            }),
            "Idunn authority paths overlap"
        );
        let mut protected_paths = authority_paths;
        protected_paths.extend([
            self.workload.release_root.clone(),
            self.workload.runtime_root.clone(),
            self.runtime_identity.trust_anchor_store.clone(),
        ]);
        if let Some(route) = &self.route {
            protected_paths.push(route.config_path.clone());
        }
        if let Some(release_authority) = &self.repository.release_authority_store {
            protected_paths.push(release_authority.clone());
        }
        for authority_path in protected_paths {
            for writable_path in self
                .workload
                .read_write_paths
                .iter()
                .chain(self.workload.state_root.iter())
            {
                ensure!(
                    !paths_overlap(&authority_path, writable_path),
                    "workload writable path {} overlaps Idunn authority {}",
                    writable_path.display(),
                    authority_path.display()
                );
            }
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
        for value in self.workload.environment.values() {
            require_value(value, "workload environment value")?;
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
                route.private_port_start < route.private_port_end,
                "candidate rollout requires at least two private ports"
            );
            require_absolute_path(&route.config_path, "route config")?;
            require_unit(&route.reload_unit, "route reload unit")?;
        }
        ensure!(
            self.rollout.retain_releases >= 2,
            "candidate rollout must retain at least current and prior releases"
        );
        ensure!(
            self.placement.desired_replicas == 1,
            "operator binding v1 admits exactly one replica"
        );
        ensure!(
            self.placement.nodes.len() == 1,
            "operator binding v1 admits exactly one node"
        );
        for node in &self.placement.nodes {
            require_id(node, "placement node")?;
        }
        let mut external_contracts = BTreeSet::new();
        for capability in &self.external_capabilities {
            require_id(&capability.provider_id, "external provider id")?;
            require_capability_contract(
                &capability.capability,
                &capability.schema,
                &capability.compatibility,
            )?;
            ensure!(
                capability.capacity > 0,
                "external capability capacity must be positive"
            );
            require_value(&capability.endpoint, "external capability endpoint")?;
            ensure!(
                external_contracts.insert((
                    capability.provider_id.as_str(),
                    capability.capability.as_str(),
                    capability.schema.as_str(),
                    capability.compatibility.as_str(),
                )),
                "external provider capability is declared twice"
            );
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
                    .external_inputs
                    .iter()
                    .map(|input| input.runner.as_str()),
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
        ensure!(
            self.process_write_lease.is_some() == declaration.write_lease_required(),
            "process write-lease binding must exist exactly when writable state requires it"
        );
        ensure!(
            self.workload.state_root.is_some() == declaration.state.is_some(),
            "state root binding must exist exactly when the recipe declares state"
        );
        for step in &declaration.steps {
            let binding = &self.runners[&step.runner];
            ensure!(
                !binding
                    .environment
                    .contains_key(&declaration.source_stamp_environment)
                    && !binding
                        .secret_files
                        .contains_key(&declaration.source_stamp_environment),
                "runner cannot replace the Idunn source stamp"
            );
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
        for input in &declaration.external_inputs {
            let binding = &self.runners[&input.runner];
            ensure!(
                binding.affordances.contains(&RunnerAffordance::SourceRead),
                "external input {} runner cannot read the frozen source",
                input.id
            );
            ensure!(
                binding
                    .affordances
                    .contains(&RunnerAffordance::ArtifactWrite),
                "external input {} runner cannot materialize inputs",
                input.id
            );
            ensure!(
                binding.network_profile.is_some(),
                "external input {} runner has no operator-bound network profile",
                input.id
            );
        }
        ensure!(
            !self
                .workload
                .environment
                .contains_key(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT)
                && !self
                    .workload
                    .secret_files
                    .contains_key(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT),
            "operator binding cannot replace the standard Idunn runtime bundle"
        );
        let mut available_environment: BTreeSet<_> = self
            .workload
            .environment
            .keys()
            .chain(self.workload.secret_files.keys())
            .cloned()
            .collect();
        available_environment.insert(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT.into());
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
            self.route.as_ref(),
        ) {
            (true, ServiceTransport::Http, Some(route))
                if route.driver == RouteDriver::NginxHttp =>
            {
                ensure!(
                    route.stable_endpoint.starts_with("http://")
                        || route.stable_endpoint.starts_with("https://"),
                    "HTTP stable endpoint is not an HTTP URI"
                );
            }
            (true, ServiceTransport::Tcp, Some(route))
                if route.driver == RouteDriver::NginxStreamTcp =>
            {
                ensure!(
                    route.stable_endpoint.starts_with("tcp://"),
                    "TCP stable endpoint is not a TCP URI"
                );
            }
            (false, _, None) => {}
            _ => bail!("route binding does not match the launch contract"),
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
                    ) && binding.capacity >= dependency.minimum_capacity
                }),
                "external capability {} has no compatible operator binding",
                dependency.capability
            );
        }
        Ok(())
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
    ensure!(
        !value.chars().any(char::is_control),
        "{label} contains a control character"
    );
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

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
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

fn require_container_identity(value: &str) -> Result<()> {
    let (uid, gid) = value
        .split_once(':')
        .context("runner user must be an explicit numeric UID:GID")?;
    let uid: u32 = uid.parse().context("runner UID is not a u32")?;
    let gid: u32 = gid.parse().context("runner GID is not a u32")?;
    ensure!(uid > 0 && gid > 0, "runner identity must be unprivileged");
    Ok(())
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

[[external_inputs]]
id = "tool"
url = "https://registry.npmjs.org/tool.tgz"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
runner = "rust"
destination = "inputs/tool.tgz"

[[artifacts]]
id = "daemon"
source_kind = "runner-output"
runner = "rust"
source = "target/release/test-service"
destination = "test-service"
expected_sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
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
required_environment = ["GAMECULT_IDUNN_RUNTIME_BUNDLE", "TEST_SERVICE_BIND"]

[service.health]
contract = "test-service.cultnet-service-health"

[state]
schema_generation = "v1"

[[state.slots]]
id = "world"
relative_path = "world.cc"
kind = "cultcache-file"
schema = "test-service.state.v1"
writer = "process-bound-single-writer"
recovery = "preserve"
startup = "create-or-open-after-write-lease"

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
user = "1000:1000"
affordances = ["source-read", "artifact-write", "build-cache"]
cache_root = "/srv/ghostlight/build-cache"
allowed_programs = ["cargo"]
network_profile = "build-dependency-egress"
memory_mebibytes = 8192
cpu_quota_percent = 400
pids_limit = 512
tmpfs_mebibytes = 1024

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

[runtime_identity]
runtime_id = "test-service-yggdrasil"
expected_signer_identity_id = "test-service-runtime-signer"
trust_anchor_store = "/etc/gamecult/trust/test-service.cc"

[route]
driver = "nginx-http"
route_id = "test-service-public"
stable_endpoint = "https://yggdrasil.gamecult.org/test-service/"
private_host = "127.0.0.1"
private_port_start = 18831
private_port_end = 18839
config_path = "/etc/nginx/idunn-routes/test-service.conf"
reload_unit = "nginx.service"

[process_write_lease]
record_path = "/etc/gamecult/test-service/runtime/process-write-lease.cc"

[brakes]
deployment_store = "/var/lib/gamecult/idunn-authority/test-service-deployment-brake.cc"
lifecycle_store = "/var/lib/gamecult/idunn-authority/test-service-lifecycle-brake.cc"

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
            "affordances = [\"source-read\", \"artifact-write\", \"build-cache\"]",
            "affordances = [\"source-read\", \"build-cache\"]",
        );
        let binding = OperatorBinding::parse(&input).unwrap();
        assert!(binding.admit(&recipe).is_err());
    }

    #[test]
    fn runner_mounts_require_their_exact_operator_affordance() {
        let cache_without_affordance = BINDING.replace(
            "affordances = [\"source-read\", \"artifact-write\", \"build-cache\"]",
            "affordances = [\"source-read\", \"artifact-write\"]",
        );
        assert!(OperatorBinding::parse(&cache_without_affordance).is_err());

        let affordance_without_secret = BINDING.replace(
            "affordances = [\"source-read\", \"artifact-write\", \"build-cache\"]",
            "affordances = [\"source-read\", \"artifact-write\", \"build-cache\", \"secret-read\"]",
        );
        assert!(OperatorBinding::parse(&affordance_without_secret).is_err());
    }

    #[test]
    fn runner_cannot_replace_the_source_stamp() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let forged = BINDING.replace(
            "[workload]",
            "[runners.rust.environment]\nTEST_SERVICE_BUILD_COMMIT = \"forged\"\n\n[workload]",
        );
        let binding = OperatorBinding::parse(&forged).unwrap();
        assert!(binding.admit(&recipe).is_err());
    }

    #[test]
    fn route_transport_mismatch_is_rejected() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let binding =
            OperatorBinding::parse(&BINDING.replace("nginx-http", "nginx-stream-tcp")).unwrap();
        assert!(binding.admit(&recipe).is_err());

        let binding = OperatorBinding::parse(&BINDING.replace(
            "https://yggdrasil.gamecult.org/test-service/",
            "tcp://yggdrasil.gamecult.org:443",
        ))
        .unwrap();
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
    fn state_slot_path_cannot_escape_the_operator_owned_root() {
        let input = RECIPE.replace(
            "relative_path = \"world.cc\"",
            "relative_path = \"../world.cc\"",
        );
        assert!(TargetDeclaration::parse(&input).is_err());
    }

    #[test]
    fn state_slots_cannot_claim_overlapping_paths() {
        let input = RECIPE.replace(
            "startup = \"create-or-open-after-write-lease\"",
            "startup = \"create-or-open-after-write-lease\"\n\n[[state.slots]]\nid = \"nested\"\nrelative_path = \"world.cc/index.cc\"\nkind = \"cultcache-file\"\nschema = \"test-service.index.v1\"\nwriter = \"process-bound-single-writer\"\nrecovery = \"preserve\"\nstartup = \"create-or-open-after-write-lease\"",
        );
        assert!(TargetDeclaration::parse(&input).is_err());
    }

    #[test]
    fn workload_mount_cannot_bypass_write_lease_controlled_state() {
        let input = BINDING.replace(
            "cpu_quota_percent = 200\n\n[workload.environment]",
            "cpu_quota_percent = 200\nread_write_paths = [\"/var/lib/gamecult/test-service/world\"]\n\n[workload.environment]",
        );
        assert!(OperatorBinding::parse(&input).is_err());
    }

    #[test]
    fn v1_binding_is_explicitly_singleton() {
        assert!(
            OperatorBinding::parse(
                &BINDING.replace("desired_replicas = 1", "desired_replicas = 2")
            )
            .is_err()
        );
        assert!(
            OperatorBinding::parse(&BINDING.replace(
                "nodes = [\"yggdrasil\"]",
                "nodes = [\"yggdrasil\", \"raven\"]"
            ))
            .is_err()
        );
    }

    #[test]
    fn external_input_uses_an_operator_bound_network_profile() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let binding = OperatorBinding::parse(BINDING).unwrap();
        binding.admit(&recipe).unwrap();
        let isolated = BINDING.replace("network_profile = \"build-dependency-egress\"\n", "");
        let binding = OperatorBinding::parse(&isolated).unwrap();
        assert!(binding.admit(&recipe).is_err());
    }

    #[test]
    fn process_write_lease_exists_exactly_for_process_writable_state() {
        let recipe = TargetDeclaration::parse(RECIPE).unwrap();
        let without_lease = BINDING.replace(
            "[process_write_lease]\nrecord_path = \"/etc/gamecult/test-service/runtime/process-write-lease.cc\"\n\n",
            "",
        );
        let binding = OperatorBinding::parse(&without_lease).unwrap();
        assert!(binding.admit(&recipe).is_err());

        let read_only_recipe = TargetDeclaration::parse(
            &RECIPE
                .replace(
                    "writer = \"process-bound-single-writer\"",
                    "writer = \"none\"",
                )
                .replace(
                    "startup = \"create-or-open-after-write-lease\"",
                    "startup = \"open-at-start\"",
                ),
        )
        .unwrap();
        binding.admit(&read_only_recipe).unwrap();

        let binding = OperatorBinding::parse(BINDING).unwrap();
        assert_eq!(
            binding.process_write_lease.as_ref().unwrap().lock_path(),
            PathBuf::from("/etc/gamecult/test-service/runtime/process-write-lease.cc.lock")
        );
        assert!(binding.admit(&read_only_recipe).is_err());
    }

    #[test]
    fn stateless_target_has_no_state_binding_or_generation() {
        let mut recipe_text = RECIPE.replace(
            "arguments = [\n  { kind = \"literal\", value = \"--state-root\" },\n  { kind = \"binding\", name = \"state_root\" },\n]",
            "arguments = []",
        );
        let state_start = recipe_text.find("\n[state]\n").unwrap();
        let state_end = recipe_text.find("\n[[provides]]\n").unwrap();
        recipe_text.replace_range(state_start..state_end, "");
        let recipe = TargetDeclaration::parse(&recipe_text).unwrap();
        assert!(recipe.state.is_none());

        let binding_text = BINDING
            .replace("state_root = \"/var/lib/gamecult/test-service\"\n", "")
            .replace(
                "[workload.argument_bindings]\nstate_root = \"/var/lib/gamecult/test-service\"\n\n",
                "",
            )
            .replace(
                "[process_write_lease]\nrecord_path = \"/etc/gamecult/test-service/runtime/process-write-lease.cc\"\n\n",
                "",
            );
        let binding = OperatorBinding::parse(&binding_text).unwrap();
        binding.admit(&recipe).unwrap();
        assert!(binding.workload.state_root.is_none());
    }

    #[test]
    fn bootstrap_dependency_must_be_available_before_start() {
        let invalid = RECIPE.replace("kind = \"shared-infrastructure\"", "kind = \"bootstrap\"");
        assert!(TargetDeclaration::parse(&invalid).is_err());
        let valid = invalid.replace(
            "startup = \"before-promotion\"",
            "startup = \"before-start\"",
        );
        TargetDeclaration::parse(&valid).unwrap();
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

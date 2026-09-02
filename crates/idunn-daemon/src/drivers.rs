use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use cultcache_rs::{
    CacheBackingStore, CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry,
    SingleFileMessagePackBackingStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::deployment::{
    ArtifactOutput, ArtifactSource, IDUNN_RUNTIME_BUNDLE_ENVIRONMENT, LaunchArgument,
    OperatorBinding, RouteBinding, RouteDriver, RunnerBinding, SourceSelectionPolicy,
    TargetDeclaration, WorkloadNetwork,
};
use crate::deployment_plan::{
    ArtifactReceipt, CompiledDeploymentPlan, ExternalInputMaterializationReceipt, GitlinkTreeFact,
    SOURCE_SELECTION_FACTS_SCHEMA, SealedRelease, SourceSelection, SourceSelectionFacts,
};
use cultnet_rs::{
    IDUNN_EXPECTED_INCARNATION_SCHEMA, IDUNN_PROCESS_WRITE_LEASE_SCHEMA,
    IDUNN_RUNTIME_ACTIVATION_SCHEMA, IdunnExpectedIncarnationRecord, IdunnProcessWriteLeaseRecord,
    IdunnRuntimeActivationRecord, ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA,
    OdinRuntimeTopologyCorrelationPurpose, OdinRuntimeTopologyCorrelationRecord,
    OdinTopologyIdentity, ServiceIdentityProfile, ServiceIdentitySignature,
    ServiceIdentityTrustAnchor, verify_service_identity_signature,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub uid: u32,
    pub gid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenSource {
    pub facts: SourceSelectionFacts,
    pub recipe_bytes: Vec<u8>,
    pub root: PathBuf,
}

pub trait SourcePort {
    fn select(
        &self,
        binding: &OperatorBinding,
        selected_at_unix_millis: u64,
    ) -> Result<FrozenSource>;
}

pub trait RunnerPort {
    fn materialize(
        &self,
        source: &FrozenSource,
        plan: &CompiledDeploymentPlan,
        staging_root: &Path,
        sealed_at_unix_millis: u64,
    ) -> Result<MaterializedRelease>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaterializedRelease {
    pub release: SealedRelease,
    pub root: PathBuf,
    pub artifacts: Vec<ArtifactReceipt>,
    pub external_inputs: Vec<ExternalInputMaterializationReceipt>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadObservation {
    pub unit: String,
    pub invocation_id: String,
    pub main_pid: u32,
    pub process_start_time: u64,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub runtime_instance_id: String,
}

pub trait WorkloadPort {
    fn start(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &MaterializedRelease,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<WorkloadObservation>;

    fn observe(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        prior: &WorkloadObservation,
    ) -> Result<WorkloadObservation>;

    fn stop(&self, observation: &WorkloadObservation) -> Result<()>;
}

pub trait TopologyPort {
    type ReadyReceipt: Clone;

    fn publish_expected(&self, expected: &IdunnExpectedIncarnationRecord) -> Result<String>;
    fn publish_observed_activation(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        observation: &WorkloadObservation,
    ) -> Result<String>;
    fn present(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<Option<PresenceObservation>>;
    fn ready(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        write_lease_sha256: Option<&str>,
    ) -> Result<Option<Self::ReadyReceipt>>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceObservation {
    pub correlation_sha256: String,
    pub signed_presence_sha256: Option<String>,
    pub runtime_instance_id: Option<String>,
    pub present: bool,
    pub state: Option<String>,
    pub write_lease_sha256: Option<String>,
    pub disagreements: Vec<String>,
}

pub trait WriteLeasePort {
    fn fence(&self, expected: &IdunnExpectedIncarnationRecord) -> Result<()>;
    fn grant(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<String>;
    fn revoke(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<()>;
    fn observe(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<bool>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteObservation {
    pub route_id: String,
    pub runtime_instance_id: String,
    pub membership_sha256: String,
}

pub trait RoutePort {
    fn promote(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        runtime_instance_id: &str,
    ) -> Result<RouteObservation>;
    fn observe(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        observation: &RouteObservation,
    ) -> Result<bool>;
}

/// Fixed-argv Git source driver. It never interprets recipe text as a command
/// and never derives source policy from the target repository. The configured
/// identity performs every Git read and materialization.
pub struct GitSourceDriver {
    pub source_root: PathBuf,
    pub identity: Option<ProcessIdentity>,
    pub git_program: PathBuf,
}

impl GitSourceDriver {
    pub fn new(source_root: impl Into<PathBuf>, identity: Option<ProcessIdentity>) -> Self {
        Self {
            source_root: source_root.into(),
            identity,
            git_program: PathBuf::from("git"),
        }
    }

    fn git<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.git_program);
        command.args(args).stdin(Stdio::null());
        apply_identity(&mut command, self.identity)?;
        let output = command.output().with_context(|| {
            format!(
                "starting fixed-argv Git driver {}",
                self.git_program.display()
            )
        })?;
        if !output.status.success() {
            bail!(
                "Git source driver exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }

    fn git_text<I, S>(&self, args: I) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.git(args)?;
        let value = String::from_utf8(output.stdout).context("Git emitted non-UTF-8 text")?;
        Ok(value.trim().to_owned())
    }

    fn ensure_checkout(&self, binding: &OperatorBinding) -> Result<()> {
        let checkout = &binding.repository.checkout;
        if !checkout.exists() {
            self.git([
                OsString::from("clone"),
                OsString::from("--filter=blob:none"),
                OsString::from("--no-checkout"),
                OsString::from("--origin"),
                OsString::from("origin"),
                binding.repository.origin.clone().into(),
                checkout.as_os_str().to_owned(),
            ])?;
        }
        ensure!(
            checkout.join(".git").exists(),
            "source checkout is not a Git worktree"
        );
        let actual_origin = self.git_text([
            OsString::from("-C"),
            checkout.as_os_str().to_owned(),
            OsString::from("remote"),
            OsString::from("get-url"),
            OsString::from("origin"),
        ])?;
        ensure!(
            actual_origin == binding.repository.origin,
            "source checkout origin differs from the operator binding"
        );
        Ok(())
    }

    fn admitted_ref_name(binding: &OperatorBinding) -> String {
        format!("refs/idunn/admitted/{}", binding.target)
    }

    fn resolve_selected_revision(
        &self,
        binding: &OperatorBinding,
        fetched_ref: &str,
    ) -> Result<(String, SourceSelection)> {
        let checkout = &binding.repository.checkout;
        let (revision, selection) = match binding.repository.selection {
            SourceSelectionPolicy::PinnedObject => (
                binding
                    .repository
                    .pinned_revision
                    .clone()
                    .context("pinned source binding lost its exact revision")?,
                SourceSelection::PinnedObject,
            ),
            SourceSelectionPolicy::RefHead => (
                self.git_text([
                    OsString::from("-C"),
                    checkout.as_os_str().to_owned(),
                    OsString::from("rev-parse"),
                    OsString::from(format!("{fetched_ref}^{{commit}}")),
                ])?,
                SourceSelection::RefHead,
            ),
            SourceSelectionPolicy::SignedRelease => {
                bail!("signed-release selection requires a release-authority source port")
            }
        };
        require_git_sha(&revision, "selected source revision")?;
        self.git([
            OsString::from("-C"),
            checkout.as_os_str().to_owned(),
            OsString::from("merge-base"),
            OsString::from("--is-ancestor"),
            binding.repository.minimum_revision.clone().into(),
            revision.clone().into(),
        ])
        .context("selected source is below the operator minimum revision")?;
        self.git([
            OsString::from("-C"),
            checkout.as_os_str().to_owned(),
            OsString::from("merge-base"),
            OsString::from("--is-ancestor"),
            revision.clone().into(),
            fetched_ref.into(),
        ])
        .context("selected source is outside the fetched admitted ref")?;
        Ok((revision, selection))
    }

    fn gitlink_fact(
        &self,
        binding: &OperatorBinding,
        revision: &str,
        path: &Path,
    ) -> Result<GitlinkTreeFact> {
        let output = self.git_text([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("ls-tree"),
            revision.into(),
            OsString::from("--"),
            path.as_os_str().to_owned(),
        ])?;
        let mut fields = output.split_whitespace();
        ensure!(
            fields.next() == Some("160000"),
            "bound Gitlink is not a commit tree entry"
        );
        ensure!(
            fields.next() == Some("commit"),
            "bound Gitlink has the wrong object kind"
        );
        let tree_revision = fields
            .next()
            .context("Gitlink tree entry has no revision")?;
        require_git_sha(tree_revision, "Gitlink tree revision")?;
        let origin = binding.repository.gitlinks[path].origin.clone();
        Ok(GitlinkTreeFact {
            origin,
            revision: tree_revision.to_owned(),
            tree_entry_revision: tree_revision.to_owned(),
        })
    }

    fn materialize_worktree(
        &self,
        binding: &OperatorBinding,
        revision: &str,
        gitlinks: &BTreeMap<PathBuf, GitlinkTreeFact>,
    ) -> Result<PathBuf> {
        let target_root = self.source_root.join(&binding.target);
        let root = target_root.join(format!("{}-{}", revision, Uuid::new_v4()));
        self.git([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("worktree"),
            OsString::from("add"),
            OsString::from("--detach"),
            root.as_os_str().to_owned(),
            revision.into(),
        ])?;

        for (path, fact) in gitlinks {
            let destination = root.join(path);
            ensure!(
                destination.starts_with(&root),
                "Gitlink escaped the frozen source root"
            );
            if destination.exists() {
                fs::remove_dir(&destination).with_context(|| {
                    format!(
                        "removing empty Gitlink placeholder {}",
                        destination.display()
                    )
                })?;
            }
            if let Some(parent) = destination.parent() {
                ensure_source_directory_tree(&root, parent, self.identity)?;
            }
            self.git([
                OsString::from("clone"),
                OsString::from("--filter=blob:none"),
                OsString::from("--no-checkout"),
                OsString::from("--origin"),
                OsString::from("origin"),
                fact.origin.clone().into(),
                destination.as_os_str().to_owned(),
            ])?;
            self.git([
                OsString::from("-C"),
                destination.as_os_str().to_owned(),
                OsString::from("fetch"),
                OsString::from("--no-tags"),
                OsString::from("origin"),
                fact.revision.clone().into(),
            ])?;
            self.git([
                OsString::from("-C"),
                destination.as_os_str().to_owned(),
                OsString::from("checkout"),
                OsString::from("--detach"),
                fact.revision.clone().into(),
            ])?;
            let actual = self.git_text([
                OsString::from("-C"),
                destination.as_os_str().to_owned(),
                OsString::from("rev-parse"),
                OsString::from("HEAD"),
            ])?;
            ensure!(
                actual == fact.revision,
                "materialized Gitlink revision differs"
            );
        }
        Ok(root)
    }
}

impl SourcePort for GitSourceDriver {
    fn select(
        &self,
        binding: &OperatorBinding,
        selected_at_unix_millis: u64,
    ) -> Result<FrozenSource> {
        binding.validate()?;
        #[cfg(unix)]
        ensure!(
            unsafe { libc::geteuid() } != 0 || self.identity.is_some(),
            "root Idunn must configure an unprivileged source identity"
        );
        ensure!(
            selected_at_unix_millis > 0,
            "source selection has no timestamp"
        );
        ensure!(
            binding.repository.checkout.starts_with(&self.source_root)
                && binding.repository.checkout != self.source_root,
            "repository checkout is outside Idunn's source authority root"
        );
        ensure_source_directory(&self.source_root, self.identity)?;
        ensure_source_directory_tree(
            &self.source_root,
            binding
                .repository
                .checkout
                .parent()
                .context("repository checkout has no parent directory")?,
            self.identity,
        )?;
        ensure_source_directory(&self.source_root.join(&binding.target), self.identity)?;
        self.ensure_checkout(binding)?;
        let fetched_ref = Self::admitted_ref_name(binding);
        self.git([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("fetch"),
            OsString::from("--force"),
            OsString::from("--no-tags"),
            OsString::from("origin"),
            OsString::from(format!(
                "+{}:{fetched_ref}",
                binding.repository.admitted_ref
            )),
        ])?;
        let (revision, selection) = self.resolve_selected_revision(binding, &fetched_ref)?;
        let source_tree = self.git_text([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("rev-parse"),
            OsString::from(format!("{revision}^{{tree}}")),
        ])?;
        require_git_sha(&source_tree, "selected source tree")?;
        let recipe_spec = format!("{}:{}", revision, binding.repository.recipe_path.display());
        let recipe_bytes = self
            .git([
                OsString::from("-C"),
                binding.repository.checkout.as_os_str().to_owned(),
                OsString::from("show"),
                recipe_spec.into(),
            ])?
            .stdout;
        ensure!(
            !recipe_bytes.is_empty(),
            "selected deployment recipe is empty"
        );

        let mut gitlinks = BTreeMap::new();
        for path in binding.repository.gitlinks.keys() {
            gitlinks.insert(path.clone(), self.gitlink_fact(binding, &revision, path)?);
        }
        let root = self.materialize_worktree(binding, &revision, &gitlinks)?;
        let materialized_recipe = fs::read(root.join(&binding.repository.recipe_path))
            .context("reading materialized deployment recipe")?;
        ensure!(
            materialized_recipe == recipe_bytes,
            "materialized recipe differs from the selected Git object"
        );
        let facts = SourceSelectionFacts {
            schema: SOURCE_SELECTION_FACTS_SCHEMA.into(),
            origin: binding.repository.origin.clone(),
            admitted_ref: binding.repository.admitted_ref.clone(),
            revision,
            source_tree,
            recipe_path: binding.repository.recipe_path.clone(),
            recipe_blob_sha256: sha256_id(&recipe_bytes),
            gitlinks,
            selection,
            selected_at_unix_millis,
        };
        facts.validate_against(binding)?;
        Ok(FrozenSource {
            facts,
            recipe_bytes,
            root,
        })
    }
}

/// Docker is only a runner substrate. Recipe argv stays an argv vector, the
/// operator binding selects the exact image/network/mount affordances, and the
/// driver returns complete materialization receipts rather than build truth by
/// convention.
pub struct DockerRunnerDriver {
    pub docker_program: PathBuf,
}

impl Default for DockerRunnerDriver {
    fn default() -> Self {
        Self {
            docker_program: PathBuf::from("docker"),
        }
    }
}

impl DockerRunnerDriver {
    fn docker<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(&self.docker_program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("starting Docker runner {}", self.docker_program.display()))?;
        if !output.status.success() {
            bail!(
                "Docker runner exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }

    fn run_in_workspace(
        &self,
        runner: &RunnerBinding,
        workspace: &Path,
        working_directory: &Path,
        argv: &[String],
        source_stamp: (&str, &str),
    ) -> Result<()> {
        ensure!(!argv.is_empty(), "runner command is empty");
        ensure!(
            runner.allowed_programs.contains(&argv[0]),
            "runner program {} is not operator-bound",
            argv[0]
        );
        let mut args = self.base_run_args(runner, workspace)?;
        args.extend([
            OsString::from("--workdir"),
            OsString::from(format!(
                "/workspace/{}",
                normalized_relative(working_directory)?
            )),
            OsString::from("--env"),
            OsString::from(format!("{}={}", source_stamp.0, source_stamp.1)),
        ]);
        for (name, value) in &runner.environment {
            args.push(OsString::from("--env"));
            args.push(OsString::from(format!("{name}={value}")));
        }
        for (name, path) in &runner.secret_files {
            args.push(OsString::from("--env"));
            args.push(OsString::from(format!("{name}=/run/idunn/secrets/{name}")));
            args.push(OsString::from("--mount"));
            args.push(bind_mount(
                path,
                &format!("/run/idunn/secrets/{name}"),
                true,
            )?);
        }
        args.push(runner.image.clone().into());
        args.extend(argv.iter().map(OsString::from));
        self.docker(args)?;
        Ok(())
    }

    fn base_run_args(&self, runner: &RunnerBinding, workspace: &Path) -> Result<Vec<OsString>> {
        let mut args = vec![
            OsString::from("run"),
            OsString::from("--rm"),
            OsString::from("--network"),
            OsString::from(runner.network_profile.as_deref().unwrap_or("none")),
            OsString::from("--memory"),
            OsString::from(format!("{}m", runner.memory_mebibytes)),
            OsString::from("--cpus"),
            OsString::from(format!(
                "{:.2}",
                f64::from(runner.cpu_quota_percent) / 100.0
            )),
            OsString::from("--mount"),
            bind_mount(workspace, "/workspace", false)?,
        ];
        if let Some(cache_root) = &runner.cache_root {
            fs::create_dir_all(cache_root)
                .with_context(|| format!("creating runner cache {}", cache_root.display()))?;
            args.push(OsString::from("--mount"));
            args.push(bind_mount(cache_root, "/cache", false)?);
        }
        Ok(args)
    }

    fn materialize_external_input(
        &self,
        declaration: &TargetDeclaration,
        binding: &OperatorBinding,
        workspaces: &BTreeMap<String, PathBuf>,
        input_id: &str,
    ) -> Result<ExternalInputMaterializationReceipt> {
        let input = declaration
            .external_inputs
            .iter()
            .find(|candidate| candidate.id == input_id)
            .context("external input declaration disappeared")?;
        let runner = &binding.runners[&input.runner];
        let workspace = &workspaces[&input.runner];
        let destination = workspace.join(&input.destination);
        ensure!(
            destination.starts_with(workspace),
            "external input escaped its runner workspace"
        );
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut args = self.base_run_args(runner, workspace)?;
        args.extend([
            runner.image.clone().into(),
            OsString::from("curl"),
            OsString::from("--fail"),
            OsString::from("--location"),
            OsString::from("--proto"),
            OsString::from("=https"),
            OsString::from("--output"),
            OsString::from(format!(
                "/workspace/{}",
                normalized_relative(&input.destination)?
            )),
            input.url.clone().into(),
        ]);
        self.docker(args)?;
        let bytes = fs::read(&destination).with_context(|| {
            format!(
                "reading materialized external input {}",
                destination.display()
            )
        })?;
        ensure!(!bytes.is_empty(), "materialized external input is empty");
        let digest = raw_sha256(&bytes);
        ensure!(
            digest == input.sha256,
            "materialized external input digest differs from its recipe pin"
        );
        Ok(ExternalInputMaterializationReceipt {
            input_id: input.id.clone(),
            url: input.url.clone(),
            sha256: format!("sha256-{digest}"),
            runner: input.runner.clone(),
            destination: input.destination.clone(),
            size_bytes: bytes.len().try_into()?,
        })
    }

    fn collect_artifact(
        &self,
        declaration: &TargetDeclaration,
        source: &FrozenSource,
        workspaces: &BTreeMap<String, PathBuf>,
        staging_root: &Path,
        artifact_id: &str,
    ) -> Result<ArtifactReceipt> {
        let artifact = declaration
            .artifacts
            .iter()
            .find(|candidate| candidate.id == artifact_id)
            .context("artifact declaration disappeared")?;
        let source_path = match artifact.source_kind {
            ArtifactSource::RunnerOutput => workspaces
                .get(
                    artifact
                        .runner
                        .as_deref()
                        .context("runner artifact lost its runner")?,
                )
                .context("runner workspace is absent")?
                .join(&artifact.source),
            ArtifactSource::WorktreeTree => source.root.join(&artifact.source),
        };
        ensure!(source_path.exists(), "declared artifact output is absent");
        let destination = staging_root.join(&artifact.destination);
        ensure!(
            destination.starts_with(staging_root),
            "artifact destination escaped its release staging root"
        );
        copy_artifact(&source_path, &destination)?;
        let (sha256, size_bytes) = digest_artifact(&destination)?;
        if let Some(expected) = &artifact.expected_sha256 {
            ensure!(
                &sha256 == expected,
                "artifact output differs from its recipe-pinned digest"
            );
        }
        Ok(ArtifactReceipt {
            artifact_id: artifact.id.clone(),
            destination: artifact.destination.clone(),
            sha256: format!("sha256-{sha256}"),
            size_bytes,
            executable: artifact.executable,
        })
    }
}

impl RunnerPort for DockerRunnerDriver {
    fn materialize(
        &self,
        source: &FrozenSource,
        plan: &CompiledDeploymentPlan,
        staging_root: &Path,
        sealed_at_unix_millis: u64,
    ) -> Result<MaterializedRelease> {
        plan.validate()?;
        ensure!(
            source.facts == plan.source && source.recipe_bytes == plan.recipe_blob,
            "runner source differs from the compiled plan"
        );
        let (declaration, binding) = plan.parsed_inputs()?;
        ensure!(
            !staging_root.exists(),
            "release staging root already exists"
        );
        fs::create_dir_all(staging_root)?;

        let mut workspaces = BTreeMap::new();
        for runner_id in binding.runners.keys() {
            let workspace = staging_root.join(format!(".runner-{runner_id}"));
            copy_tree(&source.root, &workspace)?;
            workspaces.insert(runner_id.clone(), workspace);
        }

        let mut external_inputs = Vec::new();
        for input in &declaration.external_inputs {
            external_inputs.push(self.materialize_external_input(
                &declaration,
                &binding,
                &workspaces,
                &input.id,
            )?);
        }
        for step in &declaration.steps {
            let runner = &binding.runners[&step.runner];
            let workspace = &workspaces[&step.runner];
            for required in &step.required_environment {
                ensure!(
                    runner.environment.contains_key(required)
                        || runner.secret_files.contains_key(required),
                    "step {} lacks operator-bound environment {required}",
                    step.id
                );
            }
            self.run_in_workspace(
                runner,
                workspace,
                &step.working_directory,
                &step.argv,
                (
                    &declaration.source_stamp_environment,
                    &source.facts.revision,
                ),
            )?;
        }

        let mut artifacts = Vec::new();
        for artifact in &declaration.artifacts {
            artifacts.push(self.collect_artifact(
                &declaration,
                source,
                &workspaces,
                staging_root,
                &artifact.id,
            )?);
        }
        for workspace in workspaces.values() {
            remove_tree_inside(staging_root, workspace)?;
        }
        let release = SealedRelease::new(
            plan,
            artifacts.clone(),
            external_inputs.clone(),
            sealed_at_unix_millis,
        )?;
        Ok(MaterializedRelease {
            release,
            root: staging_root.to_owned(),
            artifacts,
            external_inputs,
        })
    }
}

/// systemd owns process execution. This driver lowers one validated launch
/// contract into a transient unit and then proves the native process and
/// executable that systemd actually started. It does not decide admission,
/// readiness, write authority, or route membership.
pub struct SystemdTransientWorkloadDriver {
    pub systemd_run_program: PathBuf,
    pub systemctl_program: PathBuf,
    pub proc_root: PathBuf,
}

impl Default for SystemdTransientWorkloadDriver {
    fn default() -> Self {
        Self {
            systemd_run_program: PathBuf::from("systemd-run"),
            systemctl_program: PathBuf::from("systemctl"),
            proc_root: PathBuf::from("/proc"),
        }
    }
}

impl SystemdTransientWorkloadDriver {
    fn command<I, S>(&self, program: &Path, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("starting workload actuator {}", program.display()))?;
        if !output.status.success() {
            bail!(
                "workload actuator {} exited with {}: {}",
                program.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }

    fn unit_name(&self, prefix: &str, runtime_instance_id: &str) -> Result<String> {
        let suffix = runtime_instance_id
            .strip_prefix("sha256-")
            .context("runtime instance id has no sha256 prefix")?;
        ensure!(
            suffix.len() == 64 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "runtime instance id is not a SHA-256 digest"
        );
        Ok(format!("{prefix}-{}.service", &suffix[..16]))
    }

    fn install_release(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &MaterializedRelease,
    ) -> Result<PathBuf> {
        release.release.validate_against(plan)?;
        let (_, binding) = plan.parsed_inputs()?;
        let release_root = &binding.workload.release_root;
        fs::create_dir_all(release_root)
            .with_context(|| format!("creating release root {}", release_root.display()))?;
        let installed = release_root.join(&release.release.sealed_release_id);
        if !installed.exists() {
            let temporary = release_root.join(format!(
                ".install-{}-{}",
                release.release.sealed_release_id,
                Uuid::new_v4()
            ));
            copy_tree(&release.root, &temporary)?;
            if let Err(error) = fs::rename(&temporary, &installed) {
                let cleanup = remove_tree_inside(release_root, &temporary);
                if installed.exists() && error.kind() == ErrorKind::AlreadyExists {
                    cleanup?;
                } else {
                    cleanup?;
                    return Err(error).with_context(|| {
                        format!("installing sealed release {}", installed.display())
                    });
                }
            }
        }
        for artifact in &release.artifacts {
            let path = installed.join(&artifact.destination);
            let (sha256, size_bytes) = digest_artifact(&path)?;
            ensure!(
                format!("sha256-{sha256}") == artifact.sha256 && size_bytes == artifact.size_bytes,
                "installed artifact {} differs from its sealed receipt",
                artifact.artifact_id
            );
            if artifact.executable {
                set_executable_read_only(&path)?;
            }
        }
        Ok(installed)
    }

    fn prepare_runtime_bundle(
        &self,
        binding: &OperatorBinding,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<PathBuf> {
        let bundle = binding
            .workload
            .runtime_root
            .join(&activation.runtime_instance_id);
        fs::create_dir_all(&bundle)
            .with_context(|| format!("creating runtime bundle {}", bundle.display()))?;
        write_immutable_record(
            &bundle.join("expected.cc"),
            CultCacheEnvelope {
                key: expected.target.clone(),
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: expected.canonical_bytes()?,
                stored_at: rfc3339_millis(activation.issued_at_unix_millis)?,
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            },
        )?;
        write_immutable_record(
            &bundle.join("activation.cc"),
            CultCacheEnvelope {
                key: expected.target.clone(),
                r#type: IdunnRuntimeActivationRecord::TYPE.into(),
                payload: activation.canonical_bytes()?,
                stored_at: rfc3339_millis(activation.issued_at_unix_millis)?,
                schema_id: Some(IDUNN_RUNTIME_ACTIVATION_SCHEMA.into()),
            },
        )?;
        Ok(bundle)
    }

    fn show_unit(&self, unit: &str) -> Result<Option<BTreeMap<String, String>>> {
        let output = Command::new(&self.systemctl_program)
            .args([
                OsString::from("show"),
                OsString::from(unit),
                OsString::from("--no-pager"),
                OsString::from("--property=LoadState"),
                OsString::from("--property=ActiveState"),
                OsString::from("--property=SubState"),
                OsString::from("--property=InvocationID"),
                OsString::from("--property=MainPID"),
            ])
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("observing systemd unit {unit}"))?;
        let text =
            std::str::from_utf8(&output.stdout).context("systemd show output is not UTF-8")?;
        let mut values = BTreeMap::new();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let (name, value) = line
                .split_once('=')
                .context("systemd show output is malformed")?;
            ensure!(
                values.insert(name.to_owned(), value.to_owned()).is_none(),
                "systemd show property is duplicated"
            );
        }
        if values
            .get("LoadState")
            .is_some_and(|value| value == "not-found")
        {
            return Ok(None);
        }
        ensure!(
            output.status.success(),
            "systemd show failed for {unit}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(Some(values))
    }

    fn observe_unit(
        &self,
        unit: &str,
        expected_executable: &Path,
        runtime_instance_id: &str,
    ) -> Result<WorkloadObservation> {
        let values = self
            .show_unit(unit)?
            .with_context(|| format!("systemd unit {unit} is absent"))?;
        ensure!(
            values
                .get("ActiveState")
                .is_some_and(|value| value == "active")
                && values
                    .get("SubState")
                    .is_some_and(|value| value == "running"),
            "systemd unit {unit} is not running"
        );
        let invocation_id = values
            .get("InvocationID")
            .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .context("systemd unit has no canonical invocation id")?
            .clone();
        let main_pid: u32 = values
            .get("MainPID")
            .context("systemd unit has no MainPID")?
            .parse()
            .context("systemd MainPID is not a u32")?;
        ensure!(main_pid > 0, "systemd unit has no live main process");
        let process_root = self.proc_root.join(main_pid.to_string());
        let executable = fs::read_link(process_root.join("exe"))
            .with_context(|| format!("observing executable for process {main_pid}"))?;
        ensure!(
            fs::canonicalize(&executable)? == fs::canonicalize(expected_executable)?,
            "systemd started an executable outside the sealed release"
        );
        let executable_sha256 = sha256_id(&fs::read(&executable)?);
        let process_start_time = linux_process_start_time(&process_root.join("stat"))?;
        Ok(WorkloadObservation {
            unit: unit.to_owned(),
            invocation_id,
            main_pid,
            process_start_time,
            executable,
            executable_sha256,
            runtime_instance_id: runtime_instance_id.to_owned(),
        })
    }

    fn start_transient(
        &self,
        declaration: &TargetDeclaration,
        binding: &OperatorBinding,
        installed: &Path,
        bundle: &Path,
        unit: &str,
    ) -> Result<()> {
        ensure!(
            declaration
                .service
                .required_environment
                .contains(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT),
            "target does not declare the standard Idunn runtime bundle"
        );
        ensure!(
            !binding
                .workload
                .environment
                .contains_key(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT)
                && !binding
                    .workload
                    .secret_files
                    .contains_key(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT),
            "operator binding attempts to replace the Idunn runtime bundle"
        );
        let executable_artifact =
            release_artifact(declaration, &declaration.service.executable_artifact)?;
        let executable = installed.join(&executable_artifact.destination);
        ensure!(executable.is_file(), "sealed service executable is absent");
        let mut args = vec![
            OsString::from("--no-block"),
            OsString::from(format!("--unit={unit}")),
            OsString::from("--property=Type=exec"),
            OsString::from("--property=Restart=no"),
            OsString::from("--property=KillMode=mixed"),
            OsString::from("--property=NoNewPrivileges=yes"),
            OsString::from("--property=PrivateTmp=yes"),
            OsString::from("--property=ProtectSystem=strict"),
            OsString::from("--property=ProtectHome=yes"),
            OsString::from("--property=ProtectControlGroups=yes"),
            OsString::from("--property=ProtectKernelModules=yes"),
            OsString::from("--property=ProtectKernelTunables=yes"),
            OsString::from("--property=RestrictSUIDSGID=yes"),
            OsString::from("--property=LockPersonality=yes"),
            OsString::from("--property=UMask=0027"),
            OsString::from(format!("--property=User={}", binding.workload.user)),
            OsString::from(format!("--property=Group={}", binding.workload.group)),
            OsString::from(format!(
                "--property=MemoryMax={}M",
                binding.workload.memory_mebibytes
            )),
            OsString::from(format!(
                "--property=CPUQuota={}%",
                binding.workload.cpu_quota_percent
            )),
            OsString::from(format!("--working-directory={}", installed.display())),
            OsString::from(format!("--property=ReadOnlyPaths={}", installed.display())),
            OsString::from(format!("--property=ReadOnlyPaths={}", bundle.display())),
            OsString::from(format!(
                "--setenv={IDUNN_RUNTIME_BUNDLE_ENVIRONMENT}={}",
                bundle.display()
            )),
        ];
        if binding.workload.network == WorkloadNetwork::None {
            args.push(OsString::from("--property=PrivateNetwork=yes"));
        }
        if let Some(state_root) = &binding.workload.state_root {
            args.push(OsString::from(format!(
                "--property=ReadWritePaths={}",
                state_root.display()
            )));
        }
        for path in &binding.workload.read_only_paths {
            args.push(OsString::from(format!(
                "--property=ReadOnlyPaths={}",
                path.display()
            )));
        }
        for path in &binding.workload.read_write_paths {
            args.push(OsString::from(format!(
                "--property=ReadWritePaths={}",
                path.display()
            )));
        }
        if binding.workload.devices.is_empty() {
            args.push(OsString::from("--property=PrivateDevices=yes"));
        } else {
            args.push(OsString::from("--property=DevicePolicy=closed"));
            for device in &binding.workload.devices {
                args.push(OsString::from(format!(
                    "--property=DeviceAllow={} rw",
                    device.display()
                )));
            }
        }
        if !binding.workload.capabilities.is_empty() {
            let capabilities = binding
                .workload
                .capabilities
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(" ");
            args.push(OsString::from(format!(
                "--property=CapabilityBoundingSet={capabilities}"
            )));
            args.push(OsString::from(format!(
                "--property=AmbientCapabilities={capabilities}"
            )));
        }
        for (name, value) in &binding.workload.environment {
            args.push(OsString::from(format!("--setenv={name}={value}")));
        }
        for (name, path) in &binding.workload.secret_files {
            args.push(OsString::from(format!(
                "--property=ReadOnlyPaths={}",
                path.display()
            )));
            args.push(OsString::from(format!(
                "--setenv={name}={}",
                path.display()
            )));
        }
        args.push(executable.into_os_string());
        for argument in &declaration.service.arguments {
            args.push(match argument {
                LaunchArgument::Literal { value } => value.into(),
                LaunchArgument::Binding { name } => {
                    binding.workload.argument_bindings[name].clone().into()
                }
            });
        }
        self.command(&self.systemd_run_program, args)?;
        Ok(())
    }
}

impl WorkloadPort for SystemdTransientWorkloadDriver {
    fn start(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &MaterializedRelease,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<WorkloadObservation> {
        plan.validate()?;
        release.release.validate_against(plan)?;
        expected.validate()?;
        activation.validate()?;
        ensure!(
            expected.plan_id == plan.plan_id
                && expected.sealed_release_id == release.release.sealed_release_id
                && activation.expected_projection_sha256 == expected.canonical_sha256()?,
            "workload inputs do not describe one sealed incarnation"
        );
        let (declaration, binding) = plan.parsed_inputs()?;
        let installed = self.install_release(plan, release)?;
        let executable_artifact =
            release_artifact(&declaration, &declaration.service.executable_artifact)?;
        let executable = installed.join(&executable_artifact.destination);
        let unit = self.unit_name(
            &binding.workload.unit_prefix,
            &activation.runtime_instance_id,
        )?;
        let bundle = self.prepare_runtime_bundle(&binding, expected, activation)?;
        if self.show_unit(&unit)?.is_some() {
            let observation =
                self.observe_unit(&unit, &executable, &activation.runtime_instance_id)?;
            ensure!(
                observation.executable_sha256 == expected.artifact_sha256,
                "running workload executable differs from Expected"
            );
            return Ok(observation);
        }
        self.start_transient(&declaration, &binding, &installed, &bundle, &unit)?;
        let mut last_error = None;
        for _ in 0..100 {
            match self.observe_unit(&unit, &executable, &activation.runtime_instance_id) {
                Ok(observation) => {
                    ensure!(
                        observation.executable_sha256 == expected.artifact_sha256,
                        "started workload executable differs from Expected"
                    );
                    return Ok(observation);
                }
                Err(error) => last_error = Some(error),
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("systemd did not expose the candidate")))
    }

    fn observe(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        prior: &WorkloadObservation,
    ) -> Result<WorkloadObservation> {
        expected.validate()?;
        activation.validate()?;
        ensure!(
            activation.expected_projection_sha256 == expected.canonical_sha256()?
                && activation.runtime_instance_id == prior.runtime_instance_id,
            "workload observation belongs to another activation"
        );
        let observed =
            self.observe_unit(&prior.unit, &prior.executable, &prior.runtime_instance_id)?;
        ensure!(
            observed == *prior && observed.executable_sha256 == expected.artifact_sha256,
            "native workload identity changed after observation"
        );
        Ok(observed)
    }

    fn stop(&self, observation: &WorkloadObservation) -> Result<()> {
        self.command(
            &self.systemctl_program,
            [OsString::from("stop"), OsString::from(&observation.unit)],
        )?;
        if let Some(values) = self.show_unit(&observation.unit)? {
            ensure!(
                values
                    .get("ActiveState")
                    .is_some_and(|state| state == "inactive" || state == "failed"),
                "systemd unit remained active after stop"
            );
        }
        Ok(())
    }
}

/// One dedicated CultCache file is the write-lease authority for one target.
/// Its sibling lock is shared by the target only around durable commit edges;
/// Idunn's compare-exchange therefore fences the old writer before replacing
/// the record. Route membership never enters this store.
pub struct CultCacheWriteLeaseDriver {
    pub record_path: PathBuf,
}

impl CultCacheWriteLeaseDriver {
    fn current(&self) -> Result<Option<(CultCacheEnvelope, IdunnProcessWriteLeaseRecord)>> {
        if !self.record_path.exists() {
            return Ok(None);
        }
        let entries = SingleFileMessagePackBackingStore::new(&self.record_path)
            .pull_all_read_only_snapshot()?;
        match entries.as_slice() {
            [] => Ok(None),
            [envelope]
                if envelope.r#type == IdunnProcessWriteLeaseRecord::TYPE
                    && envelope.schema_id.as_deref() == Some(IDUNN_PROCESS_WRITE_LEASE_SCHEMA) =>
            {
                let lease = IdunnProcessWriteLeaseRecord::decode_canonical(&envelope.payload)?;
                ensure!(
                    envelope.key == lease.target,
                    "write-lease key is not its target"
                );
                Ok(Some((envelope.clone(), lease)))
            }
            _ => bail!("process write-lease store is foreign or ambiguous"),
        }
    }

    fn envelope(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<CultCacheEnvelope> {
        Ok(CultCacheEnvelope {
            key: lease.target.clone(),
            r#type: IdunnProcessWriteLeaseRecord::TYPE.into(),
            payload: lease.canonical_bytes()?,
            stored_at: rfc3339_millis(lease.issued_at_unix_millis)?,
            schema_id: Some(IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into()),
        })
    }
}

impl WriteLeasePort for CultCacheWriteLeaseDriver {
    fn fence(&self, expected: &IdunnExpectedIncarnationRecord) -> Result<()> {
        expected.validate()?;
        let Some((envelope, _)) = self.current()? else {
            return Ok(());
        };
        let store = SingleFileMessagePackBackingStore::new(&self.record_path);
        ensure!(
            store.compare_exchange(
                &[CultCacheExpectedEnvelope {
                    r#type: envelope.r#type.clone(),
                    key: envelope.key.clone(),
                    current: Some(envelope),
                }],
                &[],
            )?,
            "process write lease changed while fencing the incumbent"
        );
        Ok(())
    }

    fn grant(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<String> {
        lease.validate()?;
        if let Some((_, current)) = self.current()? {
            ensure!(
                current == *lease,
                "another process already owns the write lease"
            );
            return lease.canonical_sha256();
        }
        if let Some(parent) = self.record_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let envelope = self.envelope(lease)?;
        ensure!(
            SingleFileMessagePackBackingStore::new(&self.record_path).compare_exchange(
                &[CultCacheExpectedEnvelope {
                    r#type: IdunnProcessWriteLeaseRecord::TYPE.into(),
                    key: lease.target.clone(),
                    current: None,
                }],
                &[envelope],
            )?,
            "process write-lease grant lost its empty-store compare-exchange"
        );
        lease.canonical_sha256()
    }

    fn revoke(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<()> {
        lease.validate()?;
        let Some((envelope, current)) = self.current()? else {
            return Ok(());
        };
        ensure!(
            current == *lease,
            "refusing to revoke another process write lease"
        );
        ensure!(
            SingleFileMessagePackBackingStore::new(&self.record_path).compare_exchange(
                &[CultCacheExpectedEnvelope {
                    r#type: envelope.r#type.clone(),
                    key: envelope.key.clone(),
                    current: Some(envelope),
                }],
                &[],
            )?,
            "process write lease changed while revoking it"
        );
        Ok(())
    }

    fn observe(&self, lease: &IdunnProcessWriteLeaseRecord) -> Result<bool> {
        lease.validate()?;
        Ok(self
            .current()?
            .is_some_and(|(_, current)| current == *lease))
    }
}

/// The local CultCache projection is Idunn's typed provider surface into
/// CultMesh. Odin writes a separate signed correlation store. Reading that
/// store never lets Idunn manufacture Present or Ready from its own config.
pub struct CultCacheTopologyDriver {
    pub projection_store: PathBuf,
    pub correlation_store: PathBuf,
    pub odin_trust_anchor_store: PathBuf,
}

impl CultCacheTopologyDriver {
    fn anchor(&self) -> Result<ServiceIdentityTrustAnchor> {
        let entries = SingleFileMessagePackBackingStore::new(&self.odin_trust_anchor_store)
            .pull_all_read_only_snapshot()
            .with_context(|| {
                format!(
                    "reading Odin topology trust anchor {}",
                    self.odin_trust_anchor_store.display()
                )
            })?;
        let [envelope] = entries.as_slice() else {
            bail!("Odin topology trust-anchor store is missing or ambiguous")
        };
        ensure!(
            envelope.r#type == <OdinTopologyIdentity as ServiceIdentityProfile>::TRUST_ANCHOR_TYPE
                && envelope.schema_id.as_deref()
                    == Some(<OdinTopologyIdentity as ServiceIdentityProfile>::TRUST_ANCHOR_SCHEMA)
                && envelope.key
                    == <OdinTopologyIdentity as ServiceIdentityProfile>::TRUST_ANCHOR_KEY,
            "Odin topology trust anchor belongs to another signing profile"
        );
        rmp_serde::from_slice(&envelope.payload).context("decoding Odin topology trust anchor")
    }

    fn correlation(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        current_write_lease_sha256: Option<&str>,
    ) -> Result<Option<OdinRuntimeTopologyCorrelationRecord>> {
        if !self.correlation_store.exists() {
            return Ok(None);
        }
        let entries = SingleFileMessagePackBackingStore::new(&self.correlation_store)
            .pull_all_read_only_snapshot()?;
        let mut matches = entries.iter().filter(|envelope| {
            envelope.r#type == OdinRuntimeTopologyCorrelationRecord::TYPE
                && envelope.key == expected.target
        });
        let Some(envelope) = matches.next() else {
            return Ok(None);
        };
        ensure!(
            matches.next().is_none(),
            "Odin topology correlation is ambiguous"
        );
        ensure!(
            envelope.schema_id.as_deref() == Some(ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA),
            "Odin topology correlation schema is foreign"
        );
        let (receipt, unsigned) =
            OdinRuntimeTopologyCorrelationRecord::decode_canonical_signed_payload(
                &envelope.payload,
            )?;
        ensure!(
            envelope.key == receipt.target,
            "Odin correlation key is not its target"
        );
        receipt.validate_against_expected(expected, current_write_lease_sha256)?;
        let proof = ServiceIdentitySignature {
            identity_id: receipt.signer_identity_id.clone(),
            signature: receipt.signature.clone(),
        };
        verify_service_identity_signature::<
            OdinTopologyIdentity,
            OdinRuntimeTopologyCorrelationPurpose,
        >(&self.anchor()?, &unsigned, &proof)?;
        Ok(Some(receipt))
    }
}

impl TopologyPort for CultCacheTopologyDriver {
    type ReadyReceipt = OdinRuntimeTopologyCorrelationRecord;

    fn publish_expected(&self, expected: &IdunnExpectedIncarnationRecord) -> Result<String> {
        expected.validate()?;
        upsert_record(
            &self.projection_store,
            CultCacheEnvelope {
                key: expected.target.clone(),
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: expected.canonical_bytes()?,
                stored_at: chrono::Utc::now().to_rfc3339(),
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            },
        )?;
        expected.canonical_sha256()
    }

    fn publish_observed_activation(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        observation: &WorkloadObservation,
    ) -> Result<String> {
        expected.validate()?;
        activation.validate()?;
        ensure!(
            activation.expected_projection_sha256 == expected.canonical_sha256()?
                && activation.runtime_instance_id == observation.runtime_instance_id
                && observation.executable_sha256 == expected.artifact_sha256,
            "observed activation does not name the Expected native process"
        );
        upsert_record(
            &self.projection_store,
            CultCacheEnvelope {
                key: expected.target.clone(),
                r#type: IdunnRuntimeActivationRecord::TYPE.into(),
                payload: activation.canonical_bytes()?,
                stored_at: rfc3339_millis(activation.issued_at_unix_millis)?,
                schema_id: Some(IDUNN_RUNTIME_ACTIVATION_SCHEMA.into()),
            },
        )?;
        activation.canonical_sha256()
    }

    fn present(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<Option<PresenceObservation>> {
        expected.validate()?;
        activation.validate()?;
        let Some(receipt) = self.correlation(expected, None)? else {
            return Ok(None);
        };
        let activation_sha256 = activation.canonical_sha256()?;
        let exact_activation = receipt.current_activation_sha256.as_deref()
            == Some(activation_sha256.as_str())
            && receipt.runtime_instance_id.as_deref()
                == Some(activation.runtime_instance_id.as_str());
        let mut disagreements = receipt
            .disagreements
            .iter()
            .map(|disagreement| disagreement.code.clone())
            .collect::<Vec<_>>();
        if !exact_activation {
            disagreements.push("idunn-current-activation-mismatch".into());
        }
        disagreements.sort();
        disagreements.dedup();
        Ok(Some(PresenceObservation {
            correlation_sha256: receipt.canonical_sha256()?,
            signed_presence_sha256: receipt.signed_presence_sha256.clone(),
            runtime_instance_id: receipt.runtime_instance_id.clone(),
            present: receipt.present && exact_activation,
            state: receipt.observed_presence_state.clone(),
            write_lease_sha256: receipt.observed_write_lease_sha256.clone(),
            disagreements,
        }))
    }

    fn ready(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        write_lease_sha256: Option<&str>,
    ) -> Result<Option<Self::ReadyReceipt>> {
        expected.validate()?;
        activation.validate()?;
        let Some(receipt) = self.correlation(expected, write_lease_sha256)? else {
            return Ok(None);
        };
        let activation_sha256 = activation.canonical_sha256()?;
        let exact_activation = receipt.current_activation_sha256.as_deref()
            == Some(activation_sha256.as_str())
            && receipt.runtime_instance_id.as_deref()
                == Some(activation.runtime_instance_id.as_str());
        Ok((receipt.ready && exact_activation).then_some(receipt))
    }
}

/// nginx owns proxy mechanics. Idunn supplies one exact backend membership,
/// validates the complete nginx configuration, reloads it, and then observes
/// that nginx's loaded configuration contains those exact bytes.
pub struct NginxRouteDriver {
    pub binding: RouteBinding,
    pub nginx_program: PathBuf,
    pub systemctl_program: PathBuf,
}

impl NginxRouteDriver {
    pub fn new(binding: RouteBinding) -> Self {
        Self {
            binding,
            nginx_program: PathBuf::from("nginx"),
            systemctl_program: PathBuf::from("systemctl"),
        }
    }

    fn command<I, S>(&self, program: &Path, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .with_context(|| format!("starting route actuator {}", program.display()))?;
        if !output.status.success() {
            bail!(
                "route actuator {} exited with {}: {}",
                program.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output)
    }

    fn render(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        runtime_instance_id: &str,
    ) -> Result<Vec<u8>> {
        expected.validate()?;
        let route = expected
            .route
            .as_ref()
            .context("expected incarnation has no route")?;
        ensure!(
            route.route_id == self.binding.route_id,
            "route driver binding differs from Expected"
        );
        let (candidate_host, candidate_port) = endpoint_host_port(
            &route.candidate_endpoint,
            match self.binding.driver {
                RouteDriver::NginxHttp => "http://",
                RouteDriver::NginxStreamTcp => "tcp://",
            },
        )?;
        ensure!(
            candidate_host == self.binding.private_host
                && (self.binding.private_port_start..=self.binding.private_port_end)
                    .contains(&candidate_port),
            "Expected candidate endpoint is outside the route binding"
        );
        let upstream = nginx_identifier(&self.binding.route_id)?;
        let rendered = match self.binding.driver {
            RouteDriver::NginxHttp => format!(
                "# Idunn runtime {runtime_instance_id}\nupstream {upstream} {{\n    server {candidate_host}:{candidate_port};\n    keepalive 16;\n}}\n"
            ),
            RouteDriver::NginxStreamTcp => {
                let (stable_host, stable_port) =
                    endpoint_host_port(&route.stable_endpoint, "tcp://")?;
                format!(
                    "# Idunn runtime {runtime_instance_id}\nupstream {upstream} {{\n    server {candidate_host}:{candidate_port};\n}}\nserver {{\n    listen {stable_host}:{stable_port};\n    proxy_pass {upstream};\n}}\n"
                )
            }
        };
        Ok(rendered.into_bytes())
    }

    fn loaded_contains(&self, rendered: &[u8]) -> Result<bool> {
        let output = self.command(&self.nginx_program, [OsString::from("-T")])?;
        let mut loaded = output.stdout;
        loaded.extend_from_slice(&output.stderr);
        Ok(loaded
            .windows(rendered.len())
            .any(|window| window == rendered))
    }

    fn reload(&self) -> Result<()> {
        self.command(&self.nginx_program, [OsString::from("-t")])?;
        self.command(
            &self.systemctl_program,
            [
                OsString::from("reload"),
                OsString::from(&self.binding.reload_unit),
            ],
        )?;
        Ok(())
    }

    fn restore(&self, prior: Option<&[u8]>) -> Result<()> {
        match prior {
            Some(bytes) => atomic_replace(&self.binding.config_path, bytes)?,
            None => match fs::remove_file(&self.binding.config_path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("removing unadmitted route fragment"),
            },
        }
        self.reload()
    }

    fn fail_after_rollback(
        &self,
        prior: Option<&[u8]>,
        failure: anyhow::Error,
        context: &str,
    ) -> Result<()> {
        match self.restore(prior) {
            Ok(()) => Err(failure).context(context.to_owned()),
            Err(rollback) => Err(failure).context(format!(
                "{context}; route rollback also failed: {rollback:#}"
            )),
        }
    }
}

impl RoutePort for NginxRouteDriver {
    fn promote(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        runtime_instance_id: &str,
    ) -> Result<RouteObservation> {
        let rendered = self.render(expected, runtime_instance_id)?;
        let prior = match fs::read(&self.binding.config_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("reading current route fragment"),
        };
        atomic_replace(&self.binding.config_path, &rendered)?;
        if let Err(error) = self.reload() {
            self.fail_after_rollback(
                prior.as_deref(),
                error,
                "candidate route validation or reload failed",
            )?;
        }
        match self.loaded_contains(&rendered) {
            Ok(true) => {}
            Ok(false) => self.fail_after_rollback(
                prior.as_deref(),
                anyhow::anyhow!("nginx loaded configuration omitted the admitted membership"),
                "candidate route observation failed",
            )?,
            Err(error) => self.fail_after_rollback(
                prior.as_deref(),
                error,
                "candidate route observation failed",
            )?,
        }
        Ok(RouteObservation {
            route_id: self.binding.route_id.clone(),
            runtime_instance_id: runtime_instance_id.to_owned(),
            membership_sha256: sha256_id(&rendered),
        })
    }

    fn observe(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        observation: &RouteObservation,
    ) -> Result<bool> {
        let rendered = self.render(expected, &observation.runtime_instance_id)?;
        ensure!(
            observation.route_id == self.binding.route_id
                && observation.membership_sha256 == sha256_id(&rendered),
            "route observation does not describe the expected membership"
        );
        let current = match fs::read(&self.binding.config_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error).context("reading route observation"),
        };
        Ok(current == rendered && self.loaded_contains(&rendered)?)
    }
}

fn release_artifact<'a>(
    declaration: &'a TargetDeclaration,
    artifact_id: &str,
) -> Result<&'a ArtifactOutput> {
    declaration
        .artifacts
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .with_context(|| format!("target declares no artifact {artifact_id}"))
}

fn write_immutable_record(path: &Path, envelope: CultCacheEnvelope) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut store = SingleFileMessagePackBackingStore::new(path);
    match store.pull_all_read_only_snapshot()?.as_slice() {
        [] => store.push(&envelope),
        [current] if current == &envelope => Ok(()),
        _ => bail!(
            "immutable runtime document {} already differs",
            path.display()
        ),
    }
}

fn upsert_record(path: &Path, replacement: CultCacheEnvelope) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let store = SingleFileMessagePackBackingStore::new(path);
    for _ in 0..8 {
        let entries = store.pull_all_read_only_snapshot()?;
        let mut matches = entries
            .iter()
            .filter(|entry| entry.r#type == replacement.r#type && entry.key == replacement.key);
        let current = matches.next().cloned();
        ensure!(
            matches.next().is_none(),
            "CultCache projection identity is ambiguous"
        );
        if current.as_ref() == Some(&replacement) {
            return Ok(());
        }
        if store.compare_exchange(
            &[CultCacheExpectedEnvelope {
                r#type: replacement.r#type.clone(),
                key: replacement.key.clone(),
                current,
            }],
            std::slice::from_ref(&replacement),
        )? {
            return Ok(());
        }
    }
    bail!("CultCache projection changed too often to publish")
}

fn rfc3339_millis(millis: u64) -> Result<String> {
    Ok(
        chrono::DateTime::from_timestamp_millis(i64::try_from(millis)?)
            .context("runtime timestamp is out of range")?
            .to_rfc3339(),
    )
}

#[cfg(unix)]
fn set_executable_read_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    ensure!(path.is_file(), "executable artifact is not a regular file");
    fs::set_permissions(path, fs::Permissions::from_mode(0o555))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_read_only(_path: &Path) -> Result<()> {
    bail!("systemd workload installation requires Unix permissions")
}

fn linux_process_start_time(stat_path: &Path) -> Result<u64> {
    let stat = fs::read_to_string(stat_path)
        .with_context(|| format!("reading process stat {}", stat_path.display()))?;
    let close = stat
        .rfind(')')
        .context("process stat has no command terminator")?;
    let fields = stat[close + 1..].split_whitespace().collect::<Vec<_>>();
    fields
        .get(19)
        .context("process stat has no start-time field")?
        .parse()
        .context("process start-time field is not a u64")
}

fn endpoint_host_port(endpoint: &str, scheme: &str) -> Result<(String, u16)> {
    let authority = endpoint
        .strip_prefix(scheme)
        .with_context(|| format!("endpoint does not use {scheme}"))?;
    ensure!(
        !authority
            .chars()
            .any(|character| matches!(character, '/' | '?' | '#' | '@'))
            && !authority.starts_with('['),
        "route endpoint is not a plain host and port"
    );
    let (host, port) = authority
        .rsplit_once(':')
        .context("route endpoint has no port")?;
    ensure!(
        !host.is_empty()
            && host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')),
        "route endpoint host is invalid"
    );
    let port: u16 = port.parse().context("route endpoint port is not a u16")?;
    ensure!(port > 0, "route endpoint port is zero");
    Ok((host.to_owned(), port))
}

fn nginx_identifier(route_id: &str) -> Result<String> {
    ensure!(!route_id.is_empty(), "nginx route id is empty");
    let mut identifier = String::from("idunn_");
    for byte in route_id.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' => {
                identifier.push(char::from(byte));
            }
            b'-' | b'.' => identifier.push('_'),
            _ => bail!("route id cannot lower to an nginx identifier"),
        }
    }
    Ok(identifier)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("route path has no parent")?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .context("route path has no file name")?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.idunn-{}", Uuid::new_v4()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .with_context(|| format!("creating route stage {}", temporary.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("publishing route {}", path.display()));
    }
    Ok(())
}

fn bind_mount(source: &Path, destination: &str, read_only: bool) -> Result<OsString> {
    ensure!(source.is_absolute(), "Docker bind source is not absolute");
    let text = source.to_str().context("Docker bind source is not UTF-8")?;
    ensure!(
        !text
            .chars()
            .any(|character| matches!(character, ',' | '\n' | '\r')),
        "Docker bind source contains a forbidden mount character"
    );
    let read_only = if read_only { ",readonly" } else { "" };
    Ok(OsString::from(format!(
        "type=bind,src={text},dst={destination}{read_only}"
    )))
}

fn normalized_relative(path: &Path) -> Result<String> {
    ensure!(path.is_relative(), "runner path is not relative");
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .context("runner path contains non-UTF-8")?
                    .to_owned(),
            ),
            _ => bail!("runner path escapes its workspace"),
        }
    }
    Ok(parts.join("/"))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    ensure!(source.is_dir(), "source tree is not a directory");
    ensure!(!destination.exists(), "destination tree already exists");
    fs::create_dir_all(destination)?;
    copy_tree_contents(source, destination)
}

fn copy_tree_contents(source: &Path, destination: &Path) -> Result<()> {
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.is_dir() {
            fs::create_dir(&destination_path)?;
            copy_tree_contents(&source_path, &destination_path)?;
        } else if metadata.is_file() {
            fs::copy(&source_path, &destination_path)?;
        } else if metadata.file_type().is_symlink() {
            copy_symlink(&source_path, &destination_path)?;
        } else {
            bail!("source tree contains a special filesystem entry")
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
    Ok(())
}

#[cfg(windows)]
fn copy_symlink(source: &Path, destination: &Path) -> Result<()> {
    let target = fs::read_link(source)?;
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)?;
    } else {
        std::os::windows::fs::symlink_file(target, destination)?;
    }
    Ok(())
}

fn copy_artifact(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        copy_tree(source, destination)
    } else if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, destination)?;
        Ok(())
    } else {
        bail!("artifact output is not a regular file or directory")
    }
}

fn digest_artifact(path: &Path) -> Result<(String, u64)> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() {
        let bytes = fs::read(path)?;
        return Ok((raw_sha256(&bytes), bytes.len().try_into()?));
    }
    ensure!(metadata.is_dir(), "artifact is not a file or directory");
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    digest_tree(path, path, &mut hasher, &mut size)?;
    ensure!(size > 0, "artifact directory has no file content");
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn digest_tree(root: &Path, current: &Path, hasher: &mut Sha256, size: &mut u64) -> Result<()> {
    let mut entries = fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let relative = normalized_relative(relative)?;
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            hasher.update(b"dir\0");
            hasher.update(relative.as_bytes());
            hasher.update(b"\0");
            digest_tree(root, &path, hasher, size)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&path)?;
            hasher.update(b"file\0");
            hasher.update(relative.as_bytes());
            hasher.update(b"\0");
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
            *size = size.saturating_add(bytes.len().try_into()?);
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            hasher.update(b"link\0");
            hasher.update(relative.as_bytes());
            hasher.update(b"\0");
            hasher.update(target.as_os_str().as_encoded_bytes());
            *size = size.saturating_add(target.as_os_str().len().try_into()?);
        } else {
            bail!("artifact tree contains a special filesystem entry")
        }
    }
    Ok(())
}

fn raw_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn remove_tree_inside(root: &Path, target: &Path) -> Result<()> {
    let root = root.canonicalize()?;
    let target = target.canonicalize()?;
    ensure!(
        target.starts_with(&root) && target != root,
        "refusing broad tree removal"
    );
    fs::remove_dir_all(&target)
        .with_context(|| format!("removing disposable runner workspace {}", target.display()))
}

fn sha256_id(bytes: &[u8]) -> String {
    format!("sha256-{:x}", Sha256::digest(bytes))
}

fn require_git_sha(value: &str, label: &str) -> Result<()> {
    ensure!(
        value.len() == 40
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} is not a lowercase SHA-1 object id"
    );
    Ok(())
}

fn ensure_source_directory_tree(
    root: &Path,
    directory: &Path,
    identity: Option<ProcessIdentity>,
) -> Result<()> {
    ensure!(
        directory.starts_with(root),
        "source directory escaped Idunn's source authority root"
    );
    ensure_source_directory(root, identity)?;
    let relative = directory.strip_prefix(root)?;
    let mut current = root.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            bail!("source directory is not normalized")
        };
        current.push(component);
        ensure_source_directory(&current, identity)?;
    }
    Ok(())
}

fn ensure_source_directory(path: &Path, identity: Option<ProcessIdentity>) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "source authority path {} is not a native directory",
                path.display()
            );
            validate_source_directory_owner(path, &metadata, identity)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let parent = path
                .parent()
                .context("source authority directory has no parent")?;
            ensure!(
                parent.is_dir(),
                "source authority parent {} is absent",
                parent.display()
            );
            fs::create_dir(path).with_context(|| {
                format!("creating source authority directory {}", path.display())
            })?;
            assign_source_directory_owner(path, identity)?;
            let metadata = fs::symlink_metadata(path)?;
            validate_source_directory_owner(path, &metadata, identity)
        }
        Err(error) => Err(error)
            .with_context(|| format!("inspecting source authority path {}", path.display())),
    }
}

#[cfg(unix)]
fn assign_source_directory_owner(path: &Path, identity: Option<ProcessIdentity>) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    if let Some(identity) = identity {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .context("source authority path contains a NUL byte")?;
        let result = unsafe { libc::chown(path.as_ptr(), identity.uid, identity.gid) };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .context("assigning source authority directory owner");
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o750))?;
    Ok(())
}

#[cfg(not(unix))]
fn assign_source_directory_owner(_path: &Path, identity: Option<ProcessIdentity>) -> Result<()> {
    ensure!(
        identity.is_none(),
        "configured source identities require a Unix actuator"
    );
    Ok(())
}

#[cfg(unix)]
fn validate_source_directory_owner(
    path: &Path,
    metadata: &fs::Metadata,
    identity: Option<ProcessIdentity>,
) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let expected = identity.unwrap_or(ProcessIdentity {
        uid: unsafe { libc::geteuid() },
        gid: unsafe { libc::getegid() },
    });
    ensure!(
        metadata.uid() == expected.uid && metadata.gid() == expected.gid,
        "source authority directory {} has the wrong owner",
        path.display()
    );
    ensure!(
        metadata.permissions().mode() & 0o200 != 0,
        "source authority directory {} is not owner-writable",
        path.display()
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_source_directory_owner(
    _path: &Path,
    _metadata: &fs::Metadata,
    identity: Option<ProcessIdentity>,
) -> Result<()> {
    ensure!(
        identity.is_none(),
        "configured source identities require a Unix actuator"
    );
    Ok(())
}

fn apply_identity(command: &mut Command, identity: Option<ProcessIdentity>) -> Result<()> {
    let Some(identity) = identity else {
        return Ok(());
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.uid(identity.uid).gid(identity.gid);
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        let _ = identity;
        bail!("configured process identities require a Unix actuator")
    }
}

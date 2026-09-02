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
struct GitTreeEntry {
    mode: String,
    kind: String,
    object: String,
    path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSource {
    pub facts: SourceSelectionFacts,
    pub recipe_bytes: Vec<u8>,
}

impl ResolvedSource {
    pub fn validate_against(&self, binding: &OperatorBinding) -> Result<()> {
        self.facts.validate_against(binding)?;
        ensure!(!self.recipe_bytes.is_empty(), "resolved recipe is empty");
        ensure!(
            sha256_id(&self.recipe_bytes) == self.facts.recipe_blob_sha256,
            "resolved recipe bytes differ from the selected Git object"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenSource {
    receipt: FrozenSourceReceipt,
    facts: SourceSelectionFacts,
    recipe_bytes: Vec<u8>,
    root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenSourceReceipt {
    pub transaction_id: String,
    pub plan_id: String,
    pub snapshot_sha256: String,
}

impl FrozenSourceReceipt {
    pub fn validate_against(&self, plan: &CompiledDeploymentPlan) -> Result<()> {
        plan.validate()?;
        require_driver_id(&self.transaction_id, "source transaction")?;
        ensure!(self.plan_id == plan.plan_id, "frozen source plan differs");
        require_sha256_id(&self.snapshot_sha256, "frozen source snapshot")
    }

    fn snapshot_component(&self) -> &str {
        self.snapshot_sha256
            .strip_prefix("sha256-")
            .expect("validated frozen source digest")
    }
}

pub trait SourcePort {
    fn resolve(
        &self,
        binding: &OperatorBinding,
        resolution_id: &str,
        selected_at_unix_millis: u64,
    ) -> Result<ResolvedSource>;

    fn freeze(
        &self,
        transaction_id: &str,
        plan: &CompiledDeploymentPlan,
    ) -> Result<FrozenSourceReceipt>;

    fn observe_frozen(
        &self,
        plan: &CompiledDeploymentPlan,
        receipt: &FrozenSourceReceipt,
    ) -> Result<FrozenSource>;

    fn cleanup(&self, transaction_id: &str, receipt: Option<&FrozenSourceReceipt>) -> Result<()>;
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledReleaseObservation {
    pub sealed_release_id: String,
    pub root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadObservation {
    pub unit: String,
    pub unit_description: String,
    pub invocation_id: String,
    pub main_pid: u32,
    pub process_start_time: u64,
    pub executable: PathBuf,
    pub executable_sha256: String,
    pub runtime_instance_id: String,
    pub user: String,
    pub group: String,
    pub working_directory: PathBuf,
    pub runtime_bundle: PathBuf,
    pub command_line_sha256: String,
    pub environment_names: Vec<String>,
    pub environment_contract_sha256: String,
    pub control_group: String,
}

pub trait WorkloadPort {
    fn install(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &MaterializedRelease,
    ) -> Result<InstalledReleaseObservation>;

    fn start(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &SealedRelease,
        installed: &InstalledReleaseObservation,
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
    fn revoke_exact(&self, incumbent: Option<&IdunnProcessWriteLeaseRecord>) -> Result<()>;
    fn observe_empty(&self) -> Result<bool>;
    fn grant(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        warming: &PresenceObservation,
        lease: &IdunnProcessWriteLeaseRecord,
    ) -> Result<String>;
    fn observe(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        warming: &PresenceObservation,
        lease: &IdunnProcessWriteLeaseRecord,
    ) -> Result<bool>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteObservation {
    pub route_id: String,
    pub runtime_instance_id: String,
    pub membership_sha256: String,
}

pub trait RoutePort {
    fn preflight(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        runtime_instance_id: &str,
    ) -> Result<()>;
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
/// identity performs every Git/network read; root Idunn extracts only exact Git
/// object archives into a separate transaction-owned immutable store.
pub struct GitSourceDriver {
    pub source_cache_root: PathBuf,
    pub frozen_source_root: PathBuf,
    pub identity: Option<ProcessIdentity>,
    pub git_program: PathBuf,
    pub tar_program: PathBuf,
}

impl GitSourceDriver {
    pub fn new(
        source_cache_root: impl Into<PathBuf>,
        frozen_source_root: impl Into<PathBuf>,
        identity: Option<ProcessIdentity>,
    ) -> Self {
        Self {
            source_cache_root: source_cache_root.into(),
            frozen_source_root: frozen_source_root.into(),
            identity,
            git_program: PathBuf::from("/usr/bin/git"),
            tar_program: PathBuf::from("/usr/bin/tar"),
        }
    }

    fn git_command<I, S>(&self, args: I) -> Result<Command>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        ensure!(
            self.git_program.is_absolute(),
            "Git source driver program is not absolute"
        );
        let mut command = Command::new(&self.git_program);
        command
            .args(args)
            .stdin(Stdio::null())
            .env_clear()
            .env("HOME", self.source_cache_root.join(".home"))
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LANG", "C.UTF-8");
        apply_identity(&mut command, self.identity)?;
        Ok(command)
    }

    fn git<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.git_command(args)?.output().with_context(|| {
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
        let checkout_metadata = fs::symlink_metadata(checkout)
            .with_context(|| format!("inspecting source checkout {}", checkout.display()))?;
        ensure!(
            checkout_metadata.is_dir() && !checkout_metadata.file_type().is_symlink(),
            "source checkout is not a native directory"
        );
        let canonical_checkout = checkout.canonicalize()?;
        ensure!(
            canonical_checkout == *checkout,
            "source checkout traverses a symlink or noncanonical path"
        );
        let git_metadata = fs::symlink_metadata(checkout.join(".git"))?;
        ensure!(
            git_metadata.is_dir() && !git_metadata.file_type().is_symlink(),
            "source checkout has no native Git object directory"
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

    fn admitted_ref_name(binding: &OperatorBinding, resolution_id: &str) -> Result<String> {
        require_driver_id(resolution_id, "source resolution")?;
        Ok(format!(
            "refs/idunn/resolutions/{}/{}",
            binding.target, resolution_id
        ))
    }

    fn resolve_selected_revision(
        &self,
        binding: &OperatorBinding,
        admitted_ref_revision: &str,
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
            SourceSelectionPolicy::RefHead => {
                (admitted_ref_revision.to_owned(), SourceSelection::RefHead)
            }
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
            admitted_ref_revision.into(),
        ])
        .context("selected source is outside the fetched admitted ref")?;
        Ok((revision, selection))
    }

    fn git_tree_entries(&self, repository: &Path, revision: &str) -> Result<Vec<GitTreeEntry>> {
        let output = self
            .git([
                OsString::from("-C"),
                repository.as_os_str().to_owned(),
                OsString::from("ls-tree"),
                OsString::from("-r"),
                OsString::from("-z"),
                revision.into(),
            ])?
            .stdout;
        let mut entries = Vec::new();
        for record in output
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
        {
            let tab = record
                .iter()
                .position(|byte| *byte == b'\t')
                .context("Git tree record has no path delimiter")?;
            let header = std::str::from_utf8(&record[..tab])
                .context("Git tree record header is not UTF-8")?;
            let path =
                std::str::from_utf8(&record[tab + 1..]).context("Git tree path is not UTF-8")?;
            let mut fields = header.split(' ');
            let mode = fields.next().context("Git tree record has no mode")?;
            let kind = fields.next().context("Git tree record has no kind")?;
            let object = fields.next().context("Git tree record has no object")?;
            ensure!(fields.next().is_none(), "Git tree record has extra fields");
            require_git_sha(object, "Git tree object")?;
            let path = PathBuf::from(path);
            let normalized = normalized_relative(&path)?;
            ensure!(
                normalized == path.to_string_lossy(),
                "Git tree path is not normalized"
            );
            ensure!(
                path.components().all(|component| !matches!(
                    component,
                    std::path::Component::Normal(value) if value == ".git"
                )),
                "Git tree contains forbidden .git metadata"
            );
            entries.push(GitTreeEntry {
                mode: mode.to_owned(),
                kind: kind.to_owned(),
                object: object.to_owned(),
                path,
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        ensure!(
            entries.windows(2).all(|pair| pair[0].path != pair[1].path),
            "Git tree emits a path twice"
        );
        Ok(entries)
    }

    fn exact_recipe_and_gitlinks(
        &self,
        binding: &OperatorBinding,
        revision: &str,
    ) -> Result<(Vec<u8>, BTreeMap<PathBuf, GitlinkTreeFact>)> {
        let entries = self.git_tree_entries(&binding.repository.checkout, revision)?;
        let recipe = entries
            .iter()
            .find(|entry| entry.path == binding.repository.recipe_path)
            .context("selected tree has no deployment recipe")?;
        ensure!(
            matches!(recipe.mode.as_str(), "100644" | "100755") && recipe.kind == "blob",
            "deployment recipe is not a regular Git blob"
        );
        let recipe_bytes = self
            .git([
                OsString::from("-C"),
                binding.repository.checkout.as_os_str().to_owned(),
                OsString::from("cat-file"),
                OsString::from("blob"),
                recipe.object.clone().into(),
            ])?
            .stdout;
        ensure!(
            !recipe_bytes.is_empty(),
            "selected deployment recipe is empty"
        );

        let observed_gitlinks = entries
            .iter()
            .filter(|entry| entry.mode == "160000")
            .map(|entry| {
                ensure!(entry.kind == "commit", "Gitlink has the wrong object kind");
                Ok((entry.path.clone(), entry.object.clone()))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let expected_paths = binding
            .repository
            .gitlinks
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let observed_paths = observed_gitlinks
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        ensure!(
            observed_paths == expected_paths,
            "selected tree Gitlinks do not exactly match operator bindings"
        );
        let gitlinks = observed_gitlinks
            .into_iter()
            .map(|(path, revision)| {
                let origin = binding.repository.gitlinks[&path].origin.clone();
                (path, GitlinkTreeFact { origin, revision })
            })
            .collect();
        Ok((recipe_bytes, gitlinks))
    }

    fn prepare_source_root(&self, binding: &OperatorBinding) -> Result<()> {
        #[cfg(unix)]
        ensure!(
            unsafe { libc::geteuid() } != 0 || self.identity.is_some(),
            "root Idunn must configure an unprivileged source identity"
        );
        ensure!(
            binding
                .repository
                .checkout
                .starts_with(&self.source_cache_root)
                && binding.repository.checkout != self.source_cache_root,
            "repository checkout is outside Idunn's source authority root"
        );
        ensure_source_directory(&self.source_cache_root, self.identity)?;
        ensure!(
            self.source_cache_root.canonicalize()? == self.source_cache_root,
            "source cache root traverses a symlink or noncanonical path"
        );
        ensure_source_directory(&self.source_cache_root.join(".home"), self.identity)?;
        ensure_source_directory_tree(
            &self.source_cache_root,
            binding
                .repository
                .checkout
                .parent()
                .context("repository checkout has no parent directory")?,
            self.identity,
        )?;
        self.ensure_checkout(binding)
    }

    fn verify_exact_source(
        &self,
        binding: &OperatorBinding,
        resolved: &ResolvedSource,
    ) -> Result<()> {
        let actual_tree = self.git_text([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("rev-parse"),
            OsString::from(format!("{}^{{tree}}", resolved.facts.revision)),
        ])?;
        ensure!(
            actual_tree == resolved.facts.source_tree,
            "selected revision no longer resolves to the frozen source tree"
        );
        let (recipe_bytes, gitlinks) =
            self.exact_recipe_and_gitlinks(binding, &resolved.facts.revision)?;
        ensure!(
            recipe_bytes == resolved.recipe_bytes,
            "selected recipe object differs from the durable source resolution"
        );
        ensure!(
            gitlinks == resolved.facts.gitlinks,
            "Gitlinks differ from the durable source resolution"
        );
        Ok(())
    }

    fn git_archive_into(
        &self,
        repository: &Path,
        revision: &str,
        prefix: Option<&Path>,
        destination: &Path,
    ) -> Result<()> {
        ensure!(
            self.tar_program.is_absolute(),
            "source archive extractor is not absolute"
        );
        let mut archive_args = vec![
            OsString::from("-C"),
            repository.as_os_str().to_owned(),
            OsString::from("archive"),
            OsString::from("--format=tar"),
        ];
        if let Some(prefix) = prefix {
            archive_args.push(OsString::from(format!(
                "--prefix={}/",
                normalized_relative(prefix)?
            )));
        }
        archive_args.push(revision.into());
        let mut archive = self.git_command(archive_args)?;
        archive.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut archive = archive.spawn().context("starting exact Git archive")?;
        let archive_stdout = archive.stdout.take().context("Git archive has no stdout")?;
        let mut extractor = Command::new(&self.tar_program);
        extractor
            .args([
                OsString::from("--extract"),
                OsString::from("--file=-"),
                OsString::from("--directory"),
                destination.as_os_str().to_owned(),
                OsString::from("--no-same-owner"),
            ])
            .stdin(Stdio::from(archive_stdout))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env_clear()
            .env("LANG", "C.UTF-8");
        let extractor = extractor
            .spawn()
            .context("starting fixed-argv source archive extractor")?;
        let archive_output = archive
            .wait_with_output()
            .context("waiting for exact Git archive")?;
        let extractor_output = extractor
            .wait_with_output()
            .context("waiting for source archive extractor")?;
        ensure!(
            archive_output.status.success(),
            "Git archive failed: {}",
            String::from_utf8_lossy(&archive_output.stderr).trim()
        );
        ensure!(
            extractor_output.status.success(),
            "source archive extraction failed: {}",
            String::from_utf8_lossy(&extractor_output.stderr).trim()
        );
        Ok(())
    }

    fn materialize_gitlink_archive(
        &self,
        binding: &OperatorBinding,
        path: &Path,
        fact: &GitlinkTreeFact,
        destination: &Path,
    ) -> Result<()> {
        let gitlink_root = self
            .source_cache_root
            .join(".gitlinks")
            .join(&binding.target);
        ensure_source_directory_tree(&self.source_cache_root, &gitlink_root, self.identity)?;
        let checkout = gitlink_root.join(format!("{}-{}", fact.revision, Uuid::new_v4()));
        let result = (|| {
            self.git([
                OsString::from("clone"),
                OsString::from("--filter=blob:none"),
                OsString::from("--no-checkout"),
                OsString::from("--origin"),
                OsString::from("origin"),
                fact.origin.clone().into(),
                checkout.as_os_str().to_owned(),
            ])?;
            self.git([
                OsString::from("-C"),
                checkout.as_os_str().to_owned(),
                OsString::from("fetch"),
                OsString::from("--no-tags"),
                OsString::from("origin"),
                fact.revision.clone().into(),
            ])?;
            let actual = self.git_text([
                OsString::from("-C"),
                checkout.as_os_str().to_owned(),
                OsString::from("rev-parse"),
                OsString::from(format!("{}^{{commit}}", fact.revision)),
            ])?;
            ensure!(actual == fact.revision, "Gitlink exact revision is absent");
            ensure!(
                !self
                    .git_tree_entries(&checkout, &fact.revision)?
                    .iter()
                    .any(|entry| entry.mode == "160000"),
                "nested Gitlinks are not admitted in Idunn v1"
            );
            self.git_archive_into(&checkout, &fact.revision, Some(path), destination)
        })();
        let cleanup = if checkout.exists() {
            remove_tree_inside(&gitlink_root, &checkout)
        } else {
            Ok(())
        };
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.context("cleaning exact Gitlink checkout")),
        }
    }
}

impl SourcePort for GitSourceDriver {
    fn resolve(
        &self,
        binding: &OperatorBinding,
        resolution_id: &str,
        selected_at_unix_millis: u64,
    ) -> Result<ResolvedSource> {
        binding.validate()?;
        ensure!(
            selected_at_unix_millis > 0,
            "source selection has no timestamp"
        );
        self.prepare_source_root(binding)?;
        let fetched_ref = Self::admitted_ref_name(binding, resolution_id)?;
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
        let admitted_ref_revision = self.git_text([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("rev-parse"),
            OsString::from(format!("{fetched_ref}^{{commit}}")),
        ])?;
        require_git_sha(&admitted_ref_revision, "fetched admitted-ref revision")?;
        let (revision, selection) =
            self.resolve_selected_revision(binding, &admitted_ref_revision)?;
        let source_tree = self.git_text([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("rev-parse"),
            OsString::from(format!("{revision}^{{tree}}")),
        ])?;
        require_git_sha(&source_tree, "selected source tree")?;
        let (recipe_bytes, gitlinks) = self.exact_recipe_and_gitlinks(binding, &revision)?;
        let facts = SourceSelectionFacts {
            schema: SOURCE_SELECTION_FACTS_SCHEMA.into(),
            origin: binding.repository.origin.clone(),
            admitted_ref: binding.repository.admitted_ref.clone(),
            admitted_ref_revision,
            revision,
            source_tree,
            recipe_path: binding.repository.recipe_path.clone(),
            recipe_blob_sha256: sha256_id(&recipe_bytes),
            gitlinks,
            selection,
            selected_at_unix_millis,
        };
        facts.validate_against(binding)?;
        let resolved = ResolvedSource {
            facts,
            recipe_bytes,
        };
        resolved.validate_against(binding)?;
        Ok(resolved)
    }

    fn freeze(
        &self,
        transaction_id: &str,
        plan: &CompiledDeploymentPlan,
    ) -> Result<FrozenSourceReceipt> {
        plan.validate()?;
        require_driver_id(transaction_id, "source transaction")?;
        let (_, binding) = plan.parsed_inputs()?;
        let resolved = ResolvedSource {
            facts: plan.source.clone(),
            recipe_bytes: plan.recipe_blob.clone(),
        };
        resolved.validate_against(&binding)?;
        #[cfg(unix)]
        ensure!(
            unsafe { libc::geteuid() } == 0 && self.identity.is_some(),
            "freezing source requires root Idunn with an unprivileged Git identity"
        );
        self.prepare_source_root(&binding)?;
        self.git([
            OsString::from("-C"),
            binding.repository.checkout.as_os_str().to_owned(),
            OsString::from("fetch"),
            OsString::from("--no-tags"),
            OsString::from("origin"),
            resolved.facts.revision.clone().into(),
        ])
        .context("fetching the durable exact source revision")?;
        self.verify_exact_source(&binding, &resolved)?;
        let transaction_root =
            prepare_frozen_transaction_root(&self.frozen_source_root, transaction_id)?;
        let partial = transaction_root.join(".partial");
        prepare_frozen_source_destination(&partial)?;
        let materialization = (|| {
            self.git_archive_into(
                &binding.repository.checkout,
                &resolved.facts.revision,
                None,
                &partial,
            )?;
            for (path, fact) in &resolved.facts.gitlinks {
                self.materialize_gitlink_archive(&binding, path, fact, &partial)?;
            }
            let recipe_path = partial.join(&resolved.facts.recipe_path);
            let recipe_metadata = fs::symlink_metadata(&recipe_path)?;
            ensure!(
                recipe_metadata.is_file() && !recipe_metadata.file_type().is_symlink(),
                "frozen deployment recipe is not a regular file"
            );
            let materialized_recipe =
                fs::read(&recipe_path).context("reading frozen deployment recipe")?;
            ensure!(
                materialized_recipe == resolved.recipe_bytes,
                "frozen recipe differs from the durable source resolution"
            );
            harden_frozen_source(&partial)?;
            validate_frozen_source(&partial)?;
            frozen_source_sha256(&partial)
        })();
        let snapshot_sha256 = match materialization {
            Ok(snapshot_sha256) => snapshot_sha256,
            Err(error) => {
                let _ = remove_frozen_transaction_root(&self.frozen_source_root, transaction_id);
                return Err(error);
            }
        };
        let snapshot_component = snapshot_sha256
            .strip_prefix("sha256-")
            .expect("generated frozen source digest");
        let final_root = transaction_root.join(snapshot_component);
        fs::rename(&partial, &final_root).context("publishing immutable frozen source")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&transaction_root, fs::Permissions::from_mode(0o500))?;
        }
        let receipt = FrozenSourceReceipt {
            transaction_id: transaction_id.to_owned(),
            plan_id: plan.plan_id.clone(),
            snapshot_sha256,
        };
        receipt.validate_against(plan)?;
        Ok(receipt)
    }

    fn observe_frozen(
        &self,
        plan: &CompiledDeploymentPlan,
        receipt: &FrozenSourceReceipt,
    ) -> Result<FrozenSource> {
        receipt.validate_against(plan)?;
        validate_frozen_source_store(&self.frozen_source_root)?;
        let root = self
            .frozen_source_root
            .join(&receipt.transaction_id)
            .join(receipt.snapshot_component());
        let canonical_store = self.frozen_source_root.canonicalize()?;
        let canonical_root = root.canonicalize()?;
        ensure!(
            canonical_store == self.frozen_source_root
                && canonical_root == root
                && canonical_root.starts_with(&canonical_store),
            "frozen source receipt resolves outside its authority root"
        );
        validate_frozen_source(&root)?;
        ensure!(
            frozen_source_sha256(&root)? == receipt.snapshot_sha256,
            "frozen source snapshot differs from its receipt"
        );
        let recipe_path = root.join(&plan.source.recipe_path);
        let recipe_metadata = fs::symlink_metadata(&recipe_path)?;
        ensure!(
            recipe_metadata.is_file() && !recipe_metadata.file_type().is_symlink(),
            "observed deployment recipe is not a regular file"
        );
        ensure!(
            fs::read(&recipe_path)? == plan.recipe_blob,
            "observed deployment recipe differs from the persisted plan"
        );
        Ok(FrozenSource {
            receipt: receipt.clone(),
            facts: plan.source.clone(),
            recipe_bytes: plan.recipe_blob.clone(),
            root,
        })
    }

    fn cleanup(&self, transaction_id: &str, receipt: Option<&FrozenSourceReceipt>) -> Result<()> {
        require_driver_id(transaction_id, "source transaction")?;
        if let Some(receipt) = receipt {
            ensure!(
                receipt.transaction_id == transaction_id,
                "frozen source cleanup receipt belongs to another transaction"
            );
            require_sha256_id(&receipt.snapshot_sha256, "frozen source snapshot")?;
        }
        let transaction_root = self.frozen_source_root.join(transaction_id);
        if !transaction_root.exists() {
            return Ok(());
        }
        remove_frozen_transaction_root(&self.frozen_source_root, transaction_id)
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
            docker_program: PathBuf::from("/usr/bin/docker"),
        }
    }
}

impl DockerRunnerDriver {
    fn docker<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        ensure!(
            self.docker_program.is_absolute(),
            "Docker runner program is not absolute"
        );
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
        required_environment: &std::collections::BTreeSet<String>,
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
        for name in required_environment {
            if let Some(value) = runner.environment.get(name) {
                args.push(OsString::from("--env"));
                args.push(OsString::from(format!("{name}={value}")));
            } else if let Some(path) = runner.secret_files.get(name) {
                validate_runner_secret(path, container_identity(&runner.user)?)?;
                args.push(OsString::from("--env"));
                args.push(OsString::from(format!("{name}=/run/idunn/secrets/{name}")));
                args.push(OsString::from("--mount"));
                args.push(bind_mount(
                    path,
                    &format!("/run/idunn/secrets/{name}"),
                    true,
                )?);
            } else {
                bail!("runner lacks declared step environment {name}")
            }
        }
        args.push(runner.image.clone().into());
        args.extend(argv.iter().map(OsString::from));
        self.docker(args)?;
        Ok(())
    }

    fn base_run_args(&self, runner: &RunnerBinding, workspace: &Path) -> Result<Vec<OsString>> {
        let identity = container_identity(&runner.user)?;
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
            OsString::from("--user"),
            OsString::from(&runner.user),
            OsString::from("--cap-drop"),
            OsString::from("ALL"),
            OsString::from("--security-opt"),
            OsString::from("no-new-privileges"),
            OsString::from("--read-only"),
            OsString::from("--pids-limit"),
            OsString::from(runner.pids_limit.to_string()),
            OsString::from("--tmpfs"),
            OsString::from(format!(
                "/tmp:rw,nosuid,nodev,noexec,size={}m",
                runner.tmpfs_mebibytes
            )),
        ];
        if let Some(cache_root) = &runner.cache_root {
            ensure_runner_cache_root(cache_root, identity)?;
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
        source.receipt.validate_against(plan)?;
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

        for input in &declaration.external_inputs {
            let workspace = &workspaces[&input.runner];
            let destination = workspace.join(&input.destination);
            ensure!(
                destination.starts_with(workspace),
                "external input escaped its runner workspace"
            );
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
            }
        }
        for (runner_id, workspace) in &workspaces {
            assign_runner_tree(
                workspace,
                container_identity(&binding.runners[runner_id].user)?,
            )?;
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
                &step.required_environment,
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
        Ok(format!("{prefix}-{suffix}.service"))
    }

    fn install_release(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &MaterializedRelease,
    ) -> Result<InstalledReleaseObservation> {
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
        for artifact in &release.release.artifacts {
            let path = installed.join(&artifact.destination);
            let (sha256, size_bytes) = digest_artifact(&path)?;
            ensure!(
                format!("sha256-{sha256}") == artifact.sha256 && size_bytes == artifact.size_bytes,
                "installed artifact {} differs from its sealed receipt",
                artifact.artifact_id
            );
        }
        harden_installed_release(&installed, &release.release.artifacts)?;
        Ok(InstalledReleaseObservation {
            sealed_release_id: release.release.sealed_release_id.clone(),
            root: installed,
        })
    }

    fn validate_installed_release(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &SealedRelease,
        installed: &InstalledReleaseObservation,
    ) -> Result<()> {
        release.validate_against(plan)?;
        let (_, binding) = plan.parsed_inputs()?;
        ensure!(
            installed.sealed_release_id == release.sealed_release_id
                && installed.root
                    == binding
                        .workload
                        .release_root
                        .join(&release.sealed_release_id),
            "installed release observation belongs to another sealed release"
        );
        for artifact in &release.artifacts {
            let path = installed.root.join(&artifact.destination);
            let (sha256, size_bytes) = digest_artifact(&path)?;
            ensure!(
                format!("sha256-{sha256}") == artifact.sha256 && size_bytes == artifact.size_bytes,
                "installed artifact {} differs from its sealed receipt",
                artifact.artifact_id
            );
        }
        harden_installed_release(&installed.root, &release.artifacts)
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
        harden_runtime_bundle(&bundle)?;
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
                OsString::from("--property=Description"),
                OsString::from("--property=InvocationID"),
                OsString::from("--property=MainPID"),
                OsString::from("--property=User"),
                OsString::from("--property=Group"),
                OsString::from("--property=WorkingDirectory"),
                OsString::from("--property=ControlGroup"),
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
        environment_names: &[String],
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
        let unit_description = required_systemd_property(&values, "Description")?.to_owned();
        let user = required_systemd_property(&values, "User")?.to_owned();
        let group = required_systemd_property(&values, "Group")?.to_owned();
        let working_directory =
            PathBuf::from(required_systemd_property(&values, "WorkingDirectory")?);
        ensure!(
            working_directory.is_absolute(),
            "systemd unit working directory is not absolute"
        );
        let control_group = required_systemd_property(&values, "ControlGroup")?.to_owned();
        ensure!(
            control_group.starts_with('/') && !control_group.contains(['\n', '\r', '\0']),
            "systemd unit control group is invalid"
        );
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
        let process_control_groups = fs::read_to_string(process_root.join("cgroup"))?;
        ensure!(
            process_control_groups.lines().any(|line| {
                line.split_once("::")
                    .is_some_and(|(_, path)| path == control_group)
            }),
            "systemd MainPID is outside the unit control group"
        );
        let command_line = fs::read(process_root.join("cmdline"))?;
        ensure!(
            !command_line.is_empty(),
            "systemd MainPID has no command line"
        );
        let process_environment = read_process_environment(&process_root.join("environ"))?;
        let selected_environment =
            select_process_environment(&process_environment, environment_names)?;
        let runtime_bundle = PathBuf::from(
            selected_environment
                .get(IDUNN_RUNTIME_BUNDLE_ENVIRONMENT)
                .context("systemd MainPID lacks the Idunn runtime bundle environment")?,
        );
        ensure!(
            runtime_bundle.is_absolute(),
            "runtime bundle path is not absolute"
        );
        Ok(WorkloadObservation {
            unit: unit.to_owned(),
            unit_description,
            invocation_id,
            main_pid,
            process_start_time,
            executable,
            executable_sha256,
            runtime_instance_id: runtime_instance_id.to_owned(),
            user,
            group,
            working_directory,
            runtime_bundle,
            command_line_sha256: sha256_id(&command_line),
            environment_names: environment_names.to_vec(),
            environment_contract_sha256: sha256_id(&rmp_serde::to_vec(&selected_environment)?),
            control_group,
        })
    }

    fn launch_command(
        &self,
        declaration: &TargetDeclaration,
        binding: &OperatorBinding,
        installed: &Path,
    ) -> Result<Vec<OsString>> {
        let executable_artifact =
            release_artifact(declaration, &declaration.service.executable_artifact)?;
        let executable = installed.join(&executable_artifact.destination);
        ensure!(executable.is_file(), "sealed service executable is absent");
        let mut command = vec![executable.into_os_string()];
        for argument in &declaration.service.arguments {
            command.push(match argument {
                LaunchArgument::Literal { value } => value.into(),
                LaunchArgument::Binding { name } => {
                    binding.workload.argument_bindings[name].clone().into()
                }
            });
        }
        Ok(command)
    }

    fn launch_environment(
        &self,
        binding: &OperatorBinding,
        bundle: &Path,
    ) -> Result<BTreeMap<String, String>> {
        let mut environment = binding.workload.environment.clone();
        for (name, path) in &binding.workload.secret_files {
            ensure!(
                environment
                    .insert(name.clone(), path.display().to_string())
                    .is_none(),
                "workload environment and secret bindings collide"
            );
        }
        ensure!(
            environment
                .insert(
                    IDUNN_RUNTIME_BUNDLE_ENVIRONMENT.into(),
                    bundle.display().to_string(),
                )
                .is_none(),
            "operator binding attempts to replace the Idunn runtime bundle"
        );
        Ok(environment)
    }

    fn validate_launch_observation(
        &self,
        observation: &WorkloadObservation,
        declaration: &TargetDeclaration,
        binding: &OperatorBinding,
        installed: &Path,
        bundle: &Path,
    ) -> Result<()> {
        let command = self.launch_command(declaration, binding, installed)?;
        let environment = self.launch_environment(binding, bundle)?;
        let environment_names = environment.keys().cloned().collect::<Vec<_>>();
        let expected_description = format!(
            "Idunn {} {}",
            binding.target,
            bundle
                .file_name()
                .context("runtime bundle has no instance id")?
                .to_string_lossy()
        );
        ensure!(
            observation.unit_description == expected_description
                && observation.user == binding.workload.user
                && observation.group == binding.workload.group
                && observation.working_directory == installed
                && observation.runtime_bundle == bundle
                && observation.command_line_sha256 == sha256_id(&proc_command_line(&command))
                && observation.environment_names == environment_names
                && observation.environment_contract_sha256
                    == sha256_id(&rmp_serde::to_vec(&environment)?),
            "running workload launch contract differs from the admitted launch"
        );
        ensure!(
            Path::new(&observation.control_group).file_name()
                == Some(OsStr::new(&observation.unit)),
            "running workload control group belongs to another unit"
        );
        Ok(())
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
        let unit_description = format!(
            "Idunn {} {}",
            binding.target,
            bundle
                .file_name()
                .context("runtime bundle has no instance id")?
                .to_string_lossy()
        );
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
            OsString::from(format!("--property=Description={unit_description}")),
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
        if let Some(write_lease) = &binding.process_write_lease {
            for path in [write_lease.record_path.clone(), write_lease.lock_path()] {
                args.push(OsString::from(format!(
                    "--property=ReadOnlyPaths={}",
                    path.display()
                )));
            }
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
        args.extend(self.launch_command(declaration, binding, installed)?);
        self.command(&self.systemd_run_program, args)?;
        Ok(())
    }
}

impl WorkloadPort for SystemdTransientWorkloadDriver {
    fn install(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &MaterializedRelease,
    ) -> Result<InstalledReleaseObservation> {
        self.install_release(plan, release)
    }

    fn start(
        &self,
        plan: &CompiledDeploymentPlan,
        release: &SealedRelease,
        installed: &InstalledReleaseObservation,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
    ) -> Result<WorkloadObservation> {
        plan.validate()?;
        release.validate_against(plan)?;
        self.validate_installed_release(plan, release, installed)?;
        expected.validate()?;
        activation.validate()?;
        ensure!(
            expected.plan_id == plan.plan_id
                && expected.sealed_release_id == release.sealed_release_id
                && activation.expected_projection_sha256 == expected.canonical_sha256()?,
            "workload inputs do not describe one sealed incarnation"
        );
        let (declaration, binding) = plan.parsed_inputs()?;
        let executable_artifact =
            release_artifact(&declaration, &declaration.service.executable_artifact)?;
        let executable = installed.root.join(&executable_artifact.destination);
        let unit = self.unit_name(
            &binding.workload.unit_prefix,
            &activation.runtime_instance_id,
        )?;
        let bundle = self.prepare_runtime_bundle(&binding, expected, activation)?;
        let environment_names = self
            .launch_environment(&binding, &bundle)?
            .into_keys()
            .collect::<Vec<_>>();
        if self.show_unit(&unit)?.is_some() {
            let observation = self.observe_unit(
                &unit,
                &executable,
                &activation.runtime_instance_id,
                &environment_names,
            )?;
            self.validate_launch_observation(
                &observation,
                &declaration,
                &binding,
                &installed.root,
                &bundle,
            )?;
            ensure!(
                observation.executable_sha256 == expected.artifact_sha256,
                "running workload executable differs from Expected"
            );
            return Ok(observation);
        }
        self.start_transient(&declaration, &binding, &installed.root, &bundle, &unit)?;
        let mut last_error = None;
        for _ in 0..100 {
            match self.observe_unit(
                &unit,
                &executable,
                &activation.runtime_instance_id,
                &environment_names,
            ) {
                Ok(observation) => {
                    self.validate_launch_observation(
                        &observation,
                        &declaration,
                        &binding,
                        &installed.root,
                        &bundle,
                    )?;
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
        let observed = self.observe_unit(
            &prior.unit,
            &prior.executable,
            &prior.runtime_instance_id,
            &prior.environment_names,
        )?;
        ensure!(
            observed == *prior && observed.executable_sha256 == expected.artifact_sha256,
            "native workload identity changed after observation"
        );
        Ok(observed)
    }

    fn stop(&self, observation: &WorkloadObservation) -> Result<()> {
        let Some(values) = self.show_unit(&observation.unit)? else {
            return Ok(());
        };
        let active = required_systemd_property(&values, "ActiveState")?;
        let sub = required_systemd_property(&values, "SubState")?;
        if active == "active" && sub == "running" {
            let current = self.observe_unit(
                &observation.unit,
                &observation.executable,
                &observation.runtime_instance_id,
                &observation.environment_names,
            )?;
            ensure!(
                current == *observation,
                "refusing to stop a workload whose native identity changed"
            );
        } else {
            ensure!(
                values.get("InvocationID") == Some(&observation.invocation_id)
                    && matches!(active, "inactive" | "failed"),
                "refusing to stop a foreign or transitional systemd unit"
            );
        }
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
    pub target: String,
    pub record_path: PathBuf,
}

impl CultCacheWriteLeaseDriver {
    pub fn new(target: impl Into<String>, record_path: impl Into<PathBuf>) -> Self {
        Self {
            target: target.into(),
            record_path: record_path.into(),
        }
    }

    fn current(&self) -> Result<Option<(CultCacheEnvelope, IdunnProcessWriteLeaseRecord)>> {
        validate_root_authority_path(&self.record_path)?;
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
                ensure!(
                    lease.target == self.target,
                    "write-lease store belongs to another target"
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

    fn validate_grant(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        warming: &PresenceObservation,
        lease: &IdunnProcessWriteLeaseRecord,
    ) -> Result<()> {
        expected.validate()?;
        activation.validate()?;
        lease.validate()?;
        let expected_sha256 = expected.canonical_sha256()?;
        let activation_sha256 = activation.canonical_sha256()?;
        let warming_presence_sha256 = warming
            .signed_presence_sha256
            .as_deref()
            .context("warming observation has no signed presence")?;
        ensure!(
            self.target == expected.target
                && lease.target == expected.target
                && lease.expected_projection_sha256 == expected_sha256
                && lease.plan_id == expected.plan_id
                && lease.incarnation_id == expected.incarnation_id
                && lease.sealed_release_id == expected.sealed_release_id
                && lease.activation_witness_sha256 == activation_sha256
                && lease.runtime_id == expected.runtime_id
                && lease.runtime_instance_id == activation.runtime_instance_id
                && lease.state_schema_generation
                    == expected
                        .state_schema_generation
                        .as_deref()
                        .context("write-lease Expected has no state generation")?
                && lease.state_contract_sha256
                    == expected
                        .state_contract_sha256
                        .as_deref()
                        .context("write-lease Expected has no state contract")?
                && lease.warming_presence_sha256 == warming_presence_sha256
                && warming.present
                && warming.state.as_deref() == Some("warming")
                && warming.runtime_instance_id.as_deref()
                    == Some(activation.runtime_instance_id.as_str())
                && warming.write_lease_sha256.is_none()
                && warming.disagreements.is_empty(),
            "process write lease does not bind the exact warming candidate"
        );
        Ok(())
    }
}

impl WriteLeasePort for CultCacheWriteLeaseDriver {
    fn revoke_exact(&self, incumbent: Option<&IdunnProcessWriteLeaseRecord>) -> Result<()> {
        if let Some(incumbent) = incumbent {
            incumbent.validate()?;
            ensure!(
                incumbent.target == self.target,
                "incumbent write lease belongs to another target"
            );
        }
        let current = self.current()?;
        let Some((envelope, current_lease)) = current else {
            return Ok(());
        };
        ensure!(
            incumbent == Some(&current_lease),
            "refusing to revoke an unexpected process write lease"
        );
        let store = SingleFileMessagePackBackingStore::new(&self.record_path);
        ensure!(
            store.compare_exchange_snapshot(std::slice::from_ref(&envelope), &[])?,
            "process write lease changed while fencing the incumbent"
        );
        Ok(())
    }

    fn observe_empty(&self) -> Result<bool> {
        Ok(self.current()?.is_none())
    }

    fn grant(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        warming: &PresenceObservation,
        lease: &IdunnProcessWriteLeaseRecord,
    ) -> Result<String> {
        self.validate_grant(expected, activation, warming, lease)?;
        if let Some((_, current)) = self.current()? {
            ensure!(
                current == *lease,
                "another process already owns the write lease"
            );
            return lease.canonical_sha256();
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
        harden_root_authority_file(&self.record_path)?;
        harden_root_authority_file(&authority_lock_path(&self.record_path))?;
        lease.canonical_sha256()
    }

    fn observe(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        warming: &PresenceObservation,
        lease: &IdunnProcessWriteLeaseRecord,
    ) -> Result<bool> {
        self.validate_grant(expected, activation, warming, lease)?;
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
            route.route_id == self.binding.route_id
                && route.stable_endpoint == self.binding.stable_endpoint,
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

    fn loaded_matches(&self, rendered: &[u8]) -> Result<bool> {
        let output = self.command(&self.nginx_program, [OsString::from("-T")])?;
        let loaded = std::str::from_utf8(&output.stdout)
            .context("nginx loaded configuration is not UTF-8")?;
        Ok(nginx_config_section(loaded, &self.binding.config_path)?
            .is_some_and(|section| section.as_bytes() == rendered))
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
    fn preflight(
        &self,
        expected: &IdunnExpectedIncarnationRecord,
        runtime_instance_id: &str,
    ) -> Result<()> {
        let rendered = self.render(expected, runtime_instance_id)?;
        ensure!(!rendered.is_empty(), "candidate route rendered empty");
        self.command(&self.nginx_program, [OsString::from("-t")])?;
        let current = match fs::read(&self.binding.config_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("reading route preflight baseline"),
        };
        let output = self.command(&self.nginx_program, [OsString::from("-T")])?;
        let dump = std::str::from_utf8(&output.stdout)
            .context("nginx loaded configuration is not UTF-8")?;
        let loaded = nginx_config_section(dump, &self.binding.config_path)?;
        match (current.as_deref(), loaded) {
            (None, None) => {}
            (Some(current), Some(loaded)) if current == loaded.as_bytes() => {}
            _ => bail!("nginx route baseline differs from its loaded configuration"),
        }
        Ok(())
    }

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
        match self.loaded_matches(&rendered) {
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
        Ok(current == rendered && self.loaded_matches(&rendered)?)
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
fn validate_frozen_source_store(store: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    ensure!(store.is_absolute(), "frozen source store is not absolute");
    let canonical_store = store
        .canonicalize()
        .with_context(|| format!("resolving frozen source store {}", store.display()))?;
    ensure!(
        canonical_store == store,
        "frozen source store traverses a symlink or noncanonical path"
    );
    let metadata = fs::symlink_metadata(store)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0,
        "frozen source store is not root-owned and nonwritable"
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_frozen_source_store(_store: &Path) -> Result<()> {
    bail!("frozen source materialization requires a Unix actuator")
}

#[cfg(unix)]
fn prepare_frozen_transaction_root(store: &Path, transaction_id: &str) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    require_driver_id(transaction_id, "source transaction")?;
    validate_frozen_source_store(store)?;
    let transaction_root = store.join(transaction_id);
    if transaction_root.exists() {
        remove_frozen_transaction_root(store, transaction_id)?;
    }
    fs::create_dir(&transaction_root)?;
    fs::set_permissions(&transaction_root, fs::Permissions::from_mode(0o700))?;
    Ok(transaction_root)
}

#[cfg(not(unix))]
fn prepare_frozen_transaction_root(_store: &Path, _transaction_id: &str) -> Result<PathBuf> {
    bail!("frozen source materialization requires a Unix actuator")
}

#[cfg(unix)]
fn remove_frozen_transaction_root(store: &Path, transaction_id: &str) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    require_driver_id(transaction_id, "source transaction")?;
    validate_frozen_source_store(store)?;
    let transaction_root = store.join(transaction_id);
    let metadata = fs::symlink_metadata(&transaction_root)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0,
        "frozen source transaction root is not Idunn-owned"
    );
    fs::set_permissions(&transaction_root, fs::Permissions::from_mode(0o700))?;
    fs::remove_dir_all(&transaction_root).with_context(|| {
        format!(
            "removing exact frozen source transaction {}",
            transaction_root.display()
        )
    })
}

#[cfg(not(unix))]
fn remove_frozen_transaction_root(_store: &Path, _transaction_id: &str) -> Result<()> {
    bail!("frozen source cleanup requires a Unix actuator")
}

#[cfg(unix)]
fn prepare_frozen_source_destination(destination: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    ensure!(
        unsafe { libc::geteuid() } == 0,
        "frozen source materialization requires root Idunn"
    );
    ensure!(
        destination.is_absolute(),
        "frozen source destination is not absolute"
    );
    let parent = destination
        .parent()
        .context("frozen source destination has no parent")?;
    ensure!(destination != parent, "frozen source destination is broad");
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("resolving frozen source parent {}", parent.display()))?;
    ensure!(
        canonical_parent == parent,
        "frozen source parent traverses a symlink or noncanonical path"
    );
    let parent_metadata = fs::symlink_metadata(parent)?;
    ensure!(
        parent_metadata.is_dir()
            && !parent_metadata.file_type().is_symlink()
            && parent_metadata.uid() == 0
            && parent_metadata.permissions().mode() & 0o022 == 0,
        "frozen source parent is not root-owned and nonwritable"
    );
    if destination.exists() {
        remove_frozen_source_destination(destination)?;
    }
    fs::create_dir(destination)
        .with_context(|| format!("creating frozen source {}", destination.display()))?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_frozen_source_destination(_destination: &Path) -> Result<()> {
    bail!("frozen source materialization requires a Unix actuator")
}

#[cfg(unix)]
fn remove_frozen_source_destination(destination: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    ensure!(
        destination.is_absolute(),
        "frozen source destination is not absolute"
    );
    let parent = destination
        .parent()
        .context("frozen source destination has no parent")?;
    ensure!(destination != parent, "frozen source destination is broad");
    let parent_metadata = fs::symlink_metadata(parent)?;
    let metadata = fs::symlink_metadata(destination)?;
    ensure!(
        parent_metadata.is_dir()
            && !parent_metadata.file_type().is_symlink()
            && parent_metadata.uid() == 0
            && parent_metadata.permissions().mode() & 0o022 == 0,
        "frozen source parent is not root-owned and nonwritable"
    );
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.permissions().mode() & 0o022 == 0,
        "existing frozen source is not an Idunn-owned directory"
    );
    fs::remove_dir_all(destination)
        .with_context(|| format!("removing exact frozen source {}", destination.display()))
}

#[cfg(not(unix))]
fn remove_frozen_source_destination(_destination: &Path) -> Result<()> {
    bail!("frozen source materialization requires a Unix actuator")
}

#[cfg(unix)]
fn harden_frozen_source(root: &Path) -> Result<()> {
    harden_frozen_source_tree(root, root)
}

#[cfg(unix)]
fn harden_frozen_source_tree(root: &Path, current: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(current)?;
    ensure!(metadata.uid() == 0, "frozen source entry is not root-owned");
    ensure!(
        current == root || current.file_name() != Some(OsStr::new(".git")),
        "frozen source contains forbidden .git metadata"
    );
    if metadata.is_dir() {
        let mut entries = fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            harden_frozen_source_tree(root, &entry.path())?;
        }
        fs::set_permissions(current, fs::Permissions::from_mode(0o555))?;
    } else if metadata.is_file() {
        let mode = if metadata.permissions().mode() & 0o111 == 0 {
            0o444
        } else {
            0o555
        };
        fs::set_permissions(current, fs::Permissions::from_mode(mode))?;
    } else if metadata.file_type().is_symlink() {
        validate_frozen_source_symlink(root, current)?;
    } else {
        bail!("frozen source contains a special filesystem entry")
    }
    Ok(())
}

#[cfg(unix)]
fn validate_frozen_source_symlink(root: &Path, path: &Path) -> Result<()> {
    let target = fs::read_link(path)?;
    ensure!(target.is_relative(), "frozen source symlink is absolute");
    let parent = path
        .parent()
        .context("frozen source symlink has no parent")?;
    let mut components = parent
        .strip_prefix(root)?
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in target.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(value) => components.push(value.to_os_string()),
            std::path::Component::ParentDir => {
                ensure!(
                    components.pop().is_some(),
                    "frozen source symlink escapes its root"
                );
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                bail!("frozen source symlink is absolute")
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn harden_frozen_source(_root: &Path) -> Result<()> {
    bail!("frozen source materialization requires Unix permissions")
}

#[cfg(unix)]
fn validate_frozen_source(root: &Path) -> Result<()> {
    validate_frozen_source_tree(root, root)
}

#[cfg(unix)]
fn validate_frozen_source_tree(root: &Path, current: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(current)?;
    ensure!(metadata.uid() == 0, "frozen source entry is not root-owned");
    ensure!(
        current == root || current.file_name() != Some(OsStr::new(".git")),
        "frozen source contains forbidden .git metadata"
    );
    if metadata.is_dir() {
        ensure!(
            metadata.permissions().mode() & 0o777 == 0o555,
            "frozen source directory is not 0555"
        );
        let mut entries = fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            validate_frozen_source_tree(root, &entry.path())?;
        }
    } else if metadata.is_file() {
        ensure!(
            matches!(metadata.permissions().mode() & 0o777, 0o444 | 0o555),
            "frozen source file has a noncanonical mode"
        );
    } else if metadata.file_type().is_symlink() {
        validate_frozen_source_symlink(root, current)?;
    } else {
        bail!("frozen source contains a special filesystem entry")
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_frozen_source(_root: &Path) -> Result<()> {
    bail!("frozen source observation requires Unix permissions")
}

#[cfg(unix)]
fn frozen_source_sha256(root: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    hash_frozen_source_tree(root, root, &mut hasher)?;
    Ok(format!("sha256-{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn hash_frozen_source_tree(root: &Path, current: &Path, hasher: &mut Sha256) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut entries = fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = normalized_relative(path.strip_prefix(root)?)?;
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            hasher.update(b"dir\0");
            hasher.update(relative.as_bytes());
            hasher.update(b"\0");
            hash_frozen_source_tree(root, &path, hasher)?;
        } else if metadata.is_file() {
            let bytes = fs::read(&path)?;
            hasher.update(b"file\0");
            hasher.update(relative.as_bytes());
            hasher.update(b"\0");
            hasher.update(
                (metadata.permissions().mode() & 0o111 != 0)
                    .to_string()
                    .as_bytes(),
            );
            hasher.update(b"\0");
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        } else if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            hasher.update(b"link\0");
            hasher.update(relative.as_bytes());
            hasher.update(b"\0");
            hasher.update(target.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        } else {
            bail!("frozen source contains a special filesystem entry")
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn frozen_source_sha256(_root: &Path) -> Result<String> {
    bail!("frozen source observation requires Unix permissions")
}

#[cfg(unix)]
fn harden_installed_release(root: &Path, artifacts: &[ArtifactReceipt]) -> Result<()> {
    let executable_paths = artifacts
        .iter()
        .filter(|artifact| artifact.executable)
        .map(|artifact| root.join(&artifact.destination))
        .collect::<std::collections::BTreeSet<_>>();
    harden_root_tree(root, root, &executable_paths)
}

#[cfg(unix)]
fn harden_root_tree(
    root: &Path,
    current: &Path,
    executable_paths: &std::collections::BTreeSet<PathBuf>,
) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(current)?;
    ensure!(
        metadata.uid() == 0,
        "sealed release entry is not root-owned"
    );
    if metadata.is_dir() {
        let mut entries = fs::read_dir(current)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            harden_root_tree(root, &entry.path(), executable_paths)?;
        }
        fs::set_permissions(current, fs::Permissions::from_mode(0o555))?;
    } else if metadata.is_file() {
        fs::set_permissions(
            current,
            fs::Permissions::from_mode(if executable_paths.contains(current) {
                0o555
            } else {
                0o444
            }),
        )?;
    } else if metadata.file_type().is_symlink() {
        validate_release_symlink(root, current)?;
    } else {
        bail!("sealed release contains a special filesystem entry")
    }
    Ok(())
}

#[cfg(unix)]
fn validate_release_symlink(root: &Path, path: &Path) -> Result<()> {
    let target = fs::read_link(path)?;
    ensure!(target.is_relative(), "sealed release symlink is absolute");
    ensure!(
        target.components().all(|component| matches!(
            component,
            std::path::Component::CurDir | std::path::Component::Normal(_)
        )),
        "sealed release symlink has a parent traversal"
    );
    let destination = path
        .parent()
        .context("sealed release symlink has no parent")?
        .join(target);
    ensure!(
        destination.starts_with(root),
        "sealed release symlink escaped its release"
    );
    Ok(())
}

#[cfg(not(unix))]
fn harden_installed_release(_root: &Path, _artifacts: &[ArtifactReceipt]) -> Result<()> {
    bail!("systemd workload installation requires Unix permissions")
}

#[cfg(unix)]
fn harden_runtime_bundle(bundle: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(bundle)?;
    let parent = bundle.parent().context("runtime bundle has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink() && metadata.uid() == 0,
        "runtime bundle is not a root-owned directory"
    );
    ensure!(
        parent_metadata.is_dir()
            && !parent_metadata.file_type().is_symlink()
            && parent_metadata.uid() == 0
            && parent_metadata.permissions().mode() & 0o022 == 0,
        "runtime bundle root is not root-owned and service-nonwritable"
    );
    for name in [
        "expected.cc",
        "expected.cc.lock",
        "activation.cc",
        "activation.cc.lock",
    ] {
        let path = bundle.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink() && metadata.uid() == 0,
            "runtime bundle document is not root-owned"
        );
        fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
    }
    fs::set_permissions(bundle, fs::Permissions::from_mode(0o555))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_runtime_bundle(_bundle: &Path) -> Result<()> {
    bail!("systemd runtime bundles require Unix permissions")
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

fn required_systemd_property<'a>(
    values: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str> {
    values
        .get(name)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .with_context(|| format!("systemd unit has no {name}"))
}

fn read_process_environment(path: &Path) -> Result<Vec<Vec<u8>>> {
    let bytes = fs::read(path)
        .with_context(|| format!("reading process environment {}", path.display()))?;
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn select_process_environment(
    entries: &[Vec<u8>],
    names: &[String],
) -> Result<BTreeMap<String, String>> {
    ensure!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "workload environment witness names are not unique and sorted"
    );
    let mut selected = BTreeMap::new();
    for name in names {
        let prefix = format!("{name}=").into_bytes();
        let matches = entries
            .iter()
            .filter(|entry| entry.starts_with(&prefix))
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1,
            "workload process environment has missing or duplicate {name}"
        );
        let value = std::str::from_utf8(&matches[0][prefix.len()..])
            .with_context(|| format!("workload environment {name} is not UTF-8"))?;
        selected.insert(name.clone(), value.to_owned());
    }
    Ok(selected)
}

fn proc_command_line(command: &[OsString]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for argument in command {
        encoded.extend_from_slice(argument.as_encoded_bytes());
        encoded.push(0);
    }
    encoded
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

fn nginx_config_section<'a>(dump: &'a str, config_path: &Path) -> Result<Option<&'a str>> {
    let marker = format!("# configuration file {}:\n", config_path.display());
    let matches = dump.match_indices(&marker).collect::<Vec<_>>();
    ensure!(
        matches.len() <= 1,
        "nginx loaded configuration contains duplicate sections for {}",
        config_path.display()
    );
    let Some((start, _)) = matches.first().copied() else {
        return Ok(None);
    };
    let content = &dump[start + marker.len()..];
    let end = content
        .find("\n# configuration file ")
        .map_or(content.len(), |index| index + 1);
    Ok(Some(&content[..end]))
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

fn require_sha256_id(value: &str, label: &str) -> Result<()> {
    let digest = value
        .strip_prefix("sha256-")
        .with_context(|| format!("{label} has no sha256 prefix"))?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} is not a lowercase SHA-256 id"
    );
    Ok(())
}

fn require_driver_id(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| { byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-') }),
        "{label} id is invalid"
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

fn container_identity(value: &str) -> Result<ProcessIdentity> {
    let (uid, gid) = value
        .split_once(':')
        .context("runner user is not numeric UID:GID")?;
    let identity = ProcessIdentity {
        uid: uid.parse().context("runner UID is not a u32")?,
        gid: gid.parse().context("runner GID is not a u32")?,
    };
    ensure!(
        identity.uid > 0 && identity.gid > 0,
        "runner identity must be unprivileged"
    );
    Ok(identity)
}

#[cfg(unix)]
fn ensure_runner_cache_root(path: &Path, identity: ProcessIdentity) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    ensure!(
        unsafe { libc::geteuid() } == 0,
        "runner cache admission requires root Idunn"
    );
    let parent = path.parent().context("runner cache root has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("inspecting runner cache parent {}", parent.display()))?;
    ensure!(
        parent_metadata.is_dir()
            && !parent_metadata.file_type().is_symlink()
            && parent_metadata.uid() == 0
            && parent_metadata.permissions().mode() & 0o022 == 0,
        "runner cache parent is not root-owned and nonwritable"
    );
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == identity.uid
                && metadata.gid() == identity.gid
                && metadata.permissions().mode() & 0o777 == 0o700,
            "runner cache is not a dedicated exact-identity 0700 directory"
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir(path)
                .with_context(|| format!("creating runner cache {}", path.display()))?;
            let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())
                .context("runner cache path contains a NUL byte")?;
            if unsafe { libc::lchown(path_c.as_ptr(), identity.uid, identity.gid) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("assigning runner cache owner");
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspecting runner cache {}", path.display()));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_runner_cache_root(_path: &Path, _identity: ProcessIdentity) -> Result<()> {
    bail!("runner cache admission requires a Unix actuator")
}

#[cfg(unix)]
fn validate_runner_secret(path: &Path, identity: ProcessIdentity) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    ensure!(
        unsafe { libc::geteuid() } == 0,
        "runner secret admission requires root Idunn"
    );
    let parent = path.parent().context("runner secret has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("inspecting runner secret parent {}", parent.display()))?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspecting runner secret {}", path.display()))?;
    ensure!(
        parent_metadata.is_dir()
            && !parent_metadata.file_type().is_symlink()
            && parent_metadata.uid() == 0
            && parent_metadata.permissions().mode() & 0o022 == 0,
        "runner secret parent is not root-owned and nonwritable"
    );
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.gid() == identity.gid
            && metadata.permissions().mode() & 0o777 == 0o440,
        "runner secret is not root-owned, exact-group-bound, and 0440"
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_runner_secret(_path: &Path, _identity: ProcessIdentity) -> Result<()> {
    bail!("runner secret admission requires a Unix actuator")
}

#[cfg(unix)]
fn assign_runner_tree(path: &Path, identity: ProcessIdentity) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::PermissionsExt;

    ensure!(
        unsafe { libc::geteuid() } == 0,
        "runner workspace ownership requires root Idunn"
    );
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            assign_runner_tree(&entry.path(), identity)?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o750))?;
    } else if metadata.is_file() {
        let executable = metadata.permissions().mode() & 0o111 != 0;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if executable { 0o750 } else { 0o640 }),
        )?;
    } else if !metadata.file_type().is_symlink() {
        bail!("runner workspace contains a special filesystem entry")
    }
    let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .context("runner workspace path contains a NUL byte")?;
    let result = unsafe { libc::lchown(path_c.as_ptr(), identity.uid, identity.gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("assigning runner workspace owner");
    }
    Ok(())
}

#[cfg(not(unix))]
fn assign_runner_tree(_path: &Path, _identity: ProcessIdentity) -> Result<()> {
    bail!("non-root runner identities require a Unix actuator")
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

fn authority_lock_path(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_os_string();
    lock.push(".lock");
    PathBuf::from(lock)
}

#[cfg(unix)]
fn validate_root_authority_path(path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    ensure!(
        unsafe { libc::geteuid() } == 0,
        "process write-lease actuation requires root Idunn"
    );
    let parent = path
        .parent()
        .context("process write-lease path has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)
        .with_context(|| format!("inspecting write-lease parent {}", parent.display()))?;
    ensure!(
        parent_metadata.is_dir()
            && !parent_metadata.file_type().is_symlink()
            && parent_metadata.uid() == 0
            && parent_metadata.permissions().mode() & 0o022 == 0,
        "write-lease parent is not root-owned and service-nonwritable"
    );
    for authority_file in [path.to_owned(), authority_lock_path(path)] {
        match fs::symlink_metadata(&authority_file) {
            Ok(metadata) => ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == 0
                    && metadata.gid() == parent_metadata.gid()
                    && metadata.permissions().mode() & 0o022 == 0,
                "write-lease authority file {} is not root-owned and service-nonwritable",
                authority_file.display()
            ),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "inspecting write-lease authority file {}",
                        authority_file.display()
                    )
                });
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_root_authority_path(_path: &Path) -> Result<()> {
    bail!("process write-lease actuation requires a Unix authority path")
}

#[cfg(unix)]
fn harden_root_authority_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let parent = path
        .parent()
        .context("process write-lease authority file has no parent")?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == 0
            && metadata.gid() == parent_metadata.gid(),
        "new write-lease authority file has the wrong owner"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o640))?;
    Ok(())
}

#[cfg(not(unix))]
fn harden_root_authority_file(_path: &Path) -> Result<()> {
    bail!("process write-lease actuation requires Unix permissions")
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
        ensure!(
            identity.uid > 0 && identity.gid > 0,
            "configured source identity must be unprivileged"
        );
        unsafe {
            command.pre_exec(move || {
                if libc::setgroups(0, std::ptr::null()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setgid(identity.gid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setuid(identity.uid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        let _ = identity;
        bail!("configured process identities require a Unix actuator")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cultnet_rs::{
        IDUNN_EXPECTED_INCARNATION_SCHEMA, IDUNN_PROCESS_WRITE_LEASE_SCHEMA,
        IDUNN_RUNTIME_ACTIVATION_SCHEMA,
    };

    fn digest(byte: char) -> String {
        format!("sha256-{}", byte.to_string().repeat(64))
    }

    #[cfg(unix)]
    fn git_at(repository: &Path, arguments: &[&str]) -> Result<String> {
        let output = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(repository)
            .args(arguments)
            .env_clear()
            .env("HOME", repository)
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LANG", "C.UTF-8")
            .output()?;
        ensure!(
            output.status.success(),
            "test Git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(String::from_utf8(output.stdout)?.trim().to_owned())
    }

    fn expected() -> IdunnExpectedIncarnationRecord {
        IdunnExpectedIncarnationRecord {
            schema_version: IDUNN_EXPECTED_INCARNATION_SCHEMA.into(),
            target: "service".into(),
            plan_id: digest('1'),
            incarnation_id: "incarnation-1".into(),
            sealed_release_id: digest('2'),
            source_repository: "github.com/GameCult/Service".into(),
            source_revision: "3".repeat(40),
            recipe_sha256: digest('4'),
            runtime_id: "service-runtime".into(),
            expected_signer_identity_id: "service-signer".into(),
            health_contract: "service.health.v1".into(),
            artifact_sha256: digest('5'),
            state_schema_generation: Some("state-v1".into()),
            state_contract_sha256: Some(digest('6')),
            write_lease_required: true,
            route: None,
            capabilities: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    fn activation(expected: &IdunnExpectedIncarnationRecord) -> IdunnRuntimeActivationRecord {
        IdunnRuntimeActivationRecord {
            schema_version: IDUNN_RUNTIME_ACTIVATION_SCHEMA.into(),
            expected_projection_sha256: expected.canonical_sha256().unwrap(),
            runtime_instance_id: digest('7'),
            issued_at_unix_millis: 100,
        }
    }

    fn warming(activation: &IdunnRuntimeActivationRecord) -> PresenceObservation {
        PresenceObservation {
            correlation_sha256: digest('8'),
            signed_presence_sha256: Some(digest('9')),
            runtime_instance_id: Some(activation.runtime_instance_id.clone()),
            present: true,
            state: Some("warming".into()),
            write_lease_sha256: None,
            disagreements: Vec::new(),
        }
    }

    fn lease(
        expected: &IdunnExpectedIncarnationRecord,
        activation: &IdunnRuntimeActivationRecord,
        warming: &PresenceObservation,
    ) -> IdunnProcessWriteLeaseRecord {
        IdunnProcessWriteLeaseRecord {
            schema_version: IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into(),
            target: expected.target.clone(),
            expected_projection_sha256: expected.canonical_sha256().unwrap(),
            plan_id: expected.plan_id.clone(),
            incarnation_id: expected.incarnation_id.clone(),
            sealed_release_id: expected.sealed_release_id.clone(),
            activation_witness_sha256: activation.canonical_sha256().unwrap(),
            state_schema_generation: expected.state_schema_generation.clone().unwrap(),
            state_contract_sha256: expected.state_contract_sha256.clone().unwrap(),
            runtime_id: expected.runtime_id.clone(),
            runtime_instance_id: activation.runtime_instance_id.clone(),
            warming_presence_sha256: warming.signed_presence_sha256.clone().unwrap(),
            lease_epoch: 1,
            issued_at_unix_millis: 200,
        }
    }

    #[test]
    fn nginx_loaded_observation_is_bound_to_the_exact_file_section() -> Result<()> {
        let path = Path::new("/etc/nginx/idunn-routes/service.conf");
        let rendered = "upstream idunn_service {\n    server 127.0.0.1:4104;\n}\n";
        let dump = format!(
            "# configuration file /etc/nginx/nginx.conf:\nevents {{}}\n# configuration file {}:\n{rendered}# configuration file /etc/nginx/other.conf:\n{rendered}",
            path.display()
        );
        assert_eq!(nginx_config_section(&dump, path)?, Some(rendered));
        assert_eq!(
            nginx_config_section(&dump, Path::new("/etc/nginx/missing.conf"))?,
            None
        );
        let duplicate = format!("{dump}# configuration file {}:\n{rendered}", path.display());
        assert!(nginx_config_section(&duplicate, path).is_err());
        Ok(())
    }

    #[test]
    fn process_environment_witness_is_sorted_exact_and_secret_free() -> Result<()> {
        let entries = vec![
            b"B=two".to_vec(),
            b"A=one".to_vec(),
            b"UNRELATED=ignored".to_vec(),
        ];
        let selected = select_process_environment(&entries, &["A".into(), "B".into()])?;
        assert_eq!(selected.get("A").map(String::as_str), Some("one"));
        assert!(!selected.contains_key("UNRELATED"));
        assert!(select_process_environment(&entries, &["B".into(), "A".into()]).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn write_lease_revoke_is_exact_and_never_deletes_a_surprise() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("lease.cc");
        let driver = CultCacheWriteLeaseDriver::new("service", &path);
        let expected = expected();
        let activation = activation(&expected);
        let warming = warming(&activation);
        let lease = lease(&expected, &activation, &warming);
        let lease_sha256 = driver.grant(&expected, &activation, &warming, &lease)?;
        assert_eq!(lease_sha256, lease.canonical_sha256()?);
        assert!(driver.observe(&expected, &activation, &warming, &lease)?);

        let mut surprise = lease.clone();
        surprise.lease_epoch = 2;
        assert!(driver.revoke_exact(Some(&surprise)).is_err());
        assert!(driver.observe(&expected, &activation, &warming, &lease)?);

        driver.revoke_exact(Some(&lease))?;
        assert!(driver.observe_empty()?);
        driver.revoke_exact(Some(&lease))?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn installed_release_is_root_owned_and_nonwritable() -> Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir()?;
        let root = temp.path().join("release");
        fs::create_dir(&root)?;
        let artifact = root.join("service");
        fs::write(&artifact, b"sealed")?;
        fs::set_permissions(&artifact, fs::Permissions::from_mode(0o777))?;
        harden_installed_release(
            &root,
            &[ArtifactReceipt {
                artifact_id: "service".into(),
                destination: PathBuf::from("service"),
                sha256: sha256_id(b"sealed"),
                size_bytes: 6,
                executable: true,
            }],
        )?;
        let root_metadata = fs::symlink_metadata(&root)?;
        let artifact_metadata = fs::symlink_metadata(&artifact)?;
        assert_eq!(root_metadata.uid(), 0);
        assert_eq!(root_metadata.permissions().mode() & 0o777, 0o555);
        assert_eq!(artifact_metadata.uid(), 0);
        assert_eq!(artifact_metadata.permissions().mode() & 0o777, 0o555);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn runner_cache_is_created_once_for_one_exact_identity() -> Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let cache = temp.path().join("cache");
        let identity = ProcessIdentity {
            uid: 1000,
            gid: 1000,
        };
        ensure_runner_cache_root(&cache, identity)?;
        let metadata = fs::symlink_metadata(&cache)?;
        assert_eq!((metadata.uid(), metadata.gid()), (1000, 1000));
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        assert!(ensure_runner_cache_root(&cache, identity).is_ok());
        assert!(
            ensure_runner_cache_root(
                &cache,
                ProcessIdentity {
                    uid: 1001,
                    gid: 1001
                }
            )
            .is_err()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn runner_secret_is_root_owned_and_bound_to_the_runner_group() -> Result<()> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        let secret = temp.path().join("token");
        fs::write(&secret, b"secret")?;
        let secret_c = std::ffi::CString::new(secret.as_os_str().as_bytes())?;
        ensure!(unsafe { libc::lchown(secret_c.as_ptr(), 0, 1000) } == 0);
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o440))?;
        let identity = ProcessIdentity {
            uid: 1000,
            gid: 1000,
        };
        validate_runner_secret(&secret, identity)?;
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o444))?;
        assert!(validate_runner_secret(&secret, identity).is_err());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn exact_git_archive_becomes_root_owned_immutable_source_without_git_metadata() -> Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let temp = tempfile::tempdir()?;
        let repository = temp.path().join("repository");
        fs::create_dir(&repository)?;
        git_at(&repository, &["init", "--initial-branch=main"])?;
        git_at(&repository, &["config", "user.name", "Idunn Test"])?;
        git_at(
            &repository,
            &["config", "user.email", "idunn-test@example.invalid"],
        )?;
        fs::write(repository.join("deployment.toml"), b"target = 'test'\n")?;
        let script = repository.join("build.sh");
        fs::write(&script, b"#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        fs::create_dir(repository.join("links"))?;
        symlink("../deployment.toml", repository.join("links/deployment"))?;
        git_at(&repository, &["add", "--all"])?;
        git_at(&repository, &["commit", "-m", "fixture"])?;
        let revision = git_at(&repository, &["rev-parse", "HEAD"])?;

        let parent = temp.path().join("transaction");
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let destination = parent.join("source");
        prepare_frozen_source_destination(&destination)?;
        let driver = GitSourceDriver::new(
            temp.path().join("source-cache"),
            temp.path().join("frozen-source"),
            None,
        );
        driver.git_archive_into(&repository, &revision, None, &destination)?;
        driver.git_archive_into(
            &repository,
            &revision,
            Some(Path::new("vendor/fixture")),
            &destination,
        )?;
        harden_frozen_source(&destination)?;

        assert!(!destination.join(".git").exists());
        assert_eq!(
            fs::symlink_metadata(destination.join("deployment.toml"))?
                .permissions()
                .mode()
                & 0o777,
            0o444
        );
        let script_metadata = fs::symlink_metadata(destination.join("build.sh"))?;
        assert_eq!(script_metadata.uid(), 0);
        assert_eq!(script_metadata.permissions().mode() & 0o777, 0o555);
        assert!(destination.join("vendor/fixture/deployment.toml").is_file());
        validate_frozen_source_symlink(&destination, &destination.join("links/deployment"))?;
        validate_frozen_source(&destination)?;
        let digest = frozen_source_sha256(&destination)?;
        let recipe = destination.join("deployment.toml");
        fs::set_permissions(&recipe, fs::Permissions::from_mode(0o644))?;
        assert!(validate_frozen_source(&destination).is_err());
        fs::set_permissions(&recipe, fs::Permissions::from_mode(0o444))?;
        fs::write(&recipe, b"target = 'changed'\n")?;
        assert_ne!(frozen_source_sha256(&destination)?, digest);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn frozen_source_rejects_a_symlink_that_escapes_the_transaction_tree() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempfile::tempdir()?;
        let parent = temp.path().join("transaction");
        fs::create_dir(&parent)?;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
        let destination = parent.join("source");
        prepare_frozen_source_destination(&destination)?;
        symlink("../../outside", destination.join("escape"))?;
        assert!(harden_frozen_source(&destination).is_err());
        Ok(())
    }
}

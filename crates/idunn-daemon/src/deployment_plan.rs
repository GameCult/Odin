use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{bail, ensure, Context, Result};
pub use cultnet_rs::IdunnExpectedIncarnationRecord as ExpectedIncarnation;
use cultnet_rs::{
    IdunnExpectedCapability, IdunnExpectedDependency, IdunnExpectedRoute,
    IDUNN_EXPECTED_INCARNATION_SCHEMA,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::control_plane::SequenceAdmittedReady;
use crate::deployment::{
    capability_compatible, CapabilityDependency, DependencyKind, ExternalCapabilityBinding,
    OperatorBinding, ServiceTransport, SourceSelectionPolicy, StartupOrder, StateDeclaration,
    TargetDeclaration,
};

pub const SOURCE_SELECTION_FACTS_SCHEMA: &str = "idunn.source_selection_facts.v1";
pub const COMPILED_DEPLOYMENT_PLAN_SCHEMA: &str = "idunn.compiled_deployment_plan.v2";
pub const SEALED_RELEASE_SCHEMA: &str = "idunn.sealed_release.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitlinkTreeFact {
    pub origin: String,
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SourceSelection {
    PinnedObject,
    RefHead,
    SignedRelease { release_authority_id: String },
}

/// Private facts emitted by the future narrow source driver. Structural
/// validation here does not prove Git ancestry, signatures, or object custody.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSelectionFacts {
    pub schema: String,
    pub origin: String,
    pub admitted_ref: String,
    pub admitted_ref_revision: String,
    pub revision: String,
    pub source_tree: String,
    pub recipe_path: PathBuf,
    pub recipe_blob_sha256: String,
    pub gitlinks: BTreeMap<PathBuf, GitlinkTreeFact>,
    pub selection: SourceSelection,
    pub selected_at_unix_millis: u64,
}

impl SourceSelectionFacts {
    pub fn validate_against(&self, binding: &OperatorBinding) -> Result<()> {
        binding.validate()?;
        ensure!(
            self.schema == SOURCE_SELECTION_FACTS_SCHEMA,
            "unsupported source-selection facts schema"
        );
        ensure!(
            self.origin == binding.repository.origin,
            "selected origin differs from binding"
        );
        ensure!(
            self.admitted_ref == binding.repository.admitted_ref,
            "selected ref differs from binding"
        );
        require_sha1(
            &self.admitted_ref_revision,
            "selected admitted-ref revision",
        )?;
        require_sha1(&self.revision, "selected revision")?;
        require_sha1(&self.source_tree, "selected source tree")?;
        ensure!(
            self.recipe_path == binding.repository.recipe_path,
            "selected recipe path differs from binding"
        );
        require_sha256(&self.recipe_blob_sha256, "selected recipe blob")?;
        ensure!(
            self.selected_at_unix_millis > 0,
            "source-selection facts have no time"
        );

        match (&binding.repository.selection, &self.selection) {
            (SourceSelectionPolicy::PinnedObject, SourceSelection::PinnedObject) => {
                let pinned_revision = binding
                    .repository
                    .pinned_revision
                    .as_deref()
                    .context("validated pinned binding has no revision")?;
                ensure!(
                    self.revision == pinned_revision,
                    "selected source differs from the pinned revision"
                );
            }
            (SourceSelectionPolicy::RefHead, SourceSelection::RefHead) => ensure!(
                self.revision == self.admitted_ref_revision,
                "ref-head selection differs from the fetched admitted-ref revision"
            ),
            (
                SourceSelectionPolicy::SignedRelease,
                SourceSelection::SignedRelease {
                    release_authority_id,
                },
            ) => {
                require_token(release_authority_id, "release authority id")?;
            }
            _ => bail!("source-selection facts do not match operator policy"),
        }

        let expected_paths: BTreeSet<_> = binding.repository.gitlinks.keys().cloned().collect();
        let observed_paths: BTreeSet<_> = self.gitlinks.keys().cloned().collect();
        ensure!(
            expected_paths == observed_paths,
            "Gitlink tree facts differ from binding"
        );
        for (path, receipt) in &self.gitlinks {
            let expected = &binding.repository.gitlinks[path];
            ensure!(
                receipt.origin == expected.origin,
                "Gitlink {} origin differs from binding",
                path.display()
            );
            require_sha1(&receipt.revision, "Gitlink revision")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DependencyProviderAuthority {
    ManagedReady {
        target: String,
        incarnation_id: String,
        plan_id: String,
        sealed_release_id: String,
        expected_projection_sha256: String,
        odin_topology_correlation_sha256: String,
        odin_topology_publisher_sequence: u64,
    },
    ExternalOperatorBinding {
        binding_target: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyProviderRef {
    pub provider_id: String,
    pub authority: DependencyProviderAuthority,
    pub capability: String,
    pub schema: String,
    pub compatibility: String,
    pub capacity: u32,
    pub endpoint: Option<String>,
}

impl DependencyProviderRef {
    pub fn validate(&self) -> Result<()> {
        require_token(&self.provider_id, "dependency provider id")?;
        require_contract(&self.capability, &self.schema, &self.compatibility)?;
        ensure!(self.capacity > 0, "dependency provider capacity is zero");
        if let Some(endpoint) = &self.endpoint {
            require_value(endpoint, "dependency provider endpoint")?;
        }
        match &self.authority {
            DependencyProviderAuthority::ManagedReady {
                target,
                incarnation_id,
                plan_id,
                sealed_release_id,
                expected_projection_sha256,
                odin_topology_correlation_sha256,
                odin_topology_publisher_sequence,
            } => {
                require_token(target, "managed provider target")?;
                require_token(incarnation_id, "managed provider incarnation")?;
                require_sha256(plan_id, "managed provider plan")?;
                require_sha256(sealed_release_id, "managed provider sealed release")?;
                require_sha256(expected_projection_sha256, "managed expected projection")?;
                require_sha256(
                    odin_topology_correlation_sha256,
                    "managed Odin topology correlation",
                )?;
                ensure!(
                    *odin_topology_publisher_sequence > 0,
                    "managed Odin topology sequence is zero"
                );
            }
            DependencyProviderAuthority::ExternalOperatorBinding { binding_target } => {
                require_token(binding_target, "external binding target")?;
                ensure!(
                    self.endpoint.is_some(),
                    "external binding has no configured endpoint"
                );
            }
        }
        Ok(())
    }

    fn compatible_with(&self, dependency: &CapabilityDependency) -> bool {
        capability_compatible(
            &dependency.capability,
            &dependency.schema,
            &dependency.compatibility,
            &self.capability,
            &self.schema,
            &self.compatibility,
        ) && self.capacity >= dependency.minimum_capacity
    }

    fn is_external_binding(&self) -> bool {
        matches!(
            &self.authority,
            DependencyProviderAuthority::ExternalOperatorBinding { .. }
        )
    }
}

pub fn external_binding_provider_refs(
    binding: &OperatorBinding,
) -> Result<Vec<DependencyProviderRef>> {
    binding.validate()?;
    let providers = binding
        .external_capabilities
        .iter()
        .map(|capability| expected_external_provider(&binding.target, capability))
        .collect::<Vec<_>>();
    for provider in &providers {
        provider.validate()?;
    }
    Ok(providers)
}

fn expected_external_provider(
    binding_target: &str,
    capability: &ExternalCapabilityBinding,
) -> DependencyProviderRef {
    DependencyProviderRef {
        provider_id: capability.provider_id.clone(),
        authority: DependencyProviderAuthority::ExternalOperatorBinding {
            binding_target: binding_target.to_owned(),
        },
        capability: capability.capability.clone(),
        schema: capability.schema.clone(),
        compatibility: capability.compatibility.clone(),
        capacity: capability.capacity,
        endpoint: Some(capability.endpoint.clone()),
    }
}

/// Projects one authenticated Ready correlation into dependency-selection
/// inputs. Authentication is not replay admission: the control-plane store
/// must atomically admit `publisher_sequence` before calling this function and
/// must re-check the selected receipt before promotion.
fn managed_ready_provider_refs(
    admitted: &SequenceAdmittedReady,
) -> Result<Vec<DependencyProviderRef>> {
    let expected = admitted.expected();
    let topology = admitted.authenticated();
    expected.validate()?;
    let expected_projection_sha256 = expected.canonical_sha256()?;
    let observed = topology.record();
    ensure!(
        observed.expected_projection_sha256 == expected_projection_sha256,
        "Ready provider topology names a different Expected projection"
    );
    ensure!(
        observed.target == expected.target && observed.runtime_id == expected.runtime_id,
        "Ready provider topology names a different target or runtime"
    );
    ensure!(
        observed.expected
            && observed.present
            && observed.ready
            && observed.observed_presence_state.as_deref() == Some("active")
            && observed.disagreements.is_empty(),
        "managed provider topology is not Ready"
    );
    let odin_topology_correlation_sha256 = admitted.evidence_sha256().to_owned();
    let endpoint = expected
        .route
        .as_ref()
        .map(|route| route.candidate_endpoint.clone());
    let mut providers = Vec::new();
    for capability in &observed.observed_capabilities {
        providers.push(DependencyProviderRef {
            provider_id: observed.runtime_id.clone(),
            authority: DependencyProviderAuthority::ManagedReady {
                target: expected.target.clone(),
                incarnation_id: expected.incarnation_id.clone(),
                plan_id: expected.plan_id.clone(),
                sealed_release_id: expected.sealed_release_id.clone(),
                expected_projection_sha256: expected_projection_sha256.clone(),
                odin_topology_correlation_sha256: odin_topology_correlation_sha256.clone(),
                odin_topology_publisher_sequence: admitted.publisher_sequence(),
            },
            capability: capability.capability.clone(),
            schema: capability.schema.clone(),
            compatibility: capability.compatibility.clone(),
            capacity: capability.capacity,
            endpoint: endpoint.clone(),
        });
    }
    for provider in &providers {
        provider.validate()?;
    }
    Ok(providers)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySelection {
    pub requirement: CapabilityDependency,
    pub provider: Option<DependencyProviderRef>,
}

impl DependencySelection {
    fn validate(&self) -> Result<()> {
        require_contract(
            &self.requirement.capability,
            &self.requirement.schema,
            &self.requirement.compatibility,
        )?;
        ensure!(
            self.requirement.minimum_capacity > 0,
            "dependency capacity is zero"
        );
        if let Some(provider) = &self.provider {
            provider.validate()?;
            ensure!(
                provider.compatible_with(&self.requirement),
                "selected provider is incompatible"
            );
            ensure!(
                (self.requirement.kind == DependencyKind::ExternalOperatorBinding)
                    == provider.is_external_binding(),
                "selected provider authority does not match dependency kind"
            );
        } else {
            ensure!(
                self.requirement.kind == DependencyKind::Optional,
                "required dependency is unresolved"
            );
        }
        Ok(())
    }

    fn expected_projection(&self) -> Result<IdunnExpectedDependency> {
        self.validate()?;
        let (
            provider_id,
            provider_authority,
            provider_expected_projection_sha256,
            provider_endpoint,
        ) = match &self.provider {
            None => (None, None, None, None),
            Some(provider) => match &provider.authority {
                DependencyProviderAuthority::ManagedReady {
                    expected_projection_sha256,
                    ..
                } => (
                    Some(provider.provider_id.clone()),
                    Some("managed-incarnation".into()),
                    Some(expected_projection_sha256.clone()),
                    provider.endpoint.clone(),
                ),
                DependencyProviderAuthority::ExternalOperatorBinding { .. } => (
                    Some(provider.provider_id.clone()),
                    Some("external-operator-binding".into()),
                    None,
                    provider.endpoint.clone(),
                ),
            },
        };
        Ok(IdunnExpectedDependency {
            kind: dependency_kind_name(self.requirement.kind).into(),
            capability: self.requirement.capability.clone(),
            schema: self.requirement.schema.clone(),
            compatibility: self.requirement.compatibility.clone(),
            minimum_capacity: self.requirement.minimum_capacity,
            startup: startup_order_name(self.requirement.startup).into(),
            provider_id,
            provider_authority,
            provider_expected_projection_sha256,
            provider_endpoint,
        })
    }
}

pub fn select_dependencies(
    declaration: &TargetDeclaration,
    providers: &[DependencyProviderRef],
) -> Result<Vec<DependencySelection>> {
    declaration.validate()?;
    let mut unique_contracts = BTreeSet::new();
    for provider in providers {
        provider.validate()?;
        ensure!(
            unique_contracts.insert((
                provider.provider_id.as_str(),
                provider.capability.as_str(),
                provider.schema.as_str(),
                provider.compatibility.as_str(),
            )),
            "dependency provider contract is duplicated"
        );
    }
    for conflict in &declaration.conflicts {
        if providers
            .iter()
            .any(|provider| provider.capability == conflict.capability)
        {
            bail!(
                "capability conflict {}: {}",
                conflict.capability,
                conflict.reason
            );
        }
    }
    declaration
        .dependencies
        .iter()
        .map(|dependency| {
            let mut candidates = providers
                .iter()
                .filter(|provider| provider.compatible_with(dependency))
                .filter(|provider| {
                    (dependency.kind == DependencyKind::ExternalOperatorBinding)
                        == provider.is_external_binding()
                })
                .cloned()
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| {
                left.provider_id
                    .cmp(&right.provider_id)
                    .then_with(|| left.endpoint.cmp(&right.endpoint))
            });
            let provider = candidates.into_iter().next();
            if provider.is_none() && dependency.kind != DependencyKind::Optional {
                bail!(
                    "no expected provider satisfies {} {} {}",
                    dependency.capability,
                    dependency.schema,
                    dependency.compatibility
                );
            }
            let selection = DependencySelection {
                requirement: dependency.clone(),
                provider,
            };
            selection.validate()?;
            Ok(selection)
        })
        .collect()
}

/// Private Idunn control-plane state. This contains host binding details and is
/// never an Odin/CultMesh projection. The shared CultNet Expected incarnation
/// is the sanitized topology projection derived only after a release validates
/// against this plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledDeploymentPlan {
    pub schema: String,
    pub plan_id: String,
    pub incarnation_id: String,
    pub created_at_unix_millis: u64,
    pub source: SourceSelectionFacts,
    pub recipe_blob: Vec<u8>,
    pub binding_blob: Vec<u8>,
    pub dependencies: Vec<DependencySelection>,
    pub candidate_port: Option<u16>,
}

impl CompiledDeploymentPlan {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == COMPILED_DEPLOYMENT_PLAN_SCHEMA,
            "unsupported deployment-plan schema"
        );
        require_sha256(&self.plan_id, "plan id")?;
        require_token(&self.incarnation_id, "plan incarnation")?;
        ensure!(
            self.created_at_unix_millis >= self.source.selected_at_unix_millis,
            "deployment plan predates source selection"
        );
        let (declaration, binding) = self.parsed_inputs()?;
        self.source.validate_against(&binding)?;
        ensure!(
            sha256_id(&self.recipe_blob) == self.source.recipe_blob_sha256,
            "selected recipe blob differs from the exact plan recipe bytes"
        );
        let declared_requirements: Vec<_> = declaration.dependencies.iter().collect();
        let selected_requirements: Vec<_> = self
            .dependencies
            .iter()
            .map(|selection| &selection.requirement)
            .collect();
        ensure!(
            declared_requirements == selected_requirements,
            "selected graph differs from recipe dependencies"
        );
        for selection in &self.dependencies {
            selection.validate()?;
        }
        match (&binding.route, self.candidate_port) {
            (Some(bound), Some(port)) => {
                ensure!(
                    (bound.private_port_start..=bound.private_port_end).contains(&port),
                    "candidate port is outside the operator range"
                );
            }
            (None, None) => {}
            (Some(_), None) => bail!("routed deployment has no candidate port"),
            (None, Some(_)) => bail!("private deployment selected a routed candidate port"),
        }
        ensure!(
            self.plan_id == self.recomputed_plan_id()?,
            "deployment plan digest is not canonical"
        );
        Ok(())
    }

    pub(crate) fn parsed_inputs(&self) -> Result<(TargetDeclaration, OperatorBinding)> {
        let recipe_text = std::str::from_utf8(&self.recipe_blob)
            .context("stored deployment recipe is not UTF-8")?;
        let binding_text = std::str::from_utf8(&self.binding_blob)
            .context("stored operator binding is not UTF-8")?;
        let declaration = TargetDeclaration::parse(recipe_text)?;
        let binding = OperatorBinding::parse(binding_text)?;
        binding.admit(&declaration)?;
        Ok((declaration, binding))
    }

    fn recomputed_plan_id(&self) -> Result<String> {
        let mut unsigned = self.clone();
        unsigned.plan_id.clear();
        Ok(sha256_id(
            &rmp_serde::to_vec(&unsigned).context("encoding deployment plan identity")?,
        ))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_deployment_plan(
    recipe_bytes: &[u8],
    binding_bytes: &[u8],
    source: SourceSelectionFacts,
    incarnation_id: impl Into<String>,
    candidate_port: Option<u16>,
    created_at_unix_millis: u64,
    managed_ready_providers: &[SequenceAdmittedReady],
) -> Result<CompiledDeploymentPlan> {
    let recipe_text = std::str::from_utf8(recipe_bytes).context("recipe is not UTF-8")?;
    let binding_text = std::str::from_utf8(binding_bytes).context("binding is not UTF-8")?;
    let declaration = TargetDeclaration::parse(recipe_text)?;
    let binding = OperatorBinding::parse(binding_text)?;
    binding.admit(&declaration)?;
    source.validate_against(&binding)?;
    ensure!(
        sha256_id(recipe_bytes) == source.recipe_blob_sha256,
        "selected recipe blob differs from compiled recipe bytes"
    );
    let incarnation_id = incarnation_id.into();
    require_token(&incarnation_id, "deployment incarnation")?;
    ensure!(
        created_at_unix_millis >= source.selected_at_unix_millis,
        "deployment plan predates source selection"
    );
    let mut providers = Vec::new();
    for provider in managed_ready_providers {
        providers.extend(managed_ready_provider_refs(provider)?);
    }
    providers.extend(external_binding_provider_refs(&binding)?);
    let dependencies = select_dependencies(&declaration, &providers)?;
    let mut plan = CompiledDeploymentPlan {
        schema: COMPILED_DEPLOYMENT_PLAN_SCHEMA.into(),
        plan_id: String::new(),
        incarnation_id,
        created_at_unix_millis,
        source,
        recipe_blob: recipe_bytes.to_vec(),
        binding_blob: binding_bytes.to_vec(),
        dependencies,
        candidate_port,
    };
    plan.plan_id = plan.recomputed_plan_id()?;
    plan.validate()?;
    Ok(plan)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactReceipt {
    pub artifact_id: String,
    pub destination: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub executable: bool,
}

impl ArtifactReceipt {
    fn validate(&self, declaration: &TargetDeclaration) -> Result<()> {
        require_token(&self.artifact_id, "artifact receipt id")?;
        require_sha256(&self.sha256, "artifact digest")?;
        ensure!(self.size_bytes > 0, "sealed artifact is empty");
        let declared = declaration
            .artifacts
            .iter()
            .find(|artifact| artifact.id == self.artifact_id)
            .with_context(|| format!("plan declares no artifact {}", self.artifact_id))?;
        ensure!(
            self.destination == declared.destination && self.executable == declared.executable,
            "artifact receipt differs from declared output"
        );
        if let Some(expected_sha256) = &declared.expected_sha256 {
            ensure!(
                self.sha256 == prefixed_sha256(expected_sha256),
                "artifact receipt differs from the recipe-pinned digest"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalInputMaterializationReceipt {
    pub input_id: String,
    pub url: String,
    pub sha256: String,
    pub runner: String,
    pub destination: PathBuf,
    pub size_bytes: u64,
}

impl ExternalInputMaterializationReceipt {
    fn validate(&self, declaration: &TargetDeclaration) -> Result<()> {
        require_token(&self.input_id, "external input receipt id")?;
        require_sha256(&self.sha256, "external input materialized digest")?;
        ensure!(self.size_bytes > 0, "materialized external input is empty");
        let declared = declaration
            .external_inputs
            .iter()
            .find(|input| input.id == self.input_id)
            .with_context(|| format!("plan declares no external input {}", self.input_id))?;
        ensure!(
            self.url == declared.url
                && self.sha256 == prefixed_sha256(&declared.sha256)
                && self.runner == declared.runner
                && self.destination == declared.destination,
            "external input receipt differs from its pinned declaration"
        );
        Ok(())
    }
}

/// Private Idunn release state. The content address covers the exact plan ID,
/// sorted full artifact receipts, and sorted full external-input
/// materialization receipts. The plan remains a separate private record and is
/// supplied explicitly for validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedRelease {
    pub schema: String,
    pub sealed_release_id: String,
    pub plan_id: String,
    pub artifacts: Vec<ArtifactReceipt>,
    pub external_inputs: Vec<ExternalInputMaterializationReceipt>,
    pub sealed_at_unix_millis: u64,
}

impl SealedRelease {
    pub fn new(
        plan: &CompiledDeploymentPlan,
        mut artifacts: Vec<ArtifactReceipt>,
        mut external_inputs: Vec<ExternalInputMaterializationReceipt>,
        sealed_at_unix_millis: u64,
    ) -> Result<Self> {
        plan.validate()?;
        artifacts.sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
        external_inputs.sort_by(|left, right| left.input_id.cmp(&right.input_id));
        let mut release = Self {
            schema: SEALED_RELEASE_SCHEMA.into(),
            sealed_release_id: String::new(),
            plan_id: plan.plan_id.clone(),
            artifacts,
            external_inputs,
            sealed_at_unix_millis,
        };
        release.validate_contents(plan)?;
        release.sealed_release_id = release.recomputed_release_id()?;
        release.validate_against(plan)?;
        Ok(release)
    }

    pub fn validate_against(&self, plan: &CompiledDeploymentPlan) -> Result<()> {
        require_sha256(&self.sealed_release_id, "sealed release id")?;
        self.validate_contents(plan)?;
        ensure!(
            self.sealed_release_id == self.recomputed_release_id()?,
            "sealed release content address is wrong"
        );
        Ok(())
    }

    pub fn expected_projection(
        &self,
        plan: &CompiledDeploymentPlan,
    ) -> Result<ExpectedIncarnation> {
        self.validate_against(plan)?;
        let (declaration, binding) = plan.parsed_inputs()?;
        let route = match (&binding.route, plan.candidate_port) {
            (Some(bound), Some(port)) => Some(IdunnExpectedRoute {
                route_id: bound.route_id.clone(),
                transport: endpoint_scheme(declaration.service.transport)?.into(),
                stable_endpoint: bound.stable_endpoint.clone(),
                candidate_endpoint: format!(
                    "{}://{}:{port}",
                    endpoint_scheme(declaration.service.transport)?,
                    bound.private_host
                ),
            }),
            (None, None) => None,
            _ => bail!("plan route and candidate port disagree"),
        };
        let artifact_sha256 = self
            .artifacts
            .iter()
            .find(|artifact| artifact.artifact_id == declaration.service.executable_artifact)
            .context("sealed release has no executable service artifact")?
            .sha256
            .clone();
        let mut capabilities = declaration
            .provides
            .iter()
            .map(|capability| IdunnExpectedCapability {
                capability: capability.capability.clone(),
                schema: capability.schema.clone(),
                compatibility: capability.compatibility.clone(),
                minimum_capacity: capability.capacity,
            })
            .collect::<Vec<_>>();
        capabilities.sort_by(|left, right| {
            (&left.capability, &left.schema, &left.compatibility).cmp(&(
                &right.capability,
                &right.schema,
                &right.compatibility,
            ))
        });
        let mut dependencies = plan
            .dependencies
            .iter()
            .map(DependencySelection::expected_projection)
            .collect::<Result<Vec<_>>>()?;
        dependencies.sort_by(|left, right| {
            (&left.capability, &left.schema, &left.compatibility).cmp(&(
                &right.capability,
                &right.schema,
                &right.compatibility,
            ))
        });
        let expected = ExpectedIncarnation {
            schema_version: IDUNN_EXPECTED_INCARNATION_SCHEMA.into(),
            target: declaration.target.clone(),
            plan_id: plan.plan_id.clone(),
            incarnation_id: plan.incarnation_id.clone(),
            sealed_release_id: self.sealed_release_id.clone(),
            source_repository: canonical_repository_identity(&plan.source.origin)?,
            source_revision: plan.source.revision.clone(),
            recipe_sha256: plan.source.recipe_blob_sha256.clone(),
            runtime_id: binding.runtime_identity.runtime_id.clone(),
            expected_signer_identity_id: binding
                .runtime_identity
                .expected_signer_identity_id
                .clone(),
            health_contract: declaration.service.health.contract.clone(),
            artifact_sha256,
            state_schema_generation: declaration
                .state
                .as_ref()
                .map(|state| state.schema_generation.clone()),
            state_contract_sha256: declaration
                .state
                .as_ref()
                .map(canonical_state_contract_sha256)
                .transpose()?,
            write_lease_required: declaration.write_lease_required(),
            route,
            capabilities,
            dependencies,
        };
        expected.validate()?;
        Ok(expected)
    }

    fn validate_contents(&self, plan: &CompiledDeploymentPlan) -> Result<()> {
        ensure!(
            self.schema == SEALED_RELEASE_SCHEMA,
            "unsupported sealed-release schema"
        );
        plan.validate()?;
        let (declaration, _) = plan.parsed_inputs()?;
        require_sha256(&self.plan_id, "sealed plan id")?;
        ensure!(
            self.plan_id == plan.plan_id,
            "sealed release refers to a different plan"
        );
        ensure!(
            self.sealed_at_unix_millis >= plan.created_at_unix_millis,
            "release was sealed before it was planned"
        );
        ensure!(
            self.artifacts
                .windows(2)
                .all(|pair| pair[0].artifact_id < pair[1].artifact_id),
            "artifact receipts are not in canonical order"
        );
        ensure!(
            self.external_inputs
                .windows(2)
                .all(|pair| pair[0].input_id < pair[1].input_id),
            "external input receipts are not in canonical order"
        );
        let declared_ids = declaration
            .artifacts
            .iter()
            .map(|artifact| artifact.id.as_str())
            .collect::<BTreeSet<_>>();
        let observed_ids = self
            .artifacts
            .iter()
            .map(|artifact| artifact.artifact_id.as_str())
            .collect::<BTreeSet<_>>();
        ensure!(
            declared_ids == observed_ids && observed_ids.len() == self.artifacts.len(),
            "artifact receipts differ from declared outputs"
        );
        for artifact in &self.artifacts {
            artifact.validate(&declaration)?;
        }
        let declared_input_ids = declaration
            .external_inputs
            .iter()
            .map(|input| input.id.as_str())
            .collect::<BTreeSet<_>>();
        let observed_input_ids = self
            .external_inputs
            .iter()
            .map(|input| input.input_id.as_str())
            .collect::<BTreeSet<_>>();
        ensure!(
            declared_input_ids == observed_input_ids
                && observed_input_ids.len() == self.external_inputs.len(),
            "external input receipts differ from pinned declarations"
        );
        for input in &self.external_inputs {
            input.validate(&declaration)?;
        }
        Ok(())
    }

    fn recomputed_release_id(&self) -> Result<String> {
        Ok(sha256_id(
            &rmp_serde::to_vec(&(
                self.schema.as_str(),
                self.plan_id.as_str(),
                &self.artifacts,
                &self.external_inputs,
            ))
            .context("encoding sealed release identity")?,
        ))
    }
}

fn canonical_state_contract_sha256(state: &StateDeclaration) -> Result<String> {
    Ok(sha256_id(
        &rmp_serde::to_vec(state).context("encoding canonical state contract")?,
    ))
}

fn endpoint_scheme(transport: ServiceTransport) -> Result<&'static str> {
    match transport {
        ServiceTransport::Http => Ok("http"),
        ServiceTransport::Tcp => Ok("tcp"),
        ServiceTransport::Rudp | ServiceTransport::Private => {
            bail!("service transport has no stable route scheme")
        }
    }
}

fn dependency_kind_name(kind: DependencyKind) -> &'static str {
    match kind {
        DependencyKind::Bootstrap => "bootstrap",
        DependencyKind::Required => "required",
        DependencyKind::Optional => "optional",
        DependencyKind::SharedInfrastructure => "shared-infrastructure",
        DependencyKind::Private => "private",
        DependencyKind::ExternalOperatorBinding => "external-operator-binding",
    }
}

fn startup_order_name(startup: StartupOrder) -> &'static str {
    match startup {
        StartupOrder::BeforePromotion => "before-promotion",
        StartupOrder::BeforeStart => "before-start",
    }
}

fn canonical_repository_identity(origin: &str) -> Result<String> {
    let identity = origin
        .strip_prefix("https://")
        .and_then(|origin| origin.strip_suffix(".git"))
        .context("validated Git origin is not a canonical HTTPS repository")?;
    Ok(identity.to_owned())
}

fn prefixed_sha256(raw_sha256: &str) -> String {
    format!("sha256-{raw_sha256}")
}

fn require_token(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= 128,
        "{label} is empty or too long"
    );
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')),
        "{label} contains a forbidden character"
    );
    Ok(())
}

fn require_contract(capability: &str, schema: &str, compatibility: &str) -> Result<()> {
    require_token(capability, "capability")?;
    require_token(schema, "capability schema")?;
    require_token(compatibility, "capability compatibility")
}

fn require_value(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value == value.trim(),
        "{label} is empty or noncanonical"
    );
    ensure!(
        value.len() <= 4096 && !value.contains('\0'),
        "{label} is oversized or contains NUL"
    );
    Ok(())
}

fn require_sha1(value: &str, label: &str) -> Result<()> {
    require_lower_hex(value, 40, label)
}

fn require_sha256(value: &str, label: &str) -> Result<()> {
    let digest = value
        .strip_prefix("sha256-")
        .with_context(|| format!("{label} has no sha256- prefix"))?;
    require_lower_hex(digest, 64, label)
}

fn require_lower_hex(value: &str, length: usize, label: &str) -> Result<()> {
    ensure!(value.len() == length, "{label} has the wrong length");
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} is not lowercase hexadecimal"
    );
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use cultnet_rs::{
        authenticate_odin_runtime_topology_correlation, enroll_service_identity_at,
        verify_runtime_authority, GameCultProviderHealthIdentity, GameCultRuntimeCapability,
        IdunnRuntimeActivationLaunch, IdunnServiceIdentity, OdinRuntimeTopologyCorrelationPurpose,
        OdinRuntimeTopologyCorrelationRecord, OdinTopologyAuthenticationContext,
        OdinTopologyIdentity, ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA,
    };

    const RECIPE: &str = r#"
schema = "gamecult.idunn.target_declaration.v1"
target = "service"
source_stamp_environment = "SERVICE_BUILD_COMMIT"

[[steps]]
id = "build"
phase = "build"
runner = "rust"
argv = ["cargo", "build", "--locked"]

[[external_inputs]]
id = "toolchain-index"
url = "https://example.invalid/toolchain-index"
sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
runner = "rust"
destination = "inputs/toolchain-index"

[[artifacts]]
id = "daemon"
source_kind = "runner-output"
runner = "rust"
source = "target/release/service"
destination = "service"
expected_sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
executable = true

[service]
executable_artifact = "daemon"
arguments = [
  { kind = "binding", name = "state_root" },
]
transport = "http"
route_required = true
required_environment = ["GAMECULT_IDUNN_CANDIDATE_BIND", "GAMECULT_IDUNN_RUNTIME_BUNDLE"]

[service.health]
contract = "service.health"

[state]
schema_generation = "v1"

[[state.slots]]
id = "runtime-cache"
relative_path = "runtime-cache.cc"
kind = "cultcache-file"
schema = "service.runtime-cache.v1"
writer = "none"
recovery = "rebuildable"
startup = "open-at-start"

[[provides]]
capability = "service.runtime"
schema = "service.runtime.v1"
compatibility = "v1"

[[dependencies]]
kind = "shared-infrastructure"
capability = "odin.verse-rendezvous"
schema = "odin.verse-topology.v1"
compatibility = "v1"
"#;

    const BINDING: &str = r#"
schema = "gamecult.idunn.operator_binding.v2"
target = "service"

[repository]
origin = "https://github.com/GameCult/Service.git"
admitted_ref = "refs/heads/main"
minimum_revision = "1111111111111111111111111111111111111111"
selection = "ref-head"
checkout = "/srv/build/Service"
recipe_path = "deployment/idunn/recipe.toml"

[runners.rust]
driver = "docker"
image = "rust@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
user = "1000:1000"
affordances = ["source-read", "artifact-write"]
allowed_programs = ["cargo"]
network_profile = "build-dependency-egress"
memory_mebibytes = 2048
cpu_quota_percent = 200
pids_limit = 512
tmpfs_mebibytes = 512

[workload]
driver = "systemd-transient"
state_group = "service"
unit_prefix = "idunn-service"
release_root = "/srv/service/releases"
state_root = "/var/lib/gamecult/service"
runtime_root = "/run/gamecult/idunn/service"
network = "host-private"
hardening = "strict"
memory_mebibytes = 1024
cpu_quota_percent = 100

[workload.argument_bindings]
state_root = "/var/lib/gamecult/service"

[workload.secret_files]
GAMECULT_RUNTIME_PRESENCE_IDENTITY = "/etc/gamecult/service/runtime-presence-identity.cc"

[runtime_identity]
runtime_id = "service-yggdrasil"
expected_signer_identity_id = "service-runtime-signer"
trust_anchor_store = "/etc/gamecult/trust/service.cc"

[route]
driver = "nginx-http"
route_id = "service"
stable_endpoint = "https://example.invalid/service/"
private_host = "127.0.0.1"
private_port_start = 18000
private_port_end = 18009
config_path = "/etc/nginx/idunn-routes/service.conf"
reload_unit = "nginx.service"

[brakes]
deployment_store = "/var/lib/gamecult/idunn/service-deployment-brake.cc"
lifecycle_store = "/var/lib/gamecult/idunn/service-lifecycle-brake.cc"

[rollout]
strategy = "candidate-then-promote"
drain_seconds = 30
retain_releases = 2

[placement]
desired_replicas = 1
nodes = ["yggdrasil"]
"#;

    fn digest(byte: u8) -> String {
        format!("sha256-{}", char::from(byte).to_string().repeat(64))
    }

    fn source(recipe: &str) -> SourceSelectionFacts {
        SourceSelectionFacts {
            schema: SOURCE_SELECTION_FACTS_SCHEMA.into(),
            origin: "https://github.com/GameCult/Service.git".into(),
            admitted_ref: "refs/heads/main".into(),
            admitted_ref_revision: "2222222222222222222222222222222222222222".into(),
            revision: "2222222222222222222222222222222222222222".into(),
            source_tree: "3333333333333333333333333333333333333333".into(),
            recipe_path: "deployment/idunn/recipe.toml".into(),
            recipe_blob_sha256: sha256_id(recipe.as_bytes()),
            gitlinks: BTreeMap::new(),
            selection: SourceSelection::RefHead,
            selected_at_unix_millis: 100,
        }
    }

    fn odin() -> ExpectedIncarnation {
        ExpectedIncarnation {
            schema_version: IDUNN_EXPECTED_INCARNATION_SCHEMA.into(),
            target: "odin".into(),
            plan_id: digest(b'a'),
            incarnation_id: "odin-incarnation-1".into(),
            sealed_release_id: digest(b'e'),
            source_repository: "github.com/GameCult/Odin".into(),
            source_revision: "2222222222222222222222222222222222222222".into(),
            recipe_sha256: digest(b'b'),
            runtime_id: "odin-yggdrasil".into(),
            expected_signer_identity_id: "odin-runtime-signer".into(),
            health_contract: "odin.cultnet-service-health".into(),
            artifact_sha256: digest(b'f'),
            state_schema_generation: None,
            state_contract_sha256: None,
            write_lease_required: false,
            route: Some(IdunnExpectedRoute {
                route_id: "odin-private".into(),
                transport: "tcp".into(),
                stable_endpoint: "tcp://10.77.0.1:17871".into(),
                candidate_endpoint: "tcp://127.0.0.1:17871".into(),
            }),
            capabilities: vec![IdunnExpectedCapability {
                capability: "odin.verse-rendezvous".into(),
                schema: "odin.verse-topology.v1".into(),
                compatibility: "v1".into(),
                minimum_capacity: 1,
            }],
            dependencies: Vec::new(),
        }
    }

    fn ready_odin_provider(runtime_id: &str, capacity: u32) -> SequenceAdmittedReady {
        (|| -> Result<SequenceAdmittedReady> {
            let root = tempfile::tempdir()?;
            let provider = enroll_service_identity_at::<GameCultProviderHealthIdentity>(
                &root.path().join("provider.cc"),
            )?;
            let idunn =
                enroll_service_identity_at::<IdunnServiceIdentity>(&root.path().join("idunn.cc"))?;
            let odin_signer =
                enroll_service_identity_at::<OdinTopologyIdentity>(&root.path().join("odin.cc"))?;
            let mut expected = odin();
            expected.runtime_id = runtime_id.into();
            expected.expected_signer_identity_id = provider.entry().identity_id.clone();
            let launch = IdunnRuntimeActivationLaunch::issue(&expected, digest(b'8'), 100, &idunn)?;
            let activation = launch.activation().clone();
            launch.write_credential(std::io::sink())?;
            let authority = verify_runtime_authority(
                &expected,
                &activation,
                &idunn.trust_anchor()?,
                &provider.entry().public_key,
            )?;
            let mut topology = OdinRuntimeTopologyCorrelationRecord {
                schema_version: ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA.into(),
                target: expected.target.clone(),
                expected_projection_sha256: expected.canonical_sha256()?,
                expected: true,
                current_activation_sha256: Some(activation.canonical_sha256()?),
                signed_presence_sha256: Some(digest(b'9')),
                observed_presence_state: Some("active".into()),
                observed_presence_publisher_sequence: Some(1),
                observed_write_lease_sha256: None,
                observed_capabilities: vec![GameCultRuntimeCapability {
                    capability: "odin.verse-rendezvous".into(),
                    schema: "odin.verse-topology.v1".into(),
                    compatibility: "v1".into(),
                    capacity,
                }],
                runtime_id: expected.runtime_id.clone(),
                runtime_instance_id: Some(activation.runtime_instance_id.clone()),
                present: true,
                ready: true,
                dependencies: Vec::new(),
                disagreements: Vec::new(),
                signer_identity_id: odin_signer.entry().identity_id.clone(),
                publisher_sequence: 7,
                observed_at_unix_millis: 110,
                signature_algorithm: "ed25519".into(),
                signature: Vec::new(),
            };
            topology.signature = odin_signer
                .sign::<OdinRuntimeTopologyCorrelationPurpose>(
                    &topology.unsigned_signature_payload()?,
                )
                .signature;
            let bytes = topology.canonical_bytes()?;
            let authenticated = authenticate_odin_runtime_topology_correlation(
                &bytes,
                &authority,
                None,
                &odin_signer.entry().public_key,
                OdinTopologyAuthenticationContext {
                    trusted_received_at_unix_millis: 120,
                    maximum_age_millis: 30,
                    maximum_future_skew_millis: 5,
                },
            )?;
            SequenceAdmittedReady::for_test(&expected, authenticated, 120)
        })()
        .unwrap()
    }

    fn plan() -> CompiledDeploymentPlan {
        let providers = [ready_odin_provider("odin-yggdrasil", 1)];
        plan_with(&providers)
    }

    fn plan_with(providers: &[SequenceAdmittedReady]) -> CompiledDeploymentPlan {
        compile_deployment_plan(
            RECIPE.as_bytes(),
            BINDING.as_bytes(),
            source(RECIPE),
            "service-incarnation-1",
            Some(18001),
            110,
            providers,
        )
        .unwrap()
    }

    fn artifact_receipt() -> ArtifactReceipt {
        ArtifactReceipt {
            artifact_id: "daemon".into(),
            destination: "service".into(),
            sha256: digest(b'c'),
            size_bytes: 42,
            executable: true,
        }
    }

    fn external_input_receipt() -> ExternalInputMaterializationReceipt {
        ExternalInputMaterializationReceipt {
            input_id: "toolchain-index".into(),
            url: "https://example.invalid/toolchain-index".into(),
            sha256: digest(b'd'),
            runner: "rust".into(),
            destination: "inputs/toolchain-index".into(),
            size_bytes: 17,
        }
    }

    #[test]
    fn compiler_parses_the_exact_bytes_it_receipts() {
        let providers = [ready_odin_provider("odin-yggdrasil", 1)];
        let first = plan_with(&providers);
        let second = plan_with(&providers);
        assert_eq!(first, second);
        assert_eq!(first.recipe_blob.as_slice(), RECIPE.as_bytes());
        assert_eq!(first.binding_blob.as_slice(), BINDING.as_bytes());
        assert_eq!(
            sha256_id(&first.recipe_blob),
            first.source.recipe_blob_sha256
        );
        let changed = RECIPE.replace(
            "contract = \"service.health\"",
            "contract = \"service.health.v2\"",
        );
        let providers = [ready_odin_provider("odin-yggdrasil", 1)];
        assert!(compile_deployment_plan(
            changed.as_bytes(),
            BINDING.as_bytes(),
            source(RECIPE),
            "service-incarnation-2",
            Some(18002),
            111,
            &providers,
        )
        .is_err());
        let mut corrupted = first;
        corrupted
            .binding_blob
            .extend_from_slice(b"\nunknown = true\n");
        assert!(corrupted.validate().is_err());
    }

    #[test]
    fn pinned_policy_requires_the_exact_operator_pin() {
        let pinned_binding = BINDING.replace(
            "selection = \"ref-head\"",
            "selection = \"pinned-object\"\npinned_revision = \"2222222222222222222222222222222222222222\"",
        );
        let binding = OperatorBinding::parse(&pinned_binding).unwrap();
        assert!(source(RECIPE).validate_against(&binding).is_err());
        let mut facts = source(RECIPE);
        facts.selection = SourceSelection::PinnedObject;
        facts.validate_against(&binding).unwrap();
    }

    #[test]
    fn gitlink_fact_must_match_the_selected_superproject_tree() {
        let recipe = RECIPE.replace(
            "source_stamp_environment = \"SERVICE_BUILD_COMMIT\"",
            "source_stamp_environment = \"SERVICE_BUILD_COMMIT\"\nrequired_gitlinks = [\"vendor/lib\"]",
        );
        let binding_text = BINDING.replace(
            "recipe_path = \"deployment/idunn/recipe.toml\"",
            "recipe_path = \"deployment/idunn/recipe.toml\"\ngitlinks = { \"vendor/lib\" = { origin = \"https://github.com/GameCult/Lib.git\" } }",
        );
        let binding = OperatorBinding::parse(&binding_text).unwrap();
        binding
            .admit(&TargetDeclaration::parse(&recipe).unwrap())
            .unwrap();
        let mut facts = source(&recipe);
        facts.gitlinks.insert(
            "vendor/lib".into(),
            GitlinkTreeFact {
                origin: "https://github.com/GameCult/Lib.git".into(),
                revision: "4444444444444444444444444444444444444444".into(),
            },
        );
        facts.validate_against(&binding).unwrap();
        facts
            .gitlinks
            .get_mut(&PathBuf::from("vendor/lib"))
            .unwrap()
            .origin = "https://github.com/GameCult/Wrong.git".into();
        assert!(facts.validate_against(&binding).is_err());
    }

    #[test]
    fn dependency_selection_uses_observed_ready_capacity_and_stable_identity_order() {
        let declaration = TargetDeclaration::parse(RECIPE).unwrap();
        let admitted = [
            ready_odin_provider("odin-z", 4),
            ready_odin_provider("odin-yggdrasil", 2),
        ];
        let providers = admitted
            .iter()
            .flat_map(|provider| managed_ready_provider_refs(provider).unwrap())
            .collect::<Vec<_>>();
        let selected = select_dependencies(&declaration, &providers).unwrap();
        assert_eq!(
            selected[0].provider.as_ref().unwrap().provider_id,
            "odin-yggdrasil"
        );
        assert_eq!(selected[0].provider.as_ref().unwrap().capacity, 2);
        assert!(matches!(
            &selected[0].provider.as_ref().unwrap().authority,
            DependencyProviderAuthority::ManagedReady {
                odin_topology_publisher_sequence: 7,
                ..
            }
        ));
    }

    #[test]
    fn external_binding_produces_expected_configuration_only() {
        let input = BINDING.replace(
            "[placement]",
            "[[external_capabilities]]\nprovider_id = \"operator-archive\"\ncapability = \"archive.store\"\nschema = \"archive.store.v1\"\ncompatibility = \"v1\"\ncapacity = 2\nendpoint = \"s3://operator-bound\"\n\n[placement]",
        );
        let binding = OperatorBinding::parse(&input).unwrap();
        let providers = external_binding_provider_refs(&binding).unwrap();
        let declaration = TargetDeclaration::parse(&RECIPE.replace(
            "kind = \"shared-infrastructure\"\ncapability = \"odin.verse-rendezvous\"\nschema = \"odin.verse-topology.v1\"",
            "kind = \"external-operator-binding\"\ncapability = \"archive.store\"\nschema = \"archive.store.v1\"",
        ))
        .unwrap();
        binding.admit(&declaration).unwrap();
        let selected = select_dependencies(&declaration, &providers).unwrap();
        assert!(matches!(
            &providers[0].authority,
            DependencyProviderAuthority::ExternalOperatorBinding { .. }
        ));
        assert_eq!(providers[0].capacity, 2);
        assert_eq!(
            selected[0].provider.as_ref().unwrap().provider_id,
            "operator-archive"
        );
    }

    #[test]
    fn expected_projection_is_sanitized_and_names_the_selected_graph() {
        let plan = plan();
        let release = SealedRelease::new(
            &plan,
            vec![artifact_receipt()],
            vec![external_input_receipt()],
            120,
        )
        .unwrap();
        let expected = release.expected_projection(&plan).unwrap();
        assert_eq!(expected.plan_id, plan.plan_id);
        assert_eq!(expected.sealed_release_id, release.sealed_release_id);
        assert_eq!(expected.incarnation_id, "service-incarnation-1");
        assert_eq!(expected.runtime_id, "service-yggdrasil");
        assert_eq!(
            expected.expected_signer_identity_id,
            "service-runtime-signer"
        );
        assert_eq!(expected.health_contract, "service.health");
        assert_eq!(expected.artifact_sha256, digest(b'c'));
        assert_eq!(expected.source_repository, "github.com/GameCult/Service");
        assert_eq!(expected.source_revision, plan.source.revision);
        assert_eq!(expected.recipe_sha256, plan.source.recipe_blob_sha256);
        assert_eq!(expected.state_schema_generation.as_deref(), Some("v1"));
        assert!(expected.state_contract_sha256.is_some());
        assert!(!expected.write_lease_required);
        assert_eq!(
            expected.dependencies[0].provider_id.as_deref(),
            Some("odin-yggdrasil")
        );
        assert_eq!(
            expected.dependencies[0].provider_authority.as_deref(),
            Some("managed-incarnation")
        );
        assert_eq!(
            expected.route.as_ref().unwrap().candidate_endpoint,
            "http://127.0.0.1:18001"
        );
        assert_eq!(expected.route.as_ref().unwrap().transport, "http");
        assert!(expected.canonical_sha256().unwrap().starts_with("sha256-"));
        let mut invalid_release = release;
        invalid_release.artifacts[0].size_bytes += 1;
        assert!(invalid_release.expected_projection(&plan).is_err());
    }

    #[test]
    fn expected_projection_sorts_shared_capability_and_dependency_claims() {
        let recipe = RECIPE
            .replace(
                "[[provides]]\ncapability = \"service.runtime\"",
                "[[provides]]\ncapability = \"zeta.runtime\"\nschema = \"zeta.runtime.v1\"\ncompatibility = \"v1\"\n\n[[provides]]\ncapability = \"service.runtime\"",
            )
            .replace(
                "[[dependencies]]\nkind = \"shared-infrastructure\"",
                "[[dependencies]]\nkind = \"optional\"\ncapability = \"zeta.optional\"\nschema = \"zeta.optional.v1\"\ncompatibility = \"v1\"\n\n[[dependencies]]\nkind = \"shared-infrastructure\"",
            );
        let plan = compile_deployment_plan(
            recipe.as_bytes(),
            BINDING.as_bytes(),
            source(&recipe),
            "service-incarnation-sorted",
            Some(18002),
            111,
            &[ready_odin_provider("odin-yggdrasil", 1)],
        )
        .unwrap();
        let release = SealedRelease::new(
            &plan,
            vec![artifact_receipt()],
            vec![external_input_receipt()],
            120,
        )
        .unwrap();
        let expected = release.expected_projection(&plan).unwrap();
        assert_eq!(
            expected
                .capabilities
                .iter()
                .map(|capability| capability.capability.as_str())
                .collect::<Vec<_>>(),
            ["service.runtime", "zeta.runtime"]
        );
        assert_eq!(
            expected
                .dependencies
                .iter()
                .map(|dependency| dependency.capability.as_str())
                .collect::<Vec<_>>(),
            ["odin.verse-rendezvous", "zeta.optional"]
        );
        expected.validate().unwrap();
    }

    #[test]
    fn sealed_release_address_covers_artifacts_and_external_inputs() {
        let receipt = artifact_receipt();
        let input = external_input_receipt();
        let plan = plan();
        let mut wrong_artifact = receipt.clone();
        wrong_artifact.sha256 = digest(b'f');
        assert!(SealedRelease::new(&plan, vec![wrong_artifact], vec![input.clone()], 120).is_err());
        assert!(SealedRelease::new(&plan, vec![receipt.clone()], vec![], 120).is_err());
        let release =
            SealedRelease::new(&plan, vec![receipt.clone()], vec![input.clone()], 120).unwrap();
        let later_receipt_time =
            SealedRelease::new(&plan, vec![receipt], vec![input], 130).unwrap();
        release.validate_against(&plan).unwrap();
        assert_eq!(
            release.sealed_release_id,
            later_receipt_time.sealed_release_id
        );
        let mut changed = release.clone();
        changed.artifacts[0].size_bytes = 43;
        assert!(changed.validate_against(&plan).is_err());
        let mut changed = release;
        changed.external_inputs[0].size_bytes = 18;
        assert!(changed.validate_against(&plan).is_err());
    }
}

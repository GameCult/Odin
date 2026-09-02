use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::deployment::{
    CapabilityDependency, DependencyKind, ExternalCapabilityBinding, OperatorBinding,
    ProvidedCapability, TargetDeclaration, capability_compatible,
};

pub const COMPILED_DEPLOYMENT_PLAN_SCHEMA: &str = "idunn.compiled_deployment_plan.v1";
pub const SEALED_DEPLOYMENT_SCHEMA: &str = "idunn.sealed_deployment.v1";
pub const EXPECTED_GENERATION_SCHEMA: &str = "idunn.expected_generation.v1";
pub const GENERATION_CORRELATION_SCHEMA: &str = "idunn.generation_correlation.v1";
pub const PROMOTION_TRANSACTION_SCHEMA: &str = "idunn.promotion_transaction.v1";
pub const ADMITTED_GENERATION_SCHEMA: &str = "idunn.admitted_generation.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedGitlink {
    pub origin: String,
    pub revision: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactSource {
    pub origin: String,
    pub admitted_ref: String,
    pub revision: String,
    pub minimum_revision: String,
    pub selection_receipt_sha256: String,
    pub gitlinks: BTreeMap<PathBuf, ResolvedGitlink>,
}

impl ExactSource {
    pub fn validate_against(&self, binding: &OperatorBinding) -> Result<()> {
        ensure!(
            self.origin == binding.repository.origin,
            "exact source origin differs from binding"
        );
        ensure!(
            self.admitted_ref == binding.repository.admitted_ref,
            "exact source ref differs from binding"
        );
        require_sha1(&self.revision, "exact source revision")?;
        ensure!(
            self.minimum_revision == binding.repository.minimum_revision,
            "exact source floor differs from binding"
        );
        require_sha1(&self.minimum_revision, "exact source floor")?;
        require_sha256(&self.selection_receipt_sha256, "source-selection receipt")?;
        let expected_paths: BTreeSet<_> = binding.repository.gitlinks.keys().cloned().collect();
        let observed_paths: BTreeSet<_> = self.gitlinks.keys().cloned().collect();
        ensure!(
            expected_paths == observed_paths,
            "resolved Gitlinks differ from binding"
        );
        for (path, observed) in &self.gitlinks {
            let expected = &binding.repository.gitlinks[path];
            ensure!(
                observed.origin == expected.origin,
                "Gitlink {} origin differs from binding",
                path.display()
            );
            require_sha1(&observed.revision, "Gitlink revision")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderOrigin {
    ManagedRuntime,
    ExternalOperatorBinding,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderObservation {
    pub provider_id: String,
    pub provider_target: Option<String>,
    pub origin: ProviderOrigin,
    pub generation: String,
    pub deployment_id: Option<String>,
    pub capability: String,
    pub schema: String,
    pub compatibility: String,
    pub capacity: u32,
    pub endpoint: String,
    pub expected: bool,
    pub present: bool,
    pub ready: bool,
    pub observation_sha256: Option<String>,
}

impl ProviderObservation {
    pub fn validate(&self) -> Result<()> {
        require_token(&self.provider_id, "provider id")?;
        if let Some(target) = &self.provider_target {
            require_token(target, "provider target")?;
        }
        require_token(&self.generation, "provider generation")?;
        if let Some(deployment_id) = &self.deployment_id {
            require_token(deployment_id, "provider deployment id")?;
        }
        require_contract(&self.capability, &self.schema, &self.compatibility)?;
        ensure!(self.capacity > 0, "provider capacity is zero");
        require_value(&self.endpoint, "provider endpoint")?;
        ensure!(
            !self.present || self.expected,
            "provider is present without expected admission"
        );
        ensure!(
            !self.ready || self.present,
            "provider is ready without signed presence"
        );
        match self.origin {
            ProviderOrigin::ManagedRuntime => {
                ensure!(
                    self.provider_target.is_some(),
                    "managed provider has no target"
                );
                if self.present {
                    ensure!(
                        self.deployment_id.is_some(),
                        "present managed provider has no deployment identity"
                    );
                    require_sha256(
                        self.observation_sha256.as_deref().unwrap_or_default(),
                        "managed provider observation",
                    )?;
                }
            }
            ProviderOrigin::ExternalOperatorBinding => {
                ensure!(
                    self.provider_target.is_none(),
                    "external binding impersonates a managed target"
                );
                ensure!(
                    !self.present && !self.ready,
                    "external configuration impersonates runtime observation"
                );
                ensure!(
                    self.observation_sha256.is_none(),
                    "external configuration carries a runtime observation digest"
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyResolution {
    pub requirement: CapabilityDependency,
    pub provider: Option<ProviderObservation>,
}

impl DependencyResolution {
    pub fn blocks_start(&self) -> bool {
        self.requirement.startup == crate::deployment::StartupOrder::BeforeStart
            && self.requirement.kind != DependencyKind::Optional
            && !self
                .provider
                .as_ref()
                .is_some_and(|provider| provider.ready)
    }

    pub fn blocks_promotion(&self) -> bool {
        self.requirement.kind != DependencyKind::Optional
            && !self
                .provider
                .as_ref()
                .is_some_and(|provider| provider.ready)
    }
}

pub fn external_binding_observations(
    bindings: &[ExternalCapabilityBinding],
) -> Vec<ProviderObservation> {
    bindings
        .iter()
        .map(|binding| ProviderObservation {
            provider_id: format!("operator:{}", binding.capability),
            provider_target: None,
            origin: ProviderOrigin::ExternalOperatorBinding,
            generation: "operator-bound".into(),
            deployment_id: None,
            capability: binding.capability.clone(),
            schema: binding.schema.clone(),
            compatibility: binding.compatibility.clone(),
            capacity: 1,
            endpoint: binding.endpoint.clone(),
            expected: true,
            present: false,
            ready: false,
            observation_sha256: None,
        })
        .collect()
}

pub fn resolve_dependencies(
    declaration: &TargetDeclaration,
    observations: &[ProviderObservation],
) -> Result<Vec<DependencyResolution>> {
    for observation in observations {
        observation.validate()?;
    }
    for conflict in &declaration.conflicts {
        if observations
            .iter()
            .any(|provider| provider.expected && provider.capability == conflict.capability)
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
            let mut candidates = observations
                .iter()
                .filter(|provider| provider.expected && provider.compatible_with(dependency))
                .cloned()
                .collect::<Vec<_>>();
            if dependency.kind == DependencyKind::ExternalOperatorBinding {
                candidates
                    .retain(|provider| provider.origin == ProviderOrigin::ExternalOperatorBinding);
            } else {
                candidates.retain(|provider| provider.origin == ProviderOrigin::ManagedRuntime);
            }
            candidates.sort_by(|left, right| {
                right
                    .ready
                    .cmp(&left.ready)
                    .then_with(|| right.present.cmp(&left.present))
                    .then_with(|| left.provider_id.cmp(&right.provider_id))
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
            Ok(DependencyResolution {
                requirement: dependency.clone(),
                provider,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedGeneration {
    pub schema: String,
    pub target: String,
    pub generation: String,
    pub deployment_id: String,
    pub source_revision: String,
    pub candidate_endpoint: Option<String>,
    pub capabilities: Vec<ProvidedCapability>,
}

impl ExpectedGeneration {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == EXPECTED_GENERATION_SCHEMA,
            "unsupported expected-generation schema"
        );
        require_token(&self.target, "expected target")?;
        require_token(&self.generation, "expected generation")?;
        require_token(&self.deployment_id, "expected deployment")?;
        require_sha1(&self.source_revision, "expected source revision")?;
        if let Some(endpoint) = &self.candidate_endpoint {
            require_value(endpoint, "candidate endpoint")?;
        }
        let mut capabilities = BTreeSet::new();
        for capability in &self.capabilities {
            require_contract(
                &capability.capability,
                &capability.schema,
                &capability.compatibility,
            )?;
            ensure!(
                capability.capacity > 0,
                "expected capability capacity is zero"
            );
            ensure!(
                capabilities.insert((
                    &capability.capability,
                    &capability.schema,
                    &capability.compatibility
                )),
                "expected capability is duplicated"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledDeploymentPlan {
    pub schema: String,
    pub plan_id: String,
    pub deployment_id: String,
    pub created_at_unix_millis: u64,
    pub source: ExactSource,
    pub recipe_sha256: String,
    pub binding_sha256: String,
    pub declaration: TargetDeclaration,
    pub binding: OperatorBinding,
    pub dependencies: Vec<DependencyResolution>,
    pub expected: ExpectedGeneration,
}

impl CompiledDeploymentPlan {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == COMPILED_DEPLOYMENT_PLAN_SCHEMA,
            "unsupported deployment-plan schema"
        );
        require_sha256(&self.plan_id, "plan id")?;
        require_token(&self.deployment_id, "deployment id")?;
        ensure!(
            self.created_at_unix_millis > 0,
            "deployment plan has no creation time"
        );
        self.declaration.validate()?;
        self.binding.validate()?;
        self.binding.admit(&self.declaration)?;
        self.source.validate_against(&self.binding)?;
        require_sha256(&self.recipe_sha256, "recipe digest")?;
        require_sha256(&self.binding_sha256, "binding digest")?;
        self.expected.validate()?;
        ensure!(
            self.expected.target == self.declaration.target,
            "expected target differs from recipe"
        );
        ensure!(
            self.expected.generation == self.declaration.state.generation,
            "expected generation differs from recipe"
        );
        ensure!(
            self.expected.deployment_id == self.deployment_id,
            "expected deployment differs from plan"
        );
        ensure!(
            self.expected.source_revision == self.source.revision,
            "expected source differs from plan"
        );
        ensure!(
            self.expected.capabilities == self.declaration.provides,
            "expected capabilities differ from recipe"
        );
        let resolved_requirements: Vec<_> = self
            .dependencies
            .iter()
            .map(|resolution| &resolution.requirement)
            .collect();
        let declared_requirements: Vec<_> = self.declaration.dependencies.iter().collect();
        ensure!(
            resolved_requirements == declared_requirements,
            "resolved graph differs from recipe dependencies"
        );
        for resolution in &self.dependencies {
            if let Some(provider) = &resolution.provider {
                provider.validate()?;
                ensure!(
                    provider.compatible_with(&resolution.requirement),
                    "resolved provider is incompatible"
                );
            } else {
                ensure!(
                    resolution.requirement.kind == DependencyKind::Optional,
                    "required dependency is unresolved"
                );
            }
        }
        ensure!(
            self.plan_id == self.recomputed_plan_id()?,
            "deployment plan digest is not canonical"
        );
        Ok(())
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
pub fn compile_deployment_plan(
    declaration: TargetDeclaration,
    binding: OperatorBinding,
    source: ExactSource,
    recipe_bytes: &[u8],
    binding_bytes: &[u8],
    deployment_id: impl Into<String>,
    candidate_port: Option<u16>,
    created_at_unix_millis: u64,
    observed_providers: &[ProviderObservation],
) -> Result<CompiledDeploymentPlan> {
    declaration.validate()?;
    binding.validate()?;
    binding.admit(&declaration)?;
    source.validate_against(&binding)?;
    let deployment_id = deployment_id.into();
    require_token(&deployment_id, "deployment id")?;
    ensure!(
        created_at_unix_millis > 0,
        "deployment plan has no creation time"
    );
    let candidate_endpoint = match (&binding.route, candidate_port) {
        (Some(route), Some(port)) => {
            ensure!(
                (route.private_port_start..=route.private_port_end).contains(&port),
                "candidate port is outside the operator range"
            );
            Some(format!("{}:{port}", route.private_host))
        }
        (None, None) => None,
        (Some(_), None) => bail!("routed deployment has no candidate port"),
        (None, Some(_)) => bail!("private deployment cannot select a routed candidate port"),
    };
    let mut providers = observed_providers.to_vec();
    providers.extend(external_binding_observations(
        &binding.external_capabilities,
    ));
    let dependencies = resolve_dependencies(&declaration, &providers)?;
    let expected = ExpectedGeneration {
        schema: EXPECTED_GENERATION_SCHEMA.into(),
        target: declaration.target.clone(),
        generation: declaration.state.generation.clone(),
        deployment_id: deployment_id.clone(),
        source_revision: source.revision.clone(),
        candidate_endpoint,
        capabilities: declaration.provides.clone(),
    };
    let mut plan = CompiledDeploymentPlan {
        schema: COMPILED_DEPLOYMENT_PLAN_SCHEMA.into(),
        plan_id: String::new(),
        deployment_id,
        created_at_unix_millis,
        source,
        recipe_sha256: sha256_id(recipe_bytes),
        binding_sha256: sha256_id(binding_bytes),
        declaration,
        binding,
        dependencies,
        expected,
    };
    plan.plan_id = plan.recomputed_plan_id()?;
    plan.validate()?;
    Ok(plan)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSeal {
    pub artifact_id: String,
    pub destination: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
    pub executable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedDeployment {
    pub schema: String,
    pub release_id: String,
    pub plan: CompiledDeploymentPlan,
    pub plan_sha256: String,
    pub artifacts: Vec<ArtifactSeal>,
    pub sealed_at_unix_millis: u64,
}

impl SealedDeployment {
    pub fn new(
        plan: CompiledDeploymentPlan,
        release_id: impl Into<String>,
        artifacts: Vec<ArtifactSeal>,
        sealed_at_unix_millis: u64,
    ) -> Result<Self> {
        plan.validate()?;
        let mut sealed = Self {
            schema: SEALED_DEPLOYMENT_SCHEMA.into(),
            release_id: release_id.into(),
            plan_sha256: sha256_id(&rmp_serde::to_vec(&plan)?),
            plan,
            artifacts,
            sealed_at_unix_millis,
        };
        sealed.validate()?;
        sealed
            .artifacts
            .sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
        sealed.validate()?;
        Ok(sealed)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == SEALED_DEPLOYMENT_SCHEMA,
            "unsupported sealed-deployment schema"
        );
        require_token(&self.release_id, "release id")?;
        ensure!(
            self.sealed_at_unix_millis >= self.plan.created_at_unix_millis,
            "deployment was sealed before it was planned"
        );
        self.plan.validate()?;
        ensure!(
            self.plan_sha256 == sha256_id(&rmp_serde::to_vec(&self.plan)?),
            "sealed plan digest is wrong"
        );
        let declared = self
            .plan
            .declaration
            .artifacts
            .iter()
            .map(|artifact| {
                (
                    artifact.id.as_str(),
                    artifact.destination.as_path(),
                    artifact.executable,
                )
            })
            .chain(
                self.plan
                    .declaration
                    .external_artifacts
                    .iter()
                    .map(|artifact| {
                        (
                            artifact.id.as_str(),
                            artifact.destination.as_path(),
                            artifact.executable,
                        )
                    }),
            )
            .collect::<BTreeSet<_>>();
        let mut observed = BTreeSet::new();
        for artifact in &self.artifacts {
            require_token(&artifact.artifact_id, "artifact seal id")?;
            require_sha256(&artifact.sha256, "artifact seal digest")?;
            ensure!(artifact.size_bytes > 0, "sealed artifact is empty");
            ensure!(
                observed.insert((
                    artifact.artifact_id.as_str(),
                    artifact.destination.as_path(),
                    artifact.executable
                )),
                "artifact seal is duplicated"
            );
        }
        ensure!(
            declared == observed,
            "sealed artifacts differ from declared outputs"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePresence {
    pub target: String,
    pub generation: String,
    pub deployment_id: String,
    pub source_revision: String,
    pub endpoint: Option<String>,
    pub capabilities: Vec<ProvidedCapability>,
    pub runtime_id: String,
    pub signer_identity_id: String,
    pub signed_statement_sha256: String,
}

impl RuntimePresence {
    pub fn validate(&self) -> Result<()> {
        require_token(&self.target, "presence target")?;
        require_token(&self.generation, "presence generation")?;
        require_token(&self.deployment_id, "presence deployment")?;
        require_sha1(&self.source_revision, "presence source revision")?;
        if let Some(endpoint) = &self.endpoint {
            require_value(endpoint, "presence endpoint")?;
        }
        require_token(&self.runtime_id, "presence runtime")?;
        require_token(&self.signer_identity_id, "presence signer")?;
        require_sha256(&self.signed_statement_sha256, "presence statement")?;
        let mut capabilities = BTreeSet::new();
        for capability in &self.capabilities {
            require_contract(
                &capability.capability,
                &capability.schema,
                &capability.compatibility,
            )?;
            ensure!(
                capability.capacity > 0,
                "presence capability capacity is zero"
            );
            ensure!(
                capabilities.insert((
                    &capability.capability,
                    &capability.schema,
                    &capability.compatibility,
                )),
                "presence capability is duplicated"
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthObservation {
    pub contract: String,
    pub state: String,
    pub detail: String,
    pub signed_statement_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationCorrelation {
    pub schema: String,
    pub target: String,
    pub generation: String,
    pub expected: bool,
    pub present: bool,
    pub ready: bool,
    pub disagreements: Vec<String>,
}

pub fn correlate_generation(
    plan: &CompiledDeploymentPlan,
    presence: Option<&RuntimePresence>,
    health: Option<&HealthObservation>,
) -> Result<GenerationCorrelation> {
    plan.validate()?;
    let mut disagreements = Vec::new();
    let mut present = false;
    if let Some(presence) = presence {
        presence.validate()?;
        for (matches, code) in [
            (presence.target == plan.expected.target, "target-mismatch"),
            (
                presence.generation == plan.expected.generation,
                "generation-mismatch",
            ),
            (
                presence.deployment_id == plan.expected.deployment_id,
                "deployment-mismatch",
            ),
            (
                presence.source_revision == plan.expected.source_revision,
                "source-mismatch",
            ),
            (
                presence.endpoint == plan.expected.candidate_endpoint,
                "endpoint-mismatch",
            ),
            (
                capability_set(&presence.capabilities)
                    == capability_set(&plan.expected.capabilities),
                "capability-mismatch",
            ),
        ] {
            if !matches {
                disagreements.push(code.into());
            }
        }
        present = disagreements.is_empty();
    } else {
        disagreements.push("runtime-presence-missing".into());
    }
    let mut health_ready = false;
    if let (Some(presence), Some(health)) = (presence, health) {
        require_token(&health.contract, "health contract")?;
        require_value(&health.state, "health state")?;
        require_value(&health.detail, "health detail")?;
        require_sha256(&health.signed_statement_sha256, "health statement")?;
        if health.signed_statement_sha256 != presence.signed_statement_sha256 {
            disagreements.push("health-presence-statement-mismatch".into());
        } else if health.contract != plan.declaration.service.health.contract {
            disagreements.push("health-contract-mismatch".into());
        } else if health.state != plan.declaration.service.health.ready.state {
            disagreements.push("health-state-not-ready".into());
        } else if health.detail != plan.declaration.service.health.ready.detail {
            disagreements.push("health-detail-not-ready".into());
        } else {
            health_ready = true;
        }
    } else if health.is_none() {
        disagreements.push("health-missing".into());
    }
    if plan
        .dependencies
        .iter()
        .any(DependencyResolution::blocks_promotion)
    {
        disagreements.push("dependency-graph-not-ready".into());
    }
    disagreements.sort();
    disagreements.dedup();
    Ok(GenerationCorrelation {
        schema: GENERATION_CORRELATION_SCHEMA.into(),
        target: plan.expected.target.clone(),
        generation: plan.expected.generation.clone(),
        expected: true,
        present,
        ready: present && health_ready && disagreements.is_empty(),
        disagreements,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PromotionPhase {
    Planned,
    Materialized,
    Sealed,
    CandidateStarted,
    Staged,
    Fenced,
    RouteObserved,
    WriteAdmitted,
    Active,
    Draining,
    Complete,
}

impl PromotionPhase {
    pub fn next(self) -> Option<Self> {
        Some(match self {
            Self::Planned => Self::Materialized,
            Self::Materialized => Self::Sealed,
            Self::Sealed => Self::CandidateStarted,
            Self::CandidateStarted => Self::Staged,
            Self::Staged => Self::Fenced,
            Self::Fenced => Self::RouteObserved,
            Self::RouteObserved => Self::WriteAdmitted,
            Self::WriteAdmitted => Self::Active,
            Self::Active => Self::Draining,
            Self::Draining => Self::Complete,
            Self::Complete => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionTransaction {
    pub schema: String,
    pub transaction_id: String,
    pub plan_id: String,
    pub target: String,
    pub phase: PromotionPhase,
    pub revision: u64,
    pub updated_at_unix_millis: u64,
}

impl PromotionTransaction {
    pub fn new(
        transaction_id: impl Into<String>,
        plan: &CompiledDeploymentPlan,
        created_at_unix_millis: u64,
    ) -> Result<Self> {
        plan.validate()?;
        let transaction = Self {
            schema: PROMOTION_TRANSACTION_SCHEMA.into(),
            transaction_id: transaction_id.into(),
            plan_id: plan.plan_id.clone(),
            target: plan.declaration.target.clone(),
            phase: PromotionPhase::Planned,
            revision: 0,
            updated_at_unix_millis: created_at_unix_millis,
        };
        transaction.validate()?;
        Ok(transaction)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == PROMOTION_TRANSACTION_SCHEMA,
            "unsupported promotion transaction schema"
        );
        require_token(&self.transaction_id, "promotion transaction")?;
        require_sha256(&self.plan_id, "promotion plan")?;
        require_token(&self.target, "promotion target")?;
        ensure!(
            self.updated_at_unix_millis > 0,
            "promotion transaction has no update time"
        );
        Ok(())
    }

    pub fn advance(&self, phase: PromotionPhase, updated_at_unix_millis: u64) -> Result<Self> {
        self.validate()?;
        ensure!(
            self.phase.next() == Some(phase),
            "promotion phase transition is not adjacent"
        );
        ensure!(
            updated_at_unix_millis >= self.updated_at_unix_millis,
            "promotion time moved backwards"
        );
        let next = Self {
            schema: self.schema.clone(),
            transaction_id: self.transaction_id.clone(),
            plan_id: self.plan_id.clone(),
            target: self.target.clone(),
            phase,
            revision: self
                .revision
                .checked_add(1)
                .context("promotion revision overflow")?,
            updated_at_unix_millis,
        };
        next.validate()?;
        Ok(next)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedGeneration {
    pub schema: String,
    pub target: String,
    pub generation: String,
    pub deployment_id: String,
    pub plan_id: String,
    pub release_id: String,
    pub source_revision: String,
    pub workload_unit: String,
    pub process_id: u32,
    pub process_starttime_ticks: u64,
    pub private_endpoint: Option<String>,
    pub route_id: Option<String>,
    pub route_observation_sha256: Option<String>,
    pub write_admission_sha256: String,
    pub admitted_at_unix_millis: u64,
}

impl AdmittedGeneration {
    pub fn validate_against(&self, sealed: &SealedDeployment) -> Result<()> {
        ensure!(
            self.schema == ADMITTED_GENERATION_SCHEMA,
            "unsupported admitted-generation schema"
        );
        sealed.validate()?;
        require_token(&self.target, "admitted target")?;
        require_token(&self.generation, "admitted generation")?;
        require_token(&self.deployment_id, "admitted deployment")?;
        require_sha256(&self.plan_id, "admitted plan")?;
        require_token(&self.release_id, "admitted release")?;
        require_sha1(&self.source_revision, "admitted source")?;
        require_token(&self.workload_unit, "admitted workload unit")?;
        ensure!(self.process_id > 0, "admitted process id is zero");
        ensure!(
            self.process_starttime_ticks > 0,
            "admitted process start time is zero"
        );
        if let Some(endpoint) = &self.private_endpoint {
            require_value(endpoint, "admitted private endpoint")?;
        }
        match (&self.route_id, &self.route_observation_sha256) {
            (Some(route_id), Some(digest)) => {
                require_token(route_id, "admitted route")?;
                require_sha256(digest, "route observation")?;
            }
            (None, None) => {}
            _ => bail!("admitted route identity and observation are incomplete"),
        }
        require_sha256(&self.write_admission_sha256, "write admission")?;
        ensure!(self.admitted_at_unix_millis > 0, "admission has no time");
        ensure!(
            self.target == sealed.plan.declaration.target,
            "admitted target differs from sealed plan"
        );
        ensure!(
            self.generation == sealed.plan.declaration.state.generation,
            "admitted generation differs from sealed plan"
        );
        ensure!(
            self.deployment_id == sealed.plan.deployment_id,
            "admitted deployment differs from sealed plan"
        );
        ensure!(
            self.plan_id == sealed.plan.plan_id,
            "admitted plan differs from sealed plan"
        );
        ensure!(
            self.release_id == sealed.release_id,
            "admitted release differs from sealed plan"
        );
        ensure!(
            self.source_revision == sealed.plan.source.revision,
            "admitted source differs from sealed plan"
        );
        ensure!(
            self.private_endpoint == sealed.plan.expected.candidate_endpoint,
            "admitted endpoint differs from expected plan"
        );
        match &sealed.plan.binding.route {
            Some(route) => ensure!(
                self.route_id.as_deref() == Some(route.route_id.as_str()),
                "admitted route differs from binding"
            ),
            None => ensure!(
                self.route_id.is_none(),
                "private deployment carries a route"
            ),
        }
        Ok(())
    }
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

fn capability_set(capabilities: &[ProvidedCapability]) -> BTreeSet<(&str, &str, &str, u32)> {
    capabilities
        .iter()
        .map(|capability| {
            (
                capability.capability.as_str(),
                capability.schema.as_str(),
                capability.compatibility.as_str(),
                capability.capacity,
            )
        })
        .collect()
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
    use crate::deployment::{OperatorBinding, TargetDeclaration};

    const RECIPE: &str = r#"
schema = "gamecult.idunn.target_declaration.v1"
target = "service"
source_stamp_environment = "SERVICE_BUILD_COMMIT"

[[steps]]
id = "build"
phase = "build"
runner = "rust"
argv = ["cargo", "build", "--locked"]

[[artifacts]]
id = "daemon"
source_kind = "runner-output"
runner = "rust"
source = "target/release/service"
destination = "service"
executable = true

[service]
executable_artifact = "daemon"
transport = "http"
route_required = true
required_environment = ["SERVICE_BIND"]

[service.health]
contract = "service.health"

[service.health.staged]
state = "warming"
detail = "traffic-admission-pending"

[service.health.ready]
state = "active"
detail = "serving"

[state]
generation = "v1"

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
schema = "gamecult.idunn.operator_binding.v1"
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
affordances = ["source-read", "artifact-write"]
allowed_programs = ["cargo"]
network = "none"
memory_mebibytes = 2048
cpu_quota_percent = 200

[workload]
driver = "systemd-transient"
user = "service"
group = "service"
unit_prefix = "idunn-service"
release_root = "/srv/service/releases"
state_root = "/var/lib/gamecult/service"
runtime_root = "/run/gamecult/idunn/service"
network = "host-private"
hardening = "strict"
memory_mebibytes = 1024
cpu_quota_percent = 100

[workload.environment]
SERVICE_BIND = "idunn.private_endpoint"

[route]
driver = "nginx-http"
route_id = "service"
stable_endpoint = "https://example.invalid/service/"
private_host = "127.0.0.1"
private_port_start = 18000
private_port_end = 18009
config_path = "/etc/nginx/idunn-routes/service.conf"
reload_unit = "nginx.service"

[admission]
driver = "atomic-file"
record_path = "/run/gamecult/idunn/service/admission.cc"
lock_path = "/run/gamecult/idunn/service/admission.cc.lock"

[brakes]
deployment_store = "/var/lib/gamecult/idunn/service-deployment-brake.cc"
lifecycle_store = "/var/lib/gamecult/idunn/service-lifecycle-brake.cc"

[state_transition]
policy = "preserve"

[rollout]
strategy = "candidate-then-promote"
drain_seconds = 30
retain_releases = 2

[placement]
desired_replicas = 1
nodes = ["yggdrasil"]
"#;

    fn provider(ready: bool) -> ProviderObservation {
        ProviderObservation {
            provider_id: "odin-yggdrasil".into(),
            provider_target: Some("odin".into()),
            origin: ProviderOrigin::ManagedRuntime,
            generation: "odin-v1".into(),
            deployment_id: Some("odin-deployment-1".into()),
            capability: "odin.verse-rendezvous".into(),
            schema: "odin.verse-topology.v1".into(),
            compatibility: "v1".into(),
            capacity: 1,
            endpoint: "10.77.0.1:17871".into(),
            expected: true,
            present: ready,
            ready,
            observation_sha256: ready.then(|| format!("sha256-{}", "a".repeat(64))),
        }
    }

    fn plan(provider: ProviderObservation) -> CompiledDeploymentPlan {
        compile_deployment_plan(
            TargetDeclaration::parse(RECIPE).unwrap(),
            OperatorBinding::parse(BINDING).unwrap(),
            ExactSource {
                origin: "https://github.com/GameCult/Service.git".into(),
                admitted_ref: "refs/heads/main".into(),
                revision: "2222222222222222222222222222222222222222".into(),
                minimum_revision: "1111111111111111111111111111111111111111".into(),
                selection_receipt_sha256: format!("sha256-{}", "d".repeat(64)),
                gitlinks: BTreeMap::new(),
            },
            RECIPE.as_bytes(),
            BINDING.as_bytes(),
            "deployment-1",
            Some(18001),
            100,
            &[provider],
        )
        .unwrap()
    }

    #[test]
    fn compilation_is_deterministic_and_configuration_does_not_supply_readiness() {
        let first = plan(provider(true));
        let second = plan(provider(true));
        assert_eq!(first, second);
        assert!(!first.dependencies[0].blocks_promotion());

        let external = ExternalCapabilityBinding {
            capability: "odin.verse-rendezvous".into(),
            schema: "odin.verse-topology.v1".into(),
            compatibility: "v1".into(),
            endpoint: "10.77.0.1:17871".into(),
        };
        let observations = external_binding_observations(&[external]);
        assert!(!observations[0].present);
        assert!(!observations[0].ready);
    }

    #[test]
    fn shared_infrastructure_reuses_the_ready_provider() {
        let mut weaker = provider(false);
        weaker.provider_id = "odin-candidate".into();
        let resolved = resolve_dependencies(
            &TargetDeclaration::parse(RECIPE).unwrap(),
            &[weaker, provider(true)],
        )
        .unwrap();
        assert_eq!(
            resolved[0].provider.as_ref().unwrap().provider_id,
            "odin-yggdrasil"
        );
    }

    #[test]
    fn present_and_ready_are_distinct_from_expected() {
        let plan = plan(provider(true));
        let presence = RuntimePresence {
            target: "service".into(),
            generation: "v1".into(),
            deployment_id: "deployment-1".into(),
            source_revision: "2222222222222222222222222222222222222222".into(),
            endpoint: Some("127.0.0.1:18001".into()),
            capabilities: plan.expected.capabilities.clone(),
            runtime_id: "service-runtime".into(),
            signer_identity_id: "signer-1".into(),
            signed_statement_sha256: format!("sha256-{}", "b".repeat(64)),
        };
        let health = HealthObservation {
            contract: "service.health".into(),
            state: "active".into(),
            detail: "serving".into(),
            signed_statement_sha256: presence.signed_statement_sha256.clone(),
        };
        let absent = correlate_generation(&plan, None, None).unwrap();
        assert!(absent.expected);
        assert!(!absent.present);
        assert!(!absent.ready);
        let ready = correlate_generation(&plan, Some(&presence), Some(&health)).unwrap();
        assert!(ready.present);
        assert!(ready.ready);
    }

    #[test]
    fn disagreement_remains_visible_and_blocks_readiness() {
        let plan = plan(provider(true));
        let presence = RuntimePresence {
            target: "service".into(),
            generation: "wrong-generation".into(),
            deployment_id: "deployment-1".into(),
            source_revision: "2222222222222222222222222222222222222222".into(),
            endpoint: Some("127.0.0.1:18001".into()),
            capabilities: plan.expected.capabilities.clone(),
            runtime_id: "service-runtime".into(),
            signer_identity_id: "signer-1".into(),
            signed_statement_sha256: format!("sha256-{}", "b".repeat(64)),
        };
        let health = HealthObservation {
            contract: "service.health".into(),
            state: "active".into(),
            detail: "serving".into(),
            signed_statement_sha256: presence.signed_statement_sha256.clone(),
        };
        let correlation = correlate_generation(&plan, Some(&presence), Some(&health)).unwrap();
        assert!(!correlation.present);
        assert!(!correlation.ready);
        assert!(
            correlation
                .disagreements
                .contains(&"generation-mismatch".into())
        );
    }

    #[test]
    fn sealed_artifacts_and_promotion_phases_are_exact() {
        let plan = plan(provider(true));
        let sealed = SealedDeployment::new(
            plan.clone(),
            "release-1",
            vec![ArtifactSeal {
                artifact_id: "daemon".into(),
                destination: "service".into(),
                sha256: format!("sha256-{}", "c".repeat(64)),
                size_bytes: 42,
                executable: true,
            }],
            110,
        )
        .unwrap();
        sealed.validate().unwrap();
        let transaction = PromotionTransaction {
            schema: PROMOTION_TRANSACTION_SCHEMA.into(),
            transaction_id: "transaction-1".into(),
            plan_id: plan.plan_id,
            target: "service".into(),
            phase: PromotionPhase::Planned,
            revision: 0,
            updated_at_unix_millis: 100,
        };
        assert!(transaction.advance(PromotionPhase::Sealed, 101).is_err());
        assert_eq!(
            transaction
                .advance(PromotionPhase::Materialized, 101)
                .unwrap()
                .phase,
            PromotionPhase::Materialized
        );
    }
}

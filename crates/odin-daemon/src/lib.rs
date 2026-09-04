//! Deterministic Odin authority for runtime-topology correlation.
//!
//! This library owns no listener or lifecycle loop. Callers provide Idunn's
//! read-only projection, a trusted receive time with any service-owned
//! presence, and narrow clock/store/signer implementations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use cultcache_rs::{
    CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore,
};
use cultnet_rs::{
    GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA, GAMECULT_RUNTIME_PRESENCE_HEALTH_SIGNING_PURPOSE,
    GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA, GameCultRuntimePresenceHealthRecord,
    GameCultServiceTrustAnchorRecord, IDUNN_EXPECTED_INCARNATION_SCHEMA,
    IDUNN_PROCESS_WRITE_LEASE_SCHEMA, IDUNN_RUNTIME_ACTIVATION_SCHEMA, IdunnExpectedDependency,
    IdunnExpectedIncarnationRecord, IdunnProcessWriteLeaseRecord, IdunnRuntimeActivationPurpose,
    IdunnRuntimeActivationRecord, IdunnServiceIdentity, ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA,
    OdinRuntimeDependencyEvidence, OdinRuntimeTopologyCorrelationPurpose,
    OdinRuntimeTopologyCorrelationRecord, OdinTopologyAuthenticationContext,
    OdinTopologyDisagreement, OdinTopologyIdentity, RuntimePresenceAuthenticationContext,
    ServiceIdentitySignature, ServiceIdentitySigner, ServiceIdentityTrustAnchor,
    authenticate_odin_runtime_topology_correlation, authenticate_runtime_presence_claim,
    correlate_runtime_presence_claim, verify_runtime_authority, verify_service_identity_signature,
    verify_service_identity_signature_with_public_key,
};

const CAS_ATTEMPTS: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct AuthenticationPolicy {
    pub presence_maximum_age_millis: u64,
    pub presence_maximum_future_skew_millis: u64,
    pub correlation_maximum_age_millis: u64,
    pub correlation_maximum_future_skew_millis: u64,
}

impl Default for AuthenticationPolicy {
    fn default() -> Self {
        Self {
            presence_maximum_age_millis: 30_000,
            presence_maximum_future_skew_millis: 5_000,
            correlation_maximum_age_millis: 30_000,
            correlation_maximum_future_skew_millis: 5_000,
        }
    }
}

pub trait Clock {
    fn now_unix_millis(&self) -> Result<u64>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_millis(&self) -> Result<u64> {
        Ok(SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock predates the Unix epoch")?
            .as_millis()
            .try_into()
            .context("system clock does not fit the topology timestamp")?)
    }
}

/// One exact Idunn-owned projection. The service anchor is provider lookup
/// material selected by Expected; it is not runtime observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdunnRuntimeProjection {
    pub expected: IdunnExpectedIncarnationRecord,
    pub provider_anchor: Option<GameCultServiceTrustAnchorRecord>,
    pub activation: Option<IdunnRuntimeActivationRecord>,
    pub current_lease: Option<IdunnProcessWriteLeaseRecord>,
}

impl IdunnRuntimeProjection {
    pub fn validate(&self) -> Result<()> {
        self.expected.validate()?;
        if let Some(anchor) = &self.provider_anchor {
            anchor.validate()?;
        }
        if let Some(activation) = &self.activation {
            activation.validate()?;
        }
        if let Some(lease) = &self.current_lease {
            lease.validate()?;
        }
        Ok(())
    }
}

pub trait IdunnProjectionSource {
    fn current_projection(&self, target: &str) -> Result<Option<IdunnRuntimeProjection>>;
}

/// Read-only adapter for the atomic projection file published by Idunn.
pub struct CultCacheIdunnProjectionSource {
    path: PathBuf,
}

impl CultCacheIdunnProjectionSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl IdunnProjectionSource for CultCacheIdunnProjectionSource {
    fn current_projection(&self, target: &str) -> Result<Option<IdunnRuntimeProjection>> {
        if !self.path.is_file() {
            return Ok(None);
        }
        let entries =
            SingleFileMessagePackBackingStore::new(&self.path).pull_all_read_only_snapshot()?;
        let expected_envelope =
            unique_envelope(&entries, IdunnExpectedIncarnationRecord::TYPE, target)?;
        let anchor_key = runtime_presence_anchor_id(target);
        let anchor_envelope = unique_envelope(
            &entries,
            GameCultServiceTrustAnchorRecord::TYPE,
            &anchor_key,
        )?;
        let activation_envelope =
            unique_envelope(&entries, IdunnRuntimeActivationRecord::TYPE, target)?;
        let lease_envelope = unique_envelope(&entries, IdunnProcessWriteLeaseRecord::TYPE, target)?;

        let Some(expected_envelope) = expected_envelope else {
            ensure!(
                anchor_envelope.is_none()
                    && activation_envelope.is_none()
                    && lease_envelope.is_none(),
                "Idunn projection has runtime authority without Expected"
            );
            return Ok(None);
        };
        ensure_schema(expected_envelope, IDUNN_EXPECTED_INCARNATION_SCHEMA)?;
        let expected =
            IdunnExpectedIncarnationRecord::decode_canonical(&expected_envelope.payload)?;
        ensure!(
            expected.target == target,
            "Expected projection key is substituted"
        );

        let provider_anchor = anchor_envelope
            .map(|envelope| {
                ensure_schema(envelope, GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA)?;
                decode_canonical(&envelope.payload, "provider trust anchor")
            })
            .transpose()?;

        let activation = activation_envelope
            .map(|envelope| {
                ensure_schema(envelope, IDUNN_RUNTIME_ACTIVATION_SCHEMA)?;
                IdunnRuntimeActivationRecord::decode_canonical(&envelope.payload)
            })
            .transpose()?;

        let current_lease = lease_envelope
            .map(|envelope| {
                ensure_schema(envelope, IDUNN_PROCESS_WRITE_LEASE_SCHEMA)?;
                IdunnProcessWriteLeaseRecord::decode_canonical(&envelope.payload)
            })
            .transpose()?;
        let projection = IdunnRuntimeProjection {
            expected,
            provider_anchor,
            activation,
            current_lease,
        };
        projection.validate()?;
        Ok(Some(projection))
    }
}

pub trait OdinCorrelationSigner {
    fn identity_id(&self) -> &str;
    fn public_key(&self) -> &[u8];
    fn sign_correlation(&self, unsigned_payload: &[u8]) -> Result<Vec<u8>>;
}

impl OdinCorrelationSigner for ServiceIdentitySigner<OdinTopologyIdentity> {
    fn identity_id(&self) -> &str {
        &self.entry().identity_id
    }

    fn public_key(&self) -> &[u8] {
        &self.entry().public_key
    }

    fn sign_correlation(&self, unsigned_payload: &[u8]) -> Result<Vec<u8>> {
        Ok(self
            .sign::<OdinRuntimeTopologyCorrelationPurpose>(unsigned_payload)
            .signature)
    }
}

impl<T: OdinCorrelationSigner + ?Sized> OdinCorrelationSigner for &T {
    fn identity_id(&self) -> &str {
        (*self).identity_id()
    }

    fn public_key(&self) -> &[u8] {
        (*self).public_key()
    }

    fn sign_correlation(&self, unsigned_payload: &[u8]) -> Result<Vec<u8>> {
        (*self).sign_correlation(unsigned_payload)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedPresence {
    pub canonical_bytes: Vec<u8>,
    pub trusted_received_at_unix_millis: u64,
    pub stored_at: String,
}

impl AdmittedPresence {
    fn new(canonical_bytes: Vec<u8>, trusted_received_at_unix_millis: u64) -> Result<Self> {
        Ok(Self {
            canonical_bytes,
            trusted_received_at_unix_millis,
            stored_at: rfc3339_millis(trusted_received_at_unix_millis)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCorrelation {
    pub canonical_bytes: Vec<u8>,
    pub stored_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OdinStoreQuery {
    pub target: String,
    pub provider_signer_identity_id: String,
    pub dependency_targets: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OdinStoreSnapshot {
    pub query: OdinStoreQuery,
    pub presence: Option<AdmittedPresence>,
    pub correlation: Option<StoredCorrelation>,
    pub dependency_correlations: BTreeMap<String, Option<StoredCorrelation>>,
}

pub trait OdinTopologyStore {
    fn read(&self, query: OdinStoreQuery) -> Result<OdinStoreSnapshot>;

    fn compare_exchange(
        &self,
        observed: &OdinStoreSnapshot,
        replacement_presence: Option<&AdmittedPresence>,
        replacement_correlation: &StoredCorrelation,
    ) -> Result<bool>;

    fn withdraw_correlation(&self, target: &str) -> Result<()>;
}

/// Odin's durable replay and correlation store. It persists existing CultNet
/// documents directly instead of inventing a parallel admission schema.
pub struct CultCacheOdinTopologyStore {
    path: PathBuf,
}

impl CultCacheOdinTopologyStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl OdinTopologyStore for CultCacheOdinTopologyStore {
    fn read(&self, mut query: OdinStoreQuery) -> Result<OdinStoreSnapshot> {
        query.dependency_targets.sort();
        query.dependency_targets.dedup();
        let entries = if self.path.is_file() {
            SingleFileMessagePackBackingStore::new(&self.path).pull_all_read_only_snapshot()?
        } else {
            Vec::new()
        };
        let presence_key = presence_store_key(&query.target, &query.provider_signer_identity_id);
        let presence = unique_envelope(
            &entries,
            GameCultRuntimePresenceHealthRecord::TYPE,
            &presence_key,
        )?
        .map(decode_presence_envelope)
        .transpose()?;
        let correlation = unique_envelope(
            &entries,
            OdinRuntimeTopologyCorrelationRecord::TYPE,
            &query.target,
        )?
        .map(decode_correlation_envelope)
        .transpose()?;
        let mut dependency_correlations = BTreeMap::new();
        for dependency_target in &query.dependency_targets {
            let current = unique_envelope(
                &entries,
                OdinRuntimeTopologyCorrelationRecord::TYPE,
                dependency_target,
            )?
            .map(decode_correlation_envelope)
            .transpose()?;
            dependency_correlations.insert(dependency_target.clone(), current);
        }
        Ok(OdinStoreSnapshot {
            query,
            presence,
            correlation,
            dependency_correlations,
        })
    }

    fn compare_exchange(
        &self,
        observed: &OdinStoreSnapshot,
        replacement_presence: Option<&AdmittedPresence>,
        replacement_correlation: &StoredCorrelation,
    ) -> Result<bool> {
        let presence_key = presence_store_key(
            &observed.query.target,
            &observed.query.provider_signer_identity_id,
        );
        let mut conditions = BTreeMap::new();
        insert_condition(
            &mut conditions,
            CultCacheExpectedEnvelope {
                key: presence_key.clone(),
                r#type: GameCultRuntimePresenceHealthRecord::TYPE.into(),
                current: observed
                    .presence
                    .as_ref()
                    .map(|presence| presence_envelope(&presence_key, presence)),
            },
        )?;
        insert_condition(
            &mut conditions,
            CultCacheExpectedEnvelope {
                key: observed.query.target.clone(),
                r#type: OdinRuntimeTopologyCorrelationRecord::TYPE.into(),
                current: observed
                    .correlation
                    .as_ref()
                    .map(|correlation| correlation_envelope(&observed.query.target, correlation)),
            },
        )?;
        for (target, correlation) in &observed.dependency_correlations {
            insert_condition(
                &mut conditions,
                CultCacheExpectedEnvelope {
                    key: target.clone(),
                    r#type: OdinRuntimeTopologyCorrelationRecord::TYPE.into(),
                    current: correlation
                        .as_ref()
                        .map(|correlation| correlation_envelope(target, correlation)),
                },
            )?;
        }
        let mut replacements = vec![correlation_envelope(
            &observed.query.target,
            replacement_correlation,
        )];
        if let Some(presence) = replacement_presence {
            replacements.push(presence_envelope(&presence_key, presence));
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        SingleFileMessagePackBackingStore::new(&self.path)
            .compare_exchange(&conditions.into_values().collect::<Vec<_>>(), &replacements)
    }

    fn withdraw_correlation(&self, target: &str) -> Result<()> {
        for _ in 0..CAS_ATTEMPTS {
            if !self.path.is_file() {
                return Ok(());
            }
            let entries =
                SingleFileMessagePackBackingStore::new(&self.path).pull_all_read_only_snapshot()?;
            let current =
                unique_envelope(&entries, OdinRuntimeTopologyCorrelationRecord::TYPE, target)?;
            let Some(current) = current else {
                return Ok(());
            };
            let replacement = entries
                .iter()
                .filter(|entry| *entry != current)
                .cloned()
                .collect::<Vec<_>>();
            if SingleFileMessagePackBackingStore::new(&self.path)
                .compare_exchange_snapshot(&entries, &replacement)?
            {
                return Ok(());
            }
        }
        bail!("Odin topology store changed repeatedly while withdrawing correlation")
    }
}

pub struct OdinTopologyAuthority<P, S, K, C> {
    projections: P,
    store: S,
    signer: K,
    clock: C,
    idunn_anchor: ServiceIdentityTrustAnchor,
    policy: AuthenticationPolicy,
}

impl<P, S, K, C> OdinTopologyAuthority<P, S, K, C>
where
    P: IdunnProjectionSource,
    S: OdinTopologyStore,
    K: OdinCorrelationSigner,
    C: Clock,
{
    pub fn new(
        projections: P,
        store: S,
        signer: K,
        clock: C,
        idunn_anchor: ServiceIdentityTrustAnchor,
        policy: AuthenticationPolicy,
    ) -> Self {
        Self {
            projections,
            store,
            signer,
            clock,
            idunn_anchor,
            policy,
        }
    }

    /// Correlate current Expected with any previously admitted raw observation.
    /// With no admitted observation this can only produce Expected, never Present.
    pub fn refresh(&self, target: &str) -> Result<Option<Vec<u8>>> {
        let Some(projection) = self.projections.current_projection(target)? else {
            self.store.withdraw_correlation(target)?;
            return Ok(None);
        };
        self.reconcile(projection, None).map(Some)
    }

    /// Authenticate and monotonically admit one exact provider-owned presence.
    pub fn admit_presence(
        &self,
        target: &str,
        canonical_presence: &[u8],
        trusted_received_at_unix_millis: u64,
    ) -> Result<Vec<u8>> {
        let projection = self
            .projections
            .current_projection(target)?
            .context("runtime presence has no current Expected projection")?;
        self.reconcile(
            projection,
            Some((canonical_presence, trusted_received_at_unix_millis)),
        )
    }

    /// Return the exact signed bytes admitted by Odin for transport.
    pub fn current_signed_correlation(&self, target: &str) -> Result<Option<Vec<u8>>> {
        self.refresh(target)
    }

    fn reconcile(
        &self,
        projection: IdunnRuntimeProjection,
        incoming: Option<(&[u8], u64)>,
    ) -> Result<Vec<u8>> {
        projection.validate()?;
        let now = self.clock.now_unix_millis()?;
        let (authority, projection_disagreements) =
            classify_runtime_authority(&projection, &self.idunn_anchor, now)?;
        let (lease_sha256, lease_disagreement) = classify_current_lease(&projection)?;
        let query = OdinStoreQuery {
            target: projection.expected.target.clone(),
            provider_signer_identity_id: projection.expected.expected_signer_identity_id.clone(),
            dependency_targets: managed_dependency_targets(&projection.expected.dependencies),
        };

        for _ in 0..CAS_ATTEMPTS {
            let observed = self.store.read(query.clone())?;
            let admitted = self.select_presence(&observed, incoming, authority.as_ref())?;
            let dependencies =
                self.dependency_evidence(&projection.expected.dependencies, &observed, now)?;
            let (presence, mut disagreements, replacement_presence) = match admitted {
                SelectedPresence::None => (None, projection_disagreements.clone(), None),
                SelectedPresence::Stale(stored) => {
                    let record = decode_presence(&stored.canonical_bytes)?;
                    let mut disagreements = projection_disagreements.clone();
                    disagreements.push(OdinTopologyDisagreement {
                        code: "stored-presence-not-current".into(),
                        expected: projection
                            .activation
                            .as_ref()
                            .map(IdunnRuntimeActivationRecord::canonical_sha256)
                            .transpose()?,
                        observed: Some(record.activation_witness_sha256),
                    });
                    (None, disagreements, None)
                }
                SelectedPresence::Current {
                    correlation,
                    replacement,
                } => {
                    let mut disagreements = projection_disagreements.clone();
                    disagreements.extend_from_slice(correlation.disagreements());
                    (Some(correlation), disagreements, replacement)
                }
            };
            if let Some(disagreement) = &lease_disagreement {
                disagreements.push(disagreement.clone());
            }
            if presence.is_none() {
                disagreements.push(OdinTopologyDisagreement {
                    code: "signed-presence-missing".into(),
                    expected: Some("dual-authenticated-runtime-presence".into()),
                    observed: None,
                });
                disagreements.extend(projection.expected.capabilities.iter().enumerate().map(
                    |(index, capability)| OdinTopologyDisagreement {
                        code: format!("expected-capability-{index:03}-missing"),
                        expected: Some(format!(
                            "{}/{}/{} capacity>={}",
                            capability.capability,
                            capability.schema,
                            capability.compatibility,
                            capability.minimum_capacity
                        )),
                        observed: None,
                    },
                ));
            }

            let observed_write_lease = if let Some(presence) = &presence {
                correlate_write_lease(
                    &projection.expected,
                    lease_sha256.as_deref(),
                    presence.claim().record().write_lease_sha256.as_deref(),
                    &presence.claim().record().state,
                    &mut disagreements,
                )
            } else {
                None
            };
            disagreements.sort_by(|left, right| left.code.cmp(&right.code));

            let prior = observed
                .correlation
                .as_ref()
                .map(|stored| decode_correlation(&stored.canonical_bytes))
                .transpose()?;
            let publisher_sequence = prior.as_ref().map_or(Ok(1), |record| {
                record
                    .publisher_sequence
                    .checked_add(1)
                    .context("Odin topology publisher sequence exhausted")
            })?;
            let presence_record = presence.as_ref().map(|value| value.claim().record());
            let ready = presence_record.is_some_and(|record| record.state == "active")
                && disagreements.is_empty()
                && dependencies
                    .iter()
                    .all(|dependency| dependency.kind == "optional" || dependency.ready);
            let mut correlation = OdinRuntimeTopologyCorrelationRecord {
                schema_version: ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA.into(),
                target: projection.expected.target.clone(),
                expected_projection_sha256: projection.expected.canonical_sha256()?,
                expected: true,
                current_activation_sha256: projection
                    .activation
                    .as_ref()
                    .map(IdunnRuntimeActivationRecord::canonical_sha256)
                    .transpose()?,
                signed_presence_sha256: presence
                    .as_ref()
                    .map(|value| value.claim().signed_presence_sha256().into()),
                observed_presence_state: presence_record.map(|record| record.state.clone()),
                observed_presence_publisher_sequence: presence_record
                    .map(|record| record.publisher_sequence),
                observed_write_lease_sha256: observed_write_lease,
                observed_capabilities: presence_record
                    .map_or_else(Vec::new, |record| record.capabilities.clone()),
                runtime_id: projection.expected.runtime_id.clone(),
                runtime_instance_id: projection
                    .activation
                    .as_ref()
                    .map(|activation| activation.runtime_instance_id.clone()),
                present: presence.is_some(),
                ready,
                dependencies,
                disagreements,
                signer_identity_id: self.signer.identity_id().into(),
                publisher_sequence,
                observed_at_unix_millis: now,
                signature_algorithm: "ed25519".into(),
                signature: Vec::new(),
            };
            if let Some(prior) = &prior
                && same_correlation_facts(prior, &correlation)
            {
                return Ok(observed
                    .correlation
                    .context("decoded prior correlation disappeared")?
                    .canonical_bytes);
            }
            correlation.signature = self
                .signer
                .sign_correlation(&correlation.unsigned_signature_payload()?)?;
            let canonical = correlation.canonical_bytes()?;
            correlation.validate_against_expected(&projection.expected, lease_sha256.as_deref())?;
            let (signed, unsigned) =
                OdinRuntimeTopologyCorrelationRecord::decode_canonical_signed_payload(&canonical)?;
            verify_service_identity_signature_with_public_key::<
                OdinTopologyIdentity,
                OdinRuntimeTopologyCorrelationPurpose,
            >(
                self.signer.public_key(),
                &unsigned,
                &ServiceIdentitySignature {
                    identity_id: signed.signer_identity_id,
                    signature: signed.signature,
                },
            )?;
            if let Some(authority) = &authority {
                authenticate_odin_runtime_topology_correlation(
                    &canonical,
                    authority,
                    lease_sha256.as_deref(),
                    self.signer.public_key(),
                    OdinTopologyAuthenticationContext {
                        trusted_received_at_unix_millis: now,
                        maximum_age_millis: self.policy.correlation_maximum_age_millis,
                        maximum_future_skew_millis: self
                            .policy
                            .correlation_maximum_future_skew_millis,
                    },
                )?;
            }
            let stored_correlation = StoredCorrelation {
                canonical_bytes: canonical.clone(),
                stored_at: rfc3339_millis(now)?,
            };
            if self.store.compare_exchange(
                &observed,
                replacement_presence.as_ref(),
                &stored_correlation,
            )? {
                return Ok(canonical);
            }
        }
        bail!("Odin topology store changed repeatedly during correlation")
    }

    fn select_presence(
        &self,
        observed: &OdinStoreSnapshot,
        incoming: Option<(&[u8], u64)>,
        authority: Option<&cultnet_rs::VerifiedRuntimeAuthority>,
    ) -> Result<SelectedPresence> {
        let stored_record = observed
            .presence
            .as_ref()
            .map(|stored| decode_presence(&stored.canonical_bytes))
            .transpose()?;
        let Some(authority) = authority else {
            ensure!(
                incoming.is_none(),
                "runtime presence has no exact current activation and provider anchor"
            );
            return Ok(observed
                .presence
                .as_ref()
                .map_or(SelectedPresence::None, |stored| {
                    SelectedPresence::Stale(stored.clone())
                }));
        };
        let selected = if let Some((bytes, received_at)) = incoming {
            if let Some(stored_presence) = observed
                .presence
                .as_ref()
                .filter(|stored| stored.canonical_bytes == bytes)
            {
                let stored_claim = authenticate_runtime_presence_claim(
                    &stored_presence.canonical_bytes,
                    authority,
                    self.presence_context(stored_presence.trusted_received_at_unix_millis),
                )?;
                return Ok(SelectedPresence::Current {
                    correlation: correlate_runtime_presence_claim(stored_claim, authority)?,
                    replacement: None,
                });
            }
            let claim = authenticate_runtime_presence_claim(
                bytes,
                authority,
                self.presence_context(received_at),
            )?;
            if let Some(stored) = &stored_record {
                if claim.record().publisher_sequence < stored.publisher_sequence {
                    bail!("runtime presence publisher sequence was reordered");
                }
                if claim.record().publisher_sequence == stored.publisher_sequence {
                    bail!("runtime presence publisher sequence was reused with different bytes");
                }
            }
            let replacement = AdmittedPresence::new(bytes.to_vec(), received_at)?;
            Some((claim, Some(replacement)))
        } else if let Some(stored) = &observed.presence {
            match authenticate_runtime_presence_claim(
                &stored.canonical_bytes,
                authority,
                self.presence_context(stored.trusted_received_at_unix_millis),
            ) {
                Ok(claim) => Some((claim, None)),
                Err(error) => {
                    let prior_matches_current =
                        observed.correlation.as_ref().is_some_and(|value| {
                            decode_correlation(&value.canonical_bytes).is_ok_and(|record| {
                                record.expected_projection_sha256 == authority.expected_sha256()
                                    && record.current_activation_sha256.as_deref()
                                        == Some(authority.activation_sha256())
                            })
                        });
                    if prior_matches_current {
                        return Err(error).context("stored current presence failed authentication");
                    }
                    return Ok(SelectedPresence::Stale(stored.clone()));
                }
            }
        } else {
            None
        };
        match selected {
            Some((claim, replacement)) => Ok(SelectedPresence::Current {
                correlation: correlate_runtime_presence_claim(claim, authority)?,
                replacement,
            }),
            None => Ok(SelectedPresence::None),
        }
    }

    fn dependency_evidence(
        &self,
        requirements: &[IdunnExpectedDependency],
        observed: &OdinStoreSnapshot,
        now: u64,
    ) -> Result<Vec<OdinRuntimeDependencyEvidence>> {
        requirements
            .iter()
            .map(|requirement| {
                let mut evidence = OdinRuntimeDependencyEvidence {
                    kind: requirement.kind.clone(),
                    capability: requirement.capability.clone(),
                    schema: requirement.schema.clone(),
                    compatibility: requirement.compatibility.clone(),
                    provider_id: requirement.provider_id.clone(),
                    provider_authority: requirement.provider_authority.clone(),
                    provider_expected_projection_sha256: requirement
                        .provider_expected_projection_sha256
                        .clone(),
                    provider_endpoint: requirement.provider_endpoint.clone(),
                    observed_capacity: None,
                    provider_evidence_sha256: None,
                    ready: false,
                };
                let Some(provider_id) = requirement.provider_id.as_deref() else {
                    return Ok(evidence);
                };
                if requirement.provider_authority.as_deref() != Some("managed-incarnation") {
                    return Ok(evidence);
                }
                let Some(provider_projection) = self.projections.current_projection(provider_id)?
                else {
                    return Ok(evidence);
                };
                provider_projection.validate()?;
                if provider_projection.expected.canonical_sha256()?
                    != requirement
                        .provider_expected_projection_sha256
                        .as_deref()
                        .unwrap_or_default()
                {
                    return Ok(evidence);
                }
                if let Some(endpoint) = requirement.provider_endpoint.as_deref()
                    && provider_projection
                        .expected
                        .route
                        .as_ref()
                        .map(|route| route.stable_endpoint.as_str())
                        != Some(endpoint)
                {
                    return Ok(evidence);
                }
                let (provider_authority, authority_disagreements) =
                    classify_runtime_authority(&provider_projection, &self.idunn_anchor, now)?;
                let Some(provider_authority) = provider_authority else {
                    return Ok(evidence);
                };
                if !authority_disagreements.is_empty() {
                    return Ok(evidence);
                }
                let Some(stored) = observed
                    .dependency_correlations
                    .get(provider_id)
                    .and_then(Option::as_ref)
                else {
                    return Ok(evidence);
                };
                let (current_lease_sha256, _) = classify_current_lease(&provider_projection)?;
                let Ok(authenticated) = authenticate_odin_runtime_topology_correlation(
                    &stored.canonical_bytes,
                    &provider_authority,
                    current_lease_sha256.as_deref(),
                    self.signer.public_key(),
                    OdinTopologyAuthenticationContext {
                        trusted_received_at_unix_millis: now,
                        maximum_age_millis: self.policy.correlation_maximum_age_millis,
                        maximum_future_skew_millis: self
                            .policy
                            .correlation_maximum_future_skew_millis,
                    },
                ) else {
                    return Ok(evidence);
                };
                let provider = authenticated.record();
                let capacity = provider
                    .observed_capabilities
                    .iter()
                    .find(|capability| {
                        capability.capability == requirement.capability
                            && capability.schema == requirement.schema
                            && capability.compatibility == requirement.compatibility
                    })
                    .map(|capability| capability.capacity);
                evidence.observed_capacity = capacity;
                evidence.provider_evidence_sha256 = Some(provider.canonical_sha256()?);
                evidence.ready = provider.ready
                    && capacity.is_some_and(|capacity| capacity >= requirement.minimum_capacity);
                Ok(evidence)
            })
            .collect()
    }

    fn presence_context(
        &self,
        trusted_received_at_unix_millis: u64,
    ) -> RuntimePresenceAuthenticationContext {
        RuntimePresenceAuthenticationContext {
            trusted_received_at_unix_millis,
            maximum_age_millis: self.policy.presence_maximum_age_millis,
            maximum_future_skew_millis: self.policy.presence_maximum_future_skew_millis,
        }
    }
}

enum SelectedPresence {
    None,
    Stale(AdmittedPresence),
    Current {
        correlation: cultnet_rs::RuntimePresenceCorrelation,
        replacement: Option<AdmittedPresence>,
    },
}

fn correlate_write_lease(
    expected: &IdunnExpectedIncarnationRecord,
    current: Option<&str>,
    observed: Option<&str>,
    presence_state: &str,
    disagreements: &mut Vec<OdinTopologyDisagreement>,
) -> Option<String> {
    if presence_state == "warming" && observed.is_none() {
        return None;
    }
    if expected.write_lease_required && current.is_some() && current == observed {
        return observed.map(str::to_string);
    }
    if expected.write_lease_required {
        disagreements.push(OdinTopologyDisagreement {
            code: "write-lease".into(),
            expected: Some(
                current
                    .unwrap_or("current-idunn-process-write-lease")
                    .into(),
            ),
            observed: observed.map(str::to_string),
        });
    } else if observed.is_some() {
        disagreements.push(OdinTopologyDisagreement {
            code: "unexpected-write-lease".into(),
            expected: None,
            observed: observed.map(str::to_string),
        });
    }
    None
}

fn same_correlation_facts(
    prior: &OdinRuntimeTopologyCorrelationRecord,
    next: &OdinRuntimeTopologyCorrelationRecord,
) -> bool {
    let mut normalized = next.clone();
    normalized.publisher_sequence = prior.publisher_sequence;
    normalized.observed_at_unix_millis = prior.observed_at_unix_millis;
    normalized.signature = prior.signature.clone();
    &normalized == prior
}

fn managed_dependency_targets(requirements: &[IdunnExpectedDependency]) -> Vec<String> {
    requirements
        .iter()
        .filter(|dependency| {
            dependency.provider_authority.as_deref() == Some("managed-incarnation")
        })
        .filter_map(|dependency| dependency.provider_id.clone())
        .collect()
}

fn classify_runtime_authority(
    projection: &IdunnRuntimeProjection,
    idunn_anchor: &ServiceIdentityTrustAnchor,
    now: u64,
) -> Result<(
    Option<cultnet_rs::VerifiedRuntimeAuthority>,
    Vec<OdinTopologyDisagreement>,
)> {
    let expected = &projection.expected;
    let expected_sha256 = expected.canonical_sha256()?;
    let mut disagreements = Vec::new();
    match &projection.provider_anchor {
        None => disagreements.push(OdinTopologyDisagreement {
            code: "provider-trust-anchor-missing".into(),
            expected: Some(expected.expected_signer_identity_id.clone()),
            observed: None,
        }),
        Some(anchor) => {
            push_disagreement(
                &mut disagreements,
                "provider-trust-anchor-id",
                Some(runtime_presence_anchor_id(&expected.target)),
                Some(anchor.trust_anchor_id.clone()),
            );
            push_disagreement(
                &mut disagreements,
                "provider-service-id",
                Some(expected.target.clone()),
                Some(anchor.service_id.clone()),
            );
            push_disagreement(
                &mut disagreements,
                "provider-runtime-id",
                Some(expected.runtime_id.clone()),
                Some(anchor.runtime_id.clone()),
            );
            push_disagreement(
                &mut disagreements,
                "provider-signer-identity",
                Some(expected.expected_signer_identity_id.clone()),
                Some(anchor.signer_identity_id.clone()),
            );
            push_disagreement(
                &mut disagreements,
                "provider-signing-purpose",
                Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SIGNING_PURPOSE.into()),
                Some(anchor.signing_purpose.clone()),
            );
            push_disagreement(
                &mut disagreements,
                "provider-signed-schema",
                Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()),
                Some(anchor.signed_schema.clone()),
            );
            if anchor.bound_at_unix_millis > now
                || anchor
                    .expires_at_unix_millis
                    .is_some_and(|expires_at| now >= expires_at)
            {
                disagreements.push(OdinTopologyDisagreement {
                    code: "provider-trust-anchor-time".into(),
                    expected: Some(format!("current-at:{now}")),
                    observed: Some(format!(
                        "bound:{};expires:{}",
                        anchor.bound_at_unix_millis,
                        anchor
                            .expires_at_unix_millis
                            .map_or_else(|| "none".into(), |value| value.to_string())
                    )),
                });
            }
        }
    }
    match &projection.activation {
        None => disagreements.push(OdinTopologyDisagreement {
            code: "current-activation-missing".into(),
            expected: Some(expected_sha256.clone()),
            observed: None,
        }),
        Some(activation) => {
            if activation.idunn_signer_identity_id != idunn_anchor.identity_id {
                disagreements.push(OdinTopologyDisagreement {
                    code: "activation-idunn-signer".into(),
                    expected: Some(idunn_anchor.identity_id.clone()),
                    observed: Some(activation.idunn_signer_identity_id.clone()),
                });
            } else {
                verify_service_identity_signature::<
                    IdunnServiceIdentity,
                    IdunnRuntimeActivationPurpose,
                >(
                    idunn_anchor,
                    &activation.unsigned_signature_payload()?,
                    &ServiceIdentitySignature {
                        identity_id: activation.idunn_signer_identity_id.clone(),
                        signature: activation.signature.clone(),
                    },
                )
                .context("verifying projected Idunn runtime activation")?;
            }
            push_disagreement(
                &mut disagreements,
                "activation-expected-projection",
                Some(expected_sha256.clone()),
                Some(activation.expected_projection_sha256.clone()),
            );
            push_disagreement(
                &mut disagreements,
                "activation-runtime-id",
                Some(expected.runtime_id.clone()),
                Some(activation.runtime_id.clone()),
            );
        }
    }
    disagreements.sort_by(|left, right| left.code.cmp(&right.code));
    let authority = if disagreements.is_empty() {
        let anchor = projection.provider_anchor.as_ref().unwrap();
        let activation = projection.activation.as_ref().unwrap();
        Some(verify_runtime_authority(
            expected,
            activation,
            idunn_anchor,
            &anchor.signer_public_key,
        )?)
    } else {
        None
    };
    Ok((authority, disagreements))
}

fn classify_current_lease(
    projection: &IdunnRuntimeProjection,
) -> Result<(Option<String>, Option<OdinTopologyDisagreement>)> {
    let Some(lease) = &projection.current_lease else {
        return Ok((None, None));
    };
    let lease_sha256 = lease.canonical_sha256()?;
    let activation = projection.activation.as_ref();
    let binds = activation
        .map(|activation| lease_binds(&projection.expected, activation, lease))
        .transpose()?
        .unwrap_or(false);
    if projection.expected.write_lease_required && binds {
        return Ok((Some(lease_sha256), None));
    }
    let expected_activation = activation
        .map(IdunnRuntimeActivationRecord::canonical_sha256)
        .transpose()?
        .unwrap_or_else(|| "none".into());
    Ok((
        None,
        Some(OdinTopologyDisagreement {
            code: "projected-write-lease".into(),
            expected: Some(format!(
                "expected:{};activation:{expected_activation}",
                projection.expected.canonical_sha256()?
            )),
            observed: Some(lease_sha256),
        }),
    ))
}

fn lease_binds(
    expected: &IdunnExpectedIncarnationRecord,
    activation: &IdunnRuntimeActivationRecord,
    lease: &IdunnProcessWriteLeaseRecord,
) -> Result<bool> {
    Ok(lease.target == expected.target
        && lease.expected_projection_sha256 == expected.canonical_sha256()?
        && lease.plan_id == expected.plan_id
        && lease.incarnation_id == expected.incarnation_id
        && lease.sealed_release_id == expected.sealed_release_id
        && lease.activation_witness_sha256 == activation.canonical_sha256()?
        && Some(lease.state_schema_generation.as_str())
            == expected.state_schema_generation.as_deref()
        && Some(lease.state_contract_sha256.as_str()) == expected.state_contract_sha256.as_deref()
        && lease.runtime_id == expected.runtime_id
        && lease.runtime_instance_id == activation.runtime_instance_id)
}

fn push_disagreement(
    disagreements: &mut Vec<OdinTopologyDisagreement>,
    code: &str,
    expected: Option<String>,
    observed: Option<String>,
) {
    if expected != observed {
        disagreements.push(OdinTopologyDisagreement {
            code: code.into(),
            expected,
            observed,
        });
    }
}

fn runtime_presence_anchor_id(target: &str) -> String {
    format!("root/{target}/runtime-presence")
}

fn presence_store_key(target: &str, signer_identity_id: &str) -> String {
    format!("{target}/{signer_identity_id}")
}

fn unique_envelope<'a>(
    entries: &'a [CultCacheEnvelope],
    record_type: &str,
    key: &str,
) -> Result<Option<&'a CultCacheEnvelope>> {
    let mut matches = entries
        .iter()
        .filter(|entry| entry.r#type == record_type && entry.key == key);
    let current = matches.next();
    ensure!(matches.next().is_none(), "CultCache identity is ambiguous");
    Ok(current)
}

fn ensure_schema(envelope: &CultCacheEnvelope, schema: &str) -> Result<()> {
    ensure!(
        envelope.schema_id.as_deref() == Some(schema),
        "CultCache envelope schema is substituted"
    );
    Ok(())
}

fn decode_canonical<T>(bytes: &[u8], label: &str) -> Result<T>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let value = rmp_serde::from_slice(bytes).with_context(|| format!("decoding {label}"))?;
    ensure!(
        rmp_serde::to_vec(&value)? == bytes,
        "{label} is not canonical positional MessagePack"
    );
    Ok(value)
}

fn decode_presence(bytes: &[u8]) -> Result<GameCultRuntimePresenceHealthRecord> {
    let record: GameCultRuntimePresenceHealthRecord = decode_canonical(bytes, "runtime presence")?;
    record.validate()?;
    Ok(record)
}

fn decode_correlation(bytes: &[u8]) -> Result<OdinRuntimeTopologyCorrelationRecord> {
    Ok(OdinRuntimeTopologyCorrelationRecord::decode_canonical_signed_payload(bytes)?.0)
}

fn decode_presence_envelope(envelope: &CultCacheEnvelope) -> Result<AdmittedPresence> {
    ensure_schema(envelope, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA)?;
    decode_presence(&envelope.payload)?;
    Ok(AdmittedPresence {
        canonical_bytes: envelope.payload.clone(),
        trusted_received_at_unix_millis: parse_rfc3339_millis(&envelope.stored_at)?,
        stored_at: envelope.stored_at.clone(),
    })
}

fn decode_correlation_envelope(envelope: &CultCacheEnvelope) -> Result<StoredCorrelation> {
    ensure_schema(envelope, ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA)?;
    let record = decode_correlation(&envelope.payload)?;
    ensure!(
        record.target == envelope.key,
        "stored correlation target is substituted"
    );
    ensure!(
        parse_rfc3339_millis(&envelope.stored_at)? == record.observed_at_unix_millis,
        "stored correlation receipt time is substituted"
    );
    Ok(StoredCorrelation {
        canonical_bytes: envelope.payload.clone(),
        stored_at: envelope.stored_at.clone(),
    })
}

fn presence_envelope(key: &str, presence: &AdmittedPresence) -> CultCacheEnvelope {
    CultCacheEnvelope {
        key: key.into(),
        r#type: GameCultRuntimePresenceHealthRecord::TYPE.into(),
        payload: presence.canonical_bytes.clone(),
        stored_at: presence.stored_at.clone(),
        schema_id: Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()),
    }
}

fn correlation_envelope(target: &str, correlation: &StoredCorrelation) -> CultCacheEnvelope {
    CultCacheEnvelope {
        key: target.into(),
        r#type: OdinRuntimeTopologyCorrelationRecord::TYPE.into(),
        payload: correlation.canonical_bytes.clone(),
        stored_at: correlation.stored_at.clone(),
        schema_id: Some(ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA.into()),
    }
}

fn insert_condition(
    conditions: &mut BTreeMap<(String, String), CultCacheExpectedEnvelope>,
    condition: CultCacheExpectedEnvelope,
) -> Result<()> {
    let identity = (condition.r#type.clone(), condition.key.clone());
    if let Some(current) = conditions.get(&identity) {
        ensure!(
            current.current == condition.current,
            "store read set is incoherent"
        );
    } else {
        conditions.insert(identity, condition);
    }
    Ok(())
}

fn rfc3339_millis(value: u64) -> Result<String> {
    let value: i64 = value
        .try_into()
        .context("timestamp exceeds RFC3339 range")?;
    Ok(DateTime::<Utc>::from_timestamp_millis(value)
        .context("timestamp exceeds RFC3339 range")?
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn parse_rfc3339_millis(value: &str) -> Result<u64> {
    DateTime::parse_from_rfc3339(value)?
        .timestamp_millis()
        .try_into()
        .context("CultCache timestamp predates Unix epoch")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cultnet_rs::{
        GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA, GameCultProviderHealthIdentity,
        GameCultRuntimeCapability, GameCultRuntimePresenceHealthPurpose, IdunnExpectedCapability,
        IdunnExpectedRoute, IdunnRuntimeActivationLaunch, IdunnRuntimeActivationSigner,
        IdunnServiceIdentity, enroll_service_identity_at,
    };
    use tempfile::TempDir;

    use super::*;

    const NOW: u64 = 1_000_000;

    #[derive(Clone, Copy)]
    struct FixedClock(u64);

    impl Clock for FixedClock {
        fn now_unix_millis(&self) -> Result<u64> {
            Ok(self.0)
        }
    }

    struct TestService {
        projection: IdunnRuntimeProjection,
        provider_signer: ServiceIdentitySigner<GameCultProviderHealthIdentity>,
        activation_signer: IdunnRuntimeActivationSigner,
    }

    impl TestService {
        fn signed_presence<F>(&self, sequence: u64, edit: F) -> Result<Vec<u8>>
        where
            F: FnOnce(&mut GameCultRuntimePresenceHealthRecord),
        {
            let expected = &self.projection.expected;
            let activation = self.projection.activation.as_ref().unwrap();
            let mut record = GameCultRuntimePresenceHealthRecord {
                schema_version: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
                target: expected.target.clone(),
                expected_projection_sha256: expected.canonical_sha256()?,
                plan_id: expected.plan_id.clone(),
                incarnation_id: expected.incarnation_id.clone(),
                sealed_release_id: expected.sealed_release_id.clone(),
                activation_witness_sha256: activation.canonical_sha256()?,
                state_schema_generation: expected.state_schema_generation.clone(),
                state_contract_sha256: expected.state_contract_sha256.clone(),
                runtime_id: expected.runtime_id.clone(),
                runtime_instance_id: activation.runtime_instance_id.clone(),
                bound_endpoint: expected
                    .route
                    .as_ref()
                    .map(|route| route.candidate_endpoint.clone()),
                capabilities: expected
                    .capabilities
                    .iter()
                    .map(|capability| GameCultRuntimeCapability {
                        capability: capability.capability.clone(),
                        schema: capability.schema.clone(),
                        compatibility: capability.compatibility.clone(),
                        capacity: capability.minimum_capacity,
                    })
                    .collect(),
                health_contract: expected.health_contract.clone(),
                state: "active".into(),
                detail: "ready".into(),
                write_lease_sha256: self
                    .projection
                    .current_lease
                    .as_ref()
                    .map(IdunnProcessWriteLeaseRecord::canonical_sha256)
                    .transpose()?,
                signer_identity_id: self.provider_signer.entry().identity_id.clone(),
                publisher_sequence: sequence,
                observed_at_unix_millis: NOW - 10,
                signature_algorithm: "ed25519".into(),
                signature: Vec::new(),
                activation_signer_identity_id: activation.activation_signer_identity_id.clone(),
                activation_signature: Vec::new(),
            };
            edit(&mut record);
            let proof_payload = record.canonical_proof_payload()?;
            record.signature = self
                .provider_signer
                .sign::<GameCultRuntimePresenceHealthPurpose>(&proof_payload)
                .signature;
            record.activation_signature = self.activation_signer.sign_presence_proof(&record)?;
            record.validate()?;
            Ok(rmp_serde::to_vec(&record)?)
        }
    }

    struct TestWorld {
        _temp: TempDir,
        projection_path: PathBuf,
        topology_path: PathBuf,
        identity_root: PathBuf,
        idunn_signer: ServiceIdentitySigner<IdunnServiceIdentity>,
        idunn_anchor: ServiceIdentityTrustAnchor,
        odin_signer: ServiceIdentitySigner<OdinTopologyIdentity>,
    }

    impl TestWorld {
        fn new() -> Result<Self> {
            let temp = tempfile::tempdir()?;
            let identity_root = temp.path().join("identities");
            let idunn_signer = enroll_service_identity_at::<IdunnServiceIdentity>(
                &identity_root.join("idunn.cc"),
            )?;
            let idunn_anchor = idunn_signer.trust_anchor()?;
            let odin_signer =
                enroll_service_identity_at::<OdinTopologyIdentity>(&identity_root.join("odin.cc"))?;
            Ok(Self {
                projection_path: temp.path().join("idunn-projection.cc"),
                topology_path: temp.path().join("odin-topology.cc"),
                identity_root,
                idunn_signer,
                idunn_anchor,
                odin_signer,
                _temp: temp,
            })
        }

        fn service(
            &self,
            target: &str,
            dependencies: Vec<IdunnExpectedDependency>,
            stateful: bool,
        ) -> Result<TestService> {
            let provider_signer = enroll_service_identity_at::<GameCultProviderHealthIdentity>(
                &self.identity_root.join(format!("{target}-provider.cc")),
            )?;
            let expected = IdunnExpectedIncarnationRecord {
                schema_version: IDUNN_EXPECTED_INCARNATION_SCHEMA.into(),
                target: target.into(),
                plan_id: digest('1'),
                incarnation_id: format!("{target}/generation-1"),
                sealed_release_id: digest('2'),
                source_repository: format!("github.com/GameCult/{target}"),
                source_revision: "3".repeat(40),
                recipe_sha256: digest('4'),
                runtime_id: format!("{target}-runtime"),
                expected_signer_identity_id: provider_signer.entry().identity_id.clone(),
                health_contract: format!("{target}.runtime-health.v1"),
                artifact_sha256: digest('5'),
                state_schema_generation: stateful.then(|| "state-v1".into()),
                state_contract_sha256: stateful.then(|| digest('6')),
                write_lease_required: stateful,
                route: Some(IdunnExpectedRoute {
                    route_id: format!("{target}-route"),
                    transport: "tcp".into(),
                    stable_endpoint: format!("tcp://{target}.internal:1000"),
                    candidate_endpoint: format!("tcp://127.0.0.1:{}", target.len() + 10_000),
                }),
                capabilities: vec![IdunnExpectedCapability {
                    capability: "data".into(),
                    schema: "data.v1".into(),
                    compatibility: "v1".into(),
                    minimum_capacity: 1,
                }],
                dependencies,
            };
            expected.validate()?;
            let launch = IdunnRuntimeActivationLaunch::issue(
                &expected,
                digest('7'),
                NOW - 20,
                &self.idunn_signer,
            )?;
            let activation = launch.activation().clone();
            let mut credential = Vec::new();
            let emitted = launch.write_credential(&mut credential)?;
            ensure!(
                emitted == activation,
                "activation launch changed while emitted"
            );
            let activation_signer =
                IdunnRuntimeActivationSigner::from_credential_reader(credential.as_slice())?;
            let provider_anchor = GameCultServiceTrustAnchorRecord {
                schema_version: GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into(),
                trust_anchor_id: runtime_presence_anchor_id(target),
                service_id: target.into(),
                runtime_id: expected.runtime_id.clone(),
                signer_identity_id: provider_signer.entry().identity_id.clone(),
                signer_public_key: provider_signer.entry().public_key.clone(),
                signature_algorithm: "ed25519".into(),
                signing_purpose: GAMECULT_RUNTIME_PRESENCE_HEALTH_SIGNING_PURPOSE.into(),
                signed_schema: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
                binding_authority: "root".into(),
                bound_at_unix_millis: NOW - 100,
                expires_at_unix_millis: None,
                private_state_exposed: false,
            };
            let current_lease = if stateful {
                Some(IdunnProcessWriteLeaseRecord {
                    schema_version: IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into(),
                    target: target.into(),
                    expected_projection_sha256: expected.canonical_sha256()?,
                    plan_id: expected.plan_id.clone(),
                    incarnation_id: expected.incarnation_id.clone(),
                    sealed_release_id: expected.sealed_release_id.clone(),
                    activation_witness_sha256: activation.canonical_sha256()?,
                    state_schema_generation: expected.state_schema_generation.clone().unwrap(),
                    state_contract_sha256: expected.state_contract_sha256.clone().unwrap(),
                    runtime_id: expected.runtime_id.clone(),
                    runtime_instance_id: activation.runtime_instance_id.clone(),
                    warming_presence_sha256: digest('8'),
                    lease_epoch: 1,
                    issued_at_unix_millis: NOW - 5,
                })
            } else {
                None
            };
            let projection = IdunnRuntimeProjection {
                expected,
                provider_anchor: Some(provider_anchor),
                activation: Some(activation),
                current_lease,
            };
            projection.validate()?;
            self.publish_projection(&projection)?;
            Ok(TestService {
                projection,
                provider_signer,
                activation_signer,
            })
        }

        fn publish_projection(&self, projection: &IdunnRuntimeProjection) -> Result<()> {
            let store = SingleFileMessagePackBackingStore::new(&self.projection_path);
            let current = if self.projection_path.is_file() {
                store.pull_all_read_only_snapshot()?
            } else {
                Vec::new()
            };
            let anchor_key = runtime_presence_anchor_id(&projection.expected.target);
            let mut next: Vec<_> = current
                .iter()
                .filter(|envelope| {
                    !((envelope.r#type == IdunnExpectedIncarnationRecord::TYPE
                        || envelope.r#type == IdunnRuntimeActivationRecord::TYPE
                        || envelope.r#type == IdunnProcessWriteLeaseRecord::TYPE)
                        && envelope.key == projection.expected.target)
                        && !(envelope.r#type == GameCultServiceTrustAnchorRecord::TYPE
                            && envelope.key == anchor_key)
                })
                .cloned()
                .collect();
            next.push(CultCacheEnvelope {
                key: projection.expected.target.clone(),
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: projection.expected.canonical_bytes()?,
                stored_at: rfc3339_millis(NOW - 30)?,
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            });
            if let Some(anchor) = &projection.provider_anchor {
                next.push(CultCacheEnvelope {
                    key: anchor_key,
                    r#type: GameCultServiceTrustAnchorRecord::TYPE.into(),
                    payload: rmp_serde::to_vec(anchor)?,
                    stored_at: rfc3339_millis(anchor.bound_at_unix_millis)?,
                    schema_id: Some(GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into()),
                });
            }
            if let Some(activation) = &projection.activation {
                next.push(CultCacheEnvelope {
                    key: projection.expected.target.clone(),
                    r#type: IdunnRuntimeActivationRecord::TYPE.into(),
                    payload: activation.canonical_bytes()?,
                    stored_at: rfc3339_millis(activation.issued_at_unix_millis)?,
                    schema_id: Some(IDUNN_RUNTIME_ACTIVATION_SCHEMA.into()),
                });
            }
            if let Some(lease) = &projection.current_lease {
                next.push(CultCacheEnvelope {
                    key: projection.expected.target.clone(),
                    r#type: IdunnProcessWriteLeaseRecord::TYPE.into(),
                    payload: lease.canonical_bytes()?,
                    stored_at: rfc3339_millis(lease.issued_at_unix_millis)?,
                    schema_id: Some(IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into()),
                });
            }
            ensure!(
                store.compare_exchange_snapshot(&current, &next)?,
                "test projection CAS failed"
            );
            Ok(())
        }

        fn engine(
            &self,
            now: u64,
        ) -> OdinTopologyAuthority<
            CultCacheIdunnProjectionSource,
            CultCacheOdinTopologyStore,
            &ServiceIdentitySigner<OdinTopologyIdentity>,
            FixedClock,
        > {
            OdinTopologyAuthority::new(
                CultCacheIdunnProjectionSource::new(&self.projection_path),
                CultCacheOdinTopologyStore::new(&self.topology_path),
                &self.odin_signer,
                FixedClock(now),
                self.idunn_anchor.clone(),
                AuthenticationPolicy::default(),
            )
        }

        fn replace_provider_generation(&self, service: &mut TestService) -> Result<()> {
            service.projection.expected.plan_id = digest('9');
            service.projection.expected.incarnation_id =
                format!("{}/generation-2", service.projection.expected.target);
            service.projection.expected.validate()?;
            let launch = IdunnRuntimeActivationLaunch::issue(
                &service.projection.expected,
                digest('a'),
                NOW + 10,
                &self.idunn_signer,
            )?;
            service.projection.activation = Some(launch.activation().clone());
            service.projection.current_lease = None;
            self.publish_projection(&service.projection)
        }
    }

    fn digest(byte: char) -> String {
        format!("sha256-{}", byte.to_string().repeat(64))
    }

    fn decode_signed(bytes: &[u8]) -> Result<OdinRuntimeTopologyCorrelationRecord> {
        Ok(OdinRuntimeTopologyCorrelationRecord::decode_canonical_signed_payload(bytes)?.0)
    }

    #[test]
    fn partial_or_mismatched_projection_remains_signed_expected_only() -> Result<()> {
        let world = TestWorld::new()?;
        let mut service = world.service("ghostlight", Vec::new(), false)?;
        service.projection.activation = None;
        let anchor = service.projection.provider_anchor.as_mut().unwrap();
        anchor.trust_anchor_id = "root/other/runtime-presence".into();
        anchor.service_id = "other".into();
        world.publish_projection(&service.projection)?;

        let bytes = world.engine(NOW).refresh("ghostlight")?.unwrap();
        let record = decode_signed(&bytes)?;
        assert!(record.expected);
        assert!(!record.present);
        assert!(!record.ready);
        assert!(record.signed_presence_sha256.is_none());
        let codes: Vec<_> = record
            .disagreements
            .iter()
            .map(|value| value.code.as_str())
            .collect();
        assert!(codes.contains(&"current-activation-missing"));
        assert!(codes.contains(&"provider-service-id"));
        assert!(codes.contains(&"provider-trust-anchor-id"));
        assert!(codes.contains(&"signed-presence-missing"));
        Ok(())
    }

    #[test]
    fn authenticated_disagreement_is_present_but_not_ready() -> Result<()> {
        let world = TestWorld::new()?;
        let service = world.service("ghostlight", Vec::new(), false)?;
        let presence = service.signed_presence(1, |record| {
            record.health_contract = "ghostlight.other-health.v1".into();
        })?;

        let record = decode_signed(&world.engine(NOW).admit_presence(
            "ghostlight",
            &presence,
            NOW,
        )?)?;
        assert!(record.present);
        assert!(!record.ready);
        assert!(
            record
                .disagreements
                .iter()
                .any(|value| value.code == "health-contract")
        );
        Ok(())
    }

    #[test]
    fn exact_active_presence_and_current_lease_are_ready() -> Result<()> {
        let world = TestWorld::new()?;
        let service = world.service("ghostlight", Vec::new(), true)?;
        let presence = service.signed_presence(1, |_| {})?;

        let record = decode_signed(&world.engine(NOW).admit_presence(
            "ghostlight",
            &presence,
            NOW,
        )?)?;
        assert!(record.present);
        assert!(record.ready);
        assert_eq!(
            record.observed_write_lease_sha256,
            service
                .projection
                .current_lease
                .as_ref()
                .map(IdunnProcessWriteLeaseRecord::canonical_sha256)
                .transpose()?
        );
        Ok(())
    }

    #[test]
    fn stale_projected_lease_is_visible_while_candidate_warming_is_present() -> Result<()> {
        let world = TestWorld::new()?;
        let mut service = world.service("ghostlight", Vec::new(), true)?;
        let stale = service.projection.current_lease.as_mut().unwrap();
        stale.expected_projection_sha256 = digest('a');
        stale.plan_id = digest('b');
        stale.incarnation_id = "ghostlight/incumbent".into();
        stale.sealed_release_id = digest('c');
        stale.activation_witness_sha256 = digest('d');
        stale.runtime_instance_id = digest('e');
        world.publish_projection(&service.projection)?;
        let warming = service.signed_presence(1, |record| {
            record.state = "warming".into();
            record.detail = "starting".into();
            record.write_lease_sha256 = None;
        })?;

        let record = decode_signed(&world.engine(NOW).admit_presence(
            "ghostlight",
            &warming,
            NOW,
        )?)?;
        assert!(record.present);
        assert!(!record.ready);
        assert_eq!(record.observed_presence_state.as_deref(), Some("warming"));
        assert!(record.observed_write_lease_sha256.is_none());
        assert!(
            record
                .disagreements
                .iter()
                .any(|value| value.code == "projected-write-lease")
        );
        Ok(())
    }

    #[test]
    fn provider_sequence_admission_is_monotonic_and_exact_retry_is_idempotent() -> Result<()> {
        let world = TestWorld::new()?;
        let service = world.service("ghostlight", Vec::new(), false)?;
        let sequence_two = service.signed_presence(2, |_| {})?;
        let engine = world.engine(NOW);

        let first = engine.admit_presence("ghostlight", &sequence_two, NOW)?;
        let duplicate = engine.admit_presence("ghostlight", &sequence_two, NOW + 60_000)?;
        assert_eq!(duplicate, first);
        assert_eq!(
            engine.current_signed_correlation("ghostlight")?,
            Some(first)
        );

        let lower = service.signed_presence(1, |_| {})?;
        assert!(engine.admit_presence("ghostlight", &lower, NOW).is_err());
        let reused = service.signed_presence(2, |record| record.detail = "different".into())?;
        assert!(engine.admit_presence("ghostlight", &reused, NOW).is_err());

        let entries = SingleFileMessagePackBackingStore::new(&world.topology_path)
            .pull_all_read_only_snapshot()?;
        let stored = unique_envelope(
            &entries,
            GameCultRuntimePresenceHealthRecord::TYPE,
            &presence_store_key(
                "ghostlight",
                &service.projection.expected.expected_signer_identity_id,
            ),
        )?
        .unwrap();
        assert_eq!(stored.payload, sequence_two);
        assert_eq!(parse_rfc3339_millis(&stored.stored_at)?, NOW);
        Ok(())
    }

    #[test]
    fn restart_rehydrates_exact_bytes_without_retimestamping() -> Result<()> {
        let world = TestWorld::new()?;
        let service = world.service("ghostlight", Vec::new(), false)?;
        let presence = service.signed_presence(1, |_| {})?;
        let first = world
            .engine(NOW)
            .admit_presence("ghostlight", &presence, NOW)?;

        let after_restart = world.engine(NOW + 100).refresh("ghostlight")?.unwrap();
        assert_eq!(after_restart, first);
        assert_eq!(decode_signed(&after_restart)?.observed_at_unix_millis, NOW);
        Ok(())
    }

    #[test]
    fn withdrawn_expected_removes_current_correlation_but_preserves_presence_history() -> Result<()>
    {
        let world = TestWorld::new()?;
        let service = world.service("ghostlight", Vec::new(), false)?;
        let presence = service.signed_presence(1, |_| {})?;
        let engine = world.engine(NOW);
        engine.admit_presence("ghostlight", &presence, NOW)?;
        assert!(engine.current_signed_correlation("ghostlight")?.is_some());

        std::fs::remove_file(&world.projection_path)?;
        assert!(engine.current_signed_correlation("ghostlight")?.is_none());

        let entries = SingleFileMessagePackBackingStore::new(&world.topology_path)
            .pull_all_read_only_snapshot()?;
        assert!(entries.iter().any(|entry| {
            entry.r#type == GameCultRuntimePresenceHealthRecord::TYPE && entry.payload == presence
        }));
        assert!(!entries.iter().any(|entry| {
            entry.r#type == OdinRuntimeTopologyCorrelationRecord::TYPE && entry.key == "ghostlight"
        }));
        Ok(())
    }

    #[test]
    fn dependencies_consume_only_exact_current_admitted_correlations() -> Result<()> {
        let world = TestWorld::new()?;
        let mut provider = world.service("provider", Vec::new(), false)?;
        let provider_expected_sha256 = provider.projection.expected.canonical_sha256()?;
        let provider_endpoint = provider
            .projection
            .expected
            .route
            .as_ref()
            .unwrap()
            .stable_endpoint
            .clone();
        let requirement = IdunnExpectedDependency {
            kind: "required".into(),
            capability: "data".into(),
            schema: "data.v1".into(),
            compatibility: "v1".into(),
            minimum_capacity: 1,
            startup: "before-promotion".into(),
            provider_id: Some("provider".into()),
            provider_authority: Some("managed-incarnation".into()),
            provider_expected_projection_sha256: Some(provider_expected_sha256),
            provider_endpoint: Some(provider_endpoint),
        };
        let consumer = world.service("consumer", vec![requirement], false)?;
        let provider_presence = provider.signed_presence(1, |_| {})?;
        let consumer_presence = consumer.signed_presence(1, |_| {})?;
        let engine = world.engine(NOW);

        engine.admit_presence("provider", &provider_presence, NOW)?;
        let ready = decode_signed(&engine.admit_presence("consumer", &consumer_presence, NOW)?)?;
        assert!(ready.ready);
        assert!(ready.dependencies[0].ready);
        assert!(ready.dependencies[0].provider_evidence_sha256.is_some());

        world.replace_provider_generation(&mut provider)?;
        let no_longer_exact = decode_signed(&engine.refresh("consumer")?.unwrap())?;
        assert!(no_longer_exact.present);
        assert!(!no_longer_exact.ready);
        assert!(!no_longer_exact.dependencies[0].ready);
        assert!(
            no_longer_exact.dependencies[0]
                .provider_evidence_sha256
                .is_none()
        );
        Ok(())
    }
}

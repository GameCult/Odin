use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::net::{SocketAddr, UdpSocket};
use std::os::fd::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use cultcache_rs::{CultCacheEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore};
use cultmesh_rs::{
    CultMeshRudpDocumentServer, CultMeshRudpDocumentServerOptions, CultMeshRudpPollOutcome,
    CultMeshRudpRawDocumentReceipt, CultMeshRudpRawDocumentSink, CultMeshRudpSnapshotQuery,
    CultMeshRudpSnapshotSource, CultMeshSystemClock,
};
use cultnet_rs::{
    CultNetRawDocumentRecord, CultNetRawPayloadEncoding, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA,
    GameCultProviderHealthIdentity, GameCultRuntimeCapability,
    GameCultRuntimePresenceHealthPurpose, GameCultRuntimePresenceHealthRecord,
    IDUNN_EXPECTED_INCARNATION_SCHEMA, IDUNN_PROCESS_WRITE_LEASE_SCHEMA,
    IDUNN_RUNTIME_ACTIVATION_CREDENTIAL_NAME, IDUNN_RUNTIME_ACTIVATION_SCHEMA,
    IdunnExpectedIncarnationRecord, IdunnProcessWriteLeaseRecord, IdunnRuntimeActivationRecord,
    IdunnRuntimeActivationSigner, IdunnServiceIdentity, OdinTopologyIdentity,
    ServiceIdentityProfile,
    ServiceIdentitySigner, ServiceIdentityTrustAnchor, derive_service_identity_id,
    open_service_identity_credential_reader, verify_runtime_authority,
};
use fs2::FileExt;
use odin_daemon::{
    AuthenticationPolicy, CultCacheIdunnProjectionSource, ForeignStoreWrite,
    IdunnProjectionSnapshot, IdunnProjectionSource, IncarnationRef, MemoryOdinTopologyStore,
    OdinTopologyAuthority, PresenceAuthorityRefused, ProjectionUnreadable, SystemClock,
};

const TARGET: &str = "odin";
const HEALTH_CONTRACT: &str = "odin.runtime-health.v1";
const RENDEZVOUS_CAPABILITY: &str = "odin.verse-rendezvous";
const RENDEZVOUS_SCHEMA: &str = "odin.verse-topology.v1";
const RENDEZVOUS_COMPATIBILITY: &str = "v1";
const STATE_SCHEMA_GENERATION: &str = "odin-v2";
// rmp-serde SHA-256 of deployment/idunn/recipe.toml's exact [state] value.
const STATE_CONTRACT_SHA256: &str =
    "sha256-4f2f2dcd931d16f6b02bf295f41227b867aa982765661d55fa9f29fb2db7e449";

const RUNTIME_BUNDLE_ENVIRONMENT: &str = "GAMECULT_IDUNN_RUNTIME_BUNDLE";
const CANDIDATE_BIND_ENVIRONMENT: &str = "GAMECULT_IDUNN_CANDIDATE_BIND";
const PROCESS_WRITE_LEASE_ENVIRONMENT: &str = "GAMECULT_IDUNN_PROCESS_WRITE_LEASE";
const TOPOLOGY_IDENTITY_ENVIRONMENT: &str = "ODIN_TOPOLOGY_IDENTITY";
const SYSTEMD_LISTEN_PID_ENVIRONMENT: &str = "LISTEN_PID";
const SYSTEMD_LISTEN_FDS_ENVIRONMENT: &str = "LISTEN_FDS";
const SYSTEMD_LISTEN_FDNAMES_ENVIRONMENT: &str = "LISTEN_FDNAMES";
const ACTIVATION_SIGNER_FD_NAME: &str = IDUNN_RUNTIME_ACTIVATION_CREDENTIAL_NAME;
const PROVIDER_SIGNER_FD_NAME: &str = "gamecult-runtime-presence-identity";
const SYSTEMD_LISTEN_FDS_START: RawFd = 3;

// Long enough for Idunn to warm, fence the incumbent, and grant the lease. A
// candidate that never hears back is cleaned up by Idunn's own abort; this
// only bounds how long it sits idle first.
const BOOTSTRAP_LEASE_TIMEOUT: Duration = Duration::from_secs(300);
// How long the UDP socket may fail every poll before that is a dead socket
// rather than a hiccup. Odin then ends and Idunn restarts it.
const POLL_FAILURE_LIMIT: Duration = Duration::from_secs(30);
const POLL_FAILURE_BACKOFF: Duration = Duration::from_millis(100);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const PROJECTION_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
// Odin's own changes (its heartbeat, correlations, the watermark) are written
// at most once per interval, and not at all when nothing changed. A crash loses
// at most this much of them; a correlation sequence that was never written was
// never published. A peer's put is written before it is acknowledged
// (`accept_raw_document`). Operator ruling Q3 B, docs/write-pattern-cut.md.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(2);
const MAX_RECENT_WARMING_PROOFS: usize = 64;
const WARMING_PROOF_LIFETIME_MILLIS: u64 = 60_000;

/// Relative path of Odin's durable store inside the state root. The recipe's
/// `topology` state slot in `deployment/idunn/recipe.toml` is the authority for
/// this name; Idunn owns where the root itself lives and hardens it, so the
/// daemon is told the root and never the file.
const TOPOLOGY_SLOT: &str = "topology.cc";

#[derive(Clone, Debug)]
struct Options {
    store: PathBuf,
    idunn_projection: PathBuf,
    idunn_anchor: PathBuf,
}

struct RuntimeAuthority {
    expected: IdunnExpectedIncarnationRecord,
    expected_sha256: String,
    activation: IdunnRuntimeActivationRecord,
    activation_sha256: String,
    activation_signer: IdunnRuntimeActivationSigner,
    provider_signer: ServiceIdentitySigner<GameCultProviderHealthIdentity>,
}

struct ProcessWriteLeaseGuard {
    _lock: File,
    record: IdunnProcessWriteLeaseRecord,
    sha256: String,
}

/// Odin's topology authority once its write lease is held: the store it loaded
/// and the credentials that sign for it. An `OdinTopologyAuthority` is built
/// over it for each operation, with the projection read for that operation.
struct Topology {
    store: MemoryOdinTopologyStore,
    signer: ServiceIdentitySigner<OdinTopologyIdentity>,
    idunn_anchor: ServiceIdentityTrustAnchor,
}

impl Topology {
    fn authority<P: IdunnProjectionSource>(
        &self,
        projections: P,
    ) -> OdinTopologyAuthority<
        P,
        &MemoryOdinTopologyStore,
        &ServiceIdentitySigner<OdinTopologyIdentity>,
        SystemClock,
    > {
        OdinTopologyAuthority::new(
            projections,
            &self.store,
            &self.signer,
            SystemClock,
            self.idunn_anchor.clone(),
            AuthenticationPolicy::default(),
        )
    }
}

struct RuntimeState {
    options: Options,
    candidate: SocketAddr,
    authority_material: RuntimeAuthority,
    idunn_anchor: Option<ServiceIdentityTrustAnchor>,
    topology_signer: Option<ServiceIdentitySigner<OdinTopologyIdentity>>,
    topology: Option<Topology>,
    write_lease: Option<ProcessWriteLeaseGuard>,
    write_lease_path: PathBuf,
    recent_warming_proofs: VecDeque<(String, u64)>,
    /// Counts from this launch. Every launch is a fresh activation, and
    /// presences are ordered only within one activation.
    publisher_sequence: u64,
    log_gate: RefCell<LogGate>,
}

#[derive(Clone)]
struct SinkHandle(Rc<RefCell<RuntimeState>>);

#[derive(Clone)]
struct SnapshotHandle(Rc<RefCell<RuntimeState>>);

impl CultMeshRudpRawDocumentSink for SinkHandle {
    fn accept_raw_document(&mut self, receipt: CultMeshRudpRawDocumentReceipt) -> Result<()> {
        self.0.borrow_mut().accept_raw_document(receipt)
    }
}

impl CultMeshRudpSnapshotSource for SnapshotHandle {
    fn raw_snapshot(
        &mut self,
        query: &CultMeshRudpSnapshotQuery,
    ) -> Result<Vec<CultNetRawDocumentRecord>> {
        self.0.borrow_mut().raw_snapshot(query)
    }
}

impl RuntimeState {
    fn open(options: Options, candidate: SocketAddr) -> Result<Self> {
        let authority_material = load_runtime_authority(Path::new(&required_environment(
            RUNTIME_BUNDLE_ENVIRONMENT,
        )?))?;
        require_expected_contract(&authority_material.expected, candidate)?;

        let idunn_anchor = read_trust_anchor::<IdunnServiceIdentity>(&options.idunn_anchor)?;
        let projection_source =
            CultCacheIdunnProjectionSource::new(&options.idunn_projection).snapshot()?;
        // This process is one exact incarnation, named by the Expected digest
        // in its immutable runtime bundle. The projection may carry other
        // incarnations of `odin` at the same time -- the one being replaced,
        // or the candidate replacing this one -- and none of them is this
        // process's business here. Only its own incarnation is looked up.
        //
        // The projected activation is deliberately not compared. Idunn
        // publishes an activation only after it has observed the process that
        // owns it, so at this moment the projection has none for this launch
        // yet. The bundle's own activation is verified against the Idunn anchor
        // below, and the write lease -- the thing that actually authorises
        // writing state -- is checked against the live projection before Odin
        // writes anything.
        let projection = projection_source
            .projection(&self_incarnation(&authority_material))?
            .context("Idunn projection has no Expected for this Odin incarnation")?;
        let provider_anchor = projection
            .provider_anchor
            .as_ref()
            .context("Idunn projection has no Odin runtime-presence trust anchor")?;
        ensure!(
            provider_anchor.signer_identity_id
                == authority_material.provider_signer.entry().identity_id
                && provider_anchor.signer_public_key
                    == authority_material.provider_signer.entry().public_key,
            "Odin provider signer differs from Idunn's projected trust anchor"
        );
        verify_runtime_authority(
            &authority_material.expected,
            &authority_material.activation,
            &idunn_anchor,
            &provider_anchor.signer_public_key,
        )?;

        let topology_identity_path =
            PathBuf::from(required_environment(TOPOLOGY_IDENTITY_ENVIRONMENT)?);
        let topology_signer = open_service_identity_credential_reader::<OdinTopologyIdentity>(
            File::open(&topology_identity_path).with_context(|| {
                format!(
                    "opening Odin topology identity credential {}",
                    topology_identity_path.display()
                )
            })?,
        )?;

        Ok(Self {
            options,
            candidate,
            authority_material,
            idunn_anchor: Some(idunn_anchor),
            topology_signer: Some(topology_signer),
            topology: None,
            write_lease: None,
            write_lease_path: PathBuf::from(required_environment(PROCESS_WRITE_LEASE_ENVIRONMENT)?),
            recent_warming_proofs: VecDeque::new(),
            publisher_sequence: 0,
            log_gate: RefCell::default(),
        })
    }

    fn activated(&self) -> bool {
        self.topology.is_some() && self.write_lease.is_some()
    }

    fn try_activate(&mut self) -> Result<bool> {
        if self.activated() {
            return Ok(true);
        }
        let Some(lease) = acquire_process_write_lease(
            &self.write_lease_path,
            &self.authority_material,
            &self.recent_warming_proofs,
        )?
        else {
            return Ok(false);
        };
        let Some(projected) = CultCacheIdunnProjectionSource::new(&self.options.idunn_projection)
            .snapshot()?
            .projection(&self_incarnation(&self.authority_material))?
        else {
            return Ok(false);
        };
        if projected.current_lease.as_ref() != Some(&lease.record) {
            return Ok(false);
        }
        // The one read of the store file while this process lives: from here
        // on the working set is in memory, and the file is only written.
        let store = MemoryOdinTopologyStore::load(&self.options.store)?;
        // Correlations the previous, target-keyed Odin left in this store are
        // not this contract's and would otherwise be served to the Verse as
        // current.
        store.retire_legacy_correlations();
        let signer = self
            .topology_signer
            .take()
            .context("Odin topology signer was already consumed")?;
        let idunn_anchor = self
            .idunn_anchor
            .take()
            .context("Idunn trust anchor was already consumed")?;
        self.write_lease = Some(lease);
        self.topology = Some(Topology {
            store,
            signer,
            idunn_anchor,
        });
        Ok(true)
    }

    fn topology(&self) -> Result<&Topology> {
        self.topology
            .as_ref()
            .context("Odin topology authority is absent")
    }

    /// Write Odin's store file from the working set, if it changed; returns
    /// whether it wrote. This is the only write of the file, made on the
    /// interval, for a peer's put, and on the way out. The lease is
    /// checked immediately before it, so nothing is written once the lease is
    /// lost, and a file another process wrote is refused (`ForeignStoreWrite`)
    /// rather than written over. Both end Odin (see `survive`).
    fn flush(&self) -> Result<bool> {
        let store = &self.topology()?.store;
        if !store.is_dirty() {
            return Ok(false);
        }
        self.require_current_write_lease()?;
        store.flush()
    }

    /// A peer's put is written before it is acknowledged: the server sends the
    /// acknowledgement only when this returns `Ok`, and it returns `Ok` only
    /// once the store holding the put is on disk. The write is the one `flush`,
    /// so it carries everything else that changed too. A write that fails
    /// refuses the put.
    ///
    /// The promise runs one way only. An acknowledged put is always on disk. One
    /// refused because its write failed may still be: it stays in the working set, so the catalog
    /// serves it and the next write lands it (or the failed write had already
    /// replaced the file). That is sound because a put is the latest value for
    /// its type and key, not an event: a publisher that retries a refused put
    /// stores the same value, and one that does not has lost nothing it was
    /// promised.
    fn accept_raw_document(&mut self, receipt: CultMeshRudpRawDocumentReceipt) -> Result<()> {
        ensure!(
            self.activated(),
            "Odin does not admit or persist provider documents before its process-write lease"
        );
        validate_raw_document_shape(&receipt.document)?;
        if receipt.document.schema_id == GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA {
            self.admit_presence_document(&receipt.document, receipt.received_at_unix_millis)?;
        } else {
            persist_generic_document(&self.topology()?.store, &receipt.document)?;
        }
        self.flush()?;
        Ok(())
    }

    /// The one admission path for a runtime-presence document, whether a
    /// provider delivered it over RUDP or Odin signed it for itself.
    fn admit_presence_document(
        &self,
        document: &CultNetRawDocumentRecord,
        received_at_unix_millis: u64,
    ) -> Result<()> {
        let presence = decode_presence(&document.payload)?;
        ensure!(
            document.record_key == presence.target,
            "runtime-presence document key differs from its signed target"
        );
        let projections =
            CultCacheIdunnProjectionSource::new(&self.options.idunn_projection).snapshot()?;
        self.topology()?.authority(projections).admit_presence(
            &presence.target,
            &document.payload,
            received_at_unix_millis,
        )?;
        Ok(())
    }

    fn raw_snapshot(
        &mut self,
        query: &CultMeshRudpSnapshotQuery,
    ) -> Result<Vec<CultNetRawDocumentRecord>> {
        validate_snapshot_filters(query)?;
        if exact_self_presence_query(query) {
            let (state, detail) = if self.activated() {
                self.require_current_write_lease()?;
                ("active", format!("route-observation:{}", query.message_id))
            } else {
                ("warming", format!("idunn-warming:{}", query.message_id))
            };
            return Ok(vec![self.signed_presence_document(state, &detail)?]);
        }
        ensure!(
            self.activated(),
            "Odin catalog is unavailable until Idunn grants its process-write lease"
        );
        self.require_current_write_lease()?;
        self.stored_snapshot(query)
    }

    fn signed_presence_document(
        &mut self,
        state: &str,
        detail: &str,
    ) -> Result<CultNetRawDocumentRecord> {
        let now = unix_millis()?;
        self.publisher_sequence = self
            .publisher_sequence
            .checked_add(1)
            .context("Odin runtime-presence publisher sequence exhausted")?;
        let expected = &self.authority_material.expected;
        let activation = &self.authority_material.activation;
        let source_runtime_id = expected.runtime_id.clone();
        let mut record = GameCultRuntimePresenceHealthRecord {
            schema_version: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
            target: expected.target.clone(),
            expected_projection_sha256: self.authority_material.expected_sha256.clone(),
            plan_id: expected.plan_id.clone(),
            incarnation_id: expected.incarnation_id.clone(),
            sealed_release_id: expected.sealed_release_id.clone(),
            activation_witness_sha256: self.authority_material.activation_sha256.clone(),
            state_schema_generation: expected.state_schema_generation.clone(),
            state_contract_sha256: expected.state_contract_sha256.clone(),
            runtime_id: expected.runtime_id.clone(),
            runtime_instance_id: activation.runtime_instance_id.clone(),
            bound_endpoint: Some(format!("rudp://{}", self.candidate)),
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
            state: state.into(),
            detail: detail.into(),
            write_lease_sha256: self.write_lease.as_ref().map(|lease| lease.sha256.clone()),
            signer_identity_id: self
                .authority_material
                .provider_signer
                .entry()
                .identity_id
                .clone(),
            publisher_sequence: self.publisher_sequence,
            observed_at_unix_millis: now,
            signature_algorithm: "ed25519".into(),
            signature: Vec::new(),
            activation_signer_identity_id: activation.activation_signer_identity_id.clone(),
            activation_signature: Vec::new(),
        };
        ensure!(
            (state == "warming" && record.write_lease_sha256.is_none())
                || (state == "active" && record.write_lease_sha256.is_some()),
            "Odin presence state does not match its process-write authority"
        );
        let proof_payload = record.canonical_proof_payload()?;
        record.signature = self
            .authority_material
            .provider_signer
            .sign::<GameCultRuntimePresenceHealthPurpose>(&proof_payload)
            .signature;
        record.activation_signature = self
            .authority_material
            .activation_signer
            .sign_presence_proof(&record)?;
        record.validate()?;
        let payload = rmp_serde::to_vec(&record)?;
        ensure!(
            rmp_serde::from_slice::<GameCultRuntimePresenceHealthRecord>(&payload)? == record,
            "Odin runtime presence is not canonical MessagePack"
        );
        if state == "warming" {
            remember_recent_warming_proof(
                &mut self.recent_warming_proofs,
                record.canonical_sha256()?,
                now,
            );
        }
        Ok(CultNetRawDocumentRecord {
            schema_id: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
            record_key: TARGET.into(),
            stored_at: rfc3339_millis(now)?,
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload,
            source_runtime_id: Some(source_runtime_id),
            source_agent_id: Some(record.signer_identity_id),
            source_role: Some("runtime-presence-health-publisher".into()),
            tags: Some(vec!["cultnet.transport.rudp.v0".into()]),
        })
    }

    /// Odin is the store, so its own heartbeat is admitted into the store
    /// directly. It never crosses RUDP to itself, and so never takes one of the
    /// server's bounded session slots from the providers it serves: a table
    /// full of lingering publisher sessions cannot make Odin miss its own
    /// liveness.
    fn publish_self_presence(&mut self, detail: &str) -> Result<()> {
        self.require_current_write_lease()?;
        self.require_own_projection()?;
        let document = self.signed_presence_document("active", detail)?;
        self.admit_presence_document(&document, unix_millis()?)
    }

    /// Idunn's projection of this process's own incarnation. A failure to
    /// establish it that is not a failed file read -- undecodable, ambiguous,
    /// substituted, or with no Expected for this incarnation -- is a failure of
    /// Odin's own authority, so it is marked for the temporary fatal rule in
    /// `survive`. A failed file read is not (see `own_projection_failure`), nor
    /// are failures about other incarnations: they are read, logged and skipped
    /// where they are met.
    fn require_own_projection(&self) -> Result<()> {
        let path = &self.options.idunn_projection;
        match CultCacheIdunnProjectionSource::new(path)
            .snapshot()
            .and_then(|projections| projections.projection(&self_incarnation(&self.authority_material)))
        {
            Err(error) => Err(own_projection_failure(error)),
            // No file is an I/O condition (Idunn replaces it atomically, so it
            // is briefly or wrongly unreadable), not a projection that
            // disowns Odin.
            Ok(None) if !path.is_file() => Err(anyhow::Error::new(std::io::Error::from(
                std::io::ErrorKind::NotFound,
            ))
            .context(format!("Idunn's projection {} is absent", path.display()))),
            Ok(None) => Err(PresenceAuthorityRefused(
                "Idunn projects no Expected for Odin's own incarnation",
            )
            .into()),
            Ok(Some(_)) => Ok(()),
        }
    }

    fn require_current_write_lease(&self) -> Result<()> {
        let held = self
            .write_lease
            .as_ref()
            .context("Odin has no process-write lease")?;
        // A lease Odin cannot read is a lease Odin cannot show it still holds.
        let current = read_process_write_lease(&self.write_lease_path)
            .map_err(|error| {
                WriteLeaseLost(format!("Odin process-write lease is unreadable: {error:#}"))
            })?
            .ok_or_else(|| WriteLeaseLost("Odin process-write lease was withdrawn".into()))?;
        if current != held.record || current.canonical_sha256()? != held.sha256 {
            return Err(
                WriteLeaseLost("Odin process-write lease changed after admission".into()).into(),
            );
        }
        Ok(())
    }

    /// Refresh every incarnation Idunn projects, and every incarnation Odin
    /// holds a correlation for, projected or not (the latter are refreshed so
    /// their correlations are withdrawn). The projection is read once for the
    /// whole pass.
    fn refresh_all_correlations(&self) -> Result<()> {
        self.require_current_write_lease()?;
        let topology = self.topology()?;
        let projections =
            CultCacheIdunnProjectionSource::new(&self.options.idunn_projection).snapshot()?;
        let mut incarnations = projections.incarnations();
        incarnations.extend(topology.store.correlated_incarnations());
        let authority = topology.authority(projections);
        // Each incarnation is refreshed on its own: one whose records cannot be
        // read is that incarnation's failure, and never stops the others.
        for incarnation in incarnations {
            if let Err(error) = authority.refresh(&incarnation) {
                self.log_repeating(
                    &format!("refresh of incarnation {}", incarnation.key()),
                    format!("failed; the rest are unaffected: {error:#}"),
                );
            }
        }
        Ok(())
    }

    /// The catalog serves what is stored, one record at a time: a record that
    /// does not decode, or whose incarnation's projection does not, is skipped
    /// and logged, and is never a reason to refuse every other record. Idunn's
    /// projection is read once per query; a projection that cannot be read at
    /// all is every presence's failure and no one else's, so the query is
    /// answered as if Idunn projected nothing: peer documents are served and
    /// presences skipped. Odin's
    /// correlations are not part of the catalog: Idunn reads them from the
    /// store file, and a target has one per incarnation, so keying them by
    /// target would collide for the whole deploy window.
    fn stored_snapshot(
        &self,
        query: &CultMeshRudpSnapshotQuery,
    ) -> Result<Vec<CultNetRawDocumentRecord>> {
        let records = self.topology()?.store.records();
        let projections = CultCacheIdunnProjectionSource::new(&self.options.idunn_projection)
            .snapshot()
            .unwrap_or_else(|error| {
                self.log_repeating(
                    "catalog skips every presence",
                    format!("Idunn's projection cannot be read: {error:#}"),
                );
                IdunnProjectionSnapshot::default()
            });
        let mut selected = BTreeMap::new();
        for envelope in records.values() {
            let document = match self.public_document(&projections, envelope) {
                Ok(Some(document)) => document,
                Ok(None) => continue,
                Err(error) => {
                    self.log_repeating(
                        &format!("catalog skips {} {}", envelope.r#type, envelope.key),
                        format!("{error:#}"),
                    );
                    continue;
                }
            };
            if !query_allows(query, &document) {
                continue;
            }
            let identity = (document.schema_id.clone(), document.record_key.clone());
            ensure!(
                selected.insert(identity, document).is_none(),
                "Odin catalog contains duplicate public document identities"
            );
        }
        Ok(selected.into_values().collect())
    }

    /// One stored record as the Verse sees it; `None` when it is not public.
    fn public_document(
        &self,
        projections: &IdunnProjectionSnapshot,
        envelope: &CultCacheEnvelope,
    ) -> Result<Option<CultNetRawDocumentRecord>> {
        let Some(schema_id) = envelope.schema_id.clone() else {
            return Ok(None);
        };
        if envelope.r#type != GameCultRuntimePresenceHealthRecord::TYPE {
            return Ok(is_peer_document_type(&envelope.r#type).then(|| {
                CultNetRawDocumentRecord {
                    schema_id,
                    record_key: envelope.key.clone(),
                    stored_at: envelope.stored_at.clone(),
                    payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                    payload: envelope.payload.clone(),
                    source_runtime_id: None,
                    source_agent_id: None,
                    source_role: None,
                    tags: None,
                }
            }));
        }
        let presence = decode_presence(&envelope.payload)?;
        let Some(projection) = projections.projection(&IncarnationRef::new(
            presence.target.clone(),
            presence.expected_projection_sha256.clone(),
        ))?
        else {
            return Ok(None);
        };
        if projection.expected.expected_signer_identity_id != presence.signer_identity_id
            || projection
                .activation
                .as_ref()
                .map(IdunnRuntimeActivationRecord::canonical_sha256)
                .transpose()?
                .as_deref()
                != Some(presence.activation_witness_sha256.as_str())
            || projection
                .activation
                .as_ref()
                .map(|activation| activation.runtime_instance_id.as_str())
                != Some(presence.runtime_instance_id.as_str())
        {
            return Ok(None);
        }
        Ok(Some(CultNetRawDocumentRecord {
            schema_id,
            record_key: presence.target.clone(),
            stored_at: envelope.stored_at.clone(),
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload: envelope.payload.clone(),
            source_runtime_id: Some(presence.runtime_id),
            source_agent_id: Some(presence.signer_identity_id),
            source_role: Some("runtime-presence-health-publisher".into()),
            tags: Some(vec!["odin-observed".into()]),
        }))
    }

    /// Log a line that a persistent fault would otherwise repeat on every pass:
    /// once per subject per minute, or as soon as its message changes.
    fn log_repeating(&self, subject: &str, message: String) {
        if let Some(repeats) = self
            .log_gate
            .borrow_mut()
            .admit(subject, &message, Instant::now())
        {
            if repeats == 0 {
                eprintln!("Odin {subject}: {message}");
            } else {
                eprintln!("Odin {subject}: {message} ({repeats} repeats suppressed)");
            }
        }
    }
}

/// A failure to read Odin's own projection. Only a failure of the file read
/// itself (marked `ProjectionUnreadable` where the file is read: permission
/// denied, not found, interrupted) says nothing about what Idunn published and
/// is retried. Anything the reader decoded or validated and did not accept is a
/// failure of Odin's own authority, marked for the fatal rule in `survive`,
/// whatever error it wraps.
fn own_projection_failure(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<ProjectionUnreadable>().is_some() {
        return error;
    }
    error.context(PresenceAuthorityRefused(
        "Odin's own projected authority cannot be read",
    ))
}

/// Once-per-interval admission for repeating log lines, keyed by subject.
#[derive(Default)]
struct LogGate {
    seen: BTreeMap<String, LoggedLine>,
    suppressed_total: u64,
}

struct LoggedLine {
    at: Instant,
    message: String,
    suppressed: u64,
}

const REPEATED_LOG_INTERVAL: Duration = Duration::from_secs(60);
const MAX_LOG_SUBJECTS: usize = 1024;

impl LogGate {
    /// `Some(repeats suppressed since the last line)` when the line is to be
    /// written, `None` when it repeats one written less than the interval ago.
    fn admit(&mut self, subject: &str, message: &str, now: Instant) -> Option<u64> {
        if let Some(line) = self.seen.get_mut(subject)
            && line.message == message
            && now.saturating_duration_since(line.at) < REPEATED_LOG_INTERVAL
        {
            line.suppressed += 1;
            self.suppressed_total += 1;
            return None;
        }
        let repeats = self.seen.remove(subject).map_or(0, |line| line.suppressed);
        if self.seen.len() >= MAX_LOG_SUBJECTS {
            self.seen
                .retain(|_, line| now.saturating_duration_since(line.at) < REPEATED_LOG_INTERVAL);
            if self.seen.len() >= MAX_LOG_SUBJECTS {
                self.seen.clear();
            }
        }
        self.seen.insert(
            subject.to_owned(),
            LoggedLine {
                at: now,
                message: message.to_owned(),
                suppressed: 0,
            },
        );
        Some(repeats)
    }
}

fn main() -> Result<()> {
    let options = parse_options(std::env::args().skip(1))?;
    let candidate: SocketAddr = required_environment(CANDIDATE_BIND_ENVIRONMENT)?
        .parse()
        .context("parsing Idunn candidate bind")?;
    ensure!(
        candidate.ip().is_loopback() && candidate.port() != 0,
        "Odin candidate bind must be one fixed loopback socket"
    );
    let socket = UdpSocket::bind(candidate)
        .with_context(|| format!("binding Odin CultNet RUDP candidate {candidate}"))?;
    let state = Rc::new(RefCell::new(RuntimeState::open(options, candidate)?));
    let mut server = CultMeshRudpDocumentServer::new(
        socket,
        SinkHandle(state.clone()),
        SnapshotHandle(state.clone()),
        CultMeshSystemClock::default(),
        CultMeshRudpDocumentServerOptions::default(),
    )?;

    // Idunn runs this process as PID 1 of its own PID namespace, and a
    // namespace init ignores every signal it has not caught. Without this,
    // a stop is a ninety-second wait for SIGKILL, which is what fencing the
    // incumbent during a deployment costs its candidate.
    let stopping = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&stopping))
            .context("registering Odin's stop signal handler")?;
    }

    let bootstrap_started = Instant::now();
    let mut poll_failing_since = None;
    while !state.borrow_mut().try_activate()? {
        if stopping.load(Ordering::Relaxed) {
            eprintln!("Odin stopping before activation on request");
            return Ok(());
        }
        let progressed = poll_server(&mut server, &mut poll_failing_since)?;
        ensure!(
            bootstrap_started.elapsed() < BOOTSTRAP_LEASE_TIMEOUT,
            "timed out waiting for Idunn to admit Odin's exact Warming proof and grant its lease"
        );
        if !progressed {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }

    serve(&state, &mut server, &mut ServingTimers::default(), &stopping)
}

/// Serve until stopped on request or ended by a condition `survive` returns
/// (or a dead socket). Every way out makes the last write (`stop`).
fn serve(
    state: &Rc<RefCell<RuntimeState>>,
    server: &mut OdinServer,
    timers: &mut ServingTimers,
    stopping: &AtomicBool,
) -> Result<()> {
    let ended = loop {
        if stopping.load(Ordering::Relaxed) {
            eprintln!("Odin stopping on request");
            break Ok(());
        }
        match serving_pass(state, server, timers) {
            Ok(true) => {}
            Ok(false) => thread::sleep(IDLE_POLL_INTERVAL),
            Err(error) => break Err(error),
        }
    };
    stop(&state.borrow());
    ended
}

/// The last write before Odin ends, under `flush`'s own checks: after
/// `WriteLeaseLost` or `ForeignStoreWrite` it writes nothing. A failure is
/// logged, not returned: Odin is ending either way.
fn stop(state: &RuntimeState) {
    if let Err(error) = state.flush() {
        eprintln!("Odin's final store write was not made: {error:#}");
    }
}

/// A timer that has never fired is due, so the first pass refreshes and
/// publishes.
#[derive(Default)]
struct ServingTimers {
    last_heartbeat: Option<Instant>,
    last_projection_refresh: Option<Instant>,
    /// When the store file was last written, or a write last failed.
    last_flush: Option<Instant>,
    poll_failing_since: Option<Instant>,
}

type OdinServer = CultMeshRudpDocumentServer<SinkHandle, SnapshotHandle, CultMeshSystemClock>;

/// One turn of the serving loop: serve a datagram, refresh the topology,
/// publish Odin's own presence, and write the store file, when each is due.
/// Returns whether the poll made progress. It fails only for the conditions
/// `survive` returns, or a dead socket.
fn serving_pass(
    state: &Rc<RefCell<RuntimeState>>,
    server: &mut OdinServer,
    timers: &mut ServingTimers,
) -> Result<bool> {
    let progressed = poll_server(server, &mut timers.poll_failing_since)?;
    if is_due(timers.last_projection_refresh, PROJECTION_REFRESH_INTERVAL) {
        let refreshed = state.borrow_mut().refresh_all_correlations();
        survive(&state.borrow(), "projection refresh", refreshed)?;
        timers.last_projection_refresh = Some(Instant::now());
    }
    if is_due(timers.last_heartbeat, HEARTBEAT_INTERVAL) {
        let published = state.borrow_mut().publish_self_presence("ready");
        survive(&state.borrow(), "self-presence publication", published)?;
        timers.last_heartbeat = Some(Instant::now());
    }
    if is_due(timers.last_flush, FLUSH_INTERVAL) {
        let flushed = state.borrow().flush();
        // Only an attempt starts the interval: with nothing to write, the next
        // change is written on the first pass after it.
        if !matches!(flushed, Ok(false)) {
            timers.last_flush = Some(Instant::now());
        }
        survive(&state.borrow(), "store write", flushed)?;
    }
    Ok(progressed)
}

fn is_due(last: Option<Instant>, interval: Duration) -> bool {
    last.is_none_or(|last| last.elapsed() >= interval)
}

/// Odin held its process-write lease and no longer does: another incarnation
/// has been granted the state, or Idunn withdrew the grant. Serving on would be
/// a second writer, so this is the one serving-loop condition that ends Odin.
#[derive(Debug)]
struct WriteLeaseLost(String);

impl std::fmt::Display for WriteLeaseLost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for WriteLeaseLost {}

/// The serving loop's failure policy. Odin's liveness is Odin's own: a failed
/// refresh, publication or read is logged and retried on the next pass, never
/// allowed to end the daemon. Only `WriteLeaseLost` and `ForeignStoreWrite`,
/// wherever in the error chain they sit, are returned: each means another
/// writer holds Odin's state, and writing on would be a second writer. As a
/// named temporary rule, so is
/// `PresenceAuthorityRefused`: a failure to establish Odin's OWN authority
/// (`require_own_projection`), or a self-presence that authority will never
/// admit (no verifiable authority, a signer that does not match the anchor, a
/// stored presence of this activation that no longer authenticates)
/// leaves a frozen presence that goes stale, and a stale Odin presence makes
/// `dependency_evidence` flip every dependent to not-Ready. Ending Odin lets
/// Idunn replace it. A host clock that has not yet reached the anchor's binding
/// is not such a failure (`reconcile`): it is retried. Deleted when Idunn
/// proves Odin by its own route challenge
/// (Idunn audit cut A3, operator question Q-O5).
fn survive<T>(state: &RuntimeState, what: &str, result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error.downcast_ref::<WriteLeaseLost>().is_some()
                || error.downcast_ref::<ForeignStoreWrite>().is_some()
                || error.downcast_ref::<PresenceAuthorityRefused>().is_some() =>
        {
            Err(error)
        }
        Err(error) => {
            state.log_repeating(what, format!("failed; retrying on the next pass: {error:#}"));
            Ok(None)
        }
    }
}

fn poll_server(server: &mut OdinServer, failing_since: &mut Option<Instant>) -> Result<bool> {
    settle_poll(server.poll_once(), failing_since, Instant::now())
}

/// A failed poll is logged and retried. The socket failing every poll for
/// `POLL_FAILURE_LIMIT` is a dead socket, not a transient one; only then does
/// Odin end, so Idunn restarts it. Any successful poll clears the failure run.
fn settle_poll(
    result: Result<CultMeshRudpPollOutcome>,
    failing_since: &mut Option<Instant>,
    now: Instant,
) -> Result<bool> {
    match result {
        Ok(outcome) => {
            *failing_since = None;
            Ok(progress_of(outcome))
        }
        Err(error) => {
            let since = *failing_since.get_or_insert(now);
            ensure!(
                now.duration_since(since) < POLL_FAILURE_LIMIT,
                "Odin's RUDP socket has failed every poll for {POLL_FAILURE_LIMIT:?}: {error:#}"
            );
            eprintln!("Odin RUDP poll failed; retrying: {error:#}");
            thread::sleep(POLL_FAILURE_BACKOFF);
            Ok(false)
        }
    }
}

/// An application rejection ends only the offending peer's session, which the
/// server has already done. It is logged and never ends Odin.
fn progress_of(outcome: CultMeshRudpPollOutcome) -> bool {
    match outcome {
        CultMeshRudpPollOutcome::Idle => false,
        CultMeshRudpPollOutcome::Handled => true,
        CultMeshRudpPollOutcome::ApplicationRejected(rejection) => {
            eprintln!(
                "Odin rejected CultMesh RUDP application message {:?} {} from {:?}: {}",
                rejection.operation, rejection.message_id, rejection.session, rejection.reason
            );
            true
        }
    }
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<Options> {
    let mut args = args.peekable();
    let mut values = BTreeMap::new();
    while let Some(name) = args.next() {
        let name = name
            .strip_prefix("--")
            .with_context(|| format!("expected --option, got {name:?}"))?;
        ensure!(
            matches!(name, "state-root" | "idunn-projection" | "idunn-anchor"),
            "unsupported Odin option --{name}"
        );
        let value = args
            .next()
            .with_context(|| format!("missing value for --{name}"))?;
        ensure!(
            values
                .insert(name.to_owned(), PathBuf::from(value))
                .is_none(),
            "duplicate Odin option --{name}"
        );
    }
    let take = |name: &str| -> Result<PathBuf> {
        let path = values
            .get(name)
            .cloned()
            .with_context(|| format!("--{name} is required"))?;
        ensure!(path.is_absolute(), "--{name} must be an absolute path");
        Ok(path)
    };
    let options = Options {
        store: take("state-root")?.join(TOPOLOGY_SLOT),
        idunn_projection: take("idunn-projection")?,
        idunn_anchor: take("idunn-anchor")?,
    };
    ensure!(
        options.store != options.idunn_projection
            && options.store != options.idunn_anchor
            && options.idunn_projection != options.idunn_anchor,
        "Odin state root, Idunn projection, and Idunn anchor paths must be distinct"
    );
    Ok(options)
}

fn load_runtime_authority(bundle: &Path) -> Result<RuntimeAuthority> {
    ensure!(bundle.is_absolute(), "Idunn runtime bundle is not absolute");
    let (expected_key, expected_payload) = read_single_runtime_record(
        &bundle.join("expected.cc"),
        IdunnExpectedIncarnationRecord::TYPE,
        IDUNN_EXPECTED_INCARNATION_SCHEMA,
    )?;
    let expected = IdunnExpectedIncarnationRecord::decode_canonical(&expected_payload)?;
    ensure!(
        expected_key == expected.target,
        "Expected key is substituted"
    );
    let (activation_key, activation_payload) = read_single_runtime_record(
        &bundle.join("activation.cc"),
        IdunnRuntimeActivationRecord::TYPE,
        IDUNN_RUNTIME_ACTIVATION_SCHEMA,
    )?;
    let activation = IdunnRuntimeActivationRecord::decode_canonical(&activation_payload)?;
    ensure!(
        activation_key == expected.target,
        "activation key is substituted"
    );
    let expected_sha256 = expected.canonical_sha256()?;
    ensure!(
        activation.expected_projection_sha256 == expected_sha256
            && activation.runtime_id == expected.runtime_id,
        "activation does not bind Odin's Expected projection"
    );
    let (activation_credential, provider_identity) = take_runtime_signer_descriptors()?;
    let activation_signer =
        IdunnRuntimeActivationSigner::from_credential_reader(activation_credential)?;
    ensure!(
        activation_signer.identity_id() == activation.activation_signer_identity_id
            && activation_signer.public_key() == activation.activation_signer_public_key,
        "activation credential does not belong to Odin's activation"
    );
    let provider_signer = open_service_identity_credential_reader::<GameCultProviderHealthIdentity>(
        provider_identity,
    )?;
    ensure!(
        provider_signer.entry().identity_id == expected.expected_signer_identity_id,
        "provider credential is not the signer selected by Expected"
    );
    Ok(RuntimeAuthority {
        expected,
        expected_sha256,
        activation_sha256: activation.canonical_sha256()?,
        activation,
        activation_signer,
        provider_signer,
    })
}

fn require_expected_contract(
    expected: &IdunnExpectedIncarnationRecord,
    bind: SocketAddr,
) -> Result<()> {
    ensure!(
        expected.target == TARGET
            && expected.health_contract == HEALTH_CONTRACT
            && expected.state_schema_generation.as_deref() == Some(STATE_SCHEMA_GENERATION)
            && expected.state_contract_sha256.as_deref() == Some(STATE_CONTRACT_SHA256)
            && expected.write_lease_required,
        "Odin Expected projection differs from its compiled runtime/state contract"
    );
    let route = expected
        .route
        .as_ref()
        .context("Odin Expected projection has no admitted route")?;
    ensure!(
        route.transport == "rudp" && route.candidate_endpoint == format!("rudp://{bind}"),
        "Odin candidate bind differs from Expected"
    );
    let capability = expected
        .capabilities
        .iter()
        .find(|capability| {
            capability.capability == RENDEZVOUS_CAPABILITY
                && capability.schema == RENDEZVOUS_SCHEMA
                && capability.compatibility == RENDEZVOUS_COMPATIBILITY
        })
        .context("Odin Expected projection omits its rendezvous capability")?;
    ensure!(
        capability.minimum_capacity > 0 && expected.capabilities.len() == 1,
        "Odin Expected capability set differs from its compiled contract"
    );
    ensure!(
        expected.dependencies.is_empty(),
        "Odin bootstrap cannot depend on a managed daemon"
    );
    Ok(())
}

fn read_single_runtime_record(
    path: &Path,
    expected_type: &str,
    expected_schema: &str,
) -> Result<(String, Vec<u8>)> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let [envelope] = entries.as_slice() else {
        bail!("runtime authority store must contain exactly one record");
    };
    ensure!(
        envelope.r#type == expected_type && envelope.schema_id.as_deref() == Some(expected_schema),
        "runtime authority store has the wrong typed envelope"
    );
    Ok((envelope.key.clone(), envelope.payload.clone()))
}

fn take_runtime_signer_descriptors() -> Result<(File, File)> {
    let pid = required_environment(SYSTEMD_LISTEN_PID_ENVIRONMENT)?;
    let count = required_environment(SYSTEMD_LISTEN_FDS_ENVIRONMENT)?;
    let names = required_environment(SYSTEMD_LISTEN_FDNAMES_ENVIRONMENT)?;
    // LISTEN_PID cannot be compared to our own pid here. Idunn launches Odin
    // with PrivatePIDs=yes -- the private PID namespace is part of the
    // isolation it proves between a candidate and the incumbent -- and systemd
    // sets LISTEN_PID to the pid it knows, which is in the *outer* namespace.
    // Inside, /proc/self/status reports only the namespace-local pid, so the
    // value is not merely different, it names something this process cannot
    // observe. Requiring equality made every candidate exit immediately.
    //
    // The protocol check exists so a child does not mistake inherited
    // LISTEN_FDS for its own. What replaces it here is stronger than a pid
    // comparison: the exact descriptor count and name order below, and then the
    // content itself, which must verify against the trust anchor Idunn
    // published. Descriptors that were not Idunn's fail that verification.
    ensure!(!pid.is_empty(), "systemd passed no listener pid");
    ensure!(count == "2", "Odin requires exactly two signer descriptors");
    ensure!(
        names == format!("{ACTIVATION_SIGNER_FD_NAME}:{PROVIDER_SIGNER_FD_NAME}"),
        "systemd signer descriptor names or order differ from Idunn's contract"
    );
    // SAFETY: the exact systemd LISTEN_* contract above assigns sole ownership
    // of descriptors 3 and 4 to this process, and this is the first FD consumer.
    let activation = unsafe { File::from_raw_fd(SYSTEMD_LISTEN_FDS_START) };
    // SAFETY: descriptor 4 is distinct and covered by the same exact contract.
    let provider = unsafe { File::from_raw_fd(SYSTEMD_LISTEN_FDS_START + 1) };
    Ok((activation, provider))
}

fn read_trust_anchor<P: ServiceIdentityProfile>(path: &Path) -> Result<ServiceIdentityTrustAnchor> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let [envelope] = entries.as_slice() else {
        bail!("service trust-anchor store must contain exactly one document");
    };
    ensure!(
        envelope.r#type == P::TRUST_ANCHOR_TYPE
            && envelope.key == P::TRUST_ANCHOR_KEY
            && envelope.schema_id.as_deref() == Some(P::TRUST_ANCHOR_SCHEMA),
        "service trust anchor belongs to another identity profile"
    );
    let anchor: ServiceIdentityTrustAnchor = rmp_serde::from_slice(&envelope.payload)?;
    ensure!(
        rmp_serde::to_vec(&anchor)? == envelope.payload
            && anchor.schema_version == P::TRUST_ANCHOR_SCHEMA
            && derive_service_identity_id::<P>(&anchor.public_key)? == anchor.identity_id,
        "service trust anchor is noncanonical or self-inconsistent"
    );
    Ok(anchor)
}

/// The one incarnation this process is: its bundle's Expected digest.
fn self_incarnation(authority: &RuntimeAuthority) -> IncarnationRef {
    IncarnationRef::new(TARGET, authority.expected_sha256.clone())
}

fn acquire_process_write_lease(
    path: &Path,
    authority: &RuntimeAuthority,
    recent_warming: &VecDeque<(String, u64)>,
) -> Result<Option<ProcessWriteLeaseGuard>> {
    let Some(observed) = read_process_write_lease(path)? else {
        return Ok(None);
    };
    let now = unix_millis()?;
    let warming_is_ours =
        recent_warming_proof(recent_warming, &observed.warming_presence_sha256, now);
    let exact_incarnation = observed.target == authority.expected.target
        && observed.expected_projection_sha256 == authority.expected_sha256
        && observed.plan_id == authority.expected.plan_id
        && observed.incarnation_id == authority.expected.incarnation_id
        && observed.sealed_release_id == authority.expected.sealed_release_id
        && observed.activation_witness_sha256 == authority.activation_sha256
        && Some(observed.state_schema_generation.as_str())
            == authority.expected.state_schema_generation.as_deref()
        && Some(observed.state_contract_sha256.as_str())
            == authority.expected.state_contract_sha256.as_deref()
        && observed.runtime_id == authority.expected.runtime_id
        && observed.runtime_instance_id == authority.activation.runtime_instance_id;
    if !warming_is_ours || !exact_incarnation {
        return Ok(None);
    }
    let lock_path = sibling_lock_path(path)?;
    let lock = OpenOptions::new()
        .read(true)
        .open(&lock_path)
        .with_context(|| format!("opening process-write-lease lock {}", lock_path.display()))?;
    FileExt::lock_shared(&lock)?;
    let current = read_process_write_lease(path)?
        .context("process-write lease disappeared while acquiring its lifetime lock")?;
    ensure!(
        current == observed,
        "process-write lease changed while acquiring its lifetime lock"
    );
    Ok(Some(ProcessWriteLeaseGuard {
        _lock: lock,
        sha256: current.canonical_sha256()?,
        record: current,
    }))
}

fn remember_recent_warming_proof(
    recent_warming: &mut VecDeque<(String, u64)>,
    sha256: String,
    now: u64,
) {
    while recent_warming
        .front()
        .is_some_and(|(_, issued)| now.saturating_sub(*issued) > WARMING_PROOF_LIFETIME_MILLIS)
    {
        recent_warming.pop_front();
    }
    recent_warming.push_back((sha256, now));
    while recent_warming.len() > MAX_RECENT_WARMING_PROOFS {
        recent_warming.pop_front();
    }
}

fn recent_warming_proof(
    recent_warming: &VecDeque<(String, u64)>,
    expected_sha256: &str,
    now: u64,
) -> bool {
    recent_warming.iter().any(|(sha256, issued)| {
        sha256 == expected_sha256 && now.saturating_sub(*issued) <= WARMING_PROOF_LIFETIME_MILLIS
    })
}

fn read_process_write_lease(path: &Path) -> Result<Option<IdunnProcessWriteLeaseRecord>> {
    if !path.is_file() {
        return Ok(None);
    }
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let [envelope] = entries.as_slice() else {
        ensure!(entries.is_empty(), "process-write-lease store is ambiguous");
        return Ok(None);
    };
    ensure!(
        envelope.r#type == IdunnProcessWriteLeaseRecord::TYPE
            && envelope.schema_id.as_deref() == Some(IDUNN_PROCESS_WRITE_LEASE_SCHEMA),
        "process-write-lease store has the wrong typed envelope"
    );
    let lease = IdunnProcessWriteLeaseRecord::decode_canonical(&envelope.payload)?;
    ensure!(
        envelope.key == lease.target,
        "process-write-lease key differs from its target"
    );
    Ok(Some(lease))
}

/// The document namespaces providers publish to the rendezvous. A provider's
/// document is stored and served back only under one of these; Odin's own
/// records (correlations, the publisher watermark, every `odin.*` type) and
/// Idunn's projected authority live in namespaces no provider can name, so a
/// peer cannot write them and the catalog cannot serve them.
///
/// The list is evidence, not a guess: each entry is a namespace some GameCult
/// source publishes to Odin's catalog connection, and the publisher is named
/// here so the next edit can check it. Add a namespace only with its publisher.
///   ghostlight     Ghostlight, `ghostlight.schema_catalog.v1`
///                  (schema: CultLib `packages/cultcache-ts/src/swarm-documents.ts`)
///   heimdall       Heimdall `src/odin-publication.ts` via `src/verse-state.ts`,
///                  `heimdall.command_boundary.v1`
///   muninn         Muninn `crates/muninn-contracts/src/records.rs`,
///                  `muninn.obs_stream_catalog.v1`
///   sleipnir       Sleipnir, `sleipnir.input_mapping.v1`
///                  (schema: CultLib `packages/cultcache-ts/src/swarm-documents.ts`)
///   gamecult.eve   Eve providers (Heimdall `src/odin-publication.ts`, Muninn,
///                  AetheriaEve): `provider_advertisement`, `surface`,
///                  `surface_state`, `plugin_advertisement`
///   gamecult.model Epiphany `epiphany-core/src/atlas/transport.rs`,
///                  `gamecult.model.atlas_publication.v0`
///   gamecult.aetheria  AetheriaEve `Aetheria.State.Daemon/Program.cs`,
///                  `gamecult.aetheria.asset_manifest.v1`
const PEER_DOCUMENT_NAMESPACES: &[&str] = &[
    "ghostlight",
    "heimdall",
    "muninn",
    "sleipnir",
    "gamecult.eve",
    "gamecult.model",
    "gamecult.aetheria",
];

/// Whole document types published under no dotted namespace: Muninn's media
/// stream advertisement and Ratatoskr/Muninn's request for it (`gamecult.media`
/// is their prefix, `_stream_advertisement` and `_stream_request` are not a
/// segment of it). Publisher: Muninn `crates/muninn-daemon/src/main.rs`
/// (`GAMECULT_MEDIA_STREAM_ADVERTISEMENT_SCHEMA`, `GAMECULT_MEDIA_STREAM_REQUEST_SCHEMA`).
const PEER_DOCUMENT_TYPES: &[&str] = &[
    "gamecult.media_stream_advertisement",
    "gamecult.media_stream_request",
];

fn is_peer_document_type(document_type: &str) -> bool {
    PEER_DOCUMENT_TYPES.contains(&document_type)
        || PEER_DOCUMENT_NAMESPACES.iter().any(|namespace| {
            document_type
                .strip_prefix(namespace)
                .is_some_and(|rest| rest.starts_with('.'))
        })
}

fn persist_generic_document(
    store: &MemoryOdinTopologyStore,
    document: &CultNetRawDocumentRecord,
) -> Result<()> {
    let document_type = document_type_for_schema(&document.schema_id)?;
    ensure!(
        is_peer_document_type(&document_type),
        "Odin accepts no {document_type} document from a provider"
    );
    store.put(CultCacheEnvelope {
        key: document.record_key.clone(),
        r#type: document_type,
        payload: document.payload.clone(),
        stored_at: document.stored_at.clone(),
        schema_id: Some(document.schema_id.clone()),
    });
    Ok(())
}

fn document_type_for_schema(schema_id: &str) -> Result<String> {
    ensure!(
        !schema_id.is_empty() && schema_id.trim() == schema_id,
        "provider schema id is empty or padded"
    );
    let Some((prefix, version)) = schema_id.rsplit_once(".v") else {
        bail!("provider schema id has no explicit version suffix");
    };
    ensure!(
        !prefix.is_empty()
            && !version.is_empty()
            && version.chars().all(|value| value.is_ascii_digit()),
        "provider schema id has an invalid version suffix"
    );
    Ok(prefix.into())
}

fn validate_raw_document_shape(document: &CultNetRawDocumentRecord) -> Result<()> {
    ensure!(
        document.payload_encoding == CultNetRawPayloadEncoding::Messagepack
            && !document.record_key.is_empty()
            && document.record_key.trim() == document.record_key
            && !document.payload.is_empty(),
        "raw provider document has an invalid key, encoding, or payload"
    );
    document_type_for_schema(&document.schema_id)?;
    DateTime::parse_from_rfc3339(&document.stored_at)
        .context("raw provider document has an invalid stored_at")?;
    Ok(())
}

fn decode_presence(payload: &[u8]) -> Result<GameCultRuntimePresenceHealthRecord> {
    let presence: GameCultRuntimePresenceHealthRecord = rmp_serde::from_slice(payload)?;
    ensure!(
        rmp_serde::to_vec(&presence)? == payload,
        "runtime presence is not canonical MessagePack"
    );
    presence.validate()?;
    Ok(presence)
}

fn validate_snapshot_filters(query: &CultMeshRudpSnapshotQuery) -> Result<()> {
    ensure!(
        !query.message_id.is_empty() && query.message_id.trim() == query.message_id,
        "snapshot message id is empty or padded"
    );
    for (label, values) in [
        ("schema", query.schema_ids.as_ref()),
        ("record key", query.record_keys.as_ref()),
    ] {
        if let Some(values) = values {
            ensure!(!values.is_empty(), "snapshot {label} filter is empty");
            let unique = values.iter().collect::<BTreeSet<_>>();
            ensure!(
                unique.len() == values.len()
                    && values
                        .iter()
                        .all(|value| !value.is_empty() && value.trim() == value),
                "snapshot {label} filter is duplicated, empty, or padded"
            );
        }
    }
    Ok(())
}

fn exact_self_presence_query(query: &CultMeshRudpSnapshotQuery) -> bool {
    query.schema_ids.as_deref() == Some(&[GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.to_owned()])
        && query.record_keys.as_deref() == Some(&[TARGET.to_owned()])
}

fn query_allows(query: &CultMeshRudpSnapshotQuery, document: &CultNetRawDocumentRecord) -> bool {
    query
        .schema_ids
        .as_ref()
        .is_none_or(|values| values.contains(&document.schema_id))
        && query
            .record_keys
            .as_ref()
            .is_none_or(|values| values.contains(&document.record_key))
}

fn sibling_lock_path(path: &Path) -> Result<PathBuf> {
    let mut name = path
        .file_name()
        .context("CultCache authority path has no filename")?
        .to_os_string();
    name.push(".lock");
    Ok(path.with_file_name(name))
}

fn required_environment(name: &str) -> Result<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty() && value.trim() == value)
        .with_context(|| format!("{name} is required for an Idunn-managed Odin"))
}

fn unix_millis() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock predates Unix epoch")?
        .as_millis()
        .try_into()
        .context("Unix time exceeds u64")
}

fn rfc3339_millis(value: u64) -> Result<String> {
    let millis: i64 = value
        .try_into()
        .context("timestamp exceeds RFC3339 range")?;
    Ok(DateTime::<Utc>::from_timestamp_millis(millis)
        .context("timestamp exceeds RFC3339 range")?
        .to_rfc3339())
}

#[cfg(test)]
mod tests {
    use cultnet_rs::{
        CultNetMessage, CultNetRudpReliableSendStatus, CultNetRudpSocketTransportConnection,
        CultNetRudpSocketTransportOptions, CultNetWireContract,
        GAMECULT_RUNTIME_PRESENCE_HEALTH_SIGNING_PURPOSE, GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA,
        GameCultServiceTrustAnchorRecord, IdunnExpectedCapability, IdunnExpectedRoute,
        IdunnRuntimeActivationLaunch, ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA,
        OdinRuntimeTopologyCorrelationRecord, encode_cultnet_message_to_vec,
        enroll_service_identity_at,
    };

    use super::*;

    #[test]
    fn options_are_exact_and_absolute() {
        let parsed = parse_options(
            [
                "--state-root",
                "/var/lib/gamecult/odin-v2",
                "--idunn-projection",
                "/var/lib/gamecult/idunn-projection/topology.cc",
                "--idunn-anchor",
                "/etc/gamecult/idunn/idunn-public-anchor.cc",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(
            parsed.store,
            Path::new("/var/lib/gamecult/odin-v2/topology.cc")
        );
        // The store path is derived from the slot, never supplied: naming it
        // directly would let a binding place Odin's state outside the root
        // Idunn hardened.
        assert!(
            parse_options(
                [
                    "--store",
                    "/var/lib/gamecult/odin-v2/topology.cc",
                    "--idunn-projection",
                    "/p",
                    "--idunn-anchor",
                    "/a"
                ]
                .into_iter()
                .map(str::to_owned)
            )
            .is_err()
        );
        assert!(
            parse_options(
                [
                    "--state-root",
                    "relative",
                    "--idunn-projection",
                    "/p",
                    "--idunn-anchor",
                    "/a"
                ]
                .into_iter()
                .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn schema_to_document_type_is_versioned_and_deterministic() {
        assert_eq!(
            document_type_for_schema("heimdall.command_boundary.v1").unwrap(),
            "heimdall.command_boundary"
        );
        for value in ["heimdall.command_boundary", "heimdall.v", ".v1", " x.v1"] {
            assert!(document_type_for_schema(value).is_err());
        }
    }

    #[test]
    fn only_exact_singleton_self_presence_query_is_a_route_challenge() {
        let exact = CultMeshRudpSnapshotQuery {
            session: cultmesh_rs::CultMeshRudpSessionKey {
                remote_addr: "127.0.0.1:1".parse().unwrap(),
                connection_id: 7,
            },
            message_id: "challenge".into(),
            requested_at_unix_millis: 1,
            schema_ids: Some(vec![GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()]),
            record_keys: Some(vec![TARGET.into()]),
        };
        assert!(exact_self_presence_query(&exact));
        let mut broad = exact.clone();
        broad.record_keys = None;
        assert!(!exact_self_presence_query(&broad));
    }

    #[test]
    fn first_odin_accepts_only_recent_provider_owned_warming_proofs() {
        let mut proofs = VecDeque::new();
        remember_recent_warming_proof(&mut proofs, "warming-one".into(), 100);
        assert!(recent_warming_proof(
            &proofs,
            "warming-one",
            100 + WARMING_PROOF_LIFETIME_MILLIS,
        ));
        assert!(!recent_warming_proof(
            &proofs,
            "warming-one",
            101 + WARMING_PROOF_LIFETIME_MILLIS,
        ));
        assert!(!recent_warming_proof(&proofs, "foreign-proof", 100));

        for index in 0..=MAX_RECENT_WARMING_PROOFS {
            remember_recent_warming_proof(&mut proofs, format!("warming-{index}"), 200);
        }
        assert_eq!(proofs.len(), MAX_RECENT_WARMING_PROOFS);
        assert_eq!(proofs.back().unwrap().0, "warming-64");
        assert!(!proofs.iter().any(|(proof, _)| proof == "warming-one"));
    }

    // ---- serving-loop liveness -------------------------------------------

    fn digest(byte: char) -> String {
        format!("sha256-{}", byte.to_string().repeat(64))
    }

    /// Odin as Idunn admits it: a real activation, provider anchor, write
    /// lease and projection on disk, `try_activate` run for real, and the
    /// production RUDP server bound to the candidate socket the Expected names.
    struct OdinWorld {
        _temp: tempfile::TempDir,
        state: Rc<RefCell<RuntimeState>>,
        server: OdinServer,
        /// The server's own socket, shared: what is done to it is done to the server.
        socket: UdpSocket,
        timers: ServingTimers,
        store: PathBuf,
        lease_path: PathBuf,
        provider_identity_id: String,
        lease: IdunnProcessWriteLeaseRecord,
    }

    fn write_single_record(path: &Path, envelope: CultCacheEnvelope) -> Result<()> {
        let store = SingleFileMessagePackBackingStore::new(path);
        let current = if path.is_file() {
            store.pull_all_read_only_snapshot()?
        } else {
            Vec::new()
        };
        ensure!(
            store.compare_exchange_snapshot(&current, &[envelope])?,
            "test store CAS failed"
        );
        Ok(())
    }

    fn lease_envelope(lease: &IdunnProcessWriteLeaseRecord) -> Result<CultCacheEnvelope> {
        Ok(CultCacheEnvelope {
            key: lease.target.clone(),
            r#type: IdunnProcessWriteLeaseRecord::TYPE.into(),
            payload: lease.canonical_bytes()?,
            stored_at: rfc3339_millis(lease.issued_at_unix_millis)?,
            schema_id: Some(IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into()),
        })
    }

    fn activated_odin() -> Result<OdinWorld> {
        activated_odin_serving(CultMeshRudpDocumentServerOptions::default())
    }

    /// The same world with the production server bound under `options`.
    fn activated_odin_serving(options: CultMeshRudpDocumentServerOptions) -> Result<OdinWorld> {
        activated_odin_with(options, |_| Ok(Vec::new()))
    }

    /// The same world, with Odin's store file holding what `seed` returns when
    /// Odin activates. `seed` runs before the lease is granted, with the
    /// runtime that is about to activate, so it can sign Odin's own presences.
    fn activated_odin_with(
        options: CultMeshRudpDocumentServerOptions,
        seed: impl FnOnce(&mut RuntimeState) -> Result<Vec<CultCacheEnvelope>>,
    ) -> Result<OdinWorld> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        let candidate = socket.local_addr()?;
        let now = unix_millis()?;
        let idunn_signer =
            enroll_service_identity_at::<IdunnServiceIdentity>(&root.join("idunn.cc"))?;
        let idunn_anchor = idunn_signer.trust_anchor()?;
        let odin_signer =
            enroll_service_identity_at::<OdinTopologyIdentity>(&root.join("odin-topology.cc"))?;
        let provider_signer = enroll_service_identity_at::<GameCultProviderHealthIdentity>(
            &root.join("odin-provider.cc"),
        )?;
        let expected = IdunnExpectedIncarnationRecord {
            schema_version: IDUNN_EXPECTED_INCARNATION_SCHEMA.into(),
            target: TARGET.into(),
            plan_id: digest('1'),
            incarnation_id: "odin/generation-1".into(),
            sealed_release_id: digest('2'),
            source_repository: "github.com/GameCult/Odin".into(),
            source_revision: "3".repeat(40),
            recipe_sha256: digest('4'),
            runtime_id: "odin-runtime".into(),
            expected_signer_identity_id: provider_signer.entry().identity_id.clone(),
            health_contract: HEALTH_CONTRACT.into(),
            artifact_sha256: digest('5'),
            state_schema_generation: Some(STATE_SCHEMA_GENERATION.into()),
            state_contract_sha256: Some(STATE_CONTRACT_SHA256.into()),
            write_lease_required: true,
            route: Some(IdunnExpectedRoute {
                route_id: "odin-route".into(),
                transport: "rudp".into(),
                stable_endpoint: "rudp://odin.internal:1000".into(),
                candidate_endpoint: format!("rudp://{candidate}"),
            }),
            capabilities: vec![IdunnExpectedCapability {
                capability: RENDEZVOUS_CAPABILITY.into(),
                schema: RENDEZVOUS_SCHEMA.into(),
                compatibility: RENDEZVOUS_COMPATIBILITY.into(),
                minimum_capacity: 1,
            }],
            dependencies: Vec::new(),
        };
        expected.validate()?;
        require_expected_contract(&expected, candidate)?;
        let launch =
            IdunnRuntimeActivationLaunch::issue(&expected, digest('7'), now - 20, &idunn_signer)?;
        let activation = launch.activation().clone();
        let mut credential = Vec::new();
        launch.write_credential(&mut credential)?;
        let activation_signer =
            IdunnRuntimeActivationSigner::from_credential_reader(credential.as_slice())?;
        let provider_anchor = GameCultServiceTrustAnchorRecord {
            schema_version: GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into(),
            trust_anchor_id: format!("root/{TARGET}/runtime-presence"),
            service_id: TARGET.into(),
            runtime_id: expected.runtime_id.clone(),
            signer_identity_id: provider_signer.entry().identity_id.clone(),
            signer_public_key: provider_signer.entry().public_key.clone(),
            signature_algorithm: "ed25519".into(),
            signing_purpose: GAMECULT_RUNTIME_PRESENCE_HEALTH_SIGNING_PURPOSE.into(),
            signed_schema: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
            binding_authority: "root".into(),
            bound_at_unix_millis: now - 100,
            expires_at_unix_millis: None,
            private_state_exposed: false,
        };
        let provider_identity_id = provider_signer.entry().identity_id.clone();
        let expected_sha256 = expected.canonical_sha256()?;
        let authority_material = RuntimeAuthority {
            activation_sha256: activation.canonical_sha256()?,
            expected_sha256: expected_sha256.clone(),
            expected: expected.clone(),
            activation: activation.clone(),
            activation_signer,
            provider_signer,
        };
        let store = root.join(TOPOLOGY_SLOT);
        let projection_path = root.join("idunn-projection.cc");
        let lease_path = root.join("process-write-lease.cc");
        let mut runtime = RuntimeState {
            options: Options {
                store: store.clone(),
                idunn_projection: projection_path.clone(),
                idunn_anchor: root.join("unused-idunn-anchor.cc"),
            },
            candidate,
            authority_material,
            idunn_anchor: Some(idunn_anchor),
            topology_signer: Some(odin_signer),
            topology: None,
            write_lease: None,
            write_lease_path: lease_path.clone(),
            recent_warming_proofs: VecDeque::new(),
            publisher_sequence: 0,
            log_gate: RefCell::default(),
        };
        let seeded = seed(&mut runtime)?;
        if !seeded.is_empty() {
            ensure!(
                SingleFileMessagePackBackingStore::new(&store).compare_exchange_snapshot(&[], &seeded)?,
                "test store seed failed"
            );
        }

        // The lease names the Warming presence Odin signed for Idunn's probe.
        let warming = runtime.signed_presence_document("warming", "test warming")?;
        let lease = IdunnProcessWriteLeaseRecord {
            schema_version: IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into(),
            target: TARGET.into(),
            expected_projection_sha256: expected_sha256,
            plan_id: expected.plan_id.clone(),
            incarnation_id: expected.incarnation_id.clone(),
            sealed_release_id: expected.sealed_release_id.clone(),
            activation_witness_sha256: activation.canonical_sha256()?,
            state_schema_generation: STATE_SCHEMA_GENERATION.into(),
            state_contract_sha256: STATE_CONTRACT_SHA256.into(),
            runtime_id: expected.runtime_id.clone(),
            runtime_instance_id: activation.runtime_instance_id.clone(),
            warming_presence_sha256: decode_presence(&warming.payload)?.canonical_sha256()?,
            lease_epoch: 1,
            issued_at_unix_millis: now - 5,
        };
        write_single_record(&lease_path, lease_envelope(&lease)?)?;
        File::create(sibling_lock_path(&lease_path)?)?;

        let incarnation_key = IncarnationRef::of(&expected)?.key();
        let mut projected_lease = lease_envelope(&lease)?;
        projected_lease.key = incarnation_key.clone();
        let projection = vec![
            CultCacheEnvelope {
                key: incarnation_key.clone(),
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: expected.canonical_bytes()?,
                stored_at: rfc3339_millis(now - 30)?,
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            },
            CultCacheEnvelope {
                key: provider_anchor.trust_anchor_id.clone(),
                r#type: GameCultServiceTrustAnchorRecord::TYPE.into(),
                payload: rmp_serde::to_vec(&provider_anchor)?,
                stored_at: rfc3339_millis(provider_anchor.bound_at_unix_millis)?,
                schema_id: Some(GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into()),
            },
            CultCacheEnvelope {
                key: incarnation_key,
                r#type: IdunnRuntimeActivationRecord::TYPE.into(),
                payload: activation.canonical_bytes()?,
                stored_at: rfc3339_millis(activation.issued_at_unix_millis)?,
                schema_id: Some(IDUNN_RUNTIME_ACTIVATION_SCHEMA.into()),
            },
            projected_lease,
        ];
        ensure!(
            SingleFileMessagePackBackingStore::new(&projection_path)
                .compare_exchange_snapshot(&[], &projection)?,
            "test projection CAS failed"
        );

        ensure!(
            runtime.try_activate()?,
            "Odin did not activate in the fixture"
        );
        let state = Rc::new(RefCell::new(runtime));
        let server_socket = socket.try_clone()?;
        let server = CultMeshRudpDocumentServer::new(
            socket,
            SinkHandle(state.clone()),
            SnapshotHandle(state.clone()),
            CultMeshSystemClock::default(),
            options,
        )?;
        Ok(OdinWorld {
            _temp: temp,
            state,
            server,
            socket: server_socket,
            timers: ServingTimers::default(),
            store,
            lease_path,
            provider_identity_id,
            lease,
        })
    }

    impl OdinWorld {
        fn pass(&mut self) -> Result<bool> {
            serving_pass(&self.state, &mut self.server, &mut self.timers)
        }

        /// Serve as `main` does, with a stop already requested or not.
        fn serve(&mut self, stop_requested: bool) -> Result<()> {
            serve(
                &self.state,
                &mut self.server,
                &mut self.timers,
                &AtomicBool::new(stop_requested),
            )
        }

        /// One heartbeat of Odin's own: a change to its bookkeeping, which is
        /// written on the interval and not at once.
        fn heartbeat(&self) -> Result<()> {
            self.state.borrow_mut().publish_self_presence("test heartbeat")
        }

        /// Replace the lease under Odin with a later epoch. Odin holds a shared
        /// lock on the lease's lifetime lock, which is what stops Idunn from
        /// writing the lease under it; the test swaps the file underneath.
        fn swap_lease(&self) -> Result<()> {
            let mut replaced = self.lease.clone();
            replaced.lease_epoch += 1;
            let scratch = self.lease_path.with_file_name("replacement-lease.cc");
            write_single_record(&scratch, lease_envelope(&replaced)?)?;
            std::fs::copy(&scratch, &self.lease_path)?;
            Ok(())
        }

        /// Add records to Idunn's projection as Idunn would publish them.
        fn append_projection(&self, added: Vec<CultCacheEnvelope>) -> Result<()> {
            self.tamper_projection(|mut entries| {
                entries.extend(added);
                entries
            })
        }

        /// Rewrite Odin's own store as `change` sees it, in one exchange.
        fn tamper_store(
            &self,
            change: impl FnOnce(Vec<CultCacheEnvelope>) -> Vec<CultCacheEnvelope>,
        ) -> Result<()> {
            let store = SingleFileMessagePackBackingStore::new(&self.store);
            let current = store.pull_all_read_only_snapshot()?;
            let next = change(current.clone());
            ensure!(
                store.compare_exchange_snapshot(&current, &next)?,
                "test store CAS failed"
            );
            Ok(())
        }

        /// Rewrite Idunn's projection as `change` sees it, in one exchange.
        fn tamper_projection(
            &self,
            change: impl FnOnce(Vec<CultCacheEnvelope>) -> Vec<CultCacheEnvelope>,
        ) -> Result<()> {
            let path = self.state.borrow().options.idunn_projection.clone();
            let store = SingleFileMessagePackBackingStore::new(path);
            let current = store.pull_all_read_only_snapshot()?;
            let next = change(current.clone());
            ensure!(
                store.compare_exchange_snapshot(&current, &next)?,
                "test projection CAS failed"
            );
            Ok(())
        }

        /// The whole catalog, as a Verse peer reading it would get it.
        fn catalog(&self) -> Result<Vec<CultNetRawDocumentRecord>> {
            self.state.borrow_mut().raw_snapshot(&CultMeshRudpSnapshotQuery {
                session: cultmesh_rs::CultMeshRudpSessionKey {
                    remote_addr: "127.0.0.1:1".parse()?,
                    connection_id: 7,
                },
                message_id: "catalog".into(),
                requested_at_unix_millis: 1,
                schema_ids: None,
                record_keys: None,
            })
        }

        /// Every record in Odin's working set.
        fn working_set(&self) -> Vec<CultCacheEnvelope> {
            let state = self.state.borrow();
            let records = state.topology.as_ref().unwrap().store.records();
            records.values().cloned().collect()
        }

        /// Every record in Odin's store file.
        fn file_records(&self) -> Result<Vec<CultCacheEnvelope>> {
            SingleFileMessagePackBackingStore::new(&self.store).pull_all_read_only_snapshot()
        }

        fn stored_keys(&self, record_type: &str) -> Result<Vec<String>> {
            Ok(self
                .working_set()
                .into_iter()
                .filter(|entry| entry.r#type == record_type)
                .map(|entry| entry.key)
                .collect())
        }

        /// The publisher sequence of Odin's own admitted presence, 0 when none
        /// is held.
        fn stored_sequence(&self) -> Result<u64> {
            let key = format!(
                "{}/{}",
                self_incarnation(&self.state.borrow().authority_material).key(),
                self.provider_identity_id
            );
            self.working_set()
                .into_iter()
                .find(|entry| {
                    entry.r#type == GameCultRuntimePresenceHealthRecord::TYPE && entry.key == key
                })
                .map_or(Ok(0), |entry| {
                    Ok(decode_presence(&entry.payload)?.publisher_sequence)
                })
        }

        /// Run a peer on its own thread while Odin polls its socket, and nothing
        /// else. The timed work of a pass (refresh and heartbeat, both fsync-heavy)
        /// would leave the peers waiting past their connect deadline on a loaded
        /// host; the tests take the timed passes themselves afterwards.
        fn serve_while<T: Send + 'static>(
            &mut self,
            peer: impl FnOnce() -> T + Send + 'static,
        ) -> Result<T> {
            let peer = thread::spawn(peer);
            while !peer.is_finished() {
                if !poll_server(&mut self.server, &mut self.timers.poll_failing_since)? {
                    thread::sleep(IDLE_POLL_INTERVAL);
                }
            }
            Ok(peer.join().expect("peer thread panicked"))
        }
    }

    const PEER_CONNECTION_BASE: u32 = 0x7000_0000;

    /// A publisher the way Ghostlight and CodexConnector behave: connect,
    /// optionally put one message, never Disconnect.
    fn lingering_peer(
        target: SocketAddr,
        connection_id: u32,
        message: Option<CultNetMessage>,
    ) -> Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        let mut transport = CultNetRudpSocketTransportConnection::new(
            CultNetRudpSocketTransportOptions::client("test-peer", socket, target, connection_id),
        )?;
        transport.connect(Vec::new())?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !transport.connected() {
            let _ = transport.receive_once()?;
            transport.poll_resends()?;
            ensure!(Instant::now() < deadline, "timed out connecting");
        }
        if let Some(message) = message {
            let receipt = transport.send_reliable(
                "schema",
                encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)?,
            )?;
            let deadline = Instant::now() + Duration::from_millis(1500);
            while transport.reliable_send_status(&receipt) == CultNetRudpReliableSendStatus::Pending
                && Instant::now() < deadline
            {
                let _ = transport.receive_once()?;
                transport.poll_resends()?;
            }
        }
        Ok(())
    }

    /// Soul's 70-lingering-session probe: with the server's session table
    /// full of publishers that never Disconnect, Odin's own heartbeat still
    /// lands. The table is Odin's own production server bound to the candidate
    /// socket, so a self-publication that went over RUDP would find it full.
    #[test]
    fn self_presence_lands_while_lingering_publishers_hold_every_session() -> Result<()> {
        // Sessions do not idle out in this world. The property is that the
        // heartbeat lands while the table is full, and the fsync-bound passes
        // it takes can outlast the default 30 s idle timeout on a saturated
        // disk (Yggdrasil, 2026-09-30: the first pass alone took 20 s, and
        // 20 of the 64 sessions had expired by the last assertion), which
        // ends the sessions the test is holding rather than testing anything.
        let mut odin = activated_odin_serving(CultMeshRudpDocumentServerOptions {
            session_idle_timeout: Duration::from_secs(3600),
            ..CultMeshRudpDocumentServerOptions::default()
        })?;
        let target = odin.server.local_addr()?;
        // Peers arrive until 64 are admitted, then one more is refused. A peer
        // that times out under load is retried as a new one: the property is a
        // full table, not that no packet is ever late. Refused peers wait out
        // their whole connect deadline, so only one is tried.
        let (admitted, overflow_admitted) = odin.serve_while(move || {
            let (mut admitted, mut next) = (0, 0);
            while admitted < 64 && next < 96 {
                match lingering_peer(target, PEER_CONNECTION_BASE + next, None) {
                    Ok(()) => admitted += 1,
                    Err(error) => eprintln!("peer {next} was not admitted: {error:#}"),
                }
                next += 1;
            }
            let overflow = lingering_peer(target, PEER_CONNECTION_BASE + next, None).is_ok();
            (admitted, overflow)
        })?;
        assert_eq!(admitted, 64, "the default session table is 64 wide");
        assert!(!overflow_admitted, "the 65th publisher is refused");
        assert_eq!(odin.server.session_count(), 64, "the table is full");

        // The first timed pass, with the table already full, publishes once.
        odin.pass()?;
        let before = odin.stored_sequence()?;
        for _ in 0..5 {
            odin.timers.last_heartbeat = None;
            odin.pass()?;
        }
        assert_eq!(
            odin.stored_sequence()?,
            before + 5,
            "each heartbeat is admitted into Odin's store"
        );
        assert_eq!(odin.server.session_count(), 64);
        Ok(())
    }

    /// A store write that fails is logged and retried; it never ends Odin, and
    /// the next attempt lands once the fault clears.
    #[test]
    fn a_failed_store_write_does_not_end_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        assert_eq!(odin.file_records()?, odin.working_set(), "the first pass writes");

        let written = std::fs::read(&odin.store)?;
        std::fs::write(&odin.store, b"not a cultcache store")?;
        assert!(
            provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1]).is_err(),
            "a put whose write fails is refused"
        );
        assert!(
            odin.state.borrow().flush().is_err(),
            "the injected fault must actually fail the write"
        );
        odin.timers = ServingTimers::default();
        odin.pass()?;
        let suppressed = |odin: &OdinWorld| odin.state.borrow().log_gate.borrow().suppressed_total;
        assert_eq!(suppressed(&odin), 0, "each distinct failure is logged");

        // The same fault on later passes is counted, not printed again.
        for repeat in 1..=3 {
            odin.timers = ServingTimers::default();
            odin.pass()?;
            assert!(
                suppressed(&odin) >= repeat,
                "pass {repeat} repeated a logged failure"
            );
        }

        std::fs::write(&odin.store, written)?;
        odin.timers = ServingTimers::default();
        odin.pass()?;
        assert_eq!(
            odin.file_records()?,
            odin.working_set(),
            "the write after the fault clears lands"
        );
        Ok(())
    }

    /// Losing the write lease is the one condition that ends the serving loop.
    #[test]
    fn losing_the_write_lease_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;

        odin.swap_lease()?;
        odin.timers = ServingTimers::default();
        let error = odin.pass().unwrap_err();
        assert!(
            error.downcast_ref::<WriteLeaseLost>().is_some(),
            "{error:#}"
        );

        std::fs::remove_file(&odin.lease_path)?;
        odin.timers = ServingTimers::default();
        let error = odin.pass().unwrap_err();
        assert!(
            error.downcast_ref::<WriteLeaseLost>().is_some(),
            "{error:#}"
        );
        Ok(())
    }

    /// The heartbeat and the refresh each check the lease themselves: with only
    /// one of them due, a swapped lease still ends Odin.
    #[test]
    fn each_timer_checks_the_write_lease_itself() -> Result<()> {
        for heartbeat_due in [true, false] {
            let mut odin = activated_odin()?;
            odin.pass()?;
            odin.swap_lease()?;
            odin.timers = ServingTimers::default();
            if heartbeat_due {
                odin.timers.last_projection_refresh = Some(Instant::now());
            } else {
                odin.timers.last_heartbeat = Some(Instant::now());
            }
            let error = odin.pass().unwrap_err();
            assert!(
                error.downcast_ref::<WriteLeaseLost>().is_some(),
                "heartbeat_due={heartbeat_due}: {error:#}"
            );
        }
        Ok(())
    }

    /// A lease Odin cannot read is a lease it cannot show it still holds, so
    /// it is lost, not a transient fault to retry.
    #[test]
    fn an_unreadable_write_lease_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        std::fs::write(&odin.lease_path, b"not a cultcache store")?;
        odin.timers = ServingTimers::default();
        let error = odin.pass().unwrap_err();
        assert!(
            error.downcast_ref::<WriteLeaseLost>().is_some(),
            "{error:#}"
        );
        Ok(())
    }

    /// Soul's ghost probe: one Expected that will not decode is that
    /// incarnation's own failure. The other incarnations still refresh, and
    /// the catalog still serves.
    #[test]
    fn one_undecodable_incarnation_leaves_the_others_and_the_catalog_readable() -> Result<()> {
        let mut odin = activated_odin()?;
        let mut candidate = odin.state.borrow().authority_material.expected.clone();
        candidate.target = "sibling".into();
        candidate.validate()?;
        let candidate_key = IncarnationRef::of(&candidate)?.key();
        // "aaghost" sorts before "odin", so a refresh that stops at its first
        // failure never reaches the good incarnations.
        let ghost_key = IncarnationRef::new("aaghost", digest('a')).key();
        odin.append_projection(vec![
            CultCacheEnvelope {
                key: ghost_key,
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: vec![0xc1, 0x00],
                stored_at: rfc3339_millis(unix_millis()?)?,
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            },
            CultCacheEnvelope {
                key: candidate_key.clone(),
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: candidate.canonical_bytes()?,
                stored_at: rfc3339_millis(unix_millis()?)?,
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            },
        ])?;
        assert!(
            !odin
                .stored_keys(OdinRuntimeTopologyCorrelationRecord::TYPE)?
                .contains(&candidate_key)
        );

        // A catalog read serves what is stored; refreshing is the timer's.
        odin.catalog()?;
        assert!(
            !odin
                .stored_keys(OdinRuntimeTopologyCorrelationRecord::TYPE)?
                .contains(&candidate_key),
            "a read does not refresh"
        );

        odin.pass()?;
        assert!(
            odin.stored_keys(OdinRuntimeTopologyCorrelationRecord::TYPE)?
                .contains(&candidate_key),
            "the good incarnation was refreshed past the ghost"
        );

        // The catalog serves past the ghost; correlations are not part of it.
        let catalog = odin.catalog()?;
        assert_eq!(
            documents_of(&catalog, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA),
            1
        );
        Ok(())
    }

    fn assert_refused(error: &anyhow::Error) {
        assert!(
            error.downcast_ref::<PresenceAuthorityRefused>().is_some(),
            "{error:#}"
        );
    }

    /// Temporary rule (see `survive`): a self-presence Odin's authority will
    /// never admit ends Odin. With no current activation Idunn projects, there
    /// is no verifiable authority.
    #[test]
    fn a_projection_without_authority_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.tamper_projection(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.r#type != IdunnRuntimeActivationRecord::TYPE)
                .collect()
        })?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    /// Idunn's activation of Odin carries a signature Idunn's anchor does not
    /// verify: the authority cannot be verified.
    #[test]
    fn an_activation_that_does_not_verify_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.tamper_projection(|entries| {
            entries
                .into_iter()
                .map(|mut entry| {
                    if entry.r#type == IdunnRuntimeActivationRecord::TYPE {
                        let mut activation =
                            IdunnRuntimeActivationRecord::decode_canonical(&entry.payload)
                                .unwrap();
                        activation.signature[0] ^= 1;
                        entry.payload = activation.canonical_bytes().unwrap();
                    }
                    entry
                })
                .collect()
        })?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    /// The projected provider anchor names Odin's signer identity but carries
    /// a key that identity does not derive from: the authority cannot verify.
    #[test]
    fn a_signer_that_does_not_match_the_anchor_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.tamper_projection(|entries| {
            entries
                .into_iter()
                .map(|mut entry| {
                    if entry.r#type == GameCultServiceTrustAnchorRecord::TYPE {
                        let mut anchor: GameCultServiceTrustAnchorRecord =
                            rmp_serde::from_slice(&entry.payload).unwrap();
                        anchor.signer_public_key = vec![7; 32];
                        entry.payload = rmp_serde::to_vec(&anchor).unwrap();
                    }
                    entry
                })
                .collect()
        })?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());

        // The other way round: the authority is sound, but the key Odin signs
        // with is not the Expected signer.
        let mut odin = activated_odin()?;
        odin.pass()?;
        let other = odin._temp.path().join("other-provider.cc");
        odin.state.borrow_mut().authority_material.provider_signer =
            enroll_service_identity_at::<GameCultProviderHealthIdentity>(&other)?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    /// A stored presence of Odin's own activation that no longer authenticates
    /// makes its publisher sequence unknowable; the claim is refused, closed.
    /// The store is Odin's alone while it serves, so the bad record is in the
    /// file when Odin activates.
    #[test]
    fn a_stored_presence_that_no_longer_authenticates_ends_odin() -> Result<()> {
        let mut odin = activated_odin_with(CultMeshRudpDocumentServerOptions::default(), |runtime| {
            let warming = runtime.signed_presence_document("warming", "earlier launch")?;
            // Received two minutes after it was observed: outside the trusted window.
            Ok(vec![CultCacheEnvelope {
                key: format!(
                    "{}/{}",
                    self_incarnation(&runtime.authority_material).key(),
                    runtime.authority_material.provider_signer.entry().identity_id
                ),
                r#type: GameCultRuntimePresenceHealthRecord::TYPE.into(),
                payload: warming.payload,
                stored_at: rfc3339_millis(unix_millis()? + 120_000)?,
                schema_id: Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()),
            }])
        })?;
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    fn generic_document() -> Result<CultCacheEnvelope> {
        Ok(CultCacheEnvelope {
            key: "doc-1".into(),
            r#type: "ghostlight.doc".into(),
            payload: vec![1],
            stored_at: rfc3339_millis(unix_millis()?)?,
            schema_id: Some("ghostlight.doc.v1".into()),
        })
    }

    fn documents_of(documents: &[CultNetRawDocumentRecord], schema: &str) -> usize {
        documents
            .iter()
            .filter(|document| document.schema_id == schema)
            .count()
    }

    /// A candidate incarnation of Odin, as Idunn projects it beside the
    /// incumbent for the whole deploy window.
    fn project_candidate_incarnation(odin: &OdinWorld) -> Result<IncarnationRef> {
        let mut candidate = odin.state.borrow().authority_material.expected.clone();
        candidate.incarnation_id = "odin/generation-2".into();
        candidate.sealed_release_id = digest('9');
        candidate.validate()?;
        let incarnation = IncarnationRef::of(&candidate)?;
        odin.append_projection(vec![CultCacheEnvelope {
            key: incarnation.key(),
            r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
            payload: candidate.canonical_bytes()?,
            stored_at: rfc3339_millis(unix_millis()?)?,
            schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
        }])?;
        Ok(incarnation)
    }

    /// Idunn projects the incumbent and the candidate of a target for the whole
    /// deploy window, and Odin writes a correlation for each. The catalog does
    /// not serve correlations, so two of one target cannot collide in it, and
    /// the presence queries every consumer makes are unaffected.
    #[test]
    fn two_incarnations_of_one_target_leave_the_catalog_readable() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let candidate = project_candidate_incarnation(&odin)?;
        odin.timers = ServingTimers::default();
        odin.pass()?;

        // Idunn reads the correlations from the store, one per incarnation.
        let keys = odin.stored_keys(OdinRuntimeTopologyCorrelationRecord::TYPE)?;
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(keys.contains(&candidate.key()));

        let catalog = odin.catalog()?;
        assert_eq!(
            documents_of(&catalog, ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA),
            0
        );
        assert_eq!(
            documents_of(&catalog, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA),
            1
        );
        let presence_only = odin
            .state
            .borrow_mut()
            .raw_snapshot(&CultMeshRudpSnapshotQuery {
                session: cultmesh_rs::CultMeshRudpSessionKey {
                    remote_addr: "127.0.0.1:1".parse()?,
                    connection_id: 7,
                },
                message_id: "presence-only".into(),
                requested_at_unix_millis: 1,
                schema_ids: Some(vec![GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()]),
                record_keys: None,
            })?;
        assert_eq!(presence_only.len(), 1);
        Ok(())
    }

    /// One record that does not decode is skipped where it is met: Odin still
    /// starts and serves, and the catalog serves everything else. The records
    /// are in the file when Odin activates.
    #[test]
    fn records_that_do_not_decode_never_stop_the_catalog_or_startup() -> Result<()> {
        let mut odin = activated_odin_with(CultMeshRudpDocumentServerOptions::default(), |runtime| {
            let own_signer = runtime
                .authority_material
                .provider_signer
                .entry()
                .identity_id
                .clone();
            let stamp = rfc3339_millis(unix_millis()?)?;
            let bad_presence = |key: String| CultCacheEnvelope {
                key,
                r#type: GameCultRuntimePresenceHealthRecord::TYPE.into(),
                payload: vec![0xc1],
                stored_at: stamp.clone(),
                schema_id: Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()),
            };
            let earlier = runtime.signed_presence_document("warming", "earlier incarnation")?;
            Ok(vec![
                bad_presence("ghost".into()),
                bad_presence(format!("{TARGET}@{}/{own_signer}", digest('c'))),
                CultCacheEnvelope {
                    key: format!("{TARGET}@{}/{own_signer}", digest('d')),
                    r#type: GameCultRuntimePresenceHealthRecord::TYPE.into(),
                    payload: earlier.payload,
                    stored_at: stamp.clone(),
                    schema_id: Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()),
                },
                generic_document()?,
            ])
        })?;

        let catalog = odin.catalog()?;
        assert_eq!(
            documents_of(&catalog, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA),
            1,
            "the presence that decodes is served"
        );
        assert_eq!(documents_of(&catalog, "ghostlight.doc.v1"), 1);
        odin.pass()?;
        assert!(odin.stored_sequence()? > 0, "Odin publishes past them");
        Ok(())
    }

    /// The bytes this thread has read so far, as the kernel counts them.
    fn thread_bytes_read() -> Result<u64> {
        let io = std::fs::read_to_string("/proc/thread-self/io")?;
        Ok(io
            .lines()
            .find_map(|line| line.strip_prefix("rchar: "))
            .context("no rchar in /proc/thread-self/io")?
            .parse()?)
    }

    /// A catalog query reads Idunn's projection once, however many stored
    /// presences it has to look up there.
    #[test]
    fn a_catalog_query_reads_the_projection_once() -> Result<()> {
        const PRESENCES: usize = 8;
        let odin = activated_odin_with(CultMeshRudpDocumentServerOptions::default(), |runtime| {
            let stamp = rfc3339_millis(unix_millis()?)?;
            (0..PRESENCES)
                .map(|index| {
                    Ok(CultCacheEnvelope {
                        key: format!("stored-{index}"),
                        r#type: GameCultRuntimePresenceHealthRecord::TYPE.into(),
                        payload: runtime.signed_presence_document("warming", "stored")?.payload,
                        stored_at: stamp.clone(),
                        schema_id: Some(GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()),
                    })
                })
                .collect()
        })?;
        let projection = odin.state.borrow().options.idunn_projection.clone();
        let projection_bytes = std::fs::metadata(&projection)?.len();
        // Every stored presence is looked up in the projection; the filter
        // then keeps none of them, so none collides with another in the reply.
        let query = CultMeshRudpSnapshotQuery {
            session: cultmesh_rs::CultMeshRudpSessionKey {
                remote_addr: "127.0.0.1:1".parse()?,
                connection_id: 7,
            },
            message_id: "lookups".into(),
            requested_at_unix_millis: 1,
            schema_ids: None,
            record_keys: Some(vec!["absent".into()]),
        };
        let before = thread_bytes_read()?;
        assert!(odin.state.borrow_mut().raw_snapshot(&query)?.is_empty());
        let read = thread_bytes_read()? - before;
        assert!(read >= projection_bytes, "the projection was read: {read} bytes");
        assert!(
            read < 2 * projection_bytes,
            "and only once: {read} bytes read, the projection is {projection_bytes}"
        );
        Ok(())
    }

    /// A presence whose incarnation's projection cannot be read is skipped as
    /// that record's failure; the rest of the catalog is still served.
    #[test]
    fn a_presence_whose_projection_cannot_be_read_is_skipped_not_fatal() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1])?;
        odin.tamper_projection(|entries| {
            entries
                .into_iter()
                .map(|mut entry| {
                    if entry.r#type == GameCultServiceTrustAnchorRecord::TYPE {
                        entry.payload = vec![0xc1];
                    }
                    entry
                })
                .collect()
        })?;
        let catalog = odin.catalog()?;
        assert_eq!(documents_of(&catalog, "ghostlight.doc.v1"), 1);
        assert_eq!(
            documents_of(&catalog, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA),
            0,
            "the presence whose projection cannot be read is skipped"
        );
        Ok(())
    }

    /// A projection file that cannot be read at all, because it does not
    /// decode or cannot be opened, skips every presence and nothing else: the
    /// catalog still serves the peer documents.
    #[test]
    fn a_projection_file_that_cannot_be_read_skips_presences_not_the_catalog() -> Result<()> {
        let faults: [fn(&Path) -> Result<()>; 2] = [
            |projection| Ok(std::fs::write(projection, b"not a cultcache store")?),
            |projection| Ok(std::fs::remove_file(sibling_lock_path(projection)?)?),
        ];
        for fault in faults {
            let mut odin = activated_odin()?;
            odin.pass()?;
            provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1])?;
            assert_eq!(
                documents_of(&odin.catalog()?, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA),
                1,
                "Odin's own presence is served while the projection reads"
            );
            fault(&odin.state.borrow().options.idunn_projection)?;
            let catalog = odin.catalog()?;
            assert_eq!(documents_of(&catalog, "ghostlight.doc.v1"), 1);
            assert_eq!(documents_of(&catalog, GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA), 0);
        }
        Ok(())
    }

    /// Every failure to establish Odin's OWN authority ends Odin (temporary
    /// rule, see `survive`), whatever shape the failure takes: an anchor that
    /// does not decode or names another schema, an Expected Idunn no longer
    /// projects, an activation or lease that does not decode.
    #[test]
    fn every_failure_to_establish_its_own_authority_ends_odin() -> Result<()> {
        type Tamper = fn(CultCacheEnvelope) -> Option<CultCacheEnvelope>;
        let cases: [(&str, Tamper); 5] = [
            ("anchor does not decode", |mut entry| {
                if entry.r#type == GameCultServiceTrustAnchorRecord::TYPE {
                    entry.payload = vec![0xc1];
                }
                Some(entry)
            }),
            ("anchor names another schema", |mut entry| {
                if entry.r#type == GameCultServiceTrustAnchorRecord::TYPE {
                    entry.schema_id = Some("gamecult.service_trust_anchor.v99".into());
                }
                Some(entry)
            }),
            ("activation does not decode", |mut entry| {
                if entry.r#type == IdunnRuntimeActivationRecord::TYPE {
                    entry.payload = vec![0xc1];
                }
                Some(entry)
            }),
            ("lease does not decode", |mut entry| {
                if entry.r#type == IdunnProcessWriteLeaseRecord::TYPE {
                    entry.payload = vec![0xc1];
                }
                Some(entry)
            }),
            ("Expected is no longer projected", |entry| {
                (entry.r#type != IdunnExpectedIncarnationRecord::TYPE).then_some(entry)
            }),
        ];
        for (name, tamper) in cases {
            let mut odin = activated_odin()?;
            odin.pass()?;
            let before = odin.stored_sequence()?;
            odin.tamper_projection(|entries| entries.into_iter().filter_map(tamper).collect())?;
            for _ in 0..3 {
                odin.timers = ServingTimers::default();
                let error = odin
                    .pass()
                    .expect_err(&format!("{name}: Odin must not serve a frozen presence"));
                assert!(
                    error.downcast_ref::<PresenceAuthorityRefused>().is_some(),
                    "{name}: {error:#}"
                );
            }
            assert_eq!(odin.stored_sequence()?, before, "{name}");
        }
        Ok(())
    }

    /// Another target's broken records are that target's fault: Odin keeps
    /// serving, and the failing refresh is logged once, not on every pass.
    #[test]
    fn another_targets_broken_authority_never_ends_odin_and_logs_once() -> Result<()> {
        let mut odin = activated_odin()?;
        let mut sibling = odin.state.borrow().authority_material.expected.clone();
        sibling.target = "sibling".into();
        sibling.validate()?;
        odin.append_projection(vec![
            CultCacheEnvelope {
                key: IncarnationRef::of(&sibling)?.key(),
                r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
                payload: sibling.canonical_bytes()?,
                stored_at: rfc3339_millis(unix_millis()?)?,
                schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
            },
            CultCacheEnvelope {
                key: "root/sibling/runtime-presence".into(),
                r#type: GameCultServiceTrustAnchorRecord::TYPE.into(),
                payload: vec![0xc1],
                stored_at: rfc3339_millis(unix_millis()?)?,
                schema_id: Some(GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into()),
            },
        ])?;
        let suppressed = |odin: &OdinWorld| odin.state.borrow().log_gate.borrow().suppressed_total;

        odin.pass()?;
        assert_eq!(suppressed(&odin), 0, "the first failure is logged");
        for repeat in 1..=3 {
            odin.timers = ServingTimers::default();
            odin.pass()?;
            assert_eq!(suppressed(&odin), repeat, "a repeat is not logged again");
        }
        Ok(())
    }

    #[test]
    fn a_repeating_line_is_logged_once_per_interval_or_when_it_changes() {
        let mut gate = LogGate::default();
        let start = Instant::now();
        assert_eq!(gate.admit("s", "m", start), Some(0));
        assert_eq!(
            gate.admit(
                "s",
                "m",
                start + REPEATED_LOG_INTERVAL - Duration::from_millis(1)
            ),
            None
        );
        // A changed message is a new line, and reports what it swallowed.
        let later = start + Duration::from_secs(1);
        assert_eq!(gate.admit("s", "other", later), Some(1));
        // Subjects are independent.
        assert_eq!(gate.admit("t", "other", later), Some(0));
        // The interval is measured from the last line written.
        assert_eq!(
            gate.admit("s", "other", later + REPEATED_LOG_INTERVAL),
            Some(0)
        );
        assert_eq!(gate.suppressed_total, 1);

        for index in 0..=2 * MAX_LOG_SUBJECTS {
            gate.admit(&format!("subject-{index}"), "m", later);
        }
        assert!(gate.seen.len() <= MAX_LOG_SUBJECTS);
    }

    /// An application rejection ends only the offending peer's session, and a
    /// stray datagram is dropped; neither ends Odin's serving loop.
    #[test]
    fn an_application_rejection_or_a_stray_packet_does_not_end_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        let target = odin.server.local_addr()?;
        let not_a_presence = CultNetMessage::DocumentPutRaw {
            message_id: "rejected".into(),
            document: CultNetRawDocumentRecord {
                schema_id: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
                record_key: "ghostlight".into(),
                stored_at: rfc3339_millis(unix_millis()?)?,
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: vec![0xc1],
                source_runtime_id: None,
                source_agent_id: None,
                source_role: None,
                tags: None,
            },
        };
        odin.serve_while(move || {
            lingering_peer(target, PEER_CONNECTION_BASE, Some(not_a_presence))
        })??;
        assert_eq!(
            odin.server.session_count(),
            0,
            "the rejection ended the offending session"
        );

        UdpSocket::bind("127.0.0.1:0")?.send_to(b"junk", target)?;
        for _ in 0..50 {
            odin.pass()?;
            thread::sleep(IDLE_POLL_INTERVAL);
        }
        assert!(odin.server.packets_dropped() >= 1);

        odin.serve_while(move || lingering_peer(target, PEER_CONNECTION_BASE + 1, None))??;
        assert_eq!(odin.server.session_count(), 1, "Odin still admits peers");
        Ok(())
    }

    /// A provider put, as the server hands it to the sink.
    fn provider_put(
        odin: &OdinWorld,
        schema: &str,
        key: &str,
        payload: Vec<u8>,
    ) -> Result<()> {
        odin.state
            .borrow_mut()
            .accept_raw_document(CultMeshRudpRawDocumentReceipt {
                session: cultmesh_rs::CultMeshRudpSessionKey {
                    remote_addr: "127.0.0.1:1".parse()?,
                    connection_id: 7,
                },
                message_id: "put".into(),
                transport_sequence: 1,
                received_at_unix_millis: unix_millis()?,
                document: CultNetRawDocumentRecord {
                    schema_id: schema.into(),
                    record_key: key.into(),
                    stored_at: rfc3339_millis(unix_millis()?)?,
                    payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                    payload,
                    source_runtime_id: None,
                    source_agent_id: None,
                    source_role: None,
                    tags: None,
                },
            })
    }

    /// Soul's probe P-B: a peer that puts a document of Odin's own kind (the
    /// publisher watermark, keyed by Odin's target, holding the largest
    /// sequence) would freeze every later heartbeat and survive restarts. Odin
    /// accepts only the namespaces providers publish in, so every record Odin
    /// or Idunn owns is refused, and presence keeps advancing.
    #[test]
    fn a_peer_cannot_write_a_record_odin_owns_and_presence_keeps_advancing() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let before = odin.stored_sequence()?;
        let stored_before = odin.working_set();

        // Every kind Odin itself stored (its watermark and correlations, found
        // rather than named), and the kinds Idunn projects to it.
        let mut owned: BTreeSet<(String, String)> = stored_before
            .iter()
            .filter(|entry| entry.r#type != GameCultRuntimePresenceHealthRecord::TYPE)
            .map(|entry| (entry.r#type.clone(), entry.key.clone()))
            .collect();
        for kind in [
            IdunnExpectedIncarnationRecord::TYPE,
            IdunnRuntimeActivationRecord::TYPE,
            IdunnProcessWriteLeaseRecord::TYPE,
            GameCultServiceTrustAnchorRecord::TYPE,
            "odin.topology_publisher_watermark",
            "odin.interface_layout",
        ] {
            owned.insert((kind.into(), TARGET.into()));
        }
        assert!(
            owned
                .iter()
                .any(|(kind, _)| kind == OdinRuntimeTopologyCorrelationRecord::TYPE),
            "the store holds Odin's own records: {owned:?}"
        );
        for (kind, key) in &owned {
            let refused = provider_put(
                &odin,
                &format!("{kind}.v1"),
                key,
                rmp_serde::to_vec(&(format!("{kind}.v1"), TARGET, u64::MAX))?,
            )
            .expect_err(&format!("{kind} must be refused"));
            assert!(format!("{refused:#}").contains("accepts no"), "{refused:#}");
        }
        assert_eq!(
            odin.working_set(),
            stored_before,
            "no refused put touched the store"
        );

        odin.timers = ServingTimers::default();
        odin.pass()?;
        assert_eq!(
            odin.stored_sequence()?,
            before + 1,
            "Odin's own presence still advances"
        );
        Ok(())
    }

    /// The same rule read the other way: the catalog serves a provider's
    /// document and Odin's presence, and nothing else Odin holds.
    #[test]
    fn the_catalog_serves_provider_documents_and_presence_only() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1])?;
        let stored: BTreeSet<String> = odin
            .working_set()
            .into_iter()
            .map(|entry| entry.r#type)
            .collect();
        assert!(
            stored.len() >= 4,
            "Odin's store holds more than what it serves: {stored:?}"
        );

        let served: BTreeSet<String> = odin
            .catalog()?
            .into_iter()
            .map(|document| document.schema_id)
            .collect();
        assert_eq!(
            served,
            BTreeSet::from([
                GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.to_owned(),
                "ghostlight.doc.v1".to_owned()
            ])
        );
        Ok(())
    }

    #[test]
    fn provider_namespaces_match_whole_segments() {
        for accepted in ["ghostlight.doc", "heimdall.command_boundary", "gamecult.eve.command", "gamecult.media_stream_request"] {
            assert!(is_peer_document_type(accepted), "{accepted}");
        }
        for refused in [
            "ghostlightx.doc",
            "ghostlight",
            "gamecult.eve",
            "gamecult.runtime_presence_health",
            "gamecult.service_trust_anchor",
            "odin.topology_publisher_watermark",
            "idunn.process_write_lease",
        ] {
            assert!(!is_peer_document_type(refused), "{refused}");
        }
    }

    /// One real schema id per namespace, each taken from its publisher (see
    /// `PEER_DOCUMENT_NAMESPACES`): Odin accepts the put and the catalog serves
    /// it. Soul's probe P1-A is the media pair, which Muninn and Ratatoskr
    /// exchange through Odin.
    #[test]
    fn every_publisher_schema_is_accepted_and_served() -> Result<()> {
        const PUBLISHED: &[&str] = &[
            "ghostlight.schema_catalog.v1",
            "heimdall.command_boundary.v1",
            "muninn.obs_stream_catalog.v1",
            "sleipnir.input_mapping.v1",
            "gamecult.eve.provider_advertisement.v1",
            "gamecult.eve.surface.v1",
            "gamecult.eve.surface_state.v1",
            "gamecult.eve.plugin_advertisement.v1",
            "gamecult.model.atlas_publication.v0",
            "gamecult.aetheria.asset_manifest.v1",
            "gamecult.media_stream_advertisement.v1",
            "gamecult.media_stream_request.v1",
        ];
        let odin = activated_odin()?;
        for schema in PUBLISHED {
            provider_put(&odin, schema, "key", vec![1])?;
        }
        let served: BTreeSet<String> = odin
            .catalog()?
            .into_iter()
            .map(|document| document.schema_id)
            .collect();
        for schema in PUBLISHED {
            assert!(served.contains(*schema), "{schema} is not served");
        }
        Ok(())
    }

    /// Namespaces no source publishes to Odin are refused, and a lookalike of
    /// the media types is not the media types.
    #[test]
    fn unpublished_namespaces_are_refused() -> Result<()> {
        let odin = activated_odin()?;
        for schema in [
            "mimir.doc.v1",
            "streampixels.doc.v1",
            "vili.doc.v1",
            "gamecult.loki.doc.v1",
            "gamecult.media.doc.v1",
            "gamecult.media_stream_advertisement_x.v1",
        ] {
            assert!(
                provider_put(&odin, schema, "key", vec![1]).is_err(),
                "{schema}"
            );
        }
        Ok(())
    }

    #[test]
    fn only_the_write_lease_is_fatal_in_the_serving_loop() -> Result<()> {
        let odin = activated_odin()?;
        let state = odin.state.borrow();
        assert_eq!(survive(&state, "x", Ok(3))?, Some(3));
        assert_eq!(
            survive::<()>(&state, "x", Err(anyhow::anyhow!("io")))?,
            None
        );
        let lost = anyhow::Error::new(WriteLeaseLost("gone".into())).context("refreshing");
        assert!(survive::<()>(&state, "x", Err(lost)).is_err());
        let foreign = anyhow::Error::new(ForeignStoreWrite).context("writing");
        assert!(survive::<()>(&state, "x", Err(foreign)).is_err());
        Ok(())
    }

    /// A failure of the projection file read (permission denied, not found,
    /// interrupted) says nothing about what Idunn published: it is retried. A
    /// record the reader could not decode or validate is Odin's own authority
    /// failing, and stays fatal even when the decoder wraps an I/O error.
    #[test]
    fn a_projection_read_failure_is_retried_a_decode_failure_is_not() -> Result<()> {
        let odin = activated_odin()?;
        let state = odin.state.borrow();
        for kind in [
            std::io::ErrorKind::PermissionDenied,
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::Interrupted,
        ] {
            let read = anyhow::Error::new(std::io::Error::from(kind))
                .context("failed to read")
                .context(ProjectionUnreadable);
            assert_eq!(
                survive::<()>(&state, "projection", Err(own_projection_failure(read)))?,
                None,
                "{kind:?}"
            );
        }
        let undecodable = anyhow::anyhow!("failed to decode MessagePack: invalid marker");
        assert_refused(
            &survive::<()>(&state, "projection", Err(own_projection_failure(undecodable)))
                .unwrap_err(),
        );
        // A decoder that wraps an I/O error (MessagePack does, for a record
        // that ends early) is still a decode failure.
        let wrapped = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
            .context("failed to decode Expected");
        assert_refused(
            &survive::<()>(&state, "projection", Err(own_projection_failure(wrapped)))
                .unwrap_err(),
        );
        Ok(())
    }

    /// Odin's own Expected record cut short by one byte is undecodable, so
    /// Odin ends, though the decoder's error wraps an unexpected end of file.
    #[test]
    fn a_truncated_own_expected_record_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.tamper_projection(|mut entries| {
            for entry in &mut entries {
                if entry.r#type == IdunnExpectedIncarnationRecord::TYPE {
                    entry.payload.pop();
                }
            }
            entries
        })?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    /// A projection store cut short on disk is a decode failure of the file's
    /// content, not a read failure of the file.
    #[test]
    fn a_truncated_projection_store_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let projection = odin.state.borrow().options.idunn_projection.clone();
        let mut bytes = std::fs::read(&projection)?;
        bytes.pop();
        std::fs::write(&projection, bytes)?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    /// The same rule through the serving loop: with Idunn's projection file
    /// gone Odin keeps serving and publishes again once it is back; with a
    /// projection that does not decode it ends.
    #[test]
    fn a_missing_projection_file_is_retried_by_the_serving_loop() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let before = odin.stored_sequence()?;
        let projection = odin.state.borrow().options.idunn_projection.clone();
        let bytes = std::fs::read(&projection)?;

        std::fs::remove_file(&projection)?;
        for _ in 0..3 {
            odin.timers = ServingTimers::default();
            odin.pass()?;
        }
        assert_eq!(odin.stored_sequence()?, before, "nothing is published blind");

        std::fs::write(&projection, bytes)?;
        odin.timers = ServingTimers::default();
        odin.pass()?;
        assert!(
            odin.stored_sequence()? > before,
            "publication resumes once the projection is readable"
        );

        std::fs::write(&projection, b"not a cultcache store")?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
    }

    #[test]
    fn a_failing_socket_is_retried_and_ends_odin_only_after_the_failure_limit() {
        let start = Instant::now();
        let mut since = None;
        let fault = || Err(anyhow::anyhow!("socket fault"));
        assert!(!settle_poll(fault(), &mut since, start).unwrap());
        assert_eq!(since, Some(start));
        assert!(
            !settle_poll(
                fault(),
                &mut since,
                start + POLL_FAILURE_LIMIT - Duration::from_millis(1)
            )
            .unwrap()
        );
        // A successful poll ends the run, so the same fault starts a new one.
        assert!(settle_poll(Ok(CultMeshRudpPollOutcome::Handled), &mut since, start).unwrap());
        assert_eq!(since, None);
        let restart = start + POLL_FAILURE_LIMIT;
        assert!(!settle_poll(fault(), &mut since, restart).unwrap());
        assert!(settle_poll(fault(), &mut since, restart + POLL_FAILURE_LIMIT).is_err());
    }

    // ---- one write per interval ----------------------------------------------

    /// The store file as a reader sees it: which file, and what it holds. Each
    /// write replaces the file atomically, so it is a new file.
    fn file_identity(path: &Path) -> Result<(u64, Vec<u8>)> {
        use std::os::unix::fs::MetadataExt;
        Ok((std::fs::metadata(path)?.ino(), std::fs::read(path)?))
    }

    fn ago(millis: u64) -> Option<Instant> {
        Instant::now().checked_sub(Duration::from_millis(millis))
    }

    /// Twenty-five of Odin's own changes (heartbeats) inside one interval are
    /// one write, made once the interval has passed: not at 0.9 s after the
    /// last write, at 1.1 s.
    #[test]
    fn changes_inside_one_interval_are_one_write() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let first = file_identity(&odin.store)?;
        for _ in 0..25 {
            odin.timers.last_heartbeat = None;
            odin.timers.last_flush = Some(Instant::now());
            odin.pass()?;
        }
        assert!(odin.stored_sequence()? >= 26, "every heartbeat was admitted");
        assert_eq!(file_identity(&odin.store)?, first, "no write inside the interval");

        odin.timers.last_flush = ago(900);
        odin.pass()?;
        assert_eq!(file_identity(&odin.store)?, first, "0.9 s is inside the interval");

        odin.timers.last_flush = ago(1_100);
        odin.pass()?;
        let second = file_identity(&odin.store)?;
        assert_ne!(second.0, first.0, "one write once the interval passed");
        assert_eq!(odin.file_records()?, odin.working_set());

        // The write itself starts the next interval: a change right after it
        // waits.
        odin.heartbeat()?;
        odin.pass()?;
        assert_eq!(file_identity(&odin.store)?, second, "and only one");
        Ok(())
    }

    /// A peer's put is on disk when Odin accepts it, inside the interval, and
    /// the write carries Odin's own unwritten changes with it.
    #[test]
    fn a_put_is_written_before_it_is_accepted() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.heartbeat()?;
        odin.timers.last_flush = Some(Instant::now());
        provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1])?;
        assert!(
            odin.file_records()?
                .iter()
                .any(|entry| entry.r#type == "ghostlight.doc" && entry.key == "doc-1")
        );
        assert_eq!(odin.file_records()?, odin.working_set());
        Ok(())
    }

    /// A put whose write is not made is refused, so it is never acknowledged:
    /// with the write failing, and with the lease lost.
    #[test]
    fn a_put_whose_write_is_not_made_is_refused() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        std::fs::write(&odin.store, b"not a cultcache store")?;
        assert!(provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1]).is_err());

        let mut odin = activated_odin()?;
        odin.pass()?;
        let written = file_identity(&odin.store)?;
        odin.swap_lease()?;
        let error = provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1]).unwrap_err();
        assert!(error.downcast_ref::<WriteLeaseLost>().is_some(), "{error:#}");
        assert_eq!(file_identity(&odin.store)?, written);
        Ok(())
    }

    /// A correlation for an incarnation Idunn no longer projects is found
    /// through the correlations Odin holds, and the refresh pass withdraws it.
    #[test]
    fn a_correlation_whose_incarnation_is_no_longer_projected_is_withdrawn() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let candidate = project_candidate_incarnation(&odin)?;
        odin.state.borrow().refresh_all_correlations()?;
        let correlations = || odin.stored_keys(OdinRuntimeTopologyCorrelationRecord::TYPE);
        assert!(correlations()?.contains(&candidate.key()));

        odin.tamper_projection(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.key != candidate.key())
                .collect()
        })?;
        odin.state.borrow().refresh_all_correlations()?;
        assert!(!correlations()?.contains(&candidate.key()));
        assert_eq!(correlations()?.len(), 1, "Odin's own stays");
        Ok(())
    }

    /// An interval in which nothing changed writes nothing, however often the
    /// write is due. With nothing to write the write step reads nothing
    /// either, not even the lease: it is due on every pass until something
    /// changes.
    #[test]
    fn nothing_changed_is_no_write() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let written = file_identity(&odin.store)?;
        for _ in 0..5 {
            odin.timers.last_projection_refresh = None;
            odin.timers.last_heartbeat = Some(Instant::now());
            odin.timers.last_flush = None;
            odin.pass()?;
        }
        assert_eq!(file_identity(&odin.store)?, written);

        std::fs::write(&odin.lease_path, b"not a cultcache store")?;
        odin.timers.last_projection_refresh = Some(Instant::now());
        odin.timers.last_heartbeat = Some(Instant::now());
        odin.timers.last_flush = None;
        odin.pass()?;
        assert_eq!(file_identity(&odin.store)?, written);
        Ok(())
    }

    /// A due write with nothing to write does not start the interval: the next
    /// change is written on the first pass after it.
    #[test]
    fn with_nothing_to_write_the_next_change_is_written_on_the_next_pass() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let written = file_identity(&odin.store)?;
        odin.timers.last_projection_refresh = Some(Instant::now());
        odin.timers.last_heartbeat = Some(Instant::now());
        odin.timers.last_flush = ago(1_100);
        odin.pass()?;
        assert_eq!(file_identity(&odin.store)?, written, "nothing to write");

        odin.heartbeat()?;
        odin.pass()?;
        assert_ne!(file_identity(&odin.store)?, written, "the change is written at once");
        assert_eq!(odin.file_records()?, odin.working_set());
        Ok(())
    }

    /// The lease is checked at the write itself: lost between a change and
    /// its write, the write is not made and Odin ends.
    #[test]
    fn a_lease_lost_before_the_write_writes_nothing_and_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        let written = file_identity(&odin.store)?;
        odin.heartbeat()?;
        odin.swap_lease()?;
        odin.timers.last_projection_refresh = Some(Instant::now());
        odin.timers.last_heartbeat = Some(Instant::now());
        odin.timers.last_flush = None;
        let error = odin.pass().unwrap_err();
        assert!(
            error.downcast_ref::<WriteLeaseLost>().is_some(),
            "{error:#}"
        );
        assert_eq!(file_identity(&odin.store)?, written);
        Ok(())
    }

    /// A file another process wrote between Odin's writes ends Odin, and is
    /// not written over.
    #[test]
    fn a_foreign_write_ends_odin_and_is_not_written_over() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.tamper_store(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.r#type != GameCultRuntimePresenceHealthRecord::TYPE)
                .collect()
        })?;
        let foreign = file_identity(&odin.store)?;
        odin.heartbeat()?;
        odin.timers.last_flush = None;
        let error = odin.pass().unwrap_err();
        assert!(
            error.downcast_ref::<ForeignStoreWrite>().is_some(),
            "{error:#}"
        );
        assert_eq!(file_identity(&odin.store)?, foreign);
        Ok(())
    }

    /// Odin serving, written once, with a change of its own not yet written.
    fn odin_with_an_unwritten_change() -> Result<OdinWorld> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        odin.heartbeat()?;
        assert_ne!(odin.file_records()?, odin.working_set());
        Ok(odin)
    }

    /// Stopping on request makes the last write.
    #[test]
    fn a_stop_request_writes_what_changed() -> Result<()> {
        let mut odin = odin_with_an_unwritten_change()?;
        odin.serve(true)?;
        assert_eq!(odin.file_records()?, odin.working_set());
        Ok(())
    }

    /// Ending because Odin's own authority is refused, the lease is still
    /// current: the last write is made.
    #[test]
    fn ending_on_a_refused_authority_writes_what_changed() -> Result<()> {
        let mut odin = odin_with_an_unwritten_change()?;
        odin.tamper_projection(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.r#type != IdunnRuntimeActivationRecord::TYPE)
                .collect()
        })?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.serve(false).unwrap_err());
        assert_eq!(odin.file_records()?, odin.working_set());
        Ok(())
    }

    /// Ending on a dead socket, the lease is still current: the last write is
    /// made. The socket's own failure is real (a refused datagram, reported on
    /// the server's next receive); only the length of the failing run is set.
    #[test]
    fn ending_on_a_dead_socket_writes_what_changed() -> Result<()> {
        let mut odin = odin_with_an_unwritten_change()?;
        let closed = UdpSocket::bind("127.0.0.1:0")?.local_addr()?;
        odin.socket.connect(closed)?;
        odin.socket.send(b"to nobody")?;
        thread::sleep(Duration::from_millis(50));
        odin.timers.poll_failing_since = ago(POLL_FAILURE_LIMIT.as_millis() as u64 + 1_000);
        let error = odin.serve(false).unwrap_err();
        assert!(format!("{error:#}").contains("failed every poll"), "{error:#}");
        assert_eq!(odin.file_records()?, odin.working_set());
        Ok(())
    }

    /// Ending because the lease was lost, nothing more is written.
    #[test]
    fn ending_on_a_lost_lease_writes_nothing() -> Result<()> {
        let mut odin = odin_with_an_unwritten_change()?;
        let written = file_identity(&odin.store)?;
        odin.swap_lease()?;
        odin.timers = ServingTimers::default();
        let error = odin.serve(false).unwrap_err();
        assert!(error.downcast_ref::<WriteLeaseLost>().is_some(), "{error:#}");
        assert_eq!(file_identity(&odin.store)?, written);
        Ok(())
    }

    /// Ending because another process wrote the file, nothing is written over
    /// it.
    #[test]
    fn ending_on_a_foreign_write_writes_nothing() -> Result<()> {
        let mut odin = odin_with_an_unwritten_change()?;
        odin.tamper_store(|entries| {
            entries
                .into_iter()
                .filter(|entry| entry.r#type != GameCultRuntimePresenceHealthRecord::TYPE)
                .collect()
        })?;
        let foreign = file_identity(&odin.store)?;
        odin.timers.last_flush = None;
        let error = odin.serve(false).unwrap_err();
        assert!(error.downcast_ref::<ForeignStoreWrite>().is_some(), "{error:#}");
        assert_eq!(file_identity(&odin.store)?, foreign);
        Ok(())
    }

    // ---- the store file is not working memory -----------------------------

    /// Once Odin holds its lease, its working set is in memory. Rewriting the
    /// store file behind its back changes nothing Odin serves or refreshes;
    /// only a restart, which loads the file again, would see it.
    #[test]
    fn rewriting_the_store_file_behind_odin_changes_nothing_it_serves() -> Result<()> {
        let mut odin = activated_odin()?;
        provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1])?;
        odin.pass()?;
        let served = odin.catalog()?;
        let working = odin.working_set();
        assert_eq!(documents_of(&served, "ghostlight.doc.v1"), 1);

        odin.tamper_store(|entries| {
            let mut entries: Vec<_> = entries
                .into_iter()
                .filter(|entry| entry.r#type == "odin.topology_publisher_watermark")
                .collect();
            let mut foreign = generic_document().unwrap();
            foreign.key = "foreign".into();
            entries.push(foreign);
            entries
        })?;
        for _ in 0..3 {
            odin.state.borrow().refresh_all_correlations()?;
        }
        assert_eq!(odin.catalog()?, served);
        assert_eq!(odin.working_set(), working);
        assert_ne!(
            MemoryOdinTopologyStore::load(&odin.store)?.records().len(),
            working.len(),
            "a restart would load what the file now holds"
        );
        Ok(())
    }

    /// Refresh passes and catalog reads never read the store file. With the
    /// file replaced by bytes that do not decode, a reader would fail at once;
    /// every pass succeeds and logs nothing.
    #[test]
    fn refresh_passes_and_catalog_reads_never_read_the_store_file() -> Result<()> {
        let mut odin = activated_odin()?;
        provider_put(&odin, "ghostlight.doc.v1", "doc-1", vec![1])?;
        odin.pass()?;
        let served = odin.catalog()?;
        std::fs::write(&odin.store, b"not a cultcache store")?;
        for _ in 0..5 {
            odin.state.borrow().refresh_all_correlations()?;
            assert_eq!(odin.catalog()?, served);
        }
        assert!(
            odin.state.borrow().log_gate.borrow().seen.is_empty(),
            "a pass failed and was logged"
        );
        Ok(())
    }
}

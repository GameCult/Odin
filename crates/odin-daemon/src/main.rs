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
use cultcache_rs::{
    CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore,
};
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
    IdunnRuntimeActivationSigner, IdunnServiceIdentity,
    OdinRuntimeTopologyCorrelationRecord, OdinTopologyIdentity, ServiceIdentityProfile,
    ServiceIdentitySigner, ServiceIdentityTrustAnchor, derive_service_identity_id,
    open_service_identity_credential_reader, verify_runtime_authority,
};
use fs2::FileExt;
use odin_daemon::{
    AuthenticationPolicy, CultCacheIdunnProjectionSource, CultCacheOdinTopologyStore,
    IdunnProjectionSource, IncarnationRef, OdinTopologyAuthority, PresenceAuthorityRefused,
    SystemClock,
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
const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(2);
const MAX_RECENT_WARMING_PROOFS: usize = 64;
const WARMING_PROOF_LIFETIME_MILLIS: u64 = 60_000;
const CAS_ATTEMPTS: usize = 8;

type TopologyAuthority = OdinTopologyAuthority<
    CultCacheIdunnProjectionSource,
    CultCacheOdinTopologyStore,
    ServiceIdentitySigner<OdinTopologyIdentity>,
    SystemClock,
>;

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

struct RuntimeState {
    options: Options,
    candidate: SocketAddr,
    authority_material: RuntimeAuthority,
    idunn_anchor: Option<ServiceIdentityTrustAnchor>,
    topology_signer: Option<ServiceIdentitySigner<OdinTopologyIdentity>>,
    topology: Option<TopologyAuthority>,
    write_lease: Option<ProcessWriteLeaseGuard>,
    write_lease_path: PathBuf,
    recent_warming_proofs: VecDeque<(String, u64)>,
    publisher_sequence: u64,
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
        let projection_source = CultCacheIdunnProjectionSource::new(&options.idunn_projection);
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
        let store_sequence = prior_self_publisher_sequence(
            &options.store,
            authority_material
                .provider_signer
                .entry()
                .identity_id
                .as_str(),
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
            publisher_sequence: store_sequence,
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
            .projection(&self_incarnation(&self.authority_material))?
        else {
            return Ok(false);
        };
        if projected.current_lease.as_ref() != Some(&lease.record) {
            return Ok(false);
        }
        let signer = self
            .topology_signer
            .take()
            .context("Odin topology signer was already consumed")?;
        let idunn_anchor = self
            .idunn_anchor
            .take()
            .context("Idunn trust anchor was already consumed")?;
        self.write_lease = Some(lease);
        self.topology = Some(OdinTopologyAuthority::new(
            CultCacheIdunnProjectionSource::new(&self.options.idunn_projection),
            CultCacheOdinTopologyStore::new(&self.options.store),
            signer,
            SystemClock,
            idunn_anchor,
            AuthenticationPolicy::default(),
        ));
        Ok(true)
    }

    fn accept_raw_document(&mut self, receipt: CultMeshRudpRawDocumentReceipt) -> Result<()> {
        ensure!(
            self.activated(),
            "Odin does not admit or persist provider documents before its process-write lease"
        );
        validate_raw_document_shape(&receipt.document)?;
        if receipt.document.schema_id == GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA {
            return self
                .admit_presence_document(&receipt.document, receipt.received_at_unix_millis);
        }
        persist_generic_document(&self.options.store, &receipt.document)
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
        self.topology
            .as_ref()
            .context("Odin topology authority is absent")?
            .admit_presence(&presence.target, &document.payload, received_at_unix_millis)?;
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
        let document = self.signed_presence_document("active", detail)?;
        self.admit_presence_document(&document, unix_millis()?)
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

    fn refresh_all_correlations(&mut self) -> Result<()> {
        self.require_current_write_lease()?;
        let topology = self
            .topology
            .as_ref()
            .context("Odin topology authority is absent")?;
        let mut incarnations = incarnation_keys(
            &self.options.idunn_projection,
            IdunnExpectedIncarnationRecord::TYPE,
        )?;
        incarnations.extend(incarnation_keys(
            &self.options.store,
            OdinRuntimeTopologyCorrelationRecord::TYPE,
        )?);
        // Each incarnation is refreshed on its own: one whose records cannot be
        // read is that incarnation's failure, and never stops the others.
        for incarnation in incarnations {
            if let Err(error) = topology.refresh(&incarnation) {
                eprintln!(
                    "Odin could not refresh incarnation {}; the rest are unaffected: {error:#}",
                    incarnation.key()
                );
            }
        }
        Ok(())
    }

    fn stored_snapshot(
        &self,
        query: &CultMeshRudpSnapshotQuery,
    ) -> Result<Vec<CultNetRawDocumentRecord>> {
        let entries = if self.options.store.is_file() {
            SingleFileMessagePackBackingStore::new(&self.options.store)
                .pull_all_read_only_snapshot()?
        } else {
            Vec::new()
        };
        let projections = CultCacheIdunnProjectionSource::new(&self.options.idunn_projection);
        let mut selected = BTreeMap::new();
        for envelope in entries {
            let Some(schema_id) = envelope.schema_id.clone() else {
                continue;
            };
            let document = if envelope.r#type == GameCultRuntimePresenceHealthRecord::TYPE {
                let presence = decode_presence(&envelope.payload)?;
                let Some(projection) = projections.projection(&IncarnationRef::new(
                    presence.target.clone(),
                    presence.expected_projection_sha256.clone(),
                ))?
                else {
                    continue;
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
                    continue;
                }
                CultNetRawDocumentRecord {
                    schema_id,
                    record_key: presence.target.clone(),
                    stored_at: envelope.stored_at,
                    payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                    payload: envelope.payload,
                    source_runtime_id: Some(presence.runtime_id),
                    source_agent_id: Some(presence.signer_identity_id),
                    source_role: Some("runtime-presence-health-publisher".into()),
                    tags: Some(vec!["odin-observed".into()]),
                }
            } else if envelope.r#type == OdinRuntimeTopologyCorrelationRecord::TYPE {
                let (correlation, _) =
                    OdinRuntimeTopologyCorrelationRecord::decode_canonical_signed_payload(
                        &envelope.payload,
                    )?;
                CultNetRawDocumentRecord {
                    schema_id,
                    record_key: correlation.target,
                    stored_at: envelope.stored_at,
                    payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                    payload: envelope.payload,
                    source_runtime_id: Some(self.authority_material.expected.runtime_id.clone()),
                    source_agent_id: Some(correlation.signer_identity_id),
                    source_role: Some("odin-topology-correlation".into()),
                    tags: Some(vec!["odin-owned".into()]),
                }
            } else {
                CultNetRawDocumentRecord {
                    schema_id,
                    record_key: envelope.key,
                    stored_at: envelope.stored_at,
                    payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                    payload: envelope.payload,
                    source_runtime_id: None,
                    source_agent_id: None,
                    source_role: None,
                    tags: None,
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

    // Correlations the previous, target-keyed Odin left in this store are not
    // this contract's and would otherwise be served to the Verse as current.
    // Presence history is kept: the self publisher sequence continues from it.
    CultCacheOdinTopologyStore::new(&state.borrow().options.store).retire_legacy_correlations()?;
    // Once serving, only losing the write lease ends Odin (see `survive`).
    let mut timers = ServingTimers::default();
    loop {
        if stopping.load(Ordering::Relaxed) {
            eprintln!("Odin stopping on request");
            return Ok(());
        }
        if !serving_pass(&state, &mut server, &mut timers)? {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }
}

/// A timer that has never fired is due, so the first pass refreshes and
/// publishes.
#[derive(Default)]
struct ServingTimers {
    last_heartbeat: Option<Instant>,
    last_projection_refresh: Option<Instant>,
    poll_failing_since: Option<Instant>,
}

type OdinServer = CultMeshRudpDocumentServer<SinkHandle, SnapshotHandle, CultMeshSystemClock>;

/// One turn of the serving loop: serve a datagram, refresh the topology, and
/// publish Odin's own presence when each is due. Returns whether the poll made
/// progress. It fails only for `WriteLeaseLost` or a dead socket.
fn serving_pass(
    state: &Rc<RefCell<RuntimeState>>,
    server: &mut OdinServer,
    timers: &mut ServingTimers,
) -> Result<bool> {
    let progressed = poll_server(server, &mut timers.poll_failing_since)?;
    if is_due(timers.last_projection_refresh, PROJECTION_REFRESH_INTERVAL) {
        survive(
            "projection refresh",
            state.borrow_mut().refresh_all_correlations(),
        )?;
        timers.last_projection_refresh = Some(Instant::now());
    }
    if is_due(timers.last_heartbeat, HEARTBEAT_INTERVAL) {
        survive(
            "self-presence publication",
            state.borrow_mut().publish_self_presence("ready"),
        )?;
        timers.last_heartbeat = Some(Instant::now());
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
/// allowed to end the daemon. Only `WriteLeaseLost`, wherever in the error chain
/// it sits, is returned -- and, as a named temporary rule, so is
/// `PresenceAuthorityRefused`: a self-presence that Odin's own authority will
/// never admit (no verifiable authority, a signer that does not match the
/// anchor, a stored presence of this activation that no longer authenticates)
/// leaves a frozen presence that goes stale, and a stale Odin presence makes
/// `dependency_evidence` flip every dependent to not-Ready. Ending Odin lets
/// Idunn replace it. Deleted when Idunn proves Odin by its own route challenge
/// (Idunn audit cut A3, operator question Q-O5).
fn survive<T>(what: &str, result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error.downcast_ref::<WriteLeaseLost>().is_some()
                || error.downcast_ref::<PresenceAuthorityRefused>().is_some() =>
        {
            Err(error)
        }
        Err(error) => {
            eprintln!("Odin {what} failed; retrying on the next pass: {error:#}");
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

fn prior_self_publisher_sequence(path: &Path, signer_identity_id: &str) -> Result<u64> {
    if !path.is_file() {
        return Ok(0);
    }
    SingleFileMessagePackBackingStore::new(path)
        .pull_all_read_only_snapshot()?
        .into_iter()
        .filter(|entry| entry.r#type == GameCultRuntimePresenceHealthRecord::TYPE)
        .try_fold(0, |maximum, entry| {
            let presence = decode_presence(&entry.payload)?;
            Ok(
                if presence.target == TARGET && presence.signer_identity_id == signer_identity_id {
                    maximum.max(presence.publisher_sequence)
                } else {
                    maximum
                },
            )
        })
}

fn persist_generic_document(path: &Path, document: &CultNetRawDocumentRecord) -> Result<()> {
    let document_type = document_type_for_schema(&document.schema_id)?;
    ensure!(
        !matches!(
            document_type.as_str(),
            "gamecult.runtime_presence_health"
                | "odin.runtime_topology_correlation"
                | "idunn.expected_incarnation"
                | "idunn.runtime_activation"
                | "idunn.process_write_lease"
                | "gamecult.service_trust_anchor"
        ),
        "generic provider traffic cannot write an authority-owned Odin/Idunn document"
    );
    let replacement = CultCacheEnvelope {
        key: document.record_key.clone(),
        r#type: document_type.clone(),
        payload: document.payload.clone(),
        stored_at: document.stored_at.clone(),
        schema_id: Some(document.schema_id.clone()),
    };
    let store = SingleFileMessagePackBackingStore::new(path);
    for _ in 0..CAS_ATTEMPTS {
        let entries = if path.is_file() {
            store.pull_all_read_only_snapshot()?
        } else {
            Vec::new()
        };
        let mut matches = entries
            .iter()
            .filter(|entry| entry.r#type == document_type && entry.key == document.record_key);
        let current = matches.next().cloned();
        ensure!(
            matches.next().is_none(),
            "generic CultCache identity is ambiguous"
        );
        if current.as_ref() == Some(&replacement) {
            return Ok(());
        }
        if store.compare_exchange(
            &[CultCacheExpectedEnvelope {
                key: document.record_key.clone(),
                r#type: document_type.clone(),
                current,
            }],
            &[replacement.clone()],
        )? {
            return Ok(());
        }
    }
    bail!("Odin catalog changed repeatedly while persisting a provider document")
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

/// Every incarnation Idunn currently projects, of every target, and every
/// incarnation Odin holds a correlation for, projected or not (the latter are
/// refreshed so their correlations are withdrawn). Only the keys are read:
/// decoding a record is the refresh of that one incarnation, so a record that
/// will not decode cannot make the list unreadable.
///
/// Records keyed by anything but an incarnation key are not this contract's and
/// are skipped, not refused: a projection written by an older Idunn projects
/// nothing this daemon acts on, and a correlation written by the previous,
/// target-keyed Odin is retired at activation (`retire_legacy_correlations`).
fn incarnation_keys(path: &Path, record_type: &str) -> Result<BTreeSet<IncarnationRef>> {
    if !path.is_file() {
        return Ok(BTreeSet::new());
    }
    Ok(SingleFileMessagePackBackingStore::new(path)
        .pull_all_read_only_snapshot()?
        .into_iter()
        .filter(|entry| entry.r#type == record_type)
        .filter_map(|entry| IncarnationRef::parse_key(&entry.key))
        .collect())
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
        encode_cultnet_message_to_vec, enroll_service_identity_at,
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
        };

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
        let server = CultMeshRudpDocumentServer::new(
            socket,
            SinkHandle(state.clone()),
            SnapshotHandle(state.clone()),
            CultMeshSystemClock::default(),
            CultMeshRudpDocumentServerOptions::default(),
        )?;
        Ok(OdinWorld {
            _temp: temp,
            state,
            server,
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

        fn stored_keys(&self, record_type: &str) -> Result<Vec<String>> {
            Ok(SingleFileMessagePackBackingStore::new(&self.store)
                .pull_all_read_only_snapshot()?
                .into_iter()
                .filter(|entry| entry.r#type == record_type)
                .map(|entry| entry.key)
                .collect())
        }

        /// Odin's own publisher sequence as it is durably stored.
        fn stored_sequence(&self) -> Result<u64> {
            prior_self_publisher_sequence(&self.store, &self.provider_identity_id)
        }

        /// Run a peer on its own thread while Odin serves. Every pass Odin
        /// takes meanwhile must succeed.
        fn serve_while<T: Send + 'static>(
            &mut self,
            peer: impl FnOnce() -> T + Send + 'static,
        ) -> Result<T> {
            let peer = thread::spawn(peer);
            while !peer.is_finished() {
                if !self.pass()? {
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
        let deadline = Instant::now() + Duration::from_millis(500);
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
        let mut odin = activated_odin()?;
        let target = odin.server.local_addr()?;
        let admitted = odin.serve_while(move || {
            (0..70)
                .filter(|index| lingering_peer(target, PEER_CONNECTION_BASE + index, None).is_ok())
                .count()
        })?;
        assert_eq!(admitted, 64, "the default session table is 64 wide");
        assert_eq!(odin.server.session_count(), 64, "the table is full");

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

    /// A publication or refresh that fails is logged and retried; it never
    /// ends Odin, and the next attempt lands once the fault clears.
    #[test]
    fn a_failed_self_publication_or_refresh_does_not_end_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        assert!(odin.stored_sequence()? > 0, "the first pass publishes");

        std::fs::write(&odin.store, b"not a cultcache store")?;
        assert!(
            odin.state
                .borrow_mut()
                .publish_self_presence("probe")
                .is_err(),
            "the injected fault must actually fail the publication"
        );
        odin.timers = ServingTimers::default();
        odin.pass()?;

        std::fs::remove_file(&odin.store)?;
        odin.timers = ServingTimers::default();
        odin.pass()?;
        assert!(
            odin.stored_sequence()? > 0,
            "the publication after the fault clears lands"
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

        odin.pass()?;
        assert!(
            odin.stored_keys(OdinRuntimeTopologyCorrelationRecord::TYPE)?
                .contains(&candidate_key),
            "the good incarnation was refreshed past the ghost"
        );

        let catalog = odin.state.borrow_mut().raw_snapshot(&CultMeshRudpSnapshotQuery {
            session: cultmesh_rs::CultMeshRudpSessionKey {
                remote_addr: "127.0.0.1:1".parse()?,
                connection_id: 7,
            },
            message_id: "catalog".into(),
            requested_at_unix_millis: 1,
            schema_ids: None,
            record_keys: None,
        })?;
        assert!(
            catalog
                .iter()
                .any(|document| document.schema_id == ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA),
            "the catalog serves its correlations"
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
    #[test]
    fn a_stored_presence_that_no_longer_authenticates_ends_odin() -> Result<()> {
        let mut odin = activated_odin()?;
        odin.pass()?;
        // Received two minutes after it was observed: outside the trusted window.
        let late = rfc3339_millis(unix_millis()? + 120_000)?;
        odin.tamper_store(|entries| {
            entries
                .into_iter()
                .map(|mut entry| {
                    if entry.r#type == GameCultRuntimePresenceHealthRecord::TYPE {
                        entry.stored_at = late.clone();
                    }
                    entry
                })
                .collect()
        })?;
        odin.timers = ServingTimers::default();
        assert_refused(&odin.pass().unwrap_err());
        Ok(())
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

    #[test]
    fn only_the_write_lease_is_fatal_in_the_serving_loop() {
        assert_eq!(survive("x", Ok(3)).unwrap(), Some(3));
        assert_eq!(
            survive::<()>("x", Err(anyhow::anyhow!("io"))).unwrap(),
            None
        );
        let lost = anyhow::Error::new(WriteLeaseLost("gone".into())).context("refreshing");
        assert!(survive::<()>("x", Err(lost)).is_err());
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
}

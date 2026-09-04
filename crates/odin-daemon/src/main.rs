use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::net::{SocketAddr, UdpSocket};
use std::os::fd::{FromRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::{DateTime, Utc};
use cultcache_rs::{
    CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore,
};
use cultmesh_rs::{
    CultMeshRudpDocumentPublishOptions, CultMeshRudpDocumentServer,
    CultMeshRudpDocumentServerOptions, CultMeshRudpPollOutcome, CultMeshRudpRawDocumentReceipt,
    CultMeshRudpRawDocumentSink, CultMeshRudpSnapshotQuery, CultMeshRudpSnapshotSource,
    CultMeshSystemClock, publish_cultnet_message_to_rudp_catalog,
};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding,
    GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA, GameCultProviderHealthIdentity,
    GameCultRuntimeCapability, GameCultRuntimePresenceHealthPurpose,
    GameCultRuntimePresenceHealthRecord, IDUNN_EXPECTED_INCARNATION_SCHEMA,
    IDUNN_PROCESS_WRITE_LEASE_SCHEMA, IDUNN_RUNTIME_ACTIVATION_CREDENTIAL_NAME,
    IDUNN_RUNTIME_ACTIVATION_SCHEMA, IdunnExpectedIncarnationRecord, IdunnProcessWriteLeaseRecord,
    IdunnRuntimeActivationRecord, IdunnRuntimeActivationSigner, IdunnServiceIdentity,
    ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA, OdinRuntimeTopologyCorrelationRecord,
    OdinTopologyIdentity, ServiceIdentityProfile, ServiceIdentitySigner,
    ServiceIdentityTrustAnchor, derive_service_identity_id,
    open_service_identity_credential_reader, verify_runtime_authority,
};
use fs2::FileExt;
use odin_daemon::{
    AuthenticationPolicy, CultCacheIdunnProjectionSource, CultCacheOdinTopologyStore,
    IdunnProjectionSource, OdinTopologyAuthority, SystemClock,
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

const BOOTSTRAP_LEASE_TIMEOUT: Duration = Duration::from_secs(60);
const SELF_PUBLISH_TIMEOUT: Duration = Duration::from_secs(6);
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
        let projection = projection_source
            .current_projection(TARGET)?
            .context("Idunn projection has no Expected Odin incarnation")?;
        ensure!(
            projection.expected == authority_material.expected
                && projection.activation.as_ref() == Some(&authority_material.activation),
            "Idunn live projection differs from Odin's immutable runtime bundle"
        );
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
            .current_projection(TARGET)?
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
            let presence = decode_presence(&receipt.document.payload)?;
            ensure!(
                receipt.document.record_key == presence.target,
                "runtime-presence document key differs from its signed target"
            );
            self.topology
                .as_ref()
                .context("Odin topology authority is absent")?
                .admit_presence(
                    &presence.target,
                    &receipt.document.payload,
                    receipt.received_at_unix_millis,
                )?;
            return Ok(());
        }
        persist_generic_document(&self.options.store, &receipt.document)
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
        self.refresh_all_correlations()?;
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

    fn active_document_put(&mut self, detail: &str) -> Result<CultNetMessage> {
        self.require_current_write_lease()?;
        let document = self.signed_presence_document("active", detail)?;
        let sequence = self.publisher_sequence;
        Ok(CultNetMessage::DocumentPutRaw {
            message_id: format!(
                "odin-presence:{}:{}:{sequence}",
                self.authority_material.expected.runtime_id,
                self.authority_material.activation.runtime_instance_id
            ),
            document,
        })
    }

    fn require_current_write_lease(&self) -> Result<()> {
        let held = self
            .write_lease
            .as_ref()
            .context("Odin has no process-write lease")?;
        let current = read_process_write_lease(&self.write_lease_path)?
            .context("Odin process-write lease was withdrawn")?;
        ensure!(
            current == held.record && current.canonical_sha256()? == held.sha256,
            "Odin process-write lease changed after admission"
        );
        Ok(())
    }

    fn refresh_all_correlations(&mut self) -> Result<()> {
        self.require_current_write_lease()?;
        let topology = self
            .topology
            .as_ref()
            .context("Odin topology authority is absent")?;
        let mut targets = projection_targets(&self.options.idunn_projection)?;
        targets.extend(correlation_targets(&self.options.store)?);
        for target in targets {
            topology.refresh(&target)?;
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
                let Some(projection) = projections.current_projection(&presence.target)? else {
                    continue;
                };
                if projection.expected.expected_signer_identity_id != presence.signer_identity_id
                    || projection.expected.canonical_sha256()?
                        != presence.expected_projection_sha256
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

    let bootstrap_started = Instant::now();
    while !state.borrow_mut().try_activate()? {
        let progressed = poll_server(&mut server)?;
        ensure!(
            bootstrap_started.elapsed() < BOOTSTRAP_LEASE_TIMEOUT,
            "timed out waiting for Idunn to admit Odin's exact Warming proof and grant its lease"
        );
        if !progressed {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }

    publish_self_presence(&state, &mut server, "ready")?;
    state.borrow_mut().refresh_all_correlations()?;
    let mut last_heartbeat = Instant::now();
    let mut last_projection_refresh = Instant::now();
    loop {
        let progressed = poll_server(&mut server)?;
        if last_projection_refresh.elapsed() >= PROJECTION_REFRESH_INTERVAL {
            state.borrow_mut().refresh_all_correlations()?;
            last_projection_refresh = Instant::now();
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            publish_self_presence(&state, &mut server, "ready")?;
            last_heartbeat = Instant::now();
        }
        if !progressed {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }
}

fn publish_self_presence(
    state: &Rc<RefCell<RuntimeState>>,
    server: &mut CultMeshRudpDocumentServer<SinkHandle, SnapshotHandle, CultMeshSystemClock>,
    detail: &str,
) -> Result<()> {
    let (message, options) = {
        let mut state = state.borrow_mut();
        let message = state.active_document_put(detail)?;
        let mut options = CultMeshRudpDocumentPublishOptions::odin(
            state.candidate,
            state.authority_material.expected.runtime_id.clone(),
        );
        options.connect_timeout = Duration::from_secs(2);
        options.flush_timeout = Duration::from_secs(2);
        options.source_agent_id = Some(
            state
                .authority_material
                .provider_signer
                .entry()
                .identity_id
                .clone(),
        );
        options.source_role = Some("runtime-presence-health-publisher".into());
        options.tags = vec!["cultnet.transport.rudp.v0".into()];
        (message, options)
    };
    let publisher =
        thread::spawn(move || publish_cultnet_message_to_rudp_catalog(&message, options));
    let started = Instant::now();
    while !publisher.is_finished() {
        let progressed = poll_server(server)?;
        ensure!(
            started.elapsed() < SELF_PUBLISH_TIMEOUT,
            "Odin self-presence publication did not complete"
        );
        if !progressed {
            thread::sleep(IDLE_POLL_INTERVAL);
        }
    }
    publisher
        .join()
        .map_err(|_| anyhow!("Odin self-presence publisher panicked"))??;
    Ok(())
}

fn poll_server(
    server: &mut CultMeshRudpDocumentServer<SinkHandle, SnapshotHandle, CultMeshSystemClock>,
) -> Result<bool> {
    match server.poll_once()? {
        CultMeshRudpPollOutcome::Idle => Ok(false),
        CultMeshRudpPollOutcome::Handled => Ok(true),
        CultMeshRudpPollOutcome::ApplicationRejected(rejection) => {
            eprintln!(
                "Odin rejected CultMesh RUDP application message {:?} {} from {:?}: {}",
                rejection.operation, rejection.message_id, rejection.session, rejection.reason
            );
            Ok(true)
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
            matches!(name, "store" | "idunn-projection" | "idunn-anchor"),
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
        store: take("store")?,
        idunn_projection: take("idunn-projection")?,
        idunn_anchor: take("idunn-anchor")?,
    };
    ensure!(
        options.store != options.idunn_projection
            && options.store != options.idunn_anchor
            && options.idunn_projection != options.idunn_anchor,
        "Odin store, Idunn projection, and Idunn anchor paths must be distinct"
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
    ensure!(
        pid == std::process::id().to_string(),
        "systemd signer descriptors belong to another process"
    );
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

fn projection_targets(path: &Path) -> Result<BTreeSet<String>> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    entries
        .into_iter()
        .filter(|entry| entry.r#type == IdunnExpectedIncarnationRecord::TYPE)
        .map(|entry| {
            ensure!(
                entry.schema_id.as_deref() == Some(IDUNN_EXPECTED_INCARNATION_SCHEMA),
                "Idunn projection contains an Expected record under the wrong schema"
            );
            let expected = IdunnExpectedIncarnationRecord::decode_canonical(&entry.payload)?;
            ensure!(
                entry.key == expected.target,
                "Idunn Expected key is substituted"
            );
            Ok(expected.target)
        })
        .collect()
}

fn correlation_targets(path: &Path) -> Result<BTreeSet<String>> {
    if !path.is_file() {
        return Ok(BTreeSet::new());
    }
    SingleFileMessagePackBackingStore::new(path)
        .pull_all_read_only_snapshot()?
        .into_iter()
        .filter(|entry| entry.r#type == OdinRuntimeTopologyCorrelationRecord::TYPE)
        .map(|entry| {
            ensure!(
                entry.schema_id.as_deref() == Some(ODIN_RUNTIME_TOPOLOGY_CORRELATION_SCHEMA),
                "Odin correlation is stored under the wrong schema"
            );
            let (record, _) =
                OdinRuntimeTopologyCorrelationRecord::decode_canonical_signed_payload(
                    &entry.payload,
                )?;
            ensure!(
                entry.key == record.target,
                "Odin correlation key is substituted"
            );
            Ok(record.target)
        })
        .collect()
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
    use super::*;

    #[test]
    fn options_are_exact_and_absolute() {
        let parsed = parse_options(
            [
                "--store",
                "/var/lib/gamecult/odin-v2/topology.cc",
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
        assert!(
            parse_options(
                [
                    "--store",
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
}

//! The real odin-daemon binary, run under an Idunn-shaped world: its bundle,
//! signer descriptors, projection and write lease on disk, and Idunn's warming
//! probe and lease grant made over RUDP as Idunn makes them. It reaches what
//! the in-process tests cannot: `main`'s signal wiring and a process that dies.
//! Unix only, like the daemon.

use std::fs::File;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail, ensure};
use cultcache_rs::{CacheBackingStore, CultCacheEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore};
use cultmesh_rs::{
    CultMeshRudpDocumentPublishOptions, CultMeshRudpSnapshotOptions,
    publish_cultnet_message_to_rudp_catalog, request_raw_snapshot_from_rudp_catalog,
};
use cultnet_rs::*;
use odin_daemon::IncarnationRef;

fn digest(byte: char) -> String {
    format!("sha256-{}", byte.to_string().repeat(64))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn rfc3339(millis: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis as i64)
        .unwrap()
        .to_rfc3339()
}

/// Replace the whole store at `path` with `entries`.
fn write_store(path: &Path, entries: &[CultCacheEnvelope]) -> Result<()> {
    let store = SingleFileMessagePackBackingStore::new(path);
    let current = if path.is_file() {
        store.pull_all_read_only_snapshot()?
    } else {
        Vec::new()
    };
    ensure!(store.compare_exchange_snapshot(&current, entries)?, "store write lost a race");
    Ok(())
}

fn credential_bytes(store: &Path) -> Result<Vec<u8>> {
    Ok(rmp_serde::to_vec(&SingleFileMessagePackBackingStore::new(store).pull_all()?)?)
}

struct Odin {
    _temp: tempfile::TempDir,
    root: PathBuf,
    candidate: SocketAddr,
    child: Child,
    store: PathBuf,
}

impl Odin {
    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("odin.log")).unwrap_or_default()
    }

    /// Put one provider document and return once the server acknowledged it.
    fn put(&self, key: &str) -> Result<()> {
        let message = CultNetMessage::DocumentPutRaw {
            message_id: format!("put-{key}"),
            document: CultNetRawDocumentRecord {
                schema_id: "ghostlight.doc.v1".into(),
                record_key: key.into(),
                stored_at: rfc3339(now()),
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: vec![1],
                source_runtime_id: None,
                source_agent_id: None,
                source_role: None,
                tags: None,
            },
        };
        let mut options = CultMeshRudpDocumentPublishOptions::odin(self.candidate, "test-publisher");
        options.flush_timeout = Duration::from_secs(3);
        publish_cultnet_message_to_rudp_catalog(&message, options)
    }

    fn holds(&self, key: &str) -> Result<bool> {
        Ok(SingleFileMessagePackBackingStore::new(&self.store)
            .pull_all_read_only_snapshot()?
            .iter()
            .any(|entry| entry.r#type == "ghostlight.doc" && entry.key == key))
    }

    fn signal(&mut self, signal: &str) -> Result<ExitStatus> {
        let pid = self.child.id().to_string();
        ensure!(Command::new("kill").args([signal, &pid]).status()?.success());
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            ensure!(Instant::now() < deadline, "Odin did not exit on {signal}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Odin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Launch Odin as Idunn does and grant it its lease; returns once Odin has
/// written its store, which it does on its first pass after activating.
fn start_odin() -> Result<Odin> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let candidate = UdpSocket::bind("127.0.0.1:0")?.local_addr()?;
    let t0 = now();
    let idunn = enroll_service_identity_at::<IdunnServiceIdentity>(&root.join("idunn.cc"))?;
    enroll_service_identity_at::<OdinTopologyIdentity>(&root.join("odin-topology.cc"))?;
    let provider =
        enroll_service_identity_at::<GameCultProviderHealthIdentity>(&root.join("odin-provider.cc"))?;
    std::fs::write(root.join("topology-cred"), credential_bytes(&root.join("odin-topology.cc"))?)?;
    std::fs::write(root.join("provider-cred"), credential_bytes(&root.join("odin-provider.cc"))?)?;
    export_service_identity_trust_anchor::<IdunnServiceIdentity>(&idunn, &root.join("idunn-anchor.cc"))?;
    let expected = IdunnExpectedIncarnationRecord {
        schema_version: IDUNN_EXPECTED_INCARNATION_SCHEMA.into(),
        target: "odin".into(),
        plan_id: digest('1'),
        incarnation_id: "odin/generation-1".into(),
        sealed_release_id: digest('2'),
        source_repository: "github.com/GameCult/Odin".into(),
        source_revision: "3".repeat(40),
        recipe_sha256: digest('4'),
        runtime_id: "odin-runtime".into(),
        expected_signer_identity_id: provider.entry().identity_id.clone(),
        health_contract: "odin.runtime-health.v1".into(),
        artifact_sha256: digest('5'),
        state_schema_generation: Some("odin-v2".into()),
        state_contract_sha256: Some(
            "sha256-4f2f2dcd931d16f6b02bf295f41227b867aa982765661d55fa9f29fb2db7e449".into(),
        ),
        write_lease_required: true,
        route: Some(IdunnExpectedRoute {
            route_id: "odin-route".into(),
            transport: "rudp".into(),
            stable_endpoint: "rudp://odin.internal:1000".into(),
            candidate_endpoint: format!("rudp://{candidate}"),
        }),
        capabilities: vec![IdunnExpectedCapability {
            capability: "odin.verse-rendezvous".into(),
            schema: "odin.verse-topology.v1".into(),
            compatibility: "v1".into(),
            minimum_capacity: 1,
        }],
        dependencies: Vec::new(),
    };
    expected.validate()?;
    let launch = IdunnRuntimeActivationLaunch::issue(&expected, digest('7'), t0 - 20, &idunn)?;
    let activation = launch.activation().clone();
    let mut activation_credential = Vec::new();
    launch.write_credential(&mut activation_credential)?;
    std::fs::write(root.join("activation-cred"), &activation_credential)?;

    let expected_envelope = |key: &str| -> Result<CultCacheEnvelope> {
        Ok(CultCacheEnvelope {
            key: key.into(),
            r#type: IdunnExpectedIncarnationRecord::TYPE.into(),
            payload: expected.canonical_bytes()?,
            stored_at: rfc3339(t0 - 30),
            schema_id: Some(IDUNN_EXPECTED_INCARNATION_SCHEMA.into()),
        })
    };
    let activation_envelope = |key: &str| -> Result<CultCacheEnvelope> {
        Ok(CultCacheEnvelope {
            key: key.into(),
            r#type: IdunnRuntimeActivationRecord::TYPE.into(),
            payload: activation.canonical_bytes()?,
            stored_at: rfc3339(activation.issued_at_unix_millis),
            schema_id: Some(IDUNN_RUNTIME_ACTIVATION_SCHEMA.into()),
        })
    };
    let bundle = root.join("bundle");
    std::fs::create_dir_all(&bundle)?;
    write_store(&bundle.join("expected.cc"), &[expected_envelope("odin")?])?;
    write_store(&bundle.join("activation.cc"), &[activation_envelope("odin")?])?;

    let anchor = GameCultServiceTrustAnchorRecord {
        schema_version: GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into(),
        trust_anchor_id: "root/odin/runtime-presence".into(),
        service_id: "odin".into(),
        runtime_id: expected.runtime_id.clone(),
        signer_identity_id: provider.entry().identity_id.clone(),
        signer_public_key: provider.entry().public_key.clone(),
        signature_algorithm: "ed25519".into(),
        signing_purpose: GAMECULT_RUNTIME_PRESENCE_HEALTH_SIGNING_PURPOSE.into(),
        signed_schema: GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into(),
        binding_authority: "root".into(),
        bound_at_unix_millis: t0 - 100,
        expires_at_unix_millis: None,
        private_state_exposed: false,
    };
    let incarnation_key = IncarnationRef::of(&expected)?.key();
    let projection = root.join("projection.cc");
    let mut projected = vec![
        expected_envelope(&incarnation_key)?,
        CultCacheEnvelope {
            key: anchor.trust_anchor_id.clone(),
            r#type: GameCultServiceTrustAnchorRecord::TYPE.into(),
            payload: rmp_serde::to_vec(&anchor)?,
            stored_at: rfc3339(anchor.bound_at_unix_millis),
            schema_id: Some(GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into()),
        },
        activation_envelope(&incarnation_key)?,
    ];
    write_store(&projection, &projected)?;
    let lease_path = root.join("lease.cc");
    File::create(root.join("lease.cc.lock"))?;
    let state_root = root.join("state");
    std::fs::create_dir_all(&state_root)?;

    // Descriptors 3 and 4 carry the signer credentials, as systemd passes them.
    let child = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "exec 3<{activation} 4<{provider}; exec {bin} --state-root {state} --idunn-projection {projection} --idunn-anchor {anchor}",
            activation = root.join("activation-cred").display(),
            provider = root.join("provider-cred").display(),
            bin = env!("CARGO_BIN_EXE_odin-daemon"),
            state = state_root.display(),
            projection = projection.display(),
            anchor = root.join("idunn-anchor.cc").display(),
        ))
        .env("GAMECULT_IDUNN_RUNTIME_BUNDLE", &bundle)
        .env("GAMECULT_IDUNN_CANDIDATE_BIND", candidate.to_string())
        .env("GAMECULT_IDUNN_PROCESS_WRITE_LEASE", &lease_path)
        .env("ODIN_TOPOLOGY_IDENTITY", root.join("topology-cred"))
        .env("LISTEN_PID", "1")
        .env("LISTEN_FDS", "2")
        .env(
            "LISTEN_FDNAMES",
            format!("{IDUNN_RUNTIME_ACTIVATION_CREDENTIAL_NAME}:gamecult-runtime-presence-identity"),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(root.join("odin.log"))?))
        .spawn()?;
    let mut odin = Odin {
        _temp: temp,
        root,
        candidate,
        child,
        store: state_root.join("topology.cc"),
    };

    // Idunn's warming probe, then the lease naming that exact warming presence.
    let deadline = Instant::now() + Duration::from_secs(20);
    let response = loop {
        match request_raw_snapshot_from_rudp_catalog(CultMeshRudpSnapshotOptions {
            target: candidate,
            runtime_id: "test-idunn".into(),
            message_id: format!("warming-{}", now()),
            schema_ids: Some(vec![GAMECULT_RUNTIME_PRESENCE_HEALTH_SCHEMA.into()]),
            record_keys: Some(vec!["odin".into()]),
            ..Default::default()
        }) {
            Ok(response) => break response,
            Err(error) => {
                if let Some(status) = odin.child.try_wait()? {
                    bail!("Odin exited while warming ({status}): {}", odin.log());
                }
                ensure!(Instant::now() < deadline, "Odin never answered warming: {error:#}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    let CultNetMessage::SnapshotResponseRaw { documents, .. } = response else {
        bail!("the warming probe was not answered with raw documents")
    };
    let presence: GameCultRuntimePresenceHealthRecord = rmp_serde::from_slice(&documents[0].payload)?;
    ensure!(presence.state == "warming", "{}", presence.state);
    let lease = IdunnProcessWriteLeaseRecord {
        schema_version: IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into(),
        target: "odin".into(),
        expected_projection_sha256: expected.canonical_sha256()?,
        plan_id: expected.plan_id.clone(),
        incarnation_id: expected.incarnation_id.clone(),
        sealed_release_id: expected.sealed_release_id.clone(),
        activation_witness_sha256: activation.canonical_sha256()?,
        state_schema_generation: "odin-v2".into(),
        state_contract_sha256: expected.state_contract_sha256.clone().unwrap(),
        runtime_id: expected.runtime_id.clone(),
        runtime_instance_id: activation.runtime_instance_id.clone(),
        warming_presence_sha256: presence.canonical_sha256()?,
        lease_epoch: 1,
        issued_at_unix_millis: now() - 5,
    };
    let lease_envelope = |key: &str| -> Result<CultCacheEnvelope> {
        Ok(CultCacheEnvelope {
            key: key.into(),
            r#type: IdunnProcessWriteLeaseRecord::TYPE.into(),
            payload: lease.canonical_bytes()?,
            stored_at: rfc3339(lease.issued_at_unix_millis),
            schema_id: Some(IDUNN_PROCESS_WRITE_LEASE_SCHEMA.into()),
        })
    };
    write_store(&lease_path, &[lease_envelope("odin")?])?;
    projected.push(lease_envelope(&incarnation_key)?);
    write_store(&projection, &projected)?;

    while !odin.store.is_file() {
        if let Some(status) = odin.child.try_wait()? {
            bail!("Odin exited while activating ({status}): {}", odin.log());
        }
        ensure!(Instant::now() < deadline, "Odin never activated: {}", odin.log());
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(odin)
}

/// A put Odin acknowledged is on disk: killed outright right after the
/// acknowledgement, inside the interval its own changes wait for, Odin loses
/// none of them.
#[test]
fn an_acknowledged_put_survives_sigkill() -> Result<()> {
    for round in 0..3 {
        let mut odin = start_odin()?;
        // Inside the interval that Odin's activation write started.
        std::thread::sleep(Duration::from_millis(300));
        let key = format!("doc-{round}");
        odin.put(&key)?;
        odin.signal("-KILL")?;
        ensure!(odin.holds(&key)?, "round {round}: the acknowledged put was lost: {}", odin.log());
    }
    Ok(())
}

/// SIGTERM reaches the serving loop as a stop request: Odin makes its last
/// write and exits cleanly, rather than waiting to be killed.
#[test]
fn a_stop_signal_ends_odin_cleanly() -> Result<()> {
    let mut odin = start_odin()?;
    let status = odin.signal("-TERM")?;
    let log = odin.log();
    ensure!(status.success(), "Odin exited {status}: {log}");
    ensure!(log.contains("Odin stopping on request"), "{log}");
    ensure!(!log.contains("final store write was not made"), "{log}");
    Ok(())
}

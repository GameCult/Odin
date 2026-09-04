use anyhow::Result;
use cultmesh_rs::{
    CultMeshRudpApplicationOperation, CultMeshRudpDocumentServer,
    CultMeshRudpDocumentServerOptions, CultMeshRudpPollOutcome, CultMeshRudpRawDocumentReceipt,
    CultMeshRudpRawDocumentSink, CultMeshRudpServerClock, CultMeshRudpSnapshotQuery,
    CultMeshRudpSnapshotSource,
};
use cultnet_rs::{
    CultNetMessage, CultNetRawDocumentRecord, CultNetRawPayloadEncoding,
    CultNetRudpReliableSendReceipt, CultNetRudpReliableSendStatus, CultNetRudpSocketMode,
    CultNetRudpSocketTransportConnection, CultNetRudpSocketTransportOptions, CultNetWireContract,
    decode_cultnet_message_from_slice, encode_cultnet_message_to_vec,
};
use pretty_assertions::assert_eq;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
struct SinkState {
    receipts: Vec<CultMeshRudpRawDocumentReceipt>,
    failures_remaining: usize,
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<SinkState>>);

impl Sink {
    fn fail_once() -> Self {
        Self(Arc::new(Mutex::new(SinkState {
            failures_remaining: 1,
            ..Default::default()
        })))
    }
}

impl CultMeshRudpRawDocumentSink for Sink {
    fn accept_raw_document(&mut self, receipt: CultMeshRudpRawDocumentReceipt) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.failures_remaining > 0 {
            state.failures_remaining -= 1;
            anyhow::bail!("injected sink failure");
        }
        state.receipts.push(receipt);
        Ok(())
    }
}

#[derive(Default)]
struct SourceState {
    documents: Vec<CultNetRawDocumentRecord>,
    failures_remaining: usize,
    calls: usize,
}

#[derive(Clone, Default)]
struct Source(Arc<Mutex<SourceState>>);

impl Source {
    fn documents(documents: Vec<CultNetRawDocumentRecord>) -> Self {
        Self(Arc::new(Mutex::new(SourceState {
            documents,
            ..Default::default()
        })))
    }

    fn fail_once(documents: Vec<CultNetRawDocumentRecord>) -> Self {
        Self(Arc::new(Mutex::new(SourceState {
            documents,
            failures_remaining: 1,
            calls: 0,
        })))
    }
}

impl CultMeshRudpSnapshotSource for Source {
    fn raw_snapshot(
        &mut self,
        _: &CultMeshRudpSnapshotQuery,
    ) -> Result<Vec<CultNetRawDocumentRecord>> {
        let mut state = self.0.lock().unwrap();
        state.calls += 1;
        if state.failures_remaining > 0 {
            state.failures_remaining -= 1;
            anyhow::bail!("injected source failure");
        }
        Ok(state.documents.clone())
    }
}

#[derive(Clone)]
struct Clock {
    unix: Arc<AtomicU64>,
    monotonic: Arc<AtomicU64>,
}

impl Clock {
    fn new(now: u64) -> Self {
        Self {
            unix: Arc::new(AtomicU64::new(now)),
            monotonic: Arc::new(AtomicU64::new(now)),
        }
    }

    fn set(&self, now: u64) {
        self.unix.store(now, Ordering::SeqCst);
        self.monotonic.store(now, Ordering::SeqCst);
    }

    fn set_unix(&self, now: u64) {
        self.unix.store(now, Ordering::SeqCst);
    }

    fn set_monotonic(&self, now: u64) {
        self.monotonic.store(now, Ordering::SeqCst);
    }
}

impl CultMeshRudpServerClock for Clock {
    fn now_unix_millis(&self) -> u64 {
        self.unix.load(Ordering::SeqCst)
    }

    fn now_monotonic_millis(&self) -> u64 {
        self.monotonic.load(Ordering::SeqCst)
    }
}

type Server = CultMeshRudpDocumentServer<Sink, Source, Clock>;

fn document(key: &str, payload: Vec<u8>) -> CultNetRawDocumentRecord {
    CultNetRawDocumentRecord {
        schema_id: "test.raw.v1".into(),
        record_key: key.into(),
        stored_at: "2026-09-03T00:00:00Z".into(),
        payload_encoding: CultNetRawPayloadEncoding::Messagepack,
        payload,
        source_runtime_id: Some(format!("runtime-{key}")),
        source_agent_id: None,
        source_role: None,
        tags: None,
    }
}

fn server(
    options: CultMeshRudpDocumentServerOptions,
    clock: Clock,
    sink: Sink,
    source: Source,
) -> Result<Server> {
    CultMeshRudpDocumentServer::new(
        UdpSocket::bind("127.0.0.1:0")?,
        sink,
        source,
        clock,
        options,
    )
}

fn client(target: SocketAddr, id: u32) -> Result<CultNetRudpSocketTransportConnection> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.set_nonblocking(true)?;
    CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
        runtime_id: "test-client".into(),
        socket,
        mode: CultNetRudpSocketMode::Client,
        remote_addr: Some(target),
        connection_id: id,
        initial_sequence: 1,
        resend_delay_ms: 10,
        transport_id: None,
        max_payload_bytes: None,
        max_fragment_bytes: Some(1200),
        max_pending_reliable_packets: Some(64),
        media_reliable_expire_after_ms: None,
    })
}

fn connect(
    server: &mut Server,
    clients: &mut [&mut CultNetRudpSocketTransportConnection],
) -> Result<()> {
    for client in &mut *clients {
        client.connect(Vec::new())?;
    }
    for _ in 0..500 {
        server.poll_once()?;
        for client in &mut *clients {
            client.receive_once()?;
        }
        if clients.iter().all(|client| client.connected()) {
            for _ in 0..clients.len() {
                server.poll_once()?;
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(1));
    }
    anyhow::bail!("client connection timed out")
}

fn send(client: &mut CultNetRudpSocketTransportConnection, message: &CultNetMessage) -> Result<()> {
    client.send(
        "schema",
        encode_cultnet_message_to_vec(message, CultNetWireContract::CultNetSchemaV0)?,
    )
}

fn send_reliable(
    client: &mut CultNetRudpSocketTransportConnection,
    message: &CultNetMessage,
) -> Result<CultNetRudpReliableSendReceipt> {
    client.send_reliable(
        "schema",
        encode_cultnet_message_to_vec(message, CultNetWireContract::CultNetSchemaV0)?,
    )
}

#[test]
fn same_connection_id_is_peer_scoped_and_raw_bytes_are_untouched() -> Result<()> {
    let clock = Clock::new(42_000);
    let sink = Sink::default();
    let mut server = server(Default::default(), clock, sink.clone(), Source::default())?;
    let mut a = client(server.local_addr()?, 77)?;
    let mut b = client(server.local_addr()?, 77)?;
    connect(&mut server, &mut [&mut a, &mut b])?;

    let a_bytes = vec![0xc4, 0x04, 0x00, 0xff, 0x80, 0x01];
    let b_bytes = vec![0x92, 0x01, 0xc4, 0x02, 0x00, 0xff];
    send(
        &mut a,
        &CultNetMessage::DocumentPutRaw {
            message_id: "a".into(),
            document: document("a", a_bytes.clone()),
        },
    )?;
    send(
        &mut b,
        &CultNetMessage::DocumentPutRaw {
            message_id: "b".into(),
            document: document("b", b_bytes.clone()),
        },
    )?;
    for _ in 0..500 {
        server.poll_once()?;
        a.receive_once()?;
        b.receive_once()?;
        if sink.0.lock().unwrap().receipts.len() == 2 {
            break;
        }
    }

    let receipts = sink.0.lock().unwrap();
    assert_eq!(server.session_count(), 2);
    assert_eq!(receipts.receipts.len(), 2);
    assert_ne!(
        receipts.receipts[0].session.remote_addr,
        receipts.receipts[1].session.remote_addr
    );
    assert_eq!(
        receipts
            .receipts
            .iter()
            .find(|r| r.message_id == "a")
            .unwrap()
            .document
            .payload,
        a_bytes
    );
    assert_eq!(
        receipts
            .receipts
            .iter()
            .find(|r| r.message_id == "b")
            .unwrap()
            .document
            .payload,
        b_bytes
    );
    assert!(
        receipts
            .receipts
            .iter()
            .all(|r| r.received_at_unix_millis == 42_000)
    );
    Ok(())
}

#[test]
fn duplicate_connect_preserves_epoch_and_fresh_epoch_is_separate() -> Result<()> {
    let clock = Clock::new(45_000);
    let sink = Sink::default();
    let mut server = server(Default::default(), clock, sink.clone(), Source::default())?;
    let target = server.local_addr()?;
    let mut incumbent = client(target, 91)?;
    connect(&mut server, &mut [&mut incumbent])?;

    incumbent.connect(Vec::new())?;
    server.poll_once()?;
    incumbent.receive_once()?;
    assert_eq!(server.session_count(), 1);

    send(
        &mut incumbent,
        &CultNetMessage::DocumentPutRaw {
            message_id: "after-duplicate".into(),
            document: document("incumbent", vec![1, 2, 3]),
        },
    )?;
    for _ in 0..20 {
        server.poll_once()?;
        if sink.0.lock().unwrap().receipts.len() == 1 {
            break;
        }
    }
    assert_eq!(sink.0.lock().unwrap().receipts.len(), 1);

    let mut fresh_epoch = client(target, 92)?;
    connect(&mut server, &mut [&mut fresh_epoch])?;
    assert_eq!(server.session_count(), 2);
    Ok(())
}

#[test]
fn application_rejection_is_nonfatal_peer_scoped_and_unacknowledged() -> Result<()> {
    let clock = Clock::new(48_000);
    let sink = Sink::fail_once();
    let source = Source::fail_once(vec![document("snapshot", vec![4, 5, 6])]);
    let mut server = server(
        Default::default(),
        clock.clone(),
        sink.clone(),
        source.clone(),
    )?;
    let target = server.local_addr()?;

    let mut publisher = client(target, 101)?;
    let mut snapshot_client = client(target, 102)?;
    let mut survivor = client(target, 103)?;
    connect(
        &mut server,
        &mut [&mut publisher, &mut snapshot_client, &mut survivor],
    )?;
    let publish_receipt = send_reliable(
        &mut publisher,
        &CultNetMessage::DocumentPutRaw {
            message_id: "rejected-put".into(),
            document: document("rejected", vec![0, 255, 1]),
        },
    )?;
    let CultMeshRudpPollOutcome::ApplicationRejected(rejection) = server.poll_once()? else {
        panic!("sink rejection must be returned as a nonfatal poll outcome");
    };
    assert_eq!(rejection.session.connection_id, 101);
    assert_eq!(
        rejection.operation,
        CultMeshRudpApplicationOperation::DocumentPutRaw
    );
    assert_eq!(rejection.message_id, "rejected-put");
    assert!(rejection.reason.contains("injected sink failure"));
    assert_eq!(server.session_count(), 2);
    publisher.receive_once()?;
    assert_eq!(
        publisher.reliable_send_status(&publish_receipt),
        CultNetRudpReliableSendStatus::Pending
    );

    let snapshot_receipt = send_reliable(
        &mut snapshot_client,
        &CultNetMessage::SnapshotRequest {
            message_id: "rejected-snapshot".into(),
            schema_ids: None,
            record_keys: None,
        },
    )?;
    let CultMeshRudpPollOutcome::ApplicationRejected(rejection) = server.poll_once()? else {
        panic!("source rejection must be returned as a nonfatal poll outcome");
    };
    assert_eq!(rejection.session.connection_id, 102);
    assert_eq!(
        rejection.operation,
        CultMeshRudpApplicationOperation::SnapshotRequest
    );
    assert_eq!(rejection.message_id, "rejected-snapshot");
    assert!(rejection.reason.contains("injected source failure"));
    assert_eq!(server.session_count(), 1);
    snapshot_client.receive_once()?;
    assert_eq!(
        snapshot_client.reliable_send_status(&snapshot_receipt),
        CultNetRudpReliableSendStatus::Pending
    );

    let survivor_receipt = send_reliable(
        &mut survivor,
        &CultNetMessage::DocumentPutRaw {
            message_id: "accepted-after-rejections".into(),
            document: document("survivor", vec![9, 8, 7]),
        },
    )?;
    assert_eq!(server.poll_once()?, CultMeshRudpPollOutcome::Handled);
    for _ in 0..20 {
        survivor.receive_once()?;
        if survivor.reliable_send_status(&survivor_receipt)
            == CultNetRudpReliableSendStatus::Acknowledged
        {
            break;
        }
    }
    assert_eq!(
        survivor.reliable_send_status(&survivor_receipt),
        CultNetRudpReliableSendStatus::Acknowledged
    );
    assert_eq!(sink.0.lock().unwrap().receipts.len(), 1);
    assert_eq!(source.0.lock().unwrap().calls, 1);
    Ok(())
}

#[test]
fn snapshot_response_resends_until_acknowledged() -> Result<()> {
    let clock = Clock::new(51_000);
    let expected = document("catalog", vec![0x81, 0xa1, 0x78, 0x2a]);
    let source = Source::documents(vec![expected.clone()]);
    let options = CultMeshRudpDocumentServerOptions {
        resend_delay: Duration::from_millis(10),
        ..Default::default()
    };
    let mut server = server(options, clock.clone(), Sink::default(), source)?;
    let mut client = client(server.local_addr()?, 88)?;
    connect(&mut server, &mut [&mut client])?;
    send(
        &mut client,
        &CultNetMessage::SnapshotRequest {
            message_id: "snapshot".into(),
            schema_ids: Some(vec!["test.raw.v1".into()]),
            record_keys: Some(vec!["catalog".into()]),
        },
    )?;
    server.poll_once()?;

    clock.set(51_011);
    assert_eq!(server.maintain()?.packets_resent, 1);
    let response = loop {
        if let Some(frame) = client.receive_once()? {
            break decode_cultnet_message_from_slice(
                &frame.payload,
                CultNetWireContract::CultNetSchemaV0,
            )?;
        }
    };
    assert_eq!(
        response,
        CultNetMessage::SnapshotResponseRaw {
            message_id: "snapshot".into(),
            documents: vec![expected],
        }
    );

    for _ in 0..20 {
        server.poll_once()?;
    }
    clock.set(51_022);
    assert_eq!(server.maintain()?.packets_resent, 0);
    Ok(())
}

#[test]
fn payload_and_snapshot_output_budgets_fail_closed() -> Result<()> {
    let clock = Clock::new(55_000);
    let sink = Sink::default();
    let message = CultNetMessage::DocumentPutRaw {
        message_id: "budget".into(),
        document: document("budget", vec![7; 64]),
    };
    let encoded = encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)?;
    let options = CultMeshRudpDocumentServerOptions {
        max_admitted_payload_bytes: encoded.len(),
        max_admitted_payload_bytes_per_session: encoded.len(),
        max_snapshot_response_bytes: encoded.len(),
        ..Default::default()
    };
    let mut budget_server = server(options, clock, sink.clone(), Source::default())?;
    let target = budget_server.local_addr()?;
    let mut first = client(target, 111)?;
    let mut second = client(target, 112)?;
    connect(&mut budget_server, &mut [&mut first, &mut second])?;
    send(&mut first, &message)?;
    budget_server.poll_once()?;
    send(&mut second, &message)?;
    budget_server.poll_once()?;
    assert_eq!(sink.0.lock().unwrap().receipts.len(), 1);

    let oversized = Source::documents(vec![document("oversized", vec![8; 512])]);
    let options = CultMeshRudpDocumentServerOptions {
        max_snapshot_response_bytes: 128,
        ..Default::default()
    };
    let mut snapshot_server = server(options, Clock::new(56_000), Sink::default(), oversized)?;
    let mut snapshot_client = client(snapshot_server.local_addr()?, 113)?;
    connect(&mut snapshot_server, &mut [&mut snapshot_client])?;
    let receipt = send_reliable(
        &mut snapshot_client,
        &CultNetMessage::SnapshotRequest {
            message_id: "too-large".into(),
            schema_ids: None,
            record_keys: None,
        },
    )?;
    let CultMeshRudpPollOutcome::ApplicationRejected(rejection) = snapshot_server.poll_once()?
    else {
        panic!("oversized snapshot must reject only its session");
    };
    assert_eq!(
        rejection.operation,
        CultMeshRudpApplicationOperation::SnapshotRequest
    );
    assert_eq!(rejection.message_id, "too-large");
    assert!(rejection.reason.contains("snapshot response is"));
    assert_eq!(snapshot_server.session_count(), 0);
    snapshot_client.receive_once()?;
    assert_eq!(
        snapshot_client.reliable_send_status(&receipt),
        CultNetRudpReliableSendStatus::Pending
    );
    Ok(())
}

#[test]
fn session_cap_and_expiry_use_monotonic_time() -> Result<()> {
    let clock = Clock::new(60_000);
    let options = CultMeshRudpDocumentServerOptions {
        max_sessions: 1,
        session_idle_timeout: Duration::from_secs(1),
        session_max_lifetime: Duration::from_millis(100),
        ..Default::default()
    };
    let mut server = server(options, clock.clone(), Sink::default(), Source::default())?;
    let target = server.local_addr()?;
    let mut admitted = client(target, 1)?;
    connect(&mut server, &mut [&mut admitted])?;
    let mut rejected = client(target, 1)?;
    rejected.connect(Vec::new())?;
    server.poll_once()?;
    assert_eq!(server.session_count(), 1);
    assert!(!rejected.connected());

    clock.set_unix(1);
    clock.set_monotonic(60_101);
    assert_eq!(server.maintain()?.sessions_expired, 1);
    assert_eq!(server.session_count(), 0);
    Ok(())
}

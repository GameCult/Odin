use anyhow::Result;
use chrono::Duration;
use chrono::Utc;
use cultcache_rs::CultCache;
use cultcache_rs::DatabaseEntry;
use cultcache_rs::SingleFileMessagePackBackingStore;
use cultnet_rs::CultMesh;
use cultnet_rs::CultMeshAuthorityLease;
use cultnet_rs::CultMeshPeerCard;
use cultnet_rs::CultMeshRudpClientOptions;
use cultnet_rs::CultMeshRudpSocketOptions;
use cultnet_rs::CultNetClientSecurityOptions;
use cultnet_rs::CultNetDocumentBinding;
use cultnet_rs::CultNetDocumentMutationContract;
use cultnet_rs::CultNetDocumentOperation;
use cultnet_rs::CultNetDocumentPutOptions;
use cultnet_rs::CultNetDocumentRegistry;
use cultnet_rs::CultNetMessage;
use cultnet_rs::CultNetMutationAuthority;
use cultnet_rs::CultNetReactiveDocumentOptions;
use cultnet_rs::CultNetReconnectController;
use cultnet_rs::CultNetReconnectPolicyOptions;
use cultnet_rs::CultNetRudpPacket;
use cultnet_rs::CultNetRudpPacketType;
use cultnet_rs::CultNetRudpReconnectLoop;
use cultnet_rs::CultNetRudpSendOptions;
use cultnet_rs::CultNetRudpSession;
use cultnet_rs::CultNetRudpSessionOptions;
use cultnet_rs::CultNetRudpSocketMode;
use cultnet_rs::CultNetRudpSocketTransportConnection;
use cultnet_rs::CultNetRudpSocketTransportOptions;
use cultnet_rs::CultNetSchemaKind;
use cultnet_rs::CultNetSchemaRegistration;
use cultnet_rs::CultNetSchemaRegistry;
use cultnet_rs::CultNetSecret;
use cultnet_rs::CultNetServerSecurityOptions;
use cultnet_rs::CultNetShardCatalog;
use cultnet_rs::CultNetShardDescriptor;
use cultnet_rs::CultNetTransportChannel;
use cultnet_rs::CultNetTransportDelivery;
use cultnet_rs::CultNetTransportDescriptor;
use cultnet_rs::CultNetTransportFrame;
use cultnet_rs::CultNetTransportOrdering;
use cultnet_rs::CultNetTransportProfile;
use cultnet_rs::CultNetTransportProtocol;
use cultnet_rs::CultNetWireContract;
use cultnet_rs::LengthPrefixedMessageFramer;
use cultnet_rs::TcpFramedTransportConnection;
use cultnet_rs::TcpFramedTransportProfileOptions;
use cultnet_rs::builtin_schema_registry;
use cultnet_rs::compute_reconnect_delay_ms;
use cultnet_rs::create_reconnect_policy;
use cultnet_rs::create_rudp_transport_profile;
use cultnet_rs::create_tcp_framed_transport_profile;
use cultnet_rs::decode_cultnet_message_from_slice;
use cultnet_rs::decode_rudp_packet;
use cultnet_rs::encode_cultnet_message_for_wire;
use cultnet_rs::encode_cultnet_message_to_vec;
use cultnet_rs::encode_frame;
use cultnet_rs::encode_rudp_packet;
use pretty_assertions::assert_eq;
use std::cell::RefCell;
use std::net::UdpSocket;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration as StdDuration;

const TS_HELLO_FRAME: &[u8] = include_bytes!("fixtures/cultnet-ts-hello.frame");
const TS_LEGACY_LOGIN_FRAME: &[u8] = include_bytes!("fixtures/cultnet-ts-legacy-login.frame");

fn bind_udp_socket() -> Result<UdpSocket> {
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.set_read_timeout(Some(StdDuration::from_millis(20)))?;
    Ok(socket)
}

fn pump_rudp_handshake(
    client: &mut CultNetRudpSocketTransportConnection,
    server: &mut CultNetRudpSocketTransportConnection,
) -> Result<()> {
    for _ in 0..20 {
        let _ = server.receive_once()?;
        let _ = client.receive_once()?;
        let _ = server.receive_once()?;
        if client.connected() && server.connected() {
            return Ok(());
        }
        thread::sleep(StdDuration::from_millis(5));
    }
    anyhow::bail!("RUDP socket handshake did not complete");
}

fn receive_rudp_frame(
    transport: &mut CultNetRudpSocketTransportConnection,
) -> Result<CultNetTransportFrame> {
    for _ in 0..20 {
        if let Some(frame) = transport.receive_once()? {
            return Ok(frame);
        }
        thread::sleep(StdDuration::from_millis(5));
    }
    anyhow::bail!("RUDP socket frame was not delivered")
}

fn receive_rudp_schema_message(
    transport: &mut CultNetRudpSocketTransportConnection,
) -> Result<CultNetMessage> {
    for _ in 0..20 {
        if let Some(message) = transport.receive_schema_message_once()? {
            return Ok(message);
        }
        thread::sleep(StdDuration::from_millis(5));
    }
    anyhow::bail!("RUDP schema message was not delivered")
}

fn pump_rudp_server_until_connected(
    server: Arc<Mutex<CultNetRudpSocketTransportConnection>>,
    done: Arc<AtomicBool>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        while !done.load(Ordering::SeqCst) {
            {
                let mut server = server.lock().expect("RUDP server mutex poisoned");
                let _ = server.receive_once()?;
                server.poll_resends()?;
                if server.connected() {
                    return Ok(());
                }
            }
            thread::sleep(StdDuration::from_millis(5));
        }
        Ok(())
    })
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "ghostlight.agent-state",
    schema = "GhostlightAgentStateFixture"
)]
struct GhostlightAgentStateFixture {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    agent_id: String,
    #[cultcache(key = 2)]
    display_name: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "ghostlight.agent-state.ui",
    schema = "GhostlightAgentStateUiFixture"
)]
struct GhostlightAgentStateUiFixture {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    agent_id: String,
    #[cultcache(key = 2)]
    display_name: String,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "ghostlight.reactive-note",
    schema = "GhostlightReactiveNoteFixture"
)]
struct GhostlightReactiveNoteFixture {
    #[cultcache(key = 0)]
    body: String,
    #[cultcache(key = 1)]
    revision: i32,
}

#[derive(Clone, Debug, PartialEq, DatabaseEntry)]
#[cultcache(
    type = "ghostlight.reactive-note.ui",
    schema = "GhostlightReactiveNoteUiFixture"
)]
struct GhostlightReactiveNoteUiFixture {
    #[cultcache(key = 0)]
    body: String,
    #[cultcache(key = 1)]
    revision: i32,
}

#[test]
fn security_helpers_round_trip_encrypted_strings_and_validate_sessions() -> Result<()> {
    let server_security = CultNetServerSecurityOptions::development();
    let client_security = server_security.to_client_options();
    let nonce = CultNetSecret::new_nonce();
    let encrypted = CultNetSecret::encrypt_string(Some("hello"), &nonce, &client_security)?
        .expect("non-empty input encrypts");
    assert_eq!(
        CultNetSecret::decrypt_string(Some(&encrypted), Some(&nonce), &server_security)?,
        Some("hello".to_string())
    );

    let token = CultNetSecret::create_session_token(
        "runtime-face",
        chrono::Utc::now() + Duration::minutes(1),
        &server_security,
    )?;
    let validated = CultNetSecret::try_validate_session_token(Some(&token), &server_security)?
        .expect("token validates before expiry");
    assert_eq!(validated.user_id, "runtime-face");
    Ok(())
}

#[test]
fn cultnet_schema_messages_round_trip_through_messagepack_frames() -> Result<()> {
    let message = CultNetMessage::Hello {
        runtime_id: "voidbot-main".to_string(),
        runtime_kind: "rust-worker".to_string(),
        agent_id: Some("void".to_string()),
        role: None,
        display_name: Some("Void".to_string()),
        supported_document_types: Some(vec!["ghostlight.agent-state".to_string()]),
        supported_mutation_contracts: Some(vec![CultNetDocumentMutationContract {
            document_type: "ghostlight.agent-state".to_string(),
            payload_schema_version: Some("ghostlight.agent_state.v0".to_string()),
            operations: vec![
                CultNetDocumentOperation::Snapshot,
                CultNetDocumentOperation::DocumentPut,
            ],
            authority: CultNetMutationAuthority::Coordinator,
            intent_document_types: Some(vec!["ghostlight.agent-state.intent.v0".to_string()]),
            receipt_document_types: Some(vec!["ghostlight.agent-state.receipt.v0".to_string()]),
            notes: Some(vec![
                "Persona may request bounded memory mutation; coordinator reviews authority."
                    .to_string(),
            ]),
        }]),
        supported_message_versions: None,
        transport_profiles: Some(vec![CultNetTransportProfile {
            schema_version: "cultnet.transport_profile.v0".to_string(),
            runtime_id: "voidbot-main".to_string(),
            transports: vec![CultNetTransportDescriptor {
                transport_id: "direct-pipe".to_string(),
                protocol: CultNetTransportProtocol::TcpFramed,
                host: None,
                port: None,
                path: None,
                discovery_group: None,
                wire_contracts: Some(vec!["cultnet.schema.v0".to_string()]),
                reconnect_policy: None,
                channels: vec![CultNetTransportChannel {
                    channel_id: "schema".to_string(),
                    delivery: CultNetTransportDelivery::Reliable,
                    ordering: CultNetTransportOrdering::Ordered,
                    max_payload_bytes: None,
                    max_fragment_bytes: None,
                    max_pending_reliable_packets: None,
                }],
            }],
        }]),
        supports_schema_catalog: Some(true),
    };
    let payload = encode_cultnet_message_to_vec(&message, CultNetWireContract::CultNetSchemaV0)?;
    let frame = encode_frame(&payload)?;
    assert_eq!(&frame[..4], &(payload.len() as u32).to_be_bytes());

    let mut framer = LengthPrefixedMessageFramer::new();
    assert!(framer.push(&frame[..2]).is_empty());
    let frames = framer.push(&frame[2..]);
    assert_eq!(frames.len(), 1);
    let decoded =
        decode_cultnet_message_from_slice(&frames[0], CultNetWireContract::CultNetSchemaV0)?;
    assert_eq!(decoded, message);
    Ok(())
}

#[test]
fn tcp_framed_transport_carries_schema_payloads_with_stats() -> Result<()> {
    let payload = b"cultnet-payload".to_vec();
    let profile = create_tcp_framed_transport_profile(
        "rust-transport",
        TcpFramedTransportProfileOptions {
            transport_id: Some("test-tcp".to_string()),
            ..TcpFramedTransportProfileOptions::default()
        },
    );
    let mut sender = TcpFramedTransportConnection::new(Vec::<u8>::new(), profile.clone());
    sender.send("schema", &payload)?;
    assert_eq!(sender.stats().frames_sent, 1);
    assert_eq!(sender.stats().bytes_sent, (payload.len() + 4) as u64);
    assert!(sender.send("unreliable", &[]).is_err());

    let bytes = sender.into_inner();
    let mut receiver = TcpFramedTransportConnection::new(std::io::Cursor::new(bytes), profile);
    let frame = receiver.receive()?;
    assert_eq!(frame.channel_id, "schema");
    assert_eq!(frame.payload, payload);
    assert_eq!(receiver.stats().frames_received, 1);
    assert_eq!(
        receiver.stats().bytes_received,
        (frame.payload.len() + 4) as u64
    );
    assert_eq!(
        receiver.profile.transports[0].protocol,
        CultNetTransportProtocol::TcpFramed
    );
    Ok(())
}

#[test]
fn rudp_packet_codec_uses_deterministic_reliable_ordered_fixture() -> Result<()> {
    let encoded = encode_rudp_packet(&CultNetRudpPacket {
        packet_type: CultNetRudpPacketType::Data,
        connection_id: 0x01020304,
        sequence: 0x0000002a,
        ack: 0x00000029,
        ack_mask: 0x80000001,
        channel_id: "schema".to_string(),
        reliable: true,
        ordered: true,
        sequenced: false,
        fragment_id: 7,
        fragment_index: 1,
        fragment_count: 3,
        payload: b"hello".to_vec(),
    })?;

    assert_eq!(
        encoded,
        vec![
            67, 78, 82, 48, 0, 3, 11, 42, 1, 2, 3, 4, 0, 0, 0, 42, 0, 0, 0, 41, 128, 0, 0, 1, 0, 7,
            0, 1, 0, 3, 0, 0, 0, 5, 6, 0, 115, 99, 104, 101, 109, 97, 104, 101, 108, 108, 111,
        ]
    );

    let decoded = decode_rudp_packet(&encoded)?;
    assert_eq!(decoded.packet_type, CultNetRudpPacketType::Data);
    assert_eq!(decoded.connection_id, 0x01020304);
    assert_eq!(decoded.sequence, 0x0000002a);
    assert_eq!(decoded.ack, 0x00000029);
    assert_eq!(decoded.ack_mask, 0x80000001);
    assert_eq!(decoded.channel_id, "schema");
    assert!(decoded.reliable);
    assert!(decoded.ordered);
    assert!(!decoded.sequenced);
    assert_eq!(decoded.fragment_id, 7);
    assert_eq!(decoded.fragment_index, 1);
    assert_eq!(decoded.fragment_count, 3);
    assert_eq!(decoded.payload, b"hello");
    Ok(())
}

#[test]
fn rudp_transport_profile_advertises_state_and_realtime_channels() {
    let profile = create_rudp_transport_profile(
        "rust-rudp",
        cultnet_rs::RudpTransportProfileOptions {
            transport_id: Some("public-rudp".to_string()),
            host: Some("127.0.0.1".to_string()),
            port: Some(7777),
            max_payload_bytes: Some(1200),
            max_fragment_bytes: Some(1000),
            max_pending_reliable_packets: Some(64),
            reconnect_policy: None,
        },
    );

    assert_eq!(
        profile.transports[0].protocol,
        CultNetTransportProtocol::Rudp
    );
    assert_eq!(
        profile.transports[0]
            .reconnect_policy
            .as_ref()
            .map(|policy| policy.schema_version.as_str()),
        Some("cultnet.reconnect_policy.v0")
    );
    assert_eq!(
        profile.transports[0]
            .reconnect_policy
            .as_ref()
            .map(|policy| policy.base_delay_ms),
        Some(1_000)
    );
    let channels = profile.transports[0]
        .channels
        .iter()
        .map(|channel| {
            (
                channel.channel_id.as_str(),
                channel.delivery,
                channel.ordering,
                channel.max_pending_reliable_packets,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        channels,
        vec![
            (
                "schema",
                CultNetTransportDelivery::Reliable,
                CultNetTransportOrdering::Ordered,
                Some(64)
            ),
            (
                "latest",
                CultNetTransportDelivery::Unreliable,
                CultNetTransportOrdering::Sequenced,
                Some(64)
            ),
            (
                "realtime",
                CultNetTransportDelivery::Unreliable,
                CultNetTransportOrdering::Unordered,
                Some(64)
            ),
            (
                "media",
                CultNetTransportDelivery::Reliable,
                CultNetTransportOrdering::Unordered,
                Some(64)
            ),
        ]
    );
}

#[test]
fn reconnect_policy_exposes_portable_delay_contract() {
    let policy = create_reconnect_policy(CultNetReconnectPolicyOptions {
        policy_id: "rudp-default".to_string(),
        max_attempts: Some(8),
        ..CultNetReconnectPolicyOptions::default()
    });

    assert_eq!(policy.schema_version, "cultnet.reconnect_policy.v0");
    assert_eq!(policy.policy_id, "rudp-default");
    assert_eq!(policy.max_attempts, Some(8));
    assert_eq!(compute_reconnect_delay_ms(&policy, 1, 0), 1_000);
    assert_eq!(compute_reconnect_delay_ms(&policy, 3, 17), 4_017);
    assert_eq!(compute_reconnect_delay_ms(&policy, 9, 999), 30_250);
    assert_eq!(compute_reconnect_delay_ms(&policy, 0, 0), 1_000);
}

#[test]
fn reconnect_controller_schedules_attempts_and_reset() {
    let policy = create_reconnect_policy(CultNetReconnectPolicyOptions {
        max_attempts: Some(2),
        ..CultNetReconnectPolicyOptions::default()
    });
    let mut controller = CultNetReconnectController::new(policy);

    let first = controller.record_failure(10_000, 0);
    assert_eq!(first.attempt, 1);
    assert!(first.should_retry);
    assert_eq!(first.delay_ms, 1_000);
    assert_eq!(first.next_attempt_at_ms, Some(11_000));
    assert!(!first.exhausted);
    assert!(!controller.can_attempt(10_999));
    assert!(controller.can_attempt(11_000));

    let second = controller.record_failure(11_000, 17);
    assert_eq!(second.attempt, 2);
    assert_eq!(second.delay_ms, 2_017);
    assert_eq!(second.next_attempt_at_ms, Some(13_017));
    assert!(second.should_retry);

    let exhausted = controller.record_failure(13_017, 0);
    assert_eq!(exhausted.attempt, 2);
    assert!(!exhausted.should_retry);
    assert_eq!(exhausted.delay_ms, 0);
    assert_eq!(exhausted.next_attempt_at_ms, None);
    assert!(exhausted.exhausted);
    assert!(!controller.can_attempt(99_000));

    controller.reset();
    assert_eq!(controller.attempt(), 0);
    assert_eq!(controller.next_attempt_at_ms(), None);
    assert!(!controller.exhausted());
    assert!(controller.can_attempt(99_000));
}

#[test]
fn rudp_reconnect_loop_consumes_shared_controller() -> Result<()> {
    let server_socket = bind_udp_socket()?;
    let remote_addr = server_socket.local_addr()?;
    let opened_local_addrs = Rc::new(RefCell::new(Vec::new()));
    let opened_for_factory = Rc::clone(&opened_local_addrs);
    let connection_id = 0x2233_4455;
    let policy = create_reconnect_policy(CultNetReconnectPolicyOptions {
        max_attempts: Some(2),
        ..CultNetReconnectPolicyOptions::default()
    });

    let mut loop_ = CultNetRudpReconnectLoop::new(policy, b"join".to_vec(), move || {
        let socket = bind_udp_socket()?;
        opened_for_factory.borrow_mut().push(socket.local_addr()?);
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions::client(
            "rust-rudp-reconnect",
            socket,
            remote_addr,
            connection_id,
        ))
    });

    {
        let first = loop_.start()?;
        assert!(first.stats().bytes_sent > 0);
    }
    assert!(loop_.transport().is_some());
    assert_eq!(opened_local_addrs.borrow().len(), 1);

    let decision = loop_.handle_closed(10_000, 17).expect("retry decision");
    assert_eq!(decision.attempt, 1);
    assert!(decision.should_retry);
    assert_eq!(decision.delay_ms, 1_017);
    assert_eq!(decision.next_attempt_at_ms, Some(11_017));
    assert_eq!(loop_.reconnect_controller.attempt(), 1);
    assert_eq!(
        loop_.reconnect_controller.next_attempt_at_ms(),
        Some(11_017)
    );

    assert!(!loop_.reconnect_if_due(11_016)?);
    assert_eq!(opened_local_addrs.borrow().len(), 1);
    assert!(loop_.reconnect_if_due(11_017)?);
    assert_eq!(opened_local_addrs.borrow().len(), 2);
    assert!(loop_.transport().is_some());

    loop_.mark_connected();
    assert_eq!(loop_.reconnect_controller.attempt(), 0);

    loop_.stop();
    assert!(loop_.transport().is_none());
    assert_eq!(loop_.reconnect_controller.attempt(), 0);

    Ok(())
}

#[test]
fn rudp_session_handshake_acks_reliable_connect_and_accept_packets() -> Result<()> {
    let mut client = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 0x0a0b0c0d,
        initial_sequence: 1,
        resend_delay_ms: 50,
        ..CultNetRudpSessionOptions::default()
    });
    let mut server = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 0x0a0b0c0d,
        initial_sequence: 100,
        resend_delay_ms: 50,
        ..CultNetRudpSessionOptions::default()
    });

    let connect = client.create_connect(0, b"join".to_vec())?;
    assert_eq!(connect.packet_type, CultNetRudpPacketType::Connect);
    assert_eq!(connect.sequence, 1);
    assert_eq!(client.pending_reliable_sequences(), vec![1]);

    let accept = server.accept_connect(&connect, 10, b"ok".to_vec())?;
    assert_eq!(accept.packet_type, CultNetRudpPacketType::Accept);
    assert_eq!(accept.ack, 1);
    assert!(server.connected());
    assert_eq!(server.pending_reliable_sequences(), vec![100]);

    client.receive(&accept, 20)?;
    assert!(client.connected());
    assert!(client.pending_reliable_sequences().is_empty());

    let ack = client.create_ack();
    assert_eq!(ack.ack, 100);
    server.receive(&ack, 30)?;
    assert!(server.pending_reliable_sequences().is_empty());
    Ok(())
}

#[test]
fn rudp_session_computes_ack_masks_and_clears_pending_reliable_packets() -> Result<()> {
    let mut sender = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 7,
        initial_sequence: 10,
        resend_delay_ms: 100,
        ..CultNetRudpSessionOptions::default()
    });
    let mut receiver = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 7,
        initial_sequence: 200,
        resend_delay_ms: 100,
        ..CultNetRudpSessionOptions::default()
    });
    sender.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 7,
            sequence: 1,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    receiver.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 7,
            sequence: 2,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;

    let options = CultNetRudpSendOptions {
        reliable: true,
        ordered: true,
        ..CultNetRudpSendOptions::default()
    };
    let first = sender.send("schema", b"first".to_vec(), options.clone())?;
    let second = sender.send("schema", b"second".to_vec(), options.clone())?;
    let third = sender.send("schema", b"third".to_vec(), options)?;
    assert_eq!(sender.pending_reliable_sequences(), vec![10, 11, 12]);

    receiver.receive(&first, 0)?;
    receiver.receive(&third, 0)?;
    let ack_with_gap = receiver.create_ack();
    assert_eq!(ack_with_gap.ack, 12);
    assert_eq!(ack_with_gap.ack_mask, 0b10 | (1 << 9));
    sender.receive(&ack_with_gap, 0)?;
    assert_eq!(sender.pending_reliable_sequences(), vec![11]);

    receiver.receive(&second, 0)?;
    let full_ack = receiver.create_ack();
    assert_eq!(full_ack.ack, 12);
    assert_eq!(full_ack.ack_mask, 0b11 | (1 << 9));
    sender.receive(&full_ack, 0)?;
    assert!(sender.pending_reliable_sequences().is_empty());
    Ok(())
}

#[test]
fn rudp_session_schedules_reliable_resends_until_acked() -> Result<()> {
    let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 99,
        initial_sequence: 1,
        resend_delay_ms: 100,
        ..CultNetRudpSessionOptions::default()
    });
    session.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 99,
            sequence: 50,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    let sent = session.send(
        "schema",
        b"payload".to_vec(),
        CultNetRudpSendOptions {
            reliable: true,
            ordered: true,
            now_ms: 10,
            ..CultNetRudpSendOptions::default()
        },
    )?;

    assert!(session.due_resends(90).is_empty());
    assert_eq!(
        session
            .due_resends(110)
            .iter()
            .map(|packet| packet.sequence)
            .collect::<Vec<_>>(),
        vec![sent.sequence]
    );
    assert!(session.due_resends(150).is_empty());

    session.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Ack,
            connection_id: 99,
            sequence: 51,
            ack: sent.sequence,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    assert!(session.due_resends(250).is_empty());
    Ok(())
}

#[test]
fn rudp_session_pings_and_detects_receive_timeout() -> Result<()> {
    let mut client = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 101,
        initial_sequence: 1,
        ..CultNetRudpSessionOptions::default()
    });
    let mut server = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 101,
        initial_sequence: 100,
        ..CultNetRudpSessionOptions::default()
    });
    let connect = client.create_connect(0, b"join".to_vec())?;
    let accept = server.accept_connect(&connect, 10, Vec::new())?;
    client.receive(&accept, 20)?;

    let ping = client.create_ping(b"pulse".to_vec());
    let ping_result = server.receive(&ping, 30)?;
    let pong = ping_result.reply.expect("ping should produce pong");
    assert_eq!(pong.packet_type, CultNetRudpPacketType::Pong);
    assert_eq!(pong.payload, b"pulse");

    let pong_result = client.receive(&pong, 40)?;
    assert!(pong_result.pong);
    assert_eq!(pong_result.pong_payload, b"pulse");
    assert!(!client.check_timeout(90, 50));
    assert!(client.check_timeout(91, 50));
    assert!(!client.connected());
    Ok(())
}

#[test]
fn rudp_session_bounds_pending_reliable_packets_before_enqueue() -> Result<()> {
    let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 102,
        initial_sequence: 1,
        max_pending_reliable_packets: Some(2),
        ..CultNetRudpSessionOptions::default()
    });
    session.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 102,
            sequence: 50,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    session.send(
        "schema",
        b"first".to_vec(),
        CultNetRudpSendOptions {
            reliable: true,
            ordered: true,
            ..CultNetRudpSendOptions::default()
        },
    )?;
    session.send(
        "schema",
        b"second".to_vec(),
        CultNetRudpSendOptions {
            reliable: true,
            ordered: true,
            ..CultNetRudpSendOptions::default()
        },
    )?;
    let error = session
        .send(
            "schema",
            b"third".to_vec(),
            CultNetRudpSendOptions {
                reliable: true,
                ordered: true,
                ..CultNetRudpSendOptions::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("reliable send queue is full"));
    assert_eq!(session.pending_reliable_sequences(), vec![1, 2]);

    let mut fragmented = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 103,
        initial_sequence: 1,
        max_pending_reliable_packets: Some(3),
        ..CultNetRudpSessionOptions::default()
    });
    fragmented.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 103,
            sequence: 50,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    let error = fragmented
        .send_many(
            "schema",
            b"fragment-me".to_vec(),
            CultNetRudpSendOptions {
                reliable: true,
                ordered: true,
                ..CultNetRudpSendOptions::default()
            },
            Some(3),
        )
        .unwrap_err();
    assert!(error.to_string().contains("reliable send queue is full"));
    assert!(fragmented.pending_reliable_sequences().is_empty());
    Ok(())
}

#[test]
fn rudp_session_suppresses_duplicates_and_delivers_reliable_ordered_payloads_in_sequence()
-> Result<()> {
    let mut sender = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 123,
        initial_sequence: 1,
        ..CultNetRudpSessionOptions::default()
    });
    let mut receiver = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 123,
        initial_sequence: 100,
        ..CultNetRudpSessionOptions::default()
    });
    sender.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 123,
            sequence: 90,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    receiver.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 123,
            sequence: 91,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;

    let options = CultNetRudpSendOptions {
        reliable: true,
        ordered: true,
        ..CultNetRudpSendOptions::default()
    };
    let first = sender.send("schema", b"first".to_vec(), options.clone())?;
    let second = sender.send("schema", b"second".to_vec(), options.clone())?;
    let third = sender.send("schema", b"third".to_vec(), options)?;

    assert_eq!(
        receiver
            .receive(&first, 0)?
            .delivered
            .iter()
            .map(|frame| String::from_utf8(frame.payload.clone()).unwrap())
            .collect::<Vec<_>>(),
        vec!["first"]
    );
    assert!(receiver.receive(&third, 0)?.delivered.is_empty());
    assert!(receiver.receive(&first, 0)?.delivered.is_empty());
    assert_eq!(
        receiver
            .receive(&second, 0)?
            .delivered
            .iter()
            .map(|frame| String::from_utf8(frame.payload.clone()).unwrap())
            .collect::<Vec<_>>(),
        vec!["second", "third"]
    );
    Ok(())
}

#[test]
fn rudp_session_skips_control_packets_while_ordering_schema_payloads() -> Result<()> {
    let mut sender = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 124,
        initial_sequence: 1,
        ..CultNetRudpSessionOptions::default()
    });
    let mut receiver = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 124,
        initial_sequence: 100,
        ..CultNetRudpSessionOptions::default()
    });
    sender.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 124,
            sequence: 90,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    receiver.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 124,
            sequence: 91,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;

    let options = CultNetRudpSendOptions {
        reliable: true,
        ordered: true,
        ..CultNetRudpSendOptions::default()
    };
    let first = sender.send("schema", b"first".to_vec(), options.clone())?;
    let control = sender.create_ack();
    let second = sender.send("schema", b"second".to_vec(), options)?;
    assert_eq!(control.sequence, 0);
    assert_eq!(second.sequence, first.sequence + 1);

    assert_eq!(
        receiver
            .receive(&first, 0)?
            .delivered
            .iter()
            .map(|frame| String::from_utf8(frame.payload.clone()).unwrap())
            .collect::<Vec<_>>(),
        vec!["first"]
    );
    assert!(receiver.receive(&control, 0)?.delivered.is_empty());
    assert_eq!(
        receiver
            .receive(&second, 0)?
            .delivered
            .iter()
            .map(|frame| String::from_utf8(frame.payload.clone()).unwrap())
            .collect::<Vec<_>>(),
        vec!["second"]
    );
    Ok(())
}

#[test]
fn rudp_session_fragments_and_reassembles_reliable_ordered_payloads() -> Result<()> {
    let mut sender = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 456,
        initial_sequence: 1,
        ..CultNetRudpSessionOptions::default()
    });
    let mut receiver = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 456,
        initial_sequence: 100,
        ..CultNetRudpSessionOptions::default()
    });
    sender.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 456,
            sequence: 90,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;
    receiver.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 456,
            sequence: 91,
            ack: 0,
            ack_mask: 0,
            channel_id: "control".to_string(),
            reliable: false,
            ordered: false,
            sequenced: false,
            fragment_id: 0,
            fragment_index: 0,
            fragment_count: 0,
            payload: Vec::new(),
        },
        0,
    )?;

    let packets = sender.send_many(
        "schema",
        b"fragment-me-please".to_vec(),
        CultNetRudpSendOptions {
            reliable: true,
            ordered: true,
            now_ms: 10,
            ..CultNetRudpSendOptions::default()
        },
        Some(5),
    )?;
    assert_eq!(packets.len(), 4);
    assert_eq!(
        packets
            .iter()
            .map(|packet| packet.fragment_count)
            .collect::<Vec<_>>(),
        vec![4, 4, 4, 4]
    );
    assert_eq!(
        packets
            .iter()
            .map(|packet| packet.fragment_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
    assert!(
        packets
            .iter()
            .all(|packet| packet.fragment_id == packets[0].fragment_id)
    );

    assert!(receiver.receive(&packets[0], 0)?.delivered.is_empty());
    assert!(receiver.receive(&packets[1], 0)?.delivered.is_empty());
    assert!(receiver.receive(&packets[2], 0)?.delivered.is_empty());
    let delivered = receiver.receive(&packets[3], 0)?.delivered;
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].payload, b"fragment-me-please");
    assert_eq!(delivered[0].sequence, packets[0].sequence);
    Ok(())
}

#[test]
fn rudp_socket_transport_handshakes_and_carries_reliable_ordered_schema_frames() -> Result<()> {
    let server_socket = bind_udp_socket()?;
    let client_socket = bind_udp_socket()?;
    let server_addr = server_socket.local_addr()?;
    let connection_id = 0x1020_3040;
    let mut server =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "rust-rudp-server".to_string(),
            socket: server_socket,
            mode: CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id,
            initial_sequence: 100,
            resend_delay_ms: 25,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
            reconnect_policy: None,
        })?;
    let mut client =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "rust-rudp-client".to_string(),
            socket: client_socket,
            mode: CultNetRudpSocketMode::Client,
            remote_addr: Some(server_addr),
            connection_id,
            initial_sequence: 1,
            resend_delay_ms: 25,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
            reconnect_policy: None,
        })?;

    client.connect(b"join".to_vec())?;
    pump_rudp_handshake(&mut client, &mut server)?;
    assert!(client.connected());
    assert!(server.connected());

    client.send("schema", b"client-state".to_vec())?;
    let server_frame = receive_rudp_frame(&mut server)?;
    assert_eq!(server_frame.channel_id, "schema");
    assert_eq!(server_frame.payload, b"client-state");

    server.send("schema", b"server-state".to_vec())?;
    let client_frame = receive_rudp_frame(&mut client)?;
    assert_eq!(client_frame.channel_id, "schema");
    assert_eq!(client_frame.payload, b"server-state");

    client.send_schema_message(&CultNetMessage::Hello {
        runtime_id: "rust-rudp-client".to_string(),
        runtime_kind: "rust-worker".to_string(),
        agent_id: None,
        role: Some("schema".to_string()),
        display_name: None,
        supported_document_types: None,
        supported_mutation_contracts: None,
        supported_message_versions: None,
        transport_profiles: Some(vec![client.profile.clone()]),
        supports_schema_catalog: Some(true),
    })?;
    let message = receive_rudp_schema_message(&mut server)?;
    let CultNetMessage::Hello {
        runtime_id,
        transport_profiles,
        supports_schema_catalog,
        ..
    } = message
    else {
        anyhow::bail!("RUDP schema message did not decode as hello");
    };
    assert_eq!(runtime_id, "rust-rudp-client");
    assert_eq!(supports_schema_catalog, Some(true));
    assert_eq!(
        transport_profiles.expect("hello advertises transport")[0].transports[0].protocol,
        CultNetTransportProtocol::Rudp
    );
    assert_eq!(
        server.profile.transports[0].protocol,
        CultNetTransportProtocol::Rudp
    );
    assert_eq!(client.stats().frames_sent, 2);
    assert_eq!(server.stats().frames_received, 2);

    Ok(())
}

#[test]
fn rudp_socket_transport_carries_fragmented_reliable_ordered_schema_frames() -> Result<()> {
    let server_socket = bind_udp_socket()?;
    let client_socket = bind_udp_socket()?;
    let server_addr = server_socket.local_addr()?;
    let connection_id = 0x1020_3041;
    let mut server =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "rust-rudp-fragment-server".to_string(),
            socket: server_socket,
            mode: CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id,
            initial_sequence: 100,
            resend_delay_ms: 25,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: Some(8),
            max_pending_reliable_packets: None,
            reconnect_policy: None,
        })?;
    let mut client =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "rust-rudp-fragment-client".to_string(),
            socket: client_socket,
            mode: CultNetRudpSocketMode::Client,
            remote_addr: Some(server_addr),
            connection_id,
            initial_sequence: 1,
            resend_delay_ms: 25,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: Some(8),
            max_pending_reliable_packets: None,
            reconnect_policy: None,
        })?;

    client.connect(b"join".to_vec())?;
    pump_rudp_handshake(&mut client, &mut server)?;

    let payload = b"this-schema-frame-is-larger-than-one-rudp-fragment".to_vec();
    client.send("schema", payload.clone())?;
    let server_frame = receive_rudp_frame(&mut server)?;
    assert_eq!(server_frame.channel_id, "schema");
    assert_eq!(server_frame.payload, payload);
    assert_eq!(client.stats().frames_sent, 1);
    assert_eq!(server.stats().frames_received, 1);
    Ok(())
}

#[test]
fn cultmesh_facade_creates_rudp_client_from_peer_endpoint() -> Result<()> {
    let connection_id = 0x2030_4050;
    let options = CultMeshRudpSocketOptions {
        resend_delay_ms: 25,
        max_fragment_bytes: Some(8),
        max_pending_reliable_packets: Some(16),
        ..CultMeshRudpSocketOptions::default()
    };
    let mut server =
        CultMesh::create_rudp_server("rust-cultmesh-server", connection_id, options.clone())?;
    let server_port = server.profile.transports[0]
        .port
        .expect("RUDP server profile advertises its local port");
    let endpoint = CultMesh::parse_rudp_endpoint(&format!("rudp://127.0.0.1:{server_port}"))?;
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, server_port);
    assert_eq!(
        CultMesh::parse_rudp_endpoint("RuDp://localhost:4100")?.host,
        "localhost"
    );
    assert_eq!(
        CultMesh::parse_rudp_endpoint("rudp://[::1]:4100")?.uri(),
        "rudp://[::1]:4100"
    );

    let peer = CultMeshPeerCard::new("rust-cultmesh-server", "local", [endpoint.uri()])
        .with_roles(["schema"])
        .with_authority_lease_id("lease:rust-cultmesh-server");
    let mut peers = CultMesh::create_peer_catalog();
    peers.upsert(peer.clone())?;
    assert_eq!(peers.find("local", Some("schema")), vec![peer.clone()]);
    let now = Utc::now();
    let mut leases = CultMesh::create_authority_lease_catalog();
    assert!(
        CultMesh::create_rudp_client_for_authorized_peer(
            "rust-cultmesh-client",
            connection_id,
            &peers,
            &leases,
            "local",
            "schema",
            None,
            now,
            options.clone(),
        )
        .is_err()
    );
    leases.upsert(CultMeshAuthorityLease {
        lease_id: "lease:rust-cultmesh-server".to_string(),
        verse_id: "local".to_string(),
        peer_id: "rust-cultmesh-server".to_string(),
        roles: vec!["schema".to_string()],
        shard_ids: Vec::new(),
        issuer_runtime_id: Some("odin".to_string()),
        valid_from: now - Duration::minutes(1),
        expires_at: now + Duration::minutes(1),
    })?;

    let mut client = CultMesh::create_rudp_client_for_authorized_peer(
        "rust-cultmesh-client",
        connection_id,
        &peers,
        &leases,
        "local",
        "schema",
        None,
        now,
        options,
    )?;
    client.connect(b"join".to_vec())?;
    pump_rudp_handshake(&mut client, &mut server)?;

    client.send("schema", b"client-state".to_vec())?;
    let server_frame = receive_rudp_frame(&mut server)?;
    assert_eq!(server_frame.channel_id, "schema");
    assert_eq!(server_frame.payload, b"client-state");
    Ok(())
}

#[test]
fn cultmesh_facade_connects_authorized_rudp_client_for_schema_messages() -> Result<()> {
    let connection_id = 0x2030_4051;
    let options = CultMeshRudpSocketOptions {
        resend_delay_ms: 25,
        max_fragment_bytes: Some(1024),
        max_pending_reliable_packets: Some(16),
        ..CultMeshRudpSocketOptions::default()
    };
    let server = Arc::new(Mutex::new(CultMesh::create_rudp_server(
        "rust-cultmesh-connected-server",
        connection_id,
        options.clone(),
    )?));
    let server_port = server
        .lock()
        .expect("RUDP server mutex poisoned")
        .profile
        .transports[0]
        .port
        .expect("RUDP server profile advertises its local port");
    let endpoint = CultMesh::parse_rudp_endpoint(&format!("rudp://127.0.0.1:{server_port}"))?;
    let peer = CultMeshPeerCard::new("rust-cultmesh-connected-server", "local", [endpoint.uri()])
        .with_roles(["schema"])
        .with_authority_lease_id("lease:rust-cultmesh-connected-server");
    let mut peers = CultMesh::create_peer_catalog();
    peers.upsert(peer)?;
    let now = Utc::now();
    let mut leases = CultMesh::create_authority_lease_catalog();
    leases.upsert(CultMeshAuthorityLease {
        lease_id: "lease:rust-cultmesh-connected-server".to_string(),
        verse_id: "local".to_string(),
        peer_id: "rust-cultmesh-connected-server".to_string(),
        roles: vec!["schema".to_string()],
        shard_ids: Vec::new(),
        issuer_runtime_id: Some("odin".to_string()),
        valid_from: now - Duration::minutes(1),
        expires_at: now + Duration::minutes(1),
    })?;

    let done = Arc::new(AtomicBool::new(false));
    let pump = pump_rudp_server_until_connected(server.clone(), done.clone());
    let mut client = CultMesh::connect_rudp_client_for_authorized_peer(
        "rust-cultmesh-connected-client",
        connection_id,
        &peers,
        &leases,
        "local",
        "schema",
        None,
        now,
        CultMeshRudpClientOptions {
            socket_options: options,
            connect_payload: b"join".to_vec(),
            connect_timeout: StdDuration::from_secs(1),
            poll_interval: StdDuration::from_millis(5),
        },
    )?;
    done.store(true, Ordering::SeqCst);
    pump.join()
        .map_err(|_| anyhow::anyhow!("RUDP server pump panicked"))??;

    assert!(client.connected());
    assert!(
        server
            .lock()
            .expect("RUDP server mutex poisoned")
            .connected()
    );
    client.send_schema_message(&CultNetMessage::SchemaCatalogRequest {
        message_id: "rust-cultmesh-rudp-schema-catalog".to_string(),
        include_schema_json: Some(true),
        schema_ids: None,
        kinds: Some(vec![CultNetSchemaKind::DocumentPayload]),
    })?;
    let request = {
        let mut server = server.lock().expect("RUDP server mutex poisoned");
        receive_rudp_schema_message(&mut server)?
    };
    let CultNetMessage::SchemaCatalogRequest {
        message_id, kinds, ..
    } = request
    else {
        anyhow::bail!("RUDP schema message did not decode as schema catalog request");
    };
    assert_eq!(message_id, "rust-cultmesh-rudp-schema-catalog");
    assert_eq!(kinds, Some(vec![CultNetSchemaKind::DocumentPayload]));
    assert_eq!(
        client.profile.transports[0].protocol,
        CultNetTransportProtocol::Rudp
    );
    Ok(())
}

#[test]
fn cultmesh_facade_exposes_schema_registry_and_shard_catalog_owners() -> Result<()> {
    let mut schemas = CultMesh::create_schema_registry();
    schemas.register(CultNetSchemaRegistration {
        schema_id: "rust.cultmesh.note.v1".to_string(),
        kind: CultNetSchemaKind::DocumentPayload,
        wire_contracts: vec![CultNetWireContract::CultNetSchemaV0],
        schema_version: Some("rust.cultmesh.note.v1".to_string()),
        document_type: Some("rust.cultmesh.note".to_string()),
        title: Some("Rust CultMesh Note".to_string()),
        schema_json: Some(
            r#"{"$id":"rust.cultmesh.note.v1","type":"object","properties":{"body":{"type":"string"}}}"#
                .to_string(),
        ),
    })?;
    assert_eq!(
        schemas
            .get("rust.cultmesh.note.v1", false)
            .expect("registered schema is discoverable")
            .document_type
            .as_deref(),
        Some("rust.cultmesh.note")
    );

    let builtins = CultMesh::create_builtin_schema_registry()?;
    let shared_contracts = builtins.list(&cultnet_rs::CultNetSchemaCatalogOptions {
        include_schema_json: false,
        schema_ids: None,
        kinds: Some(vec![CultNetSchemaKind::SharedContract]),
    });
    assert!(shared_contracts.iter().any(|schema| {
        schema.schema_id
            == "https://github.com/GameCult/cultnet-ts/contracts/cultnet.transport-profile.schema.json"
            && schema.schema_version.as_deref() == Some("cultnet.transport_profile.v0")
    }));

    let mut shards = CultMesh::create_shard_catalog();
    shards.upsert(CultNetShardDescriptor {
        shard_id: "rust-notes-a".to_string(),
        owner_runtime_id: "rust-cultmesh".to_string(),
        epoch: 7,
        is_primary: Some(true),
        schema_ids: vec!["rust.cultmesh.note.v1".to_string()],
        key_prefix: Some("note:".to_string()),
        primary_endpoints: vec!["rudp://127.0.0.1:4100".to_string()],
        replica_endpoints: Vec::new(),
        read_replica_endpoints: Vec::new(),
        region: None,
        authority_lease_id: Some("lease:rust-notes-a".to_string()),
    })?;
    let matching = shards.list(&cultnet_rs::CultNetShardCatalogOptions {
        schema_ids: Some(vec!["rust.cultmesh.note.v1".to_string()]),
        record_keys: Some(vec!["note:1".to_string()]),
    });
    assert_eq!(matching.len(), 1);
    assert_eq!(matching[0].shard_id, "rust-notes-a");
    assert!(shards.get("rust-notes-a").is_some());
    Ok(())
}

#[test]
fn cultmesh_authority_leases_gate_peer_contact_hints() -> Result<()> {
    let now = Utc::now();
    let peer = CultMeshPeerCard::new("rust-peer", "public", ["rudp://127.0.0.1:4100"])
        .with_roles(["shard-primary"])
        .with_authority_lease_id("lease:rust-peer");
    let mut leases = CultMesh::create_authority_lease_catalog();
    let mut peers = CultMesh::create_peer_catalog();
    peers.upsert(peer.clone())?;

    assert!(!leases.is_authorized(&peer, "shard-primary", Some("players"), now));
    assert!(
        peers
            .find_authorized("public", "shard-primary", &leases, Some("players"), now)
            .is_empty()
    );
    leases.upsert(CultMeshAuthorityLease {
        lease_id: "lease:rust-peer".to_string(),
        verse_id: "public".to_string(),
        peer_id: "rust-peer".to_string(),
        roles: vec!["shard-primary".to_string()],
        shard_ids: vec!["players".to_string()],
        issuer_runtime_id: Some("odin".to_string()),
        valid_from: now - Duration::minutes(1),
        expires_at: now + Duration::minutes(1),
    })?;

    assert!(leases.is_authorized(&peer, "shard-primary", Some("players"), now));
    assert_eq!(
        peers.find_authorized("public", "shard-primary", &leases, Some("players"), now),
        vec![peer.clone()]
    );
    assert_eq!(
        peers.first_authorized("public", "shard-primary", &leases, Some("players"), now),
        Some(peer.clone())
    );
    assert!(!leases.is_authorized(&peer, "schema", Some("players"), now));
    assert!(
        peers
            .first_authorized("public", "schema", &leases, Some("players"), now)
            .is_none()
    );
    assert!(!leases.is_authorized(&peer, "shard-primary", Some("inventory"), now));
    assert!(!leases.is_authorized(
        &peer,
        "shard-primary",
        Some("players"),
        now + Duration::minutes(1)
    ));
    Ok(())
}

#[test]
fn legacy_gamecult_networking_contract_round_trips_login_union() -> Result<()> {
    let message = CultNetMessage::Login {
        nonce: CultNetSecret::to_base64_url(b"nonce"),
        auth: CultNetSecret::to_base64_url(b"auth"),
        password: CultNetSecret::to_base64_url(b"password"),
    };
    let wire =
        encode_cultnet_message_for_wire(&message, CultNetWireContract::GameCultNetworkingV0)?;
    let items = wire.as_array().expect("legacy union is an array");
    assert_eq!(items[0].as_i64(), Some(0));
    assert_eq!(
        items[1].as_array().unwrap()[0].as_slice(),
        Some(&b"nonce"[..])
    );

    let bytes = rmp_serde::to_vec(&wire)?;
    let decoded =
        decode_cultnet_message_from_slice(&bytes, CultNetWireContract::GameCultNetworkingV0)?;
    assert_eq!(decoded, message);
    Ok(())
}

#[test]
fn rust_decodes_typescript_generated_cultnet_frames() -> Result<()> {
    let mut framer = LengthPrefixedMessageFramer::new();
    let hello_frames = framer.push(TS_HELLO_FRAME);
    assert_eq!(hello_frames.len(), 1);
    let hello =
        decode_cultnet_message_from_slice(&hello_frames[0], CultNetWireContract::CultNetSchemaV0)?;
    assert_eq!(
        hello,
        CultNetMessage::Hello {
            runtime_id: "voidbot-main".to_string(),
            runtime_kind: "node-worker".to_string(),
            agent_id: Some("void".to_string()),
            role: None,
            display_name: Some("Void".to_string()),
            supported_document_types: Some(vec!["ghostlight.agent-state".to_string()]),
            supported_mutation_contracts: None,
            supported_message_versions: None,
            transport_profiles: None,
            supports_schema_catalog: None,
        }
    );

    let mut framer = LengthPrefixedMessageFramer::new();
    let login_frames = framer.push(TS_LEGACY_LOGIN_FRAME);
    assert_eq!(login_frames.len(), 1);
    let login = decode_cultnet_message_from_slice(
        &login_frames[0],
        CultNetWireContract::GameCultNetworkingV0,
    )?;
    assert_eq!(
        login,
        CultNetMessage::Login {
            nonce: "bm9uY2U".to_string(),
            auth: "YXV0aA".to_string(),
            password: "cGFzc3dvcmQ".to_string(),
        }
    );
    Ok(())
}

#[test]
fn document_registry_advertises_mutation_contracts_with_bindings() -> Result<()> {
    let mut registry = CultNetDocumentRegistry::new();
    let contract = CultNetDocumentMutationContract {
        document_type: "ghostlight.agent-state".to_string(),
        payload_schema_version: Some("ghostlight.agent_state.v0".to_string()),
        operations: vec![
            CultNetDocumentOperation::Snapshot,
            CultNetDocumentOperation::IntentSubmit,
            CultNetDocumentOperation::ReceiptWatch,
        ],
        authority: CultNetMutationAuthority::Coordinator,
        intent_document_types: Some(vec!["ghostlight.agent-state.intent.v0".to_string()]),
        receipt_document_types: Some(vec!["ghostlight.agent-state.receipt.v0".to_string()]),
        notes: Some(vec![
            "Intent documents are reviewed before cache mutation.".to_string(),
        ]),
    };
    registry.register(
        CultNetDocumentBinding::for_entry::<GhostlightAgentStateFixture>(
            "ghostlight.agent_state.v0".to_string(),
        )
        .with_mutation_contract(contract.clone()),
    );

    assert_eq!(registry.mutation_contracts(), vec![contract]);
    Ok(())
}

#[test]
fn document_registry_replicates_typed_cultcache_state() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let origin_store = temp.path().join("origin.msgpack");
    let target_store = temp.path().join("target.msgpack");
    let payload = GhostlightAgentStateFixture {
        schema_version: "ghostlight.agent_state.v0".to_string(),
        agent_id: "epiphany.persona".to_string(),
        display_name: "Persona".to_string(),
    };

    let mut registry = CultNetDocumentRegistry::new();
    registry.register(CultNetDocumentBinding::for_entry::<
        GhostlightAgentStateFixture,
    >("ghostlight.agent_state.v0".to_string()));

    let mut origin = CultCache::new();
    origin.register_entry_type::<GhostlightAgentStateFixture>()?;
    origin.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&origin_store));
    origin.pull_all_backing_stores()?;
    origin.put("epiphany.persona", &payload)?;

    let snapshot = registry.create_snapshot_response(&origin, "snapshot-1", None, None)?;

    let mut target = CultCache::new();
    target.register_entry_type::<GhostlightAgentStateFixture>()?;
    target.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&target_store));
    target.pull_all_backing_stores()?;
    let applied =
        registry.apply_snapshot_response::<GhostlightAgentStateFixture>(&mut target, &snapshot)?;
    let synced = registry.sync_document_from_snapshot_response::<GhostlightAgentStateFixture>(
        &mut target,
        &snapshot,
        "epiphany.persona",
    )?;
    assert_eq!(applied, vec![payload.clone()]);
    assert_eq!(synced, payload);
    assert_eq!(
        target.get_required::<GhostlightAgentStateFixture>("epiphany.persona")?,
        payload
    );

    let direct_put = registry.create_document_put_message(
        "put-1",
        "epiphany.persona",
        &GhostlightAgentStateFixture {
            display_name: "Persona Prime".to_string(),
            ..payload
        },
        CultNetDocumentPutOptions::default(),
    )?;
    let updated = registry
        .apply_document_put_message::<GhostlightAgentStateFixture>(&mut target, &direct_put)?;
    assert_eq!(updated.display_name, "Persona Prime");
    Ok(())
}

#[test]
fn raw_snapshot_replication_preserves_messagepack_payload_bytes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let origin_store = temp.path().join("origin-raw.msgpack");
    let target_store = temp.path().join("target-raw.msgpack");
    let payload = GhostlightAgentStateFixture {
        schema_version: "ghostlight.agent_state.v0".to_string(),
        agent_id: "epiphany.persona".to_string(),
        display_name: "Persona".to_string(),
    };

    let mut registry = CultNetDocumentRegistry::new();
    registry.register(CultNetDocumentBinding::for_entry::<
        GhostlightAgentStateFixture,
    >("ghostlight.agent_state.v0".to_string()));

    let mut origin = CultCache::new();
    origin.register_entry_type::<GhostlightAgentStateFixture>()?;
    origin.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&origin_store));
    origin.pull_all_backing_stores()?;
    origin.put("epiphany.persona", &payload)?;

    let raw_snapshot =
        registry.create_raw_snapshot_response(&origin, "raw-snapshot-1", None, None)?;
    let source_envelope =
        origin.get_required_envelope::<GhostlightAgentStateFixture>("epiphany.persona")?;

    let mut target = CultCache::new();
    target.register_entry_type::<GhostlightAgentStateFixture>()?;
    target.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&target_store));
    target.pull_all_backing_stores()?;
    let applied = registry
        .apply_raw_snapshot_response::<GhostlightAgentStateFixture>(&mut target, &raw_snapshot)?;
    let target_envelope =
        target.get_required_envelope::<GhostlightAgentStateFixture>("epiphany.persona")?;

    assert_eq!(applied, vec![payload.clone()]);
    assert_eq!(target_envelope.payload, source_envelope.payload);
    assert_eq!(
        target.get_required::<GhostlightAgentStateFixture>("epiphany.persona")?,
        payload
    );
    Ok(())
}

#[test]
fn raw_snapshot_replication_hydrates_same_schema_rust_aliases() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let origin_store = temp.path().join("origin-alias.msgpack");
    let target_store = temp.path().join("target-alias.msgpack");
    let canonical = GhostlightAgentStateFixture {
        schema_version: "ghostlight.agent_state.v0".to_string(),
        agent_id: "epiphany.persona".to_string(),
        display_name: "Persona".to_string(),
    };

    let mut origin_registry = CultNetDocumentRegistry::new();
    origin_registry.register(CultNetDocumentBinding::for_entry_with_schema_id::<
        GhostlightAgentStateFixture,
    >(
        "ghostlight.agent_state.v0".to_string(),
        "ghostlight.agent_state.v0".to_string(),
    ));
    let mut origin = CultCache::new();
    origin.register_entry_type::<GhostlightAgentStateFixture>()?;
    origin.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&origin_store));
    origin.pull_all_backing_stores()?;
    origin.put("epiphany.persona", &canonical)?;
    let raw_snapshot = origin_registry.create_raw_snapshot_response(
        &origin,
        "raw-snapshot-alias",
        Some(&["ghostlight.agent_state.v0".to_string()]),
        Some(&["epiphany.persona".to_string()]),
    )?;

    let mut alias_registry = CultNetDocumentRegistry::new();
    alias_registry.register(CultNetDocumentBinding::for_entry_with_schema_id::<
        GhostlightAgentStateUiFixture,
    >(
        "ghostlight.agent_state.v0".to_string(),
        "ghostlight.agent_state.v0".to_string(),
    ));
    let mut target = CultCache::new();
    target.register_entry_type::<GhostlightAgentStateUiFixture>()?;
    target.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&target_store));
    target.pull_all_backing_stores()?;

    let applied = alias_registry
        .sync_raw_document_from_snapshot_response::<GhostlightAgentStateUiFixture>(
            &mut target,
            &raw_snapshot,
            "epiphany.persona",
        )?;

    assert_eq!(
        applied,
        GhostlightAgentStateUiFixture {
            schema_version: "ghostlight.agent_state.v0".to_string(),
            agent_id: "epiphany.persona".to_string(),
            display_name: "Persona".to_string(),
        }
    );
    assert_eq!(
        target.get_required::<GhostlightAgentStateUiFixture>("epiphany.persona")?,
        applied
    );
    Ok(())
}

#[test]
fn reactive_document_coalesces_direct_same_schema_alias_member_writes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let target_store = temp.path().join("target-reactive-alias.msgpack");
    let mut registry = CultNetDocumentRegistry::new();
    registry.register(CultNetDocumentBinding::for_entry_with_schema_id::<
        GhostlightReactiveNoteUiFixture,
    >(
        "ghostlight.reactive_note.v0".to_string(),
        "ghostlight.reactive_note.v0".to_string(),
    ));
    let cache = Arc::new(Mutex::new(CultCache::new()));
    {
        let mut cache = cache.lock().expect("cache mutex");
        cache.register_entry_type::<GhostlightReactiveNoteUiFixture>()?;
        cache.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&target_store));
        cache.pull_all_backing_stores()?;
        cache.put(
            "note:reactive",
            &GhostlightReactiveNoteUiFixture {
                body: "initial".to_string(),
                revision: 1,
            },
        )?;
    }

    let reactive = registry.reactive_document::<GhostlightReactiveNoteUiFixture>(
        Arc::clone(&cache),
        "note:reactive",
        CultNetReactiveDocumentOptions {
            flush_delay: StdDuration::from_millis(5),
            ..CultNetReactiveDocumentOptions::default()
        },
    )?;
    {
        let current = reactive.current();
        let mut current = current.lock().expect("reactive current mutex");
        current.body = "first-local-edit".to_string();
        current.body = "second-local-edit".to_string();
        current.revision = 2;
    }

    for _ in 0..30 {
        let body = cache
            .lock()
            .expect("cache mutex")
            .get_required::<GhostlightReactiveNoteUiFixture>("note:reactive")?
            .body;
        if body == "second-local-edit" {
            break;
        }
        thread::sleep(StdDuration::from_millis(5));
    }

    let stored = cache
        .lock()
        .expect("cache mutex")
        .get_required::<GhostlightReactiveNoteUiFixture>("note:reactive")?;
    assert_eq!(stored.body, "second-local-edit");
    assert_eq!(stored.revision, 2);
    assert!(!reactive.is_dirty());
    assert!(reactive.last_error().is_none());
    Ok(())
}

#[test]
fn reactive_document_tracks_canonical_reconciliation_delta() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let target_store = temp.path().join("target-reactive-reconcile.msgpack");
    let mut registry = CultNetDocumentRegistry::new();
    registry.register(CultNetDocumentBinding::for_entry_with_schema_id::<
        GhostlightReactiveNoteFixture,
    >(
        "ghostlight.reactive_note.v0".to_string(),
        "ghostlight.reactive_note.v0".to_string(),
    ));
    let cache = Arc::new(Mutex::new(CultCache::new()));
    {
        let mut cache = cache.lock().expect("cache mutex");
        cache.register_entry_type::<GhostlightReactiveNoteFixture>()?;
        cache.add_generic_backing_store(SingleFileMessagePackBackingStore::new(&target_store));
        cache.pull_all_backing_stores()?;
        cache.put(
            "note:reconcile",
            &GhostlightReactiveNoteFixture {
                body: "initial".to_string(),
                revision: 1,
            },
        )?;
    }

    let reactive = registry.reactive_document::<GhostlightReactiveNoteFixture>(
        Arc::clone(&cache),
        "note:reconcile",
        CultNetReactiveDocumentOptions {
            flush_delay: StdDuration::from_secs(60),
            detect_local_changes: false,
            ..CultNetReactiveDocumentOptions::default()
        },
    )?;
    reactive.update(|note| {
        note.body = "local-prediction".to_string();
        note.revision = 2;
    })?;
    let authoritative = registry.create_document_put_message(
        "put-authoritative",
        "note:reconcile",
        &GhostlightReactiveNoteFixture {
            body: "authoritative".to_string(),
            revision: 7,
        },
        CultNetDocumentPutOptions::default(),
    )?;

    reactive.apply_document_put_message(&authoritative)?;

    let current = reactive.current();
    let current = current.lock().expect("reactive current mutex").clone();
    assert_eq!(current.body, "local-prediction");
    let reconciliation = reactive
        .reconciliation()
        .expect("dirty canonical apply records reconciliation");
    assert_eq!(reconciliation.canonical.body, "authoritative");
    assert_eq!(reconciliation.predicted.body, "local-prediction");
    assert_eq!(
        reconciliation.delta.get("0"),
        Some(&serde_json::json!("local-prediction"))
    );
    assert_eq!(
        reconciliation.delta.get("1"),
        Some(&serde_json::json!(-5.0))
    );

    reactive.flush()?;
    let stored = cache
        .lock()
        .expect("cache mutex")
        .get_required::<GhostlightReactiveNoteFixture>("note:reconcile")?;
    assert_eq!(stored.body, "local-prediction");
    assert!(reactive.reconciliation().is_none());
    Ok(())
}

#[test]
fn client_security_keeps_the_connection_key_visible_without_exposing_cipher_logic() -> Result<()> {
    let options = CultNetClientSecurityOptions::development();
    assert_eq!(options.connection_key, "gamecult-dev-connection-key");
    assert_ne!(options.encryption_key(), [0_u8; 32]);
    Ok(())
}

#[test]
fn builtin_schema_registry_advertises_canonical_ghostlight_schema_without_inline_body_by_default()
-> Result<()> {
    let registry = builtin_schema_registry()?;
    let response = registry.create_catalog_response(&CultNetMessage::SchemaCatalogRequest {
        message_id: "catalog-1".to_string(),
        include_schema_json: None,
        schema_ids: None,
        kinds: None,
    })?;

    let CultNetMessage::SchemaCatalogResponse { schemas, .. } = response else {
        panic!("expected catalog response");
    };

    let ghostlight = schemas
        .iter()
        .find(|schema| schema.document_type.as_deref() == Some("ghostlight.agent-state"))
        .expect("ghostlight agent-state schema is advertised");

    assert_eq!(ghostlight.kind, CultNetSchemaKind::DocumentPayload);
    assert_eq!(
        ghostlight.schema_version.as_deref(),
        Some("ghostlight.agent_state.v0")
    );
    assert_eq!(ghostlight.schema_json, None);
    assert!(!ghostlight.content_hash.is_empty());

    let transport_profile = schemas
        .iter()
        .find(|schema| schema.schema_version.as_deref() == Some("cultnet.transport_profile.v0"))
        .expect("transport profile schema is advertised");

    assert_eq!(transport_profile.kind, CultNetSchemaKind::SharedContract);
    assert_eq!(
        transport_profile.schema_id,
        "https://github.com/GameCult/cultnet-ts/contracts/cultnet.transport-profile.schema.json"
    );
    assert_eq!(
        transport_profile.wire_contracts,
        vec![CultNetWireContract::CultNetSchemaV0]
    );
    Ok(())
}

#[test]
fn builtin_schema_registry_advertises_shard_catalog_wire_messages() -> Result<()> {
    let registry = builtin_schema_registry()?;
    let response = registry.create_catalog_response(&CultNetMessage::SchemaCatalogRequest {
        message_id: "catalog-shards".to_string(),
        include_schema_json: None,
        schema_ids: None,
        kinds: Some(vec![CultNetSchemaKind::WireMessage]),
    })?;

    let CultNetMessage::SchemaCatalogResponse { schemas, .. } = response else {
        panic!("expected catalog response");
    };

    assert!(schemas.iter().any(|schema| {
        schema.schema_version.as_deref() == Some("cultnet.shard_catalog_request.v0")
    }));
    assert!(schemas.iter().any(|schema| {
        schema.schema_version.as_deref() == Some("cultnet.shard_catalog_response.v0")
    }));
    Ok(())
}

#[test]
fn shard_catalog_filters_descriptors_and_applies_remote_responses() -> Result<()> {
    let mut catalog = CultNetShardCatalog::new();
    catalog.upsert(CultNetShardDescriptor {
        shard_id: "notes-a".to_string(),
        owner_runtime_id: "rust-primary".to_string(),
        epoch: 3,
        is_primary: Some(true),
        schema_ids: vec!["note.v0".to_string()],
        key_prefix: Some("note:".to_string()),
        primary_endpoints: vec!["rudp://127.0.0.1:4100".to_string()],
        replica_endpoints: vec![],
        read_replica_endpoints: vec![],
        region: Some("local".to_string()),
        authority_lease_id: Some("lease:notes-a".to_string()),
    })?;
    catalog.upsert(CultNetShardDescriptor {
        shard_id: "players-a".to_string(),
        owner_runtime_id: "rust-primary".to_string(),
        epoch: 1,
        is_primary: Some(false),
        schema_ids: vec!["player.v0".to_string()],
        key_prefix: Some("player:".to_string()),
        primary_endpoints: vec!["cultnet://127.0.0.1:3075".to_string()],
        replica_endpoints: vec![],
        read_replica_endpoints: vec![],
        region: None,
        authority_lease_id: None,
    })?;

    let response = catalog.create_catalog_response(&CultNetMessage::ShardCatalogRequest {
        message_id: "shards".to_string(),
        schema_ids: Some(vec!["note.v0".to_string()]),
        record_keys: Some(vec!["note:1".to_string()]),
    })?;

    let CultNetMessage::ShardCatalogResponse { message_id, shards } = &response else {
        panic!("expected shard catalog response");
    };
    assert_eq!(message_id, "shards");
    assert_eq!(shards.len(), 1);
    assert_eq!(shards[0].shard_id, "notes-a");

    let bytes = encode_cultnet_message_to_vec(&response, CultNetWireContract::CultNetSchemaV0)?;
    let decoded = decode_cultnet_message_from_slice(&bytes, CultNetWireContract::CultNetSchemaV0)?;
    assert_eq!(decoded, response);

    let mut applied = CultNetShardCatalog::new();
    let applied_shards = applied.apply_response(&decoded)?;
    assert_eq!(applied_shards.len(), 1);
    assert!(applied.get("notes-a").is_some_and(|shard| {
        shard.serves(Some("note.v0"), Some("note:2"))
            && !shard.serves(Some("note.v0"), Some("player:2"))
    }));
    Ok(())
}

#[test]
fn schema_discovery_round_trips_over_legacy_gamecult_contract_when_inline_schemas_are_requested()
-> Result<()> {
    let registry = {
        let mut registry = CultNetSchemaRegistry::new();
        registry.register(cultnet_rs::CultNetSchemaRegistration {
            schema_id: "https://example.test/contracts/example.schema.json".to_string(),
            kind: CultNetSchemaKind::SharedContract,
            wire_contracts: vec![
                CultNetWireContract::CultNetSchemaV0,
                CultNetWireContract::GameCultNetworkingV0,
            ],
            schema_version: None,
            document_type: None,
            title: Some("Example Schema".to_string()),
            schema_json: Some(
                r#"{
                    "$schema":"https://json-schema.org/draft/2020-12/schema",
                    "$id":"https://example.test/contracts/example.schema.json",
                    "title":"Example Schema",
                    "type":"object",
                    "properties":{"value":{"type":"string"}},
                    "required":["value"],
                    "additionalProperties":false
                }"#
                .to_string(),
            ),
        })?;
        registry
    };

    let response = registry.create_catalog_response(&CultNetMessage::SchemaCatalogRequest {
        message_id: "catalog-legacy".to_string(),
        include_schema_json: Some(true),
        schema_ids: None,
        kinds: None,
    })?;
    let wire =
        encode_cultnet_message_for_wire(&response, CultNetWireContract::GameCultNetworkingV0)?;
    let bytes = rmp_serde::to_vec(&wire)?;
    let decoded =
        decode_cultnet_message_from_slice(&bytes, CultNetWireContract::GameCultNetworkingV0)?;

    let CultNetMessage::SchemaCatalogResponse {
        message_id,
        schemas,
    } = decoded
    else {
        panic!("expected legacy schema catalog response");
    };

    assert_eq!(message_id, "catalog-legacy");
    assert_eq!(
        schemas[0].schema_id,
        "https://example.test/contracts/example.schema.json"
    );
    assert!(
        schemas[0]
            .schema_json
            .as_deref()
            .is_some_and(|schema| schema.contains("\"value\""))
    );
    Ok(())
}

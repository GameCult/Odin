use anyhow::Result;
use chrono::Duration;
use cultcache_rs::CultCache;
use cultcache_rs::DatabaseEntry;
use cultcache_rs::SingleFileMessagePackBackingStore;
use cultnet_rs::CultNetClientSecurityOptions;
use cultnet_rs::CultNetDocumentBinding;
use cultnet_rs::CultNetDocumentMutationContract;
use cultnet_rs::CultNetDocumentOperation;
use cultnet_rs::CultNetDocumentPutOptions;
use cultnet_rs::CultNetDocumentRegistry;
use cultnet_rs::CultNetMessage;
use cultnet_rs::CultNetMutationAuthority;
use cultnet_rs::CultNetRudpPacket;
use cultnet_rs::CultNetRudpPacketType;
use cultnet_rs::CultNetRudpSendOptions;
use cultnet_rs::CultNetRudpSession;
use cultnet_rs::CultNetRudpSessionOptions;
use cultnet_rs::CultNetRudpSocketMode;
use cultnet_rs::CultNetRudpSocketTransportConnection;
use cultnet_rs::CultNetRudpSocketTransportOptions;
use cultnet_rs::CultNetSchemaKind;
use cultnet_rs::CultNetSchemaRegistry;
use cultnet_rs::CultNetSecret;
use cultnet_rs::CultNetServerSecurityOptions;
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
use cultnet_rs::create_rudp_transport_profile;
use cultnet_rs::create_tcp_framed_transport_profile;
use cultnet_rs::decode_cultnet_message_from_slice;
use cultnet_rs::decode_rudp_packet;
use cultnet_rs::encode_cultnet_message_for_wire;
use cultnet_rs::encode_cultnet_message_to_vec;
use cultnet_rs::encode_frame;
use cultnet_rs::encode_rudp_packet;
use pretty_assertions::assert_eq;
use std::net::UdpSocket;
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
                channels: vec![CultNetTransportChannel {
                    channel_id: "schema".to_string(),
                    delivery: CultNetTransportDelivery::Reliable,
                    ordering: CultNetTransportOrdering::Ordered,
                    max_payload_bytes: None,
                    max_fragment_bytes: None,
                    max_pending_reliable_packets: None,
                    reliable_expire_after_ms: None,
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
        },
    );

    assert_eq!(
        profile.transports[0].protocol,
        CultNetTransportProtocol::Rudp
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
                channel.reliable_expire_after_ms,
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
                Some(64),
                None
            ),
            (
                "latest",
                CultNetTransportDelivery::Unreliable,
                CultNetTransportOrdering::Sequenced,
                Some(64),
                None
            ),
            (
                "realtime",
                CultNetTransportDelivery::Unreliable,
                CultNetTransportOrdering::Unordered,
                Some(64),
                None
            ),
            (
                "media",
                CultNetTransportDelivery::Reliable,
                CultNetTransportOrdering::Unordered,
                Some(64),
                Some(75)
            ),
        ]
    );
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
fn rudp_session_expires_bounded_reliable_media_resends() -> Result<()> {
    let mut session = CultNetRudpSession::new(CultNetRudpSessionOptions {
        connection_id: 100,
        initial_sequence: 1,
        resend_delay_ms: 20,
        max_pending_reliable_packets: Some(1),
    });
    session.receive(
        &CultNetRudpPacket {
            packet_type: CultNetRudpPacketType::Accept,
            connection_id: 100,
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
        "media",
        b"frame-chunk".to_vec(),
        CultNetRudpSendOptions {
            reliable: true,
            ordered: false,
            now_ms: 10,
            reliable_expire_after_ms: Some(50),
            ..CultNetRudpSendOptions::default()
        },
    )?;

    assert_eq!(session.pending_reliable_sequences(), vec![sent.sequence]);
    assert!(session.due_resends(25).is_empty());
    assert_eq!(
        session
            .due_resends(35)
            .iter()
            .map(|packet| packet.sequence)
            .collect::<Vec<_>>(),
        vec![sent.sequence]
    );
    assert!(session.due_resends(61).is_empty());
    assert!(session.pending_reliable_sequences().is_empty());
    assert_eq!(session.reliable_packets_expired(), 1);

    session.send(
        "media",
        b"fresh-frame-chunk".to_vec(),
        CultNetRudpSendOptions {
            reliable: true,
            ordered: false,
            now_ms: 62,
            reliable_expire_after_ms: Some(50),
            ..CultNetRudpSendOptions::default()
        },
    )?;
    assert_eq!(session.reliable_packets_expired(), 1);
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
    assert_eq!(
        server.profile.transports[0].protocol,
        CultNetTransportProtocol::Rudp
    );
    assert_eq!(client.stats().frames_sent, 1);
    assert_eq!(server.stats().frames_received, 1);

    Ok(())
}

#[test]
fn rudp_socket_transport_reports_expired_reliable_media_packets() -> Result<()> {
    let server_socket = bind_udp_socket()?;
    let client_socket = bind_udp_socket()?;
    let server_addr = server_socket.local_addr()?;
    let connection_id = 0x1020_3042;
    let mut server =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "rust-rudp-media-expiry-server".to_string(),
            socket: server_socket,
            mode: CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id,
            initial_sequence: 100,
            resend_delay_ms: 5,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
        })?;
    let mut client =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: "rust-rudp-media-expiry-client".to_string(),
            socket: client_socket,
            mode: CultNetRudpSocketMode::Client,
            remote_addr: Some(server_addr),
            connection_id,
            initial_sequence: 1,
            resend_delay_ms: 5,
            transport_id: None,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
        })?;

    client.connect(b"join".to_vec())?;
    pump_rudp_handshake(&mut client, &mut server)?;
    client.send("media", b"late-frame".to_vec())?;
    assert_eq!(client.stats().reliable_packets_expired, 0);

    thread::sleep(StdDuration::from_millis(90));
    client.poll_resends()?;

    assert_eq!(client.stats().reliable_packets_expired, 1);
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
    assert_eq!(applied, vec![payload.clone()]);
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

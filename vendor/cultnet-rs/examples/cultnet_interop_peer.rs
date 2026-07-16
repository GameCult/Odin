use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use cultcache_rs::CultCache;
use cultcache_rs::DatabaseEntry;
use cultcache_rs::SingleFileMessagePackBackingStore;
use cultnet_rs::CultNetDocumentBinding;
use cultnet_rs::CultNetDocumentMutationContract;
use cultnet_rs::CultNetDocumentOperation;
use cultnet_rs::CultNetDocumentPutOptions;
use cultnet_rs::CultNetDocumentRegistry;
use cultnet_rs::CultNetMessage;
use cultnet_rs::CultNetMutationAuthority;
use cultnet_rs::CultNetRudpPacketType;
use cultnet_rs::CultNetRudpSendOptions;
use cultnet_rs::CultNetRudpSession;
use cultnet_rs::CultNetRudpSessionOptions;
use cultnet_rs::CultNetRudpSocketTransportConnection;
use cultnet_rs::CultNetRudpSocketTransportOptions;
use cultnet_rs::CultNetSchemaKind;
use cultnet_rs::CultNetSchemaRegistration;
use cultnet_rs::CultNetSchemaRegistry;
use cultnet_rs::CultNetTransportProfile;
use cultnet_rs::CultNetWireContract;
use cultnet_rs::RudpTransportProfileOptions;
use cultnet_rs::TcpFramedTransportConnection;
use cultnet_rs::TcpFramedTransportProfileOptions;
use cultnet_rs::builtin_schema_registry;
use cultnet_rs::create_rudp_transport_profile;
use cultnet_rs::create_tcp_framed_transport_profile;
use cultnet_rs::decode_cultnet_message_from_slice;
use cultnet_rs::decode_rudp_packet;
use cultnet_rs::encode_cultnet_message_to_vec;
use cultnet_rs::encode_rudp_packet;
use serde::Deserialize;
use serde::Serialize;
use socket2::Domain;
use socket2::Protocol;
use socket2::Socket;
use socket2::Type;
use std::collections::BTreeMap;
use std::fs;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::net::SocketAddrV4;
use std::net::TcpListener;
use std::net::TcpStream;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::time::Instant;

const INTEROP_DOCUMENT_TYPE: &str = "cultnet.interop-note";
const INTEROP_SCHEMA_VERSION: &str = "cultnet.interop_note.v0";
const MUTATION_INTENT_TYPE: &str = "cultnet.interop-note-mutation-intent";
const MUTATION_INTENT_SCHEMA_ID: &str = "https://github.com/GameCult/cultnet-ts/integration/contracts/cultnet.interop-note-mutation-intent.schema.json";
const MUTATION_INTENT_SCHEMA_VERSION: &str = "cultnet.interop_note_mutation_intent.v0";
const MUTATION_RECEIPT_TYPE: &str = "cultnet.interop-note-mutation-receipt";
const MUTATION_RECEIPT_SCHEMA_ID: &str = "https://github.com/GameCult/cultnet-ts/integration/contracts/cultnet.interop-note-mutation-receipt.schema.json";
const MUTATION_RECEIPT_SCHEMA_VERSION: &str = "cultnet.interop_note_mutation_receipt.v0";
const FIRE_COMMAND_TYPE: &str = "cultnet.interop-fire-weapon-command";
const FIRE_COMMAND_SCHEMA_ID: &str = "https://github.com/GameCult/cultnet-ts/integration/contracts/cultnet.interop-fire-weapon-command.schema.json";
const FIRE_COMMAND_SCHEMA_VERSION: &str = "cultnet.interop_fire_weapon_command.v0";
const FIRE_RECEIPT_TYPE: &str = "cultnet.interop-fire-weapon-receipt";
const FIRE_RECEIPT_SCHEMA_ID: &str = "https://github.com/GameCult/cultnet-ts/integration/contracts/cultnet.interop-fire-weapon-receipt.schema.json";
const FIRE_RECEIPT_SCHEMA_VERSION: &str = "cultnet.interop_fire_weapon_receipt.v0";
const DISCOVERY_ANNOUNCE_SCHEMA_VERSION: &str = "cultnet.discovery_announce.v0";
const RUDP_INTEROP_CONNECTION_ID: u32 = 0x43554C54;
const RUDP_INTEROP_MAX_FRAGMENT_BYTES: usize = 1024;
const RUDP_INTEROP_RESEND_DELAY_MS: u64 = 25;
const RUDP_INTEROP_READ_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(type = "cultnet.interop-note", schema = "CultNetInteropNote")]
struct CultNetInteropNote {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    document_id: String,
    #[cultcache(key = 2)]
    author_runtime_id: String,
    #[cultcache(key = 3)]
    title: String,
    #[cultcache(key = 4)]
    body: String,
    #[cultcache(key = 5)]
    tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "cultnet.interop-note-mutation-intent",
    schema = "CultNetInteropMutationIntent"
)]
struct CultNetInteropMutationIntent {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    intent_id: String,
    #[cultcache(key = 2)]
    target_document_id: String,
    #[cultcache(key = 3)]
    append_body: String,
    #[cultcache(key = 4)]
    append_tag: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "cultnet.interop-note-mutation-receipt",
    schema = "CultNetInteropMutationReceipt"
)]
struct CultNetInteropMutationReceipt {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    intent_id: String,
    #[cultcache(key = 2)]
    accepted: bool,
    #[cultcache(key = 3)]
    document_id: String,
    #[cultcache(key = 4)]
    body: String,
    #[cultcache(key = 5)]
    tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "cultnet.interop-fire-weapon-command",
    schema = "CultNetInteropFireCommand"
)]
struct CultNetInteropFireCommand {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    command_id: String,
    #[cultcache(key = 2)]
    character_id: String,
    #[cultcache(key = 3)]
    weapon_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, DatabaseEntry)]
#[cultcache(
    type = "cultnet.interop-fire-weapon-receipt",
    schema = "CultNetInteropFireReceipt"
)]
struct CultNetInteropFireReceipt {
    #[cultcache(key = 0)]
    schema_version: String,
    #[cultcache(key = 1)]
    command_id: String,
    #[cultcache(key = 2)]
    accepted: bool,
    #[cultcache(key = 3)]
    character_id: String,
    #[cultcache(key = 4)]
    weapon_id: String,
    #[cultcache(key = 5)]
    shots_fired: u32,
    #[cultcache(key = 6)]
    ammo_remaining: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "schemaVersion", rename_all = "camelCase")]
enum DiscoveryMessage {
    #[serde(rename = "cultnet.discovery_probe.v0", rename_all = "camelCase")]
    Probe {
        message_id: String,
        requester_runtime_id: String,
    },
    #[serde(rename = "cultnet.discovery_announce.v0", rename_all = "camelCase")]
    Announce {
        message_id: String,
        runtime_id: String,
        runtime_kind: String,
        display_name: String,
        agent_id: Option<String>,
        tcp_host: String,
        tcp_port: u16,
        wire_contract: String,
        supported_document_types: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport_profiles: Option<Vec<CultNetTransportProfile>>,
        supports_schema_catalog: bool,
    },
}

#[derive(Clone, Debug)]
struct PeerConfig {
    runtime_id: String,
    runtime_kind: String,
    display_name: String,
    agent_id: String,
    bind_host: String,
    advertise_host: String,
    tcp_port: u16,
    rudp_port: u16,
    discovery_port: u16,
    discovery_group: Ipv4Addr,
    schema_path: String,
}

#[derive(Clone, Debug)]
struct DialConfig {
    runtime_id: String,
    runtime_kind: String,
    display_name: String,
    agent_id: String,
    target_host: String,
    target_port: Option<u16>,
    target_rudp_port: Option<u16>,
    schema_path: String,
}

trait SchemaMessageSender {
    fn send_schema_message(&mut self, message: &CultNetMessage) -> Result<()>;
}

trait SchemaMessageTransport: SchemaMessageSender {
    fn read_schema_message(&mut self) -> Result<CultNetMessage>;
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().ok_or_else(|| {
        anyhow!("expected mode: serve | probe | dial | rudp-serve-once | rudp-dial-once | rudp-serve-message-once | rudp-dial-message-once")
    })?;
    let options = parse_args(args.collect());

    match mode.as_str() {
        "serve" => serve(parse_peer_config(&options)?)?,
        "probe" => probe(&options)?,
        "dial" => dial(parse_dial_config(&options)?)?,
        "rudp-serve-once" => rudp_serve_once(&options)?,
        "rudp-dial-once" => rudp_dial_once(&options)?,
        "rudp-serve-message-once" => rudp_serve_message_once(&options)?,
        "rudp-dial-message-once" => rudp_dial_message_once(&options)?,
        _ => return Err(anyhow!("unknown mode {mode}")),
    }

    Ok(())
}

fn serve(config: PeerConfig) -> Result<()> {
    let schema_registration = load_schema_registration(&config.schema_path)?;
    let mut schema_registry = builtin_schema_registry()?;
    schema_registry.register(schema_registration.clone())?;

    let mut cache = CultCache::new();
    cache.register_entry_type::<CultNetInteropNote>()?;
    register_capability_entry_types(&mut cache)?;
    cache.add_generic_backing_store(SingleFileMessagePackBackingStore::new(runtime_store_path(
        &config.runtime_id,
    )));
    cache.pull_all_backing_stores()?;
    let note = build_note(&config.runtime_id, &config.display_name);
    cache.put(&note.document_id, &note)?;

    let mut document_registry = CultNetDocumentRegistry::new();
    register_capability_bindings(&mut document_registry, &schema_registration.schema_id);

    let cache = Arc::new(Mutex::new(cache));
    let document_registry = Arc::new(document_registry);
    let schema_registry = Arc::new(schema_registry);
    let config = Arc::new(config);

    start_udp_discovery_server(config.clone())?;
    start_tcp_server(
        config.clone(),
        cache.clone(),
        document_registry.clone(),
        schema_registry.clone(),
    )?;
    start_rudp_server(config.clone(), cache, document_registry, schema_registry)?;

    print_json(&serde_json::json!({
        "status": "ready",
        "mode": "serve",
        "runtimeId": config.runtime_id,
        "runtimeKind": config.runtime_kind,
        "tcpPort": config.tcp_port,
        "rudpPort": config.rudp_port,
        "discoveryPort": config.discovery_port,
        "discoveryGroup": config.discovery_group.to_string(),
    }))?;

    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

fn probe(options: &BTreeMap<String, String>) -> Result<()> {
    let runtime_id = require_arg(options, "runtime-id")?.to_string();
    let discovery_port = parse_u16_arg(options, "discovery-port")?;
    let discovery_group = parse_ipv4_arg(options, "discovery-group")?;
    let timeout_ms = parse_u64_arg(options, "timeout-ms").unwrap_or(1_500);
    let message_id = format!("{runtime_id}-{}", chrono::Utc::now().timestamp_millis());

    let socket = create_discovery_socket(0, false)?;
    socket.set_read_timeout(Some(Duration::from_millis(timeout_ms)))?;

    let probe_message = DiscoveryMessage::Probe {
        message_id: message_id.clone(),
        requester_runtime_id: runtime_id.clone(),
    };
    let payload = rmp_serde::to_vec_named(&probe_message)?;
    socket.send_to(
        &payload,
        SocketAddr::V4(SocketAddrV4::new(discovery_group, discovery_port)),
    )?;

    let mut buffer = vec![0_u8; 4096];
    let mut found = BTreeMap::<String, serde_json::Value>::new();
    loop {
        match socket.recv_from(&mut buffer) {
            Ok((len, _)) => {
                if let Ok(DiscoveryMessage::Announce {
                    message_id: response_message_id,
                    runtime_id,
                    runtime_kind,
                    display_name,
                    agent_id,
                    tcp_host,
                    tcp_port,
                    wire_contract,
                    supported_document_types,
                    transport_profiles,
                    supports_schema_catalog,
                }) = rmp_serde::from_slice::<DiscoveryMessage>(&buffer[..len])
                {
                    if response_message_id == message_id {
                        found.insert(
                            runtime_id.clone(),
                            serde_json::json!({
                                "schemaVersion": DISCOVERY_ANNOUNCE_SCHEMA_VERSION,
                                "messageId": response_message_id,
                                "runtimeId": runtime_id,
                                "runtimeKind": runtime_kind,
                                "displayName": display_name,
                                "agentId": agent_id,
                                "tcpHost": tcp_host,
                                "tcpPort": tcp_port,
                                "wireContract": wire_contract,
                                "supportedDocumentTypes": supported_document_types,
                                "transportProfiles": transport_profiles,
                                "supportsSchemaCatalog": supports_schema_catalog,
                            }),
                        );
                    }
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }

    print_json(&serde_json::json!({
        "mode": "probe",
        "runtimeId": runtime_id,
        "peers": found.into_values().collect::<Vec<_>>(),
    }))?;
    Ok(())
}

fn dial(config: DialConfig) -> Result<()> {
    let schema_registration = load_schema_registration(&config.schema_path)?;

    let mut cache = CultCache::new();
    cache.register_entry_type::<CultNetInteropNote>()?;
    register_capability_entry_types(&mut cache)?;
    cache.add_generic_backing_store(SingleFileMessagePackBackingStore::new(runtime_store_path(
        &format!("{}-dial", config.runtime_id),
    )));
    cache.pull_all_backing_stores()?;

    let mut document_registry = CultNetDocumentRegistry::new();
    register_capability_bindings(&mut document_registry, &schema_registration.schema_id);

    let transport_name = if config.target_rudp_port.is_some() {
        "rudp"
    } else {
        "tcp"
    };
    let mut transport = open_dial_transport(&config)?;

    transport.send_schema_message(&CultNetMessage::Hello {
        runtime_id: config.runtime_id.clone(),
        runtime_kind: config.runtime_kind.clone(),
        agent_id: Some(config.agent_id.clone()),
        role: None,
        display_name: Some(config.display_name.clone()),
        supported_document_types: Some(vec![INTEROP_DOCUMENT_TYPE.to_string()]),
        supported_mutation_contracts: Some(interaction_contracts()),
        supported_message_versions: Some(vec![INTEROP_SCHEMA_VERSION.to_string()]),
        transport_profiles: Some(dial_transport_profiles(&config)),
        supports_schema_catalog: Some(true),
    })?;

    let remote_hello = transport
        .read_schema_message()
        .context("waiting for remote hello")?;
    let remote_runtime_id = match &remote_hello {
        CultNetMessage::Hello { runtime_id, .. } => runtime_id.clone(),
        other => return Err(anyhow!("expected hello response, got {other:?}")),
    };

    transport.send_schema_message(&CultNetMessage::SchemaCatalogRequest {
        message_id: format!("{}-catalog", config.runtime_id),
        include_schema_json: Some(true),
        schema_ids: None,
        kinds: None,
    })?;
    let catalog_response = transport
        .read_schema_message()
        .context("waiting for schema catalog response")?;
    let has_interop_schema = match &catalog_response {
        CultNetMessage::SchemaCatalogResponse { schemas, .. } => schemas.iter().any(|schema| {
            schema.schema_id == schema_registration.schema_id
                && schema.document_type.as_deref() == Some(INTEROP_DOCUMENT_TYPE)
        }),
        other => return Err(anyhow!("expected catalog response, got {other:?}")),
    };

    transport.send_schema_message(&CultNetMessage::SnapshotRequest {
        message_id: format!("{}-snapshot", config.runtime_id),
        schema_ids: Some(vec![schema_registration.schema_id.clone()]),
        record_keys: None,
    })?;
    let snapshot_response = transport
        .read_schema_message()
        .context("waiting for snapshot response")?;
    let applied = document_registry
        .apply_raw_snapshot_response::<CultNetInteropNote>(&mut cache, &snapshot_response)?;
    let note = applied
        .into_iter()
        .find(|candidate| candidate.author_runtime_id == remote_runtime_id)
        .ok_or_else(|| anyhow!("did not receive an interop note from {remote_runtime_id}"))?;

    print_json(&serde_json::json!({
        "mode": "dial",
        "transport": transport_name,
        "runtimeId": config.runtime_id,
        "targetHost": config.target_host,
        "targetPort": config.target_rudp_port.or(config.target_port),
        "remoteHello": {
            "schemaVersion": "cultnet.hello.v0",
            "runtimeId": remote_runtime_id,
        },
        "hasInteropSchema": has_interop_schema,
        "retrievedNote": {
            "schemaVersion": note.schema_version,
            "documentId": note.document_id,
            "authorRuntimeId": note.author_runtime_id,
            "title": note.title,
            "body": note.body,
            "tags": note.tags,
        },
        "mutatedNote": mutate_remote_note(&mut *transport, &mut cache, &document_registry, &schema_registration.schema_id, &config.runtime_id, &note)?,
        "fireReceipt": fire_remote_weapon(&mut *transport, &mut cache, &document_registry, &config.runtime_id, &remote_runtime_id)?,
    }))?;
    Ok(())
}

fn rudp_serve_once(options: &BTreeMap<String, String>) -> Result<()> {
    let bind_host = options
        .get("bind-host")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let bind_port = options
        .get("bind-port")
        .map(|value| value.parse::<u16>())
        .transpose()
        .with_context(|| "argument --bind-port must be a u16")?
        .unwrap_or(0);
    let expected_client_payload = options
        .get("client-payload")
        .map(|value| value.as_bytes().to_vec())
        .unwrap_or_else(|| b"ts-rust-client-state".to_vec());
    let server_payload = options
        .get("server-payload")
        .map(|value| value.as_bytes().to_vec())
        .unwrap_or_else(|| b"rust-server-state".to_vec());
    let server_extra_payload = options
        .get("server-extra-payload")
        .map(|value| value.as_bytes().to_vec());
    let disconnect_reason = options
        .get("disconnect-reason")
        .map(|value| value.as_bytes().to_vec());
    let max_fragment_bytes = options
        .get("max-fragment-bytes")
        .map(|value| value.parse::<u32>())
        .transpose()
        .with_context(|| "argument --max-fragment-bytes must be a u32")?;
    let socket = UdpSocket::bind((bind_host.as_str(), bind_port))
        .with_context(|| format!("failed to bind RUDP server on {bind_host}:{bind_port}"))?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    let local_port = socket.local_addr()?.port();
    let mut transport_options =
        CultNetRudpSocketTransportOptions::server("rust-rudp-interop", socket, 0x22446688);
    transport_options.initial_sequence = 100;
    transport_options.resend_delay_ms = 25;
    transport_options.max_fragment_bytes = max_fragment_bytes;
    let mut transport = CultNetRudpSocketTransportConnection::new(transport_options)?;

    print_json(&serde_json::json!({
        "status": "ready",
        "port": local_port,
    }))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = transport.receive_once()? {
            if frame.channel_id != "schema" || frame.payload != expected_client_payload {
                return Err(anyhow!(
                    "unexpected RUDP frame: channel={} payload={:?}",
                    frame.channel_id,
                    frame.payload
                ));
            }
            transport.send("schema", server_payload.clone())?;
            if let Some(extra_payload) = &server_extra_payload {
                transport.send("schema", extra_payload.clone())?;
            }
            if let Some(reason) = &disconnect_reason {
                transport.disconnect(reason.clone())?;
            }
            poll_rudp_after_send(&mut transport, Duration::from_millis(250))?;
            print_json(&serde_json::json!({ "status": "ok" }))?;
            return Ok(());
        }
        transport.poll_resends()?;
        thread::sleep(Duration::from_millis(5));
    }

    Err(anyhow!("timed out waiting for TypeScript RUDP frame"))
}

fn rudp_dial_once(options: &BTreeMap<String, String>) -> Result<()> {
    let target_host = require_arg(options, "target-host")?;
    let target_port = parse_u16_arg(options, "target-port")?;
    let remote_addr: SocketAddr = format!("{target_host}:{target_port}")
        .parse()
        .with_context(|| {
            format!("failed to parse RUDP remote endpoint {target_host}:{target_port}")
        })?;
    let socket = UdpSocket::bind(("127.0.0.1", 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    let mut transport_options = CultNetRudpSocketTransportOptions::client(
        "rust-rudp-client-interop",
        socket,
        remote_addr,
        0x88664422,
    );
    transport_options.resend_delay_ms = 25;
    let mut transport = CultNetRudpSocketTransportConnection::new(transport_options)?;
    transport.connect(b"rust-join".to_vec())?;

    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = transport.receive_once()? {
            if frame.channel_id != "schema" || frame.payload != b"ts-rust-server-state" {
                return Err(anyhow!(
                    "unexpected RUDP frame: channel={} payload={:?}",
                    frame.channel_id,
                    frame.payload
                ));
            }
            print_json(&serde_json::json!({ "status": "ok" }))?;
            return Ok(());
        }
        transport.poll_resends()?;
        if transport.connected() && !sent {
            transport.send("schema", b"rust-client-state".to_vec())?;
            sent = true;
        }
        thread::sleep(Duration::from_millis(5));
    }

    Err(anyhow!("timed out waiting for TypeScript RUDP response"))
}

fn rudp_serve_message_once(options: &BTreeMap<String, String>) -> Result<()> {
    let bind_host = options
        .get("bind-host")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let bind_port = options
        .get("bind-port")
        .map(|value| value.parse::<u16>())
        .transpose()
        .with_context(|| "argument --bind-port must be a u16")?
        .unwrap_or(0);
    let socket = UdpSocket::bind((bind_host.as_str(), bind_port)).with_context(|| {
        format!("failed to bind RUDP message server on {bind_host}:{bind_port}")
    })?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    let local_port = socket.local_addr()?.port();
    let mut transport_options =
        CultNetRudpSocketTransportOptions::server("rust-rudp-message-interop", socket, 0x22446689);
    transport_options.initial_sequence = 100;
    transport_options.resend_delay_ms = 25;
    let mut transport = CultNetRudpSocketTransportConnection::new(transport_options)?;

    print_json(&serde_json::json!({
        "status": "ready",
        "port": local_port,
    }))?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = transport.receive_once()? {
            if frame.channel_id != "schema" {
                return Err(anyhow!("unexpected RUDP channel: {}", frame.channel_id));
            }
            let message = decode_cultnet_message_from_slice(
                &frame.payload,
                CultNetWireContract::CultNetSchemaV0,
            )?;
            let CultNetMessage::SchemaCatalogRequest { .. } = message else {
                return Err(anyhow!("unexpected schema message: {message:?}"));
            };
            let response = CultNetMessage::Hello {
                runtime_id: "rust-rudp-message-interop".to_string(),
                runtime_kind: "rust".to_string(),
                agent_id: None,
                role: None,
                display_name: Some("Rust RUDP Message Interop".to_string()),
                supported_document_types: Some(vec![]),
                supported_mutation_contracts: Some(vec![]),
                supported_message_versions: Some(vec![
                    "cultnet.hello.v0".to_string(),
                    "cultnet.schema_catalog_request.v0".to_string(),
                ]),
                transport_profiles: Some(vec![create_rudp_transport_profile(
                    "rust-rudp-message-interop",
                    RudpTransportProfileOptions {
                        host: Some("127.0.0.1".to_string()),
                        port: Some(local_port),
                        ..RudpTransportProfileOptions::default()
                    },
                )]),
                supports_schema_catalog: Some(true),
            };
            transport.send(
                "schema",
                encode_cultnet_message_to_vec(&response, CultNetWireContract::CultNetSchemaV0)?,
            )?;
            poll_rudp_after_send(&mut transport, Duration::from_millis(250))?;
            print_json(&serde_json::json!({ "status": "ok" }))?;
            return Ok(());
        }
        transport.poll_resends()?;
        thread::sleep(Duration::from_millis(5));
    }

    Err(anyhow!(
        "timed out waiting for TypeScript RUDP schema-v0 message"
    ))
}

fn rudp_dial_message_once(options: &BTreeMap<String, String>) -> Result<()> {
    let target_host = require_arg(options, "target-host")?;
    let target_port = parse_u16_arg(options, "target-port")?;
    let remote_addr: SocketAddr = format!("{target_host}:{target_port}")
        .parse()
        .with_context(|| {
            format!("failed to parse RUDP remote endpoint {target_host}:{target_port}")
        })?;
    let socket = UdpSocket::bind(("127.0.0.1", 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;
    let mut transport_options = CultNetRudpSocketTransportOptions::client(
        "rust-rudp-message-client-interop",
        socket,
        remote_addr,
        0x88664423,
    );
    transport_options.resend_delay_ms = 25;
    let mut transport = CultNetRudpSocketTransportConnection::new(transport_options)?;
    transport.connect(b"rust-message-join".to_vec())?;

    let request = CultNetMessage::SchemaCatalogRequest {
        message_id: "rust-ts-schema-message".to_string(),
        include_schema_json: Some(false),
        schema_ids: Some(vec![]),
        kinds: Some(vec![CultNetSchemaKind::WireMessage]),
    };

    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(frame) = transport.receive_once()? {
            if frame.channel_id != "schema" {
                return Err(anyhow!("unexpected RUDP channel: {}", frame.channel_id));
            }
            let message = decode_cultnet_message_from_slice(
                &frame.payload,
                CultNetWireContract::CultNetSchemaV0,
            )?;
            match message {
                CultNetMessage::Hello { runtime_id, .. }
                    if runtime_id == "ts-rust-rudp-message-server" =>
                {
                    print_json(&serde_json::json!({ "status": "ok" }))?;
                    return Ok(());
                }
                other => return Err(anyhow!("unexpected schema message: {other:?}")),
            }
        }
        transport.poll_resends()?;
        if transport.connected() && !sent {
            transport.send(
                "schema",
                encode_cultnet_message_to_vec(&request, CultNetWireContract::CultNetSchemaV0)?,
            )?;
            sent = true;
        }
        thread::sleep(Duration::from_millis(5));
    }

    Err(anyhow!(
        "timed out waiting for TypeScript RUDP schema-v0 response"
    ))
}

fn poll_rudp_after_send(
    transport: &mut CultNetRudpSocketTransportConnection,
    duration: Duration,
) -> Result<()> {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn mutate_remote_note(
    transport: &mut dyn SchemaMessageTransport,
    cache: &mut CultCache,
    document_registry: &CultNetDocumentRegistry,
    note_schema_id: &str,
    runtime_id: &str,
    note: &CultNetInteropNote,
) -> Result<serde_json::Value> {
    let intent = CultNetInteropMutationIntent {
        schema_version: MUTATION_INTENT_SCHEMA_VERSION.to_string(),
        intent_id: format!("{runtime_id}-decorate"),
        target_document_id: note.document_id.clone(),
        append_body: format!(" Decorated by {runtime_id}."),
        append_tag: format!("decorated:{runtime_id}"),
    };
    let message = document_registry.create_raw_document_put_message(
        format!("{runtime_id}-decorate-put"),
        intent.intent_id.clone(),
        &intent,
        CultNetDocumentPutOptions::default(),
    )?;
    transport.send_schema_message(&message)?;

    let receipt_message = transport
        .read_schema_message()
        .context("waiting for mutation receipt")?;
    let _receipt = document_registry
        .apply_raw_document_put_message::<CultNetInteropMutationReceipt>(cache, &receipt_message)?;
    let mutated_message = transport
        .read_schema_message()
        .context("waiting for mutated note")?;
    let mutated = document_registry
        .apply_raw_document_put_message::<CultNetInteropNote>(cache, &mutated_message)?;
    if mutated_message_schema_id(&mutated_message) != Some(note_schema_id) {
        return Err(anyhow!("mutation response used the wrong schema id"));
    }
    Ok(serde_json::json!({
        "schemaVersion": mutated.schema_version,
        "documentId": mutated.document_id,
        "authorRuntimeId": mutated.author_runtime_id,
        "title": mutated.title,
        "body": mutated.body,
        "tags": mutated.tags,
    }))
}

fn fire_remote_weapon(
    transport: &mut dyn SchemaMessageTransport,
    cache: &mut CultCache,
    document_registry: &CultNetDocumentRegistry,
    runtime_id: &str,
    remote_runtime_id: &str,
) -> Result<serde_json::Value> {
    let command = CultNetInteropFireCommand {
        schema_version: FIRE_COMMAND_SCHEMA_VERSION.to_string(),
        command_id: format!("{runtime_id}-fire"),
        character_id: remote_runtime_id.to_string(),
        weapon_id: "interop-rifle".to_string(),
    };
    let message = document_registry.create_raw_document_put_message(
        format!("{runtime_id}-fire-put"),
        command.command_id.clone(),
        &command,
        CultNetDocumentPutOptions::default(),
    )?;
    transport.send_schema_message(&message)?;
    let receipt_message = transport
        .read_schema_message()
        .context("waiting for fire receipt")?;
    let receipt = document_registry
        .apply_raw_document_put_message::<CultNetInteropFireReceipt>(cache, &receipt_message)?;
    Ok(serde_json::json!({
        "schemaVersion": receipt.schema_version,
        "commandId": receipt.command_id,
        "accepted": receipt.accepted,
        "characterId": receipt.character_id,
        "weaponId": receipt.weapon_id,
        "shotsFired": receipt.shots_fired,
        "ammoRemaining": receipt.ammo_remaining,
    }))
}

fn start_udp_discovery_server(config: Arc<PeerConfig>) -> Result<()> {
    let socket = create_discovery_socket(config.discovery_port, true)?;
    socket.join_multicast_v4(&config.discovery_group, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;

    thread::spawn(move || {
        let mut buffer = vec![0_u8; 4096];
        loop {
            match socket.recv_from(&mut buffer) {
                Ok((len, remote)) => {
                    if let Ok(DiscoveryMessage::Probe {
                        message_id,
                        requester_runtime_id: _,
                    }) = rmp_serde::from_slice::<DiscoveryMessage>(&buffer[..len])
                    {
                        let announce = DiscoveryMessage::Announce {
                            message_id,
                            runtime_id: config.runtime_id.clone(),
                            runtime_kind: config.runtime_kind.clone(),
                            display_name: config.display_name.clone(),
                            agent_id: Some(config.agent_id.clone()),
                            tcp_host: config.advertise_host.clone(),
                            tcp_port: config.tcp_port,
                            wire_contract: "cultnet.schema.v0".to_string(),
                            supported_document_types: vec![INTEROP_DOCUMENT_TYPE.to_string()],
                            transport_profiles: Some(interop_transport_profiles(
                                &config.runtime_id,
                                &config.advertise_host,
                                config.tcp_port,
                                config.rudp_port,
                            )),
                            supports_schema_catalog: true,
                        };
                        if let Ok(payload) = rmp_serde::to_vec_named(&announce) {
                            let _ = socket.send_to(&payload, remote);
                        }
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
    });

    Ok(())
}

fn start_tcp_server(
    config: Arc<PeerConfig>,
    cache: Arc<Mutex<CultCache>>,
    document_registry: Arc<CultNetDocumentRegistry>,
    schema_registry: Arc<CultNetSchemaRegistry>,
) -> Result<()> {
    let listener =
        TcpListener::bind((config.bind_host.as_str(), config.tcp_port)).with_context(|| {
            format!(
                "failed to bind TCP listener on {}:{}",
                config.bind_host, config.tcp_port
            )
        })?;
    listener.set_nonblocking(false)?;

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let config = config.clone();
            let cache = cache.clone();
            let document_registry = document_registry.clone();
            let schema_registry = schema_registry.clone();
            thread::spawn(move || {
                if let Err(error) =
                    handle_connection(stream, config, cache, document_registry, schema_registry)
                {
                    eprintln!("{error:#}");
                }
            });
        }
    });

    Ok(())
}

fn start_rudp_server(
    config: Arc<PeerConfig>,
    cache: Arc<Mutex<CultCache>>,
    document_registry: Arc<CultNetDocumentRegistry>,
    schema_registry: Arc<CultNetSchemaRegistry>,
) -> Result<()> {
    let socket =
        UdpSocket::bind((config.bind_host.as_str(), config.rudp_port)).with_context(|| {
            format!(
                "failed to bind RUDP listener on {}:{}",
                config.bind_host, config.rudp_port
            )
        })?;
    socket.set_read_timeout(Some(Duration::from_millis(20)))?;

    thread::spawn(move || {
        if let Err(error) =
            run_rudp_server(socket, config, cache, document_registry, schema_registry)
        {
            eprintln!("{error:#}");
        }
    });

    Ok(())
}

struct RudpPeerSession {
    session: CultNetRudpSession,
}

fn run_rudp_server(
    socket: UdpSocket,
    config: Arc<PeerConfig>,
    cache: Arc<Mutex<CultCache>>,
    document_registry: Arc<CultNetDocumentRegistry>,
    schema_registry: Arc<CultNetSchemaRegistry>,
) -> Result<()> {
    let mut peers = BTreeMap::<SocketAddr, RudpPeerSession>::new();
    let mut buffer = vec![0_u8; 65_535];
    loop {
        match socket.recv_from(&mut buffer) {
            Ok((len, remote_addr)) => {
                let packet = match decode_rudp_packet(&buffer[..len]) {
                    Ok(packet) if packet.connection_id == RUDP_INTEROP_CONNECTION_ID => packet,
                    Ok(_) => continue,
                    Err(error) => {
                        eprintln!("{error:#}");
                        continue;
                    }
                };
                let peer = peers.entry(remote_addr).or_insert_with(|| RudpPeerSession {
                    session: CultNetRudpSession::new(CultNetRudpSessionOptions {
                        connection_id: RUDP_INTEROP_CONNECTION_ID,
                        initial_sequence: 100,
                        resend_delay_ms: RUDP_INTEROP_RESEND_DELAY_MS,
                        max_pending_reliable_packets: None,
                    }),
                });
                if packet.packet_type == CultNetRudpPacketType::Connect {
                    let accept = peer.session.accept_connect(
                        &packet,
                        rudp_now_ms(),
                        b"cultnet-interop-rudp".to_vec(),
                    )?;
                    send_rudp_packet(&socket, remote_addr, &accept)?;
                    continue;
                }

                let result = peer.session.receive(&packet, rudp_now_ms())?;
                if let Some(reply) = result.reply {
                    send_rudp_packet(&socket, remote_addr, &reply)?;
                }
                if packet.packet_type == CultNetRudpPacketType::Data {
                    let ack = peer.session.create_ack();
                    send_rudp_packet(&socket, remote_addr, &ack)?;
                }
                for frame in result.delivered {
                    if frame.channel_id != "schema" {
                        continue;
                    }
                    let message = decode_cultnet_message_from_slice(
                        &frame.payload,
                        CultNetWireContract::CultNetSchemaV0,
                    )?;
                    let mut transport = RudpSessionSender {
                        socket: &socket,
                        remote_addr,
                        session: &mut peer.session,
                    };
                    handle_server_message(
                        &mut transport,
                        message,
                        &config,
                        &cache,
                        &document_registry,
                        &schema_registry,
                    )?;
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut
                    || error.kind() == std::io::ErrorKind::ConnectionReset =>
            {
                let now = rudp_now_ms();
                for (remote_addr, peer) in peers.iter_mut() {
                    for packet in peer.session.due_resends(now) {
                        send_rudp_packet(&socket, *remote_addr, &packet)?;
                    }
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn handle_connection(
    stream: TcpStream,
    config: Arc<PeerConfig>,
    cache: Arc<Mutex<CultCache>>,
    document_registry: Arc<CultNetDocumentRegistry>,
    schema_registry: Arc<CultNetSchemaRegistry>,
) -> Result<()> {
    let mut transport = TcpFramedTransportConnection::new(
        stream,
        tcp_transport_profile(&config.runtime_id, &config.advertise_host, config.tcp_port),
    );
    loop {
        let message = match transport.read_schema_message() {
            Ok(message) => message,
            Err(error) if is_eof_like(&error) => break,
            Err(error) => return Err(error),
        };

        handle_server_message(
            &mut transport,
            message,
            &config,
            &cache,
            &document_registry,
            &schema_registry,
        )?;
    }

    Ok(())
}

fn handle_server_message(
    transport: &mut dyn SchemaMessageSender,
    message: CultNetMessage,
    config: &PeerConfig,
    cache: &Arc<Mutex<CultCache>>,
    document_registry: &CultNetDocumentRegistry,
    schema_registry: &CultNetSchemaRegistry,
) -> Result<()> {
    match message {
        CultNetMessage::Hello { .. } => {
            transport.send_schema_message(&CultNetMessage::Hello {
                runtime_id: config.runtime_id.clone(),
                runtime_kind: config.runtime_kind.clone(),
                agent_id: Some(config.agent_id.clone()),
                role: None,
                display_name: Some(config.display_name.clone()),
                supported_document_types: Some(vec![INTEROP_DOCUMENT_TYPE.to_string()]),
                supported_mutation_contracts: Some(interaction_contracts()),
                supported_message_versions: Some(vec![INTEROP_SCHEMA_VERSION.to_string()]),
                transport_profiles: Some(interop_transport_profiles(
                    &config.runtime_id,
                    &config.advertise_host,
                    config.tcp_port,
                    config.rudp_port,
                )),
                supports_schema_catalog: Some(true),
            })?;
        }
        request @ CultNetMessage::SchemaCatalogRequest { .. } => {
            let response = schema_registry.create_catalog_response(&request)?;
            transport.send_schema_message(&response)?;
        }
        CultNetMessage::SnapshotRequest {
            message_id,
            schema_ids,
            record_keys,
        } => {
            let mut response = document_registry.create_raw_snapshot_response(
                &cache.lock().expect("cache poisoned"),
                message_id,
                schema_ids.as_deref(),
                record_keys.as_deref(),
            )?;
            if let CultNetMessage::SnapshotResponseRaw { documents, .. } = &mut response {
                for document in documents.iter_mut() {
                    document.source_runtime_id = Some(config.runtime_id.clone());
                    document.source_agent_id = Some(config.agent_id.clone());
                    document.source_role = Some("peer".to_string());
                    document.tags = Some(vec!["interop".to_string(), config.runtime_id.clone()]);
                }
            }
            transport.send_schema_message(&response)?;
        }
        message @ CultNetMessage::DocumentPutRaw { .. } => {
            handle_raw_put(transport, config, cache, document_registry, &message)?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_raw_put(
    transport: &mut dyn SchemaMessageSender,
    config: &PeerConfig,
    cache: &Arc<Mutex<CultCache>>,
    document_registry: &CultNetDocumentRegistry,
    message: &CultNetMessage,
) -> Result<()> {
    let CultNetMessage::DocumentPutRaw { document, .. } = message else {
        return Ok(());
    };
    if document.schema_id == MUTATION_INTENT_SCHEMA_ID {
        let mut cache = cache.lock().expect("cache poisoned");
        let intent = document_registry
            .apply_raw_document_put_message::<CultNetInteropMutationIntent>(&mut cache, message)?;
        let mut note = cache.get_required::<CultNetInteropNote>(&intent.target_document_id)?;
        note.body = format!("{}{}", note.body, intent.append_body);
        note.tags.push(intent.append_tag);
        cache.put(&note.document_id, &note)?;
        let receipt = CultNetInteropMutationReceipt {
            schema_version: MUTATION_RECEIPT_SCHEMA_VERSION.to_string(),
            intent_id: intent.intent_id.clone(),
            accepted: true,
            document_id: note.document_id.clone(),
            body: note.body.clone(),
            tags: note.tags.clone(),
        };
        let options = response_options(config, "mutation");
        let receipt_message = document_registry.create_raw_document_put_message(
            format!("{}-mutation-receipt", config.runtime_id),
            receipt.intent_id.clone(),
            &receipt,
            options.clone(),
        )?;
        let note_message = document_registry.create_raw_document_put_message(
            format!("{}-mutated-note", config.runtime_id),
            note.document_id.clone(),
            &note,
            options,
        )?;
        transport.send_schema_message(&receipt_message)?;
        transport.send_schema_message(&note_message)?;
    } else if document.schema_id == FIRE_COMMAND_SCHEMA_ID {
        let mut cache = cache.lock().expect("cache poisoned");
        let command = document_registry
            .apply_raw_document_put_message::<CultNetInteropFireCommand>(&mut cache, message)?;
        let receipt = CultNetInteropFireReceipt {
            schema_version: FIRE_RECEIPT_SCHEMA_VERSION.to_string(),
            command_id: command.command_id,
            accepted: true,
            character_id: command.character_id,
            weapon_id: command.weapon_id,
            shots_fired: 1,
            ammo_remaining: 29,
        };
        let receipt_message = document_registry.create_raw_document_put_message(
            format!("{}-fire-receipt", config.runtime_id),
            receipt.command_id.clone(),
            &receipt,
            response_options(config, "side-effect"),
        )?;
        transport.send_schema_message(&receipt_message)?;
    }
    Ok(())
}

fn load_schema_registration(schema_path: &str) -> Result<CultNetSchemaRegistration> {
    let schema_json = fs::read_to_string(schema_path)
        .with_context(|| format!("failed to read schema {}", schema_path))?;
    let parsed: serde_json::Value = serde_json::from_str(&schema_json)
        .with_context(|| format!("failed to parse schema {}", schema_path))?;
    let schema_id = parsed
        .get("$id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("schema {} is missing $id", schema_path))?
        .to_string();
    let title = parsed
        .get("title")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);

    Ok(CultNetSchemaRegistration {
        schema_id,
        kind: CultNetSchemaKind::DocumentPayload,
        wire_contracts: vec![CultNetWireContract::CultNetSchemaV0],
        schema_version: Some(INTEROP_SCHEMA_VERSION.to_string()),
        document_type: Some(INTEROP_DOCUMENT_TYPE.to_string()),
        title,
        schema_json: Some(schema_json),
    })
}

fn register_capability_entry_types(cache: &mut CultCache) -> Result<()> {
    cache.register_entry_type::<CultNetInteropMutationIntent>()?;
    cache.register_entry_type::<CultNetInteropMutationReceipt>()?;
    cache.register_entry_type::<CultNetInteropFireCommand>()?;
    cache.register_entry_type::<CultNetInteropFireReceipt>()?;
    Ok(())
}

fn register_capability_bindings(
    document_registry: &mut CultNetDocumentRegistry,
    note_schema_id: &str,
) {
    document_registry
        .register(CultNetDocumentBinding::for_entry_with_schema_id::<
            CultNetInteropNote,
        >(
            note_schema_id.to_string(),
            INTEROP_SCHEMA_VERSION.to_string(),
        ))
        .register(CultNetDocumentBinding::for_entry_with_schema_id::<
            CultNetInteropMutationIntent,
        >(
            MUTATION_INTENT_SCHEMA_ID.to_string(),
            MUTATION_INTENT_SCHEMA_VERSION.to_string(),
        ))
        .register(CultNetDocumentBinding::for_entry_with_schema_id::<
            CultNetInteropMutationReceipt,
        >(
            MUTATION_RECEIPT_SCHEMA_ID.to_string(),
            MUTATION_RECEIPT_SCHEMA_VERSION.to_string(),
        ))
        .register(CultNetDocumentBinding::for_entry_with_schema_id::<
            CultNetInteropFireCommand,
        >(
            FIRE_COMMAND_SCHEMA_ID.to_string(),
            FIRE_COMMAND_SCHEMA_VERSION.to_string(),
        ))
        .register(CultNetDocumentBinding::for_entry_with_schema_id::<
            CultNetInteropFireReceipt,
        >(
            FIRE_RECEIPT_SCHEMA_ID.to_string(),
            FIRE_RECEIPT_SCHEMA_VERSION.to_string(),
        ));
}

fn interaction_contracts() -> Vec<CultNetDocumentMutationContract> {
    vec![CultNetDocumentMutationContract {
        document_type: INTEROP_DOCUMENT_TYPE.to_string(),
        payload_schema_version: Some(INTEROP_SCHEMA_VERSION.to_string()),
        operations: vec![
            CultNetDocumentOperation::Snapshot,
            CultNetDocumentOperation::DocumentPut,
            CultNetDocumentOperation::IntentSubmit,
            CultNetDocumentOperation::ReceiptWatch,
        ],
        authority: CultNetMutationAuthority::Runtime,
        intent_document_types: Some(vec![
            MUTATION_INTENT_TYPE.to_string(),
            FIRE_COMMAND_TYPE.to_string(),
        ]),
        receipt_document_types: Some(vec![
            MUTATION_RECEIPT_TYPE.to_string(),
            FIRE_RECEIPT_TYPE.to_string(),
        ]),
        notes: None,
    }]
}

fn tcp_transport_profile(runtime_id: &str, host: &str, port: u16) -> CultNetTransportProfile {
    create_tcp_framed_transport_profile(
        runtime_id,
        TcpFramedTransportProfileOptions {
            transport_id: Some("interop-tcp".to_string()),
            host: Some(host.to_string()),
            port: Some(port),
            ..TcpFramedTransportProfileOptions::default()
        },
    )
}

fn rudp_transport_profile(runtime_id: &str, host: &str, port: u16) -> CultNetTransportProfile {
    create_rudp_transport_profile(
        runtime_id,
        RudpTransportProfileOptions {
            transport_id: Some("interop-rudp".to_string()),
            host: Some(host.to_string()),
            port: Some(port),
            max_fragment_bytes: Some(RUDP_INTEROP_MAX_FRAGMENT_BYTES as u32),
            ..RudpTransportProfileOptions::default()
        },
    )
}

fn interop_transport_profiles(
    runtime_id: &str,
    host: &str,
    tcp_port: u16,
    rudp_port: u16,
) -> Vec<CultNetTransportProfile> {
    vec![
        tcp_transport_profile(runtime_id, host, tcp_port),
        rudp_transport_profile(runtime_id, host, rudp_port),
    ]
}

fn dial_transport_profiles(config: &DialConfig) -> Vec<CultNetTransportProfile> {
    let mut profiles = Vec::new();
    if let Some(target_port) = config.target_port {
        profiles.push(tcp_transport_profile(
            &config.runtime_id,
            &config.target_host,
            target_port,
        ));
    }
    if let Some(target_rudp_port) = config.target_rudp_port {
        profiles.push(rudp_transport_profile(
            &config.runtime_id,
            &config.target_host,
            target_rudp_port,
        ));
    }
    profiles
}

fn open_dial_transport(config: &DialConfig) -> Result<Box<dyn SchemaMessageTransport>> {
    if let Some(target_rudp_port) = config.target_rudp_port {
        let remote_addr: SocketAddr = format!("{}:{}", config.target_host, target_rudp_port)
            .parse()
            .with_context(|| {
                format!(
                    "failed to parse RUDP remote endpoint {}:{}",
                    config.target_host, target_rudp_port
                )
            })?;
        let socket = UdpSocket::bind(("127.0.0.1", 0))?;
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        let mut options = CultNetRudpSocketTransportOptions::client(
            format!("{}-interop-rudp-dial", config.runtime_id),
            socket,
            remote_addr,
            RUDP_INTEROP_CONNECTION_ID,
        );
        options.transport_id = Some("interop-rudp".to_string());
        options.resend_delay_ms = RUDP_INTEROP_RESEND_DELAY_MS;
        options.max_fragment_bytes = Some(RUDP_INTEROP_MAX_FRAGMENT_BYTES as u32);
        let mut transport = CultNetRudpSocketTransportConnection::new(options)?;
        transport.connect(b"cultnet-interop-rudp".to_vec())?;
        wait_for_rudp_connected(&mut transport, Duration::from_secs(5))?;
        return Ok(Box::new(transport));
    }

    let target_port = config
        .target_port
        .ok_or_else(|| anyhow!("dial requires --target-port or --target-rudp-port"))?;
    let stream =
        TcpStream::connect((config.target_host.as_str(), target_port)).with_context(|| {
            format!(
                "failed to connect to {}:{}",
                config.target_host, target_port
            )
        })?;
    Ok(Box::new(TcpFramedTransportConnection::new(
        stream,
        tcp_transport_profile(&config.runtime_id, &config.target_host, target_port),
    )))
}

fn response_options(config: &PeerConfig, tag: &str) -> CultNetDocumentPutOptions {
    CultNetDocumentPutOptions {
        source_runtime_id: Some(config.runtime_id.clone()),
        source_agent_id: Some(config.agent_id.clone()),
        source_role: Some("peer".to_string()),
        tags: Some(vec![tag.to_string(), config.runtime_id.clone()]),
        ..CultNetDocumentPutOptions::default()
    }
}

fn mutated_message_schema_id(message: &CultNetMessage) -> Option<&str> {
    match message {
        CultNetMessage::DocumentPutRaw { document, .. } => Some(document.schema_id.as_str()),
        _ => None,
    }
}

fn build_note(runtime_id: &str, display_name: &str) -> CultNetInteropNote {
    CultNetInteropNote {
        schema_version: INTEROP_SCHEMA_VERSION.to_string(),
        document_id: format!("note:{runtime_id}"),
        author_runtime_id: runtime_id.to_string(),
        title: format!("{display_name} keeps a little note"),
        body: format!(
            "{runtime_id} can move CultNet state without begging the gods for translation."
        ),
        tags: vec![
            runtime_id.to_string(),
            "interop".to_string(),
            "cultnet".to_string(),
        ],
    }
}

fn is_eof_like(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::UnexpectedEof)
}

impl SchemaMessageSender for TcpFramedTransportConnection<TcpStream> {
    fn send_schema_message(&mut self, message: &CultNetMessage) -> Result<()> {
        let payload = encode_cultnet_message_to_vec(message, CultNetWireContract::CultNetSchemaV0)?;
        self.send("schema", &payload)
    }
}

impl SchemaMessageTransport for TcpFramedTransportConnection<TcpStream> {
    fn read_schema_message(&mut self) -> Result<CultNetMessage> {
        let frame = self.receive()?;
        decode_cultnet_message_from_slice(&frame.payload, CultNetWireContract::CultNetSchemaV0)
    }
}

impl SchemaMessageSender for CultNetRudpSocketTransportConnection {
    fn send_schema_message(&mut self, message: &CultNetMessage) -> Result<()> {
        CultNetRudpSocketTransportConnection::send_schema_message(self, message)
    }
}

impl SchemaMessageTransport for CultNetRudpSocketTransportConnection {
    fn read_schema_message(&mut self) -> Result<CultNetMessage> {
        let deadline = Instant::now() + RUDP_INTEROP_READ_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(message) = self.receive_schema_message_once()? {
                return Ok(message);
            }
            self.poll_resends()?;
            thread::sleep(Duration::from_millis(5));
        }
        Err(anyhow!("timed out waiting for RUDP schema message"))
    }
}

struct RudpSessionSender<'a> {
    socket: &'a UdpSocket,
    remote_addr: SocketAddr,
    session: &'a mut CultNetRudpSession,
}

impl SchemaMessageSender for RudpSessionSender<'_> {
    fn send_schema_message(&mut self, message: &CultNetMessage) -> Result<()> {
        let payload = encode_cultnet_message_to_vec(message, CultNetWireContract::CultNetSchemaV0)?;
        let packets = self.session.send_many(
            "schema",
            payload,
            CultNetRudpSendOptions {
                reliable: true,
                ordered: true,
                sequenced: false,
                now_ms: rudp_now_ms(),
            },
            Some(RUDP_INTEROP_MAX_FRAGMENT_BYTES),
        )?;
        for packet in packets {
            send_rudp_packet(self.socket, self.remote_addr, &packet)?;
        }
        Ok(())
    }
}

fn send_rudp_packet(
    socket: &UdpSocket,
    remote_addr: SocketAddr,
    packet: &cultnet_rs::CultNetRudpPacket,
) -> Result<()> {
    socket.send_to(&encode_rudp_packet(packet)?, remote_addr)?;
    Ok(())
}

fn wait_for_rudp_connected(
    transport: &mut CultNetRudpSocketTransportConnection,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if transport.connected() {
            return Ok(());
        }
        let _ = transport.receive_once()?;
        transport.poll_resends()?;
        thread::sleep(Duration::from_millis(5));
    }
    Err(anyhow!("timed out waiting for RUDP connect"))
}

fn rudp_now_ms() -> u64 {
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn create_discovery_socket(port: u16, _join_group: bool) -> Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port).into())?;
    let socket = UdpSocket::from(socket);
    socket.set_multicast_loop_v4(true)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.set_nonblocking(false)?;
    Ok(socket)
}

fn parse_peer_config(options: &BTreeMap<String, String>) -> Result<PeerConfig> {
    let tcp_port = parse_u16_arg(options, "tcp-port")?;
    Ok(PeerConfig {
        runtime_id: require_arg(options, "runtime-id")?.to_string(),
        runtime_kind: require_arg(options, "runtime-kind")?.to_string(),
        display_name: require_arg(options, "display-name")?.to_string(),
        agent_id: require_arg(options, "agent-id")?.to_string(),
        bind_host: options
            .get("bind-host")
            .cloned()
            .unwrap_or_else(|| "127.0.0.1".to_string()),
        advertise_host: require_arg(options, "advertise-host")?.to_string(),
        tcp_port,
        rudp_port: options
            .get("rudp-port")
            .map(|value| value.parse::<u16>())
            .transpose()
            .with_context(|| "argument --rudp-port must be a u16")?
            .unwrap_or(tcp_port),
        discovery_port: parse_u16_arg(options, "discovery-port")?,
        discovery_group: parse_ipv4_arg(options, "discovery-group")?,
        schema_path: require_arg(options, "schema-path")?.to_string(),
    })
}

fn parse_dial_config(options: &BTreeMap<String, String>) -> Result<DialConfig> {
    Ok(DialConfig {
        runtime_id: require_arg(options, "runtime-id")?.to_string(),
        runtime_kind: require_arg(options, "runtime-kind")?.to_string(),
        display_name: require_arg(options, "display-name")?.to_string(),
        agent_id: require_arg(options, "agent-id")?.to_string(),
        target_host: require_arg(options, "target-host")?.to_string(),
        target_port: options
            .get("target-port")
            .map(|value| value.parse::<u16>())
            .transpose()
            .with_context(|| "argument --target-port must be a u16")?,
        target_rudp_port: options
            .get("target-rudp-port")
            .map(|value| value.parse::<u16>())
            .transpose()
            .with_context(|| "argument --target-rudp-port must be a u16")?,
        schema_path: require_arg(options, "schema-path")?.to_string(),
    })
}

fn parse_args(args: Vec<String>) -> BTreeMap<String, String> {
    let mut parsed = BTreeMap::new();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if !token.starts_with("--") {
            index += 1;
            continue;
        }
        let name = token.trim_start_matches("--").to_string();
        let value = args
            .get(index + 1)
            .cloned()
            .unwrap_or_else(|| panic!("missing value for --{name}"));
        parsed.insert(name, value);
        index += 2;
    }
    parsed
}

fn require_arg<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("missing required argument --{name}"))
}

fn parse_u16_arg(options: &BTreeMap<String, String>, name: &str) -> Result<u16> {
    require_arg(options, name)?
        .parse::<u16>()
        .with_context(|| format!("argument --{name} must be a u16"))
}

fn parse_u64_arg(options: &BTreeMap<String, String>, name: &str) -> Option<u64> {
    options
        .get(name)
        .map(|value| value.parse::<u64>().expect("u64 arg"))
}

fn parse_ipv4_arg(options: &BTreeMap<String, String>, name: &str) -> Result<Ipv4Addr> {
    require_arg(options, name)?
        .parse::<Ipv4Addr>()
        .with_context(|| format!("argument --{name} must be an IPv4 address"))
}

fn runtime_store_path(runtime_id: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("cultnet-rs-interop-{runtime_id}.msgpack"))
}

fn print_json(value: &serde_json::Value) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

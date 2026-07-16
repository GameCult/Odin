use anyhow::Result;
use anyhow::anyhow;
use chrono::DateTime;
use chrono::Utc;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::net::ToSocketAddrs;
use std::net::UdpSocket;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use crate::CultNetRudpSocketMode;
use crate::CultNetRudpSocketTransportConnection;
use crate::CultNetRudpSocketTransportOptions;
use crate::CultNetSchemaRegistry;
use crate::CultNetShardCatalog;
use crate::builtin_schema_registry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshPeerCard {
    pub peer_id: String,
    pub verse_id: String,
    pub endpoints: Vec<String>,
    pub roles: Vec<String>,
    pub shard_ids: Vec<String>,
    pub authority_lease_id: Option<String>,
}

impl CultMeshPeerCard {
    pub fn new(
        peer_id: impl Into<String>,
        verse_id: impl Into<String>,
        endpoints: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            peer_id: peer_id.into(),
            verse_id: verse_id.into(),
            endpoints: endpoints.into_iter().map(Into::into).collect(),
            roles: Vec::new(),
            shard_ids: Vec::new(),
            authority_lease_id: None,
        }
    }

    pub fn with_roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.roles = roles.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_shard_ids(
        mut self,
        shard_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.shard_ids = shard_ids.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_authority_lease_id(mut self, authority_lease_id: impl Into<String>) -> Self {
        self.authority_lease_id = Some(authority_lease_id.into());
        self
    }

    pub fn rudp_endpoint(&self) -> Result<CultNetRudpEndpoint> {
        let endpoint = self
            .endpoints
            .iter()
            .find(|endpoint| endpoint.to_ascii_lowercase().starts_with("rudp://"))
            .ok_or_else(|| anyhow!("Peer {} does not advertise a RUDP endpoint", self.peer_id))?;
        CultMesh::parse_rudp_endpoint(endpoint)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CultMeshPeerCatalog {
    peers: BTreeMap<String, CultMeshPeerCard>,
}

impl CultMeshPeerCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, peer: CultMeshPeerCard) -> Result<()> {
        require_non_empty(&peer.peer_id, "peer.peer_id")?;
        require_non_empty(&peer.verse_id, "peer.verse_id")?;
        self.peers.insert(peer.peer_id.clone(), peer);
        Ok(())
    }

    pub fn get(&self, peer_id: &str) -> Option<&CultMeshPeerCard> {
        self.peers.get(peer_id)
    }

    pub fn peers(&self) -> Vec<CultMeshPeerCard> {
        self.peers.values().cloned().collect()
    }

    pub fn find(&self, verse_id: &str, role: Option<&str>) -> Vec<CultMeshPeerCard> {
        self.peers
            .values()
            .filter(|peer| {
                peer.verse_id == verse_id
                    && role.is_none_or(|role| peer.roles.iter().any(|value| value == role))
            })
            .cloned()
            .collect()
    }

    pub fn find_authorized(
        &self,
        verse_id: &str,
        role: &str,
        leases: &CultMeshAuthorityLeaseCatalog,
        shard_id: Option<&str>,
        at: DateTime<Utc>,
    ) -> Vec<CultMeshPeerCard> {
        self.find(verse_id, Some(role))
            .into_iter()
            .filter(|peer| leases.is_authorized(peer, role, shard_id, at))
            .collect()
    }

    pub fn first_authorized(
        &self,
        verse_id: &str,
        role: &str,
        leases: &CultMeshAuthorityLeaseCatalog,
        shard_id: Option<&str>,
        at: DateTime<Utc>,
    ) -> Option<CultMeshPeerCard> {
        self.find_authorized(verse_id, role, leases, shard_id, at)
            .into_iter()
            .next()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshAuthorityLease {
    pub lease_id: String,
    pub verse_id: String,
    pub peer_id: String,
    pub roles: Vec<String>,
    pub shard_ids: Vec<String>,
    pub issuer_runtime_id: Option<String>,
    pub valid_from: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl CultMeshAuthorityLease {
    pub fn covers(
        &self,
        peer: &CultMeshPeerCard,
        role: &str,
        shard_id: Option<&str>,
        at: DateTime<Utc>,
    ) -> bool {
        at >= self.valid_from
            && at < self.expires_at
            && self.verse_id == peer.verse_id
            && self.peer_id == peer.peer_id
            && self.roles.iter().any(|value| value == role)
            && peer.roles.iter().any(|value| value == role)
            && shard_id.is_none_or(|shard_id| {
                self.shard_ids.is_empty() || self.shard_ids.iter().any(|value| value == shard_id)
            })
    }
}

#[derive(Clone, Debug, Default)]
pub struct CultMeshAuthorityLeaseCatalog {
    leases: BTreeMap<String, CultMeshAuthorityLease>,
}

impl CultMeshAuthorityLeaseCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, lease: CultMeshAuthorityLease) -> Result<()> {
        require_non_empty(&lease.lease_id, "lease.lease_id")?;
        require_non_empty(&lease.verse_id, "lease.verse_id")?;
        require_non_empty(&lease.peer_id, "lease.peer_id")?;
        if lease.expires_at <= lease.valid_from {
            return Err(anyhow!(
                "CultMesh authority lease expiry must be after valid_from"
            ));
        }
        self.leases.insert(lease.lease_id.clone(), lease);
        Ok(())
    }

    pub fn get(&self, lease_id: &str) -> Option<&CultMeshAuthorityLease> {
        self.leases.get(lease_id)
    }

    pub fn leases(&self) -> Vec<CultMeshAuthorityLease> {
        self.leases.values().cloned().collect()
    }

    pub fn is_authorized(
        &self,
        peer: &CultMeshPeerCard,
        role: &str,
        shard_id: Option<&str>,
        at: DateTime<Utc>,
    ) -> bool {
        let Some(lease_id) = peer.authority_lease_id.as_deref() else {
            return false;
        };
        self.leases
            .get(lease_id)
            .is_some_and(|lease| lease.covers(peer, role, shard_id, at))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetRudpEndpoint {
    pub host: String,
    pub port: u16,
}

impl CultNetRudpEndpoint {
    pub fn uri(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("rudp://[{}]:{}", self.host, self.port)
        } else {
            format!("rudp://{}:{}", self.host, self.port)
        }
    }

    pub fn socket_addr(&self) -> Result<SocketAddr> {
        format!("{}:{}", socket_host(&self.host), self.port)
            .to_socket_addrs()
            .map_err(|error| anyhow!("Invalid RUDP socket endpoint {}: {error}", self.uri()))?
            .next()
            .ok_or_else(|| anyhow!("RUDP socket endpoint {} did not resolve", self.uri()))
    }
}

#[derive(Clone, Debug)]
pub struct CultMeshRudpSocketOptions {
    pub bind_host: String,
    pub bind_port: u16,
    pub read_timeout: Option<Duration>,
    pub initial_sequence: u32,
    pub resend_delay_ms: u64,
    pub max_payload_bytes: Option<u32>,
    pub max_fragment_bytes: Option<u32>,
    pub max_pending_reliable_packets: Option<u32>,
}

impl Default for CultMeshRudpSocketOptions {
    fn default() -> Self {
        Self {
            bind_host: "127.0.0.1".to_string(),
            bind_port: 0,
            read_timeout: Some(Duration::from_millis(20)),
            initial_sequence: 1,
            resend_delay_ms: 250,
            max_payload_bytes: None,
            max_fragment_bytes: None,
            max_pending_reliable_packets: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CultMeshRudpClientOptions {
    pub socket_options: CultMeshRudpSocketOptions,
    pub connect_payload: Vec<u8>,
    pub connect_timeout: Duration,
    pub poll_interval: Duration,
}

impl Default for CultMeshRudpClientOptions {
    fn default() -> Self {
        Self {
            socket_options: CultMeshRudpSocketOptions::default(),
            connect_payload: Vec::new(),
            connect_timeout: Duration::from_secs(1),
            poll_interval: Duration::from_millis(5),
        }
    }
}

pub struct CultMesh;

impl CultMesh {
    pub fn create_schema_registry() -> CultNetSchemaRegistry {
        CultNetSchemaRegistry::new()
    }

    pub fn create_builtin_schema_registry() -> Result<CultNetSchemaRegistry> {
        builtin_schema_registry()
    }

    pub fn create_shard_catalog() -> CultNetShardCatalog {
        CultNetShardCatalog::new()
    }

    pub fn create_peer_catalog() -> CultMeshPeerCatalog {
        CultMeshPeerCatalog::new()
    }

    pub fn create_authority_lease_catalog() -> CultMeshAuthorityLeaseCatalog {
        CultMeshAuthorityLeaseCatalog::new()
    }

    pub fn parse_rudp_endpoint(endpoint: &str) -> Result<CultNetRudpEndpoint> {
        cultnet_rudp_endpoint(endpoint)
    }

    pub fn create_rudp_server(
        runtime_id: impl Into<String>,
        connection_id: u32,
        options: CultMeshRudpSocketOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let socket = bind_rudp_socket(&options)?;
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: runtime_id.into(),
            socket,
            mode: CultNetRudpSocketMode::Server,
            remote_addr: None,
            connection_id,
            initial_sequence: options.initial_sequence,
            resend_delay_ms: options.resend_delay_ms,
            transport_id: None,
            max_payload_bytes: options.max_payload_bytes,
            max_fragment_bytes: options.max_fragment_bytes,
            max_pending_reliable_packets: options.max_pending_reliable_packets,
            reconnect_policy: None,
        })
    }

    pub fn create_rudp_client(
        runtime_id: impl Into<String>,
        connection_id: u32,
        endpoint: &CultNetRudpEndpoint,
        options: CultMeshRudpSocketOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let socket = bind_rudp_socket(&options)?;
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: runtime_id.into(),
            socket,
            mode: CultNetRudpSocketMode::Client,
            remote_addr: Some(endpoint.socket_addr()?),
            connection_id,
            initial_sequence: options.initial_sequence,
            resend_delay_ms: options.resend_delay_ms,
            transport_id: None,
            max_payload_bytes: options.max_payload_bytes,
            max_fragment_bytes: options.max_fragment_bytes,
            max_pending_reliable_packets: options.max_pending_reliable_packets,
            reconnect_policy: None,
        })
    }

    pub fn create_rudp_client_for_endpoint(
        runtime_id: impl Into<String>,
        connection_id: u32,
        endpoint: &str,
        options: CultMeshRudpSocketOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let endpoint = Self::parse_rudp_endpoint(endpoint)?;
        Self::create_rudp_client(runtime_id, connection_id, &endpoint, options)
    }

    pub fn create_rudp_client_for_peer(
        runtime_id: impl Into<String>,
        connection_id: u32,
        peer: &CultMeshPeerCard,
        options: CultMeshRudpSocketOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let endpoint = peer.rudp_endpoint()?;
        Self::create_rudp_client(runtime_id, connection_id, &endpoint, options)
    }

    pub fn create_rudp_client_for_authorized_peer(
        runtime_id: impl Into<String>,
        connection_id: u32,
        peers: &CultMeshPeerCatalog,
        leases: &CultMeshAuthorityLeaseCatalog,
        verse_id: &str,
        role: &str,
        shard_id: Option<&str>,
        at: DateTime<Utc>,
        options: CultMeshRudpSocketOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let peer = peers
            .first_authorized(verse_id, role, leases, shard_id, at)
            .ok_or_else(|| {
                anyhow!("No authorized RUDP peer for role {role} in Verse {verse_id}")
            })?;
        Self::create_rudp_client_for_peer(runtime_id, connection_id, &peer, options)
    }

    pub fn connect_rudp_client(
        runtime_id: impl Into<String>,
        connection_id: u32,
        endpoint: &CultNetRudpEndpoint,
        options: CultMeshRudpClientOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let runtime_id = runtime_id.into();
        let mut client = Self::create_rudp_client(
            runtime_id.clone(),
            connection_id,
            endpoint,
            options.socket_options,
        )?;
        client.connect(options.connect_payload)?;
        let deadline = Instant::now() + options.connect_timeout;
        while Instant::now() < deadline {
            let _ = client.receive_once()?;
            client.poll_resends()?;
            if client.connected() {
                return Ok(client);
            }
            thread::sleep(options.poll_interval);
        }
        if client.connected() {
            return Ok(client);
        }
        Err(anyhow!(
            "Timed out waiting for RUDP client {runtime_id} to connect"
        ))
    }

    pub fn connect_rudp_client_for_endpoint(
        runtime_id: impl Into<String>,
        connection_id: u32,
        endpoint: &str,
        options: CultMeshRudpClientOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let endpoint = Self::parse_rudp_endpoint(endpoint)?;
        Self::connect_rudp_client(runtime_id, connection_id, &endpoint, options)
    }

    pub fn connect_rudp_client_for_peer(
        runtime_id: impl Into<String>,
        connection_id: u32,
        peer: &CultMeshPeerCard,
        options: CultMeshRudpClientOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let endpoint = peer.rudp_endpoint()?;
        Self::connect_rudp_client(runtime_id, connection_id, &endpoint, options)
    }

    pub fn connect_rudp_client_for_authorized_peer(
        runtime_id: impl Into<String>,
        connection_id: u32,
        peers: &CultMeshPeerCatalog,
        leases: &CultMeshAuthorityLeaseCatalog,
        verse_id: &str,
        role: &str,
        shard_id: Option<&str>,
        at: DateTime<Utc>,
        options: CultMeshRudpClientOptions,
    ) -> Result<CultNetRudpSocketTransportConnection> {
        let peer = peers
            .first_authorized(verse_id, role, leases, shard_id, at)
            .ok_or_else(|| {
                anyhow!("No authorized RUDP peer for role {role} in Verse {verse_id}")
            })?;
        Self::connect_rudp_client_for_peer(runtime_id, connection_id, &peer, options)
    }
}

pub fn cultnet_rudp_endpoint(endpoint: &str) -> Result<CultNetRudpEndpoint> {
    let scheme = "rudp://";
    if endpoint.len() < scheme.len() || !endpoint[..scheme.len()].eq_ignore_ascii_case(scheme) {
        return Err(anyhow!("RUDP endpoint must use the rudp scheme"));
    }
    let rest = &endpoint[scheme.len()..];
    let (host, port_text) = if let Some(rest) = rest.strip_prefix('[') {
        let (host, port_rest) = rest
            .split_once(']')
            .ok_or_else(|| anyhow!("RUDP IPv6 endpoint must close the host bracket"))?;
        let port_text = port_rest
            .strip_prefix(':')
            .ok_or_else(|| anyhow!("RUDP endpoint must include a valid port"))?;
        (host.to_string(), port_text)
    } else {
        let (host, port_text) = rest
            .rsplit_once(':')
            .ok_or_else(|| anyhow!("RUDP endpoint must include a host and port"))?;
        if host.contains(':') {
            return Err(anyhow!("RUDP IPv6 endpoint host must be bracketed"));
        }
        (host.to_string(), port_text)
    };
    require_non_empty(&host, "rudp host")?;
    let port = port_text
        .parse::<u16>()
        .map_err(|error| anyhow!("RUDP endpoint must include a valid port: {error}"))?;
    if port == 0 {
        return Err(anyhow!("RUDP endpoint port must be greater than zero"));
    }
    Ok(CultNetRudpEndpoint { host, port })
}

fn bind_rudp_socket(options: &CultMeshRudpSocketOptions) -> Result<UdpSocket> {
    let socket = UdpSocket::bind(format!(
        "{}:{}",
        socket_host(&options.bind_host),
        options.bind_port
    ))?;
    socket.set_read_timeout(options.read_timeout)?;
    Ok(socket)
}

fn socket_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn require_non_empty(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(anyhow!("CultMesh {field} must not be empty"));
    }
    Ok(())
}

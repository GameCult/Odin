use anyhow::{Context, Result};
use cultcache_rs::CultCache;
use cultcache_rs::CultCacheEnvelope;
use cultcache_rs::DatabaseEntry;
use cultcache_rs::SingleFileMessagePackBackingStore;
use cultnet_rs::CultNetDocumentPutOptions;
use cultnet_rs::CultNetDocumentRegistry;
use cultnet_rs::CultNetRudpSocketTransportConnection;
use cultnet_rs::CultNetRudpSocketTransportOptions;
use cultnet_rs::CultNetWireContract;
use cultnet_rs::decode_cultnet_message_from_slice;
use cultnet_rs::encode_cultnet_message_to_vec;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::hash_map::RandomState;
use std::env;
use std::hash::{BuildHasher, Hasher};
use std::net::SocketAddr;
use std::net::UdpSocket;
use std::path::Path;
use std::thread;
use std::time::Duration;
use std::time::Instant;

mod rudp_document_server;

pub use rudp_document_server::*;

pub const CULTMESH_RUDP_DOCUMENT_CATALOG_CONNECTION_ID: u32 = 0x0d1d_0002;

pub trait CultMeshDocumentSet: Clone + Send + Sync + 'static {
    fn register_cache(&self, cache: &mut CultCache) -> Result<()>;
    fn register_documents(&self, registry: &mut CultNetDocumentRegistry) -> Result<()>;
}

#[macro_export]
macro_rules! cultmesh_documents {
    ($name:ident { $($entry:ty => $schema_version:expr),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::CultMeshDocumentSet for $name {
            fn register_cache(
                &self,
                cache: &mut cultcache_rs::CultCache,
            ) -> ::anyhow::Result<()> {
                $(
                    cache.register_entry_type::<$entry>()?;
                )*
                Ok(())
            }

            fn register_documents(
                &self,
                registry: &mut cultnet_rs::CultNetDocumentRegistry,
            ) -> ::anyhow::Result<()> {
                $(
                    registry.register(
                        cultnet_rs::CultNetDocumentBinding::for_entry_with_schema_id::<$entry>(
                            $schema_version.to_string(),
                            $schema_version.to_string(),
                        ),
                    );
                )*
                Ok(())
            }
        }
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshNodeOptions {
    pub runtime_id: String,
    pub pull_on_start: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshRudpDocumentPublishOptions {
    pub target: SocketAddr,
    pub runtime_id: String,
    pub connect_timeout: Duration,
    pub flush_timeout: Duration,
    pub poll_interval: Duration,
    pub resend_delay_ms: u64,
    pub source_agent_id: Option<String>,
    pub source_role: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshRudpSnapshotOptions {
    pub target: SocketAddr,
    pub runtime_id: String,
    /// Exact correlation id for the SnapshotRequest. Raw callers must supply
    /// this; `CultMeshNode::pull_rudp_catalog_snapshot` generates one when it
    /// owns the request.
    pub message_id: String,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub poll_interval: Duration,
    pub resend_delay_ms: u64,
    pub schema_ids: Option<Vec<String>>,
    pub record_keys: Option<Vec<String>>,
}

impl CultMeshRudpSnapshotOptions {
    pub fn odin(target: SocketAddr, runtime_id: impl Into<String>) -> Self {
        Self {
            target,
            runtime_id: runtime_id.into(),
            ..Self::default()
        }
    }
}

impl Default for CultMeshRudpSnapshotOptions {
    fn default() -> Self {
        Self {
            target: SocketAddr::from(([0, 0, 0, 0], 0)),
            runtime_id: "cultmesh-rudp-snapshot-client".to_string(),
            message_id: String::new(),
            connect_timeout: Duration::from_secs(3),
            response_timeout: Duration::from_secs(3),
            poll_interval: Duration::from_millis(10),
            resend_delay_ms: 50,
            schema_ids: None,
            record_keys: None,
        }
    }
}

impl CultMeshRudpDocumentPublishOptions {
    pub fn odin(target: SocketAddr, runtime_id: impl Into<String>) -> Self {
        Self {
            target,
            runtime_id: runtime_id.into(),
            ..Self::default()
        }
    }
}

impl Default for CultMeshRudpDocumentPublishOptions {
    fn default() -> Self {
        Self {
            target: SocketAddr::from(([0, 0, 0, 0], 0)),
            runtime_id: "cultmesh-rudp-document-publisher".to_string(),
            connect_timeout: Duration::from_secs(3),
            flush_timeout: Duration::from_millis(300),
            poll_interval: Duration::from_millis(10),
            resend_delay_ms: 50,
            source_agent_id: None,
            source_role: None,
            tags: Vec::new(),
        }
    }
}

impl Default for CultMeshNodeOptions {
    fn default() -> Self {
        Self {
            runtime_id: "cultmesh-local".to_string(),
            pull_on_start: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshRudpEndpoint {
    pub host: String,
    pub port: u16,
    pub addr: SocketAddr,
}

pub struct CultMeshNode {
    runtime_id: String,
    cache: CultCache,
    documents: CultNetDocumentRegistry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CultMeshStreamKind {
    Audio,
    Video,
    Tensor,
    Bytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CultMeshStreamBodyTransport {
    SharedMemory,
    SharedD3d12Texture,
    SharedD3d11Texture,
    DmaBuf,
    IoSurface,
    AHardwareBuffer,
    CultCachePage,
    InlineBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CultMeshStreamAccess {
    Read,
    Write,
    ReadWrite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CultMeshStreamCopyBudget {
    ZeroCopyTarget,
    OneCopyFallback,
    OpaqueRuntime,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CultMeshStreamClock {
    pub clock_domain_id: String,
    pub source_id: Option<String>,
    pub sample_rate: i32,
    pub offset_to_verse_time_ns: i64,
    pub confidence: f64,
    pub evidence_kind: Option<String>,
}

impl CultMeshStreamClock {
    pub fn new(clock_domain_id: impl Into<String>) -> Result<Self> {
        let clock_domain_id = require_non_empty(clock_domain_id.into(), "clock_domain_id")?;
        Ok(Self {
            clock_domain_id,
            source_id: None,
            sample_rate: 0,
            offset_to_verse_time_ns: 0,
            confidence: 0.0,
            evidence_kind: None,
        })
    }

    pub fn source_id(mut self, source_id: impl Into<String>) -> Result<Self> {
        self.source_id = Some(require_non_empty(source_id.into(), "source_id")?);
        Ok(self)
    }

    pub fn confidence(mut self, confidence: f64) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn evidence_kind(mut self, evidence_kind: impl Into<String>) -> Result<Self> {
        self.evidence_kind = Some(require_non_empty(evidence_kind.into(), "evidence_kind")?);
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CultMeshStreamDescriptor {
    pub stream_id: String,
    pub verse_id: String,
    pub owner_peer_id: String,
    pub kind: CultMeshStreamKind,
    pub clock: CultMeshStreamClock,
    pub preferred_transports: Vec<CultMeshStreamBodyTransport>,
    pub label: Option<String>,
    pub required_access: CultMeshStreamAccess,
    pub max_in_flight_frames: u32,
    pub metadata_schema_id: Option<String>,
}

impl CultMeshStreamDescriptor {
    pub fn new(
        stream_id: impl Into<String>,
        verse_id: impl Into<String>,
        owner_peer_id: impl Into<String>,
        kind: CultMeshStreamKind,
        clock: CultMeshStreamClock,
        preferred_transports: Vec<CultMeshStreamBodyTransport>,
    ) -> Result<Self> {
        if preferred_transports.is_empty() {
            anyhow::bail!("preferred_transports must not be empty");
        }
        Ok(Self {
            stream_id: require_non_empty(stream_id.into(), "stream_id")?,
            verse_id: require_non_empty(verse_id.into(), "verse_id")?,
            owner_peer_id: require_non_empty(owner_peer_id.into(), "owner_peer_id")?,
            kind,
            clock,
            preferred_transports,
            label: None,
            required_access: CultMeshStreamAccess::Read,
            max_in_flight_frames: 0,
            metadata_schema_id: None,
        })
    }

    pub fn label(mut self, label: impl Into<String>) -> Result<Self> {
        self.label = Some(require_non_empty(label.into(), "label")?);
        Ok(self)
    }

    pub fn max_in_flight_frames(mut self, max_in_flight_frames: u32) -> Self {
        self.max_in_flight_frames = max_in_flight_frames;
        self
    }

    pub fn metadata_schema_id(mut self, metadata_schema_id: impl Into<String>) -> Result<Self> {
        self.metadata_schema_id = Some(require_non_empty(
            metadata_schema_id.into(),
            "metadata_schema_id",
        )?);
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshStreamConsumerProfile {
    pub peer_id: String,
    pub verse_id: String,
    pub supported_transports: Vec<CultMeshStreamBodyTransport>,
    pub accepted_kinds: Vec<CultMeshStreamKind>,
    pub max_in_flight_frames: u32,
}

impl CultMeshStreamConsumerProfile {
    pub fn new(
        peer_id: impl Into<String>,
        verse_id: impl Into<String>,
        supported_transports: Vec<CultMeshStreamBodyTransport>,
    ) -> Result<Self> {
        if supported_transports.is_empty() {
            anyhow::bail!("supported_transports must not be empty");
        }
        Ok(Self {
            peer_id: require_non_empty(peer_id.into(), "peer_id")?,
            verse_id: require_non_empty(verse_id.into(), "verse_id")?,
            supported_transports,
            accepted_kinds: Vec::new(),
            max_in_flight_frames: 0,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshStreamNegotiation {
    pub stream_id: String,
    pub producer_peer_id: String,
    pub consumer_peer_id: String,
    pub transport: CultMeshStreamBodyTransport,
    pub access: CultMeshStreamAccess,
    pub max_in_flight_frames: u32,
    pub copy_budget: CultMeshStreamCopyBudget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshStreamFrameHandle {
    pub stream_id: String,
    pub sequence: u64,
    pub timestamp_ns: i64,
    pub duration_ns: i64,
    pub transport: CultMeshStreamBodyTransport,
    pub byte_length: usize,
    pub resource_key: Option<String>,
    pub unavoidable_copy_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshSharedMemoryFrameRingStats {
    pub stream_id: String,
    pub slot_count: usize,
    pub slot_byte_length: usize,
    pub published_frames: u64,
    pub dropped_frames: u64,
    pub blocked_writes: u64,
    pub latest_sequence: u64,
    pub unavoidable_copy_count: u64,
}

pub struct CultMeshFrameReadLease<'a> {
    pub handle: CultMeshStreamFrameHandle,
    bytes: &'a [u8],
}

impl CultMeshFrameReadLease<'_> {
    pub fn bytes(&self) -> &[u8] {
        self.bytes
    }
}

#[derive(Clone, Debug)]
pub struct CultMeshSharedMemoryFrameRing {
    stream_id: String,
    slot_byte_length: usize,
    slots: Vec<Option<(CultMeshStreamFrameHandle, Vec<u8>)>>,
    write_cursor: usize,
    latest_slot: Option<usize>,
    next_sequence: u64,
    published_frames: u64,
    dropped_frames: u64,
    blocked_writes: u64,
    unavoidable_copy_count: u64,
}

impl CultMeshSharedMemoryFrameRing {
    pub fn new(
        stream_id: impl Into<String>,
        slot_count: usize,
        slot_byte_length: usize,
    ) -> Result<Self> {
        let stream_id = require_non_empty(stream_id.into(), "stream_id")?;
        if slot_count == 0 {
            anyhow::bail!("slot_count must be greater than zero");
        }
        if slot_byte_length == 0 {
            anyhow::bail!("slot_byte_length must be greater than zero");
        }
        Ok(Self {
            stream_id,
            slot_byte_length,
            slots: vec![None; slot_count],
            write_cursor: 0,
            latest_slot: None,
            next_sequence: 0,
            published_frames: 0,
            dropped_frames: 0,
            blocked_writes: 0,
            unavoidable_copy_count: 0,
        })
    }

    pub fn try_publish_copy(
        &mut self,
        bytes: &[u8],
        timestamp_ns: i64,
        duration_ns: i64,
    ) -> Result<Option<CultMeshStreamFrameHandle>> {
        if bytes.len() > self.slot_byte_length {
            anyhow::bail!("frame byte length exceeds slot size");
        }
        if self.slots.is_empty() {
            self.blocked_writes += 1;
            return Ok(None);
        }

        let slot_index = self.write_cursor;
        if self.slots[slot_index].is_some() {
            self.dropped_frames += 1;
        }
        let handle = CultMeshStreamFrameHandle {
            stream_id: self.stream_id.clone(),
            sequence: self.next_sequence,
            timestamp_ns,
            duration_ns,
            transport: CultMeshStreamBodyTransport::SharedMemory,
            byte_length: bytes.len(),
            resource_key: Some(format!("{}:slot:{slot_index}", self.stream_id)),
            unavoidable_copy_count: 1,
        };
        self.slots[slot_index] = Some((handle.clone(), bytes.to_vec()));
        self.latest_slot = Some(slot_index);
        self.write_cursor = (slot_index + 1) % self.slots.len();
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.published_frames = self.published_frames.saturating_add(1);
        self.unavoidable_copy_count = self.unavoidable_copy_count.saturating_add(1);
        Ok(Some(handle))
    }

    pub fn try_acquire_latest_read(&self) -> Option<CultMeshFrameReadLease<'_>> {
        let slot_index = self.latest_slot?;
        let (handle, bytes) = self.slots[slot_index].as_ref()?;
        Some(CultMeshFrameReadLease {
            handle: handle.clone(),
            bytes,
        })
    }

    pub fn stats(&self) -> CultMeshSharedMemoryFrameRingStats {
        let latest_sequence = self
            .latest_slot
            .and_then(|slot| self.slots[slot].as_ref().map(|(handle, _)| handle.sequence))
            .unwrap_or(0);
        CultMeshSharedMemoryFrameRingStats {
            stream_id: self.stream_id.clone(),
            slot_count: self.slots.len(),
            slot_byte_length: self.slot_byte_length,
            published_frames: self.published_frames,
            dropped_frames: self.dropped_frames,
            blocked_writes: self.blocked_writes,
            latest_sequence,
            unavoidable_copy_count: self.unavoidable_copy_count,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CultMeshStreamCatalog {
    streams: BTreeMap<String, CultMeshStreamDescriptor>,
    latest_frames: BTreeMap<String, CultMeshStreamFrameHandle>,
    rings: BTreeMap<String, CultMeshSharedMemoryFrameRing>,
}

impl CultMeshStreamCatalog {
    pub fn declare(&mut self, stream: CultMeshStreamDescriptor) {
        self.streams.insert(stream.stream_id.clone(), stream);
    }

    pub fn get(&self, stream_id: &str) -> Option<&CultMeshStreamDescriptor> {
        self.streams.get(stream_id)
    }

    pub fn streams(&self) -> Vec<&CultMeshStreamDescriptor> {
        self.streams.values().collect()
    }

    pub fn find(
        &self,
        verse_id: &str,
        kind: Option<CultMeshStreamKind>,
    ) -> Vec<&CultMeshStreamDescriptor> {
        self.streams
            .values()
            .filter(|stream| stream.verse_id == verse_id && kind.is_none_or(|k| stream.kind == k))
            .collect()
    }

    pub fn negotiate(
        &self,
        stream_id: &str,
        consumer: &CultMeshStreamConsumerProfile,
    ) -> Result<CultMeshStreamNegotiation> {
        let stream = self
            .streams
            .get(stream_id)
            .ok_or_else(|| anyhow::anyhow!("unknown CultMesh stream '{stream_id}'"))?;
        if stream.verse_id != consumer.verse_id {
            anyhow::bail!("stream and consumer must belong to the same Verse");
        }
        if !consumer.accepted_kinds.is_empty() && !consumer.accepted_kinds.contains(&stream.kind) {
            anyhow::bail!("consumer does not accept stream kind");
        }
        let transport = stream
            .preferred_transports
            .iter()
            .copied()
            .find(|candidate| consumer.supported_transports.contains(candidate))
            .ok_or_else(|| {
                anyhow::anyhow!("stream and consumer have no compatible body transport")
            })?;
        let producer_max = if stream.max_in_flight_frames == 0 {
            u32::MAX
        } else {
            stream.max_in_flight_frames
        };
        let consumer_max = if consumer.max_in_flight_frames == 0 {
            u32::MAX
        } else {
            consumer.max_in_flight_frames
        };
        Ok(CultMeshStreamNegotiation {
            stream_id: stream.stream_id.clone(),
            producer_peer_id: stream.owner_peer_id.clone(),
            consumer_peer_id: consumer.peer_id.clone(),
            transport,
            access: stream.required_access,
            max_in_flight_frames: producer_max.min(consumer_max),
            copy_budget: copy_budget_for(transport),
        })
    }

    pub fn create_shared_memory_ring(
        &mut self,
        stream_id: &str,
        slot_count: usize,
        slot_byte_length: usize,
    ) -> Result<&mut CultMeshSharedMemoryFrameRing> {
        if !self.streams.contains_key(stream_id) {
            anyhow::bail!("unknown CultMesh stream '{stream_id}'");
        }
        self.rings.insert(
            stream_id.to_string(),
            CultMeshSharedMemoryFrameRing::new(stream_id, slot_count, slot_byte_length)?,
        );
        Ok(self
            .rings
            .get_mut(stream_id)
            .expect("ring was inserted for stream"))
    }

    pub fn ring(&self, stream_id: &str) -> Option<&CultMeshSharedMemoryFrameRing> {
        self.rings.get(stream_id)
    }

    pub fn ring_mut(&mut self, stream_id: &str) -> Option<&mut CultMeshSharedMemoryFrameRing> {
        self.rings.get_mut(stream_id)
    }

    pub fn publish_frame(&mut self, handle: CultMeshStreamFrameHandle) -> Result<()> {
        if !self.streams.contains_key(&handle.stream_id) {
            anyhow::bail!("unknown CultMesh stream '{}'", handle.stream_id);
        }
        self.latest_frames.insert(handle.stream_id.clone(), handle);
        Ok(())
    }

    pub fn latest_frame(&self, stream_id: &str) -> Option<&CultMeshStreamFrameHandle> {
        self.latest_frames.get(stream_id)
    }
}

fn copy_budget_for(transport: CultMeshStreamBodyTransport) -> CultMeshStreamCopyBudget {
    match transport {
        CultMeshStreamBodyTransport::SharedMemory
        | CultMeshStreamBodyTransport::SharedD3d12Texture
        | CultMeshStreamBodyTransport::SharedD3d11Texture
        | CultMeshStreamBodyTransport::DmaBuf
        | CultMeshStreamBodyTransport::IoSurface
        | CultMeshStreamBodyTransport::AHardwareBuffer => CultMeshStreamCopyBudget::ZeroCopyTarget,
        CultMeshStreamBodyTransport::CultCachePage => CultMeshStreamCopyBudget::OneCopyFallback,
        CultMeshStreamBodyTransport::InlineBytes => CultMeshStreamCopyBudget::OpaqueRuntime,
    }
}

fn require_non_empty(value: String, name: &str) -> Result<String> {
    if value.trim().is_empty() {
        anyhow::bail!("{name} must be non-empty");
    }
    Ok(value)
}

impl CultMeshNode {
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn cache(&self) -> &CultCache {
        &self.cache
    }

    pub fn documents(&self) -> &CultNetDocumentRegistry {
        &self.documents
    }

    pub fn get<T: DatabaseEntry>(&self, key: &str) -> Result<Option<T>> {
        self.cache.get::<T>(key)
    }

    pub fn get_required<T: DatabaseEntry>(&self, key: &str) -> Result<T> {
        self.cache.get_required::<T>(key)
    }

    pub fn put<T: DatabaseEntry>(&mut self, key: impl Into<String>, value: &T) -> Result<T> {
        self.cache.put(key, value)
    }

    pub fn pull_all_backing_stores(&mut self) -> Result<()> {
        self.cache.pull_all_backing_stores()
    }

    pub fn delete<T: DatabaseEntry>(&mut self, key: &str) -> Result<bool> {
        self.cache.delete::<T>(key)
    }

    pub fn flush(&mut self) -> Result<()> {
        Ok(())
    }

    pub fn publish_document_to_rudp_catalog<T>(
        &self,
        key: impl Into<String>,
        value: &T,
        options: CultMeshRudpDocumentPublishOptions,
    ) -> Result<()>
    where
        T: DatabaseEntry + Serialize,
    {
        let key = key.into();
        let message = self.documents.create_raw_document_put_message(
            format!("{}:{}:{}", options.runtime_id, T::TYPE, key),
            key,
            value,
            CultNetDocumentPutOptions {
                source_runtime_id: Some(options.runtime_id.clone()),
                source_agent_id: options.source_agent_id.clone(),
                source_role: options.source_role.clone(),
                tags: if options.tags.is_empty() {
                    None
                } else {
                    Some(options.tags.clone())
                },
                ..CultNetDocumentPutOptions::default()
            },
        )?;
        publish_cultnet_message_to_rudp_catalog(&message, options)
    }

    pub fn pull_rudp_catalog_snapshot(
        &mut self,
        mut options: CultMeshRudpSnapshotOptions,
    ) -> Result<usize> {
        if options.message_id.trim().is_empty() {
            options.message_id = format!(
                "{}:snapshot:{}:{}",
                options.runtime_id,
                unix_millis(),
                fresh_rudp_connection_epoch()
            );
        }
        let response = request_raw_snapshot_from_rudp_catalog(options)?;
        let cultnet_rs::CultNetMessage::SnapshotResponseRaw { documents, .. } = response else {
            anyhow::bail!("expected cultnet.snapshot_response_raw.v0");
        };
        let mut applied = 0usize;
        for document in documents {
            let Some(document_type) =
                registered_document_type_for_schema(&self.documents, &document.schema_id)
            else {
                continue;
            };
            self.cache.put_raw_envelope(CultCacheEnvelope {
                key: document.record_key,
                r#type: document_type,
                payload: document.payload,
                stored_at: document.stored_at,
                schema_id: Some(document.schema_id),
            })?;
            applied += 1;
        }
        Ok(applied)
    }
}

fn registered_document_type_for_schema(
    documents: &CultNetDocumentRegistry,
    schema_id: &str,
) -> Option<String> {
    let normalized = strip_schema_version_suffix(schema_id);
    if normalized != schema_id {
        return Some(normalized.to_string());
    }
    documents
        .binding_by_schema_id(schema_id)
        .map(|binding| binding.document_type.clone())
        .or_else(|| documents.binding(schema_id).map(|_| schema_id.to_string()))
}

fn strip_schema_version_suffix(value: &str) -> &str {
    let Some((prefix, suffix)) = value.rsplit_once(".v") else {
        return value;
    };
    if suffix.chars().all(|character| character.is_ascii_digit()) {
        prefix
    } else {
        value
    }
}

/// Request a raw CultNet snapshot without importing it into a local cache.
///
/// The returned value is the exact validated `SnapshotResponseRaw`, including
/// its correlation `message_id`. Each call binds a fresh source port, chooses a
/// fresh connection epoch, and succeeds only after the reliable request is
/// acknowledged.
pub fn request_raw_snapshot_from_rudp_catalog(
    options: CultMeshRudpSnapshotOptions,
) -> Result<cultnet_rs::CultNetMessage> {
    if options.runtime_id.trim().is_empty() {
        anyhow::bail!("runtime_id must be non-empty");
    }
    if options.message_id.trim().is_empty() {
        anyhow::bail!("raw CultMesh RUDP snapshot requires a message_id");
    }
    let message_id = options.message_id.clone();
    let mut client = connect_rudp_client(
        options.target,
        &options.runtime_id,
        options.connect_timeout,
        options.poll_interval,
        options.resend_delay_ms,
        "cultmesh-rudp-snapshot-client",
    )?;

    let request = cultnet_rs::CultNetMessage::SnapshotRequest {
        message_id: message_id.clone(),
        schema_ids: options.schema_ids,
        record_keys: options.record_keys,
    };
    let payload = encode_cultnet_message_to_vec(&request, CultNetWireContract::CultNetSchemaV0)?;
    let receipt = client.send_reliable("schema", payload)?;
    let response_deadline = Instant::now() + options.response_timeout;
    let mut snapshot_response = None;
    while Instant::now() < response_deadline {
        if let Some(frame) = client.receive_once()? {
            if frame.channel_id == "schema" {
                let message = decode_cultnet_message_from_slice(
                    &frame.payload,
                    CultNetWireContract::CultNetSchemaV0,
                )?;
                match &message {
                    cultnet_rs::CultNetMessage::SnapshotResponseRaw {
                        message_id: received,
                        ..
                    } if received == &message_id => {
                        snapshot_response = Some(message);
                    }
                    cultnet_rs::CultNetMessage::Error { error } => {
                        let _ = client.disconnect(b"snapshot-error".to_vec());
                        anyhow::bail!("CultMesh RUDP snapshot failed: {error}");
                    }
                    _ => {}
                }
            }
        }
        match client.reliable_send_status(&receipt) {
            cultnet_rs::CultNetRudpReliableSendStatus::Acknowledged
                if snapshot_response.is_some() =>
            {
                let _ = client.disconnect(b"snapshot-complete".to_vec());
                return Ok(snapshot_response.expect("response was checked as present"));
            }
            cultnet_rs::CultNetRudpReliableSendStatus::Invalidated => {
                anyhow::bail!("CultMesh RUDP snapshot request receipt was invalidated")
            }
            _ => {}
        }
        client.poll_resends()?;
        thread::sleep(options.poll_interval);
    }
    let _ = client.disconnect(b"snapshot-timeout".to_vec());
    if snapshot_response.is_some() {
        anyhow::bail!(
            "timed out waiting for reliable acknowledgement of CultMesh RUDP snapshot request to {}",
            options.target
        );
    }
    anyhow::bail!(
        "timed out waiting for CultMesh RUDP snapshot from {}",
        options.target
    )
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Publish one raw CultNet schema message to a RUDP catalog.
///
/// Each call owns a fresh connection epoch and returns only after the catalog
/// acknowledges reliable application admission.
pub fn publish_cultnet_message_to_rudp_catalog(
    message: &cultnet_rs::CultNetMessage,
    options: CultMeshRudpDocumentPublishOptions,
) -> Result<()> {
    if options.runtime_id.trim().is_empty() {
        anyhow::bail!("runtime_id must be non-empty");
    }
    let mut client = connect_rudp_client(
        options.target,
        &options.runtime_id,
        options.connect_timeout,
        options.poll_interval,
        options.resend_delay_ms,
        "cultmesh-rudp-document-publisher",
    )?;

    let payload = encode_cultnet_message_to_vec(message, CultNetWireContract::CultNetSchemaV0)?;
    let receipt = client.send_reliable("schema", payload)?;
    let flush_deadline = Instant::now() + options.flush_timeout;
    while Instant::now() < flush_deadline {
        let _ = client.receive_once()?;
        match client.reliable_send_status(&receipt) {
            cultnet_rs::CultNetRudpReliableSendStatus::Acknowledged => {
                let _ = client.disconnect(b"document-acknowledged".to_vec());
                return Ok(());
            }
            cultnet_rs::CultNetRudpReliableSendStatus::Invalidated => {
                anyhow::bail!("CultMesh RUDP document receipt was invalidated")
            }
            cultnet_rs::CultNetRudpReliableSendStatus::Pending => {}
        }
        client.poll_resends()?;
        thread::sleep(options.poll_interval);
    }
    let _ = client.disconnect(b"document-unacknowledged".to_vec());
    anyhow::bail!(
        "timed out waiting for reliable acknowledgement from CultMesh RUDP catalog {}",
        options.target
    )
}

fn connect_rudp_client(
    target: SocketAddr,
    runtime_id: &str,
    connect_timeout: Duration,
    poll_interval: Duration,
    resend_delay_ms: u64,
    transport_id: &str,
) -> Result<CultNetRudpSocketTransportConnection> {
    let bind_addr = if target.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0))
    };
    let socket = UdpSocket::bind(bind_addr)?;
    socket.set_read_timeout(Some(poll_interval))?;
    let mut client =
        CultNetRudpSocketTransportConnection::new(CultNetRudpSocketTransportOptions {
            runtime_id: runtime_id.to_string(),
            socket,
            mode: cultnet_rs::CultNetRudpSocketMode::Client,
            remote_addr: Some(target),
            connection_id: fresh_rudp_connection_epoch(),
            initial_sequence: 1,
            resend_delay_ms,
            transport_id: Some(transport_id.to_string()),
            max_payload_bytes: None,
            max_fragment_bytes: Some(1200),
            max_pending_reliable_packets: Some(1024),
            media_reliable_expire_after_ms: None,
            reconnect_policy: None,
        })?;
    client.connect(Vec::new())?;
    let connect_deadline = Instant::now() + connect_timeout;
    while !client.connected() && Instant::now() < connect_deadline {
        let _ = client.receive_once()?;
        client.poll_resends()?;
        thread::sleep(poll_interval);
    }
    if !client.connected() {
        anyhow::bail!("timed out connecting CultMesh RUDP client {runtime_id} to {target}");
    }
    Ok(client)
}

fn fresh_rudp_connection_epoch() -> u32 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(unix_millis());
    let epoch = hasher.finish() as u32;
    if epoch == 0 { 1 } else { epoch }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CultMesh;

impl CultMesh {
    pub fn resolve_rudp_endpoint(endpoint: &str) -> Result<SocketAddr> {
        let text = require_non_empty(endpoint.trim().to_string(), "endpoint")?;
        if text.starts_with("rudp://") {
            return parse_rudp_socket_addr(&text);
        }
        if text.starts_with("cultmesh://") {
            let resolved = default_cultmesh_rudp_endpoint_resolver(&text)?.ok_or_else(|| {
                anyhow::anyhow!("CultMesh URI {text} did not resolve to a RUDP endpoint")
            })?;
            return parse_rudp_socket_addr(&resolved);
        }
        parse_socket_addr(&text)
    }

    pub fn create_node<D>(
        store_path: impl AsRef<Path>,
        documents: D,
        options: CultMeshNodeOptions,
    ) -> Result<CultMeshNode>
    where
        D: CultMeshDocumentSet,
    {
        let mut cache = CultCache::new();
        documents.register_cache(&mut cache)?;
        cache
            .add_generic_backing_store(SingleFileMessagePackBackingStore::new(store_path.as_ref()));
        if options.pull_on_start {
            cache.pull_all_backing_stores()?;
        }

        let mut registry = CultNetDocumentRegistry::new();
        documents.register_documents(&mut registry)?;

        Ok(CultMeshNode {
            runtime_id: options.runtime_id,
            cache,
            documents: registry,
        })
    }

    pub fn start_node<D>(
        store_path: impl AsRef<Path>,
        documents: D,
        options: CultMeshNodeOptions,
    ) -> Result<CultMeshNode>
    where
        D: CultMeshDocumentSet,
    {
        Self::create_node(store_path, documents, options)
    }

    pub fn create_stream_catalog() -> CultMeshStreamCatalog {
        CultMeshStreamCatalog::default()
    }
}

pub fn create_node<D>(
    store_path: impl AsRef<Path>,
    documents: D,
    options: CultMeshNodeOptions,
) -> Result<CultMeshNode>
where
    D: CultMeshDocumentSet,
{
    CultMesh::create_node(store_path, documents, options)
}

pub fn start_node<D>(
    store_path: impl AsRef<Path>,
    documents: D,
    options: CultMeshNodeOptions,
) -> Result<CultMeshNode>
where
    D: CultMeshDocumentSet,
{
    CultMesh::start_node(store_path, documents, options)
}

pub fn create_stream_catalog() -> CultMeshStreamCatalog {
    CultMesh::create_stream_catalog()
}

pub fn resolve_rudp_endpoint(endpoint: &str) -> Result<SocketAddr> {
    CultMesh::resolve_rudp_endpoint(endpoint)
}

fn parse_rudp_socket_addr(value: &str) -> Result<SocketAddr> {
    let text = value.trim().strip_prefix("rudp://").unwrap_or(value.trim());
    parse_socket_addr(text)
}

fn parse_socket_addr(value: &str) -> Result<SocketAddr> {
    value
        .parse()
        .with_context(|| format!("RUDP endpoint must be a socket address, got {value:?}"))
}

fn default_cultmesh_rudp_endpoint_resolver(uri: &str) -> Result<Option<String>> {
    let authority = cultmesh_uri_authority(uri)?;
    let slug = authority
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if slug.is_empty() {
        return Ok(None);
    }

    for key in [
        format!("CULTMESH_URI_{slug}_RUDP"),
        format!("{slug}_CULTMESH_RUDP_ENDPOINT"),
    ] {
        if let Ok(value) = env::var(&key) {
            let text = value.trim();
            if !text.is_empty() {
                return Ok(Some(if text.starts_with("rudp://") {
                    text.to_string()
                } else {
                    format!("rudp://{text}")
                }));
            }
        }
    }
    Ok(None)
}

fn cultmesh_uri_authority(uri: &str) -> Result<String> {
    let rest = uri
        .strip_prefix("cultmesh://")
        .ok_or_else(|| anyhow::anyhow!("CultMesh URI must start with cultmesh://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    if authority.is_empty() {
        anyhow::bail!("CultMesh URI must include an authority");
    }
    Ok(authority.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[derive(Clone, Debug, PartialEq, Eq, cultcache_rs::DatabaseEntry)]
    #[cultcache(type = "cultmesh.test.note", schema = "CultMeshTestNote")]
    struct Note {
        #[cultcache(key = 0)]
        body: String,
    }

    cultmesh_documents!(TestDocuments {
        Note => "cultmesh.test.note.v0",
    });

    #[test]
    fn node_round_trips_registered_documents() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("cultmesh.cc");
        let note = Note {
            body: "blessed circuit".to_string(),
        };

        let mut node = CultMesh::create_node(
            &store_path,
            TestDocuments,
            CultMeshNodeOptions {
                runtime_id: "test-runtime".to_string(),
                ..CultMeshNodeOptions::default()
            },
        )?;
        assert_eq!(node.runtime_id(), "test-runtime");
        node.put("note", &note)?;
        node.flush()?;

        let reloaded = CultMesh::create_node(&store_path, TestDocuments, Default::default())?;
        assert_eq!(reloaded.get_required::<Note>("note")?, note);
        assert!(
            reloaded
                .documents()
                .binding_by_schema_id("cultmesh.test.note.v0")
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn node_publishes_registered_document_to_rudp_catalog() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("cultmesh.cc");
        let note = Note {
            body: "respect odin once".to_string(),
        };
        let node = CultMesh::create_node(
            &store_path,
            TestDocuments,
            CultMeshNodeOptions {
                runtime_id: "muninn-test".to_string(),
                ..CultMeshNodeOptions::default()
            },
        )?;

        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_read_timeout(Some(Duration::from_millis(10)))?;
        let target = socket.local_addr()?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let delivered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let delivered_by_sink = delivered.clone();
        let server = thread::spawn(move || -> Result<()> {
            let mut server = CultMeshRudpDocumentServer::new(
                socket,
                move |receipt: CultMeshRudpRawDocumentReceipt| {
                    sender
                        .send((
                            receipt.document.schema_id,
                            receipt.document.record_key,
                            receipt.document.source_runtime_id,
                        ))
                        .ok();
                    delivered_by_sink.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
                |_query: &CultMeshRudpSnapshotQuery| Ok(Vec::new()),
                CultMeshSystemClock::default(),
                CultMeshRudpDocumentServerOptions::default(),
            )?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                server.poll_once()?;
                if delivered.load(std::sync::atomic::Ordering::SeqCst) {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(1));
            }
            anyhow::bail!("timed out waiting for RUDP document put")
        });

        node.publish_document_to_rudp_catalog(
            "note",
            &note,
            CultMeshRudpDocumentPublishOptions::odin(target, "muninn-test"),
        )?;
        let received = receiver.recv_timeout(Duration::from_secs(2))?;
        server.join().expect("server thread should not panic")?;

        assert_eq!(received.0, "cultmesh.test.note.v0");
        assert_eq!(received.1, "note");
        assert_eq!(received.2.as_deref(), Some("muninn-test"));
        Ok(())
    }

    #[test]
    fn document_publish_fails_when_catalog_is_unreachable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let node = CultMesh::create_node(
            temp.path().join("cultmesh.cc"),
            TestDocuments,
            Default::default(),
        )?;
        let unused = UdpSocket::bind("127.0.0.1:0")?;
        let target = unused.local_addr()?;
        drop(unused);

        let error = node
            .publish_document_to_rudp_catalog(
                "note",
                &Note {
                    body: "no false success".into(),
                },
                CultMeshRudpDocumentPublishOptions {
                    target,
                    connect_timeout: Duration::from_millis(30),
                    poll_interval: Duration::from_millis(2),
                    ..CultMeshRudpDocumentPublishOptions::default()
                },
            )
            .expect_err("unreachable catalog must fail closed");
        assert!(error.to_string().contains("timed out connecting"));
        Ok(())
    }

    #[test]
    fn document_publish_requires_application_acknowledgement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let node = CultMesh::create_node(
            temp.path().join("cultmesh.cc"),
            TestDocuments,
            Default::default(),
        )?;
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        let target = socket.local_addr()?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stop = stop.clone();
        let server = thread::spawn(move || -> Result<()> {
            let mut server = CultMeshRudpDocumentServer::new(
                socket,
                |_receipt: CultMeshRudpRawDocumentReceipt| {
                    anyhow::bail!("injected durable sink failure")
                },
                |_query: &CultMeshRudpSnapshotQuery| Ok(Vec::new()),
                CultMeshSystemClock::default(),
                CultMeshRudpDocumentServerOptions::default(),
            )?;
            while !server_stop.load(std::sync::atomic::Ordering::SeqCst) {
                let _ = server.poll_once();
                thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });

        let error = node
            .publish_document_to_rudp_catalog(
                "note",
                &Note {
                    body: "still not accepted".into(),
                },
                CultMeshRudpDocumentPublishOptions {
                    target,
                    connect_timeout: Duration::from_millis(100),
                    flush_timeout: Duration::from_millis(50),
                    poll_interval: Duration::from_millis(2),
                    resend_delay_ms: 5,
                    ..CultMeshRudpDocumentPublishOptions::default()
                },
            )
            .expect_err("unacknowledged document must fail closed");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        server.join().expect("server thread should not panic")?;
        assert!(error.to_string().contains("reliable acknowledgement"));
        Ok(())
    }

    #[test]
    fn raw_snapshot_preserves_caller_challenge_and_raw_response() -> Result<()> {
        let expected = cultnet_rs::CultNetRawDocumentRecord {
            schema_id: "cultmesh.test.raw.v0".into(),
            record_key: "route-proof".into(),
            stored_at: "2026-09-03T00:00:00Z".into(),
            payload_encoding: cultnet_rs::CultNetRawPayloadEncoding::Messagepack,
            payload: vec![0xc4, 0x03, 0, 0xff, 1],
            source_runtime_id: Some("odin-test".into()),
            source_agent_id: None,
            source_role: None,
            tags: None,
        };
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        let target = socket.local_addr()?;
        let challenge = "9aa63ad8-83b5-4cc4-ae4a-91d361d5f845".to_string();
        let server_document = expected.clone();
        let server_challenge = challenge.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let server_stop = stop.clone();
        let server = thread::spawn(move || -> Result<()> {
            let mut server = CultMeshRudpDocumentServer::new(
                socket,
                |_receipt: CultMeshRudpRawDocumentReceipt| Ok(()),
                move |query: &CultMeshRudpSnapshotQuery| {
                    if query.message_id != server_challenge {
                        anyhow::bail!("snapshot challenge changed")
                    }
                    Ok(vec![server_document.clone()])
                },
                CultMeshSystemClock::default(),
                CultMeshRudpDocumentServerOptions::default(),
            )?;
            while !server_stop.load(std::sync::atomic::Ordering::SeqCst) {
                server.poll_once()?;
                thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        });

        let response = request_raw_snapshot_from_rudp_catalog(CultMeshRudpSnapshotOptions {
            target,
            runtime_id: "route-proof-test".into(),
            message_id: challenge.clone(),
            connect_timeout: Duration::from_millis(100),
            response_timeout: Duration::from_millis(500),
            poll_interval: Duration::from_millis(2),
            resend_delay_ms: 5,
            schema_ids: Some(vec!["cultmesh.test.raw.v0".into()]),
            record_keys: Some(vec!["route-proof".into()]),
        });
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        server.join().expect("server thread should not panic")?;
        let response = response?;
        assert_eq!(
            response,
            cultnet_rs::CultNetMessage::SnapshotResponseRaw {
                message_id: challenge,
                documents: vec![expected],
            }
        );
        Ok(())
    }

    #[test]
    fn raw_snapshot_rejects_response_without_request_acknowledgement() -> Result<()> {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let target = socket.local_addr()?;
        let server = thread::spawn(move || -> Result<()> {
            let mut wire = vec![0_u8; 65_535];
            let (received, peer) = socket.recv_from(&mut wire)?;
            let connect = cultnet_rs::decode_rudp_packet(&wire[..received])?;
            let mut session =
                cultnet_rs::CultNetRudpSession::new(cultnet_rs::CultNetRudpSessionOptions {
                    connection_id: connect.connection_id,
                    initial_sequence: 1,
                    resend_delay_ms: 5,
                    max_pending_reliable_packets: Some(64),
                });
            let accept = session.accept_connect(&connect, 1, Vec::new())?;
            socket.send_to(&cultnet_rs::encode_rudp_packet(&accept)?, peer)?;

            loop {
                let (received, received_peer) = socket.recv_from(&mut wire)?;
                if received_peer != peer {
                    continue;
                }
                let packet = cultnet_rs::decode_rudp_packet(&wire[..received])?;
                let result = session.receive(&packet, 2)?;
                let Some(frame) = result.delivered.into_iter().next() else {
                    continue;
                };
                let request = decode_cultnet_message_from_slice(
                    &frame.payload,
                    CultNetWireContract::CultNetSchemaV0,
                )?;
                let cultnet_rs::CultNetMessage::SnapshotRequest { message_id, .. } = request else {
                    anyhow::bail!("expected snapshot request")
                };
                let payload = encode_cultnet_message_to_vec(
                    &cultnet_rs::CultNetMessage::SnapshotResponseRaw {
                        message_id,
                        documents: Vec::new(),
                    },
                    CultNetWireContract::CultNetSchemaV0,
                )?;
                let mut packets = session.send_many(
                    "schema",
                    payload,
                    cultnet_rs::CultNetRudpSendOptions {
                        reliable: true,
                        ordered: true,
                        sequenced: false,
                        now_ms: 2,
                        reliable_expire_after_ms: None,
                    },
                    Some(1200),
                )?;
                for packet in &mut packets {
                    packet.ack = 0;
                    packet.ack_mask = 0;
                    socket.send_to(&cultnet_rs::encode_rudp_packet(packet)?, peer)?;
                }
                return Ok(());
            }
        });

        let error = request_raw_snapshot_from_rudp_catalog(CultMeshRudpSnapshotOptions {
            target,
            runtime_id: "unacknowledged-snapshot-test".into(),
            message_id: "b7987efb-69c5-4c42-9355-b6e09535ad21".into(),
            connect_timeout: Duration::from_millis(100),
            response_timeout: Duration::from_millis(50),
            poll_interval: Duration::from_millis(2),
            resend_delay_ms: 5,
            schema_ids: None,
            record_keys: None,
        })
        .expect_err("response without request ACK must fail closed");
        server.join().expect("server thread should not panic")?;
        assert!(error.to_string().contains("reliable acknowledgement"));
        Ok(())
    }

    #[test]
    fn resolves_rudp_endpoint_from_cultmesh_uri_bootstrap_env() -> Result<()> {
        let previous = env::var("CULTMESH_URI_ODIN_RUDP").ok();
        unsafe {
            env::set_var("CULTMESH_URI_ODIN_RUDP", "127.0.0.1:17871");
        }
        let result = CultMesh::resolve_rudp_endpoint("cultmesh://odin/rendezvous/provider-catalog");
        unsafe {
            if let Some(value) = previous {
                env::set_var("CULTMESH_URI_ODIN_RUDP", value);
            } else {
                env::remove_var("CULTMESH_URI_ODIN_RUDP");
            }
        }

        assert_eq!(result?, "127.0.0.1:17871".parse()?);
        Ok(())
    }

    #[test]
    fn rejects_unresolved_cultmesh_rudp_endpoint() {
        let previous = env::var("CULTMESH_URI_MISSING_ODIN_RUDP").ok();
        unsafe {
            env::remove_var("CULTMESH_URI_MISSING_ODIN_RUDP");
        }
        let error =
            CultMesh::resolve_rudp_endpoint("cultmesh://missing-odin/rendezvous/provider-catalog")
                .expect_err("unresolved CultMesh URI should fail");
        unsafe {
            if let Some(value) = previous {
                env::set_var("CULTMESH_URI_MISSING_ODIN_RUDP", value);
            }
        }
        assert!(error.to_string().contains("did not resolve"));
    }

    #[test]
    fn raw_snapshot_application_uses_registered_document_type() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("cultmesh.cc");
        let mut node = CultMesh::create_node(
            &store_path,
            TestDocuments,
            CultMeshNodeOptions {
                runtime_id: "snapshot-apply-test".to_string(),
                ..CultMeshNodeOptions::default()
            },
        )?;
        let payload = rmp_serde::to_vec(&Note {
            body: "catalog truth".to_string(),
        })?;

        node.cache
            .put_raw_envelope(cultcache_rs::CultCacheEnvelope {
                key: "note".to_string(),
                r#type: node
                    .documents
                    .binding_by_schema_id("cultmesh.test.note.v0")
                    .expect("test binding")
                    .document_type
                    .clone(),
                payload,
                stored_at: "2026-07-03T00:00:00Z".to_string(),
                schema_id: Some("cultmesh.test.note.v0".to_string()),
            })?;
        node.flush()?;

        let reloaded = CultMesh::create_node(&store_path, TestDocuments, Default::default())?;
        assert_eq!(reloaded.get_required::<Note>("note")?.body, "catalog truth");
        assert_eq!(reloaded.cache().snapshot()[0].r#type, Note::TYPE);
        Ok(())
    }

    #[test]
    fn stream_catalog_negotiates_and_publishes_latest_shared_memory_frame() -> Result<()> {
        let mut catalog = CultMesh::create_stream_catalog();
        let stream = CultMeshStreamDescriptor::new(
            "muninn:nightwing:move-evidence",
            "mimir-live",
            "muninn:nightwing",
            CultMeshStreamKind::Bytes,
            CultMeshStreamClock::new("muninn:nightwing:clock")?
                .source_id("muninn:nightwing:move-evidence")?
                .confidence(1.0)
                .evidence_kind("muninn-move-evidence")?,
            vec![
                CultMeshStreamBodyTransport::SharedMemory,
                CultMeshStreamBodyTransport::CultCachePage,
            ],
        )?
        .label("Muninn Move evidence")?
        .max_in_flight_frames(4)
        .metadata_schema_id("mimir.muninn_move_evidence_stream_frame.v1")?;
        catalog.declare(stream);

        let consumer = CultMeshStreamConsumerProfile::new(
            "mimir:starfire",
            "mimir-live",
            vec![CultMeshStreamBodyTransport::SharedMemory],
        )?;
        let negotiation = catalog.negotiate("muninn:nightwing:move-evidence", &consumer)?;

        assert_eq!(
            negotiation.transport,
            CultMeshStreamBodyTransport::SharedMemory
        );
        assert_eq!(
            negotiation.copy_budget,
            CultMeshStreamCopyBudget::ZeroCopyTarget
        );

        let handle = {
            let ring =
                catalog.create_shared_memory_ring("muninn:nightwing:move-evidence", 4, 128)?;
            ring.try_publish_copy(&[1, 2, 3], 42, 0)?
                .expect("frame should publish")
        };
        catalog.publish_frame(handle.clone())?;

        assert_eq!(
            catalog.latest_frame("muninn:nightwing:move-evidence"),
            Some(&handle)
        );
        let lease = catalog
            .ring("muninn:nightwing:move-evidence")
            .and_then(CultMeshSharedMemoryFrameRing::try_acquire_latest_read)
            .expect("latest frame should be readable");
        assert_eq!(lease.handle.sequence, 0);
        assert_eq!(lease.bytes(), &[1, 2, 3]);
        Ok(())
    }
}

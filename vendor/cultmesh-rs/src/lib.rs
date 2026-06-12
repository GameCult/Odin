use anyhow::Result;
use cultcache_rs::CultCache;
use cultcache_rs::DatabaseEntry;
use cultcache_rs::SingleFileMessagePackBackingStore;
use cultnet_rs::CultNetDocumentRegistry;
use std::collections::BTreeMap;
use std::path::Path;

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

impl Default for CultMeshNodeOptions {
    fn default() -> Self {
        Self {
            runtime_id: "cultmesh-local".to_string(),
            pull_on_start: true,
        }
    }
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
        self.metadata_schema_id = Some(require_non_empty(metadata_schema_id.into(), "metadata_schema_id")?);
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
    pub fn new(stream_id: impl Into<String>, slot_count: usize, slot_byte_length: usize) -> Result<Self> {
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

    pub fn find(&self, verse_id: &str, kind: Option<CultMeshStreamKind>) -> Vec<&CultMeshStreamDescriptor> {
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
            .ok_or_else(|| anyhow::anyhow!("stream and consumer have no compatible body transport"))?;
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

    pub fn delete<T: DatabaseEntry>(&mut self, key: &str) -> Result<bool> {
        self.cache.delete::<T>(key)
    }

    pub fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CultMesh;

impl CultMesh {
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

        assert_eq!(negotiation.transport, CultMeshStreamBodyTransport::SharedMemory);
        assert_eq!(negotiation.copy_budget, CultMeshStreamCopyBudget::ZeroCopyTarget);

        let handle = {
            let ring = catalog.create_shared_memory_ring(
                "muninn:nightwing:move-evidence",
                4,
                128,
            )?;
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

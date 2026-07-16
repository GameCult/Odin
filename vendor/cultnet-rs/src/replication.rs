use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use cultcache_rs::CultCache;
use cultcache_rs::CultCacheEnvelope;
use cultcache_rs::DatabaseEntry;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::CultNetDocumentMutationContract;
use crate::CultNetDocumentRecord;
use crate::CultNetMessage;
use crate::CultNetRawDocumentRecord;
use crate::CultNetRawPayloadEncoding;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CultNetDocumentPutOptions {
    pub stored_at: Option<String>,
    pub source_runtime_id: Option<String>,
    pub source_agent_id: Option<String>,
    pub source_role: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetDocumentBinding {
    pub schema_id: String,
    pub document_type: String,
    pub mutation_contract: Option<CultNetDocumentMutationContract>,
    pub payload_schema_version: Option<String>,
}

impl CultNetDocumentBinding {
    pub fn for_entry<T: DatabaseEntry>(payload_schema_version: impl Into<Option<String>>) -> Self {
        Self {
            document_type: T::TYPE.to_string(),
            schema_id: T::TYPE.to_string(),
            mutation_contract: None,
            payload_schema_version: payload_schema_version.into(),
        }
    }

    pub fn for_entry_with_schema_id<T: DatabaseEntry>(
        schema_id: impl Into<String>,
        payload_schema_version: impl Into<Option<String>>,
    ) -> Self {
        Self {
            document_type: T::TYPE.to_string(),
            schema_id: schema_id.into(),
            mutation_contract: None,
            payload_schema_version: payload_schema_version.into(),
        }
    }

    pub fn with_mutation_contract(mut self, contract: CultNetDocumentMutationContract) -> Self {
        self.mutation_contract = Some(contract);
        self
    }
}

#[derive(Clone, Debug, Default)]
pub struct CultNetDocumentRegistry {
    bindings_by_type: BTreeMap<String, CultNetDocumentBinding>,
    bindings_by_schema_id: BTreeMap<String, CultNetDocumentBinding>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultNetReactiveDocumentOptions {
    pub flush_delay: Duration,
    pub detect_local_changes: bool,
    pub replace_dirty_current_on_canonical_snapshot: bool,
}

impl Default for CultNetReactiveDocumentOptions {
    fn default() -> Self {
        Self {
            flush_delay: Duration::from_millis(16),
            detect_local_changes: true,
            replace_dirty_current_on_canonical_snapshot: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CultNetReactiveDocumentReconciliation<T> {
    pub canonical: T,
    pub predicted: T,
    pub delta: BTreeMap<String, Value>,
    pub version: u64,
    pub received_at: String,
}

struct CultNetReactiveDocumentState<T> {
    dirty: bool,
    flushing: bool,
    flush_queued: bool,
    last_clean_payload: Vec<u8>,
    reconciliation: Option<CultNetReactiveDocumentReconciliation<T>>,
    reconciliation_version: u64,
    last_error: Option<String>,
}

pub struct CultNetReactiveDocument<T>
where
    T: DatabaseEntry + Serialize + DeserializeOwned,
{
    registry: CultNetDocumentRegistry,
    cache: Arc<Mutex<CultCache>>,
    record_key: String,
    current: Arc<Mutex<T>>,
    state: Arc<Mutex<CultNetReactiveDocumentState<T>>>,
    options: CultNetReactiveDocumentOptions,
    disposed: Arc<AtomicBool>,
    detect_worker: Option<JoinHandle<()>>,
}

impl CultNetDocumentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, binding: CultNetDocumentBinding) -> &mut Self {
        self.bindings_by_schema_id
            .insert(binding.schema_id.clone(), binding.clone());
        self.bindings_by_type
            .insert(binding.document_type.clone(), binding);
        self
    }

    pub fn binding(&self, document_type: &str) -> Option<&CultNetDocumentBinding> {
        self.bindings_by_type.get(document_type)
    }

    pub fn binding_by_schema_id(&self, schema_id: &str) -> Option<&CultNetDocumentBinding> {
        self.bindings_by_schema_id.get(schema_id)
    }

    pub fn mutation_contracts(&self) -> Vec<CultNetDocumentMutationContract> {
        self.bindings_by_type
            .values()
            .filter_map(|binding| binding.mutation_contract.clone())
            .collect()
    }

    pub fn create_document_put_message<T>(
        &self,
        message_id: impl Into<String>,
        record_key: impl Into<String>,
        value: &T,
        options: CultNetDocumentPutOptions,
    ) -> Result<CultNetMessage>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let binding = self.require_binding(T::TYPE)?;
        let payload = serde_json::to_value(round_trip_typed(value)?)?;
        Ok(CultNetMessage::DocumentPut {
            message_id: message_id.into(),
            document: CultNetDocumentRecord {
                schema_id: binding.schema_id.clone(),
                record_key: record_key.into(),
                stored_at: options.stored_at.unwrap_or_else(now_utc_second),
                payload,
                source_runtime_id: options.source_runtime_id,
                source_agent_id: options.source_agent_id,
                source_role: options.source_role,
                tags: options.tags,
            },
        })
    }

    pub fn create_document_delete_message(
        &self,
        message_id: impl Into<String>,
        schema_id: impl Into<String>,
        record_key: impl Into<String>,
    ) -> CultNetMessage {
        CultNetMessage::DocumentDelete {
            message_id: message_id.into(),
            schema_id: schema_id.into(),
            record_key: record_key.into(),
        }
    }

    pub fn create_raw_document_put_message_from_envelope(
        &self,
        message_id: impl Into<String>,
        envelope: &CultCacheEnvelope,
    ) -> Result<CultNetMessage> {
        Ok(CultNetMessage::DocumentPutRaw {
            message_id: message_id.into(),
            document: self.raw_document_record_from_envelope(envelope)?,
        })
    }

    pub fn create_raw_document_put_message<T>(
        &self,
        message_id: impl Into<String>,
        record_key: impl Into<String>,
        value: &T,
        options: CultNetDocumentPutOptions,
    ) -> Result<CultNetMessage>
    where
        T: DatabaseEntry + Serialize,
    {
        let binding = self.require_binding(T::TYPE)?;
        Ok(CultNetMessage::DocumentPutRaw {
            message_id: message_id.into(),
            document: CultNetRawDocumentRecord {
                schema_id: binding.schema_id.clone(),
                record_key: record_key.into(),
                stored_at: options.stored_at.unwrap_or_else(now_utc_second),
                payload_encoding: CultNetRawPayloadEncoding::Messagepack,
                payload: rmp_serde::to_vec(value)?,
                source_runtime_id: options.source_runtime_id,
                source_agent_id: options.source_agent_id,
                source_role: options.source_role,
                tags: options.tags,
            },
        })
    }

    pub fn create_snapshot_response(
        &self,
        cache: &CultCache,
        message_id: impl Into<String>,
        schema_ids: Option<&[String]>,
        record_keys: Option<&[String]>,
    ) -> Result<CultNetMessage> {
        let requested_schema_ids = schema_ids.map(|items| items.iter().collect::<BTreeSet<_>>());
        let requested_record_keys = record_keys.map(|items| items.iter().collect::<BTreeSet<_>>());
        let mut documents = Vec::new();
        for envelope in cache.snapshot() {
            let binding = self.require_binding(&envelope.r#type)?;
            if requested_schema_ids
                .as_ref()
                .is_some_and(|ids| !ids.contains(&binding.schema_id))
            {
                continue;
            }
            if requested_record_keys
                .as_ref()
                .is_some_and(|keys| !keys.contains(&envelope.key))
            {
                continue;
            }
            documents.push(self.document_record_from_envelope(&envelope)?);
        }
        Ok(CultNetMessage::SnapshotResponse {
            message_id: message_id.into(),
            documents,
        })
    }

    pub fn create_raw_snapshot_response(
        &self,
        cache: &CultCache,
        message_id: impl Into<String>,
        schema_ids: Option<&[String]>,
        record_keys: Option<&[String]>,
    ) -> Result<CultNetMessage> {
        let requested_schema_ids = schema_ids.map(|items| items.iter().collect::<BTreeSet<_>>());
        let requested_record_keys = record_keys.map(|items| items.iter().collect::<BTreeSet<_>>());
        let mut documents = Vec::new();
        for envelope in cache.snapshot() {
            let binding = self.require_binding(&envelope.r#type)?;
            if requested_schema_ids
                .as_ref()
                .is_some_and(|ids| !ids.contains(&binding.schema_id))
            {
                continue;
            }
            if requested_record_keys
                .as_ref()
                .is_some_and(|keys| !keys.contains(&envelope.key))
            {
                continue;
            }
            documents.push(self.raw_document_record_from_envelope(&envelope)?);
        }
        Ok(CultNetMessage::SnapshotResponseRaw {
            message_id: message_id.into(),
            documents,
        })
    }

    pub fn apply_document_put_message<T>(
        &self,
        cache: &mut CultCache,
        message: &CultNetMessage,
    ) -> Result<T>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let CultNetMessage::DocumentPut { document, .. } = message else {
            return Err(anyhow!("expected cultnet.document_put.v0"));
        };
        let binding = self.require_binding(T::TYPE)?;
        if document.schema_id != binding.schema_id {
            return Err(anyhow!(
                "schema id {:?} does not match registered Rust type {:?} schema {:?}",
                document.schema_id,
                T::TYPE,
                binding.schema_id
            ));
        }
        let value: T = serde_json::from_value(document.payload.clone()).with_context(|| {
            format!(
                "failed to decode CultNet payload schema {:?} as {}",
                binding.schema_id,
                T::SCHEMA_NAME
            )
        })?;
        cache.put(&document.record_key, &value)
    }

    pub fn apply_document_delete_message<T>(
        &self,
        cache: &mut CultCache,
        message: &CultNetMessage,
    ) -> Result<bool>
    where
        T: DatabaseEntry,
    {
        let CultNetMessage::DocumentDelete {
            schema_id,
            record_key,
            ..
        } = message
        else {
            return Err(anyhow!("expected cultnet.document_delete.v0"));
        };
        let binding = self.require_binding(T::TYPE)?;
        if schema_id != &binding.schema_id {
            return Err(anyhow!(
                "schema id {:?} does not match registered Rust type {:?} schema {:?}",
                schema_id,
                T::TYPE,
                binding.schema_id
            ));
        }
        cache.delete::<T>(record_key)
    }

    pub fn apply_raw_document_put_message<T>(
        &self,
        cache: &mut CultCache,
        message: &CultNetMessage,
    ) -> Result<T>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let CultNetMessage::DocumentPutRaw { document, .. } = message else {
            return Err(anyhow!("expected cultnet.document_put_raw.v0"));
        };
        let binding = self.require_binding(T::TYPE)?;
        if document.schema_id != binding.schema_id {
            return Err(anyhow!(
                "schema id {:?} does not match registered Rust type {:?} schema {:?}",
                document.schema_id,
                T::TYPE,
                binding.schema_id
            ));
        }
        cache.put_envelope::<T>(CultCacheEnvelope {
            key: document.record_key.clone(),
            r#type: binding.document_type.clone(),
            payload: document.payload.clone(),
            stored_at: document.stored_at.clone(),
            schema_id: Some(binding.schema_id.clone()),
        })
    }

    pub fn apply_snapshot_response<T>(
        &self,
        cache: &mut CultCache,
        response: &CultNetMessage,
    ) -> Result<Vec<T>>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let CultNetMessage::SnapshotResponse { documents, .. } = response else {
            return Err(anyhow!("expected cultnet.snapshot_response.v0"));
        };
        let mut applied = Vec::new();
        let binding = self.require_binding(T::TYPE)?;
        for document in documents {
            if document.schema_id != binding.schema_id {
                continue;
            }
            applied.push(self.apply_document_put_message::<T>(
                cache,
                &CultNetMessage::DocumentPut {
                    message_id: "snapshot-apply".to_string(),
                    document: document.clone(),
                },
            )?);
        }
        Ok(applied)
    }

    pub fn sync_document_from_snapshot_response<T>(
        &self,
        cache: &mut CultCache,
        response: &CultNetMessage,
        record_key: &str,
    ) -> Result<T>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let CultNetMessage::SnapshotResponse { documents, .. } = response else {
            return Err(anyhow!("expected cultnet.snapshot_response.v0"));
        };
        let binding = self.require_binding(T::TYPE)?;
        let document = documents
            .iter()
            .find(|document| {
                document.schema_id == binding.schema_id && document.record_key == record_key
            })
            .ok_or_else(|| {
                anyhow!(
                    "No CultNet snapshot document for schema {:?} and record key {:?}",
                    binding.schema_id,
                    record_key
                )
            })?;
        self.apply_document_put_message::<T>(
            cache,
            &CultNetMessage::DocumentPut {
                message_id: "snapshot-sync".to_string(),
                document: document.clone(),
            },
        )
    }

    pub fn apply_raw_snapshot_response<T>(
        &self,
        cache: &mut CultCache,
        response: &CultNetMessage,
    ) -> Result<Vec<T>>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let CultNetMessage::SnapshotResponseRaw { documents, .. } = response else {
            return Err(anyhow!("expected cultnet.snapshot_response_raw.v0"));
        };
        let mut applied = Vec::new();
        let binding = self.require_binding(T::TYPE)?;
        for document in documents {
            if document.schema_id != binding.schema_id {
                continue;
            }
            applied.push(self.apply_raw_document_put_message::<T>(
                cache,
                &CultNetMessage::DocumentPutRaw {
                    message_id: "snapshot-apply-raw".to_string(),
                    document: document.clone(),
                },
            )?);
        }
        Ok(applied)
    }

    pub fn sync_raw_document_from_snapshot_response<T>(
        &self,
        cache: &mut CultCache,
        response: &CultNetMessage,
        record_key: &str,
    ) -> Result<T>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        let CultNetMessage::SnapshotResponseRaw { documents, .. } = response else {
            return Err(anyhow!("expected cultnet.snapshot_response_raw.v0"));
        };
        let binding = self.require_binding(T::TYPE)?;
        let document = documents
            .iter()
            .find(|document| {
                document.schema_id == binding.schema_id && document.record_key == record_key
            })
            .ok_or_else(|| {
                anyhow!(
                    "No raw CultNet snapshot document for schema {:?} and record key {:?}",
                    binding.schema_id,
                    record_key
                )
            })?;
        self.apply_raw_document_put_message::<T>(
            cache,
            &CultNetMessage::DocumentPutRaw {
                message_id: "snapshot-sync-raw".to_string(),
                document: document.clone(),
            },
        )
    }

    pub fn reactive_document<T>(
        &self,
        cache: Arc<Mutex<CultCache>>,
        record_key: impl Into<String>,
        options: CultNetReactiveDocumentOptions,
    ) -> Result<CultNetReactiveDocument<T>>
    where
        T: DatabaseEntry + Serialize + DeserializeOwned,
    {
        CultNetReactiveDocument::new(self.clone(), cache, record_key, options)
    }

    fn document_record_from_envelope(
        &self,
        envelope: &CultCacheEnvelope,
    ) -> Result<CultNetDocumentRecord<Value>> {
        let binding = self.require_binding(&envelope.r#type)?;
        let payload: Value = rmp_serde::from_slice(&envelope.payload).with_context(|| {
            format!(
                "failed to decode CultCache envelope {:?} at {:?} as generic CultNet payload",
                envelope.r#type, envelope.key
            )
        })?;
        Ok(CultNetDocumentRecord {
            schema_id: binding.schema_id.clone(),
            record_key: envelope.key.clone(),
            stored_at: envelope.stored_at.clone(),
            payload,
            source_runtime_id: None,
            source_agent_id: None,
            source_role: None,
            tags: None,
        })
    }

    fn raw_document_record_from_envelope(
        &self,
        envelope: &CultCacheEnvelope,
    ) -> Result<CultNetRawDocumentRecord> {
        let binding = self.require_binding(&envelope.r#type)?;
        Ok(CultNetRawDocumentRecord {
            schema_id: binding.schema_id.clone(),
            record_key: envelope.key.clone(),
            stored_at: envelope.stored_at.clone(),
            payload_encoding: CultNetRawPayloadEncoding::Messagepack,
            payload: envelope.payload.clone(),
            source_runtime_id: None,
            source_agent_id: None,
            source_role: None,
            tags: None,
        })
    }

    fn require_binding(&self, document_type: &str) -> Result<&CultNetDocumentBinding> {
        self.binding(document_type).ok_or_else(|| {
            anyhow!("No CultNet document binding is registered for {document_type:?}")
        })
    }
}

impl<T> CultNetReactiveDocument<T>
where
    T: DatabaseEntry + Serialize + DeserializeOwned,
{
    pub fn new(
        registry: CultNetDocumentRegistry,
        cache: Arc<Mutex<CultCache>>,
        record_key: impl Into<String>,
        options: CultNetReactiveDocumentOptions,
    ) -> Result<Self> {
        let record_key = record_key.into();
        if record_key.trim().is_empty() {
            return Err(anyhow!("record_key must be non-empty"));
        }
        registry.require_binding(T::TYPE)?;
        let current = {
            let cache = cache
                .lock()
                .map_err(|_| anyhow!("CultCache mutex poisoned"))?;
            cache.get_required::<T>(&record_key)?
        };
        let last_clean_payload = serialize_typed(&current)?;
        let current = Arc::new(Mutex::new(current));
        let state = Arc::new(Mutex::new(CultNetReactiveDocumentState {
            dirty: false,
            flushing: false,
            flush_queued: false,
            last_clean_payload,
            reconciliation: None,
            reconciliation_version: 0,
            last_error: None,
        }));
        let disposed = Arc::new(AtomicBool::new(false));
        let detect_worker = if options.detect_local_changes {
            Some(start_reactive_detection_worker::<T>(
                Arc::clone(&cache),
                Arc::clone(&current),
                Arc::clone(&state),
                Arc::clone(&disposed),
                options.flush_delay,
                record_key.clone(),
            ))
        } else {
            None
        };
        Ok(Self {
            registry,
            cache,
            record_key,
            current,
            state,
            options,
            disposed,
            detect_worker,
        })
    }

    pub fn current(&self) -> Arc<Mutex<T>> {
        Arc::clone(&self.current)
    }

    pub fn record_key(&self) -> &str {
        &self.record_key
    }

    pub fn is_dirty(&self) -> bool {
        self.state.lock().map(|state| state.dirty).unwrap_or(false)
    }

    pub fn reconciliation(&self) -> Option<CultNetReactiveDocumentReconciliation<T>> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.reconciliation.clone())
    }

    pub fn last_error(&self) -> Option<String> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.last_error.clone())
    }

    pub fn update(&self, update: impl FnOnce(&mut T)) -> Result<T> {
        let predicted = {
            let mut current = self
                .current
                .lock()
                .map_err(|_| anyhow!("reactive document mutex poisoned"))?;
            update(&mut current);
            current.clone()
        };
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
            state.dirty = true;
        }
        if self.options.flush_delay.is_zero() {
            self.flush()?;
        }
        Ok(predicted)
    }

    pub fn set_current(&self, value: T) -> Result<T> {
        {
            let mut current = self
                .current
                .lock()
                .map_err(|_| anyhow!("reactive document mutex poisoned"))?;
            *current = value.clone();
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
            state.dirty = true;
        }
        if self.options.flush_delay.is_zero() {
            self.flush()?;
        }
        Ok(value)
    }

    pub fn mark_dirty(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        state.dirty = true;
        Ok(())
    }

    pub fn refresh(&self) -> Result<T> {
        let canonical = {
            let cache = self
                .cache
                .lock()
                .map_err(|_| anyhow!("CultCache mutex poisoned"))?;
            cache.get_required::<T>(&self.record_key)?
        };
        {
            let mut current = self
                .current
                .lock()
                .map_err(|_| anyhow!("reactive document mutex poisoned"))?;
            *current = canonical.clone();
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
            state.last_clean_payload = serialize_typed(&canonical)?;
            state.dirty = false;
            state.flush_queued = false;
            state.reconciliation = None;
        }
        Ok(canonical)
    }

    pub fn flush(&self) -> Result<()> {
        flush_reactive_document::<T>(&self.cache, &self.current, &self.state, &self.record_key)
    }

    pub fn apply_document_put_message(&self, message: &CultNetMessage) -> Result<T> {
        let canonical = {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| anyhow!("CultCache mutex poisoned"))?;
            self.registry
                .apply_document_put_message::<T>(&mut cache, message)?
        };
        self.apply_canonical_value(canonical)
    }

    pub fn apply_raw_document_put_message(&self, message: &CultNetMessage) -> Result<T> {
        let canonical = {
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| anyhow!("CultCache mutex poisoned"))?;
            self.registry
                .apply_raw_document_put_message::<T>(&mut cache, message)?
        };
        self.apply_canonical_value(canonical)
    }

    pub fn clear_reconciliation(&self) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        state.reconciliation = None;
        Ok(())
    }

    fn apply_canonical_value(&self, canonical: T) -> Result<T> {
        let predicted = self
            .current
            .lock()
            .map_err(|_| anyhow!("reactive document mutex poisoned"))?
            .clone();
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        if state.dirty || state.flushing {
            let delta = create_reconciliation_delta(&predicted, &canonical)?;
            if delta.is_empty() {
                state.reconciliation = None;
            } else {
                state.reconciliation_version += 1;
                state.reconciliation = Some(CultNetReactiveDocumentReconciliation {
                    canonical: canonical.clone(),
                    predicted,
                    delta,
                    version: state.reconciliation_version,
                    received_at: now_utc_second(),
                });
            }
            if !self.options.replace_dirty_current_on_canonical_snapshot {
                return Ok(canonical);
            }
        }
        drop(state);
        {
            let mut current = self
                .current
                .lock()
                .map_err(|_| anyhow!("reactive document mutex poisoned"))?;
            *current = canonical.clone();
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        state.last_clean_payload = serialize_typed(&canonical)?;
        state.reconciliation = None;
        Ok(canonical)
    }
}

impl<T> Drop for CultNetReactiveDocument<T>
where
    T: DatabaseEntry + Serialize + DeserializeOwned,
{
    fn drop(&mut self) {
        self.disposed.store(true, Ordering::SeqCst);
        let _ = self.detect_worker.take();
    }
}

fn start_reactive_detection_worker<T>(
    cache: Arc<Mutex<CultCache>>,
    current: Arc<Mutex<T>>,
    state: Arc<Mutex<CultNetReactiveDocumentState<T>>>,
    disposed: Arc<AtomicBool>,
    flush_delay: Duration,
    record_key: String,
) -> JoinHandle<()>
where
    T: DatabaseEntry + Serialize + DeserializeOwned,
{
    thread::spawn(move || {
        let delay = if flush_delay.is_zero() {
            Duration::from_millis(1)
        } else {
            flush_delay
        };
        while !disposed.load(Ordering::SeqCst) {
            thread::sleep(delay);
            match detect_reactive_document_change::<T>(&current, &state).and_then(|dirty| {
                if dirty {
                    flush_reactive_document::<T>(&cache, &current, &state, &record_key)?;
                }
                Ok(())
            }) {
                Ok(()) => {}
                Err(error) => {
                    if let Ok(mut state) = state.lock() {
                        state.last_error = Some(error.to_string());
                    }
                }
            }
        }
    })
}

fn detect_reactive_document_change<T>(
    current: &Arc<Mutex<T>>,
    state: &Arc<Mutex<CultNetReactiveDocumentState<T>>>,
) -> Result<bool>
where
    T: DatabaseEntry + Serialize + DeserializeOwned,
{
    let payload = {
        let current = current
            .lock()
            .map_err(|_| anyhow!("reactive document mutex poisoned"))?;
        serialize_typed(&*current)?
    };
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
    if !state.dirty && !state.flushing && payload != state.last_clean_payload {
        state.dirty = true;
    }
    Ok(state.dirty && !state.flushing)
}

fn flush_reactive_document<T>(
    cache: &Arc<Mutex<CultCache>>,
    current: &Arc<Mutex<T>>,
    state: &Arc<Mutex<CultNetReactiveDocumentState<T>>>,
    record_key: &str,
) -> Result<()>
where
    T: DatabaseEntry + Serialize + DeserializeOwned,
{
    let predicted = {
        let mut state_guard = state
            .lock()
            .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        if !state_guard.dirty {
            drop(state_guard);
            if !detect_reactive_document_change::<T>(current, state)? {
                return Ok(());
            }
            state_guard = state
                .lock()
                .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        }
        if state_guard.flushing {
            state_guard.flush_queued = true;
            return Ok(());
        }
        state_guard.flushing = true;
        state_guard.dirty = false;
        current
            .lock()
            .map_err(|_| anyhow!("reactive document mutex poisoned"))?
            .clone()
    };
    let payload = serialize_typed(&predicted)?;
    {
        let mut cache = cache
            .lock()
            .map_err(|_| anyhow!("CultCache mutex poisoned"))?;
        cache.put::<T>(record_key.to_string(), &predicted)?;
    }
    let should_flush_again = {
        let mut state = state
            .lock()
            .map_err(|_| anyhow!("reactive document state mutex poisoned"))?;
        state.flushing = false;
        state.last_clean_payload = payload;
        state.last_error = None;
        state.reconciliation = None;
        let changed_after_flush = {
            let current = current
                .lock()
                .map_err(|_| anyhow!("reactive document mutex poisoned"))?;
            serialize_typed(&*current)? != state.last_clean_payload
        };
        if changed_after_flush {
            state.dirty = true;
        }
        let should_flush_again = state.flush_queued || state.dirty;
        state.flush_queued = false;
        should_flush_again
    };
    if should_flush_again {
        flush_reactive_document::<T>(cache, current, state, record_key)?;
    }
    Ok(())
}

fn serialize_typed<T>(value: &T) -> Result<Vec<u8>>
where
    T: Serialize,
{
    Ok(rmp_serde::to_vec(value)?)
}

fn create_reconciliation_delta<T>(predicted: &T, canonical: &T) -> Result<BTreeMap<String, Value>>
where
    T: Serialize,
{
    let predicted = serde_json::to_value(predicted)?;
    let canonical = serde_json::to_value(canonical)?;
    let mut delta = BTreeMap::new();
    match (predicted, canonical) {
        (Value::Object(predicted), Value::Object(canonical)) => {
            for (key, predicted_value) in predicted {
                let canonical_value = canonical.get(&key).cloned().unwrap_or(Value::Null);
                if predicted_value == canonical_value {
                    continue;
                }
                delta.insert(
                    key,
                    numeric_delta(&predicted_value, &canonical_value).unwrap_or(predicted_value),
                );
            }
        }
        (Value::Array(predicted), Value::Array(canonical)) => {
            for (index, predicted_value) in predicted.into_iter().enumerate() {
                let canonical_value = canonical.get(index).cloned().unwrap_or(Value::Null);
                if predicted_value == canonical_value {
                    continue;
                }
                delta.insert(
                    index.to_string(),
                    numeric_delta(&predicted_value, &canonical_value).unwrap_or(predicted_value),
                );
            }
        }
        (predicted, canonical) if predicted != canonical => {
            delta.insert(
                "value".to_string(),
                numeric_delta(&predicted, &canonical).unwrap_or(predicted),
            );
        }
        _ => {}
    }
    Ok(delta)
}

fn numeric_delta(predicted: &Value, canonical: &Value) -> Option<Value> {
    let predicted = predicted.as_f64()?;
    let canonical = canonical.as_f64()?;
    serde_json::Number::from_f64(predicted - canonical).map(Value::Number)
}

fn round_trip_typed<T>(value: &T) -> Result<T>
where
    T: Serialize + DeserializeOwned,
{
    Ok(rmp_serde::from_slice(&rmp_serde::to_vec(value)?)?)
}

fn now_utc_second() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

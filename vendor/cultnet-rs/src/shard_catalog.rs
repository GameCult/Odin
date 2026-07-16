use anyhow::Result;
use anyhow::anyhow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::CultNetMessage;
use crate::CultNetShardDescriptor;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CultNetShardCatalogOptions {
    pub schema_ids: Option<Vec<String>>,
    pub record_keys: Option<Vec<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct CultNetShardCatalog {
    shards: BTreeMap<String, CultNetShardDescriptor>,
}

impl CultNetShardCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn upsert(&mut self, descriptor: CultNetShardDescriptor) -> Result<()> {
        validate_descriptor(&descriptor)?;
        self.shards.insert(descriptor.shard_id.clone(), descriptor);
        Ok(())
    }

    pub fn get(&self, shard_id: &str) -> Option<&CultNetShardDescriptor> {
        self.shards.get(shard_id)
    }

    pub fn list(&self, options: &CultNetShardCatalogOptions) -> Vec<CultNetShardDescriptor> {
        let schema_ids = options
            .schema_ids
            .as_ref()
            .map(|values| values.iter().cloned().collect::<BTreeSet<_>>());
        let record_keys = options
            .record_keys
            .as_ref()
            .map(|values| values.iter().cloned().collect::<BTreeSet<_>>());

        self.shards
            .values()
            .filter(|shard| {
                let schema_match = schema_ids.as_ref().is_none_or(|requested| {
                    requested
                        .iter()
                        .any(|schema_id| shard.serves(Some(schema_id), None))
                });
                let key_match = record_keys.as_ref().is_none_or(|requested| {
                    requested
                        .iter()
                        .any(|record_key| shard.serves(None, Some(record_key)))
                });
                schema_match && key_match
            })
            .cloned()
            .collect()
    }

    pub fn create_catalog_response(&self, request: &CultNetMessage) -> Result<CultNetMessage> {
        let CultNetMessage::ShardCatalogRequest {
            message_id,
            schema_ids,
            record_keys,
        } = request
        else {
            return Err(anyhow!(
                "expected cultnet.shard_catalog_request.v0 for shard discovery"
            ));
        };

        Ok(CultNetMessage::ShardCatalogResponse {
            message_id: message_id.clone(),
            shards: self.list(&CultNetShardCatalogOptions {
                schema_ids: schema_ids.clone(),
                record_keys: record_keys.clone(),
            }),
        })
    }

    pub fn apply_response(
        &mut self,
        response: &CultNetMessage,
    ) -> Result<Vec<CultNetShardDescriptor>> {
        let CultNetMessage::ShardCatalogResponse { shards, .. } = response else {
            return Err(anyhow!(
                "expected cultnet.shard_catalog_response.v0 for shard discovery"
            ));
        };

        let mut applied = Vec::with_capacity(shards.len());
        for shard in shards {
            self.upsert(shard.clone())?;
            applied.push(shard.clone());
        }
        Ok(applied)
    }
}

fn validate_descriptor(descriptor: &CultNetShardDescriptor) -> Result<()> {
    if descriptor.shard_id.trim().is_empty() {
        return Err(anyhow!("shard_id must be non-empty"));
    }
    if descriptor.owner_runtime_id.trim().is_empty() {
        return Err(anyhow!("owner_runtime_id must be non-empty"));
    }
    for schema_id in &descriptor.schema_ids {
        if schema_id.trim().is_empty() {
            return Err(anyhow!("schema_ids must not contain empty values"));
        }
    }
    if descriptor
        .key_prefix
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(anyhow!("key_prefix must be non-empty when present"));
    }
    for endpoint in descriptor
        .primary_endpoints
        .iter()
        .chain(descriptor.replica_endpoints.iter())
        .chain(descriptor.read_replica_endpoints.iter())
    {
        if endpoint.trim().is_empty() {
            return Err(anyhow!("shard endpoints must not contain empty values"));
        }
    }
    Ok(())
}

use anyhow::{Context, Result};
use cultmesh_rs::CultMesh;
use odin_core::{EVE_PROVIDER_ADVERTISEMENT_SCHEMA, OdinDocuments};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let store = PathBuf::from(env::args().nth(1).context("usage: dump_provider_catalog <store>")?);
    let node = CultMesh::create_node(&store, OdinDocuments, Default::default())
        .with_context(|| format!("opening {}", store.display()))?;
    for envelope in node.cache().snapshot() {
        if envelope.schema_id.as_deref() != Some(EVE_PROVIDER_ADVERTISEMENT_SCHEMA) {
            continue;
        }
        let value: serde_json::Value = rmp_serde::from_slice(&envelope.payload)?;
        println!("key={} value={}", envelope.key, serde_json::to_string(&value)?);
    }
    Ok(())
}

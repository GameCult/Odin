use anyhow::{Context, Result, anyhow};
use cultmesh_rs::CultMesh;
use odin_core::{OdinDocuments, SleipnirInputMappingRecord};
use serde_json::json;
use std::{env, path::PathBuf};

fn required_arg(name: &str) -> Result<String> {
    let prefix = format!("--{name}=");
    env::args()
        .find_map(|arg| arg.strip_prefix(&prefix).map(ToString::to_string))
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("missing {prefix}<value>"))
}

fn main() -> Result<()> {
    let store = PathBuf::from(required_arg("store")?);
    let provider_id = required_arg("provider-id")?;
    let device_filter = required_arg("device-filter")?;
    let stream_id = required_arg("stream-id")?;
    let updated_at = required_arg("updated-at")?;
    let mut node = CultMesh::create_node(&store, OdinDocuments, Default::default())
        .with_context(|| format!("opening {}", store.display()))?;
    let record = SleipnirInputMappingRecord {
        provider_id: provider_id.clone(),
        enabled: true,
        device_filter,
        stream_id,
        presentation: "xbox360".to_string(),
        axis_map: json!({}),
        button_map: json!({}),
        pending_learn: json!({}),
        updated_at,
        source: "sleipnir-input-field-fixture".to_string(),
    };
    node.put(&provider_id, &record)?;
    node.flush()?;
    println!("wrote enabled Sleipnir input mapping for {provider_id}");
    Ok(())
}

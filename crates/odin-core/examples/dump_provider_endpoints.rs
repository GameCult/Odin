use anyhow::{Context, Result};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{
    EveProviderAdvertisementRecord, EveSurfaceStateRecord, MUNINN_HID_CONTROLLER_STATE_SCHEMA,
    OdinDocuments, OdinEndpointQuery, discover_provider_endpoints,
};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let store = PathBuf::from(
        args.next()
            .context("usage: dump_provider_endpoints STORE [HOST] [DEVICE]")?,
    );
    let host = args.next();
    let device = args.next();
    let node = CultMesh::create_node(
        &store,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "odin-dump-provider-endpoints".to_string(),
            pull_on_start: true,
        },
    )
    .with_context(|| format!("opening {}", store.display()))?;

    println!("provider advertisements:");
    for provider in node
        .cache()
        .get_all::<EveProviderAdvertisementRecord>()
        .unwrap_or_default()
    {
        println!("{}", serde_json::to_string_pretty(&provider.value)?);
    }

    println!("endpoint matches:");
    for endpoint in discover_provider_endpoints(
        &node,
        OdinEndpointQuery {
            schema: Some(MUNINN_HID_CONTROLLER_STATE_SCHEMA),
            transport_contains: Some("rudp"),
            host_hint: host.as_deref(),
            device_filter: device.as_deref(),
        },
    ) {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "address": endpoint.address,
                "schema": endpoint.schema,
                "transport": endpoint.transport,
                "streamId": endpoint.stream_id,
            }))?
        );
    }

    println!("surface states:");
    for surface in node
        .cache()
        .get_all::<EveSurfaceStateRecord>()
        .unwrap_or_default()
    {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "providerId": surface.provider_id,
                "updatedAt": surface.updated_at,
                "surface": surface.surface,
            }))?
        );
    }
    Ok(())
}

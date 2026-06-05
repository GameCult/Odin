use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNode, CultMeshNodeOptions};
use odin_core::{
    EveSurfaceStateRecord, GjallarComposition, GjallarCompositionInput, GjallarOverviewRecord,
    GjallarOverviewTileRecord, OdinDocuments, OdinInterfaceRecord, OdinObservationStreamRecord,
    OdinServiceRecord, OdinSnapshotRecord, OdinTranslationRouteRecord, OdinVerseRecord,
    compose_gjallar_overview,
};
use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
struct Options {
    odin_store_path: PathBuf,
    gjallar_store_path: PathBuf,
    overview_id: String,
    surface_key: String,
    target_columns: u32,
    verse_keys: Vec<String>,
    service_keys: Vec<String>,
    interface_keys: Vec<String>,
    observation_stream_keys: Vec<String>,
    translation_route_keys: Vec<String>,
    interval_seconds: Option<u64>,
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;

    if let Some(parent) = options.gjallar_store_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    if let Some(interval_seconds) = options.interval_seconds {
        loop {
            run_cycle(&options)?;
            thread::sleep(Duration::from_secs(interval_seconds));
        }
    }

    run_cycle(&options)
}

fn run_cycle(options: &Options) -> Result<()> {
    if !options.odin_store_path.exists() {
        return Err(anyhow!(
            "Odin CultMesh store not found: {}",
            options.odin_store_path.display()
        ));
    }

    let odin = CultMesh::create_node(
        &options.odin_store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "gjallar-odin-reader".to_string(),
            pull_on_start: true,
        },
    )?;

    let mut gjallar = CultMesh::create_node(
        &options.gjallar_store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "gjallar-daemon".to_string(),
            pull_on_start: true,
        },
    )?;

    let composition = match odin.get::<OdinSnapshotRecord>("latest")? {
        Some(snapshot) => compose_gjallar_overview(GjallarCompositionInput {
            overview_id: options.overview_id.clone(),
            composed_at: timestamp()?,
            target_columns: options.target_columns,
            snapshot,
            verses: read_many::<OdinVerseRecord>(&odin, &options.verse_keys)?,
            services: read_many::<OdinServiceRecord>(&odin, &options.service_keys)?,
            interfaces: read_many::<OdinInterfaceRecord>(&odin, &options.interface_keys)?,
            observation_streams: read_many::<OdinObservationStreamRecord>(
                &odin,
                &options.observation_stream_keys,
            )?,
            translation_routes: read_many::<OdinTranslationRouteRecord>(
                &odin,
                &options.translation_route_keys,
            )?,
        }),
        None => compose_from_compat_surface(
            &options.overview_id,
            options.target_columns,
            timestamp()?,
            read_compat_surface(&odin, &options.surface_key)?,
        ),
    };

    gjallar.put(&composition.overview.overview_id, &composition.overview)?;
    for tile in &composition.tiles {
        gjallar.put(&tile.tile_id, tile)?;
    }

    println!(
        "Gjallar composed {} tiles for {}",
        composition.overview.tile_count, composition.overview.overview_id
    );
    println!("Odin source: {}", options.odin_store_path.display());
    println!("Gjallar feed: {}", options.gjallar_store_path.display());
    Ok(())
}

fn read_many<T>(node: &CultMeshNode, keys: &[String]) -> Result<Vec<T>>
where
    T: cultcache_rs::DatabaseEntry,
{
    let mut records = Vec::new();
    for key in keys {
        if let Some(record) = node.get::<T>(key)? {
            records.push(record);
        }
    }
    Ok(records)
}

fn read_compat_surface(node: &CultMeshNode, key: &str) -> Result<serde_json::Value> {
    let envelope = node
        .cache()
        .get_required_envelope::<EveSurfaceStateRecord>(key)?;
    rmp_serde::from_slice(&envelope.payload).with_context(|| {
        format!("decoding CommonJS Odin surface payload at {key:?} as MessagePack map")
    })
}

fn compose_from_compat_surface(
    overview_id: &str,
    target_columns: u32,
    composed_at: String,
    surface: serde_json::Value,
) -> GjallarComposition {
    let provider_id = string_field(&surface, "providerId", "surface");
    let detail = surface
        .get("surface")
        .and_then(|surface| surface.get("root"))
        .or_else(|| surface.get("root"))
        .and_then(|root| root.get("props"))
        .and_then(|props| props.get("summary"))
        .and_then(|summary| summary.as_str())
        .unwrap_or("Odin CommonJS surface");
    let title = string_field(&surface, "title", &provider_id);
    let updated_at = string_field(&surface, "updatedAt", "unknown");
    let tile = GjallarOverviewTileRecord {
        tile_id: format!("{overview_id}:tile:0"),
        overview_id: overview_id.to_string(),
        source_record: format!("gamecult.eve.surface_state:{provider_id}"),
        tile_kind: "compat-surface".to_string(),
        title,
        state: "active".to_string(),
        detail: detail.to_string(),
        priority: 0,
        row_span: 1,
        column_span: target_columns.max(1),
        observed_at: updated_at.clone(),
    };
    GjallarComposition {
        overview: GjallarOverviewRecord {
            overview_id: overview_id.to_string(),
            source_snapshot_id: provider_id,
            title: "Gjallar".to_string(),
            status: "active".to_string(),
            summary: "1 drawable tile from Odin compatibility surface".to_string(),
            tile_count: 1,
            target_columns: target_columns.max(1),
            target_rows: 1,
            source_observed_at: updated_at,
            composed_at,
        },
        tiles: vec![tile],
    }
}

fn string_field(value: &serde_json::Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(|entry| entry.as_str())
        .filter(|entry| !entry.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let repo_root = find_repo_root(env::current_dir().context("reading current directory")?);
        let mut options = Options {
            odin_store_path: repo_root.join("scratch").join("odin").join("odin.ccmp"),
            gjallar_store_path: repo_root
                .join("scratch")
                .join("gjallar")
                .join("gjallar.overview.ccmp"),
            overview_id: "gjallar.overview:nightwing".to_string(),
            surface_key: "surface:gamecult.network.status".to_string(),
            target_columns: 4,
            verse_keys: split_keys(
                "starfire.local,nightwing.local,periwinkle.local,yggdrasil.local",
            ),
            service_keys: split_keys("odin,cultcache,interfaces,gpu"),
            interface_keys: split_keys("odin.allseer"),
            observation_stream_keys: split_keys("periwinkle:motion:sensor,periwinkle:camera:media"),
            translation_route_keys: Vec::new(),
            interval_seconds: None,
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--odin-store" => {
                    options.odin_store_path = PathBuf::from(take_value(&mut args, "--odin-store")?)
                }
                "--store" | "--gjallar-store" => {
                    options.gjallar_store_path =
                        PathBuf::from(take_value(&mut args, "--gjallar-store")?)
                }
                "--overview" => options.overview_id = take_value(&mut args, "--overview")?,
                "--surface-key" => options.surface_key = take_value(&mut args, "--surface-key")?,
                "--columns" => {
                    options.target_columns = take_value(&mut args, "--columns")?
                        .parse()
                        .context("--columns must be a positive integer")?
                }
                "--verses" => options.verse_keys = split_keys(take_value(&mut args, "--verses")?),
                "--services" => {
                    options.service_keys = split_keys(take_value(&mut args, "--services")?)
                }
                "--interfaces" => {
                    options.interface_keys = split_keys(take_value(&mut args, "--interfaces")?)
                }
                "--streams" => {
                    options.observation_stream_keys =
                        split_keys(take_value(&mut args, "--streams")?)
                }
                "--routes" => {
                    options.translation_route_keys = split_keys(take_value(&mut args, "--routes")?)
                }
                "--interval-seconds" => {
                    options.interval_seconds = Some(
                        take_value(&mut args, "--interval-seconds")?
                            .parse()
                            .context("--interval-seconds must be a positive integer")?,
                    )
                }
                "--help" | "-h" => return Err(anyhow!(help_text())),
                other => {
                    return Err(anyhow!(
                        "unknown Gjallar argument: {other}\n\n{}",
                        help_text()
                    ));
                }
            }
        }

        if options.overview_id.trim().is_empty() {
            return Err(anyhow!("--overview must not be empty"));
        }
        if options.target_columns == 0 {
            return Err(anyhow!("--columns must be greater than zero"));
        }
        if options.interval_seconds == Some(0) {
            return Err(anyhow!("--interval-seconds must be greater than zero"));
        }

        Ok(options)
    }
}

fn take_value(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn split_keys(value: impl AsRef<str>) -> Vec<String> {
    value
        .as_ref()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn find_repo_root(start: PathBuf) -> PathBuf {
    for candidate in start.ancestors() {
        if candidate.join("Cargo.toml").exists() && candidate.join("personas").is_dir() {
            return candidate.to_path_buf();
        }
    }
    start
}

fn timestamp() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    Ok(format!("unix:{seconds}"))
}

fn help_text() -> &'static str {
    "Usage: gjallar [--odin-store <path>] [--gjallar-store <path>] [--overview <id>] [--surface-key <key>] [--columns <n>] [--verses <keys>] [--services <keys>] [--interfaces <keys>] [--streams <keys>] [--routes <keys>] [--interval-seconds <seconds>]\n\nGjallar reads Odin-owned typed records and publishes a compact typed overview feed for Nightwing."
}

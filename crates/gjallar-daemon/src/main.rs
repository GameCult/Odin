use anyhow::{Context, Result, anyhow};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{GjallarAffordanceRecord, OdinDocuments};
use std::env;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
struct Options {
    store_path: PathBuf,
    persona_store_path: PathBuf,
    persona_key: String,
    verse_id: Option<String>,
    interval_seconds: Option<u64>,
}

fn main() -> Result<()> {
    let options = Options::parse(env::args().skip(1))?;

    if let Some(parent) = options.store_path.parent() {
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
    if !options.persona_store_path.exists() {
        return Err(anyhow!(
            "Gjallar Persona CultCache store not found: {}",
            options.persona_store_path.display()
        ));
    }

    let now = timestamp()?;
    let source_record = format!("gamecult.persona_state:{}", options.persona_key);
    let affordance = GjallarAffordanceRecord {
        affordance_id: format!("gjallar:{}:transmit-context", options.persona_key),
        source_record,
        verse_id: options.verse_id.clone(),
        surface_kind: "persona".to_string(),
        action: "transmit-context".to_string(),
        authority: "gjallar".to_string(),
        status: "available".to_string(),
        provenance: options.persona_store_path.display().to_string(),
        observed_at: now,
    };

    let mut node = CultMesh::create_node(
        &options.store_path,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "gjallar-daemon".to_string(),
            pull_on_start: true,
        },
    )?;

    node.put(&affordance.affordance_id, &affordance)?;

    println!("Gjallar published {}", affordance.affordance_id);
    println!("CultMesh store: {}", options.store_path.display());
    println!("Persona source: {}", options.persona_store_path.display());
    Ok(())
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let repo_root = find_repo_root(env::current_dir().context("reading current directory")?);
        let mut options = Options {
            store_path: repo_root
                .join("scratch")
                .join("gjallar")
                .join("gjallar.affordances.cc"),
            persona_store_path: repo_root.join("personas").join("gjallar.persona_state.cc"),
            persona_key: "persona:gjallar".to_string(),
            verse_id: Some("odin.local".to_string()),
            interval_seconds: None,
        };

        let mut args = args.peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--store" => options.store_path = PathBuf::from(take_value(&mut args, "--store")?),
                "--persona-store" => {
                    options.persona_store_path =
                        PathBuf::from(take_value(&mut args, "--persona-store")?)
                }
                "--persona-key" => options.persona_key = take_value(&mut args, "--persona-key")?,
                "--verse" => options.verse_id = Some(take_value(&mut args, "--verse")?),
                "--no-verse" => options.verse_id = None,
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

        if options.persona_key.trim().is_empty() {
            return Err(anyhow!("--persona-key must not be empty"));
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
    "Usage: gjallar [--store <path>] [--persona-store <path>] [--persona-key <key>] [--verse <id>|--no-verse] [--interval-seconds <seconds>]\n\nGjallar reads its canonical Persona CultCache source and publishes typed gjallar.affordance.v1 state through CultMesh."
}

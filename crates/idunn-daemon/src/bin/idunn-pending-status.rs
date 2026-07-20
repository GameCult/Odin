use anyhow::{Context, Result, anyhow};
use cultcache_rs::{DatabaseEntry, SingleFileMessagePackBackingStore};
use odin_core::{IdunnCurrentDeploymentRequestRecord, IdunnDeploymentRequestRecord};
use serde::Serialize;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut store = None;
    let mut daemon = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--store" => store = Some(PathBuf::from(args.next().ok_or_else(|| anyhow!("--store requires a path"))?)),
            "--daemon" => daemon = Some(args.next().ok_or_else(|| anyhow!("--daemon requires an id"))?),
            _ => return Err(anyhow!("usage: idunn-pending-status --store <path> [--daemon <id>]")),
        }
    }
    let store = store.ok_or_else(|| anyhow!("--store is required"))?;
    let entries = SingleFileMessagePackBackingStore::new(&store)
        .pull_all_read_only_snapshot()
        .with_context(|| format!("reading Idunn store {}", store.display()))?;
    let mut heads = entries.iter()
        .filter(|entry| entry.r#type == IdunnCurrentDeploymentRequestRecord::TYPE)
        .map(|entry| rmp_serde::from_slice::<IdunnCurrentDeploymentRequestRecord>(&entry.payload))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    heads.sort_by(|left, right| left.daemon_id.cmp(&right.daemon_id));
    for head in heads.into_iter().filter(|head| daemon.as_ref().is_none_or(|id| id == &head.daemon_id)) {
        let request = entries.iter()
            .find(|entry| entry.r#type == IdunnDeploymentRequestRecord::TYPE && entry.key == head.request_id)
            .map(|entry| rmp_serde::from_slice::<IdunnDeploymentRequestRecord>(&entry.payload))
            .transpose()?
            .ok_or_else(|| anyhow!("head {} lost request {}", head.daemon_id, head.request_id))?;
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct PendingStatus<'a> {
            schema_version: &'static str,
            daemon_id: &'a str,
            request_id: &'a str,
            state: &'a str,
            execution_subphase: &'a str,
            reason_code: &'a str,
            sequence: u64,
            source_revision: &'a str,
            release_authority_id: &'a str,
            release_authority_envelope_sha256: &'a str,
            deployment_brake_authorization_id: &'a str,
            deployment_brake_envelope_sha256: &'a str,
            owner_incarnation_id: &'a str,
            private_state_exposed: bool,
        }
        println!("{}", serde_json::to_string(&PendingStatus {
            schema_version: "idunn.pending_deployment_status.v1",
            daemon_id: &head.daemon_id,
            request_id: &head.request_id,
            state: &head.state,
            execution_subphase: &head.execution_subphase,
            reason_code: &head.reason_code,
            sequence: head.sequence,
            source_revision: &request.source_revision,
            release_authority_id: &request.release_authority_id,
            release_authority_envelope_sha256: &request.release_authority_envelope_sha256,
            deployment_brake_authorization_id: &head.deployment_brake_authorization_id,
            deployment_brake_envelope_sha256: &head.deployment_brake_envelope_sha256,
            owner_incarnation_id: &head.owner_incarnation_id,
            private_state_exposed: false,
        })?);
    }
    Ok(())
}

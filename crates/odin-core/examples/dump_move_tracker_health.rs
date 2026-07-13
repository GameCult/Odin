use anyhow::{Context, Result};
use cultmesh_rs::{CultMesh, CultMeshNodeOptions};
use odin_core::{MuninnMoveTrackerHealthRecord, OdinDocuments};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let store = PathBuf::from(
        env::args()
            .nth(1)
            .context("usage: dump_move_tracker_health STORE")?,
    );
    let node = CultMesh::create_node(
        &store,
        OdinDocuments,
        CultMeshNodeOptions {
            runtime_id: "odin-dump-move-tracker-health".to_string(),
            pull_on_start: true,
        },
    )
    .with_context(|| format!("opening {}", store.display()))?;

    let mut records = node
        .cache()
        .get_all::<MuninnMoveTrackerHealthRecord>()
        .unwrap_or_default();
    records.sort_by(|left, right| left.camera_id.cmp(&right.camera_id));
    for record in records {
        println!(
            "camera={} index={} state={} backend={}/{} size={}x{} exposure={} calibrated={} updates={} observations={} latest={} last_observation={} mean_rgb={:?} peak_rgb={:?} color_pixels={:?} updated={} detail={}",
            record.camera_id,
            record.camera_index,
            record.state,
            record.camera_api,
            record.camera_name,
            record.width,
            record.height,
            record.exposure,
            record.calibrated_controller_count,
            record.update_count,
            record.observation_count,
            record.latest_observation_count,
            record.last_observation_at,
            record.image_mean_rgb,
            record.image_peak_rgb,
            record.color_evidence_move_ids.iter().cloned().zip(record.color_evidence_pixel_counts.iter().copied()).collect::<Vec<_>>(),
            record.updated_at,
            record.detail,
        );
    }
    Ok(())
}

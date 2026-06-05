use crate::documents::{
    GjallarOverviewRecord, GjallarOverviewTileRecord, OdinInterfaceRecord,
    OdinObservationStreamRecord, OdinServiceRecord, OdinSnapshotRecord, OdinTranslationRouteRecord,
    OdinVerseRecord,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GjallarCompositionInput {
    pub overview_id: String,
    pub composed_at: String,
    pub target_columns: u32,
    pub snapshot: OdinSnapshotRecord,
    pub verses: Vec<OdinVerseRecord>,
    pub services: Vec<OdinServiceRecord>,
    pub interfaces: Vec<OdinInterfaceRecord>,
    pub observation_streams: Vec<OdinObservationStreamRecord>,
    pub translation_routes: Vec<OdinTranslationRouteRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GjallarComposition {
    pub overview: GjallarOverviewRecord,
    pub tiles: Vec<GjallarOverviewTileRecord>,
}

pub fn compose_gjallar_overview(input: GjallarCompositionInput) -> GjallarComposition {
    let mut tiles = Vec::new();
    let overview_id = input.overview_id;
    let target_columns = input.target_columns.max(1);

    tiles.push(tile(
        &overview_id,
        0,
        "odin.snapshot:latest",
        "summary",
        "Odin",
        snapshot_status(&input.snapshot),
        format!(
            "{} verses / {} services / {} interfaces / {} streams",
            input.snapshot.verse_count,
            input.snapshot.service_count,
            input.snapshot.interface_count,
            input.snapshot.observation_stream_count
        ),
        input.snapshot.observed_at.clone(),
    ));

    for (index, interface) in input.interfaces.iter().enumerate() {
        tiles.push(tile(
            &overview_id,
            100 + index as i32,
            format!("odin.interface:{}", interface.provider_id),
            "interface",
            interface.title.clone(),
            interface.state.clone(),
            interface.source.clone(),
            interface.observed_at.clone(),
        ));
    }

    for (index, stream) in input.observation_streams.iter().enumerate() {
        tiles.push(tile(
            &overview_id,
            300 + index as i32,
            format!("odin.observation_stream:{}", stream.stream_key),
            "observation-stream",
            format!("{} {}", stream.device_id, stream.stream_id),
            stream.state.clone(),
            stream.detail.clone(),
            stream.observed_at.clone(),
        ));
    }

    for (index, service) in input.services.iter().enumerate() {
        tiles.push(tile(
            &overview_id,
            500 + index as i32,
            format!("odin.service:{}", service.service_id),
            "service",
            service.name.clone(),
            service.state.clone(),
            service.detail.clone(),
            service.observed_at.clone(),
        ));
    }

    for (index, verse) in input.verses.iter().enumerate() {
        tiles.push(tile(
            &overview_id,
            700 + index as i32,
            format!("odin.verse:{}", verse.verse_id),
            "verse",
            verse.name.clone(),
            verse.status.clone(),
            verse.role.clone(),
            verse.observed_at.clone(),
        ));
    }

    for (index, route) in input.translation_routes.iter().enumerate() {
        tiles.push(tile(
            &overview_id,
            900 + index as i32,
            format!("odin.translation_route:{}", route.route_id),
            "translation-route",
            route.target_schema.clone(),
            route.translation_kind.clone(),
            route.owner.clone(),
            input.snapshot.observed_at.clone(),
        ));
    }

    tiles.sort_by(|left, right| left.priority.cmp(&right.priority));
    let target_rows = (tiles.len() as u32).div_ceil(target_columns).max(1);
    let status = if tiles.iter().any(|entry| entry.state == "failed") {
        "degraded"
    } else {
        "active"
    };

    let overview = GjallarOverviewRecord {
        overview_id: overview_id.clone(),
        source_snapshot_id: input.snapshot.snapshot_id,
        title: "Gjallar".to_string(),
        status: status.to_string(),
        summary: format!("{} drawable tiles from Odin sight", tiles.len()),
        tile_count: tiles.len() as u32,
        target_columns,
        target_rows,
        source_observed_at: input.snapshot.observed_at,
        composed_at: input.composed_at,
    };

    GjallarComposition { overview, tiles }
}

fn tile(
    overview_id: &str,
    priority: i32,
    source_record: impl Into<String>,
    tile_kind: impl Into<String>,
    title: impl Into<String>,
    state: impl Into<String>,
    detail: impl Into<String>,
    observed_at: impl Into<String>,
) -> GjallarOverviewTileRecord {
    GjallarOverviewTileRecord {
        tile_id: format!("{}:tile:{priority}", overview_id),
        overview_id: overview_id.to_string(),
        source_record: source_record.into(),
        tile_kind: tile_kind.into(),
        title: title.into(),
        state: state.into(),
        detail: detail.into(),
        priority,
        row_span: 1,
        column_span: 1,
        observed_at: observed_at.into(),
    }
}

fn snapshot_status(snapshot: &OdinSnapshotRecord) -> String {
    if snapshot.service_count == 0 && snapshot.interface_count == 0 {
        "waiting".to_string()
    } else {
        "active".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn compose_overview_orders_dashboard_tiles_for_nightwing() {
        let composition = compose_gjallar_overview(GjallarCompositionInput {
            overview_id: "gjallar.overview:nightwing".to_string(),
            composed_at: "2026-06-05T00:00:00Z".to_string(),
            target_columns: 2,
            snapshot: OdinSnapshotRecord {
                snapshot_id: "latest".to_string(),
                observed_at: "2026-06-05T00:00:00Z".to_string(),
                verse_count: 1,
                service_count: 1,
                interface_count: 1,
                observation_stream_count: 1,
                source: "unit-test".to_string(),
            },
            verses: vec![OdinVerseRecord {
                verse_id: "nightwing.local".to_string(),
                name: "Nightwing".to_string(),
                role: "terminal-lowerer".to_string(),
                status: "active".to_string(),
                capabilities: vec!["tui".to_string()],
                observed_at: "2026-06-05T00:00:00Z".to_string(),
            }],
            services: vec![OdinServiceRecord {
                service_id: "odin".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Odin".to_string(),
                state: "active".to_string(),
                detail: "all-seer".to_string(),
                authority: "odin".to_string(),
                observed_at: "2026-06-05T00:00:00Z".to_string(),
            }],
            interfaces: vec![OdinInterfaceRecord {
                provider_id: "odin.allseer".to_string(),
                title: "Odin All-Seer".to_string(),
                state: "active".to_string(),
                source: "cultmesh".to_string(),
                version: Some("v1".to_string()),
                updated_at: None,
                observed_at: "2026-06-05T00:00:00Z".to_string(),
            }],
            observation_streams: vec![OdinObservationStreamRecord {
                stream_key: "periwinkle:motion:sensor".to_string(),
                device_id: "periwinkle".to_string(),
                stream_id: "motion".to_string(),
                kind: "sensor".to_string(),
                state: "active".to_string(),
                detail: "fresh".to_string(),
                owner: "mimir".to_string(),
                observed_at: "2026-06-05T00:00:00Z".to_string(),
            }],
            translation_routes: vec![OdinTranslationRouteRecord {
                route_id: "source=>target".to_string(),
                source_schema: "source".to_string(),
                target_schema: "target".to_string(),
                translation_kind: "projection".to_string(),
                owner: "odin".to_string(),
                version: "v1".to_string(),
                notes: "test".to_string(),
            }],
        });

        assert_eq!(composition.overview.tile_count, 6);
        assert_eq!(composition.overview.target_rows, 3);
        assert_eq!(composition.tiles[0].tile_kind, "summary");
        assert_eq!(
            composition.tiles[1].source_record,
            "odin.interface:odin.allseer"
        );
    }
}

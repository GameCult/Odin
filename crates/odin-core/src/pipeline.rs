use crate::documents::{
    GjallarAffordanceRecord, OdinInterfaceRecord, OdinObservationStreamRecord, OdinRecords,
    OdinServiceRecord, OdinSnapshotRecord, OdinTranslationRouteRecord, OdinVerseRecord,
};
use crate::ports::{
    Clock, GjallarAffordanceIngest, InterfaceIngest, ObservationStreamIngest, OdinInputBatch,
    ServiceIngest, TranslationRouteIngest, VerseIngest,
};
use anyhow::Result;

pub struct OdinIngestPipeline<V, S, I, O, T, G, C> {
    verses: V,
    services: S,
    interfaces: I,
    streams: O,
    translations: T,
    gjallar: G,
    clock: C,
}

impl<V, S, I, O, T, G, C> OdinIngestPipeline<V, S, I, O, T, G, C>
where
    V: VerseIngest,
    S: ServiceIngest,
    I: InterfaceIngest,
    O: ObservationStreamIngest,
    T: TranslationRouteIngest,
    G: GjallarAffordanceIngest,
    C: Clock,
{
    pub fn new(
        verses: V,
        services: S,
        interfaces: I,
        streams: O,
        translations: T,
        gjallar: G,
        clock: C,
    ) -> Self {
        Self {
            verses,
            services,
            interfaces,
            streams,
            translations,
            gjallar,
            clock,
        }
    }

    pub fn collect(&self) -> Result<OdinInputBatch> {
        Ok(OdinInputBatch {
            observed_at: self.clock.now(),
            source: "odin.rust.ingest".to_string(),
            verses: self.verses.observe_verses()?,
            services: self.services.observe_services()?,
            interfaces: self.interfaces.observe_interfaces()?,
            observation_streams: self.streams.observe_streams()?,
            translation_routes: self.translations.observe_translation_routes()?,
            gjallar_affordances: self.gjallar.observe_gjallar_affordances()?,
        })
    }
}

pub fn normalize_odin_records(input: OdinInputBatch) -> OdinRecords {
    let verses = input
        .verses
        .into_iter()
        .map(|entry| OdinVerseRecord {
            verse_id: entry.verse_id,
            name: entry.name,
            role: entry.role,
            status: entry.status,
            capabilities: entry.capabilities,
            observed_at: input.observed_at.clone(),
        })
        .collect::<Vec<_>>();

    let services = input
        .services
        .into_iter()
        .map(|entry| OdinServiceRecord {
            service_id: entry.service_id,
            verse_id: entry.verse_id,
            name: entry.name,
            state: entry.state,
            detail: entry.detail,
            authority: entry.authority,
            observed_at: input.observed_at.clone(),
        })
        .collect::<Vec<_>>();

    let interfaces = input
        .interfaces
        .into_iter()
        .map(|entry| OdinInterfaceRecord {
            provider_id: entry.provider_id,
            title: entry.title,
            state: entry.state,
            source: entry.source,
            version: entry.version,
            updated_at: entry.updated_at,
            observed_at: input.observed_at.clone(),
        })
        .collect::<Vec<_>>();

    let observation_streams = input
        .observation_streams
        .into_iter()
        .map(|entry| {
            let stream_key = format!("{}:{}:{}", entry.device_id, entry.stream_id, entry.kind);
            OdinObservationStreamRecord {
                stream_key,
                device_id: entry.device_id,
                stream_id: entry.stream_id,
                kind: entry.kind,
                state: entry.state,
                detail: entry.detail,
                owner: entry.owner,
                observed_at: input.observed_at.clone(),
            }
        })
        .collect::<Vec<_>>();

    let translation_routes = input
        .translation_routes
        .into_iter()
        .map(|entry| {
            let route_id = format!("{}=>{}", entry.source_schema, entry.target_schema);
            OdinTranslationRouteRecord {
                route_id,
                source_schema: entry.source_schema,
                target_schema: entry.target_schema,
                translation_kind: entry.translation_kind,
                owner: entry.owner,
                version: entry.version,
                notes: entry.notes,
            }
        })
        .collect::<Vec<_>>();

    let gjallar_affordances = input
        .gjallar_affordances
        .into_iter()
        .enumerate()
        .map(|(index, entry)| GjallarAffordanceRecord {
            affordance_id: format!("gjallar:{}:{}", entry.source_record, index),
            source_record: entry.source_record,
            verse_id: entry.verse_id,
            surface_kind: entry.surface_kind,
            action: entry.action,
            authority: entry.authority,
            status: entry.status,
            provenance: entry.provenance,
            observed_at: input.observed_at.clone(),
        })
        .collect::<Vec<_>>();

    let snapshot = OdinSnapshotRecord {
        snapshot_id: "latest".to_string(),
        observed_at: input.observed_at,
        verse_count: verses.len() as u32,
        service_count: services.len() as u32,
        interface_count: interfaces.len() as u32,
        observation_stream_count: observation_streams.len() as u32,
        source: input.source,
    };

    OdinRecords {
        snapshot: Some(snapshot),
        verses,
        services,
        interfaces,
        observation_streams,
        translation_routes,
        gjallar_affordances,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        FixedClock, GjallarAffordanceObservation, InterfaceObservation,
        ObservationStreamObservation, ServiceObservation, TranslationRouteObservation,
        VerseObservation,
    };
    use anyhow::Result;
    use pretty_assertions::assert_eq;

    #[derive(Clone, Debug, Default)]
    struct MockIngest;

    impl VerseIngest for MockIngest {
        fn observe_verses(&self) -> Result<Vec<VerseObservation>> {
            Ok(vec![VerseObservation {
                verse_id: "starfire.local".to_string(),
                name: "Starfire".to_string(),
                role: "coordinator".to_string(),
                status: "active".to_string(),
                capabilities: vec!["cultmesh".to_string()],
            }])
        }
    }

    impl ServiceIngest for MockIngest {
        fn observe_services(&self) -> Result<Vec<ServiceObservation>> {
            Ok(vec![ServiceObservation {
                service_id: "odin".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Odin".to_string(),
                state: "active".to_string(),
                detail: "typed state".to_string(),
                authority: "odin".to_string(),
            }])
        }
    }

    impl InterfaceIngest for MockIngest {
        fn observe_interfaces(&self) -> Result<Vec<InterfaceObservation>> {
            Ok(vec![InterfaceObservation {
                provider_id: "odin.allseer".to_string(),
                title: "Odin".to_string(),
                state: "active".to_string(),
                source: "cultmesh".to_string(),
                version: Some("1".to_string()),
                updated_at: None,
            }])
        }
    }

    impl ObservationStreamIngest for MockIngest {
        fn observe_streams(&self) -> Result<Vec<ObservationStreamObservation>> {
            Ok(vec![ObservationStreamObservation {
                device_id: "periwinkle".to_string(),
                stream_id: "motion".to_string(),
                kind: "sensor".to_string(),
                state: "active".to_string(),
                detail: "fresh".to_string(),
                owner: "mimir".to_string(),
            }])
        }
    }

    impl TranslationRouteIngest for MockIngest {
        fn observe_translation_routes(&self) -> Result<Vec<TranslationRouteObservation>> {
            Ok(vec![TranslationRouteObservation {
                source_schema: "mimir.eve_sensor_observation.v1".to_string(),
                target_schema: "odin.observation_stream.v1".to_string(),
                translation_kind: "projection".to_string(),
                owner: "odin".to_string(),
                version: "v1".to_string(),
                notes: "dashboard projection".to_string(),
            }])
        }
    }

    impl GjallarAffordanceIngest for MockIngest {
        fn observe_gjallar_affordances(&self) -> Result<Vec<GjallarAffordanceObservation>> {
            Ok(vec![GjallarAffordanceObservation {
                source_record: "odin.service:odin".to_string(),
                verse_id: Some("starfire.local".to_string()),
                surface_kind: "service".to_string(),
                action: "inspect".to_string(),
                authority: "odin".to_string(),
                status: "available".to_string(),
                provenance: "unit-test".to_string(),
            }])
        }
    }

    #[test]
    fn pipeline_collects_from_injected_ports() -> Result<()> {
        let pipeline = OdinIngestPipeline::new(
            MockIngest,
            MockIngest,
            MockIngest,
            MockIngest,
            MockIngest,
            MockIngest,
            FixedClock::new("2026-06-03T00:00:00Z"),
        );

        let records = normalize_odin_records(pipeline.collect()?);

        assert_eq!(records.snapshot.unwrap().service_count, 1);
        assert_eq!(records.verses[0].verse_id, "starfire.local");
        assert_eq!(records.services[0].authority, "odin");
        assert_eq!(
            records.observation_streams[0].stream_key,
            "periwinkle:motion:sensor"
        );
        assert_eq!(
            records.translation_routes[0].route_id,
            "mimir.eve_sensor_observation.v1=>odin.observation_stream.v1"
        );
        assert_eq!(
            records.gjallar_affordances[0].affordance_id,
            "gjallar:odin.service:odin:0"
        );
        Ok(())
    }
}

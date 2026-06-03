use crate::documents::{
    OdinDocuments, OdinRecords, OdinServiceRecord, OdinSnapshotRecord, OdinVerseRecord,
};
use anyhow::Result;
use cultmesh_rs::{CultMesh, CultMeshNode, CultMeshNodeOptions};
use std::path::Path;

pub trait OdinRepository {
    fn persist_records(&mut self, records: &OdinRecords) -> Result<()>;
}

pub struct CultMeshOdinRepository {
    node: CultMeshNode,
}

impl CultMeshOdinRepository {
    pub fn open(store_path: impl AsRef<Path>, runtime_id: impl Into<String>) -> Result<Self> {
        let node = CultMesh::create_node(
            store_path,
            OdinDocuments,
            CultMeshNodeOptions {
                runtime_id: runtime_id.into(),
                pull_on_start: true,
            },
        )?;
        Ok(Self { node })
    }

    pub fn get_snapshot(&self) -> Result<OdinSnapshotRecord> {
        self.node.get_required::<OdinSnapshotRecord>("latest")
    }

    pub fn get_service(&self, service_id: &str) -> Result<Option<OdinServiceRecord>> {
        self.node.get::<OdinServiceRecord>(service_id)
    }

    pub fn get_verse(&self, verse_id: &str) -> Result<Option<OdinVerseRecord>> {
        self.node.get::<OdinVerseRecord>(verse_id)
    }

    pub fn document_binding_version(&self, document_type: &str) -> Option<String> {
        self.node
            .documents()
            .binding(document_type)
            .and_then(|binding| binding.payload_schema_version.clone())
    }
}

impl OdinRepository for CultMeshOdinRepository {
    fn persist_records(&mut self, records: &OdinRecords) -> Result<()> {
        if let Some(snapshot) = &records.snapshot {
            self.node.put("latest", snapshot)?;
        }
        for verse in &records.verses {
            self.node.put(&verse.verse_id, verse)?;
        }
        for service in &records.services {
            self.node.put(&service.service_id, service)?;
        }
        for interface in &records.interfaces {
            self.node.put(&interface.provider_id, interface)?;
        }
        for stream in &records.observation_streams {
            self.node.put(&stream.stream_key, stream)?;
        }
        for route in &records.translation_routes {
            self.node.put(&route.route_id, route)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct InMemoryOdinRepository {
    pub records: OdinRecords,
}

impl OdinRepository for InMemoryOdinRepository {
    fn persist_records(&mut self, records: &OdinRecords) -> Result<()> {
        self.records = records.clone();
        Ok(())
    }
}

pub fn persist_pipeline_records(
    repository: &mut impl OdinRepository,
    records: OdinRecords,
) -> Result<OdinRecords> {
    repository.persist_records(&records)?;
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::ODIN_SERVICE_SCHEMA;
    use crate::pipeline::normalize_odin_records;
    use crate::ports::{
        InterfaceObservation, ObservationStreamObservation, OdinInputBatch, ServiceObservation,
        TranslationRouteObservation, VerseObservation,
    };
    use pretty_assertions::assert_eq;

    fn sample_records() -> OdinRecords {
        normalize_odin_records(OdinInputBatch {
            observed_at: "2026-06-03T00:00:00Z".to_string(),
            source: "unit-test".to_string(),
            verses: vec![VerseObservation {
                verse_id: "starfire.local".to_string(),
                name: "Starfire".to_string(),
                role: "coordinator".to_string(),
                status: "active".to_string(),
                capabilities: vec!["cultmesh".to_string()],
            }],
            services: vec![ServiceObservation {
                service_id: "odin".to_string(),
                verse_id: "starfire.local".to_string(),
                name: "Odin".to_string(),
                state: "active".to_string(),
                detail: "persisted".to_string(),
                authority: "odin".to_string(),
            }],
            interfaces: vec![InterfaceObservation {
                provider_id: "odin.allseer".to_string(),
                title: "Odin".to_string(),
                state: "active".to_string(),
                source: "cultmesh".to_string(),
                version: Some("1".to_string()),
                updated_at: None,
            }],
            observation_streams: vec![ObservationStreamObservation {
                device_id: "periwinkle".to_string(),
                stream_id: "camera".to_string(),
                kind: "media".to_string(),
                state: "active".to_string(),
                detail: "fresh".to_string(),
                owner: "mimir".to_string(),
            }],
            translation_routes: vec![TranslationRouteObservation {
                source_schema: "source.v1".to_string(),
                target_schema: "target.v1".to_string(),
                translation_kind: "projection".to_string(),
                owner: "odin".to_string(),
                version: "v1".to_string(),
                notes: "test".to_string(),
            }],
        })
    }

    #[test]
    fn memory_repository_supports_fast_unit_tests() -> Result<()> {
        let records = sample_records();
        let mut repository = InMemoryOdinRepository::default();

        repository.persist_records(&records)?;

        assert_eq!(repository.records.services[0].service_id, "odin");
        Ok(())
    }

    #[test]
    fn cultmesh_repository_round_trips_typed_records() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("odin.cc");
        let records = sample_records();

        let mut repository = CultMeshOdinRepository::open(&store_path, "odin-test")?;
        repository.persist_records(&records)?;
        assert_eq!(
            repository
                .document_binding_version("odin.service")
                .as_deref(),
            Some(ODIN_SERVICE_SCHEMA)
        );

        let reloaded = CultMeshOdinRepository::open(&store_path, "odin-test-reloaded")?;
        assert_eq!(reloaded.get_snapshot()?.service_count, 1);
        assert_eq!(reloaded.get_service("odin")?.unwrap().detail, "persisted");
        assert_eq!(
            reloaded.get_verse("starfire.local")?.unwrap().capabilities,
            vec!["cultmesh".to_string()]
        );
        Ok(())
    }
}

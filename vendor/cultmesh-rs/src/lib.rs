use anyhow::Result;
use cultcache_rs::CultCache;
use cultcache_rs::DatabaseEntry;
use cultcache_rs::SingleFileMessagePackBackingStore;
use cultnet_rs::CultNetDocumentRegistry;
use std::path::Path;

pub trait CultMeshDocumentSet: Clone + Send + Sync + 'static {
    fn register_cache(&self, cache: &mut CultCache) -> Result<()>;
    fn register_documents(&self, registry: &mut CultNetDocumentRegistry) -> Result<()>;
}

#[macro_export]
macro_rules! cultmesh_documents {
    ($name:ident { $($entry:ty => $schema_version:expr),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $name;

        impl $crate::CultMeshDocumentSet for $name {
            fn register_cache(
                &self,
                cache: &mut cultcache_rs::CultCache,
            ) -> ::anyhow::Result<()> {
                $(
                    cache.register_entry_type::<$entry>()?;
                )*
                Ok(())
            }

            fn register_documents(
                &self,
                registry: &mut cultnet_rs::CultNetDocumentRegistry,
            ) -> ::anyhow::Result<()> {
                $(
                    registry.register(
                        cultnet_rs::CultNetDocumentBinding::for_entry::<$entry>(
                            $schema_version.to_string(),
                        ),
                    );
                )*
                Ok(())
            }
        }
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CultMeshNodeOptions {
    pub runtime_id: String,
    pub pull_on_start: bool,
}

impl Default for CultMeshNodeOptions {
    fn default() -> Self {
        Self {
            runtime_id: "cultmesh-local".to_string(),
            pull_on_start: true,
        }
    }
}

pub struct CultMeshNode {
    runtime_id: String,
    cache: CultCache,
    documents: CultNetDocumentRegistry,
}

impl CultMeshNode {
    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn cache(&self) -> &CultCache {
        &self.cache
    }

    pub fn documents(&self) -> &CultNetDocumentRegistry {
        &self.documents
    }

    pub fn get<T: DatabaseEntry>(&self, key: &str) -> Result<Option<T>> {
        self.cache.get::<T>(key)
    }

    pub fn get_required<T: DatabaseEntry>(&self, key: &str) -> Result<T> {
        self.cache.get_required::<T>(key)
    }

    pub fn put<T: DatabaseEntry>(&mut self, key: impl Into<String>, value: &T) -> Result<T> {
        self.cache.put(key, value)
    }

    pub fn delete<T: DatabaseEntry>(&mut self, key: &str) -> Result<bool> {
        self.cache.delete::<T>(key)
    }

    pub fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CultMesh;

impl CultMesh {
    pub fn create_node<D>(
        store_path: impl AsRef<Path>,
        documents: D,
        options: CultMeshNodeOptions,
    ) -> Result<CultMeshNode>
    where
        D: CultMeshDocumentSet,
    {
        let mut cache = CultCache::new();
        documents.register_cache(&mut cache)?;
        cache
            .add_generic_backing_store(SingleFileMessagePackBackingStore::new(store_path.as_ref()));
        if options.pull_on_start {
            cache.pull_all_backing_stores()?;
        }

        let mut registry = CultNetDocumentRegistry::new();
        documents.register_documents(&mut registry)?;

        Ok(CultMeshNode {
            runtime_id: options.runtime_id,
            cache,
            documents: registry,
        })
    }

    pub fn start_node<D>(
        store_path: impl AsRef<Path>,
        documents: D,
        options: CultMeshNodeOptions,
    ) -> Result<CultMeshNode>
    where
        D: CultMeshDocumentSet,
    {
        Self::create_node(store_path, documents, options)
    }
}

pub fn create_node<D>(
    store_path: impl AsRef<Path>,
    documents: D,
    options: CultMeshNodeOptions,
) -> Result<CultMeshNode>
where
    D: CultMeshDocumentSet,
{
    CultMesh::create_node(store_path, documents, options)
}

pub fn start_node<D>(
    store_path: impl AsRef<Path>,
    documents: D,
    options: CultMeshNodeOptions,
) -> Result<CultMeshNode>
where
    D: CultMeshDocumentSet,
{
    CultMesh::start_node(store_path, documents, options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[derive(Clone, Debug, PartialEq, Eq, cultcache_rs::DatabaseEntry)]
    #[cultcache(type = "cultmesh.test.note", schema = "CultMeshTestNote")]
    struct Note {
        #[cultcache(key = 0)]
        body: String,
    }

    cultmesh_documents!(TestDocuments {
        Note => "cultmesh.test.note.v0",
    });

    #[test]
    fn node_round_trips_registered_documents() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("cultmesh.cc");
        let note = Note {
            body: "blessed circuit".to_string(),
        };

        let mut node = CultMesh::create_node(
            &store_path,
            TestDocuments,
            CultMeshNodeOptions {
                runtime_id: "test-runtime".to_string(),
                ..CultMeshNodeOptions::default()
            },
        )?;
        assert_eq!(node.runtime_id(), "test-runtime");
        node.put("note", &note)?;
        node.flush()?;

        let reloaded = CultMesh::create_node(&store_path, TestDocuments, Default::default())?;
        assert_eq!(reloaded.get_required::<Note>("note")?, note);
        assert!(
            reloaded
                .documents()
                .binding_by_schema_id("cultmesh.test.note.v0")
                .is_some()
        );
        Ok(())
    }
}

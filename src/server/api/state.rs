use super::super::{now_unix_ms, FileRegistry, RequestActivity};
use crate::annotations::AnnotationStore;
use crate::api::contracts::TagNode;
use crate::pixels::{self, FrameCache, RawFrameCache};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct AppState {
    registry: FileRegistry,
    pixel_cache: Arc<Mutex<FrameCache>>,
    raw_cache: Arc<Mutex<RawFrameCache>>,
    tag_cache: Arc<Mutex<HashMap<usize, Vec<TagNode>>>>,
    annotations: AnnotationStore,
    server_start_ms: u64,
    activity: RequestActivity,
}

impl AppState {
    pub fn new(registry: FileRegistry, annotations: AnnotationStore) -> Self {
        Self {
            registry,
            pixel_cache: pixels::new_cache(),
            raw_cache: pixels::new_raw_cache(),
            tag_cache: Arc::new(Mutex::new(HashMap::new())),
            annotations,
            server_start_ms: now_unix_ms(),
            activity: RequestActivity::new(),
        }
    }

    pub fn registry(&self) -> &FileRegistry {
        &self.registry
    }

    pub fn activity(&self) -> &RequestActivity {
        &self.activity
    }

    pub(crate) fn pixel_cache(&self) -> Arc<Mutex<FrameCache>> {
        self.pixel_cache.clone()
    }

    pub(crate) fn raw_cache(&self) -> Arc<Mutex<RawFrameCache>> {
        self.raw_cache.clone()
    }

    pub(crate) fn cached_tags(&self, index: usize) -> Option<Vec<TagNode>> {
        self.tag_cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(&index).cloned())
    }

    pub(crate) fn cache_tags(&self, index: usize, nodes: Vec<TagNode>) {
        if let Ok(mut cache) = self.tag_cache.lock() {
            cache.insert(index, nodes);
        }
    }

    pub(crate) fn annotations(&self) -> &AnnotationStore {
        &self.annotations
    }

    pub(crate) fn server_start_ms(&self) -> u64 {
        self.server_start_ms
    }
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use crate::annotations::AnnotationStore;
    use crate::server::FileRegistry;

    #[test]
    fn constructor_owns_ephemeral_resources() {
        let registry = FileRegistry::new();
        let state = AppState::new(registry.clone(), AnnotationStore::empty());

        assert_eq!(state.registry().status().file_count, 0);
        assert_eq!(registry.status().file_count, 0);
        assert!(state.server_start_ms() > 0);
        assert_eq!(state.activity().in_flight(), 0);
    }
}

use super::super::{now_unix_ms, FileRegistry, RequestActivity};
use crate::annotations::AnnotationStore;
use crate::api::contracts::{SemanticContextResponse, TagNode};
use crate::pixels::{self, FrameCache, OverlayCache, RawFrameCache};
use crate::types::OverlayCacheKey;
use crate::value_mapping::FileValueMappings;
use bytes::Bytes;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

/// Serialized tag trees kept for recently viewed files. Trees are capped in
/// size by the tag serializer's limits, so a file count bounds the memory.
const TAG_CACHE_MAX_FILES: NonZeroUsize = NonZeroUsize::new(64).expect("non-zero");

/// Semantic contexts of recently viewed objects. A segmentation overlay
/// needs its context for every frame, and building it reads the object.
const SEMANTIC_CACHE_MAX_FILES: NonZeroUsize = NonZeroUsize::new(16).expect("non-zero");

/// A semantic context is valid for the file set it was resolved against;
/// the registry only grows, so its length identifies that set.
type SemanticCacheKey = (usize, usize);

/// Parsed value mappings of recently viewed files. A value readout asks for
/// them on every frame change, and reading them parses the whole header.
const VALUE_MAPPING_CACHE_MAX_FILES: NonZeroUsize = NonZeroUsize::new(16).expect("non-zero");

#[derive(Clone)]
pub struct AppState {
    registry: FileRegistry,
    pixel_cache: Arc<Mutex<FrameCache>>,
    raw_cache: Arc<Mutex<RawFrameCache>>,
    tag_cache: Arc<Mutex<LruCache<usize, Vec<TagNode>>>>,
    semantic_cache: Arc<Mutex<LruCache<SemanticCacheKey, Arc<SemanticContextResponse>>>>,
    value_mapping_cache: Arc<Mutex<LruCache<usize, Arc<FileValueMappings>>>>,
    overlay_cache: Arc<Mutex<OverlayCache>>,
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
            tag_cache: Arc::new(Mutex::new(LruCache::new(TAG_CACHE_MAX_FILES))),
            semantic_cache: Arc::new(Mutex::new(LruCache::new(SEMANTIC_CACHE_MAX_FILES))),
            value_mapping_cache: Arc::new(Mutex::new(LruCache::new(VALUE_MAPPING_CACHE_MAX_FILES))),
            overlay_cache: pixels::new_overlay_cache(),
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
            .and_then(|mut cache| cache.get(&index).cloned())
    }

    pub(crate) fn cache_tags(&self, index: usize, nodes: Vec<TagNode>) {
        if let Ok(mut cache) = self.tag_cache.lock() {
            cache.put(index, nodes);
        }
    }

    pub(crate) fn cached_semantic_context(
        &self,
        key: SemanticCacheKey,
    ) -> Option<Arc<SemanticContextResponse>> {
        self.semantic_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&key).cloned())
    }

    pub(crate) fn cache_semantic_context(
        &self,
        key: SemanticCacheKey,
        context: Arc<SemanticContextResponse>,
    ) {
        if let Ok(mut cache) = self.semantic_cache.lock() {
            cache.put(key, context);
        }
    }

    pub(crate) fn cached_value_mappings(&self, index: usize) -> Option<Arc<FileValueMappings>> {
        self.value_mapping_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&index).cloned())
    }

    pub(crate) fn cache_value_mappings(&self, index: usize, mappings: Arc<FileValueMappings>) {
        if let Ok(mut cache) = self.value_mapping_cache.lock() {
            cache.put(index, mappings);
        }
    }

    pub(crate) fn cached_overlay(&self, key: &OverlayCacheKey) -> Option<Bytes> {
        self.overlay_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(key))
    }

    pub(crate) fn cache_overlay(&self, key: OverlayCacheKey, png: Bytes) {
        if let Ok(mut cache) = self.overlay_cache.lock() {
            cache.insert(key, png);
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

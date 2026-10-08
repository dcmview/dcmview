use super::super::{now_unix_ms, FileRegistry, RequestActivity};
use super::auth::AccessToken;
use crate::annotations::AnnotationStore;
use crate::api::contracts::{SemanticContextResponse, TagNode};
use crate::pixels::{self, FrameCache, OverlayCache, RawFrameCache, ThumbnailCache};
use crate::redactions::RedactionStore;
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

/// A semantic context or value mapping is valid for the file set it was
/// resolved against; the registry only grows, so its length identifies that
/// set.
type FileSetCacheKey = (usize, usize);

/// Parsed value mappings of recently viewed files. A value readout asks for
/// them on every frame change, and reading them parses the whole header and
/// every RWVM instance in the file set.
const VALUE_MAPPING_CACHE_MAX_FILES: NonZeroUsize = NonZeroUsize::new(16).expect("non-zero");

#[derive(Clone)]
pub struct AppState {
    registry: FileRegistry,
    pixel_cache: Arc<Mutex<FrameCache>>,
    raw_cache: Arc<Mutex<RawFrameCache>>,
    tag_cache: Arc<Mutex<LruCache<usize, Vec<TagNode>>>>,
    semantic_cache: Arc<Mutex<LruCache<FileSetCacheKey, Arc<SemanticContextResponse>>>>,
    value_mapping_cache: Arc<Mutex<LruCache<FileSetCacheKey, Arc<FileValueMappings>>>>,
    overlay_cache: Arc<Mutex<OverlayCache>>,
    thumbnail_cache: Arc<Mutex<ThumbnailCache>>,
    annotations: AnnotationStore,
    redactions: RedactionStore,
    server_start_ms: u64,
    activity: RequestActivity,
    access_token: Option<AccessToken>,
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
            thumbnail_cache: pixels::new_thumbnail_cache(),
            annotations,
            redactions: RedactionStore::new(),
            server_start_ms: now_unix_ms(),
            activity: RequestActivity::new(),
            access_token: None,
        }
    }

    /// Requires `token` on every `/api` request. Without this call the API
    /// is open, which is what `--no-token` and in-process tests use.
    pub fn with_access_token(mut self, token: AccessToken) -> Self {
        self.access_token = Some(token);
        self
    }

    /// Replaces the display, raw, overlay and thumbnail caches with ones sized by
    /// `budget`. Call it before the state is cloned into a router.
    pub fn with_cache_budget(mut self, budget: pixels::CacheBudget) -> Self {
        self.pixel_cache = Arc::new(Mutex::new(FrameCache::new(budget.frame_bytes)));
        self.raw_cache = Arc::new(Mutex::new(RawFrameCache::new(budget.raw_bytes)));
        self.overlay_cache = Arc::new(Mutex::new(OverlayCache::new(budget.overlay_bytes)));
        self.thumbnail_cache = Arc::new(Mutex::new(ThumbnailCache::new(budget.thumbnail_bytes)));
        self
    }

    pub fn access_token(&self) -> Option<&AccessToken> {
        self.access_token.as_ref()
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

    pub(crate) fn thumbnail_cache(&self) -> Arc<Mutex<ThumbnailCache>> {
        self.thumbnail_cache.clone()
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
        key: FileSetCacheKey,
    ) -> Option<Arc<SemanticContextResponse>> {
        self.semantic_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&key).cloned())
    }

    pub(crate) fn cache_semantic_context(
        &self,
        key: FileSetCacheKey,
        context: Arc<SemanticContextResponse>,
    ) {
        if let Ok(mut cache) = self.semantic_cache.lock() {
            cache.put(key, context);
        }
    }

    pub(crate) fn cached_value_mappings(
        &self,
        key: FileSetCacheKey,
    ) -> Option<Arc<FileValueMappings>> {
        self.value_mapping_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&key).cloned())
    }

    pub(crate) fn cache_value_mappings(
        &self,
        key: FileSetCacheKey,
        mappings: Arc<FileValueMappings>,
    ) {
        if let Ok(mut cache) = self.value_mapping_cache.lock() {
            cache.put(key, mappings);
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

    pub(crate) fn redactions(&self) -> &RedactionStore {
        &self.redactions
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

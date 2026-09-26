use crate::api::contracts::RawFrameMetadata;
use crate::types::{FrameCacheKey, RawFrameCacheKey};
use bytes::Bytes;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

pub const CACHE_CAPACITY: usize = 128;
// Keep these budgets in sync with README memory guidance and frontend frame retention.
pub const FRAME_CACHE_MAX_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
pub const RAW_CACHE_CAPACITY: usize = 512;
pub const RAW_CACHE_MAX_BYTES: usize = 384 * 1024 * 1024; // 384 MiB

pub struct FrameCache {
    entries: LruCache<FrameCacheKey, Bytes>,
    bytes: usize,
}

impl FrameCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: LruCache::new(NonZeroUsize::new(capacity).expect("non-zero cache capacity")),
            bytes: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &FrameCacheKey) -> Option<Bytes> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn insert_with_budget(&mut self, key: FrameCacheKey, body: Bytes, max_bytes: usize) {
        let incoming = body.len();
        if incoming > max_bytes {
            return;
        }

        if let Some(existing) = self.entries.pop(&key) {
            self.bytes = self.bytes.saturating_sub(existing.len());
        }

        while self.bytes.saturating_add(incoming) > max_bytes {
            let Some((_, evicted)) = self.entries.pop_lru() else {
                return;
            };
            self.bytes = self.bytes.saturating_sub(evicted.len());
        }

        self.entries.put(key, body);
        self.bytes = self.bytes.saturating_add(incoming);
    }
}

pub struct RawFrameCache {
    entries: LruCache<RawFrameCacheKey, (Bytes, RawFrameMetadata)>,
    bytes: usize,
}

impl RawFrameCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: LruCache::new(
                NonZeroUsize::new(capacity).expect("non-zero raw cache capacity"),
            ),
            bytes: 0,
        }
    }

    pub(crate) fn get(&mut self, key: &RawFrameCacheKey) -> Option<(Bytes, RawFrameMetadata)> {
        self.entries.get(key).cloned()
    }

    pub(crate) fn insert_with_budget(
        &mut self,
        key: RawFrameCacheKey,
        body: Bytes,
        metadata: RawFrameMetadata,
        max_bytes: usize,
    ) {
        let incoming = body.len();
        if incoming > max_bytes {
            return;
        }

        if let Some((existing, _)) = self.entries.pop(&key) {
            self.bytes = self.bytes.saturating_sub(existing.len());
        }

        while self.bytes.saturating_add(incoming) > max_bytes {
            let Some((_, (evicted, _))) = self.entries.pop_lru() else {
                return;
            };
            self.bytes = self.bytes.saturating_sub(evicted.len());
        }

        self.entries.put(key, (body, metadata));
        self.bytes = self.bytes.saturating_add(incoming);
    }
}

pub fn new_cache() -> Arc<Mutex<FrameCache>> {
    Arc::new(Mutex::new(FrameCache::new(CACHE_CAPACITY)))
}

pub fn new_raw_cache() -> Arc<Mutex<RawFrameCache>> {
    Arc::new(Mutex::new(RawFrameCache::new(RAW_CACHE_CAPACITY)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::contracts::WindowMode;

    fn frame_key(frame: u32) -> FrameCacheKey {
        FrameCacheKey::new(0, frame, None, None, WindowMode::Default)
    }

    fn raw_key(frame: u32) -> RawFrameCacheKey {
        RawFrameCacheKey {
            file_index: 0,
            frame,
        }
    }

    fn raw_meta() -> RawFrameMetadata {
        RawFrameMetadata {
            rows: 1,
            columns: 1,
            bits_allocated: 8,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            default_wc: None,
            default_ww: None,
            padding_low: None,
            padding_high: None,
        }
    }

    fn frame_cache_contains(cache: &FrameCache, key: &FrameCacheKey) -> bool {
        cache
            .entries
            .iter()
            .any(|(cached_key, _)| cached_key == key)
    }

    fn raw_cache_contains(cache: &RawFrameCache, key: &RawFrameCacheKey) -> bool {
        cache
            .entries
            .iter()
            .any(|(cached_key, _)| cached_key == key)
    }

    #[test]
    fn frame_cache_budget_evicts_lru_entries() {
        let mut cache = FrameCache::new(4);
        let key0 = frame_key(0);
        let key1 = frame_key(1);
        let key2 = frame_key(2);

        cache.insert_with_budget(key0.clone(), Bytes::from(vec![0_u8; 4]), 8);
        cache.insert_with_budget(key1.clone(), Bytes::from(vec![1_u8; 4]), 8);
        cache.insert_with_budget(key2.clone(), Bytes::from(vec![2_u8; 4]), 8);

        assert!(
            !frame_cache_contains(&cache, &key0),
            "least-recently-used entry should be evicted"
        );
        assert!(
            frame_cache_contains(&cache, &key1),
            "second entry should still be cached"
        );
        assert!(
            frame_cache_contains(&cache, &key2),
            "new entry should be cached"
        );
        assert_eq!(cache.bytes, 8);
    }

    #[test]
    fn frame_cache_budget_skips_oversized_entries() {
        let mut cache = FrameCache::new(4);
        let key0 = frame_key(0);

        cache.insert_with_budget(key0.clone(), Bytes::from(vec![0_u8; 9]), 8);

        assert!(
            !frame_cache_contains(&cache, &key0),
            "oversized entry should be skipped"
        );
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn raw_cache_budget_evicts_lru_entries() {
        let mut cache = RawFrameCache::new(4);
        let key0 = raw_key(0);
        let key1 = raw_key(1);
        let key2 = raw_key(2);

        cache.insert_with_budget(key0.clone(), Bytes::from(vec![0_u8; 4]), raw_meta(), 8);
        cache.insert_with_budget(key1.clone(), Bytes::from(vec![1_u8; 4]), raw_meta(), 8);
        cache.insert_with_budget(key2.clone(), Bytes::from(vec![2_u8; 4]), raw_meta(), 8);

        assert!(
            !raw_cache_contains(&cache, &key0),
            "least-recently-used raw entry should be evicted"
        );
        assert!(
            raw_cache_contains(&cache, &key1),
            "second raw entry should still be cached"
        );
        assert!(
            raw_cache_contains(&cache, &key2),
            "new raw entry should be cached"
        );
        assert_eq!(cache.bytes, 8);
    }

    #[test]
    fn frame_cache_replacement_updates_tracked_bytes() {
        let mut cache = FrameCache::new(4);
        let key = frame_key(0);

        cache.insert_with_budget(key.clone(), Bytes::from(vec![0_u8; 6]), 8);
        cache.insert_with_budget(key.clone(), Bytes::from(vec![1_u8; 3]), 8);

        assert!(frame_cache_contains(&cache, &key));
        assert_eq!(cache.bytes, 3);
    }
}

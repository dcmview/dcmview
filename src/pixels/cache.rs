use crate::api::contracts::RawFrameMetadata;
use crate::types::{FrameCacheKey, OverlayCacheKey, RawFrameCacheKey};
use bytes::Bytes;
use lru::LruCache;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

pub const CACHE_CAPACITY: usize = 128;
// Keep these budgets in sync with README memory guidance and frontend frame retention.
pub const FRAME_CACHE_MAX_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
pub const RAW_CACHE_CAPACITY: usize = 512;
pub const RAW_CACHE_MAX_BYTES: usize = 384 * 1024 * 1024; // 384 MiB
pub const OVERLAY_CACHE_CAPACITY: usize = 256;
pub const OVERLAY_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Encoded display frames keyed by file, frame, and window request.
pub type FrameCache = BudgetedLru<FrameCacheKey, Bytes>;
/// Decoded raw samples and their metadata keyed by file and frame.
pub type RawFrameCache = BudgetedLru<RawFrameCacheKey, (Bytes, RawFrameMetadata)>;
/// Encoded value-overlay PNGs keyed by overlay object and displayed frame.
pub type OverlayCache = BudgetedLru<OverlayCacheKey, Bytes>;

/// A cached value whose memory cost is the length of its frame body.
pub trait FrameBody: Clone {
    fn body_len(&self) -> usize;
}

impl FrameBody for Bytes {
    fn body_len(&self) -> usize {
        self.len()
    }
}

impl FrameBody for (Bytes, RawFrameMetadata) {
    fn body_len(&self) -> usize {
        self.0.len()
    }
}

/// An LRU bounded by both an entry count and a total body-byte budget.
///
/// Callers hold the surrounding mutex only for `get` and `insert`; decoding
/// and encoding happen outside the lock.
pub struct BudgetedLru<K: Hash + Eq, V> {
    entries: LruCache<K, V>,
    bytes: usize,
    max_bytes: usize,
}

impl<K: Hash + Eq, V: FrameBody> BudgetedLru<K, V> {
    fn new(capacity: usize, max_bytes: usize) -> Self {
        Self {
            entries: LruCache::new(NonZeroUsize::new(capacity).expect("non-zero cache capacity")),
            bytes: 0,
            max_bytes,
        }
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        self.entries.get(key).cloned()
    }

    /// Inserts `value`, evicting least-recently-used entries until it fits the
    /// byte budget. A value larger than the whole budget is not cached.
    pub(crate) fn insert(&mut self, key: K, value: V) {
        let incoming = value.body_len();
        if incoming > self.max_bytes {
            return;
        }

        if let Some(existing) = self.entries.pop(&key) {
            self.bytes = self.bytes.saturating_sub(existing.body_len());
        }

        while self.bytes.saturating_add(incoming) > self.max_bytes {
            let Some((_, evicted)) = self.entries.pop_lru() else {
                return;
            };
            self.bytes = self.bytes.saturating_sub(evicted.body_len());
        }

        // The entry-count bound may still evict an entry the byte budget kept.
        if let Some((_, evicted)) = self.entries.push(key, value) {
            self.bytes = self.bytes.saturating_sub(evicted.body_len());
        }
        self.bytes = self.bytes.saturating_add(incoming);
    }
}

pub fn new_cache() -> Arc<Mutex<FrameCache>> {
    Arc::new(Mutex::new(FrameCache::new(
        CACHE_CAPACITY,
        FRAME_CACHE_MAX_BYTES,
    )))
}

pub fn new_raw_cache() -> Arc<Mutex<RawFrameCache>> {
    Arc::new(Mutex::new(RawFrameCache::new(
        RAW_CACHE_CAPACITY,
        RAW_CACHE_MAX_BYTES,
    )))
}

pub fn new_overlay_cache() -> Arc<Mutex<OverlayCache>> {
    Arc::new(Mutex::new(OverlayCache::new(
        OVERLAY_CACHE_CAPACITY,
        OVERLAY_CACHE_MAX_BYTES,
    )))
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

    fn contains<K: Hash + Eq, V: FrameBody>(cache: &BudgetedLru<K, V>, key: &K) -> bool {
        cache.entries.contains(key)
    }

    #[test]
    fn frame_cache_budget_evicts_lru_entries() {
        let mut cache = FrameCache::new(4, 8);
        let key0 = frame_key(0);
        let key1 = frame_key(1);
        let key2 = frame_key(2);

        cache.insert(key0.clone(), Bytes::from(vec![0_u8; 4]));
        cache.insert(key1.clone(), Bytes::from(vec![1_u8; 4]));
        cache.insert(key2.clone(), Bytes::from(vec![2_u8; 4]));

        assert!(
            !contains(&cache, &key0),
            "least-recently-used entry should be evicted"
        );
        assert!(
            contains(&cache, &key1),
            "second entry should still be cached"
        );
        assert!(contains(&cache, &key2), "new entry should be cached");
        assert_eq!(cache.bytes, 8);
    }

    #[test]
    fn frame_cache_budget_skips_oversized_entries() {
        let mut cache = FrameCache::new(4, 8);
        let key0 = frame_key(0);

        cache.insert(key0.clone(), Bytes::from(vec![0_u8; 9]));

        assert!(
            !contains(&cache, &key0),
            "oversized entry should be skipped"
        );
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn raw_cache_budget_evicts_lru_entries() {
        let mut cache = RawFrameCache::new(4, 8);
        let key0 = raw_key(0);
        let key1 = raw_key(1);
        let key2 = raw_key(2);

        cache.insert(key0.clone(), (Bytes::from(vec![0_u8; 4]), raw_meta()));
        cache.insert(key1.clone(), (Bytes::from(vec![1_u8; 4]), raw_meta()));
        cache.insert(key2.clone(), (Bytes::from(vec![2_u8; 4]), raw_meta()));

        assert!(
            !contains(&cache, &key0),
            "least-recently-used raw entry should be evicted"
        );
        assert!(
            contains(&cache, &key1),
            "second raw entry should still be cached"
        );
        assert!(contains(&cache, &key2), "new raw entry should be cached");
        assert_eq!(cache.bytes, 8);
    }

    #[test]
    fn frame_cache_replacement_updates_tracked_bytes() {
        let mut cache = FrameCache::new(4, 8);
        let key = frame_key(0);

        cache.insert(key.clone(), Bytes::from(vec![0_u8; 6]));
        cache.insert(key.clone(), Bytes::from(vec![1_u8; 3]));

        assert!(contains(&cache, &key));
        assert_eq!(cache.bytes, 3);
    }

    #[test]
    fn entry_count_eviction_releases_tracked_bytes() {
        let mut cache = FrameCache::new(2, 64);
        for frame in 0..3 {
            cache.insert(frame_key(frame), Bytes::from(vec![0_u8; 4]));
        }

        assert!(!contains(&cache, &frame_key(0)));
        assert_eq!(cache.bytes, 8);
    }
}

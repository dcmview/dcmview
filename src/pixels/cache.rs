use super::error::PixelError;
use crate::api::contracts::RawFrameMetadata;
use crate::types::{FrameCacheKey, OverlayCacheKey, RawFrameCacheKey};
use bytes::Bytes;
use futures::future::{BoxFuture, Shared};
use lru::LruCache;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

// Keep these budgets in sync with README memory guidance and frontend frame retention.
pub const FRAME_CACHE_MAX_BYTES: usize = 256 * 1024 * 1024; // 256 MiB
pub const RAW_CACHE_MAX_BYTES: usize = 384 * 1024 * 1024; // 384 MiB
pub const OVERLAY_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Encoded display frames keyed by file, frame, and window request.
pub type FrameCache = BudgetedLru<FrameCacheKey, Bytes>;
/// Decoded raw samples and their metadata keyed by file and frame.
pub type RawFrameCache = BudgetedLru<RawFrameCacheKey, (Bytes, RawFrameMetadata)>;
/// Encoded overlay PNGs keyed by overlay object (and SEG frame) and displayed frame.
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

/// A decode other requests for the same key await instead of repeating.
pub(crate) type InFlight<V> = Shared<BoxFuture<'static, Result<V, Arc<PixelError>>>>;

/// An LRU bounded by the total bytes of its bodies, plus the decodes
/// currently running for keys it does not hold yet. Entries are not counted:
/// a cine loop of small PNGs must fit as long as its bytes do.
///
/// Callers hold the surrounding mutex only for lookups and inserts; decoding
/// and encoding happen outside the lock.
pub struct BudgetedLru<K: Hash + Eq, V> {
    entries: LruCache<K, V>,
    bytes: usize,
    max_bytes: usize,
    in_flight: HashMap<K, InFlight<V>>,
}

impl<K: Hash + Eq, V: FrameBody> BudgetedLru<K, V> {
    fn new(max_bytes: usize) -> Self {
        Self {
            entries: LruCache::unbounded(),
            bytes: 0,
            max_bytes,
            in_flight: HashMap::new(),
        }
    }

    pub(crate) fn in_flight(&self, key: &K) -> Option<InFlight<V>> {
        self.in_flight.get(key).cloned()
    }

    pub(crate) fn start_flight(&mut self, key: K, decode: InFlight<V>) {
        self.in_flight.insert(key, decode);
    }

    pub(crate) fn finish_flight(&mut self, key: &K) {
        self.in_flight.remove(key);
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

        self.entries.put(key, value);
        self.bytes = self.bytes.saturating_add(incoming);
    }
}

pub fn new_cache() -> Arc<Mutex<FrameCache>> {
    Arc::new(Mutex::new(FrameCache::new(FRAME_CACHE_MAX_BYTES)))
}

pub fn new_raw_cache() -> Arc<Mutex<RawFrameCache>> {
    Arc::new(Mutex::new(RawFrameCache::new(RAW_CACHE_MAX_BYTES)))
}

pub fn new_overlay_cache() -> Arc<Mutex<OverlayCache>> {
    Arc::new(Mutex::new(OverlayCache::new(OVERLAY_CACHE_MAX_BYTES)))
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
        let mut cache = FrameCache::new(8);
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
        let mut cache = FrameCache::new(8);
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
        let mut cache = RawFrameCache::new(8);
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
        let mut cache = FrameCache::new(8);
        let key = frame_key(0);

        cache.insert(key.clone(), Bytes::from(vec![0_u8; 6]));
        cache.insert(key.clone(), Bytes::from(vec![1_u8; 3]));

        assert!(contains(&cache, &key));
        assert_eq!(cache.bytes, 3);
    }
}

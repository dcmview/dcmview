use super::error::PixelError;
use super::render::DisplayPng;
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
pub type FrameCache = BudgetedLru<FrameCacheKey, DisplayPng>;
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

impl FrameBody for DisplayPng {
    fn body_len(&self) -> usize {
        self.png.len()
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
    pub(crate) fn new(max_bytes: usize) -> Self {
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

/// One byte budget divided among the display, raw and overlay caches
/// (`--cache-budget`), in the proportions of the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheBudget {
    pub frame_bytes: usize,
    pub raw_bytes: usize,
    pub overlay_bytes: usize,
}

impl CacheBudget {
    pub const DEFAULT: Self = Self {
        frame_bytes: FRAME_CACHE_MAX_BYTES,
        raw_bytes: RAW_CACHE_MAX_BYTES,
        overlay_bytes: OVERLAY_CACHE_MAX_BYTES,
    };

    /// Smallest accepted total. A frame larger than its cache's share is
    /// served but not kept, so a very small budget turns every request for a
    /// large image into a decode; below this floor that is true of ordinary
    /// images too.
    pub const MIN_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

    /// Splits `total_bytes` in the proportions of [`Self::DEFAULT`], rounding
    /// each share down so the three never exceed the total. A total below
    /// [`Self::MIN_TOTAL_BYTES`] is an error that names the minimum.
    pub fn from_total(total_bytes: u64) -> Result<Self, String> {
        if total_bytes < Self::MIN_TOTAL_BYTES {
            return Err(format!(
                "cache budget must be at least {} bytes (16MiB)",
                Self::MIN_TOTAL_BYTES
            ));
        }
        let share = |bytes: usize| {
            let scaled =
                u128::from(total_bytes) * bytes as u128 / u128::from(Self::DEFAULT.total_bytes());
            usize::try_from(scaled)
                .map_err(|_| "cache budget is too large for this platform".to_string())
        };
        Ok(Self {
            frame_bytes: share(Self::DEFAULT.frame_bytes)?,
            raw_bytes: share(Self::DEFAULT.raw_bytes)?,
            overlay_bytes: share(Self::DEFAULT.overlay_bytes)?,
        })
    }

    pub fn total_bytes(&self) -> u64 {
        self.frame_bytes as u64 + self.raw_bytes as u64 + self.overlay_bytes as u64
    }
}

/// Parses a `--cache-budget` value: a whole number of bytes, optionally with
/// a binary suffix `KiB`, `MiB` or `GiB` (case-insensitive, no space), such
/// as `268435456`, `256MiB` or `2GiB`. Decimal suffixes (`MB`), fractions,
/// signs and overflow are errors.
pub fn parse_byte_size(raw: &str) -> Result<u64, String> {
    let digits = raw.bytes().take_while(u8::is_ascii_digit).count();
    let (number, suffix) = raw.split_at(digits);
    let multiplier = match suffix.to_ascii_lowercase().as_str() {
        "" => 1,
        "kib" => 1024,
        "mib" => 1024 * 1024,
        "gib" => 1024 * 1024 * 1024,
        _ => return Err("expected whole bytes or a KiB, MiB or GiB suffix, without spaces".into()),
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|bytes| bytes.checked_mul(multiplier))
        .ok_or_else(|| "expected an unsigned whole byte size that fits in 64 bits".into())
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
        FrameCacheKey::new(0, frame, None, None, WindowMode::Default, None)
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

    fn png(len: usize) -> DisplayPng {
        DisplayPng::color(Bytes::from(vec![0_u8; len]))
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

        cache.insert(key0.clone(), png(4));
        cache.insert(key1.clone(), png(4));
        cache.insert(key2.clone(), png(4));

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

        cache.insert(key0.clone(), png(9));

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

        cache.insert(key.clone(), png(6));
        cache.insert(key.clone(), png(3));

        assert!(contains(&cache, &key));
        assert_eq!(cache.bytes, 3);
    }

    #[test]
    fn cache_budget_keeps_the_default_proportions_within_the_total() {
        use super::{parse_byte_size, CacheBudget};
        const MIB: u64 = 1024 * 1024;

        assert_eq!(
            CacheBudget::from_total(CacheBudget::DEFAULT.total_bytes()),
            Ok(CacheBudget::DEFAULT)
        );
        assert_eq!(
            CacheBudget::from_total(352 * MIB),
            Ok(CacheBudget {
                frame_bytes: 128 * 1024 * 1024,
                raw_bytes: 192 * 1024 * 1024,
                overlay_bytes: 32 * 1024 * 1024,
            })
        );
        // Totals that do not divide evenly round down and never exceed the total.
        for total in [16 * MIB, 16 * MIB + 1, 100 * MIB + 7, 8 * 1024 * MIB] {
            let budget = CacheBudget::from_total(total).expect("budget");
            assert!(budget.total_bytes() <= total, "{total}");
            assert!(total - budget.total_bytes() < 3, "{total}");
            assert!(budget.raw_bytes > budget.frame_bytes, "{total}");
            assert!(budget.frame_bytes > budget.overlay_bytes, "{total}");
        }
        assert!(CacheBudget::from_total(CacheBudget::MIN_TOTAL_BYTES - 1).is_err());
        assert!(CacheBudget::from_total(0).is_err());

        for (raw, expected) in [
            ("268435456", Some(268_435_456)),
            ("512KiB", Some(512 * 1024)),
            ("256MiB", Some(256 * MIB)),
            ("256mib", Some(256 * MIB)),
            ("2GiB", Some(2 * 1024 * MIB)),
            ("", None),
            ("MiB", None),
            ("256MB", None),
            ("1.5GiB", None),
            ("-1", None),
            ("256 MiB", None),
            ("99999999999GiB", None),
        ] {
            assert_eq!(parse_byte_size(raw).ok(), expected, "{raw:?}");
        }
    }
}

//! Cancellable DICOM discovery and `FileEntry` construction.
//!
//! `discovery` walks input paths and streams one event per candidate,
//! `entry` inspects a candidate into a `FileEntry`, `metadata` extracts the
//! geometry, LUT, overlay, and shutter details that entry carries, and
//! `filter` owns the `--filter` metadata predicates.

mod discovery;
mod entry;
mod filter;
mod metadata;
#[cfg(test)]
mod test_fixtures;

pub use discovery::{
    discover, discover_progressive, discovery_cancellation_reason, DiscoverOptions,
    DiscoveryCancellation, DiscoveryCancellationReason, DiscoveryCancelled, DiscoveryDisposition,
    DiscoveryEvent, DiscoveryReason, DiscoveryRecord, DiscoveryReport,
};
pub use filter::{ScanFilter, ScanFilterField};

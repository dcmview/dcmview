//! Cancellable file discovery and `FileEntry` construction.
//!
//! `discovery` walks input paths and streams one event per candidate,
//! `entry` decides a candidate's format from its content and inspects a
//! DICOM file into a `FileEntry`, `format` owns the `--formats` selection and
//! the raster signatures, `raster` inspects a raster image's header into a
//! `FileEntry`, `metadata` extracts the geometry, LUT, overlay, and shutter
//! details a DICOM entry carries, and `filter` owns the `--filter` metadata
//! predicates.

mod discovery;
mod entry;
mod filter;
mod format;
mod metadata;
mod raster;
#[cfg(test)]
mod test_fixtures;

pub use discovery::{
    discover_progressive, discovery_cancellation_reason, DiscoverOptions, DiscoveryCancellation,
    DiscoveryCancellationReason, DiscoveryCancelled, DiscoveryDisposition, DiscoveryEvent,
    DiscoveryReason, DiscoveryRecord, DiscoveryReport,
};
pub(crate) use entry::build_label;
pub use filter::{ScanFilter, ScanFilterField};
pub use format::FormatSelection;

/// Root of the independently generated DICOM corpus that `#[ignore]` tests
/// read, from `DCMVIEW_PREPARED_CORPUS` (`python scripts/check.py corpus`).
#[cfg(test)]
pub(crate) fn prepared_corpus_root() -> std::path::PathBuf {
    let root = std::env::var_os("DCMVIEW_PREPARED_CORPUS")
        .map(std::path::PathBuf::from)
        .expect("set DCMVIEW_PREPARED_CORPUS to the generated corpus directory");
    assert!(
        root.is_dir(),
        "prepared corpus {} is not a directory",
        root.display()
    );
    root
}

/// One case file of the prepared corpus, found in a flat corpus (`all`
/// profile) or in the per-profile `core`, `extended`, or `extended-deflate`
/// roots of the original prepared layout.
#[cfg(test)]
pub(crate) fn prepared_corpus_case(relative: &str) -> std::path::PathBuf {
    let root = prepared_corpus_root();
    ["", "core", "extended", "extended-deflate"]
        .into_iter()
        .map(|profile| root.join(profile).join(relative))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("prepared corpus {} has no case {relative}", root.display()))
}

/// Inspect one Part 10 file into a `FileEntry` for unit tests that need the
/// loader's metadata without running a discovery.
#[cfg(test)]
pub(crate) fn test_entry(path: &std::path::Path) -> crate::types::FileEntry {
    match entry::build_entry(path).expect("inspect DICOM file") {
        entry::EntryInspection::Selected(file) => *file,
        entry::EntryInspection::Skipped(reason) => {
            panic!("{} was skipped: {}", path.display(), reason.code())
        }
    }
}

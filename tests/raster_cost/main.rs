//! What decoding one raster frame may cost, whatever the file holds
//! (`pixels::decode_raster_frame`, "What a decode may cost").
//!
//! A raster file is input nobody vouches for, so the limits are stated in
//! things that can be counted, and none of them is a time: the reads issued
//! to the file and the bytes they return, counted at the source the decoder
//! is given, and the heap the decoding thread holds, counted at the
//! allocator.
//!
//! These tests are a test binary of their own because they replace the
//! global allocator, and the allocator shim of a debug build checks every
//! layout it is handed: the JPEG 2000 decoder the main integration binary
//! exercises frees with layouts that fail that check.
//!
//! `RASTER_COST_REPORT=1 cargo test --test raster_cost -- --nocapture` prints
//! what each hostile case and each scaled file cost.

mod admission;
mod agreement;
mod bounds;
mod heap;
mod layout;
mod overlays;
#[path = "../integration/raster_cases.rs"]
mod raster_cases;
#[path = "../integration/raster_files.rs"]
mod raster_files;
#[path = "../integration/raster_tag_files.rs"]
mod raster_tag_files;
mod scale;
mod tags;

/// Every test of this binary runs on the counting allocator
/// (`heap::peak_during`).
#[global_allocator]
static ALLOCATOR: heap::CountingAllocator = heap::CountingAllocator;

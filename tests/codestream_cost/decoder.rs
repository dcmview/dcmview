//! Whether the JPEG XL decoder ran, told from what it logs.
//!
//! The decoder's crates log through `tracing`, each under its own crate
//! name. The header parser (`jxl_image`) and the container and bit readers
//! (`jxl_bitstream`) are what `codestream::declared` uses; every other
//! `jxl_` crate is part of the decoder proper. A test watches which of them
//! spoke, on every thread, because decodes run on blocking threads.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

static WATCHING: AtomicBool = AtomicBool::new(false);
static SPOKE: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
static INSTALLED: Once = Once::new();

struct Watcher;

fn note(metadata: &Metadata<'static>) {
    let name = metadata.target().split("::").next().unwrap_or_default();
    SPOKE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(name);
}

impl Subscriber for Watcher {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        WATCHING.load(Ordering::Relaxed) && metadata.target().starts_with("jxl_")
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        note(attributes.metadata());
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        note(event.metadata());
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// Runs `call` and returns its result with the `jxl_` crates that logged
/// anything during it, on any thread. Hold `heap::alone` around it.
pub fn crates_heard<T>(call: impl FnOnce() -> T) -> (T, BTreeSet<&'static str>) {
    INSTALLED.call_once(|| {
        tracing::subscriber::set_global_default(Watcher).expect("no other subscriber");
    });
    let spoke = || {
        SPOKE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    };
    spoke().clear();
    WATCHING.store(true, Ordering::Relaxed);
    let result = call();
    WATCHING.store(false, Ordering::Relaxed);
    let heard = spoke().clone();
    (result, heard)
}

/// The `jxl_` crates that read a header and nothing else.
pub const HEADER_READERS: [&str; 2] = ["jxl_bitstream", "jxl_image"];

//! The adapters a process offers, by id.

use crate::adapter::{AdapterInfo, Confidence, ExportAdapter, ImportAdapter};
use crate::embed::EmbedAdapter;
use crate::native::{NativeJson, NativeJsonLines};

/// The adapters a process offers. Built once and read from then on.
///
/// The built-in ones ([`Registry::builtin`]):
///
/// | Id | Reads | Writes |
/// |---|---|---|
/// | `embed` | EMBED ROI CSV | EMBED ROI CSV |
/// | `native` | a `dcmview.annotations` JSON document, and the JSON Lines form | a JSON document |
/// | `native-jsonl` | | the JSON Lines form |
///
/// An id is unique among the exporters and among the importers; an adapter
/// that does both is registered under one id for both.
pub struct Registry {
    exporters: Vec<Box<dyn ExportAdapter>>,
    importers: Vec<Box<dyn ImportAdapter>>,
}

impl Registry {
    /// A registry with no adapter.
    pub fn empty() -> Self {
        Self {
            exporters: Vec::new(),
            importers: Vec::new(),
        }
    }

    /// The built-in adapters, in the order of the table above.
    pub fn builtin() -> Self {
        let mut registry = Self::empty();
        registry.add_exporter(Box::new(EmbedAdapter));
        registry.add_importer(Box::new(EmbedAdapter));
        registry.add_exporter(Box::new(NativeJson));
        registry.add_importer(Box::new(NativeJson));
        registry.add_exporter(Box::new(NativeJsonLines));
        registry
    }

    /// Adds an exporter after the ones already there. An id already
    /// registered as an exporter is not replaced: the first one stays the
    /// one [`Registry::exporter`] finds.
    pub fn add_exporter(&mut self, adapter: Box<dyn ExportAdapter>) {
        self.exporters.push(adapter);
    }

    /// Adds an importer after the ones already there, as
    /// [`Registry::add_exporter`] does.
    pub fn add_importer(&mut self, adapter: Box<dyn ImportAdapter>) {
        self.importers.push(adapter);
    }

    /// The exporter registered under `id`.
    pub fn exporter(&self, id: &str) -> Option<&dyn ExportAdapter> {
        self.exporters
            .iter()
            .find(|adapter| adapter.info().id == id)
            .map(|adapter| &**adapter)
    }

    /// The importer registered under `id`.
    pub fn importer(&self, id: &str) -> Option<&dyn ImportAdapter> {
        self.importers
            .iter()
            .find(|adapter| adapter.info().id == id)
            .map(|adapter| &**adapter)
    }

    /// What every adapter says of itself: the exporters in their order,
    /// then the importers that are not also exporters. An adapter that
    /// does both appears once.
    pub fn infos(&self) -> Vec<AdapterInfo> {
        let mut infos: Vec<AdapterInfo> = self
            .exporters
            .iter()
            .map(|adapter| adapter.info())
            .collect();
        for adapter in &self.importers {
            let info = adapter.info();
            if !infos.iter().any(|known| known.id == info.id) {
                infos.push(info);
            }
        }
        infos
    }

    /// The importer most confident that an input starting with `head` is
    /// in its format, and how confident it is. `None` when every importer
    /// answers [`Confidence::None`]. Of two equally confident importers the
    /// one registered first wins. `head` is cut to
    /// [`crate::limits::SNIFF_BYTES`] before any importer sees it.
    pub fn sniff(&self, head: &[u8]) -> Option<(&dyn ImportAdapter, Confidence)> {
        let head = &head[..head.len().min(crate::limits::SNIFF_BYTES)];
        let mut best = None;
        let mut confidence = Confidence::None;
        for importer in &self.importers {
            let candidate = importer.sniff(head);
            if candidate > confidence {
                best = Some((&**importer, candidate));
                confidence = candidate;
            }
        }
        best
    }
}

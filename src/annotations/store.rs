//! What a session holds of annotations: the backend, and the state of the
//! `--annotations` import.

use super::memory::{MemoryBackend, MemoryConfig};
use super::{AnnotationIndexMap, EmbedRoiAnnotations};
use anyhow::{anyhow, Result};
use dcmview_adapters::Report;
use dcmview_annotation::Author;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

/// Who the rows of an EMBED CSV are authored by
/// (`docs/design/annotation-model.md` 9.2).
pub const EMBED_IMPORT_AUTHOR: &str = "import:embed";

/// Who the records of a native `--annotations` document are authored by:
/// `import:` and the id of the adapter that read them
/// (`docs/design/output-adapters.md` 8).
pub const NATIVE_IMPORT_AUTHOR: &str = "import:native";

/// Where a session's `--annotations` import stands
/// ([`AnnotationStore::import_status`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportStatus {
    /// The session was started without `--annotations`.
    None,
    /// The file is being read.
    Loading,
    /// The import ended and its records or rows are in the session. The
    /// report is what the import's adapter answered.
    Ready(Option<Report>),
    /// The import failed, with its message, and nothing of it is in the
    /// session.
    Failed(String),
}

/// The author a standalone session stamps on what its user writes:
/// `user:<name>`, the name from the environment variable `USER`, else
/// `USERNAME` (`docs/design/annotation-model.md` 12, decision 10). A name
/// that is missing, empty or not one an author may hold (it has whitespace
/// or a control character, or is too long) gives `user:local`.
pub fn session_author() -> Author {
    ["USER", "USERNAME"]
        .into_iter()
        .filter_map(|variable| std::env::var(variable).ok())
        .find_map(|name| Author::parse(&format!("user:{name}")).ok())
        .unwrap_or_else(|| Author::parse("user:local").expect("a valid author"))
}

/// How many bytes of document text the downloads being sent may hold
/// together: 512 MiB. A download that would pass it while another is being
/// sent is refused, and can be asked for again.
pub const DOWNLOAD_BUDGET_BYTES: u64 = 512 * 1024 * 1024;

/// A share of the download budget, given back when this is dropped.
#[derive(Debug)]
pub struct DownloadHold {
    held: Arc<Mutex<u64>>,
    bytes: u64,
}

impl Drop for DownloadHold {
    fn drop(&mut self) {
        if let Ok(mut held) = self.held.lock() {
            *held = held.saturating_sub(self.bytes);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoadState {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct EmbedImport {
    /// Whether the session has an `--annotations` file at all.
    expected: bool,
    state: LoadState,
    /// What the import's adapter answered, once it has ended well.
    report: Option<Report>,
    /// The files whose ROIs the EMBED endpoint has replaced. The import
    /// leaves them alone.
    edited: HashSet<usize>,
    /// The rows the import read that are not records yet, by file index, as
    /// the CSV wrote them.
    staged: HashMap<usize, EmbedRoiAnnotations>,
}

/// A session's annotations: its backend, and where the `--annotations`
/// import stands. Cheap to clone; clones share everything.
///
/// # The import
///
/// The CSV is read in the background after discovery. Until that has
/// finished, a read through the EMBED endpoints waits
/// ([`AnnotationStore::wait_until_ready`]) while writes go ahead; a file
/// the EMBED endpoint wrote to meanwhile keeps what was written and the
/// import's rows for it are dropped. An import that fails leaves the store
/// failed for the session: the EMBED reads and the export answer with its
/// message, and viewing goes on.
///
/// # Staged rows
///
/// The rows the import read are not made records at once. A record holds
/// its file's settled key, settling a key may mean reading the whole file
/// and every file that shares its UID, and an import must not cost a read
/// of the dataset before the first ROI can be shown. So the rows are
/// *staged*: kept here by file index, exactly as the CSV wrote them, at
/// the cost they had before the store held records (a ROI is its four
/// coordinates and its frame list, not a record).
///
/// A file's staged rows are what its EMBED view shows and what the CSV
/// export writes for it, from the moment the import ends and with no file
/// read. They become records ([`AnnotationStore::take_staged`]) the first
/// time something needs them as records and the file's key is settled:
/// a read of the file's view once its key is settled, or an operation that
/// names the file. A save through the EMBED endpoint replaces them, so it
/// drops them instead. A file whose key is never settled keeps its rows
/// staged for the session, and a file that cannot be given a key shows and
/// exports its rows like any other.
///
/// Staged rows are in no snapshot and in no exported document: those hold
/// records. Whoever exports a document settles and takes the staged rows
/// first (`server::annotations::export_native`).
///
/// # Document exports
///
/// A document export copies every record and writes its text, so the
/// store lets one be built at a time ([`AnnotationStore::export_permit`]):
/// a second waits, holding nothing, until the first one's text is
/// written. The permit is not held while a text is sent: a client that
/// reads slowly must not hold up every later export. What exports make the
/// process hold is one copy of the records, the text that is being built,
/// and the texts that are being sent, which are bounded together
/// ([`DOWNLOAD_BUDGET_BYTES`], [`AnnotationStore::hold_download`]).
#[derive(Clone)]
pub struct AnnotationStore {
    backend: Arc<MemoryBackend>,
    import: Arc<Mutex<EmbedImport>>,
    ready: Arc<Notify>,
    exports: Arc<Semaphore>,
    /// Bytes of document text held by downloads that are being sent, and
    /// how many they may hold together.
    downloads: Arc<Mutex<u64>>,
    download_budget: u64,
    /// Run by [`AnnotationStore::before_embed_write`], so a test can make a
    /// second write at exactly the point between a save's read of the view
    /// and its write.
    #[cfg(test)]
    before_embed_write: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl AnnotationStore {
    fn new(backend: MemoryBackend, state: LoadState) -> Self {
        Self {
            backend: Arc::new(backend),
            import: Arc::new(Mutex::new(EmbedImport {
                expected: state == LoadState::Loading,
                state,
                report: None,
                edited: HashSet::new(),
                staged: HashMap::new(),
            })),
            ready: Arc::new(Notify::new()),
            exports: Arc::new(Semaphore::new(1)),
            downloads: Arc::new(Mutex::new(0)),
            download_budget: DOWNLOAD_BUDGET_BYTES,
            #[cfg(test)]
            before_embed_write: None,
        }
    }

    /// A store over `backend` with no import to wait for.
    pub fn with_backend(backend: MemoryBackend) -> Self {
        Self::new(backend, LoadState::Ready)
    }

    /// An empty in-memory store for [`session_author`] without a label
    /// schema, with no import to wait for.
    pub fn empty() -> Self {
        Self::with_backend(Self::session_backend())
    }

    /// The same store while an `--annotations` import is still to come:
    /// it stays loading until [`AnnotationStore::stage_rows`] or
    /// [`AnnotationStore::fail_loading`].
    pub fn loading() -> Self {
        Self::new(Self::session_backend(), LoadState::Loading)
    }

    fn session_backend() -> MemoryBackend {
        MemoryBackend::new(MemoryConfig {
            author: session_author(),
            schema: None,
        })
    }

    /// The store's backend.
    pub fn backend(&self) -> &Arc<MemoryBackend> {
        &self.backend
    }

    /// Returns once the import has finished, or with its message when it
    /// failed. At once for a store that had none.
    pub async fn wait_until_ready(&self) -> Result<()> {
        loop {
            let notified = self.ready.notified();
            let state = self
                .import
                .lock()
                .map_err(|_| anyhow!("annotations store lock poisoned"))?
                .state
                .clone();
            match state {
                LoadState::Ready => return Ok(()),
                LoadState::Failed(message) => return Err(anyhow!(message)),
                LoadState::Loading => notified.await,
            }
        }
    }

    /// Keeps `report` as what the import's adapter answered. Call it
    /// before the call that ends the import
    /// ([`AnnotationStore::stage_rows`], [`AnnotationStore::finish_loading`]),
    /// so that whoever sees the import ended sees its report.
    pub fn record_import_report(&self, report: Report) -> Result<()> {
        self.import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?
            .report = Some(report);
        Ok(())
    }

    /// Where the import stands, without waiting for it.
    pub fn import_status(&self) -> Result<ImportStatus> {
        let import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        Ok(match &import.state {
            _ if !import.expected => ImportStatus::None,
            LoadState::Loading => ImportStatus::Loading,
            LoadState::Ready => ImportStatus::Ready(import.report.clone()),
            LoadState::Failed(message) => ImportStatus::Failed(message.clone()),
        })
    }

    /// Waits for the session's one document export permit ("Document
    /// exports"). Whoever holds it is the only document export running;
    /// dropping it lets the next one start.
    pub async fn export_permit(&self) -> Result<OwnedSemaphorePermit> {
        self.exports
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| anyhow!("annotations store is closed"))
    }

    /// The same store with `bytes` as its download budget in place of
    /// [`DOWNLOAD_BUDGET_BYTES`], for a session or a test that wants
    /// another. Holds taken before the change still count.
    pub fn with_download_budget(mut self, bytes: u64) -> Self {
        self.download_budget = bytes;
        self
    }

    /// Takes `bytes` of the session's download budget for a document text
    /// that is about to be sent, until the hold is dropped. `None` when
    /// other downloads are being sent and this one would make them hold
    /// more than the budget together. A text is always admitted when
    /// no other is being sent, whatever its size, so every session can be
    /// downloaded.
    pub fn hold_download(&self, bytes: u64) -> Option<DownloadHold> {
        let budget = self.download_budget;
        let mut held = self.downloads.lock().ok()?;
        if *held > 0 && held.saturating_add(bytes) > budget {
            return None;
        }
        *held = held.saturating_add(bytes);
        Some(DownloadHold {
            held: self.downloads.clone(),
            bytes,
        })
    }

    /// Ends the import: readers stop waiting. A store that already failed
    /// stays failed.
    pub fn finish_loading(&self) -> Result<()> {
        let mut import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        if import.state == LoadState::Loading {
            import.state = LoadState::Ready;
        }
        drop(import);
        self.ready.notify_waiters();
        Ok(())
    }

    /// Ends the import as failed with `error`, which the EMBED reads and
    /// the export then answer with.
    pub fn fail_loading(&self, error: impl Into<String>) -> Result<()> {
        let mut import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        import.state = LoadState::Failed(error.into());
        drop(import);
        self.ready.notify_waiters();
        Ok(())
    }

    /// Notes that the EMBED endpoint is replacing the ROIs of file `index`,
    /// so that the import leaves the file alone from here on.
    pub(crate) fn mark_edited(&self, index: usize) -> Result<()> {
        self.import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?
            .edited
            .insert(index);
        Ok(())
    }

    /// Ends the import with `rows`, the rows the CSV matched to loaded
    /// files: stages them and lets readers through. A file the EMBED
    /// endpoint has already written to gets none of its rows, and a file
    /// with no ROI is not staged. Returns how many files were staged and
    /// how many were left out as edited.
    ///
    /// Nothing is read, hashed or validated: the cost is that of keeping
    /// the rows. A store that already failed stays failed and stages
    /// nothing.
    pub(crate) fn stage_rows(&self, rows: AnnotationIndexMap) -> Result<(usize, usize)> {
        let mut import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        let (mut staged, mut edited) = (0, 0);
        if import.state == LoadState::Loading {
            for (index, rois) in rows {
                if rois.roi_coords.is_empty() {
                    continue;
                }
                if import.edited.contains(&index) {
                    edited += 1;
                } else {
                    import.staged.insert(index, rois);
                    staged += 1;
                }
            }
            import.state = LoadState::Ready;
        }
        drop(import);
        self.ready.notify_waiters();
        Ok((staged, edited))
    }

    /// The rows staged for file `index`, as the CSV wrote them, or `None`
    /// when the file has none (it never had, or they were taken).
    pub(crate) fn staged_rows(&self, index: usize) -> Result<Option<EmbedRoiAnnotations>> {
        Ok(self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?
            .staged
            .get(&index)
            .cloned())
    }

    /// Takes the rows staged for file `index` and hands them to `with`,
    /// which makes them records (or, for a save that replaces them, drops
    /// them); `None` when the file has none. The rows are gone from the
    /// stage whatever `with` returns.
    ///
    /// `with` runs under the import's lock, so two requests never take the
    /// same rows and a reader never sees a file with its rows neither
    /// staged nor stored. It must not wait for anything and must not call
    /// back into this store's import state.
    pub(crate) fn take_staged<T>(
        &self,
        index: usize,
        with: impl FnOnce(EmbedRoiAnnotations) -> T,
    ) -> Result<Option<T>> {
        let mut import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        Ok(import.staged.remove(&index).map(with))
    }

    /// Runs `with` on every file's staged rows at once, under the import's
    /// lock: what an export reads, so that the rows it writes for one file
    /// and for the next were staged at the same moment. `with` may take
    /// rows out (to make them records) and must not wait for anything or
    /// call back into this store's import state.
    pub(crate) fn with_staged<T>(
        &self,
        with: impl FnOnce(&mut HashMap<usize, EmbedRoiAnnotations>) -> T,
    ) -> Result<T> {
        let mut import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        Ok(with(&mut import.staged))
    }

    /// Called by a save through the EMBED endpoint after it has read the
    /// file's view and before it writes. It does nothing; a test hangs a
    /// competing write on it ([`AnnotationStore::with_before_embed_write`]).
    pub(crate) fn before_embed_write(&self) {
        #[cfg(test)]
        if let Some(hook) = &self.before_embed_write {
            hook();
        }
    }

    /// Runs `hook` each time a save through the EMBED endpoint has read
    /// the view it will replace and is about to write.
    #[cfg(test)]
    pub(crate) fn with_before_embed_write(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.before_embed_write = Some(hook);
        self
    }
}

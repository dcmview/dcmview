//! What a session holds of annotations: the backend, and the state of the
//! `--annotations` import.

use super::memory::{MemoryBackend, MemoryConfig};
use super::{AnnotationIndexMap, EmbedRoiAnnotations};
use anyhow::{anyhow, Result};
use dcmview_annotation::Author;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

/// Who the rows of an EMBED CSV are authored by
/// (`docs/design/annotation-model.md` 9.2).
pub const EMBED_IMPORT_AUTHOR: &str = "import:embed";

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum LoadState {
    Loading,
    Ready,
    Failed(String),
}

#[derive(Debug)]
struct EmbedImport {
    state: LoadState,
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
/// first.
#[derive(Clone)]
pub struct AnnotationStore {
    backend: Arc<MemoryBackend>,
    import: Arc<Mutex<EmbedImport>>,
    ready: Arc<Notify>,
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
                state,
                edited: HashSet::new(),
                staged: HashMap::new(),
            })),
            ready: Arc::new(Notify::new()),
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

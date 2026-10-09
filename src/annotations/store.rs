//! What a session holds of annotations: the backend, and the state of the
//! `--annotations` import.

#![expect(dead_code, reason = "nothing calls into the store yet")]

use super::memory::{MemoryBackend, MemoryConfig};
use anyhow::{anyhow, Result};
use dcmview_annotation::Author;
use std::collections::HashSet;
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
}

/// A session's annotations: its backend, and where the `--annotations`
/// import stands. Cheap to clone; clones share everything.
///
/// The import runs in the background after discovery. Until it has
/// finished, a read through the EMBED endpoints waits
/// ([`AnnotationStore::wait_until_ready`]) while writes go ahead; a file
/// the EMBED endpoint wrote to meanwhile keeps what was written and the
/// import's rows for it are dropped. An import that fails leaves the store
/// failed for the session: the EMBED reads and the export answer with its
/// message, and viewing goes on.
#[derive(Clone)]
pub struct AnnotationStore {
    backend: Arc<MemoryBackend>,
    import: Arc<Mutex<EmbedImport>>,
    ready: Arc<Notify>,
}

impl AnnotationStore {
    fn new(backend: MemoryBackend, state: LoadState) -> Self {
        Self {
            backend: Arc::new(backend),
            import: Arc::new(Mutex::new(EmbedImport {
                state,
                edited: HashSet::new(),
            })),
            ready: Arc::new(Notify::new()),
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
    /// it stays loading until [`AnnotationStore::finish_loading`] or
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

    /// Runs `load`, which writes the import's rows of file `index` to the
    /// backend, unless the EMBED endpoint has replaced that file's ROIs;
    /// `None` when it has. The check and the write are one step as far as
    /// [`AnnotationStore::mark_edited`] is concerned: an edit is either
    /// seen here, or made after the rows are in and so replaces them.
    ///
    /// `load` runs under the import's lock. It must not wait for anything
    /// and must not call back into this store's import state.
    pub(crate) fn load_unless_edited<T>(
        &self,
        index: usize,
        load: impl FnOnce() -> T,
    ) -> Result<Option<T>> {
        let import = self
            .import
            .lock()
            .map_err(|_| anyhow!("annotations store lock poisoned"))?;
        if import.edited.contains(&index) {
            return Ok(None);
        }
        Ok(Some(load()))
    }
}

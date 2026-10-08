//! The catalog's side of file keys: what a session shows for a key, the
//! background hashing behind `b3:` keys, and the entry points other parts of
//! the server use (`docs/design/annotation-model.md` 1.7, 1.8).
//!
//! The rules for which key a file has are `crate::keys::KeyTable`'s. This
//! module owns everything around them that needs the registry: its lock,
//! its entries, a runtime and the files themselves.

use super::{FileRegistry, RegistryStatus};
use crate::api::contracts::{FileKeyError, FileRekey, FileSummary};
use crate::keys::{
    FileHasher, FileKey, FileKeyStatus, HashProgress, KeyFailure, KeyScheme, KeyView,
};
use crate::loader::DiscoveryRecord;
use crate::masking::Masker;
use crate::pixels::{self, DecodeClass, DecodeScheduler};
use std::collections::{HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use tokio::sync::Notify;

/// One answer to `GET /api/files`, before the fields the handler adds that
/// do not come from the registry. The first six members are those of
/// `FilesResponse` with the same names, and their contract is written
/// there. Every member was read at the same moment
/// ([`FileRegistry::files_page`]).
#[derive(Debug, Clone)]
pub struct FilesPage {
    pub files: Vec<FileSummary>,
    pub revision: u64,
    pub reset: bool,
    pub more: bool,
    pub keys_hashing: usize,
    pub rekeys: Vec<FileRekey>,
    /// The scan state and counters the entries belong to.
    pub status: RegistryStatus,
    /// The most recent skipped and filtered records, sorted by path.
    pub discovery: Vec<DiscoveryRecord>,
}

/// How much whole-file hashing a registry has done since it was created.
/// Tests bound the work with these counts instead of with time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyStats {
    /// Files whose hashing ran to an outcome, a digest or a failure.
    pub files_hashed: u64,
    /// Bytes read from files for hashing, as `FileHasher::bytes_read` counts
    /// them.
    pub bytes_hashed: u64,
    /// Calls to `FileHasher::next_slice`, each made under its own background
    /// decode permit.
    pub slices: u64,
}

/// Why [`FileRegistry::ensure_key`] has no key to give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("no file has index {0}")]
    NotFound(usize),
    /// A digest the key needs could not be computed.
    #[error("the file could not be hashed for its key")]
    Unavailable(KeyFailure),
    /// [`FileRegistry::stop_key_work`] was called before the key was known.
    #[error("key hashing was stopped")]
    Stopped,
}

impl From<KeyFailure> for FileKeyError {
    fn from(failure: KeyFailure) -> Self {
        match failure {
            KeyFailure::Unreadable => Self::Unreadable,
            KeyFailure::Changed => Self::Changed,
        }
    }
}

/// A key as this session sends it. A masked session never sends a real
/// instance UID: a `sop:` key is rebuilt from the masked UID, which is what
/// the entry's `sop_instance_uid` shows, so the two still agree. A `b3:` key
/// is a digest of the file's bytes and carries no header value; it is sent
/// as it is, like the file's path.
pub(super) fn shown_key(key: &FileKey, masker: Option<&Masker>) -> String {
    match (masker, key.scheme()) {
        (Some(masker), KeyScheme::Sop) => format!("sop:{}", masker.uid(key.body())),
        _ => key.as_str().to_string(),
    }
}

/// Writes a file's key state into its catalog entry: `file_key` left out
/// when it is `sop:` plus the entry's own (shown) `sop_instance_uid`, `null`
/// while the file has no key, the shown key otherwise; `alias_of`; and
/// `key_error`.
///
/// `state` is the file's real key state (`KeyTable::view`). For an ordinary
/// DICOM file outside a masked session, which is nearly every file of
/// nearly every scan, this compares two strings and allocates nothing.
pub(super) fn show_key_state(
    summary: &mut FileSummary,
    state: KeyView<'_>,
    masker: Option<&Masker>,
) {
    let implied = state.key.is_some_and(|key| {
        key.scheme() == KeyScheme::Sop
            && match masker {
                // The entry shows the masked UID, and the key is sent built
                // from that same masked UID.
                Some(masker) => masker.uid(key.body()) == summary.sop_instance_uid,
                None => key.body() == summary.sop_instance_uid,
            }
    });
    summary.file_key = if implied {
        None
    } else {
        Some(state.key.map(|key| shown_key(key, masker)))
    };
    summary.alias_of = state.alias_of;
    summary.key_error = state.failure.map(FileKeyError::from);
}

impl FileRegistry {
    /// Hashes under `scheduler`'s background permits instead of the
    /// process-wide `pixels::decode_scheduler()`. For tests, which need a
    /// pool they alone hold permits of. Call it before the registry is
    /// cloned or a file is registered.
    pub fn with_decode_scheduler(self, scheduler: Arc<DecodeScheduler>) -> Self {
        self.hashing.work().scheduler = Some(scheduler);
        self
    }

    /// The file's key state, or `None` for an index that is not registered.
    /// The key here is the real one, whatever the session shows.
    pub fn key_status(&self, index: usize) -> Option<FileKeyStatus> {
        self.read().keys.status(index)
    }

    /// The file's key as this session sends it (see `shown_key`), or `None`
    /// while it has none. This is the value of `X-File-Key` and, written in
    /// full, of the entry's `file_key`.
    pub fn shown_file_key(&self, index: usize) -> Option<String> {
        self.read()
            .keys
            .view(index)?
            .key
            .map(|key| shown_key(key, self.masker.as_deref()))
    }

    /// The file a key received from a client names: `key` is a key as this
    /// session sends it, current or replaced. In a masked session a `sop:`
    /// key is looked up by its masked UID; a real UID a client could not
    /// have been sent names nothing. `None` for text that is not a file key
    /// and for a key no file has held.
    pub fn file_for_shown_key(&self, key: &str) -> Option<usize> {
        let key = FileKey::parse(key).ok()?;
        let inner = self.read();
        let real = if self.masker.is_some() && key.scheme() == KeyScheme::Sop {
            inner.shown_keys.get(key.as_str())?
        } else {
            &key
        };
        inner.keys.file_for_key(real)
    }

    /// Notes that a frame of the file was served, which is when a file
    /// without a key starts being hashed (`KeyTable::frame_sent`).
    ///
    /// Called on every display and raw frame response, so it is cheap: it
    /// takes the registry's write lock only the first time for each file
    /// (`KeyTable::served` under the read lock otherwise), never blocks on
    /// hashing and never fails. An index that is not registered is ignored.
    pub fn frame_sent(&self, index: usize) {
        let served = self.read().keys.served(index);
        let wanted = if served {
            Vec::new()
        } else {
            self.write().keys.frame_sent(index).wanted
        };
        self.queue_keys(wanted, false);
    }

    /// The file's key once it can be relied on for a write or an export,
    /// hashing what `KeyTable::required_for` names first: nothing for an
    /// ordinary DICOM file, the file for one without a key, the file and the
    /// file it is an alias of for an unchecked alias.
    ///
    /// Repeats until `required_for` is empty, since comparing two aliases
    /// can split their group. The files it needs go to the front of the
    /// hashing queue, ahead of digests wanted in the background, and each is
    /// still hashed in slices under background decode permits. A file whose
    /// last attempt failed is tried again, once per call.
    ///
    /// Errors: [`KeyError::NotFound`]; [`KeyError::Unavailable`] when a
    /// digest it needs fails, with that failure; [`KeyError::Stopped`].
    /// Cancel safe: dropping the future leaves the queued work to finish in
    /// the background.
    pub async fn ensure_key(&self, index: usize) -> Result<FileKey, KeyError> {
        let mut asked = HashSet::new();
        loop {
            let changed = self.hashing.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.hashing.stopped.load(Ordering::Acquire) {
                return Err(KeyError::Stopped);
            }
            // Sample queue membership before the table. Outcomes update the
            // table before leaving the pending set, so an absent request is
            // complete when its failure is inspected below.
            let pending = {
                let work = self.hashing.work();
                asked
                    .iter()
                    .copied()
                    .filter(|index| work.pending.contains(index))
                    .collect::<HashSet<_>>()
            };
            let needed = {
                let inner = self.read();
                let state = inner.keys.view(index).ok_or(KeyError::NotFound(index))?;
                let required = inner.keys.required_for(index);
                if required.is_empty() {
                    return state.key.cloned().ok_or(KeyError::Unavailable(
                        state.failure.unwrap_or(KeyFailure::Unreadable),
                    ));
                }
                let mut needed = Vec::new();
                for file in required {
                    if asked.contains(&file) {
                        if !pending.contains(&file) {
                            if let Some(failure) =
                                inner.keys.view(file).and_then(|state| state.failure)
                            {
                                return Err(KeyError::Unavailable(failure));
                            }
                        }
                    } else {
                        asked.insert(file);
                        needed.push(file);
                    }
                }
                needed
            };
            self.queue_keys(needed, true);
            changed.await;
        }
    }

    /// The hashing done so far.
    pub fn key_stats(&self) -> KeyStats {
        self.hashing.work().stats
    }

    /// Stops hashing: no further slice starts, queued digests are dropped
    /// without an outcome (no failure is recorded for them), and waiting
    /// [`FileRegistry::ensure_key`] calls return [`KeyError::Stopped`]. The
    /// slice being read, if any, finishes first; it is at most
    /// `KEY_HASH_SLICE_BYTES` long. Idempotent, and final for this registry.
    pub fn stop_key_work(&self) {
        {
            let mut work = self.hashing.work();
            self.hashing.stopped.store(true, Ordering::Release);
            work.queue.clear();
            let active = work.active;
            work.pending.retain(|index| Some(*index) == active);
        }
        self.hashing.stop.notify_waiters();
        self.hashing.changed.notify_waiters();
    }
}

#[derive(Default)]
pub(super) struct Hashing {
    work: Mutex<HashWork>,
    stopped: AtomicBool,
    pub(super) changed: Notify,
    stop: Notify,
}

#[derive(Default)]
struct HashWork {
    queue: VecDeque<usize>,
    pending: HashSet<usize>,
    active: Option<usize>,
    running: bool,
    scheduler: Option<Arc<DecodeScheduler>>,
    stats: KeyStats,
}

impl Hashing {
    fn work(&self) -> MutexGuard<'_, HashWork> {
        self.work.lock().expect("key hashing lock poisoned")
    }
}

impl FileRegistry {
    pub(super) fn hashing_count(&self) -> usize {
        self.hashing.work().pending.len()
    }

    pub(super) fn queue_keys(&self, indexes: Vec<usize>, front: bool) {
        let start = {
            let mut work = self.hashing.work();
            if self.hashing.stopped.load(Ordering::Acquire) {
                return;
            }
            if front {
                // Move an already queued request too: explicit callers
                // take precedence over files queued by viewing.
                for index in indexes.into_iter().rev() {
                    if work.active == Some(index) {
                        continue;
                    }
                    if !work.pending.insert(index) {
                        work.queue.retain(|queued| *queued != index);
                    }
                    work.queue.push_front(index);
                }
            } else {
                for index in indexes {
                    if work.pending.insert(index) {
                        work.queue.push_back(index);
                    }
                }
            }
            if !work.running && !work.queue.is_empty() {
                tokio::runtime::Handle::try_current()
                    .ok()
                    .inspect(|_| work.running = true)
            } else {
                None
            }
        };
        if let Some(runtime) = start {
            let registry = self.clone();
            runtime.spawn(async move { registry.hash_worker().await });
        }
    }

    async fn hash_worker(self) {
        loop {
            let next = {
                let mut work = self.hashing.work();
                match work.queue.pop_front() {
                    Some(index) => {
                        work.active = Some(index);
                        Some((
                            index,
                            work.scheduler
                                .clone()
                                .unwrap_or_else(|| pixels::decode_scheduler().clone()),
                        ))
                    }
                    None => {
                        work.running = false;
                        None
                    }
                }
            };
            let Some((index, scheduler)) = next else {
                return;
            };
            let known = self
                .read()
                .keys
                .status(index)
                .is_some_and(|state| state.digest.is_some());
            let outcome = if known {
                None
            } else {
                self.hash_file(index, scheduler).await
            };
            let wanted = if let Some(outcome) = outcome {
                if self.hashing.stopped.load(Ordering::Acquire) {
                    Vec::new()
                } else {
                    let mut inner = self.write();
                    let changes = inner.keys.resolve(index, outcome);
                    self.apply_key_changes(&mut inner, &changes);
                    drop(inner);
                    self.hashing.work().stats.files_hashed += 1;
                    changes.wanted
                }
            } else {
                Vec::new()
            };
            {
                let mut work = self.hashing.work();
                work.active = None;
                work.pending.remove(&index);
            }
            self.queue_keys(wanted, false);
            self.hashing.changed.notify_waiters();
            self.notify.notify_waiters();
        }
    }

    async fn hash_file(
        &self,
        index: usize,
        scheduler: Arc<DecodeScheduler>,
    ) -> Option<Result<[u8; 32], KeyFailure>> {
        if self.hashing.stopped.load(Ordering::Acquire) {
            return None;
        }
        let file = self.get(index)?;
        let opened =
            tokio::task::spawn_blocking(move || FileHasher::open(&file.path, file.size_bytes))
                .await;
        let mut hasher = match opened {
            Ok(Ok(hasher)) => hasher,
            Ok(Err(failure)) => return Some(Err(failure)),
            Err(_) => return Some(Err(KeyFailure::Unreadable)),
        };
        loop {
            let stopped = self.hashing.stop.notified();
            tokio::pin!(stopped);
            stopped.as_mut().enable();
            if self.hashing.stopped.load(Ordering::Acquire) {
                return None;
            }
            let permit = tokio::select! {
                _ = stopped => return None,
                permit = scheduler.acquire(DecodeClass::Background) => permit,
            };
            if self.hashing.stopped.load(Ordering::Acquire) {
                return None;
            }
            let hashing = self.hashing.clone();
            let sliced = tokio::task::spawn_blocking(move || {
                {
                    let mut work = hashing.work();
                    if hashing.stopped.load(Ordering::Acquire) {
                        return None;
                    }
                    work.stats.slices += 1;
                }
                let before = hasher.bytes_read();
                let outcome = hasher.next_slice();
                let bytes = hasher.bytes_read() - before;
                drop(permit);
                Some((hasher, outcome, bytes))
            })
            .await;
            match sliced {
                Ok(None) => return None,
                Ok(Some((next, outcome, bytes))) => {
                    self.hashing.work().stats.bytes_hashed += bytes;
                    hasher = next;
                    match outcome {
                        Ok(HashProgress::More) => {}
                        Ok(HashProgress::Done(digest)) => return Some(Ok(digest)),
                        Err(failure) => return Some(Err(failure)),
                    }
                }
                Err(_) => return Some(Err(KeyFailure::Unreadable)),
            }
        }
    }
}

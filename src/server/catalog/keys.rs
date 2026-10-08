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
    FileHasher, FileKey, FileKeyStatus, HashProgress, KeyFailure, KeyRef, KeyScheme, KeyView,
    KeyedFile, Reliance,
};
use crate::loader::DiscoveryRecord;
use crate::masking::Masker;
use crate::pixels::{self, DecodeClass, DecodeScheduler};
use crate::types::FileEntry;
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard, PoisonError,
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

/// What the key table holds of a registered file: the entry itself, shared
/// with the registry's file list, so the table stores no UID and no path of
/// its own (`KeyTable`, "Cost").
#[derive(Clone)]
pub(super) struct Keyed {
    file: Arc<FileEntry>,
    /// The path discovery resolved, kept only when it is not the entry's
    /// own path: a file reached through a symbolic link, or under a
    /// directory that is one.
    resolved: Option<Arc<PathBuf>>,
}

impl Keyed {
    pub(super) fn new(file: Arc<FileEntry>, resolved: PathBuf) -> Self {
        let resolved = (resolved != file.path).then(|| Arc::new(resolved));
        Self { file, resolved }
    }
}

impl KeyedFile for Keyed {
    fn sop_instance_uid(&self) -> &str {
        &self.file.sop_instance_uid
    }

    fn size_bytes(&self) -> u64 {
        self.file.size_bytes
    }

    fn path(&self) -> &Path {
        match &self.resolved {
            Some(resolved) => resolved,
            None => &self.file.path,
        }
    }
}

/// In a masked session, the first file that carries each masked SOP
/// Instance UID, found by that masked UID: the way from a `sop:` key a
/// client was sent back to a file.
///
/// It holds no string. A masked UID is already in the catalog entry of the
/// file that carries it (`FileSummary::sop_instance_uid`), so a file is
/// found by the hash of the text and confirmed against its entry. Two
/// masked UIDs with one hash are both kept, the later one in `collided`,
/// so a lookup is exact whatever the hashes do.
#[derive(Default)]
pub(super) struct ShownUids {
    first: HashMap<u64, usize>,
    collided: Vec<usize>,
    hasher: RandomState,
}

impl ShownUids {
    /// Notes file `index`, whose entry `summaries[index]` shows its masked
    /// UID. A UID an earlier file shows keeps naming that file.
    pub(super) fn insert(&mut self, index: usize, summaries: &[FileSummary]) {
        let shown = summaries[index].sop_instance_uid.as_str();
        let earlier = *self
            .first
            .entry(self.hasher.hash_one(shown))
            .or_insert(index);
        if summaries[earlier].sop_instance_uid != shown
            && !self
                .collided
                .iter()
                .any(|&other| summaries[other].sop_instance_uid == shown)
        {
            self.collided.push(index);
        }
    }

    /// The first file whose entry shows the masked UID `shown`.
    fn get(&self, shown: &str, summaries: &[FileSummary]) -> Option<usize> {
        let shows = |index: &usize| summaries[*index].sop_instance_uid == shown;
        self.first
            .get(&self.hasher.hash_one(shown))
            .copied()
            .filter(shows)
            .or_else(|| self.collided.iter().copied().find(shows))
    }
}

/// A key as this session sends it. A masked session never sends a real
/// instance UID: a `sop:` key is built from the masked UID, which is what
/// the entry's `sop_instance_uid` shows, so the two still agree. A `b3:` key
/// is a digest of the file's bytes and carries no header value; it is sent
/// as it is, like the file's path.
pub(super) fn shown_key(key: KeyRef<'_>, masker: Option<&Masker>) -> String {
    match (key, masker) {
        (KeyRef::Sop(uid), Some(masker)) => format!("sop:{}", masker.uid(uid)),
        (KeyRef::Sop(uid), None) => format!("sop:{uid}"),
        (KeyRef::Content(key), _) => key.as_str().to_string(),
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
    let implied = match (state.key, masker) {
        // A masked entry shows the masked form of its file's UID
        // (`Masker::summary`), and the key is sent built from that same
        // masked UID (`shown_key`): the two agree without masking the UID
        // again to compare them.
        (Some(KeyRef::Sop(_)), Some(_)) => true,
        (Some(KeyRef::Sop(uid)), None) => uid == summary.sop_instance_uid,
        _ => false,
    };
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
        if self.masker.is_some() && key.scheme() == KeyScheme::Sop {
            let carrier = inner
                .files
                .get(inner.shown_uids.get(key.body(), &inner.summaries)?)?;
            let real = FileKey::sop(&carrier.sop_instance_uid).ok()?;
            inner.keys.file_for_key(&real)
        } else {
            inner.keys.file_for_key(&key)
        }
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

    /// The file's key once it is settled, which is what a record or an
    /// export may hold: the key this returns is the file's key for the rest
    /// of the session. No file registered or hashed afterwards replaces it,
    /// two files it returns the same key for hold the same bytes (or are one
    /// file), and `file_for_shown_key` of it keeps naming the same file,
    /// which is this one or one with the same bytes.
    ///
    /// `docs/design/annotation-model.md` 1.7 has a key shared by UID and
    /// size verified "before the first annotation write on either file".
    /// This is that verification. It hashes what `KeyTable::required_for`
    /// names and then asks `KeyTable::rely_on`:
    ///
    /// - nothing for a DICOM file whose UID no other loaded file has: the
    ///   key is returned at once and no byte is read;
    /// - the file for one without a key;
    /// - every file that has the UID for a file whose UID another file of
    ///   the same size also has, whichever of them is asked about. The cost
    ///   is the bytes of those files, each read once, however many of their
    ///   keys are asked for afterwards;
    /// - the file and the first file that has the UID for a file found
    ///   after a key of its UID was returned.
    ///
    /// The files it needs go to the front of the hashing queue, ahead of
    /// digests wanted in the background, in ascending order, and each is
    /// still hashed in slices under background decode permits. A file
    /// without a digest is tried once per call, also when its last attempt
    /// failed. Callers that wait on the same file share one attempt. When
    /// the attempts it asked for have an outcome it asks again what is
    /// required, since a file may have been registered meanwhile, and
    /// returns when nothing new is.
    ///
    /// A call costs one visit to each file of the group per round of
    /// attempts, not one per digest that arrives, and queueing `n` files
    /// costs `O(n)` plus one pass over the queue.
    ///
    /// Errors: [`KeyError::NotFound`]; [`KeyError::Unavailable`] when the
    /// file's own digest or its group's first file's digest could not be
    /// computed, with that failure (another file of the group that cannot
    /// be read does not fail the call: that file loses the shared key
    /// instead, `KeyTable` rule 4); [`KeyError::Stopped`]. Cancel safe:
    /// dropping the future leaves the queued work to finish in the
    /// background.
    pub async fn ensure_key(&self, index: usize) -> Result<FileKey, KeyError> {
        let mut asked = HashSet::new();
        let mut waiting = VecDeque::new();
        loop {
            let changed = self.hashing.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.hashing.stopped.load(Ordering::Acquire) {
                return Err(KeyError::Stopped);
            }
            {
                let inner = self.read();
                inner.keys.view(index).ok_or(KeyError::NotFound(index))?;
                if inner.keys.settled(index) {
                    return inner
                        .keys
                        .status(index)
                        .and_then(|state| state.key)
                        .ok_or(KeyError::Unavailable(KeyFailure::Unreadable));
                }
            }
            // Each requested file leaves this list once. A digest wake-up
            // must not rescan a group whose other files are still queued.
            {
                let work = self.hashing.work();
                while waiting
                    .front()
                    .is_some_and(|file| !work.pending.contains(file))
                {
                    waiting.pop_front();
                }
            }
            if waiting.is_empty() {
                let mut inner = self.write();
                let needed: Vec<_> = inner
                    .keys
                    .required_for(index)
                    .into_iter()
                    .filter(|file| asked.insert(*file))
                    .collect();
                if needed.is_empty() {
                    let (reliance, changes) = inner.keys.rely_on(index);
                    self.apply_key_changes(&mut inner, &changes);
                    drop(inner);
                    if !changes.updated.is_empty() {
                        self.notify.notify_waiters();
                    }
                    match reliance {
                        Some(Reliance::Final(key)) => return Ok(key),
                        Some(Reliance::Failed(failure)) => {
                            return Err(KeyError::Unavailable(failure))
                        }
                        Some(Reliance::Wanted) => {}
                        None => return Err(KeyError::NotFound(index)),
                    }
                } else {
                    drop(inner);
                    waiting.extend(needed.iter().copied());
                    self.queue_keys(needed, true);
                }
            }
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
    /// A file whose turn in the worker panics, once. The seam a test uses
    /// to kill the worker where a bug would.
    #[cfg(test)]
    fault: Mutex<Option<usize>>,
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

/// Gives the worker's claim on the queue back when its task is dropped
/// before it ran out of work: its runtime shut down, or it was dropped
/// before its first poll. The file it held goes back to the front of the
/// queue, with no failure recorded: nothing went wrong with that file. The
/// next request that queues from a live runtime starts a worker again.
///
/// This never spawns and never looks at whether the thread is panicking. A
/// runtime is dropped on a panicking thread whenever a test fails, and a
/// task spawned on a runtime that is shutting down is dropped at once, so a
/// guard that restarted the worker there would run inside its own drop
/// without end. A panic of the worker itself never reaches this guard
/// (`FileRegistry::hash_worker` catches it and carries on).
struct HashWorkerGuard {
    hashing: Arc<Hashing>,
    notify: Arc<Notify>,
    armed: bool,
}

impl Drop for HashWorkerGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        {
            let mut work = self.hashing.work();
            work.running = false;
            if let Some(index) = work.active.take() {
                if self.hashing.stopped.load(Ordering::Acquire) {
                    work.pending.remove(&index);
                } else {
                    work.queue.push_front(index);
                }
            }
        }
        self.hashing.changed.notify_waiters();
        self.notify.notify_waiters();
    }
}

impl Hashing {
    fn work(&self) -> MutexGuard<'_, HashWork> {
        self.work.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes the worker panic when it takes `index` from the queue, once.
    #[cfg(test)]
    fn fail_at(&self, index: usize) {
        *self.fault.lock().expect("fault lock") = Some(index);
    }

    #[cfg(test)]
    fn fault(&self, index: usize) {
        let armed = {
            let mut fault = self.fault.lock().expect("fault lock");
            (*fault == Some(index)).then(|| fault.take()).is_some()
        };
        if armed {
            panic!("injected fault: the hashing worker dies holding file {index}");
        }
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
                // Promote the whole batch with one pass over the queue.
                let promoted: HashSet<_> = indexes.iter().copied().collect();
                work.queue.retain(|queued| !promoted.contains(queued));
                for index in indexes.into_iter().rev() {
                    if work.active != Some(index) {
                        work.pending.insert(index);
                        work.queue.push_front(index);
                    }
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
            let guard = HashWorkerGuard {
                hashing: self.hashing.clone(),
                notify: self.notify.clone(),
                armed: true,
            };
            runtime.spawn(async move { registry.hash_worker(guard).await });
        }
    }

    /// Hashes what is queued, one file at a time, until the queue is empty.
    ///
    /// A panic while a file is being worked on is caught here, so the worker
    /// survives its own fault: that file is recorded as unreadable, as the
    /// one thing known about it is that it could not be hashed, and the
    /// files queued behind it are hashed without anyone asking again.
    async fn hash_worker(self, mut guard: HashWorkerGuard) {
        use futures::FutureExt;
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
                        guard.armed = false;
                        work.running = false;
                        None
                    }
                }
            };
            let Some((index, scheduler)) = next else {
                return;
            };
            let turn = std::panic::AssertUnwindSafe(self.hash_turn(index, scheduler));
            if turn.catch_unwind().await.is_err() {
                self.record_worker_fault(index);
            }
            {
                let mut work = self.hashing.work();
                work.active = None;
                work.pending.remove(&index);
            }
            self.hashing.changed.notify_waiters();
            self.notify.notify_waiters();
        }
    }

    /// One file's turn in the worker: hashes it unless its digest is known
    /// and records the outcome.
    async fn hash_turn(&self, index: usize, scheduler: Arc<DecodeScheduler>) {
        #[cfg(test)]
        self.hashing.fault(index);
        let known = self
            .read()
            .keys
            .status(index)
            .is_some_and(|state| state.digest.is_some());
        if known {
            return;
        }
        let Some(outcome) = self.hash_file(index, scheduler).await else {
            return;
        };
        if self.hashing.stopped.load(Ordering::Acquire) {
            return;
        }
        let mut inner = self.write();
        let changes = inner.keys.resolve(index, outcome);
        self.apply_key_changes(&mut inner, &changes);
        drop(inner);
        self.hashing.work().stats.files_hashed += 1;
        self.queue_keys(changes.wanted, false);
    }

    /// Records that the worker panicked while it held `index`. A registry
    /// whose lock the panic poisoned is left alone.
    fn record_worker_fault(&self, index: usize) {
        if let Ok(mut inner) = self.inner.write() {
            let changes = inner.keys.resolve(index, Err(KeyFailure::Unreadable));
            self.apply_key_changes(&mut inner, &changes);
            drop(inner);
            self.queue_keys(changes.wanted, false);
        }
        self.hashing.work().stats.files_hashed += 1;
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
        let opened = tokio::task::spawn_blocking(move || {
            FileHasher::open(&file.path, file.size_bytes, file.modified)
        })
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

#[cfg(test)]
mod tests {
    use super::KeyError;
    use crate::keys::{FileKey, KeyFailure};
    use crate::server::FileRegistry;
    use std::path::Path;
    use std::time::Duration;

    /// A fixture registered as a file without a UID, so that its key needs
    /// its bytes.
    fn pending(name: &str) -> (crate::types::FileEntry, String) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let mut file = crate::loader::test_entry(&path);
        file.sop_instance_uid = String::new();
        let bytes = std::fs::read(&path).expect("read fixture");
        let key = FileKey::blake3(blake3::hash(&bytes).as_bytes());
        (file, key.as_str().to_string())
    }

    /// A worker that panics must not take hashing down with it. Without
    /// that, the queue stays marked as running: nothing is hashed again
    /// and every `ensure_key` waits until shutdown, with no sign of it.
    ///
    /// The bound on each wait is not a measurement. A key that is never
    /// decided would hang the test, and the bound turns that into a
    /// failure.
    #[tokio::test]
    async fn a_worker_that_dies_fails_the_file_it_held_and_hashing_goes_on() {
        let (first, first_key) = pending("golden-uncompressed-u16-multiframe.dcm");
        let (second, second_key) = pending("golden-image-no-pixels.dcm");
        let registry = FileRegistry::new();
        registry.insert(first);
        registry.insert(second);
        registry.hashing.fail_at(0);
        // Both files are viewed, so both are queued when the worker dies
        // on the first.
        registry.frame_sent(0);
        registry.frame_sent(1);
        let decided = |index: usize| {
            let registry = registry.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(30), registry.ensure_key(index))
                    .await
                    .unwrap_or_else(|_| panic!("the key of file {index} is never decided"))
            }
        };

        assert_eq!(
            decided(0).await,
            Err(KeyError::Unavailable(KeyFailure::Unreadable)),
            "the file the worker held fails like one that cannot be read"
        );
        let status = registry.key_status(0).expect("registered");
        assert_eq!(
            (status.key, status.failure),
            (None, Some(KeyFailure::Unreadable))
        );
        // The file queued behind it is hashed.
        assert_eq!(
            decided(1).await.as_ref().map(FileKey::as_str),
            Ok(second_key.as_str())
        );
        // The fault was the worker's, not the file's: asked again, it has
        // its key.
        assert_eq!(
            decided(0).await.as_ref().map(FileKey::as_str),
            Ok(first_key.as_str())
        );
        assert_eq!(registry.files_page(None, None).keys_hashing, 0);
        assert_eq!(registry.key_stats().files_hashed, 3);
    }

    /// The files queued behind the one a worker died on are hashed without
    /// anyone asking again: a viewer that opened them makes no further
    /// request for their keys. This test only waits; a key request would
    /// start a worker itself and hide one that was never restarted.
    #[tokio::test]
    async fn files_queued_behind_a_worker_fault_are_hashed_without_another_request() {
        let (first, _) = pending("golden-uncompressed-u16-multiframe.dcm");
        let (second, second_key) = pending("golden-image-no-pixels.dcm");
        let registry = FileRegistry::new();
        registry.insert(first);
        registry.insert(second);
        registry.hashing.fail_at(0);
        registry.frame_sent(0);
        registry.frame_sent(1);

        let hashed = async {
            loop {
                let changed = registry.changed();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if registry.files_page(None, None).keys_hashing == 0 {
                    return;
                }
                changed.await;
            }
        };
        // A hang guard, not a measurement.
        tokio::time::timeout(Duration::from_secs(30), hashed)
            .await
            .expect("the file queued behind the fault is never hashed");

        let key = |index: usize| {
            let status = registry.key_status(index).expect("registered");
            (
                status.key.map(|key| key.as_str().to_string()),
                status.failure,
            )
        };
        assert_eq!(key(0), (None, Some(KeyFailure::Unreadable)));
        assert_eq!(key(1), (Some(second_key), None));
    }

    /// A test that fails drops its runtime while its thread unwinds, and
    /// the worker with it. That must end nothing but the test: the process
    /// goes on (a restart from the worker's drop would run inside itself
    /// until the stack ran out, taking every other test's result with it),
    /// no failure is recorded for files nothing was wrong with, and they
    /// are hashed once a runtime is there again.
    #[test]
    fn a_runtime_dropped_by_a_panic_fails_no_file_and_ends_no_process() {
        let (first, first_key) = pending("golden-uncompressed-u16-multiframe.dcm");
        let (second, second_key) = pending("golden-image-no-pixels.dcm");
        let registry = FileRegistry::new();
        registry.insert(first);
        registry.insert(second);

        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            runtime.block_on(async {
                registry.frame_sent(0);
                registry.frame_sent(1);
                panic!("a failing assertion of the test itself");
            });
        }));
        assert!(failed.is_err());
        for index in 0..2 {
            let status = registry.key_status(index).expect("registered");
            assert_eq!((status.key, status.failure), (None, None), "file {index}");
        }
        assert_eq!(registry.files_page(None, None).keys_hashing, 2);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            for (index, key) in [(0, first_key), (1, second_key)] {
                let decided =
                    tokio::time::timeout(Duration::from_secs(30), registry.ensure_key(index))
                        .await
                        .unwrap_or_else(|_| panic!("the key of file {index} is never decided"));
                assert_eq!(decided.as_ref().map(FileKey::as_str), Ok(key.as_str()));
            }
        });
        assert_eq!(registry.key_stats().files_hashed, 2);
    }
}

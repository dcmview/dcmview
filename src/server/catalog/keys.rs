//! The catalog's side of file keys: what a session shows for a key, the
//! background hashing behind `b3:` keys, and the entry points other parts of
//! the server use (`docs/design/annotation-model.md` 1.7, 1.8).
//!
//! The rules for which key a file has are `crate::keys::KeyTable`'s. This
//! module owns everything around them that needs the registry: its lock,
//! its entries, a runtime and the files themselves.

use super::FileRegistry;
use crate::api::contracts::{FileKeyError, FileRekey, FileSummary};
use crate::keys::{FileKey, FileKeyStatus, KeyFailure, KeyScheme, KeyView};
use crate::masking::Masker;
use crate::pixels::DecodeScheduler;
use std::sync::Arc;

/// One answer to `GET /api/files`, before the fields the handler adds. The
/// members are those of `FilesResponse` with the same names, and their
/// contract is written there.
#[derive(Debug, Clone)]
pub struct FilesPage {
    pub files: Vec<FileSummary>,
    pub revision: u64,
    pub reset: bool,
    pub more: bool,
    pub keys_hashing: usize,
    pub rekeys: Vec<FileRekey>,
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
        let _ = scheduler;
        todo!("FND4: keep the scheduler background hashing takes its permits from")
    }

    /// The file's key state, or `None` for an index that is not registered.
    /// The key here is the real one, whatever the session shows.
    pub fn key_status(&self, index: usize) -> Option<FileKeyStatus> {
        let _ = index;
        todo!("FND4: one file's key state from the key table")
    }

    /// The file's key as this session sends it (see `shown_key`), or `None`
    /// while it has none. This is the value of `X-File-Key` and, written in
    /// full, of the entry's `file_key`.
    pub fn shown_file_key(&self, index: usize) -> Option<String> {
        let _ = index;
        todo!("FND4: the key the session shows for a file")
    }

    /// The file a key received from a client names: `key` is a key as this
    /// session sends it, current or replaced. In a masked session a `sop:`
    /// key is looked up by its masked UID; a real UID a client could not
    /// have been sent names nothing. `None` for text that is not a file key
    /// and for a key no file has held.
    pub fn file_for_shown_key(&self, key: &str) -> Option<usize> {
        let _ = key;
        todo!("FND4: resolve a shown key, current or replaced, to its file")
    }

    /// Notes that a frame of the file was served, which is when a file
    /// without a key starts being hashed (`KeyTable::frame_sent`).
    ///
    /// Called on every display and raw frame response, so it is cheap: it
    /// takes the registry's write lock only the first time for each file
    /// (`KeyTable::served` under the read lock otherwise), never blocks on
    /// hashing and never fails. An index that is not registered is ignored.
    pub fn frame_sent(&self, index: usize) {
        let _ = index;
        todo!("FND4: mark the file served and queue a wanted digest")
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
        let _ = index;
        todo!("FND4: hash what the key needs and return it")
    }

    /// The hashing done so far.
    pub fn key_stats(&self) -> KeyStats {
        todo!("FND4: the hashing counters")
    }

    /// Stops hashing: no further slice starts, queued digests are dropped
    /// without an outcome (no failure is recorded for them), and waiting
    /// [`FileRegistry::ensure_key`] calls return [`KeyError::Stopped`]. The
    /// slice being read, if any, finishes first; it is at most
    /// `KEY_HASH_SLICE_BYTES` long. Idempotent, and final for this registry.
    pub fn stop_key_work(&self) {
        todo!("FND4: cancel background hashing")
    }
}

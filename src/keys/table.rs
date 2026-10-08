//! Which key each loaded file has.

use super::FileKey;
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What decides a file's key, read from the value that already holds it.
/// None of it needs a read beyond the header discovery already parsed.
///
/// The table keeps a few clones of every file registered with it and copies
/// nothing out of them: the UID and the path stay where the caller stored
/// them. A clone must therefore be cheap and allocate nothing, which an
/// `Arc` of anything that implements the trait is.
pub trait KeyedFile {
    /// The SOP Instance UID exactly as discovery read it (`docs/api.md`,
    /// "File keys", says how discovery reads it); empty for a raster and for
    /// a DICOM file without one.
    fn sop_instance_uid(&self) -> &str;
    /// The file's length in bytes at discovery.
    fn size_bytes(&self) -> u64;
    /// The path that names the file itself, as discovery resolved it (the
    /// canonical path). Two entries with equal paths are one file reached
    /// twice. The table compares this value and never touches the
    /// filesystem.
    fn path(&self) -> &Path;
}

impl<T: KeyedFile + ?Sized> KeyedFile for Arc<T> {
    fn sop_instance_uid(&self) -> &str {
        (**self).sop_instance_uid()
    }

    fn size_bytes(&self) -> u64 {
        (**self).size_bytes()
    }

    fn path(&self) -> &Path {
        (**self).path()
    }
}

/// A file's key facts as owned values, for callers that have no entry of
/// their own to share. Cloning it copies its strings, so a table of many
/// files registers `Arc<FileIdentity>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    pub sop_instance_uid: String,
    pub size_bytes: u64,
    pub path: PathBuf,
}

impl KeyedFile for FileIdentity {
    fn sop_instance_uid(&self) -> &str {
        &self.sop_instance_uid
    }

    fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

/// Why a file's digest could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyFailure {
    /// The file could not be opened or read.
    Unreadable,
    /// The file is not the one discovery saw.
    Changed,
}

/// One file's key replaced by another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rekey {
    pub index: usize,
    pub old_key: FileKey,
    pub new_key: FileKey,
}

/// What one call changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyChanges {
    /// The files whose key, `alias_of` or failure is different after the
    /// call, in ascending index order, each once. [`KeyTable::register`]
    /// always lists the file it registered.
    pub updated: Vec<usize>,
    /// The files among `updated` that had a key and now have another, in
    /// ascending index order. A file that gains its first key, or loses its
    /// key, is not listed.
    pub rekeys: Vec<Rekey>,
    /// The files whose digest should now be computed in the background, in
    /// ascending index order. An index is listed in `wanted` at most once in
    /// the table's life.
    pub wanted: Vec<usize>,
}

/// A file's key without the string: the table holds no `sop:` key as text,
/// because that text is `sop:` plus a UID the registered file already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRef<'a> {
    /// The key `sop:<uid>`; the value is the UID, as the file was registered
    /// with it.
    Sop(&'a str),
    /// A `b3:` key.
    Content(&'a FileKey),
}

impl<'a> From<&'a FileKey> for KeyRef<'a> {
    fn from(key: &'a FileKey) -> Self {
        match key.scheme() {
            super::KeyScheme::Sop => Self::Sop(key.body()),
            super::KeyScheme::Blake3 => Self::Content(key),
        }
    }
}

/// The part of a file's key state its catalog entry shows, borrowed from
/// the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyView<'a> {
    pub key: Option<KeyRef<'a>>,
    pub alias_of: Option<usize>,
    pub failure: Option<KeyFailure>,
}

/// One file's key as it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileKeyStatus {
    pub key: Option<FileKey>,
    pub alias_of: Option<usize>,
    /// Set when the last attempt to hash the file failed.
    pub failure: Option<KeyFailure>,
    /// The file's digest, when it has been computed.
    pub digest: Option<[u8; 32]>,
    /// [`KeyTable::settled`] for the file.
    pub settled: bool,
}

/// The answer to "may this file's key be written down?"
/// ([`KeyTable::rely_on`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reliance {
    /// The key, which is now settled: it is this file's key for as long as
    /// the table lives.
    Final(FileKey),
    /// A digest [`KeyTable::required_for`] names has no outcome yet.
    Wanted,
    /// A digest the key depends on could not be computed: the file's own,
    /// or the one of its group's first file.
    Failed(KeyFailure),
}

/// The keys of the registered files (`docs/design/annotation-model.md` 1.3,
/// 1.4, 1.7). Files are numbered in registration order from 0, which is the
/// catalog's file index.
///
/// # Terms
///
/// - A UID is *usable* when `FileKey::is_sop_uid` accepts it: 1 to 128
///   characters, each an ASCII letter, a digit, `.`, `-` or `_`, taken as
///   written.
/// - A file's *group* is the set of registered files with its usable UID.
///   The group's *first file* is the one registered first.
/// - Two entries are the *same file* when their [`KeyedFile::path`]s are
///   equal. A digest or a failure recorded for one is recorded for every
///   entry of that file, including entries registered later. A group whose
///   entries are all the same file is a group of one file.
/// - A group is *relied on* once [`KeyTable::rely_on`] has returned
///   `sop:<uid>` for one of its files. That key then names the bytes of the
///   group's first file for as long as the table lives.
/// - A group that is not relied on is *split* once two of its files are
///   known to hold different bytes: their sizes differ, or their digests
///   are both known and differ. It is also split when a key of it is asked
///   for ([`KeyTable::rely_on`]) and its first file could not be hashed:
///   `sop:<uid>` would name bytes nobody can read, so no file can be said
///   to share them. A split group stays split and is never relied on; a
///   group that is relied on never splits.
///
/// # The key a file has
///
/// 1. No usable UID (a raster, an empty UID, a UID with another character or
///    more than 128 of them): `b3:<digest>` once its digest is known, and no
///    key before that.
/// 2. A usable UID whose group is neither split nor relied on: `sop:<uid>`.
///    Every file of the group has this key; whether their bytes are equal
///    has not been checked. The key is *provisional*: it is what the file's
///    catalog entry and frames show, and it is not written down
///    ([`KeyTable::rely_on`]).
/// 3. A usable UID whose group is split: `b3:<digest>` once its digest is
///    known. Before that, a file that had `sop:<uid>` when the group split
///    keeps it, so that its key is replaced once, old by new, and never
///    goes missing in between; a file registered into a group that is
///    already split has no key. A file of a split group whose digest attempt
///    failed has no key.
/// 4. A usable UID whose group is relied on:
///    - the first file, and every file whose digest and the first file's
///      are both known and equal: `sop:<uid>`;
///    - a file known to differ from the first file (its size differs, or
///      both digests are known and differ): `b3:<digest>` once its digest
///      is known, and no key before that;
///    - any other file, which is one that has not been compared with the
///      first file yet: no key. A file that had the provisional `sop:<uid>`
///      and no digest when the group became relied on loses it at that
///      moment.
///
/// Under rules 1 to 3 the keys do not depend on registration order: a group
/// has `sop:` keys exactly while nothing shows its files to differ, and once
/// something does, every file of it is keyed by content, so byte-identical
/// copies share one `b3:` key and a differing copy gets its own. (A group
/// split because its first file could not be hashed is keyed by content the
/// same way, which is true of every file whatever the first file holds.)
/// Rule 4 is
/// what makes a key that was written down final. It does depend on what was
/// registered when the key was relied on: a file that turns up afterwards
/// can no longer change that key, so it is the late file that waits for a
/// comparison and takes a content key if it differs.
///
/// A key is replaced by another only under rule 3. Under rule 4 no file
/// goes from `sop:<uid>` to a `b3:` key, so no [`Rekey`] ever names a file
/// of a group that is relied on.
///
/// # Settled keys
///
/// A key is *settled* when it can no longer change: every `b3:` key, and
/// `sop:<uid>` in a group that is relied on. Two files with the same settled
/// key hold the same bytes, or are the same file.
///
/// # Aliases
///
/// A file's `alias_of` is the first file that came to hold its current key,
/// when that is another entry. Within one call, files that gain the same key
/// together gain it in ascending index order. A file has no `alias_of` while
/// it has no key, and none while it keeps the `sop:` key of a split group
/// (rule 3): that key no longer says the files are one image.
///
/// # Digests wanted in the background
///
/// A file is *served* once [`KeyTable::frame_sent`] has been called for it.
/// Its digest becomes wanted when it is served, its digest is unknown, no
/// failure is recorded for it, and it has or awaits a content key (rule 1,
/// rule 3, or a file of rule 4 that differs from the first file) or awaits
/// its comparison under rule 4. A served file that awaits its comparison
/// and has no recorded failure also makes the first file's digest wanted,
/// when that is unknown and no failure is recorded for it, since the
/// comparison needs both. A file under rule 2 is never
/// wanted: files that share a UID and a size are compared only when someone
/// relies on a key of theirs, so a dataset with a copy of every file is
/// never read twice just for being opened.
///
/// # Cost
///
/// Every call is a constant number of hash-map operations, plus one visit to
/// each entry of the same file, with these exceptions, each of which visits
/// every file of one group once: the call that splits the group; the call
/// that makes it relied on; the call that records the first file's digest
/// once it is relied on; and [`KeyTable::required_for`] and
/// [`KeyTable::rely_on`] for a file under rule 2 whose group has more than
/// one entry.
///
/// The table holds no string of its own for a file whose UID and path no
/// other registered file shares: such a file costs a fixed number of bytes
/// in the table whatever the length of its UID and path (a few machine
/// words and clones of `F` in vectors and hash maps) and no allocation of
/// its own; the table allocates only to grow those vectors and maps.
/// Nothing is built per file that a later call could build on demand: a
/// `sop:` key is text only in [`KeyTable::status`], [`Reliance::Final`] and
/// [`Rekey`]. `tests/key_table_memory.rs` holds the bound.
pub struct KeyTable<F> {
    files: Vec<F>,
    entries: Vec<Entry>,
    paths: HashMap<ByPath<F>, u32>,
    uids: HashMap<ByUid<F>, u32>,
    groups: HashMap<u32, Group>,
    digests: HashMap<u32, Digest>,
    holders: HashMap<FileKey, usize>,
}

const NONE: u32 = u32::MAX;
const SOP: u16 = 1;
const CONTENT: u16 = 2;
const SERVED: u16 = 4;
const WANTED: u16 = 8;
const SPLIT: u16 = 16;
const RELIED: u16 = 32;
const UNREADABLE: u16 = 64;
const CHANGED: u16 = 128;
const FAILURE: u16 = UNREADABLE | CHANGED;

#[derive(Clone, Copy)]
struct Entry {
    file: u32,
    group: u32,
    alias_of: u32,
    next: u32,
    flags: u16,
}

struct Group {
    members: Vec<usize>,
    multiple_files: bool,
    agreed: Option<[u8; 32]>,
}

struct Digest {
    bytes: [u8; 32],
    key: FileKey,
}

struct ByUid<F>(F);

impl<F: KeyedFile> Borrow<str> for ByUid<F> {
    fn borrow(&self) -> &str {
        self.0.sop_instance_uid()
    }
}
impl<F: KeyedFile> Hash for ByUid<F> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.sop_instance_uid().hash(state);
    }
}
impl<F: KeyedFile> PartialEq for ByUid<F> {
    fn eq(&self, other: &Self) -> bool {
        self.0.sop_instance_uid() == other.0.sop_instance_uid()
    }
}
impl<F: KeyedFile> Eq for ByUid<F> {}

struct ByPath<F>(F);

impl<F: KeyedFile> Borrow<Path> for ByPath<F> {
    fn borrow(&self) -> &Path {
        self.0.path()
    }
}
impl<F: KeyedFile> Hash for ByPath<F> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.path().hash(state);
    }
}
impl<F: KeyedFile> PartialEq for ByPath<F> {
    fn eq(&self, other: &Self) -> bool {
        self.0.path() == other.0.path()
    }
}
impl<F: KeyedFile> Eq for ByPath<F> {}

impl<F: KeyedFile + Clone> KeyTable<F> {
    /// An empty table.
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            entries: Vec::new(),
            paths: HashMap::new(),
            uids: HashMap::new(),
            groups: HashMap::new(),
            digests: HashMap::new(),
            holders: HashMap::new(),
        }
    }

    /// The number of registered files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether no file is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Registers the next file, whose index is [`KeyTable::len`] before the
    /// call, and gives it the key the rules on [`KeyTable`] say.
    ///
    /// While the file's group is neither split nor relied on, registering a
    /// file whose size differs from the group's splits the group; so does
    /// registering another entry of a file whose known digest differs from
    /// the group's. The files the split changes are in `updated`, those of
    /// them whose digest is already known are rekeyed in this call, and
    /// those that are served and still without a digest are in `wanted`.
    ///
    /// A file registered into a group that is relied on changes no other
    /// file: it has `sop:<uid>` at once when it is another entry of the
    /// first file or its known digest equals the first file's known digest,
    /// and otherwise the key rule 4 gives it.
    ///
    /// For a file whose UID and path no registered file has, this is two
    /// hash-map insertions and no allocation beyond the result and the
    /// growth of the table.
    pub fn register(&mut self, file: F) -> KeyChanges {
        let index = self.entries.len();
        let id = u32::try_from(index).expect("file index exceeds table capacity");
        assert_ne!(id, NONE, "file index exceeds table capacity");
        let physical = *self.paths.entry(ByPath(file.clone())).or_insert(id);
        let group = if FileKey::is_sop_uid(file.sop_instance_uid()) {
            *self.uids.entry(ByUid(file.clone())).or_insert(id)
        } else {
            NONE
        };
        let next = if physical != id {
            self.entries[physical as usize].next
        } else {
            NONE
        };
        self.files.push(file);
        self.entries.push(Entry {
            file: physical,
            group,
            alias_of: NONE,
            next,
            flags: if group == id { SOP } else { 0 },
        });
        // The ordinary case needs only the two entry insertions above.
        if physical == id && (group == id || group == NONE) {
            return KeyChanges {
                updated: vec![index],
                ..KeyChanges::default()
            };
        }
        if physical != id {
            self.entries[physical as usize].next = id;
        }
        let mut affected = Vec::new();
        if group != NONE && group != id {
            let first = group as usize;
            let agreed = self.digest(first);
            let members = self.groups.entry(group).or_insert_with(|| Group {
                members: vec![first],
                multiple_files: false,
                agreed,
            });
            members.members.push(index);
            members.multiple_files |= physical != self.entries[first].file;
            self.check_group(index, &mut affected);
        }
        affected.push(index);
        let mut changes = self.refresh(affected);
        if changes.updated.last() != Some(&index) {
            changes.updated.push(index);
        }
        changes
    }

    /// Records that a frame of the file has been served. Only `wanted` of
    /// the result can be non-empty. An index that is not registered changes
    /// nothing.
    pub fn frame_sent(&mut self, index: usize) -> KeyChanges {
        let mut changes = KeyChanges::default();
        if let Some(entry) = self.entries.get_mut(index) {
            entry.flags |= SERVED;
            self.want(index, &mut changes);
            changes.wanted.sort_unstable();
        }
        changes
    }

    /// Whether [`KeyTable::frame_sent`] has been called for the file.
    pub fn served(&self, index: usize) -> bool {
        self.entries
            .get(index)
            .is_some_and(|entry| entry.flags & SERVED != 0)
    }

    /// The files whose digests are unknown and must be computed before
    /// [`KeyTable::rely_on`] can settle the key of `index`, in ascending
    /// index order, never two entries of the same file. The file itself is
    /// named by `index`; any other file by its first entry.
    ///
    /// - Rule 1, rule 3, and a file of rule 4 that differs from the first
    ///   file: the file itself.
    /// - Rule 2, in a group of one file: nothing. Its `sop:` key is its own.
    /// - Rule 2, in a group of more than one file: every file of the group,
    ///   the file itself included. Each of them could share the key or
    ///   contest it, and a key that is written down has to be final, so
    ///   none is left unread. The work this asks for is the bytes of the
    ///   group's files, each read once.
    /// - Rule 4, the first file or a file compared equal to it: nothing.
    /// - Rule 4, a file not compared with the first file yet: the file
    ///   itself and the first file.
    ///
    /// A recorded failure does not exempt a file: asking again is how a
    /// failed digest is retried. Empty when nothing is needed, and for an
    /// index that is not registered.
    pub fn required_for(&self, index: usize) -> Vec<usize> {
        let Some(entry) = self.entries.get(index) else {
            return Vec::new();
        };
        let mut required = Vec::new();
        if self.provisional_group(index) {
            if let Some(group) = self.groups.get(&entry.group).filter(|g| g.multiple_files) {
                let mut seen = HashSet::new();
                for &member in &group.members {
                    let file = self.entries[member].file;
                    if seen.insert(file) && self.digest(member).is_none() {
                        required.push(if file == entry.file {
                            index
                        } else {
                            file as usize
                        });
                    }
                }
            }
        } else if !self.settled(index) {
            if self.digest(index).is_none() {
                required.push(index);
            }
            if self.awaiting_comparison(index) {
                let first = entry.group as usize;
                if self.digest(first).is_none() {
                    required.push(self.entries[first].file as usize);
                }
            }
        }
        required.sort_unstable();
        required.dedup();
        required
    }

    /// Settles the key of `index` if it can be settled now, and returns it.
    /// This is the one way a key becomes fit to be written into a record or
    /// an export: [`Reliance::Final`] is never followed by another key for
    /// that file, whatever is registered or hashed afterwards.
    ///
    /// - A file with a `b3:` key: `Final` with it.
    /// - A file that has or awaits a content key and has no digest:
    ///   `Failed` with its recorded failure, else `Wanted`.
    /// - Rule 2, in a group of one file: the group becomes relied on;
    ///   `Final(sop:<uid>)`. Nothing was read and nothing changes.
    /// - Rule 2, in a group of more than one file, in this order:
    ///   - the file's own digest is unknown: `Failed` with its recorded
    ///     failure, else `Wanted`;
    ///   - the first file's digest is unknown and no failure is recorded
    ///     for it: `Wanted`;
    ///   - the first file's digest is unknown and a failure is recorded for
    ///     it: the group is split, here and now. The file takes its `b3:`
    ///     key, which is the answer, `Final`. Every other file of the group
    ///     whose digest is known is rekeyed to its `b3:` key in the same
    ///     call, those without a digest keep `sop:<uid>` until they are
    ///     hashed (rule 3), and the first file has no key and keeps its
    ///     failure;
    ///   - any file of the group has neither a digest nor a recorded
    ///     failure: `Wanted`;
    ///   - otherwise the known digests are equal (a difference would have
    ///     split the group), the group becomes relied on and the answer is
    ///     `Final(sop:<uid>)`. The files of the group without a digest lose
    ///     their key then (rule 4) and are in `updated`.
    /// - Rule 4, the first file or a file compared equal to it:
    ///   `Final(sop:<uid>)`.
    /// - Rule 4, a file not compared with the first file yet: `Failed` with
    ///   its own recorded failure when its digest is unknown, else with the
    ///   first file's; `Wanted` when the digest that is missing has no
    ///   failure recorded.
    ///
    /// The first file's digest is asked of every file that shares its key
    /// because `sop:<uid>` names the first file's bytes
    /// ([`KeyTable::file_for_key`]): a file that cannot be compared with
    /// them cannot be said to be that image. Before any key of the group
    /// was relied on, a first file that cannot be read therefore costs the
    /// group its shared key and nothing else: each readable file is keyed
    /// by its own bytes. Afterwards the shared key stands, and it is the
    /// file asked about that has none.
    ///
    /// `None` for an index that is not registered. The changes are empty
    /// unless the call made the group relied on (only `updated`) or split
    /// it (`updated`, `rekeys`, and in `wanted` the served files that now
    /// await a content key).
    pub fn rely_on(&mut self, index: usize) -> (Option<Reliance>, KeyChanges) {
        let Some(entry) = self.entries.get(index).copied() else {
            return (None, KeyChanges::default());
        };
        let mut changes = KeyChanges::default();
        if self.settled(index) {
            return (self.owned_key(index).map(Reliance::Final), changes);
        }
        if self.provisional_group(index) {
            let multiple = self
                .groups
                .get(&entry.group)
                .is_some_and(|g| g.multiple_files);
            if multiple {
                if self.digest(index).is_none() {
                    return (Some(self.missing(index)), changes);
                }
                let first = entry.group as usize;
                if self.digest(first).is_none() {
                    if self.failure(first).is_none() {
                        return (Some(Reliance::Wanted), changes);
                    }
                    // The key would name bytes that cannot be read.
                    self.entries[first].flags |= SPLIT;
                    changes = self.refresh(self.groups[&entry.group].members.clone());
                    return (self.owned_key(index).map(Reliance::Final), changes);
                }
                let group = &self.groups[&entry.group];
                if group
                    .members
                    .iter()
                    .any(|&member| self.digest(member).is_none() && self.failure(member).is_none())
                {
                    return (Some(Reliance::Wanted), changes);
                }
            }
            self.entries[entry.group as usize].flags |= RELIED;
            if let Some(group) = self.groups.get(&entry.group) {
                changes = self.refresh(group.members.clone());
            }
            return (self.owned_key(index).map(Reliance::Final), changes);
        }
        let missing = if self.digest(index).is_none() {
            index
        } else {
            entry.group as usize
        };
        (Some(self.missing(missing)), changes)
    }

    /// Whether the file's key is settled: it has a `b3:` key, or it has
    /// `sop:<uid>` and its group is relied on. False for an index that is
    /// not registered.
    pub fn settled(&self, index: usize) -> bool {
        self.entries.get(index).is_some_and(|entry| {
            entry.flags & CONTENT != 0
                || (entry.flags & SOP != 0
                    && self.entries[entry.group as usize].flags & RELIED != 0)
        })
    }

    /// Records the outcome of hashing the file, for every entry of it.
    ///
    /// A digest clears a recorded failure. Under rule 1 the file gains its
    /// `b3:` key. Under rule 2 the digest is compared with the group's other
    /// known digests: a difference splits the group, and otherwise the key
    /// stays `sop:<uid>`. Under rule 3 the file gains its `b3:` key, which
    /// is a rekey when it still had `sop:<uid>`. Under rule 4 a file that is
    /// not the first file gains `sop:<uid>` or its `b3:` key once both its
    /// digest and the first file's are known, and the first file's digest
    /// does that for every file of the group that was waiting for it.
    ///
    /// A failure is recorded and, under rule 3, removes the `sop:` key the
    /// file kept; under the other rules the key, or its absence, stays.
    ///
    /// An outcome for an index that is not registered changes nothing. A
    /// digest for a file that already has one replaces nothing: the first
    /// digest stands.
    pub fn resolve(&mut self, index: usize, outcome: Result<[u8; 32], KeyFailure>) -> KeyChanges {
        let Some(entry) = self.entries.get(index) else {
            return KeyChanges::default();
        };
        let file = entry.file;
        if self.digests.contains_key(&file) {
            return KeyChanges::default();
        }
        if let Ok(bytes) = outcome {
            self.digests.insert(
                file,
                Digest {
                    bytes,
                    key: FileKey::blake3(&bytes),
                },
            );
        }
        let mut affected = Vec::new();
        let mut member = file;
        let mut groups = HashSet::new();
        while member != NONE {
            let index = member as usize;
            affected.push(index);
            if outcome.is_ok() {
                self.check_group(index, &mut affected);
                let group = self.entries[index].group;
                if group != NONE
                    && groups.insert(group)
                    && self.entries[group as usize].flags & RELIED != 0
                    && self.entries[group as usize].file == file
                {
                    if let Some(group) = self.groups.get(&group) {
                        affected.extend_from_slice(&group.members);
                    }
                }
            }
            member = self.entries[index].next;
        }
        // Refresh compares against the previous visible failure before it is changed.
        self.refresh_outcome(affected, Some((file, outcome.err())))
    }

    /// What a catalog entry shows of the file's key, borrowed, or `None` for
    /// an index that is not registered. The catalog reads this for every
    /// file it registers and every frame it serves, so it allocates nothing.
    pub fn view(&self, index: usize) -> Option<KeyView<'_>> {
        self.entries.get(index).map(|entry| KeyView {
            key: if entry.flags & SOP != 0 {
                Some(KeyRef::Sop(self.files[index].sop_instance_uid()))
            } else if entry.flags & CONTENT != 0 {
                Some(KeyRef::Content(&self.digests[&entry.file].key))
            } else {
                None
            },
            alias_of: (entry.alias_of != NONE).then_some(entry.alias_of as usize),
            failure: self.failure(index),
        })
    }

    /// The file's key state, or `None` for an index that is not registered.
    pub fn status(&self, index: usize) -> Option<FileKeyStatus> {
        self.entries.get(index).map(|entry| FileKeyStatus {
            key: self.owned_key(index),
            alias_of: (entry.alias_of != NONE).then_some(entry.alias_of as usize),
            failure: self.failure(index),
            digest: self.digest(index),
            settled: self.settled(index),
        })
    }

    /// The file a key names. A `sop:` key names the first file registered
    /// with that UID, and keeps naming it after a split has replaced the key
    /// (`docs/design/annotation-model.md` 1.7: "the server keeps an
    /// old-to-new map"), so an operation sent under the old key still finds
    /// its file. A `b3:` key names the first file that came to hold it.
    /// `None` for a key no file has held.
    ///
    /// For a settled key the answer never changes, and it is the file the
    /// key was settled for or one that holds the same bytes.
    pub fn file_for_key(&self, key: &FileKey) -> Option<usize> {
        match key.scheme() {
            super::KeyScheme::Sop => self.uids.get(key.body()).map(|&index| index as usize),
            super::KeyScheme::Blake3 => self.holders.get(key).copied(),
        }
    }
}

impl<F: KeyedFile + Clone> KeyTable<F> {
    fn digest(&self, index: usize) -> Option<[u8; 32]> {
        self.digests.get(&self.entries[index].file).map(|d| d.bytes)
    }

    fn failure(&self, index: usize) -> Option<KeyFailure> {
        match self.entries[index].flags & FAILURE {
            UNREADABLE => Some(KeyFailure::Unreadable),
            CHANGED => Some(KeyFailure::Changed),
            _ => None,
        }
    }

    fn owned_key(&self, index: usize) -> Option<FileKey> {
        match self.view(index)?.key? {
            KeyRef::Sop(uid) => FileKey::sop(uid).ok(),
            KeyRef::Content(key) => Some(key.clone()),
        }
    }

    fn missing(&self, index: usize) -> Reliance {
        self.failure(index)
            .map_or(Reliance::Wanted, Reliance::Failed)
    }

    fn provisional_group(&self, index: usize) -> bool {
        let group = self.entries[index].group;
        group != NONE && self.entries[group as usize].flags & (SPLIT | RELIED) == 0
    }

    fn awaiting_comparison(&self, index: usize) -> bool {
        let entry = &self.entries[index];
        if entry.group == NONE {
            return false;
        }
        let first = entry.group as usize;
        self.entries[first].flags & RELIED != 0
            && entry.file != self.entries[first].file
            && self.files[index].size_bytes() == self.files[first].size_bytes()
            && (self.digest(index).is_none() || self.digest(first).is_none())
    }

    fn check_group(&mut self, index: usize, affected: &mut Vec<usize>) {
        if !self.provisional_group(index) {
            return;
        }
        let first = self.entries[index].group;
        let digest = self.digest(index);
        let Some(group) = self.groups.get_mut(&first) else {
            return;
        };
        if self.files[index].size_bytes() != self.files[first as usize].size_bytes()
            || matches!((digest, group.agreed), (Some(a), Some(b)) if a != b)
        {
            self.entries[first as usize].flags |= SPLIT;
            affected.extend_from_slice(&group.members);
        } else if group.agreed.is_none() {
            group.agreed = digest;
        }
    }

    fn mark_wanted(&mut self, index: usize, changes: &mut KeyChanges) {
        if self.entries[index].flags & WANTED == 0
            && self.digest(index).is_none()
            && self.failure(index).is_none()
        {
            self.entries[index].flags |= WANTED;
            changes.wanted.push(index);
        }
    }

    fn want(&mut self, index: usize, changes: &mut KeyChanges) {
        if !self.served(index)
            || self.provisional_group(index)
            || self.settled(index)
            || self.failure(index).is_some()
        {
            return;
        }
        self.mark_wanted(index, changes);
        if self.awaiting_comparison(index) {
            self.mark_wanted(self.entries[index].group as usize, changes);
        }
    }

    fn refresh(&mut self, affected: Vec<usize>) -> KeyChanges {
        self.refresh_outcome(affected, None)
    }

    fn refresh_outcome(
        &mut self,
        mut affected: Vec<usize>,
        outcome: Option<(u32, Option<KeyFailure>)>,
    ) -> KeyChanges {
        affected.sort_unstable();
        affected.dedup();
        let mut changes = KeyChanges::default();
        for index in affected {
            let old = self.entries[index];
            let failure = if let Some((_, failure)) = outcome.filter(|(file, _)| *file == old.file)
            {
                failure
            } else {
                self.failure(old.file as usize)
            };
            let digest = self.digest(index);
            let key = if old.group == NONE {
                if digest.is_some() {
                    CONTENT
                } else {
                    0
                }
            } else {
                let first = old.group as usize;
                let state = self.entries[first].flags;
                if state & SPLIT != 0 {
                    if digest.is_some() {
                        CONTENT
                    } else if failure.is_some() {
                        0
                    } else {
                        old.flags & SOP
                    }
                } else if state & RELIED != 0 {
                    if old.file == self.entries[first].file
                        || matches!((digest, self.digest(first)), (Some(a), Some(b)) if a == b)
                    {
                        SOP
                    } else if digest.is_some()
                        && (self.files[index].size_bytes() != self.files[first].size_bytes()
                            || self.digest(first).is_some())
                    {
                        CONTENT
                    } else {
                        0
                    }
                } else {
                    SOP
                }
            };
            let alias = if key == CONTENT {
                let digest = &self.digests[&old.file];
                *self.holders.entry(digest.key.clone()).or_insert(index) as u32
            } else if key == SOP && self.entries[old.group as usize].flags & SPLIT == 0 {
                old.group
            } else {
                NONE
            };
            let alias = if alias as usize == index { NONE } else { alias };
            let failure = match failure {
                Some(KeyFailure::Unreadable) => UNREADABLE,
                Some(KeyFailure::Changed) => CHANGED,
                None => 0,
            };
            if old.flags & (SOP | CONTENT | FAILURE) != key | failure || old.alias_of != alias {
                if old.flags & SOP != 0 && key == CONTENT {
                    if let Ok(old_key) = FileKey::sop(self.files[index].sop_instance_uid()) {
                        changes.rekeys.push(Rekey {
                            index,
                            old_key,
                            new_key: self.digests[&old.file].key.clone(),
                        });
                    }
                }
                let entry = &mut self.entries[index];
                entry.flags = (entry.flags & !(SOP | CONTENT | FAILURE)) | key | failure;
                entry.alias_of = alias;
                changes.updated.push(index);
            }
            self.want(index, &mut changes);
        }
        changes.wanted.sort_unstable();
        changes
    }
}

impl<F: KeyedFile + Clone> Default for KeyTable<F> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileIdentity, KeyChanges, KeyFailure, KeyRef, KeyTable, Reliance};
    use crate::keys::FileKey;
    use std::path::PathBuf;

    const UID: &str = "1.2.826.0.1.3680043.10.511.7";

    type Table = KeyTable<FileIdentity>;

    fn file(uid: &str, size: u64, path: &str) -> FileIdentity {
        FileIdentity {
            sop_instance_uid: uid.to_string(),
            size_bytes: size,
            path: PathBuf::from(path),
        }
    }

    fn digest(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn b3(byte: u8) -> String {
        FileKey::blake3(&digest(byte)).as_str().to_string()
    }

    fn sop(uid: &str) -> String {
        format!("sop:{uid}")
    }

    /// `(key, alias_of)` of every file, for comparing a whole table at once.
    fn keys(table: &Table) -> Vec<(Option<String>, Option<usize>)> {
        (0..table.len())
            .map(|index| {
                let status = table.status(index).expect("registered file");
                (
                    status.key.map(|key| key.as_str().to_string()),
                    status.alias_of,
                )
            })
            .collect()
    }

    fn key(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    fn key_of(table: &Table, index: usize) -> Option<String> {
        table
            .status(index)
            .expect("registered file")
            .key
            .map(|key| key.as_str().to_string())
    }

    /// The key and alias each file of a case is expected to have.
    type Expected = Vec<(Option<String>, Option<usize>)>;

    /// A file found late: its name, the file, its key before it is read,
    /// how many files relying on it reads besides a first file that was
    /// never read, its key once read, and its alias then.
    type LateCase = (
        &'static str,
        Member,
        Option<String>,
        usize,
        String,
        Option<usize>,
    );

    /// One file of a group case: a path, a size and the byte its digest is
    /// made of, so equal bytes stand for equal content.
    #[derive(Debug, Clone, Copy)]
    struct Member {
        path: &'static str,
        size: u64,
        content: u8,
    }

    const fn member(path: &'static str, size: u64, content: u8) -> Member {
        Member {
            path,
            size,
            content,
        }
    }

    /// Every order of `0..count`.
    fn orders(count: usize) -> Vec<Vec<usize>> {
        if count == 0 {
            return vec![Vec::new()];
        }
        let mut all = Vec::new();
        for shorter in orders(count - 1) {
            for position in 0..=shorter.len() {
                let mut order = shorter.clone();
                order.insert(position, count - 1);
                all.push(order);
            }
        }
        all
    }

    /// Does what a caller that needs a key does: hashes what
    /// `required_for` names, each file once, asks again in case that made
    /// more required, and then takes the answer of `rely_on`. Returns the
    /// answer, every index it hashed in order, and every change the table
    /// reported on the way.
    fn rely(
        table: &mut Table,
        index: usize,
        outcome: &dyn Fn(usize) -> Result<[u8; 32], KeyFailure>,
    ) -> (Reliance, Vec<usize>, Vec<KeyChanges>) {
        let mut hashed = Vec::new();
        let mut changes = Vec::new();
        loop {
            let required = table
                .required_for(index)
                .into_iter()
                .filter(|file| !hashed.contains(file))
                .collect::<Vec<_>>();
            if required.is_empty() {
                let (reliance, changed) = table.rely_on(index);
                changes.push(changed);
                return (reliance.expect("registered file"), hashed, changes);
            }
            for file in required {
                hashed.push(file);
                changes.push(table.resolve(file, outcome(file)));
            }
        }
    }

    #[test]
    fn a_file_gets_the_key_its_uid_and_the_other_files_allow() {
        // One table per case: the files registered in order, then the key
        // and alias of each before any file is hashed or relied on.
        let cases: Vec<(&str, Vec<FileIdentity>, Expected)> = vec![
            (
                "unique UIDs",
                vec![file("1.2.3", 10, "/a"), file("1.2.4", 10, "/b")],
                vec![(key("sop:1.2.3"), None), (key("sop:1.2.4"), None)],
            ),
            (
                "a UID a de-identifier replaced keeps a UID key",
                vec![file("anon-0001_A.b", 10, "/a")],
                vec![(key("sop:anon-0001_A.b"), None)],
            ),
            (
                "same UID and size: aliases, bytes not compared",
                vec![
                    file(UID, 10, "/all/a"),
                    file(UID, 10, "/train/a"),
                    file(UID, 10, "/test/a"),
                ],
                vec![
                    (key(&sop(UID)), None),
                    (key(&sop(UID)), Some(0)),
                    (key(&sop(UID)), Some(0)),
                ],
            ),
            (
                "same UID, another size: the first keeps its key until hashed, the second waits",
                vec![file(UID, 10, "/a"), file(UID, 11, "/b")],
                vec![(key(&sop(UID)), None), (None, None)],
            ),
            (
                "a copy and a differing file: no alias under a key the group lost",
                vec![
                    file(UID, 10, "/a"),
                    file(UID, 10, "/copy"),
                    file(UID, 11, "/other"),
                    file(UID, 10, "/late-copy"),
                ],
                vec![
                    (key(&sop(UID)), None),
                    (key(&sop(UID)), None),
                    (None, None),
                    (None, None),
                ],
            ),
            (
                "no UID, a raster, and UIDs that are not usable",
                vec![
                    file("", 10, "/no-uid.dcm"),
                    file("", 10, "/image.png"),
                    file("1.2 3", 10, "/space.dcm"),
                    file("1.2/3", 10, "/slash.dcm"),
                    file(&"1".repeat(129), 10, "/long.dcm"),
                    file(&"1".repeat(128), 10, "/longest.dcm"),
                ],
                vec![
                    (None, None),
                    (None, None),
                    (None, None),
                    (None, None),
                    (None, None),
                    (Some(format!("sop:{}", "1".repeat(128))), None),
                ],
            ),
            (
                "one file reached by two paths that resolve to it",
                vec![file(UID, 10, "/data/a.dcm"), file(UID, 10, "/data/a.dcm")],
                vec![(key(&sop(UID)), None), (key(&sop(UID)), Some(0))],
            ),
        ];

        for (name, files, expected) in cases {
            let mut table = Table::new();
            for (index, identity) in files.iter().cloned().enumerate() {
                let changes = table.register(identity);
                assert!(changes.updated.contains(&index), "{name}: file {index}");
                assert!(changes.wanted.is_empty(), "{name}: nothing is served yet");
            }
            assert_eq!(keys(&table), expected, "{name}");
            // The borrowed view says the same without building a key: a
            // `sop:` key is the file's own UID, as it was registered.
            for (index, identity) in files.iter().enumerate() {
                let view = table.view(index).expect("registered");
                let shown = view.key.map(|key| match key {
                    KeyRef::Sop(uid) => {
                        assert_eq!(uid, identity.sop_instance_uid, "{name}: file {index}");
                        sop(uid)
                    }
                    KeyRef::Content(key) => key.as_str().to_string(),
                });
                assert_eq!(
                    (shown, view.alias_of),
                    expected[index],
                    "{name}: file {index}"
                );
                assert!(
                    !table.settled(index),
                    "{name}: nothing was hashed or relied on"
                );
            }
        }
    }

    #[test]
    fn keys_do_not_depend_on_the_order_files_are_found_in() {
        // Two byte-identical copies and one differing file under one UID,
        // registered in every order and then all hashed: each ends with the
        // key of its content.
        let files = [
            (file(UID, 10, "/a"), 1_u8),
            (file(UID, 10, "/copy"), 1),
            (file(UID, 11, "/other"), 2),
        ];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut table = Table::new();
            for position in order {
                table.register(files[position].0.clone());
            }
            for (index, position) in order.into_iter().enumerate() {
                table.resolve(index, Ok(digest(files[position].1)));
            }
            for (index, position) in order.into_iter().enumerate() {
                let status = table.status(index).expect("registered");
                assert_eq!(
                    status.key.map(|key| key.as_str().to_string()),
                    Some(b3(files[position].1)),
                    "order {order:?}, file {position}"
                );
                assert!(status.settled, "order {order:?}, file {position}");
            }
            let copies = order
                .into_iter()
                .enumerate()
                .filter(|(_, position)| *position != 2)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            assert_eq!(
                (
                    table.status(copies[0]).expect("first copy").alias_of,
                    table.status(copies[1]).expect("second copy").alias_of
                ),
                (None, Some(copies[0])),
                "order {order:?}: the later copy is the alias of the earlier"
            );
        }
    }

    #[test]
    fn a_pending_key_resolves_once_and_identical_bytes_share_it() {
        let mut table = Table::new();
        table.register(file("", 10, "/one.png"));
        table.register(file("", 10, "/two.png"));
        table.register(file("", 99, "/three.png"));

        // Nothing is wanted until a frame is served, and then once.
        assert!(table.frame_sent(7).wanted.is_empty(), "unknown index");
        assert!(!table.served(1));
        assert_eq!(table.frame_sent(1).wanted, vec![1]);
        assert!(table.served(1));
        assert!(table.frame_sent(1).wanted.is_empty());

        let changes = table.resolve(1, Ok(digest(5)));
        assert_eq!(changes.updated, vec![1]);
        assert!(
            changes.rekeys.is_empty(),
            "a first key replaces no other key"
        );
        assert_eq!(
            keys(&table),
            vec![(None, None), (key(&b3(5)), None), (None, None)]
        );

        // The lower index resolves later to the same bytes: the file that
        // held the key first stays the one the other is an alias of.
        let changes = table.resolve(0, Ok(digest(5)));
        assert_eq!(changes.updated, vec![0]);
        assert_eq!(
            keys(&table),
            vec![(key(&b3(5)), Some(1)), (key(&b3(5)), None), (None, None)]
        );
        assert_eq!(
            table.file_for_key(&FileKey::blake3(&digest(5))),
            Some(1),
            "a key names the first file that held it"
        );

        // A second digest for a file changes nothing.
        assert_eq!(table.resolve(1, Ok(digest(6))), Default::default());
        assert_eq!(table.status(1).expect("registered").digest, Some(digest(5)));
        assert_eq!(table.resolve(42, Ok(digest(6))), Default::default());
    }

    #[test]
    fn a_collision_found_late_replaces_each_key_once() {
        let mut table = Table::new();
        table.register(file(UID, 10, "/a"));
        table.register(file(UID, 10, "/copy"));
        table.frame_sent(0);
        assert!(
            table.frame_sent(1).wanted.is_empty(),
            "aliases are not hashed for being viewed"
        );
        for index in 0..2 {
            assert!(
                !table.status(index).expect("registered").settled,
                "file {index}: a key shared by UID and size is unchecked"
            );
        }

        // A third file with the UID and another size: the group splits. Both
        // earlier files were served, so both digests are wanted now.
        let changes = table.register(file(UID, 11, "/other"));
        assert_eq!(changes.updated, vec![1, 2], "file 0 shows what it showed");
        assert!(changes.rekeys.is_empty());
        assert_eq!(changes.wanted, vec![0, 1]);
        assert_eq!(
            keys(&table),
            vec![(key(&sop(UID)), None), (key(&sop(UID)), None), (None, None)]
        );
        assert!(!table.status(0).expect("registered").settled);
        // Each file of a split group needs its own digest and no other.
        assert_eq!(table.required_for(1), vec![1]);

        let changes = table.resolve(0, Ok(digest(1)));
        assert_eq!(changes.updated, vec![0]);
        assert_eq!(changes.rekeys.len(), 1);
        assert_eq!(
            (
                changes.rekeys[0].index,
                changes.rekeys[0].old_key.as_str(),
                changes.rekeys[0].new_key.as_str()
            ),
            (0, sop(UID).as_str(), b3(1).as_str())
        );
        assert!(table.status(0).expect("registered").settled);

        // The copy has the same bytes: it follows to the same key.
        let changes = table.resolve(1, Ok(digest(1)));
        assert_eq!(changes.rekeys.len(), 1);
        assert_eq!(changes.rekeys[0].new_key.as_str(), b3(1));
        // The third never had a key, so its first key is not a rekey.
        assert_eq!(table.frame_sent(2).wanted, vec![2]);
        let changes = table.resolve(2, Ok(digest(2)));
        assert!(changes.rekeys.is_empty());
        assert_eq!(
            keys(&table),
            vec![
                (key(&b3(1)), None),
                (key(&b3(1)), Some(0)),
                (key(&b3(2)), None)
            ]
        );

        // The replaced key still names the file that held it first.
        assert_eq!(
            table.file_for_key(&FileKey::sop(UID).expect("usable UID")),
            Some(0)
        );
        assert_eq!(
            table.file_for_key(&FileKey::sop("1.2.3").expect("usable UID")),
            None
        );
    }

    /// `docs/design/annotation-model.md` 1.7: a provisional alias is
    /// verified "before the first annotation write on either file". A key
    /// is relied on only after every file that could share or contest it
    /// was read, whichever file of the group is asked about and whatever
    /// order the files were found in, and from then on it does not change.
    #[test]
    fn a_shared_uid_key_is_relied_on_only_after_every_file_of_the_group_was_read() {
        let cases: Vec<(&str, Vec<Member>)> = vec![
            (
                "two copies",
                vec![member("/all/a", 10, 1), member("/train/a", 10, 1)],
            ),
            (
                "two files of one size that differ",
                vec![member("/a", 10, 1), member("/b", 10, 2)],
            ),
            (
                "two copies and a file of the same size that differs",
                vec![
                    member("/a", 10, 1),
                    member("/b", 10, 2),
                    member("/copy", 10, 1),
                ],
            ),
            (
                "three copies",
                vec![
                    member("/all/a", 10, 1),
                    member("/train/a", 10, 1),
                    member("/test/a", 10, 1),
                ],
            ),
            (
                "three files of one size that all differ",
                vec![
                    member("/a", 10, 1),
                    member("/b", 10, 2),
                    member("/c", 10, 3),
                ],
            ),
            (
                "one file reached twice and a copy of it",
                vec![
                    member("/a", 10, 1),
                    member("/a", 10, 1),
                    member("/copy", 10, 1),
                ],
            ),
        ];
        for (name, members) in cases {
            let identical = members
                .iter()
                .all(|member| member.content == members[0].content);
            let mut paths = members.iter().map(|member| member.path).collect::<Vec<_>>();
            paths.sort_unstable();
            paths.dedup();
            for order in orders(members.len()) {
                for asked in 0..members.len() {
                    let context = format!("{name}, order {order:?}, asked {asked}");
                    let mut table = Table::new();
                    for &position in &order {
                        let member = members[position];
                        table.register(file(UID, member.size, member.path));
                    }
                    let content = |index: usize| members[order[index]].content;
                    let before = keys(&table);

                    // Nothing was read: the key cannot be relied on, and
                    // being asked changes nothing.
                    let (reliance, changes) = table.rely_on(asked);
                    assert_eq!(reliance, Some(Reliance::Wanted), "{context}");
                    assert_eq!(changes, KeyChanges::default(), "{context}");
                    assert_eq!(keys(&table), before, "{context}");

                    let (reliance, hashed, _) =
                        rely(&mut table, asked, &|index| Ok(digest(content(index))));
                    // Every file of the group was read, each once.
                    let mut read = hashed
                        .iter()
                        .map(|&index| members[order[index]].path)
                        .collect::<Vec<_>>();
                    read.sort_unstable();
                    assert_eq!(read, paths, "{context}: files read");

                    let expected = if identical {
                        sop(UID)
                    } else {
                        b3(content(asked))
                    };
                    assert_eq!(
                        reliance,
                        Reliance::Final(FileKey::parse(&expected).expect("key")),
                        "{context}"
                    );
                    assert!(table.settled(asked), "{context}");

                    // The other files' keys are settled by the same reads,
                    // and no later answer or file changes the first one.
                    let settled = keys(&table);
                    for index in 0..members.len() {
                        let (reliance, hashed, changes) =
                            rely(&mut table, index, &|index| Ok(digest(content(index))));
                        assert!(hashed.is_empty(), "{context}: file {index} read again");
                        assert!(
                            changes
                                .iter()
                                .all(|changes| *changes == KeyChanges::default()),
                            "{context}: file {index}"
                        );
                        let expected = if identical {
                            sop(UID)
                        } else {
                            b3(content(index))
                        };
                        assert_eq!(
                            reliance,
                            Reliance::Final(FileKey::parse(&expected).expect("key")),
                            "{context}: file {index}"
                        );
                    }
                    let late = table.register(file(UID, 11, "/late/other-size"));
                    assert_eq!(late.updated, vec![members.len()], "{context}");
                    assert!(late.rekeys.is_empty(), "{context}");
                    table.register(file(UID, 10, "/late/same-size"));
                    table.resolve(members.len(), Ok(digest(8)));
                    table.resolve(members.len() + 1, Ok(digest(9)));
                    assert_eq!(
                        keys(&table)[..members.len()],
                        settled[..],
                        "{context}: keys after two late files"
                    );
                    // A settled key names a file with the same bytes as the
                    // one it was given for, and always the same one.
                    let named = table
                        .file_for_key(&FileKey::parse(&expected).expect("key"))
                        .expect("a settled key names a file");
                    assert_eq!(content(named), content(asked), "{context}");
                }
            }
        }
    }

    /// The decision this pins is the owner's to confirm: once a key was
    /// relied on, a file found later with the same UID cannot change it, so
    /// it is the late file that waits and, if it differs, takes a content
    /// key. `docs/design/annotation-model.md` 1.7 has the earlier file
    /// rekeyed instead ("or even annotated").
    #[test]
    fn a_file_found_after_a_key_was_relied_on_never_changes_that_key() {
        // The group before the late file: one file relied on without a
        // read, or two copies relied on after both were read.
        for copies in [1_usize, 2] {
            // (name, late file, its content, key before it is read, what
            // relying on it reads, key once read, alias)
            let cases: Vec<LateCase> = vec![
                (
                    "the same file again",
                    member("/a", 10, 1),
                    key(&sop(UID)),
                    0,
                    sop(UID),
                    Some(0),
                ),
                ("a copy", member("/late", 10, 1), None, 1, sop(UID), Some(0)),
                (
                    "the same size, other bytes",
                    member("/late", 10, 2),
                    None,
                    1,
                    b3(2),
                    None,
                ),
                ("another size", member("/late", 11, 2), None, 1, b3(2), None),
            ];
            for (name, late, pending, reads, read_key, alias) in cases {
                let context = format!("{copies} file(s) relied on, then {name}");
                let mut table = Table::new();
                table.register(file(UID, 10, "/a"));
                if copies == 2 {
                    table.register(file(UID, 10, "/copy"));
                }
                let content = |index: usize| if index < copies { 1 } else { late.content };
                let (reliance, hashed, _) =
                    rely(&mut table, 0, &|index| Ok(digest(content(index))));
                assert_eq!(
                    reliance,
                    Reliance::Final(FileKey::sop(UID).expect("key")),
                    "{context}"
                );
                assert_eq!(
                    hashed.len(),
                    if copies == 1 { 0 } else { 2 },
                    "{context}: one file with a UID of its own is never read for its key"
                );
                let relied = keys(&table);

                let index = copies;
                let changes = table.register(file(UID, late.size, late.path));
                assert_eq!(changes.updated, vec![index], "{context}");
                assert!(changes.rekeys.is_empty(), "{context}");
                assert_eq!(key_of(&table, index), pending, "{context}: before a read");
                assert_eq!(keys(&table)[..copies], relied[..], "{context}");

                // Viewing the late file asks for what its comparison needs:
                // itself, and the first file when that was never read.
                let wanted = table.frame_sent(index).wanted;
                let mut expected_wanted = Vec::new();
                if pending.is_none() {
                    if copies == 1 && late.size == 10 {
                        expected_wanted.push(0);
                    }
                    expected_wanted.push(index);
                }
                assert_eq!(wanted, expected_wanted, "{context}: wanted when served");

                let (reliance, hashed, changes) =
                    rely(&mut table, index, &|index| Ok(digest(content(index))));
                assert_eq!(
                    reliance,
                    Reliance::Final(FileKey::parse(&read_key).expect("key")),
                    "{context}"
                );
                let first_unread = copies == 1 && late.size == 10 && late.path != "/a";
                assert_eq!(
                    hashed.len(),
                    reads + usize::from(first_unread),
                    "{context}: files read for the late file"
                );
                assert!(
                    changes.iter().all(|changes| changes.rekeys.is_empty()),
                    "{context}: no key of a group that is relied on is replaced"
                );
                assert_eq!(
                    (
                        key_of(&table, index),
                        table.status(index).expect("registered").alias_of
                    ),
                    (key(&read_key), alias),
                    "{context}"
                );
                assert_eq!(keys(&table)[..copies], relied[..], "{context}");
                for earlier in 0..copies {
                    assert!(table.settled(earlier), "{context}: file {earlier}");
                }
                assert_eq!(
                    table.file_for_key(&FileKey::sop(UID).expect("key")),
                    Some(0),
                    "{context}: the UID key names the file it was relied on for"
                );
            }
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_holds_up_only_the_keys_that_depend_on_it() {
        let unreadable = Err(KeyFailure::Unreadable);

        // A copy that cannot be read does not stop the first file's key
        // from being relied on. It loses the key it showed, because nothing
        // says it is the same image, and gets its own answer when read.
        for (later, expected) in [(1_u8, sop(UID)), (2, b3(2))] {
            let mut table = Table::new();
            table.register(file(UID, 10, "/a"));
            table.register(file(UID, 10, "/copy"));
            table.register(file(UID, 10, "/gone"));
            let (reliance, hashed, changes) = rely(&mut table, 0, &|index| {
                if index == 2 {
                    unreadable
                } else {
                    Ok(digest(1))
                }
            });
            assert_eq!(reliance, Reliance::Final(FileKey::sop(UID).expect("key")));
            assert_eq!(hashed, vec![0, 1, 2]);
            assert_eq!(
                changes.last().expect("the answer").updated,
                vec![2],
                "the unread file changes when the key is relied on"
            );
            assert_eq!(
                keys(&table),
                vec![
                    (key(&sop(UID)), None),
                    (key(&sop(UID)), Some(0)),
                    (None, None)
                ]
            );
            let status = table.status(2).expect("registered");
            assert_eq!(
                (status.failure, status.settled),
                (Some(KeyFailure::Unreadable), false)
            );
            assert_eq!(
                table.rely_on(2).0,
                Some(Reliance::Failed(KeyFailure::Unreadable))
            );
            assert_eq!(table.required_for(2), vec![2], "asking retries it");

            // Readable again: its first key, which replaces no other.
            let changes = table.resolve(2, Ok(digest(later)));
            assert_eq!(changes.updated, vec![2]);
            assert!(changes.rekeys.is_empty());
            let status = table.status(2).expect("registered");
            assert_eq!(
                (
                    status.key.map(|key| key.as_str().to_string()),
                    status.failure,
                    status.settled
                ),
                (key(&expected), None, true)
            );
            assert_eq!(
                keys(&table)[..2],
                [(key(&sop(UID)), None), (key(&sop(UID)), Some(0))]
            );
        }

        // The file asked about cannot be read: its key cannot be relied on,
        // and nothing changes but the failure shown.
        for (gone, asked, failed) in [(1_usize, 1_usize, true), (0, 0, true)] {
            let context = format!("file {gone} unreadable, file {asked} asked");
            let mut table = Table::new();
            table.register(file(UID, 10, "/a"));
            table.register(file(UID, 10, "/copy"));
            let (reliance, hashed, _) = rely(&mut table, asked, &|index| {
                if index == gone {
                    Err(KeyFailure::Changed)
                } else {
                    Ok(digest(1))
                }
            });
            assert_eq!(
                reliance == Reliance::Failed(KeyFailure::Changed),
                failed,
                "{context}: {reliance:?}"
            );
            assert_eq!(hashed, vec![0, 1], "{context}");
            assert_eq!(
                keys(&table),
                vec![(key(&sop(UID)), None), (key(&sop(UID)), Some(0))],
                "{context}: the keys shown stay"
            );
            assert!(!table.settled(0) && !table.settled(1), "{context}");
            assert_eq!(
                table.status(gone).expect("registered").failure,
                Some(KeyFailure::Changed),
                "{context}"
            );
            // Asked again once it can be read, the key settles.
            let (reliance, hashed, _) = rely(&mut table, asked, &|_| Ok(digest(1)));
            assert_eq!(
                reliance,
                Reliance::Final(FileKey::sop(UID).expect("key")),
                "{context}"
            );
            assert_eq!(
                hashed,
                vec![gone],
                "{context}: only the failed file is read again"
            );
        }

        // The group's first file cannot be read when another file's key is
        // asked for: the shared key would name bytes nobody can read, so
        // the group is split there. Every file that was read has its
        // content key, which is true whatever the first file holds; one
        // that was not read keeps the key it showed until it is; the first
        // file has no key and keeps its failure.
        for failure in [KeyFailure::Unreadable, KeyFailure::Changed] {
            let context = format!("the first file is {failure:?}");
            let mut table = Table::new();
            for path in ["/a", "/copy", "/copy-2", "/unread"] {
                table.register(file(UID, 10, path));
            }
            assert!(table.frame_sent(3).wanted.is_empty(), "{context}");
            table.resolve(0, Err(failure));
            table.resolve(1, Ok(digest(1)));
            table.resolve(2, Ok(digest(1)));
            assert_eq!(
                table.rely_on(0).0,
                Some(Reliance::Failed(failure)),
                "{context}: the first file's own key"
            );
            assert_eq!(keys(&table)[1], (key(&sop(UID)), Some(0)), "{context}");

            let (reliance, changes) = table.rely_on(1);
            assert_eq!(
                reliance,
                Some(Reliance::Final(FileKey::parse(&b3(1)).expect("key"))),
                "{context}"
            );
            assert_eq!(changes.updated, vec![0, 1, 2, 3], "{context}");
            let rekeyed = changes
                .rekeys
                .iter()
                .map(|rekey| {
                    (
                        rekey.index,
                        rekey.old_key.as_str().to_string(),
                        rekey.new_key.as_str().to_string(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                rekeyed,
                vec![(1, sop(UID), b3(1)), (2, sop(UID), b3(1))],
                "{context}"
            );
            assert_eq!(
                changes.wanted,
                vec![3],
                "{context}: the viewed file now awaits a content key"
            );
            assert_eq!(
                keys(&table),
                vec![
                    (None, None),
                    (key(&b3(1)), None),
                    (key(&b3(1)), Some(1)),
                    (key(&sop(UID)), None),
                ],
                "{context}"
            );
            let first = table.status(0).expect("registered");
            assert_eq!((first.failure, first.settled), (Some(failure), false));
            assert!(table.settled(1) && table.settled(2) && !table.settled(3));
            assert_eq!(
                table.file_for_key(&FileKey::sop(UID).expect("key")),
                Some(0),
                "{context}: the replaced key still names its file"
            );

            // The first file is read after all: it is keyed like the rest,
            // and no key that was given changes.
            let changes = table.resolve(0, Ok(digest(1)));
            assert_eq!(changes.updated, vec![0], "{context}");
            assert!(changes.rekeys.is_empty(), "{context}");
            assert_eq!(keys(&table)[0], (key(&b3(1)), Some(1)), "{context}");
            assert_eq!(table.status(0).expect("registered").failure, None);
            let changes = table.resolve(3, Ok(digest(2)));
            assert_eq!(changes.rekeys.len(), 1, "{context}");
            assert_eq!(keys(&table)[3], (key(&b3(2)), None), "{context}");
        }

        // The first file of a group that is relied on, read only later for
        // a late file's comparison, cannot be read: the late file has no
        // key and the first file keeps the one it was given.
        let mut table = Table::new();
        table.register(file(UID, 10, "/a"));
        let (reliance, _, _) = rely(&mut table, 0, &|_| unreadable);
        assert_eq!(reliance, Reliance::Final(FileKey::sop(UID).expect("key")));
        table.register(file(UID, 10, "/late"));
        let (reliance, hashed, _) = rely(&mut table, 1, &|index| {
            if index == 0 {
                unreadable
            } else {
                Ok(digest(1))
            }
        });
        assert_eq!(reliance, Reliance::Failed(KeyFailure::Unreadable));
        assert_eq!(hashed, vec![0, 1]);
        assert_eq!(keys(&table), vec![(key(&sop(UID)), None), (None, None)]);
        assert!(table.settled(0));
        assert_eq!(table.required_for(1), vec![0]);
        // The first file is read after all: the late file is compared.
        let changes = table.resolve(0, Ok(digest(1)));
        assert_eq!(
            changes.updated,
            vec![0, 1],
            "the failure clears, the late file gains its key"
        );
        assert_eq!(
            keys(&table),
            vec![(key(&sop(UID)), None), (key(&sop(UID)), Some(0))]
        );
    }

    #[test]
    fn one_file_under_two_paths_is_hashed_once() {
        let mut table = Table::new();
        table.register(file("", 10, "/data/image.png"));
        table.register(file("", 10, "/data/image.png"));
        table.register(file(UID, 10, "/data/a.dcm"));
        table.register(file(UID, 10, "/data/a.dcm"));

        // The DICOM pair is one file: nothing to compare, and its key is
        // relied on without a read.
        assert_eq!(table.required_for(3), Vec::<usize>::new());
        assert_eq!(
            table.rely_on(3).0,
            Some(Reliance::Final(FileKey::sop(UID).expect("key")))
        );
        assert!(table.status(3).expect("registered").settled);
        assert!(table.status(2).expect("registered").settled);

        // Hashing either entry of the raster keys both.
        assert_eq!(table.required_for(1), vec![1]);
        let changes = table.resolve(1, Ok(digest(4)));
        assert_eq!(changes.updated, vec![0, 1]);
        assert_eq!(
            keys(&table)[..2],
            [(key(&b3(4)), None), (key(&b3(4)), Some(0))]
        );
        assert_eq!(table.required_for(0), Vec::<usize>::new());

        // An entry of the same file registered afterwards has the key at
        // once.
        table.register(file("", 10, "/data/image.png"));
        assert_eq!(keys(&table)[4], (key(&b3(4)), Some(0)));
    }

    #[test]
    fn a_failed_digest_leaves_no_wrong_key_and_can_be_retried() {
        let mut table = Table::new();
        table.register(file("", 10, "/image.png"));
        table.register(file(UID, 10, "/a"));
        table.register(file(UID, 11, "/b"));
        table.register(file("1.2.9", 10, "/c"));
        table.register(file("1.2.9", 10, "/c-copy"));

        // No UID: still no key, with the failure recorded.
        let changes = table.resolve(0, Err(KeyFailure::Unreadable));
        assert_eq!(changes.updated, vec![0]);
        let status = table.status(0).expect("registered");
        assert_eq!(
            (status.key, status.failure),
            (None, Some(KeyFailure::Unreadable))
        );
        assert_eq!(
            table.rely_on(0).0,
            Some(Reliance::Failed(KeyFailure::Unreadable))
        );
        // A failed file is not queued again by being viewed, but asking for
        // its key names it, and a digest then clears the failure.
        assert!(table.frame_sent(0).wanted.is_empty());
        assert_eq!(table.required_for(0), vec![0]);
        table.resolve(0, Ok(digest(3)));
        let status = table.status(0).expect("registered");
        assert_eq!(
            (
                status.key.map(|key| key.as_str().to_string()),
                status.failure
            ),
            (key(&b3(3)), None)
        );

        // A file that kept the `sop:` key of a split group loses it: the key
        // is known not to identify it, and no digest replaces it.
        let changes = table.resolve(1, Err(KeyFailure::Changed));
        assert_eq!(changes.updated, vec![1]);
        assert!(changes.rekeys.is_empty(), "losing a key is not a rekey");
        let status = table.status(1).expect("registered");
        assert_eq!(
            (status.key, status.failure, status.settled),
            (None, Some(KeyFailure::Changed), false)
        );

        // An alias that cannot be read keeps the key its UID gives it while
        // nobody relies on it, with the failure recorded, and is not
        // settled.
        let changes = table.resolve(4, Err(KeyFailure::Unreadable));
        assert_eq!(changes.updated, vec![4]);
        let status = table.status(4).expect("registered");
        assert_eq!(
            (status.key, status.failure, status.settled),
            (
                FileKey::sop("1.2.9").ok(),
                Some(KeyFailure::Unreadable),
                false
            )
        );
        assert_eq!(table.required_for(4), vec![3, 4]);
    }

    /// The cost rule, counted in entries reported rather than in time: one
    /// UID on 20,000 files of one size, then a file that splits the group,
    /// then every digest. A table that walked the group on each call would
    /// report (and do) hundreds of millions of visits.
    #[test]
    fn a_group_of_any_size_costs_one_visit_per_file_to_split() {
        const FILES: usize = 20_000;
        let mut table = Table::new();
        let mut reported = 0;
        for index in 0..FILES {
            let changes = table.register(file(UID, 10, &format!("/copies/{index}")));
            assert_eq!(changes.updated, vec![index]);
        }
        let split = table.register(file(UID, 11, "/other"));
        // Every alias loses its `alias_of`; the first file shows what it
        // showed.
        assert_eq!(split.updated.len(), FILES);
        for index in 0..FILES {
            let changes = table.resolve(index, Ok(digest((index % 3) as u8)));
            assert!(
                changes.updated.len() <= 1 && changes.rekeys.len() <= 1,
                "file {index} changed {} entries",
                changes.updated.len()
            );
            reported += changes.updated.len();
        }
        assert_eq!(reported, FILES);
        assert_eq!(table.status(5).expect("registered").alias_of, Some(2));
        assert_eq!(table.status(2).expect("registered").alias_of, None);
    }
}

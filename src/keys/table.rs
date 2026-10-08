//! Which key each loaded file has.

use super::FileKey;
use std::path::PathBuf;

/// What discovery knows about a file that decides its key. None of it needs
/// a read beyond the header discovery already parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIdentity {
    /// The SOP Instance UID exactly as discovery read it; empty for a raster
    /// and for a DICOM file without one.
    pub sop_instance_uid: String,
    /// The file's length in bytes at discovery.
    pub size_bytes: u64,
    /// The path that names the file itself, as discovery resolved it (the
    /// canonical path). Two entries with equal paths are one file reached
    /// twice. The table compares this value and never touches the
    /// filesystem.
    pub path: PathBuf,
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
    /// ascending index order. A file is listed in `wanted` at most once in
    /// the table's life.
    pub wanted: Vec<usize>,
}

/// The part of a file's key state its catalog entry shows, borrowed from
/// the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyView<'a> {
    pub key: Option<&'a FileKey>,
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
    /// Whether [`KeyTable::required_for`] is empty for the file and it has a
    /// key: nothing more can be learned about this key without hashing a
    /// file nobody asked about.
    pub settled: bool,
}

/// The keys of the registered files (`docs/design/annotation-model.md` 1.3,
/// 1.4, 1.7). Files are numbered in registration order from 0, which is the
/// catalog's file index.
///
/// # Terms
///
/// - A UID is *usable* when `FileKey::sop` accepts it: 1 to 128 characters,
///   each an ASCII letter, a digit, `.`, `-` or `_`, taken as written.
/// - A file's *group* is the set of registered files with its usable UID.
/// - Two entries are the *same file* when their [`FileIdentity::path`]s are
///   equal. A digest or a failure recorded for one is recorded for every
///   entry of that file, including entries registered later.
/// - A group is *split* once two of its files are known to hold different
///   bytes: their sizes differ, or their digests are both known and differ.
///   A split group stays split.
///
/// # The key a file has
///
/// 1. No usable UID (a raster, an empty UID, a UID with another character or
///    more than 128 of them): `b3:<digest>` once its digest is known, and no
///    key before that.
/// 2. A usable UID whose group is not split: `sop:<uid>`. Every file of the
///    group has this key; whether their bytes are equal has not been
///    checked unless both digests happen to be known.
/// 3. A usable UID whose group is split: `b3:<digest>` once its digest is
///    known. Before that, a file that had `sop:<uid>` when the group split
///    keeps it, so that its key is replaced once, old by new, and never
///    goes missing in between; a file registered into a group that is
///    already split has no key. A file of a split group whose digest attempt
///    failed has no key.
///
/// The rule does not depend on registration order: a group has `sop:` keys
/// exactly while nothing shows its files to differ, and once something does,
/// every file of it is keyed by content, so byte-identical copies share one
/// `b3:` key and a differing copy gets its own.
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
/// failure is recorded for it, and its key falls under rule 1 or rule 3.
/// A file under rule 2 is never wanted: aliases that share a UID and a size
/// are compared only when someone asks ([`KeyTable::required_for`]), so a
/// dataset with a copy of every file is never read twice just for being
/// opened.
///
/// # Cost
///
/// Every call is a constant number of hash-map operations, plus one visit to
/// each entry of the same file, except the one call that splits a group,
/// which visits each file of the group once. A table of `n` files therefore
/// costs `O(n)` over its whole life however the files are grouped.
pub struct KeyTable {
    _private: (),
}

impl KeyTable {
    /// An empty table.
    pub fn new() -> Self {
        todo!("FND4: an empty key table")
    }

    /// The number of registered files.
    pub fn len(&self) -> usize {
        todo!("FND4: the number of registered files")
    }

    /// Whether no file is registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Registers the next file, whose index is [`KeyTable::len`] before the
    /// call, and gives it the key the rules on [`KeyTable`] say.
    ///
    /// Registering a file whose size differs from its group's splits the
    /// group; so does registering another entry of a file whose known digest
    /// differs from the group's. The files the split changes are in
    /// `updated`, those of them whose digest is already known are rekeyed in
    /// this call, and those that are served and still without a digest are
    /// in `wanted`.
    pub fn register(&mut self, identity: FileIdentity) -> KeyChanges {
        let _ = identity;
        todo!("FND4: register a file and assign its key")
    }

    /// Records that a frame of the file has been served. Only `wanted` of
    /// the result can be non-empty. An index that is not registered changes
    /// nothing.
    pub fn frame_sent(&mut self, index: usize) -> KeyChanges {
        let _ = index;
        todo!("FND4: mark the file served and report a wanted digest")
    }

    /// Whether [`KeyTable::frame_sent`] has been called for the file.
    pub fn served(&self, index: usize) -> bool {
        let _ = index;
        todo!("FND4: whether a frame of the file has been served")
    }

    /// The files whose digests must be computed before the key of `index`
    /// can be relied on for a write or an export: at most two, in ascending
    /// index order, never two entries of the same file.
    ///
    /// - The file itself, when its digest is unknown and it has no key, or
    ///   keeps the `sop:` key of a split group, or has an `alias_of` that is
    ///   another file.
    /// - Its `alias_of` file, when that is another file whose digest is
    ///   unknown.
    ///
    /// A recorded failure does not exempt a file: asking again is how a
    /// failed digest is retried. Empty when nothing is needed, and for an
    /// index that is not registered. The first file of a group that is not
    /// split needs nothing: its `sop:` key is its own until a copy is shown
    /// to differ.
    pub fn required_for(&self, index: usize) -> Vec<usize> {
        let _ = index;
        todo!("FND4: the digests a settled key still needs")
    }

    /// Records the outcome of hashing the file, for every entry of it.
    ///
    /// A digest clears a recorded failure. Under rule 1 the file gains its
    /// `b3:` key. Under rule 2 the digest is compared with the group's other
    /// known digests: a difference splits the group, and otherwise the key
    /// stays `sop:<uid>`. Under rule 3 the file gains its `b3:` key, which
    /// is a rekey when it still had `sop:<uid>`.
    ///
    /// A failure is recorded and, under rule 3, removes the `sop:` key the
    /// file kept; under rules 1 and 2 the key, or its absence, stays.
    ///
    /// An outcome for an index that is not registered changes nothing. A
    /// digest for a file that already has one replaces nothing: the first
    /// digest stands.
    pub fn resolve(&mut self, index: usize, outcome: Result<[u8; 32], KeyFailure>) -> KeyChanges {
        let _ = (index, outcome);
        todo!("FND4: record a digest or a failure and apply its consequences")
    }

    /// What a catalog entry shows of the file's key, borrowed, or `None` for
    /// an index that is not registered. The catalog reads this for every
    /// file it registers and every frame it serves, so it allocates nothing.
    pub fn view(&self, index: usize) -> Option<KeyView<'_>> {
        let _ = index;
        todo!("FND4: one file's key, alias and failure, borrowed")
    }

    /// The file's key state, or `None` for an index that is not registered.
    pub fn status(&self, index: usize) -> Option<FileKeyStatus> {
        let _ = index;
        todo!("FND4: one file's key state")
    }

    /// The file a key names. A `sop:` key names the first file registered
    /// with that UID, and keeps naming it after a split has replaced the key
    /// (`docs/design/annotation-model.md` 1.7: "the server keeps an
    /// old-to-new map"), so an operation sent under the old key still finds
    /// its file. A `b3:` key names the first file that came to hold it.
    /// `None` for a key no file has held.
    pub fn file_for_key(&self, key: &FileKey) -> Option<usize> {
        let _ = key;
        todo!("FND4: the file a current or replaced key names")
    }
}

impl Default for KeyTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{FileIdentity, KeyFailure, KeyTable};
    use crate::keys::FileKey;
    use std::path::PathBuf;

    const UID: &str = "1.2.826.0.1.3680043.10.511.7";

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
    fn keys(table: &KeyTable) -> Vec<(Option<String>, Option<usize>)> {
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

    /// The  each file of a case is expected to have.
    type Expected = Vec<(Option<String>, Option<usize>)>;

    #[test]
    fn a_file_gets_the_key_its_uid_and_the_other_files_allow() {
        // One table per case: the files registered in order, then the key
        // and alias of each before any file is hashed.
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
            let mut table = KeyTable::new();
            for (index, identity) in files.into_iter().enumerate() {
                let changes = table.register(identity);
                assert!(changes.updated.contains(&index), "{name}: file {index}");
                assert!(changes.wanted.is_empty(), "{name}: nothing is served yet");
            }
            assert_eq!(keys(&table), expected, "{name}");
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
            let mut table = KeyTable::new();
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
        let mut table = KeyTable::new();
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
        let mut table = KeyTable::new();
        table.register(file(UID, 10, "/a"));
        table.register(file(UID, 10, "/copy"));
        table.frame_sent(0);
        assert!(
            table.frame_sent(1).wanted.is_empty(),
            "aliases are not hashed for being viewed"
        );
        assert!(table.status(0).expect("registered").settled);
        assert!(
            !table.status(1).expect("registered").settled,
            "an alias by UID and size is unchecked"
        );

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

    #[test]
    fn aliases_are_compared_only_when_asked_and_two_files_at_a_time() {
        let mut table = KeyTable::new();
        for path in ["/all/a", "/train/a", "/test/a", "/val/a"] {
            table.register(file(UID, 10, path));
        }
        // The first file's key is its own; an alias needs itself and the
        // file it is an alias of, and no other member of the group.
        assert_eq!(table.required_for(0), Vec::<usize>::new());
        assert_eq!(table.required_for(2), vec![0, 2]);
        assert_eq!(table.required_for(9), Vec::<usize>::new());

        // Equal bytes: still one `sop:` key, now checked for this pair.
        assert_eq!(table.resolve(0, Ok(digest(1))), Default::default());
        assert_eq!(table.required_for(2), vec![2]);
        assert_eq!(table.resolve(2, Ok(digest(1))), Default::default());
        assert_eq!(table.required_for(2), Vec::<usize>::new());
        assert!(table.status(2).expect("registered").settled);
        assert_eq!(
            table.status(2).expect("registered").key,
            FileKey::sop(UID).ok()
        );

        // Another alias turns out to differ: the group splits, the two files
        // with known digests are rekeyed at once, and the last one keeps
        // `sop:` until it is hashed.
        let changes = table.resolve(1, Ok(digest(9)));
        assert_eq!(changes.updated, vec![0, 1, 2, 3]);
        assert_eq!(
            changes
                .rekeys
                .iter()
                .map(|rekey| (rekey.index, rekey.new_key.as_str().to_string()))
                .collect::<Vec<_>>(),
            vec![(0, b3(1)), (1, b3(9)), (2, b3(1))]
        );
        assert_eq!(
            keys(&table),
            vec![
                (key(&b3(1)), None),
                (key(&b3(9)), None),
                (key(&b3(1)), Some(0)),
                (key(&sop(UID)), None)
            ]
        );
        assert_eq!(table.required_for(3), vec![3]);
    }

    #[test]
    fn one_file_under_two_paths_is_hashed_once() {
        let mut table = KeyTable::new();
        table.register(file("", 10, "/data/image.png"));
        table.register(file("", 10, "/data/image.png"));
        table.register(file(UID, 10, "/data/a.dcm"));
        table.register(file(UID, 10, "/data/a.dcm"));

        // The DICOM pair is one file: nothing to compare.
        assert_eq!(table.required_for(3), Vec::<usize>::new());
        assert!(table.status(3).expect("registered").settled);

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
        let mut table = KeyTable::new();
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

        // An alias that cannot be read keeps the key its UID gives it, with
        // the failure recorded, and is not settled.
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
        let mut table = KeyTable::new();
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

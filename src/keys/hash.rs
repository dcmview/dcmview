//! The BLAKE3 digest of one file's bytes, read in bounded slices.

use super::KeyFailure;
use std::path::Path;

/// The most bytes one [`FileHasher::next_slice`] call reads. A slice is the
/// unit of background work: the catalog takes a background decode permit for
/// each one and gives it back before the next, so a viewer's decode never
/// waits behind more than one slice of hashing.
pub const KEY_HASH_SLICE_BYTES: u64 = 8 * 1024 * 1024;

/// What one slice left to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashProgress {
    /// More of the file remains; call [`FileHasher::next_slice`] again.
    More,
    /// The digest of the whole file, exactly as stored.
    Done([u8; 32]),
}

/// Hashes one file whose length discovery already knows.
///
/// The digest is only ever of the file discovery saw. A file whose length
/// differs from `expected_len`, that ends early, that has grown, or whose
/// length or modification time changes between [`FileHasher::open`] and the
/// last slice is [`KeyFailure::Changed`]; no digest of mixed or partial
/// bytes is returned.
///
/// The work is bounded by what discovery saw, not by what the file has
/// become: across all calls at most `expected_len + 1` bytes are read (the
/// one extra byte is the probe that detects growth), and each call reads at
/// most [`KEY_HASH_SLICE_BYTES`].
pub struct FileHasher {
    _private: (),
}

impl FileHasher {
    /// Opens `path` for hashing and reads nothing.
    ///
    /// The path is checked with `std::fs::metadata` first, and anything that
    /// is not a regular file is [`KeyFailure::Unreadable`] without being
    /// opened, so a FIFO put in a file's place never blocks the caller. A
    /// path that cannot be inspected or opened is `Unreadable`. A regular
    /// file whose length is not `expected_len` is [`KeyFailure::Changed`].
    pub fn open(path: &Path, expected_len: u64) -> Result<Self, KeyFailure> {
        let _ = (path, expected_len);
        todo!("FND4: check the path is a regular file of the expected length and open it")
    }

    /// Reads and hashes the next slice.
    ///
    /// Returns [`HashProgress::More`] while bytes of the expected length
    /// remain. The call that reaches the expected length also tries to read
    /// one more byte and inspects the open file again: when no byte follows
    /// and the file's length and modification time are what they were at
    /// [`FileHasher::open`], it returns [`HashProgress::Done`]; otherwise
    /// [`KeyFailure::Changed`]. An end of file before the expected length is
    /// `Changed`. A read error other than an interruption is
    /// [`KeyFailure::Unreadable`]. An empty file is `Done` on the first
    /// call. A platform without modification times compares lengths only.
    ///
    /// Not called again after `Done` or an error.
    pub fn next_slice(&mut self) -> Result<HashProgress, KeyFailure> {
        todo!("FND4: hash up to one slice and finish with the growth probe and the second stat")
    }

    /// The bytes read from the file so far, the growth probe included.
    pub fn bytes_read(&self) -> u64 {
        todo!("FND4: the count of bytes read")
    }
}

#[cfg(test)]
mod tests {
    use super::{FileHasher, HashProgress, KEY_HASH_SLICE_BYTES};
    use crate::keys::KeyFailure;
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    /// Bytes that differ along the file, so a digest of the wrong range or
    /// of slices in the wrong order is a different digest.
    fn patterned(len: usize) -> Vec<u8> {
        (0..len)
            .map(|position| (position % 251) as u8 ^ (position / 65_536) as u8)
            .collect()
    }

    /// Hashes to the end, returning the outcome and how many slices it took.
    fn finish(mut hasher: FileHasher) -> (Result<[u8; 32], KeyFailure>, u64, u64) {
        let mut slices = 0;
        loop {
            slices += 1;
            match hasher.next_slice() {
                Ok(HashProgress::More) => {
                    assert!(
                        slices <= 64,
                        "a file of a few slices never needs this many calls"
                    );
                }
                Ok(HashProgress::Done(digest)) => return (Ok(digest), slices, hasher.bytes_read()),
                Err(failure) => return (Err(failure), slices, hasher.bytes_read()),
            }
        }
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("write fixture");
    }

    #[test]
    fn the_digest_is_blake3_of_the_bytes_in_slices_of_at_most_eight_mebibytes() {
        assert_eq!(KEY_HASH_SLICE_BYTES, 8 * 1024 * 1024);
        let dir = tempfile::tempdir().expect("temp dir");
        let slice = KEY_HASH_SLICE_BYTES as usize;
        // (length, slices): empty, small, exactly one slice, and two and a
        // half slices.
        for (len, expected_slices) in [
            (0, 1),
            (1, 1),
            (70_000, 1),
            (slice, 1),
            (slice + 1, 2),
            (2 * slice + slice / 2, 3),
        ] {
            let path = dir.path().join(format!("file-{len}.bin"));
            let bytes = patterned(len);
            write(&path, &bytes);
            let hasher = FileHasher::open(&path, len as u64).expect("open");
            assert_eq!(hasher.bytes_read(), 0, "opening reads nothing");
            let (digest, slices, read) = finish(hasher);
            assert_eq!(
                digest,
                Ok(*blake3::hash(&bytes).as_bytes()),
                "digest of {len} bytes"
            );
            assert_eq!(slices, expected_slices, "slices for {len} bytes");
            assert_eq!(read, len as u64, "bytes read for {len} bytes");
        }
    }

    #[test]
    fn a_file_that_is_not_the_one_discovery_saw_has_no_digest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let original = patterned(200_000);

        // Longer or shorter than discovery saw: refused before any read.
        for (name, expected_len) in [("longer", 199_999_u64), ("shorter", 200_001)] {
            let path = dir.path().join(name);
            write(&path, &original);
            assert_eq!(
                FileHasher::open(&path, expected_len).err(),
                Some(KeyFailure::Changed),
                "{name}"
            );
        }

        // Grown after it was opened: at most one byte past the expected
        // length is read, however much was appended.
        let grown = dir.path().join("grown");
        write(&grown, &original);
        let hasher = FileHasher::open(&grown, 200_000).expect("open");
        fs::OpenOptions::new()
            .append(true)
            .open(&grown)
            .expect("reopen")
            .write_all(&patterned(3 * KEY_HASH_SLICE_BYTES as usize))
            .expect("append");
        let (outcome, _, read) = finish(hasher);
        assert_eq!(outcome, Err(KeyFailure::Changed));
        assert!(read <= 200_001, "read {read} bytes of a 200,000-byte file");

        // Cut short after it was opened.
        let cut = dir.path().join("cut");
        write(&cut, &original);
        let hasher = FileHasher::open(&cut, 200_000).expect("open");
        fs::OpenOptions::new()
            .write(true)
            .open(&cut)
            .expect("reopen")
            .set_len(1_000)
            .expect("truncate");
        let (outcome, _, read) = finish(hasher);
        assert_eq!(outcome, Err(KeyFailure::Changed));
        assert!(read <= 1_000);

        // Gone, or not a file at all.
        assert_eq!(
            FileHasher::open(&dir.path().join("missing"), 10).err(),
            Some(KeyFailure::Unreadable)
        );
        assert_eq!(
            FileHasher::open(dir.path(), 10).err(),
            Some(KeyFailure::Unreadable)
        );
    }

    /// A FIFO where a file was: opening one for reading blocks until a
    /// writer appears, so it must be refused from its metadata.
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_place_of_a_file_is_refused_without_being_opened() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("fifo");
        let name = std::ffi::CString::new(path.to_str().expect("utf-8 path")).expect("c string");
        // SAFETY: `name` is a valid NUL-terminated path for the whole call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0, "mkfifo");
        assert_eq!(
            FileHasher::open(&path, 0).err(),
            Some(KeyFailure::Unreadable)
        );
    }
}

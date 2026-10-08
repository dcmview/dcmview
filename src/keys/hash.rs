//! The BLAKE3 digest of one file's bytes, read in bounded slices.

use super::KeyFailure;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::time::SystemTime;

/// The most of a file's expected bytes one [`FileHasher::next_slice`] call
/// reads; the last call reads one probe byte more. A slice is the
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

/// Hashes one file as discovery saw it.
///
/// Discovery's `stat` gives a length and a modification time, and those two
/// are all that says a file is still the one discovery saw: its bytes were
/// not read then. A file whose length differs from `expected_len`, whose
/// modification time differs from `expected_modified`, that ends early,
/// that has grown, or whose length or modification time changes between
/// [`FileHasher::open`] and the last slice is [`KeyFailure::Changed`]; no
/// digest of mixed or partial bytes is returned.
///
/// What this cannot see: a file rewritten with other bytes of the same
/// length whose modification time was then set back to what discovery saw
/// (or rewritten within one tick of the filesystem's clock) hashes as those
/// other bytes, with no failure. The digest is then of what the path holds
/// now, which is also what a frame decoded now shows.
///
/// The work is bounded by what discovery saw, not by what the file has
/// become: across all calls at most `expected_len + 1` bytes are read (the
/// one extra byte is the probe that detects growth), and each call reads at
/// most [`KEY_HASH_SLICE_BYTES`] of the file's expected bytes. The call that
/// reaches the expected length reads the probe byte as well, so that one
/// call may read `KEY_HASH_SLICE_BYTES + 1` bytes.
pub struct FileHasher {
    file: File,
    expected_len: u64,
    modified: Option<SystemTime>,
    hasher: blake3::Hasher,
    buffer: Box<[u8]>,
    bytes_read: u64,
}

impl FileHasher {
    /// Opens `path` for hashing and reads nothing.
    ///
    /// The path is named once, to open it, and everything is checked on the
    /// file that call opened, so what is checked is what is read:
    ///
    /// - On Unix the file is opened with `O_NONBLOCK`, so opening never
    ///   waits: a FIFO put in a file's place, at any moment, neither blocks
    ///   the caller nor is read. On other platforms it is opened as usual.
    /// - Anything that is not a regular file is [`KeyFailure::Unreadable`],
    ///   as is a path that cannot be opened or inspected.
    /// - A regular file whose length is not `expected_len` is
    ///   [`KeyFailure::Changed`]. So is one whose modification time is not
    ///   `expected_modified`, when both are known. With `None`, or on a
    ///   platform without modification times, only lengths are compared.
    ///
    /// `expected_len` and `expected_modified` are the two values of one
    /// `stat`, the one discovery took (`FileEntry::size_bytes` and
    /// `FileEntry::modified`).
    pub fn open(
        path: &Path,
        expected_len: u64,
        expected_modified: Option<SystemTime>,
    ) -> Result<Self, KeyFailure> {
        Self::open_probed(path, expected_len, expected_modified, || {})
    }

    /// [`FileHasher::open`], calling `before_open` once, after everything
    /// this function does with the path by name other than opening it and
    /// immediately before the call that opens it. The seam a test uses to
    /// change what the path names at the last moment; `open` passes a
    /// closure that does nothing.
    pub(super) fn open_probed(
        path: &Path,
        expected_len: u64,
        expected_modified: Option<SystemTime>,
        before_open: impl FnOnce(),
    ) -> Result<Self, KeyFailure> {
        before_open();
        #[cfg(unix)]
        let file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)
        };
        #[cfg(not(unix))]
        let file = File::open(path);
        let file = file.map_err(|_| KeyFailure::Unreadable)?;
        let opened = file.metadata().map_err(|_| KeyFailure::Unreadable)?;
        if !opened.is_file() {
            return Err(KeyFailure::Unreadable);
        }
        let modified = opened.modified().ok();
        if opened.len() != expected_len
            || matches!((modified, expected_modified), (Some(actual), Some(expected)) if actual != expected)
        {
            return Err(KeyFailure::Changed);
        }
        Ok(Self {
            file,
            expected_len,
            modified,
            hasher: blake3::Hasher::new(),
            buffer: vec![0; 1024 * 1024].into_boxed_slice(),
            bytes_read: 0,
        })
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
        let end = self.bytes_read + (self.expected_len - self.bytes_read).min(KEY_HASH_SLICE_BYTES);
        while self.bytes_read < end {
            let count = (end - self.bytes_read).min(self.buffer.len() as u64) as usize;
            let read = self.read(count)?;
            if read == 0 {
                return Err(KeyFailure::Changed);
            }
            self.hasher.update(&self.buffer[..read]);
        }
        if self.bytes_read < self.expected_len {
            return Ok(HashProgress::More);
        }
        if self.read(1)? != 0 {
            return Err(KeyFailure::Changed);
        }
        let metadata = self.file.metadata().map_err(|_| KeyFailure::Unreadable)?;
        if metadata.len() != self.expected_len || metadata.modified().ok() != self.modified {
            return Err(KeyFailure::Changed);
        }
        Ok(HashProgress::Done(*self.hasher.finalize().as_bytes()))
    }

    /// The bytes read from the file so far, the growth probe included.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }
}

impl FileHasher {
    fn read(&mut self, count: usize) -> Result<usize, KeyFailure> {
        loop {
            match self.file.read(&mut self.buffer[..count]) {
                Ok(read) => {
                    self.bytes_read += read as u64;
                    return Ok(read);
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(KeyFailure::Unreadable),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FileHasher, HashProgress, KEY_HASH_SLICE_BYTES};
    use crate::keys::KeyFailure;
    use std::fs;
    use std::io::Write;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

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
            let hasher = FileHasher::open(&path, len as u64, None).expect("open");
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
                FileHasher::open(&path, expected_len, None).err(),
                Some(KeyFailure::Changed),
                "{name}"
            );
        }

        // Grown after it was opened: at most one byte past the expected
        // length is read, however much was appended.
        let grown = dir.path().join("grown");
        write(&grown, &original);
        let hasher = FileHasher::open(&grown, 200_000, None).expect("open");
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
        let hasher = FileHasher::open(&cut, 200_000, None).expect("open");
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
            FileHasher::open(&dir.path().join("missing"), 10, None).err(),
            Some(KeyFailure::Unreadable)
        );
        assert_eq!(
            FileHasher::open(dir.path(), 10, None).err(),
            Some(KeyFailure::Unreadable)
        );
    }

    fn modified(path: &Path) -> SystemTime {
        fs::metadata(path)
            .expect("file metadata")
            .modified()
            .expect("modification time")
    }

    fn set_modified(path: &Path, time: SystemTime) {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("reopen")
            .set_modified(time)
            .expect("set modification time");
    }

    /// Discovery reads a file's header and its `stat`, not its bytes, so a
    /// file replaced by other bytes of the same length is told from the one
    /// discovery saw by its modification time alone.
    #[test]
    fn a_file_rewritten_at_the_same_length_is_told_by_its_modification_time() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("file");
        let original = patterned(70_000);
        let other = original.iter().map(|byte| byte ^ 0x5a).collect::<Vec<_>>();
        write(&path, &original);
        let seen = modified(&path);
        let digest_with = |expected_modified| {
            FileHasher::open(&path, 70_000, expected_modified).map(|hasher| finish(hasher).0)
        };

        assert_eq!(
            digest_with(Some(seen)).expect("open"),
            Ok(*blake3::hash(&original).as_bytes()),
            "the file discovery saw"
        );

        // Rewritten, and the clock moved either way. The time is set
        // outright so the test does not depend on the filesystem's tick.
        write(&path, &other);
        for (name, time) in [
            ("later", seen + Duration::from_secs(10)),
            ("earlier", seen - Duration::from_secs(10)),
        ] {
            set_modified(&path, time);
            assert_eq!(
                digest_with(Some(seen)).err(),
                Some(KeyFailure::Changed),
                "rewritten, modified {name}: refused before any read"
            );
        }

        // Without a time from discovery only the length is compared.
        assert_eq!(
            digest_with(None).expect("open"),
            Ok(*blake3::hash(&other).as_bytes())
        );

        // The stated limit: other bytes of the same length under the
        // modification time discovery saw are hashed as the file.
        set_modified(&path, seen);
        assert_eq!(
            digest_with(Some(seen)).expect("open"),
            Ok(*blake3::hash(&other).as_bytes())
        );
    }

    /// A file whose modification time moves while it is hashed was written
    /// to, whatever its length says, and gets no digest.
    #[test]
    fn a_file_modified_while_it_is_hashed_has_no_digest() {
        let dir = tempfile::tempdir().expect("temp dir");
        let slice = KEY_HASH_SLICE_BYTES as usize;
        // One slice, touched before it is read; two slices, touched between.
        for (len, slices_before) in [(70_000, 0), (slice + 70_000, 1)] {
            let path = dir.path().join(format!("touched-{len}"));
            write(&path, &patterned(len));
            let seen = modified(&path);
            let mut hasher = FileHasher::open(&path, len as u64, Some(seen)).expect("open");
            for _ in 0..slices_before {
                assert_eq!(hasher.next_slice(), Ok(HashProgress::More));
            }
            set_modified(&path, seen + Duration::from_secs(10));
            let (outcome, _, read) = finish(hasher);
            assert_eq!(outcome, Err(KeyFailure::Changed), "{len} bytes");
            assert!(read <= len as u64, "{len} bytes: read {read}");
        }
    }

    #[cfg(unix)]
    fn make_fifo(path: &Path) {
        let name = std::ffi::CString::new(path.to_str().expect("utf-8 path")).expect("c string");
        // SAFETY: `name` is a valid NUL-terminated path for the whole call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0, "mkfifo");
    }

    /// Opening a FIFO for reading waits until a writer appears, and nothing
    /// here ever writes: a hasher that waited would hold a thread of the
    /// blocking pool, and the one hashing worker with it, for good. The
    /// FIFO is there from the start, or takes the file's place at the last
    /// moment before the path is opened, after anything that looked at the
    /// path by name saw a regular file.
    ///
    /// The open runs on its own thread so that a hasher that waits fails
    /// this test instead of hanging it. The bound is not a measurement: a
    /// correct open returns at once, and a waiting one never does.
    #[cfg(unix)]
    #[test]
    fn a_fifo_in_place_of_a_file_never_blocks_whenever_it_appears() {
        for swapped_in_late in [false, true] {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join("file");
            if swapped_in_late {
                write(&path, &patterned(1_000));
            } else {
                make_fifo(&path);
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn({
                let path = path.clone();
                move || {
                    let opened = FileHasher::open_probed(&path, 1_000, None, || {
                        if swapped_in_late {
                            fs::remove_file(&path).expect("remove the file");
                            make_fifo(&path);
                        }
                    });
                    // The receiver may have given up.
                    let _ = sender.send(opened.err());
                }
            });
            match receiver.recv_timeout(Duration::from_secs(20)) {
                Ok(outcome) => assert_eq!(
                    outcome,
                    Some(KeyFailure::Unreadable),
                    "swapped in late: {swapped_in_late}"
                ),
                Err(_) => panic!("opening waited on the FIFO (swapped in late: {swapped_in_late})"),
            }
        }
    }
}

//! Private socket paths and same-account connection admission.

use anyhow::{bail, Context, Result};
use axum::serve::Listener;
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions, Permissions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::{unix::SocketAddr, UnixListener, UnixStream};

/// Owns the listener and the filesystem entry created by this process.
pub struct UnixSocket {
    listener: UnixListener,
    file: OwnedFile,
    // Drop the listener and socket entry before removing/releasing the lock.
    _lock: Box<SocketLock>,
    uid: libc::uid_t,
}

impl UnixSocket {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path).context("failed to resolve Unix socket path")?;
        let parent = path
            .parent()
            .context("Unix socket path needs a parent directory")?;
        // Checked before the parent is touched, so a path without a file
        // name never leaves a directory behind.
        let file_name = path
            .file_name()
            .context("Unix socket path needs a file name")?;
        // SAFETY: geteuid takes no arguments and has no safety preconditions.
        let uid = unsafe { libc::geteuid() };
        let path = prepare_parent(parent, uid)?.join(file_name);
        let lock = SocketLock::acquire(&path)?;

        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    bail!(
                        "Unix socket path {} exists and is not a socket",
                        path.display()
                    );
                }
                // The lock protects live viewers even when connect refuses
                // a full accept queue. This probe only adds a veto for live
                // listeners that do not participate in our locking protocol.
                match UnixStream::connect(&path).await {
                    Ok(_) => bail!("Unix socket {} is already in use", path.display()),
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        // Only unlink the socket we inspected, never a replacement.
                        if !same_file(&path, &metadata) {
                            bail!("Unix socket {} changed while checking it", path.display());
                        }
                        fs::remove_file(&path).with_context(|| {
                            format!("failed to remove stale Unix socket {}", path.display())
                        })?;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("failed to check existing Unix socket {}", path.display())
                        });
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to inspect Unix socket path"),
        }

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("failed to bind Unix socket {}", path.display()))?;
        let file = OwnedFile {
            metadata: fs::symlink_metadata(&path)
                .context("failed to inspect newly bound Unix socket")?,
            path,
        };
        // The peer check also protects the bind-to-chmod interval; do not
        // change the process-wide umask while discovery threads may run.
        fs::set_permissions(&file.path, Permissions::from_mode(0o600))
            .with_context(|| format!("failed to restrict Unix socket {}", file.path.display()))?;
        Ok(Self {
            listener,
            file,
            _lock: Box::new(lock),
            uid,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.file.path
    }
}

impl Listener for UnixSocket {
    type Io = UnixStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            // Reuse Axum's retry/backoff handling for listener errors.
            let (stream, address) = Listener::accept(&mut self.listener).await;
            if stream.peer_cred().is_ok_and(|peer| peer.uid() == self.uid) {
                return (stream, address);
            }
            // Failed credential reads also fail closed. No HTTP bytes have
            // been read and the next connection can still be accepted.
            drop(stream);
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.listener.local_addr()
    }
}

/// Creates the parent when missing and returns its canonical path after
/// checking that directory. The check and the bind use the one resolved
/// location: if the supplied path runs through a symlink, swapping the link
/// afterwards cannot move the socket into a directory that was never checked.
fn prepare_parent(parent: &Path, uid: libc::uid_t) -> Result<PathBuf> {
    if !parent.try_exists()? {
        DirBuilder::new()
            .mode(0o700)
            .create(parent)
            .with_context(|| {
                format!(
                    "failed to create Unix socket directory {}",
                    parent.display()
                )
            })?;
        fs::set_permissions(parent, Permissions::from_mode(0o700))?;
    }
    let resolved = parent.canonicalize().with_context(|| {
        format!(
            "failed to resolve Unix socket directory {}",
            parent.display()
        )
    })?;
    let metadata = fs::symlink_metadata(&resolved).with_context(|| {
        format!(
            "failed to inspect Unix socket directory {}",
            parent.display()
        )
    })?;
    if !metadata.is_dir() {
        bail!("Unix socket parent {} is not a directory", parent.display());
    }
    if metadata.uid() != uid {
        bail!(
            "Unix socket directory {} is not owned by effective uid {uid}",
            parent.display()
        );
    }
    if metadata.mode() & 0o022 != 0 {
        bail!(
            "Unix socket directory {} is group- or other-writable",
            parent.display()
        );
    }
    Ok(resolved)
}

fn same_file(path: &Path, original: &Metadata) -> bool {
    fs::symlink_metadata(path).is_ok_and(|current| {
        current.file_type() == original.file_type()
            && current.dev() == original.dev()
            && current.ino() == original.ino()
    })
}

struct OwnedFile {
    path: PathBuf,
    metadata: Metadata,
}

impl Drop for OwnedFile {
    fn drop(&mut self) {
        if same_file(&self.path, &self.metadata) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

struct SocketLock {
    // Unlink while still locked; closing the descriptor releases the flock.
    _entry: OwnedFile,
    _file: File,
}

impl SocketLock {
    fn acquire(socket: &Path) -> Result<Self> {
        let mut name = socket
            .file_name()
            .context("Unix socket path needs a file name")?
            .to_os_string();
        name.push(".lock");
        let path = socket.with_file_name(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => {
                bail!("Unix socket lock {} is not a regular file", path.display());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to inspect Unix socket lock"),
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            // NONBLOCK also prevents a raced-in FIFO from blocking the open.
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .with_context(|| format!("failed to open Unix socket lock {}", path.display()))?;
        let metadata = file
            .metadata()
            .context("failed to inspect opened Unix socket lock")?;
        if !metadata.is_file() {
            bail!("Unix socket lock {} is not a regular file", path.display());
        }
        // SAFETY: file owns a valid descriptor for the duration of this call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock {
                bail!("Unix socket {} is already in use", socket.display());
            }
            return Err(error).context("failed to lock Unix socket path");
        }
        // A previous owner may have unlinked this entry during shutdown
        // between our open and flock. Never bind under an obsolete lock.
        if !same_file(&path, &metadata) {
            bail!(
                "Unix socket lock {} changed while acquiring it",
                path.display()
            );
        }
        let lock = Self {
            _entry: OwnedFile { path, metadata },
            _file: file,
        };
        lock._file
            .set_permissions(Permissions::from_mode(0o600))
            .context("failed to restrict Unix socket lock")?;
        Ok(lock)
    }
}

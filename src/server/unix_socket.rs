//! Private socket paths and same-account connection admission.

use anyhow::{bail, Context, Result};
use axum::serve::Listener;
use std::fs::{self, DirBuilder, Metadata, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::{unix::SocketAddr, UnixListener, UnixStream};

/// Owns the listener and the filesystem entry created by this process.
pub struct UnixSocket {
    listener: UnixListener,
    file: SocketFile,
    uid: libc::uid_t,
}

impl UnixSocket {
    pub(super) async fn bind(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path).context("failed to resolve Unix socket path")?;
        let parent = path
            .parent()
            .context("Unix socket path needs a parent directory")?;
        // SAFETY: geteuid takes no arguments and has no safety preconditions.
        let uid = unsafe { libc::geteuid() };
        prepare_parent(parent, uid)?;
        // Resolve the parent once so cleanup and startup output use the same
        // absolute location, including when the supplied parent is a symlink.
        let path = parent.canonicalize()?.join(
            path.file_name()
                .context("Unix socket path needs a file name")?,
        );

        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    bail!(
                        "Unix socket path {} exists and is not a socket",
                        path.display()
                    );
                }
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
        let file = SocketFile {
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

fn prepare_parent(parent: &Path, uid: libc::uid_t) -> Result<()> {
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
    let metadata = fs::metadata(parent).with_context(|| {
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
    Ok(())
}

fn same_file(path: &Path, original: &Metadata) -> bool {
    fs::symlink_metadata(path).is_ok_and(|current| {
        current.file_type().is_socket()
            && current.dev() == original.dev()
            && current.ino() == original.ino()
    })
}

struct SocketFile {
    path: PathBuf,
    metadata: Metadata,
}

impl Drop for SocketFile {
    fn drop(&mut self) {
        if same_file(&self.path, &self.metadata) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

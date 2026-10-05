#![cfg(unix)]

use super::support;
use dcmview::server::{AccessToken, BoundServer, ServerConfig};
use std::fs;
use std::os::unix::fs::{symlink, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        .expect("private directory permissions");
    directory
}

fn config(path: PathBuf) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        unix_socket: Some(path),
        timeout_seconds: None,
        open_browser: false,
        startup_json: false,
        shutdown: CancellationToken::new(),
    }
}

async fn health_status(path: &Path) -> String {
    health_status_with(path, "").await
}

/// The status line of `GET /api/health` sent with `headers`, which is empty
/// or whole header lines each ending in CRLF.
async fn health_status_with(path: &Path, headers: &str) -> String {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut stream = UnixStream::connect(path).await.expect("connect to socket");
        let request = format!(
            "GET /api/health HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write HTTP request");
        let mut status = String::new();
        BufReader::new(stream)
            .read_line(&mut status)
            .await
            .expect("read HTTP status");
        status
    })
    .await
    .expect("health request timeout")
}

async fn await_exit(task: JoinHandle<anyhow::Result<()>>) {
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("shutdown timeout")
        .expect("server task")
        .expect("server result");
}

#[tokio::test]
async fn serves_the_api_over_a_unix_socket() {
    let directory = private_directory();
    let path = directory.path().join("scan.sock");
    let config = config(path.clone());
    let shutdown = config.shutdown.clone();
    let bound = BoundServer::bind(&config).await.expect("bind socket");
    let metadata = fs::metadata(&path).expect("socket metadata");
    assert!(metadata.file_type().is_socket());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

    let task = tokio::spawn(bound.serve(config, support::app_state(Vec::new())));
    assert_eq!(health_status(&path).await, "HTTP/1.1 200 OK\r\n");
    shutdown.cancel();
    await_exit(task).await;
}

#[tokio::test]
async fn socket_path_states_are_handled_safely() {
    #[derive(Clone, Copy, Debug)]
    enum Case {
        Stale,
        Live,
        RegularFile,
        MissingParent,
    }

    for case in [
        Case::Stale,
        Case::Live,
        Case::RegularFile,
        Case::MissingParent,
    ] {
        let directory = private_directory();
        let path = match case {
            Case::MissingParent => directory.path().join("private/scan.sock"),
            _ => directory.path().join("scan.sock"),
        };
        let _live_listener = match case {
            Case::Stale => {
                drop(UnixListener::bind(&path).expect("create stale socket"));
                None
            }
            Case::Live => Some(UnixListener::bind(&path).expect("create live socket")),
            Case::RegularFile => {
                fs::write(&path, b"keep this file").expect("create regular file");
                None
            }
            Case::MissingParent => None,
        };

        let result = BoundServer::bind(&config(path.clone())).await;
        match case {
            Case::Stale | Case::MissingParent => {
                let bound = result.unwrap_or_else(|error| panic!("{case:?}: {error:#}"));
                assert!(fs::metadata(&path)
                    .expect("socket metadata")
                    .file_type()
                    .is_socket());
                if matches!(case, Case::MissingParent) {
                    assert_eq!(
                        fs::metadata(path.parent().expect("parent"))
                            .expect("created directory")
                            .permissions()
                            .mode()
                            & 0o777,
                        0o700
                    );
                }
                drop(bound);
                assert!(!path.exists(), "bound socket Drop must clean up");
            }
            Case::Live | Case::RegularFile => {
                let error = match result {
                    Ok(_) => panic!("{case:?} must be refused"),
                    Err(error) => error,
                };
                if matches!(case, Case::Live) {
                    assert!(error.to_string().contains("already in use"), "{error:#}");
                    UnixStream::connect(&path)
                        .await
                        .expect("live socket remains connectable");
                } else {
                    assert!(error.to_string().contains("not a socket"), "{error:#}");
                    assert_eq!(
                        fs::read(&path).expect("regular file remains"),
                        b"keep this file"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn refuses_a_parent_directory_others_can_write() {
    for mode in [0o777, 0o770] {
        let directory = private_directory();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(mode))
            .expect("set unsafe directory permissions");
        let error = match BoundServer::bind(&config(directory.path().join("scan.sock"))).await {
            Ok(_) => panic!("writable parent must be refused"),
            Err(error) => error,
        };
        let message = error.to_string();
        assert!(
            message.contains(&directory.path().display().to_string()),
            "{message}"
        );
        assert!(message.contains("group- or other-writable"), "{message}");
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("read directory")
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn removes_the_socket_on_shutdown() {
    let directory = private_directory();
    let path = directory.path().join("scan.sock");
    let config = config(path.clone());
    let shutdown = config.shutdown.clone();
    let bound = BoundServer::bind(&config).await.expect("bind socket");
    let task = tokio::spawn(bound.serve(config, support::app_state(Vec::new())));
    assert_eq!(health_status(&path).await, "HTTP/1.1 200 OK\r\n");

    shutdown.cancel();
    await_exit(task).await;
    assert!(!path.exists(), "graceful shutdown must unlink the socket");
}

/// The socket's same-account admission does not stand in for the token: a
/// forwarded socket is reachable by whatever the forwarding end lets in.
#[tokio::test]
async fn a_socket_server_with_a_token_still_requires_it() {
    let directory = private_directory();
    let path = directory.path().join("scan.sock");
    let config = config(path.clone());
    let shutdown = config.shutdown.clone();
    let bound = BoundServer::bind(&config).await.expect("bind socket");
    let token = AccessToken::fixed("socket-test-token").expect("test token");
    let state = support::app_state(Vec::new()).with_access_token(token);
    let task = tokio::spawn(bound.serve(config, state));

    assert_eq!(health_status(&path).await, "HTTP/1.1 401 Unauthorized\r\n");
    assert_eq!(
        health_status_with(&path, "Authorization: Bearer socket-test-token\r\n").await,
        "HTTP/1.1 200 OK\r\n"
    );
    shutdown.cancel();
    await_exit(task).await;
}

/// The parent is judged where it really is. A link in a private directory
/// must not vouch for the directory it points at, and the socket belongs to
/// the directory that was checked even if the link moves afterwards.
#[tokio::test]
async fn a_symlinked_parent_is_checked_and_used_at_its_resolved_location() {
    let links = private_directory();

    let shared = private_directory();
    fs::set_permissions(shared.path(), fs::Permissions::from_mode(0o777))
        .expect("set unsafe directory permissions");
    let to_shared = links.path().join("shared");
    symlink(shared.path(), &to_shared).expect("link to shared directory");
    assert!(
        BoundServer::bind(&config(to_shared.join("scan.sock")))
            .await
            .is_err(),
        "a link to a writable directory must be refused"
    );
    assert_eq!(fs::read_dir(shared.path()).expect("read shared").count(), 0);

    let private = private_directory();
    let elsewhere = private_directory();
    let to_private = links.path().join("private");
    symlink(private.path(), &to_private).expect("link to private directory");
    let bound = BoundServer::bind(&config(to_private.join("scan.sock")))
        .await
        .expect("a link to a private directory is accepted");
    let socket = private.path().join("scan.sock");
    let created = fs::symlink_metadata(&socket).expect("socket in the resolved directory");
    assert!(created.file_type().is_socket());
    assert_eq!(created.permissions().mode() & 0o777, 0o600);

    // Repoint the link: the server keeps the directory it checked.
    fs::remove_file(&to_private).expect("remove link");
    symlink(elsewhere.path(), &to_private).expect("repoint link");
    drop(bound);
    assert!(!socket.exists(), "the socket is removed where it was bound");
    assert_eq!(
        fs::read_dir(elsewhere.path())
            .expect("read elsewhere")
            .count(),
        0
    );
}

/// A live viewer whose accept queue is full must not be mistaken for a stale
/// socket. macOS refuses such a connection with `ECONNREFUSED`, the same
/// answer a socket with no listener gives.
#[tokio::test]
async fn a_live_socket_with_a_full_accept_queue_is_not_taken_over() {
    let directory = private_directory();
    let path = directory.path().join("scan.sock");
    // The first viewer is bound but never accepts, as a stopped or hung
    // process would not.
    let live = BoundServer::bind(&config(path.clone()))
        .await
        .expect("bind the live viewer");
    let original = fs::symlink_metadata(&path).expect("live socket metadata");

    // Fill its accept queue: macOS starts refusing at `kern.ipc.somaxconn`
    // (128 by default). Linux has a longer queue and keeps accepting, so the
    // loop is capped well under the usual descriptor limit; the assertions
    // below have to hold either way.
    let mut waiting = Vec::new();
    for _ in 0..256 {
        match tokio::time::timeout(Duration::from_millis(200), UnixStream::connect(&path)).await {
            Ok(Ok(stream)) => waiting.push(stream),
            Ok(Err(_)) | Err(_) => break,
        }
    }

    let second = BoundServer::bind(&config(path.clone())).await;
    let current = fs::symlink_metadata(&path).expect("the live socket must still exist");
    assert!(
        current.dev() == original.dev() && current.ino() == original.ino(),
        "the live viewer's socket was replaced"
    );
    assert!(second.is_err(), "a second viewer bound over a live one");
    drop(live);
}

/// The lock file beside the socket is what marks a path as owned: it goes
/// away with the viewer, a leftover one does not block the next launch, and
/// it is never followed or replaced when it is not a plain file.
#[tokio::test]
async fn the_lock_file_follows_the_viewer_and_is_never_followed() {
    // A viewer leaves nothing behind.
    let directory = private_directory();
    let path = directory.path().join("scan.sock");
    let lock = directory.path().join("scan.sock.lock");
    let bound = BoundServer::bind(&config(path.clone()))
        .await
        .expect("bind");
    assert!(lock.exists(), "the lock file sits beside the socket");
    drop(bound);
    assert_eq!(
        fs::read_dir(directory.path())
            .expect("read directory")
            .count(),
        0,
        "socket and lock are both removed"
    );

    // A killed viewer leaves both files and holds no lock: the next launch
    // takes the path over.
    drop(UnixListener::bind(&path).expect("leftover socket"));
    fs::write(&lock, b"").expect("leftover lock");
    let bound = BoundServer::bind(&config(path.clone()))
        .await
        .expect("a leftover lock with no holder does not block a launch");
    assert!(fs::metadata(&path)
        .expect("socket metadata")
        .file_type()
        .is_socket());
    drop(bound);

    // A lock path that is not a plain file is refused and left as it was.
    let target = directory.path().join("elsewhere");
    fs::write(&target, b"keep this file").expect("link target");
    symlink(&target, &lock).expect("lock as a symlink");
    assert!(BoundServer::bind(&config(path.clone())).await.is_err());
    assert_eq!(
        fs::read(&target).expect("target remains"),
        b"keep this file"
    );
    assert!(fs::symlink_metadata(&lock)
        .expect("link remains")
        .file_type()
        .is_symlink());
    assert!(!path.exists(), "nothing is bound under a refused lock");

    fs::remove_file(&lock).expect("remove link");
    fs::create_dir(&lock).expect("lock as a directory");
    assert!(BoundServer::bind(&config(path.clone())).await.is_err());
    assert!(lock.is_dir());
    assert!(!path.exists());
}

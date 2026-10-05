#![cfg(unix)]

use super::support;
use dcmview::server::{BoundServer, ServerConfig};
use std::fs;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
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
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut stream = UnixStream::connect(path).await.expect("connect to socket");
        stream
            .write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
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

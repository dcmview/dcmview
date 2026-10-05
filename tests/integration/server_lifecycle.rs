use super::support;
use dcmview::server::{BoundServer, RequestActivity, ServerConfig};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

fn server_config(shutdown: CancellationToken, timeout_seconds: Option<u64>) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        unix_socket: None,
        timeout_seconds,
        open_browser: false,
        startup_json: false,
        shutdown,
    }
}

async fn spawn_server(
    config: ServerConfig,
    state: dcmview::server::AppState,
) -> (String, JoinHandle<anyhow::Result<()>>) {
    let bound = BoundServer::bind(&config).await.expect("bind server");
    assert_ne!(bound.local_addr().port(), 0);
    let url = bound.url();
    let task = tokio::spawn(bound.serve(config, state));
    wait_until_ready(&url).await;
    (url, task)
}

async fn wait_until_ready(url: &str) {
    let client = reqwest::Client::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if client
                .get(format!("{url}/api/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server readiness");
}

async fn await_exit(task: JoinHandle<anyhow::Result<()>>) {
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("server exit timeout")
        .expect("server task")
        .expect("server result")
}

#[tokio::test]
async fn port_zero_serves_on_the_reported_bound_listener() {
    let shutdown = CancellationToken::new();
    let config = server_config(shutdown.clone(), None);
    let bound = BoundServer::bind(&config).await.expect("bind server");
    let address = bound.local_addr();
    let url = bound.url();
    assert_ne!(address.port(), 0);
    assert_eq!(url, format!("http://{address}"));

    let task = tokio::spawn(bound.serve(config, support::app_state(Vec::new())));
    wait_until_ready(&url).await;
    let response = reqwest::get(format!("{url}/api/health"))
        .await
        .expect("health request");
    assert!(response.status().is_success());

    shutdown.cancel();
    await_exit(task).await;
}

#[tokio::test]
async fn occupied_port_keeps_the_bind_error_context_used_by_the_cli() {
    let shutdown = CancellationToken::new();
    let first_config = server_config(shutdown.clone(), None);
    let first = BoundServer::bind(&first_config).await.expect("first bind");
    let occupied = first.local_addr();
    let mut second_config = server_config(shutdown, None);
    second_config.port = occupied.port();

    let error = match BoundServer::bind(&second_config).await {
        Ok(_) => panic!("occupied port must fail"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains(&format!("failed to bind to 127.0.0.1:{}", occupied.port())),
        "unexpected bind error: {error:#}"
    );
}

#[tokio::test]
async fn external_notification_returns_from_serve_normally() {
    let shutdown = CancellationToken::new();
    let (url, task) = spawn_server(
        server_config(shutdown.clone(), None),
        support::app_state(Vec::new()),
    )
    .await;

    assert!(reqwest::get(format!("{url}/"))
        .await
        .expect("root request")
        .status()
        .is_success());
    shutdown.cancel();

    await_exit(task).await;
}

#[tokio::test]
async fn idle_timeout_returns_without_terminating_the_test_process() {
    let shutdown = CancellationToken::new();
    let config = server_config(shutdown, Some(1));
    let bound = BoundServer::bind(&config).await.expect("bind server");
    let task = tokio::spawn(bound.serve(config, support::app_state(Vec::new())));

    await_exit(task).await;
}

#[tokio::test]
async fn browser_route_requests_reset_the_idle_timeout() {
    let shutdown = CancellationToken::new();
    let (url, task) = spawn_server(
        server_config(shutdown, Some(1)),
        support::app_state(Vec::new()),
    )
    .await;
    let client = reqwest::Client::new();

    for _ in 0..3 {
        assert!(client
            .get(format!("{url}/"))
            .send()
            .await
            .expect("root request")
            .status()
            .is_success());
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(!task.is_finished(), "root request did not reset timeout");
    }

    await_exit(task).await;
}

#[tokio::test]
async fn graceful_shutdown_drains_an_in_flight_request() {
    let shutdown = CancellationToken::new();
    let state = support::app_state(Vec::new());
    let activity: RequestActivity = state.activity().clone();
    let config = server_config(shutdown.clone(), None);
    let bound = BoundServer::bind(&config).await.expect("bind server");
    let address = bound.local_addr();
    let task = tokio::spawn(bound.serve(config, state));

    let mut stream = TcpStream::connect(address).await.expect("connect");
    let body = br#"{"num_roi":0,"roi_coords":[],"roi_frames":[]}"#;
    let split = body.len() / 2;
    let headers = format!(
        "PUT /api/file/0/annotations HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .await
        .expect("write headers");
    stream
        .write_all(&body[..split])
        .await
        .expect("write partial body");

    tokio::time::timeout(Duration::from_secs(2), async {
        while activity.in_flight() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("request entered middleware");

    shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !task.is_finished(),
        "server exited before the in-flight body completed"
    );

    stream
        .write_all(&body[split..])
        .await
        .expect("finish request body");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    assert!(response.starts_with(b"HTTP/1.1 404"));

    await_exit(task).await;
}

#[tokio::test]
async fn startup_json_timeout_cli_exits_naturally() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_dcmview"));
    command
        .arg("--startup-json")
        .arg("--timeout")
        .arg("1")
        .arg("--no-browser")
        .arg(fixture)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = tokio::time::timeout(
        Duration::from_secs(5),
        command.spawn().expect("spawn dcmview").wait_with_output(),
    )
    .await
    .expect("CLI timeout")
    .expect("CLI output");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 stdout");
    let startup = stdout
        .lines()
        .find(|line| line.starts_with("{\"type\":\"server_started\""))
        .expect("startup JSON line");
    let payload: serde_json::Value = serde_json::from_str(startup).expect("startup JSON");
    assert_ne!(payload["port"], 0);
    assert!(stdout.contains("dcmview: shutting down..."));
}

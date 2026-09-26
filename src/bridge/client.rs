use super::protocol::{BridgeLaunchRequest, BridgeLaunchResponse, BridgeWaitResponse};
use super::registry::{
    bridge_debug, discover_vscode_bridge_endpoints, remove_vscode_bridge_registry_endpoint,
    BridgeEndpoint,
};
use anyhow::{Context, Result};
use dcmview::signals::StopSignals;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Budget for connecting to a bridge and for the best-effort stop request.
const BRIDGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Budget for a `/launch` response. The extension answers only after the
/// viewer has started, bounded by its `startupTimeoutSeconds` setting (20 s by
/// default), so this must comfortably exceed that.
const BRIDGE_LAUNCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Whether a launch was handed to VS Code.
pub(crate) enum BridgeOutcome {
    /// VS Code owns the launch; exit with this code.
    Routed(i32),
    /// No bridge took the launch; run the local viewer.
    NotRouted,
}

/// A failed launch, classified by whether VS Code may have opened a viewer.
#[derive(Debug, thiserror::Error)]
enum LaunchError {
    /// Nothing listens at the endpoint, so its registry entry is stale.
    #[error("VS Code bridge is not running: {0}")]
    Unreachable(String),
    /// The bridge never received or explicitly refused the launch.
    #[error("{0}")]
    NotLaunched(String),
    /// The bridge received the launch but did not confirm it. VS Code may
    /// still open a viewer, so neither another endpoint nor the local viewer
    /// may be tried.
    #[error("VS Code bridge did not confirm the launch: {0}")]
    Uncertain(String),
}

enum LaunchAttempt {
    Launched {
        endpoint: BridgeEndpoint,
        response: BridgeLaunchResponse,
    },
    Failed(LaunchError),
    Uncertain(LaunchError),
}

/// Route a launch into VS Code when the routing rule selects a bridge.
///
/// `program` only names the VS Code tab; `args` are forwarded unchanged.
pub(crate) async fn launch_in_vscode(
    program: &str,
    args: &[String],
    startup_json: bool,
) -> BridgeOutcome {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let endpoints = discover_vscode_bridge_endpoints(&cwd);
    if endpoints.is_empty() {
        return BridgeOutcome::NotRouted;
    }
    let client = match reqwest::Client::builder()
        .connect_timeout(BRIDGE_REQUEST_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!(
                "dcmview: VS Code bridge unavailable ({error}); falling back to local viewer"
            );
            return BridgeOutcome::NotRouted;
        }
    };

    let request = launch_request(program, args, &cwd, current_executable());
    let (attempt, unreachable) =
        launch_on_first_endpoint(&client, &endpoints, &request, BRIDGE_LAUNCH_TIMEOUT).await;
    for endpoint in &unreachable {
        remove_vscode_bridge_registry_endpoint(endpoint);
    }

    match attempt {
        LaunchAttempt::Launched { endpoint, response } => {
            // Listen before announcing the URL: a wrapper may interrupt as soon
            // as it reads it, and an unhandled signal would kill this process
            // without closing the VS Code viewer.
            let stop_signals = StopSignals::listen();
            print_launched(&response.url, startup_json);
            BridgeOutcome::Routed(
                wait_for_launched_vscode_session(&client, &endpoint, &response, stop_signals).await,
            )
        }
        LaunchAttempt::Failed(error) => {
            eprintln!(
                "dcmview: VS Code bridge unavailable ({error}); falling back to local viewer"
            );
            BridgeOutcome::NotRouted
        }
        LaunchAttempt::Uncertain(error) => {
            eprintln!(
                "dcmview: {error}. VS Code may still open the viewer, so no local viewer was started."
            );
            BridgeOutcome::Routed(1)
        }
    }
}

fn current_executable() -> Option<String> {
    env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
}

fn launch_request(
    program: &str,
    args: &[String],
    cwd: &Path,
    binary_path: Option<String>,
) -> BridgeLaunchRequest {
    BridgeLaunchRequest {
        program: program.to_string(),
        args: args.to_vec(),
        cwd: cwd.display().to_string(),
        wait: false,
        binary_path,
    }
}

fn print_launched(url: &str, startup_json: bool) {
    if startup_json {
        println!(
            "{}",
            serde_json::json!({ "type": "vscode_session_started", "url": url })
        );
    }
    println!("dcmview: opened in VS Code at {url}");
}

/// Try endpoints in order until one launches or the outcome is uncertain.
///
/// Also returns the endpoints that refused the connection, whose registry
/// entries are stale.
async fn launch_on_first_endpoint(
    client: &reqwest::Client,
    endpoints: &[BridgeEndpoint],
    request: &BridgeLaunchRequest,
    launch_timeout: Duration,
) -> (LaunchAttempt, Vec<BridgeEndpoint>) {
    let mut unreachable = Vec::new();
    let mut last_error = None;
    for endpoint in endpoints {
        match launch_vscode_session(client, endpoint, request, launch_timeout).await {
            Ok(response) => {
                let attempt = LaunchAttempt::Launched {
                    endpoint: endpoint.clone(),
                    response,
                };
                return (attempt, unreachable);
            }
            Err(error @ LaunchError::Uncertain(_)) => {
                return (LaunchAttempt::Uncertain(error), unreachable);
            }
            Err(error) => {
                bridge_debug(&format!("endpoint {} failed: {error}", endpoint.url));
                if matches!(error, LaunchError::Unreachable(_)) {
                    unreachable.push(endpoint.clone());
                }
                last_error = Some(error);
            }
        }
    }
    let error = last_error
        .unwrap_or_else(|| LaunchError::NotLaunched("no VS Code bridge endpoints".to_string()));
    (LaunchAttempt::Failed(error), unreachable)
}

async fn launch_vscode_session(
    client: &reqwest::Client,
    endpoint: &BridgeEndpoint,
    request: &BridgeLaunchRequest,
    launch_timeout: Duration,
) -> std::result::Result<BridgeLaunchResponse, LaunchError> {
    let launch_url = format!("{}/launch", endpoint.url.trim_end_matches('/'));
    let response = client
        .post(launch_url)
        .bearer_auth(&endpoint.token)
        .json(request)
        .timeout(launch_timeout)
        .send()
        .await
        .map_err(classify_send_error)?;
    if !response.status().is_success() {
        let status = response.status();
        let message = bridge_error_response_message(response).await;
        return Err(LaunchError::NotLaunched(format!(
            "VS Code bridge returned {status}: {message}"
        )));
    }
    response
        .json::<BridgeLaunchResponse>()
        .await
        .map_err(|error| LaunchError::Uncertain(format!("unreadable launch response: {error}")))
}

fn classify_send_error(error: reqwest::Error) -> LaunchError {
    if error.is_connect() && !error.is_timeout() {
        LaunchError::Unreachable(error.to_string())
    } else if error.is_connect() || error.is_builder() {
        LaunchError::NotLaunched(format!("failed to contact VS Code bridge: {error}"))
    } else {
        LaunchError::Uncertain(error.to_string())
    }
}

async fn bridge_error_response_message(response: reqwest::Response) -> String {
    let status = response.status();
    let Ok(text) = response.text().await else {
        return status.to_string();
    };
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
            return error.to_string();
        }
    }
    if text.is_empty() {
        status.to_string()
    } else {
        text
    }
}

async fn wait_for_launched_vscode_session(
    client: &reqwest::Client,
    endpoint: &BridgeEndpoint,
    response: &BridgeLaunchResponse,
    mut stop_signals: StopSignals,
) -> i32 {
    let interrupt = async move { stop_signals.recv().await };
    let result = wait_for_launched_vscode_session_with_interrupt(
        client,
        endpoint,
        response,
        BRIDGE_REQUEST_TIMEOUT,
        interrupt,
    )
    .await;
    result.unwrap_or_else(|error| {
        eprintln!("dcmview: VS Code bridge session was captured but wait failed: {error:#}");
        1
    })
}

async fn wait_for_launched_vscode_session_with_interrupt<F>(
    client: &reqwest::Client,
    endpoint: &BridgeEndpoint,
    response: &BridgeLaunchResponse,
    request_timeout: Duration,
    interrupt: F,
) -> Result<i32>
where
    F: Future<Output = ()>,
{
    let base_url = endpoint.url.trim_end_matches('/');
    let wait_url = format!("{base_url}/sessions/{}/wait", response.session_id);
    let stop_url = format!("{base_url}/sessions/{}/stop", response.session_id);

    tokio::select! {
        wait_result = wait_for_vscode_session(client, &wait_url, &endpoint.token) => wait_result,
        () = interrupt => {
            let _ = client
                .post(stop_url)
                .bearer_auth(&endpoint.token)
                .timeout(request_timeout)
                .send()
                .await;
            // Report how the stopped viewer exited, as a local viewer would.
            let stopped = tokio::time::timeout(
                request_timeout,
                wait_for_vscode_session(client, &wait_url, &endpoint.token),
            )
            .await;
            Ok(match stopped {
                Ok(Ok(exit_code)) => exit_code,
                _ => 130,
            })
        }
    }
}

async fn wait_for_vscode_session(
    client: &reqwest::Client,
    wait_url: &str,
    token: &str,
) -> Result<i32> {
    let response = client
        .get(wait_url)
        .bearer_auth(token)
        .send()
        .await
        .context("failed to wait for VS Code dcmview session")?;
    if !response.status().is_success() {
        return Ok(1);
    }
    let wait_response = response.json::<BridgeWaitResponse>().await?;
    Ok(wait_response.exit_code.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Path as AxumPath, State};
    use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;
    use tokio::sync::Notify;
    use tokio::task::JoinHandle;

    fn endpoint(url: impl Into<String>) -> BridgeEndpoint {
        BridgeEndpoint {
            url: url.into(),
            token: "bridge-token".to_string(),
        }
    }

    fn test_request() -> BridgeLaunchRequest {
        launch_request(
            "dcmview",
            &["scan.dcm".to_string()],
            Path::new("/workspace"),
            Some("/opt/dcmview/bin/dcmview".to_string()),
        )
    }

    #[derive(Clone, Default)]
    struct MockBridgeState {
        events: Arc<Mutex<Vec<String>>>,
        launch_requests: Arc<Mutex<Vec<serde_json::Value>>>,
    }

    impl MockBridgeState {
        fn events(&self) -> Vec<String> {
            self.events.lock().expect("mock events").clone()
        }
    }

    async fn authenticated_launch(
        State(state): State<MockBridgeState>,
        headers: HeaderMap,
        Json(request): Json<BridgeLaunchRequest>,
    ) -> Response {
        if bearer_token(&headers) != Some("bridge-token") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        state
            .events
            .lock()
            .expect("mock events")
            .push(format!("launch:{}:{}", request.program, request.cwd));
        state
            .launch_requests
            .lock()
            .expect("mock launch requests")
            .push(serde_json::to_value(request).expect("serialize mock launch request"));
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "sessionId": "session-1",
                "url": "http://127.0.0.1:51234"
            })),
        )
            .into_response()
    }

    async fn authenticated_wait(
        AxumPath(session_id): AxumPath<String>,
        State(state): State<MockBridgeState>,
        headers: HeaderMap,
    ) -> Response {
        if bearer_token(&headers) != Some("bridge-token") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        state
            .events
            .lock()
            .expect("mock events")
            .push(format!("wait:{session_id}"));
        (StatusCode::OK, Json(serde_json::json!({ "exitCode": 7 }))).into_response()
    }

    fn bearer_token(headers: &HeaderMap) -> Option<&str> {
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
    }

    fn launch_router(state: MockBridgeState) -> Router {
        Router::new()
            .route("/launch", post(authenticated_launch))
            .route("/sessions/{session_id}/wait", get(authenticated_wait))
            .with_state(state)
    }

    #[tokio::test]
    async fn authenticated_launch_and_wait_round_trip_against_axum_bridge() {
        let state = MockBridgeState::default();
        let server = MockServer::spawn(launch_router(state.clone())).await;
        let client = reqwest::Client::new();

        let (attempt, unreachable) = launch_on_first_endpoint(
            &client,
            &[endpoint(format!("{}/", server.url()))],
            &test_request(),
            Duration::from_secs(1),
        )
        .await;
        let LaunchAttempt::Launched { endpoint, response } = attempt else {
            panic!("launch should succeed");
        };
        let exit_code = wait_for_launched_vscode_session_with_interrupt(
            &client,
            &endpoint,
            &response,
            Duration::from_secs(1),
            std::future::pending(),
        )
        .await
        .expect("wait succeeds");

        assert_eq!(exit_code, 7);
        assert!(unreachable.is_empty());
        assert_eq!(
            state.events(),
            vec!["launch:dcmview:/workspace", "wait:session-1"]
        );
        assert_eq!(
            *state.launch_requests.lock().expect("mock launch requests"),
            vec![serde_json::json!({
                "program": "dcmview",
                "args": ["scan.dcm"],
                "cwd": "/workspace",
                "wait": false,
                "binaryPath": "/opt/dcmview/bin/dcmview"
            })]
        );
    }

    #[derive(Clone, Default)]
    struct InterruptBridgeState {
        wait_started: Arc<Notify>,
        stop_events: Arc<Mutex<Vec<String>>>,
    }

    async fn pending_wait(
        AxumPath(session_id): AxumPath<String>,
        State(state): State<InterruptBridgeState>,
        headers: HeaderMap,
    ) -> Response {
        if bearer_token(&headers) != Some("bridge-token") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        assert_eq!(session_id, "session-1");
        state.wait_started.notify_one();
        std::future::pending::<Response>().await
    }

    async fn record_stop(
        AxumPath(session_id): AxumPath<String>,
        State(state): State<InterruptBridgeState>,
        headers: HeaderMap,
    ) -> Response {
        let token = bearer_token(&headers).unwrap_or("missing");
        state
            .stop_events
            .lock()
            .expect("stop events")
            .push(format!("{session_id}:{token}"));
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }

    #[tokio::test]
    async fn interrupt_posts_authenticated_stop_and_ignores_stop_failure() {
        let state = InterruptBridgeState::default();
        let server = MockServer::spawn(
            Router::new()
                .route("/sessions/{session_id}/wait", get(pending_wait))
                .route("/sessions/{session_id}/stop", post(record_stop))
                .with_state(state.clone()),
        )
        .await;
        let wait_started = state.wait_started.clone();
        let interrupt = async move { wait_started.notified().await };

        let exit_code = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_launched_vscode_session_with_interrupt(
                &reqwest::Client::new(),
                &endpoint(server.url()),
                &BridgeLaunchResponse {
                    session_id: "session-1".to_string(),
                    url: "http://127.0.0.1:51234".to_string(),
                },
                Duration::from_millis(100),
                interrupt,
            ),
        )
        .await
        .expect("interrupt flow completes")
        .expect("interrupt flow succeeds");

        assert_eq!(exit_code, 130);
        assert_eq!(
            *state.stop_events.lock().expect("stop events"),
            vec!["session-1:bridge-token".to_string()]
        );
    }

    #[derive(Clone, Default)]
    struct StoppableBridgeState {
        wait_started: Arc<Notify>,
        stopped: Arc<std::sync::atomic::AtomicBool>,
    }

    async fn wait_until_stopped(State(state): State<StoppableBridgeState>) -> Response {
        if state.stopped.load(std::sync::atomic::Ordering::SeqCst) {
            return (StatusCode::OK, Json(serde_json::json!({ "exitCode": 0 }))).into_response();
        }
        state.wait_started.notify_one();
        std::future::pending::<Response>().await
    }

    async fn accept_stop(State(state): State<StoppableBridgeState>) -> Response {
        state
            .stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response()
    }

    #[tokio::test]
    async fn interrupt_reports_the_stopped_viewers_exit_code() {
        let state = StoppableBridgeState::default();
        let server = MockServer::spawn(
            Router::new()
                .route("/sessions/{session_id}/wait", get(wait_until_stopped))
                .route("/sessions/{session_id}/stop", post(accept_stop))
                .with_state(state.clone()),
        )
        .await;
        let wait_started = state.wait_started.clone();

        let exit_code = wait_for_launched_vscode_session_with_interrupt(
            &reqwest::Client::new(),
            &endpoint(server.url()),
            &BridgeLaunchResponse {
                session_id: "session-1".to_string(),
                url: "http://127.0.0.1:51234".to_string(),
            },
            Duration::from_secs(1),
            async move { wait_started.notified().await },
        )
        .await
        .expect("interrupt flow succeeds");

        assert_eq!(exit_code, 0);
    }

    async fn http_error() -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "bridge unavailable" })),
        )
            .into_response()
    }

    async fn invalid_launch_response() -> Response {
        (StatusCode::OK, Json(serde_json::json!({ "sessionId": 42 }))).into_response()
    }

    async fn slow_launch() -> Response {
        tokio::time::sleep(Duration::from_millis(500)).await;
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "sessionId": "late",
                "url": "http://127.0.0.1:51234"
            })),
        )
            .into_response()
    }

    #[tokio::test]
    async fn launch_errors_are_classified_by_whether_vs_code_may_have_launched() {
        let client = reqwest::Client::new();
        let launch = |url: String, timeout: Duration| {
            let client = client.clone();
            async move {
                launch_vscode_session(&client, &endpoint(url), &test_request(), timeout)
                    .await
                    .expect_err("launch fails")
            }
        };

        let refused = launch(unused_loopback_url().await, Duration::from_secs(1)).await;
        assert!(
            matches!(refused, LaunchError::Unreachable(_)),
            "{refused:?}"
        );

        let invalid_url = launch("not a URL".to_string(), Duration::from_secs(1)).await;
        assert!(
            matches!(invalid_url, LaunchError::NotLaunched(_)),
            "{invalid_url:?}"
        );

        let http_server = MockServer::spawn(Router::new().route("/launch", post(http_error))).await;
        let http = launch(http_server.url(), Duration::from_secs(1)).await;
        assert!(
            matches!(&http, LaunchError::NotLaunched(message) if message.contains("bridge unavailable")),
            "{http:?}"
        );

        let decode_server =
            MockServer::spawn(Router::new().route("/launch", post(invalid_launch_response))).await;
        let decode = launch(decode_server.url(), Duration::from_secs(1)).await;
        assert!(matches!(decode, LaunchError::Uncertain(_)), "{decode:?}");

        let slow_server =
            MockServer::spawn(Router::new().route("/launch", post(slow_launch))).await;
        let timed_out = launch(slow_server.url(), Duration::from_millis(50)).await;
        assert!(
            matches!(timed_out, LaunchError::Uncertain(_)),
            "{timed_out:?}"
        );
    }

    #[tokio::test]
    async fn refused_and_rejected_endpoints_fall_through_to_the_next() {
        let state = MockBridgeState::default();
        let good = MockServer::spawn(launch_router(state.clone())).await;
        let rejecting = MockServer::spawn(Router::new().route("/launch", post(http_error))).await;
        let refused = endpoint(unused_loopback_url().await);

        let (attempt, unreachable) = launch_on_first_endpoint(
            &reqwest::Client::new(),
            &[
                refused.clone(),
                endpoint(rejecting.url()),
                endpoint(good.url()),
            ],
            &test_request(),
            Duration::from_secs(1),
        )
        .await;

        assert!(matches!(attempt, LaunchAttempt::Launched { .. }));
        assert_eq!(
            unreachable,
            vec![refused],
            "only refused endpoints are stale"
        );
        assert_eq!(state.events(), vec!["launch:dcmview:/workspace"]);
    }

    #[tokio::test]
    async fn slow_launch_stops_without_trying_other_bridges_or_marking_it_stale() {
        let state = MockBridgeState::default();
        let slow = MockServer::spawn(Router::new().route("/launch", post(slow_launch))).await;
        let good = MockServer::spawn(launch_router(state.clone())).await;

        let (attempt, unreachable) = launch_on_first_endpoint(
            &reqwest::Client::new(),
            &[endpoint(slow.url()), endpoint(good.url())],
            &test_request(),
            Duration::from_millis(50),
        )
        .await;

        assert!(matches!(attempt, LaunchAttempt::Uncertain(_)));
        assert!(
            unreachable.is_empty(),
            "a live but slow bridge is not stale"
        );
        assert!(state.events().is_empty(), "no second viewer is launched");
    }

    struct MockServer {
        base_url: String,
        task: JoinHandle<()>,
    }

    impl MockServer {
        async fn spawn(router: Router) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock bridge");
            let address = listener.local_addr().expect("mock bridge address");
            let task = tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("serve mock bridge");
            });
            Self {
                base_url: format!("http://{address}"),
                task,
            }
        }

        fn url(&self) -> String {
            self.base_url.clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn unused_loopback_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind unused endpoint");
        let address = listener.local_addr().expect("unused endpoint address");
        drop(listener);
        format!("http://{address}")
    }
}

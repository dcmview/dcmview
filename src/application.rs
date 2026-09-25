use crate::bridge::{
    discover_vscode_bridge_endpoints, run_vscode_bridge_client, run_vscode_bridge_launch,
    RegistryMatch,
};
use crate::startup::{self, LocalViewerOptions, LocalViewerOutcome};
use crate::Cli;
use anyhow::Result;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Once;

type ApplicationFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

trait ApplicationServices: Send + Sync {
    fn current_dir(&self) -> PathBuf;

    fn run_hidden_bridge_client<'a>(&'a self, values: Vec<String>) -> ApplicationFuture<'a, i32>;

    fn try_workspace_bridge<'a>(
        &'a self,
        cwd: &'a Path,
        raw_args: &'a [String],
    ) -> ApplicationFuture<'a, Option<i32>>;

    fn initialize_tracing(&self);

    fn run_local_viewer<'a>(
        &'a self,
        options: LocalViewerOptions,
    ) -> ApplicationFuture<'a, LocalViewerOutcome>;
}

struct ProductionApplicationServices;

impl ApplicationServices for ProductionApplicationServices {
    fn current_dir(&self) -> PathBuf {
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn run_hidden_bridge_client<'a>(&'a self, values: Vec<String>) -> ApplicationFuture<'a, i32> {
        Box::pin(async move { run_vscode_bridge_client(values).await })
    }

    fn try_workspace_bridge<'a>(
        &'a self,
        cwd: &'a Path,
        raw_args: &'a [String],
    ) -> ApplicationFuture<'a, Option<i32>> {
        Box::pin(async move {
            let bridge_endpoints =
                discover_vscode_bridge_endpoints(cwd, RegistryMatch::RequireWorkspace);
            if bridge_endpoints.is_empty() {
                return Ok(None);
            }

            run_vscode_bridge_launch("dcmview", raw_args, &bridge_endpoints)
                .await
                .map(Some)
        })
    }

    fn initialize_tracing(&self) {
        initialize_tracing();
    }

    fn run_local_viewer<'a>(
        &'a self,
        options: LocalViewerOptions,
    ) -> ApplicationFuture<'a, LocalViewerOutcome> {
        Box::pin(async move { startup::run_local_viewer(options).await })
    }
}

pub(crate) async fn run(cli: Cli, raw_args: &[String]) -> Result<i32> {
    let services = ProductionApplicationServices;
    run_with_services(cli, raw_args, &services).await
}

async fn run_with_services(
    mut cli: Cli,
    raw_args: &[String],
    services: &dyn ApplicationServices,
) -> Result<i32> {
    if let Some(bridge_args) = cli.vscode_bridge_client.take() {
        return services.run_hidden_bridge_client(bridge_args).await;
    }

    let cwd = services.current_dir();
    match services.try_workspace_bridge(&cwd, raw_args).await {
        Ok(Some(exit_code)) => return Ok(exit_code),
        Ok(None) => {}
        Err(error) => {
            eprintln!(
                "dcmview: VS Code bridge unavailable ({error}); falling back to local viewer"
            );
        }
    }

    let options = local_viewer_options(cli);
    services.initialize_tracing();
    let outcome = services.run_local_viewer(options).await?;
    Ok(outcome.exit_code())
}

fn local_viewer_options(cli: Cli) -> LocalViewerOptions {
    LocalViewerOptions {
        input_paths: cli.paths,
        recursive: !cli.no_recursive,
        filters: cli.filters,
        annotation_path: cli.annotations,
        host: cli.host,
        port: cli.port,
        timeout_seconds: cli.timeout,
        open_browser: !cli.no_browser,
        startup_json: cli.startup_json,
    }
}

fn initialize_tracing() {
    static TRACING_INITIALIZATION: Once = Once::new();

    TRACING_INITIALIZATION.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("info,jpeg2k=warn")
            .try_init();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use dcmview::loader;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum ApplicationCall {
        CurrentDir,
        HiddenBridgeClient(Vec<String>),
        WorkspaceBridge { cwd: PathBuf, raw_args: Vec<String> },
        InitializeTracing,
        LocalViewer(RecordedLocalOptions),
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedLocalOptions {
        input_paths: Vec<PathBuf>,
        recursive: bool,
        filters: Vec<loader::ScanFilter>,
        annotation_path: Option<PathBuf>,
        host: String,
        port: u16,
        timeout_seconds: Option<u64>,
        open_browser: bool,
        startup_json: bool,
    }

    impl From<LocalViewerOptions> for RecordedLocalOptions {
        fn from(options: LocalViewerOptions) -> Self {
            Self {
                input_paths: options.input_paths,
                recursive: options.recursive,
                filters: options.filters,
                annotation_path: options.annotation_path,
                host: options.host,
                port: options.port,
                timeout_seconds: options.timeout_seconds,
                open_browser: options.open_browser,
                startup_json: options.startup_json,
            }
        }
    }

    #[derive(Debug, Clone, Copy)]
    enum WorkspaceBridgeBehavior {
        Missing,
        Exit(i32),
        Error(&'static str),
    }

    struct RecordingApplicationServices {
        calls: Mutex<Vec<ApplicationCall>>,
        cwd: PathBuf,
        hidden_exit_code: i32,
        workspace_behavior: WorkspaceBridgeBehavior,
        local_outcome: LocalViewerOutcome,
    }

    impl RecordingApplicationServices {
        fn new(
            workspace_behavior: WorkspaceBridgeBehavior,
            local_outcome: LocalViewerOutcome,
        ) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                cwd: PathBuf::from("/workspace/dicom"),
                hidden_exit_code: 23,
                workspace_behavior,
                local_outcome,
            }
        }

        fn record(&self, call: ApplicationCall) {
            self.calls
                .lock()
                .expect("application call log lock")
                .push(call);
        }

        fn calls(&self) -> Vec<ApplicationCall> {
            self.calls
                .lock()
                .expect("application call log lock")
                .clone()
        }
    }

    impl ApplicationServices for RecordingApplicationServices {
        fn current_dir(&self) -> PathBuf {
            self.record(ApplicationCall::CurrentDir);
            self.cwd.clone()
        }

        fn run_hidden_bridge_client<'a>(
            &'a self,
            values: Vec<String>,
        ) -> ApplicationFuture<'a, i32> {
            self.record(ApplicationCall::HiddenBridgeClient(values));
            let exit_code = self.hidden_exit_code;
            Box::pin(async move { Ok(exit_code) })
        }

        fn try_workspace_bridge<'a>(
            &'a self,
            cwd: &'a Path,
            raw_args: &'a [String],
        ) -> ApplicationFuture<'a, Option<i32>> {
            self.record(ApplicationCall::WorkspaceBridge {
                cwd: cwd.to_path_buf(),
                raw_args: raw_args.to_vec(),
            });
            let behavior = self.workspace_behavior;
            Box::pin(async move {
                match behavior {
                    WorkspaceBridgeBehavior::Missing => Ok(None),
                    WorkspaceBridgeBehavior::Exit(exit_code) => Ok(Some(exit_code)),
                    WorkspaceBridgeBehavior::Error(message) => Err(anyhow::Error::msg(message)),
                }
            })
        }

        fn initialize_tracing(&self) {
            self.record(ApplicationCall::InitializeTracing);
        }

        fn run_local_viewer<'a>(
            &'a self,
            options: LocalViewerOptions,
        ) -> ApplicationFuture<'a, LocalViewerOutcome> {
            self.record(ApplicationCall::LocalViewer(options.into()));
            let outcome = self.local_outcome;
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn local_cli() -> Cli {
        Cli {
            paths: vec![
                PathBuf::from("/dicom/first.dcm"),
                PathBuf::from("/dicom/second.dcm"),
            ],
            port: 8123,
            host: "0.0.0.0".to_string(),
            no_browser: true,
            timeout: Some(47),
            no_recursive: true,
            annotations: Some(PathBuf::from("/annotations/rois.csv")),
            filters: vec!["modality=CT".parse().expect("filter parses")],
            startup_json: true,
            vscode_bridge_client: None,
        }
    }

    fn raw_args() -> Vec<String> {
        vec![
            "--no-browser".to_string(),
            "--port".to_string(),
            "8123".to_string(),
            "/dicom/first.dcm".to_string(),
        ]
    }

    fn expected_local_options() -> RecordedLocalOptions {
        RecordedLocalOptions {
            input_paths: vec![
                PathBuf::from("/dicom/first.dcm"),
                PathBuf::from("/dicom/second.dcm"),
            ],
            recursive: false,
            filters: vec!["modality=CT".parse().expect("filter parses")],
            annotation_path: Some(PathBuf::from("/annotations/rois.csv")),
            host: "0.0.0.0".to_string(),
            port: 8123,
            timeout_seconds: Some(47),
            open_browser: false,
            startup_json: true,
        }
    }

    #[tokio::test]
    async fn hidden_bridge_client_short_circuits_workspace_and_local() {
        let mut cli = local_cli();
        let hidden_args = vec![
            "python".to_string(),
            "-m".to_string(),
            "pipeline".to_string(),
        ];
        cli.vscode_bridge_client = Some(hidden_args.clone());
        let services = RecordingApplicationServices::new(
            WorkspaceBridgeBehavior::Error("workspace bridge must not run"),
            LocalViewerOutcome::DiscoveryFailed,
        );

        let exit_code = run_with_services(cli, &raw_args(), &services)
            .await
            .expect("hidden bridge client succeeds");

        assert_eq!(exit_code, 23);
        assert_eq!(
            services.calls(),
            vec![ApplicationCall::HiddenBridgeClient(hidden_args)]
        );
    }

    #[tokio::test]
    async fn workspace_bridge_success_short_circuits_local_viewer() {
        let cli = local_cli();
        let raw_args = raw_args();
        let services = RecordingApplicationServices::new(
            WorkspaceBridgeBehavior::Exit(1),
            LocalViewerOutcome::DiscoveryFailed,
        );

        let exit_code = run_with_services(cli, &raw_args, &services)
            .await
            .expect("workspace bridge exit is returned");

        assert_eq!(exit_code, 1);
        assert_eq!(
            services.calls(),
            vec![
                ApplicationCall::CurrentDir,
                ApplicationCall::WorkspaceBridge {
                    cwd: PathBuf::from("/workspace/dicom"),
                    raw_args,
                },
            ]
        );
    }

    #[tokio::test]
    async fn workspace_bridge_error_falls_back_to_local_in_process() {
        let raw_args = raw_args();
        let services = RecordingApplicationServices::new(
            WorkspaceBridgeBehavior::Error("connection refused"),
            LocalViewerOutcome::DiscoveryFailed,
        );

        let exit_code = run_with_services(local_cli(), &raw_args, &services)
            .await
            .expect("local fallback returns a typed outcome");

        assert_eq!(exit_code, 1);
        assert_eq!(
            services.calls(),
            vec![
                ApplicationCall::CurrentDir,
                ApplicationCall::WorkspaceBridge {
                    cwd: PathBuf::from("/workspace/dicom"),
                    raw_args,
                },
                ApplicationCall::InitializeTracing,
                ApplicationCall::LocalViewer(expected_local_options()),
            ]
        );
    }

    #[tokio::test]
    async fn missing_workspace_bridge_runs_local_in_process() {
        let raw_args = raw_args();
        let services = RecordingApplicationServices::new(
            WorkspaceBridgeBehavior::Missing,
            LocalViewerOutcome::Completed,
        );

        let exit_code = run_with_services(local_cli(), &raw_args, &services)
            .await
            .expect("local viewer succeeds");

        assert_eq!(exit_code, 0);
        assert_eq!(
            services.calls(),
            vec![
                ApplicationCall::CurrentDir,
                ApplicationCall::WorkspaceBridge {
                    cwd: PathBuf::from("/workspace/dicom"),
                    raw_args,
                },
                ApplicationCall::InitializeTracing,
                ApplicationCall::LocalViewer(expected_local_options()),
            ]
        );
    }
}

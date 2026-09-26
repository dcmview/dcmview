use crate::bridge::{launch_in_vscode, BridgeOutcome};
use crate::startup::{self, LocalViewerOptions};
use crate::Cli;
use anyhow::Result;
use clap::Parser;
use std::sync::Once;

/// Route the launch into VS Code when the routing rule selects a bridge;
/// otherwise run the local viewer in this process.
pub(crate) async fn run(mut cli: Cli, raw_args: &[String]) -> Result<i32> {
    let (program, args) = match cli.vscode_bridge_client.take() {
        Some(values) => {
            let (program, args) = split_bridge_client_values(values);
            cli = Cli::parse_from(std::iter::once("dcmview".to_string()).chain(args.clone()));
            (program, args)
        }
        None => ("dcmview".to_string(), raw_args.to_vec()),
    };

    if let BridgeOutcome::Routed(exit_code) =
        launch_in_vscode(&program, &args, cli.startup_json).await
    {
        return Ok(exit_code);
    }

    let options = local_viewer_options(cli);
    initialize_tracing();
    Ok(startup::run_local_viewer(options).await?.exit_code())
}

/// Terminal shims and the Python wrapper call
/// `dcmview --vscode-bridge-client <program> <args>...`. `program` only names
/// the VS Code tab; `args` are an ordinary dcmview command line.
fn split_bridge_client_values(mut values: Vec<String>) -> (String, Vec<String>) {
    let program = if values.is_empty() {
        "dcmview".to_string()
    } else {
        values.remove(0)
    };
    (program, values)
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
    use std::path::PathBuf;

    #[test]
    fn bridge_client_values_name_the_tab_and_forward_a_normal_command_line() {
        let cli = Cli::parse_from([
            "dcmview",
            "--vscode-bridge-client",
            "dcmview_py",
            "--startup-json",
            "--no-browser",
            "--port",
            "0",
            "/data/scan.dcm",
        ]);
        let (program, args) =
            split_bridge_client_values(cli.vscode_bridge_client.expect("hidden values"));
        let forwarded = Cli::parse_from(std::iter::once("dcmview".to_string()).chain(args.clone()));

        assert_eq!(program, "dcmview_py");
        assert_eq!(
            args,
            [
                "--startup-json",
                "--no-browser",
                "--port",
                "0",
                "/data/scan.dcm"
            ]
        );
        assert!(forwarded.startup_json);
        assert!(forwarded.no_browser);
        assert_eq!(forwarded.paths, vec![PathBuf::from("/data/scan.dcm")]);
    }
}

mod application;
mod bridge;
mod startup;

use clap::Parser;
use dcmview::loader;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "dcmview",
    version,
    about = "Start a temporary local DICOM inspection viewer",
    long_about = "Start a temporary local web server for inspecting DICOM files, directories, image frames, tags, and optional ROI annotations. dcmview is intended for research and development inspection, not clinical diagnosis.",
    after_long_help = "\
Examples:
  dcmview ./scan.dcm
  dcmview ./study_dir
  dcmview --no-recursive ./study_dir
  dcmview --no-browser --host 127.0.0.1 --port 8010 ./study_dir
  ssh -L 8010:127.0.0.1:8010 user@remote
  dcmview --annotations ./rois.csv ./study_dir
  dcmview --filter modality=CT --filter patient_id=phantom ./study_dir

For remote use, run dcmview on the machine that has the DICOM files, keep the
server bound to 127.0.0.1, and forward the chosen port over SSH."
)]
struct Cli {
    #[arg(
        value_name = "PATH",
        required_unless_present = "vscode_bridge_client",
        help = "DICOM file or directory to inspect; repeat for multiple inputs"
    )]
    paths: Vec<PathBuf>,

    #[arg(
        short = 'p',
        long = "port",
        value_name = "PORT",
        default_value_t = 0,
        help = "Local HTTP port to bind; 0 selects an available port"
    )]
    port: u16,

    #[arg(
        long = "host",
        value_name = "ADDR",
        default_value = "127.0.0.1",
        help = "Local interface to bind; keep 127.0.0.1 unless you understand the network exposure"
    )]
    host: String,

    #[arg(
        long = "no-browser",
        help = "Print the viewer URL instead of opening a browser automatically"
    )]
    no_browser: bool,

    #[arg(
        long = "timeout",
        value_name = "SECONDS",
        help = "Exit after this many seconds without API or browser requests once the scan has finished"
    )]
    timeout: Option<u64>,

    #[arg(
        long = "no-recursive",
        help = "Scan only the top level of input directories"
    )]
    no_recursive: bool,

    #[arg(
        long = "annotations",
        value_name = "CSV",
        help = "Load EMBED-style ROI annotations from CSV without modifying the file"
    )]
    annotations: Option<PathBuf>,

    #[arg(
        long = "filter",
        value_name = "FIELD=VALUE",
        value_parser = parse_scan_filter,
        help = "Include only files whose metadata field contains the value; repeatable"
    )]
    filters: Vec<loader::ScanFilter>,

    #[arg(
        long = "startup-json",
        hide = true,
        help = "Print machine-readable startup events for integrations"
    )]
    startup_json: bool,

    #[arg(
        long = "vscode-bridge-client",
        hide = true,
        num_args = 1..,
        allow_hyphen_values = true
    )]
    vscode_bridge_client: Option<Vec<String>>,
}

fn parse_scan_filter(raw: &str) -> std::result::Result<loader::ScanFilter, String> {
    raw.parse()
}

#[tokio::main]
async fn main() {
    let program_name = env::args().next().unwrap_or_else(|| "dcmview".to_string());
    let raw_args = env::args().skip(1).collect::<Vec<_>>();
    let cli = Cli::parse_from(std::iter::once(program_name).chain(raw_args.clone()));

    match application::run(cli, &raw_args).await {
        Ok(exit_code) => {
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Err(error) => {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_satisfies_clap_debug_assertions() {
        Cli::command().debug_assert();
    }
}

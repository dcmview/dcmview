from __future__ import annotations

import argparse
import re
import subprocess
import sys
from importlib import metadata
from pathlib import Path
from typing import Optional, Sequence

from .wrapper import view


def _package_version() -> str:
	try:
		return metadata.version("dcmview-py")
	except metadata.PackageNotFoundError:
		pyproject = Path(__file__).resolve().parents[2] / "pyproject.toml"
		if not pyproject.is_file():
			return "unknown"
		match = re.search(
			r'(?m)^\[project\]\s*(?:\n(?!\[).*)*?\nversion\s*=\s*"([^"]+)"',
			pyproject.read_text(encoding="utf-8"),
		)
		return match.group(1) if match else "unknown"


def _build_parser() -> argparse.ArgumentParser:
	parser = argparse.ArgumentParser(
		prog="python -m dcmview_py",
		description=(
			"Start a temporary local web server for inspecting DICOM files, "
			"directories, image frames, tags, and optional ROI annotations. "
			"dcmview is intended for research and development inspection, not "
			"clinical diagnosis."
		),
		formatter_class=argparse.RawDescriptionHelpFormatter,
		epilog="""\
Examples:
  python -m dcmview_py ./scan.dcm
  python -m dcmview_py ./study_dir
  python -m dcmview_py --no-recursive ./study_dir
  python -m dcmview_py --no-browser --host 127.0.0.1 --port 8010 ./study_dir
  ssh -L 8010:127.0.0.1:8010 user@remote
  python -m dcmview_py --annotations ./rois.csv ./study_dir
  python -m dcmview_py --filter Modality=CT --filter PatientID=phantom ./study_dir

For remote use, run dcmview on the machine that has the DICOM files, keep the
server bound to 127.0.0.1, and forward the chosen port over SSH.
""",
	)
	parser.add_argument("--version", action="version", version=f"dcmview {_package_version()}")
	parser.add_argument(
		"paths",
		metavar="PATH",
		nargs="+",
		help="DICOM file or directory to inspect; repeat for multiple inputs",
	)
	parser.add_argument(
		"-p",
		"--port",
		metavar="PORT",
		type=int,
		default=0,
		help="local HTTP port to bind; 0 selects an available port",
	)
	parser.add_argument(
		"--host",
		metavar="ADDR",
		default="127.0.0.1",
		help="local interface to bind; keep 127.0.0.1 unless you understand the network exposure",
	)
	parser.add_argument(
		"--no-browser",
		action="store_true",
		help="print the viewer URL instead of opening a browser automatically",
	)
	parser.add_argument(
		"--timeout",
		metavar="SECONDS",
		type=int,
		help="exit after this many seconds without API or browser requests",
	)
	parser.add_argument(
		"--no-recursive",
		action="store_true",
		help="scan only the top level of input directories",
	)
	parser.add_argument(
		"--annotations",
		metavar="CSV",
		help="load EMBED-style ROI annotations from CSV without modifying the file",
	)
	parser.add_argument(
		"--filter",
		metavar="FIELD=VALUE",
		action="append",
		default=[],
		help="include only files whose metadata field contains the value; repeatable",
	)
	return parser


def run_cli(argv: Optional[Sequence[str]] = None) -> int:
	parser = _build_parser()
	args = parser.parse_args(argv)

	try:
		view_kwargs = {
			"port": args.port,
			"host": args.host,
			"browser": not args.no_browser,
			"recursive": not args.no_recursive,
			"timeout": args.timeout,
			"annotations": args.annotations,
			"block": True,
		}
		if args.filter:
			view_kwargs["filters"] = args.filter
		view(args.paths, **view_kwargs)
	except subprocess.CalledProcessError as error:
		return int(error.returncode)
	except (RuntimeError, ValueError, TypeError) as error:
		print(str(error), file=sys.stderr)
		return 1

	return 0


def main() -> None:
	raise SystemExit(run_cli())


if __name__ == "__main__":
	main()

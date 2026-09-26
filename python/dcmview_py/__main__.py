"""Command-line entry point: forward arguments to the dcmview binary.

The binary owns the CLI (options, ``--help``, ``--version``) and the VS Code
routing rule, so this module only resolves the binary and relays its exit code.
"""

from __future__ import annotations

import subprocess
import sys
from typing import Optional, Sequence

from .wrapper import _BRIDGE_CLIENT_FLAG, _BRIDGE_PROGRAM, _resolve_binary


def run_cli(argv: Optional[Sequence[str]] = None) -> int:
	args = list(sys.argv[1:] if argv is None else argv)
	try:
		binary = _resolve_binary()
	except RuntimeError as error:
		print(f"dcmview: {error}", file=sys.stderr)
		return 1

	# The hidden bridge-client form applies the binary's VS Code routing rule
	# and names the VS Code tab after this package.
	process = subprocess.Popen([binary, _BRIDGE_CLIENT_FLAG, _BRIDGE_PROGRAM, *args])
	while True:
		try:
			return_code = process.wait()
			break
		except KeyboardInterrupt:
			# Ctrl+C reaches the binary too; let it shut down and report.
			continue
	if return_code < 0:
		return 128 - return_code
	return return_code


def main() -> None:
	raise SystemExit(run_cli())


if __name__ == "__main__":
	main()

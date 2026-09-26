from __future__ import annotations

import os
import json
import shutil
import signal
import subprocess
import sys
import threading
from pathlib import Path
from typing import Iterable, Optional, Union

_STARTUP_PREFIXES = ("dcmview: server running at ", "dcmview: opened in VS Code at ")
_STARTUP_EVENT_TYPES = ("server_started", "vscode_session_started")
_URL_WAIT_SECONDS = 5.0
_STOP_TIMEOUT_SECONDS = 5.0
_BINARY_ENV = "DCMVIEW_BINARY"
_VSCODE_BRIDGE_BYPASS_ENV = "DCMVIEW_VSCODE_BYPASS"
_BRIDGE_CLIENT_FLAG = "--vscode-bridge-client"
_BRIDGE_PROGRAM = "dcmview_py"

PathInput = Union[str, os.PathLike[str]]


class _OutputMonitor:
	def __init__(self, process: subprocess.Popen[str]) -> None:
		self._process = process
		self._url: Optional[str] = None
		self._url_lock = threading.Lock()
		self._integration_flags_unsupported = False
		self._integration_flags_unsupported_lock = threading.Lock()
		self._url_ready = threading.Event()
		self._thread = threading.Thread(target=self._run, name="dcmview-py-output", daemon=True)

	def start(self) -> None:
		self._thread.start()

	def join(self) -> None:
		self._thread.join()

	def wait_for_url(self, timeout: float) -> Optional[str]:
		self._url_ready.wait(timeout)
		return self.url

	@property
	def url(self) -> Optional[str]:
		with self._url_lock:
			return self._url

	@property
	def integration_flags_unsupported(self) -> bool:
		with self._integration_flags_unsupported_lock:
			return self._integration_flags_unsupported

	def _set_url(self, url: str) -> None:
		with self._url_lock:
			if self._url is None:
				self._url = url
				self._url_ready.set()

	def _set_integration_flags_unsupported(self) -> None:
		with self._integration_flags_unsupported_lock:
			self._integration_flags_unsupported = True

	def _run(self) -> None:
		stdout = self._process.stdout
		if stdout is None:
			self._url_ready.set()
			return

		try:
			for line in stdout:
				sys.stdout.write(line)
				sys.stdout.flush()
				url = _parse_startup_url(line)
				if url is not None:
					self._set_url(url)
				if _is_integration_flag_unsupported_line(line):
					self._set_integration_flags_unsupported()
		finally:
			stdout.close()
			self._url_ready.set()


class ShutdownHandle:
	"""Handle for a non-blocking dcmview launch, local or opened in VS Code.

	``stop()`` interrupts the dcmview process; for a VS Code-managed viewer the
	process asks VS Code to close the viewer before exiting.
	"""

	def __init__(self, process: subprocess.Popen[str], monitor: _OutputMonitor) -> None:
		self._process = process
		self._monitor = monitor

	@property
	def url(self) -> Optional[str]:
		return self._monitor.url

	def stop(self, timeout: float = _STOP_TIMEOUT_SECONDS) -> int:
		if self._process.poll() is not None:
			self._monitor.join()
			return int(self._process.returncode or 0)

		try:
			self._process.send_signal(_graceful_stop_signal())
		except (ProcessLookupError, ValueError):
			self._monitor.join()
			return int(self._process.returncode or 0)
		try:
			return_code = self._process.wait(timeout=timeout)
		except subprocess.TimeoutExpired:
			self._process.terminate()
			try:
				return_code = self._process.wait(timeout=timeout)
			except subprocess.TimeoutExpired:
				self._process.kill()
				return_code = self._process.wait(timeout=timeout)

		self._monitor.join()
		return int(return_code)

	def __enter__(self) -> ShutdownHandle:
		return self

	def __exit__(self, _exc_type, _exc, _tb) -> None:
		self.stop()


def view(
	files: PathInput | Iterable[PathInput],
	*,
	port: int = 0,
	host: str = "127.0.0.1",
	browser: bool = True,
	block: bool = True,
	recursive: bool = True,
	timeout: Optional[int] = None,
	annotations: Optional[PathInput] = None,
	filters: Optional[Iterable[str]] = None,
	vscode_bridge: bool = True,
) -> Optional[ShutdownHandle]:
	"""Launch dcmview for one or more DICOM files or directories.

	Args:
		files: A path-like value or iterable of path-like values to inspect.
		port: Local HTTP port to bind. ``0`` asks the OS for an available port.
		host: Local interface to bind. Keep ``"127.0.0.1"`` unless you have
			added your own network access controls.
		browser: Open the system browser when ``True``; pass ``False`` to print
			the viewer URL for terminals, notebooks, or SSH forwarding.
		block: Wait for the viewer process to exit when ``True``. When ``False``,
			return a handle with ``url``, ``stop()``, and context-manager support.
		recursive: Recursively scan input directories when ``True``.
		timeout: Exit after this many seconds without API or browser requests.
		annotations: Optional EMBED-style ROI annotation CSV to load in memory.
		filters: Optional iterable of ``FIELD=VALUE`` metadata filters. Filters
			are forwarded to the binary and combined with AND semantics.
		vscode_bridge: When ``True``, open the viewer in VS Code when the
			dcmview extension's bridge applies: inside a VS Code terminal, or
			when the working directory is inside an open workspace folder
			(this covers notebooks started from the workspace). Pass ``False``
			or set ``DCMVIEW_VSCODE_BYPASS=1`` to always launch locally.

	Returns:
		``None`` for blocking calls after dcmview exits successfully. For
		non-blocking calls, returns a ``ShutdownHandle`` for the local or
		VS Code-managed viewer.

	Raises:
		ValueError: If no files are provided.
		TypeError: If file, annotation, or filter arguments have invalid types.
		RuntimeError: If no dcmview binary can be resolved or startup fails.
		subprocess.CalledProcessError: If the underlying viewer exits with a
			non-zero status.

	The Python wrapper resolves the binary from ``DCMVIEW_BINARY``, then a
	bundled wheel binary, then ``PATH``. It is intended for research and
	development inspection, not clinical diagnosis.
	"""

	paths = _normalize_files(files)
	annotation_path = _normalize_optional_path(annotations, field_name="annotations")
	filter_args = _normalize_filters(filters)

	# Old binaries without the integration flags are retried with plain args.
	for integration_flags in (True, False):
		command = _build_command(
			paths,
			port=port,
			host=host,
			browser=browser,
			recursive=recursive,
			timeout=timeout,
			annotations=annotation_path,
			filters=filter_args,
			include_startup_json=integration_flags,
			vscode_bridge=vscode_bridge and integration_flags,
		)

		process = subprocess.Popen(command, **_popen_options(vscode_bridge=vscode_bridge))
		monitor = _OutputMonitor(process)
		monitor.start()

		if block:
			return_code = process.wait()
			monitor.join()
			if return_code != 0:
				if integration_flags and monitor.integration_flags_unsupported:
					continue
				raise subprocess.CalledProcessError(return_code, command)
			return None

		monitor.wait_for_url(_URL_WAIT_SECONDS)
		if process.poll() is not None and process.returncode not in (0, None):
			monitor.join()
			if integration_flags and monitor.integration_flags_unsupported:
				continue
			raise subprocess.CalledProcessError(int(process.returncode), command)

		return ShutdownHandle(process, monitor)

	raise RuntimeError("dcmview failed to start")


def _normalize_files(files: PathInput | Iterable[PathInput]) -> list[str]:
	if isinstance(files, (str, os.PathLike)):
		candidates: list[PathInput] = [files]
	else:
		candidates = list(files)

	if not candidates:
		raise ValueError("at least one file path is required")

	normalized: list[str] = []
	for candidate in candidates:
		if not isinstance(candidate, (str, os.PathLike)):
			raise TypeError("files must be path-like values")
		normalized.append(str(Path(candidate)))
	return normalized


def _normalize_optional_path(path: Optional[PathInput], *, field_name: str) -> Optional[str]:
	if path is None:
		return None
	if not isinstance(path, (str, os.PathLike)):
		raise TypeError(f"{field_name} must be a path-like value")
	return str(Path(path))


def _normalize_filters(filters: Optional[Iterable[str]]) -> list[str]:
	if filters is None:
		return []
	normalized: list[str] = []
	for value in filters:
		if not isinstance(value, str):
			raise TypeError("filters must contain FIELD=VALUE strings")
		normalized.append(value)
	return normalized


def _build_command(
	paths: list[str],
	*,
	port: int,
	host: str,
	browser: bool,
	recursive: bool,
	timeout: Optional[int],
	annotations: Optional[str],
	filters: Optional[Iterable[str]] = None,
	include_startup_json: bool = True,
	vscode_bridge: bool = False,
) -> list[str]:
	bridge_client = [_BRIDGE_CLIENT_FLAG, _BRIDGE_PROGRAM] if vscode_bridge else []
	return [
		_resolve_binary(),
		*bridge_client,
		*_build_args(
			paths,
			port=port,
			host=host,
			browser=browser,
			recursive=recursive,
			timeout=timeout,
			annotations=annotations,
			filters=filters,
			include_startup_json=include_startup_json,
		),
	]


def _build_args(
	paths: list[str],
	*,
	port: int,
	host: str,
	browser: bool,
	recursive: bool,
	timeout: Optional[int],
	annotations: Optional[str],
	filters: Optional[Iterable[str]] = None,
	include_startup_json: bool = True,
) -> list[str]:
	command = ["--port", str(port), "--host", host]
	if include_startup_json:
		command.append("--startup-json")
	if not browser:
		command.append("--no-browser")
	if timeout is not None:
		command.extend(["--timeout", str(timeout)])
	if not recursive:
		command.append("--no-recursive")
	if annotations is not None:
		command.extend(["--annotations", annotations])
	for filter_value in _normalize_filters(filters):
		command.extend(["--filter", filter_value])
	command.extend(paths)
	return command


def _parse_startup_url(line: str) -> Optional[str]:
	trimmed = line.strip()
	if trimmed.startswith("{"):
		try:
			event = json.loads(trimmed)
		except json.JSONDecodeError:
			return None
		if (
			isinstance(event, dict)
			and event.get("type") in _STARTUP_EVENT_TYPES
			and isinstance(event.get("url"), str)
			and event["url"]
		):
			return event["url"]
		return None

	for prefix in _STARTUP_PREFIXES:
		if trimmed.startswith(prefix):
			url = trimmed[len(prefix) :].strip()
			return url or None
	return None


def _is_integration_flag_unsupported_line(line: str) -> bool:
	normalized = line.lower()
	return ("--startup-json" in normalized or _BRIDGE_CLIENT_FLAG in normalized) and any(
		marker in normalized
		for marker in [
			"unexpected",
			"unrecognized",
			"unknown",
			"wasn't expected",
			"was not expected",
			"found argument",
		]
	)


def _resolve_binary() -> str:
	configured = os.environ.get(_BINARY_ENV)
	if configured:
		candidate = Path(configured).expanduser()
		if candidate.is_file():
			_ensure_executable(candidate)
			return str(candidate)
		raise RuntimeError(f"{_BINARY_ENV} points to a missing file: {candidate}")

	bundled = Path(__file__).resolve().parent / "bin" / _binary_name()
	if bundled.is_file():
		_ensure_executable(bundled)
		return str(bundled)

	path_binary = shutil.which(_binary_name())
	if path_binary is not None:
		return path_binary

	raise RuntimeError(
		"dcmview binary not found — install a bundled wheel or install the Rust binary separately"
	)


def _binary_name() -> str:
	return "dcmview.exe" if _is_windows() else "dcmview"


def _is_windows() -> bool:
	return os.name == "nt"


def _popen_options(*, vscode_bridge: bool = True) -> dict[str, object]:
	env = dict(os.environ)
	if not vscode_bridge:
		env[_VSCODE_BRIDGE_BYPASS_ENV] = "1"
	options: dict[str, object] = {
		"stdout": subprocess.PIPE,
		"stderr": subprocess.STDOUT,
		"text": True,
		"bufsize": 1,
		"env": env,
	}
	if _is_windows():
		options["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
	return options


def _graceful_stop_signal() -> signal.Signals | int:
	if _is_windows():
		return getattr(signal, "CTRL_BREAK_EVENT", signal.SIGTERM)
	return signal.SIGINT


def _ensure_executable(path: Path) -> None:
	if _is_windows() or os.access(path, os.X_OK):
		return

	mode = path.stat().st_mode
	exec_bits = (mode & 0o444) >> 2
	path.chmod(mode | exec_bits)

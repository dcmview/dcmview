from __future__ import annotations

import atexit
import os
import json
import shutil
import signal
import subprocess
import sys
import threading
import warnings
from collections import deque
from pathlib import Path
from typing import Iterable, Optional, Union

_STARTUP_EVENT_TYPES = ("server_started", "vscode_session_started")
# The binary announces its URL before discovery ends, then this event once it
# found files; a scan that finds none exits non-zero instead. In VS Code the
# extension's viewer does the scan, so the session event settles it.
_SCAN_SETTLED_EVENT_TYPES = ("scan_complete", "vscode_session_started")
# Returns as soon as the URL is printed; the budget covers a VS Code viewer,
# which the extension may take up to its 20 s startup timeout to report.
_URL_WAIT_SECONDS = 30.0
_SCAN_WAIT_SECONDS = 5.0
_OUTPUT_TAIL_LINES = 20
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
		self._url_ready = threading.Event()
		self._scan_settled = threading.Event()
		self._scan_succeeded = threading.Event()
		self._closed = threading.Event()
		self._tail: deque[str] = deque(maxlen=_OUTPUT_TAIL_LINES)
		self._tail_lock = threading.Lock()
		self._thread = threading.Thread(target=self._run, name="dcmview-py-output", daemon=True)
		self._stderr_thread = threading.Thread(
			target=self._relay_stderr, name="dcmview-py-stderr", daemon=True
		)

	def start(self) -> None:
		self._thread.start()
		self._stderr_thread.start()

	def join(self) -> None:
		self._thread.join()
		self._stderr_thread.join()

	def wait_for_url(self, timeout: float) -> Optional[str]:
		self._url_ready.wait(timeout)
		return self.url

	def wait_for_scan(self, timeout: float) -> bool:
		"""Wait until discovery found files or VS Code took the launch (``True``),
		or until output ended or ``timeout`` passed (``False``)."""
		self._scan_settled.wait(timeout)
		return self._scan_succeeded.is_set()

	@property
	def url(self) -> Optional[str]:
		with self._url_lock:
			return self._url

	def output_ended(self) -> bool:
		return self._closed.is_set()

	def tail(self) -> str:
		"""The last lines of output, for an error raised after the viewer exited."""
		with self._tail_lock:
			return "".join(self._tail)

	def _remember(self, line: str) -> None:
		with self._tail_lock:
			self._tail.append(line)

	def _set_url(self, url: str) -> None:
		with self._url_lock:
			if self._url is None:
				self._url = url
				self._url_ready.set()

	def _run(self) -> None:
		stdout = self._process.stdout
		if stdout is None:
			self._url_ready.set()
			return

		try:
			for line in stdout:
				sys.stdout.write(line)
				sys.stdout.flush()
				self._remember(line)
				event = _parse_event(line)
				url = _startup_url(event)
				if url is not None:
					self._set_url(url)
				if event is not None and event.get("type") in _SCAN_SETTLED_EVENT_TYPES:
					self._scan_succeeded.set()
					self._scan_settled.set()
		finally:
			stdout.close()
			self._closed.set()
			self._url_ready.set()
			self._scan_settled.set()

	def _relay_stderr(self) -> None:
		# Warnings on stderr quote file names and DICOM values, so a crafted
		# name could spell a startup event. They are echoed and kept for error
		# reports but never parsed: only stdout carries events.
		stderr = self._process.stderr
		if stderr is None:
			return
		try:
			for line in stderr:
				sys.stderr.write(line)
				sys.stderr.flush()
				self._remember(line)
		finally:
			stderr.close()


class ShutdownHandle:
	"""Handle for a non-blocking dcmview launch, local or opened in VS Code.

	``stop()`` interrupts the dcmview process; for a VS Code-managed viewer the
	process asks VS Code to close the viewer before exiting.
	"""

	def __init__(self, process: subprocess.Popen[str], monitor: _OutputMonitor) -> None:
		self._process = process
		self._monitor = monitor
		_LIVE_HANDLES.add(self)

	@property
	def url(self) -> Optional[str]:
		return self._monitor.url

	def stop(self, timeout: float = _STOP_TIMEOUT_SECONDS) -> int:
		_LIVE_HANDLES.discard(self)
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


# Viewers started without blocking are stopped when the interpreter exits, so a
# restarted kernel or finished script does not leave servers running. Handles
# are held strongly: a caller that dropped its handle still gets its viewer
# stopped.
_LIVE_HANDLES: set[ShutdownHandle] = set()


@atexit.register
def _stop_live_handles() -> None:
	for handle in list(_LIVE_HANDLES):
		try:
			handle.stop(timeout=2.0)
		except Exception:  # best effort while the interpreter shuts down
			pass


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
		timeout: Exit after this many seconds without API or browser requests
			once the scan has finished.
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
		RuntimeError: If no dcmview binary can be resolved.
		subprocess.CalledProcessError: If the underlying viewer exits with a
			non-zero status.

	The Python wrapper resolves the binary from ``DCMVIEW_BINARY``, then a
	bundled wheel binary, then ``PATH``. It is intended for research and
	development inspection, not clinical diagnosis.
	"""

	paths = _normalize_files(files)
	annotation_path = _normalize_optional_path(annotations, field_name="annotations")
	filter_args = _normalize_filters(filters)

	command = _build_command(
		paths,
		port=port,
		host=host,
		browser=browser,
		recursive=recursive,
		timeout=timeout,
		annotations=annotation_path,
		filters=filter_args,
		vscode_bridge=vscode_bridge,
	)

	process = subprocess.Popen(command, **_popen_options(vscode_bridge=vscode_bridge))
	monitor = _OutputMonitor(process)
	monitor.start()

	if block:
		try:
			return_code = process.wait()
		except KeyboardInterrupt:
			# Ctrl+C in a terminal reaches the viewer too, but a notebook
			# interrupt reaches only Python: stop the viewer before re-raising.
			ShutdownHandle(process, monitor).stop()
			raise
		monitor.join()
		if return_code != 0:
			raise subprocess.CalledProcessError(return_code, command, output=monitor.tail())
		return None

	url = monitor.wait_for_url(_URL_WAIT_SECONDS)
	scan_succeeded = monitor.wait_for_scan(_SCAN_WAIT_SECONDS)
	return_code = process.poll()
	if return_code is None and not scan_succeeded and monitor.output_ended():
		# Output ended before discovery found anything: the process is exiting.
		try:
			return_code = process.wait(timeout=_STOP_TIMEOUT_SECONDS)
		except subprocess.TimeoutExpired:
			return_code = None
	if return_code not in (0, None):
		monitor.join()
		raise subprocess.CalledProcessError(int(return_code), command, output=monitor.tail())
	if url is None:
		warnings.warn(
			f"dcmview did not report its URL within {_URL_WAIT_SECONDS:.0f} s; "
			"handle.url stays None until it does",
			RuntimeWarning,
			stacklevel=2,
		)

	return ShutdownHandle(process, monitor)


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
) -> list[str]:
	command = ["--port", str(port), "--host", host, "--startup-json"]
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


def _parse_event(line: str) -> Optional[dict]:
	trimmed = line.strip()
	if not trimmed.startswith("{"):
		return None
	try:
		event = json.loads(trimmed)
	except json.JSONDecodeError:
		return None
	return event if isinstance(event, dict) else None


def _startup_url(event: Optional[dict]) -> Optional[str]:
	if (
		event is not None
		and event.get("type") in _STARTUP_EVENT_TYPES
		and isinstance(event.get("url"), str)
		and event["url"]
	):
		return event["url"]
	return None


def _parse_startup_url(line: str) -> Optional[str]:
	return _startup_url(_parse_event(line))


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
		"stderr": subprocess.PIPE,
		"text": True,
		"encoding": "utf-8",
		"errors": "replace",
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

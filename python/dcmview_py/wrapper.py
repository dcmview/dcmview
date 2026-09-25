from __future__ import annotations

import os
import json
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Iterable, Optional, Union

_STARTUP_PREFIX = "dcmview: server running at "
_STARTUP_EVENT_TYPE = "server_started"
_URL_WAIT_SECONDS = 5.0
_STOP_TIMEOUT_SECONDS = 5.0
_BINARY_ENV = "DCMVIEW_BINARY"
_VSCODE_BRIDGE_URL_ENV = "DCMVIEW_VSCODE_BRIDGE_URL"
_VSCODE_BRIDGE_TOKEN_ENV = "DCMVIEW_VSCODE_BRIDGE_TOKEN"
_VSCODE_BRIDGE_BYPASS_ENV = "DCMVIEW_VSCODE_BYPASS"
_VSCODE_BRIDGE_REGISTRY_DIR_ENV = "DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR"
_VSCODE_BRIDGE_DEBUG_ENV = "DCMVIEW_VSCODE_BRIDGE_DEBUG"
_BRIDGE_REGISTRY_MAX_AGE_SECONDS = 3 * 60 * 60

PathInput = Union[str, os.PathLike[str]]


class _OutputMonitor:
	def __init__(self, process: subprocess.Popen[str]) -> None:
		self._process = process
		self._url: Optional[str] = None
		self._url_lock = threading.Lock()
		self._startup_json_unsupported = False
		self._startup_json_unsupported_lock = threading.Lock()
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
	def startup_json_unsupported(self) -> bool:
		with self._startup_json_unsupported_lock:
			return self._startup_json_unsupported

	def _set_url(self, url: str) -> None:
		with self._url_lock:
			if self._url is None:
				self._url = url
				self._url_ready.set()

	def _set_startup_json_unsupported(self) -> None:
		with self._startup_json_unsupported_lock:
			self._startup_json_unsupported = True

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
				if _is_startup_json_unsupported_line(line):
					self._set_startup_json_unsupported()
		finally:
			stdout.close()
			self._url_ready.set()


class ShutdownHandle:
	"""Handle for controlling a non-blocking dcmview subprocess."""

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


BridgeEndpoint = tuple[str, str]


class _BridgeSessionWaitError(RuntimeError):
	pass


class BridgeShutdownHandle:
	"""Handle for controlling a VS Code-managed dcmview session."""

	def __init__(self, session_id: str, url: str, endpoint: BridgeEndpoint) -> None:
		self._session_id = session_id
		self._url = url
		self._endpoint = endpoint

	@property
	def url(self) -> Optional[str]:
		return self._url

	def stop(self, timeout: float = _STOP_TIMEOUT_SECONDS) -> int:
		_bridge_json_request("POST", f"/sessions/{self._session_id}/stop", endpoint=self._endpoint, timeout=timeout)
		response = _bridge_json_request("GET", f"/sessions/{self._session_id}/wait", endpoint=self._endpoint, timeout=timeout)
		return int(response.get("exitCode") or 0)

	def __enter__(self) -> BridgeShutdownHandle:
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
) -> Optional[ShutdownHandle | BridgeShutdownHandle]:
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
		vscode_bridge: When ``True``, route through an active dcmview VS Code
			bridge when one is available. Pass ``False`` or set
			``DCMVIEW_VSCODE_BYPASS=1`` to force a local subprocess launch.

	Returns:
		``None`` for blocking calls after dcmview exits successfully. For
		non-blocking calls, returns a ``ShutdownHandle`` for local subprocesses
		or ``BridgeShutdownHandle`` for VS Code-managed sessions.

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
	args = _build_args(
		paths,
		port=port,
		host=host,
		browser=browser,
		recursive=recursive,
		timeout=timeout,
		annotations=annotation_path,
		filters=filter_args,
	)
	bridge_endpoints = _bridge_endpoints() if vscode_bridge else []
	if bridge_endpoints:
		try:
			return _view_via_vscode_bridge(args, block=block, endpoints=bridge_endpoints)
		except (RuntimeError, OSError, urllib.error.URLError) as error:
			if isinstance(error, _BridgeSessionWaitError):
				raise
			print(
				f"dcmview: VS Code bridge unavailable ({error}); falling back to local viewer",
				file=sys.stderr,
			)

	for include_startup_json in (True, False):
		command = _build_command(
			paths,
			port=port,
			host=host,
			browser=browser,
			recursive=recursive,
			timeout=timeout,
			annotations=annotation_path,
			filters=filter_args,
			include_startup_json=include_startup_json,
		)

		process = subprocess.Popen(command, **_popen_options())
		monitor = _OutputMonitor(process)
		monitor.start()

		if block:
			return_code = process.wait()
			monitor.join()
			if return_code != 0:
				if include_startup_json and monitor.startup_json_unsupported:
					continue
				raise subprocess.CalledProcessError(return_code, command)
			return None

		monitor.wait_for_url(_URL_WAIT_SECONDS)
		if process.poll() is not None and process.returncode not in (0, None):
			monitor.join()
			if include_startup_json and monitor.startup_json_unsupported:
				continue
			raise subprocess.CalledProcessError(int(process.returncode), command)

		return ShutdownHandle(process, monitor)

	raise RuntimeError("dcmview failed to start")


def _view_via_vscode_bridge(
	args: list[str],
	*,
	block: bool,
	endpoints: list[BridgeEndpoint],
) -> Optional[BridgeShutdownHandle]:
	payload = {
		"program": "dcmview_py",
		"args": args,
		"cwd": os.getcwd(),
		"wait": False,
	}
	try:
		payload["binaryPath"] = _resolve_binary()
	except RuntimeError:
		_bridge_debug("omitting bridge binaryPath because local binary resolution failed")
	response: dict[str, object] | None = None
	endpoint: BridgeEndpoint | None = None
	last_error: BaseException | None = None
	registry_endpoints = set(_bridge_registry_endpoints(os.getcwd()))
	for candidate in endpoints:
		try:
			_bridge_debug(f"trying endpoint {candidate[0]}")
			response = _bridge_json_request("POST", "/launch", payload, endpoint=candidate)
			endpoint = candidate
			_bridge_debug(f"selected endpoint {candidate[0]}")
			break
		except (OSError, urllib.error.URLError, RuntimeError) as error:
			if isinstance(error, urllib.error.URLError) and candidate in registry_endpoints:
				_remove_bridge_registry_endpoint(candidate)
			_bridge_debug(f"endpoint {candidate[0]} failed: {error}")
			last_error = error
	if response is None or endpoint is None:
		if last_error is not None:
			raise last_error
		raise RuntimeError("VS Code bridge unavailable")
	session_id = str(response["sessionId"])
	url = str(response["url"])
	print(f"dcmview: opened in VS Code at {url}")

	if not block:
		return BridgeShutdownHandle(session_id, url, endpoint)

	try:
		wait_response = _bridge_json_request(
			"GET",
			f"/sessions/{session_id}/wait",
			endpoint=endpoint,
			timeout=None,
		)
	except (RuntimeError, OSError, urllib.error.URLError) as error:
		raise _BridgeSessionWaitError(f"VS Code bridge session was captured but wait failed: {error}") from error
	exit_code = int(wait_response.get("exitCode") or 0)
	if exit_code != 0:
		raise subprocess.CalledProcessError(exit_code, ["dcmview-vscode-bridge", *args])
	return None


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
) -> list[str]:
	return [
		_resolve_binary(),
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
			and event.get("type") == _STARTUP_EVENT_TYPE
			and isinstance(event.get("url"), str)
			and event["url"]
		):
			return event["url"]
		return None

	if trimmed.startswith(_STARTUP_PREFIX):
		url = trimmed[len(_STARTUP_PREFIX) :].strip()
		return url or None
	return None


def _is_startup_json_unsupported_line(line: str) -> bool:
	normalized = line.lower()
	return "--startup-json" in normalized and any(
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


def _bridge_available() -> bool:
	return bool(_bridge_endpoints())


def _bridge_endpoints() -> list[BridgeEndpoint]:
	if os.environ.get(_VSCODE_BRIDGE_BYPASS_ENV) == "1":
		_bridge_debug("bridge discovery bypassed by DCMVIEW_VSCODE_BYPASS=1")
		return []

	endpoints: list[BridgeEndpoint] = []
	seen: set[BridgeEndpoint] = set()
	url = os.environ.get(_VSCODE_BRIDGE_URL_ENV)
	token = os.environ.get(_VSCODE_BRIDGE_TOKEN_ENV)
	if url and token:
		endpoint = (url, token)
		endpoints.append(endpoint)
		seen.add(endpoint)
		_bridge_debug(f"accepted env endpoint {url}")

	for endpoint in _bridge_registry_endpoints(os.getcwd()):
		if endpoint not in seen:
			endpoints.append(endpoint)
			seen.add(endpoint)
	_bridge_debug(f"discovered {len(endpoints)} bridge endpoint(s)")
	return endpoints


def _bridge_registry_endpoints(cwd: str, *, now_ms: Optional[int] = None) -> list[BridgeEndpoint]:
	endpoints: list[BridgeEndpoint] = []
	seen: set[BridgeEndpoint] = set()
	for registry_dir in _bridge_registry_dirs():
		for endpoint in _bridge_registry_endpoints_from_dir(Path(registry_dir), cwd, now_ms=now_ms):
			if endpoint not in seen:
				endpoints.append(endpoint)
				seen.add(endpoint)
	return endpoints


def _bridge_registry_endpoints_from_dir(
	registry_dir: Path,
	cwd: str,
	*,
	now_ms: Optional[int] = None,
) -> list[BridgeEndpoint]:
	_bridge_debug(f"scanning bridge registry dir {registry_dir}")
	if not _registry_dir_is_trusted(registry_dir):
		_bridge_debug(f"registry dir untrusted or missing: {registry_dir}")
		return []
	try:
		paths = list(registry_dir.glob("*.json"))
	except OSError:
		_bridge_debug(f"registry dir unreadable: {registry_dir}")
		return []

	if now_ms is None:
		now_ms = int(time.time() * 1000)
	cwd_path = _normalized_path(cwd)
	candidates: list[tuple[int, int, BridgeEndpoint]] = []
	for path in paths:
		try:
			entry = json.loads(path.read_text(encoding="utf-8"))
		except (OSError, json.JSONDecodeError):
			_bridge_debug(f"registry entry skipped unreadable: {path}")
			continue
		if not isinstance(entry, dict):
			_bridge_debug(f"registry entry skipped non-object: {path}")
			continue
		version = entry.get("version")
		if version is not None and (
			not isinstance(version, int) or isinstance(version, bool) or version != 1
		):
			_bridge_debug(f"registry entry skipped unsupported version: {path}")
			continue
		url = entry.get("bridgeUrl")
		token = entry.get("token")
		if not isinstance(url, str) or not url or not isinstance(token, str) or not token:
			_bridge_debug(f"registry entry skipped missing endpoint: {path}")
			continue
		created_at = entry.get("createdAtMs")
		if not isinstance(created_at, int) or isinstance(created_at, bool):
			_remove_registry_file(path)
			_bridge_debug(f"registry entry removed malformed timestamp: {path}")
			continue
		if _is_expired_registry_entry(created_at, now_ms):
			_remove_registry_file(path)
			_bridge_debug(f"registry entry removed expired: {path}")
			continue
		workspace_roots = entry.get("workspaceRoots")
		if not isinstance(workspace_roots, list):
			workspace_roots = []
		match_score = _workspace_match_score(cwd_path, workspace_roots)
		_bridge_debug(f"registry entry accepted: {path}")
		candidates.append((match_score, created_at, (url, token)))

	candidates.sort(key=lambda candidate: (candidate[0], candidate[1]), reverse=True)
	endpoints: list[BridgeEndpoint] = []
	seen: set[BridgeEndpoint] = set()
	for _, _, endpoint in candidates:
		if endpoint not in seen:
			endpoints.append(endpoint)
			seen.add(endpoint)
	return endpoints


def _registry_dir_is_trusted(
	path: Path,
	*,
	stat_result: Optional[os.stat_result] = None,
	euid: Optional[int] = None,
) -> bool:
	if _is_windows():
		return True
	if euid is None:
		if not hasattr(os, "geteuid"):
			return True
		euid = os.geteuid()
	try:
		stat_result = path.stat() if stat_result is None else stat_result
	except OSError:
		return False
	uid = getattr(stat_result, "st_uid", euid)
	mode = getattr(stat_result, "st_mode", 0)
	return uid == euid and mode & 0o022 == 0


def _is_expired_registry_entry(created_at_ms: int, now_ms: int) -> bool:
	max_age_ms = _BRIDGE_REGISTRY_MAX_AGE_SECONDS * 1000
	return created_at_ms <= 0 or created_at_ms > now_ms + max_age_ms or now_ms - created_at_ms > max_age_ms


def _remove_registry_file(path: Path) -> None:
	try:
		path.unlink()
	except OSError:
		pass


def _remove_bridge_registry_endpoint(endpoint: BridgeEndpoint) -> None:
	for registry_dir_name in _bridge_registry_dirs():
		_remove_bridge_registry_endpoint_from_dir(endpoint, Path(registry_dir_name))


def _remove_bridge_registry_endpoint_from_dir(endpoint: BridgeEndpoint, registry_dir: Path) -> None:
	if not _registry_dir_is_trusted(registry_dir):
		return
	try:
		paths = list(registry_dir.glob("*.json"))
	except OSError:
		return
	for path in paths:
		try:
			entry = json.loads(path.read_text(encoding="utf-8"))
		except (OSError, json.JSONDecodeError):
			continue
		if (
			isinstance(entry, dict)
			and entry.get("bridgeUrl") == endpoint[0]
			and entry.get("token") == endpoint[1]
		):
			_remove_registry_file(path)


def _bridge_registry_dir() -> str:
	return _bridge_registry_dirs()[0]


def _bridge_registry_dirs() -> list[str]:
	configured = os.environ.get(_VSCODE_BRIDGE_REGISTRY_DIR_ENV)
	if configured:
		return [configured]

	dirs = [_canonical_bridge_registry_dir()]

	runtime_dir = os.environ.get("XDG_RUNTIME_DIR")
	if runtime_dir and os.path.isabs(runtime_dir):
		dirs.append(_join_registry_path(runtime_dir, "dcmview", "vscode-bridges"))

	user = os.environ.get("USER") or os.environ.get("USERNAME") or "default"
	dirs.append(_join_registry_path(
		tempfile.gettempdir(),
		f"dcmview-vscode-bridges-{_safe_registry_segment(user)}",
	))
	return _dedupe_strings(dirs)


def _canonical_bridge_registry_dir() -> str:
	state_home = os.environ.get("XDG_STATE_HOME")
	if state_home and os.path.isabs(state_home):
		return _join_registry_path(state_home, "dcmview", "vscode-bridges")
	home = os.environ.get("HOME")
	if home and os.path.isabs(home):
		return _join_registry_path(home, ".local", "state", "dcmview", "vscode-bridges")
	user_profile = os.environ.get("USERPROFILE")
	if user_profile and os.path.isabs(user_profile):
		return _join_registry_path(user_profile, ".local", "state", "dcmview", "vscode-bridges")
	try:
		home = str(Path.home())
	except RuntimeError:
		home = "."
	if not os.path.isabs(home):
		home = "."
	return _join_registry_path(home, ".local", "state", "dcmview", "vscode-bridges")


def _dedupe_strings(values: Iterable[str]) -> list[str]:
	seen: set[str] = set()
	result: list[str] = []
	for value in values:
		if value not in seen:
			result.append(value)
			seen.add(value)
	return result


def _bridge_debug(message: str) -> None:
	if os.environ.get(_VSCODE_BRIDGE_DEBUG_ENV) == "1":
		print(f"dcmview bridge: {message}", file=sys.stderr)


def _join_registry_path(base: str, *segments: str) -> str:
	if "/" in base and "\\" not in base:
		parts = [base.rstrip("/"), *(segment.strip("/") for segment in segments)]
		return "/".join(part for part in parts if part)
	return str(Path(base).joinpath(*segments))


def _safe_registry_segment(value: str) -> str:
	return "".join(character if character.isalnum() or character in "_.-" else "_" for character in value)


def _normalized_path(value: str) -> Path:
	return Path(value).expanduser().resolve(strict=False)


def _workspace_match_score(cwd: Path, workspace_roots: list[object]) -> int:
	best = 0
	for root in workspace_roots:
		if not isinstance(root, str) or not root:
			continue
		root_path = _normalized_path(root)
		try:
			cwd.relative_to(root_path)
		except ValueError:
			continue
		best = max(best, len(str(root_path)))
	return best


def _bridge_json_request(
	method: str,
	path: str,
	payload: Optional[dict[str, object]] = None,
	*,
	endpoint: Optional[BridgeEndpoint] = None,
	timeout: Optional[float] = _STOP_TIMEOUT_SECONDS,
) -> dict[str, object]:
	if endpoint is None:
		endpoints = _bridge_endpoints()
		if not endpoints:
			raise RuntimeError("VS Code bridge unavailable")
		endpoint = endpoints[0]
	base_url = endpoint[0].rstrip("/")
	token = endpoint[1]
	body = None if payload is None else json.dumps(payload).encode("utf-8")
	request = urllib.request.Request(
		f"{base_url}{path}",
		data=body,
		method=method,
		headers={
			"Authorization": f"Bearer {token}",
			"Content-Type": "application/json",
		},
	)
	try:
		with urllib.request.urlopen(request, timeout=timeout) as response:
			return json.loads(response.read().decode("utf-8"))
	except urllib.error.HTTPError as error:
		raise RuntimeError(_bridge_http_error_message(error)) from error


def _bridge_http_error_message(error: urllib.error.HTTPError) -> str:
	detail = ""
	try:
		body = error.read().decode("utf-8")
		parsed = json.loads(body)
		if isinstance(parsed, dict) and isinstance(parsed.get("error"), str):
			detail = f": {parsed['error']}"
	except (OSError, UnicodeDecodeError, json.JSONDecodeError):
		pass
	return f"VS Code bridge returned HTTP {error.code}{detail}"


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


def _popen_options() -> dict[str, object]:
	env = dict(os.environ)
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

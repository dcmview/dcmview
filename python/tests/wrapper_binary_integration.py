from __future__ import annotations

import json
import os
import queue
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
import urllib.parse
import urllib.request
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Callable, Iterator, Optional
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parents[2]
PYTHON_SRC = REPO_ROOT / "python"
if str(PYTHON_SRC) not in sys.path:
	sys.path.insert(0, str(PYTHON_SRC))

from dcmview_py import wrapper

FIXTURE_FILE = REPO_ROOT / "tests" / "fixtures" / "golden-uncompressed-u16-multiframe.dcm"
OTHER_PATIENT_FILE = REPO_ROOT / "tests" / "fixtures" / "golden-jpeg-lossless-u16-single-frame.dcm"
BRIDGE_TOKEN = "integration-token"
VSCODE_VIEWER_URL = "http://127.0.0.1:9/vscode-viewer"
CLI_URL_PREFIXES = ("dcmview: server running at ", "dcmview: opened in VS Code at ")
BRIDGE_ENV_KEYS = (
	"DCMVIEW_VSCODE_BRIDGE_URL",
	"DCMVIEW_VSCODE_BRIDGE_TOKEN",
	"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR",
	"DCMVIEW_VSCODE_BYPASS",
)


def api_request(url: str, path: str) -> urllib.request.Request:
	base_url, fragment = urllib.parse.urldefrag(url)
	token = urllib.parse.parse_qs(fragment).get("token", [""])[0]
	return urllib.request.Request(
		f"{base_url.rstrip('/')}{path}",
		headers={"Authorization": f"Bearer {token}"},
	)


class FakeBridge:
	"""A stand-in for the VS Code extension's bridge HTTP server."""

	def __init__(self) -> None:
		self.launches: list[dict[str, object]] = []
		self.stopped = threading.Event()
		bridge = self

		class Handler(BaseHTTPRequestHandler):
			def log_message(self, *_args: object) -> None:
				pass

			def _reply(self, body: dict[str, object], status: int = 200) -> None:
				payload = json.dumps(body).encode("utf-8")
				self.send_response(status)
				self.send_header("Content-Type", "application/json")
				self.send_header("Content-Length", str(len(payload)))
				self.end_headers()
				self.wfile.write(payload)

			def _authorized(self) -> bool:
				if self.headers.get("Authorization") == f"Bearer {BRIDGE_TOKEN}":
					return True
				# The extension's reply, which the binary probes for.
				self._reply({"error": "unauthorized"}, status=401)
				return False

			def do_POST(self) -> None:
				if not self._authorized():
					return
				length = int(self.headers.get("Content-Length") or 0)
				body = json.loads(self.rfile.read(length) or b"{}")
				if self.path == "/launch":
					bridge.launches.append(body)
					self._reply({"sessionId": "session-1", "url": VSCODE_VIEWER_URL})
				elif self.path == "/sessions/session-1/stop":
					bridge.stopped.set()
					self._reply({"ok": True})

			def do_GET(self) -> None:
				if self._authorized() and self.path == "/sessions/session-1/wait":
					bridge.stopped.wait(30)
					self._reply({"exitCode": 0})

		self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
		self.url = f"http://127.0.0.1:{self._server.server_address[1]}"
		self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
		self._thread.start()

	def close(self) -> None:
		self.stopped.set()
		self._server.shutdown()
		self._server.server_close()


class WrapperBinaryIntegrationTests(unittest.TestCase):
	@classmethod
	def setUpClass(cls) -> None:
		if not FIXTURE_FILE.is_file():
			raise AssertionError(f"committed DICOM fixture is missing: {FIXTURE_FILE}")

		cls.binary = REPO_ROOT / "target" / "debug" / wrapper._binary_name()
		if not cls.binary.is_file():
			raise AssertionError(
				f"dcmview binary is missing: {cls.binary}; "
				"run `python scripts/check.py python-integration`"
			)

	@contextmanager
	def bridge_scenario(
		self,
		*,
		terminal_env: bool,
		workspace_contains_cwd: bool,
	) -> Iterator[FakeBridge]:
		"""Run with a fake bridge, a private registry, and a chosen cwd."""
		bridge = FakeBridge()
		with tempfile.TemporaryDirectory() as root:
			root_path = Path(root).resolve()
			registry = root_path / "registry"
			workspace = root_path / "workspace"
			outside = root_path / "outside"
			for directory in (registry, workspace, outside):
				directory.mkdir(mode=0o700)
			(registry / "bridge.json").write_text(
				json.dumps(
					{
						"version": 1,
						"bridgeUrl": bridge.url,
						"token": BRIDGE_TOKEN,
						"workspaceRoots": [str(workspace)],
						"createdAtMs": int(time.time() * 1000),
					}
				),
				encoding="utf-8",
			)
			environment = {
				key: value for key, value in os.environ.items() if key not in BRIDGE_ENV_KEYS
			}
			environment["DCMVIEW_BINARY"] = str(self.binary)
			environment["DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR"] = str(registry)
			if terminal_env:
				environment["DCMVIEW_VSCODE_BRIDGE_URL"] = bridge.url
				environment["DCMVIEW_VSCODE_BRIDGE_TOKEN"] = BRIDGE_TOKEN
			previous_cwd = os.getcwd()
			os.chdir(workspace if workspace_contains_cwd else outside)
			try:
				with mock.patch.dict(os.environ, environment, clear=True):
					yield bridge
			finally:
				os.chdir(previous_cwd)
				bridge.close()

	def wait_for_url(self, handle: wrapper.ShutdownHandle) -> Optional[str]:
		deadline = time.time() + 10.0
		while handle.url is None and time.time() < deadline:
			time.sleep(0.1)
		return handle.url

	def run_module_cli_until_ctrl_c(self, *args: str) -> tuple[Optional[str], int]:
		"""Run ``python -m dcmview_py`` in its own process group, wait for its
		URL, then send Ctrl+C to the whole group as a terminal does."""
		environment = dict(os.environ)
		environment["DCMVIEW_BINARY"] = str(self.binary)
		environment["PYTHONPATH"] = os.pathsep.join(
			[str(PYTHON_SRC), *filter(None, [environment.get("PYTHONPATH")])]
		)
		process = subprocess.Popen(
			[sys.executable, "-m", "dcmview_py", *args],
			stdout=subprocess.PIPE,
			stderr=subprocess.STDOUT,
			text=True,
			env=environment,
			start_new_session=True,
		)
		lines: queue.Queue[str] = queue.Queue()

		def read() -> None:
			assert process.stdout is not None
			for line in process.stdout:
				lines.put(line)

		reader = threading.Thread(target=read, daemon=True)
		reader.start()
		url: Optional[str] = None
		deadline = time.time() + 10.0
		while url is None and time.time() < deadline:
			try:
				line = lines.get(timeout=0.1).strip()
			except queue.Empty:
				continue
			for prefix in CLI_URL_PREFIXES:
				if line.startswith(prefix):
					url = line[len(prefix) :]

		if url is not None and url != VSCODE_VIEWER_URL:
			# The local viewer prints its URL before it installs its Ctrl+C
			# handler; one served request shows the handler is in place.
			urllib.request.urlopen(api_request(url, "/api/health"), timeout=10).close()
		os.killpg(process.pid, signal.SIGINT)
		try:
			exit_code = process.wait(timeout=10)
		except subprocess.TimeoutExpired:
			process.kill()
			exit_code = process.wait()
		reader.join()
		assert process.stdout is not None
		process.stdout.close()
		return url, exit_code

	def test_module_cli_help_and_version_come_from_the_binary(self) -> None:
		environment = {**os.environ, "DCMVIEW_BINARY": str(self.binary), "PYTHONPATH": str(PYTHON_SRC)}
		for flag in ("--help", "--version"):
			with self.subTest(flag=flag):
				expected = subprocess.run(
					[str(self.binary), flag], capture_output=True, text=True, check=True
				)
				actual = subprocess.run(
					[sys.executable, "-m", "dcmview_py", flag],
					capture_output=True,
					text=True,
					env=environment,
				)
				self.assertEqual(actual.returncode, 0)
				self.assertEqual(actual.stdout, expected.stdout)

	@unittest.skipIf(os.name == "nt", "process-group Ctrl+C is POSIX-only")
	def test_module_cli_runs_locally_and_stops_on_ctrl_c(self) -> None:
		with self.bridge_scenario(terminal_env=False, workspace_contains_cwd=False) as bridge:
			url, exit_code = self.run_module_cli_until_ctrl_c(
				"--no-browser", "--timeout", "30", str(FIXTURE_FILE)
			)

			self.assertIsNotNone(url)
			self.assertNotEqual(url, VSCODE_VIEWER_URL)
			self.assertEqual(exit_code, 0)
			self.assertEqual(bridge.launches, [])

	@unittest.skipIf(os.name == "nt", "process-group Ctrl+C is POSIX-only")
	def test_module_cli_in_vscode_terminal_opens_in_vscode_and_closes_on_ctrl_c(self) -> None:
		with self.bridge_scenario(terminal_env=True, workspace_contains_cwd=False) as bridge:
			url, exit_code = self.run_module_cli_until_ctrl_c("--no-browser", str(FIXTURE_FILE))

			self.assertEqual(url, VSCODE_VIEWER_URL)
			self.assertEqual(exit_code, 0)
			self.assertTrue(bridge.stopped.is_set())
			self.assertEqual(bridge.launches[0]["program"], "dcmview_py")

	def test_vscode_terminal_launch_opens_in_vscode_and_stops_there(self) -> None:
		with self.bridge_scenario(terminal_env=True, workspace_contains_cwd=False) as bridge:
			handle = wrapper.view([FIXTURE_FILE], browser=False, block=False)
			assert handle is not None
			try:
				self.assertEqual(self.wait_for_url(handle), VSCODE_VIEWER_URL)
			finally:
				exit_code = handle.stop()

			self.assertEqual(exit_code, 0)
			self.assertTrue(bridge.stopped.is_set())
			self.assertEqual(len(bridge.launches), 1)
			self.assertEqual(bridge.launches[0]["program"], "dcmview_py")
			self.assertIn(str(FIXTURE_FILE), bridge.launches[0]["args"])

	def test_notebook_inside_workspace_opens_in_vscode_through_registry(self) -> None:
		with self.bridge_scenario(terminal_env=False, workspace_contains_cwd=True) as bridge:
			handle = wrapper.view([FIXTURE_FILE], browser=False, block=False)
			assert handle is not None
			try:
				self.assertEqual(self.wait_for_url(handle), VSCODE_VIEWER_URL)
			finally:
				handle.stop()

			self.assertEqual(len(bridge.launches), 1)

	def test_launch_outside_vscode_and_workspaces_runs_locally(self) -> None:
		with self.bridge_scenario(terminal_env=False, workspace_contains_cwd=False) as bridge:
			handle = wrapper.view([FIXTURE_FILE], browser=False, timeout=30, block=False)
			assert handle is not None
			try:
				url = self.wait_for_url(handle)
			finally:
				handle.stop()

			self.assertIsNotNone(url)
			self.assertNotEqual(url, VSCODE_VIEWER_URL)
			self.assertEqual(bridge.launches, [])

	def test_every_view_keyword_reaches_the_binary_and_takes_effect(self) -> None:
		"""Each flag view() builds must be accepted by the real clap CLI.

		The launch goes through the bridge-client form (outside VS Code, so it
		runs locally), and the served catalog shows the scan options applied.
		"""
		with tempfile.TemporaryDirectory() as root:
			study = Path(root).resolve() / "study"
			(study / "nested").mkdir(parents=True)
			kept = study / "kept.dcm"
			kept.write_bytes(FIXTURE_FILE.read_bytes())
			(study / "filtered-out.dcm").write_bytes(OTHER_PATIENT_FILE.read_bytes())
			(study / "nested" / "not-scanned.dcm").write_bytes(FIXTURE_FILE.read_bytes())
			annotations = Path(root).resolve() / "rois.csv"
			annotations.write_text(
				f'anon_dicom_path,ROI_coords\n{kept},"[[0, 0, 2, 2]]"\n', encoding="utf-8"
			)

			with self.bridge_scenario(terminal_env=False, workspace_contains_cwd=False) as bridge:
				handle = wrapper.view(
					study,
					port=0,
					host="127.0.0.1",
					browser=False,
					block=False,
					recursive=False,
					timeout=30,
					annotations=annotations,
					filters=["patient_id=GOLDEN-UNCOMP"],
				)
				assert handle is not None
				try:
					url = self.wait_for_url(handle)
					assert url is not None
					self.assertTrue(url.startswith("http://127.0.0.1:"))
					catalog = self.wait_for_json(url, "/api/files", lambda body: body["scan_complete"])
					self.assertEqual([entry["path"] for entry in catalog["files"]], [str(kept)])
					index = catalog["files"][0]["index"]
					rois = self.wait_for_json(
						url, f"/api/file/{index}/annotations", lambda body: body["num_roi"] > 0
					)
					self.assertEqual(rois["roi_coords"], [[0, 0, 2, 2]])
				finally:
					exit_code = handle.stop()

				self.assertEqual(exit_code, 0)
				self.assertEqual(bridge.launches, [])

	def wait_for_json(self, url: str, path: str, ready: Callable[[dict], bool]) -> dict:
		deadline = time.time() + 10.0
		while True:
			with urllib.request.urlopen(api_request(url, path), timeout=10) as response:
				body = json.loads(response.read())
			if ready(body) or time.time() > deadline:
				return body
			time.sleep(0.1)

	def test_non_blocking_launch_captures_url_and_stops_cleanly(self) -> None:
		with mock.patch.dict(
			os.environ,
			{"DCMVIEW_BINARY": str(self.binary)},
			clear=False,
		):
			handle = wrapper.view(
				[FIXTURE_FILE],
				browser=False,
				timeout=30,
				block=False,
				vscode_bridge=False,
			)

			assert isinstance(handle, wrapper.ShutdownHandle)
			try:
				deadline = time.time() + 10.0
				while handle.url is None and time.time() < deadline:
					time.sleep(0.1)

				self.assertIsNotNone(handle.url)
				assert handle.url is not None
				self.assertTrue(handle.url.startswith("http://"))
			finally:
				exit_code = handle.stop()

			self.assertEqual(exit_code, 0)
			self.assertIsInstance(handle.stop(), int)


if __name__ == "__main__":
	unittest.main()

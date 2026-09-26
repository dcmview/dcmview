from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
import time
import unittest
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Iterator, Optional
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parents[2]
PYTHON_SRC = REPO_ROOT / "python"
if str(PYTHON_SRC) not in sys.path:
	sys.path.insert(0, str(PYTHON_SRC))

from dcmview_py import wrapper

FIXTURE_FILE = REPO_ROOT / "tests" / "fixtures" / "golden-uncompressed-u16-multiframe.dcm"
BRIDGE_TOKEN = "integration-token"
VSCODE_VIEWER_URL = "http://127.0.0.1:9/vscode-viewer"
BRIDGE_ENV_KEYS = (
	"DCMVIEW_VSCODE_BRIDGE_URL",
	"DCMVIEW_VSCODE_BRIDGE_TOKEN",
	"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR",
	"DCMVIEW_VSCODE_BYPASS",
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

			def _reply(self, body: dict[str, object]) -> None:
				payload = json.dumps(body).encode("utf-8")
				self.send_response(200)
				self.send_header("Content-Type", "application/json")
				self.send_header("Content-Length", str(len(payload)))
				self.end_headers()
				self.wfile.write(payload)

			def _authorized(self) -> bool:
				if self.headers.get("Authorization") == f"Bearer {BRIDGE_TOKEN}":
					return True
				self.send_response(401)
				self.end_headers()
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

			self.assertIsInstance(exit_code, int)
			self.assertIsInstance(handle.stop(), int)


if __name__ == "__main__":
	unittest.main()

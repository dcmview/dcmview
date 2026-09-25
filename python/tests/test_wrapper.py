from __future__ import annotations

import os
import importlib.util
import json
import subprocess
import sys
import time
import tempfile
import unittest
import urllib.error
import zipfile
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parents[2]
PYTHON_SRC = REPO_ROOT / "python"
if str(PYTHON_SRC) not in sys.path:
	sys.path.insert(0, str(PYTHON_SRC))

from dcmview_py import __main__ as dcmview_main
from dcmview_py import wrapper

FIXTURE_FILE = REPO_ROOT / "tests" / "fixtures" / "golden-uncompressed-u16-multiframe.dcm"
BRIDGE_CONTRACT = REPO_ROOT / "docs" / "contracts" / "bridge-protocol.json"
REGISTRY_CONTRACT = REPO_ROOT / "docs" / "contracts" / "vscode-bridge-registry.json"
VERIFY_WHEEL_INSTALL = REPO_ROOT / "scripts" / "verify_wheel_install.py"


def _load_script_module(name: str, path: Path):
	spec = importlib.util.spec_from_file_location(name, path)
	assert spec is not None
	module = importlib.util.module_from_spec(spec)
	assert spec.loader is not None
	spec.loader.exec_module(module)
	return module


class WrapperTests(unittest.TestCase):
	def test_pyproject_declares_both_console_script_names(self) -> None:
		pyproject = (REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8")
		self.assertIn('dcmview = "dcmview_py.__main__:main"', pyproject)
		self.assertIn('dcmview-py = "dcmview_py.__main__:main"', pyproject)

	def test_cli_reports_wrapper_version(self) -> None:
		with mock.patch("dcmview_py.__main__._package_version", return_value="9.8.7"):
			parser = dcmview_main._build_parser()
		output = StringIO()
		with redirect_stdout(output):
			with self.assertRaises(SystemExit) as context:
				parser.parse_args(["--version"])

		self.assertEqual(context.exception.code, 0)
		self.assertEqual(output.getvalue().strip(), "dcmview 9.8.7")

	def test_view_docstring_documents_public_api(self) -> None:
		docstring = wrapper.view.__doc__ or ""

		self.assertIn("Launch dcmview", docstring)
		self.assertIn("Args:", docstring)
		self.assertIn("Returns:", docstring)
		self.assertIn("Raises:", docstring)
		self.assertIn("vscode_bridge", docstring)
		self.assertIn("DCMVIEW_BINARY", docstring)

	def test_module_cli_help_describes_options_and_examples(self) -> None:
		parser = dcmview_main._build_parser()
		output = StringIO()
		with redirect_stdout(output):
			with self.assertRaises(SystemExit) as context:
				parser.parse_args(["--help"])

		self.assertEqual(context.exception.code, 0)
		help_text = output.getvalue()
		self.assertIn("DICOM file or directory to inspect", help_text)
		self.assertIn("--host ADDR", help_text)
		self.assertIn("--annotations CSV", help_text)
		self.assertIn("--filter FIELD=VALUE", help_text)
		self.assertIn("ssh -L 8010:127.0.0.1:8010 user@remote", help_text)
		self.assertIn("not clinical diagnosis", help_text)
		self.assertNotIn("--startup-json", help_text)

	def test_missing_binary_raises_runtime_error(self) -> None:
		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch("dcmview_py.wrapper.shutil.which", return_value=None):
				with self.assertRaisesRegex(RuntimeError, "dcmview binary not found"):
					wrapper.view([FIXTURE_FILE], browser=False, vscode_bridge=False)

	def test_explicit_binary_env_var_takes_precedence(self) -> None:
		with mock.patch.dict(os.environ, {"DCMVIEW_BINARY": "/tmp/env-dcmview"}, clear=True):
			with mock.patch.object(wrapper.Path, "is_file", return_value=True):
				with mock.patch("dcmview_py.wrapper._ensure_executable") as ensure_mock:
					command = wrapper._build_command(
						["/tmp/scan.dcm"],
						port=0,
						host="127.0.0.1",
						browser=True,
						recursive=True,
						timeout=None,
						annotations=None,
					)

		self.assertEqual(command[0], str(Path("/tmp/env-dcmview")))
		ensure_mock.assert_called_once()

	def test_prefers_bundled_binary_before_path_lookup(self) -> None:
		bundled = (PYTHON_SRC / "dcmview_py" / "bin" / wrapper._binary_name()).resolve()
		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch.object(wrapper.Path, "is_file", return_value=True):
				with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/usr/local/bin/dcmview"):
					with mock.patch("dcmview_py.wrapper._ensure_executable") as ensure_mock:
						resolved = wrapper._resolve_binary()

		self.assertEqual(resolved, str(bundled))
		ensure_mock.assert_called_once()

	def test_prefers_windows_bundled_exe_before_path_lookup(self) -> None:
		bundled = (PYTHON_SRC / "dcmview_py" / "bin" / "dcmview.exe").resolve()
		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch("dcmview_py.wrapper._is_windows", return_value=True):
				with mock.patch.object(wrapper.Path, "is_file", return_value=True):
					with mock.patch("dcmview_py.wrapper.shutil.which", return_value="C:\\Tools\\dcmview.exe"):
						with mock.patch("dcmview_py.wrapper._ensure_executable") as ensure_mock:
							resolved = wrapper._resolve_binary()

		self.assertEqual(resolved, str(bundled))
		ensure_mock.assert_called_once()

	def test_windows_path_lookup_uses_exe_name(self) -> None:
		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch("dcmview_py.wrapper._is_windows", return_value=True):
				with mock.patch.object(wrapper.Path, "is_file", return_value=False):
					with mock.patch("dcmview_py.wrapper.shutil.which", return_value="C:\\Tools\\dcmview.exe") as which_mock:
						resolved = wrapper._resolve_binary()

		self.assertEqual(resolved, "C:\\Tools\\dcmview.exe")
		which_mock.assert_called_once_with("dcmview.exe")

	def test_missing_explicit_binary_env_var_raises(self) -> None:
		with mock.patch.dict(os.environ, {"DCMVIEW_BINARY": "/tmp/missing-dcmview"}, clear=True):
			with mock.patch.object(wrapper.Path, "is_file", return_value=False):
				with self.assertRaisesRegex(RuntimeError, "points to a missing file"):
					wrapper._resolve_binary()

	def test_windows_subprocess_launch_uses_new_process_group(self) -> None:
		with mock.patch("dcmview_py.wrapper._is_windows", return_value=True):
			with mock.patch.object(wrapper.subprocess, "CREATE_NEW_PROCESS_GROUP", 512, create=True):
				options = wrapper._popen_options()

		self.assertEqual(options["creationflags"], 512)

	def test_local_subprocess_launch_sets_bridge_bypass(self) -> None:
		with mock.patch.dict(os.environ, {"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567"}, clear=True):
			options = wrapper._popen_options()

		self.assertEqual(options["env"]["DCMVIEW_VSCODE_BYPASS"], "1")
		self.assertEqual(options["env"]["DCMVIEW_VSCODE_BRIDGE_URL"], "http://127.0.0.1:4567")

	def test_windows_shutdown_uses_ctrl_break_event(self) -> None:
		process = mock.Mock()
		process.poll.return_value = None
		process.wait.return_value = 0
		monitor = mock.Mock()

		with mock.patch("dcmview_py.wrapper._is_windows", return_value=True):
			with mock.patch.object(wrapper.signal, "CTRL_BREAK_EVENT", 123, create=True):
				handle = wrapper.ShutdownHandle(process, monitor)
				self.assertEqual(handle.stop(), 0)

		process.send_signal.assert_called_once_with(123)
		monitor.join.assert_called_once()

	def test_wheel_verifier_accepts_windows_bundled_exe(self) -> None:
		verify_wheel = _load_script_module("verify_wheel_install_for_test", VERIFY_WHEEL_INSTALL)
		with tempfile.TemporaryDirectory(prefix="dcmview-wheel-test-") as temp_dir:
			wheel = Path(temp_dir) / "dcmview_py-0.2.8-py3-none-win_amd64.whl"
			with zipfile.ZipFile(wheel, "w") as archive:
				archive.writestr("dcmview_py/bin/dcmview.exe", b"binary")
				archive.writestr(
					"dcmview_py-0.2.8.dist-info/entry_points.txt",
					"[console_scripts]\ndcmview = dcmview_py.__main__:main\ndcmview-py = dcmview_py.__main__:main\n",
				)

			verify_wheel.validate_wheel_archive(wheel, "win_amd64")
			self.assertEqual(verify_wheel.resolve_wheel_path(Path(temp_dir), "win_amd64"), wheel.resolve())

	def test_wheel_verifier_uses_absolute_windows_console_script_paths(self) -> None:
		verify_wheel = _load_script_module("verify_wheel_install_for_console_test", VERIFY_WHEEL_INSTALL)
		with mock.patch.object(verify_wheel, "os_name_is_windows", return_value=True):
			self.assertEqual(
				verify_wheel.console_script(Path("C:/venv/Scripts"), "dcmview"),
				Path("C:/venv/Scripts/dcmview.exe"),
			)

	def test_cli_forwards_no_browser_no_recursive_and_timeout(self) -> None:
		with mock.patch("dcmview_py.__main__.view", return_value=None) as view_mock:
			exit_code = dcmview_main.run_cli(
				[
					"--no-browser",
					"--no-recursive",
					"--timeout",
					"9",
					"-p",
					"1042",
					"--host",
					"0.0.0.0",
					str(FIXTURE_FILE),
				]
			)

		self.assertEqual(exit_code, 0)
		view_mock.assert_called_once_with(
			[str(FIXTURE_FILE)],
			port=1042,
			host="0.0.0.0",
			browser=False,
			recursive=False,
			timeout=9,
			block=True,
			annotations=None,
		)

	def test_cli_forwards_annotations_path(self) -> None:
		annotations_path = REPO_ROOT / "tests" / "fixtures" / "embed_annotations.csv"
		with mock.patch("dcmview_py.__main__.view", return_value=None) as view_mock:
			exit_code = dcmview_main.run_cli([
				"--annotations",
				str(annotations_path),
				str(FIXTURE_FILE),
			])

		self.assertEqual(exit_code, 0)
		view_mock.assert_called_once_with(
			[str(FIXTURE_FILE)],
			port=0,
			host="127.0.0.1",
			browser=True,
			recursive=True,
			timeout=None,
			annotations=str(annotations_path),
			block=True,
		)

	def test_cli_forwards_filter_flags(self) -> None:
		with mock.patch("dcmview_py.__main__.view", return_value=None) as view_mock:
			exit_code = dcmview_main.run_cli([
				"--filter",
				"modality=MR",
				"--filter",
				"patient_id=123",
				str(FIXTURE_FILE),
			])

		self.assertEqual(exit_code, 0)
		view_mock.assert_called_once_with(
			[str(FIXTURE_FILE)],
			port=0,
			host="127.0.0.1",
			browser=True,
			recursive=True,
			timeout=None,
			annotations=None,
			block=True,
			filters=["modality=MR", "patient_id=123"],
		)

	def test_build_command_includes_annotations_flag_when_provided(self) -> None:
		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			command = wrapper._build_command(
				["/tmp/scan.dcm"],
				port=0,
				host="127.0.0.1",
				browser=True,
				recursive=True,
				timeout=None,
				annotations="/tmp/annotations.csv",
			)

		self.assertIn("--annotations", command)
		flag_index = command.index("--annotations")
		self.assertEqual(command[flag_index + 1], "/tmp/annotations.csv")

	def test_build_command_includes_filter_flags_when_provided(self) -> None:
		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			command = wrapper._build_command(
				["/tmp/scan.dcm"],
				port=0,
				host="127.0.0.1",
				browser=True,
				recursive=True,
				timeout=None,
				annotations=None,
				filters=["modality=MR", "patient_id=123"],
			)

		self.assertIn("--filter", command)
		self.assertEqual(
			command[command.index("--filter") : command.index("/tmp/scan.dcm")],
			["--filter", "modality=MR", "--filter", "patient_id=123"],
		)

	def test_build_command_requests_structured_startup_event(self) -> None:
		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			command = wrapper._build_command(
				["/tmp/scan.dcm"],
				port=0,
				host="127.0.0.1",
				browser=True,
				recursive=True,
				timeout=None,
				annotations=None,
			)

		self.assertIn("--startup-json", command)
		self.assertLess(command.index("--startup-json"), command.index("/tmp/scan.dcm"))

	def test_parse_structured_startup_event(self) -> None:
		url = wrapper._parse_startup_url(
			'{"type":"server_started","url":"http://127.0.0.1:51234","host":"127.0.0.1","port":51234}'
		)

		self.assertEqual(url, "http://127.0.0.1:51234")

	def test_parse_legacy_startup_line_as_fallback(self) -> None:
		url = wrapper._parse_startup_url("dcmview: server running at http://127.0.0.1:51234")

		self.assertEqual(url, "http://127.0.0.1:51234")

	def test_parse_startup_url_ignores_malformed_json_lines(self) -> None:
		self.assertIsNone(wrapper._parse_startup_url('{"type":"server_started",'))
		self.assertIsNone(wrapper._parse_startup_url('{"type":"other","url":"http://127.0.0.1:1"}'))
		self.assertIsNone(wrapper._parse_startup_url("dcmview: loaded 1 DICOM file"))

	def test_output_monitor_wait_for_url_times_out_without_startup_line(self) -> None:
		process = mock.Mock()
		process.stdout = StringIO("dcmview: loaded 1 DICOM file\n")
		monitor = wrapper._OutputMonitor(process)

		with redirect_stdout(StringIO()):
			monitor.start()
			self.assertIsNone(monitor.wait_for_url(0.01))
			monitor.join()

	def test_view_relaunches_without_startup_json_for_older_blocking_binary(self) -> None:
		old_process = mock.Mock()
		old_process.stdout = StringIO("error: unexpected argument '--startup-json' found\n")
		old_process.wait.return_value = 2
		old_process.returncode = 2
		new_process = mock.Mock()
		new_process.stdout = StringIO("dcmview: server running at http://127.0.0.1:51234\n")
		new_process.wait.return_value = 0
		new_process.returncode = 0

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch("dcmview_py.wrapper.subprocess.Popen", side_effect=[old_process, new_process]) as popen:
				with redirect_stdout(StringIO()):
					result = wrapper.view(
						["/tmp/scan.dcm"],
						browser=False,
						block=True,
						vscode_bridge=False,
					)

		self.assertIsNone(result)
		self.assertIn("--startup-json", popen.call_args_list[0].args[0])
		self.assertNotIn("--startup-json", popen.call_args_list[1].args[0])

	def test_view_relaunches_without_startup_json_for_older_nonblocking_binary(self) -> None:
		old_process = mock.Mock()
		old_process.stdout = StringIO("error: Found argument '--startup-json' which wasn't expected\n")
		old_process.poll.return_value = 2
		old_process.returncode = 2
		new_process = mock.Mock()
		new_process.stdout = StringIO("dcmview: server running at http://127.0.0.1:51234\n")
		new_process.poll.return_value = None
		new_process.returncode = None

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch("dcmview_py.wrapper.subprocess.Popen", side_effect=[old_process, new_process]) as popen:
				with redirect_stdout(StringIO()):
					handle = wrapper.view(
						["/tmp/scan.dcm"],
						browser=False,
						block=False,
						vscode_bridge=False,
					)

		self.assertIsNotNone(handle)
		assert handle is not None
		self.assertEqual(handle.url, "http://127.0.0.1:51234")
		self.assertIn("--startup-json", popen.call_args_list[0].args[0])
		self.assertNotIn("--startup-json", popen.call_args_list[1].args[0])

	def test_view_routes_blocking_calls_through_vscode_bridge(self) -> None:
		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": "secret",
			},
			clear=True,
		):
			with mock.patch(
				"dcmview_py.wrapper._bridge_json_request",
				side_effect=[
					{"sessionId": "abc", "url": "http://127.0.0.1:9999"},
					{"exitCode": 0},
				],
			) as bridge_mock:
				with mock.patch("dcmview_py.wrapper._resolve_binary", return_value="/tmp/dcmview"):
					with mock.patch("dcmview_py.wrapper.subprocess.Popen") as popen_mock:
						with redirect_stdout(StringIO()):
							result = wrapper.view([FIXTURE_FILE], browser=True, block=True)

		self.assertIsNone(result)
		popen_mock.assert_not_called()
		launch_payload = bridge_mock.call_args_list[0].args[2]
		self.assertEqual(launch_payload["program"], "dcmview_py")
		self.assertIn(str(FIXTURE_FILE), launch_payload["args"])
		self.assertFalse(launch_payload["wait"])
		self.assertEqual(launch_payload["binaryPath"], "/tmp/dcmview")
		self.assertIsNone(bridge_mock.call_args_list[1].kwargs["timeout"])

	def test_view_does_not_fallback_after_bridge_launch_is_captured(self) -> None:
		def fake_bridge_request(method, path, payload=None, *, endpoint=None, timeout=5.0):
			if method == "POST" and path == "/launch":
				self.assertEqual(timeout, 5.0)
				return {"sessionId": "abc", "url": "http://127.0.0.1:9999"}
			if method == "GET" and path == "/sessions/abc/wait":
				self.assertIsNone(timeout)
				raise TimeoutError("timed out")
			raise AssertionError(f"unexpected bridge request {method} {path}")

		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": "secret",
			},
			clear=True,
		):
			with mock.patch("dcmview_py.wrapper._bridge_json_request", side_effect=fake_bridge_request):
				with mock.patch("dcmview_py.wrapper._resolve_binary", return_value="/tmp/dcmview"):
					with mock.patch("dcmview_py.wrapper.subprocess.Popen") as popen_mock:
						stdout = StringIO()
						stderr = StringIO()
						with redirect_stdout(stdout), mock.patch("sys.stderr", stderr):
							with self.assertRaisesRegex(
								RuntimeError,
								"VS Code bridge session was captured but wait failed",
							):
								wrapper.view([FIXTURE_FILE], browser=True, block=True)

		popen_mock.assert_not_called()
		self.assertIn("dcmview: opened in VS Code at http://127.0.0.1:9999", stdout.getvalue())
		self.assertNotIn("VS Code bridge unavailable", stderr.getvalue())

	def test_view_omits_bridge_binary_path_when_resolution_fails(self) -> None:
		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": "secret",
			},
			clear=True,
		):
			with mock.patch(
				"dcmview_py.wrapper._bridge_json_request",
				side_effect=[
					{"sessionId": "abc", "url": "http://127.0.0.1:9999"},
					{"exitCode": 0},
				],
			) as bridge_mock:
				with mock.patch("dcmview_py.wrapper._resolve_binary", side_effect=RuntimeError("not found")):
					with mock.patch("dcmview_py.wrapper.subprocess.Popen") as popen_mock:
						with redirect_stdout(StringIO()):
							result = wrapper.view([FIXTURE_FILE], browser=True, block=True)

		self.assertIsNone(result)
		popen_mock.assert_not_called()
		launch_payload = bridge_mock.call_args_list[0].args[2]
		self.assertEqual(launch_payload["program"], "dcmview_py")
		self.assertNotIn("binaryPath", launch_payload)

	def test_bridge_json_request_matches_shared_launch_fixture(self) -> None:
		fixture = json.loads(BRIDGE_CONTRACT.read_text(encoding="utf-8"))
		launch = fixture["launch"]
		auth = fixture["auth"]
		captured_request = None

		class FakeResponse:
			def __enter__(self) -> "FakeResponse":
				return self

			def __exit__(self, _exc_type, _exc, _tb) -> None:
				return None

			def read(self) -> bytes:
				return json.dumps(launch["response"]).encode("utf-8")

		def fake_urlopen(request, *, timeout):
			nonlocal captured_request
			captured_request = request
			self.assertEqual(timeout, 5.0)
			return FakeResponse()

		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567/",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": auth["bearerToken"],
			},
			clear=True,
		):
			with mock.patch("dcmview_py.wrapper.urllib.request.urlopen", side_effect=fake_urlopen):
				response = wrapper._bridge_json_request(
					launch["method"],
					launch["path"],
					launch["request"],
				)

		self.assertEqual(response, launch["response"])
		self.assertIsNotNone(captured_request)
		assert captured_request is not None
		self.assertEqual(captured_request.full_url, "http://127.0.0.1:4567/launch")
		self.assertEqual(captured_request.get_method(), "POST")
		self.assertEqual(captured_request.get_header("Authorization"), f"Bearer {auth['bearerToken']}")
		self.assertEqual(captured_request.get_header("Content-type"), "application/json")
		self.assertEqual(json.loads(captured_request.data.decode("utf-8")), launch["request"])

	def test_bridge_json_request_parses_shared_wait_fixture(self) -> None:
		fixture = json.loads(BRIDGE_CONTRACT.read_text(encoding="utf-8"))
		wait = fixture["wait"]

		class FakeResponse:
			def __enter__(self) -> "FakeResponse":
				return self

			def __exit__(self, _exc_type, _exc, _tb) -> None:
				return None

			def read(self) -> bytes:
				return json.dumps(wait["response"]).encode("utf-8")

		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": fixture["auth"]["bearerToken"],
			},
			clear=True,
		):
			with mock.patch("dcmview_py.wrapper.urllib.request.urlopen", return_value=FakeResponse()):
				response = wrapper._bridge_json_request(wait["method"], wait["path"])

		self.assertEqual(response, wait["response"])

	def test_bridge_registry_endpoints_prefer_matching_workspace_then_newest(self) -> None:
		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			now_ms = 100_000
			(registry / "old.json").write_text(
				json.dumps(
					{
						"version": 1,
						"instanceId": "old",
						"bridgeUrl": "http://127.0.0.1:1111",
						"token": "old-token",
						"workspaceRoots": ["/elsewhere"],
						"createdAtMs": now_ms,
					}
				),
				encoding="utf-8",
			)
			(registry / "match.json").write_text(
				json.dumps(
					{
						"version": 1,
						"instanceId": "match",
						"bridgeUrl": "http://127.0.0.1:2222",
						"token": "match-token",
						"workspaceRoots": [str(REPO_ROOT)],
						"createdAtMs": now_ms - 1,
					}
				),
				encoding="utf-8",
			)

			with mock.patch.dict(
				os.environ,
				{"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry)},
				clear=True,
			):
				endpoints = wrapper._bridge_registry_endpoints(str(REPO_ROOT / "tests"), now_ms=now_ms)

		self.assertEqual(endpoints[0], ("http://127.0.0.1:2222", "match-token"))
		self.assertEqual(endpoints[1], ("http://127.0.0.1:1111", "old-token"))

	def test_bridge_registry_matches_shared_contract(self) -> None:
		contract = json.loads(REGISTRY_CONTRACT.read_text(encoding="utf-8"))
		self.assertEqual(wrapper._BRIDGE_REGISTRY_MAX_AGE_SECONDS * 1000, contract["ttlMs"])

		for test_case in contract["registryDirs"]:
			with mock.patch.dict(os.environ, test_case["env"], clear=True):
				with mock.patch("dcmview_py.wrapper.tempfile.gettempdir", return_value=test_case["tmpDir"]):
					self.assertEqual(wrapper._bridge_registry_dir(), test_case["expected"])
		for test_case in contract["safeSegments"]:
			self.assertEqual(wrapper._safe_registry_segment(test_case["input"]), test_case["expected"])
		for test_case in contract["expiry"]["cases"]:
			self.assertEqual(
				wrapper._is_expired_registry_entry(test_case["createdAtMs"], contract["expiry"]["nowMs"]),
				test_case["expired"],
			)

		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			for item in contract["ordering"]["entries"]:
				(registry / item["file"]).write_text(json.dumps(item["entry"]), encoding="utf-8")
			with mock.patch.dict(
				os.environ,
				{"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry)},
				clear=True,
			):
				endpoints = wrapper._bridge_registry_endpoints(
					contract["ordering"]["cwd"],
					now_ms=contract["ordering"]["nowMs"],
				)

		self.assertEqual([list(endpoint) for endpoint in endpoints], contract["ordering"]["expectedAllowAny"])

		for test_case in contract["legacyRegistryDirs"]:
			with mock.patch.dict(os.environ, test_case["env"], clear=True):
				with mock.patch("dcmview_py.wrapper.tempfile.gettempdir", return_value=test_case["tmpDir"]):
					self.assertIn(test_case["expected"], wrapper._bridge_registry_dirs())

	def test_bridge_discovery_uses_env_then_registry(self) -> None:
		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			(registry / "live.json").write_text(
				json.dumps(
					{
						"version": 1,
						"instanceId": "live",
						"bridgeUrl": "http://127.0.0.1:2222",
						"token": "registry-token",
						"workspaceRoots": [],
						"createdAtMs": int(time.time() * 1000),
					}
				),
				encoding="utf-8",
			)

			with mock.patch.dict(
				os.environ,
				{
					"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:1111",
					"DCMVIEW_VSCODE_BRIDGE_TOKEN": "env-token",
					"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry),
				},
				clear=True,
			):
				endpoints = wrapper._bridge_endpoints()

		self.assertEqual(
			endpoints,
			[
				("http://127.0.0.1:1111", "env-token"),
				("http://127.0.0.1:2222", "registry-token"),
			],
		)

	def test_bridge_registry_path_join_preserves_posix_style_inputs(self) -> None:
		self.assertEqual(
			wrapper._join_registry_path("/run/user/1000", "dcmview", "vscode-bridges"),
			"/run/user/1000/dcmview/vscode-bridges",
		)
		self.assertEqual(
			wrapper._join_registry_path("/tmp", "dcmview-vscode-bridges-remote_user_example"),
			"/tmp/dcmview-vscode-bridges-remote_user_example",
		)

	def test_bridge_registry_dir_falls_back_when_home_is_unavailable(self) -> None:
		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch.object(wrapper.Path, "home", side_effect=RuntimeError("no home")):
				self.assertEqual(
					wrapper._canonical_bridge_registry_dir(),
					str(Path(".").joinpath(".local", "state", "dcmview", "vscode-bridges")),
				)

	def test_bridge_registry_discovery_ignores_invalid_entries(self) -> None:
		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			now_ms = 100_000
			(registry / "invalid.json").write_text("{", encoding="utf-8")
			(registry / "missing-token.json").write_text(
				json.dumps({"bridgeUrl": "http://127.0.0.1:1111"}),
				encoding="utf-8",
			)
			(registry / "valid.json").write_text(
				json.dumps(
					{
						"bridgeUrl": "http://127.0.0.1:3333",
						"token": "valid-token",
						"workspaceRoots": [],
						"createdAtMs": now_ms,
					}
				),
				encoding="utf-8",
			)

			with mock.patch.dict(
				os.environ,
				{"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry)},
				clear=True,
			):
				endpoints = wrapper._bridge_registry_endpoints(str(REPO_ROOT), now_ms=now_ms)

		self.assertEqual(endpoints, [("http://127.0.0.1:3333", "valid-token")])

	def test_bridge_registry_discovery_preserves_unknown_versions(self) -> None:
		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			future = registry / "future.json"
			malformed_v1 = registry / "malformed-v1.json"
			future.write_text(
				json.dumps({"version": 2, "createdAtMs": "new-format"}),
				encoding="utf-8",
			)
			malformed_v1.write_text(
				json.dumps({"version": 1, "bridgeUrl": "http://127.0.0.1:3333", "token": "token"}),
				encoding="utf-8",
			)

			with mock.patch.dict(
				os.environ,
				{"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry)},
				clear=True,
			):
				endpoints = wrapper._bridge_registry_endpoints(str(REPO_ROOT), now_ms=100_000)

			self.assertEqual(endpoints, [])
			self.assertTrue(future.exists())
			self.assertFalse(malformed_v1.exists())

	def test_bridge_registry_discovery_ignores_untrusted_unix_directories(self) -> None:
		stat_result = os.stat_result((0o777, 0, 0, 0, 9999, 0, 0, 0, 0, 0))

		with mock.patch("dcmview_py.wrapper._is_windows", return_value=False):
			trusted = wrapper._registry_dir_is_trusted(Path("/tmp/dcmview"), stat_result=stat_result, euid=1000)

		self.assertFalse(trusted)

	def test_bridge_registry_discovery_removes_expired_entries(self) -> None:
		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			expired = registry / "expired.json"
			expired.write_text(
				json.dumps(
					{
						"bridgeUrl": "http://127.0.0.1:3333",
						"token": "expired-token",
						"workspaceRoots": [],
						"createdAtMs": 1,
					}
				),
				encoding="utf-8",
			)

			with mock.patch.dict(
				os.environ,
				{"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry)},
				clear=True,
			):
				endpoints = wrapper._bridge_registry_endpoints(
					str(REPO_ROOT),
					now_ms=wrapper._BRIDGE_REGISTRY_MAX_AGE_SECONDS * 1000 + 2,
				)

			self.assertEqual(endpoints, [])
			self.assertFalse(expired.exists())

	def test_remove_bridge_registry_endpoint_deletes_matching_entries_only(self) -> None:
		with tempfile.TemporaryDirectory() as tmp:
			registry = Path(tmp)
			stale = registry / "stale.json"
			live = registry / "live.json"
			stale.write_text(
				json.dumps(
					{
						"bridgeUrl": "http://127.0.0.1:1111",
						"token": "stale-token",
						"workspaceRoots": [],
						"createdAtMs": 100_000,
					}
				),
				encoding="utf-8",
			)
			live.write_text(
				json.dumps(
					{
						"bridgeUrl": "http://127.0.0.1:2222",
						"token": "live-token",
						"workspaceRoots": [],
						"createdAtMs": 100_000,
					}
				),
				encoding="utf-8",
			)

			with mock.patch.dict(
				os.environ,
				{"DCMVIEW_VSCODE_BRIDGE_REGISTRY_DIR": str(registry)},
				clear=True,
			):
				wrapper._remove_bridge_registry_endpoint(("http://127.0.0.1:1111", "stale-token"))

			self.assertFalse(stale.exists())
			self.assertTrue(live.exists())

	def test_view_tries_next_registry_bridge_after_stale_entry(self) -> None:
		endpoints = [
			("http://127.0.0.1:1111", "stale"),
			("http://127.0.0.1:2222", "live"),
		]

		def fake_bridge_request(method, path, payload=None, *, endpoint=None, timeout=5.0):
			if endpoint == endpoints[0]:
				raise urllib.error.URLError("stale")
			self.assertEqual(endpoint, endpoints[1])
			if method == "POST":
				return {"sessionId": "abc", "url": "http://127.0.0.1:9999"}
			return {"exitCode": 0}

		with mock.patch("dcmview_py.wrapper._bridge_endpoints", return_value=endpoints):
			with mock.patch("dcmview_py.wrapper._bridge_json_request", side_effect=fake_bridge_request):
				with mock.patch("dcmview_py.wrapper._resolve_binary", return_value="/tmp/dcmview"):
					with mock.patch("dcmview_py.wrapper.subprocess.Popen") as popen_mock:
						with redirect_stdout(StringIO()):
							result = wrapper.view([FIXTURE_FILE], browser=True, block=True)

		self.assertIsNone(result)
		popen_mock.assert_not_called()

	def test_view_can_disable_vscode_bridge_per_call(self) -> None:
		process = mock.Mock()
		process.stdout = StringIO("dcmview: server running at http://127.0.0.1:51234\n")
		process.poll.return_value = None
		process.returncode = None

		with mock.patch("dcmview_py.wrapper._bridge_endpoints") as endpoints_mock:
			with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
				with mock.patch("dcmview_py.wrapper.subprocess.Popen", return_value=process) as popen_mock:
					with redirect_stdout(StringIO()):
						handle = wrapper.view([FIXTURE_FILE], browser=True, block=False, vscode_bridge=False)

		self.assertIsInstance(handle, wrapper.ShutdownHandle)
		endpoints_mock.assert_not_called()
		popen_env = popen_mock.call_args.kwargs["env"]
		self.assertEqual(popen_env["DCMVIEW_VSCODE_BYPASS"], "1")

	def test_view_returns_bridge_shutdown_handle_for_nonblocking_calls(self) -> None:
		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": "secret",
			},
			clear=True,
		):
			with mock.patch(
				"dcmview_py.wrapper._bridge_json_request",
				side_effect=[
					{"sessionId": "abc", "url": "http://127.0.0.1:9999"},
					{"ok": True},
					{"exitCode": 0},
				],
			) as bridge_mock:
				with mock.patch("dcmview_py.wrapper._resolve_binary", return_value="/tmp/dcmview"):
					with redirect_stdout(StringIO()):
						handle = wrapper.view([FIXTURE_FILE], browser=False, block=False)
					self.assertIsNotNone(handle)
					assert handle is not None
					self.assertEqual(handle.url, "http://127.0.0.1:9999")
					self.assertEqual(handle.stop(), 0)

		self.assertEqual(bridge_mock.call_args_list[1].args[:2], ("POST", "/sessions/abc/stop"))
		self.assertEqual(bridge_mock.call_args_list[2].args[:2], ("GET", "/sessions/abc/wait"))

	def test_vscode_bridge_bypass_uses_local_subprocess_path(self) -> None:
		with mock.patch.dict(
			os.environ,
			{
				"DCMVIEW_VSCODE_BRIDGE_URL": "http://127.0.0.1:4567",
				"DCMVIEW_VSCODE_BRIDGE_TOKEN": "secret",
				"DCMVIEW_VSCODE_BYPASS": "1",
			},
			clear=True,
		):
			with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
				command = wrapper._build_command(
					["/tmp/scan.dcm"],
					port=0,
					host="127.0.0.1",
					browser=True,
					recursive=True,
					timeout=None,
					annotations=None,
				)

		self.assertEqual(command[0], "/tmp/dcmview")

	def test_cli_returns_child_exit_code(self) -> None:
		with mock.patch(
			"dcmview_py.__main__.view",
			side_effect=subprocess.CalledProcessError(7, ["dcmview"]),
		):
			exit_code = dcmview_main.run_cli([str(FIXTURE_FILE)])

		self.assertEqual(exit_code, 7)


if __name__ == "__main__":
	unittest.main()

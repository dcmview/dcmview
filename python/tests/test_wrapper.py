from __future__ import annotations

import os
import importlib.util
import sys
import tempfile
import unittest
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

	def test_view_docstring_documents_public_api(self) -> None:
		docstring = wrapper.view.__doc__ or ""

		self.assertIn("Launch dcmview", docstring)
		self.assertIn("Args:", docstring)
		self.assertIn("Returns:", docstring)
		self.assertIn("Raises:", docstring)
		self.assertIn("vscode_bridge", docstring)
		self.assertIn("DCMVIEW_BINARY", docstring)

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
			options = wrapper._popen_options(vscode_bridge=False)

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

	def test_parse_vscode_session_startup(self) -> None:
		self.assertEqual(
			wrapper._parse_startup_url('{"type":"vscode_session_started","url":"http://127.0.0.1:9999"}'),
			"http://127.0.0.1:9999",
		)
		self.assertEqual(
			wrapper._parse_startup_url("dcmview: opened in VS Code at http://127.0.0.1:9999"),
			"http://127.0.0.1:9999",
		)

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

	def test_view_routes_through_the_binary_bridge_client_by_default(self) -> None:
		process = mock.Mock()
		process.stdout = StringIO(
			'{"type":"vscode_session_started","url":"http://127.0.0.1:9999"}\n'
			"dcmview: opened in VS Code at http://127.0.0.1:9999\n"
		)
		process.poll.return_value = None
		process.returncode = None

		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
				with mock.patch("dcmview_py.wrapper.subprocess.Popen", return_value=process) as popen_mock:
					with redirect_stdout(StringIO()):
						handle = wrapper.view([FIXTURE_FILE], browser=False, block=False)

		assert handle is not None
		self.assertEqual(handle.url, "http://127.0.0.1:9999")
		command = popen_mock.call_args.args[0]
		self.assertEqual(command[:3], ["/tmp/dcmview", "--vscode-bridge-client", "dcmview_py"])
		self.assertIn("--startup-json", command)
		self.assertNotIn("DCMVIEW_VSCODE_BYPASS", popen_mock.call_args.kwargs["env"])

	def test_view_can_disable_vscode_bridge_per_call(self) -> None:
		process = mock.Mock()
		process.stdout = StringIO("dcmview: server running at http://127.0.0.1:51234\n")
		process.poll.return_value = None
		process.returncode = None

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch("dcmview_py.wrapper.subprocess.Popen", return_value=process) as popen_mock:
				with redirect_stdout(StringIO()):
					handle = wrapper.view([FIXTURE_FILE], browser=True, block=False, vscode_bridge=False)

		self.assertIsInstance(handle, wrapper.ShutdownHandle)
		self.assertNotIn("--vscode-bridge-client", popen_mock.call_args.args[0])
		self.assertEqual(popen_mock.call_args.kwargs["env"]["DCMVIEW_VSCODE_BYPASS"], "1")

	def test_view_relaunches_without_integration_flags_for_pre_bridge_binary(self) -> None:
		old_process = mock.Mock()
		old_process.stdout = StringIO("error: unexpected argument '--vscode-bridge-client' found\n")
		old_process.wait.return_value = 2
		old_process.returncode = 2
		new_process = mock.Mock()
		new_process.stdout = StringIO("dcmview: server running at http://127.0.0.1:51234\n")
		new_process.wait.return_value = 0
		new_process.returncode = 0

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch("dcmview_py.wrapper.subprocess.Popen", side_effect=[old_process, new_process]) as popen:
				with redirect_stdout(StringIO()):
					wrapper.view(["/tmp/scan.dcm"], browser=False, block=True)

		retried = popen.call_args_list[1].args[0]
		self.assertNotIn("--vscode-bridge-client", retried)
		self.assertNotIn("--startup-json", retried)

	def test_cli_forwards_argv_through_the_bridge_client_form(self) -> None:
		process = mock.Mock()
		process.wait.return_value = 7
		argv = ["--no-browser", "--filter", "modality=MR", str(FIXTURE_FILE)]

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch.dict(os.environ, {}, clear=True):
				with mock.patch("dcmview_py.__main__.subprocess.Popen", return_value=process) as popen:
					exit_code = dcmview_main.run_cli(argv)

		self.assertEqual(exit_code, 7)
		popen.assert_called_once_with(["/tmp/dcmview", "--vscode-bridge-client", "dcmview_py", *argv])

	def test_cli_keeps_waiting_for_the_binary_after_ctrl_c(self) -> None:
		process = mock.Mock()
		process.wait.side_effect = [KeyboardInterrupt(), 0]

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch.dict(os.environ, {}, clear=True):
				with mock.patch("dcmview_py.__main__.subprocess.Popen", return_value=process):
					exit_code = dcmview_main.run_cli([str(FIXTURE_FILE)])

		self.assertEqual(exit_code, 0)
		self.assertEqual(process.wait.call_count, 2)

	def test_cli_reports_signal_exits_as_shell_status(self) -> None:
		process = mock.Mock()
		process.wait.return_value = -9

		with mock.patch("dcmview_py.wrapper.shutil.which", return_value="/tmp/dcmview"):
			with mock.patch.dict(os.environ, {}, clear=True):
				with mock.patch("dcmview_py.__main__.subprocess.Popen", return_value=process):
					self.assertEqual(dcmview_main.run_cli([str(FIXTURE_FILE)]), 137)

	def test_cli_without_a_binary_exits_nonzero(self) -> None:
		with mock.patch.dict(os.environ, {}, clear=True):
			with mock.patch("dcmview_py.wrapper.shutil.which", return_value=None):
				with mock.patch.object(wrapper.Path, "is_file", return_value=False):
					with mock.patch("sys.stderr", new_callable=StringIO):
						self.assertEqual(dcmview_main.run_cli([str(FIXTURE_FILE)]), 1)


if __name__ == "__main__":
	unittest.main()

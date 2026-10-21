from __future__ import annotations

import unittest
import os
from pathlib import Path
from unittest import mock

from scripts import check


class RecordingRunner(check.CheckRunner):
	def __init__(self) -> None:
		super().__init__(install=False)
		self.calls: list[str] = []

	@property
	def cargo(self) -> str:
		return "cargo"

	def versions(self) -> None:
		self.calls.append("versions")

	def frontend(self) -> None:
		self.calls.append("frontend")

	def build_frontend(self) -> None:
		self.calls.append("frontend-assets")

	def rust_lint(self) -> None:
		self.calls.append("rust-lint")

	def rust_test(self) -> None:
		self.calls.append("rust-test")

	def python_unit(self) -> None:
		self.calls.append("python-unit")

	def python_integration(self) -> None:
		self.calls.append("python-integration")

	def vscode_compile(self) -> None:
		self.calls.append("vscode")

	def vscode_integration(self) -> None:
		self.calls.append("vscode-integration")


class CheckProfileCompositionTests(unittest.TestCase):
	def test_aggregate_profiles_run_the_documented_layers(self) -> None:
		cases = {
			"quick": [
				"versions",
				"frontend",
				"rust-lint",
				"python-unit",
			],
			"core": [
				"versions",
				"frontend",
				"rust-lint",
				"rust-test",
				"python-unit",
				"vscode",
			],
			"e2e": [
				"versions",
				"frontend",
				"rust-lint",
				"rust-test",
				"python-unit",
				"vscode",
				"python-integration",
				"vscode-integration",
			],
		}

		for profile, expected in cases.items():
			with self.subTest(profile=profile):
				runner = RecordingRunner()
				getattr(runner, profile)()
				self.assertCountEqual(runner.calls, expected)

	def test_external_is_an_independent_remote_fixture_profile(self) -> None:
		runner = RecordingRunner()

		with mock.patch.object(check, "run") as run:
			runner.external()

		self.assertEqual(runner.calls, ["frontend-assets"])
		run.assert_called_once()
		_label, command = run.call_args.args
		self.assertEqual(
			command,
			[
				"cargo",
				"test",
				"--locked",
				"--features",
				"remote-fixtures",
				"--test",
				"integration",
				"integration::remote_fixtures",
				"--",
				"--ignored",
			],
		)

	def test_timing_enforces_the_startup_threshold_and_is_in_no_aggregate(self) -> None:
		runner = RecordingRunner()
		with mock.patch.object(check, "run") as run:
			runner.timing()
		_label, command = run.call_args.args
		self.assertEqual(command[1:], ["scripts/startup_timing.py", "run", "--enforce"])
		# A timing gate on a shared runner would fail on noise: no profile CI runs includes it.
		for profile in (runner.quick, runner.core, runner.e2e):
			with mock.patch.object(runner, "timing") as timing, mock.patch.object(check, "run"):
				profile()
			timing.assert_not_called()

	def test_compatibility_artifact_runs_the_local_container(self) -> None:
		runner = RecordingRunner()
		with (
			mock.patch.dict(
				os.environ,
				{
					"DCMVIEW_COMPAT_CORPUS_ROOT": "/tmp/current-smoke",
					"DCMVIEW_COMPAT_OUTPUT": "/tmp/compatibility-output",
				},
			),
			mock.patch.object(runner, "build_binary"),
			mock.patch.object(check, "run") as run,
		):
			runner.compatibility_artifact()

		_label, command = run.call_args.args
		self.assertEqual(command[command.index("--corpus-root") + 1], "/tmp/current-smoke")
		self.assertEqual(command[-2:], ["--output", "/tmp/compatibility-output"])

	def test_compatibility_artifact_requires_a_local_container(self) -> None:
		runner = RecordingRunner()
		with mock.patch.dict(os.environ, {}, clear=True):
			with self.assertRaises(check.CheckError):
				runner.compatibility_artifact()

	def test_corpus_runs_every_ignored_non_remote_test_with_the_corpus(self) -> None:
		runner = RecordingRunner()
		runner.corpus = os.path.dirname(os.path.abspath(__file__))
		with mock.patch.dict(os.environ, {}, clear=True), mock.patch.object(check, "run") as run:
			runner.prepared_corpus()

		self.assertEqual(runner.calls, ["frontend-assets"])
		_label, command = run.call_args.args
		self.assertEqual(
			command,
			[
				"cargo",
				"test",
				"--locked",
				"--lib",
				"--test",
				"integration",
				"--",
				"--ignored",
				"--skip",
				"remote_fixtures",
				# An opt-in timing measurement, not a corpus test.
				"--skip",
				"thumbnail_timing",
			],
		)
		self.assertEqual(run.call_args.kwargs["env"]["DCMVIEW_PREPARED_CORPUS"], runner.corpus)

	def test_corpus_requires_an_existing_directory(self) -> None:
		for corpus in (None, "/nonexistent/dcmview-prepared-corpus"):
			with self.subTest(corpus=corpus):
				runner = RecordingRunner()
				runner.corpus = corpus
				with mock.patch.dict(os.environ, {}, clear=True):
					with self.assertRaises(check.CheckError):
						runner.prepared_corpus()
				self.assertEqual(runner.calls, [])


	def test_remote_ssh_tests_the_named_wheel_without_rebuilding(self) -> None:
		runner = RecordingRunner()
		with (
			mock.patch.dict(os.environ, {"DCMVIEW_REMOTE_WHEEL": "/tmp/dcmview.whl"}),
			mock.patch.object(check, "run") as run,
		):
			runner.remote_ssh()

		run.assert_called_once()
		_label, command = run.call_args.args
		self.assertEqual(command[-1], "python.tests.remote_ssh_integration")
		# The profile resolves the path, and /tmp is a symlink on macOS.
		self.assertEqual(
			run.call_args.kwargs["env"]["DCMVIEW_REMOTE_WHEEL"],
			str(Path("/tmp/dcmview.whl").resolve()),
		)

	def test_vscode_remote_ssh_tests_the_named_artifacts_without_packaging(self) -> None:
		runner = RecordingRunner()
		with (
			mock.patch.dict(
				os.environ,
				{"DCMVIEW_REMOTE_WHEEL": "/tmp/dcmview.whl", "DCMVIEW_REMOTE_VSIX": "/tmp/dcmview.vsix"},
			),
			mock.patch.object(runner, "package_linux_vsix") as package,
			mock.patch.object(check, "run") as run,
		):
			runner.vscode_remote_ssh()

		package.assert_not_called()
		_label, command = run.call_args.args
		self.assertEqual(command[-1], "python.tests.vscode_remote_ssh_integration")
		self.assertEqual(
			run.call_args.kwargs["env"]["DCMVIEW_REMOTE_VSIX"],
			str(Path("/tmp/dcmview.vsix").resolve()),
		)

if __name__ == "__main__":
	unittest.main()

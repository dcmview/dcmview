from __future__ import annotations

import unittest
import os
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
	def test_aggregate_profiles_compose_the_documented_layers(self) -> None:
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
				self.assertEqual(runner.calls, expected)

	def test_external_is_an_independent_remote_fixture_profile(self) -> None:
		runner = RecordingRunner()

		with mock.patch.object(check, "run") as run:
			runner.external()

		self.assertEqual(runner.calls, ["frontend-assets"])
		run.assert_called_once()
		label, command = run.call_args.args
		self.assertEqual(label, "Run feature-gated remote fixture tests")
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

	def test_compatibility_artifact_requires_downloaded_root_and_forwards_pins(self) -> None:
		runner = RecordingRunner()
		with (
			mock.patch.dict(
				os.environ,
				{
					"DCMVIEW_COMPAT_CORPUS_ROOT": "/tmp/current-smoke",
					"DCMVIEW_COMPAT_OUTPUT": "/tmp/compatibility-output",
					"DCMVIEW_CORPUS_GENERATOR_REVISION": "a" * 40,
					"DCMVIEW_CORPUS_GENERATOR_ARTIFACT_SHA256": "b" * 64,
					"DCMVIEW_CORPUS_GENERATOR_ARTIFACT_SIZE_BYTES": "123",
					"DCMVIEW_CORPUS_GENERATOR_TARGET": "aarch64-apple-darwin",
					"DCMVIEW_CORPUS_GENERATOR_TOOLCHAIN": "rustc 1.88.0 (fake)",
					"DCMVIEW_CORPUS_MANIFEST_SHA256": "a" * 64,
					"DCMVIEW_CORPUS_DEFINITION_SHA256": "b" * 64,
					"DCMVIEW_CORPUS_DEFINITION_MANIFEST_SHA256": "c" * 64,
					"DCMVIEW_CORPUS_MANIFEST_SIZE_BYTES": "456",
					"DCMVIEW_CORPUS_PROFILE": "smoke",
					"DCMVIEW_CORPUS_SEED": "1",
					"DCMVIEW_CORPUS_BINDING_ID": "d" * 64,
					"DCMVIEW_CORPUS_ARCHIVE_SHA256": "e" * 64,
					"DCMVIEW_CORPUS_ARCHIVE_SIZE_BYTES": "789",
					"DCMVIEW_CORPUS_RUNTIME_IDENTITIES_SHA256": "f" * 64,
					"DCMVIEW_CORPUS_GENERATOR_VERSION": "0.3.0",
					"DCMVIEW_CORPUS_GENERATOR_FEATURES": "jpeg,  wsi",
				},
			),
			mock.patch.object(runner, "build_binary"),
			mock.patch.object(check, "run") as run,
		):
			runner.compatibility_artifact()

		label, command = run.call_args.args
		self.assertEqual(label, "Run stored external-corpus smoke against the real binary")
		self.assertIn("--corpus-root", command)
		self.assertIn("--expected-generator-feature", command)
		self.assertEqual(command[-2:], ["--expected-generator-feature", "wsi"])

	def test_compatibility_artifact_fails_closed_when_a_pin_is_missing(self) -> None:
		runner = RecordingRunner()
		with mock.patch.dict(
			os.environ,
			{"DCMVIEW_COMPAT_CORPUS_ROOT": "/tmp/current-smoke"},
			clear=True,
		):
			with self.assertRaisesRegex(check.CheckError, "complete immutable pins"):
				runner.compatibility_artifact()


if __name__ == "__main__":
	unittest.main()

from __future__ import annotations

import contextlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

from scripts import check_versions


class ReleaseVersionTests(unittest.TestCase):
	def setUp(self) -> None:
		self.directory = tempfile.TemporaryDirectory()
		self.addCleanup(self.directory.cleanup)
		self.root = Path(self.directory.name)
		(self.root / "Cargo.toml").write_text('[package]\nname = "dcmview"\nversion = "0.3.0"\n')
		(self.root / "Cargo.lock").write_text(
			'version = 4\n\n[[package]]\nname = "dependency"\nversion = "9.9.9"\n'
			'\n[[package]]\nname = "dcmview"\nversion = "0.3.0"\n'
		)
		(self.root / "pyproject.toml").write_text('[project]\nversion = "0.3.0"\n')
		for directory in ("frontend", "vscode"):
			(self.root / directory).mkdir()
			(self.root / directory / "package.json").write_text(json.dumps({"version": "0.3.0"}))
			(self.root / directory / "package-lock.json").write_text(json.dumps({
				"version": "0.3.0", "lockfileVersion": 3,
				"packages": {"": {"version": "0.3.0"}, "node_modules/dependency": {"version": "9.9.9"}},
			}))

	def run_check(self, *arguments: str) -> tuple[int, str]:
		output = io.StringIO()
		with mock.patch.object(check_versions, "REPO_ROOT", self.root), \
			mock.patch("sys.argv", ["check_versions.py", *arguments]), \
			contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
			status = check_versions.main()
		return status, output.getvalue()

	def test_matching_release_ignores_dependency_versions_and_checks_tag(self) -> None:
		self.assertEqual(self.run_check("--tag", "v0.3.0", "--print-version"), (0, "0.3.0\n"))
		status, output = self.run_check("--tag", "v0.2.13")
		self.assertEqual(status, 1)
		self.assertIn("tag v0.2.13", output)

	def test_every_manifest_and_lock_version_independently_rejects_drift(self) -> None:
		for relative in ("Cargo.toml", "Cargo.lock", "pyproject.toml", "frontend/package.json", "vscode/package.json"):
			with self.subTest(path=relative):
				path = self.root / relative
				original = path.read_text()
				path.write_text(original.replace("0.3.0", "0.2.13"))
				status, output = self.run_check()
				self.assertEqual(status, 1, output)
				self.assertIn(relative, output)
				path.write_text(original)
		for directory in ("frontend", "vscode"):
			for package_root in (False, True):
				with self.subTest(directory=directory, package_root=package_root):
					path = self.root / directory / "package-lock.json"
					original = path.read_text()
					data = json.loads(original)
					(data["packages"][""] if package_root else data)["version"] = "0.2.13"
					path.write_text(json.dumps(data))
					status, output = self.run_check()
					self.assertEqual(status, 1, output)
					self.assertIn(f"{directory}/package-lock.json", output)
					path.write_text(original)

	def test_missing_or_malformed_lock_fails_without_traceback(self) -> None:
		path = self.root / "frontend/package-lock.json"
		for invalid in ('{', '{}', '{"version":"0.3.0","packages":{}}'):
			with self.subTest(content=invalid):
				path.write_text(invalid)
				status, _ = self.run_check()
				self.assertEqual(status, 1)
		path.unlink()
		self.assertEqual(self.run_check()[0], 1)

	def test_cargo_lock_requires_one_root_package(self) -> None:
		path = self.root / "Cargo.lock"
		original = path.read_text()
		for invalid in (original.replace('name = "dcmview"', 'name = "other"'), original + original):
			with self.subTest(content=invalid):
				path.write_text(invalid)
				status, output = self.run_check()
				self.assertEqual(status, 1, output)
				self.assertIn("Cargo.lock", output)

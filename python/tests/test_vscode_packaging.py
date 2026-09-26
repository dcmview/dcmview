from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PACKAGE_VSCODE_TARGETS = REPO_ROOT / "scripts" / "package_vscode_targets.py"


def _load_packaging_module():
	spec = importlib.util.spec_from_file_location(
		"package_vscode_targets_for_test",
		PACKAGE_VSCODE_TARGETS,
	)
	assert spec is not None
	module = importlib.util.module_from_spec(spec)
	assert spec.loader is not None
	sys.modules[spec.name] = module
	spec.loader.exec_module(module)
	return module


class VSCodePackagingTests(unittest.TestCase):
	def test_extracts_windows_binary_from_zip_archive(self) -> None:
		packaging = _load_packaging_module()
		with tempfile.TemporaryDirectory(prefix="dcmview-vsix-package-") as temp_dir:
			root = Path(temp_dir)
			archive = root / "release-win32-x64" / "dcmview-0.2.8-x86_64-pc-windows-msvc.zip"
			archive.parent.mkdir()
			with zipfile.ZipFile(archive, "w") as package:
				package.writestr("dcmview.exe", b"windows-binary")
				package.writestr("README.md", "readme")

			found = packaging.find_archive(root, packaging.TARGETS["win32-x64"])
			destination = root / "vscode" / "resources" / "bin" / "win32-x64" / "dcmview.exe"
			packaging.extract_binary(found, destination, "dcmview.exe")

			self.assertEqual(destination.read_bytes(), b"windows-binary")

	def test_verifies_windows_vsix_contains_only_target_exe(self) -> None:
		packaging = _load_packaging_module()
		with tempfile.TemporaryDirectory(prefix="dcmview-vsix-verify-") as temp_dir:
			vsix = Path(temp_dir) / "dcmview-0.2.8-win32-x64.vsix"
			with zipfile.ZipFile(vsix, "w") as package:
				package.writestr("extension/package.json", "{}")
				package.writestr("extension/resources/bin/win32-x64/dcmview.exe", b"binary")

			packaging.verify_single_bundled_binary(vsix, "win32-x64", "dcmview.exe")

	def test_verifies_marketplace_readme_documentation_urls(self) -> None:
		packaging = _load_packaging_module()
		with tempfile.TemporaryDirectory(prefix="dcmview-vsix-readme-") as temp_dir:
			vsix = Path(temp_dir) / "dcmview-0.2.8-win32-x64.vsix"
			readme = "\n".join(sorted(packaging.MARKETPLACE_DOCUMENTATION_URLS))
			with zipfile.ZipFile(vsix, "w") as package:
				package.writestr("extension/readme.md", readme)

			packaging.verify_marketplace_readme(vsix)

	def test_rejects_repository_relative_marketplace_links(self) -> None:
		packaging = _load_packaging_module()
		with tempfile.TemporaryDirectory(prefix="dcmview-vsix-readme-") as temp_dir:
			vsix = Path(temp_dir) / "dcmview-0.2.8-win32-x64.vsix"
			readme = "\n".join(sorted(packaging.MARKETPLACE_DOCUMENTATION_URLS))
			with zipfile.ZipFile(vsix, "w") as package:
				package.writestr("extension/README.md", f"{readme}\n[broken](../docs/index.md)")

			with self.assertRaises(RuntimeError):
				packaging.verify_marketplace_readme(vsix)


if __name__ == "__main__":
	unittest.main()

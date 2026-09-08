#!/usr/bin/env python3
"""Run the same development checks locally and in CI."""

from __future__ import annotations

import argparse
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Callable, Sequence
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_DIR = REPO_ROOT / "tests" / "fixtures"
FRONTEND_FIXTURES = (
	FIXTURE_DIR / "golden-uncompressed-u16-multiframe.dcm",
	FIXTURE_DIR / "golden-no-pixels-sr.dcm",
)
VSCODE_TEST_VERSION = "1.90.2"


class CheckError(RuntimeError):
	"""A check profile could not establish one of its invariants."""


def executable(name: str) -> str:
	resolved = shutil.which(name)
	if resolved is None:
		raise SystemExit(f"required executable is not on PATH: {name}")
	return resolved


def run(
	label: str,
	command: Sequence[str],
	*,
	env: dict[str, str] | None = None,
) -> None:
	print(f"\n==> {label}", flush=True)
	subprocess.run(
		list(command),
		cwd=REPO_ROOT,
		env=env,
		check=True,
	)


def cargo_env() -> dict[str, str]:
	env = os.environ.copy()
	env["DCMVIEW_SKIP_FRONTEND_BUILD"] = "1"
	return env


def fixture_snapshot() -> dict[str, str]:
	"""Return stable content hashes for every committed-fixture candidate."""
	if not FIXTURE_DIR.is_dir():
		return {}

	return {
		path.relative_to(FIXTURE_DIR).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
		for path in sorted(FIXTURE_DIR.rglob("*"))
		if path.is_file()
	}


def check_fixture_snapshot(before: dict[str, str], after: dict[str, str]) -> None:
	if before == after:
		return

	changed = []
	for name in sorted(before.keys() | after.keys()):
		if name not in before:
			change = "created"
		elif name not in after:
			change = "removed"
		else:
			change = "changed"
		changed.append(f"{change}: tests/fixtures/{name}")

	raise CheckError(
		"fixture generation changed the working tree; regenerate and commit these outputs:\n"
		+ "\n".join(f"  - {entry}" for entry in changed)
	)


class CheckRunner:
	def __init__(self, *, install: bool) -> None:
		self.install = install
		self._frontend_built = False
		self._frontend_installed = False
		self._vscode_installed = False
		self._binary_built = False

	@property
	def python(self) -> str:
		return sys.executable

	@property
	def npm(self) -> str:
		return executable("npm")

	@property
	def cargo(self) -> str:
		return executable("cargo")

	def install_frontend(self) -> None:
		if self.install and not self._frontend_installed:
			run("Install frontend dependencies", [self.npm, "--prefix", "frontend", "ci"])
			self._frontend_installed = True

	def install_vscode(self) -> None:
		if self.install and not self._vscode_installed:
			run("Install VS Code extension dependencies", [self.npm, "--prefix", "vscode", "ci"])
			self._vscode_installed = True

	def build_frontend(self) -> None:
		if self._frontend_built:
			return
		self.install_frontend()
		run("Build frontend assets", [self.npm, "--prefix", "frontend", "run", "build"])
		self._frontend_built = True

	def versions(self) -> None:
		run("Check package version parity", [self.python, "scripts/check_versions.py"])

	def frontend(self) -> None:
		self.install_frontend()
		run(
			"Check generated frontend contracts",
			[self.npm, "--prefix", "frontend", "run", "check:contracts"],
		)
		run("Typecheck Svelte and TypeScript", [self.npm, "--prefix", "frontend", "run", "typecheck"])
		run("Run frontend behavior tests", [self.npm, "--prefix", "frontend", "run", "test"])
		self.build_frontend()

	def rust_lint(self) -> None:
		self.build_frontend()
		env = cargo_env()
		run("Check Rust formatting", [self.cargo, "fmt", "--all", "--", "--check"], env=env)
		run(
			"Run strict Rust lints",
			[self.cargo, "clippy", "--all-targets", "--locked", "--", "-D", "warnings"],
			env=env,
		)

	def rust_test(self) -> None:
		self.build_frontend()
		env = cargo_env()
		fixtures_before = fixture_snapshot()
		run(
			"Regenerate deterministic DICOM fixtures",
			[self.cargo, "run", "--locked", "--example", "generate_test_fixtures"],
			env=env,
		)
		print("\n==> Check deterministic DICOM fixture drift", flush=True)
		check_fixture_snapshot(fixtures_before, fixture_snapshot())
		run("Run Rust tests", [self.cargo, "test", "--locked"], env=env)

	def rust(self) -> None:
		self.rust_lint()
		self.rust_test()

	def python_unit(self) -> None:
		run(
			"Run Python unit and packaging-helper tests",
			[self.python, "-m", "unittest", "discover", "-s", "python/tests"],
		)

	def python_integration(self) -> None:
		self.build_binary()
		run(
			"Run Python wrapper binary integration",
			[self.python, "-m", "unittest", "python.tests.wrapper_binary_integration"],
		)
		self.run_smoke()

	def vscode_compile(self) -> None:
		self.install_vscode()
		run("Compile the VS Code extension", [self.npm, "--prefix", "vscode", "run", "compile"])

	def vscode_integration(self) -> None:
		self.install_vscode()
		env = os.environ.copy()
		env.setdefault("DCMVIEW_VSCODE_TEST_VERSION", VSCODE_TEST_VERSION)
		run("Run VS Code extension integration tests", [self.npm, "--prefix", "vscode", "test"], env=env)

	def smoke(self) -> None:
		self.build_binary()
		self.run_smoke()

	def build_binary(self) -> None:
		if self._binary_built:
			return
		self.build_frontend()
		run(
			"Build the real dcmview binary",
			[self.cargo, "build", "--locked", "--bin", "dcmview"],
			env=cargo_env(),
		)
		self._binary_built = True

	def run_smoke(self) -> None:
		binary_name = "dcmview.exe" if os.name == "nt" else "dcmview"
		binary = REPO_ROOT / "target" / "debug" / binary_name
		run(
			"Smoke test the built binary",
			[
				self.python,
				"scripts/smoke_release_binary.py",
				str(binary),
				*(str(path) for path in FRONTEND_FIXTURES),
			],
		)

	def compatibility_artifact(self) -> None:
		"""Run the viewer-owned consumer against a stored smoke artifact."""
		corpus_root = os.environ.get("DCMVIEW_COMPAT_CORPUS_ROOT")
		if not corpus_root:
			raise CheckError(
				"DCMVIEW_COMPAT_CORPUS_ROOT must name a downloaded smoke artifact; "
				"this profile never generates a corpus"
			)
		self.build_binary()
		binary_name = "dcmview.exe" if os.name == "nt" else "dcmview"
		binary = REPO_ROOT / "target" / "debug" / binary_name
		output = os.environ.get("DCMVIEW_COMPAT_OUTPUT")
		if not output:
			output = tempfile.mkdtemp(prefix="dcmview-compatibility-artifact-")
		command = [
			self.python,
			"scripts/compatibility/run.py",
			"--corpus-root",
			corpus_root,
			"--binary",
			str(binary),
			"--output",
			output,
		]
		pins = (
			("DCMVIEW_CORPUS_MANIFEST_SHA256", "--expected-manifest-sha256"),
			("DCMVIEW_CORPUS_DEFINITION_SHA256", "--expected-corpus-definition-sha256"),
			("DCMVIEW_CORPUS_GENERATOR_VERSION", "--expected-generator-version"),
		)
		for variable, option in pins:
			value = os.environ.get(variable)
			if value:
				command.extend((option, value))
		features = os.environ.get("DCMVIEW_CORPUS_GENERATOR_FEATURES", "")
		for feature in filter(None, (item.strip() for item in features.split(","))):
			command.extend(("--expected-generator-feature", feature))
		run("Run stored external-corpus smoke against the real binary", command)

	def quick(self) -> None:
		self.versions()
		self.frontend()
		self.rust_lint()
		self.python_unit()

	def core(self) -> None:
		self.versions()
		self.frontend()
		self.rust()
		self.python_unit()
		self.vscode_compile()

	def e2e(self) -> None:
		self.core()
		self.python_integration()
		self.vscode_integration()

	def external(self) -> None:
		self.build_frontend()
		run(
			"Run feature-gated remote fixture tests",
			[
				self.cargo,
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
			env=cargo_env(),
		)

	def marketing(self) -> None:
		run("Validate marketing source and capture manifests", [self.python, "scripts/marketing_media.py", "validate"])
		node = executable("node")
		run("Check browser capture harness syntax", [node, "--check", "marketing/capture_browser.mjs"])
		run("Check VS Code capture harness syntax", [node, "--check", "marketing/capture_vscode.mjs"])
		run(
			"Run marketing-media unit tests",
			[self.python, "-m", "unittest", "python.tests.test_marketing_media"],
		)
		published_lock = REPO_ROOT / "media" / "marketing" / "media-lock.json"
		if published_lock.is_file():
			run(
				"Reject stale or modified published media",
				[
					self.python, "scripts/marketing_media.py", "verify", "--offline",
					"--bundle", "media/marketing",
				],
			)
		else:
			print("\n==> No approved media bundle is committed yet; drift gate is inactive", flush=True)


def parse_args() -> argparse.Namespace:
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument(
		"profile",
		choices=[
			"quick",
			"frontend",
			"frontend-assets",
			"rust-lint",
			"rust-test",
			"rust",
			"python-unit",
			"python-integration",
			"vscode",
			"vscode-integration",
			"smoke",
			"compatibility-artifact",
			"core",
			"e2e",
			"external",
			"marketing",
		],
		help="check profile to execute",
	)
	parser.add_argument(
		"--install",
		action="store_true",
		help="run npm ci for profiles which use frontend or VS Code dependencies",
	)
	return parser.parse_args()


def main() -> int:
	args = parse_args()
	runner = CheckRunner(install=args.install)
	profiles: dict[str, Callable[[], None]] = {
		"quick": runner.quick,
		"frontend": runner.frontend,
		"frontend-assets": runner.build_frontend,
		"rust-lint": runner.rust_lint,
		"rust-test": runner.rust_test,
		"rust": runner.rust,
		"python-unit": runner.python_unit,
		"python-integration": runner.python_integration,
		"vscode": runner.vscode_compile,
		"vscode-integration": runner.vscode_integration,
		"smoke": runner.smoke,
		"compatibility-artifact": runner.compatibility_artifact,
		"core": runner.core,
		"e2e": runner.e2e,
		"external": runner.external,
		"marketing": runner.marketing,
	}

	try:
		profiles[args.profile]()
	except subprocess.CalledProcessError as error:
		print(
			f"\ncheck profile {args.profile!r} failed with exit code {error.returncode}",
			file=sys.stderr,
		)
		return error.returncode
	except CheckError as error:
		print(f"\ncheck profile {args.profile!r} failed:\n{error}", file=sys.stderr)
		return 1

	print(f"\ncheck profile {args.profile!r} passed", flush=True)
	return 0


if __name__ == "__main__":
	raise SystemExit(main())

#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import stat
import subprocess
import tarfile
import zipfile
from dataclasses import dataclass

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]
MARKETPLACE_DOCUMENTATION_URLS = {
	"https://github.com/dcmview/dcmview/blob/main/docs/configuration.md",
	"https://github.com/dcmview/dcmview/blob/main/docs/index.md",
	"https://github.com/dcmview/dcmview/blob/main/docs/python.md",
	"https://github.com/dcmview/dcmview/blob/main/docs/troubleshooting.md",
}

@dataclass(frozen=True)
class TargetSpec:
	archive_suffix: str
	archive_extension: str
	binary_name: str


TARGETS = {
	"linux-x64": TargetSpec("x86_64-unknown-linux-gnu", ".tar.gz", "dcmview"),
	"darwin-x64": TargetSpec("x86_64-apple-darwin", ".tar.gz", "dcmview"),
	"darwin-arm64": TargetSpec("aarch64-apple-darwin", ".tar.gz", "dcmview"),
	"win32-x64": TargetSpec("x86_64-pc-windows-msvc", ".zip", "dcmview.exe"),
}


def find_archive(artifacts_dir: pathlib.Path, spec: TargetSpec) -> pathlib.Path:
	candidates = sorted(artifacts_dir.glob(f"**/dcmview-*-{spec.archive_suffix}{spec.archive_extension}"))
	if len(candidates) != 1:
		found = ", ".join(str(candidate) for candidate in candidates) or "none"
		raise RuntimeError(f"expected one release archive for {spec.archive_suffix}, found {found}")
	return candidates[0]


def extract_binary(archive: pathlib.Path, destination: pathlib.Path, binary_name: str) -> None:
	destination.parent.mkdir(parents=True, exist_ok=True)
	if archive.suffix == ".zip":
		with zipfile.ZipFile(archive) as package:
			member = next(
				(name for name in package.namelist() if pathlib.PurePosixPath(name).name == binary_name),
				None,
			)
			if member is None:
				raise RuntimeError(f"{archive} does not contain a {binary_name} binary")
			with package.open(member) as source, destination.open("wb") as output:
				shutil.copyfileobj(source, output)
		return set_executable(destination)

	with tarfile.open(archive, "r:gz") as tar:
		member = next(
			(member for member in tar.getmembers() if pathlib.PurePosixPath(member.name).name == binary_name),
			None,
		)
		if member is None:
			raise RuntimeError(f"{archive} does not contain a {binary_name} binary")
		source = tar.extractfile(member)
		if source is None:
			raise RuntimeError(f"{archive} contains an unreadable {binary_name} binary")
		with destination.open("wb") as output:
			shutil.copyfileobj(source, output)
	set_executable(destination)


def set_executable(destination: pathlib.Path) -> None:
	mode = destination.stat().st_mode
	destination.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def read_extension_version(extension_root: pathlib.Path) -> str:
	package_json = json.loads((extension_root / "package.json").read_text(encoding="utf-8"))
	version = package_json.get("version")
	if not isinstance(version, str) or not version:
		raise RuntimeError("vscode/package.json does not define a version")
	return version


def clear_staged_binaries(extension_root: pathlib.Path) -> None:
	for binary in (extension_root / "resources" / "bin").glob("*/dcmview*"):
		binary.unlink()


def package_target(
	*,
	target: str,
	spec: TargetSpec,
	artifacts_dir: pathlib.Path,
	extension_root: pathlib.Path,
	out_dir: pathlib.Path,
	version: str,
) -> pathlib.Path:
	clear_staged_binaries(extension_root)
	archive = find_archive(artifacts_dir, spec)
	destination = extension_root / "resources" / "bin" / target / spec.binary_name
	extract_binary(archive, destination, spec.binary_name)

	out_dir.mkdir(parents=True, exist_ok=True)
	vsix = out_dir / f"dcmview-{version}-{target}.vsix"
	subprocess.run(
		[
			"npm",
			"exec",
			"--",
			"vsce",
			"package",
			"--target",
			target,
			"--no-dependencies",
			"--out",
			str(vsix),
		],
		cwd=extension_root,
		check=True,
	)
	verify_single_bundled_binary(vsix, target, spec.binary_name)
	verify_marketplace_readme(vsix)
	return vsix


def verify_single_bundled_binary(vsix: pathlib.Path, target: str, binary_name: str) -> None:
	expected = f"extension/resources/bin/{target}/{binary_name}"
	with zipfile.ZipFile(vsix) as package:
		bundled = sorted(
			name
			for name in package.namelist()
			if name.startswith("extension/resources/bin/")
			and pathlib.PurePosixPath(name).name in {"dcmview", "dcmview.exe"}
		)
	if bundled != [expected]:
		raise RuntimeError(f"{vsix} should contain only {expected}; found {bundled}")


def verify_marketplace_readme(vsix: pathlib.Path) -> None:
	with zipfile.ZipFile(vsix) as package:
		readme_name = next(
			(name for name in package.namelist() if name.casefold() == "extension/readme.md"),
			None,
		)
		if readme_name is None:
			raise RuntimeError(f"{vsix} does not contain extension/README.md")
		readme = package.read(readme_name).decode("utf-8")

	missing = sorted(url for url in MARKETPLACE_DOCUMENTATION_URLS if url not in readme)
	if missing:
		raise RuntimeError(f"{vsix} README is missing Marketplace documentation URLs: {missing}")
	if "../docs/" in readme:
		raise RuntimeError(f"{vsix} README contains repository-relative documentation links")


def main() -> int:
	parser = argparse.ArgumentParser(description="Package target-specific dcmview VSIX artifacts")
	parser.add_argument(
		"--artifacts-dir",
		type=pathlib.Path,
		default=REPO_ROOT / "artifacts",
		help="Directory containing release-* artifact folders downloaded from GitHub Actions",
	)
	parser.add_argument(
		"--extension-root",
		type=pathlib.Path,
		default=REPO_ROOT / "vscode",
		help="VS Code extension root",
	)
	parser.add_argument(
		"--out-dir",
		type=pathlib.Path,
		default=REPO_ROOT / "dist",
		help="Directory for generated VSIX artifacts",
	)
	parser.add_argument(
		"--target",
		action="append",
		choices=sorted(TARGETS),
		help="Target to package; may be repeated. Defaults to all supported targets.",
	)
	parser.add_argument(
		"--skip-compile",
		action="store_true",
		help="Skip npm compile before packaging",
	)
	args = parser.parse_args()

	extension_root = args.extension_root.resolve()
	targets = args.target or sorted(TARGETS)
	version = read_extension_version(extension_root)

	if not args.skip_compile:
		subprocess.run(["npm", "run", "compile"], cwd=extension_root, check=True)

	try:
		for target in targets:
			vsix = package_target(
				target=target,
				spec=TARGETS[target],
				artifacts_dir=args.artifacts_dir,
				extension_root=extension_root,
				out_dir=args.out_dir,
				version=version,
			)
			print(f"packaged {vsix}")
	finally:
		clear_staged_binaries(extension_root)

	return 0


if __name__ == "__main__":
	raise SystemExit(main())

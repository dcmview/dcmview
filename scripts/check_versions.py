#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parents[1]


def read_toml_version(path: pathlib.Path, section: str) -> str:
	try:
		import tomllib
	except ModuleNotFoundError:
		return read_toml_version_fallback(path, section)

	with path.open("rb") as file:
		data = tomllib.load(file)
	version = data.get(section, {}).get("version")
	if not isinstance(version, str) or not version:
		raise ValueError(f"{path.name} does not define [{section}].version")
	return version


def read_toml_version_fallback(path: pathlib.Path, section: str) -> str:
	in_section = False
	version_pattern = re.compile(r'^version\s*=\s*"([^"]+)"\s*$')

	for raw_line in path.read_text(encoding="utf-8").splitlines():
		line = raw_line.strip()
		if not line or line.startswith("#"):
			continue
		if line.startswith("[") and line.endswith("]"):
			in_section = line == f"[{section}]"
			continue
		if in_section:
			match = version_pattern.match(line)
			if match:
				return match.group(1)

	raise ValueError(f"{path.name} does not define [{section}].version")


def normalize_tag(tag: str) -> str:
	if not tag.startswith("v"):
		raise ValueError(f"release tag must start with 'v': {tag}")
	version = tag[1:]
	if not version:
		raise ValueError("release tag is missing a version after 'v'")
	return version


def read_package_json_version(path: pathlib.Path, *, package_root: bool = False) -> str:
	data = json.loads(path.read_text(encoding="utf-8"))
	if package_root:
		packages = data.get("packages") if isinstance(data, dict) else None
		data = packages.get("") if isinstance(packages, dict) else None
	version = data.get("version") if isinstance(data, dict) else None
	if not isinstance(version, str) or not version:
		field = 'packages[""].version' if package_root else "version"
		raise ValueError(f"{path} does not define {field}")
	return version


def read_cargo_lock_version(path: pathlib.Path) -> str:
	# Cargo's generated lockfile uses scalar name/version entries in package
	# blocks. Read just the root package, also on Python 3.9 without tomllib.
	packages = re.split(r"(?m)^\[\[package\]\]\s*$", path.read_text(encoding="utf-8"))[1:]
	roots = [block for block in packages if re.search(r'^name\s*=\s*"dcmview"\s*$', block, re.M)]
	if len(roots) != 1:
		raise ValueError(f"{path} must contain exactly one dcmview package")
	version = re.search(r'^version\s*=\s*"([^\"]+)"\s*$', roots[0], re.M)
	if version is None:
		raise ValueError(f"{path} does not define the dcmview package version")
	return version.group(1)


def release_versions(root: pathlib.Path) -> dict[str, str]:
	versions = {
		"Cargo.toml": read_toml_version(root / "Cargo.toml", "package"),
		"Cargo.lock": read_cargo_lock_version(root / "Cargo.lock"),
		"pyproject.toml": read_toml_version(root / "pyproject.toml", "project"),
	}
	for directory in ("frontend", "vscode"):
		for filename in ("package.json", "package-lock.json"):
			name = f"{directory}/{filename}"
			versions[name] = read_package_json_version(root / name)
			if filename == "package-lock.json":
				versions[f'{name} packages[""].version'] = read_package_json_version(root / name, package_root=True)
	return versions


def main() -> int:
	parser = argparse.ArgumentParser(description="Validate dcmview release versions")
	parser.add_argument(
		"--tag",
		help="Release tag to compare against Cargo.toml, e.g. v0.1.0",
	)
	parser.add_argument(
		"--print-version",
		action="store_true",
		help="Print only the canonical Cargo version",
	)
	args = parser.parse_args()

	try:
		versions = release_versions(REPO_ROOT)
	except (ValueError, OSError) as error:
		print(str(error), file=sys.stderr)
		return 1

	cargo_version = versions["Cargo.toml"]
	if any(version != cargo_version for version in versions.values()):
		print(
			"version mismatch: " + ", ".join(f"{name} has {version}" for name, version in versions.items()),
			file=sys.stderr,
		)
		return 1

	if args.tag is not None:
		try:
			tag_version = normalize_tag(args.tag)
		except ValueError as error:
			print(str(error), file=sys.stderr)
			return 1
		if tag_version != cargo_version:
			print(
				f"version mismatch: tag {args.tag} resolves to {tag_version}, "
				f"Cargo.toml has {cargo_version}",
				file=sys.stderr,
			)
			return 1

	if args.print_version:
		print(cargo_version)
	else:
		print(f"dcmview version ok: {cargo_version}")
	return 0


if __name__ == "__main__":
	raise SystemExit(main())

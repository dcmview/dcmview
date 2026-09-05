#!/usr/bin/env python3
"""Fetch, capture, verify, and publish release marketing media."""

from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import hashlib
import json
import os
import queue
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
from collections.abc import Iterable, Sequence
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SOURCES = REPO_ROOT / "marketing" / "sources.json"
DEFAULT_CAPTURES = REPO_ROOT / "marketing" / "captures.json"
DEFAULT_SOURCE_ROOT = REPO_ROOT / "marketing-source-data"
DEFAULT_REVIEW_ROOT = REPO_ROOT / "marketing-review"
SOURCE_INVENTORY_NAME = "SOURCE_FILES.json"
SOURCE_LINKAGE_NAME = "SOURCE_LINKAGE.md"
MEDIA_LOCK_NAME = "media-lock.json"
ATTRIBUTION_NAME = "ATTRIBUTION.md"
CAPTURE_INPUTS = (
	"frontend/src",
	"src/api/contracts.rs",
	"src/geometry.rs",
	"src/pixels/segmentation.rs",
	"src/semantic.rs",
	"vscode/src",
	"vscode/package.json",
	"marketing",
)


class MarketingMediaError(RuntimeError):
	"""The requested media workflow could not establish its invariants."""


def load_json(path: Path) -> dict[str, Any]:
	try:
		value = json.loads(path.read_text(encoding="utf-8"))
	except FileNotFoundError as error:
		raise MarketingMediaError(f"required manifest does not exist: {path}") from error
	except json.JSONDecodeError as error:
		raise MarketingMediaError(f"invalid JSON in {path}: {error}") from error
	if not isinstance(value, dict):
		raise MarketingMediaError(f"manifest root must be an object: {path}")
	return value


def sha256_file(path: Path) -> str:
	hasher = hashlib.sha256()
	with path.open("rb") as source:
		for chunk in iter(lambda: source.read(1024 * 1024), b""):
			hasher.update(chunk)
	return hasher.hexdigest()


def manifest_sha256(path: Path) -> str:
	return sha256_file(path)


def require_string(value: Any, field: str) -> str:
	if not isinstance(value, str) or not value.strip():
		raise MarketingMediaError(f"{field} must be a non-empty string")
	return value


def require_positive_int(value: Any, field: str) -> int:
	if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
		raise MarketingMediaError(f"{field} must be a positive integer")
	return value


def source_groups(manifest: dict[str, Any]) -> list[dict[str, Any]]:
	if manifest.get("schema_version") != 1:
		raise MarketingMediaError("marketing source manifest schema_version must be 1")
	groups = manifest.get("groups")
	if not isinstance(groups, list) or not groups:
		raise MarketingMediaError("marketing source manifest must contain groups")

	seen_groups: set[str] = set()
	seen_series: set[str] = set()
	validated: list[dict[str, Any]] = []
	for group_index, raw_group in enumerate(groups):
		if not isinstance(raw_group, dict):
			raise MarketingMediaError(f"groups[{group_index}] must be an object")
		group_id = require_string(raw_group.get("id"), f"groups[{group_index}].id")
		if group_id in seen_groups:
			raise MarketingMediaError(f"duplicate marketing source group: {group_id}")
		seen_groups.add(group_id)
		for field in ("title", "collection", "dataset_title", "attribution_party", "year_version", "doi"):
			require_string(raw_group.get(field), f"groups[{group_index}].{field}")
		patient_ids = raw_group.get("patient_ids")
		if not isinstance(patient_ids, list) or not all(
			isinstance(value, str) and value for value in patient_ids
		):
			raise MarketingMediaError(f"groups[{group_index}].patient_ids must be strings")
		series = raw_group.get("series")
		if not isinstance(series, list) or not series:
			raise MarketingMediaError(f"groups[{group_index}].series must not be empty")
		seen_roles: set[str] = set()
		for series_index, raw_series in enumerate(series):
			if not isinstance(raw_series, dict):
				raise MarketingMediaError(
					f"groups[{group_index}].series[{series_index}] must be an object"
				)
			prefix = f"groups[{group_index}].series[{series_index}]"
			role = require_string(raw_series.get("role"), f"{prefix}.role")
			uid = require_string(
				raw_series.get("series_instance_uid"), f"{prefix}.series_instance_uid"
			)
			require_positive_int(raw_series.get("expected_files"), f"{prefix}.expected_files")
			if role in seen_roles:
				raise MarketingMediaError(f"duplicate series role {role!r} in group {group_id}")
			if uid in seen_series:
				raise MarketingMediaError(f"duplicate Series Instance UID: {uid}")
			seen_roles.add(role)
			seen_series.add(uid)
		validated.append(raw_group)
	return validated


def select_groups(groups: Sequence[dict[str, Any]], requested: Sequence[str]) -> list[dict[str, Any]]:
	if not requested:
		return list(groups)
	by_id = {str(group["id"]): group for group in groups}
	unknown = sorted(set(requested) - by_id.keys())
	if unknown:
		raise MarketingMediaError(f"unknown source group(s): {', '.join(unknown)}")
	return [by_id[group_id] for group_id in requested]


def ensure_ignored(path: Path) -> None:
	path.mkdir(parents=True, exist_ok=True)
	probe = path / ".dcmview-ignore-probe"
	result = subprocess.run(
		["git", "check-ignore", "--quiet", str(probe)],
		cwd=REPO_ROOT,
		check=False,
	)
	if result.returncode != 0:
		raise MarketingMediaError(
			f"source root is not ignored by git; refusing to download DICOM data: {path}"
		)


def dicom_files(path: Path) -> list[Path]:
	return sorted(
		candidate
		for candidate in path.rglob("*")
		if candidate.is_file() and candidate.suffix.casefold() in {".dcm", ".dicom"}
	)


def find_series_root(source_root: Path, group: dict[str, Any], series: dict[str, Any]) -> Path:
	group_root = source_root / str(group["id"])
	uid = str(series["series_instance_uid"])
	candidates = sorted(
		path
		for path in group_root.rglob("*")
		if path.is_dir() and (path.name == uid or path.name.endswith(f"_{uid}"))
	)
	if len(candidates) != 1:
		raise MarketingMediaError(
			f"{group['id']}/{series['role']} expected one directory for Series {uid}, "
			f"found {len(candidates)}"
		)
	return candidates[0]


def inventory_series(
	*,
	source_root: Path,
	group: dict[str, Any],
	series: dict[str, Any],
) -> list[dict[str, Any]]:
	series_root = find_series_root(source_root, group, series)
	files = dicom_files(series_root)
	expected = int(series["expected_files"])
	if len(files) != expected:
		raise MarketingMediaError(
			f"{group['id']}/{series['role']} expected {expected} DICOM files, found {len(files)}"
		)
	return [
		{
			"path": path.relative_to(source_root).as_posix(),
			"bytes": path.stat().st_size,
			"sha256": sha256_file(path),
			"group": group["id"],
			"series_role": series["role"],
			"series_instance_uid": series["series_instance_uid"],
		}
		for path in files
	]


def write_source_inventory(
	*,
	source_root: Path,
	sources_path: Path,
	groups: Sequence[dict[str, Any]],
) -> Path:
	entries = [
		entry
		for group in groups
		for series in group["series"]
		for entry in inventory_series(source_root=source_root, group=group, series=series)
	]
	inventory = {
		"schema_version": 1,
		"sources_manifest_sha256": manifest_sha256(sources_path),
		"file_count": len(entries),
		"total_bytes": sum(int(entry["bytes"]) for entry in entries),
		"files": entries,
	}
	destination = source_root / SOURCE_INVENTORY_NAME
	destination.write_text(json.dumps(inventory, indent=2, sort_keys=True) + "\n", encoding="utf-8")
	write_source_linkage(source_root=source_root, groups=groups, inventory=inventory)
	return destination


def write_source_linkage(
	*, source_root: Path, groups: Sequence[dict[str, Any]], inventory: dict[str, Any]
) -> Path:
	lines = [
		"# DICOM marketing source linkage",
		"",
		"This ignored directory contains public DICOM source material. Do not commit its contents.",
		"Each downloaded file is content-addressed in `SOURCE_FILES.json`.",
		"",
	]
	for group in groups:
		lines.extend(
			[
				f"## {group['title']}",
				"",
				f"- Dataset: {group['dataset_title']}",
				f"- Collection: `{group['collection']}`",
				f"- Attribution: {group['attribution_party']}",
				f"- Version: {group['year_version']}",
				f"- DOI: {group['doi']}",
				f"- Public subject identifier(s): {', '.join(f'`{value}`' for value in group['patient_ids'])}",
				"- Series:",
			]
		)
		for series in group["series"]:
			lines.append(
				f"  - `{series['role']}` — `{series['series_instance_uid']}` "
				f"({series['expected_files']} file(s))"
			)
		lines.append("")
	lines.extend(
		[
			"## Local inventory",
			"",
			f"- Files: {inventory['file_count']}",
			f"- Bytes: {inventory['total_bytes']}",
			"- Per-file relative path, size, SHA-256, source group, role, and Series Instance UID: `SOURCE_FILES.json`",
			"",
		]
	)
	destination = source_root / SOURCE_LINKAGE_NAME
	destination.write_text("\n".join(lines), encoding="utf-8")
	return destination


def require_executable(name: str, installation_hint: str) -> str:
	resolved = shutil.which(name)
	if resolved is None:
		raise MarketingMediaError(f"required executable is unavailable: {name}\n{installation_hint}")
	return resolved


def fetch_sources(args: argparse.Namespace) -> None:
	sources_path = args.sources.resolve()
	manifest = load_json(sources_path)
	groups = select_groups(source_groups(manifest), args.group)
	source_root = args.source_root.resolve()
	ensure_ignored(source_root)
	idc = require_executable(
		"idc",
		"Install the pinned dependency with: python -m pip install -r marketing/requirements.txt",
	)

	for group in groups:
		for series in group["series"]:
			destination = source_root / str(group["id"])
			destination.mkdir(parents=True, exist_ok=True)
			command = [
				idc,
				"download-from-selection",
				"--series-instance-uid",
				str(series["series_instance_uid"]),
				"--download-dir",
				str(destination),
				"--dir-template",
				"%SeriesInstanceUID",
				"--use-s5cmd-sync",
			]
			print(f"\n==> Fetch {group['id']} / {series['role']}", flush=True)
			subprocess.run(command, cwd=REPO_ROOT, check=True)

	inventory_path = write_source_inventory(
		source_root=source_root,
		sources_path=sources_path,
		groups=groups,
	)
	print(f"wrote source inventory: {inventory_path}")


def inventory_sources(args: argparse.Namespace) -> None:
	sources_path = args.sources.resolve()
	groups = select_groups(source_groups(load_json(sources_path)), args.group)
	source_root = args.source_root.resolve()
	ensure_ignored(source_root)
	inventory_path = write_source_inventory(
		source_root=source_root, sources_path=sources_path, groups=groups
	)
	print(f"inventoried existing source files: {inventory_path}")


def verify_source_inventory(args: argparse.Namespace) -> None:
	sources_path = args.sources.resolve()
	manifest = load_json(sources_path)
	groups = select_groups(source_groups(manifest), args.group)
	source_root = args.source_root.resolve()
	ensure_ignored(source_root)
	inventory_path = source_root / SOURCE_INVENTORY_NAME
	inventory = load_json(inventory_path)
	if inventory.get("schema_version") != 1:
		raise MarketingMediaError("source inventory schema_version must be 1")
	if inventory.get("sources_manifest_sha256") != manifest_sha256(sources_path):
		raise MarketingMediaError("source inventory was generated from a different sources manifest")
	recorded = inventory.get("files")
	if not isinstance(recorded, list):
		raise MarketingMediaError("source inventory files must be an array")
	selected_group_ids = {str(group["id"]) for group in groups}
	selected_records = [entry for entry in recorded if entry.get("group") in selected_group_ids]
	expected_count = sum(
		int(series["expected_files"])
		for group in groups
		for series in group["series"]
	)
	if len(selected_records) != expected_count:
		raise MarketingMediaError(
			f"source inventory expected {expected_count} selected files, found {len(selected_records)}"
		)

	errors: list[str] = []
	for entry in selected_records:
		relative = entry.get("path")
		if not isinstance(relative, str):
			errors.append("inventory entry has no path")
			continue
		path = source_root / relative
		if not path.is_file():
			errors.append(f"missing: {relative}")
			continue
		actual_size = path.stat().st_size
		if actual_size != entry.get("bytes"):
			errors.append(f"size mismatch: {relative}")
			continue
		if sha256_file(path) != entry.get("sha256"):
			errors.append(f"SHA-256 mismatch: {relative}")
	if errors:
		raise MarketingMediaError("source verification failed:\n  - " + "\n  - ".join(errors))
	print(f"verified {len(selected_records)} DICOM source files against {inventory_path}")


def capture_scenes(manifest: dict[str, Any]) -> tuple[dict[str, Any], list[dict[str, Any]]]:
	if manifest.get("schema_version") != 1:
		raise MarketingMediaError("marketing capture manifest schema_version must be 1")
	viewport = manifest.get("viewport")
	if not isinstance(viewport, dict):
		raise MarketingMediaError("capture viewport must be an object")
	for field in ("width", "height", "device_scale_factor"):
		require_positive_int(viewport.get(field), f"viewport.{field}")
	defaults = {
		"viewport": viewport,
		"theme": require_string(manifest.get("theme"), "theme"),
		"locale": require_string(manifest.get("locale"), "locale"),
	}
	scenes = manifest.get("scenes")
	if not isinstance(scenes, list) or not scenes:
		raise MarketingMediaError("capture manifest must contain scenes")
	seen_ids: set[str] = set()
	seen_outputs: set[str] = set()
	validated: list[dict[str, Any]] = []
	for index, raw_scene in enumerate(scenes):
		if not isinstance(raw_scene, dict):
			raise MarketingMediaError(f"scenes[{index}] must be an object")
		scene = {**defaults, **raw_scene}
		scene_id = require_string(scene.get("id"), f"scenes[{index}].id")
		output = require_string(scene.get("output"), f"scenes[{index}].output")
		if Path(output).name != output:
			raise MarketingMediaError(f"scene output must be a filename: {output}")
		if scene_id in seen_ids or output in seen_outputs:
			raise MarketingMediaError(f"duplicate capture scene id or output: {scene_id}")
		if scene.get("kind") not in {"screenshot", "gif"}:
			raise MarketingMediaError(f"unsupported capture kind for {scene_id}")
		if scene.get("surface", "browser") not in {"browser", "vscode"}:
			raise MarketingMediaError(f"unsupported capture surface for {scene_id}")
		require_string(scene.get("group"), f"scenes[{index}].group")
		require_string(scene.get("series_role"), f"scenes[{index}].series_role")
		if not isinstance(scene.get("modifications"), list) or not all(
			isinstance(value, str) and value for value in scene["modifications"]
		):
			raise MarketingMediaError(f"scenes[{index}].modifications must be strings")
		seen_ids.add(scene_id)
		seen_outputs.add(output)
		validated.append(scene)
	return defaults, validated


def resolve_scenes(
	*, captures: dict[str, Any], sources: dict[str, Any], requested: Sequence[str]
) -> list[dict[str, Any]]:
	_, scenes = capture_scenes(captures)
	groups = {str(group["id"]): group for group in source_groups(sources)}
	by_id = {str(scene["id"]): scene for scene in scenes}
	unknown = sorted(set(requested) - by_id.keys())
	if unknown:
		raise MarketingMediaError(f"unknown capture scene(s): {', '.join(unknown)}")
	selected = [by_id[value] for value in requested] if requested else scenes
	resolved: list[dict[str, Any]] = []
	for scene in selected:
		group = groups.get(str(scene["group"]))
		if group is None:
			raise MarketingMediaError(f"scene {scene['id']} references an unknown source group")
		series = next(
			(candidate for candidate in group["series"] if candidate["role"] == scene["series_role"]),
			None,
		)
		if series is None:
			raise MarketingMediaError(
				f"scene {scene['id']} references unknown role {scene['series_role']}"
			)
		resolved.append(
			{
				**scene,
				"surface": scene.get("surface", "browser"),
				"series_instance_uid": series["series_instance_uid"],
				"preferred_filename": scene.get("preferred_filename", series.get("preferred_filename")),
				"allowed_patient_ids": group["patient_ids"],
				"source": {
					"group": group["id"],
					"title": group["title"],
					"dataset_title": group["dataset_title"],
					"attribution_party": group["attribution_party"],
					"year_version": group["year_version"],
					"doi": group["doi"],
				},
			}
		)
	return resolved


def tracked_input_digest(repo_root: Path = REPO_ROOT) -> tuple[str, list[str]]:
	result = subprocess.run(
		["git", "ls-files", "--", *CAPTURE_INPUTS],
		cwd=repo_root,
		check=True,
		text=True,
		capture_output=True,
	)
	paths = sorted(line for line in result.stdout.splitlines() if line)
	hasher = hashlib.sha256()
	for relative in paths:
		hasher.update(relative.encode("utf-8") + b"\0")
		hasher.update((repo_root / relative).read_bytes())
		hasher.update(b"\0")
	return hasher.hexdigest(), paths


def git_value(*args: str) -> str:
	return subprocess.run(
		["git", *args], cwd=REPO_ROOT, check=True, text=True, capture_output=True
	).stdout.strip()


def git_is_dirty() -> bool:
	return bool(git_value("status", "--porcelain", "--untracked-files=normal"))


def wait_for_startup(process: subprocess.Popen[str], timeout: float = 60.0) -> str:
	lines: queue.Queue[str | None] = queue.Queue()

	def read_stdout() -> None:
		assert process.stdout is not None
		for line in process.stdout:
			lines.put(line)
		lines.put(None)

	threading.Thread(target=read_stdout, daemon=True).start()
	deadline = time.monotonic() + timeout
	recent: list[str] = []
	while time.monotonic() < deadline:
		try:
			line = lines.get(timeout=min(0.25, max(deadline - time.monotonic(), 0.01)))
		except queue.Empty:
			if process.poll() is not None:
				break
			continue
		if line is None:
			break
		print(line, end="")
		recent.append(line.rstrip())
		recent = recent[-20:]
		try:
			event = json.loads(line)
		except json.JSONDecodeError:
			continue
		if event.get("type") == "server_started" and isinstance(event.get("url"), str):
			return str(event["url"])
	raise MarketingMediaError(
		"dcmview did not report startup within the timeout\n" + "\n".join(recent)
	)


def wait_for_catalog(url: str, expected_files: int, timeout: float = 120.0) -> dict[str, Any]:
	deadline = time.monotonic() + timeout
	last_count = 0
	while time.monotonic() < deadline:
		try:
			with urllib.request.urlopen(f"{url.rstrip('/')}/api/files", timeout=5) as response:
				catalog = json.load(response)
		except (OSError, urllib.error.URLError, json.JSONDecodeError):
			time.sleep(0.2)
			continue
		last_count = len(catalog.get("files", []))
		if catalog.get("scan_complete"):
			if last_count != expected_files:
				raise MarketingMediaError(
					f"capture source expected {expected_files} DICOM objects, dcmview found {last_count}"
				)
			return catalog
		time.sleep(0.2)
	raise MarketingMediaError(f"catalog scan timed out after finding {last_count}/{expected_files} files")


@contextlib.contextmanager
def running_server(binary: Path, source_path: Path, expected_files: int) -> Iterable[str]:
	process = subprocess.Popen(
		[
			str(binary), "--no-browser", "--host", "127.0.0.1", "--port", "0",
			"--startup-json", str(source_path),
		],
		cwd=REPO_ROOT,
		stdout=subprocess.PIPE,
		stderr=subprocess.STDOUT,
		text=True,
		bufsize=1,
	)
	try:
		url = wait_for_startup(process)
		wait_for_catalog(url, expected_files)
		yield url
	finally:
		if process.poll() is None:
			process.send_signal(signal.SIGINT)
			try:
				process.wait(timeout=10)
			except subprocess.TimeoutExpired:
				process.kill()
				process.wait(timeout=5)


def build_capture_binary(binary: Path, no_build: bool) -> Path:
	if not no_build:
		print("\n==> Build capture binary", flush=True)
		subprocess.run(["cargo", "build", "--locked"], cwd=REPO_ROOT, check=True)
	if not binary.is_file():
		raise MarketingMediaError(f"capture binary is unavailable: {binary}")
	return binary.resolve()


def source_group_file_count(group: dict[str, Any]) -> int:
	return sum(int(series["expected_files"]) for series in group["series"])


def write_attribution(
	*, destination: Path, scenes: Sequence[dict[str, Any]], sources: dict[str, Any]
) -> None:
	license_info = sources["license"]
	retrieved = sources["retrieved_via"]
	lines = [
		"# Marketing media attribution",
		"",
		"The screenshots and GIFs are documentation derivatives, not clinical material.",
		f"Source DICOM data are licensed under [{license_info['name']}]({license_info['url']}).",
		"No endorsement by the dataset creators, IDC, TCIA, NIH, or NCI is implied.",
		"",
		"## Capture-to-source linkage",
		"",
	]
	for scene in scenes:
		source = scene["source"]
		lines.extend(
			[
				f"### `{scene['output']}` — {source['title']}",
				"",
				f"{source['dataset_title']}. {source['attribution_party']} ({source['year_version']}). "
				f"[{source['doi']}]({source['doi']})",
				"",
				f"Series Instance UID: `{scene['series_instance_uid']}`. "
				f"Modifications: {', '.join(scene['modifications'])}.",
				"",
			]
		)
	lines.extend(
		[
			"## Retrieval platform",
			"",
			f"Retrieved from {retrieved['name']} ({retrieved['data_version']}) using {retrieved['tool']}.",
			f"{retrieved['citation']} [{retrieved['doi']}]({retrieved['doi']})",
			"",
		]
	)
	destination.write_text("\n".join(lines), encoding="utf-8")


def capture_media(args: argparse.Namespace) -> None:
	sources_path = args.sources.resolve()
	captures_path = args.captures.resolve()
	sources = load_json(sources_path)
	groups = {str(group["id"]): group for group in source_groups(sources)}
	scenes = resolve_scenes(
		captures=load_json(captures_path), sources=sources, requested=args.scene
	)
	if args.surface:
		scenes = [scene for scene in scenes if scene["surface"] in set(args.surface)]
	if not scenes:
		raise MarketingMediaError("no capture scenes matched the selection")
	if git_is_dirty() and not args.allow_dirty:
		raise MarketingMediaError("capture requires a clean worktree; commit changes or pass --allow-dirty")

	if not args.skip_source_verification:
		verification_args = argparse.Namespace(
			sources=sources_path,
			source_root=args.source_root,
			group=sorted({str(scene["group"]) for scene in scenes}),
		)
		verify_source_inventory(verification_args)

	binary = build_capture_binary(args.binary.resolve(), args.no_build)
	node = require_executable("node", "Install Node.js 20.19 or newer.")
	capture_script = REPO_ROOT / "marketing" / "capture_browser.mjs"
	vscode_script = REPO_ROOT / "marketing" / "capture_vscode.mjs"
	if any(scene["surface"] == "vscode" for scene in scenes):
		print("\n==> Compile VS Code extension", flush=True)
		subprocess.run(["npm", "--prefix", "vscode", "run", "compile"], cwd=REPO_ROOT, check=True)
	review_root = args.review_root.resolve()
	review_root.mkdir(parents=True, exist_ok=True)
	staging = Path(tempfile.mkdtemp(prefix="capture-", dir=review_root))
	reports: list[dict[str, Any]] = []
	try:
		for scene in scenes:
			group = groups[str(scene["group"])]
			source_path = args.source_root.resolve() / str(group["id"])
			output = staging / str(scene["output"])
			report_path = staging / f"{scene['id']}.capture.json"
			scene_path = staging / f"{scene['id']}.scene.json"
			scene_path.write_text(json.dumps(scene, indent=2) + "\n", encoding="utf-8")
			print(f"\n==> Capture {scene['id']}", flush=True)
			if scene["surface"] == "browser":
				with running_server(binary, source_path, source_group_file_count(group)) as url:
					subprocess.run(
						[
							node, str(capture_script), "--url", url, "--scene", str(scene_path),
							"--output", str(output), "--report", str(report_path),
						],
						cwd=REPO_ROOT,
						check=True,
					)
			else:
				subprocess.run(
					[
						node, str(vscode_script), "--source", str(source_path),
						"--scene", str(scene_path), "--output", str(output),
						"--report", str(report_path), "--binary", str(binary),
						"--repo", str(REPO_ROOT),
					],
					cwd=REPO_ROOT,
					check=True,
				)
			reports.append(load_json(report_path))

		write_attribution(destination=staging / ATTRIBUTION_NAME, scenes=scenes, sources=sources)
		input_digest, input_paths = tracked_input_digest()
		artifacts = []
		for scene in scenes:
			path = staging / str(scene["output"])
			artifacts.append(
				{
					"scene_id": scene["id"], "path": scene["output"],
					"bytes": path.stat().st_size, "sha256": sha256_file(path),
					"width": scene["viewport"]["width"], "height": scene["viewport"]["height"],
					"source_group": scene["group"], "series_instance_uid": scene["series_instance_uid"],
				}
			)
		inventory_path = args.source_root.resolve() / SOURCE_INVENTORY_NAME
		lock = {
			"schema_version": 1,
			"captured_at_utc": dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat(),
			"git_commit": git_value("rev-parse", "HEAD"),
			"git_dirty": git_is_dirty(),
			"dcmview_version": git_value("show", "HEAD:Cargo.toml").split('version = "', 1)[1].split('"', 1)[0],
			"sources_manifest_sha256": manifest_sha256(sources_path),
			"captures_manifest_sha256": manifest_sha256(captures_path),
			"source_inventory_sha256": sha256_file(inventory_path),
			"capture_inputs_sha256": input_digest,
			"capture_input_paths": input_paths,
			"artifacts": artifacts,
			"reports": reports,
		}
		(staging / MEDIA_LOCK_NAME).write_text(
			json.dumps(lock, indent=2, sort_keys=True) + "\n", encoding="utf-8"
		)
		final = review_root / "current"
		if final.exists():
			shutil.rmtree(final)
		staging.replace(final)
		print(f"\ncapture review bundle: {final}")
	except BaseException:
		shutil.rmtree(staging, ignore_errors=True)
		raise


def verify_media_bundle(args: argparse.Namespace, *, quiet: bool = False) -> dict[str, Any]:
	bundle = args.bundle.resolve()
	lock = load_json(bundle / MEDIA_LOCK_NAME)
	if lock.get("schema_version") != 1:
		raise MarketingMediaError("media lock schema_version must be 1")
	if lock.get("git_dirty"):
		raise MarketingMediaError("media bundle was captured from a dirty worktree")
	sources_path = args.sources.resolve()
	captures_path = args.captures.resolve()
	inventory_path = args.source_root.resolve() / SOURCE_INVENTORY_NAME
	expected_hashes = {
		"sources_manifest_sha256": manifest_sha256(sources_path),
		"captures_manifest_sha256": manifest_sha256(captures_path),
	}
	if inventory_path.is_file():
		expected_hashes["source_inventory_sha256"] = sha256_file(inventory_path)
	elif not args.offline:
		raise MarketingMediaError(f"source inventory is unavailable: {inventory_path}")
	for field, expected in expected_hashes.items():
		if lock.get(field) != expected:
			raise MarketingMediaError(f"media bundle is stale: {field} changed")
	input_digest, _ = tracked_input_digest()
	if lock.get("capture_inputs_sha256") != input_digest:
		raise MarketingMediaError("media bundle is stale: viewer/capture inputs changed")
	artifacts = lock.get("artifacts")
	if not isinstance(artifacts, list) or not artifacts:
		raise MarketingMediaError("media lock has no artifacts")
	for artifact in artifacts:
		path = bundle / require_string(artifact.get("path"), "artifact.path")
		if not path.is_file():
			raise MarketingMediaError(f"captured artifact is missing: {path.name}")
		if path.stat().st_size != artifact.get("bytes") or sha256_file(path) != artifact.get("sha256"):
			raise MarketingMediaError(f"captured artifact hash mismatch: {path.name}")
	attribution = (bundle / ATTRIBUTION_NAME).read_text(encoding="utf-8")
	for group in source_groups(load_json(sources_path)):
		if any(artifact.get("source_group") == group["id"] for artifact in artifacts):
			for required in (group["dataset_title"], group["attribution_party"], group["doi"]):
				if required not in attribution:
					raise MarketingMediaError(f"attribution is missing source text: {required}")
	if not quiet:
		print(f"verified {len(artifacts)} current capture artifact(s) in {bundle}")
	return lock


def validate_configuration(args: argparse.Namespace) -> None:
	sources = load_json(args.sources.resolve())
	scenes = resolve_scenes(
		captures=load_json(args.captures.resolve()), sources=sources, requested=[]
	)
	package_lock = load_json(REPO_ROOT / "marketing" / "package-lock.json")
	if not package_lock.get("lockfileVersion"):
		raise MarketingMediaError("marketing capture dependencies are not locked")
	print(f"validated {len(source_groups(sources))} source groups and {len(scenes)} capture scenes")


def smoke_capture(args: argparse.Namespace) -> None:
	fixture = REPO_ROOT / "tests" / "fixtures" / "golden-jpeg-baseline-large-single-frame.dcm"
	binary = build_capture_binary(args.binary.resolve(), args.no_build)
	node = require_executable("node", "Install Node.js 20.19 or newer.")
	destination = args.review_root.resolve() / "smoke"
	destination.mkdir(parents=True, exist_ok=True)
	with running_server(binary, fixture, 1) as url:
		with urllib.request.urlopen(f"{url.rstrip('/')}/api/files", timeout=5) as response:
			catalog = json.load(response)
		file = catalog["files"][0]
		scene = {
			"id": "synthetic-smoke",
			"kind": "screenshot",
			"output": "synthetic-smoke.png",
			"viewport": {"width": 1440, "height": 900, "device_scale_factor": 1},
			"theme": "dark",
			"locale": "en-US",
			"series_instance_uid": file["series_instance_uid"],
			"allowed_patient_ids": [file["patient_id"]] if file["patient_id"] else [],
		}
		scene_path = destination / "synthetic-smoke.scene.json"
		scene_path.write_text(json.dumps(scene, indent=2) + "\n", encoding="utf-8")
		subprocess.run(
			[
				node, str(REPO_ROOT / "marketing" / "capture_browser.mjs"),
				"--url", url, "--scene", str(scene_path),
				"--output", str(destination / scene["output"]),
				"--report", str(destination / "synthetic-smoke.capture.json"),
			],
			cwd=REPO_ROOT,
			check=True,
		)
	print(f"synthetic capture preflight passed: {destination / scene['output']}")
	if args.vscode:
		vscode_scene = next(
			scene
			for scene in capture_scenes(load_json(DEFAULT_CAPTURES))[1]
			if scene.get("surface") == "vscode"
		)
		print("\n==> Compile and preflight VS Code capture", flush=True)
		subprocess.run(["npm", "--prefix", "vscode", "run", "compile"], cwd=REPO_ROOT, check=True)
		workspace = Path(tempfile.mkdtemp(prefix="dcmview-vscode-smoke-"))
		try:
			shutil.copy2(fixture, workspace / "synthetic.dcm")
			scene.update(
				{
					"id": "synthetic-vscode-smoke",
					"surface": "vscode",
					"vscode_version": vscode_scene["vscode_version"],
				}
			)
			scene_path = destination / "synthetic-vscode-smoke.scene.json"
			scene_path.write_text(json.dumps(scene, indent=2) + "\n", encoding="utf-8")
			subprocess.run(
				[
					node, str(REPO_ROOT / "marketing" / "capture_vscode.mjs"),
					"--source", str(workspace), "--scene", str(scene_path),
					"--output", str(destination / "synthetic-vscode-smoke.png"),
					"--report", str(destination / "synthetic-vscode-smoke.capture.json"),
					"--binary", str(binary), "--repo", str(REPO_ROOT),
				],
				cwd=REPO_ROOT,
				check=True,
			)
		finally:
			shutil.rmtree(workspace, ignore_errors=True)
		print(f"VS Code capture preflight passed: {destination / 'synthetic-vscode-smoke.png'}")


def replace_marked_block(path: Path, block: str, *, anchor: str, mdx: bool = False) -> None:
	html_markers = ("<!-- dcmview-marketing:start -->", "<!-- dcmview-marketing:end -->")
	mdx_markers = ("{/* dcmview-marketing:start */}", "{/* dcmview-marketing:end */}")
	start, end = mdx_markers if mdx else html_markers
	text = path.read_text(encoding="utf-8")
	marked = f"{start}\n{block.rstrip()}\n{end}"
	present = [markers for markers in (html_markers, mdx_markers) if markers[0] in text or markers[1] in text]
	if present:
		if len(present) != 1:
			raise MarketingMediaError(f"invalid marketing markers in {path}")
		existing_start, existing_end = present[0]
		if (
			text.count(existing_start) != 1
			or text.count(existing_end) != 1
			or text.index(existing_start) > text.index(existing_end)
		):
			raise MarketingMediaError(f"invalid marketing markers in {path}")
		text = (
			text[: text.index(existing_start)]
			+ marked
			+ text[text.index(existing_end) + len(existing_end) :]
		)
	else:
		position = text.find(anchor)
		if position < 0:
			raise MarketingMediaError(f"publication anchor {anchor!r} not found in {path}")
		text = text[:position] + marked + "\n\n" + text[position:]
	path.write_text(text, encoding="utf-8")


def copy_bundle_files(bundle: Path, destination: Path, lock: dict[str, Any]) -> None:
	destination.mkdir(parents=True, exist_ok=True)
	for artifact in lock["artifacts"]:
		shutil.copy2(bundle / artifact["path"], destination / artifact["path"])
	for name in (ATTRIBUTION_NAME, MEDIA_LOCK_NAME):
		shutil.copy2(bundle / name, destination / name)


def publication_artifact_paths(lock: dict[str, Any]) -> dict[str, str]:
	paths: dict[str, str] = {}
	for artifact in lock.get("artifacts", []):
		scene_id = require_string(artifact.get("scene_id"), "artifact.scene_id")
		path = require_string(artifact.get("path"), "artifact.path")
		if scene_id in paths:
			raise MarketingMediaError(f"duplicate published scene: {scene_id}")
		paths[scene_id] = path
	return paths


def publication_image(paths: dict[str, str], scene_id: str, alt: str, base: str) -> str:
	try:
		path = paths[scene_id]
	except KeyError as error:
		raise MarketingMediaError(f"published gallery scene is unavailable: {scene_id}") from error
	return f"![{alt}]({base.rstrip('/')}/{path})"


def viewer_gallery(paths: dict[str, str], *, asset_base: str, attribution_url: str) -> str:
	images = {
		"ct": publication_image(paths, "chest-ct-cine", "Chest CT cine playback in dcmview", asset_base),
		"seg": publication_image(paths, "mr-seg-cine", "DICOM SEG semantic overlay in dcmview", asset_base),
		"radiograph": publication_image(paths, "radiograph", "Chest radiograph in dcmview", asset_base),
		"mammography": publication_image(paths, "mammography", "Mammography study in dcmview", asset_base),
		"pet": publication_image(paths, "pet-cine", "PET cine playback in dcmview", asset_base),
		"ultrasound": publication_image(paths, "ultrasound-cine", "Ultrasound cine playback in dcmview", asset_base),
		"dose": publication_image(paths, "rt-dose-context", "RT Dose semantic context in dcmview", asset_base),
		"wsi": publication_image(paths, "wsi-context", "DICOM whole-slide microscopy context in dcmview", asset_base),
	}
	return (
		"## Viewer gallery\n\n"
		"### Cine playback and semantic context\n\n"
		f"{images['ct']}\n\n{images['seg']}\n\n"
		"### Modality coverage\n\n"
		f"{images['radiograph']}\n\n{images['mammography']}\n\n{images['pet']}\n\n"
		f"{images['ultrasound']}\n\n{images['dose']}\n\n{images['wsi']}\n\n"
		f"[Source imagery attribution]({attribution_url})"
	)


def vscode_gallery(paths: dict[str, str], *, asset_base: str, attribution_url: str) -> str:
	workflow = publication_image(
		paths, "vscode-workflow", "Open DICOM data with dcmview from VS Code Explorer", asset_base
	)
	ct = publication_image(paths, "chest-ct-cine", "DICOM cine playback in dcmview", asset_base)
	return (
		"## In VS Code\n\n"
		f"{workflow}\n\n{ct}\n\n"
		f"[Source imagery attribution]({attribution_url})"
	)


def publish_media(args: argparse.Namespace) -> None:
	if not args.approve:
		raise MarketingMediaError("publication requires the explicit --approve flag after visual review")
	lock = verify_media_bundle(args, quiet=True)
	expected_scene_ids = {
		str(scene["id"])
		for scene in resolve_scenes(
			captures=load_json(args.captures.resolve()),
			sources=load_json(args.sources.resolve()),
			requested=[],
		)
	}
	actual_scene_ids = {str(artifact.get("scene_id")) for artifact in lock["artifacts"]}
	if actual_scene_ids != expected_scene_ids:
		missing = ", ".join(sorted(expected_scene_ids - actual_scene_ids)) or "none"
		raise MarketingMediaError(f"only a complete reviewed bundle may be published; missing: {missing}")
	version = require_string(lock.get("dcmview_version"), "dcmview_version")
	tag = require_string(args.tag, "tag")
	if tag != f"v{version}" or not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
		raise MarketingMediaError(f"publication tag {tag!r} does not match captured version v{version}")
	bundle = args.bundle.resolve()
	repository = "https://raw.githubusercontent.com/dcmview/dcmview"
	artifact_paths = publication_artifact_paths(lock)
	root_media = REPO_ROOT / "media" / "marketing"
	vscode_media = REPO_ROOT / "vscode" / "media" / "marketing"
	copy_bundle_files(bundle, root_media, lock)
	copy_bundle_files(bundle, vscode_media, lock)
	root_gallery = viewer_gallery(
		artifact_paths,
		asset_base=f"{repository}/{tag}/media/marketing",
		attribution_url=f"{repository}/{tag}/media/marketing/ATTRIBUTION.md",
	)
	replace_marked_block(REPO_ROOT / "README.md", root_gallery, anchor="## Why use it?")
	replace_marked_block(REPO_ROOT / "docs" / "index.md", root_gallery, anchor="## User Guides")
	extension_gallery = vscode_gallery(
		artifact_paths,
		asset_base=f"{repository}/{tag}/vscode/media/marketing",
		attribution_url=f"{repository}/{tag}/vscode/media/marketing/ATTRIBUTION.md",
	)
	replace_marked_block(
		REPO_ROOT / "vscode" / "README.md", extension_gallery, anchor="## Supported Platforms"
	)

	docs_repo = args.docs_repo.resolve()
	docs_index = docs_repo / "src" / "content" / "docs" / "index.mdx"
	if not docs_index.is_file():
		raise MarketingMediaError(f"dcmview-docs index is unavailable: {docs_index}")
	docs_media = docs_repo / "public" / "media" / "dcmview"
	copy_bundle_files(bundle, docs_media, lock)
	docs_gallery = viewer_gallery(
		artifact_paths,
		asset_base="/media/dcmview",
		attribution_url="/reference/media-attribution/",
	)
	replace_marked_block(docs_index, docs_gallery, anchor="## Choose your workflow", mdx=True)
	attribution_page = docs_repo / "src" / "content" / "docs" / "reference" / "media-attribution.md"
	attribution_page.write_text(
		"---\ntitle: Marketing media attribution\ndescription: Source and license attribution for dcmview documentation imagery.\n---\n\n"
		+ (bundle / ATTRIBUTION_NAME).read_text(encoding="utf-8").replace(
			"# Marketing media attribution\n\n", ""
		),
		encoding="utf-8",
	)
	print("published the approved media set to README, Marketplace, and dcmview-docs surfaces")


def add_common_manifest_arguments(parser: argparse.ArgumentParser) -> None:
	parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
	parser.add_argument("--source-root", type=Path, default=DEFAULT_SOURCE_ROOT)
	parser.add_argument("--group", action="append", default=[], help="Source group ID; repeatable")


def add_capture_manifest_arguments(parser: argparse.ArgumentParser) -> None:
	parser.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
	parser.add_argument("--captures", type=Path, default=DEFAULT_CAPTURES)
	parser.add_argument("--source-root", type=Path, default=DEFAULT_SOURCE_ROOT)
	parser.add_argument("--review-root", type=Path, default=DEFAULT_REVIEW_ROOT)
	parser.add_argument("--scene", action="append", default=[], help="Capture scene ID; repeatable")


def build_parser() -> argparse.ArgumentParser:
	parser = argparse.ArgumentParser(description=__doc__)
	subparsers = parser.add_subparsers(dest="command", required=True)

	fetch = subparsers.add_parser("fetch", help="Download and inventory pinned IDC series")
	add_common_manifest_arguments(fetch)
	fetch.set_defaults(handler=fetch_sources)

	inventory = subparsers.add_parser(
		"inventory", help="Checksum already-downloaded IDC series and write linkage records"
	)
	add_common_manifest_arguments(inventory)
	inventory.set_defaults(handler=inventory_sources)

	verify_sources = subparsers.add_parser(
		"verify-sources", help="Verify downloaded source files against their local inventory"
	)
	add_common_manifest_arguments(verify_sources)
	verify_sources.set_defaults(handler=verify_source_inventory)

	capture = subparsers.add_parser(
		"capture", help="Capture deterministic browser media into an ignored review bundle"
	)
	add_capture_manifest_arguments(capture)
	capture.add_argument(
		"--surface", action="append", choices=("browser", "vscode"), default=[]
	)
	capture.add_argument("--binary", type=Path, default=REPO_ROOT / "target" / "debug" / "dcmview")
	capture.add_argument("--no-build", action="store_true")
	capture.add_argument("--skip-source-verification", action="store_true")
	capture.add_argument("--allow-dirty", action="store_true")
	capture.set_defaults(handler=capture_media)

	verify = subparsers.add_parser(
		"verify", help="Reject stale, modified, dirty, or incompletely attributed media"
	)
	add_capture_manifest_arguments(verify)
	verify.add_argument("--bundle", type=Path, default=DEFAULT_REVIEW_ROOT / "current")
	verify.add_argument(
		"--offline", action="store_true",
		help="Verify committed media without requiring the ignored source inventory",
	)
	verify.set_defaults(handler=verify_media_bundle)

	publish = subparsers.add_parser(
		"publish", help="Publish a visually approved bundle to all documentation surfaces"
	)
	add_capture_manifest_arguments(publish)
	publish.add_argument("--bundle", type=Path, default=DEFAULT_REVIEW_ROOT / "current")
	publish.add_argument("--docs-repo", type=Path, default=REPO_ROOT.parent / "dcmview-docs")
	publish.add_argument("--tag", required=True)
	publish.add_argument("--approve", action="store_true")
	publish.set_defaults(offline=False)
	publish.set_defaults(handler=publish_media)

	validate = subparsers.add_parser("validate", help="Validate tracked source and scene manifests")
	validate.add_argument("--sources", type=Path, default=DEFAULT_SOURCES)
	validate.add_argument("--captures", type=Path, default=DEFAULT_CAPTURES)
	validate.set_defaults(handler=validate_configuration)

	smoke = subparsers.add_parser(
		"smoke", help="Preflight the real browser capture path with a committed synthetic fixture"
	)
	smoke.add_argument("--review-root", type=Path, default=DEFAULT_REVIEW_ROOT)
	smoke.add_argument("--binary", type=Path, default=REPO_ROOT / "target" / "debug" / "dcmview")
	smoke.add_argument("--no-build", action="store_true")
	smoke.add_argument("--vscode", action="store_true", help="Also launch the pinned VS Code host")
	smoke.set_defaults(handler=smoke_capture)
	return parser


def main(argv: Sequence[str] | None = None) -> int:
	args = build_parser().parse_args(argv)
	try:
		args.handler(args)
	except (MarketingMediaError, subprocess.CalledProcessError) as error:
		print(f"marketing media error: {error}", file=sys.stderr)
		return 1
	return 0


if __name__ == "__main__":
	raise SystemExit(main())

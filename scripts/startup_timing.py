#!/usr/bin/env python3
"""Startup and discovery timing against a released baseline.

This is the instrument behind the rule that file identity must not slow
startup or discovery (`docs/design/annotation-model.md` 1.7). It starts two
`dcmview` binaries, a baseline and a candidate, on synthetic folders written
at run time, and compares how long each takes to answer its first request, to
list its first file and to finish discovery. It is opt-in: no check profile
that CI runs calls it, because a shared runner's timing is not a gate.

    python scripts/startup_timing.py run             # this checkout against v0.4.0
    python scripts/startup_timing.py run --enforce   # exit 1 past the threshold
    python scripts/startup_timing.py build-ref v0.4.0

The method, the profiles and the threshold are described in
`docs/development.md`, "Startup And Discovery Timing".
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import shutil
import statistics
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import zlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterable

REPO_ROOT = Path(__file__).resolve().parents[1]

#: The released tag every candidate is compared with.
BASELINE_REF = "v0.4.0"

#: A candidate passes a metric when its best run is at most the baseline's
#: best run times `1 + THRESHOLD_PERCENT / 100`, plus `THRESHOLD_FLOOR_MS`.
#: The best (fastest) of the timed runs is compared, not the median: other
#: work on the machine only ever adds time, so the fastest run is the closest
#: to what the code itself costs and repeats far better than the median. The
#: floor absorbs scheduling jitter on runs that take a few milliseconds, where
#: a percentage alone would fail on noise.
THRESHOLD_PERCENT = 5.0
THRESHOLD_FLOOR_MS = 3.0

#: Timed runs per binary and profile, after one discarded warm-up run each.
DEFAULT_RUNS = 9

#: Give up on a run that has not finished discovery after this long.
RUN_TIMEOUT_SECONDS = 300.0

#: The metrics the threshold applies to, with the label the table prints.
GATED_METRICS = (
	("first_response_ms", "first response"),
	("first_file_ms", "first file"),
	("scan_complete_ms", "scan complete"),
)
#: Reported, never gated: one full catalog listing after the scan.
REPORTED_METRICS = (("catalog_ms", "catalog listing"),)


# ---------------------------------------------------------------------------
# Synthetic inputs
# ---------------------------------------------------------------------------

_SHORT_VRS = {"AE", "AS", "AT", "CS", "DA", "DS", "DT", "FL", "FD", "IS", "LO", "LT", "PN", "SH", "SL", "SS", "ST", "TM", "UI", "UL", "US"}


def _element(group: int, element: int, vr: str, value: bytes) -> bytes:
	"""One Explicit VR Little Endian data element, value padded to even length."""
	if len(value) % 2:
		value += b"\0" if vr in ("UI", "OB") else b" "
	if vr in _SHORT_VRS:
		return struct.pack("<HH2sH", group, element, vr.encode(), len(value)) + value
	return struct.pack("<HH2sHI", group, element, vr.encode(), 0, len(value)) + value


def _text(group: int, element: int, vr: str, value: str) -> bytes:
	return _element(group, element, vr, value.encode("ascii"))


def _us(group: int, element: int, value: int) -> bytes:
	return _element(group, element, "US", struct.pack("<H", value))


CT_IMAGE_STORAGE = "1.2.840.10008.5.1.4.1.1.2"
EXPLICIT_VR_LITTLE_ENDIAN = "1.2.840.10008.1.2.1"
#: A UID root under 2.25 that no real object uses.
UID_ROOT = "2.25.990011223344"


def dicom_file(patient: int, study: int, series: int, instance: int) -> bytes:
	"""A small, valid CT image: a header of ordinary size and 32x32 pixels.

	Discovery reads a file up to its pixel data, so the header is what is
	timed; the pixel payload only has to exist.
	"""
	study_uid = f"{UID_ROOT}.{patient}.{study}"
	series_uid = f"{study_uid}.{series}"
	sop_uid = f"{series_uid}.{instance}"
	rows = columns = 32
	meta_body = b"".join(
		[
			_element(0x0002, 0x0001, "OB", b"\x00\x01"),
			_text(0x0002, 0x0002, "UI", CT_IMAGE_STORAGE),
			_text(0x0002, 0x0003, "UI", sop_uid),
			_text(0x0002, 0x0010, "UI", EXPLICIT_VR_LITTLE_ENDIAN),
			_text(0x0002, 0x0012, "UI", f"{UID_ROOT}.0"),
			_text(0x0002, 0x0013, "SH", "TIMING"),
		]
	)
	meta = _element(0x0002, 0x0000, "UL", struct.pack("<I", len(meta_body))) + meta_body
	dataset = b"".join(
		[
			_text(0x0008, 0x0005, "CS", "ISO_IR 100"),
			_text(0x0008, 0x0008, "CS", "ORIGINAL\\PRIMARY\\AXIAL"),
			_text(0x0008, 0x0016, "UI", CT_IMAGE_STORAGE),
			_text(0x0008, 0x0018, "UI", sop_uid),
			_text(0x0008, 0x0020, "DA", "20200102"),
			_text(0x0008, 0x0021, "DA", "20200102"),
			_text(0x0008, 0x0030, "TM", "101500"),
			_text(0x0008, 0x0050, "SH", f"ACC{patient:05d}{study:02d}"),
			_text(0x0008, 0x0060, "CS", "CT"),
			_text(0x0008, 0x0070, "LO", "Synthetic Scanner Works"),
			_text(0x0008, 0x0090, "PN", "Referrer^Synthetic"),
			_text(0x0008, 0x1030, "LO", "Timing study, synthetic"),
			_text(0x0008, 0x103E, "LO", f"Timing series {series}"),
			_text(0x0008, 0x1090, "LO", "Synthetic Model 1"),
			_text(0x0010, 0x0010, "PN", f"Synthetic^Patient{patient:05d}"),
			_text(0x0010, 0x0020, "LO", f"SYN{patient:05d}"),
			_text(0x0010, 0x0030, "DA", "19700101"),
			_text(0x0010, 0x0040, "CS", "O"),
			_text(0x0018, 0x0050, "DS", "1.25"),
			_text(0x0018, 0x0060, "DS", "120"),
			_text(0x0018, 0x1030, "LO", "Synthetic protocol"),
			_text(0x0018, 0x5100, "CS", "HFS"),
			_text(0x0020, 0x000D, "UI", study_uid),
			_text(0x0020, 0x000E, "UI", series_uid),
			_text(0x0020, 0x0010, "SH", str(study)),
			_text(0x0020, 0x0011, "IS", str(series)),
			_text(0x0020, 0x0013, "IS", str(instance)),
			_text(0x0020, 0x0032, "DS", f"-100\\-100\\{instance * 1.25:.2f}"),
			_text(0x0020, 0x0037, "DS", "1\\0\\0\\0\\1\\0"),
			_text(0x0020, 0x0052, "UI", f"{study_uid}.9999"),
			_text(0x0020, 0x1041, "DS", f"{instance * 1.25:.2f}"),
			_us(0x0028, 0x0002, 1),
			_text(0x0028, 0x0004, "CS", "MONOCHROME2"),
			_us(0x0028, 0x0010, rows),
			_us(0x0028, 0x0011, columns),
			_text(0x0028, 0x0030, "DS", "0.7\\0.7"),
			_us(0x0028, 0x0100, 16),
			_us(0x0028, 0x0101, 12),
			_us(0x0028, 0x0102, 11),
			_us(0x0028, 0x0103, 0),
			_text(0x0028, 0x1050, "DS", "40"),
			_text(0x0028, 0x1051, "DS", "400"),
			_text(0x0028, 0x1052, "DS", "-1024"),
			_text(0x0028, 0x1053, "DS", "1"),
			_element(0x7FE0, 0x0010, "OW", struct.pack("<H", instance % 4096) * (rows * columns)),
		]
	)
	return b"\0" * 128 + b"DICM" + meta + dataset


def png_file(seed: int) -> bytes:
	"""A valid 64x64 8-bit grayscale PNG whose bytes differ per `seed`."""

	def chunk(kind: bytes, body: bytes) -> bytes:
		return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

	edge = 64
	rows = b"".join(b"\0" + bytes((seed + row + column) % 256 for column in range(edge)) for row in range(edge))
	return (
		b"\x89PNG\r\n\x1a\n"
		+ chunk(b"IHDR", struct.pack(">IIBBBBB", edge, edge, 8, 0, 0, 0, 0))
		+ chunk(b"IDAT", zlib.compress(rows, 6))
		+ chunk(b"IEND", b"")
	)


def _write_series_tree(root: Path, patients: int, studies: int, series: int, instances: int) -> int:
	"""`patient/study/series/instance.dcm`, the layout of a research dump."""
	count = 0
	for patient in range(1, patients + 1):
		for study in range(1, studies + 1):
			for number in range(1, series + 1):
				directory = root / f"patient_{patient:05d}" / f"study_{study:02d}" / f"series_{number:02d}"
				directory.mkdir(parents=True, exist_ok=True)
				for instance in range(1, instances + 1):
					(directory / f"image_{instance:04d}.dcm").write_bytes(dicom_file(patient, study, number, instance))
					count += 1
	return count


def _small(root: Path) -> int:
	return _write_series_tree(root, patients=1, studies=1, series=1, instances=16)


def _study(root: Path) -> int:
	return _write_series_tree(root, patients=1, studies=1, series=8, instances=250)


def _tree(root: Path) -> int:
	return _write_series_tree(root, patients=100, studies=2, series=2, instances=50)


def _cohort(root: Path) -> int:
	return _write_series_tree(root, patients=500, studies=2, series=2, instances=50)


def _duplicated(root: Path) -> int:
	"""A cohort and a byte-identical copy of it, as `train/` beside `all/`."""
	count = _write_series_tree(root / "all", patients=25, studies=2, series=2, instances=50)
	shutil.copytree(root / "all", root / "train")
	return 2 * count


def _images(root: Path) -> int:
	count = 5000
	for index in range(count):
		directory = root / f"class_{index % 20:02d}"
		directory.mkdir(parents=True, exist_ok=True)
		(directory / f"image_{index:05d}.png").write_bytes(png_file(index))
	return count


@dataclass(frozen=True)
class Profile:
	name: str
	description: str
	write: Callable[[Path], int]


PROFILES = (
	Profile("small", "16 DICOM files in one folder", _small),
	Profile("study", "2,000 DICOM files, one study of 8 series", _study),
	Profile("tree", "20,000 DICOM files, 100 patients in nested folders", _tree),
	Profile("cohort", "100,000 DICOM files, 500 patients in nested folders", _cohort),
	Profile("duplicated", "10,000 DICOM files: 5,000 and a byte-identical copy of the tree", _duplicated),
	Profile("images", "5,000 PNG files in 20 folders", _images),
)


def prepare_inputs(root: Path, profiles: Iterable[Profile]) -> dict[str, tuple[Path, int]]:
	"""Write each profile's folder under `root` unless a complete one is there."""
	inputs = {}
	for profile in profiles:
		directory = root / profile.name
		stamp = directory / ".complete"
		if stamp.is_file():
			inputs[profile.name] = (directory, int(stamp.read_text()))
			continue
		shutil.rmtree(directory, ignore_errors=True)
		directory.mkdir(parents=True)
		print(f"writing profile {profile.name}: {profile.description}", flush=True)
		count = profile.write(directory)
		stamp.write_text(str(count))
		inputs[profile.name] = (directory, count)
	return inputs


# ---------------------------------------------------------------------------
# Building a binary from a git ref
# ---------------------------------------------------------------------------

def timing_root() -> Path:
	"""Where built baselines and synthetic inputs are kept, inside `target/`."""
	return Path(os.environ.get("DCMVIEW_TIMING_DIR", REPO_ROOT / "target" / "timing"))


def _git(*arguments: str) -> str:
	return subprocess.run(["git", *arguments], cwd=REPO_ROOT, check=True, capture_output=True, text=True).stdout.strip()


def _binary_name() -> str:
	return "dcmview.exe" if os.name == "nt" else "dcmview"


def build_ref(ref: str) -> Path:
	"""A release binary of `ref`, built once and kept by commit.

	The sources come from `git archive`, so the working tree and its branch are
	not touched and no worktree is created. The build uses this checkout's
	built frontend (`frontend/dist`) with `DCMVIEW_SKIP_FRONTEND_BUILD=1`: the
	embedded page plays no part in what is timed, and this keeps the build off
	the network. Dependencies compile into one shared `build` directory, so a
	second ref reuses what the first one built.
	"""
	commit = _git("rev-parse", f"{ref}^{{commit}}")
	directory = timing_root() / "refs" / commit[:12]
	binary = directory / _binary_name()
	if binary.is_file():
		return binary
	dist = REPO_ROOT / "frontend" / "dist"
	if not (dist / "index.html").is_file():
		raise SystemExit("frontend/dist is missing; run `npm --prefix frontend run build` first")
	source = directory / "src"
	shutil.rmtree(directory, ignore_errors=True)
	source.mkdir(parents=True)
	print(f"building {ref} ({commit[:12]}) in {source}", flush=True)
	archive = subprocess.run(["git", "archive", "--format=tar", commit], cwd=REPO_ROOT, check=True, capture_output=True)
	with tempfile.TemporaryFile() as buffer:
		buffer.write(archive.stdout)
		buffer.seek(0)
		with tarfile.open(fileobj=buffer) as tar:
			if sys.version_info >= (3, 12):
				tar.extractall(source, filter="data")
			else:
				tar.extractall(source)
	shutil.copytree(dist, source / "frontend" / "dist")
	environment = {
		**os.environ,
		"DCMVIEW_SKIP_FRONTEND_BUILD": "1",
		"CARGO_TARGET_DIR": str(timing_root() / "build"),
	}
	subprocess.run(["cargo", "build", "--release", "--locked", "--bin", "dcmview"], cwd=source, check=True, env=environment)
	shutil.copy2(timing_root() / "build" / "release" / _binary_name(), binary)
	(directory / "ref.txt").write_text(f"{ref} {commit}\n")
	shutil.rmtree(source)
	return binary


def build_candidate() -> Path:
	"""A release binary of this checkout as it is on disk.

	It compiles into the same `build` directory as the baselines, so the
	dependencies are built once for all of them and the checkout's own
	`target/release` is neither needed nor touched.
	"""
	environment = {**os.environ, "CARGO_TARGET_DIR": str(timing_root() / "build")}
	if (REPO_ROOT / "frontend" / "dist" / "index.html").is_file():
		environment["DCMVIEW_SKIP_FRONTEND_BUILD"] = "1"
	subprocess.run(["cargo", "build", "--release", "--locked", "--bin", "dcmview"], cwd=REPO_ROOT, check=True, env=environment)
	binary = timing_root() / "candidate" / _binary_name()
	binary.parent.mkdir(parents=True, exist_ok=True)
	shutil.copy2(timing_root() / "build" / "release" / _binary_name(), binary)
	return binary


# ---------------------------------------------------------------------------
# One timed run
# ---------------------------------------------------------------------------

@dataclass
class Run:
	first_response_ms: float
	first_file_ms: float
	scan_complete_ms: float
	catalog_ms: float
	catalog_bytes: int
	file_count: int


class RunFailed(RuntimeError):
	"""A binary could not serve a folder, for example because it lists no file from it."""


def _get(port: int, path: str) -> tuple[int, bytes]:
	connection = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
	try:
		connection.request("GET", path)
		response = connection.getresponse()
		return response.status, response.read()
	finally:
		connection.close()


def timed_run(binary: Path, folder: Path) -> Run:
	"""Start `binary` on `folder` and time it from just before the spawn.

	- first response: the first `200` from `/api/health`, asked for as soon
	  as the `server_started` line names the port;
	- first file: the first health response that counts a file, polled
	  every millisecond (the health endpoint does not list files, so the
	  polling does not weigh on the scan it is timing);
	- scan complete: the `scan_complete` line of `--startup-json`;
	- catalog listing: one `/api/files` request after the scan.
	"""
	events: dict[str, float] = {}
	details: dict[str, object] = {}
	command = [str(binary), "--no-browser", "--no-token", "--startup-json", "--port", "0", str(folder)]
	started = time.perf_counter()
	process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
	assert process.stdout is not None

	def read_startup_lines() -> None:
		for line in process.stdout:
			now = time.perf_counter()
			try:
				event = json.loads(line)
			except ValueError:
				continue
			if isinstance(event, dict) and event.get("type") in ("server_started", "scan_complete"):
				details[event["type"]] = event
				events[event["type"]] = now

	reader = threading.Thread(target=read_startup_lines, daemon=True)
	reader.start()
	try:
		deadline = started + RUN_TIMEOUT_SECONDS

		def wait_for(event: str) -> None:
			while event not in events:
				if process.poll() is not None or time.perf_counter() > deadline:
					raise RunFailed(f"{binary.name} did not report {event}")
				time.sleep(0.0005)

		wait_for("server_started")
		port = int(details["server_started"]["port"])  # type: ignore[index]
		first_response = first_file = None
		while first_file is None:
			try:
				status, body = _get(port, "/api/health")
			except OSError as error:
				# A binary that finds nothing to load exits, and its listener with it.
				raise RunFailed(f"{binary.name} stopped serving: {error}") from error
			now = time.perf_counter()
			if status != 200:
				raise RunFailed(f"/api/health answered {status}")
			first_response = first_response or now
			if json.loads(body)["file_count"] > 0:
				first_file = now
			elif "scan_complete" in events or time.perf_counter() > deadline:
				raise RunFailed(f"{binary.name} listed no file")
			else:
				time.sleep(0.001)
		wait_for("scan_complete")
		before = time.perf_counter()
		status, body = _get(port, "/api/files")
		catalog = time.perf_counter() - before
		if status != 200:
			raise RunFailed(f"/api/files answered {status}")
		return Run(
			first_response_ms=(first_response - started) * 1000,
			first_file_ms=(first_file - started) * 1000,
			scan_complete_ms=(events["scan_complete"] - started) * 1000,
			catalog_ms=catalog * 1000,
			catalog_bytes=len(body),
			file_count=int(details["scan_complete"]["file_count"]),  # type: ignore[index]
		)
	finally:
		if process.poll() is None:
			process.terminate()
			try:
				process.wait(timeout=20)
			except subprocess.TimeoutExpired:
				process.kill()
				process.wait()
		reader.join(timeout=5)


# ---------------------------------------------------------------------------
# Comparison
# ---------------------------------------------------------------------------

def allowed_ms(baseline_ms: float, percent: float = THRESHOLD_PERCENT, floor_ms: float = THRESHOLD_FLOOR_MS) -> float:
	"""The slowest best run a candidate may have against a baseline's `baseline_ms`."""
	return baseline_ms * (1 + percent / 100) + floor_ms


@dataclass
class MetricResult:
	label: str
	#: The best (fastest) run of each binary, which is what is compared.
	baseline_ms: float
	candidate_ms: float
	#: The medians, printed beside the best runs to show how noisy the runs were.
	baseline_median_ms: float
	candidate_median_ms: float
	gated: bool

	@property
	def change_percent(self) -> float:
		return (self.candidate_ms / self.baseline_ms - 1) * 100 if self.baseline_ms else 0.0

	@property
	def passed(self) -> bool:
		return not self.gated or self.candidate_ms <= allowed_ms(self.baseline_ms)


@dataclass
class ProfileResult:
	profile: str
	files: int
	baseline_files: int
	candidate_files: int
	catalog_bytes: tuple[int, int]
	metrics: list[MetricResult] = field(default_factory=list)
	#: Every timed run, for whoever wants another statistic than the median.
	runs: dict[str, list[Run]] = field(default_factory=dict)

	@property
	def comparable(self) -> bool:
		"""A profile compares like with like only when both binaries list the same files."""
		return self.baseline_files == self.candidate_files


def compare(profile: Profile, files: int, baseline: list[Run], candidate: list[Run]) -> ProfileResult:
	result = ProfileResult(
		profile=profile.name,
		files=files,
		baseline_files=baseline[0].file_count,
		candidate_files=candidate[0].file_count,
		catalog_bytes=(baseline[0].catalog_bytes, candidate[0].catalog_bytes),
		runs={"baseline": baseline, "candidate": candidate},
	)
	for metrics, gated in ((GATED_METRICS, True), (REPORTED_METRICS, False)):
		for name, label in metrics:
			before = [getattr(run, name) for run in baseline]
			after = [getattr(run, name) for run in candidate]
			result.metrics.append(
				MetricResult(
					label=label,
					baseline_ms=min(before),
					candidate_ms=min(after),
					baseline_median_ms=statistics.median(before),
					candidate_median_ms=statistics.median(after),
					gated=gated and result.comparable,
				)
			)
	return result


def measure(baseline: Path, candidate: Path, profiles: Iterable[Profile], runs: int) -> list[ProfileResult]:
	selected = list(profiles)
	inputs = prepare_inputs(timing_root() / "inputs", selected)
	results = []
	for profile in selected:
		folder, files = inputs[profile.name]
		# One discarded run each warms the page cache and the binary's pages.
		try:
			timed_run(baseline, folder)
			timed_run(candidate, folder)
		except RunFailed as error:
			# A baseline older than a file format lists nothing from a folder of it.
			print(f"\n{profile.name}: skipped, not comparable: {error}", flush=True)
			continue
		before: list[Run] = []
		after: list[Run] = []
		# Alternate, so drift in the machine's load falls on both binaries.
		for _ in range(runs):
			before.append(timed_run(baseline, folder))
			after.append(timed_run(candidate, folder))
		results.append(compare(profile, files, before, after))
		print_profile(results[-1])
	return results


def print_profile(result: ProfileResult) -> None:
	print(f"\n{result.profile}: {result.files} files written, baseline lists {result.baseline_files}, candidate {result.candidate_files}", flush=True)
	if not result.comparable:
		print("  not comparable: the two binaries list different files; reported, not gated")
	print(f"  {'metric':<16} {'baseline best (median)':>24} {'candidate best (median)':>24} {'change':>8} {'allowed':>10}  verdict")
	for metric in result.metrics:
		allowed = f"{allowed_ms(metric.baseline_ms):.1f} ms" if metric.gated else "-"
		verdict = "reported" if not metric.gated else "ok" if metric.passed else "REGRESSION"
		before = f"{metric.baseline_ms:.1f} ({metric.baseline_median_ms:.1f}) ms"
		after = f"{metric.candidate_ms:.1f} ({metric.candidate_median_ms:.1f}) ms"
		print(f"  {metric.label:<16} {before:>24} {after:>24} {metric.change_percent:>+7.1f}% {allowed:>10}  {verdict}")
	print(f"  catalog listing bytes: baseline {result.catalog_bytes[0]}, candidate {result.catalog_bytes[1]}")


def to_json(results: list[ProfileResult], baseline: Path, candidate: Path, runs: int) -> dict[str, object]:
	return {
		"baseline": str(baseline),
		"candidate": str(candidate),
		"runs": runs,
		"threshold": {"percent": THRESHOLD_PERCENT, "floor_ms": THRESHOLD_FLOOR_MS},
		"profiles": [
			{
				"profile": result.profile,
				"files": result.files,
				"baseline_files": result.baseline_files,
				"candidate_files": result.candidate_files,
				"comparable": result.comparable,
				"catalog_bytes": {"baseline": result.catalog_bytes[0], "candidate": result.catalog_bytes[1]},
				"metrics": [
					{
						"metric": metric.label,
						"baseline_ms": round(metric.baseline_ms, 3),
						"candidate_ms": round(metric.candidate_ms, 3),
						"baseline_median_ms": round(metric.baseline_median_ms, 3),
						"candidate_median_ms": round(metric.candidate_median_ms, 3),
						"change_percent": round(metric.change_percent, 2),
						"gated": metric.gated,
						"passed": metric.passed,
					}
					for metric in result.metrics
				],
				"runs": {
					name: [
						{metric: round(getattr(run, metric), 3) for metric, _ in GATED_METRICS + REPORTED_METRICS}
						for run in runs
					]
					for name, runs in result.runs.items()
				},
			}
			for result in results
		],
	}


# ---------------------------------------------------------------------------
# Command line
# ---------------------------------------------------------------------------

def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
	parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
	commands = parser.add_subparsers(dest="command", required=True)
	build = commands.add_parser("build-ref", help="build and keep a release binary of a git ref")
	build.add_argument("ref")
	run = commands.add_parser("run", help="time a candidate against a baseline")
	run.add_argument("--baseline", type=Path, help="baseline binary (default: build --baseline-ref)")
	run.add_argument("--baseline-ref", default=BASELINE_REF, help=f"git ref of the baseline (default: {BASELINE_REF})")
	run.add_argument("--candidate", type=Path, help="candidate binary (default: a release build of this checkout)")
	run.add_argument("--candidate-ref", help="build the candidate from this git ref instead of the working tree")
	run.add_argument("--profile", action="append", choices=[profile.name for profile in PROFILES], help="profile to run; repeatable (default: all)")
	run.add_argument("--runs", type=int, default=DEFAULT_RUNS, help=f"timed runs per binary and profile (default: {DEFAULT_RUNS})")
	run.add_argument("--json", type=Path, help="also write the results as JSON to this file")
	run.add_argument("--enforce", action="store_true", help="exit 1 when a gated metric is past the threshold")
	return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
	args = parse_args(argv)
	if args.command == "build-ref":
		print(build_ref(args.ref))
		return 0
	if args.runs < 1:
		raise SystemExit("--runs must be at least 1")
	baseline = args.baseline or build_ref(args.baseline_ref)
	if args.candidate:
		candidate = args.candidate
	elif args.candidate_ref:
		candidate = build_ref(args.candidate_ref)
	else:
		candidate = build_candidate()
	for binary in (baseline, candidate):
		if not binary.is_file():
			raise SystemExit(f"no binary at {binary}")
	profiles = [profile for profile in PROFILES if not args.profile or profile.name in args.profile]
	print(f"baseline:  {baseline}\ncandidate: {candidate}")
	print(f"threshold: baseline best x {1 + THRESHOLD_PERCENT / 100:.2f} + {THRESHOLD_FLOOR_MS:.0f} ms; best of {args.runs} runs each")
	print("a gated metric that passes with a change above 2% is still worth a second run and a line in the report")
	results = measure(baseline, candidate, profiles, args.runs)
	if args.json:
		args.json.write_text(json.dumps(to_json(results, baseline, candidate, args.runs), indent="\t") + "\n")
	failed = [f"{result.profile}: {metric.label}" for result in results for metric in result.metrics if not metric.passed]
	if failed:
		print("\npast the threshold: " + "; ".join(failed))
		return 1 if args.enforce else 0
	print("\nno gated metric is past the threshold")
	return 0


if __name__ == "__main__":
	sys.exit(main())

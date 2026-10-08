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

Exit status of `run`: 0 when no gated metric is past the threshold (or, without
`--enforce`, whatever the metrics say); 1 when one is and `--enforce` is given,
and whenever the candidate could not serve a profile; 2 when the comparison
could not be made, because the baseline could not serve a profile or no local
port could be assigned.

Beside the gated comparison with the release it reports, without gating, the
same metrics against the commit this branch left the main branch at, so that
what a change costs is not hidden in (or blamed for) what other merged work
cost since the release.

The method, the profiles and the threshold are described in
`docs/development.md`, "Startup And Discovery Timing".
"""

from __future__ import annotations

import argparse
import errno
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

#: The main branch, in the order tried, for finding where this branch left it.
MAIN_BRANCHES = ("origin/main", "main")

#: Exit statuses of `run`. A candidate that is slower, or that cannot serve,
#: fails the gate; a comparison that could not be made is another matter and
#: says nothing about the candidate.
EXIT_GATE_FAILED = 1
EXIT_NOT_COMPARED = 2

#: How many of a failed process's last standard error lines are printed.
STDERR_TAIL_LINES = 12

#: Seconds between two polls for the first file. Each poll is one request on
#: a connection that is kept open, so the resolution of "first file" is this
#: plus one round trip: about a third of a millisecond on a value of ten.
FIRST_FILE_POLL_SECONDS = 0.0002


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


def dicom_file(patient: int, study: int, series: int, instance: int, *, with_uid: bool = True, rows: int = 32) -> bytes:
	"""A small, valid CT image: a header of ordinary size and 32x32 pixels.

	Discovery reads a file up to its pixel data, so the header is what is
	timed; the pixel payload only has to exist. `with_uid=False` leaves the
	SOP Instance UID out of the data set; another `rows` gives the same
	instance another length.
	"""
	study_uid = f"{UID_ROOT}.{patient}.{study}"
	series_uid = f"{study_uid}.{series}"
	sop_uid = f"{series_uid}.{instance}"
	columns = 32
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
			_text(0x0008, 0x0018, "UI", sop_uid) if with_uid else b"",
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


def _write_series_tree(root: Path, patients: int, studies: int, series: int, instances: int, **file_options: object) -> int:
	"""`patient/study/series/instance.dcm`, the layout of a research dump."""
	count = 0
	for patient in range(1, patients + 1):
		for study in range(1, studies + 1):
			for number in range(1, series + 1):
				directory = root / f"patient_{patient:05d}" / f"study_{study:02d}" / f"series_{number:02d}"
				directory.mkdir(parents=True, exist_ok=True)
				for instance in range(1, instances + 1):
					(directory / f"image_{instance:04d}.dcm").write_bytes(dicom_file(patient, study, number, instance, **file_options))  # type: ignore[arg-type]
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


def _collision(root: Path) -> int:
	"""Every instance twice under one UID with different lengths, as a re-export beside the original."""
	count = _write_series_tree(root / "original", patients=25, studies=2, series=2, instances=50)
	return count + _write_series_tree(root / "reexport", patients=25, studies=2, series=2, instances=50, rows=33)


def _no_uid(root: Path) -> int:
	return _write_series_tree(root, patients=50, studies=2, series=2, instances=50, with_uid=False)


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
	#: Extra command-line arguments both binaries are started with.
	arguments: tuple[str, ...] = ()
	#: The profile whose folder this one reads, when it writes none of its own.
	folder_of: str | None = None


PROFILES = (
	Profile("small", "16 DICOM files in one folder", _small),
	Profile("study", "2,000 DICOM files, one study of 8 series", _study),
	Profile("tree", "20,000 DICOM files, 100 patients in nested folders", _tree),
	Profile("cohort", "100,000 DICOM files, 500 patients in nested folders", _cohort),
	Profile("duplicated", "10,000 DICOM files: 5,000 and a byte-identical copy of the tree", _duplicated),
	Profile("collision", "10,000 DICOM files: 5,000 and a copy of each with the same UID and another length", _collision),
	Profile("no-uid", "10,000 DICOM files without a SOP Instance UID", _no_uid),
	Profile("masked", "the 20,000 files of `tree`, served with --mask", _tree, arguments=("--mask",), folder_of="tree"),
	Profile("images", "5,000 PNG files in 20 folders", _images),
)


def prepare_inputs(root: Path, profiles: Iterable[Profile]) -> dict[str, tuple[Path, int]]:
	"""Write each profile's folder under `root` unless a complete one is there."""
	inputs = {}
	for profile in profiles:
		directory = root / (profile.folder_of or profile.name)
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


def toolchain() -> str:
	"""The compiler a build made now would use, as `<version>-<commit>`.

	A binary is only comparable with one built by the same compiler, so a
	kept baseline is kept under this as well as under its commit.
	"""
	words = subprocess.run(["rustc", "--version"], cwd=REPO_ROOT, check=True, capture_output=True, text=True).stdout.split()
	# `rustc 1.92.0 (ded5c06cf 2025-12-08)`
	version = words[1] if len(words) > 1 else "unknown"
	commit = words[2].lstrip("(") if len(words) > 2 else "unknown"
	return f"{version}-{commit}"


def merge_base() -> str | None:
	"""The commit this checkout left the main branch at, or `None`.

	`None` when no main branch is known here, or when the checkout is on it
	(there is then nothing between the two to report).
	"""
	head = _git("rev-parse", "HEAD")
	for branch in MAIN_BRANCHES:
		found = subprocess.run(["git", "merge-base", "HEAD", branch], cwd=REPO_ROOT, capture_output=True, text=True)
		if found.returncode == 0 and found.stdout.strip():
			base = found.stdout.strip()
			return None if base == head else base
	return None


def build_ref(ref: str) -> Path:
	"""A release binary of `ref`, built once and kept by commit and compiler.

	The sources come from `git archive`, so the working tree and its branch are
	not touched and no worktree is created. The build uses this checkout's
	built frontend (`frontend/dist`) with `DCMVIEW_SKIP_FRONTEND_BUILD=1`: the
	embedded page plays no part in what is timed, and this keeps the build off
	the network. Dependencies compile into one shared `build` directory, so a
	second ref reuses what the first one built.
	"""
	commit = _git("rev-parse", f"{ref}^{{commit}}")
	directory = timing_root() / "refs" / f"{commit[:12]}-rustc-{toolchain()}"
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
	(directory / "ref.txt").write_text(f"{ref} {commit} rustc {toolchain()}\n")
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
	#: Resident memory once the scan is complete, before the listing, in
	#: kibibytes; `None` where `ps` cannot say.
	resident_kib: int | None = None


class RunFailed(RuntimeError):
	"""A binary could not serve a folder.

	`listed_nothing` tells the one failure that is expected of a baseline: it
	answered, counted no file of the folder and finished its scan or exited
	(a release older than a file format). `no_port` is set when the cause is
	this machine, which had no local port to give. `status` is the process's
	exit status, `None` when it was still running, and `stderr_tail` the end
	of what it wrote to standard error; `timed_run` fills both in.
	"""

	def __init__(self, message: str, *, listed_nothing: bool = False, no_port: bool = False) -> None:
		super().__init__(message)
		self.listed_nothing = listed_nothing
		self.no_port = no_port
		self.status: int | None = None
		self.stderr_tail: list[str] = []
		#: Which of the timed binaries failed; `measure` fills it in.
		self.binary = ""

	def describe(self) -> str:
		"""The failure with the process's exit status and the end of its standard error."""
		status = "it was still running" if self.status is None else f"it exited with status {self.status}"
		lines = [f"{self}; {status}"]
		if self.stderr_tail:
			lines.append("  the end of its standard error:")
			lines.extend(f"    {line}" for line in self.stderr_tail)
		else:
			lines.append("  it wrote nothing to standard error")
		return "\n".join(lines)


def _no_port(error: BaseException) -> bool:
	"""Whether `error` is the system refusing a local address (`EADDRNOTAVAIL`)."""
	return isinstance(error, OSError) and error.errno == errno.EADDRNOTAVAIL


def _get(port: int, path: str) -> tuple[int, bytes]:
	connection = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
	try:
		connection.request("GET", path)
		response = connection.getresponse()
		return response.status, response.read()
	finally:
		connection.close()


def resident_kib(pid: int) -> int | None:
	"""The resident set size of a running process, where `ps` reports it."""
	if os.name == "nt":
		return None
	try:
		text = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True, timeout=10).stdout.strip()
		return int(text) if text else None
	except (OSError, ValueError, subprocess.TimeoutExpired):
		return None


def timed_run(binary: Path, folder: Path, arguments: tuple[str, ...] = ()) -> Run:
	"""Start `binary` on `folder` and time it from just before the spawn.

	- first response: the first `200` from `/api/health`, asked for as soon
	  as the `server_started` line names the port;
	- first file: the first health response that counts a file, polled
	  every `FIRST_FILE_POLL_SECONDS` on one connection that stays open (the
	  health endpoint does not list files, and the polling ends with the
	  first file, so it does not weigh on the scan it is timing);
	- scan complete: the `scan_complete` line of `--startup-json`;
	- catalog listing: one `/api/files` request after the scan;
	- resident memory: the process's resident set once the scan is complete,
	  before that listing.
	"""
	events: dict[str, float] = {}
	details: dict[str, object] = {}
	command = [str(binary), "--no-browser", "--no-token", "--startup-json", "--port", "0", *arguments, str(folder)]
	started = time.perf_counter()
	# Standard error goes to a file, not a pipe: nobody reads it while the
	# run is timed, and a full pipe would stop the process.
	stderr = tempfile.TemporaryFile()
	process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=stderr, text=True)
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
		answered = False
		health = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
		try:
			while first_file is None:
				try:
					health.request("GET", "/api/health")
					response = health.getresponse()
					status, body = response.status, response.read()
				except (OSError, http.client.HTTPException) as error:
					# A binary that finds nothing to load exits, and its listener
					# with it: that is the case when it had answered and counted
					# no file. One that never answered did not serve at all.
					raise RunFailed(f"{binary.name} stopped serving: {error}", listed_nothing=answered, no_port=_no_port(error)) from error
				now = time.perf_counter()
				if status != 200:
					raise RunFailed(f"/api/health answered {status}")
				answered = True
				first_response = first_response or now
				if json.loads(body)["file_count"] > 0:
					first_file = now
				elif "scan_complete" in events:
					raise RunFailed(f"{binary.name} listed no file", listed_nothing=True)
				elif time.perf_counter() > deadline:
					raise RunFailed(f"{binary.name} listed no file in {RUN_TIMEOUT_SECONDS:.0f} s")
				else:
					time.sleep(FIRST_FILE_POLL_SECONDS)
		finally:
			health.close()
		wait_for("scan_complete")
		# Before the listing, whose response is a transient allocation of its own.
		resident = resident_kib(process.pid)
		before = time.perf_counter()
		try:
			status, body = _get(port, "/api/files")
		except (OSError, http.client.HTTPException) as error:
			raise RunFailed(f"{binary.name} stopped serving before it listed its files: {error}", no_port=_no_port(error)) from error
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
			resident_kib=resident,
		)
	except RunFailed as error:
		# A process that is on its way out gets a moment to finish, so that
		# its own exit status is reported and not the one of being stopped.
		try:
			error.status = process.wait(timeout=2)
		except subprocess.TimeoutExpired:
			error.status = None
		stderr.seek(0)
		text = stderr.read().decode("utf-8", errors="replace")
		error.stderr_tail = text.splitlines()[-STDERR_TAIL_LINES:]
		error.no_port = error.no_port or os.strerror(errno.EADDRNOTAVAIL) in text
		raise
	finally:
		if process.poll() is None:
			process.terminate()
			try:
				process.wait(timeout=20)
			except subprocess.TimeoutExpired:
				process.kill()
				process.wait()
		reader.join(timeout=5)
		process.stdout.close()
		stderr.close()


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
	#: The same metrics against the merge base, none of them gated; empty
	#: when no merge base was timed or it lists other files.
	base_metrics: list[MetricResult] = field(default_factory=list)

	@property
	def comparable(self) -> bool:
		"""A profile compares like with like only when both binaries list the same files."""
		return self.baseline_files == self.candidate_files


def _metrics(before_runs: list[Run], after_runs: list[Run], gate: bool) -> list[MetricResult]:
	results = []
	for metrics, gated in ((GATED_METRICS, True), (REPORTED_METRICS, False)):
		for name, label in metrics:
			before = [getattr(run, name) for run in before_runs]
			after = [getattr(run, name) for run in after_runs]
			results.append(
				MetricResult(
					label=label,
					baseline_ms=min(before),
					candidate_ms=min(after),
					baseline_median_ms=statistics.median(before),
					candidate_median_ms=statistics.median(after),
					gated=gated and gate,
				)
			)
	return results


def compare(profile: Profile, files: int, baseline: list[Run], candidate: list[Run], base: list[Run] | None = None) -> ProfileResult:
	result = ProfileResult(
		profile=profile.name,
		files=files,
		baseline_files=baseline[0].file_count,
		candidate_files=candidate[0].file_count,
		catalog_bytes=(baseline[0].catalog_bytes, candidate[0].catalog_bytes),
		runs={"baseline": baseline, "candidate": candidate},
	)
	result.metrics = _metrics(baseline, candidate, gate=result.comparable)
	if base and base[0].file_count == candidate[0].file_count:
		result.runs["base"] = base
		result.base_metrics = _metrics(base, candidate, gate=False)
	return result


def resident_mib(runs: list[Run]) -> float | None:
	"""The median resident memory of a set of runs, in mebibytes."""
	values = [run.resident_kib for run in runs if run.resident_kib]
	return statistics.median(values) / 1024 if values else None


@dataclass
class Failure:
	"""A profile that gave no comparison, and whose doing that was."""

	profile: str
	#: "candidate": it could not serve, which fails the gate. "baseline": the
	#: comparison could not be made, which says nothing about the candidate.
	binary: str
	reason: str


class NoLocalPort(RuntimeError):
	"""This machine had no local port to give, so nothing can be timed now."""


def _run_of(name: str, binary: Path, folder: Path, arguments: tuple[str, ...]) -> Run:
	"""`timed_run`, with a failure marked with which binary it was."""
	try:
		return timed_run(binary, folder, arguments)
	except RunFailed as error:
		error.binary = name
		if error.no_port:
			raise NoLocalPort(f"no local port could be assigned ({os.strerror(errno.EADDRNOTAVAIL)}) while timing the {name}:\n{error.describe()}") from error
		raise


def measure(baseline: Path, candidate: Path, profiles: Iterable[Profile], runs: int, base: Path | None = None) -> tuple[list[ProfileResult], list[Failure]]:
	"""Time `candidate` against `baseline` (gated) and, when given, `base` (reported).

	Returns the profiles that were compared and those that could not be. A
	profile is skipped, and is in neither list, in one case only: the
	baseline lists none of its files while the candidate serves it. Any
	other failure of a binary to serve a profile is a `Failure`. A process
	that dies during the timed runs is reported with its exit status and
	standard error, and the profile's timed runs are made once more before
	it counts. Raises `NoLocalPort` when the machine cannot give a port.
	"""
	selected = list(profiles)
	inputs = prepare_inputs(timing_root() / "inputs", selected)
	results: list[ProfileResult] = []
	failures: list[Failure] = []
	for profile in selected:
		folder, files = inputs[profile.name]
		# One discarded run each warms the page cache and the binary's pages.
		# The candidate first: it has to serve every profile, whatever the
		# baseline does with it.
		try:
			_run_of("candidate", candidate, folder, profile.arguments)
		except RunFailed as error:
			print(f"\n{profile.name}: FAILED, the candidate cannot serve it: {error.describe()}", flush=True)
			failures.append(Failure(profile.name, "candidate", str(error)))
			continue
		try:
			_run_of("baseline", baseline, folder, profile.arguments)
		except RunFailed as error:
			if error.listed_nothing:
				# A baseline older than a file format lists nothing from a folder of it.
				print(f"\n{profile.name}: skipped, not comparable: the baseline lists none of its files ({error})", flush=True)
			else:
				print(f"\n{profile.name}: NOT COMPARED, the baseline cannot serve it: {error.describe()}", flush=True)
				failures.append(Failure(profile.name, "baseline", str(error)))
			continue
		timed_base = base
		if timed_base:
			try:
				_run_of("merge base", timed_base, folder, profile.arguments)
			except RunFailed:
				timed_base = None
		failed_before = False
		while True:
			before: list[Run] = []
			between: list[Run] = []
			after: list[Run] = []
			try:
				# Alternate, so drift in the machine's load falls on every binary.
				for _ in range(runs):
					before.append(_run_of("baseline", baseline, folder, profile.arguments))
					if timed_base:
						between.append(_run_of("merge base", timed_base, folder, profile.arguments))
					after.append(_run_of("candidate", candidate, folder, profile.arguments))
			except RunFailed as error:
				if error.binary == "merge base":
					# Only reported: the gate does not need it.
					print(f"\n{profile.name}: the merge base died during the timed runs and is left out: {error.describe()}", flush=True)
					timed_base = None
				elif not failed_before:
					print(f"\n{profile.name}: the {error.binary} died during the timed runs; timing the profile once more: {error.describe()}", flush=True)
					failed_before = True
				else:
					word = "FAILED" if error.binary == "candidate" else "NOT COMPARED"
					print(f"\n{profile.name}: {word}, the {error.binary} died during the timed runs again: {error.describe()}", flush=True)
					failures.append(Failure(profile.name, error.binary, str(error)))
					break
				continue
			results.append(compare(profile, files, before, after, between or None))
			print_profile(results[-1])
			break
	return results, failures


def verdict(results: list[ProfileResult], failures: list[Failure], enforce: bool) -> int:
	"""Print what the run found and return the exit status that says it."""
	regressions = [f"{result.profile}: {metric.label}" for result in results for metric in result.metrics if not metric.passed]
	broken = [failure for failure in failures if failure.binary == "candidate"]
	uncompared = [failure for failure in failures if failure.binary != "candidate"]
	if regressions:
		print("\npast the threshold: " + "; ".join(regressions))
	if broken:
		print("\nthe candidate could not serve: " + "; ".join(f"{failure.profile} ({failure.reason})" for failure in broken))
	if uncompared:
		print("\nnot compared, the baseline could not serve: " + "; ".join(f"{failure.profile} ({failure.reason})" for failure in uncompared))
	if broken:
		return EXIT_GATE_FAILED
	if uncompared:
		return EXIT_NOT_COMPARED
	if regressions:
		return EXIT_GATE_FAILED if enforce else 0
	print("\nno gated metric is past the threshold")
	return 0


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
	memory = {name: resident_mib(runs) for name, runs in result.runs.items()}
	if memory.get("baseline") and memory.get("candidate"):
		line = f"  resident memory after the scan (median, reported): baseline {memory['baseline']:.1f} MiB, candidate {memory['candidate']:.1f} MiB ({(memory['candidate'] / memory['baseline'] - 1) * 100:+.1f}%)"
		if memory.get("base"):
			line += f"; merge base {memory['base']:.1f} MiB ({(memory['candidate'] / memory['base'] - 1) * 100:+.1f}%)"
		print(line)
	if result.base_metrics:
		print("  against the merge base, reported and never gated:")
		print(f"  {'metric':<16} {'merge base best (median)':>24} {'candidate best (median)':>24} {'change':>8}")
		for metric in result.base_metrics:
			before = f"{metric.baseline_ms:.1f} ({metric.baseline_median_ms:.1f}) ms"
			after = f"{metric.candidate_ms:.1f} ({metric.candidate_median_ms:.1f}) ms"
			print(f"  {metric.label:<16} {before:>24} {after:>24} {metric.change_percent:>+7.1f}%")


def to_json(results: list[ProfileResult], baseline: Path, candidate: Path, runs: int, base: Path | None = None) -> dict[str, object]:
	return {
		"baseline": str(baseline),
		"base": str(base) if base else None,
		"candidate": str(candidate),
		"toolchain": toolchain(),
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
				"base_metrics": [
					{"metric": metric.label, "base_ms": round(metric.baseline_ms, 3), "candidate_ms": round(metric.candidate_ms, 3), "change_percent": round(metric.change_percent, 2)}
					for metric in result.base_metrics
				],
				"resident_mib": {name: resident_mib(runs) for name, runs in result.runs.items()},
				"runs": {
					name: [
						{**{metric: round(getattr(run, metric), 3) for metric, _ in GATED_METRICS + REPORTED_METRICS}, "resident_kib": run.resident_kib}
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
	run.add_argument("--base", type=Path, help="binary to report against, never gated (default: build --base-ref)")
	run.add_argument("--base-ref", help="git ref to report against (default: where this checkout left the main branch)")
	run.add_argument("--no-base", action="store_true", help="skip the reported comparison with the merge base")
	run.add_argument("--profile", action="append", choices=[profile.name for profile in PROFILES], help="profile to run; repeatable (default: all)")
	run.add_argument("--runs", type=int, default=DEFAULT_RUNS, help=f"timed runs per binary and profile (default: {DEFAULT_RUNS})")
	run.add_argument("--json", type=Path, help="also write the results as JSON to this file")
	run.add_argument("--enforce", action="store_true", help="exit 1 when a gated metric is past the threshold (a candidate that cannot serve exits 1 either way)")
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
	base = None
	if args.base:
		base = args.base
	elif not args.no_base:
		base_ref = args.base_ref or merge_base()
		if base_ref:
			base = build_ref(base_ref)
	for binary in (baseline, candidate, base):
		if binary and not binary.is_file():
			raise SystemExit(f"no binary at {binary}")
	profiles = [profile for profile in PROFILES if not args.profile or profile.name in args.profile]
	print(f"baseline:  {baseline}\ncandidate: {candidate}")
	print(f"merge base: {base if base else 'none (reported comparison skipped)'}")
	print(f"compiler:  rustc {toolchain()}")
	print(f"threshold: baseline best x {1 + THRESHOLD_PERCENT / 100:.2f} + {THRESHOLD_FLOOR_MS:.0f} ms; best of {args.runs} runs each")
	print("a gated metric that passes with a change above 2% is still worth a second run and a line in the report")
	try:
		results, failures = measure(baseline, candidate, profiles, args.runs, base)
	except NoLocalPort as error:
		print(f"\nnothing was compared: {error}\nother work on this machine holds the local ports; run again when it has finished")
		return EXIT_NOT_COMPARED
	if args.json:
		args.json.write_text(json.dumps(to_json(results, baseline, candidate, args.runs, base), indent="\t") + "\n")
	return verdict(results, failures, args.enforce)


if __name__ == "__main__":
	sys.exit(main())

#!/usr/bin/env python3
"""Check dcmview against a stored synth-dicom-gen smoke corpus.

The corpus container is a local producer container: ``smoke.tar.gz`` plus
``artifact-index.json``. The archive must match the index digest, and every
payload must match the SHA-256 and size its manifest entry declares, before the
viewer starts. The runner then launches the real binary on every payload and
compares what the HTTP API serves with the manifest, which the generator wrote
independently of dcmview:

- metadata: identity fields, image geometry, and declared tag values;
- raw frames: SHA-256 of every decoded frame against the manifest frame hashes
  (lossless syntaxes) or error bounds against the recipe samples (JPEG
  Baseline);
- display frames: the exact 8-bit output computed here from the recipe samples,
  rescale, window, padding, and photometric interpretation (see
  ``expected_display``), plus overlay, shutter, and ICC checks where declared;
- feature context where declared: references, series ordering, segmentation,
  parametric map, RT dose, WSI tile placement, and modality-specific tags;
- unsupported transfer syntaxes answer 422 with the stable JSON error, and the
  server keeps serving after an error.

Results go to ``<output>/report.json``; the process exits 1 if any check fails.
Uses only the standard library so it runs on a bare CI Python.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import signal
import struct
import subprocess
import sys
import tarfile
import threading
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request
import zlib
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any, BinaryIO, Optional

MANIFEST_SCHEMA_VERSION = "2.0.0"
ARCHIVE_NAME = "smoke.tar.gz"
INDEX_NAME = "artifact-index.json"

# The viewer answers these with 422 unsupported_transfer_syntax
# (classify_transfer_syntax in src/pixels/syntax.rs).
UNSUPPORTED_TRANSFER_SYNTAXES = {
    "1.2.840.10008.1.2.4.51",
    "1.2.840.10008.1.2.4.81",
    "1.2.840.10008.1.2.4.91",
    "1.2.840.10008.1.2.4.111",
    "1.2.840.10008.1.2.4.112",
    "1.2.840.10008.1.2.4.201",
    "1.2.840.10008.1.2.4.203",
}
JPEG_BASELINE = "1.2.840.10008.1.2.4.50"
SEMANTIC_SOP_CLASSES = {
    "1.2.840.10008.5.1.4.1.1.66.4",  # Segmentation
    "1.2.840.10008.5.1.4.1.1.66.7",  # Label Map Segmentation
    "1.2.840.10008.5.1.4.1.1.30",  # Parametric Map
    "1.2.840.10008.5.1.4.1.1.481.2",  # RT Dose
}
SERIES_CAPABILITIES = {
    "sort_series_by_geometry",
    "interpret_gantry_tilt",
    "organize_series_by_study_and_frame_of_reference",
    "parse_multiframe_functional_groups",
}
# Recipe parameters whose display effect needs data the manifest does not carry
# (LUT and palette tables) or has its own dedicated check (overlay, shutter).
DISPLAY_ORACLE_EXCLUSIONS = ("modality_lut", "voi_lut", "palette", "overlay", "display_shutter")


class CampaignError(RuntimeError):
    """The corpus or viewer could not be prepared, so no case was judged."""


def check(passed: Optional[bool], **details: Any) -> dict[str, Any]:
    """A named check result: True/False, or None when it could not be evaluated."""
    return {"passed": passed, **details}


def not_computable(reason: str) -> dict[str, Any]:
    return check(None, reason=reason)


# ---------------------------------------------------------------------------
# Corpus preparation
# ---------------------------------------------------------------------------


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def safe_members(archive: tarfile.TarFile) -> list[tarfile.TarInfo]:
    """Accept only regular files and directories that stay inside the destination."""
    members = []
    for member in archive.getmembers():
        path = PurePosixPath(member.name)
        if path.is_absolute() or ".." in path.parts:
            raise CampaignError(f"archive member escapes the corpus root: {member.name}")
        if not (member.isfile() or member.isdir()):
            raise CampaignError(f"archive member is not a regular file or directory: {member.name}")
        members.append(member)
    return members


def extract_archive(archive_path: Path, destination: Path) -> None:
    with tarfile.open(archive_path, "r:gz") as archive:
        members = safe_members(archive)
        if hasattr(tarfile, "data_filter"):
            archive.extractall(destination, members=members, filter="data")
        else:  # Python < 3.12 without the backported filter
            archive.extractall(destination, members=members)


def load_corpus(container: Path, workdir: Path) -> tuple[Path, list[dict[str, Any]], str]:
    """Verify and extract the container; return the corpus root, its entries, and the archive digest."""
    archive = container / ARCHIVE_NAME
    index_path = container / INDEX_NAME
    if not archive.is_file() or not index_path.is_file():
        raise CampaignError(f"{container} must contain {ARCHIVE_NAME} and {INDEX_NAME}")
    index = json.loads(index_path.read_text(encoding="utf-8"))
    digest = sha256_file(archive)
    declared = str(index.get("archive_sha256") or "").removeprefix("sha256:")
    if digest != declared:
        raise CampaignError(f"{ARCHIVE_NAME} SHA-256 {digest} does not match index {declared or 'missing'}")
    if index.get("archive_size_bytes") not in (None, archive.stat().st_size):
        raise CampaignError(f"{ARCHIVE_NAME} size does not match {INDEX_NAME}")

    extract_archive(archive, workdir)
    corpus = workdir / "corpus"
    manifest = json.loads((corpus / "manifest.json").read_text(encoding="utf-8"))
    if manifest.get("manifest_schema_version") != MANIFEST_SCHEMA_VERSION:
        raise CampaignError(
            f"manifest schema {manifest.get('manifest_schema_version')!r} is not {MANIFEST_SCHEMA_VERSION}"
        )
    entries = manifest.get("files") or []
    if not entries:
        raise CampaignError("manifest declares no files")
    for entry in entries:
        payload = (corpus / entry["path"]).resolve()
        if corpus.resolve() not in payload.parents or not payload.is_file():
            raise CampaignError(f"manifest payload is missing or outside the corpus: {entry['path']}")
        if payload.stat().st_size != entry["size_bytes"] or sha256_file(payload) != entry["sha256"]:
            raise CampaignError(f"payload does not match its manifest digest: {entry['path']}")
    return corpus, entries, digest


# ---------------------------------------------------------------------------
# Viewer process and HTTP
# ---------------------------------------------------------------------------


class ViewerProcess:
    """The dcmview binary under test, with stdout/stderr drained in the background."""

    def __init__(self, command: list[str]):
        environment = dict(os.environ, DCMVIEW_VSCODE_BYPASS="1")
        self.process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            start_new_session=True,
        )
        self.stdout = bytearray()
        self.stderr = bytearray()
        self._changed = threading.Condition()
        self._threads = [
            threading.Thread(target=self._drain, args=(self.process.stdout, self.stdout), daemon=True),
            threading.Thread(target=self._drain, args=(self.process.stderr, self.stderr), daemon=True),
        ]
        for thread in self._threads:
            thread.start()

    def _drain(self, stream: BinaryIO, target: bytearray) -> None:
        for chunk in iter(lambda: stream.read1(4096), b""):
            with self._changed:
                target.extend(chunk)
                self._changed.notify_all()

    def wait_for_url(self, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        with self._changed:
            while time.monotonic() < deadline:
                for line in bytes(self.stdout).splitlines():
                    try:
                        event = json.loads(line)
                    except ValueError:
                        continue
                    if isinstance(event, dict) and event.get("type") == "server_started":
                        launch = urllib.parse.urlsplit(str(event["url"]))
                        if event.get("token") is not None:
                            launch = launch._replace(fragment=urllib.parse.urlencode({"token": event["token"]}))
                        return launch.geturl().rstrip("/")
                if self.process.poll() is not None:
                    raise CampaignError(f"dcmview exited during startup with code {self.process.returncode}")
                self._changed.wait(timeout=0.1)
        raise CampaignError(f"dcmview did not report startup within {timeout:.0f}s")

    def stop(self) -> int:
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait()
        for thread in self._threads:
            thread.join(timeout=1)
        return self.process.returncode


def http_get(base_url: str, path: str, timeout: float) -> dict[str, Any]:
    base_url, fragment = urllib.parse.urldefrag(base_url)
    token = urllib.parse.parse_qs(fragment).get("token", [""])[0]
    request = urllib.request.Request(
        f"{base_url.rstrip('/')}{path}", headers={"Authorization": f"Bearer {token}"}
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            status, headers, body = response.status, response.headers, response.read()
    except urllib.error.HTTPError as error:
        status, headers, body = error.code, error.headers, error.read()
    content_type = headers.get("content-type", "")
    parsed = None
    if content_type.split(";", 1)[0] == "application/json":
        try:
            parsed = json.loads(body)
        except ValueError:
            parsed = None
    return {
        "path": path,
        "status": status,
        "content_type": content_type,
        "x_cache": headers.get("x-cache"),
        "headers": {key.lower(): value for key, value in headers.items()},
        "body": body,
        "json": parsed,
    }


def wait_for_scan(base_url: str, timeout: float) -> tuple[dict[str, Any], dict[str, Any]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        files = http_get(base_url, "/api/files", 10)["json"] or {}
        series = http_get(base_url, "/api/series", 10)["json"] or {}
        if files.get("scan_complete") and series.get("scan_complete"):
            return files, series
        time.sleep(0.05)
    raise CampaignError(f"discovery did not complete within {timeout:.0f}s")


# ---------------------------------------------------------------------------
# PNG decoding (display frames)
# ---------------------------------------------------------------------------


def png_chunks(payload: bytes) -> list[tuple[bytes, bytes]]:
    if payload[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("response is not a PNG")
    offset, chunks = 8, []
    while offset + 12 <= len(payload):
        (length,) = struct.unpack(">I", payload[offset : offset + 4])
        kind = payload[offset + 4 : offset + 8]
        data = payload[offset + 8 : offset + 8 + length]
        (crc,) = struct.unpack(">I", payload[offset + 8 + length : offset + 12 + length])
        if zlib.crc32(kind + data) != crc:
            raise ValueError(f"invalid PNG {kind!r} chunk CRC")
        chunks.append((kind, data))
        offset += 12 + length
        if kind == b"IEND":
            return chunks
    raise ValueError("PNG is missing IEND")


def png_pixels(payload: bytes) -> tuple[int, int, list[tuple[int, int, int]]]:
    """Decode an 8-bit, non-interlaced PNG into RGB triples."""
    header = b""
    compressed = bytearray()
    for kind, data in png_chunks(payload):
        if kind == b"IHDR":
            header = data
        elif kind == b"IDAT":
            compressed.extend(data)
    width, height, bit_depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", header)
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if bit_depth != 8 or interlace or channels is None:
        raise ValueError(f"unsupported PNG layout: depth {bit_depth}, color type {color_type}")
    raw = zlib.decompress(bytes(compressed))
    stride = width * channels
    prior = bytearray(stride)
    pixels: list[tuple[int, int, int]] = []
    for row in range(height):
        start = row * (stride + 1)
        filter_type, line = raw[start], bytearray(raw[start + 1 : start + 1 + stride])
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up, up_left = prior[i], prior[i - channels] if i >= channels else 0
            if filter_type == 1:
                line[i] = (line[i] + left) & 255
            elif filter_type == 2:
                line[i] = (line[i] + up) & 255
            elif filter_type == 3:
                line[i] = (line[i] + (left + up) // 2) & 255
            elif filter_type == 4:
                estimate = left + up - up_left
                predictor = min((abs(estimate - left), left), (abs(estimate - up), up), (abs(estimate - up_left), up_left), key=lambda pair: pair[0])[1]
                line[i] = (line[i] + predictor) & 255
            elif filter_type != 0:
                raise ValueError(f"unsupported PNG filter {filter_type}")
        prior = line
        for i in range(0, stride, channels):
            sample = line[i : i + channels]
            pixels.append((sample[0], sample[0], sample[0]) if channels <= 2 else (sample[0], sample[1], sample[2]))
    return width, height, pixels


def png_icc_profile(payload: bytes) -> Optional[bytes]:
    profiles = [data for kind, data in png_chunks(payload) if kind == b"iCCP"]
    if not profiles:
        return None
    data = profiles[0]
    separator = data.index(b"\0")
    return zlib.decompress(data[separator + 2 :])


# ---------------------------------------------------------------------------
# Display oracle
# ---------------------------------------------------------------------------


def _number(value: Any) -> Optional[float]:
    """Parse a manifest number, taking the first value of a DICOM multi-value string."""
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return float(value)
    if isinstance(value, str) and value.strip():
        return float(value.split("\\")[0])
    return None


def linear_window(value: float, center: float, width: float) -> int:
    """The DICOM LINEAR VOI function (PS3.3 C.11.2.1.2.1) onto 0..255."""
    width = max(width, 1.0)
    if value <= center - 0.5 - (width - 1) / 2:
        return 0
    if value > center - 0.5 + (width - 1) / 2 or width == 1:
        return 255
    return math.floor(((value - (center - 0.5)) / (width - 1) + 0.5) * 255 + 0.5)


def expected_display(entry: dict[str, Any], frame: int) -> tuple[Optional[dict[str, Any]], str]:
    """Compute the exact display frame dcmview should serve, independently of dcmview.

    Returns ``(oracle, reason)``. The oracle names the request ``mode`` and the
    expected RGB triples, with ``None`` for pixels whose display value the viewer
    contract leaves open (padding). When a manifest lacks what the computation
    needs, the oracle is ``None`` and ``reason`` says why.

    Monochrome frames follow the documented pipeline: stored value, Rescale
    Slope/Intercept, LINEAR window, MONOCHROME1 inversion. A declared window is
    checked through the default request. Without one the viewer's default is a
    percentile heuristic, so the check uses ``mode=full_dynamic`` instead, whose
    contract is the LINEAR window spanning the frame's rescaled minimum and
    maximum. Pixel Padding samples are left out of that range.
    """
    image = entry.get("image") or {}
    recipe = (entry.get("recipe") or {}).get("recipe_parameters") or {}
    values = recipe.get("pixel_values")
    if (entry.get("dicom") or {}).get("transfer_syntax_uid") == JPEG_BASELINE:
        return None, "lossy JPEG output is checked against error bounds on raw frames"
    if not isinstance(values, list) or not image:
        return None, "manifest has no recipe pixel values"
    if values and all(isinstance(frame_values, list) for frame_values in values):
        values = [sample for frame_values in values for sample in frame_values]  # one list per frame
    if not all(isinstance(value, (int, float)) and not isinstance(value, bool) for value in values):
        return None, "recipe pixel values are not plain numbers"
    for key in DISPLAY_ORACLE_EXCLUSIONS:
        if recipe.get(key):
            return None, f"recipe declares {key}"
    if recipe.get("presentation_lut_shape") not in (None, "IDENTITY"):
        return None, f"presentation LUT shape {recipe['presentation_lut_shape']}"

    samples_per_pixel = image.get("samples_per_pixel") or 1
    count = image["rows"] * image["columns"] * samples_per_pixel
    frame_values = values[frame * count : (frame + 1) * count]
    if len(frame_values) != count:
        return None, f"recipe pixel values do not cover frame {frame}"
    photometric = image.get("photometric_interpretation")

    if samples_per_pixel == 3 and photometric == "RGB":
        pixel_count = count // 3
        if image.get("planar_configuration") == 1:
            planes = [frame_values[plane * pixel_count : (plane + 1) * pixel_count] for plane in range(3)]
            rgb = list(zip(*planes))
        else:
            rgb = [tuple(frame_values[i : i + 3]) for i in range(0, count, 3)]
        return {"mode": "default", "pixels": rgb}, ""
    if samples_per_pixel != 1 or photometric not in ("MONOCHROME1", "MONOCHROME2"):
        return None, f"no independent oracle for {samples_per_pixel}-sample {photometric}"

    rescale = recipe.get("rescale") or {}
    slope = _number(rescale.get("slope"))
    intercept = _number(rescale.get("intercept"))
    rescaled = [value * (1.0 if slope is None else slope) + (intercept or 0.0) for value in frame_values]
    padding = recipe.get("pixel_padding") or {}
    padding_value = _number(padding.get("value"))
    if padding_value is None:
        padded = [False] * len(frame_values)
    else:
        limit = _number(padding.get("range_limit"))
        low, high = sorted((padding_value, padding_value if limit is None else limit))
        padded = [low <= value <= high for value in frame_values]

    window = recipe.get("window") or {}
    center = _number(window.get("center", recipe.get("window_center")))
    width = _number(window.get("width", recipe.get("window_width")))
    if center is not None and width is not None:
        mode = "default"
    else:
        unpadded = [value for value, pad in zip(rescaled, padded) if not pad] or rescaled
        low, high = min(unpadded), max(unpadded)
        width = max(high - low, 1.0)
        center = low + width / 2
        mode = "full_dynamic"
    pixels: list[Optional[tuple[int, int, int]]] = []
    for value, pad in zip(rescaled, padded):
        if pad:
            pixels.append(None)
            continue
        level = linear_window(value, center, width)
        if photometric == "MONOCHROME1":
            level = 255 - level
        pixels.append((level, level, level))
    return {"mode": mode, "pixels": pixels}, ""


def compare_display(expected: list[Optional[tuple[int, int, int]]], observed: list[tuple[int, int, int]]) -> dict[str, Any]:
    if len(expected) != len(observed):
        return check(False, error=f"expected {len(expected)} pixels, observed {len(observed)}")
    mismatches = [
        {"index": index, "expected": list(want), "observed": list(got)}
        for index, (want, got) in enumerate(zip(expected, observed))
        if want is not None and tuple(want) != got
    ]
    return check(
        not mismatches,
        compared_pixels=sum(want is not None for want in expected),
        mismatched_pixels=len(mismatches),
        first_mismatches=mismatches[:5],
    )


# ---------------------------------------------------------------------------
# Raw frame checks
# ---------------------------------------------------------------------------


def canonical_raw_bytes(payload: bytes, image: dict[str, Any], transfer_syntax_uid: Optional[str]) -> bytes:
    """Normalize raw samples to the generator's frame-hash convention.

    dcmview expands one-bit deflated frames to one byte per sample and masks
    integer samples to Bits Stored; the manifest hashes the bit-packed frame and
    the stored value respectively.
    """
    bits_allocated = image.get("bits_allocated")
    bits_stored = image.get("bits_stored")
    if bits_allocated == 1 and transfer_syntax_uid == "1.2.840.10008.1.2.8.1":
        packed = bytearray((len(payload) + 7) // 8)
        for index, sample in enumerate(payload):
            packed[index // 8] |= (sample & 1) << (index % 8)
        return bytes(packed)
    width = (bits_allocated or 0) // 8
    if bits_allocated in (8, 16) and isinstance(bits_stored, int) and 0 < bits_stored < bits_allocated and len(payload) % width == 0:
        mask = (1 << bits_stored) - 1
        return b"".join(
            (int.from_bytes(payload[i : i + width], "little") & mask).to_bytes(width, "little")
            for i in range(0, len(payload), width)
        )
    return payload


def raw_header_check(headers: dict[str, str], image: dict[str, Any]) -> dict[str, Any]:
    declared = {
        "x-frame-rows": image.get("rows"),
        "x-frame-columns": image.get("columns"),
        "x-frame-bits-allocated": image.get("bits_allocated"),
        "x-frame-pixel-representation": image.get("pixel_representation"),
        "x-frame-samples-per-pixel": image.get("samples_per_pixel"),
        "x-frame-photometric-interpretation": image.get("photometric_interpretation"),
    }
    expected = {name: str(value) for name, value in declared.items() if value is not None}
    observed = {name: headers.get(name) for name in expected}
    return check(observed == expected, expected=expected, observed=observed)


def lossy_check(payload: bytes, entry: dict[str, Any]) -> dict[str, Any]:
    reference = ((entry.get("recipe") or {}).get("recipe_parameters") or {}).get("pixel_values")
    tolerance = None
    for validation in (entry.get("validation") or {}).get("internal") or []:
        match = re.search(r"within \+/-([0-9]+(?:\.[0-9]+)?)", validation.get("message") or "")
        if validation.get("name") == "jpeg_baseline_decoded_frame_tolerance" and match:
            tolerance = float(match.group(1))
    if not isinstance(reference, list) or tolerance is None:
        return check(False, error="manifest lacks recipe pixel values or a JPEG Baseline tolerance")
    observed = list(payload)
    if len(observed) != len(reference):
        return check(False, expected_samples=len(reference), observed_samples=len(observed))
    errors = [abs(got - want) for got, want in zip(observed, reference)]
    maximum = max(errors, default=0)
    rmse = math.sqrt(sum(error * error for error in errors) / len(errors)) if errors else 0.0
    return check(maximum <= tolerance and rmse <= tolerance, max_abs_error=maximum, rmse=rmse, tolerance=tolerance)


def _integer_samples(payload: bytes, image: dict[str, Any]) -> Optional[list[int]]:
    bits = image.get("bits_allocated")
    if bits not in (8, 16, 32) or len(payload) % (bits // 8):
        return None
    width, signed = bits // 8, image.get("pixel_representation") == 1
    return [int.from_bytes(payload[i : i + width], "little", signed=signed) for i in range(0, len(payload), width)]


# ---------------------------------------------------------------------------
# Metadata and feature checks
# ---------------------------------------------------------------------------


def _tag_key(value: str) -> str:
    return f"({value.strip().strip('()').upper()})"


def _tag_scalar(node: Optional[dict[str, Any]]) -> Any:
    value = (node or {}).get("value") or {}
    return value.get("value") if value.get("type") in {"string", "number", "numbers"} else None


def _declared_tags(expected_metadata: dict[str, Any]) -> list[dict[str, Any]]:
    """Flatten the manifest's expected_metadata into tag expectations the tag API can confirm."""
    rows: list[dict[str, Any]] = []

    def visit(value: Any) -> None:
        if isinstance(value, list):
            for item in value:
                visit(item)
            return
        if not isinstance(value, dict):
            return
        tag = value.get("tag")
        if isinstance(tag, str) and "decoded_value" in value:
            rows.append({"tag": _tag_key(tag), "vr": value.get("vr"), "expected": value["decoded_value"]})
        elif isinstance(tag, str) and "decoded_values" in value:
            decoded = value["decoded_values"]
            if value.get("vr") not in {"DS", "IS"}:
                joined = "; ".join(str(item) for item in decoded)
                decoded = joined[:256] + ("…" if len(joined) > 256 else "")
            rows.append({"tag": _tag_key(tag), "vr": value.get("vr"), "expected": decoded})
        if isinstance(value.get("sequence_tag"), str) and isinstance(value.get("decoded_items"), list):
            rows.append({"tag": _tag_key(value["sequence_tag"]), "vr": "SQ", "expected_items": value["decoded_items"]})
        if isinstance(value.get("creator_tag"), str) and isinstance(value.get("creator_id"), str):
            rows.append({"tag": _tag_key(value["creator_tag"]), "vr": value.get("vr"), "expected": value["creator_id"]})
        for nested in value.values():
            visit(nested)

    visit(expected_metadata)
    for row in expected_metadata.get("empty_type2_attributes", []):
        rows.append({"tag": _tag_key(row["tag"]), "vr": row.get("vr"), "expected": ""})
    character_sets = expected_metadata.get("specific_character_sets")
    if isinstance(character_sets, list):
        rows.append({"tag": "(0008,0005)", "vr": "CS", "expected": "; ".join(map(str, character_sets))})
    return list({(row["tag"], row["vr"]): row for row in rows}.values())


def metadata_check(summary: dict[str, Any], info: Any, tags: Any, entry: dict[str, Any]) -> dict[str, Any]:
    if not isinstance(info, dict) or not isinstance(tags, list):
        return check(False, error="info or tags endpoint did not return JSON")
    dicom, uids, image = entry.get("dicom") or {}, entry.get("uids") or {}, entry.get("image") or {}
    identity = {
        "sop_instance_uid": uids.get("sop_instance_uid"),
        "study_instance_uid": uids.get("study_instance_uid"),
        "series_instance_uid": uids.get("series_instance_uid"),
        "sop_class_uid": dicom.get("sop_class_uid"),
        "transfer_syntax_uid": dicom.get("transfer_syntax_uid"),
        "modality": dicom.get("modality"),
    }
    geometry = {"frame_count": image.get("frames"), "rows": image.get("rows"), "columns": image.get("columns")} if image else {}
    mismatches = []
    for source_name, source, declared in (
        ("files", summary, {**identity, **geometry}),
        ("info", info, {"sop_class_uid": identity["sop_class_uid"], "transfer_syntax_uid": identity["transfer_syntax_uid"], **geometry}),
    ):
        for key, want in declared.items():
            if want is not None and source.get(key) != want:
                mismatches.append({"source": source_name, "field": key, "expected": want, "observed": source.get(key)})

    tag_index = {row.get("tag"): row for row in tags if isinstance(row, dict)}
    for expectation in _declared_tags(entry.get("expected_metadata") or {}):
        node = tag_index.get(expectation["tag"])
        if "expected_items" in expectation:
            items = ((node or {}).get("value") or {}).get("items") or []
            observed_items = [
                {
                    "code_meaning": by_keyword.get("CodeMeaning"),
                    "code_value": by_keyword.get("CodeValue"),
                    "coding_scheme_designator": by_keyword.get("CodingSchemeDesignator"),
                }
                for by_keyword in ({child.get("keyword"): _tag_scalar(child) for child in item} for item in items)
            ]
            if observed_items != expectation["expected_items"]:
                mismatches.append({"tag": expectation["tag"], "expected": expectation["expected_items"], "observed": observed_items})
            continue
        want = expectation["expected"]
        if expectation["vr"] in {"DS", "IS"} and isinstance(want, list):
            want = [float(value) for value in want]
            want = want[0] if len(want) == 1 else want
        observed = _tag_scalar(node)
        if node is None or node.get("vr") != expectation["vr"] or observed != want:
            mismatches.append({"tag": expectation["tag"], "vr": expectation["vr"], "expected": want, "observed": observed})
    return check(not mismatches, mismatches=mismatches)


def exact_tags_check(tags: Any, fields: list[tuple[str, str, Any]]) -> dict[str, Any]:
    tag_index = {row.get("tag"): row for row in tags or [] if isinstance(row, dict)}
    results = []
    for name, tag, want in fields:
        if isinstance(want, list) and all(isinstance(item, str) for item in want):
            want = "; ".join(want)
        observed = _tag_scalar(tag_index.get(_tag_key(tag)))
        results.append({"field": name, "tag": _tag_key(tag), "expected": want, "observed": observed, "passed": observed == want})
    return check(bool(results) and all(row["passed"] for row in results), results=results)


def nm_dimensions_check(tags: Any, entry: dict[str, Any]) -> dict[str, Any]:
    nm = entry.get("expected_nm_multiframe") or {}
    return exact_tags_check(tags, [
        ("image_type", "0008,0008", nm.get("image_type")),
        ("counts_accumulated", "0018,0070", nm.get("counts_accumulated")),
        ("actual_frame_duration_ms", "0018,1242", nm.get("actual_frame_duration_ms")),
        ("energy_window_vector", "0054,0010", nm.get("energy_window_vector")),
        ("number_of_energy_windows", "0054,0011", nm.get("number_of_energy_windows")),
        ("detector_vector", "0054,0020", nm.get("detector_vector")),
        ("number_of_detectors", "0054,0021", nm.get("number_of_detectors")),
    ])


def frame_time_check(summary: dict[str, Any], tags: Any, entry: dict[str, Any]) -> dict[str, Any]:
    ultrasound = entry.get("expected_us_multiframe") or {}
    result = exact_tags_check(tags, [
        ("image_type", "0008,0008", ultrasound.get("image_type")),
        ("frame_time_ms", "0018,1063", ultrasound.get("frame_time_ms")),
        ("color_data_present", "0028,0014", int(bool(ultrasound.get("color_data_present")))),
        ("lossy_image_compression", "0028,2110", ultrasound.get("lossy_image_compression")),
    ])
    frame_time, frame_count = ultrasound.get("frame_time_ms"), summary.get("frame_count")
    relative = [index * frame_time for index in range(frame_count)] if isinstance(frame_time, (int, float)) and isinstance(frame_count, int) else None
    passed = relative == ultrasound.get("frame_relative_times_ms")
    result["results"].append({"field": "frame_relative_times_ms", "expected": ultrasound.get("frame_relative_times_ms"), "observed": relative, "passed": passed})
    result["passed"] = result["passed"] and passed
    return result


def projection_geometry_check(tags: Any, entry: dict[str, Any]) -> dict[str, Any]:
    projection = entry.get("expected_xa_projection") or entry.get("expected_xrf_projection") or {}
    fields = [
        ("image_type", "0008,0008", projection.get("image_type")),
        ("body_part_examined", "0018,0015", projection.get("body_part_examined")),
        ("kvp", "0018,0060", projection.get("kvp")),
        ("distance_source_to_detector_mm", "0018,1110", projection.get("distance_source_to_detector_mm")),
        ("distance_source_to_patient_mm", "0018,1111", projection.get("distance_source_to_patient_mm")),
        ("estimated_radiographic_magnification_factor", "0018,1114", projection.get("estimated_radiographic_magnification_factor")),
        ("exposure_mas", "0018,1152", projection.get("exposure_mas")),
        ("radiation_setting", "0018,1155", projection.get("radiation_setting")),
        ("imager_pixel_spacing_mm", "0018,1164", projection.get("imager_pixel_spacing_mm")),
        ("pixel_intensity_relationship", "0028,1040", projection.get("pixel_intensity_relationship")),
        ("lossy_image_compression", "0028,2110", projection.get("lossy_image_compression")),
    ]
    if "expected_xa_projection" in entry:
        fields += [
            ("positioner_primary_angle_degrees", "0018,1510", projection.get("positioner_primary_angle_degrees")),
            ("positioner_secondary_angle_degrees", "0018,1511", projection.get("positioner_secondary_angle_degrees")),
        ]
    else:
        fields.append(("column_angulation_degrees", "0018,1450", projection.get("column_angulation_degrees")))
    return exact_tags_check(tags, fields)


def pet_activity_check(tags: Any, raw: Optional[bytes], entry: dict[str, Any]) -> dict[str, Any]:
    pet = entry.get("expected_pet_activity") or {}
    result = exact_tags_check(tags, [
        ("image_type", "0008,0008", pet.get("image_type")),
        ("actual_frame_duration_ms", "0018,1242", pet.get("actual_frame_duration_ms")),
        ("corrected_image", "0028,0051", pet.get("corrected_image")),
        ("rescale_intercept", "0028,1052", pet.get("rescale_intercept")),
        ("rescale_slope", "0028,1053", pet.get("rescale_slope")),
        ("number_of_slices", "0054,0081", pet.get("number_of_slices")),
        ("series_type", "0054,1000", pet.get("series_type")),
        ("units", "0054,1001", pet.get("units")),
        ("counts_source", "0054,1002", pet.get("counts_source")),
        ("decay_correction", "0054,1102", pet.get("decay_correction")),
        ("frame_reference_time_ms", "0054,1300", pet.get("frame_reference_time_ms")),
        ("dose_calibration_factor", "0054,1322", pet.get("dose_calibration_factor")),
        ("image_index", "0054,1330", pet.get("image_index")),
    ])
    stored = _integer_samples(raw or b"", entry.get("image") or {})
    slope, intercept = pet.get("rescale_slope"), pet.get("rescale_intercept")
    activity = [value * slope + intercept for value in stored] if stored is not None and isinstance(slope, (int, float)) and isinstance(intercept, (int, float)) else None
    for field, want, got in (("stored_values", pet.get("stored_values"), stored), ("activity_values_bqml", pet.get("activity_values_bqml"), activity)):
        result["results"].append({"field": field, "expected": want, "observed": got, "passed": got == want})
    result["passed"] = all(row["passed"] for row in result["results"])
    return result


def pixel_geometry_check(summary: dict[str, Any], entry: dict[str, Any]) -> dict[str, Any]:
    geometry = entry.get("expected_nonsquare_spacing") or {}
    spacing = geometry.get("pixel_spacing") or {}
    aspect = geometry.get("pixel_aspect_ratio") or {}
    expected = None
    if _number(spacing.get("row_spacing_mm")) and _number(spacing.get("column_spacing_mm")):
        expected = spacing["row_spacing_mm"] / spacing["column_spacing_mm"]
    elif _number(aspect.get("vertical_extent")) and _number(aspect.get("horizontal_extent")):
        expected = aspect["vertical_extent"] / aspect["horizontal_extent"]
    observed = summary.get("pixel_aspect_ratio")
    return check(expected is not None and observed == expected, expected=expected, observed=observed)


def overlay_check(entry: dict[str, Any], pixels: list[tuple[int, int, int]]) -> dict[str, Any]:
    semantics, image = entry.get("expected_semantics") or {}, entry.get("image") or {}
    if semantics.get("overlay_pattern") != "2x2_diagonal_overlay" or (image.get("rows"), image.get("columns")) != (2, 2):
        return not_computable(f"no exact oracle for overlay pattern {semantics.get('overlay_pattern')!r}")
    white = [index for index, pixel in enumerate(pixels) if pixel == (255, 255, 255)]
    return check(len(pixels) == 4 and 0 in white and 3 in white and 1 not in white and 2 not in white, observed_white=white)


def shutter_check(entry: dict[str, Any], pixels: list[tuple[int, int, int]]) -> dict[str, Any]:
    """Pixels outside a declared rectangular opening must take the shutter value."""
    shutter = (entry.get("expected_semantics") or {}).get("display_shutter") or {}
    image = entry.get("image") or {}
    try:
        left, right = int(shutter["left_vertical_edge"]), int(shutter["right_vertical_edge"])
        upper, lower = int(shutter["upper_horizontal_edge"]), int(shutter["lower_horizontal_edge"])
    except (KeyError, TypeError, ValueError):
        return not_computable("manifest does not declare rectangular shutter bounds")
    columns = image.get("columns") or 1
    outside = [
        index for index in range(len(pixels))
        if not (left <= index % columns + 1 <= right and upper <= index // columns + 1 <= lower)
    ]
    if not outside:
        return not_computable("the declared opening covers the whole frame, so no pixel is shuttered")
    value = (entry.get("expected_semantics") or {}).get("shutter_presentation_value")
    level = None if value is None else (int(value) * 255 + 32_767) // 65_535
    values = sorted({pixels[index] for index in outside})
    return check(len(values) == 1 and (level is None or values[0] == (level, level, level)), outside_values=values)


def icc_check(entry: dict[str, Any], png: bytes) -> dict[str, Any]:
    contract = entry.get("expected_icc_profile") or {}
    try:
        profile = png_icc_profile(png)
    except (ValueError, zlib.error) as error:
        return check(False, error=str(error))
    observed = hashlib.sha256(profile).hexdigest() if profile is not None else None
    return check(
        observed == contract.get("profile_sha256") and len(profile or b"") == contract.get("profile_size_bytes"),
        expected_sha256=contract.get("profile_sha256"),
        observed_sha256=observed,
    )


def references_check(payload: Any, expected_references: list[dict[str, Any]]) -> dict[str, Any]:
    """Every declared reference appears with the same identity and resolves to its local target."""

    def identity(relationship: Any, target: dict[str, Any]) -> dict[str, Any]:
        return {
            "relationship": relationship,
            "sop_class_uid": target.get("sop_class_uid"),
            "sop_instance_uid": target.get("sop_instance_uid"),
            "series_instance_uid": target.get("series_instance_uid"),
            "frame_numbers": target.get("frame_numbers") or [],
        }

    observed_rows = [row for row in (payload or {}).get("references") or [] if isinstance(row, dict)]
    observed = [identity(row.get("relationship"), row.get("target") or {}) for row in observed_rows]
    expected = [identity(row.get("relationship"), row) for row in expected_references]
    key = lambda value: json.dumps(value, sort_keys=True)  # noqa: E731
    identities_match = sorted(map(key, observed)) == sorted(map(key, expected))
    unresolved = []
    for want, row in zip(expected, expected_references):
        frames = [number - 1 for number in want["frame_numbers"] if number > 0]
        source_path = row.get("source_path")
        candidate = next((r for r, i in zip(observed_rows, observed) if i == want), None)
        resolved = any(
            match.get("sop_instance_uid") == want["sop_instance_uid"]
            and (not frames or (match.get("frame_indices") or []) == frames)
            and (not source_path or str(match.get("path") or "").replace("\\", "/").endswith("/" + source_path))
            for match in (candidate or {}).get("matches") or []
            if isinstance(match, dict)
        )
        if not resolved:
            unresolved.append(want)
    return check(identities_match and not unresolved, identities_match=identities_match, unresolved=unresolved)


def series_check(catalog: dict[str, Any], summary: dict[str, Any], entry: dict[str, Any]) -> dict[str, Any]:
    located = next(
        (
            (series, stack, frame)
            for series in catalog.get("series") or []
            for stack in series.get("stacks") or []
            for frame in stack.get("frames") or []
            if frame.get("file_index") == summary["index"]
        ),
        None,
    )
    if located is None:
        return check(False, error="file is not in the series catalog")
    series, stack, frame = located
    capabilities = set(entry.get("expected_capabilities") or [])
    geometry, semantics = entry.get("expected_geometry") or {}, entry.get("expected_semantics") or {}
    warnings = [row.get("code") for row in stack.get("warnings") or []]
    results: dict[str, Any] = {}
    if "sort_series_by_geometry" in capabilities:
        order = geometry.get("geometric_order_index")
        if not isinstance(order, int):
            order = (semantics.get("geometry_sort_key") or {}).get("slice_order_index")
        results["sort_series_by_geometry"] = {
            "expected_virtual_index": order - 1 if isinstance(order, int) else None,
            "observed_virtual_index": frame.get("virtual_index"),
            "passed": isinstance(order, int) and frame.get("virtual_index") == order - 1,
        }
    if "interpret_gantry_tilt" in capabilities:
        results["interpret_gantry_tilt"] = {"observed_warnings": warnings, "passed": "gantry_tilt" in warnings}
    if "organize_series_by_study_and_frame_of_reference" in capabilities:
        frame_uids = set(series.get("frame_of_reference_uids") or [])
        peers = [
            other for other in catalog.get("series") or []
            if other.get("study_instance_uid") == series.get("study_instance_uid")
            and frame_uids.intersection(other.get("frame_of_reference_uids") or [])
        ]
        results["organize_series_by_study_and_frame_of_reference"] = {
            "peer_series": len(peers),
            "passed": len(peers) >= 2 and len({other.get("series_instance_uid") for other in peers}) == len(peers),
        }
    if "parse_multiframe_functional_groups" in capabilities:
        expected_kind = "concatenation" if semantics.get("concatenation") else "ordinary"
        results["parse_multiframe_functional_groups"] = {
            "expected_stack_kind": expected_kind,
            "observed_stack_kind": stack.get("kind"),
            "passed": stack.get("kind") == expected_kind,
        }
    return check(all(row["passed"] for row in results.values()), results=results)


def _overlay_eligibility(overlay: dict[str, Any], expected: bool) -> bool:
    has_source = isinstance(overlay.get("source_file_index"), int)
    return overlay.get("eligible") is expected and bool(overlay.get("reason")) and has_source is expected


def semantic_checks(payload: Any, entry: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Declared SEG, Parametric Map, and RT Dose meaning, with pixel preview as the default."""
    semantics = entry.get("expected_semantics") or {}
    context = (payload or {}).get("context")
    if not isinstance(context, dict):
        return {"semantic_context": check(False, error="missing semantic context payload")}
    preview_default = payload.get("default_mode") == "pixel_preview" and payload.get("pixel_preview_preserves_stored_values") is True
    checks = {"semantic_pixel_preview_default": check(preview_default, default_mode=payload.get("default_mode"))}
    kind = context.get("kind")
    if kind == "segmentation":
        segments = {row.get("number") for row in context.get("segments", [])}
        mappings = context.get("frame_mappings", [])
        frames = sorted({number for row in mappings for number in row.get("source_frame_numbers", [])})
        return {
            **checks,
            "segmentation_context": check(
                context.get("segmentation_type") == semantics.get("segmentation_type")
                and context.get("segmentation_fractional_type") == semantics.get("segmentation_fractional_type")
                and context.get("maximum_fractional_value") == semantics.get("maximum_fractional_value")
                and len(context.get("segments", [])) == semantics.get("segment_sequence_items")
            ),
            "segment_reference_closure": check(
                bool(mappings)
                and all(row.get("segment_number") in segments for row in mappings)
                and frames == (semantics.get("referenced_frame_numbers") or [])
                and all(row.get("source_sop_instance_uid") == semantics.get("source_sop_instance_uid") for row in mappings),
                observed_frames=frames,
            ),
            "segmentation_overlay": check(_overlay_eligibility(context.get("overlay") or {}, semantics.get("overlay_eligible", False))),
        }
    if kind == "parametric_map":
        declared = semantics.get("real_world_value_mapping") or {}
        mapping = (context.get("mappings") or [{}])[0]
        return {
            **checks,
            "parametric_context": check(context.get("stored_value_type") == semantics.get("sample_type")),
            "rwvm": check(
                mapping.get("label") == declared.get("lut_label")
                and mapping.get("slope") == declared.get("slope")
                and mapping.get("intercept") == declared.get("intercept")
                and (mapping.get("units") or {}).get("value") == (declared.get("units") or {}).get("code_value")
                and (mapping.get("quantity") or {}).get("value") == (declared.get("quantity_definition") or {}).get("code_value"),
                observed=context.get("mappings"),
            ),
            "stored_values_displayed": check(context.get("displayed_value_kind") == "stored"),
        }
    if kind == "rt_dose":
        dose = semantics.get("rt_dose") or {}
        scaling = _number(dose.get("dose_grid_scaling"))
        bounds_known = scaling is not None and all(isinstance(semantics.get(key), (int, float)) for key in ("pixel_min", "pixel_max"))
        return {
            **checks,
            "dose_context": check(
                context.get("dose_units") == dose.get("dose_units")
                and context.get("dose_type") == dose.get("dose_type")
                and context.get("dose_summation_type") == dose.get("dose_summation_type")
                and "grid_frame_offsets" in (context.get("geometry") or {})
            ),
            "dose_scaling": check(
                bounds_known
                and context.get("dose_grid_scaling") == scaling
                and context.get("scaling_status") == "available"
                and context.get("displayed_value_kind") == "mapped",
                expected=scaling,
                observed=context.get("dose_grid_scaling"),
            ),
            "dose_overlay": check(_overlay_eligibility(context.get("overlay") or {}, semantics.get("overlay_eligible", False))),
        }
    return {**checks, "semantic_context": check(False, error=f"unexpected semantic context kind {kind!r}")}


def wsi_checks(payloads: list[dict[str, Any]], entry: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Declared tile placement and optical paths, and no claim of slide reconstruction."""
    image = entry.get("image") or {}
    full = entry.get("expected_wsi_tiled_full") or {}
    sparse = entry.get("expected_wsi_tiled_sparse") or {}
    positions = {row["frame_number"] - 1: row for row in (full.get("tiling") or {}).get("implicit_frame_positions", [])}
    positions.update({row["frame_number"] - 1: row for row in sparse.get("per_frame_functional_groups", [])})
    optical = {}
    for path in (entry.get("expected_wsi_multiple_optical_paths") or {}).get("optical_paths", []):
        start, end = path.get("frame_ordinal_range", [0, -1])
        optical.update({frame: path.get("identifier") for frame in range(max(start - 1, 0), max(end, 0))})
    placement, minimap = [], []
    for payload in payloads:
        frame = payload.get("frame_index")
        rect, matrix = payload.get("tile_rectangle") or {}, payload.get("total_pixel_matrix") or {}
        placed = payload.get("positioning_status") == "positioned" and bool(rect)
        want = positions.get(frame)
        if want is not None:
            placed = placed and (
                rect.get("x") == want["column_position"] - 1
                and rect.get("y") == want["row_position"] - 1
                and payload.get("tile_row") == (want["row_position"] - 1) // image.get("rows", 1)
                and payload.get("tile_column") == (want["column_position"] - 1) // image.get("columns", 1)
            )
        if frame in optical:
            placed = placed and (payload.get("optical_path") or {}).get("identifier") == optical[frame]
        placement.append({"frame": frame, "passed": placed})
        inside = (
            all(isinstance(value, int) for value in (matrix.get("rows"), matrix.get("columns"), rect.get("x"), rect.get("y"), rect.get("width"), rect.get("height")))
            and rect["x"] >= 0 and rect["y"] >= 0
            and rect["x"] + rect["width"] <= matrix["columns"]
            and rect["y"] + rect["height"] <= matrix["rows"]
            and payload.get("reconstruction_claimed") is False
        )
        minimap.append({"frame": frame, "passed": inside})
    return {
        "wsi_position": check(bool(placement) and all(row["passed"] for row in placement), frames=placement),
        "wsi_minimap": check(bool(minimap) and all(row["passed"] for row in minimap), frames=minimap),
    }


# ---------------------------------------------------------------------------
# Per-file probe
# ---------------------------------------------------------------------------


def navigation_frames(case_id: str, frame_count: int) -> list[int]:
    """First, middle, last, and one case-seeded frame."""
    if frame_count <= 0:
        return []
    seeded = int.from_bytes(hashlib.sha256(case_id.encode()).digest()[:8], "big") % frame_count
    return sorted({0, frame_count // 2, frame_count - 1, seeded})


def probe(base_url: str, entry: dict[str, Any], summary: dict[str, Any], catalog: dict[str, Any], timeout: float) -> dict[str, dict[str, Any]]:
    get = lambda path: http_get(base_url, path, timeout)  # noqa: E731
    index = summary["index"]
    image = entry.get("image") or {}
    capabilities = set(entry.get("expected_capabilities") or [])
    transfer_syntax = (entry.get("dicom") or {}).get("transfer_syntax_uid")
    frame_count = int(summary.get("frame_count") or 0)
    checks: dict[str, dict[str, Any]] = {}

    info, tags = get(f"/api/file/{index}/info"), get(f"/api/file/{index}/tags")
    tag_rows = tags["json"] if tags["status"] == 200 else None
    checks["metadata"] = metadata_check(summary, info["json"] if info["status"] == 200 else None, tag_rows, entry)

    invalid = get(f"/api/file/{index}/frame/{frame_count}")
    recovered = get(f"/api/file/{index}/info")
    checks["error_recovery"] = check(
        invalid["status"] in (400, 404, 422)
        and isinstance((invalid["json"] or {}).get("error"), str)
        and recovered["status"] == 200,
        error_status=invalid["status"],
        recovery_status=recovered["status"],
    )

    if capabilities & SERIES_CAPABILITIES:
        checks["series"] = series_check(catalog, summary, entry)
    if capabilities & {"interpret_pixel_geometry"}:
        checks["pixel_geometry"] = pixel_geometry_check(summary, entry)
    if "interpret_nm_dimensions" in capabilities:
        checks["nm_dimensions"] = nm_dimensions_check(tag_rows, entry)
    if "interpret_frame_time" in capabilities:
        checks["frame_time"] = frame_time_check(summary, tag_rows, entry)
    if "interpret_projection_geometry" in capabilities:
        checks["projection_geometry"] = projection_geometry_check(tag_rows, entry)
    if entry.get("references"):
        references = get(f"/api/file/{index}/references")
        checks["references"] = references_check(references["json"], entry["references"])

    if not summary.get("has_pixels") or not image:
        checks["metadata_only"] = check(invalid["status"] == 404 and not summary.get("has_pixels"), frame_status=invalid["status"])
        return checks

    display = get(f"/api/file/{index}/frame/0")
    raw = get(f"/api/file/{index}/frame/0/raw")
    if transfer_syntax in UNSUPPORTED_TRANSFER_SYNTAXES:
        codes = [(response["json"] or {}).get("code") for response in (display, raw)]
        checks["unsupported_transfer_syntax"] = check(
            [display["status"], raw["status"]] == [422, 422] and codes == ["unsupported_transfer_syntax"] * 2,
            statuses=[display["status"], raw["status"]],
            codes=codes,
        )
        return checks

    display_again = get(f"/api/file/{index}/frame/0")
    raw_again = get(f"/api/file/{index}/frame/0/raw")
    checks["cache"] = check(
        [display["x_cache"], display_again["x_cache"], raw["x_cache"], raw_again["x_cache"]] == ["MISS", "HIT", "MISS", "HIT"]
        and display["body"] == display_again["body"],
        x_cache=[display["x_cache"], display_again["x_cache"], raw["x_cache"], raw_again["x_cache"]],
    )

    # Display frames: geometry for every navigated frame, exact values where computable.
    pixels_by_frame: dict[int, list[tuple[int, int, int]]] = {}
    geometry_errors = []
    for frame in navigation_frames(entry["case_id"], frame_count):
        response = display if frame == 0 else get(f"/api/file/{index}/frame/{frame}")
        try:
            width, height, pixels = png_pixels(response["body"]) if response["status"] == 200 else (None, None, [])
        except (ValueError, zlib.error, struct.error) as error:
            width, height, pixels = None, None, []
            geometry_errors.append({"frame": frame, "error": str(error)})
        if [width, height] != [image.get("columns"), image.get("rows")]:
            geometry_errors.append({"frame": frame, "status": response["status"], "observed": [width, height]})
        pixels_by_frame[frame] = pixels
    checks["display_frames"] = check(not geometry_errors, frames=sorted(pixels_by_frame), errors=geometry_errors)

    exact_results: dict[int, dict[str, Any]] = {}
    oracle: Optional[dict[str, Any]] = None
    for frame in sorted(pixels_by_frame):
        oracle, reason = expected_display(entry, frame)
        if oracle is None:
            checks["display_exact"] = not_computable(reason)
            break
        pixels = pixels_by_frame[frame]
        if oracle["mode"] != "default":
            response = get(f"/api/file/{index}/frame/{frame}?mode={oracle['mode']}")
            pixels = png_pixels(response["body"])[2] if response["status"] == 200 else []
        exact_results[frame] = compare_display(oracle["pixels"], pixels)
    else:
        checks["display_exact"] = check(
            bool(exact_results) and all(result["passed"] for result in exact_results.values()),
            mode=oracle["mode"] if oracle else None,
            frames={str(frame): result for frame, result in exact_results.items()},
        )

    first_pixels = pixels_by_frame.get(0, [])
    if "read_overlay_plane" in capabilities:
        overlay = get(f"/api/file/{index}/frame/0?mode=full_dynamic")
        checks["overlay"] = overlay_check(entry, png_pixels(overlay["body"])[2] if overlay["status"] == 200 else [])
    if "apply_display_shutter" in capabilities:
        checks["shutter"] = shutter_check(entry, first_pixels)
    if entry.get("expected_icc_profile"):
        checks["icc_profile"] = icc_check(entry, display["body"])

    # Raw frames: headers, then every frame against the generator's oracle.
    checks["raw_headers"] = raw_header_check(raw["headers"], image)
    frame_hashes = (entry.get("pixel_data") or {}).get("frame_hashes") or []
    if transfer_syntax == JPEG_BASELINE:
        checks["raw_lossy_error"] = lossy_check(raw["body"], entry) if raw["status"] == 200 else check(False, status=raw["status"])
    elif frame_hashes:
        observed = []
        for frame in range(len(frame_hashes)):
            response = raw if frame == 0 else get(f"/api/file/{index}/frame/{frame}/raw")
            observed.append(
                hashlib.sha256(canonical_raw_bytes(response["body"], image, transfer_syntax)).hexdigest()
                if response["status"] == 200 else f"http {response['status']}"
            )
        mismatched = [frame for frame, (want, got) in enumerate(zip(frame_hashes, observed)) if want != got]
        checks["raw_frame_hashes"] = check(not mismatched, frames=len(frame_hashes), mismatched_frames=mismatched)
    if "interpret_pet_activity" in capabilities:
        checks["pet_activity"] = pet_activity_check(tag_rows, raw["body"] if raw["status"] == 200 else None, entry)

    if (entry.get("dicom") or {}).get("sop_class_uid") in SEMANTIC_SOP_CLASSES:
        semantic = get(f"/api/file/{index}/semantic-context")
        checks.update(semantic_checks(semantic["json"] if semantic["status"] == 200 else None, entry))
        raw_after = get(f"/api/file/{index}/frame/0/raw")
        checks["semantic_preserves_raw"] = check(raw_after["status"] == 200 and raw_after["body"] == raw["body"])
    if summary.get("object_kind") == "whole_slide_microscopy":
        payloads = [get(f"/api/file/{index}/frame/{frame}/wsi-context")["json"] for frame in navigation_frames(entry["case_id"], frame_count)]
        checks.update(wsi_checks([payload for payload in payloads if isinstance(payload, dict)], entry))
    return checks


# ---------------------------------------------------------------------------
# Campaign
# ---------------------------------------------------------------------------


def run_campaign(container: Path, binary: Path, output: Path, timeout: float) -> dict[str, Any]:
    output.mkdir(parents=True, exist_ok=True)
    workdir = output / "corpus-extract"
    if workdir.exists():
        raise CampaignError(f"{workdir} already exists; use an empty output directory")
    corpus, entries, archive_sha256 = load_corpus(container, workdir)
    version = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True).stdout.strip()

    paths = [str((corpus / entry["path"]).resolve()) for entry in entries]
    viewer = ViewerProcess([str(binary), "--no-browser", "--host", "127.0.0.1", "--port", "0", "--startup-json", *paths])
    results = []
    try:
        base_url = viewer.wait_for_url(60)
        files, catalog = wait_for_scan(base_url, 120)
        health = http_get(base_url, "/api/health", timeout)["json"]
        by_identity = {(os.path.normcase(str(Path(row["path"]).resolve())), row["sop_instance_uid"]): row for row in files.get("files", [])}
        for entry, path in zip(entries, paths):
            started = time.monotonic()
            summary = by_identity.get((os.path.normcase(path), (entry.get("uids") or {}).get("sop_instance_uid")))
            try:
                checks = (
                    probe(base_url, entry, summary, catalog, timeout)
                    if summary is not None
                    else {"discovered": check(False, error="path and SOP Instance UID not in /api/files")}
                )
            except Exception as error:  # one broken case must not hide the others
                checks = {"probe": check(False, error=repr(error), traceback=traceback.format_exc())}
            failed = sorted(name for name, result in checks.items() if result["passed"] is False)
            results.append({
                "case_id": entry["case_id"],
                "path": entry["path"],
                "status": "failed" if failed else "passed",
                "failed_checks": failed,
                "not_computable": sorted(name for name, result in checks.items() if result["passed"] is None),
                "elapsed_ms": round((time.monotonic() - started) * 1000, 1),
                "checks": checks,
            })
    finally:
        exit_code = viewer.stop()
        (output / "viewer-stdout.log").write_bytes(bytes(viewer.stdout))
        (output / "viewer-stderr.log").write_bytes(bytes(viewer.stderr))

    tallies: dict[str, dict[str, int]] = {}
    for result in results:
        for name, outcome in result["checks"].items():
            bucket = tallies.setdefault(name, {"passed": 0, "failed": 0, "not_computable": 0})
            bucket[{True: "passed", False: "failed", None: "not_computable"}[outcome["passed"]]] += 1
    report = {
        "generated_at": datetime.now(timezone.utc).isoformat(),
        "viewer": {"binary": str(binary), "version": version, "health": health, "exit_code": exit_code},
        "corpus": {"archive_sha256": archive_sha256, "files": len(entries)},
        "summary": {
            "passed": sum(result["status"] == "passed" for result in results),
            "failed": sum(result["status"] == "failed" for result in results),
            "checks": dict(sorted(tallies.items())),
        },
        "results": results,
    }
    (output / "report.json").write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return report


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--corpus-root", type=Path, required=True, help=f"directory holding {ARCHIVE_NAME} and {INDEX_NAME}")
    parser.add_argument("--binary", type=Path, required=True, help="dcmview binary under test")
    parser.add_argument("--output", type=Path, required=True, help="empty directory for report.json and logs")
    parser.add_argument("--request-timeout", type=float, default=30.0)
    args = parser.parse_args(argv)
    try:
        report = run_campaign(args.corpus_root, args.binary.resolve(), args.output, args.request_timeout)
    except CampaignError as error:
        print(f"compatibility: {error}", file=sys.stderr)
        return 2
    summary = report["summary"]
    print(f"compatibility: {summary['passed']} passed, {summary['failed']} failed of {report['corpus']['files']} files")
    for name, counts in summary["checks"].items():
        print(f"  {name:28} {counts['passed']:4} passed  {counts['failed']:4} failed  {counts['not_computable']:4} not computable")
    for result in report["results"]:
        if result["status"] == "failed":
            print(f"  FAILED {result['path']}: {', '.join(result['failed_checks'])}")
    print(f"compatibility: report written to {args.output / 'report.json'}")
    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())

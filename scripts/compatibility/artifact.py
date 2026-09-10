#!/usr/bin/env python3
"""Verify and adapt a published external-corpus artifact for dcmview."""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import stat
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any

try:
    from scripts.compatibility.policy import (
        DEFAULT_POLICY,
        PolicyError,
        load_policy,
        policy_sha256,
        resolve,
    )
    from scripts.compatibility.worklist import (
        CONTRACT_EXCLUDED_FIELDS,
        WORKLIST_SCHEMA_VERSION,
        CompatibilityError,
        applicable_assertions,
        canonical_json,
    )
except ModuleNotFoundError:
    from policy import (  # type: ignore[no-redef]
        DEFAULT_POLICY,
        PolicyError,
        load_policy,
        policy_sha256,
        resolve,
    )
    from worklist import (  # type: ignore[no-redef]
        CONTRACT_EXCLUDED_FIELDS,
        WORKLIST_SCHEMA_VERSION,
        CompatibilityError,
        applicable_assertions,
        canonical_json,
    )


EXTERNAL_MANIFEST_SCHEMA_VERSION = "2.0.0"
EXTERNAL_MANIFEST_KIND = "external_corpus"
DEFAULT_PROFILE = "smoke"
DEFAULT_SEED = 1
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
SOURCE_REVISION_PATTERN = re.compile(r"^[0-9a-f]{40}$")
MAX_INDEX_BYTES = 16 * 1024 * 1024
MAX_ARCHIVE_BYTES = 1024 * 1024 * 1024
MAX_ARCHIVE_FILE_BYTES = 256 * 1024 * 1024
MAX_ARCHIVE_FILES = 4096
PRODUCER_ARCHIVE = "smoke.tar.gz"
PRODUCER_INDEX = "artifact-index.json"
PRODUCER_REPOSITORY = "beatrice-b-m/dcmview-test-corpus"
PRODUCER_WORKFLOW = "publish-smoke-artifact.yml"


class ArtifactError(CompatibilityError):
    """The supplied external corpus artifact is not safe to consume."""


def _require_sha256(value: Any, field: str) -> str:
    if not isinstance(value, str) or SHA256_PATTERN.fullmatch(value) is None:
        raise ArtifactError(f"{field} must be a lowercase SHA-256 digest")
    return value


def _relative_path(value: Any, field: str) -> Path:
    if not isinstance(value, str) or not value or "\x00" in value:
        raise ArtifactError(f"{field} must be a non-empty relative path")
    # Manifest paths are POSIX paths even when a consumer runs on Windows.  Do
    # not permit either platform's absolute or parent-traversing spelling.
    if (
        PurePosixPath(value).is_absolute()
        or PureWindowsPath(value).is_absolute()
        or PureWindowsPath(value).drive
        or "\\" in value
        or "//" in value
    ):
        raise ArtifactError(f"{field} must be relative: {value!r}")
    parts = PurePosixPath(value).parts
    windows_parts = PureWindowsPath(value).parts
    if not parts or any(part in {"", ".", ".."} for part in parts) or any(
        part in {"", ".", ".."} for part in windows_parts
    ):
        raise ArtifactError(f"{field} contains an unsafe path: {value!r}")
    return Path(*parts)


def _mapping(value: Any, field: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ArtifactError(f"{field} must be an object")
    return value


def _canonical_producer_json(value: Any) -> bytes:
    """Match publish_smoke_artifact.py's binding-byte canonicalization."""
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
            allow_nan=False,
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise ArtifactError(f"artifact binding is not canonical JSON: {error}") from error


def _canonical_sha256(value: Any) -> str:
    return hashlib.sha256(_canonical_producer_json(value)).hexdigest()


def _reject_symlink_components(path: Path) -> None:
    """Reject a caller path whose root or any component is a symlink."""
    absolute = Path(os.path.abspath(path))
    current = Path(absolute.anchor or os.sep)
    for part in absolute.parts[1:]:
        current /= part
        try:
            info = os.lstat(current)
        except OSError as error:
            raise ArtifactError(f"cannot inspect artifact path {current}: {error}") from error
        if stat.S_ISLNK(info.st_mode):
            raise ArtifactError(f"artifact path contains a symlink: {current}")


def _open_nofollow(root: Path, relative: PurePosixPath) -> int:
    """Open a relative file below root with no-follow directory traversal."""
    if relative.is_absolute() or not relative.parts:
        raise ArtifactError(f"unsafe relative path: {relative}")
    try:
        descriptor = os.open(
            root,
            os.O_RDONLY
            | getattr(os, "O_DIRECTORY", 0)
            | getattr(os, "O_NOFOLLOW", 0),
        )
    except OSError as error:
        raise ArtifactError(f"cannot safely open artifact root {root}: {error}") from error
    try:
        for component in relative.parts[:-1]:
            if component in {"", ".", ".."}:
                raise ArtifactError(f"unsafe relative path: {relative}")
            next_descriptor = os.open(
                component,
                os.O_RDONLY
                | getattr(os, "O_DIRECTORY", 0)
                | getattr(os, "O_NOFOLLOW", 0),
                dir_fd=descriptor,
            )
            os.close(descriptor)
            descriptor = next_descriptor
        leaf = relative.parts[-1]
        if leaf in {"", ".", ".."}:
            raise ArtifactError(f"unsafe relative path: {relative}")
        file_descriptor = os.open(
            leaf,
            os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=descriptor,
        )
    except OSError as error:
        os.close(descriptor)
        raise ArtifactError(f"cannot safely open {relative}: {error}") from error
    os.close(descriptor)
    return file_descriptor


def _fingerprint(info: os.stat_result) -> tuple[int, int, int, int, int, int, int]:
    return (
        info.st_dev,
        info.st_ino,
        info.st_mode,
        info.st_nlink,
        info.st_size,
        info.st_mtime_ns,
        info.st_ctime_ns,
    )


def _read_nofollow(
    root: Path,
    relative: PurePosixPath,
    *,
    max_bytes: int,
    expected_size: int | None = None,
    expected_sha256: str | None = None,
) -> tuple[bytes, int, str]:
    """Read a bounded regular file through a stable no-follow descriptor."""
    descriptor = _open_nofollow(root, relative)
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode) or stat.S_ISLNK(before.st_mode):
            raise ArtifactError(f"artifact entry is not a regular file: {relative}")
        if before.st_nlink != 1:
            raise ArtifactError(f"artifact entry has hard-link aliases: {relative}")
        if before.st_size < 0 or before.st_size > max_bytes:
            raise ArtifactError(f"artifact entry exceeds its bounded size: {relative}")
        if expected_size is not None and before.st_size != expected_size:
            raise ArtifactError(
                f"artifact size mismatch for {relative}: expected {expected_size}, observed {before.st_size}"
            )
        digest = hashlib.sha256()
        chunks: list[bytes] = []
        size = 0
        while True:
            chunk = os.read(descriptor, min(1024 * 1024, max_bytes - size + 1))
            if not chunk:
                break
            size += len(chunk)
            if size > max_bytes or size > before.st_size:
                raise ArtifactError(f"artifact changed while reading: {relative}")
            digest.update(chunk)
            chunks.append(chunk)
        after = os.fstat(descriptor)
        if size != before.st_size or _fingerprint(before) != _fingerprint(after):
            raise ArtifactError(f"artifact changed while reading: {relative}")
        observed = digest.hexdigest()
        if expected_sha256 is not None and observed != expected_sha256:
            raise ArtifactError(
                f"artifact hash mismatch for {relative}: expected {expected_sha256}, observed {observed}"
            )
        return b"".join(chunks), size, observed
    except OSError as error:
        raise ArtifactError(f"cannot read artifact entry {relative}: {error}") from error
    finally:
        os.close(descriptor)


def _read_json_nofollow(
    root: Path,
    relative: PurePosixPath,
    *,
    max_bytes: int,
    expected_size: int | None = None,
    expected_sha256: str | None = None,
) -> tuple[dict[str, Any], int, str]:
    raw, size, digest = _read_nofollow(
        root,
        relative,
        max_bytes=max_bytes,
        expected_size=expected_size,
        expected_sha256=expected_sha256,
    )
    try:
        value = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ArtifactError(f"cannot read JSON object {root / relative}: {error}") from error
    if not isinstance(value, dict):
        raise ArtifactError(f"expected JSON object: {root / relative}")
    return value, size, digest


def _write_nofollow(root: Path, relative: PurePosixPath, data: bytes) -> None:
    """Write only into a private extracted tree, rejecting path races."""
    parent = root
    for component in relative.parts[:-1]:
        if component in {"", ".", ".."}:
            raise ArtifactError(f"unsafe extracted path: {relative}")
        parent = parent / component
        if parent.exists() or parent.is_symlink():
            info = os.lstat(parent)
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
                raise ArtifactError(f"extracted path component is not a directory: {relative}")
        else:
            parent.mkdir(mode=0o700)
    destination = root.joinpath(*relative.parts)
    if destination.exists() or destination.is_symlink():
        raise ArtifactError(f"duplicate extracted path: {relative}")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(destination, flags, 0o400)
    except OSError as error:
        raise ArtifactError(f"cannot create extracted file {relative}: {error}") from error
    try:
        offset = 0
        while offset < len(data):
            written = os.write(descriptor, data[offset : offset + 1024 * 1024])
            if written <= 0:
                raise ArtifactError(f"cannot write extracted file {relative}")
            offset += written
        os.fsync(descriptor)
    except OSError as error:
        raise ArtifactError(f"cannot write extracted file {relative}: {error}") from error
    finally:
        os.close(descriptor)


def _seal_tree(root: Path) -> None:
    """Make the verified tree read-only before handing paths to dcmview."""
    for directory, directories, files in os.walk(root, topdown=False, followlinks=False):
        for name in files:
            path = Path(directory) / name
            info = os.lstat(path)
            if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
                raise ArtifactError(f"extracted tree contains a non-regular file: {path}")
            os.chmod(path, 0o400)
        for name in directories:
            path = Path(directory) / name
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
                raise ArtifactError(f"extracted tree contains a non-directory: {path}")
            # Keep private directory write permission so the temporary tree
            # can be removed cleanly after the viewer campaign.  Payload files
            # themselves are read-only and every consumer read is no-follow.
            os.chmod(path, 0o700)
    os.chmod(root, 0o700)


def _safe_archive_member(value: str) -> tuple[PurePosixPath, bool]:
    if not isinstance(value, str) or not value or "\x00" in value or "\\" in value or "//" in value:
        raise ArtifactError(f"archive member has an unsafe path: {value!r}")
    is_directory = value.endswith("/")
    normalized = value[:-1] if is_directory else value
    path = PurePosixPath(normalized)
    if (
        path.is_absolute()
        or not path.parts
        or any(part in {"", ".", ".."} for part in path.parts)
        or not path.parts[0] == "corpus"
    ):
        raise ArtifactError(f"archive member has an unsafe path: {value!r}")
    return path, is_directory


def _safe_zip_member(value: str) -> PurePosixPath:
    if not isinstance(value, str) or not value or "\x00" in value or "\\" in value or "//" in value:
        raise ArtifactError(f"downloaded artifact member has an unsafe path: {value!r}")
    path = PurePosixPath(value[:-1] if value.endswith("/") else value)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        raise ArtifactError(f"downloaded artifact member has an unsafe path: {value!r}")
    return path


def extract_github_artifact(zip_path: Path, destination: Path) -> Path:
    """Safely extract the ZIP returned by the immutable Actions artifact API."""
    _reject_symlink_components(zip_path)
    try:
        zip_info = os.lstat(zip_path)
    except OSError as error:
        raise ArtifactError(f"cannot inspect downloaded artifact {zip_path}: {error}") from error
    if not stat.S_ISREG(zip_info.st_mode) or zip_path.is_symlink():
        raise ArtifactError(f"downloaded artifact is not a regular file: {zip_path}")
    if zip_info.st_size > MAX_ARCHIVE_BYTES:
        raise ArtifactError("downloaded artifact ZIP exceeds the bounded size")
    _reject_symlink_components(destination.parent)
    if destination.exists() or destination.is_symlink():
        raise ArtifactError(f"artifact extraction destination already exists: {destination}")
    destination.mkdir(mode=0o700, parents=True)
    seen: set[str] = set()
    total = 0
    descriptor = _open_nofollow(zip_path.parent, PurePosixPath(zip_path.name))
    zip_before = os.fstat(descriptor)
    try:
        with os.fdopen(descriptor, "rb", closefd=True) as stream:
            with zipfile.ZipFile(stream, "r") as archive:
                infos = archive.infolist()
                if len(infos) > MAX_ARCHIVE_FILES + 32:
                    raise ArtifactError("downloaded artifact contains too many entries")
                for member in infos:
                    path = _safe_zip_member(member.filename)
                    key = str(path)
                    if key in seen:
                        raise ArtifactError(f"downloaded artifact contains a duplicate path: {key}")
                    seen.add(key)
                    mode = (member.external_attr >> 16) & 0o170000
                    is_directory = member.filename.endswith("/")
                    if mode not in {0, stat.S_IFREG, stat.S_IFDIR}:
                        raise ArtifactError(f"downloaded artifact contains a special entry: {key}")
                    if is_directory or mode == stat.S_IFDIR:
                        if not member.filename.endswith("/"):
                            raise ArtifactError(f"downloaded artifact directory lacks a trailing slash: {key}")
                        target = destination.joinpath(*path.parts)
                        if target.exists() or target.is_symlink():
                            raise ArtifactError(f"downloaded artifact contains a duplicate path: {key}")
                        target.mkdir(mode=0o700, parents=True)
                        continue
                    if member.file_size < 0 or member.file_size > MAX_ARCHIVE_FILE_BYTES:
                        raise ArtifactError(f"downloaded artifact entry exceeds its bounded size: {key}")
                    total += member.file_size
                    if total > MAX_ARCHIVE_BYTES:
                        raise ArtifactError("downloaded artifact exceeds the bounded total size")
                    try:
                        payload = archive.read(member)
                    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
                        raise ArtifactError(f"cannot read downloaded artifact entry {key}: {error}") from error
                    if len(payload) != member.file_size:
                        raise ArtifactError(f"downloaded artifact entry changed while reading: {key}")
                    _write_nofollow(destination, path, payload)
        zip_after = os.stat(zip_path, follow_symlinks=False)
        if _fingerprint(zip_before) != _fingerprint(zip_after):
            raise ArtifactError("downloaded artifact ZIP changed while reading")
    except (OSError, zipfile.BadZipFile) as error:
        raise ArtifactError(f"cannot read downloaded artifact ZIP {zip_path}: {error}") from error
    _seal_tree(destination)
    return destination


def _extract_deterministic_archive(
    archive_bytes: bytes,
    extraction_root: Path,
) -> tuple[set[str], int]:
    """Extract only regular ``corpus/`` members into a fresh closed tree."""
    extraction_root.mkdir(mode=0o700, parents=True)
    seen: set[str] = set()
    file_paths: set[str] = set()
    total = 0
    try:
        with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:gz") as archive:
            members = archive.getmembers()
            if len(members) > MAX_ARCHIVE_FILES + 64:
                raise ArtifactError("smoke archive contains too many members")
            for member in members:
                path, named_directory = _safe_archive_member(member.name)
                is_directory = named_directory or member.isdir()
                key = str(path)
                if key in seen:
                    raise ArtifactError(f"smoke archive contains a duplicate path: {key}")
                seen.add(key)
                if is_directory:
                    if not member.isdir():
                        raise ArtifactError(f"smoke archive directory entry is not a directory: {key}")
                    target = extraction_root.joinpath(*path.parts)
                    if target.exists() or target.is_symlink():
                        raise ArtifactError(f"smoke archive contains a duplicate path: {key}")
                    target.mkdir(mode=0o700, parents=True)
                    continue
                if not member.isreg() or member.issym() or member.islnk() or member.isdev():
                    raise ArtifactError(f"smoke archive contains a non-regular entry: {key}")
                if member.size < 0 or member.size > MAX_ARCHIVE_FILE_BYTES:
                    raise ArtifactError(f"smoke archive entry exceeds its bounded size: {key}")
                total += member.size
                if total > MAX_ARCHIVE_BYTES:
                    raise ArtifactError("smoke archive payload exceeds the bounded total size")
                stream = archive.extractfile(member)
                if stream is None:
                    raise ArtifactError(f"smoke archive member cannot be read: {key}")
                payload = stream.read(member.size + 1)
                if len(payload) != member.size:
                    raise ArtifactError(f"smoke archive member changed while reading: {key}")
                _write_nofollow(extraction_root, path, payload)
                file_paths.add(key)
    except (OSError, EOFError, tarfile.TarError) as error:
        raise ArtifactError(f"cannot read deterministic smoke archive: {error}") from error
    if "corpus/manifest.json" not in file_paths:
        raise ArtifactError("smoke archive must contain corpus/manifest.json")
    return file_paths, total


def _walk_closed_tree(root: Path) -> set[str]:
    if not root.is_dir() or root.is_symlink():
        raise ArtifactError(f"extracted corpus root is not a directory: {root}")
    paths: set[str] = set()
    for directory, directories, files in os.walk(root, topdown=True, followlinks=False):
        for name in directories + files:
            path = Path(directory) / name
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                raise ArtifactError(f"extracted corpus contains a symlink: {path}")
            if not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)):
                raise ArtifactError(f"extracted corpus contains a special entry: {path}")
            relative = path.relative_to(root)
            paths.add(str(PurePosixPath(*relative.parts)))
    return paths


def _expected_pin(value: Any, field: str) -> Any:
    if value is None:
        raise ArtifactError(f"required immutable pin is missing: {field}")
    return value


def _verify_index(
    index: dict[str, Any],
    *,
    archive_sha256: str,
    archive_size: int,
    expected_generator_revision: str | None,
    expected_generator_artifact_sha256: str | None,
    expected_generator_artifact_size_bytes: int | None,
    expected_target: str | None,
    expected_toolchain: str | None,
    expected_generator_features: tuple[str, ...] | None,
    expected_runtime_identities_sha256: str | None,
    expected_definition_manifest_sha256: str | None,
    expected_corpus_definition_sha256: str | None,
    expected_manifest_sha256: str | None,
    expected_manifest_size_bytes: int | None,
    expected_profile: str | None,
    expected_seed: int | None,
    expected_binding_id: str | None,
    expected_archive_sha256: str | None,
    expected_archive_size_bytes: int | None,
    expected_generator_version: str | None,
    required_pins: bool,
) -> dict[str, Any]:
    expected_fields = {
        "artifact_descriptor_schema_version",
        "artifact_name",
        "binding_id",
        "binding_sha256",
        "archive_path",
        "archive_sha256",
        "archive_size_bytes",
        "archive_kind",
        "source_revision",
        "generator_artifact_sha256",
        "generator_artifact_size_bytes",
        "target",
        "rust_toolchain",
        "enabled_features",
        "runtime_identities",
        "definition_manifest_sha256",
        "corpus_definition_sha256",
        "generated_manifest_sha256",
        "generated_manifest_size_bytes",
        "profile",
        "seed",
        "binding",
        "payload_root",
        "manifest_path",
        "evidence",
        "retrieval",
    }
    extra_fields = sorted(set(index) - expected_fields)
    if extra_fields:
        raise ArtifactError(f"artifact-index contains undeclared fields: {extra_fields}")
    if index.get("artifact_descriptor_schema_version") != "1.0.0":
        raise ArtifactError("unsupported artifact-index schema")
    for field in (
        "artifact_name",
        "archive_path",
        "archive_kind",
        "source_revision",
        "target",
        "rust_toolchain",
        "profile",
        "payload_root",
        "manifest_path",
    ):
        if not isinstance(index.get(field), str) or not index[field]:
            raise ArtifactError(f"artifact-index.{field} is required")
    if index["archive_path"] != PRODUCER_ARCHIVE or index["archive_kind"] != "deterministic_tar_gz":
        raise ArtifactError("artifact-index does not describe the producer deterministic smoke archive")
    if index["payload_root"] != "corpus" or index["manifest_path"] != "corpus/manifest.json":
        raise ArtifactError("artifact-index payload root or manifest path is not producer-compatible")
    source_revision = index["source_revision"]
    if SOURCE_REVISION_PATTERN.fullmatch(source_revision) is None:
        raise ArtifactError("artifact-index.source_revision is not a commit SHA")
    index_archive_sha = _require_sha256(index.get("archive_sha256"), "artifact-index.archive_sha256")
    if index_archive_sha != archive_sha256:
        raise ArtifactError(
            f"archive digest mismatch: index declares {index_archive_sha}, observed {archive_sha256}"
        )
    archive_size_declared = index.get("archive_size_bytes")
    if not isinstance(archive_size_declared, int) or isinstance(archive_size_declared, bool) or archive_size_declared < 0:
        raise ArtifactError("artifact-index.archive_size_bytes must be a non-negative integer")
    if archive_size_declared != archive_size:
        raise ArtifactError(
            f"archive size mismatch: index declares {archive_size_declared}, observed {archive_size}"
        )

    artifact_sha = _require_sha256(index.get("generator_artifact_sha256"), "artifact-index.generator_artifact_sha256")
    artifact_size = index.get("generator_artifact_size_bytes")
    if not isinstance(artifact_size, int) or isinstance(artifact_size, bool) or artifact_size < 0:
        raise ArtifactError("artifact-index.generator_artifact_size_bytes must be a non-negative integer")
    features = index.get("enabled_features")
    if not isinstance(features, list) or not all(isinstance(feature, str) and feature for feature in features):
        raise ArtifactError("artifact-index.enabled_features must be a string list")
    if len(set(features)) != len(features):
        raise ArtifactError("artifact-index feature identity is inconsistent")
    runtime = index.get("runtime_identities")
    if not isinstance(runtime, list):
        raise ArtifactError("artifact-index.runtime_identities must be a list")
    runtime_sha = _canonical_sha256(runtime)
    definition_manifest_sha = _require_sha256(
        index.get("definition_manifest_sha256"), "artifact-index.definition_manifest_sha256"
    )
    corpus_definition_sha = _require_sha256(
        index.get("corpus_definition_sha256"), "artifact-index.corpus_definition_sha256"
    )
    manifest_sha = _require_sha256(
        index.get("generated_manifest_sha256"), "artifact-index.generated_manifest_sha256"
    )
    manifest_size = index.get("generated_manifest_size_bytes")
    if not isinstance(manifest_size, int) or isinstance(manifest_size, bool) or manifest_size < 0:
        raise ArtifactError("artifact-index.generated_manifest_size_bytes must be a non-negative integer")
    seed = index.get("seed")
    if not isinstance(seed, int) or isinstance(seed, bool) or seed < 0:
        raise ArtifactError("artifact-index.seed must be a non-negative integer")
    binding_id = _require_sha256(index.get("binding_id"), "artifact-index.binding_id")
    binding_sha = _require_sha256(index.get("binding_sha256"), "artifact-index.binding_sha256")
    if binding_id != binding_sha:
        raise ArtifactError("artifact-index binding_id and binding_sha256 differ")
    expected_artifact_name = (
        f"dcmview-smoke-s{source_revision}-a{artifact_sha}-d"
        f"{definition_manifest_sha}-b{binding_id[:32]}"
    )
    if index.get("artifact_name") != expected_artifact_name:
        raise ArtifactError("artifact-index artifact_name does not match its immutable binding")
    if index.get("profile") != DEFAULT_PROFILE:
        raise ArtifactError("artifact-index.profile must be smoke")
    binding = _mapping(index.get("binding"), "artifact-index.binding")
    if _canonical_sha256(binding) != binding_id:
        raise ArtifactError("artifact-index binding_id does not match the producer binding")
    if binding.get("archive_sha256") != archive_sha256:
        raise ArtifactError("artifact-index binding archive digest differs from the index")
    generator_binding = _mapping(binding.get("generator"), "artifact-index.binding.generator")
    corpus_binding = _mapping(binding.get("corpus"), "artifact-index.binding.corpus")
    run_binding = _mapping(binding.get("run"), "artifact-index.binding.run")
    runtime_binding = _mapping(binding.get("runtime"), "artifact-index.binding.runtime")
    payload_binding = _mapping(binding.get("payload"), "artifact-index.binding.payload")
    generator_artifact = _mapping(
        generator_binding.get("artifact"), "artifact-index.binding.generator.artifact"
    )
    if generator_binding.get("source_revision") != source_revision:
        raise ArtifactError("artifact-index generator source revision is inconsistent")
    if generator_artifact.get("sha256") != artifact_sha or generator_artifact.get("size_bytes") != artifact_size:
        raise ArtifactError("artifact-index generator artifact identity is inconsistent")
    if generator_binding.get("target") != index["target"] or generator_binding.get("rust_toolchain") != index["rust_toolchain"]:
        raise ArtifactError("artifact-index generator target/toolchain identity is inconsistent")
    if generator_binding.get("enabled_features") != features:
        raise ArtifactError("artifact-index binding generator features differ from index")
    if binding.get("features") != features:
        raise ArtifactError("artifact-index binding features differ from index")
    if corpus_binding.get("definition_manifest_sha256") != definition_manifest_sha or corpus_binding.get("corpus_definition_sha256") != corpus_definition_sha:
        raise ArtifactError("artifact-index definition identity is inconsistent")
    if corpus_binding.get("generated_manifest_sha256") != manifest_sha or corpus_binding.get("generated_manifest_size_bytes") != manifest_size:
        raise ArtifactError("artifact-index generated manifest identity is inconsistent")
    if run_binding.get("profile") != index["profile"] or run_binding.get("seed") != seed or run_binding.get("include_stress") is not False:
        raise ArtifactError("artifact-index run identity is inconsistent")
    if runtime_binding.get("external_runtime") != runtime or runtime_sha != _canonical_sha256(runtime_binding.get("external_runtime")):
        raise ArtifactError("artifact-index runtime identity is inconsistent")
    file_entries = payload_binding.get("files")
    file_count = payload_binding.get("file_count")
    total_size = payload_binding.get("total_size_bytes")
    if not isinstance(file_entries, list) or not isinstance(file_count, int) or file_count != len(file_entries):
        raise ArtifactError("artifact-index payload file count is inconsistent")
    if not isinstance(total_size, int) or total_size != sum(
        entry.get("size_bytes", -1) for entry in file_entries if isinstance(entry, dict)
    ):
        raise ArtifactError("artifact-index payload total size is inconsistent")
    expected_payload_paths: set[str] = set()
    for entry in file_entries:
        if not isinstance(entry, dict):
            raise ArtifactError("artifact-index payload file entry is not an object")
        relative = _relative_path(entry.get("path"), "artifact-index payload path")
        key = str(PurePosixPath("corpus", *PurePosixPath(relative).parts))
        if key in expected_payload_paths:
            raise ArtifactError(f"artifact-index contains a duplicate payload path: {key}")
        expected_payload_paths.add(key)
        _require_sha256(entry.get("sha256"), f"artifact-index payload {key} hash")
        size = entry.get("size_bytes")
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise ArtifactError(f"artifact-index payload {key} size is invalid")
    expected_pairs = (
        (expected_generator_revision, source_revision, "generator revision"),
        (expected_generator_artifact_sha256, artifact_sha, "generator artifact SHA-256"),
        (expected_target, index["target"], "generator target"),
        (expected_toolchain, index["rust_toolchain"], "generator toolchain"),
        (expected_definition_manifest_sha256, definition_manifest_sha, "definition manifest SHA-256"),
        (expected_corpus_definition_sha256, corpus_definition_sha, "corpus definition SHA-256"),
        (expected_manifest_sha256, manifest_sha, "generated manifest SHA-256"),
        (expected_profile, index["profile"], "profile"),
        (expected_binding_id, binding_id, "binding ID"),
        (expected_archive_sha256, archive_sha256, "archive SHA-256"),
    )
    if required_pins:
        _expected_pin(expected_generator_revision, "generator revision")
        _expected_pin(expected_generator_artifact_sha256, "generator artifact SHA-256")
        _expected_pin(expected_generator_artifact_size_bytes, "generator artifact size")
        _expected_pin(expected_target, "generator target")
        _expected_pin(expected_toolchain, "generator toolchain")
        _expected_pin(expected_generator_features, "generator features")
        _expected_pin(expected_runtime_identities_sha256, "runtime identities")
        _expected_pin(expected_definition_manifest_sha256, "definition manifest SHA-256")
        _expected_pin(expected_corpus_definition_sha256, "corpus definition SHA-256")
        _expected_pin(expected_manifest_sha256, "generated manifest SHA-256")
        _expected_pin(expected_manifest_size_bytes, "generated manifest size")
        _expected_pin(expected_profile, "profile")
        _expected_pin(expected_seed, "seed")
        _expected_pin(expected_binding_id, "binding ID")
        _expected_pin(expected_archive_sha256, "archive SHA-256")
        _expected_pin(expected_archive_size_bytes, "archive size")
        _expected_pin(expected_generator_version, "generator version")
    for expected, observed, label in expected_pairs:
        if expected is not None and expected != observed:
            raise ArtifactError(f"{label} mismatch: expected {expected!r}, observed {observed!r}")
    if expected_generator_artifact_size_bytes is not None and expected_generator_artifact_size_bytes != artifact_size:
        raise ArtifactError("generator artifact size mismatch")
    if expected_generator_features is not None and tuple(features) != tuple(expected_generator_features):
        raise ArtifactError(f"generator features mismatch: expected {list(expected_generator_features)!r}, observed {features!r}")
    if expected_runtime_identities_sha256 is not None and runtime_sha != expected_runtime_identities_sha256:
        raise ArtifactError("runtime identities mismatch")
    if expected_manifest_size_bytes is not None and expected_manifest_size_bytes != manifest_size:
        raise ArtifactError("generated manifest size mismatch")
    if expected_seed is not None and expected_seed != seed:
        raise ArtifactError("seed mismatch")
    if expected_archive_size_bytes is not None and expected_archive_size_bytes != archive_size:
        raise ArtifactError("archive size pin mismatch")
    if expected_generator_version is not None:
        product = _mapping(generator_binding.get("product"), "artifact-index.binding.generator.product")
        if product.get("version") != expected_generator_version:
            raise ArtifactError("generator version mismatch")
    retrieval = _mapping(index.get("retrieval"), "artifact-index.retrieval")
    if retrieval.get("repository") != PRODUCER_REPOSITORY or retrieval.get("workflow") != PRODUCER_WORKFLOW:
        raise ArtifactError("artifact-index retrieval origin is not the trusted producer workflow")
    for field in ("run_id", "artifact_id", "artifact_digest"):
        if retrieval.get(field) is not None:
            raise ArtifactError("artifact-index retrieval fields must remain unset until upload")
    return {
        "source_revision": source_revision,
        "generator_artifact_sha256": artifact_sha,
        "generator_artifact_size_bytes": artifact_size,
        "target": index["target"],
        "toolchain": index["rust_toolchain"],
        "features": list(features),
        "runtime_identities": runtime,
        "runtime_identities_sha256": runtime_sha,
        "definition_manifest_sha256": definition_manifest_sha,
        "corpus_definition_sha256": corpus_definition_sha,
        "generated_manifest_sha256": manifest_sha,
        "generated_manifest_size_bytes": manifest_size,
        "profile": index["profile"],
        "seed": seed,
        "binding_id": binding_id,
        "archive_sha256": archive_sha256,
        "archive_size_bytes": archive_size,
        "payload": payload_binding,
        "index": index,
    }


def _verify_selection_identity(
    manifest: dict[str, Any],
    *,
    profile: str,
    expected_seed: int | None,
    expected_manifest_sha256: str | None,
    expected_corpus_definition_sha256: str | None,
    expected_generator_version: str | None,
    expected_generator_features: tuple[str, ...] | None,
) -> tuple[int, dict[str, Any]]:
    if manifest.get("manifest_schema_version") != EXTERNAL_MANIFEST_SCHEMA_VERSION:
        raise ArtifactError(
            "unsupported external-corpus manifest schema: "
            f"{manifest.get('manifest_schema_version')!r}"
        )

    run = _mapping(manifest.get("run"), "manifest.run")
    if run.get("kind") != EXTERNAL_MANIFEST_KIND:
        raise ArtifactError("manifest.run.kind must be external_corpus")
    if run.get("profile") != profile:
        raise ArtifactError(
            f"manifest profile mismatch: expected {profile!r}, observed {run.get('profile')!r}"
        )
    if run.get("include_stress") is not False:
        raise ArtifactError("the viewer smoke consumer requires include_stress=false")
    selector = _mapping(run.get("selector"), "manifest.run.selector")
    if selector != {"kind": "profile"}:
        raise ArtifactError("the viewer smoke consumer requires selector.kind=profile")
    seed = run.get("seed")
    if not isinstance(seed, int) or isinstance(seed, bool) or seed < 0:
        raise ArtifactError("manifest.run.seed must be a non-negative integer")
    if expected_seed is not None and seed != expected_seed:
        raise ArtifactError(
            f"manifest seed mismatch: expected {expected_seed}, observed {seed}"
        )

    generator = _mapping(manifest.get("generator"), "manifest.generator")
    if generator.get("name") != "synth-dicom-gen":
        raise ArtifactError("manifest.generator.name must be synth-dicom-gen")
    generator_version = generator.get("version")
    if not isinstance(generator_version, str) or not generator_version:
        raise ArtifactError("manifest.generator.version is required")
    if expected_generator_version is not None and generator_version != expected_generator_version:
        raise ArtifactError(
            "generator version mismatch: "
            f"expected {expected_generator_version!r}, observed {generator_version!r}"
        )
    generator_features = generator.get("feature_flags")
    if not isinstance(generator_features, list) or not all(
        isinstance(feature, str) and feature for feature in generator_features
    ) or len(set(generator_features)) != len(generator_features):
        raise ArtifactError("manifest.generator.feature_flags must be a unique string list")

    projection = _mapping(manifest.get("identity_projection"), "manifest.identity_projection")
    if projection.get("identity_projection_schema_version") != "1.0.0":
        raise ArtifactError("unsupported identity projection schema")
    if projection.get("projection_state") != "projected":
        raise ArtifactError("external artifact identity projection must be projected")
    execution = _mapping(projection.get("execution"), "manifest.identity_projection.execution")
    if execution.get("product_name") != "synth-dicom-gen":
        raise ArtifactError("identity projection execution product_name must be synth-dicom-gen")
    enabled_features = execution.get("enabled_features")
    if enabled_features != generator_features:
        raise ArtifactError("generator feature flags differ from execution identity")
    toolchain = _mapping(projection.get("toolchain"), "manifest.identity_projection.toolchain")
    if toolchain.get("enabled_features") != generator_features:
        raise ArtifactError("generator feature flags differ from toolchain identity")
    _require_sha256(generator.get("cargo_lock_sha256"), "manifest.generator.cargo_lock_sha256")
    for field, value in (
        ("execution.execution_sha256", execution.get("execution_sha256")),
        ("toolchain.toolchain_sha256", toolchain.get("toolchain_sha256")),
        ("engine.engine_sha256", _mapping(projection.get("engine"), "manifest.identity_projection.engine").get("engine_sha256")),
        ("schema_set.schema_set_sha256", _mapping(projection.get("schema_set"), "manifest.identity_projection.schema_set").get("schema_set_sha256")),
        ("template_catalog.template_catalog_sha256", _mapping(projection.get("template_catalog"), "manifest.identity_projection.template_catalog").get("template_catalog_sha256")),
        ("provider_catalog.provider_catalog_sha256", _mapping(projection.get("provider_catalog"), "manifest.identity_projection.provider_catalog").get("provider_catalog_sha256")),
        ("standards.standards_lock_sha256", _mapping(projection.get("standards"), "manifest.identity_projection.standards").get("standards_lock_sha256")),
    ):
        _require_sha256(value, f"manifest.identity_projection.{field}")
    if expected_generator_features is not None and tuple(generator_features) != expected_generator_features:
        raise ArtifactError(
            "generator feature mismatch: "
            f"expected {list(expected_generator_features)!r}, observed {generator_features!r}"
        )

    corpus_definition = _mapping(
        projection.get("corpus_definition"),
        "manifest.identity_projection.corpus_definition",
    )
    if corpus_definition.get("state") != "verified_bundle":
        raise ArtifactError("external artifact corpus_definition must be verified_bundle")
    definition_identity = _mapping(
        corpus_definition.get("identity"),
        "manifest.identity_projection.corpus_definition.identity",
    )
    _require_sha256(
        definition_identity.get("corpus_definition_sha256"),
        "corpus_definition.identity.corpus_definition_sha256",
    )
    _require_sha256(
        definition_identity.get("manifest_sha256"),
        "corpus_definition.identity.manifest_sha256",
    )
    if expected_corpus_definition_sha256 is not None and definition_identity["corpus_definition_sha256"] != expected_corpus_definition_sha256:
        raise ArtifactError(
            "corpus definition mismatch: "
            f"expected {expected_corpus_definition_sha256}, observed {definition_identity['corpus_definition_sha256']}"
        )

    if expected_manifest_sha256 is not None:
        _require_sha256(expected_manifest_sha256, "expected manifest SHA-256")
    return seed, {
        "manifest_schema_version": EXTERNAL_MANIFEST_SCHEMA_VERSION,
        "profile": profile,
        "seed": seed,
        "run": {
            "kind": run["kind"],
            "profile": run["profile"],
            "include_stress": run["include_stress"],
            "selector": selector,
        },
        "generator": {
            "name": generator["name"],
            "version": generator_version,
            "feature_flags": list(generator_features),
            "cargo_lock_sha256": generator["cargo_lock_sha256"],
        },
        "generator_version": generator_version,
        "generator_git_sha": generator.get("git_sha"),
        "generator_features": list(generator_features),
        "corpus_definition": definition_identity,
        "identity_projection": projection,
    }


def verify_external_artifact(
    root: Path,
    *,
    profile: str = DEFAULT_PROFILE,
    expected_seed: int | None = DEFAULT_SEED,
    expected_manifest_sha256: str | None = None,
    expected_corpus_definition_sha256: str | None = None,
    expected_generator_version: str | None = None,
    expected_generator_features: tuple[str, ...] | None = None,
    expected_generator_revision: str | None = None,
    expected_generator_artifact_sha256: str | None = None,
    expected_generator_artifact_size_bytes: int | None = None,
    expected_target: str | None = None,
    expected_toolchain: str | None = None,
    expected_runtime_identities_sha256: str | None = None,
    expected_definition_manifest_sha256: str | None = None,
    expected_manifest_size_bytes: int | None = None,
    expected_profile: str | None = DEFAULT_PROFILE,
    expected_binding_id: str | None = None,
    expected_archive_sha256: str | None = None,
    expected_archive_size_bytes: int | None = None,
    extraction_root: Path | None = None,
    required_pins: bool = False,
) -> dict[str, Any]:
    """Verify one exact producer container without another repository checkout.

    ``root`` is the uploaded container containing ``smoke.tar.gz`` and its
    sibling ``artifact-index.json``.  The tarball is copied into a private,
    read-only extracted tree before any path is returned to the viewer runner.
    """
    if profile != DEFAULT_PROFILE:
        raise ArtifactError(f"viewer artifact consumer only supports profile {DEFAULT_PROFILE!r}")
    resolved_container = Path(os.path.abspath(root))
    _reject_symlink_components(resolved_container)
    try:
        info = os.lstat(resolved_container)
    except OSError as error:
        raise ArtifactError(f"cannot inspect artifact container {resolved_container}: {error}") from error
    if not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode):
        raise ArtifactError(f"artifact container is not a directory: {resolved_container}")
    try:
        names = sorted(os.listdir(resolved_container))
    except OSError as error:
        raise ArtifactError(f"cannot list artifact container {resolved_container}: {error}") from error
    for name in names:
        if name in {".", ".."} or "/" in name or "\\" in name or "\x00" in name:
            raise ArtifactError(f"artifact container has an unsafe entry: {name!r}")
        entry_info = os.lstat(resolved_container / name)
        if stat.S_ISLNK(entry_info.st_mode):
            raise ArtifactError(f"artifact container contains a symlink: {name}")
        if not stat.S_ISREG(entry_info.st_mode):
            raise ArtifactError(f"artifact container contains a directory or special entry: {name}")
    index, index_size, index_sha256 = _read_json_nofollow(
        resolved_container,
        PurePosixPath(PRODUCER_INDEX),
        max_bytes=MAX_INDEX_BYTES,
    )
    archive_bytes, archive_size, archive_sha256 = _read_nofollow(
        resolved_container,
        PurePosixPath(PRODUCER_ARCHIVE),
        max_bytes=MAX_ARCHIVE_BYTES,
    )
    index_identity = _verify_index(
        index,
        archive_sha256=archive_sha256,
        archive_size=archive_size,
        expected_generator_revision=expected_generator_revision,
        expected_generator_artifact_sha256=expected_generator_artifact_sha256,
        expected_generator_artifact_size_bytes=expected_generator_artifact_size_bytes,
        expected_target=expected_target,
        expected_toolchain=expected_toolchain,
        expected_generator_features=expected_generator_features,
        expected_runtime_identities_sha256=expected_runtime_identities_sha256,
        expected_definition_manifest_sha256=expected_definition_manifest_sha256,
        expected_corpus_definition_sha256=expected_corpus_definition_sha256,
        expected_manifest_sha256=expected_manifest_sha256,
        expected_manifest_size_bytes=expected_manifest_size_bytes,
        expected_profile=expected_profile,
        expected_seed=expected_seed,
        expected_binding_id=expected_binding_id,
        expected_archive_sha256=expected_archive_sha256,
        expected_archive_size_bytes=expected_archive_size_bytes,
        expected_generator_version=expected_generator_version,
        required_pins=required_pins,
    )
    evidence = index.get("evidence")
    if not isinstance(evidence, list):
        raise ArtifactError("artifact-index.evidence must be a list")
    allowed = {PRODUCER_ARCHIVE, PRODUCER_INDEX}
    for row in evidence:
        if not isinstance(row, dict):
            raise ArtifactError("artifact-index evidence entry is not an object")
        relative = _relative_path(row.get("path"), "artifact-index evidence path")
        key = str(PurePosixPath(*relative.parts))
        if key in allowed:
            raise ArtifactError(f"artifact-index evidence collides with a required entry: {key}")
        if key in {str(PurePosixPath(*_relative_path(item, "artifact-index evidence path").parts)) for item in allowed}:
            raise ArtifactError(f"artifact-index evidence path is duplicated: {key}")
        _require_sha256(row.get("sha256"), f"artifact-index evidence {key} hash")
        size = row.get("size_bytes")
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise ArtifactError(f"artifact-index evidence {key} size is invalid")
        if key in allowed:
            raise ArtifactError(f"artifact-index evidence path is reserved: {key}")
        allowed.add(key)
        _read_nofollow(
            resolved_container,
            PurePosixPath(key),
            max_bytes=MAX_ARCHIVE_FILE_BYTES,
            expected_size=size,
            expected_sha256=row["sha256"],
        )
    actual_names = set(names)
    if actual_names != allowed:
        raise ArtifactError(
            f"artifact container has undeclared or missing entries: expected {sorted(allowed)!r}, observed {sorted(actual_names)!r}"
        )

    temporary_directory: tempfile.TemporaryDirectory[str] | None = None
    if extraction_root is None:
        temporary_directory = tempfile.TemporaryDirectory(prefix="dcmview-smoke-artifact-")
        extraction_root = Path(temporary_directory.name) / "tree"
    else:
        extraction_root = Path(os.path.abspath(extraction_root))
        _reject_symlink_components(extraction_root.parent)
        if extraction_root.exists() or extraction_root.is_symlink():
            raise ArtifactError(f"artifact extraction destination already exists: {extraction_root}")
    file_paths, extracted_total = _extract_deterministic_archive(archive_bytes, extraction_root)
    if extracted_total != index_identity["payload"]["total_size_bytes"]:
        raise ArtifactError("smoke archive total payload size differs from artifact-index")
    closed_paths = _walk_closed_tree(extraction_root)
    expected_tree_paths = set(file_paths)
    for file_path in file_paths:
        parts = PurePosixPath(file_path).parts
        expected_tree_paths.update(
            str(PurePosixPath(*parts[:index])) for index in range(1, len(parts))
        )
    if closed_paths != expected_tree_paths:
        raise ArtifactError(
            f"extracted smoke archive is not a closed tree: expected {sorted(expected_tree_paths)!r}, observed {sorted(closed_paths)!r}"
        )
    payload_entries = index_identity["payload"]["files"]
    index_file_paths: set[str] = set()
    for entry in payload_entries:
        relative = _relative_path(entry.get("path"), "artifact-index payload path")
        key = str(PurePosixPath("corpus", *relative.parts))
        index_file_paths.add(key)
        _read_nofollow(
            extraction_root,
            PurePosixPath(key),
            max_bytes=MAX_ARCHIVE_FILE_BYTES if key != "corpus/manifest.json" else MAX_INDEX_BYTES,
            expected_size=entry["size_bytes"],
            expected_sha256=entry["sha256"],
        )
    if file_paths != index_file_paths:
        raise ArtifactError(
            f"smoke archive payload set differs from artifact-index: expected {sorted(index_file_paths)!r}, observed {sorted(file_paths)!r}"
        )
    manifest, manifest_size, manifest_sha256 = _read_json_nofollow(
        extraction_root,
        PurePosixPath("corpus/manifest.json"),
        max_bytes=MAX_INDEX_BYTES,
        expected_size=index_identity["generated_manifest_size_bytes"],
        expected_sha256=index_identity["generated_manifest_sha256"],
    )
    seed, identity = _verify_selection_identity(
        manifest,
        profile=profile,
        expected_seed=expected_seed,
        expected_manifest_sha256=expected_manifest_sha256,
        expected_corpus_definition_sha256=expected_corpus_definition_sha256,
        expected_generator_version=expected_generator_version,
        expected_generator_features=expected_generator_features,
    )
    if manifest["run"]["profile"] != index_identity["profile"] or seed != index_identity["seed"]:
        raise ArtifactError("manifest run identity differs from artifact-index")
    if identity["generator_features"] != index_identity["features"]:
        raise ArtifactError("manifest generator features differ from artifact-index")
    if identity["generator_version"] != _mapping(index_identity["index"]["binding"]["generator"].get("product"), "artifact-index.binding.generator.product").get("version"):
        raise ArtifactError("manifest generator version differs from artifact-index")
    if identity["corpus_definition"]["manifest_sha256"] != index_identity["definition_manifest_sha256"]:
        raise ArtifactError("manifest definition identity differs from artifact-index")
    corpus_root = extraction_root / "corpus"

    files = manifest.get("files")
    ledger = manifest.get("selection_ledger")
    if not isinstance(files, list) or not files:
        raise ArtifactError("external artifact manifest must contain files")
    if not isinstance(ledger, list) or not ledger:
        raise ArtifactError("external artifact manifest must contain selection_ledger")
    manifest_payload_paths = {
        str(PurePosixPath("corpus", *_relative_path(entry.get("path"), "manifest file path").parts))
        for entry in files
        if isinstance(entry, dict)
    }
    if index_file_paths != manifest_payload_paths | {"corpus/manifest.json"}:
        raise ArtifactError(
            "artifact-index payloads do not exactly match manifest-declared viewer inputs"
        )

    verified_files: list[dict[str, Any]] = []
    file_keys: set[tuple[str, str]] = set()
    path_keys: set[str] = set()
    for index, entry in enumerate(files):
        if not isinstance(entry, dict):
            raise ArtifactError(f"manifest.files[{index}] must be an object")
        relative = _relative_path(entry.get("path"), f"manifest.files[{index}].path")
        case_id = entry.get("case_id")
        if not isinstance(case_id, str) or not case_id:
            raise ArtifactError(f"manifest.files[{index}].case_id is required")
        declared_size = entry.get("size_bytes")
        if not isinstance(declared_size, int) or isinstance(declared_size, bool) or declared_size < 0:
            raise ArtifactError(f"manifest.files[{index}].size_bytes must be a non-negative integer")
        declared_hash = _require_sha256(entry.get("sha256"), f"manifest.files[{index}].sha256")
        selected = corpus_root / relative
        _read_nofollow(
            corpus_root,
            PurePosixPath(*relative.parts),
            max_bytes=MAX_ARCHIVE_FILE_BYTES,
            expected_size=declared_size,
            expected_sha256=declared_hash,
        )
        membership = entry.get("profile_membership")
        if not isinstance(membership, list) or profile not in membership:
            raise ArtifactError(f"manifest file is not a member of {profile}: {entry['path']}")
        key = (case_id, entry["path"])
        if key in file_keys or entry["path"] in path_keys:
            raise ArtifactError(f"duplicate manifest file identity: {case_id}:{entry['path']}")
        file_keys.add(key)
        path_keys.add(entry["path"])
        verified_files.append({**entry, "normalized_path": str(selected)})

    ledger_keys: set[tuple[str, str]] = set()
    for index, row in enumerate(ledger):
        if not isinstance(row, dict):
            raise ArtifactError(f"manifest.selection_ledger[{index}] must be an object")
        case_id = row.get("case_id")
        artifact_paths = row.get("artifact_paths")
        if not isinstance(case_id, str) or not isinstance(artifact_paths, list):
            raise ArtifactError(f"manifest.selection_ledger[{index}] lacks case/artifact identity")
        if row.get("selection") not in {"direct", "dependency"}:
            raise ArtifactError(f"manifest.selection_ledger[{index}] has an invalid selection")
        if row.get("registry_status") != "implemented":
            raise ArtifactError(f"manifest.selection_ledger[{index}] is not implemented")
        if row.get("outcome") != "generated":
            if artifact_paths:
                raise ArtifactError("non-generated selection ledger rows cannot own artifacts")
            continue
        for path in artifact_paths:
            if not isinstance(path, str):
                raise ArtifactError(f"selection ledger path is not a string: {case_id}:{path}")
            _relative_path(path, f"manifest.selection_ledger[{index}].artifact_paths")
            if (case_id, path) in ledger_keys:
                raise ArtifactError(f"duplicate selection ledger artifact: {case_id}:{path}")
            ledger_keys.add((case_id, path))
    if ledger_keys != file_keys:
        raise ArtifactError("selection ledger does not exactly close over generated manifest files")

    if set(file_keys) != ledger_keys:
        raise ArtifactError("selection ledger has an unrecognized generated path")
    _seal_tree(extraction_root)
    result = {
        "root": str(corpus_root),
        "container_root": str(resolved_container),
        "manifest_path": str(corpus_root / "manifest.json"),
        "manifest_sha256": manifest_sha256,
        "manifest_size_bytes": manifest_size,
        "artifact_index_sha256": index_sha256,
        "artifact_index_size_bytes": index_size,
        "archive_sha256": archive_sha256,
        "archive_size_bytes": archive_size,
        "manifest": manifest,
        "files": verified_files,
        "identity": {**identity, "artifact_index": index_identity},
        "seed": seed,
    }
    if temporary_directory is not None:
        result["_temporary_directory"] = temporary_directory
    return result


def _expected_contract(entry: dict[str, Any]) -> dict[str, Any]:
    return {
        key: value
        for key, value in sorted(entry.items())
        if key not in CONTRACT_EXCLUDED_FIELDS
    }


def build_external_worklist(
    root: Path,
    *,
    policy_path: Path = DEFAULT_POLICY,
    expected_seed: int | None = DEFAULT_SEED,
    expected_manifest_sha256: str | None = None,
    expected_corpus_definition_sha256: str | None = None,
    expected_generator_version: str | None = None,
    expected_generator_features: tuple[str, ...] | None = None,
    expected_generator_revision: str | None = None,
    expected_generator_artifact_sha256: str | None = None,
    expected_generator_artifact_size_bytes: int | None = None,
    expected_target: str | None = None,
    expected_toolchain: str | None = None,
    expected_runtime_identities_sha256: str | None = None,
    expected_definition_manifest_sha256: str | None = None,
    expected_manifest_size_bytes: int | None = None,
    expected_profile: str | None = DEFAULT_PROFILE,
    expected_binding_id: str | None = None,
    expected_archive_sha256: str | None = None,
    expected_archive_size_bytes: int | None = None,
    required_pins: bool = False,
) -> dict[str, Any]:
    """Adapt a verified smoke artifact to the existing compatibility runner."""
    verified = verify_external_artifact(
        root,
        expected_seed=expected_seed,
        expected_manifest_sha256=expected_manifest_sha256,
        expected_corpus_definition_sha256=expected_corpus_definition_sha256,
        expected_generator_version=expected_generator_version,
        expected_generator_features=expected_generator_features,
        expected_generator_revision=expected_generator_revision,
        expected_generator_artifact_sha256=expected_generator_artifact_sha256,
        expected_generator_artifact_size_bytes=expected_generator_artifact_size_bytes,
        expected_target=expected_target,
        expected_toolchain=expected_toolchain,
        expected_runtime_identities_sha256=expected_runtime_identities_sha256,
        expected_definition_manifest_sha256=expected_definition_manifest_sha256,
        expected_manifest_size_bytes=expected_manifest_size_bytes,
        expected_profile=expected_profile,
        expected_binding_id=expected_binding_id,
        expected_archive_sha256=expected_archive_sha256,
        expected_archive_size_bytes=expected_archive_size_bytes,
        required_pins=required_pins,
    )
    manifest = verified["manifest"]
    profile = manifest["run"]["profile"]
    try:
        policy = load_policy(policy_path)
    except PolicyError as error:
        raise ArtifactError(str(error)) from error
    policy_profile = "all" if profile in {"smoke", "core", "extended"} else profile
    files: list[dict[str, Any]] = []
    for entry in verified["files"]:
        try:
            rule = resolve(policy, entry, policy_profile)
        except PolicyError as error:
            raise ArtifactError(str(error)) from error
        uids = entry.get("uids") or {}
        sop_uid = uids.get("sop_instance_uid") if isinstance(uids, dict) else None
        if not isinstance(sop_uid, str) or not sop_uid:
            raise ArtifactError(f"valid manifest entry lacks SOP Instance UID: {entry['path']}")
        identity = {
            "profile": profile,
            "manifest_sha256": verified["manifest_sha256"],
            "case_id": entry["case_id"],
            "path": entry["path"],
        }
        row = {
            "kind": "valid",
            "manifest_identity": identity,
            "manifest_identity_sha256": hashlib.sha256(canonical_json(identity)).hexdigest(),
            "case_id": entry["case_id"],
            "path": entry["path"],
            "normalized_path": entry["normalized_path"],
            "sha256": entry["sha256"],
            "contract_sha256": hashlib.sha256(canonical_json(_expected_contract(entry))).hexdigest(),
            "sop_instance_uid": sop_uid,
            "expected_contract": _expected_contract(entry),
            "policy": {
                "rule_id": rule["id"],
                "classification": rule["classification"],
                "required_assertions": applicable_assertions(rule, entry),
                "semantic_context_assertions": rule["semantic_context_assertions"],
                "expected_unsupported": rule["expected_unsupported"],
            },
        }
        files.append(row)
    files.sort(key=lambda row: (row["manifest_identity"]["profile"], row["case_id"], row["path"]))
    manifest_record = {
        "profile": profile,
        "manifest": verified["manifest_path"],
        "sha256": verified["manifest_sha256"],
        "root": verified["root"],
        "physical_files": len(files),
        "logical_cases": len({row["case_id"] for row in files}),
        "qualifications": len(manifest.get("qualifications") or []),
    }
    worklist: dict[str, Any] = {
        "worklist_schema_version": WORKLIST_SCHEMA_VERSION,
        "suite": {"root": None, "commit": None, "source": "external_corpus_manifest"},
        "inputs": {
            "lock": None,
            "lock_sha256": None,
            "policy": str(policy_path.resolve()),
            "policy_sha256": policy_sha256(policy),
            "profiles": [profile],
            "manifests": [manifest_record],
            "artifact_identity": verified["identity"],
        },
        "models": {
            "valid_files": files,
            "legacy_files": [],
            "negative_inputs": [],
            "stress_files": [],
            "stress_scenarios": [],
            "fuzz_qualifications": [],
        },
        "files": files,
        "unavailable": [],
        "summary": {
            "files": len(files),
            "logical_cases": len({row["case_id"] for row in files}),
            "qualifications": len(manifest.get("qualifications") or []),
            "unavailable_selected_profiles": 0,
        },
    }
    worklist["worklist_sha256"] = hashlib.sha256(canonical_json(worklist)).hexdigest()
    if verified.get("_temporary_directory") is not None:
        worklist["_temporary_directory"] = verified["_temporary_directory"]
    return worklist


__all__ = [
    "ArtifactError",
    "DEFAULT_PROFILE",
    "DEFAULT_SEED",
    "EXTERNAL_MANIFEST_SCHEMA_VERSION",
    "build_external_worklist",
    "extract_github_artifact",
    "verify_external_artifact",
]

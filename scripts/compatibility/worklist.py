#!/usr/bin/env python3
"""Shared helpers for viewer-owned compatibility worklists."""

from __future__ import annotations

import hashlib
import json
import math
import os
import re
import stat
import tempfile
from pathlib import Path
from pathlib import PurePosixPath
from typing import Any, NoReturn

try:
    from scripts.compatibility.assertions import CAPABILITY_ASSERTIONS
except ModuleNotFoundError:
    from assertions import CAPABILITY_ASSERTIONS  # type: ignore[no-redef]


WORKLIST_SCHEMA_VERSION = "0.2.0"
CONTRACT_EXCLUDED_FIELDS = {"sha256", "size_bytes"}
ROBUSTNESS_PROFILES = frozenset({"negative", "stress", "fuzz"})

# A robustness worklist is caller-supplied input. Keep its parser bounded even
# when the caller supplies a syntactically valid but intentionally enormous
# document. The cap is large enough for the historical stress contracts while
# still making accidental/unbounded reads impossible.
MAX_WORKLIST_BYTES = 128 * 1024 * 1024
MAX_WORKLIST_ROWS = 4096
MAX_WORKLIST_PATH_BYTES = 4096
MAX_PAYLOAD_BYTES = 512 * 1024 * 1024
MAX_STAGED_PAYLOAD_BYTES = 1024 * 1024 * 1024

_SHA256 = re.compile(r"\A[0-9a-f]{64}\Z")
_TOP_LEVEL_FIELDS = {
    "files",
    "inputs",
    "models",
    "suite",
    "summary",
    "unavailable",
    "worklist_schema_version",
    "worklist_sha256",
}
_SUITE_FIELDS = {"commit", "root"}
_INPUT_FIELDS = {"lock", "lock_sha256", "manifests", "policy", "policy_sha256", "profiles"}
_MANIFEST_FIELDS = {
    "logical_cases",
    "manifest",
    "physical_files",
    "profile",
    "qualifications",
    "root",
    "sha256",
}
_MODEL_FIELDS = {
    "fuzz_qualifications",
    "legacy_files",
    "negative_inputs",
    "stress_files",
    "stress_scenarios",
    "valid_files",
}
_SUMMARY_FIELDS = {"files", "logical_cases", "qualifications", "unavailable_selected_profiles"}
_FILE_FIELDS = {
    "case_id",
    "contract_sha256",
    "expected_contract",
    "kind",
    "manifest_identity",
    "manifest_identity_sha256",
    "normalized_path",
    "path",
    "policy",
    "sha256",
    "size_bytes",
    "sop_instance_uid",
}
_IDENTITY_FIELDS = {"case_id", "manifest_sha256", "path", "profile"}
_QUALIFICATION_FIELDS = {"case_id", "contract", "contract_sha256", "policy", "profile"}
_UNAVAILABLE_FIELDS = {
    "case_id",
    "message",
    "profile",
    "reason_code",
    "recheck_phase",
    "standards_evidence",
    "status",
}
_FILE_POLICY_FIELDS = {
    "classification",
    "expected_unsupported",
    "required_assertions",
    "rule_id",
    "semantic_context_assertions",
}


class CompatibilityError(RuntimeError):
    """A viewer-owned compatibility input is malformed or unverifiable."""


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def canonical_sha256(value: Any) -> str:
    """Return the digest of the one canonical JSON representation we accept."""

    return hashlib.sha256(canonical_json(value)).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _invalid(label: str, detail: str) -> NoReturn:
    raise CompatibilityError(f"invalid worklist {label}: {detail}")


def _unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise CompatibilityError(f"duplicate JSON field {key!r}")
        value[key] = item
    return value


def _reject_constant(value: str) -> NoReturn:
    raise CompatibilityError(f"JSON constant {value!r} is not supported")


def _bounded_integer(value: str) -> int:
    if len(value) > 128:
        raise CompatibilityError("JSON integer exceeds the 128-character limit")
    return int(value)


def _bounded_float(value: str) -> float:
    if len(value) > 128:
        raise CompatibilityError("JSON number exceeds the 128-character limit")
    parsed = float(value)
    if not math.isfinite(parsed):
        raise CompatibilityError("non-finite JSON numbers are not supported")
    return parsed


def load_json(path: Path) -> dict[str, Any]:
    path = Path(path)
    try:
        with path.open("rb") as stream:
            encoded = stream.read(MAX_WORKLIST_BYTES + 1)
    except OSError as error:
        raise CompatibilityError(f"cannot read JSON object {path}: {error}") from error
    if len(encoded) > MAX_WORKLIST_BYTES:
        raise CompatibilityError(
            f"worklist JSON exceeds the {MAX_WORKLIST_BYTES}-byte limit: {path}"
        )
    try:
        value = json.loads(
            encoded.decode("utf-8"),
            object_pairs_hook=_unique_object,
            parse_int=_bounded_integer,
            parse_float=_bounded_float,
            parse_constant=_reject_constant,
        )
    except CompatibilityError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError, MemoryError, ValueError) as error:
        raise CompatibilityError(f"cannot read JSON object {path}: {error}") from error
    if not isinstance(value, dict):
        raise CompatibilityError(f"expected JSON object: {path}")
    return value


def applicable_assertions(rule: dict[str, Any], entry: dict[str, Any]) -> list[str]:
    declared = set(entry.get("expected_capabilities") or [])
    conditional = {
        CAPABILITY_ASSERTIONS[capability]
        for capability in declared
        if capability in CAPABILITY_ASSERTIONS
    }
    selected = list(rule["required_assertions"])
    selected.extend(
        assertion
        for assertion in rule.get("conditional_assertions", [])
        if assertion in conditional
    )
    return list(dict.fromkeys(selected))


def _object(value: Any, label: str, fields: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict):
        _invalid(label, "must be an object")
    missing = fields - set(value)
    extra = set(value) - fields
    if missing:
        _invalid(label, f"missing fields {sorted(missing)!r}")
    if extra:
        _invalid(label, f"unsupported fields {sorted(extra)!r}")
    return value


def _string(value: Any, label: str, *, nonempty: bool = True) -> str:
    if not isinstance(value, str) or (nonempty and not value):
        _invalid(label, "must be a non-empty string" if nonempty else "must be a string")
    return value


def _sha256(value: Any, label: str) -> str:
    value = _string(value, label)
    if _SHA256.fullmatch(value) is None:
        _invalid(label, "must be a lowercase SHA-256 digest")
    return value


def _integer(value: Any, label: str, *, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        _invalid(label, "must be a non-negative integer")
    if maximum is not None and value > maximum:
        _invalid(label, f"exceeds the {maximum} limit")
    return value


def _list(value: Any, label: str, *, maximum: int = MAX_WORKLIST_ROWS) -> list[Any]:
    if not isinstance(value, list):
        _invalid(label, "must be an array")
    if len(value) > maximum:
        _invalid(label, f"contains more than {maximum} entries")
    return value


def _safe_relative_path(value: Any, label: str) -> PurePosixPath:
    value = _string(value, label)
    if len(value.encode("utf-8")) > MAX_WORKLIST_PATH_BYTES:
        _invalid(label, f"exceeds the {MAX_WORKLIST_PATH_BYTES}-byte limit")
    if "\x00" in value or "\\" in value or "//" in value:
        _invalid(label, "contains an unsafe path separator")
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in {"", ".", ".."} for part in path.parts):
        _invalid(label, "must be a confined relative path")
    return path


def _absolute_path(value: Any, label: str) -> Path:
    value = _string(value, label)
    if len(value.encode("utf-8")) > MAX_WORKLIST_PATH_BYTES:
        _invalid(label, f"exceeds the {MAX_WORKLIST_PATH_BYTES}-byte limit")
    if "\x00" in value:
        _invalid(label, "contains a NUL byte")
    path = Path(value)
    if not path.is_absolute():
        _invalid(label, "must be absolute")
    return path


def _regular_directory(path: Path, label: str) -> Path:
    if path.is_symlink():
        _invalid(label, "contains a symlink")
    try:
        resolved = path.resolve(strict=True)
        info = os.lstat(resolved)
    except (OSError, RuntimeError) as error:
        _invalid(label, f"cannot inspect directory {path}: {error}")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        _invalid(label, "must name a regular directory")
    return resolved


def _write_all(descriptor: int, value: bytes) -> None:
    view = memoryview(value)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise OSError("staging write made no progress")
        view = view[written:]


def _read_digest(
    path: Path,
    label: str,
    expected_size: int | None = None,
    *,
    stage_path: Path | None = None,
    source_descriptor: int | None = None,
) -> tuple[int, str]:
    owns_descriptor = source_descriptor is None
    if source_descriptor is None:
        flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        try:
            descriptor = os.open(path, flags)
        except OSError as error:
            _invalid(label, f"cannot open payload: {error}")
    else:
        descriptor = source_descriptor
    stage_descriptor: int | None = None
    stage_sync_error: OSError | None = None
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            _invalid(label, "must name a regular file")
        if before.st_size > MAX_PAYLOAD_BYTES:
            _invalid(label, f"exceeds the {MAX_PAYLOAD_BYTES}-byte payload limit")
        if expected_size is not None and before.st_size != expected_size:
            _invalid(label, f"size mismatch: expected {expected_size}, observed {before.st_size}")
        if stage_path is not None:
            stage_path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            stage_flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0)
            try:
                stage_descriptor = os.open(stage_path, stage_flags, 0o600)
            except OSError as error:
                _invalid(label, f"cannot create verified payload staging file: {error}")
        digest = hashlib.sha256()
        total = 0
        while True:
            chunk = os.read(descriptor, 1024 * 1024)
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_PAYLOAD_BYTES:
                _invalid(label, f"exceeds the {MAX_PAYLOAD_BYTES}-byte payload limit")
            digest.update(chunk)
            if stage_descriptor is not None:
                _write_all(stage_descriptor, chunk)
        after = os.fstat(descriptor)
    except CompatibilityError:
        raise
    except OSError as error:
        _invalid(label, f"cannot read payload: {error}")
    finally:
        if stage_descriptor is not None:
            try:
                os.fsync(stage_descriptor)
            except OSError as error:
                stage_sync_error = error
            os.close(stage_descriptor)
        if owns_descriptor:
            os.close(descriptor)
    if stage_sync_error is not None:
        _invalid(label, f"cannot finalize verified payload staging file: {stage_sync_error}")
    if before.st_dev != after.st_dev or before.st_ino != after.st_ino or before.st_size != after.st_size or total != after.st_size:
        _invalid(label, "changed while being verified")
    if expected_size is not None and total != expected_size:
        _invalid(label, f"size mismatch: expected {expected_size}, observed {total}")
    return total, digest.hexdigest()


def _open_confined_descriptor(root: Path, relative: PurePosixPath, label: str) -> int:
    """Open every path component with no-follow semantics beneath the resolved root."""

    directory_flags = os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        current = os.open(root, directory_flags)
    except OSError as error:
        _invalid(label, f"cannot open payload root: {error}")
    try:
        for component in relative.parts[:-1]:
            child = os.open(component, directory_flags, dir_fd=current)
            os.close(current)
            current = child
        leaf_flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
        descriptor = os.open(relative.parts[-1], leaf_flags, dir_fd=current)
    except (OSError, TypeError) as error:
        os.close(current)
        _invalid(label, f"cannot open confined payload: {error}")
    os.close(current)
    return descriptor


class PayloadStage:
    """Own an ephemeral tree of bytes copied from verified no-follow descriptors."""

    def __init__(self) -> None:
        self._temporary_directory = tempfile.TemporaryDirectory(prefix="dcmview-compat-payload-")
        self.root = Path(self._temporary_directory.name)
        self._closed = False
        self._bytes = 0

    def stage(
        self,
        relative: PurePosixPath,
        source: Path,
        label: str,
        expected_hash: str,
        expected_size: int,
        *,
        source_descriptor: int | None = None,
    ) -> Path:
        if self._closed:
            raise RuntimeError("payload staging lifetime is already closed")
        if self._bytes + expected_size > MAX_STAGED_PAYLOAD_BYTES:
            _invalid(
                label,
                f"staged payloads exceed the {MAX_STAGED_PAYLOAD_BYTES}-byte limit",
            )
        target = self.root.joinpath(*relative.parts)
        try:
            observed_size, observed_hash = _read_digest(
                source,
                label,
                expected_size,
                stage_path=target,
                source_descriptor=source_descriptor,
            )
            if observed_size != expected_size:
                _invalid(label, f"size mismatch: expected {expected_size}, observed {observed_size}")
            if observed_hash != expected_hash:
                _invalid(label, f"SHA-256 mismatch: expected {expected_hash}, observed {observed_hash}")
            os.chmod(target, 0o400)
            self._bytes += observed_size
            return target
        except Exception:
            try:
                target.unlink(missing_ok=True)
            except OSError:
                pass
            raise

    def close(self) -> None:
        if not self._closed:
            self._closed = True
            self._temporary_directory.cleanup()

    cleanup = close

    def __enter__(self) -> "PayloadStage":
        if self._closed:
            raise RuntimeError("payload staging lifetime is already closed")
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.close()


class ValidatedWorklist(dict[str, Any]):
    """Dictionary-compatible worklist with an explicit payload lifetime."""

    def __init__(
        self,
        value: dict[str, Any],
        payload_stage: PayloadStage | None = None,
        staged_models: dict[str, list[dict[str, Any]]] | None = None,
    ) -> None:
        super().__init__(value)
        self.payload_stage = payload_stage
        self.staged_models = staged_models or {}

    def close(self) -> None:
        if self.payload_stage is not None:
            self.payload_stage.close()


def _verify_payload(
    path: Path,
    label: str,
    expected_hash: str,
    expected_size: int,
    *,
    stage_path: Path | None = None,
    source_descriptor: int | None = None,
) -> None:
    observed_size, observed_hash = _read_digest(
        path,
        label,
        expected_size,
        stage_path=stage_path,
        source_descriptor=source_descriptor,
    )
    if observed_size != expected_size:
        if stage_path is not None:
            stage_path.unlink(missing_ok=True)
        _invalid(label, f"size mismatch: expected {expected_size}, observed {observed_size}")
    if observed_hash != expected_hash:
        if stage_path is not None:
            stage_path.unlink(missing_ok=True)
        _invalid(label, f"SHA-256 mismatch: expected {expected_hash}, observed {observed_hash}")


def _reject_symlink_components(root: Path, relative: PurePosixPath, label: str) -> Path:
    candidate = root.joinpath(*relative.parts)
    current = root
    for part in relative.parts:
        current = current / part
        try:
            info = os.lstat(current)
        except OSError as error:
            _invalid(label, f"cannot inspect payload path: {error}")
        if stat.S_ISLNK(info.st_mode):
            _invalid(label, "contains a symlink")
        if current != candidate and not stat.S_ISDIR(info.st_mode):
            _invalid(label, "contains a non-directory path component")
    return candidate


def _validate_policy(value: Any, label: str, *, qualification: bool = False) -> None:
    if not isinstance(value, dict):
        _invalid(label, "must be an object")
    required = {"classification", "required_assertions", "rule_id"}
    if not required <= set(value):
        _invalid(label, f"missing fields {sorted(required - set(value))!r}")
    if not all(isinstance(value[key], str) and value[key] for key in ("classification", "rule_id")):
        _invalid(label, "classification and rule_id must be non-empty strings")
    assertions = value["required_assertions"]
    if not isinstance(assertions, list) or not all(isinstance(item, str) and item for item in assertions):
        _invalid(label, "required_assertions must be a string array")
    if qualification and set(value) - required:
        _invalid(label, f"unsupported fields {sorted(set(value) - required)!r}")
    if not qualification:
        extra = set(value) - _FILE_POLICY_FIELDS
        missing = _FILE_POLICY_FIELDS - set(value)
        if missing:
            _invalid(label, f"missing fields {sorted(missing)!r}")
        if extra:
            _invalid(label, f"unsupported fields {sorted(extra)!r}")
        semantic = value["semantic_context_assertions"]
        if not isinstance(semantic, list) or not all(isinstance(item, str) and item for item in semantic):
            _invalid(label, "semantic_context_assertions must be a string array")
        expected = value["expected_unsupported"]
        if expected is not None:
            if not isinstance(expected, dict):
                _invalid(f"{label}.expected_unsupported", "must be an object")
            extra = set(expected) - {"error_behavior", "statuses"}
            if extra:
                _invalid(
                    f"{label}.expected_unsupported",
                    f"unsupported fields {sorted(extra)!r}",
                )
            if "error_behavior" in expected:
                _string(expected["error_behavior"], f"{label}.expected_unsupported.error_behavior")
            if "statuses" not in expected:
                _invalid(f"{label}.expected_unsupported", "missing fields ['statuses']")
            statuses = expected["statuses"]
            if not isinstance(statuses, list) or not statuses:
                _invalid(f"{label}.expected_unsupported.statuses", "must be a non-empty array")
            for index, status in enumerate(statuses):
                if not (isinstance(status, int) and not isinstance(status, bool) and status >= 100) and status != "discovery_skip":
                    _invalid(
                        f"{label}.expected_unsupported.statuses[{index}]",
                        "must be an HTTP status integer or discovery_skip",
                    )


def _validate_file_row(
    row: Any,
    profile: str,
    root: Path,
    index: int,
    manifest_hash: str,
    payload_stage: PayloadStage | None = None,
) -> Path | None:
    label = f"{profile} payload[{index}]"
    row = _object(row, label, _FILE_FIELDS)
    if row["kind"] != profile:
        _invalid(label, f"kind must be {profile!r}")
    case_id = _string(row["case_id"], f"{label}.case_id")
    relative = _safe_relative_path(row["path"], f"{label}.path")
    normalized = _absolute_path(row["normalized_path"], f"{label}.normalized_path")
    _sha256(row["sha256"], f"{label}.sha256")
    _integer(row["size_bytes"], f"{label}.size_bytes", maximum=MAX_PAYLOAD_BYTES)
    _sha256(row["contract_sha256"], f"{label}.contract_sha256")
    _sha256(row["manifest_identity_sha256"], f"{label}.manifest_identity_sha256")
    if not isinstance(row["expected_contract"], dict):
        _invalid(f"{label}.expected_contract", "must be an object")
    expected_contract_hash = canonical_sha256(row["expected_contract"])
    if row["contract_sha256"] != expected_contract_hash:
        _invalid(
            label,
            f"contract SHA-256 mismatch: expected {expected_contract_hash}, observed {row['contract_sha256']}",
        )
    identity = _object(row["manifest_identity"], f"{label}.manifest_identity", _IDENTITY_FIELDS)
    if identity["profile"] != profile or identity["case_id"] != case_id or identity["path"] != row["path"]:
        _invalid(label, "manifest identity does not match the payload row")
    _sha256(identity["manifest_sha256"], f"{label}.manifest_identity.manifest_sha256")
    if identity["manifest_sha256"] != manifest_hash:
        _invalid(
            label,
            "manifest identity is not bound to the independently verified profile manifest",
        )
    expected_identity_hash = canonical_sha256(identity)
    if row["manifest_identity_sha256"] != expected_identity_hash:
        _invalid(
            label,
            f"manifest identity SHA-256 mismatch: expected {expected_identity_hash}, observed {row['manifest_identity_sha256']}",
        )
    sop_uid = row["sop_instance_uid"]
    if profile == "stress" and (not isinstance(sop_uid, str) or not sop_uid):
        _invalid(label, "stress payload requires a SOP Instance UID")
    if profile == "negative" and sop_uid is not None and (not isinstance(sop_uid, str) or not sop_uid):
        _invalid(label, "negative payload SOP Instance UID must be null or a string")
    _validate_policy(row["policy"], f"{label}.policy")
    candidate = _reject_symlink_components(root, relative, label)
    try:
        normalized_resolved = normalized.resolve(strict=True)
        candidate_resolved = candidate.resolve(strict=True)
    except (OSError, RuntimeError) as error:
        _invalid(label, f"cannot resolve payload path: {error}")
    if candidate_resolved != candidate or normalized != candidate or normalized_resolved != candidate_resolved:
        _invalid(label, "normalized_path is not the confined payload path")
    try:
        info = os.lstat(candidate)
    except OSError as error:
        _invalid(label, f"cannot inspect payload: {error}")
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        _invalid(label, "payload must be a regular file")
    descriptor = _open_confined_descriptor(root, relative, label)
    try:
        if payload_stage is None:
            _verify_payload(
                candidate,
                label,
                row["sha256"],
                row["size_bytes"],
                source_descriptor=descriptor,
            )
            return None
        staged = payload_stage.stage(
            relative,
            candidate,
            label,
            row["sha256"],
            row["size_bytes"],
            source_descriptor=descriptor,
        )
        return staged
    finally:
        os.close(descriptor)


def _validate_qualification(value: Any, profile: str, index: int) -> None:
    label = f"{profile} qualification[{index}]"
    row = _object(value, label, _QUALIFICATION_FIELDS)
    if row["profile"] != profile:
        _invalid(label, f"profile must be {profile!r}")
    _string(row["case_id"], f"{label}.case_id")
    if not isinstance(row["contract"], dict):
        _invalid(f"{label}.contract", "must be an object")
    _sha256(row["contract_sha256"], f"{label}.contract_sha256")
    expected_contract_hash = canonical_sha256(row["contract"])
    if row["contract_sha256"] != expected_contract_hash:
        _invalid(
            label,
            f"qualification contract SHA-256 mismatch: expected {expected_contract_hash}, observed {row['contract_sha256']}",
        )
    _validate_policy(row["policy"], f"{label}.policy", qualification=True)


def _selected_count_keys(
    worklist: dict[str, Any],
    profile: str,
) -> tuple[set[tuple[str, str]], set[str], set[tuple[str, str]], set[tuple[str, str, str]]]:
    files = worklist["files"]
    file_keys: set[tuple[str, str]] = set()
    logical_cases: set[str] = set()
    for index, value in enumerate(files):
        row = _object(value, f"files[{index}]", _FILE_FIELDS)
        case_id = _string(row["case_id"], f"files[{index}].case_id")
        path = _string(row["path"], f"files[{index}].path")
        file_keys.add((case_id, path))
        logical_cases.add(case_id)

    qualifications = list(worklist["models"]["stress_scenarios"])
    qualifications.extend(worklist["models"]["fuzz_qualifications"])
    qualification_keys: set[tuple[str, str]] = set()
    for index, value in enumerate(qualifications):
        row = _object(value, f"selected qualifications[{index}]", _QUALIFICATION_FIELDS)
        case_id = _string(row["case_id"], f"selected qualifications[{index}].case_id")
        if row["profile"] != profile:
            _invalid(
                f"selected qualifications[{index}].profile",
                f"must be {profile!r}",
            )
        qualification_keys.add((row["profile"], case_id))

    unavailable_keys: set[tuple[str, str, str]] = set()
    for index, value in enumerate(worklist["unavailable"]):
        row = _object(value, f"unavailable[{index}]", _UNAVAILABLE_FIELDS)
        unavailable_profile = _string(row["profile"], f"unavailable[{index}].profile")
        if unavailable_profile != profile:
            _invalid(
                f"unavailable[{index}].profile",
                f"must be {profile!r} for the selected profile",
            )
        case_id = _string(row["case_id"], f"unavailable[{index}].case_id")
        _string(row["message"], f"unavailable[{index}].message")
        reason_code = _string(row["reason_code"], f"unavailable[{index}].reason_code")
        _string(row["recheck_phase"], f"unavailable[{index}].recheck_phase")
        if row["status"] != "unavailable":
            _invalid(f"unavailable[{index}].status", "must be 'unavailable'")
        if not isinstance(row["standards_evidence"], list):
            _invalid(f"unavailable[{index}].standards_evidence", "must be an array")
        unavailable_keys.add((unavailable_profile, case_id, reason_code))
    if len(qualification_keys) != len(qualifications):
        _invalid("selected qualifications", "contains duplicate qualification identities")
    if len(unavailable_keys) != len(worklist["unavailable"]):
        _invalid("unavailable", "contains duplicate unavailable identities")
    logical_cases.update(case_id for _profile, case_id in qualification_keys)
    return file_keys, logical_cases, qualification_keys, unavailable_keys


def _validate_worklist_shape(worklist: dict[str, Any]) -> str:
    if worklist.get("worklist_schema_version") != WORKLIST_SCHEMA_VERSION:
        _invalid(
            "schema",
            f"incompatible worklist schema {worklist.get('worklist_schema_version')!r}; expected {WORKLIST_SCHEMA_VERSION}",
        )
    missing = _TOP_LEVEL_FIELDS - set(worklist)
    extra = set(worklist) - _TOP_LEVEL_FIELDS
    if missing:
        _invalid("root", f"missing fields {sorted(missing)!r}")
    if extra:
        _invalid("root", f"unsupported fields {sorted(extra)!r}")
    declared = _sha256(worklist["worklist_sha256"], "worklist_sha256")
    unhashed = {key: value for key, value in worklist.items() if key != "worklist_sha256"}
    observed = hashlib.sha256(canonical_json(unhashed)).hexdigest()
    if declared != observed:
        _invalid("hash", f"content SHA-256 mismatch: expected {declared}, observed {observed}")

    suite = _object(worklist["suite"], "suite", _SUITE_FIELDS)
    _absolute_path(suite["root"], "suite.root")
    commit = _string(suite["commit"], "suite.commit")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        _invalid("suite.commit", "must be a 40-character commit SHA")

    inputs = _object(worklist["inputs"], "inputs", _INPUT_FIELDS)
    _string(inputs["lock"], "inputs.lock")
    _sha256(inputs["lock_sha256"], "inputs.lock_sha256")
    _string(inputs["policy"], "inputs.policy")
    _sha256(inputs["policy_sha256"], "inputs.policy_sha256")
    profiles = _list(inputs["profiles"], "inputs.profiles", maximum=len(ROBUSTNESS_PROFILES))
    if len(profiles) != 1 or not isinstance(profiles[0], str) or profiles[0] not in ROBUSTNESS_PROFILES:
        _invalid("inputs.profiles", f"must select exactly one of {sorted(ROBUSTNESS_PROFILES)!r}")
    profile = profiles[0]
    manifests = _list(inputs["manifests"], "inputs.manifests", maximum=1)
    if len(manifests) != 1:
        _invalid("inputs.manifests", "must contain exactly one profile manifest")
    manifest = _object(manifests[0], "inputs.manifests[0]", _MANIFEST_FIELDS)
    if manifest["profile"] != profile:
        _invalid("inputs.manifests[0].profile", f"must be {profile!r}")
    _absolute_path(manifest["manifest"], "inputs.manifests[0].manifest")
    _absolute_path(manifest["root"], "inputs.manifests[0].root")
    _sha256(manifest["sha256"], "inputs.manifests[0].sha256")
    for field in ("physical_files", "logical_cases", "qualifications"):
        _integer(manifest[field], f"inputs.manifests[0].{field}", maximum=MAX_WORKLIST_ROWS)

    models = _object(worklist["models"], "models", _MODEL_FIELDS)
    for name, value in models.items():
        _list(value, f"models.{name}")
    selected_model = {"negative": "negative_inputs", "stress": "stress_files", "fuzz": "fuzz_qualifications"}[profile]
    for name, value in models.items():
        if name != selected_model and name != "stress_scenarios" and name != "fuzz_qualifications":
            if value:
                _invalid(f"models.{name}", f"must be empty for the {profile} profile")
    if profile != "stress" and models["stress_scenarios"]:
        _invalid("models.stress_scenarios", f"must be empty for the {profile} profile")
    if profile != "fuzz" and models["fuzz_qualifications"]:
        _invalid("models.fuzz_qualifications", f"must be empty for the {profile} profile")
    files = _list(worklist["files"], "files")
    _list(worklist["unavailable"], "unavailable")
    summary = _object(worklist["summary"], "summary", _SUMMARY_FIELDS)
    for field in _SUMMARY_FIELDS:
        _integer(summary[field], f"summary.{field}", maximum=MAX_WORKLIST_ROWS)
    if profile in {"negative", "stress"} and not models[selected_model]:
        _invalid(f"models.{selected_model}", "must not be empty")
    if profile == "fuzz" and len(models[selected_model]) != 1:
        _invalid("models.fuzz_qualifications", "must contain exactly one qualification")
    expected_files = models[selected_model] if profile in {"negative", "stress"} else []
    if files != expected_files:
        _invalid("files", f"must exactly match models.{selected_model}")
    file_keys, logical_cases, qualification_keys, unavailable_keys = _selected_count_keys(
        worklist,
        profile,
    )
    if len(file_keys) != len(files):
        _invalid("files", "contains duplicate selected payload rows")
    expected_counts = {
        "physical_files": len(file_keys),
        "logical_cases": len(logical_cases),
        "qualifications": len(qualification_keys),
    }
    for field, expected in expected_counts.items():
        if manifest[field] != expected:
            _invalid(
                "inputs.manifests[0]",
                f"{field} count does not match distinct selected rows/qualifications: expected {expected}, observed {manifest[field]}",
            )
    expected_summary = {
        "files": len(file_keys),
        "logical_cases": len(logical_cases),
        "qualifications": len(qualification_keys),
        "unavailable_selected_profiles": len(unavailable_keys),
    }
    for field, expected in expected_summary.items():
        if summary[field] != expected:
            _invalid(
                "summary",
                f"{field} count does not match distinct selected rows/qualifications: expected {expected}, observed {summary[field]}",
            )
    return profile


def load_worklist(
    path: Path,
    *,
    stage_payloads: bool = False,
) -> ValidatedWorklist:
    """Load and hash-check a generic robustness worklist.

    Valid-corpus campaigns construct their worklist from a verified producer
    artifact. This parser remains for the opt-in negative, stress, and fuzz
    runners, which consume caller-supplied bounded worklists.
    """

    worklist = load_json(path)
    profile = _validate_worklist_shape(worklist)
    manifest = worklist["inputs"]["manifests"][0]
    root = _regular_directory(Path(manifest["root"]), "inputs.manifests[0].root")
    manifest_path = _absolute_path(manifest["manifest"], "inputs.manifests[0].manifest")
    expected_manifest = root / "manifest.json"
    try:
        manifest_resolved = manifest_path.resolve(strict=False)
    except (OSError, RuntimeError) as error:
        _invalid("inputs.manifests[0].manifest", f"cannot resolve path: {error}")
    if manifest_resolved != expected_manifest:
        _invalid("inputs.manifests[0].manifest", "must be root/manifest.json")
    _manifest_size, manifest_hash = _read_digest(
        manifest_path, "inputs.manifests[0].manifest"
    )
    if manifest_hash != manifest["sha256"]:
        _invalid(
            "inputs.manifests[0].manifest",
            f"SHA-256 mismatch: expected {manifest['sha256']}, observed {manifest_hash}",
        )
    selected_model = {"negative": "negative_inputs", "stress": "stress_files", "fuzz": "fuzz_qualifications"}[profile]
    payload_stage = PayloadStage() if stage_payloads and profile in {"negative", "stress"} else None
    staged_models: dict[str, list[dict[str, Any]]] = {}
    try:
        if profile in {"negative", "stress"}:
            seen_identity: set[tuple[str, str]] = set()
            seen_paths: set[str] = set()
            staged_rows: list[dict[str, Any]] = []
            for index, row in enumerate(worklist["models"][selected_model]):
                original_path = row["normalized_path"]
                staged = _validate_file_row(
                    row,
                    profile,
                    root,
                    index,
                    manifest_hash,
                    payload_stage,
                )
                identity = (row["case_id"], row["path"])
                if identity in seen_identity or original_path in seen_paths:
                    _invalid(f"{profile} payload[{index}]", "duplicates an earlier payload")
                seen_identity.add(identity)
                seen_paths.add(original_path)
                if staged is not None:
                    staged_row = dict(row)
                    staged_row["normalized_path"] = str(staged)
                    staged_rows.append(staged_row)
                else:
                    staged_rows.append(row)
            if payload_stage is not None:
                staged_models[selected_model] = staged_rows
        if profile == "stress":
            for index, row in enumerate(worklist["models"]["stress_scenarios"]):
                _validate_qualification(row, profile, index)
        if profile == "fuzz":
            for index, row in enumerate(worklist["models"]["fuzz_qualifications"]):
                _validate_qualification(row, profile, index)
        return ValidatedWorklist(worklist, payload_stage, staged_models)
    except Exception:
        if payload_stage is not None:
            payload_stage.close()
        raise


__all__ = [
    "CONTRACT_EXCLUDED_FIELDS",
    "CompatibilityError",
    "MAX_PAYLOAD_BYTES",
    "MAX_STAGED_PAYLOAD_BYTES",
    "MAX_WORKLIST_BYTES",
    "MAX_WORKLIST_PATH_BYTES",
    "MAX_WORKLIST_ROWS",
    "PayloadStage",
    "ROBUSTNESS_PROFILES",
    "ValidatedWorklist",
    "WORKLIST_SCHEMA_VERSION",
    "applicable_assertions",
    "canonical_json",
    "canonical_sha256",
    "load_json",
    "load_worklist",
    "sha256_file",
]

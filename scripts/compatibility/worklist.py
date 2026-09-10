#!/usr/bin/env python3
"""Shared helpers for viewer-owned compatibility worklists."""

from __future__ import annotations

import hashlib
import json
import os
import re
import stat
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


class CompatibilityError(RuntimeError):
    """A viewer-owned compatibility input is malformed or unverifiable."""


def canonical_json(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


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
            parse_constant=_reject_constant,
        )
    except CompatibilityError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError, MemoryError) as error:
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


def _read_digest(path: Path, label: str, expected_size: int | None = None) -> tuple[int, str]:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        _invalid(label, f"cannot open payload: {error}")
    try:
        before = os.fstat(descriptor)
        if not stat.S_ISREG(before.st_mode):
            _invalid(label, "must name a regular file")
        if before.st_size > MAX_PAYLOAD_BYTES:
            _invalid(label, f"exceeds the {MAX_PAYLOAD_BYTES}-byte payload limit")
        if expected_size is not None and before.st_size != expected_size:
            _invalid(label, f"size mismatch: expected {expected_size}, observed {before.st_size}")
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
        after = os.fstat(descriptor)
    except CompatibilityError:
        raise
    except OSError as error:
        _invalid(label, f"cannot read payload: {error}")
    finally:
        os.close(descriptor)
    if before.st_dev != after.st_dev or before.st_ino != after.st_ino or before.st_size != after.st_size or total != after.st_size:
        _invalid(label, "changed while being verified")
    if expected_size is not None and total != expected_size:
        _invalid(label, f"size mismatch: expected {expected_size}, observed {total}")
    return total, digest.hexdigest()


def _verify_payload(path: Path, label: str, expected_hash: str, expected_size: int) -> None:
    observed_size, observed_hash = _read_digest(path, label, expected_size)
    if observed_size != expected_size:
        _invalid(label, f"size mismatch: expected {expected_size}, observed {observed_size}")
    if observed_hash != expected_hash:
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


def _validate_file_row(row: Any, profile: str, root: Path, index: int) -> None:
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
    identity = _object(row["manifest_identity"], f"{label}.manifest_identity", _IDENTITY_FIELDS)
    if identity["profile"] != profile or identity["case_id"] != case_id or identity["path"] != row["path"]:
        _invalid(label, "manifest identity does not match the payload row")
    _sha256(identity["manifest_sha256"], f"{label}.manifest_identity.manifest_sha256")
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
    _verify_payload(candidate, label, row["sha256"], row["size_bytes"])


def _validate_qualification(value: Any, profile: str, index: int) -> None:
    label = f"{profile} qualification[{index}]"
    row = _object(value, label, _QUALIFICATION_FIELDS)
    if row["profile"] != profile:
        _invalid(label, f"profile must be {profile!r}")
    _string(row["case_id"], f"{label}.case_id")
    if not isinstance(row["contract"], dict):
        _invalid(f"{label}.contract", "must be an object")
    _sha256(row["contract_sha256"], f"{label}.contract_sha256")
    _validate_policy(row["policy"], f"{label}.policy", qualification=True)


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
    unavailable = _list(worklist["unavailable"], "unavailable")
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
    expected_qualifications = len(models["stress_scenarios"]) + len(models["fuzz_qualifications"])
    if summary["files"] != len(files) or summary["qualifications"] != expected_qualifications:
        _invalid("summary", "file/qualification counts do not match the selected profile")
    if summary["unavailable_selected_profiles"] != len(unavailable):
        _invalid("summary", "unavailable count does not match the selected profile")
    return profile


def load_worklist(path: Path) -> dict[str, Any]:
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
    if profile in {"negative", "stress"}:
        seen_identity: set[tuple[str, str]] = set()
        seen_paths: set[str] = set()
        for index, row in enumerate(worklist["models"][selected_model]):
            _validate_file_row(row, profile, root, index)
            identity = (row["case_id"], row["path"])
            if identity in seen_identity or row["normalized_path"] in seen_paths:
                _invalid(f"{profile} payload[{index}]", "duplicates an earlier payload")
            seen_identity.add(identity)
            seen_paths.add(row["normalized_path"])
    if profile == "stress":
        for index, row in enumerate(worklist["models"]["stress_scenarios"]):
            _validate_qualification(row, profile, index)
    if profile == "fuzz":
        for index, row in enumerate(worklist["models"]["fuzz_qualifications"]):
            _validate_qualification(row, profile, index)
    return worklist


__all__ = [
    "CONTRACT_EXCLUDED_FIELDS",
    "CompatibilityError",
    "MAX_PAYLOAD_BYTES",
    "MAX_WORKLIST_BYTES",
    "MAX_WORKLIST_PATH_BYTES",
    "MAX_WORKLIST_ROWS",
    "ROBUSTNESS_PROFILES",
    "WORKLIST_SCHEMA_VERSION",
    "applicable_assertions",
    "canonical_json",
    "load_json",
    "load_worklist",
    "sha256_file",
]

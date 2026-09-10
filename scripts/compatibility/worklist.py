#!/usr/bin/env python3
"""Shared helpers for viewer-owned compatibility worklists."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

try:
    from scripts.compatibility.assertions import CAPABILITY_ASSERTIONS
except ModuleNotFoundError:
    from assertions import CAPABILITY_ASSERTIONS  # type: ignore[no-redef]


WORKLIST_SCHEMA_VERSION = "0.2.0"
CONTRACT_EXCLUDED_FIELDS = {"sha256", "size_bytes"}


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


def load_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
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


def load_worklist(path: Path) -> dict[str, Any]:
    """Load and hash-check a generic robustness worklist.

    Valid-corpus campaigns construct their worklist from a verified producer
    artifact. This parser remains for the opt-in negative, stress, and fuzz
    runners, which consume caller-supplied bounded worklists.
    """

    worklist = load_json(path)
    version = worklist.get("worklist_schema_version")
    if version != WORKLIST_SCHEMA_VERSION:
        raise CompatibilityError(
            f"incompatible worklist schema {version!r}; "
            f"expected {WORKLIST_SCHEMA_VERSION}"
        )
    declared = worklist.get("worklist_sha256")
    unhashed = {key: value for key, value in worklist.items() if key != "worklist_sha256"}
    observed = hashlib.sha256(canonical_json(unhashed)).hexdigest()
    if declared != observed:
        raise CompatibilityError(
            f"worklist content hash mismatch: expected {declared}, observed {observed}"
        )
    return worklist


__all__ = [
    "CONTRACT_EXCLUDED_FIELDS",
    "CompatibilityError",
    "WORKLIST_SCHEMA_VERSION",
    "applicable_assertions",
    "canonical_json",
    "load_json",
    "load_worklist",
    "sha256_file",
]

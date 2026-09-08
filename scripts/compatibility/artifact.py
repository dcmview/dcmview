#!/usr/bin/env python3
"""Verify and adapt a published external-corpus artifact for dcmview."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any

try:
    from scripts.compatibility.corpus import CorpusError, load_json, sha256_file
    from scripts.compatibility.policy import (
        DEFAULT_POLICY,
        PolicyError,
        load_policy,
        policy_sha256,
        resolve,
    )
    from scripts.compatibility.scope import (
        CONTRACT_EXCLUDED_FIELDS,
        WORKLIST_SCHEMA_VERSION,
        ScopeError,
        applicable_assertions,
        canonical_json,
    )
except ModuleNotFoundError:
    from corpus import CorpusError, load_json, sha256_file  # type: ignore[no-redef]
    from policy import (  # type: ignore[no-redef]
        DEFAULT_POLICY,
        PolicyError,
        load_policy,
        policy_sha256,
        resolve,
    )
    from scope import (  # type: ignore[no-redef]
        CONTRACT_EXCLUDED_FIELDS,
        WORKLIST_SCHEMA_VERSION,
        ScopeError,
        applicable_assertions,
        canonical_json,
    )


EXTERNAL_MANIFEST_SCHEMA_VERSION = "2.0.0"
EXTERNAL_MANIFEST_KIND = "external_corpus"
DEFAULT_PROFILE = "smoke"
DEFAULT_SEED = 1
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")


class ArtifactError(ScopeError):
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
) -> dict[str, Any]:
    """Verify one published external-corpus root without a suite checkout."""
    if profile != DEFAULT_PROFILE:
        raise ArtifactError(f"viewer artifact consumer only supports profile {DEFAULT_PROFILE!r}")
    resolved_root = root.resolve()
    if not resolved_root.is_dir():
        raise ArtifactError(f"corpus root is not a directory: {resolved_root}")
    manifest_path = resolved_root / "manifest.json"
    try:
        manifest = load_json(manifest_path)
    except CorpusError as error:
        raise ArtifactError(str(error)) from error
    manifest_sha256 = sha256_file(manifest_path)
    if expected_manifest_sha256 is not None and manifest_sha256 != expected_manifest_sha256:
        raise ArtifactError(
            f"manifest digest mismatch: expected {expected_manifest_sha256}, observed {manifest_sha256}"
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

    files = manifest.get("files")
    ledger = manifest.get("selection_ledger")
    if not isinstance(files, list) or not files:
        raise ArtifactError("external artifact manifest must contain files")
    if not isinstance(ledger, list) or not ledger:
        raise ArtifactError("external artifact manifest must contain selection_ledger")

    verified_files: list[dict[str, Any]] = []
    file_keys: set[tuple[str, str]] = set()
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
        selected = (resolved_root / relative).resolve()
        try:
            selected.relative_to(resolved_root)
        except ValueError as error:
            raise ArtifactError(f"manifest path escapes corpus root: {entry['path']!r}") from error
        if not selected.is_file():
            raise ArtifactError(f"manifest file is missing: {selected}")
        observed_size = selected.stat().st_size
        if observed_size != declared_size:
            raise ArtifactError(
                f"manifest file size mismatch for {entry['path']}: expected {declared_size}, observed {observed_size}"
            )
        observed_hash = sha256_file(selected)
        if observed_hash != declared_hash:
            raise ArtifactError(
                f"manifest file hash mismatch for {entry['path']}: expected {declared_hash}, observed {observed_hash}"
            )
        membership = entry.get("profile_membership")
        if not isinstance(membership, list) or profile not in membership:
            raise ArtifactError(f"manifest file is not a member of {profile}: {entry['path']}")
        key = (case_id, entry["path"])
        if key in file_keys:
            raise ArtifactError(f"duplicate manifest file identity: {case_id}:{entry['path']}")
        file_keys.add(key)
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
            if not isinstance(path, str) or (case_id, path) in ledger_keys:
                raise ArtifactError(f"duplicate selection ledger artifact: {case_id}:{path}")
            ledger_keys.add((case_id, path))
    if ledger_keys != file_keys:
        raise ArtifactError("selection ledger does not exactly close over generated manifest files")

    return {
        "root": str(resolved_root),
        "manifest_path": str(manifest_path),
        "manifest_sha256": manifest_sha256,
        "manifest": manifest,
        "files": verified_files,
        "identity": identity,
        "seed": seed,
    }


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
) -> dict[str, Any]:
    """Adapt a verified smoke artifact to the existing compatibility runner."""
    verified = verify_external_artifact(
        root,
        expected_seed=expected_seed,
        expected_manifest_sha256=expected_manifest_sha256,
        expected_corpus_definition_sha256=expected_corpus_definition_sha256,
        expected_generator_version=expected_generator_version,
        expected_generator_features=expected_generator_features,
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
    return worklist


__all__ = [
    "ArtifactError",
    "DEFAULT_PROFILE",
    "DEFAULT_SEED",
    "EXTERNAL_MANIFEST_SCHEMA_VERSION",
    "build_external_worklist",
    "verify_external_artifact",
]

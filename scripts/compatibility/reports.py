"""Build suite-compatible and dcmview-specific compatibility reports."""

from __future__ import annotations

import hashlib
import json
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any

try:
    from scripts.compatibility.assertions import evaluate
except ModuleNotFoundError:
    from assertions import evaluate  # type: ignore[no-redef]

DETAIL_VERSION = "1.0.0"
STATUSES = {"passed", "failed", "expected_unsupported", "unexpected_unsupported", "timeout", "crash", "unavailable", "not_applicable"}

def _passed(value: Any) -> bool:
    return value is True or isinstance(value, dict) and (value.get("passed") is True or value.get("status") == "passed")

def assertion_evidence(observations: dict[str, Any]) -> dict[str, Any]:
    series = observations.get("series_navigation")
    series_capabilities = (series or {}).get("capabilities") or {}
    specialized_geometry = [
        observations[key]
        for key in (
            "nm_dimensions",
            "pet_activity",
            "frame_time",
            "projection_geometry",
            "pixel_geometry",
        )
        if key in observations
    ]
    series_checks = list(series_capabilities.values()) + specialized_geometry
    series_evidence = None if not series_checks else {
        "passed": bool((series or {}).get("mapped"))
        and all(_passed(value) for value in series_checks)
    }
    presentation = [observations.get(key) for key in ("visual", "overlay_display", "display_shutter", "icc_profile") if key in observations]
    visual = observations.get("visual")
    normalized_display = visual if visual is not None else observations.get("png_dimensions")
    return {**observations,
            "mapped_after_scan": _passed(observations.get("mapped_after_scan")) and _passed(observations.get("file_info")),
            "raw_headers": observations.get("raw_headers"), "lossy_metrics": observations.get("lossy_metrics"),
            "normalized_display_hash": observations.get("normalized_display_hash") or normalized_display,
            "presentation_checks": None if not presentation else {"passed": all(_passed(v) for v in presentation)},
            "frame_access": observations.get("frame_access") or observations.get("frame_navigation") or observations.get("png_dimensions"),
            "series_navigation": series_evidence, "recovery_after_error": observations.get("recovery_after_error"),
            "renderer_absent": observations.get("metadata_only_response")}

def _status(result: dict[str, Any], policy: dict[str, Any], assertions: list[dict[str, Any]]) -> str:
    if result.get("execution_safety") == "crash": return "crash"
    if result.get("execution_safety") == "timeout": return "timeout"
    if any(row["status"] != "passed" for row in assertions): return "failed"
    statuses = [row.get("status") for row in result.get("http", {}).values() if isinstance(row, dict)]
    if policy["classification"] == "controlled_unsupported":
        expected = set((policy.get("expected_unsupported") or {}).get("statuses", []))
        return "expected_unsupported" if any(status in expected for status in statuses) else "unexpected_unsupported"
    return "passed"

def build_evidence_report(base: dict[str, Any], worklist: dict[str, Any], viewer_commit: str, build_features: list[str]) -> dict[str, Any]:
    policies = {(row["manifest_identity"]["profile"], row["case_id"], row["path"]): row for row in worklist["files"]}
    results = []
    for result in base["results"]:
        source = policies[(result["root"], result["case_id"], result["path"])]
        policy = source["policy"]; evidence = assertion_evidence(result["observations"])
        semantic_ids = set(policy["semantic_context_assertions"])
        assertions = []
        for name in policy["required_assertions"] + policy["semantic_context_assertions"]:
            assertion = evaluate(name, evidence)
            assertion["scope"] = "semantic_context" if name in semantic_ids else "required"
            assertions.append(assertion)
        status = _status(result, policy, assertions)
        expected = source["expected_contract"]; dicom = expected.get("dicom") or {}
        results.append({"profile": result["root"], "case_id": result["case_id"], "path": result["path"], "status": status,
                        "object_family": result["case_id"].split("/")[0], "transfer_syntax_uid": dicom.get("transfer_syntax_uid"),
                        "policy": policy, "assertions": assertions, "observations": result["observations"], "http": result["http"],
                        "timings_ms": result["timings_ms"], "errors": result["errors"],
                        "payload_hashes": {name: row.get("body_sha256") for name, row in result["http"].items() if isinstance(row, dict) and row.get("body_sha256")}})
    dimensions: dict[str, Counter[str]] = defaultdict(Counter)
    for row in results:
        for assertion in row["assertions"]: dimensions[assertion["dimension"]][assertion["status"]] += 1
    def grouped(field: str) -> dict[str, dict[str, int]]:
        groups: dict[str, Counter[str]] = defaultdict(Counter)
        for row in results: groups[str(row.get(field) or "none")][row["status"]] += 1
        return {key: dict(value) for key, value in sorted(groups.items())}
    suite = {"commit": worklist["suite"]["commit"], "manifests": worklist["inputs"]["manifests"], "policy_sha256": worklist["inputs"]["policy_sha256"]}
    if worklist["inputs"].get("artifact_identity") is not None:
        suite["artifact_identity"] = worklist["inputs"]["artifact_identity"]
    return {"evidence_schema_version": DETAIL_VERSION, "generated_at": base["generated_at"],
            "suite": suite,
            "viewer": {**base["viewer"], "commit": viewer_commit, "build_features": build_features}, "run": base["run"], "results": results,
            "summary": {"statuses": dict(Counter(row["status"] for row in results)), "by_object_family": grouped("object_family"),
                        "by_transfer_syntax": grouped("transfer_syntax_uid"), "by_classification": grouped_policy(results),
                        "by_dimension": {key: dict(value) for key, value in sorted(dimensions.items())}}, "artifacts": base["artifacts"]}

def grouped_policy(results: list[dict[str, Any]]) -> dict[str, dict[str, int]]:
    groups: dict[str, Counter[str]] = defaultdict(Counter)
    for row in results: groups[row["policy"]["classification"]][row["status"]] += 1
    return {key: dict(value) for key, value in sorted(groups.items())}

def build_viewer_report(evidence: dict[str, Any]) -> dict[str, Any]:
    manifests = {row["sha256"] for row in evidence["suite"]["manifests"] if any(result["profile"] == row["profile"] for result in evidence["results"])}
    if not manifests:
        # Preserve the input identity when the viewer exits before discovery
        # yields a result.  The detail/evidence report still records the
        # startup failure; omitting the companion report would hide that fact.
        manifests = {row["sha256"] for row in evidence["suite"]["manifests"]}
    if len(manifests) != 1: raise ValueError("suite viewer report requires a single selected profile manifest")
    mapped = {"passed": "passed", "expected_unsupported": "passed", "failed": "failed", "unexpected_unsupported": "failed", "crash": "failed", "timeout": "timeout", "unavailable": "unavailable", "not_applicable": "skipped"}
    rows = []
    for row in evidence["results"]:
        status = mapped[row["status"]]; assertions = {item["assertion"]: item for item in row["assertions"]}
        rows.append({"case_id": row["case_id"], "path": row["path"], "status": status,
                     "file_open": {"status": status}, "object_recognition": {"status": status},
                     "metadata": {"status": status, "extracted": {}},
                     "pixel_rendering": {"status": status, "decoded_pixel_hash": row["payload_hashes"].get("raw_first"), "normalized_preview_hash": row["payload_hashes"].get("display_first")},
                     "timing": {"open_ms": None, "render_ms": None, "total_ms": row["timings_ms"].get("total")},
                     "errors": row["errors"], "warnings": [], "artifacts": []})
    counts = Counter(row["status"] for row in rows)
    report = {"viewer_report_schema_version": "0.1.0", "generated_at": evidence["generated_at"], "suite_manifest_sha256": next(iter(manifests)),
            "viewer": {"name": "dcmview", "version": evidence["viewer"]["version"], "command": " ".join(evidence["run"]["command"]), "environment": {}},
            "run": {"started_at": evidence["run"]["started_at"], "completed_at": evidence["run"]["completed_at"], "timeout_seconds": evidence["run"]["timeouts_seconds"]["shard"]},
            "results": rows, "summary": {name: counts[name] for name in ("passed", "failed", "skipped", "timeout", "unavailable")}}
    if evidence["suite"].get("artifact_identity") is not None:
        report["artifact_identity"] = evidence["suite"]["artifact_identity"]
    return report

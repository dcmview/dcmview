#!/usr/bin/env python3
"""Exercise every negative worklist input in a bounded isolated viewer."""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

try:
    from scripts.compatibility.robustness import BoundedProcess, RobustnessError, bounded_get, load_profile, poll_catalog, utc_now, viewer_identity, write_report
except ModuleNotFoundError:
    from robustness import BoundedProcess, RobustnessError, bounded_get, load_profile, poll_catalog, utc_now, viewer_identity, write_report  # type: ignore[no-redef]


def acceptable_outcomes(entry: dict[str, Any]) -> set[str]:
    steps = entry["expected_contract"].get("negative_evidence", {}).get("mutation_steps", [])
    return {value for step in steps for value in step.get("acceptable_outcomes", []) if isinstance(value, str)}


def classify(
    discovered: bool,
    response: dict[str, Any] | None,
    expected_failure_layer: str | None = None,
) -> str:
    if not discovered:
        return "clean_rejection"
    assert response is not None
    if response["status"] == 200:
        return "accepted_with_bounded_warning"
    error = response.get("json") or {}
    text = json.dumps(error, sort_keys=True).lower()
    if "data set token" in text or "dataset token" in text:
        return (
            "parse_failure"
            if expected_failure_layer == "dataset_parser"
            else "decode_failure"
        )
    if "decode" in text or "pixel" in text:
        return "decode_failure"
    if "parse" in text or "dicom" in text:
        return "parse_failure"
    return "validation_failure"


def run_case(entry: dict[str, Any], binary: Path, healthy: Path, viewer_root: Path, args: argparse.Namespace) -> dict[str, Any]:
    started = time.monotonic()
    malformed = Path(entry["normalized_path"]).resolve()
    command = [str(binary.resolve()), "--no-browser", "--host", "127.0.0.1", "--port", "0", "--startup-json", str(malformed), str(healthy)]
    process = BoundedProcess(command, viewer_root, args.max_output_bytes)
    result: dict[str, Any] = {"case_id": entry["case_id"], "path": str(malformed), "command": command}
    outcome = "crash"
    try:
        base_url = process.wait_for_url(args.startup_timeout)
        catalog = poll_catalog(base_url, args.case_timeout, args.request_timeout, args.max_response_bytes)
        files = catalog["json"]["files"]
        malformed_row = next((row for row in files if Path(row["path"]).resolve() == malformed), None)
        healthy_row = next((row for row in files if Path(row["path"]).resolve() == healthy), None)
        response = None
        if malformed_row is not None:
            endpoint = f"/api/file/{malformed_row['index']}/frame/0" if malformed_row.get("has_pixels") else f"/api/file/{malformed_row['index']}/tags"
            response = bounded_get(base_url, endpoint, args.request_timeout, args.max_response_bytes)
        mutation_steps = entry["expected_contract"].get("negative_evidence", {}).get("mutation_steps", [])
        expected_failure_layer = mutation_steps[-1].get("expected_failure_layer") if mutation_steps else None
        outcome = classify(malformed_row is not None, response, expected_failure_layer)
        recovery = None if healthy_row is None else bounded_get(base_url, f"/api/file/{healthy_row['index']}/tags", args.request_timeout, args.max_response_bytes)
        permitted = acceptable_outcomes(entry)
        policy_statuses = entry["policy"].get("expected_unsupported", {}).get("statuses", [])
        policy_ok = (not malformed_row and "discovery_skip" in policy_statuses) or (response is not None and response["status"] in policy_statuses) or outcome == "accepted_with_bounded_warning"
        result.update({
            "suite_acceptable_outcomes": sorted(permitted), "observed_outcome": outcome,
            "malformed_discovered": malformed_row is not None, "malformed_response": response,
            "recovery_response": recovery,
            "assertions": {
                "negative_bounded_outcome": outcome in permitted and policy_ok,
                "server_recovers_after_error": recovery is not None and recovery["status"] == 200 and process.process.poll() is None,
            },
        })
    except TimeoutError as error:
        outcome = "timeout"
        result["error"] = str(error)
    except Exception as error:  # evidence must survive one malformed case
        outcome = "crash" if process.process.poll() is not None else "runner_error"
        result["error"] = f"{type(error).__name__}: {error}"
    finally:
        shutdown = process.shutdown(args.shutdown_timeout)
        logs = process.logs()
        result.update({
            "observed_outcome": outcome, "shutdown": shutdown,
            "output": {key: value for key, value in logs.items() if not isinstance(value, bytes)},
            "stdout_sha256": __import__("hashlib").sha256(logs["stdout"]).hexdigest(),
            "stderr_sha256": __import__("hashlib").sha256(logs["stderr"]).hexdigest(),
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        })
    result["passed"] = all(result.get("assertions", {}).values()) and not result["shutdown"]["forced"]
    return result


def run(args: argparse.Namespace) -> dict[str, Any]:
    loaded = load_profile(args.worklist.resolve(), "negative_inputs", "negative")
    try:
        return _run_loaded(args, loaded)
    finally:
        loaded.close()


def _run_loaded(args: argparse.Namespace, loaded: Any) -> dict[str, Any]:
    worklist, entries = loaded.worklist, loaded.entries
    if not entries:
        raise RobustnessError("negative_inputs model is empty")
    healthy = args.healthy_file.resolve()
    if not healthy.is_file():
        raise RobustnessError(f"healthy recovery file does not exist: {healthy}")
    identity = viewer_identity(args.binary)
    viewer_root = Path(__file__).resolve().parents[2]
    results = [run_case(entry, args.binary, healthy, viewer_root, args) for entry in entries]
    report = {
        "robustness_report_version": "1.0.0", "runner": "negative", "generated_at": utc_now(),
        "worklist": {"path": str(args.worklist.resolve()), "content_sha256": worklist["worklist_sha256"]},
        "viewer": identity, "bounds": {name: getattr(args, name) for name in ("startup_timeout", "request_timeout", "case_timeout", "shutdown_timeout", "max_output_bytes", "max_response_bytes")},
        "healthy_recovery_file": str(healthy), "results": results,
        "summary": {"selected": len(results), "passed": sum(row["passed"] for row in results), "failed": sum(not row["passed"] for row in results)},
    }
    write_report(args.output, report)
    return report


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worklist", type=Path, required=True); parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--healthy-file", type=Path, required=True); parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--startup-timeout", type=float, default=15.0); parser.add_argument("--request-timeout", type=float, default=5.0)
    parser.add_argument("--case-timeout", type=float, default=20.0); parser.add_argument("--shutdown-timeout", type=float, default=5.0)
    parser.add_argument("--max-output-bytes", type=int, default=1_048_576); parser.add_argument("--max-response-bytes", type=int, default=8_388_608)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    try:
        report = run(parse_args(sys.argv[1:] if argv is None else argv))
    except (RobustnessError, OSError, ValueError) as error:
        print(f"negative runner error: {error}", file=sys.stderr); return 2
    print(json.dumps(report["summary"], sort_keys=True)); return 0 if report["summary"]["failed"] == 0 else 1


if __name__ == "__main__": raise SystemExit(main())

#!/usr/bin/env python3
"""Run a bounded, payload-disciplined dcmview fuzz qualification."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Iterator

try:
    from scripts.compatibility.robustness import BoundedProcess, RobustnessError, bounded_get, load_profile, poll_catalog, utc_now, viewer_identity, write_report
except ModuleNotFoundError:
    from robustness import BoundedProcess, RobustnessError, bounded_get, load_profile, poll_catalog, utc_now, viewer_identity, write_report  # type: ignore[no-redef]


def candidates(seed_bytes: bytes, run_seed: int, count: int, max_mutations: int, max_input_bytes: int) -> Iterator[tuple[bytes, int]]:
    if len(seed_bytes) > max_input_bytes: raise RobustnessError(f"seed input exceeds {max_input_bytes} byte limit")
    for iteration in range(count):
        rng = random.Random((run_seed << 32) ^ iteration ^ len(seed_bytes)); value = bytearray(seed_bytes); mutations = 0
        for _ in range(1 + rng.randrange(max_mutations)):
            operation = rng.randrange(4)
            if operation == 0 and value:
                value[rng.randrange(len(value))] ^= 1 << rng.randrange(8)
            elif operation == 1 and value:
                del value[rng.randrange(len(value))]
            elif operation == 2 and len(value) < max_input_bytes:
                value.insert(rng.randrange(len(value) + 1), rng.randrange(256))
            elif operation == 3 and value:
                del value[rng.randrange(len(value)):]
            mutations += 1
        yield bytes(value), mutations


def qualification_budget(qualification: dict[str, Any], args: argparse.Namespace) -> dict[str, int]:
    contract = qualification.get("contract", qualification).get("budget", {})
    requested = {
        "max_candidates": args.max_candidates, "max_mutations_per_candidate": args.max_mutations_per_candidate,
        "max_input_bytes": args.max_input_bytes, "max_total_target_operations": args.max_total_target_operations,
    }
    for key, value in requested.items():
        ceiling = contract.get(key)
        if not isinstance(ceiling, int) or ceiling <= 0: raise RobustnessError(f"fuzz qualification lacks positive {key}")
        if value > ceiling: raise RobustnessError(f"requested {key}={value} exceeds qualification ceiling {ceiling}")
    return requested


def exercise(payload: bytes, binary: Path, healthy: Path, viewer_root: Path, args: argparse.Namespace) -> tuple[str, dict[str, Any]]:
    with tempfile.TemporaryDirectory(prefix="dcmview-fuzz-") as directory:
        candidate = Path(directory) / "candidate.dcm"; candidate.write_bytes(payload)
        command = [str(binary.resolve()), "--no-browser", "--host", "127.0.0.1", "--port", "0", "--startup-json", str(candidate), str(healthy)]
        process = BoundedProcess(command, viewer_root, args.max_output_bytes)
        evidence: dict[str, Any] = {}
        outcome = "crash"
        try:
            base_url = process.wait_for_url(args.target_timeout)
            catalog = poll_catalog(base_url, args.target_timeout, args.request_timeout, args.max_response_bytes)
            rows = catalog["json"]["files"]
            healthy_row = next((row for row in rows if Path(row["path"]).resolve() == healthy), None)
            recovery = None if healthy_row is None else bounded_get(base_url, f"/api/file/{healthy_row['index']}/tags", args.request_timeout, args.max_response_bytes)
            outcome = "accepted" if any(Path(row["path"]).resolve() == candidate for row in rows) else "clean_rejection"
            if recovery is None or recovery["status"] != 200: outcome = "recovery_failure"
            evidence["recovery_status"] = None if recovery is None else recovery["status"]
        except TimeoutError as error:
            outcome = "timeout"; evidence["error"] = str(error)
        except Exception as error:
            outcome = "crash" if process.process.poll() is not None else "target_error"; evidence["error"] = f"{type(error).__name__}: {error}"
        shutdown = process.shutdown(args.shutdown_timeout); logs = process.logs()
        if shutdown["forced"] and outcome not in {"timeout", "crash"}: outcome = "hang"
        evidence.update({"shutdown": shutdown, "stdout_sha256": hashlib.sha256(logs["stdout"]).hexdigest(), "stderr_sha256": hashlib.sha256(logs["stderr"]).hexdigest(), "discarded_output_bytes": logs["stdout_discarded_bytes"] + logs["stderr_discarded_bytes"]})
        return outcome, evidence


def run(args: argparse.Namespace) -> dict[str, Any]:
    loaded = load_profile(args.worklist.resolve(), "fuzz_qualifications", "fuzz")
    try:
        return _run_loaded(args, loaded)
    finally:
        loaded.close()


def _run_loaded(args: argparse.Namespace, loaded: Any) -> dict[str, Any]:
    worklist, qualifications = loaded.worklist, loaded.entries
    if len(qualifications) != 1: raise RobustnessError(f"expected exactly one fuzz qualification, found {len(qualifications)}")
    bounds = qualification_budget(qualifications[0], args); identity = viewer_identity(args.binary)
    healthy = args.healthy_file.resolve()
    if not healthy.is_file(): raise RobustnessError(f"healthy recovery file does not exist: {healthy}")
    seed_paths = [path.resolve() for path in args.seed_file]
    if not seed_paths or any(not path.is_file() for path in seed_paths): raise RobustnessError("every --seed-file must identify an existing file")
    viewer_root = Path(__file__).resolve().parents[2]; deadline = time.monotonic() + args.max_duration
    results: list[dict[str, Any]] = []; retained: list[tuple[str, bytes]] = []; total_operations = total_mutations = 0
    per_seed = max(1, args.max_candidates // len(seed_paths))
    stop_reason = "candidate_limit"
    for seed_index, seed_path in enumerate(seed_paths):
        seed_bytes = seed_path.read_bytes(); remaining = args.max_candidates - len(results)
        for local_iteration, (payload, mutations) in enumerate(candidates(seed_bytes, args.run_seed ^ seed_index, min(per_seed, remaining), args.max_mutations_per_candidate, args.max_input_bytes)):
            if time.monotonic() >= deadline: stop_reason = "duration_limit"; break
            operations = len(payload)
            if total_operations + operations > args.max_total_target_operations: stop_reason = "operation_limit"; break
            total_operations += operations; total_mutations += mutations
            outcome, evidence = exercise(payload, args.binary, healthy, viewer_root, args)
            digest = hashlib.sha256(payload).hexdigest(); unacceptable = outcome in {"crash", "hang", "timeout", "recovery_failure", "resource_limit"}
            result = {"seed": str(seed_path), "iteration": local_iteration, "sha256": digest, "size_bytes": len(payload), "mutations": mutations, "target_operations": operations, "outcome": outcome, "unacceptable": unacceptable, "evidence": evidence, "retained": False}
            if unacceptable and len(retained) < args.max_retained_artifacts and sum(len(item[1]) for item in retained) + len(payload) <= args.max_retained_bytes:
                retained.append((digest, payload)); result["retained"] = True
            results.append(result)
        if stop_reason != "candidate_limit" or len(results) >= args.max_candidates: break
    unacceptable_count = sum(row["unacceptable"] for row in results)
    report = {
        "robustness_report_version": "1.0.0", "runner": "fuzz", "generated_at": utc_now(), "viewer": identity,
        "worklist": {"path": str(args.worklist.resolve()), "content_sha256": worklist["worklist_sha256"]},
        "upstream_qualification": qualifications[0], "target": {"kind": "dcmview_process_and_http_adapter", "independence": "dcmview_specific"},
        "bounds": {**bounds, "max_duration_seconds": args.max_duration, "target_timeout_seconds": args.target_timeout, "max_output_bytes": args.max_output_bytes, "max_response_bytes": args.max_response_bytes, "max_retained_artifacts": args.max_retained_artifacts, "max_retained_bytes": args.max_retained_bytes},
        "counters": {"candidates": len(results), "mutations": total_mutations, "target_operations": total_operations, "retained_artifacts": len(retained), "retained_bytes": sum(len(item[1]) for item in retained)},
        "stop_reason": stop_reason, "payload_policy": "retain_bounded_unacceptable_only", "results": results,
        "assertions": {"fuzz_qualification_bounded": bool(results) and unacceptable_count == 0},
        "summary": {"unacceptable": unacceptable_count, "outcomes": {name: sum(row["outcome"] == name for row in results) for name in sorted({row["outcome"] for row in results})}},
    }
    write_report(args.output, report)
    if retained:
        failure_dir = args.output.resolve() / "retained-failures"; failure_dir.mkdir()
        for digest, payload in retained: (failure_dir / f"{digest}.dcm").write_bytes(payload)
    return report


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worklist", type=Path, required=True); parser.add_argument("--binary", type=Path, required=True); parser.add_argument("--healthy-file", type=Path, required=True); parser.add_argument("--seed-file", type=Path, action="append", required=True); parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--run-seed", type=int, default=1); parser.add_argument("--max-candidates", type=int, default=64); parser.add_argument("--max-mutations-per-candidate", type=int, default=8); parser.add_argument("--max-input-bytes", type=int, default=8_388_608); parser.add_argument("--max-total-target-operations", type=int, default=100_000_000)
    parser.add_argument("--max-duration", type=float, default=600.0); parser.add_argument("--target-timeout", type=float, default=10.0); parser.add_argument("--request-timeout", type=float, default=5.0); parser.add_argument("--shutdown-timeout", type=float, default=5.0); parser.add_argument("--max-output-bytes", type=int, default=1_048_576); parser.add_argument("--max-response-bytes", type=int, default=8_388_608); parser.add_argument("--max-retained-artifacts", type=int, default=8); parser.add_argument("--max-retained-bytes", type=int, default=16_777_216)
    args = parser.parse_args(argv)
    for name in ("max_candidates", "max_mutations_per_candidate", "max_input_bytes", "max_total_target_operations", "max_retained_bytes"):
        if getattr(args, name) <= 0: parser.error(f"--{name.replace('_', '-')} must be positive")
    if args.max_retained_artifacts < 0: parser.error("--max-retained-artifacts cannot be negative")
    return args


def main(argv: list[str] | None = None) -> int:
    try: report = run(parse_args(sys.argv[1:] if argv is None else argv))
    except (RobustnessError, OSError, ValueError) as error: print(f"fuzz runner error: {error}", file=sys.stderr); return 2
    print(json.dumps({"candidates": report["counters"]["candidates"], **report["summary"]}, sort_keys=True)); return 0 if report["assertions"]["fuzz_qualification_bounded"] else 1


if __name__ == "__main__": raise SystemExit(main())

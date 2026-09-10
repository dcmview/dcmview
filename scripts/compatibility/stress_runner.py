#!/usr/bin/env python3
"""Record dcmview baselines for the stress worklist without invented limits."""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import json
import sys
import time
from pathlib import Path
from typing import Any

try:
    from scripts.compatibility.robustness import BoundedProcess, PeakRssSampler, RobustnessError, bounded_get, load_profile, poll_catalog, utc_now, viewer_identity, write_report
except ModuleNotFoundError:
    from robustness import BoundedProcess, PeakRssSampler, RobustnessError, bounded_get, load_profile, poll_catalog, utc_now, viewer_identity, write_report  # type: ignore[no-redef]


def deterministic_frames(case_id: str, count: int) -> list[int]:
    if count <= 0: return []
    candidates = [0, count // 2, count - 1, int.from_bytes(hashlib.sha256(case_id.encode()).digest()[:8], "big") % count]
    return list(dict.fromkeys(candidates))


def cache_pressure_frames(count: int, limit: int) -> list[int]:
    if count <= 0: return []
    selected = min(count, limit)
    if selected == 1: return [0]
    return [(ordinal * (count - 1)) // (selected - 1) for ordinal in range(selected)]


def request_frame(base_url: str, index: int, frame: int, args: argparse.Namespace) -> dict[str, Any]:
    return bounded_get(base_url, f"/api/file/{index}/frame/{frame}", args.request_timeout, args.max_response_bytes)


def cancellation_probe(
    process: BoundedProcess, args: argparse.Namespace
) -> dict[str, Any]:
    started = time.monotonic()
    observed_incomplete = False
    catalog_status = None
    error = None
    try:
        base_url = process.wait_for_url(args.startup_timeout)
        catalog = bounded_get(
            base_url, "/api/files", args.request_timeout, args.max_response_bytes
        )
        catalog_status = catalog["status"]
        observed_incomplete = (
            catalog["status"] == 200
            and isinstance(catalog.get("json"), dict)
            and catalog["json"].get("scan_complete") is False
        )
        if not observed_incomplete:
            error = "discovery completed before cancellation state was observed"
    except Exception as caught:
        error = f"{type(caught).__name__}: {caught}"
    shutdown = process.shutdown(args.shutdown_timeout)
    return {
        **shutdown,
        "startup_observed": catalog_status is not None,
        "catalog_status": catalog_status,
        "scan_incomplete_observed": observed_incomplete,
        "requested_during_discovery": observed_incomplete,
        "error": error,
        "total_elapsed_ms": round((time.monotonic() - started) * 1000, 3),
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    loaded = load_profile(args.worklist.resolve(), "stress_files", "stress")
    try:
        return _run_loaded(args, loaded)
    finally:
        loaded.close()


def _run_loaded(args: argparse.Namespace, loaded: Any) -> dict[str, Any]:
    worklist, entries = loaded.worklist, loaded.entries
    scenarios = worklist["models"].get("stress_scenarios", [])
    if not entries: raise RobustnessError("stress_files model is empty")
    identity = viewer_identity(args.binary); viewer_root = Path(__file__).resolve().parents[2]
    command = [identity["binary"], "--no-browser", "--host", "127.0.0.1", "--port", "0", "--startup-json", *[row["normalized_path"] for row in entries]]
    started_at = utc_now(); started = time.monotonic()
    process = BoundedProcess(command, viewer_root, args.max_output_bytes); rss = PeakRssSampler(process.process.pid); rss.start()
    results: list[dict[str, Any]] = []; error = None; startup_ms = discovery_ms = None
    try:
        base_url = process.wait_for_url(args.startup_timeout); startup_ms = round((time.monotonic() - started) * 1000, 3)
        discovery_started = time.monotonic(); catalog = poll_catalog(base_url, args.discovery_timeout, args.request_timeout, args.max_response_bytes)
        discovery_ms = round((time.monotonic() - discovery_started) * 1000, 3)
        by_path = {str(Path(row["path"]).resolve()): row for row in catalog["json"]["files"]}
        for entry in entries:
            row = by_path.get(str(Path(entry["normalized_path"]).resolve()))
            observation: dict[str, Any] = {"case_id": entry["case_id"], "path": entry["normalized_path"], "discovered": row is not None}
            if row is None:
                observation.update({"passed": False, "error": "discovery_omission"}); results.append(observation); continue
            frames = deterministic_frames(entry["case_id"], int(row.get("frame_count", 0))) if row.get("has_pixels") else []
            sequential = [request_frame(base_url, row["index"], frame, args) for frame in frames]
            cache_first = request_frame(base_url, row["index"], 0, args) if frames else None
            pressure_frames = cache_pressure_frames(int(row.get("frame_count", 0)), args.cache_probe_frames) if frames else []
            pressure = [request_frame(base_url, row["index"], frame, args) for frame in pressure_frames]
            cache_after_pressure = request_frame(base_url, row["index"], 0, args) if frames else None
            same: list[dict[str, Any]] = []; different: list[dict[str, Any]] = []
            if frames:
                with concurrent.futures.ThreadPoolExecutor(max_workers=args.concurrency) as pool:
                    same = list(pool.map(lambda _: request_frame(base_url, row["index"], frames[0], args), range(args.concurrency)))
                    different = list(pool.map(lambda frame: request_frame(base_url, row["index"], frame, args), (frames * args.concurrency)[:args.concurrency]))
            invalid = bounded_get(base_url, f"/api/file/{row['index']}/frame/{max(1, int(row.get('frame_count', 0)) + 1)}", args.request_timeout, args.max_response_bytes)
            recovery = bounded_get(base_url, f"/api/file/{row['index']}/tags", args.request_timeout, args.max_response_bytes)
            statuses = [response["status"] for response in sequential + pressure + same + different]
            observation.update({
                "frame_selection": frames, "first_and_random_frames": sequential,
                "cache_repeat": cache_first, "cache_pressure_frames": pressure_frames, "cache_pressure": pressure,
                "cache_after_pressure": cache_after_pressure, "concurrent_same_frame": same, "concurrent_different_frames": different,
                "error_probe": invalid, "recovery_probe": recovery,
                "cache_observed": sorted({response["headers"].get("x-cache") for response in sequential + pressure + ([cache_first, cache_after_pressure] if cache_first and cache_after_pressure else []) if response["headers"].get("x-cache")}),
                "passed": all(status == 200 for status in statuses) and recovery["status"] == 200 and invalid["status"] >= 400,
            }); results.append(observation)
    except Exception as caught:
        error = f"{type(caught).__name__}: {caught}"
    finally:
        peak_rss = rss.stop(); shutdown = process.shutdown(args.shutdown_timeout); logs = process.logs()
    cancellation_process = BoundedProcess(command, viewer_root, args.max_output_bytes)
    cancellation = cancellation_probe(cancellation_process, args)
    measurements = {
        "startup_ms": startup_ms, "discovery_ms": discovery_ms, "peak_rss_bytes": peak_rss,
        "rss_supported": peak_rss is not None, "shutdown": shutdown, "cancellation": cancellation,
        "total_ms": round((time.monotonic() - started) * 1000, 3),
        "output_discarded_bytes": logs["stdout_discarded_bytes"] + logs["stderr_discarded_bytes"],
    }
    complete = error is None and len(results) == len(entries) and all(row["passed"] for row in results) and not shutdown["forced"]
    report = {
        "robustness_report_version": "1.0.0", "runner": "stress", "generated_at": utc_now(), "viewer": identity,
        "worklist": {"path": str(args.worklist.resolve()), "content_sha256": worklist["worklist_sha256"]},
        "bounds": {name: getattr(args, name) for name in ("startup_timeout", "discovery_timeout", "request_timeout", "shutdown_timeout", "max_output_bytes", "max_response_bytes", "concurrency", "cache_probe_frames")},
        "threshold_policy": "recording_baselines_only", "qualification_contracts": scenarios,
        "measurements": measurements, "results": results, "campaign_error": error,
        "assertions": {"stress_bounded_execution": complete and cancellation["scan_incomplete_observed"] and not cancellation["forced"], "stress_resource_measurements": startup_ms is not None and discovery_ms is not None, "server_recovers_after_error": bool(results) and all(row.get("recovery_probe", {}).get("status") == 200 for row in results)},
    }
    write_report(args.output, report); return report


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worklist", type=Path, required=True); parser.add_argument("--binary", type=Path, required=True); parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--startup-timeout", type=float, default=30.0); parser.add_argument("--discovery-timeout", type=float, default=120.0); parser.add_argument("--request-timeout", type=float, default=30.0); parser.add_argument("--shutdown-timeout", type=float, default=10.0)
    parser.add_argument("--max-output-bytes", type=int, default=4_194_304); parser.add_argument("--max-response-bytes", type=int, default=134_217_728); parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--cache-probe-frames", type=int, default=16)
    args = parser.parse_args(argv)
    if args.concurrency < 1 or args.concurrency > 32: parser.error("--concurrency must be between 1 and 32")
    if args.cache_probe_frames < 1 or args.cache_probe_frames > 256: parser.error("--cache-probe-frames must be between 1 and 256")
    return args


def main(argv: list[str] | None = None) -> int:
    try: report = run(parse_args(sys.argv[1:] if argv is None else argv))
    except (RobustnessError, OSError, ValueError) as error: print(f"stress runner error: {error}", file=sys.stderr); return 2
    print(json.dumps({"selected": len(report["results"]), "passed": all(report["assertions"].values())}, sort_keys=True)); return 0 if all(report["assertions"].values()) else 1


if __name__ == "__main__": raise SystemExit(main())

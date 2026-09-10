#!/usr/bin/env python3
"""Bounded process and HTTP primitives for opt-in robustness runners."""

from __future__ import annotations

import hashlib
import json
import os
import signal
import subprocess
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, BinaryIO

try:
    from scripts.compatibility.worklist import CompatibilityError, ValidatedWorklist, load_worklist, sha256_file
except ModuleNotFoundError:
    from worklist import CompatibilityError, ValidatedWorklist, load_worklist, sha256_file  # type: ignore[no-redef]


class RobustnessError(RuntimeError):
    """Raised when a runner cannot preserve its safety contract."""


class LoadedProfile:
    """Hold validated rows and their private payload tree for one runner call."""

    def __init__(self, worklist: ValidatedWorklist, entries: list[dict[str, Any]]) -> None:
        self.worklist = worklist
        self.entries = entries
        self._closed = False

    def close(self) -> None:
        if not self._closed:
            self._closed = True
            self.worklist.close()

    def __enter__(self) -> "LoadedProfile":
        if self._closed:
            raise RuntimeError("loaded profile lifetime is already closed")
        return self

    def __exit__(self, _type: object, _value: object, _traceback: object) -> None:
        self.close()

    def __iter__(self):
        """Keep tuple unpacking readable for callers while exposing close()."""

        yield self.worklist
        yield self.entries


def utc_now() -> str:
    from datetime import datetime, timezone

    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


class BoundedProcess:
    """Drain a process without retaining more than the declared byte budget."""

    def __init__(self, command: list[str], cwd: Path, max_output_bytes: int):
        if max_output_bytes <= 0:
            raise ValueError("max_output_bytes must be positive")
        environment = dict(os.environ)
        environment["DCMVIEW_VSCODE_BYPASS"] = "1"
        self.process = subprocess.Popen(
            command, cwd=cwd, env=environment, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, start_new_session=True, bufsize=0,
        )
        self.command = command
        self._limit = max_output_bytes
        self._stdout = bytearray()
        self._stderr = bytearray()
        self._discarded = {"stdout": 0, "stderr": 0}
        self._condition = threading.Condition()
        assert self.process.stdout is not None and self.process.stderr is not None
        self._threads = [
            threading.Thread(target=self._drain, args=(self.process.stdout, self._stdout, "stdout"), daemon=True),
            threading.Thread(target=self._drain, args=(self.process.stderr, self._stderr, "stderr"), daemon=True),
        ]
        for thread in self._threads:
            thread.start()

    def _drain(self, stream: BinaryIO, target: bytearray, name: str) -> None:
        while chunk := stream.read(8192):
            with self._condition:
                remaining = max(0, self._limit - len(target))
                target.extend(chunk[:remaining])
                self._discarded[name] += max(0, len(chunk) - remaining)
                self._condition.notify_all()

    def wait_for_url(self, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._condition:
                for line in bytes(self._stdout).splitlines():
                    try:
                        event = json.loads(line)
                    except (UnicodeDecodeError, json.JSONDecodeError):
                        continue
                    if event.get("type") == "server_started" and isinstance(event.get("url"), str):
                        return event["url"].rstrip("/")
                if self.process.poll() is not None:
                    raise RobustnessError(f"viewer exited before startup: {self.process.returncode}")
                self._condition.wait(min(0.05, max(0.0, deadline - time.monotonic())))
        raise TimeoutError(f"viewer startup exceeded {timeout:.3f}s")

    def shutdown(self, timeout: float) -> dict[str, Any]:
        started = time.monotonic()
        forced = False
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                forced = True
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=max(1.0, timeout))
        for thread in self._threads:
            thread.join(timeout=1.0)
        return {
            "exit_code": self.process.returncode,
            "forced": forced,
            "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
        }

    def logs(self) -> dict[str, Any]:
        return {
            "stdout": bytes(self._stdout), "stderr": bytes(self._stderr),
            "stdout_discarded_bytes": self._discarded["stdout"],
            "stderr_discarded_bytes": self._discarded["stderr"],
        }


class PeakRssSampler:
    """Best-effort process RSS sampler; unsupported platforms report null."""

    def __init__(self, pid: int, interval: float = 0.05):
        self.pid = pid
        self.interval = interval
        self.peak_bytes: int | None = None
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._sample, daemon=True)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> int | None:
        self._stop.set(); self._thread.join(timeout=1.0); return self.peak_bytes

    def _read(self) -> int | None:
        status = Path(f"/proc/{self.pid}/status")
        if status.is_file():
            for line in status.read_text(encoding="utf-8", errors="replace").splitlines():
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
        try:
            value = subprocess.run(
                ["ps", "-o", "rss=", "-p", str(self.pid)], capture_output=True,
                text=True, timeout=1, check=False,
            ).stdout.strip()
            return int(value) * 1024 if value else None
        except (OSError, ValueError, subprocess.SubprocessError):
            return None

    def _sample(self) -> None:
        while not self._stop.is_set():
            value = self._read()
            if value is not None:
                self.peak_bytes = max(self.peak_bytes or 0, value)
            self._stop.wait(self.interval)

def bounded_get(base_url: str, path: str, timeout: float, max_body_bytes: int) -> dict[str, Any]:
    started = time.monotonic()
    request = urllib.request.Request(f"{base_url}{path}", method="GET")
    try:
        response = urllib.request.urlopen(request, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        body = response.read(max_body_bytes + 1)
        status = response.status
        headers = {key.lower(): value for key, value in response.headers.items()}
    truncated = len(body) > max_body_bytes
    body = body[:max_body_bytes]
    parsed = None
    if headers.get("content-type", "").split(";", 1)[0] == "application/json" and not truncated:
        try:
            parsed = json.loads(body)
        except (UnicodeDecodeError, json.JSONDecodeError):
            pass
    return {
        "path": path, "status": status, "headers": headers, "json": parsed,
        "body_sha256": hashlib.sha256(body).hexdigest(), "body_bytes": len(body),
        "body_truncated": truncated,
        "elapsed_ms": round((time.monotonic() - started) * 1000, 3),
    }


def poll_catalog(base_url: str, timeout: float, request_timeout: float, max_body_bytes: int) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        response = bounded_get(base_url, "/api/files", request_timeout, max_body_bytes)
        if response["status"] == 200 and isinstance(response["json"], dict) and response["json"].get("scan_complete") is True:
            return response
        time.sleep(0.05)
    raise TimeoutError(f"discovery exceeded {timeout:.3f}s")


def load_profile(path: Path, model: str, profile: str) -> LoadedProfile:
    try:
        worklist = load_worklist(path, stage_payloads=profile in {"negative", "stress"})
    except CompatibilityError as error:
        raise RobustnessError(f"invalid {profile} robustness worklist: {error}") from error
    try:
        selected = worklist["inputs"]["profiles"][0]
        if selected != profile:
            raise RobustnessError(
                f"worklist profile {selected!r} is not supported by the {profile} runner"
            )
        rows = worklist.staged_models.get(model, worklist["models"].get(model))
        if not isinstance(rows, list):
            raise RobustnessError(f"validated worklist model {model!r} is absent")
        return LoadedProfile(worklist, rows)
    except Exception:
        worklist.close()
        raise


def viewer_identity(binary: Path) -> dict[str, Any]:
    binary = binary.resolve()
    if not os.access(binary, os.X_OK):
        raise RobustnessError(f"viewer binary is not executable: {binary}")
    version = subprocess.run([str(binary), "--version"], check=True, capture_output=True, text=True, timeout=5).stdout.strip()
    return {"binary": str(binary), "sha256": sha256_file(binary), "version": version}


def write_report(output: Path, report: dict[str, Any]) -> Path:
    output = output.resolve()
    if output.exists() and any(output.iterdir()):
        raise RobustnessError(f"artifact output is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)
    path = output / "report.json"
    path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path

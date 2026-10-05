#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

STARTUP_PREFIX = "dcmview: server running at "


class OutputMonitor:
	def __init__(self, process: subprocess.Popen[str]) -> None:
		self.process = process
		self.lines: list[str] = []
		self.url: str | None = None
		self._ready = threading.Event()
		self._thread = threading.Thread(target=self._run, daemon=True)

	def start(self) -> None:
		self._thread.start()

	def wait_for_url(self, timeout: float) -> str:
		self._ready.wait(timeout)
		if self.url is None:
			raise RuntimeError(
				f"dcmview did not print a startup URL within {timeout:.1f}s.\n"
				+ "".join(self.lines[-20:])
			)
		return self.url

	def join(self) -> None:
		self._thread.join(timeout=5)

	def _run(self) -> None:
		assert self.process.stdout is not None
		for line in self.process.stdout:
			self.lines.append(line)
			if line.startswith(STARTUP_PREFIX) and self.url is None:
				self.url = line[len(STARTUP_PREFIX) :].strip()
				self._ready.set()
		self._ready.set()


def api_request(url: str, path: str) -> urllib.request.Request:
	base_url, fragment = urllib.parse.urldefrag(url)
	token = urllib.parse.parse_qs(fragment).get("token", [""])[0]
	return urllib.request.Request(
		f"{base_url.rstrip('/')}{path}",
		headers={"Authorization": f"Bearer {token}"},
	)


def get_json(url: str, path: str) -> dict:
	with urllib.request.urlopen(api_request(url, path), timeout=10) as response:
		return json.load(response)


def wait_for_files(base_url: str, expected_count: int, timeout: float) -> dict:
	deadline = time.monotonic() + timeout
	last_payload: dict | None = None
	last_error: Exception | None = None

	while time.monotonic() < deadline:
		try:
			payload = get_json(base_url, "/api/files")
			entries = payload.get("files", [])
			last_payload = payload
			last_error = None

			if len(entries) == expected_count:
				return payload
			if payload.get("scan_complete"):
				raise AssertionError(
					f"expected {expected_count} files, got {len(entries)} after scan completed "
					f"(scanned={payload.get('scanned')}, skipped={payload.get('skipped')}, "
					f"filtered={payload.get('filtered')})"
				)
		except urllib.error.URLError as error:
			last_error = error

		time.sleep(0.1)

	if last_payload is not None:
		entries = last_payload.get("files", [])
		raise RuntimeError(
			f"timed out waiting for {expected_count} files; last response had {len(entries)} "
			f"(scan_complete={last_payload.get('scan_complete')}, "
			f"scanned={last_payload.get('scanned')}, skipped={last_payload.get('skipped')}, "
			f"filtered={last_payload.get('filtered')})"
		)
	if last_error is not None:
		raise RuntimeError(f"timed out waiting for /api/files: {last_error}")
	raise RuntimeError("timed out waiting for /api/files")


def get_response(url: str, path: str) -> tuple[int, dict[str, str], bytes]:
	request = api_request(url, path)
	try:
		with urllib.request.urlopen(request, timeout=10) as response:
			return response.status, dict(response.headers.items()), response.read()
	except urllib.error.HTTPError as error:
		return error.code, dict(error.headers.items()), error.read()


def expect(condition: bool, message: str) -> None:
	if not condition:
		raise AssertionError(message)


def is_windows() -> bool:
	return os.name == "nt"


def popen_options() -> dict[str, object]:
	options: dict[str, object] = {
		"stdout": subprocess.PIPE,
		"stderr": subprocess.STDOUT,
		"text": True,
		"bufsize": 1,
	}
	if is_windows():
		options["creationflags"] = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
	return options


def graceful_stop_signal() -> signal.Signals | int:
	if is_windows():
		return getattr(signal, "CTRL_BREAK_EVENT", signal.SIGTERM)
	return signal.SIGINT


def main() -> int:
	if len(sys.argv) != 4:
		print(
			"usage: smoke_release_binary.py <binary> <pixel_fixture> <no_pixel_fixture>",
			file=sys.stderr,
		)
		return 2

	binary = Path(sys.argv[1]).resolve()
	pixel_fixture = Path(sys.argv[2]).resolve()
	no_pixel_fixture = Path(sys.argv[3]).resolve()

	process = subprocess.Popen(
		[
			str(binary),
			"--no-browser",
			"--timeout",
			"30",
			str(pixel_fixture),
			str(no_pixel_fixture),
		],
		**popen_options(),
	)
	monitor = OutputMonitor(process)
	monitor.start()

	try:
		base_url = monitor.wait_for_url(20.0)
		files = wait_for_files(base_url, 2, 20.0)
		entries = files["files"]
		expect(len(entries) == 2, f"expected 2 files, got {len(entries)}")

		pixel_index = next(
			entry["index"]
			for entry in entries
			if Path(entry["path"]).name == pixel_fixture.name
		)
		no_pixel_index = next(
			entry["index"]
			for entry in entries
			if Path(entry["path"]).name == no_pixel_fixture.name
		)

		status, headers, body = get_response(base_url, f"/api/file/{pixel_index}/frame/0")
		expect(status == 200, f"display frame returned status {status}")
		expect(headers.get("content-type") == "image/png", "display frame must be PNG")
		expect(headers.get("x-cache") == "MISS", "first display request must be a cache MISS")
		expect(body.startswith(b"\x89PNG\r\n\x1a\n"), "display frame must be a PNG file")

		status, headers, _ = get_response(base_url, f"/api/file/{pixel_index}/frame/0")
		expect(status == 200, f"repeat display frame returned status {status}")
		expect(headers.get("x-cache") == "HIT", "repeat display request must be a cache HIT")

		status, _, _ = get_response(base_url, f"/api/file/{no_pixel_index}/frame/0")
		expect(status == 404, f"no-pixel fixture should return 404, got {status}")
		return 0
	finally:
		graceful_signal_sent = False
		if process.poll() is None:
			try:
				process.send_signal(graceful_stop_signal())
				graceful_signal_sent = True
			except (ProcessLookupError, ValueError):
				pass
		try:
			return_code = process.wait(timeout=10)
			if graceful_signal_sent:
				expect(return_code == 0, f"graceful shutdown returned exit code {return_code}")
		except subprocess.TimeoutExpired:
			process.terminate()
			try:
				process.wait(timeout=5)
			except subprocess.TimeoutExpired:
				process.kill()
				process.wait(timeout=10)
			if graceful_signal_sent:
				raise RuntimeError("dcmview did not exit after graceful shutdown signal")
		monitor.join()


if __name__ == "__main__":
	raise SystemExit(main())

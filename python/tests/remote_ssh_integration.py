"""The remote-server workflow, over real SSH, against the shipped Linux wheel.

A container plays the remote server: sshd, Python and a normal user, with no
dcmview until this suite copies the wheel in. The client reaches it only with
the real ``ssh`` client, the way a user does: launch over SSH, forward what
the printed hint says, and open the printed link in a browser.

Run it through ``python scripts/check.py remote-ssh``. It needs Docker, an
``ssh`` client, Python Playwright with Chromium, and ``DCMVIEW_REMOTE_WHEEL``
naming a manylinux wheel. ``DCMVIEW_REMOTE_BASE_IMAGE`` overrides the
container's base image (default Ubuntu 22.04, the oldest glibc we promise).
"""

from __future__ import annotations

import json
import os
import queue
import re
import shlex
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator, Optional

REPO_ROOT = Path(__file__).resolve().parents[2]
IMAGE_DIR = REPO_ROOT / "tests" / "remote"
FIXTURE_FILE = REPO_ROOT / "tests" / "fixtures" / "golden-uncompressed-u16-multiframe.dcm"
REMOTE_HOME = "/home/annot"
REMOTE_FIXTURE = f"{REMOTE_HOME}/study/fixture.dcm"
REMOTE_BINARY = f"{REMOTE_HOME}/bin/dcmview"
REMOTE_VENV = f"{REMOTE_HOME}/venv"
SOCKET_DIR = f"{REMOTE_HOME}/dcmview-sockets"
STARTUP_SECONDS = 30.0
PAINT_SECONDS = 20.0

OPEN_LINE = re.compile(r"then open (http://localhost:(\d+)/#token=([A-Za-z0-9._~-]+))")
TCP_HINT = re.compile(r"ssh -L (\d+):localhost:(\d+) user@host")
SOCKET_HINT = re.compile(r"ssh -L (\d+):(/\S+) user@host")


def free_local_port() -> int:
	with socket.socket() as probe:
		probe.bind(("127.0.0.1", 0))
		return probe.getsockname()[1]


def http_status(url: str, token: Optional[str] = None) -> int:
	headers = {"Authorization": f"Bearer {token}"} if token else {}
	try:
		with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=10) as response:
			return response.status
	except urllib.error.HTTPError as error:
		return error.code


def first_frame_paints(url: str) -> bool:
	"""Open ``url`` in headless Chromium and wait for a canvas with image pixels."""
	from playwright.sync_api import TimeoutError as PlaywrightTimeout
	from playwright.sync_api import sync_playwright

	with sync_playwright() as playwright:
		browser = playwright.chromium.launch()
		try:
			page = browser.new_page()
			page.goto(url)
			try:
				page.wait_for_function(
					"""() => [...document.querySelectorAll('canvas')].some((canvas) => {
						if (!canvas.width || !canvas.height) return false;
						const pixels = canvas.getContext('2d')?.getImageData(0, 0, canvas.width, canvas.height).data;
						return !!pixels && pixels.some((value, index) => index % 4 !== 3 && value > 0);
					})""",
					timeout=PAINT_SECONDS * 1000,
				)
			except PlaywrightTimeout:
				return False
			return True
		finally:
			browser.close()


class RemoteProcess:
	"""One command running on the remote over its own SSH session."""

	def __init__(self, ssh: list[str], command: str, *, tty: bool) -> None:
		self.process = subprocess.Popen(
			[*ssh, *(["-tt"] if tty else []), command],
			stdin=subprocess.PIPE,
			stdout=subprocess.PIPE,
			stderr=subprocess.STDOUT,
		)
		self.lines: list[str] = []
		self._queue: queue.Queue[Optional[str]] = queue.Queue()
		self._reader = threading.Thread(target=self._read, daemon=True)
		self._reader.start()

	def _read(self) -> None:
		assert self.process.stdout is not None
		for raw in self.process.stdout:
			self._queue.put(raw.decode("utf-8", "replace").rstrip("\r\n"))
		self._queue.put(None)

	def wait_for(self, pattern: re.Pattern[str], timeout: float = STARTUP_SECONDS) -> re.Match[str]:
		for line in self.lines:
			if match := pattern.search(line):
				return match
		deadline = time.monotonic() + timeout
		while (remaining := deadline - time.monotonic()) > 0:
			try:
				line = self._queue.get(timeout=remaining)
			except queue.Empty:
				break
			if line is None:
				break
			self.lines.append(line)
			if match := pattern.search(line):
				return match
		raise AssertionError(f"no line matched {pattern.pattern!r}; output so far:\n" + "\n".join(self.lines))

	def send(self, data: bytes) -> None:
		assert self.process.stdin is not None
		self.process.stdin.write(data)
		self.process.stdin.flush()

	def close(self) -> None:
		if self.process.poll() is None:
			self.process.kill()
		self.process.wait(timeout=10)
		self._reader.join(timeout=10)
		for stream in (self.process.stdin, self.process.stdout):
			if stream is not None:
				stream.close()


class RemoteHost:
	"""A throwaway SSH server container holding the wheel under test."""

	def __init__(self, wheel: Path) -> None:
		self.wheel = wheel
		self.workdir = Path(tempfile.mkdtemp(prefix="dcmview-remote-ssh-"))
		self.key = self.workdir / "id_ed25519"
		self.container: Optional[str] = None
		self.port = 0

	def start(self) -> None:
		base = os.environ.get("DCMVIEW_REMOTE_BASE_IMAGE", "ubuntu:22.04")
		tag = "dcmview-remote-ssh:" + re.sub(r"[^A-Za-z0-9_.-]", "-", base)
		subprocess.run(
			["docker", "build", "--quiet", "--build-arg", f"BASE_IMAGE={base}", "--tag", tag, str(IMAGE_DIR)],
			check=True,
			stdout=subprocess.DEVNULL,
		)
		subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(self.key)], check=True)
		self.container = subprocess.run(
			["docker", "run", "--detach", "--rm", "--publish", "127.0.0.1::22", tag],
			check=True,
			capture_output=True,
			text=True,
		).stdout.strip()
		published = subprocess.run(
			["docker", "port", self.container, "22/tcp"], check=True, capture_output=True, text=True
		).stdout.splitlines()[0]
		self.port = int(published.rsplit(":", 1)[1])

		self.copy(self.key.with_suffix(".pub"), f"{REMOTE_HOME}/.ssh/authorized_keys")
		self.copy(self.wheel, f"/tmp/{self.wheel.name}")
		self.copy(FIXTURE_FILE, REMOTE_FIXTURE)
		self._wait_for_ssh()
		# The wheel's bundled binary is the same build as the Linux release
		# archive's, so it also stands in for a plain binary install.
		with zipfile.ZipFile(self.wheel) as archive:
			binary = self.workdir / "dcmview"
			binary.write_bytes(archive.read("dcmview_py/bin/dcmview"))
		self.copy(binary, REMOTE_BINARY)
		self.run(
			f"chmod +x {REMOTE_BINARY} && python3 -m venv {REMOTE_VENV} "
			f"&& {REMOTE_VENV}/bin/pip install --quiet --no-index /tmp/{self.wheel.name}"
		)

	def copy(self, source: Path, destination: str) -> None:
		assert self.container is not None
		subprocess.run(
			["docker", "exec", self.container, "install", "-d", "-o", "annot", "-g", "annot", str(Path(destination).parent)],
			check=True,
		)
		subprocess.run(["docker", "cp", str(source), f"{self.container}:{destination}"], check=True)
		subprocess.run(["docker", "exec", self.container, "chown", "annot:annot", destination], check=True)

	def _wait_for_ssh(self) -> None:
		deadline = time.monotonic() + 30
		while True:
			result = subprocess.run([*self.ssh, "true"], capture_output=True)
			if result.returncode == 0:
				return
			if time.monotonic() > deadline:
				raise AssertionError(f"sshd never accepted the key: {result.stderr.decode(errors='replace')}")
			time.sleep(0.5)

	@property
	def ssh(self) -> list[str]:
		return [
			"ssh",
			"-i", str(self.key),
			"-p", str(self.port),
			"-o", "BatchMode=yes",
			"-o", "StrictHostKeyChecking=no",
			"-o", "UserKnownHostsFile=/dev/null",
			"-o", "LogLevel=ERROR",
			"-o", "ExitOnForwardFailure=yes",
			"annot@127.0.0.1",
		]

	def run(self, command: str) -> str:
		return subprocess.run([*self.ssh, command], check=True, capture_output=True, text=True).stdout

	def launch(self, command: str, *, tty: bool = False) -> RemoteProcess:
		return RemoteProcess(self.ssh, command, tty=tty)

	def dcmview_running(self) -> bool:
		return bool(self.run("pgrep -u annot -x dcmview || true").strip())

	def wait_until_stopped(self, timeout: float = 15) -> bool:
		deadline = time.monotonic() + timeout
		while self.dcmview_running():
			if time.monotonic() > deadline:
				return False
			time.sleep(0.25)
		return True

	@contextmanager
	def forward(self, remote_target: str) -> Iterator[int]:
		"""``ssh -L`` a free local port to ``remote_target`` (``host:port`` or a socket path)."""
		local_port = free_local_port()
		tunnel = subprocess.Popen([*self.ssh, "-N", "-L", f"{local_port}:{remote_target}"])
		try:
			deadline = time.monotonic() + 15
			while True:
				try:
					socket.create_connection(("127.0.0.1", local_port), timeout=1).close()
					break
				except OSError:
					if tunnel.poll() is not None or time.monotonic() > deadline:
						raise AssertionError(f"ssh -L {local_port}:{remote_target} did not come up")
					time.sleep(0.2)
			yield local_port
		finally:
			tunnel.terminate()
			tunnel.wait(timeout=10)

	def stop(self) -> None:
		if self.container:
			subprocess.run(["docker", "rm", "--force", self.container], capture_output=True)
		shutil.rmtree(self.workdir, ignore_errors=True)


class RemoteSshWorkflowTests(unittest.TestCase):
	host: RemoteHost

	@classmethod
	def setUpClass(cls) -> None:
		wheel = os.environ.get("DCMVIEW_REMOTE_WHEEL")
		if not wheel:
			raise unittest.SkipTest("DCMVIEW_REMOTE_WHEEL is not set; run through scripts/check.py remote-ssh")
		cls.host = RemoteHost(Path(wheel).resolve())
		try:
			cls.host.start()
		except BaseException:
			cls.host.stop()
			raise

	@classmethod
	def tearDownClass(cls) -> None:
		cls.host.stop()

	def setUp(self) -> None:
		self.processes: list[RemoteProcess] = []
		self.host.run(f"install -d -m 700 {SOCKET_DIR}")

	def tearDown(self) -> None:
		for process in self.processes:
			process.close()
		self.host.run(f"pkill -u annot -x dcmview; rm -rf {SOCKET_DIR}; true")
		self.assertTrue(self.host.wait_until_stopped(), "a dcmview process outlived its test")

	def launch(self, command: str, *, tty: bool = False) -> RemoteProcess:
		process = self.host.launch(command, tty=tty)
		self.processes.append(process)
		return process

	def assert_forwarded_viewer(self, local_port: int, token: Optional[str], *, paint: bool) -> None:
		"""The page shell is public, the API needs the token, and the link paints a frame."""
		base = f"http://localhost:{local_port}"
		self.assertEqual(http_status(f"{base}/"), 200, "page shell through the forward")
		if token is None:
			self.assertEqual(http_status(f"{base}/api/files"), 200, "--no-token API without a token")
			return
		self.assertEqual(http_status(f"{base}/api/files"), 401, "API without the token")
		self.assertEqual(http_status(f"{base}/api/files", token), 200, "API with the bearer token")
		if paint:
			self.assertTrue(first_frame_paints(f"{base}/#token={token}"), "the forwarded link never painted a frame")

	def test_printed_hint_and_link_work_through_the_forward(self) -> None:
		cases = {
			"tcp": (f"{REMOTE_BINARY} --no-browser --port 0 {REMOTE_FIXTURE}", TCP_HINT),
			"unix-socket": (
				f"{REMOTE_BINARY} --no-browser --unix-socket {SOCKET_DIR}/viewer.sock {REMOTE_FIXTURE}",
				SOCKET_HINT,
			),
		}
		for name, (command, hint_pattern) in cases.items():
			with self.subTest(listener=name):
				process = self.launch(command)
				hint = process.wait_for(hint_pattern)
				link = process.wait_for(OPEN_LINE)
				self.assertEqual(link.group(2), hint.group(1), "the link's port is the hint's local port")
				remote_target = f"localhost:{hint.group(2)}" if name == "tcp" else hint.group(2)
				with self.host.forward(remote_target) as local_port:
					self.assert_forwarded_viewer(local_port, link.group(3), paint=True)
				process.close()
				self.host.run("pkill -u annot -x dcmview; true")
				self.assertTrue(self.host.wait_until_stopped())

	def test_python_entry_points_serve_through_the_forward(self) -> None:
		commands = {
			"dcmview console script": f"{REMOTE_VENV}/bin/dcmview",
			"dcmview-py console script": f"{REMOTE_VENV}/bin/dcmview-py",
			"python -m dcmview_py": f"{REMOTE_VENV}/bin/python -m dcmview_py",
		}
		for name, program in commands.items():
			with self.subTest(entry_point=name):
				process = self.launch(f"{program} --no-browser --port 0 {REMOTE_FIXTURE}")
				hint = process.wait_for(TCP_HINT)
				token = process.wait_for(OPEN_LINE).group(3)
				with self.host.forward(f"localhost:{hint.group(2)}") as local_port:
					self.assert_forwarded_viewer(local_port, token, paint=False)
				process.close()
				self.host.run("pkill -u annot -x dcmview; true")
				self.assertTrue(self.host.wait_until_stopped())

	def test_view_handle_reports_a_url_and_token_that_work_remotely(self) -> None:
		script = (
			"import json, sys\n"
			"from dcmview_py import view\n"
			f"handle = view([{REMOTE_FIXTURE!r}], browser=False, block=False)\n"
			"print('HANDLE ' + json.dumps({'base_url': handle.base_url, 'token': handle.token, 'url': handle.url}), flush=True)\n"
			"sys.stdin.readline()\n"
			"print('STOPPED', handle.stop(), flush=True)\n"
		)
		process = self.launch(f"{REMOTE_VENV}/bin/python -c {shlex.quote(script)}")
		handle = json.loads(process.wait_for(re.compile(r"HANDLE (\{.*\})")).group(1))
		self.assertTrue(handle["url"].endswith(f"#token={handle['token']}"))
		remote_port = urllib.parse.urlsplit(handle["base_url"]).port
		with self.host.forward(f"localhost:{remote_port}") as local_port:
			self.assert_forwarded_viewer(local_port, handle["token"], paint=True)
		process.send(b"\n")
		process.wait_for(re.compile(r"STOPPED"))
		self.assertTrue(self.host.wait_until_stopped(), "handle.stop() left the viewer running")

	def test_token_can_be_fixed_or_disabled(self) -> None:
		cases = {
			"DCMVIEW_TOKEN": ("DCMVIEW_TOKEN=fixed-remote-token ", "", "fixed-remote-token"),
			"--no-token": ("", "--no-token ", None),
		}
		for name, (env, flag, expected_token) in cases.items():
			with self.subTest(option=name):
				process = self.launch(f"{env}{REMOTE_BINARY} --no-browser --port 0 {flag}{REMOTE_FIXTURE}")
				hint = process.wait_for(TCP_HINT)
				if expected_token is not None:
					self.assertEqual(process.wait_for(OPEN_LINE).group(3), expected_token)
				with self.host.forward(f"localhost:{hint.group(2)}") as local_port:
					self.assert_forwarded_viewer(local_port, expected_token, paint=False)
				process.close()
				self.host.run("pkill -u annot -x dcmview; true")
				self.assertTrue(self.host.wait_until_stopped())

	def test_ctrl_c_and_idle_timeout_stop_the_server_and_remove_its_socket(self) -> None:
		cases = {
			"ctrl-c": ("", True),
			"--timeout": ("--timeout 2 ", False),
		}
		for name, (flag, tty) in cases.items():
			with self.subTest(stop=name):
				sock = f"{SOCKET_DIR}/{name.strip('-')}.sock"
				process = self.launch(
					f"{REMOTE_BINARY} --no-browser {flag}--unix-socket {sock} {REMOTE_FIXTURE}", tty=tty
				)
				process.wait_for(SOCKET_HINT)
				if tty:
					process.send(b"\x03")
				self.assertTrue(self.host.wait_until_stopped(), f"{name} did not stop the server")
				self.assertEqual(self.host.run(f"ls -A {SOCKET_DIR}").split(), [], f"{name} left files behind")

	def test_dropping_an_interactive_ssh_session_stops_the_server(self) -> None:
		process = self.launch(
			f"{REMOTE_BINARY} --no-browser --unix-socket {SOCKET_DIR}/hangup.sock {REMOTE_FIXTURE}", tty=True
		)
		process.wait_for(SOCKET_HINT)
		process.close()
		self.assertTrue(self.host.wait_until_stopped(), "the server outlived its SSH session")


if __name__ == "__main__":
	unittest.main()

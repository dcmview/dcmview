"""The VS Code Remote-SSH workflow against the shipped linux-x64 VSIX.

Real VS Code with the real Remote-SSH extension connects to the same
container as ``remote_ssh_integration``, the VSIX is installed on the remote
side the way Remote-SSH installs extensions, and
``tests/remote/vscode_remote_ssh.mjs`` drives the window with Playwright. Each
flow passes only when the viewer frame inside the webview, served through
VS Code's port forward, paints.

Run it through ``python scripts/check.py vscode-remote-ssh``. It needs
everything ``remote-ssh`` needs plus Node, ``npm ci`` in ``vscode/``, a display
(``xvfb-run`` on Linux CI), network access to download VS Code, Remote-SSH and
the VS Code Server, and ``DCMVIEW_REMOTE_VSIX`` naming a linux-x64 VSIX.
"""

from __future__ import annotations

import json
import os
import subprocess
import unittest
from pathlib import Path

from python.tests.remote_ssh_integration import FIXTURE_FILE, REMOTE_HOME, REMOTE_VENV, RemoteHost

REPO_ROOT = Path(__file__).resolve().parents[2]
DRIVER = REPO_ROOT / "tests" / "remote" / "vscode_remote_ssh.mjs"
HOST_ALIAS = "dcmview-remote"
REMOTE_FOLDER = f"{REMOTE_HOME}/work"
FIXTURE_NAME = "fixture.dcm"
FLOWS = ("custom-editor", "terminal-dcmview", "terminal-python")


class VSCodeRemoteSshTests(unittest.TestCase):
	host: RemoteHost

	@classmethod
	def setUpClass(cls) -> None:
		wheel = os.environ.get("DCMVIEW_REMOTE_WHEEL")
		vsix = os.environ.get("DCMVIEW_REMOTE_VSIX")
		if not wheel or not vsix:
			raise unittest.SkipTest(
				"DCMVIEW_REMOTE_WHEEL and DCMVIEW_REMOTE_VSIX are not set; "
				"run through scripts/check.py vscode-remote-ssh"
			)
		cls.vsix = Path(vsix).resolve()
		cls.host = RemoteHost(Path(wheel).resolve())
		try:
			cls.host.start()
			cls.host.copy(FIXTURE_FILE, f"{REMOTE_FOLDER}/{FIXTURE_NAME}")
			cls.host.copy(cls.vsix, f"/tmp/{cls.vsix.name}")
			cls.config_path = cls._write_config()
			# The first connection installs the VS Code Server; the VSIX then
			# goes in with that server's own CLI, as an installed extension.
			cls._drive("connect", expect=["connect"])
			cls.host.run(
				"server=$(find ~/.vscode-server -path '*/bin/code-server' -type f | head -n 1) "
				f'&& test -n "$server" && "$server" --install-extension /tmp/{cls.vsix.name} --force'
			)
		except BaseException:
			cls.host.stop()
			raise

	@classmethod
	def tearDownClass(cls) -> None:
		cls.host.stop()

	@classmethod
	def _write_config(cls) -> Path:
		workdir = cls.host.workdir
		ssh_config = workdir / "ssh_config"
		ssh_config.write_text(
			f"Host {HOST_ALIAS}\n"
			"  HostName 127.0.0.1\n"
			f"  Port {cls.host.port}\n"
			"  User annot\n"
			f"  IdentityFile {cls.host.key}\n"
			"  IdentitiesOnly yes\n"
			"  StrictHostKeyChecking no\n"
			"  UserKnownHostsFile /dev/null\n"
			"  LogLevel ERROR\n",
			encoding="utf-8",
		)
		config = {
			"workdir": str(workdir),
			"sshConfig": str(ssh_config),
			"hostAlias": HOST_ALIAS,
			"remoteFolder": REMOTE_FOLDER,
			"fixtureName": FIXTURE_NAME,
			"pythonEntry": f"{REMOTE_VENV}/bin/dcmview-py",
			"artifactsDir": os.environ.get("DCMVIEW_REMOTE_ARTIFACTS", ""),
			"vscodeVersion": os.environ.get("DCMVIEW_VSCODE_REMOTE_VERSION", "stable"),
		}
		path = workdir / "vscode-remote.json"
		path.write_text(json.dumps(config), encoding="utf-8")
		return path

	@classmethod
	def _drive(cls, phase: str, *, expect: list[str]) -> dict[str, dict[str, object]]:
		env = os.environ.copy()
		env["DCMVIEW_VSCODE_REMOTE_CONFIG"] = str(cls.config_path)
		completed = subprocess.run(
			["node", str(DRIVER), phase],
			env=env,
			capture_output=True,
			text=True,
			timeout=900,
		)
		results = {}
		for line in completed.stdout.splitlines():
			if line.startswith("RESULT "):
				result = json.loads(line[len("RESULT "):])
				results[result["name"]] = result
		missing = [name for name in expect if not results.get(name, {}).get("ok")]
		if phase == "connect" and missing:
			raise AssertionError(
				f"VS Code did not open the remote folder over Remote-SSH: {results}\n"
				f"stdout:\n{completed.stdout[-4000:]}\nstderr:\n{completed.stderr[-4000:]}"
			)
		if completed.returncode != 0 and not results:
			raise AssertionError(f"driver failed:\n{completed.stdout[-4000:]}\n{completed.stderr[-4000:]}")
		return results

	def test_remote_flows_paint_in_a_webview_and_leave_nothing_running(self) -> None:
		results = self._drive("scenarios", expect=list(FLOWS))
		for flow in FLOWS:
			with self.subTest(flow=flow):
				self.assertTrue(results.get(flow, {}).get("ok"), f"{flow}: {results.get(flow)}")
		with self.subTest(flow="cleanup"):
			self.assertTrue(results.get("closed-all-editors", {}).get("ok"), results)
			self.assertTrue(
				self.host.wait_until_stopped(timeout=30),
				"dcmview was still running on the remote after its editors closed",
			)


if __name__ == "__main__":
	unittest.main()

#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import pathlib
import time
import urllib.error
import urllib.request


def request_json(url: str) -> dict:
	request = urllib.request.Request(
		url,
		headers={
			"Accept": "application/vnd.github+json",
			"User-Agent": "dcmview-release-pipeline",
		},
	)
	with urllib.request.urlopen(request, timeout=30) as response:
		return json.loads(response.read().decode("utf-8"))


def download(url: str, destination: pathlib.Path) -> None:
	request = urllib.request.Request(
		url,
		headers={
			"Accept": "application/octet-stream",
			"User-Agent": "dcmview-release-pipeline",
		},
	)
	destination.parent.mkdir(parents=True, exist_ok=True)
	with urllib.request.urlopen(request, timeout=120) as response, destination.open("wb") as output:
		output.write(response.read())


def release_assets(repo: str, tag: str) -> dict[str, str]:
	data = request_json(f"https://api.github.com/repos/{repo}/releases/tags/{tag}")
	assets = data.get("assets")
	if not isinstance(assets, list):
		raise RuntimeError(f"release {tag} for {repo} did not return an assets list")

	result: dict[str, str] = {}
	for asset in assets:
		name = asset.get("name")
		url = asset.get("browser_download_url")
		if isinstance(name, str) and isinstance(url, str):
			result[name] = url
	return result


def wait_for_assets(repo: str, tag: str, required: list[str], timeout: int, interval: int) -> dict[str, str]:
	deadline = time.monotonic() + timeout
	last_error: str | None = None

	while time.monotonic() < deadline:
		try:
			assets = release_assets(repo, tag)
			missing = [name for name in required if name not in assets]
			if not missing:
				return assets
			last_error = f"missing assets: {', '.join(missing)}"
		except urllib.error.HTTPError as error:
			last_error = f"GitHub API returned HTTP {error.code}"
		except urllib.error.URLError as error:
			last_error = f"GitHub API request failed: {error.reason}"

		print(f"waiting for GitHub release assets for {tag}: {last_error}")
		time.sleep(interval)

	raise TimeoutError(f"timed out waiting for {repo} release {tag}: {last_error}")


def main() -> int:
	parser = argparse.ArgumentParser(description="Download required assets from a GitHub Release")
	parser.add_argument("--repo", required=True, help="GitHub repository, for example dcmview/dcmview")
	parser.add_argument("--tag", required=True, help="Release tag, for example v0.2.1")
	parser.add_argument("--output-dir", type=pathlib.Path, required=True)
	parser.add_argument("--required", action="append", required=True, help="Required asset name; may be repeated")
	parser.add_argument("--timeout", type=int, default=3600, help="Seconds to wait for all required assets")
	parser.add_argument("--interval", type=int, default=30, help="Seconds between GitHub API polls")
	args = parser.parse_args()

	assets = wait_for_assets(args.repo, args.tag, args.required, args.timeout, args.interval)
	for name in args.required:
		destination = args.output_dir / name
		download(assets[name], destination)
		print(f"downloaded {name} to {destination}")

	return 0


if __name__ == "__main__":
	raise SystemExit(main())

#!/usr/bin/env python3
"""Safely unpack one immutable GitHub Actions artifact ZIP."""

from __future__ import annotations

import argparse
from pathlib import Path

try:
    from scripts.compatibility.artifact import ArtifactError, extract_github_artifact
except ModuleNotFoundError:
    from artifact import ArtifactError, extract_github_artifact  # type: ignore[no-redef]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    try:
        extract_github_artifact(args.archive, args.destination)
    except ArtifactError as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

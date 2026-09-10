from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path


class StoredArtifactWorkflowTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )
        match = re.search(r"<<'PY'\n(?P<script>.*?)\n\s+PY\n", cls.workflow, re.DOTALL)
        if match is None:
            raise AssertionError("stored-artifact workflow metadata validator is missing")
        cls.validator = textwrap.dedent(match.group("script"))

    def test_workflow_uses_run_independent_artifact_endpoint_and_container_root(self) -> None:
        self.assertIn(
            '"$api_root/actions/artifacts/${DCMVIEW_CORPUS_ARTIFACT_ID}/zip"',
            self.workflow,
        )
        self.assertNotIn(
            "actions/runs/${DCMVIEW_CORPUS_ARTIFACT_RUN_ID}/artifacts/",
            self.workflow,
        )
        self.assertIn(
            "DCMVIEW_COMPAT_CORPUS_ROOT: ${{ runner.temp }}/dcmview-current-smoke",
            self.workflow,
        )
        for check in (
            'run.get("status") != "completed"',
            'run.get("conclusion") != "success"',
            'run.get("head_repository")',
            'repository.get("default_branch")',
            'run.get("head_branch") != default_branch',
        ):
            self.assertIn(check, self.workflow)

    def test_stored_artifact_job_is_not_conditionally_skipped_and_uses_v2_pins(self) -> None:
        job = self.workflow.split("  compatibility-artifact:", 1)[1].split("\n  vscode-compile:", 1)[0]
        self.assertNotIn("\n    if:", job)
        for name in (
            "DCMVIEW_CORPUS_ACTIONS_ZIP_SHA256",
            "DCMVIEW_CORPUS_ACTIONS_ZIP_SIZE_BYTES",
            "DCMVIEW_CORPUS_NESTED_ARCHIVE_SHA256",
            "DCMVIEW_CORPUS_NESTED_ARCHIVE_SIZE_BYTES",
            "DCMVIEW_CORPUS_RELEASE_MANIFEST_SHA256",
            "DCMVIEW_CORPUS_RELEASE_MANIFEST_SIZE_BYTES",
            "DCMVIEW_CORPUS_INSTALLED_BINARY_SHA256",
            "DCMVIEW_CORPUS_INSTALLED_BINARY_SIZE_BYTES",
        ):
            self.assertIn(name, job)

    def test_metadata_validator_rejects_untrusted_run_fixtures(self) -> None:
        artifact = {
            "id": 42,
            "expired": False,
            "workflow_run": {"id": 7, "repository_id": 99, "workflow_id": 11},
        }
        run = {
            "id": 7,
            "status": "completed",
            "conclusion": "success",
            "head_branch": "main",
            "head_repository": {"full_name": "producer/corpus"},
            "repository": {"full_name": "producer/corpus", "id": 99, "default_branch": "main"},
            "workflow_id": 11,
            "path": ".github/workflows/publish-smoke-artifact.yml",
        }

        def run_validator(mutated_run: dict[str, object]) -> subprocess.CompletedProcess[str]:
            with tempfile.TemporaryDirectory() as directory:
                artifact_path = Path(directory) / "artifact.json"
                run_path = Path(directory) / "run.json"
                artifact_path.write_text(json.dumps(artifact), encoding="utf-8")
                run_path.write_text(json.dumps(mutated_run), encoding="utf-8")
                return subprocess.run(
                    [
                        sys.executable,
                        "-c",
                        self.validator,
                        str(artifact_path),
                        str(run_path),
                        "producer/corpus",
                        "publish-smoke-artifact.yml",
                        "7",
                        "42",
                    ],
                    check=False,
                    capture_output=True,
                    text=True,
                )

        self.assertEqual(run_validator(run).returncode, 0)
        for field, value in (
            ("status", "in_progress"),
            ("conclusion", "failure"),
            ("head_branch", "feature"),
            ("head_repository", {"full_name": "attacker/corpus"}),
        ):
            mutated = dict(run)
            mutated[field] = value
            self.assertNotEqual(run_validator(mutated).returncode, 0, field)


if __name__ == "__main__":
    unittest.main()

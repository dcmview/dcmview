from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from scripts.compatibility.artifact import ArtifactError, build_external_worklist, verify_external_artifact


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


class ExternalArtifactTests(unittest.TestCase):
    def fixture(self) -> tuple[Path, dict[str, object], bytes]:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name) / "smoke"
        root.mkdir()
        payload = b"synthetic dicom bytes"
        relative = "classic/sc/example/instance.dcm"
        path = root / relative
        path.parent.mkdir(parents=True)
        path.write_bytes(payload)
        file_entry = {
            "case_id": "classic/sc/example",
            "path": relative,
            "size_bytes": len(payload),
            "sha256": _sha256(payload),
            "profile_membership": ["smoke"],
            "expected_capabilities": ["open_file", "read_metadata", "render_native_pixels"],
            "dicom": {
                "sop_class_uid": "1.2.840.10008.5.1.4.1.1.7",
                "transfer_syntax_uid": "1.2.840.10008.1.2.1",
            },
            "uids": {"sop_instance_uid": "2.25.1"},
            "image": {"bits_allocated": 8, "rows": 1, "columns": 1, "frames": 1},
        }
        identity = {
            "corpus_definition_sha256": "a" * 64,
            "manifest_sha256": "b" * 64,
            "definition_id": "dcmview-test-corpus",
            "definition_version": "0.3.0",
        }
        manifest = {
            "manifest_schema_version": "2.0.0",
            "generated_at": "2026-09-08T00:00:00Z",
            "generator": {
                "name": "synth-dicom-gen",
                "version": "0.3.0",
                "cargo_lock_sha256": "c" * 64,
                "feature_flags": [],
            },
            "identity_projection": {
                "identity_projection_schema_version": "1.0.0",
                "projection_state": "projected",
                "execution": {
                    "product_name": "synth-dicom-gen",
                    "enabled_features": [],
                    "execution_sha256": "d" * 64,
                },
                "toolchain": {"enabled_features": [], "toolchain_sha256": "e" * 64},
                "engine": {"engine_sha256": "f" * 64},
                "schema_set": {"schema_set_sha256": "1" * 64},
                "template_catalog": {"template_catalog_sha256": "2" * 64},
                "provider_catalog": {"provider_catalog_sha256": "3" * 64},
                "standards": {"standards_lock_sha256": "4" * 64},
                "corpus_definition": {"state": "verified_bundle", "identity": identity},
            },
            "run": {
                "kind": "external_corpus",
                "profile": "smoke",
                "seed": 1,
                "include_stress": False,
                "selector": {"kind": "profile"},
            },
            "files": [file_entry],
            "qualifications": [],
            "selection_ledger": [{
                "case_id": file_entry["case_id"],
                "selection": "direct",
                "registry_status": "implemented",
                "outcome": "generated",
                "reason_code": None,
                "artifact_paths": [relative],
                "dependency_case_ids": [],
                "case_definition": {"case_id": file_entry["case_id"]},
            }],
        }
        manifest_path = root / "manifest.json"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        return root, manifest, payload

    def test_verifies_manifest_identity_size_and_payload_hash(self) -> None:
        root, _, _ = self.fixture()
        verified = verify_external_artifact(
            root,
            expected_generator_version="0.3.0",
            expected_corpus_definition_sha256="a" * 64,
            expected_generator_features=(),
        )
        self.assertEqual(verified["identity"]["profile"], "smoke")
        self.assertEqual(verified["identity"]["run"]["selector"], {"kind": "profile"})
        self.assertEqual(verified["identity"]["generator"]["version"], "0.3.0")
        self.assertIn("identity_projection", verified["identity"])
        self.assertEqual(len(verified["files"]), 1)

    def test_builds_existing_runner_worklist_without_suite_checkout(self) -> None:
        root, _, _ = self.fixture()
        worklist = build_external_worklist(root)
        self.assertIsNone(worklist["suite"]["root"])
        self.assertEqual(worklist["inputs"]["profiles"], ["smoke"])
        self.assertEqual(worklist["files"][0]["normalized_path"], str((root / "classic/sc/example/instance.dcm").resolve()))
        self.assertEqual(worklist["files"][0]["policy"]["classification"], "pixel_faithful_interactive")

    def test_rejects_size_hash_and_selection_drift(self) -> None:
        root, manifest, payload = self.fixture()
        manifest["files"][0]["size_bytes"] += 1  # type: ignore[index]
        (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ArtifactError, "size mismatch"):
            verify_external_artifact(root)

        root, manifest, payload = self.fixture()
        (root / "classic/sc/example/instance.dcm").write_bytes(payload + b"tampered")
        manifest["files"][0]["size_bytes"] = len(payload) + len(b"tampered")  # type: ignore[index]
        (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ArtifactError, "hash mismatch"):
            verify_external_artifact(root)

        root, manifest, _ = self.fixture()
        manifest["run"]["profile"] = "core"  # type: ignore[index]
        (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ArtifactError, "profile mismatch"):
            verify_external_artifact(root)

    def test_rejects_parent_traversal_and_ledger_orphans(self) -> None:
        root, manifest, _ = self.fixture()
        manifest["files"][0]["path"] = "../outside.dcm"  # type: ignore[index]
        (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ArtifactError, "unsafe path"):
            verify_external_artifact(root)

        root, manifest, _ = self.fixture()
        manifest["selection_ledger"][0]["artifact_paths"] = []  # type: ignore[index]
        (root / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
        with self.assertRaisesRegex(ArtifactError, "selection ledger"):
            verify_external_artifact(root)

    def test_rejects_missing_manifest_as_an_artifact_error(self) -> None:
        root, _, _ = self.fixture()
        (root / "manifest.json").unlink()
        with self.assertRaisesRegex(ArtifactError, "cannot read JSON object"):
            verify_external_artifact(root)


if __name__ == "__main__":
    unittest.main()

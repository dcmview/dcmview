from __future__ import annotations

import hashlib
import json
import os
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path
from unittest import mock

import scripts.compatibility.worklist as worklist_module
from scripts.compatibility.worklist import (
    CompatibilityError,
    _validate_qualification,
    applicable_assertions,
    canonical_json,
    canonical_sha256,
    load_worklist,
)


class WorklistTests(unittest.TestCase):
    def test_selects_only_capability_backed_conditional_assertions(self) -> None:
        rule = {
            "required_assertions": ["discovery_identity"],
            "conditional_assertions": ["frame_navigation", "reference_closure"],
        }
        entry = {"expected_capabilities": ["navigate_multiframe"]}
        self.assertEqual(
            applicable_assertions(rule, entry),
            ["discovery_identity", "frame_navigation"],
        )

    def test_rejects_historical_worklist_schema(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "old.json"
            path.write_text(json.dumps({"worklist_schema_version": "0.1.0"}), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "incompatible worklist schema"):
                load_worklist(path)

    def test_rejects_worklist_hash_drift(self) -> None:
        with self._worklist() as (path, _root, value):
            self.assertEqual(load_worklist(path), value)
            value["inputs"]["profiles"] = ["stress"]
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "content SHA-256 mismatch"):
                load_worklist(path)

    def test_rejects_oversize_json_before_parsing(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "oversize.json"
            path.write_bytes(b"{" + b"x" * 64 + b"}")
            with mock.patch.object(worklist_module, "MAX_WORKLIST_BYTES", 32):
                with self.assertRaisesRegex(CompatibilityError, "exceeds the 32-byte limit"):
                    load_worklist(path)

    def test_rejects_malformed_unbounded_integer_without_traceback(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "huge-number.json"
            path.write_text('{"value":' + "9" * 5000 + "}", encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "JSON integer exceeds"):
                load_worklist(path)

    def test_rejects_malformed_shape_and_unsupported_profile(self) -> None:
        with self._worklist() as (path, _root, value):
            value["unexpected"] = True
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "unsupported fields"):
                load_worklist(path)

        with self._worklist() as (path, _root, value):
            value["inputs"]["profiles"] = ["all"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "exactly one"):
                load_worklist(path)

        with self._worklist() as (path, _root, value):
            value["models"]["negative_inputs"][0]["policy"]["unexpected"] = True
            value["files"] = value["models"]["negative_inputs"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "unsupported fields"):
                load_worklist(path)

    def test_rejects_traversal_before_runner_payload_access(self) -> None:
        with self._worklist() as (path, root, value):
            row = value["models"]["negative_inputs"][0]
            row["path"] = "../outside.dcm"
            row["normalized_path"] = str(root.parent / "outside.dcm")
            value["files"] = value["models"]["negative_inputs"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "confined relative path"):
                load_worklist(path)

    def test_rejects_symlink_payload_before_hashing(self) -> None:
        with self._worklist() as (path, root, _value):
            payload = root / "cases" / "input.dcm"
            payload.unlink()
            os.symlink("../../outside.dcm", payload)
            with self.assertRaisesRegex(CompatibilityError, "symlink"):
                load_worklist(path)

    def test_rejects_payload_hash_or_size_mismatch_before_runner_launch(self) -> None:
        with self._worklist() as (path, root, _value):
            payload = root / "cases" / "input.dcm"
            payload.write_bytes(b"changed!")
            with self.assertRaisesRegex(CompatibilityError, "size mismatch"):
                load_worklist(path)

        with self._worklist() as (path, root, value):
            payload = root / "cases" / "input.dcm"
            payload.write_bytes(b"other")
            row = value["models"]["negative_inputs"][0]
            row["size_bytes"] = len(b"other")
            value["files"] = value["models"]["negative_inputs"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "SHA-256 mismatch"):
                load_worklist(path)

    def test_rejects_contract_and_manifest_identity_hash_mismatch(self) -> None:
        with self._worklist() as (path, _root, value):
            row = value["models"]["negative_inputs"][0]
            row["contract_sha256"] = "0" * 64
            value["files"] = value["models"]["negative_inputs"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "contract SHA-256 mismatch"):
                load_worklist(path)

        with self._worklist() as (path, _root, value):
            row = value["models"]["negative_inputs"][0]
            row["manifest_identity_sha256"] = "0" * 64
            value["files"] = value["models"]["negative_inputs"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "manifest identity SHA-256 mismatch"):
                load_worklist(path)

        with self._worklist() as (path, _root, value):
            row = value["models"]["negative_inputs"][0]
            row["manifest_identity"]["manifest_sha256"] = "f" * 64
            row["manifest_identity_sha256"] = canonical_sha256(row["manifest_identity"])
            value["files"] = value["models"]["negative_inputs"]
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "not bound to the independently verified"):
                load_worklist(path)

    def test_rejects_manifest_count_drift_and_duplicate_selected_rows(self) -> None:
        with self._worklist() as (path, _root, value):
            value["inputs"]["manifests"][0]["physical_files"] = 0
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "physical_files count"):
                load_worklist(path)

        with self._worklist() as (path, _root, value):
            row = value["models"]["negative_inputs"][0]
            value["models"]["negative_inputs"] = [row, dict(row)]
            value["files"] = value["models"]["negative_inputs"]
            value["inputs"]["manifests"][0]["physical_files"] = 2
            value["summary"]["files"] = 2
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "duplicate selected payload rows"):
                load_worklist(path)

        with self._worklist() as (path, _root, value):
            unavailable = {
                "case_id": "negative/missing",
                "message": "missing",
                "profile": "negative",
                "reason_code": "planned",
                "recheck_phase": "phase-5",
                "standards_evidence": [],
                "status": "unavailable",
            }
            value["unavailable"] = [unavailable, dict(unavailable)]
            value["summary"]["unavailable_selected_profiles"] = 1
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "duplicate unavailable identities"):
                load_worklist(path)

    def test_rejects_qualification_contract_hash_mismatch(self) -> None:
        qualification = {
            "case_id": "fuzz/example",
            "contract": {"budget": {"max_candidates": 1}},
            "contract_sha256": "0" * 64,
            "policy": {
                "classification": "bounded",
                "required_assertions": ["fuzz_qualification_bounded"],
                "rule_id": "fuzz",
            },
            "profile": "fuzz",
        }
        with self.assertRaisesRegex(CompatibilityError, "qualification contract SHA-256 mismatch"):
            _validate_qualification(qualification, "fuzz", 0)

    def test_counts_qualification_only_case_as_logical_case(self) -> None:
        with self._worklist() as (path, _root, value):
            contract = {"budget": {"max_candidates": 1}}
            qualification = {
                "case_id": "fuzz/example",
                "contract": contract,
                "contract_sha256": canonical_sha256(contract),
                "policy": {
                    "classification": "bounded",
                    "required_assertions": ["fuzz_qualification_bounded"],
                    "rule_id": "fuzz",
                },
                "profile": "fuzz",
            }
            value["inputs"]["profiles"] = ["fuzz"]
            value["inputs"]["manifests"][0].update(
                {"profile": "fuzz", "physical_files": 0, "logical_cases": 1, "qualifications": 1}
            )
            value["models"].update(
                {
                    "negative_inputs": [],
                    "stress_files": [],
                    "stress_scenarios": [],
                    "fuzz_qualifications": [qualification],
                }
            )
            value["files"] = []
            value["summary"].update({"files": 0, "logical_cases": 1, "qualifications": 1})
            value["worklist_sha256"] = hashlib.sha256(
                canonical_json({key: item for key, item in value.items() if key != "worklist_sha256"})
            ).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            loaded = load_worklist(path)
            self.assertEqual(loaded["summary"]["logical_cases"], 1)

    def test_stages_verified_bytes_across_atomic_source_replacement(self) -> None:
        from scripts.compatibility.robustness import load_profile

        with self._worklist() as (path, root, _value):
            loaded = load_profile(path, "negative_inputs", "negative")
            try:
                staged = Path(loaded.entries[0]["normalized_path"])
                replacement = root / "replacement.dcm"
                replacement.write_bytes(b"replacement")
                os.replace(replacement, root / "cases" / "input.dcm")
                self.assertEqual(staged.read_bytes(), b"payload")
                self.assertNotEqual(staged, root / "cases" / "input.dcm")
            finally:
                staged_path = Path(loaded.entries[0]["normalized_path"])
                loaded.close()
            self.assertFalse(staged_path.exists())

    def test_staging_keeps_signed_worklist_content_unchanged(self) -> None:
        with self._worklist() as (path, _root, value):
            loaded = load_worklist(path, stage_payloads=True)
            try:
                self.assertEqual(loaded, value)
                self.assertNotEqual(
                    loaded.staged_models["negative_inputs"][0]["normalized_path"],
                    value["models"]["negative_inputs"][0]["normalized_path"],
                )
            finally:
                loaded.close()

    @contextmanager
    def _worklist(self):
        temporary = tempfile.TemporaryDirectory()
        try:
            base = Path(temporary.name)
            root = base / "negative-root"
            payload = root / "cases" / "input.dcm"
            payload.parent.mkdir(parents=True)
            payload.write_bytes(b"payload")
            manifest = root / "manifest.json"
            manifest.write_bytes(b"{}\n")
            manifest_sha256 = hashlib.sha256(manifest.read_bytes()).hexdigest()
            expected_contract = {"case_id": "negative/example"}
            manifest_identity = {
                "case_id": "negative/example",
                "manifest_sha256": manifest_sha256,
                "path": "cases/input.dcm",
                "profile": "negative",
            }
            row = {
                "case_id": "negative/example",
                "contract_sha256": canonical_sha256(expected_contract),
                "expected_contract": expected_contract,
                "kind": "negative",
                "manifest_identity": manifest_identity,
                "manifest_identity_sha256": canonical_sha256(manifest_identity),
                "normalized_path": str(payload.resolve()),
                "path": "cases/input.dcm",
                "policy": {
                    "classification": "controlled_unsupported",
                    "expected_unsupported": {"statuses": [422]},
                    "required_assertions": ["negative_bounded_outcome"],
                    "rule_id": "negative_isolated_robustness",
                    "semantic_context_assertions": [],
                },
                "sha256": hashlib.sha256(b"payload").hexdigest(),
                "size_bytes": len(b"payload"),
                "sop_instance_uid": None,
            }
            value = {
                "files": [row],
                "inputs": {
                    "lock": str(base / "lock.json"),
                    "lock_sha256": "4" * 64,
                    "manifests": [
                        {
                            "logical_cases": 1,
                            "manifest": str(manifest),
                            "physical_files": 1,
                            "profile": "negative",
                            "qualifications": 0,
                            "root": str(root),
                            "sha256": manifest_sha256,
                        }
                    ],
                    "policy": str(base / "policy.json"),
                    "policy_sha256": "5" * 64,
                    "profiles": ["negative"],
                },
                "models": {
                    "fuzz_qualifications": [],
                    "legacy_files": [],
                    "negative_inputs": [row],
                    "stress_files": [],
                    "stress_scenarios": [],
                    "valid_files": [],
                },
                "suite": {"commit": "a" * 40, "root": str(base)},
                "summary": {
                    "files": 1,
                    "logical_cases": 1,
                    "qualifications": 0,
                    "unavailable_selected_profiles": 0,
                },
                "unavailable": [],
                "worklist_schema_version": "0.2.0",
            }
            value["worklist_sha256"] = hashlib.sha256(canonical_json(value)).hexdigest()
            path = base / "worklist.json"
            path.write_text(json.dumps(value), encoding="utf-8")
            yield path, root, value
        finally:
            temporary.cleanup()


if __name__ == "__main__":
    unittest.main()

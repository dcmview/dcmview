from __future__ import annotations

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from scripts.compatibility.worklist import (
    CompatibilityError,
    applicable_assertions,
    canonical_json,
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
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "worklist.json"
            value = {"worklist_schema_version": "0.2.0", "inputs": {"profiles": ["stress"]}}
            value["worklist_sha256"] = hashlib.sha256(canonical_json(value)).hexdigest()
            path.write_text(json.dumps(value), encoding="utf-8")
            self.assertEqual(load_worklist(path), value)
            value["inputs"]["profiles"] = ["negative"]
            path.write_text(json.dumps(value), encoding="utf-8")
            with self.assertRaisesRegex(CompatibilityError, "worklist content hash mismatch"):
                load_worklist(path)


if __name__ == "__main__":
    unittest.main()

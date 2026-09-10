from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts.compatibility.robustness import RobustnessError, load_profile


class RobustnessLoaderTests(unittest.TestCase):
    def test_invalid_external_worklist_is_a_clear_runner_error(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "malformed.json"
            path.write_text(json.dumps({"worklist_schema_version": "0.2.0"}), encoding="utf-8")
            with self.assertRaisesRegex(RobustnessError, "invalid negative robustness worklist"):
                load_profile(path, "negative_inputs", "negative")


if __name__ == "__main__":
    unittest.main()

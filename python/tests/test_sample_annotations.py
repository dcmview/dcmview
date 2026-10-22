from __future__ import annotations

import json
from pathlib import Path
import unittest

REPO_ROOT = Path(__file__).resolve().parents[2]
SCHEMA = REPO_ROOT / "crates" / "dcmview-annotation" / "schema" / "dcmview.annotations.schema.json"
DOCUMENT = REPO_ROOT / "tests" / "fixtures" / "annotation-documents" / "sample.json"

try:
	import jsonschema
except ImportError:  # pragma: no cover - optional where the package is absent
	jsonschema = None


class SampleAnnotationDocumentTests(unittest.TestCase):
	"""The committed sample document against the model's generated JSON Schema.

	The Rust test that keeps the document current also checks it with the
	model's own `Document::validate`; this one needs the `jsonschema`
	package and is skipped without it.
	"""

	@unittest.skipIf(jsonschema is None, "the jsonschema package is not installed")
	def test_the_sample_document_matches_the_json_schema(self) -> None:
		schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
		document = json.loads(DOCUMENT.read_text(encoding="utf-8"))
		jsonschema.Draft202012Validator(schema).validate(document)


if __name__ == "__main__":
	unittest.main()

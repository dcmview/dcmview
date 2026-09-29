from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

SCRIPT_PATH = Path(__file__).resolve().parents[2] / "scripts" / "marketing_media.py"
SPEC = importlib.util.spec_from_file_location("marketing_media", SCRIPT_PATH)
assert SPEC is not None and SPEC.loader is not None
marketing_media = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(marketing_media)


class MarketingMediaSourceTests(unittest.TestCase):
	def source_manifest(self) -> dict:
		return {
			"schema_version": 1,
			"groups": [
				{
					"id": "example",
					"title": "Example modality",
					"collection": "EXAMPLE",
					"dataset_title": "Example dataset",
					"attribution_party": "Example creator",
					"year_version": "2026",
					"doi": "https://doi.org/10.example/example",
					"patient_ids": ["PUBLIC-001"],
					"series": [
						{
							"role": "source",
							"series_instance_uid": "1.2.3",
							"expected_files": 2,
						}
					],
				}
			],
		}

	def test_validates_unique_groups_roles_and_series(self) -> None:
		manifest = self.source_manifest()
		self.assertEqual(marketing_media.source_groups(manifest)[0]["id"], "example")

		manifest["groups"].append(dict(manifest["groups"][0]))
		with self.assertRaises(marketing_media.MarketingMediaError):
			marketing_media.source_groups(manifest)

	def test_writes_and_verifies_content_addressed_inventory(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			sources_path = root / "sources.json"
			manifest = self.source_manifest()
			sources_path.write_text(json.dumps(manifest), encoding="utf-8")
			series_root = root / "payload" / "example" / "1.2.3"
			series_root.mkdir(parents=True)
			(series_root / "one.dcm").write_bytes(b"one")
			(series_root / "two.dcm").write_bytes(b"two")

			inventory_path = marketing_media.write_source_inventory(
				source_root=root / "payload",
				sources_path=sources_path,
				groups=marketing_media.source_groups(manifest),
			)
			inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
			self.assertEqual(inventory["file_count"], 2)
			self.assertEqual(inventory["total_bytes"], 6)
			self.assertEqual(len({entry["sha256"] for entry in inventory["files"]}), 2)

	def test_rejects_incomplete_series_inventory(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			series_root = root / "example" / "1.2.3"
			series_root.mkdir(parents=True)
			(series_root / "one.dcm").write_bytes(b"one")
			group = self.source_manifest()["groups"][0]
			with self.assertRaises(marketing_media.MarketingMediaError):
				marketing_media.inventory_series(
					source_root=root,
					group=group,
					series=group["series"][0],
				)

	def test_accepts_standard_idc_series_directory_layout(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			series_root = root / "example" / "collection" / "patient" / "study" / "MR_1.2.3"
			series_root.mkdir(parents=True)
			(series_root / "one.dcm").write_bytes(b"one")
			(series_root / "two.dcm").write_bytes(b"two")
			group = self.source_manifest()["groups"][0]
			entries = marketing_media.inventory_series(
				source_root=root, group=group, series=group["series"][0]
			)
			self.assertEqual(len(entries), 2)

	def test_resolves_capture_scene_to_allowlisted_source(self) -> None:
		captures = {
			"schema_version": 1,
			"viewport": {"width": 1440, "height": 900, "device_scale_factor": 1},
			"theme": "dark",
			"locale": "en-US",
			"scenes": [
				{
					"id": "example-scene",
					"group": "example",
					"series_role": "source",
					"kind": "screenshot",
					"output": "example.png",
					"modifications": ["windowed"],
				}
			],
		}
		resolved = marketing_media.resolve_scenes(
			captures=captures, sources=self.source_manifest(), requested=[]
		)
		self.assertEqual(resolved[0]["series_instance_uid"], "1.2.3")
		self.assertEqual(resolved[0]["allowed_patient_ids"], ["PUBLIC-001"])
		self.assertEqual(resolved[0]["viewport"]["width"], 1440)

	def test_rejects_capture_output_outside_bundle(self) -> None:
		captures = {
			"schema_version": 1,
			"viewport": {"width": 1, "height": 1, "device_scale_factor": 1},
			"theme": "dark",
			"locale": "en-US",
			"scenes": [
				{
					"id": "escape",
					"group": "example",
					"series_role": "source",
					"kind": "screenshot",
					"output": "../escape.png",
					"modifications": [],
				}
			],
		}
		with self.assertRaises(marketing_media.MarketingMediaError):
			marketing_media.capture_scenes(captures)

	def test_publication_markers_are_idempotent(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			path = Path(directory) / "README.md"
			path.write_text("# Example\n\n## Install\n", encoding="utf-8")
			marketing_media.replace_marked_block(path, "## Gallery\n\nFirst", anchor="## Install")
			marketing_media.replace_marked_block(path, "## Gallery\n\nSecond", anchor="## Install")
			text = path.read_text(encoding="utf-8")
			self.assertEqual(text.count("dcmview-marketing:start"), 1)
			self.assertNotIn("First", text)
			self.assertIn("Second", text)

	def test_publication_markers_migrate_to_mdx_comments(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			path = Path(directory) / "index.mdx"
			path.write_text(
				"<!-- dcmview-marketing:start -->\nOld\n<!-- dcmview-marketing:end -->\n\n## Docs\n",
				encoding="utf-8",
			)
			marketing_media.replace_marked_block(path, "## Gallery\n\nNew", anchor="## Docs", mdx=True)
			text = path.read_text(encoding="utf-8")
			self.assertIn("{/* dcmview-marketing:start */}", text)
			self.assertIn("{/* dcmview-marketing:end */}", text)
			self.assertNotIn("<!-- dcmview-marketing:start -->", text)
			self.assertNotIn("Old", text)

	def test_publication_galleries_resolve_current_scene_outputs(self) -> None:
		paths = {
			"mr-seg-cine": "mr-seg-cine.gif",
			"chest-ct-cine": "chest-ct-cine.gif",
			"radiograph": "radiograph.png",
			"mammography": "mammography.gif",
			"pet-cine": "pet-cine.gif",
			"ultrasound-cine": "ultrasound-cine.gif",
			"rt-dose-context": "rt-dose-context.png",
			"wsi-context": "wsi-context.png",
			"vscode-workflow": "vscode-workflow.gif",
		}
		viewer = marketing_media.viewer_gallery(
			paths, asset_base="https://example.test/media", attribution_url="/attribution"
		)
		extension = marketing_media.vscode_gallery(
			paths, asset_base="https://example.test/vscode", attribution_url="/attribution"
		)
		self.assertIn("https://example.test/media/mr-seg-cine.gif", viewer)
		self.assertIn("https://example.test/media/wsi-context.png", viewer)
		self.assertIn("https://example.test/vscode/vscode-workflow.gif", extension)
		self.assertNotIn("brain-mr-seg.png", viewer)
		self.assertNotIn("vscode-workflow.png", extension)

	def test_publication_gallery_rejects_missing_scene(self) -> None:
		with self.assertRaises(marketing_media.MarketingMediaError):
			marketing_media.viewer_gallery(
				{}, asset_base="https://example.test/media", attribution_url="/attribution"
			)

	def test_repo_only_publication_never_touches_the_sibling_repository(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory) / "dcmview"
			root.mkdir()
			for name, anchor in [("README.md", "## Why use it?"), ("docs/index.md", "## User Guides"), ("vscode/README.md", "## Supported Platforms")]:
				path = root / name
				path.parent.mkdir(parents=True, exist_ok=True)
				path.write_text(anchor + "\n")
			docs = root.parent / "dcmview-docs"
			docs.mkdir()
			sentinel = docs / "owner-work.txt"
			sentinel.write_text("preserve")
			bundle = root / "bundle"
			bundle.mkdir()
			scenes = marketing_media.resolve_scenes(
				captures=marketing_media.load_json(marketing_media.DEFAULT_CAPTURES),
				sources=marketing_media.load_json(marketing_media.DEFAULT_SOURCES), requested=[],
			)
			lock = {"dcmview_version": "0.3.0", "artifacts": [{"scene_id": scene["id"], "path": scene["output"]} for scene in scenes]}
			for artifact in lock["artifacts"]:
				(bundle / artifact["path"]).write_bytes(b"reviewed media")
			(bundle / marketing_media.MEDIA_LOCK_NAME).write_text(json.dumps(lock))
			(bundle / marketing_media.ATTRIBUTION_NAME).write_text("Attribution")
			with patch.object(marketing_media, "REPO_ROOT", root):
				parser = marketing_media.build_parser()
				default = parser.parse_args(["publish", "--tag", "v0.3.0"])
				self.assertEqual(default.docs_repo, docs)
				self.assertFalse(default.repo_only)
				args = parser.parse_args(["publish", "--tag", "v0.3.0", "--approve", "--repo-only", "--bundle", str(bundle)])
				with patch.object(marketing_media, "verify_media_bundle", return_value=lock) as verify:
					marketing_media.publish_media(args)
					verify.assert_called_once_with(args, quiet=True)
			self.assertEqual(list(docs.iterdir()), [sentinel])
			self.assertEqual(sentinel.read_text(), "preserve")
			self.assertEqual((root / "media/marketing/mr-seg-cine.gif").read_bytes(), b"reviewed media")
			self.assertIn("/v0.3.0/media/marketing/", (root / "README.md").read_text())

	def test_parses_the_binary_startup_event(self) -> None:
		class FakeProcess:
			stdout = iter(
				['{"type":"server_started","url":"http://127.0.0.1:54321","host":"127.0.0.1","port":54321}\n']
			)

			@staticmethod
			def poll() -> None:
				return None

		self.assertEqual(
			marketing_media.wait_for_startup(FakeProcess(), timeout=1),  # type: ignore[arg-type]
			"http://127.0.0.1:54321",
		)


if __name__ == "__main__":
	unittest.main()

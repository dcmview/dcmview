"""Unit tests for the compatibility runner's oracles and corpus verification."""

from __future__ import annotations

import hashlib
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path

from scripts.compatibility import run


def mono_entry(values, *, photometric="MONOCHROME2", rows=2, columns=2, frames=1, **recipe):
    return {
        "dicom": {"transfer_syntax_uid": "1.2.840.10008.1.2.1"},
        "image": {"rows": rows, "columns": columns, "frames": frames, "samples_per_pixel": 1, "photometric_interpretation": photometric},
        "recipe": {"recipe_parameters": {"pixel_values": values, **recipe}},
    }


class LinearWindowTest(unittest.TestCase):
    def test_follows_the_dicom_linear_function_boundaries(self):
        # PS3.3 C.11.2.1.2.1 with c=40, w=400: black at or below -160, white above 239.
        self.assertEqual(run.linear_window(-160, 40, 400), 0)
        self.assertEqual(run.linear_window(-159, 40, 400), 1)
        self.assertEqual(run.linear_window(39.5, 40, 400), 128)
        self.assertEqual(run.linear_window(239, 40, 400), 255)
        self.assertEqual(run.linear_window(240, 40, 400), 255)

    def test_width_one_is_a_threshold(self):
        self.assertEqual(run.linear_window(9.5, 10, 1), 0)
        self.assertEqual(run.linear_window(9.6, 10, 1), 255)


class ExpectedDisplayTest(unittest.TestCase):
    def test_declared_window_uses_the_default_request_after_rescale(self):
        entry = mono_entry([1024, 1064, 824, 1264], rescale={"slope": "1", "intercept": "-1024"}, window={"center": "40", "width": "400"})
        oracle, _ = run.expected_display(entry, 0)
        self.assertEqual(oracle["mode"], "default")
        self.assertEqual([pixel[0] for pixel in oracle["pixels"]], [102, 128, 0, 255])

    def test_without_a_window_full_dynamic_spans_unpadded_samples_and_monochrome1_inverts(self):
        entry = mono_entry([0, 1000, 2000, 3000], photometric="MONOCHROME1", pixel_padding={"value": 0, "range_limit": 0})
        oracle, _ = run.expected_display(entry, 0)
        self.assertEqual(oracle["mode"], "full_dynamic")
        # Padding is excluded from the range and left unasserted; 2000 windows to 128, inverted to 127.
        self.assertEqual(oracle["pixels"], [None, (255, 255, 255), (127, 127, 127), (0, 0, 0)])

    def test_reads_the_requested_frame_from_flat_or_per_frame_values(self):
        flat = mono_entry([0, 1, 2, 3, 30, 20, 10, 0], frames=2)
        nested = mono_entry([[0, 1, 2, 3], [30, 20, 10, 0]], frames=2)
        for entry in (flat, nested):
            oracle, _ = run.expected_display(entry, 1)
            self.assertEqual([pixel[0] for pixel in oracle["pixels"]], [255, 176, 88, 0])

    def test_rgb_planar_configuration_is_reassembled(self):
        entry = mono_entry([255, 0, 0, 0, 0, 255, 0, 0, 0, 0, 255, 0], rows=1, columns=4)
        entry["image"].update(samples_per_pixel=3, photometric_interpretation="RGB", planar_configuration=1)
        oracle, _ = run.expected_display(entry, 0)
        self.assertEqual(oracle["pixels"], [(255, 0, 0), (0, 255, 0), (0, 0, 255), (0, 0, 0)])

    def test_reports_why_no_oracle_exists(self):
        lut = mono_entry([0, 1, 2, 3], voi_lut={"descriptor": [4, 0, 16]})
        lossy = mono_entry([0, 1, 2, 3])
        lossy["dicom"]["transfer_syntax_uid"] = run.JPEG_BASELINE
        self.assertEqual(run.expected_display(lut, 0), (None, "recipe declares voi_lut"))
        self.assertIsNone(run.expected_display(lossy, 0)[0])

    def test_compare_ignores_unasserted_pixels_and_reports_mismatches(self):
        result = run.compare_display([None, (1, 1, 1), (2, 2, 2)], [(9, 9, 9), (1, 1, 1), (3, 3, 3)])
        self.assertFalse(result["passed"])
        self.assertEqual((result["compared_pixels"], result["mismatched_pixels"]), (2, 1))


class RawBytesTest(unittest.TestCase):
    def test_masks_samples_to_bits_stored_before_hashing(self):
        image = {"bits_allocated": 16, "bits_stored": 12}
        self.assertEqual(run.canonical_raw_bytes(b"\xff\xff\x01\x00", image, None), b"\xff\x0f\x01\x00")

    def test_repacks_expanded_one_bit_deflated_frames(self):
        image = {"bits_allocated": 1}
        packed = run.canonical_raw_bytes(bytes([1, 0, 1, 1, 0, 0, 0, 0, 1]), image, "1.2.840.10008.1.2.8.1")
        self.assertEqual(packed, bytes([0b00001101, 0b00000001]))


class CorpusTest(unittest.TestCase):
    def build_container(self, root: Path, *, payload=b"DICM-payload", declared_sha=None, member=None) -> Path:
        manifest = {
            "manifest_schema_version": run.MANIFEST_SCHEMA_VERSION,
            "files": [{"path": "case/instance.dcm", "sha256": declared_sha or hashlib.sha256(payload).hexdigest(), "size_bytes": len(payload)}],
        }
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w:gz") as tar:
            for name, data in (("corpus/manifest.json", json.dumps(manifest).encode()), ("corpus/case/instance.dcm", payload)):
                info = tarfile.TarInfo(name)
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
            if member is not None:
                tar.addfile(member)
        container = root / "container"
        container.mkdir()
        (container / run.ARCHIVE_NAME).write_bytes(archive.getvalue())
        index = {"archive_sha256": "sha256:" + hashlib.sha256(archive.getvalue()).hexdigest(), "archive_size_bytes": len(archive.getvalue())}
        (container / run.INDEX_NAME).write_text(json.dumps(index))
        return container

    def test_accepts_a_container_whose_digests_agree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            corpus, entries, _ = run.load_corpus(self.build_container(root), root / "extract")
            self.assertEqual((corpus / entries[0]["path"]).read_bytes(), b"DICM-payload")

    def test_rejects_a_payload_that_differs_from_its_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            container = self.build_container(root, declared_sha="0" * 64)
            with self.assertRaisesRegex(run.CampaignError, "does not match its manifest"):
                run.load_corpus(container, root / "extract")

    def test_rejects_an_archive_that_differs_from_its_index(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            container = self.build_container(root)
            (container / run.ARCHIVE_NAME).write_bytes(b"tampered")
            with self.assertRaisesRegex(run.CampaignError, "does not match index"):
                run.load_corpus(container, root / "extract")

    def test_rejects_links_and_paths_outside_the_corpus(self):
        link = tarfile.TarInfo("corpus/link")
        link.type, link.linkname = tarfile.SYMTYPE, "/etc/passwd"
        escape = tarfile.TarInfo("../outside")
        for member, message in ((link, "not a regular file"), (escape, "escapes")):
            with self.subTest(member=member.name), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                container = self.build_container(root, member=member)
                with self.assertRaisesRegex(run.CampaignError, message):
                    run.load_corpus(container, root / "extract")


if __name__ == "__main__":
    unittest.main()

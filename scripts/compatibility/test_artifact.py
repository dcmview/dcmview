from __future__ import annotations

import gzip
import hashlib
import io
import json
import os
import stat
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

from scripts.compatibility.artifact import (
    ArtifactError,
    _extract_deterministic_archive,
    build_external_worklist,
    extract_github_artifact,
    verify_external_artifact,
)


def _sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def _canonical(payload: object) -> bytes:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


class ExternalArtifactTests(unittest.TestCase):
    def fixture(self) -> tuple[Path, dict[str, object]]:
        temporary = tempfile.TemporaryDirectory(dir="/private/tmp")
        self.addCleanup(temporary.cleanup)
        container = Path(temporary.name) / "producer-container"
        container.mkdir()
        corpus = Path(temporary.name) / "source-corpus"
        corpus.mkdir()
        payloads = {
            "classic/sc/example/first.dcm": b"synthetic dicom bytes one",
            "classic/sc/example/second.dcm": b"synthetic dicom bytes two",
        }
        files: list[dict[str, object]] = []
        ledger_paths: list[str] = []
        for index, (relative, payload) in enumerate(payloads.items(), start=1):
            path = corpus / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(payload)
            files.append(
                {
                    "case_id": "classic/sc/example",
                    "path": relative,
                    "size_bytes": len(payload),
                    "sha256": _sha256(payload),
                    "profile_membership": ["smoke"],
                    "expected_capabilities": ["open_file", "read_metadata"],
                    "dicom": {
                        "sop_class_uid": "1.2.840.10008.5.1.4.1.1.7",
                        "transfer_syntax_uid": "1.2.840.10008.1.2.1",
                    },
                    "uids": {"sop_instance_uid": f"2.25.{index}"},
                    "image": {"bits_allocated": 8, "rows": 1, "columns": 1, "frames": 1},
                }
            )
            ledger_paths.append(relative)
        identity = {
            "corpus_definition_sha256": "d" * 64,
            "manifest_sha256": "c" * 64,
            "definition_id": "dcmview-test-corpus",
            "definition_version": "0.3.0",
        }
        manifest = {
            "manifest_schema_version": "2.0.0",
            "generated_at": "2026-09-08T00:00:00Z",
            "generator": {
                "name": "synth-dicom-gen",
                "version": "0.3.0",
                "cargo_lock_sha256": "e" * 64,
                "feature_flags": [],
            },
            "identity_projection": {
                "identity_projection_schema_version": "1.0.0",
                "projection_state": "projected",
                "execution": {
                    "product_name": "synth-dicom-gen",
                    "enabled_features": [],
                    "execution_sha256": "f" * 64,
                },
                "toolchain": {"enabled_features": [], "toolchain_sha256": "1" * 64},
                "engine": {"engine_sha256": "2" * 64},
                "schema_set": {"schema_set_sha256": "3" * 64},
                "template_catalog": {"template_catalog_sha256": "4" * 64},
                "provider_catalog": {"provider_catalog_sha256": "5" * 64},
                "standards": {"standards_lock_sha256": "6" * 64},
                "corpus_definition": {"state": "verified_bundle", "identity": identity},
            },
            "run": {
                "kind": "external_corpus",
                "profile": "smoke",
                "seed": 1,
                "include_stress": False,
                "selector": {"kind": "profile"},
            },
            "files": files,
            "qualifications": [],
            "selection_ledger": [
                {
                    "case_id": "classic/sc/example",
                    "selection": "direct",
                    "registry_status": "implemented",
                    "outcome": "generated",
                    "reason_code": None,
                    "artifact_paths": ledger_paths,
                    "dependency_case_ids": [],
                    "case_definition": {"case_id": "classic/sc/example"},
                }
            ],
        }
        manifest_bytes = json.dumps(manifest, sort_keys=True, indent=2).encode("utf-8") + b"\n"
        (corpus / "manifest.json").write_bytes(manifest_bytes)
        manifest_sha = _sha256(manifest_bytes)

        binding: dict[str, object] = {
            "generator": {
                "product": {"name": "synth-dicom-gen", "version": "0.3.0"},
                "source_revision": "a" * 40,
                "artifact": {"sha256": "b" * 64, "size_bytes": 123},
                "target": "aarch64-apple-darwin",
                "rust_toolchain": "rustc 1.88.0 (fake)",
                "enabled_features": [],
            },
            "corpus": {
                "definition_manifest_sha256": identity["manifest_sha256"],
                "corpus_definition_sha256": identity["corpus_definition_sha256"],
                "definition_version": "0.3.0",
                "generated_manifest_sha256": manifest_sha,
                "generated_manifest_size_bytes": len(manifest_bytes),
                "manifest_schema_version": "2.0.0",
            },
            "run": {"profile": "smoke", "seed": 1, "parallelism": 1, "include_stress": False},
            "features": [],
            "runtime": {"external_runtime": [], "identity_domains": {}},
            "payload": {
                "file_count": len(files) + 1,
                "total_size_bytes": len(manifest_bytes) + sum(len(payload) for payload in payloads.values()),
                "files": [
                    {"path": "manifest.json", "size_bytes": len(manifest_bytes), "sha256": manifest_sha},
                    *[
                        {"path": relative, "size_bytes": len(payload), "sha256": _sha256(payload)}
                        for relative, payload in payloads.items()
                    ],
                ],
            },
            "archive_sha256": None,
        }
        archive_path = container / "smoke.tar.gz"
        directories = {"corpus"}
        for relative in ["manifest.json", *payloads]:
            parts = Path(relative).parts
            directories.update(str(Path("corpus", *parts[:index])) for index in range(1, len(parts)))
        with archive_path.open("wb") as stream:
            with gzip.GzipFile(fileobj=stream, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
                    for directory in sorted(directories, key=lambda value: (value.count("/"), value)):
                        info = tarfile.TarInfo(directory + "/")
                        info.type = tarfile.DIRTYPE
                        info.mode = 0o755
                        info.mtime = 0
                        info.uid = info.gid = 0
                        archive.addfile(info)
                    for relative in ["manifest.json", *payloads]:
                        payload = (corpus / relative).read_bytes()
                        info = tarfile.TarInfo(str(Path("corpus", relative)))
                        info.size = len(payload)
                        info.mode = 0o644
                        info.mtime = 0
                        info.uid = info.gid = 0
                        archive.addfile(info, io.BytesIO(payload))
        archive_bytes = archive_path.read_bytes()
        archive_sha = _sha256(archive_bytes)
        binding["archive_sha256"] = archive_sha
        binding_id = _sha256(_canonical(binding))
        index = {
            "artifact_descriptor_schema_version": "1.0.0",
            "artifact_name": (
                "dcmview-smoke-s" + "a" * 40 + "-a" + "b" * 64 + "-d" + "c" * 64 + "-b" + binding_id[:32]
            ),
            "binding_id": binding_id,
            "binding_sha256": binding_id,
            "archive_path": "smoke.tar.gz",
            "archive_sha256": archive_sha,
            "archive_size_bytes": len(archive_bytes),
            "archive_kind": "deterministic_tar_gz",
            "source_revision": "a" * 40,
            "generator_artifact_sha256": "b" * 64,
            "generator_artifact_size_bytes": 123,
            "target": "aarch64-apple-darwin",
            "rust_toolchain": "rustc 1.88.0 (fake)",
            "enabled_features": [],
            "runtime_identities": [],
            "definition_manifest_sha256": identity["manifest_sha256"],
            "corpus_definition_sha256": identity["corpus_definition_sha256"],
            "generated_manifest_sha256": manifest_sha,
            "generated_manifest_size_bytes": len(manifest_bytes),
            "profile": "smoke",
            "seed": 1,
            "binding": binding,
            "payload_root": "corpus",
            "manifest_path": "corpus/manifest.json",
            "evidence": [],
            "retrieval": {
                "repository": "beatrice-b-m/dcmview-test-corpus",
                "workflow": "publish-smoke-artifact.yml",
                "run_id": None,
                "artifact_id": None,
                "artifact_digest": None,
            },
        }
        (container / "artifact-index.json").write_bytes(_canonical(index) + b"\n")
        return container, {"index": index, "manifest": manifest, "payloads": payloads}

    def pins(self, index: dict[str, object]) -> dict[str, object]:
        return {
            "expected_generator_revision": index["source_revision"],
            "expected_generator_artifact_sha256": index["generator_artifact_sha256"],
            "expected_generator_artifact_size_bytes": index["generator_artifact_size_bytes"],
            "expected_target": index["target"],
            "expected_toolchain": index["rust_toolchain"],
            "expected_generator_features": (),
            "expected_runtime_identities_sha256": _sha256(_canonical([])),
            "expected_definition_manifest_sha256": index["definition_manifest_sha256"],
            "expected_corpus_definition_sha256": index["corpus_definition_sha256"],
            "expected_manifest_sha256": index["generated_manifest_sha256"],
            "expected_manifest_size_bytes": index["generated_manifest_size_bytes"],
            "expected_profile": "smoke",
            "expected_seed": 1,
            "expected_binding_id": index["binding_id"],
            "expected_archive_sha256": index["archive_sha256"],
            "expected_archive_size_bytes": index["archive_size_bytes"],
            "expected_generator_version": "0.3.0",
        }

    def test_exact_producer_container_reaches_every_worklist_payload(self) -> None:
        container, expected = self.fixture()
        pins = self.pins(expected["index"])
        verified = verify_external_artifact(container, **pins, required_pins=True)
        self.addCleanup(verified["_temporary_directory"].cleanup)
        self.assertEqual(len(verified["files"]), len(expected["payloads"]))
        self.assertEqual({row["path"] for row in verified["files"]}, set(expected["payloads"]))
        worklist = build_external_worklist(container, **pins, required_pins=True)
        self.addCleanup(worklist["_temporary_directory"].cleanup)
        self.assertEqual({row["path"] for row in worklist["files"]}, set(expected["payloads"]))
        self.assertTrue(all(Path(row["normalized_path"]).is_file() for row in worklist["files"]))

    def test_github_container_extraction_rejects_zip_links_and_traversal(self) -> None:
        temporary = tempfile.TemporaryDirectory(dir="/private/tmp")
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        archive = root / "artifact.zip"
        with zipfile.ZipFile(archive, "w") as zipped:
            link = zipfile.ZipInfo("link")
            link.create_system = 3
            link.external_attr = (stat.S_IFLNK | 0o777) << 16
            zipped.writestr(link, "artifact-index.json")
            zipped.writestr("smoke.tar.gz", b"archive")
        with self.assertRaisesRegex(ArtifactError, "special entry"):
            extract_github_artifact(archive, root / "out-links")

        archive = root / "artifact-traversal.zip"
        with zipfile.ZipFile(archive, "w") as zipped:
            zipped.writestr("../escape", b"no")
        with self.assertRaisesRegex(ArtifactError, "unsafe path"):
            extract_github_artifact(archive, root / "out")

    def test_deterministic_archive_rejects_traversal_duplicates_and_specials(self) -> None:
        temporary = tempfile.TemporaryDirectory(dir="/private/tmp")
        self.addCleanup(temporary.cleanup)

        def archive_bytes(entries: list[tarfile.TarInfo], payloads: dict[str, bytes]) -> bytes:
            stream = io.BytesIO()
            with gzip.GzipFile(fileobj=stream, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
                    for entry in entries:
                        data = payloads.get(entry.name)
                        archive.addfile(entry, io.BytesIO(data) if data is not None else None)
            return stream.getvalue()

        traversal = tarfile.TarInfo("corpus/../escape")
        traversal.size = 1
        with self.assertRaisesRegex(ArtifactError, "unsafe path"):
            _extract_deterministic_archive(
                archive_bytes([traversal], {traversal.name: b"x"}),
                Path(temporary.name) / "traversal",
            )

        duplicate_one = tarfile.TarInfo("corpus/manifest.json")
        duplicate_one.size = 1
        duplicate_two = tarfile.TarInfo("corpus/manifest.json")
        duplicate_two.size = 1
        with self.assertRaisesRegex(ArtifactError, "duplicate path"):
            _extract_deterministic_archive(
                archive_bytes(
                    [duplicate_one, duplicate_two],
                    {"corpus/manifest.json": b"{"},
                ),
                Path(temporary.name) / "duplicate",
            )

        special = tarfile.TarInfo("corpus/fifo")
        special.type = tarfile.FIFOTYPE
        with self.assertRaisesRegex(ArtifactError, "non-regular entry"):
            _extract_deterministic_archive(
                archive_bytes([special], {}),
                Path(temporary.name) / "special",
            )

    def test_rejects_container_symlinks_and_extras(self) -> None:
        container, expected = self.fixture()
        (container / "extra.txt").write_bytes(b"extra")
        with self.assertRaisesRegex(ArtifactError, "undeclared"):
            verify_external_artifact(container, **self.pins(expected["index"]))

        container, expected = self.fixture()
        os.symlink("artifact-index.json", container / "link")
        with self.assertRaisesRegex(ArtifactError, "symlink"):
            verify_external_artifact(container, **self.pins(expected["index"]))

    def test_rejects_pin_drift_and_archive_tampering(self) -> None:
        container, expected = self.fixture()
        pins = self.pins(expected["index"])
        wrong = dict(pins, expected_binding_id="0" * 64)
        with self.assertRaisesRegex(ArtifactError, "binding ID mismatch"):
            verify_external_artifact(container, **wrong)

        container, expected = self.fixture()
        archive = container / "smoke.tar.gz"
        archive.write_bytes(archive.read_bytes() + b"tampered")
        with self.assertRaisesRegex(ArtifactError, "archive digest mismatch"):
            verify_external_artifact(container, **self.pins(expected["index"]))

    def test_missing_required_pin_fails_closed(self) -> None:
        container, expected = self.fixture()
        pins = self.pins(expected["index"])
        pins.pop("expected_archive_sha256")
        with self.assertRaisesRegex(ArtifactError, "required immutable pin"):
            verify_external_artifact(container, **pins, required_pins=True)


if __name__ == "__main__":
    unittest.main()

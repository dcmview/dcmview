from __future__ import annotations

import contextlib
import hashlib
import io
import struct
import subprocess
import sys
import unittest
import zlib
from pathlib import Path

from scripts.compatibility.run import (
    PROBED_CAPABILITIES,
    canonical_raw_bytes,
    deterministic_navigation_frames,
    frame_time_observation,
    normalized_report,
    icc_profile_observation,
    lossy_metrics_observation,
    metadata_observation,
    nm_dimensions_observation,
    overlay_display_observation,
    pet_activity_observation,
    pixel_geometry_observation,
    png_pixels,
    projection_geometry_observation,
    reference_observation,
    raw_header_observation,
    semantic_context_observations,
    series_observation,
    shutter_display_observation,
    validate_json_schema,
    validate_report,
    validate_visual,
    unprobed_capabilities,
    wsi_context_observations,
)
from scripts.compatibility.run import parse_args


def grayscale_png(
    values: bytes, width: int, height: int, icc_profile: bytes | None = None
) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)
    rows = b"".join(b"\x00" + values[row * width : (row + 1) * width] for row in range(height))
    iccp = b""
    if icc_profile is not None:
        iccp = chunk(b"iCCP", b"DICOM ICC\0\0" + zlib.compress(icc_profile))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + iccp
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )


class RunnerTests(unittest.TestCase):
    def test_artifact_source_uses_only_pinned_producer_container(self) -> None:
        args = parse_args([
            "--corpus-root", "/tmp/current-smoke",
            "--binary", "/tmp/dcmview",
            "--output", "/tmp/compatibility-output",
        ])
        self.assertEqual(str(args.corpus_root), "/tmp/current-smoke")
        self.assertFalse(hasattr(args, "suite_root"))
        self.assertFalse(hasattr(args, "worklist"))
        self.assertEqual(args.expected_seed, 1)

    def test_legacy_checkout_sources_are_not_cli_inputs(self) -> None:
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                parse_args([
                    "--suite-root", "/tmp/dicom-test-suite",
                    "--worklist", "/tmp/worklist.json",
                    "--binary", "/tmp/dcmview",
                    "--output", "/tmp/compatibility-output",
                ])

    def test_metadata_observation_compares_manifest_fields_and_declared_tags(self) -> None:
        expected = {
            "dicom": {"modality": "OT", "sop_class_uid": "1.2.class", "transfer_syntax_uid": "1.2.syntax"},
            "uids": {"sop_instance_uid": "1.2.instance", "study_instance_uid": "1.2.study", "series_instance_uid": "1.2.series"},
            "image": {"frames": 1, "rows": 2, "columns": 3},
            "expected_metadata": {
                "specific_character_sets": ["ISO_IR 192"],
                "person_names": [{"tag": "0010,0010", "vr": "PN", "decoded_value": "Wang^XiaoDong"}],
            },
        }
        summary = {
            "modality": "OT", "sop_class_uid": "1.2.class", "transfer_syntax_uid": "1.2.syntax",
            "sop_instance_uid": "1.2.instance", "study_instance_uid": "1.2.study", "series_instance_uid": "1.2.series",
            "frame_count": 1, "rows": 2, "columns": 3,
        }
        info = {"sop_class_uid": "1.2.class", "transfer_syntax_uid": "1.2.syntax", "frame_count": 1, "rows": 2, "columns": 3}
        tags = [
            {"tag": "(0008,0005)", "vr": "CS", "value": {"type": "string", "value": "ISO_IR 192"}},
            {"tag": "(0010,0010)", "vr": "PN", "value": {"type": "string", "value": "Wang^XiaoDong"}},
        ]
        self.assertTrue(metadata_observation(summary, info, tags, expected)["passed"])
        tags[1]["value"]["value"] = "Wrong^Name"
        self.assertFalse(metadata_observation(summary, info, tags, expected)["passed"])

        metadata_only = {**expected, "image": {}}
        summary["frame_count"] = 1
        info["frame_count"] = 1
        tags[1]["value"]["value"] = "Wang^XiaoDong"
        self.assertTrue(metadata_observation(summary, info, tags, metadata_only)["passed"])

    def test_wsi_observation_proves_selected_tile_without_reconstruction(self) -> None:
        expected = {
            "image": {"rows": 2, "columns": 2},
            "expected_wsi_tiled_full": {"tiling": {"implicit_frame_positions": [
                {"frame_number": 2, "row_position": 1, "column_position": 3}
            ]}},
        }
        payload = {
            "frame_index": 1, "positioning_status": "positioned",
            "tile_rectangle": {"x": 2, "y": 0, "width": 2, "height": 2},
            "total_pixel_matrix": {"rows": 4, "columns": 4},
            "tile_row": 0, "tile_column": 1, "reconstruction_claimed": False,
        }
        observed = wsi_context_observations([payload], expected)
        self.assertTrue(observed["wsi_position"]["exact_evidence"])
        self.assertTrue(observed["wsi_minimap"]["passed"])
        self.assertFalse(observed["wsi_minimap"]["neighboring_tiles_decoded"])

    def test_segmentation_semantic_observation_checks_declared_frame_closure(self) -> None:
        expected = {"expected_semantics": {
            "segmentation_type": "BINARY", "segmentation_fractional_type": None,
            "maximum_fractional_value": None, "segment_sequence_items": 1,
            "referenced_frame_numbers": [1, 2], "source_sop_instance_uid": "1.2.source",
        }}
        payload = {"default_mode": "pixel_preview", "pixel_preview_preserves_stored_values": True, "context": {
            "kind": "segmentation", "segmentation_type": "BINARY",
            "segmentation_fractional_type": None, "maximum_fractional_value": None,
            "segments": [{"number": 1}],
            "frame_mappings": [
                {"segment_number": 1, "source_frame_numbers": [1], "source_sop_instance_uid": "1.2.source"},
                {"segment_number": 1, "source_frame_numbers": [2], "source_sop_instance_uid": "1.2.source"},
            ],
            "overlay": {"eligible": False, "reason": "geometry unavailable"},
        }}
        observed = semantic_context_observations(payload, expected)
        self.assertTrue(observed["segmentation_context"]["passed"])
        self.assertTrue(observed["segment_reference_closure"]["passed"])
        self.assertTrue(observed["segmentation_overlay"]["passed"])

    def test_parametric_semantic_observation_requires_explicit_rwvm(self) -> None:
        expected = {"expected_semantics": {"sample_type": "float32", "real_world_value_mapping": {
            "lut_label": "MAP", "slope": 2.0, "intercept": -1.0,
            "units": {"code_value": "1"}, "quantity_definition": {"code_value": "Q"},
        }}}
        payload = {"default_mode": "pixel_preview", "pixel_preview_preserves_stored_values": True, "context": {"kind": "parametric_map",
            "stored_value_type": "float32", "displayed_value_kind": "stored", "mappings": [{
                "label": "MAP", "slope": 2.0, "intercept": -1.0,
                "units": {"value": "1"}, "quantity": {"value": "Q"},
            }]}}
        observed = semantic_context_observations(payload, expected)
        self.assertTrue(observed["parametric_context"]["passed"])
        self.assertTrue(observed["rwvm"]["passed"])
        self.assertTrue(observed["stored_mapped"]["passed"])

    def test_semantic_overlay_and_dose_checks_require_declared_safe_state(self) -> None:
        expected = {"expected_semantics": {
            "pixel_min": 0, "pixel_max": 700,
            "rt_dose": {
                "dose_grid_scaling": "0.001", "dose_units": "GY",
                "dose_type": "PHYSICAL", "dose_summation_type": "RECORD",
            },
        }}
        payload = {"default_mode": "pixel_preview", "pixel_preview_preserves_stored_values": True, "context": {
            "kind": "rt_dose", "dose_grid_scaling": 0.001, "dose_units": "GY",
            "dose_type": "PHYSICAL", "dose_summation_type": "RECORD",
            "scaling_status": "available", "displayed_value_kind": "mapped",
            "geometry": {"grid_frame_offsets": [0.0]},
            "overlay": {"eligible": False, "reason": "geometry incompatible", "source_file_index": None},
        }}
        observed = semantic_context_observations(payload, expected)
        self.assertTrue(observed["dose_scaling"]["passed"])
        self.assertEqual(observed["dose_scaling"]["expected_mapped_bounds"], [0.0, 0.7])
        self.assertTrue(observed["dose_overlay"]["passed"])
        payload["context"]["overlay"] = {"eligible": True, "reason": "eligible", "source_file_index": 4}
        self.assertFalse(semantic_context_observations(payload, expected)["dose_overlay"]["passed"])

    def test_canonical_raw_bytes_masks_sign_extension_to_stored_bits(self) -> None:
        sign_extended = struct.pack("<HHHH", 0xFC00, 0xFFFF, 0x0000, 0x03FF)
        expected_stored = struct.pack("<HHHH", 0x0C00, 0x0FFF, 0x0000, 0x03FF)
        image = {"bits_allocated": 16, "bits_stored": 12}

        self.assertEqual(canonical_raw_bytes(sign_extended, image), expected_stored)
        self.assertEqual(
            hashlib.sha256(canonical_raw_bytes(sign_extended, image)).hexdigest(),
            hashlib.sha256(expected_stored).hexdigest(),
        )

    def test_canonical_raw_bytes_preserves_expanded_one_bit_samples(self) -> None:
        expanded = bytes((1, 0, 1, 0, 1, 0, 1, 0, 1))
        self.assertEqual(
            canonical_raw_bytes(expanded, {"bits_allocated": 1, "bits_stored": 1}),
            expanded,
        )

    def test_canonical_raw_bytes_packs_deflated_image_frame_samples(self) -> None:
        expanded = bytes((1, 0, 0, 1))
        image = {
            "bits_allocated": 1,
            "bits_stored": 1,
            "rows": 2,
            "columns": 2,
            "samples_per_pixel": 1,
        }
        self.assertEqual(
            canonical_raw_bytes(expanded, image, "1.2.840.10008.1.2.8.1"),
            b"\x09",
        )

    def test_raw_headers_match_declared_image_organization(self) -> None:
        image = {
            "rows": 2,
            "columns": 3,
            "bits_allocated": 16,
            "pixel_representation": 1,
            "samples_per_pixel": 1,
            "photometric_interpretation": "MONOCHROME2",
        }
        headers = {
            "x-frame-rows": "2",
            "x-frame-columns": "3",
            "x-frame-bits-allocated": "16",
            "x-frame-pixel-representation": "1",
            "x-frame-samples-per-pixel": "1",
            "x-frame-photometric-interpretation": "MONOCHROME2",
        }
        self.assertTrue(raw_header_observation(headers, image)["passed"])
        headers["x-frame-columns"] = "4"
        self.assertFalse(raw_header_observation(headers, image)["passed"])

    def test_navigation_frames_cover_required_positions_deterministically(self) -> None:
        first = deterministic_navigation_frames("case/a", 17)
        self.assertEqual(first, deterministic_navigation_frames("case/a", 17))
        self.assertTrue({0, 8, 16}.issubset(first))
        self.assertLessEqual(len(first), 4)

    def test_overlay_capability_requires_exact_declared_display_pixels(self) -> None:
        expected = {
            "image": {"rows": 2, "columns": 2},
            "expected_semantics": {"overlay_pattern": "2x2_diagonal_overlay"},
        }
        pixels = [
            (255, 255, 255, 255),
            (85, 85, 85, 255),
            (170, 170, 170, 255),
            (255, 255, 255, 255),
        ]
        observed = overlay_display_observation(expected, pixels)
        self.assertTrue(observed["exact_evidence"])
        self.assertEqual(observed["expected_white_indices"], [0, 3])
        self.assertNotIn(
            "read_overlay_plane",
            unprobed_capabilities(
                ["read_overlay_plane"], {"overlay_display": observed}
            ),
        )

        mismatch = overlay_display_observation(expected, pixels[:3] + [(0, 0, 0, 255)])
        self.assertFalse(mismatch["exact_evidence"])
        self.assertIn(
            "read_overlay_plane",
            unprobed_capabilities(
                ["read_overlay_plane"], {"overlay_display": mismatch}
            ),
        )

    def test_full_frame_shutter_records_non_regression_without_claiming_application(self) -> None:
        expected = {
            "image": {"rows": 2, "columns": 2},
            "expected_semantics": {
                "display_shutter": {
                    "shape": "RECTANGULAR",
                    "left_vertical_edge": "1",
                    "right_vertical_edge": "2",
                    "upper_horizontal_edge": "1",
                    "lower_horizontal_edge": "2",
                }
            },
        }
        pixels = [(value, value, value, 255) for value in (0, 85, 170, 255)]
        observed = shutter_display_observation(expected, pixels)
        self.assertTrue(observed["opening_covers_full_frame"])
        self.assertTrue(observed["non_regression_passed"])
        self.assertFalse(observed["exact_evidence"])
        self.assertIn("cannot prove outside-opening replacement", observed["caveat"])
        self.assertIn(
            "apply_display_shutter",
            unprobed_capabilities(
                ["apply_display_shutter"], {"display_shutter": observed}
            ),
        )

    def test_icc_probe_hashes_png_profile_without_claiming_color_transform(self) -> None:
        profile = b"synthetic ICC profile bytes"
        expected = {
            "expected_icc_profile": {
                "profile_sha256": hashlib.sha256(profile).hexdigest(),
                "profile_size_bytes": len(profile),
            }
        }
        observed = icc_profile_observation(
            expected, grayscale_png(bytes((0, 85, 170, 255)), 2, 2, profile)
        )
        self.assertTrue(observed["profile_present"])
        self.assertTrue(observed["exact_evidence"])
        self.assertFalse(observed["numeric_transform_verified"])
        self.assertFalse(observed["path_specific_mapping_verified"])
        self.assertIn(
            "apply_icc_profile",
            unprobed_capabilities(
                ["apply_icc_profile"], {"icc_profile": observed}
            ),
        )

    def test_reference_observation_requires_exact_identity_and_local_frames(self) -> None:
        expected = [
            {
                "relationship": "source_image",
                "sop_class_uid": "1.2.class",
                "sop_instance_uid": "1.2.instance",
                "series_instance_uid": "1.2.series",
                "frame_numbers": [1, 4],
                "source_path": "source/instance.dcm",
            }
        ]
        payload = {
            "references": [
                {
                    "relationship": "source_image",
                    "target": {
                        "sop_class_uid": "1.2.class",
                        "sop_instance_uid": "1.2.instance",
                        "series_instance_uid": "1.2.series",
                        "frame_numbers": [1, 4],
                        "segment_numbers": [],
                    },
                    "matches": [
                        {
                            "file_index": 9,
                            "path": "/corpus/extended/source/instance.dcm",
                            "sop_instance_uid": "1.2.instance",
                            "frame_indices": [0, 3],
                        }
                    ],
                }
            ]
        }
        observed = reference_observation(payload, expected)
        self.assertTrue(observed["identities_match"])
        self.assertTrue(observed["all_resolved"])
        self.assertTrue(observed["exact_evidence"])
        self.assertNotIn(
            "resolve_references",
            unprobed_capabilities(
                ["resolve_references"], {"references": observed}
            ),
        )

        payload["references"][0]["matches"][0]["frame_indices"] = [0]
        mismatch = reference_observation(payload, expected)
        self.assertFalse(mismatch["exact_evidence"])
        self.assertIn(
            "resolve_references",
            unprobed_capabilities(
                ["resolve_references"], {"references": mismatch}
            ),
        )

    def test_pixel_geometry_requires_exact_api_evidence_without_claiming_ui_rendering(self) -> None:
        expected = {
            "expected_nonsquare_spacing": {
                "pixel_spacing": {
                    "row_spacing_mm": 0.6,
                    "column_spacing_mm": 0.3,
                },
                "pixel_aspect_ratio": None,
            },
        }
        observed = pixel_geometry_observation(
            {"pixel_aspect_ratio": 2.0}, expected
        )

        self.assertEqual(observed["source"], "pixel_spacing")
        self.assertEqual(observed["expected_row_to_column_ratio"], 2.0)
        self.assertEqual(observed["observed_row_to_column_ratio"], 2.0)
        self.assertTrue(observed["exact_evidence"])
        self.assertTrue(observed["passed"])
        self.assertEqual(observed["evidence_scope"], "api_files_metadata")
        self.assertFalse(observed["ui_rendering_verified"])
        self.assertNotIn(
            "interpret_pixel_geometry",
            unprobed_capabilities(
                ["interpret_pixel_geometry"], {"pixel_geometry": observed}
            ),
        )

    def test_specialized_geometry_capabilities_emit_exact_tag_and_raw_evidence(self) -> None:
        def node(tag: str, value: object) -> dict:
            kind = "numbers" if isinstance(value, list) else "number" if isinstance(value, (int, float)) else "string"
            return {"tag": f"({tag})", "value": {"type": kind, "value": value}}

        nm_expected = {"expected_nm_multiframe": {
            "image_type": ["ORIGINAL", "PRIMARY", "STATIC", "EMISSION"],
            "counts_accumulated": 904,
            "actual_frame_duration_ms": 1000,
            "energy_window_vector": [1, 1, 2, 2],
            "number_of_energy_windows": 2,
            "detector_vector": [1, 2, 1, 2],
            "number_of_detectors": 2,
        }}
        nm_tags = [
            node("0008,0008", "ORIGINAL; PRIMARY; STATIC; EMISSION"),
            node("0018,0070", 904), node("0018,1242", 1000),
            node("0054,0010", [1, 1, 2, 2]), node("0054,0011", 2),
            node("0054,0020", [1, 2, 1, 2]), node("0054,0021", 2),
        ]
        nm = nm_dimensions_observation(nm_tags, nm_expected)
        self.assertTrue(nm["exact_evidence"])
        self.assertNotIn(
            "interpret_nm_dimensions",
            unprobed_capabilities(["interpret_nm_dimensions"], {"nm_dimensions": nm}),
        )

        us_expected = {"expected_us_multiframe": {
            "image_type": ["ORIGINAL", "PRIMARY", "ABDOMINAL", "0001"],
            "frame_time_ms": 100.0,
            "frame_relative_times_ms": [0.0, 100.0, 200.0, 300.0],
            "color_data_present": False,
            "lossy_image_compression": "00",
        }}
        us_tags = [
            node("0008,0008", "ORIGINAL; PRIMARY; ABDOMINAL; 0001"),
            node("0018,1063", 100.0), node("0028,0014", 0),
            node("0028,2110", "00"),
        ]
        frame_time = frame_time_observation({"frame_count": 4}, us_tags, us_expected)
        self.assertTrue(frame_time["exact_evidence"])

        projection_expected = {"expected_xa_projection": {
            "image_type": ["ORIGINAL", "PRIMARY", "SINGLE PLANE"],
            "body_part_examined": "HEART", "kvp": 80.0,
            "distance_source_to_detector_mm": 1200.0,
            "distance_source_to_patient_mm": 800.0,
            "estimated_radiographic_magnification_factor": 1.5,
            "exposure_mas": 4, "radiation_setting": "GR",
            "imager_pixel_spacing_mm": [0.2, 0.2],
            "pixel_intensity_relationship": "LIN",
            "lossy_image_compression": "00",
            "positioner_primary_angle_degrees": 15.0,
            "positioner_secondary_angle_degrees": -10.0,
        }}
        projection_tags = [
            node("0008,0008", "ORIGINAL; PRIMARY; SINGLE PLANE"),
            node("0018,0015", "HEART"), node("0018,0060", 80.0),
            node("0018,1110", 1200.0), node("0018,1111", 800.0),
            node("0018,1114", 1.5), node("0018,1152", 4),
            node("0018,1155", "GR"), node("0018,1164", [0.2, 0.2]),
            node("0028,1040", "LIN"), node("0028,2110", "00"),
            node("0018,1510", 15.0), node("0018,1511", -10.0),
        ]
        self.assertTrue(
            projection_geometry_observation(projection_tags, projection_expected)["exact_evidence"]
        )

        pet_expected = {
            "image": {"bits_allocated": 16, "pixel_representation": 0},
            "expected_pet_activity": {
                "image_type": ["ORIGINAL", "PRIMARY"],
                "actual_frame_duration_ms": 60000,
                "corrected_image": ["DCAL"], "rescale_intercept": 0.0,
                "rescale_slope": 2.5, "number_of_slices": 1,
                "series_type": ["STATIC", "IMAGE"], "units": "BQML",
                "counts_source": "EMISSION", "decay_correction": "NONE",
                "frame_reference_time_ms": 30000.0,
                "dose_calibration_factor": 1.0, "image_index": 1,
                "stored_values": [0, 100, 200, 400],
                "activity_values_bqml": [0.0, 250.0, 500.0, 1000.0],
            },
        }
        pet_tags = [
            node("0008,0008", "ORIGINAL; PRIMARY"), node("0018,1242", 60000),
            node("0028,0051", "DCAL"), node("0028,1052", 0.0),
            node("0028,1053", 2.5), node("0054,0081", 1),
            node("0054,1000", "STATIC; IMAGE"), node("0054,1001", "BQML"),
            node("0054,1002", "EMISSION"), node("0054,1102", "NONE"),
            node("0054,1300", 30000.0), node("0054,1322", 1.0),
            node("0054,1330", 1),
        ]
        pet = pet_activity_observation(pet_tags, struct.pack("<4H", 0, 100, 200, 400), pet_expected)
        self.assertTrue(pet["exact_evidence"])

    def test_pixel_aspect_ratio_manifest_variant_uses_vertical_horizontal_extent(self) -> None:
        expected = {
            "expected_nonsquare_spacing": {
                "pixel_spacing": None,
                "pixel_aspect_ratio": {
                    "vertical_extent": 2,
                    "horizontal_extent": 1,
                },
            },
        }
        observed = pixel_geometry_observation(
            {"pixel_aspect_ratio": 2.0}, expected
        )

        self.assertEqual(observed["source"], "pixel_aspect_ratio")
        self.assertTrue(observed["exact_evidence"])

    def test_pixel_geometry_remains_unprobed_without_an_exact_api_ratio(self) -> None:
        expected = {
            "expected_nonsquare_spacing": {
                "pixel_spacing": {
                    "row_spacing_mm": 0.6,
                    "column_spacing_mm": 0.3,
                },
            },
        }
        observed = pixel_geometry_observation(
            {"pixel_aspect_ratio": None}, expected
        )

        self.assertFalse(observed["exact_evidence"])
        self.assertFalse(observed["passed"])
        self.assertIn(
            "interpret_pixel_geometry",
            unprobed_capabilities(
                ["interpret_pixel_geometry"], {"pixel_geometry": observed}
            ),
        )

        mismatch = pixel_geometry_observation(
            {"pixel_aspect_ratio": 1.0}, expected
        )
        self.assertFalse(mismatch["exact_evidence"])
        self.assertIn(
            "interpret_pixel_geometry",
            unprobed_capabilities(
                ["interpret_pixel_geometry"], {"pixel_geometry": mismatch}
            ),
        )

    def test_rle_capability_is_backed_by_display_and_lossless_raw_probes(self) -> None:
        self.assertIn("decode_rle_lossless_pixels", PROBED_CAPABILITIES)
        self.assertIn("render_color", PROBED_CAPABILITIES)
        self.assertIn("render_palette_color", PROBED_CAPABILITIES)

    def test_lossy_metrics_use_manifest_recipe_and_suite_tolerance(self) -> None:
        expected = {
            "recipe": {"recipe_parameters": {"pixel_values": [255, 0, 0, 255]}},
            "validation": {"internal": [{
                "name": "jpeg_baseline_decoded_frame_tolerance",
                "message": "JPEG Baseline decoded samples are within +/-10 of the native source frame.",
            }]},
        }
        observed = lossy_metrics_observation(bytes([250, 4, 1, 255]), expected)

        self.assertEqual(observed["maximum_absolute_error"], {"observed": 5, "limit": 10.0})
        self.assertAlmostEqual(observed["overall_rmse"]["observed"], (42 / 4) ** 0.5)
        self.assertEqual(
            observed["overall_rmse"]["limit_basis"],
            "implied_by_suite_maximum_absolute_error_limit",
        )
        self.assertTrue(observed["passed"])

        failed = lossy_metrics_observation(bytes([244, 0, 0, 255]), expected)
        self.assertFalse(failed["passed"])

    def test_grayscale_capabilities_are_backed_by_display_and_raw_probes(self) -> None:
        self.assertTrue(
            {
                "apply_modality_lut",
                "apply_modality_rescale",
                "apply_rescale",
                "apply_voi_lut",
                "apply_window",
                "decode_jpeg_2000_lossless_pixels",
                "decode_jpeg_baseline_pixels",
                "decode_jpeg_ls_lossless_pixels",
                "decode_jpeg_xl_lossless_pixels",
                "decode_native_pixels",
                "render_grayscale",
            }.issubset(PROBED_CAPABILITIES)
        )

    def test_extended_native_numeric_patterns_are_automated(self) -> None:
        checkerboard = [
            (255, 255, 255, 255),
            (0, 0, 0, 255),
            (255, 255, 255, 255),
            (0, 0, 0, 255),
            (255, 255, 255, 255),
            (0, 0, 0, 255),
            (255, 255, 255, 255),
            (0, 0, 0, 255),
            (255, 255, 255, 255),
        ]
        self.assertEqual(
            validate_visual(
                "3x3x2_continuous_lsb_first_checkerboards", checkerboard
            )["status"],
            "passed",
        )

    def test_prepared_overlay_and_selected_wsi_tile_have_exact_oracles(self) -> None:
        overlay = [(255, 255, 255, 255)] * 4
        self.assertEqual(validate_visual("2x2_cr_overlay_lut_gradient", overlay)["status"], "passed")
        full_dynamic = [
            (255, 255, 255, 255),
            (85, 85, 85, 255),
            (170, 170, 170, 255),
            (255, 255, 255, 255),
        ]
        expected = {
            "image": {"rows": 2, "columns": 2},
            "expected_semantics": {
                "overlay_pattern": "2x2_diagonal_overlay",
            },
        }
        self.assertTrue(overlay_display_observation(expected, full_dynamic)["passed"])

    def test_visual_oracles_distinguish_frame_order_from_luminance_order(self) -> None:
        ascending = [(value, value, value, 255) for value in (0, 85, 170, 255)]
        self.assertEqual(
            validate_visual(
                "2x2x2_monochrome_i16_rle_lossless_gradient_reversed", ascending
            )["status"],
            "passed",
        )

    def test_visual_oracles_match_ultrasound_and_projection_patterns(self) -> None:
        ultrasound = [
            0, 16, 32, 48,
            16, 64, 80, 64,
            32, 80, 255, 80,
            48, 64, 80, 64,
        ]
        projection = [
            0, 16, 32, 48,
            16, 64, 96, 64,
            32, 96, 255, 96,
            48, 64, 96, 64,
        ]
        to_pixels = lambda values: [(value, value, value, 255) for value in values]
        self.assertEqual(
            validate_visual("four_frame_moving_ultrasound_echo", to_pixels(ultrasound))["status"],
            "passed",
        )
        self.assertEqual(
            validate_visual(
                "single_plane_synthetic_angiographic_projection", to_pixels(projection)
            )["status"],
            "passed",
        )
        red_tile = [(255, 0, 0, 255)] * 4
        self.assertEqual(
            validate_visual("4x4_tiled_full_red_green_blue_white_quadrants", red_tile)["status"],
            "passed",
        )
        gradient = [
            (0, 0, 0, 255),
            (1, 1, 1, 255),
            (128, 128, 128, 255),
            (255, 255, 255, 255),
        ]
        self.assertEqual(
            validate_visual("2x2_monochrome_u32_unsigned_boundaries", gradient)[
                "status"
            ],
            "passed",
        )

    def test_named_monochrome_gradient_families_are_automated(self) -> None:
        ascending = [
            (0, 0, 0, 255),
            (85, 85, 85, 255),
            (170, 170, 170, 255),
            (255, 255, 255, 255),
        ]
        descending = list(reversed(ascending))
        self.assertEqual(
            validate_visual("2x2_signed_ct_hu_gradient", ascending)["status"],
            "passed",
        )
        self.assertEqual(
            validate_visual(
                "2x2_inverse_monochrome_i16_rle_lossless_gradient", descending
            )["status"],
            "passed",
        )
        self.assertEqual(
            validate_visual("2x2_signed_ct_hu_gradient", descending)["status"],
            "failed",
        )

    def test_runner_supports_documented_direct_invocation(self) -> None:
        runner = Path(__file__).with_name("run.py")
        completed = subprocess.run(
            [sys.executable, str(runner), "--help"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)

    def test_png_decoder_and_smoke_gradient_check(self) -> None:
        width, height, pixels = png_pixels(grayscale_png(bytes((0, 85, 170, 255)), 2, 2))
        self.assertEqual((width, height), (2, 2))
        self.assertEqual(
            validate_visual("2x2_monochrome_gradient", pixels)["status"], "passed"
        )

    def test_color_pattern_aliases_and_ybr_422_chroma_pairs(self) -> None:
        quadrants = [
            (255, 0, 0, 255),
            (0, 255, 0, 255),
            (0, 0, 255, 255),
            (255, 255, 255, 255),
        ]
        self.assertEqual(
            validate_visual("2x2_palette_red_green_blue_white", quadrants)["status"],
            "passed",
        )
        subsampled = [
            (90, 91, 0, 255),
            (164, 165, 38, 255),
            (15, 14, 142, 255),
            (241, 240, 255, 255),
        ]
        self.assertEqual(
            validate_visual("2x2_ybr_full_422_red_green_blue_white", subsampled)["status"],
            "passed",
        )

    def test_series_observation_validates_geometry_and_gantry_capabilities(self) -> None:
        catalog = {
            "series": [{
                "id": "series-id",
                "study_instance_uid": "study",
                "series_instance_uid": "series",
                "frame_of_reference_uids": ["for"],
                "stacks": [{
                    "id": "stack-id",
                    "kind": "ordinary",
                    "warnings": [{"code": "gantry_tilt"}],
                    "frames": [
                        {
                            "virtual_index": 0,
                            "file_index": 7,
                            "frame_index": 0,
                            "source_path": "/scan/a.dcm",
                            "position_along_normal_mm": 0.0,
                        },
                        {
                            "virtual_index": 1,
                            "file_index": 9,
                            "frame_index": 0,
                            "source_path": "/scan/b.dcm",
                            "position_along_normal_mm": 5.0,
                        },
                    ],
                }],
            }],
        }
        observed = series_observation(
            catalog,
            {"index": 9},
            {
                "expected_capabilities": [
                    "sort_series_by_geometry",
                    "interpret_gantry_tilt",
                ],
                "expected_geometry": {"geometric_order_index": 2},
            },
        )
        self.assertTrue(observed["mapped"])
        self.assertTrue(observed["capabilities"]["sort_series_by_geometry"]["passed"])
        self.assertTrue(observed["capabilities"]["interpret_gantry_tilt"]["passed"])

    def test_report_validation_rejects_duplicate_result_identity(self) -> None:
        result = {
            "root": "smoke", "case_id": "classic/sc/example", "path": "x.dcm",
            "identity": {"file_sha256": "0" * 64}, "execution_safety": "safe",
            "compatibility": "verified",
        }
        report = {
            "detail_schema_version": "0.1.0", "generated_at": "now", "worklist": {},
            "viewer": {}, "run": {}, "results": [result, result],
            "summary": {"results": 2}, "artifacts": [], "validation": {},
        }
        with self.assertRaisesRegex(RuntimeError, "duplicate result"):
            validate_report(report)

    def test_json_schema_validator_enforces_required_and_strict_fields(self) -> None:
        schema = {
            "type": "object",
            "required": ["value"],
            "additionalProperties": False,
            "properties": {"value": {"type": "string", "pattern": "^[a-z]+$"}},
        }
        validate_json_schema({"value": "valid"}, schema)
        with self.assertRaisesRegex(RuntimeError, "pattern violation"):
            validate_json_schema({"value": "INVALID"}, schema)
        with self.assertRaisesRegex(RuntimeError, "additional-field violation"):
            validate_json_schema({"value": "valid", "extra": True}, schema)

    def test_normalization_removes_timings_run_identity_and_registry_indices(self) -> None:
        def report(registry_index: int, series_hash: str) -> dict:
            return {
                "detail_schema_version": "0.1.0",
                "worklist": {"content_sha256": "a" * 64},
                "viewer": {"sha256": "b" * 64},
                "run": {"selection": "smoke", "started_at": "variable"},
                "results": [
                    {
                        "case_id": "x",
                        "timings_ms": {"total": 2},
                        "observations": {
                            "references": {
                                "matches": [
                                    {
                                        "file_index": registry_index,
                                        "path": "/corpus/source.dcm",
                                        "sop_instance_uid": "1.2.3",
                                    }
                                ]
                            }
                        },
                        "http": {
                            "info": {
                                "elapsed_ms": 1,
                                "status": 200,
                                "path": f"/api/file/{registry_index}/info",
                                "body_sha256": "c" * 64,
                            },
                            "series_catalog": {
                                "elapsed_ms": 2,
                                "status": 200,
                                "path": "/api/series",
                                "body_sha256": series_hash,
                                "size_bytes": 100 + registry_index,
                            },
                            "references": {
                                "elapsed_ms": 3,
                                "status": 200,
                                "path": f"/api/file/{registry_index}/references",
                                "body_sha256": series_hash,
                                "size_bytes": 200 + registry_index,
                            },
                        },
                    },
                ],
                "summary": {"results": 1},
            }
        normalized = normalized_report(report(17, "d" * 64))
        self.assertNotIn("timings_ms", normalized["results"][0])
        self.assertNotIn("elapsed_ms", normalized["results"][0]["http"]["info"])
        self.assertEqual(
            normalized["results"][0]["http"]["info"]["path"],
            "/api/file/{mapped}/info",
        )
        self.assertNotIn(
            "file_index",
            normalized["results"][0]["observations"]["references"]["matches"][0],
        )
        self.assertEqual(
            normalized["results"][0]["observations"]["references"]["matches"][0][
                "sop_instance_uid"
            ],
            "1.2.3",
        )
        self.assertNotIn(
            "body_sha256", normalized["results"][0]["http"]["series_catalog"]
        )
        self.assertNotIn(
            "size_bytes", normalized["results"][0]["http"]["series_catalog"]
        )
        self.assertNotIn(
            "body_sha256", normalized["results"][0]["http"]["references"]
        )
        self.assertNotIn("size_bytes", normalized["results"][0]["http"]["references"])
        self.assertIn("body_sha256", normalized["results"][0]["http"]["info"])
        self.assertNotIn("started_at", normalized)
        self.assertEqual(
            normalized,
            normalized_report(report(104, "e" * 64)),
        )


if __name__ == "__main__":
    unittest.main()

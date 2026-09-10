#!/usr/bin/env python3
"""Execute a bounded, manifest-driven dcmview HTTP compatibility campaign."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import signal
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import zlib
from datetime import datetime, timezone
from decimal import Decimal
from pathlib import Path
from typing import Any, BinaryIO, Optional

try:
    from scripts.compatibility.artifact import ArtifactError, build_external_worklist
    from scripts.compatibility.reports import build_evidence_report, build_viewer_report
    from scripts.compatibility.worklist import CompatibilityError, sha256_file
except ModuleNotFoundError:
    from artifact import ArtifactError, build_external_worklist  # type: ignore[no-redef]
    from reports import build_evidence_report, build_viewer_report
    from worklist import CompatibilityError, sha256_file  # type: ignore[no-redef]


DETAIL_SCHEMA_VERSION = "0.1.0"
EXECUTION_OUTCOMES = {"safe", "timeout", "crash", "flaky"}
COMPATIBILITY_OUTCOMES = {
    "verified",
    "metadata_only",
    "unverified",
    "failure",
    "unavailable",
}
PROBED_CAPABILITIES = {
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
    "open_file",
    "read_metadata",
    "read_rt_image",
    "render_grayscale",
    "render_native_pixels",
    "render_compressed_pixels",
    "decode_rle_lossless_pixels",
    "render_color",
    "render_palette_color",
    "render_float_pixels",
    "render_double_float_pixels",
    "unpack_native_bit_packed_pixels",
    "navigate_multiframe",
    "sort_series_by_geometry",
    "parse_multiframe_functional_groups",
    "interpret_gantry_tilt",
    "organize_series_by_study_and_frame_of_reference",
}
CONDITIONAL_CAPABILITY_CHECKS = {
    "interpret_frame_time": "frame_time",
    "interpret_nm_dimensions": "nm_dimensions",
    "interpret_pet_activity": "pet_activity",
    "interpret_pixel_geometry": "pixel_geometry",
    "interpret_projection_geometry": "projection_geometry",
    "read_overlay_plane": "overlay_display",
    "resolve_frame_references": "references",
    "resolve_references": "references",
    "reconstruct_total_pixel_matrix": "wsi_position",
    "reconstruct_sparse_total_pixel_matrix": "wsi_position",
    "reconstruct_optical_path_matrices": "wsi_position",
    "reconstruct_wsi_pyramid": "wsi_position",
}


def _positive_finite_number(value: Any) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value > 0
    )

ASCENDING_GRAYSCALE_PATTERNS = {
    "1x1_monochrome_u16_tiny_maximum",
    "2x2_monochrome_gradient_with_empty_type2_attributes",
    "2x2_monochrome_gradient_with_iso2022_person_name",
    "2x2_monochrome_gradient_with_private_creator_blocks",
    "2x2_monochrome_gradient_with_sequence_length_variants",
    "2x2_monochrome_gradient_with_string_vr_boundaries",
    "2x2_monochrome_gradient_with_utf8_patient_name",
    "2x2_monochrome_i16_gradient",
    "2x2_monochrome_u16_gradient",
    "2x2_signed_ct_hu_gradient",
    "2x2_ultrasound_mono2_gradient",
    "2x3_monochrome_u16_rect_gradient",
    "3_slice_oblique_mr_gradient_stack",
    "3x3_monochrome_u16_odd_gradient",
    "single_member_enhanced_ct_concatenation_gradient",
    "two_2x2_monochrome_gradients_with_timezone_extrema",
    "two_frame_enhanced_ct_unsigned_gradient_stack",
    "two_frame_enhanced_mr_echo_gradient_stack",
    "two_frame_enhanced_mr_phase_velocity_encoding_stack",
    "two_frame_enhanced_mr_temporal_gradient_stack",
    "2x2_dx_mono2_12bit_display_shutter",
    "2x2_mammography_mono2_12bit_processing_gradient",
    "single_slice_mr_rle_lossless_gradient",
    "four_frame_nm_two_energy_windows_two_detectors",
    "pet_stored_values_rescaled_to_bqml",
    "2x3_monochrome_i16_rect_rle_lossless_centered_gradient",
    "2x2_monochrome_i16_rle_lossless_gradient",
    "1x1_monochrome_i16_rle_lossless_tiny_minimum",
    "3x3_monochrome_u16_odd_rle_lossless_gradient",
    "2x2_monochrome_u16_with_padding_value",
    "2x2_monochrome_u16_rle_lossless_with_padding_value",
    "2x3_monochrome_u16_rect_rle_lossless_gradient",
    "1x1_monochrome_u16_rle_lossless_tiny_maximum",
    "1x2_monochrome_rle_lossless_odd_fragment",
    "2x2_monochrome_u8_rle_lossless_with_padding_value",
    "2x2x2_monochrome_i16_rle_lossless_gradient_reversed",
    "3x3_monochrome_i16_odd_rle_lossless_centered_gradient",
    "2x2x2_monochrome_i16_rle_lossless_signed_padding_reversed",
    "2x2_monochrome_i16_rle_lossless_with_signed_padding_value",
    "2x2x2_monochrome_rle_lossless_gradient_reversed",
    "2x2x2_monochrome_u16_rle_lossless_gradient_reversed",
    "2x2x2_monochrome_u16_rle_lossless_padding_reversed",
    "2x2x2_monochrome_u8_rle_lossless_padding_reversed",
    "two_frame_fractional_probability_segmentation",
    "2x2x3_monochrome_rle_lossless_literal_repeat_reverse",
    "two_identical_pet_activity_frames_at_distinct_z_positions",
    "tiny_two_frame_rt_dose_grid",
    "4x4_monochrome_gradient",
}

DESCENDING_GRAYSCALE_PATTERNS = {
    "1x1_inverse_monochrome_i16_rle_lossless_tiny_minimum",
    "1x1_inverse_monochrome_u16_rle_lossless_tiny_maximum",
    "1x2_inverse_monochrome_rle_lossless_odd_fragment",
    "2x2_inverse_monochrome_i16_rle_lossless_gradient",
    "2x2_inverse_monochrome_i16_rle_lossless_with_signed_padding_value",
    "2x2_inverse_monochrome_rle_lossless_gradient",
    "2x2_inverse_monochrome_u16_rle_lossless_gradient",
    "2x2_inverse_monochrome_u16_rle_lossless_with_padding_value",
    "2x2x2_inverse_monochrome_i16_rle_lossless_gradient_reversed",
    "2x2x2_inverse_monochrome_i16_rle_lossless_signed_padding_reversed",
    "2x2x2_inverse_monochrome_rle_lossless_gradient_reversed",
    "2x2x2_inverse_monochrome_u16_rle_lossless_gradient_reversed",
    "2x2x2_inverse_monochrome_u16_rle_lossless_padding_reversed",
    "2x2x2_inverse_monochrome_u8_rle_lossless_padding_reversed",
    "2x3_inverse_monochrome_i16_rle_lossless_centered_gradient",
    "2x3_inverse_monochrome_u16_rle_lossless_gradient",
    "3x3_inverse_monochrome_i16_odd_rle_lossless_centered_gradient",
    "3x3_inverse_monochrome_u16_odd_rle_lossless_gradient",
    "2x2_mammography_mono1_12bit_gradient",
    "2x3_inverse_monochrome_i16_rect_rle_lossless_centered_gradient",
    "2x3_inverse_monochrome_u16_rect_rle_lossless_gradient",
    "2x2_inverse_monochrome_u8_rle_lossless_with_padding_value",
}
LOSSY_TRANSFER_SYNTAXES = {
    "1.2.840.10008.1.2.4.50",
    "1.2.840.10008.1.2.4.51",
}
RAW_HEADERS = (
    "x-frame-rows",
    "x-frame-columns",
    "x-frame-bits-allocated",
    "x-frame-pixel-representation",
    "x-frame-samples-per-pixel",
    "x-frame-photometric-interpretation",
    "x-frame-rescale-slope",
    "x-frame-rescale-intercept",
    "x-frame-default-wc",
    "x-frame-default-ww",
)


def canonical_raw_bytes(
    payload: bytes,
    image: dict[str, Any],
    transfer_syntax_uid: Optional[str] = None,
) -> bytes:
    """Normalize decoded samples to the suite's stored-bit byte convention."""
    bits_allocated = image.get("bits_allocated")
    bits_stored = image.get("bits_stored")
    if bits_allocated == 1 and transfer_syntax_uid == "1.2.840.10008.1.2.8.1":
        pixel_count = math.prod(
            value
            for value in (
                image.get("rows"),
                image.get("columns"),
                image.get("samples_per_pixel"),
            )
            if isinstance(value, int)
        )
        if pixel_count == len(payload) and all(sample in {0, 1} for sample in payload):
            packed = bytearray((pixel_count + 7) // 8)
            for index, sample in enumerate(payload):
                packed[index // 8] |= sample << (index % 8)
            return bytes(packed)
    if (
        isinstance(bits_allocated, int)
        and isinstance(bits_stored, int)
        and bits_allocated in {8, 16}
        and 0 < bits_stored < bits_allocated
    ):
        sample_bytes = bits_allocated // 8
        if len(payload) % sample_bytes:
            return payload
        mask = (1 << bits_stored) - 1
        normalized = bytearray(len(payload))
        for offset in range(0, len(payload), sample_bytes):
            value = int.from_bytes(payload[offset : offset + sample_bytes], "little") & mask
            normalized[offset : offset + sample_bytes] = value.to_bytes(sample_bytes, "little")
        return bytes(normalized)
    return payload


def raw_header_observation(headers: dict[str, str], image: dict[str, Any]) -> dict[str, Any]:
    declared = {
        "x-frame-rows": image.get("rows"),
        "x-frame-columns": image.get("columns"),
        "x-frame-bits-allocated": image.get("bits_allocated"),
        "x-frame-pixel-representation": image.get("pixel_representation"),
        "x-frame-samples-per-pixel": image.get("samples_per_pixel"),
        "x-frame-photometric-interpretation": image.get("photometric_interpretation"),
    }
    expected = {name: str(value) for name, value in declared.items() if value is not None}
    observed = {name: headers.get(name) for name in expected}
    return {
        "expected": expected,
        "observed": observed,
        "passed": observed == expected,
    }


def lossy_metrics_observation(payload: bytes, expected: dict[str, Any]) -> dict[str, Any]:
    recipe = ((expected.get("recipe") or {}).get("recipe_parameters") or {})
    reference = recipe.get("pixel_values")
    tolerance = None
    for validation in (expected.get("validation") or {}).get("internal") or []:
        if validation.get("name") != "jpeg_baseline_decoded_frame_tolerance":
            continue
        match = re.search(r"within \+/-([0-9]+(?:\.[0-9]+)?)", validation.get("message") or "")
        if match:
            tolerance = float(match.group(1))
            break
    if not isinstance(reference, list) or not all(isinstance(value, int) for value in reference):
        return {"passed": False, "error": "manifest lacks integer recipe pixel values"}
    if tolerance is None:
        return {"passed": False, "error": "manifest lacks a JPEG Baseline tolerance"}
    observed = list(payload)
    if len(observed) != len(reference):
        return {
            "passed": False,
            "expected_sample_count": len(reference),
            "observed_sample_count": len(observed),
        }
    errors = [abs(actual - declared) for actual, declared in zip(observed, reference)]
    maximum_error = max(errors, default=0)
    rmse = math.sqrt(sum(error * error for error in errors) / len(errors)) if errors else 0.0
    return {
        "evidence_scope": "decoded_raw_samples_against_manifest_recipe",
        "sample_count": len(errors),
        "maximum_absolute_error": {"observed": maximum_error, "limit": tolerance},
        "overall_rmse": {
            "observed": rmse,
            "limit": tolerance,
            "limit_basis": "implied_by_suite_maximum_absolute_error_limit",
        },
        "passed": maximum_error <= tolerance and rmse <= tolerance,
    }
def _tag_key(value: str) -> str:
    return f"({value.strip().strip('()').upper()})"


def _tag_scalar(node: dict[str, Any]) -> Any:
    value = node.get("value") or {}
    if value.get("type") in {"string", "number", "numbers"}:
        return value.get("value")
    return None


def _metadata_tag_expectations(expected_metadata: dict[str, Any]) -> list[dict[str, Any]]:
    expectations: list[dict[str, Any]] = []

    def visit(value: Any) -> None:
        if isinstance(value, dict):
            tag = value.get("tag")
            if isinstance(tag, str) and "decoded_value" in value:
                expectations.append({"tag": _tag_key(tag), "vr": value.get("vr"), "expected": value["decoded_value"]})
            elif isinstance(tag, str) and "decoded_values" in value:
                decoded = value["decoded_values"]
                expected_value: Any = decoded
                if value.get("vr") not in {"DS", "IS"}:
                    joined = "; ".join(str(item) for item in decoded)
                    expected_value = joined[:256] + ("…" if len(joined) > 256 else "")
                expectations.append({"tag": _tag_key(tag), "vr": value.get("vr"), "expected": expected_value})
            sequence_tag = value.get("sequence_tag")
            if isinstance(sequence_tag, str) and isinstance(value.get("decoded_items"), list):
                expectations.append({
                    "tag": _tag_key(sequence_tag),
                    "vr": "SQ",
                    "expected_items": value["decoded_items"],
                })
            creator_tag = value.get("creator_tag")
            if isinstance(creator_tag, str) and isinstance(value.get("creator_id"), str):
                expectations.append({
                    "tag": _tag_key(creator_tag),
                    "vr": value.get("vr"),
                    "expected": value["creator_id"],
                })
            for nested in value.values():
                visit(nested)
        elif isinstance(value, list):
            for nested in value:
                visit(nested)

    visit(expected_metadata)
    for row in expected_metadata.get("empty_type2_attributes", []):
        expectations.append({"tag": _tag_key(row["tag"]), "vr": row.get("vr"), "expected": ""})
    character_sets = expected_metadata.get("specific_character_sets")
    if isinstance(character_sets, list):
        expectations.append({"tag": "(0008,0005)", "vr": "CS", "expected": "; ".join(str(value) for value in character_sets)})
    unique = {(row["tag"], row["vr"]): row for row in expectations}
    return list(unique.values())


def metadata_observation(
    file_summary: dict[str, Any], info: Any, tags: Any, expected: dict[str, Any]
) -> dict[str, Any]:
    if not isinstance(info, dict) or not isinstance(tags, list):
        return {"passed": False, "error": "metadata endpoints did not return typed payloads"}
    dicom = expected.get("dicom") or {}
    uids = expected.get("uids") or {}
    image = expected.get("image") or {}
    summary_expected: dict[str, Any] = {
        "sop_instance_uid": uids.get("sop_instance_uid"),
        "study_instance_uid": uids.get("study_instance_uid"),
        "series_instance_uid": uids.get("series_instance_uid"),
        "sop_class_uid": dicom.get("sop_class_uid"),
        "transfer_syntax_uid": dicom.get("transfer_syntax_uid"),
        "modality": dicom.get("modality"),
    }
    info_expected: dict[str, Any] = {
        "sop_class_uid": dicom.get("sop_class_uid"),
        "transfer_syntax_uid": dicom.get("transfer_syntax_uid"),
    }
    if image:
        image_fields = {
            "frame_count": image.get("frames"),
            "rows": image.get("rows"),
            "columns": image.get("columns"),
        }
        summary_expected.update(image_fields)
        info_expected.update(image_fields)

    def compare_fields(source: dict[str, Any], declared: dict[str, Any]) -> dict[str, Any]:
        return {
            key: {"expected": value, "observed": source.get(key), "passed": source.get(key) == value}
            for key, value in declared.items()
            if value is not None
        }

    summary_results = compare_fields(file_summary, summary_expected)
    info_results = compare_fields(info, info_expected)
    tag_index = {row.get("tag"): row for row in tags if isinstance(row, dict)}
    tag_results = []
    for expectation in _metadata_tag_expectations(expected.get("expected_metadata") or {}):
        node = tag_index.get(expectation["tag"])
        if "expected_items" in expectation:
            sequence = (node or {}).get("value") or {}
            items = sequence.get("items") if sequence.get("type") == "sequence" else None
            observed_items = []
            if isinstance(items, list):
                for item in items:
                    by_keyword = {child.get("keyword"): _tag_scalar(child) for child in item}
                    observed_items.append({
                        "code_meaning": by_keyword.get("CodeMeaning"),
                        "code_value": by_keyword.get("CodeValue"),
                        "coding_scheme_designator": by_keyword.get("CodingSchemeDesignator"),
                    })
            tag_results.append({
                **expectation,
                "observed_items": observed_items,
                "passed": isinstance(node, dict)
                and node.get("vr") == "SQ"
                and observed_items == expectation["expected_items"],
            })
            continue
        observed = _tag_scalar(node) if isinstance(node, dict) else None
        expected_value = expectation["expected"]
        if expectation["vr"] in {"DS", "IS"} and isinstance(expected_value, list):
            expected_value = [float(value) for value in expected_value]
            if len(expected_value) == 1:
                expected_value = expected_value[0]
        tag_results.append({
            **expectation,
            "expected": expected_value,
            "observed": observed,
            "passed": isinstance(node, dict) and node.get("vr") == expectation["vr"] and observed == expected_value,
        })
    passed = all(row["passed"] for row in summary_results.values()) and all(row["passed"] for row in info_results.values()) and all(row["passed"] for row in tag_results)
    return {
        "passed": passed,
        "evidence_scope": "manifest_identity_image_fields_and_declared_api_visible_tags",
        "summary_fields": summary_results,
        "info_fields": info_results,
        "declared_tags": tag_results,
    }


def deterministic_navigation_frames(case_id: str, frame_count: int) -> list[int]:
    if frame_count <= 0:
        return []
    digest = hashlib.sha256(case_id.encode("utf-8")).digest()
    random_index = int.from_bytes(digest[:8], "big") % frame_count
    return sorted({0, frame_count // 2, frame_count - 1, random_index})


def semantic_context_observations(
    payload: Any, expected: dict[str, Any]
) -> dict[str, Any]:
    semantics = expected.get("expected_semantics") or {}
    if not isinstance(payload, dict) or not isinstance(payload.get("context"), dict):
        return {"semantic_context": {"passed": False, "error": "missing semantic payload"}}
    context = payload["context"]
    kind = context.get("kind")
    observations: dict[str, Any] = {}
    default_ok = (
        payload.get("default_mode") == "pixel_preview"
        and payload.get("pixel_preview_preserves_stored_values") is True
    )
    if kind == "segmentation":
        declared_segments = {row.get("number") for row in context.get("segments", [])}
        mappings = context.get("frame_mappings", [])
        expected_frames = semantics.get("referenced_frame_numbers") or []
        observed_frames = sorted({frame for row in mappings for frame in row.get("source_frame_numbers", [])})
        segment_context = (
            context.get("segmentation_type") == semantics.get("segmentation_type")
            and context.get("segmentation_fractional_type") == semantics.get("segmentation_fractional_type")
            and context.get("maximum_fractional_value") == semantics.get("maximum_fractional_value")
            and len(context.get("segments", [])) == semantics.get("segment_sequence_items")
        )
        closure = (
            bool(mappings)
            and all(row.get("segment_number") in declared_segments for row in mappings)
            and observed_frames == expected_frames
            and all(row.get("source_sop_instance_uid") == semantics.get("source_sop_instance_uid") for row in mappings)
        )
        overlay = context.get("overlay") or {}
        expected_overlay = semantics.get("overlay_eligible", False)
        observations.update({
            "segmentation_context": {"passed": segment_context, "observed": context},
            "segment_reference_closure": {"passed": closure, "expected_frames": expected_frames, "observed_frames": observed_frames},
            "segmentation_overlay": {
                "passed": overlay.get("eligible") is expected_overlay
                and bool(overlay.get("reason"))
                and ((isinstance(overlay.get("source_file_index"), int)) if expected_overlay else overlay.get("source_file_index") is None),
                "expected_eligible": expected_overlay,
                "observed": overlay,
            },
        })
    elif kind == "parametric_map":
        expected_mapping = semantics.get("real_world_value_mapping") or {}
        mappings = context.get("mappings") or []
        mapping = mappings[0] if mappings else {}
        units = mapping.get("units") or {}
        quantity = mapping.get("quantity") or {}
        observations.update({
            "parametric_context": {"passed": context.get("stored_value_type") == semantics.get("sample_type"), "observed": context},
            "rwvm": {"passed": bool(mappings)
                     and mapping.get("label") == expected_mapping.get("lut_label")
                     and mapping.get("slope") == expected_mapping.get("slope")
                     and mapping.get("intercept") == expected_mapping.get("intercept")
                     and units.get("value") == (expected_mapping.get("units") or {}).get("code_value")
                     and quantity.get("value") == (expected_mapping.get("quantity_definition") or {}).get("code_value"),
                     "observed": mappings},
            "stored_mapped": {"passed": default_ok and context.get("displayed_value_kind") == "stored", "default_mode": payload.get("default_mode"), "semantic_value_kind": context.get("displayed_value_kind")},
        })
    elif kind == "rt_dose":
        expected_dose = semantics.get("rt_dose") or {}
        try:
            expected_scaling = float(expected_dose["dose_grid_scaling"])
        except (KeyError, TypeError, ValueError):
            expected_scaling = None
        geometry = context.get("geometry") or {}
        overlay = context.get("overlay") or {}
        expected_overlay = semantics.get("overlay_eligible", False)
        pixel_min = semantics.get("pixel_min")
        pixel_max = semantics.get("pixel_max")
        mapped_bounds = (
            [
                float(Decimal(str(pixel_min)) * Decimal(str(expected_scaling))),
                float(Decimal(str(pixel_max)) * Decimal(str(expected_scaling))),
            ]
            if expected_scaling is not None and isinstance(pixel_min, (int, float)) and isinstance(pixel_max, (int, float))
            else None
        )
        observations.update({
            "dose_context": {"passed": context.get("dose_units") == expected_dose.get("dose_units")
                             and context.get("dose_type") == expected_dose.get("dose_type")
                             and context.get("dose_summation_type") == expected_dose.get("dose_summation_type")
                             and "grid_frame_offsets" in geometry, "observed": context},
            "dose_scaling": {
                "passed": expected_scaling is not None
                and context.get("dose_grid_scaling") == expected_scaling
                and context.get("scaling_status") == "available"
                and context.get("displayed_value_kind") == "mapped"
                and mapped_bounds is not None,
                "expected": expected_scaling,
                "observed": context.get("dose_grid_scaling"),
                "expected_mapped_bounds": mapped_bounds,
            },
            "dose_overlay": {
                "passed": overlay.get("eligible") is expected_overlay
                and bool(overlay.get("reason"))
                and ((isinstance(overlay.get("source_file_index"), int)) if expected_overlay else overlay.get("source_file_index") is None),
                "expected_eligible": expected_overlay,
                "observed": overlay,
            },
        })
    else:
        observations["semantic_context"] = {"passed": False, "observed_kind": kind}
    observations["semantic_raw_identity"] = {"passed": False, "context_contract_passed": default_ok}
    return observations


def wsi_context_observations(
    payloads: list[dict[str, Any]], expected: dict[str, Any]
) -> dict[str, Any]:
    image = expected.get("image") or {}
    full = expected.get("expected_wsi_tiled_full") or {}
    sparse = expected.get("expected_wsi_tiled_sparse") or {}
    multiple = expected.get("expected_wsi_multiple_optical_paths") or {}
    exact_positions = {
        row["frame_number"] - 1: row
        for row in (full.get("tiling") or {}).get("implicit_frame_positions", [])
    }
    exact_positions.update({
        row["frame_number"] - 1: row
        for row in sparse.get("per_frame_functional_groups", [])
    })
    optical_by_frame: dict[int, str] = {}
    for path in multiple.get("optical_paths", []):
        start, end = path.get("frame_ordinal_range", [0, -1])
        for frame in range(max(start - 1, 0), max(end, 0)):
            optical_by_frame[frame] = path.get("identifier")

    position_results = []
    minimap_results = []
    for payload in payloads:
        frame = payload.get("frame_index")
        rectangle = payload.get("tile_rectangle") or {}
        matrix = payload.get("total_pixel_matrix") or {}
        expected_position = exact_positions.get(frame)
        expected_optical = optical_by_frame.get(frame)
        positioned = payload.get("positioning_status") == "positioned" and bool(rectangle)
        exact = True
        if expected_position is not None:
            exact = (
                rectangle.get("x") == expected_position.get("column_position") - 1
                and rectangle.get("y") == expected_position.get("row_position") - 1
                and payload.get("tile_row") == (expected_position.get("row_position") - 1) // image.get("rows", 1)
                and payload.get("tile_column") == (expected_position.get("column_position") - 1) // image.get("columns", 1)
            )
        if expected_optical is not None:
            exact = exact and (payload.get("optical_path") or {}).get("identifier") == expected_optical
        position_results.append({"frame": frame, "passed": positioned and exact, "observed": payload})
        inside = (
            isinstance(matrix.get("rows"), int) and isinstance(matrix.get("columns"), int)
            and isinstance(rectangle.get("x"), int) and isinstance(rectangle.get("y"), int)
            and isinstance(rectangle.get("width"), int) and isinstance(rectangle.get("height"), int)
            and rectangle["x"] >= 0 and rectangle["y"] >= 0
            and rectangle["x"] + rectangle["width"] <= matrix["columns"]
            and rectangle["y"] + rectangle["height"] <= matrix["rows"]
            and payload.get("reconstruction_claimed") is False
        )
        minimap_results.append({"frame": frame, "passed": inside})
    return {
        "wsi_position": {"exact_evidence": bool(position_results) and all(row["passed"] for row in position_results), "passed": bool(position_results) and all(row["passed"] for row in position_results), "results": position_results},
        "wsi_minimap": {"passed": bool(minimap_results) and all(row["passed"] for row in minimap_results), "metadata_only": True, "neighboring_tiles_decoded": False, "results": minimap_results},
    }


class CampaignError(RuntimeError):
    """Raised when a campaign invariant cannot be established."""


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def json_bytes(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode("utf-8")


def artifact(path: Path, kind: str) -> dict[str, Any]:
    return {
        "kind": kind,
        "path": str(path.resolve()),
        "sha256": sha256_file(path),
        "size_bytes": path.stat().st_size,
    }


class CapturedProcess:
    def __init__(self, command: list[str], environment: dict[str, str], directory: Path):
        self.command = command
        self.directory = directory
        self.process = subprocess.Popen(
            command,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=directory,
            env=environment,
            start_new_session=True,
            bufsize=0,
        )
        assert self.process.stdout is not None and self.process.stderr is not None
        self._stdout = bytearray()
        self._stderr = bytearray()
        self._condition = threading.Condition()
        self._threads = [
            threading.Thread(target=self._drain, args=(self.process.stdout, self._stdout), daemon=True),
            threading.Thread(target=self._drain, args=(self.process.stderr, self._stderr), daemon=True),
        ]
        for thread in self._threads:
            thread.start()

    def _drain(self, stream: BinaryIO, target: bytearray) -> None:
        while True:
            chunk = stream.read(4096)
            if not chunk:
                break
            with self._condition:
                target.extend(chunk)
                self._condition.notify_all()

    def wait_for_startup_url(self, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        consumed = 0
        while time.monotonic() < deadline:
            with self._condition:
                data = bytes(self._stdout)
                lines = data[consumed:].splitlines(keepends=True)
                complete = lines if data.endswith((b"\n", b"\r")) else lines[:-1]
                for line in complete:
                    consumed += len(line)
                    try:
                        event = json.loads(line.decode("utf-8"))
                    except (UnicodeDecodeError, json.JSONDecodeError):
                        continue
                    if event.get("type") == "server_started" and isinstance(event.get("url"), str):
                        return event["url"].rstrip("/")
                if self.process.poll() is not None:
                    raise CampaignError(
                        f"dcmview exited before startup with code {self.process.returncode}"
                    )
                self._condition.wait(timeout=min(0.1, max(0.0, deadline - time.monotonic())))
        raise TimeoutError(f"dcmview startup exceeded {timeout:.1f}s")

    def shutdown(self, timeout: float) -> tuple[int, bool]:
        forced = False
        if self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                forced = True
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=max(timeout, 1.0))
        for thread in self._threads:
            thread.join(timeout=1.0)
        return int(self.process.returncode or 0), forced

    def write_logs(self, stdout_path: Path, stderr_path: Path) -> None:
        stdout_path.write_bytes(bytes(self._stdout))
        stderr_path.write_bytes(bytes(self._stderr))


def http_request(base_url: str, path: str, timeout: float) -> dict[str, Any]:
    started = time.monotonic()
    request = urllib.request.Request(f"{base_url}{path}", method="GET")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            status = response.status
            headers = {key.lower(): value for key, value in response.headers.items()}
    except urllib.error.HTTPError as error:
        body = error.read()
        status = error.code
        headers = {key.lower(): value for key, value in error.headers.items()}
    elapsed = round((time.monotonic() - started) * 1000, 3)
    parsed: Any = None
    if headers.get("content-type", "").split(";", 1)[0] == "application/json":
        try:
            parsed = json.loads(body.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError):
            parsed = None
    return {
        "path": path,
        "status": status,
        "content_type": headers.get("content-type"),
        "headers": headers,
        "body": body,
        "json": parsed,
        "body_sha256": hashlib.sha256(body).hexdigest(),
        "elapsed_ms": elapsed,
    }


def evidence(response: dict[str, Any], header_names: tuple[str, ...] = ()) -> dict[str, Any]:
    return {
        "path": response["path"],
        "status": response["status"],
        "content_type": response["content_type"],
        "headers": {
            name: response["headers"][name]
            for name in header_names
            if name in response["headers"]
        },
        "body_sha256": response["body_sha256"],
        "size_bytes": len(response["body"]),
        "elapsed_ms": response["elapsed_ms"],
    }


def png_pixels(payload: bytes) -> tuple[int, int, list[tuple[int, int, int, int]]]:
    chunks = png_chunks(payload)
    width = height = bit_depth = color_type = None
    compressed = bytearray()
    for kind, data in chunks:
        if kind == b"IHDR":
            width, height, bit_depth, color_type, compression, filtering, interlace = struct.unpack(
                ">IIBBBBB", data
            )
            if compression or filtering or interlace:
                raise ValueError("unsupported PNG encoding")
        elif kind == b"IDAT":
            compressed.extend(data)
    if None in (width, height, bit_depth, color_type) or bit_depth != 8:
        raise ValueError("unsupported PNG header")
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError(f"unsupported PNG color type {color_type}")
    raw = zlib.decompress(bytes(compressed))
    stride = int(width) * channels
    rows: list[bytes] = []
    prior = bytes(stride)
    cursor = 0
    for _ in range(int(height)):
        filter_type = raw[cursor]
        scanline = bytearray(raw[cursor + 1 : cursor + 1 + stride])
        cursor += stride + 1
        for index in range(stride):
            left = scanline[index - channels] if index >= channels else 0
            above = prior[index]
            upper_left = prior[index - channels] if index >= channels else 0
            if filter_type == 1:
                scanline[index] = (scanline[index] + left) & 255
            elif filter_type == 2:
                scanline[index] = (scanline[index] + above) & 255
            elif filter_type == 3:
                scanline[index] = (scanline[index] + ((left + above) // 2)) & 255
            elif filter_type == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                predictor = (left, above, upper_left)[distances.index(min(distances))]
                scanline[index] = (scanline[index] + predictor) & 255
            elif filter_type != 0:
                raise ValueError(f"unsupported PNG filter {filter_type}")
        prior = bytes(scanline)
        rows.append(prior)
    pixels: list[tuple[int, int, int, int]] = []
    for row in rows:
        for index in range(0, len(row), channels):
            values = row[index : index + channels]
            if color_type == 0:
                pixels.append((values[0], values[0], values[0], 255))
            elif color_type == 2:
                pixels.append((values[0], values[1], values[2], 255))
            elif color_type == 4:
                pixels.append((values[0], values[0], values[0], values[1]))
            else:
                pixels.append(tuple(values))  # type: ignore[arg-type]
    return int(width), int(height), pixels


def png_chunks(payload: bytes) -> list[tuple[bytes, bytes]]:
    if payload[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("response is not a PNG")
    offset = 8
    chunks: list[tuple[bytes, bytes]] = []
    while offset + 12 <= len(payload):
        length = struct.unpack(">I", payload[offset : offset + 4])[0]
        kind = payload[offset + 4 : offset + 8]
        chunk_end = offset + 12 + length
        if chunk_end > len(payload):
            raise ValueError(f"truncated PNG {kind.decode('latin-1')} chunk")
        data = payload[offset + 8 : offset + 8 + length]
        expected_crc = struct.unpack(">I", payload[offset + 8 + length : chunk_end])[0]
        if zlib.crc32(kind + data) != expected_crc:
            raise ValueError(f"invalid PNG {kind.decode('latin-1')} chunk CRC")
        chunks.append((kind, data))
        offset = chunk_end
        if kind == b"IEND":
            break
    if not chunks or chunks[-1][0] != b"IEND":
        raise ValueError("PNG is missing IEND")
    return chunks


def png_icc_profile(payload: bytes) -> tuple[str, bytes] | None:
    profiles = [data for kind, data in png_chunks(payload) if kind == b"iCCP"]
    if not profiles:
        return None
    if len(profiles) != 1:
        raise ValueError("PNG contains multiple iCCP chunks")
    data = profiles[0]
    separator = data.find(b"\0")
    if separator <= 0 or separator + 2 > len(data):
        raise ValueError("PNG iCCP chunk has an invalid profile name")
    if data[separator + 1] != 0:
        raise ValueError("PNG iCCP chunk uses an unsupported compression method")
    try:
        name = data[:separator].decode("latin-1")
    except UnicodeDecodeError as error:
        raise ValueError("PNG iCCP profile name is invalid") from error
    return name, zlib.decompress(data[separator + 2 :])


def overlay_display_observation(
    expected: dict[str, Any], pixels: list[tuple[int, int, int, int]]
) -> dict[str, Any]:
    semantics = expected.get("expected_semantics") or {}
    image = expected.get("image") or {}
    exact_contract = (
        semantics.get("overlay_pattern") == "2x2_diagonal_overlay"
        and image.get("rows") == 2
        and image.get("columns") == 2
    )
    expected_white_indices = [0, 3] if exact_contract else []
    observed_white_indices = [
        index for index, pixel in enumerate(pixels) if pixel[:3] == (255, 255, 255)
    ]
    passed = (
        exact_contract
        and len(pixels) == 4
        and all(pixels[index][:3] == (255, 255, 255) for index in expected_white_indices)
        and all(pixels[index][:3] != (255, 255, 255) for index in (1, 2))
    )
    return {
        "evidence_scope": "decoded_display_png_pixels",
        "declared_pattern": semantics.get("overlay_pattern"),
        "expected_white_indices": expected_white_indices,
        "observed_white_indices": observed_white_indices,
        "exact_evidence": passed,
        "passed": passed,
        "caveat": None if exact_contract else "manifest does not declare a supported exact overlay pattern",
    }


def shutter_display_observation(
    expected: dict[str, Any], pixels: list[tuple[int, int, int, int]]
) -> dict[str, Any]:
    semantics = expected.get("expected_semantics") or {}
    shutter = semantics.get("display_shutter") or {}
    image = expected.get("image") or {}
    try:
        bounds = {
            "left": int(shutter.get("left_vertical_edge")),
            "right": int(shutter.get("right_vertical_edge")),
            "upper": int(shutter.get("upper_horizontal_edge")),
            "lower": int(shutter.get("lower_horizontal_edge")),
        }
    except (TypeError, ValueError):
        bounds = None
    full_frame = bounds == {
        "left": 1,
        "right": image.get("columns"),
        "upper": 1,
        "lower": image.get("rows"),
    }
    luminance = [round(0.2126 * r + 0.7152 * g + 0.0722 * b, 3) for r, g, b, _ in pixels]
    non_regression = (
        full_frame
        and len(pixels) == image.get("rows", 0) * image.get("columns", 0)
        and luminance == sorted(luminance)
    )
    return {
        "evidence_scope": "decoded_display_png_pixels",
        "shape": shutter.get("shape"),
        "declared_bounds": bounds,
        "opening_covers_full_frame": full_frame,
        "non_regression_passed": non_regression,
        "exact_evidence": False,
        "passed": non_regression,
        "caveat": (
            "the prepared rectangular opening covers the full frame, so display pixels prove "
            "bounds-preserving non-regression but cannot prove outside-opening replacement"
        ),
    }


def icc_profile_observation(expected: dict[str, Any], payload: bytes) -> dict[str, Any]:
    contract = expected.get("expected_icc_profile") or {}
    expected_hash = contract.get("profile_sha256")
    expected_size = contract.get("profile_size_bytes")
    try:
        profile = png_icc_profile(payload)
    except (ValueError, zlib.error) as error:
        return {
            "evidence_scope": "display_png_iccp_chunk",
            "expected_sha256": expected_hash,
            "expected_size_bytes": expected_size,
            "observed_sha256": None,
            "observed_size_bytes": None,
            "profile_present": False,
            "exact_evidence": False,
            "passed": False,
            "error": str(error),
            "numeric_transform_verified": False,
            "path_specific_mapping_verified": False,
        }
    if profile is None:
        name = None
        profile_bytes = None
    else:
        name, profile_bytes = profile
    observed_hash = hashlib.sha256(profile_bytes).hexdigest() if profile_bytes is not None else None
    observed_size = len(profile_bytes) if profile_bytes is not None else None
    exact_contract = isinstance(expected_hash, str) and isinstance(expected_size, int)
    passed = exact_contract and observed_hash == expected_hash and observed_size == expected_size
    return {
        "evidence_scope": "display_png_iccp_chunk",
        "profile_name": name,
        "profile_present": profile_bytes is not None,
        "expected_sha256": expected_hash,
        "observed_sha256": observed_hash,
        "expected_size_bytes": expected_size,
        "observed_size_bytes": observed_size,
        "exact_evidence": passed,
        "passed": passed,
        "numeric_transform_verified": False,
        "path_specific_mapping_verified": False,
        "caveat": (
            "byte-identical iCCP preservation does not prove a numeric color transform or "
            "frame-to-optical-path profile mapping"
        ),
    }


def validate_visual(pattern: Optional[str], pixels: list[tuple[int, int, int, int]]) -> dict[str, Any]:
    if not pattern:
        return {"status": "not_declared", "pattern": None}
    luminance = [round(0.2126 * r + 0.7152 * g + 0.0722 * b, 3) for r, g, b, _ in pixels]
    if pattern == "2x2_cr_overlay_lut_gradient":
        # The fixture's modality LUT maps stored values to 0, 1024, 2048,
        # and 4095, while its four-entry VOI LUT starts at zero. The latter
        # therefore clamps the final three inputs to white, and the diagonal
        # overlay raises the first input to white as well.
        passed = len(pixels) == 4 and [pixel[0] for pixel in pixels] == [255] * 4 and all(r == g == b for r, g, b, _ in pixels)
    elif pattern == "four_frame_moving_ultrasound_echo":
        passed = [pixel[0] for pixel in pixels] == [
            0, 16, 32, 48,
            16, 64, 80, 64,
            32, 80, 255, 80,
            48, 64, 80, 64,
        ] and all(r == g == b for r, g, b, _ in pixels)
    elif pattern in {
        "single_plane_synthetic_angiographic_projection",
        "single_plane_synthetic_radiofluoroscopic_projection",
    }:
        passed = [pixel[0] for pixel in pixels] == [
            0, 16, 32, 48,
            16, 64, 96, 64,
            32, 96, 255, 96,
            48, 64, 96, 64,
        ] and all(r == g == b for r, g, b, _ in pixels)
    elif pattern == "2x2_monochrome_gradient":
        passed = len(luminance) == 4 and luminance == sorted(luminance)
    elif pattern in ASCENDING_GRAYSCALE_PATTERNS:
        passed = bool(luminance) and luminance == sorted(luminance)
    elif pattern == "2x2_inverse_monochrome_gradient":
        passed = len(luminance) == 4 and luminance == sorted(luminance, reverse=True)
    elif pattern in DESCENDING_GRAYSCALE_PATTERNS:
        passed = bool(luminance) and luminance == sorted(luminance, reverse=True)
    elif pattern in {
        "2x2_rgb_red_green_blue_white",
        "2x2_rgb_planar1_red_green_blue_white",
        "2x2_ybr_full_red_green_blue_white",
        "2x2_palette_red_green_blue_white",
        "2x2_vl_endoscopic_rgb_red_green_blue_white",
        "2x2_vl_microscopic_rgb_red_green_blue_white",
        "2x2_vl_photo_palette_red_green_blue_white",
        "2x2_vl_photo_rgb_red_green_blue_white",
        "2x2_vl_photo_rgb_red_green_blue_white_with_srgb_icc",
        "2x2_palette_rle_lossless_red_green_blue_white",
        "2x2x2_palette_rle_lossless_palette_order_reversed",
        "2x2_rgb_rle_lossless_red_green_blue_white",
        "2x2_rgb_planar1_rle_lossless_red_green_blue_white",
        "2x2x2_rgb_planar0_rle_lossless_primary_secondary",
        "2x2x2_rgb_planar1_rle_lossless_primary_secondary",
        "2x2_ybr_full_rle_lossless_red_green_blue_white",
        "2x2_ybr_full_planar1_rle_lossless_red_green_blue_white",
        "2x2x2_ybr_full_planar0_rle_lossless_primary_secondary",
        "2x2x2_ybr_full_planar1_rle_lossless_primary_secondary",
        "2x2_vl_photo_palette_rle_lossless_red_green_blue_white",
        "2x2_vl_photo_rgb_rle_lossless_red_green_blue_white",
        "2x2_vl_photo_rgb_planar1_rle_lossless_red_green_blue_white",
    }:
        passed = len(pixels) == 4 and (
            pixels[0][0] > pixels[0][1] and pixels[0][0] > pixels[0][2]
            and pixels[1][1] > pixels[1][0] and pixels[1][1] > pixels[1][2]
            and pixels[2][2] > pixels[2][0] and pixels[2][2] > pixels[2][1]
            and min(pixels[3][:3]) > 200
        )
    elif pattern == "2x2_ybr_full_422_red_green_blue_white":
        # The red/green and blue/white neighbors share chroma by definition.
        # Lock the two shared-chroma pairs rather than pretending 4:2:2 can
        # retain four independent RGB primaries.
        passed = len(pixels) == 4 and (
            abs(pixels[0][0] - pixels[0][1]) <= 2 and pixels[0][2] < 10
            and abs(pixels[1][0] - pixels[1][1]) <= 2 and pixels[1][2] < 50
            and pixels[1][0] > pixels[0][0]
            and pixels[2][2] > pixels[2][0] * 5 and pixels[2][2] > pixels[2][1] * 5
            and min(pixels[3][:3]) > 200
        )
    elif pattern == "3x3x2_continuous_lsb_first_checkerboards":
        passed = len(luminance) == 9 and luminance == [255, 0, 255, 0, 255, 0, 255, 0, 255]
    elif pattern in {
        "2x2_monochrome_u32_unsigned_boundaries",
        "three_frame_ct_derived_float32_parametric_map",
        "three_frame_ct_derived_float64_parametric_map",
    }:
        passed = len(luminance) == 4 and luminance == sorted(luminance)
    elif pattern == "4x6_monochrome_checkerboard_with_nonsquare_pixels":
        passed = len(luminance) == 24 and set(luminance) == {0.0, 255.0} and all(
            luminance[index] != luminance[index + 1]
            for index in range(len(luminance) - 1)
            if (index + 1) % 6 != 0
        )
    elif pattern in {
        "two_frame_binary_segmentation_mask",
        "two_frame_labelmap_segmentation",
        "two_diagonal_wsi_tile_occupancy_masks",
    }:
        passed = len(luminance) == 4 and set(luminance) == {0.0, 255.0}
    elif pattern in {
        "two_distinct_4x4_rgb_optical_path_matrices",
        "4x4_tiled_full_red_green_blue_white_quadrants",
        "4x4_tiled_sparse_red_and_white_diagonal_with_two_absent_tiles",
    }:
        passed = len(pixels) == 4 and all(r > 250 and g < 5 and b < 5 for r, g, b, _ in pixels)
    else:
        return {"status": "unautomated", "pattern": pattern}
    return {"status": "passed" if passed else "failed", "pattern": pattern}


def _normalized_path(value: str) -> str:
    return os.path.normcase(str(Path(value).resolve()))


def select_entries(worklist: dict[str, Any], root: Optional[str]) -> list[dict[str, Any]]:
    selected = []
    for entry in worklist["files"]:
        profile = entry["manifest_identity"]["profile"]
        if root is not None and profile != root:
            continue
        occurrence = {
            "root": profile,
            "case_id": entry["case_id"],
            "path": entry["path"],
            "normalized_path": entry["normalized_path"],
            "sop_instance_uid": entry.get("sop_instance_uid"),
        }
        selected.append({**entry, "campaign_occurrence": occurrence})
    return sorted(
        selected,
        key=lambda row: (
            row["campaign_occurrence"]["root"],
            row["case_id"],
            row["campaign_occurrence"]["path"],
        ),
    )


def poll_files(base_url: str, request_timeout: float, startup_deadline: float) -> dict[str, Any]:
    while time.monotonic() < startup_deadline:
        response = http_request(base_url, "/api/files", request_timeout)
        if response["status"] == 200 and isinstance(response["json"], dict):
            if response["json"].get("scan_complete") is True:
                return response["json"]
        time.sleep(0.05)
    raise TimeoutError("discovery did not report scan_complete before startup deadline")


def pixel_geometry_observation(
    file_summary: dict[str, Any], expected: dict[str, Any]
) -> dict[str, Any]:
    geometry = expected.get("expected_nonsquare_spacing") or {}
    pixel_spacing = geometry.get("pixel_spacing") or {}
    pixel_aspect_ratio = geometry.get("pixel_aspect_ratio") or {}
    source = None
    expected_ratio = None
    if isinstance(pixel_spacing, dict):
        row = pixel_spacing.get("row_spacing_mm")
        column = pixel_spacing.get("column_spacing_mm")
        if all(_positive_finite_number(value) for value in (row, column)):
            source = "pixel_spacing"
            expected_ratio = row / column
    if expected_ratio is None and isinstance(pixel_aspect_ratio, dict):
        vertical = pixel_aspect_ratio.get("vertical_extent")
        horizontal = pixel_aspect_ratio.get("horizontal_extent")
        if all(_positive_finite_number(value) for value in (vertical, horizontal)):
            source = "pixel_aspect_ratio"
            expected_ratio = vertical / horizontal

    observed_ratio = file_summary.get("pixel_aspect_ratio")
    exact_evidence = (
        _positive_finite_number(expected_ratio)
        and _positive_finite_number(observed_ratio)
        and expected_ratio == observed_ratio
    )
    return {
        "evidence_scope": "api_files_metadata",
        "ui_rendering_verified": False,
        "source": source,
        "expected_row_to_column_ratio": expected_ratio,
        "observed_row_to_column_ratio": observed_ratio,
        "exact_evidence": exact_evidence,
        "passed": exact_evidence,
    }


def _expected_tag_value(value: Any) -> Any:
    if isinstance(value, list) and all(isinstance(item, str) for item in value):
        return "; ".join(value)
    return value


def _exact_tag_observation(
    tags: Any, fields: list[tuple[str, str, Any]]
) -> dict[str, Any]:
    tag_index = {
        row.get("tag"): row for row in tags or [] if isinstance(row, dict)
    }
    results = []
    for name, tag, expected in fields:
        node = tag_index.get(_tag_key(tag))
        observed = _tag_scalar(node) if isinstance(node, dict) else None
        expected = _expected_tag_value(expected)
        results.append(
            {
                "field": name,
                "tag": _tag_key(tag),
                "expected": expected,
                "observed": observed,
                "passed": observed == expected,
            }
        )
    exact = bool(results) and all(row["passed"] for row in results)
    return {
        "evidence_scope": "declared_manifest_values_against_tag_api",
        "results": results,
        "exact_evidence": exact,
        "passed": exact,
    }


def nm_dimensions_observation(tags: Any, expected: dict[str, Any]) -> dict[str, Any]:
    nm = expected.get("expected_nm_multiframe") or {}
    return _exact_tag_observation(
        tags,
        [
            ("image_type", "0008,0008", nm.get("image_type")),
            ("counts_accumulated", "0018,0070", nm.get("counts_accumulated")),
            ("actual_frame_duration_ms", "0018,1242", nm.get("actual_frame_duration_ms")),
            ("energy_window_vector", "0054,0010", nm.get("energy_window_vector")),
            ("number_of_energy_windows", "0054,0011", nm.get("number_of_energy_windows")),
            ("detector_vector", "0054,0020", nm.get("detector_vector")),
            ("number_of_detectors", "0054,0021", nm.get("number_of_detectors")),
        ],
    )


def frame_time_observation(
    file_summary: dict[str, Any], tags: Any, expected: dict[str, Any]
) -> dict[str, Any]:
    ultrasound = expected.get("expected_us_multiframe") or {}
    observation = _exact_tag_observation(
        tags,
        [
            ("image_type", "0008,0008", ultrasound.get("image_type")),
            ("frame_time_ms", "0018,1063", ultrasound.get("frame_time_ms")),
            ("color_data_present", "0028,0014", int(bool(ultrasound.get("color_data_present")))),
            ("lossy_image_compression", "0028,2110", ultrasound.get("lossy_image_compression")),
        ],
    )
    frame_count = file_summary.get("frame_count")
    frame_time = ultrasound.get("frame_time_ms")
    observed_relative_times = (
        [index * frame_time for index in range(frame_count)]
        if isinstance(frame_count, int) and isinstance(frame_time, (int, float))
        else None
    )
    derived = {
        "field": "frame_relative_times_ms",
        "expected": ultrasound.get("frame_relative_times_ms"),
        "observed": observed_relative_times,
        "passed": observed_relative_times == ultrasound.get("frame_relative_times_ms"),
    }
    observation["results"].append(derived)
    observation["exact_evidence"] = observation["passed"] = all(
        row["passed"] for row in observation["results"]
    )
    return observation


def projection_geometry_observation(tags: Any, expected: dict[str, Any]) -> dict[str, Any]:
    projection = expected.get("expected_xa_projection") or expected.get("expected_xrf_projection") or {}
    fields = [
        ("image_type", "0008,0008", projection.get("image_type")),
        ("body_part_examined", "0018,0015", projection.get("body_part_examined")),
        ("kvp", "0018,0060", projection.get("kvp")),
        ("distance_source_to_detector_mm", "0018,1110", projection.get("distance_source_to_detector_mm")),
        ("distance_source_to_patient_mm", "0018,1111", projection.get("distance_source_to_patient_mm")),
        ("estimated_radiographic_magnification_factor", "0018,1114", projection.get("estimated_radiographic_magnification_factor")),
        ("exposure_mas", "0018,1152", projection.get("exposure_mas")),
        ("radiation_setting", "0018,1155", projection.get("radiation_setting")),
        ("imager_pixel_spacing_mm", "0018,1164", projection.get("imager_pixel_spacing_mm")),
        ("pixel_intensity_relationship", "0028,1040", projection.get("pixel_intensity_relationship")),
        ("lossy_image_compression", "0028,2110", projection.get("lossy_image_compression")),
    ]
    if "expected_xa_projection" in expected:
        fields.extend(
            [
                ("positioner_primary_angle_degrees", "0018,1510", projection.get("positioner_primary_angle_degrees")),
                ("positioner_secondary_angle_degrees", "0018,1511", projection.get("positioner_secondary_angle_degrees")),
            ]
        )
    else:
        fields.append(("column_angulation_degrees", "0018,1450", projection.get("column_angulation_degrees")))
    return _exact_tag_observation(tags, fields)


def _decoded_integer_samples(payload: bytes, image: dict[str, Any]) -> list[int] | None:
    bits_allocated = image.get("bits_allocated")
    signed = image.get("pixel_representation") == 1
    if bits_allocated not in {8, 16, 32} or len(payload) % (bits_allocated // 8):
        return None
    width = bits_allocated // 8
    return [
        int.from_bytes(payload[offset : offset + width], "little", signed=signed)
        for offset in range(0, len(payload), width)
    ]


def pet_activity_observation(
    tags: Any, raw_payload: bytes | None, expected: dict[str, Any]
) -> dict[str, Any]:
    pet = expected.get("expected_pet_activity") or {}
    observation = _exact_tag_observation(
        tags,
        [
            ("image_type", "0008,0008", pet.get("image_type")),
            ("actual_frame_duration_ms", "0018,1242", pet.get("actual_frame_duration_ms")),
            ("corrected_image", "0028,0051", pet.get("corrected_image")),
            ("rescale_intercept", "0028,1052", pet.get("rescale_intercept")),
            ("rescale_slope", "0028,1053", pet.get("rescale_slope")),
            ("number_of_slices", "0054,0081", pet.get("number_of_slices")),
            ("series_type", "0054,1000", pet.get("series_type")),
            ("units", "0054,1001", pet.get("units")),
            ("counts_source", "0054,1002", pet.get("counts_source")),
            ("decay_correction", "0054,1102", pet.get("decay_correction")),
            ("frame_reference_time_ms", "0054,1300", pet.get("frame_reference_time_ms")),
            ("dose_calibration_factor", "0054,1322", pet.get("dose_calibration_factor")),
            ("image_index", "0054,1330", pet.get("image_index")),
        ],
    )
    stored = _decoded_integer_samples(raw_payload or b"", expected.get("image") or {})
    slope = pet.get("rescale_slope")
    intercept = pet.get("rescale_intercept")
    activity = (
        [sample * slope + intercept for sample in stored]
        if stored is not None and isinstance(slope, (int, float)) and isinstance(intercept, (int, float))
        else None
    )
    observation["results"].extend(
        [
            {"field": "stored_values", "expected": pet.get("stored_values"), "observed": stored, "passed": stored == pet.get("stored_values")},
            {"field": "activity_values_bqml", "expected": pet.get("activity_values_bqml"), "observed": activity, "passed": activity == pet.get("activity_values_bqml")},
        ]
    )
    observation["exact_evidence"] = observation["passed"] = all(
        row["passed"] for row in observation["results"]
    )
    return observation


def unprobed_capabilities(
    expected_capabilities: list[str], checks: dict[str, Any]
) -> list[str]:
    unprobed = set(expected_capabilities) - PROBED_CAPABILITIES
    for capability, check_name in CONDITIONAL_CAPABILITY_CHECKS.items():
        check = checks.get(check_name) or {}
        if capability in unprobed and check.get("exact_evidence") is True:
            unprobed.remove(capability)
    return sorted(unprobed)


def reference_observation(
    payload: Any, expected_references: list[dict[str, Any]]
) -> dict[str, Any]:
    identity_fields = (
        "relationship",
        "sop_class_uid",
        "sop_instance_uid",
        "series_instance_uid",
        "frame_numbers",
    )

    def expected_identity(reference: dict[str, Any]) -> dict[str, Any]:
        return {
            "relationship": reference.get("relationship"),
            "sop_class_uid": reference.get("sop_class_uid"),
            "sop_instance_uid": reference.get("sop_instance_uid"),
            "series_instance_uid": reference.get("series_instance_uid"),
            "frame_numbers": reference.get("frame_numbers") or [],
        }

    def observed_identity(reference: dict[str, Any]) -> dict[str, Any]:
        target = reference.get("target") or {}
        return {
            "relationship": reference.get("relationship"),
            "sop_class_uid": target.get("sop_class_uid"),
            "sop_instance_uid": target.get("sop_instance_uid"),
            "series_instance_uid": target.get("series_instance_uid"),
            "frame_numbers": target.get("frame_numbers") or [],
        }

    def sorted_identities(values: list[dict[str, Any]]) -> list[dict[str, Any]]:
        return sorted(
            values,
            key=lambda value: json.dumps(value, sort_keys=True, separators=(",", ":")),
        )

    expected = sorted_identities([expected_identity(row) for row in expected_references])
    observed_rows = (
        payload.get("references")
        if isinstance(payload, dict) and isinstance(payload.get("references"), list)
        else []
    )
    observed = sorted_identities(
        [observed_identity(row) for row in observed_rows if isinstance(row, dict)]
    )
    identities_match = observed == expected

    resolution_checks = []
    for expected_row in expected_references:
        identity = expected_identity(expected_row)
        candidate = next(
            (
                row
                for row in observed_rows
                if isinstance(row, dict) and observed_identity(row) == identity
            ),
            None,
        )
        expected_frames = [number - 1 for number in identity["frame_numbers"] if number > 0]
        expected_path = expected_row.get("source_path")
        matching_targets = []
        if candidate is not None:
            for match in candidate.get("matches") or []:
                if not isinstance(match, dict):
                    continue
                normalized_match_path = str(match.get("path") or "").replace("\\", "/")
                path_matches = not expected_path or normalized_match_path.endswith(
                    "/" + expected_path
                )
                frames = match.get("frame_indices") or []
                frames_match = not expected_frames or frames == expected_frames
                uid_matches = match.get("sop_instance_uid") == identity["sop_instance_uid"]
                if path_matches and frames_match and uid_matches:
                    matching_targets.append(
                        {
                            "file_index": match.get("file_index"),
                            "path": match.get("path"),
                            "frame_indices": frames,
                        }
                    )
        resolution_checks.append(
            {
                "identity": identity,
                "expected_source_path": expected_path,
                "matches": matching_targets,
                "passed": bool(matching_targets),
            }
        )

    all_resolved = all(check["passed"] for check in resolution_checks)
    exact_evidence = identities_match and all_resolved
    return {
        "evidence_scope": "typed_api_identity_and_local_resolution",
        "expected": expected,
        "observed": observed,
        "identity_fields": list(identity_fields),
        "identities_match": identities_match,
        "resolution_checks": resolution_checks,
        "all_resolved": all_resolved,
        "exact_evidence": exact_evidence,
        "passed": exact_evidence,
    }


def series_observation(
    catalog: dict[str, Any], file_summary: dict[str, Any], expected: dict[str, Any]
) -> dict[str, Any]:
    index = file_summary["index"]
    located_series = None
    located_stack = None
    located_frame = None
    for series in catalog.get("series") or []:
        for stack in series.get("stacks") or []:
            for frame in stack.get("frames") or []:
                if frame.get("file_index") == index:
                    located_series = series
                    located_stack = stack
                    located_frame = frame
                    break
            if located_frame is not None:
                break
        if located_frame is not None:
            break
    if located_series is None or located_stack is None or located_frame is None:
        return {"mapped": False, "capabilities": {}}

    warnings = [row.get("code") for row in located_stack.get("warnings") or []]
    observation: dict[str, Any] = {
        "mapped": True,
        "series_id": located_series.get("id"),
        "series_instance_uid": located_series.get("series_instance_uid"),
        "frame_of_reference_uids": located_series.get("frame_of_reference_uids") or [],
        "stack_id": located_stack.get("id"),
        "stack_kind": located_stack.get("kind"),
        "virtual_index": located_frame.get("virtual_index"),
        "source_frame_index": located_frame.get("frame_index"),
        "position_along_normal_mm": located_frame.get("position_along_normal_mm"),
        "ordered_sources": [
            {"path": frame.get("source_path"), "frame_index": frame.get("frame_index")}
            for frame in located_stack.get("frames") or []
        ],
        "warning_codes": warnings,
        "capabilities": {},
    }
    capabilities = set(expected.get("expected_capabilities") or [])
    geometry = expected.get("expected_geometry") or {}
    semantics = expected.get("expected_semantics") or {}
    checks = observation["capabilities"]
    if "sort_series_by_geometry" in capabilities:
        expected_index = geometry.get("geometric_order_index")
        if not isinstance(expected_index, int):
            expected_index = (semantics.get("geometry_sort_key") or {}).get(
                "slice_order_index"
            )
        checks["sort_series_by_geometry"] = {
            "expected_virtual_index": expected_index - 1 if isinstance(expected_index, int) else None,
            "observed_virtual_index": located_frame.get("virtual_index"),
            "passed": isinstance(expected_index, int)
            and located_frame.get("virtual_index") == expected_index - 1,
        }
    if "interpret_gantry_tilt" in capabilities:
        checks["interpret_gantry_tilt"] = {
            "expected_warning": "gantry_tilt",
            "observed_warnings": warnings,
            "passed": "gantry_tilt" in warnings,
        }
    if "organize_series_by_study_and_frame_of_reference" in capabilities:
        frame_uids = set(located_series.get("frame_of_reference_uids") or [])
        peers = [
            series
            for series in catalog.get("series") or []
            if series.get("study_instance_uid") == located_series.get("study_instance_uid")
            and frame_uids.intersection(series.get("frame_of_reference_uids") or [])
        ]
        checks["organize_series_by_study_and_frame_of_reference"] = {
            "peer_series_ids": sorted(series.get("id") for series in peers),
            "passed": len(peers) >= 2
            and len({series.get("series_instance_uid") for series in peers}) == len(peers),
        }
    if "parse_multiframe_functional_groups" in capabilities:
        concatenation = semantics.get("concatenation") or {}
        expected_kind = "concatenation" if concatenation else "ordinary"
        checks["parse_multiframe_functional_groups"] = {
            "expected_stack_kind": expected_kind,
            "observed_stack_kind": located_stack.get("kind"),
            "passed": located_stack.get("kind") == expected_kind,
        }
    return observation


def probe_case(
    base_url: str,
    entry: dict[str, Any],
    file_summary: dict[str, Any],
    request_timeout: float,
    case_timeout: float,
    series_catalog: dict[str, Any],
    series_http: dict[str, Any],
) -> dict[str, Any]:
    started = time.monotonic()
    index = file_summary["index"]
    frame_count = int(file_summary["frame_count"])
    has_pixels = bool(file_summary["has_pixels"])
    expected = entry["expected_contract"]
    expected_capabilities = sorted(expected.get("expected_capabilities") or [])
    image = expected.get("image") or {}
    pixel_data = expected.get("pixel_data") or {}
    checks: dict[str, Any] = {"mapped_after_scan": True}
    checks["series_navigation"] = series_observation(series_catalog, file_summary, expected)
    if "interpret_pixel_geometry" in expected_capabilities:
        checks["pixel_geometry"] = pixel_geometry_observation(file_summary, expected)
    http: dict[str, Any] = {"series_catalog": series_http}
    errors: list[dict[str, Any]] = []
    raw_first: dict[str, Any] | None = None

    def request(name: str, path: str, headers: tuple[str, ...] = ()) -> dict[str, Any]:
        if time.monotonic() - started >= case_timeout:
            raise TimeoutError(f"case exceeded {case_timeout:.1f}s")
        response = http_request(base_url, path, min(request_timeout, case_timeout))
        http[name] = evidence(response, headers)
        return response

    try:
        info = request("info", f"/api/file/{index}/info")
        tags = request("tags", f"/api/file/{index}/tags")
        checks["file_info"] = info["status"] == 200 and isinstance(info["json"], dict)
        checks["tags"] = metadata_observation(
            file_summary,
            info["json"] if info["status"] == 200 else None,
            tags["json"] if tags["status"] == 200 else None,
            expected,
        )
        tag_payload = tags["json"] if tags["status"] == 200 else None
        if "interpret_nm_dimensions" in expected_capabilities:
            checks["nm_dimensions"] = nm_dimensions_observation(tag_payload, expected)
        if "interpret_frame_time" in expected_capabilities:
            checks["frame_time"] = frame_time_observation(
                file_summary, tag_payload, expected
            )
        if "interpret_projection_geometry" in expected_capabilities:
            checks["projection_geometry"] = projection_geometry_observation(
                tag_payload, expected
            )
        expected_references = expected.get("references") or []
        if expected_references:
            reference_response = request("references", f"/api/file/{index}/references")
            checks["references"] = reference_observation(
                reference_response["json"] if reference_response["status"] == 200 else None,
                expected_references,
            )
        invalid = request("invalid_frame", f"/api/file/{index}/frame/{frame_count}")
        checks["error_envelope"] = (
            invalid["status"] in {400, 404, 422}
            and isinstance(invalid["json"], dict)
            and isinstance(invalid["json"].get("error"), str)
        )
        recovery = request("recovery_after_error", f"/api/file/{index}/info")
        checks["recovery_after_error"] = {
            "passed": recovery["status"] == 200 and isinstance(recovery["json"], dict),
            "error_status": invalid["status"],
            "recovery_status": recovery["status"],
        }
        if has_pixels and frame_count > 0:
            display_first = request(
                "display_first", f"/api/file/{index}/frame/0", ("x-cache",)
            )
            display_second = request(
                "display_second", f"/api/file/{index}/frame/0", ("x-cache",)
            )
            checks["display_cache"] = (
                display_first["headers"].get("x-cache") == "MISS"
                and display_second["headers"].get("x-cache") == "HIT"
            )
            checks["display_body_stable"] = (
                display_first["body_sha256"] == display_second["body_sha256"]
            )
            if display_first["status"] == 200:
                try:
                    width, height, pixels = png_pixels(display_first["body"])
                    checks["png_dimensions"] = {
                        "expected": [image.get("columns"), image.get("rows")],
                        "observed": [width, height],
                        "passed": width == image.get("columns") and height == image.get("rows"),
                    }
                    checks["visual"] = validate_visual(
                        (expected.get("expected_visual_checks") or {}).get("pattern"), pixels
                    )
                    if "read_overlay_plane" in expected_capabilities:
                        overlay_response = request(
                            "overlay_full_dynamic",
                            f"/api/file/{index}/frame/0?mode=full_dynamic",
                        )
                        overlay_pixels: list[tuple[int, int, int, int]] = []
                        if overlay_response["status"] == 200:
                            try:
                                _, _, overlay_pixels = png_pixels(overlay_response["body"])
                            except (ValueError, zlib.error):
                                pass
                        checks["overlay_display"] = overlay_display_observation(
                            expected, overlay_pixels
                        )
                    if "apply_display_shutter" in expected_capabilities:
                        checks["display_shutter"] = shutter_display_observation(expected, pixels)
                    if expected.get("expected_icc_profile"):
                        checks["icc_profile"] = icc_profile_observation(
                            expected, display_first["body"]
                        )
                except (ValueError, zlib.error) as error:
                    checks["png_dimensions"] = {"passed": False, "error": str(error)}
            if frame_count > 1:
                navigation_results = []
                for navigation_index in deterministic_navigation_frames(entry["case_id"], frame_count):
                    display_frame = display_first if navigation_index == 0 else request(
                        f"display_navigation_{navigation_index}",
                        f"/api/file/{index}/frame/{navigation_index}",
                    )
                    observed_dimensions = None
                    if display_frame["status"] == 200:
                        try:
                            observed_width, observed_height, _ = png_pixels(display_frame["body"])
                            observed_dimensions = [observed_width, observed_height]
                        except (ValueError, zlib.error):
                            pass
                    navigation_results.append({
                        "frame": navigation_index,
                        "status": display_frame["status"],
                        "observed_dimensions": observed_dimensions,
                        "passed": display_frame["status"] == 200
                        and observed_dimensions == [image.get("columns"), image.get("rows")],
                    })
                checks["frame_navigation"] = {
                    "requested_frames": [row["frame"] for row in navigation_results],
                    "results": navigation_results,
                    "passed": all(row["passed"] for row in navigation_results),
                }
            raw_first = request(
                "raw_first", f"/api/file/{index}/frame/0/raw", ("x-cache",) + RAW_HEADERS
            )
            raw_second = request(
                "raw_second", f"/api/file/{index}/frame/0/raw", ("x-cache",) + RAW_HEADERS
            )
            checks["raw_cache"] = (
                raw_first["headers"].get("x-cache") == "MISS"
                and raw_second["headers"].get("x-cache") == "HIT"
            )
            checks["raw_headers"] = raw_header_observation(raw_first["headers"], image)
            if transfer_syntax := (expected.get("dicom") or {}).get("transfer_syntax_uid"):
                if transfer_syntax in LOSSY_TRANSFER_SYNTAXES and raw_first["status"] == 200:
                    checks["lossy_metrics"] = lossy_metrics_observation(
                        raw_first["body"], expected
                    )
            if "interpret_pet_activity" in expected_capabilities:
                checks["pet_activity"] = pet_activity_observation(
                    tag_payload,
                    raw_first["body"] if raw_first["status"] == 200 else None,
                    expected,
                )
            if entry["policy"]["classification"] == "controlled_unsupported":
                expected_statuses = set(
                    (entry["policy"].get("expected_unsupported") or {}).get("statuses", [])
                )
                unsupported_responses = [display_first, display_second, raw_first, raw_second]
                checks["controlled_unsupported"] = {
                    "passed": bool(expected_statuses)
                    and all(response["status"] in expected_statuses for response in unsupported_responses)
                    and all(
                        isinstance(response.get("json"), dict)
                        and response["json"].get("code") == "unsupported_transfer_syntax"
                        and isinstance(response["json"].get("error"), str)
                        for response in unsupported_responses
                    ),
                    "statuses": [response["status"] for response in unsupported_responses],
                    "codes": [(response.get("json") or {}).get("code") for response in unsupported_responses],
                }
            expected_hashes = pixel_data.get("frame_hashes") or []
            transfer_syntax = (expected.get("dicom") or {}).get("transfer_syntax_uid")
            if expected_hashes and transfer_syntax not in LOSSY_TRANSFER_SYNTAXES:
                first_canonical_hash = (
                    hashlib.sha256(
                        canonical_raw_bytes(raw_first["body"], image, transfer_syntax)
                    ).hexdigest()
                    if raw_first["status"] == 200 else None
                )
                checks["lossless_frame_hash"] = {
                    "expected": expected_hashes[0],
                    "observed": first_canonical_hash,
                    "response_body_sha256": raw_first["body_sha256"] if raw_first["status"] == 200 else None,
                    "passed": raw_first["status"] == 200
                    and first_canonical_hash == expected_hashes[0],
                }
                observed_hashes = []
                for frame_index, expected_hash in enumerate(expected_hashes):
                    raw_frame = raw_first if frame_index == 0 else request(
                        f"raw_frame_{frame_index}",
                        f"/api/file/{index}/frame/{frame_index}/raw",
                        RAW_HEADERS,
                    )
                    observed_hashes.append(hashlib.sha256(
                        canonical_raw_bytes(raw_frame["body"], image, transfer_syntax)
                    ).hexdigest() if raw_frame["status"] == 200 else None)
                checks["lossless_frame_hashes"] = {
                    "expected": expected_hashes,
                    "observed": observed_hashes,
                    "passed": observed_hashes == expected_hashes,
                }
        else:
            checks["metadata_only_response"] = invalid["status"] == 404
        if entry["policy"].get("semantic_context_assertions"):
            semantic = request("semantic_context", f"/api/file/{index}/semantic-context")
            semantic_checks = semantic_context_observations(
                semantic["json"] if semantic["status"] == 200 else None, expected
            )
            if raw_first is not None and raw_first["status"] == 200:
                semantic_raw = request("semantic_raw_identity", f"/api/file/{index}/frame/0/raw")
                semantic_checks["semantic_raw_identity"] = {
                    "before": hashlib.sha256(canonical_raw_bytes(raw_first["body"], image)).hexdigest(),
                    "after": hashlib.sha256(canonical_raw_bytes(semantic_raw["body"], image)).hexdigest() if semantic_raw["status"] == 200 else None,
                    "context_contract_passed": semantic_checks["semantic_raw_identity"]["context_contract_passed"],
                    "passed": semantic_checks["semantic_raw_identity"]["context_contract_passed"]
                    and semantic_raw["status"] == 200
                    and canonical_raw_bytes(raw_first["body"], image) == canonical_raw_bytes(semantic_raw["body"], image),
                }
            checks.update(semantic_checks)
        if file_summary.get("object_kind") == "whole_slide_microscopy":
            wsi_payloads = []
            for wsi_frame in deterministic_navigation_frames(entry["case_id"], frame_count):
                wsi_response = request(
                    f"wsi_context_{wsi_frame}",
                    f"/api/file/{index}/frame/{wsi_frame}/wsi-context",
                )
                if wsi_response["status"] == 200 and isinstance(wsi_response["json"], dict):
                    wsi_payloads.append(wsi_response["json"])
            checks.update(wsi_context_observations(wsi_payloads, expected))
    except TimeoutError as error:
        errors.append({"code": "case_timeout", "message": str(error)})
        return _finish_result(entry, expected_capabilities, checks, http, errors, started, "timeout")
    except (OSError, urllib.error.URLError) as error:
        errors.append({"code": "request_failure", "message": str(error)})
        return _finish_result(entry, expected_capabilities, checks, http, errors, started, "safe")
    return _finish_result(entry, expected_capabilities, checks, http, errors, started, "safe")


def _finish_result(
    entry: dict[str, Any],
    expected_capabilities: list[str],
    checks: dict[str, Any],
    http: dict[str, Any],
    errors: list[dict[str, Any]],
    started: float,
    safety: str,
) -> dict[str, Any]:
    occurrence = entry["campaign_occurrence"]
    unprobed = unprobed_capabilities(expected_capabilities, checks)
    statuses = [row["status"] for row in http.values()]
    controlled_gap = any(status == 422 for status in statuses)
    server_failure = any(status >= 500 for status in statuses)
    tags = checks.get("tags")
    tags_passed = tags is True or isinstance(tags, dict) and tags.get("passed") is True
    required_checks = [checks.get("mapped_after_scan"), checks.get("file_info"), tags_passed]
    if safety != "safe" or server_failure or not all(required_checks):
        compatibility = "failure"
    elif unprobed or controlled_gap:
        compatibility = "unverified"
    elif checks.get("metadata_only_response"):
        compatibility = "metadata_only"
    else:
        validation_failures = []
        for name in ("display_cache", "display_body_stable", "raw_cache"):
            if name in checks and checks[name] is not True:
                validation_failures.append(name)
        for name in (
            "png_dimensions",
            "frame_navigation",
            "lossless_frame_hash",
            "lossless_frame_hashes",
            "raw_headers",
            "lossy_metrics",
            "nm_dimensions",
            "pet_activity",
            "frame_time",
            "projection_geometry",
            "pixel_geometry",
            "references",
            "overlay_display",
            "display_shutter",
            "icc_profile",
        ):
            if name in checks and checks[name].get("passed") is not True:
                validation_failures.append(name)
        series_checks = (checks.get("series_navigation") or {}).get("capabilities") or {}
        validation_failures.extend(
            f"series:{name}"
            for name, check in series_checks.items()
            if check.get("passed") is not True
        )
        visual = checks.get("visual")
        if visual and visual["status"] in {"failed", "unautomated"}:
            validation_failures.append("visual")
        compatibility = "unverified" if validation_failures else "verified"
    return {
        "root": occurrence["root"],
        "case_id": occurrence["case_id"],
        "path": occurrence["path"],
        "identity": {
            "normalized_path": occurrence["normalized_path"],
            "sop_instance_uid": entry["sop_instance_uid"],
            "file_sha256": entry["sha256"],
            "contract_sha256": entry["contract_sha256"],
        },
        "execution_safety": safety,
        "compatibility": compatibility,
        "expected_capabilities": expected_capabilities,
        "unprobed_capabilities": unprobed,
        "observations": checks,
        "http": http,
        "timings_ms": {"total": round((time.monotonic() - started) * 1000, 3)},
        "errors": errors,
    }


def validate_report(report: dict[str, Any]) -> None:
    required = {
        "detail_schema_version", "generated_at", "worklist", "viewer", "run",
        "results", "summary", "artifacts", "validation",
    }
    if set(report) != required:
        raise CampaignError(f"report fields do not match companion schema: {sorted(set(report) ^ required)}")
    if report["detail_schema_version"] != DETAIL_SCHEMA_VERSION:
        raise CampaignError("unexpected detail schema version")
    seen: set[tuple[str, str, str]] = set()
    for result in report["results"]:
        key = (result["root"], result["case_id"], result["path"])
        if key in seen:
            raise CampaignError(f"duplicate result identity: {key}")
        seen.add(key)
        if result["execution_safety"] not in EXECUTION_OUTCOMES:
            raise CampaignError(f"invalid execution outcome: {key}")
        if result["compatibility"] not in COMPATIBILITY_OUTCOMES:
            raise CampaignError(f"invalid compatibility outcome: {key}")
        if len(result["identity"]["file_sha256"]) != 64:
            raise CampaignError(f"invalid file identity: {key}")
    if report["summary"]["results"] != len(report["results"]):
        raise CampaignError("summary/result count mismatch")


def validate_json_schema(
    instance: Any,
    schema: dict[str, Any],
    root_schema: Optional[dict[str, Any]] = None,
    path: str = "$",
) -> None:
    """Validate the strict JSON Schema subset used by the detail contract."""
    root = schema if root_schema is None else root_schema
    reference = schema.get("$ref")
    if reference is not None:
        if not reference.startswith("#/"):
            raise CampaignError(f"unsupported external schema reference at {path}: {reference}")
        target: Any = root
        for component in reference[2:].split("/"):
            target = target[component.replace("~1", "/").replace("~0", "~")]
        validate_json_schema(instance, target, root, path)
        return
    if "const" in schema and instance != schema["const"]:
        raise CampaignError(f"schema const violation at {path}")
    if "enum" in schema and instance not in schema["enum"]:
        raise CampaignError(f"schema enum violation at {path}")
    if "anyOf" in schema:
        failures = []
        for option in schema["anyOf"]:
            try:
                validate_json_schema(instance, option, root, path)
                break
            except CampaignError as error:
                failures.append(str(error))
        else:
            raise CampaignError(f"schema anyOf violation at {path}: {failures}")
        return
    expected_type = schema.get("type")
    if expected_type is not None:
        accepted = expected_type if isinstance(expected_type, list) else [expected_type]
        predicates = {
            "object": lambda value: isinstance(value, dict),
            "array": lambda value: isinstance(value, list),
            "string": lambda value: isinstance(value, str),
            "integer": lambda value: isinstance(value, int) and not isinstance(value, bool),
            "number": lambda value: isinstance(value, (int, float)) and not isinstance(value, bool),
            "boolean": lambda value: isinstance(value, bool),
            "null": lambda value: value is None,
        }
        if not any(predicates[kind](instance) for kind in accepted):
            raise CampaignError(f"schema type violation at {path}: expected {accepted}")
    if isinstance(instance, dict):
        required = schema.get("required", [])
        missing = [key for key in required if key not in instance]
        if missing:
            raise CampaignError(f"schema required-field violation at {path}: {missing}")
        properties = schema.get("properties", {})
        if schema.get("additionalProperties") is False:
            extra = sorted(set(instance) - set(properties))
            if extra:
                raise CampaignError(f"schema additional-field violation at {path}: {extra}")
        for key, value in instance.items():
            if key in properties:
                validate_json_schema(value, properties[key], root, f"{path}.{key}")
    if isinstance(instance, list) and "items" in schema:
        for index, value in enumerate(instance):
            validate_json_schema(value, schema["items"], root, f"{path}[{index}]")
    if isinstance(instance, str) and "pattern" in schema:
        if re.search(schema["pattern"], instance) is None:
            raise CampaignError(f"schema pattern violation at {path}")
    if isinstance(instance, (int, float)) and not isinstance(instance, bool):
        if "minimum" in schema and instance < schema["minimum"]:
            raise CampaignError(f"schema minimum violation at {path}")


def normalized_report(report: dict[str, Any]) -> dict[str, Any]:
    registry_index_fields = {"file_index", "file_indices", "source_file_index"}
    index_bearing_http_evidence = {"references", "series_catalog"}

    def normalize_value(value: Any) -> Any:
        if isinstance(value, dict):
            return {
                key: normalize_value(item)
                for key, item in value.items()
                if key not in registry_index_fields
            }
        if isinstance(value, list):
            return [normalize_value(item) for item in value]
        return value

    results = []
    for result in report["results"]:
        normalized_result = normalize_value(result)
        normalized_result.pop("timings_ms", None)
        normalized_http = normalized_result["http"]
        for name, row in normalized_http.items():
            row.pop("elapsed_ms", None)
            if "path" in row:
                row["path"] = re.sub(r"^/api/file/\d+", "/api/file/{mapped}", row["path"])
            if name in index_bearing_http_evidence:
                row.pop("body_sha256", None)
                row.pop("size_bytes", None)
        results.append(normalized_result)
    return {
        "detail_schema_version": report["detail_schema_version"],
        "worklist_content_sha256": report["worklist"]["content_sha256"],
        "viewer_sha256": report["viewer"]["sha256"],
        "selection": report["run"]["selection"],
        "results": results,
        "summary": report["summary"],
    }


def _ensure_external_output(
    output: Path,
    source_roots: tuple[Path, ...],
    viewer_root: Path,
) -> None:
    resolved = output.resolve()
    for repository in source_roots + (viewer_root,):
        try:
            resolved.relative_to(repository)
        except ValueError:
            continue
        raise CampaignError(f"artifact output must be outside input roots: {resolved}")
    if output.exists() and any(output.iterdir()):
        raise CampaignError(f"artifact output is not empty: {output}")
    output.mkdir(parents=True, exist_ok=True)


def _require_external_pins(args: argparse.Namespace) -> None:
    required = (
        "expected_generator_revision",
        "expected_actions_zip_sha256",
        "expected_actions_zip_size_bytes",
        "expected_nested_archive_sha256",
        "expected_nested_archive_size_bytes",
        "expected_release_manifest_sha256",
        "expected_release_manifest_size_bytes",
        "expected_installed_binary_sha256",
        "expected_installed_binary_size_bytes",
        "expected_target",
        "expected_toolchain",
        "expected_generator_features",
        "expected_runtime_identities_sha256",
        "expected_definition_manifest_sha256",
        "expected_corpus_definition_sha256",
        "expected_manifest_sha256",
        "expected_manifest_size_bytes",
        "expected_profile",
        "expected_seed",
        "expected_binding_id",
        "expected_archive_sha256",
        "expected_archive_size_bytes",
        "expected_generator_version",
    )
    missing = [name for name in required if getattr(args, name, None) is None]
    if missing:
        raise CampaignError(
            "stored smoke artifact consumption requires complete immutable pins: "
            + ", ".join(missing)
        )


def run_campaign(args: argparse.Namespace) -> dict[str, Any]:
    viewer_root = Path(__file__).resolve().parents[2]
    # Keep the caller spelling intact until the artifact verifier has rejected
    # symlink components; resolving here would erase that security boundary.
    artifact_root = Path(os.path.abspath(args.corpus_root))
    output = args.output.resolve()
    temporary_directory = None
    _require_external_pins(args)
    try:
        worklist = build_external_worklist(
            artifact_root,
            expected_seed=args.expected_seed,
            expected_manifest_sha256=args.expected_manifest_sha256,
            expected_corpus_definition_sha256=args.expected_corpus_definition_sha256,
            expected_generator_version=args.expected_generator_version,
            expected_generator_features=args.expected_generator_features,
            expected_generator_revision=args.expected_generator_revision,
            expected_actions_zip_sha256=args.expected_actions_zip_sha256,
            expected_actions_zip_size_bytes=args.expected_actions_zip_size_bytes,
            expected_nested_archive_sha256=args.expected_nested_archive_sha256,
            expected_nested_archive_size_bytes=args.expected_nested_archive_size_bytes,
            expected_release_manifest_sha256=args.expected_release_manifest_sha256,
            expected_release_manifest_size_bytes=args.expected_release_manifest_size_bytes,
            expected_installed_binary_sha256=args.expected_installed_binary_sha256,
            expected_installed_binary_size_bytes=args.expected_installed_binary_size_bytes,
            expected_target=args.expected_target,
            expected_toolchain=args.expected_toolchain,
            expected_runtime_identities_sha256=args.expected_runtime_identities_sha256,
            expected_definition_manifest_sha256=args.expected_definition_manifest_sha256,
            expected_manifest_size_bytes=args.expected_manifest_size_bytes,
            expected_profile=args.expected_profile,
            expected_binding_id=args.expected_binding_id,
            expected_archive_sha256=args.expected_archive_sha256,
            expected_archive_size_bytes=args.expected_archive_size_bytes,
            required_pins=True,
        )
    except ArtifactError as error:
        raise CampaignError(str(error)) from error
    temporary_directory = worklist.pop("_temporary_directory", None)
    worklist_path = Path(worklist["inputs"]["manifests"][0]["manifest"])
    extracted_root = Path(worklist["inputs"]["manifests"][0]["root"]).parent
    source_roots = (artifact_root, extracted_root)
    try:
        return _run_campaign_verified(
            args,
            viewer_root=viewer_root,
            artifact_root=artifact_root,
            output=output,
            worklist=worklist,
            worklist_path=worklist_path,
            source_roots=source_roots,
            selection="smoke",
        )
    finally:
        if temporary_directory is not None:
            temporary_directory.cleanup()


def _run_campaign_verified(
    args: argparse.Namespace,
    *,
    viewer_root: Path,
    artifact_root: Path,
    output: Path,
    worklist: dict[str, Any],
    worklist_path: Path,
    source_roots: tuple[Path, ...],
    selection: str,
) -> dict[str, Any]:
    _ensure_external_output(output, source_roots, viewer_root)
    entries = select_entries(worklist, "smoke")
    if not entries:
        raise CampaignError(f"selection contains no cases: {selection}")
    binary = args.binary.resolve()
    if not os.access(binary, os.X_OK):
        raise CampaignError(f"dcmview binary is not executable: {binary}")
    version = subprocess.run(
        [str(binary), "--version"], check=True, capture_output=True, text=True, timeout=5
    ).stdout.strip()
    input_paths = [row["campaign_occurrence"]["normalized_path"] for row in entries]
    verified_paths = [row["normalized_path"] for row in worklist["files"]]
    if len(input_paths) != len(verified_paths) or set(input_paths) != set(verified_paths):
        raise CampaignError(
            "stored smoke selection does not pass every verified payload to the viewer"
        )
    command = [
        str(binary), "--no-browser", "--host", "127.0.0.1", "--port", "0",
        "--startup-json", *input_paths,
    ]
    environment = dict(os.environ)
    environment["DCMVIEW_VSCODE_BYPASS"] = "1"
    started_at = utc_now()
    shard_started = time.monotonic()
    process = CapturedProcess(command, environment, viewer_root)
    base_url = ""
    results: list[dict[str, Any]] = []
    shutdown_forced = False
    exit_code = -1
    campaign_error: Optional[str] = None
    try:
        base_url = process.wait_for_startup_url(args.startup_timeout)
        scan = poll_files(
            base_url,
            args.request_timeout,
            time.monotonic() + args.startup_timeout,
        )
        series_response = http_request(base_url, "/api/series", args.request_timeout)
        if series_response["status"] != 200 or not isinstance(series_response["json"], dict):
            raise CampaignError("series catalog did not return a JSON success response")
        if series_response["json"].get("scan_complete") is not True:
            raise CampaignError("series catalog did not report scan_complete")
        series_http = evidence(series_response)
        by_identity = {
            (_normalized_path(row["path"]), row["sop_instance_uid"]): row
            for row in scan["files"]
        }
        for entry in entries:
            if time.monotonic() - shard_started >= args.shard_timeout:
                raise TimeoutError(f"shard exceeded {args.shard_timeout:.1f}s")
            occurrence = entry["campaign_occurrence"]
            key = (_normalized_path(occurrence["normalized_path"]), entry["sop_instance_uid"])
            summary = by_identity.get(key)
            if summary is None:
                result = _finish_result(
                    entry,
                    sorted(entry["expected_contract"].get("expected_capabilities") or []),
                    {"mapped_after_scan": False},
                    {},
                    [{"code": "discovery_omission", "message": "path and SOP UID not found"}],
                    time.monotonic(),
                    "safe" if process.process.poll() is None else "crash",
                )
            else:
                result = probe_case(
                    base_url,
                    entry,
                    summary,
                    args.request_timeout,
                    args.case_timeout,
                    series_response["json"],
                    series_http,
                )
            if process.process.poll() is not None:
                result["execution_safety"] = "crash"
                result["compatibility"] = "failure"
            results.append(result)
    except (CampaignError, TimeoutError, OSError, urllib.error.URLError) as error:
        campaign_error = str(error)
    finally:
        exit_code, shutdown_forced = process.shutdown(args.shutdown_timeout)
        stdout_path = output / "stdout.log"
        stderr_path = output / "stderr.log"
        process.write_logs(stdout_path, stderr_path)

    counts = {
        outcome: sum(row["compatibility"] == outcome for row in results)
        for outcome in sorted(COMPATIBILITY_OUTCOMES)
    }
    safety_counts = {
        outcome: sum(row["execution_safety"] == outcome for row in results)
        for outcome in sorted(EXECUTION_OUTCOMES)
    }
    schema_path = Path(__file__).with_name("detail-schema.json")
    report = {
        "detail_schema_version": DETAIL_SCHEMA_VERSION,
        "generated_at": utc_now(),
        "worklist": {
            "path": str(worklist_path),
            "sha256": sha256_file(worklist_path),
            "content_sha256": worklist["worklist_sha256"],
        },
        "viewer": {"binary": str(binary), "sha256": sha256_file(binary), "version": version},
        "run": {
            "started_at": started_at,
            "completed_at": utc_now(),
            "selection": selection,
            "base_url": base_url,
            "command": command,
            "timeouts_seconds": {
                "startup": args.startup_timeout,
                "request": args.request_timeout,
                "case": args.case_timeout,
                "shard": args.shard_timeout,
                "shutdown": args.shutdown_timeout,
            },
            "exit_code": exit_code,
            "shutdown_forced": shutdown_forced,
            "campaign_error": campaign_error,
        },
        "results": results,
        "summary": {
            "selected": len(entries),
            "results": len(results),
            "compatibility": counts,
            "execution_safety": safety_counts,
            "campaign_complete": campaign_error is None and len(results) == len(entries),
        },
        "artifacts": [artifact(output / "stdout.log", "stdout"), artifact(output / "stderr.log", "stderr")],
        "validation": {
            "schema": "detail-schema.json",
            "schema_sha256": sha256_file(schema_path),
            "status": "passed",
        },
    }
    validate_report(report)
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    validate_json_schema(report, schema)
    report_path = output / "report.json"
    report_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    normalized = normalized_report(report)
    normalized_path = output / "normalized.json"
    normalized_path.write_text(
        json.dumps(normalized, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    viewer_commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=viewer_root, check=True,
        capture_output=True, text=True, timeout=5,
    ).stdout.strip()
    evidence_report = build_evidence_report(report, worklist, viewer_commit, ["default"])
    evidence_schema_path = Path(__file__).with_name("evidence-schema.json")
    validate_json_schema(evidence_report, json.loads(evidence_schema_path.read_text(encoding="utf-8")))
    evidence_path = output / "evidence-report.json"
    evidence_path.write_text(json.dumps(evidence_report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    viewer_report = build_viewer_report(evidence_report)
    viewer_schema_path = Path(__file__).with_name("viewer-report.schema.json")
    validate_json_schema(viewer_report, json.loads(viewer_schema_path.read_text(encoding="utf-8")))
    viewer_report_path = output / "viewer-report.json"
    viewer_report_path.write_text(json.dumps(viewer_report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    index_paths = [
        (report_path, "report"),
        (normalized_path, "normalized_report"),
        (evidence_path, "evidence_report"),
        (viewer_report_path, "suite_viewer_report"),
        (output / "stdout.log", "stdout"),
        (output / "stderr.log", "stderr"),
    ]
    index_paths.append((worklist_path, "corpus_manifest"))
    index = {"artifacts": [artifact(path, kind) for path, kind in index_paths]}
    (output / "artifact-index.json").write_text(
        json.dumps(index, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return report


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--corpus-root", type=Path,
        help="published producer container containing smoke.tar.gz and artifact-index.json (smoke only)",
        required=True,
    )
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expected-seed", type=int, default=os.environ.get("DCMVIEW_CORPUS_SEED", 1))
    parser.add_argument("--expected-profile", default=os.environ.get("DCMVIEW_CORPUS_PROFILE", "smoke"))
    parser.add_argument("--expected-manifest-sha256", default=os.environ.get("DCMVIEW_CORPUS_MANIFEST_SHA256"))
    parser.add_argument("--expected-manifest-size-bytes", type=int, default=os.environ.get("DCMVIEW_CORPUS_MANIFEST_SIZE_BYTES"))
    parser.add_argument(
        "--expected-corpus-definition-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_DEFINITION_SHA256"),
    )
    parser.add_argument(
        "--expected-definition-manifest-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_DEFINITION_MANIFEST_SHA256"),
    )
    parser.add_argument(
        "--expected-generator-version",
        default=os.environ.get("DCMVIEW_CORPUS_GENERATOR_VERSION"),
    )
    parser.add_argument(
        "--expected-generator-revision",
        default=os.environ.get("DCMVIEW_CORPUS_GENERATOR_REVISION"),
    )
    parser.add_argument(
        "--expected-actions-zip-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_ACTIONS_ZIP_SHA256"),
    )
    parser.add_argument(
        "--expected-actions-zip-size-bytes",
        type=int,
        default=os.environ.get("DCMVIEW_CORPUS_ACTIONS_ZIP_SIZE_BYTES"),
    )
    parser.add_argument(
        "--expected-nested-archive-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_NESTED_ARCHIVE_SHA256"),
    )
    parser.add_argument(
        "--expected-nested-archive-size-bytes",
        type=int,
        default=os.environ.get("DCMVIEW_CORPUS_NESTED_ARCHIVE_SIZE_BYTES"),
    )
    parser.add_argument(
        "--expected-release-manifest-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_RELEASE_MANIFEST_SHA256"),
    )
    parser.add_argument(
        "--expected-release-manifest-size-bytes",
        type=int,
        default=os.environ.get("DCMVIEW_CORPUS_RELEASE_MANIFEST_SIZE_BYTES"),
    )
    parser.add_argument(
        "--expected-installed-binary-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_INSTALLED_BINARY_SHA256"),
    )
    parser.add_argument(
        "--expected-installed-binary-size-bytes",
        type=int,
        default=os.environ.get("DCMVIEW_CORPUS_INSTALLED_BINARY_SIZE_BYTES"),
    )
    parser.add_argument("--expected-target", default=os.environ.get("DCMVIEW_CORPUS_GENERATOR_TARGET"))
    parser.add_argument("--expected-toolchain", default=os.environ.get("DCMVIEW_CORPUS_GENERATOR_TOOLCHAIN"))
    parser.add_argument(
        "--expected-generator-feature",
        dest="expected_generator_features",
        action="append",
        default=None,
        help="repeat for each expected generator feature; an empty list is the default",
    )
    parser.add_argument(
        "--expected-runtime-identities-sha256",
        default=os.environ.get("DCMVIEW_CORPUS_RUNTIME_IDENTITIES_SHA256"),
    )
    parser.add_argument("--expected-binding-id", default=os.environ.get("DCMVIEW_CORPUS_BINDING_ID"))
    parser.add_argument("--expected-archive-sha256", default=os.environ.get("DCMVIEW_CORPUS_ARCHIVE_SHA256"))
    parser.add_argument(
        "--expected-archive-size-bytes",
        type=int,
        default=os.environ.get("DCMVIEW_CORPUS_ARCHIVE_SIZE_BYTES"),
    )
    parser.add_argument("--startup-timeout", type=float, default=20.0)
    parser.add_argument("--request-timeout", type=float, default=10.0)
    parser.add_argument("--case-timeout", type=float, default=60.0)
    parser.add_argument("--shard-timeout", type=float, default=1800.0)
    parser.add_argument("--shutdown-timeout", type=float, default=10.0)
    return parser.parse_args(argv)


def main(argv: Optional[list[str]] = None) -> int:
    try:
        args = parse_args(sys.argv[1:] if argv is None else argv)
        if args.expected_generator_features is None:
            args.expected_generator_features = ()
        else:
            args.expected_generator_features = tuple(args.expected_generator_features)
        report = run_campaign(args)
    except (CampaignError, CompatibilityError, ArtifactError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"compatibility campaign error: {error}", file=sys.stderr)
        return 2
    print(json.dumps(report["summary"], sort_keys=True))
    return 0 if report["summary"]["campaign_complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

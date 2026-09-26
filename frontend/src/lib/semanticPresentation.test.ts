import { describe, expect, it } from "vitest";
import type { SemanticContextResponse } from "../generated/api-types";
import {
	codedConceptLabel,
	formatDeclaredVector,
	gridFrameOffsetSummary,
	mappingFormula,
	segmentationOverlaySelection,
	semanticKindLabel,
	semanticModeLabel,
	supportsSemanticContext,
} from "./semanticPresentation";

describe("semantic presentation labels", () => {
	it("makes the active interpretation mode explicit", () => {
		expect(semanticModeLabel("pixel_preview")).toBe("Pixel Preview");
		expect(semanticModeLabel("semantic_context")).toBe("Semantic Context");
	});

	it("does not promote non-semantic objects", () => {
		expect(semanticKindLabel({ kind: "not_applicable", reason: "not derived" })).toBe(
			"Generic image",
		);
	});

	it("shows semantic controls only for supported derived image types", () => {
		expect(supportsSemanticContext("segmentation", "seg")).toBe(true);
		expect(supportsSemanticContext("parametric_map", "pm")).toBe(true);
		expect(supportsSemanticContext("radiation_therapy", "1.2.840.10008.5.1.4.1.1.481.2")).toBe(true);
		expect(supportsSemanticContext("classic_image", "ct")).toBe(false);
		expect(supportsSemanticContext("radiation_therapy", "1.2.840.10008.5.1.4.1.1.481.1")).toBe(false);
	});

	it("renders declared codes and mappings without inventing missing values", () => {
		expect(
			codedConceptLabel({ value: "HU", scheme: "UCUM", meaning: "Hounsfield unit" }),
		).toBe("Hounsfield unit (HU · UCUM)");
		expect(codedConceptLabel(null)).toBe("Not declared");
		expect(mappingFormula(2, -1)).toBe("mapped = stored × 2 + -1");
		expect(mappingFormula(null, null)).toBeNull();
	});

	it("selects the uniquely resolved source frame for a SEG overlay", () => {
		const response = {
			source_file_index: 4,
			default_mode: "pixel_preview",
			pixel_preview_preserves_stored_values: true,
			context: {
				kind: "segmentation",
				frame_mappings: [{
					frame_index: 1,
					mapping_status: "resolved",
					source_frames: [{
						file_index: 193,
						frame_index: 0,
						sop_instance_uid: "1.2.3",
					}],
				}],
			},
		} as unknown as SemanticContextResponse;

		expect(segmentationOverlaySelection(response, 1)).toEqual({
			segmentationFileIndex: 4,
			segmentationFrameIndex: 1,
			sourceFileIndex: 193,
			sourceFrameIndex: 0,
		});
		expect(segmentationOverlaySelection(response, 0)).toBeNull();
	});

	it("rejects ambiguous SEG frame mappings", () => {
		const response = {
			source_file_index: 4,
			context: {
				kind: "segmentation",
				frame_mappings: [{
					frame_index: 0,
					mapping_status: "ambiguous",
					source_frames: [],
				}],
			},
		} as unknown as SemanticContextResponse;

		expect(segmentationOverlaySelection(response, 0)).toBeNull();
	});
});

describe("RT Dose grid presentation", () => {
	it("formats declared vectors without inventing absent ones", () => {
		expect(formatDeclaredVector([-120.5, 30, 0.123456])).toBe("-120.5 \\ 30 \\ 0.1235");
		expect(formatDeclaredVector(null)).toBeNull();
	});

	it("summarizes the Grid Frame Offset Vector", () => {
		expect(gridFrameOffsetSummary([])).toBeNull();
		expect(gridFrameOffsetSummary([0])).toBe("1 plane at 0 mm");
		expect(gridFrameOffsetSummary([0, 2.5, 5, 7.5])).toBe("4 planes, 0 to 7.5 mm, 2.5 mm step");
		expect(gridFrameOffsetSummary([0, 2, 5])).toBe("3 planes, 0 to 5 mm, uneven spacing");
	});
});

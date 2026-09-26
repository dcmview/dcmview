import type { CodedConceptSummary, SegmentSummary, SemanticContext } from "../generated/api-types";
import type { SemanticContextResponse } from "../generated/api-types";

export type SemanticMode = "pixel_preview" | "semantic_context";

export interface SegmentationOverlaySelection {
	segmentationFileIndex: number;
	segmentationFrameIndex: number;
	sourceFileIndex: number;
	sourceFrameIndex: number;
}

export function segmentationOverlaySelection(
	response: SemanticContextResponse | null,
	frameIndex: number,
): SegmentationOverlaySelection | null {
	if (!response || response.context.kind !== "segmentation") return null;
	const mapping = response.context.frame_mappings.find(
		(candidate) => candidate.frame_index === frameIndex,
	);
	if (!mapping || mapping.mapping_status !== "resolved" || mapping.source_frames.length !== 1) {
		return null;
	}
	const source = mapping.source_frames[0];
	return {
		segmentationFileIndex: response.source_file_index,
		segmentationFrameIndex: frameIndex,
		sourceFileIndex: source.file_index,
		sourceFrameIndex: source.frame_index,
	};
}

const RT_DOSE_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.481.2";

export function supportsSemanticContext(objectKind: string, sopClassUid: string): boolean {
	return objectKind === "segmentation"
		|| objectKind === "parametric_map"
		|| sopClassUid === RT_DOSE_SOP_CLASS_UID;
}

export function semanticModeLabel(mode: SemanticMode): string {
	return mode === "pixel_preview" ? "Pixel Preview" : "Semantic Context";
}

export function semanticKindLabel(context: SemanticContext): string {
	switch (context.kind) {
		case "segmentation":
			return "Segmentation";
		case "parametric_map":
			return "Parametric Map";
		case "rt_dose":
			return "RT Dose";
		case "not_applicable":
			return "Generic image";
	}
}

export function codedConceptLabel(code: CodedConceptSummary | null): string {
	if (!code) return "Not declared";
	const identity = [code.value, code.scheme].filter(Boolean).join(" · ");
	return identity.length > 0 ? `${code.meaning} (${identity})` : code.meaning;
}

export function mappingFormula(slope: number | null, intercept: number | null): string | null {
	if (slope === null && intercept === null) return null;
	const resolvedSlope = slope ?? 1;
	const resolvedIntercept = intercept ?? 0;
	return `mapped = stored × ${resolvedSlope} + ${resolvedIntercept}`;
}

export type Rgb = [number, number, number];

/** Where the server's overlay color for a segment comes from, for the panel. */
export function segmentColorSource(segment: SegmentSummary): string {
	switch (segment.display_color_source) {
		case "recommended_cielab":
			return `recommended CIELab ${segment.recommended_display_cielab?.join(" \\ ")}`;
		case "recommended_grayscale":
			return `recommended grayscale ${segment.recommended_display_grayscale}`;
		default:
			return "palette, no recommended color declared";
	}
}

/**
 * A declared recommended color the overlay does not paint: a grayscale value
 * beside the CIELab value it prefers, or a CIELab value without three
 * components. `color` is set when the value can be shown as a swatch.
 */
export function unusedRecommendedColor(
	segment: SegmentSummary,
): { text: string; color: Rgb | null } | null {
	const cielab = segment.recommended_display_cielab;
	if (cielab !== null && segment.display_color_source !== "recommended_cielab") {
		return { text: `CIELab ${cielab.join(" \\ ")} (malformed)`, color: null };
	}
	const grayscale = segment.recommended_display_grayscale;
	if (grayscale !== null && segment.display_color_source !== "recommended_grayscale") {
		// A P-Value from 0 (black) to 0xFFFF (white).
		const level = Math.round((grayscale * 255) / 0xffff);
		return { text: `grayscale ${grayscale}`, color: [level, level, level] };
	}
	return null;
}

export function rgbCss([red, green, blue]: Rgb): string {
	return `rgb(${red}, ${green}, ${blue})`;
}

/** A declared number rounded for display, without trailing zeros. */
export function formatDeclaredNumber(value: number): string {
	return String(Number(value.toFixed(4)));
}

/** A declared vector such as Image Position (Patient), or null when absent. */
export function formatDeclaredVector(values: readonly number[] | null): string | null {
	return values === null ? null : values.map(formatDeclaredNumber).join(" \\ ");
}

/**
 * Summarize the Grid Frame Offset Vector: plane count, offset range, and
 * whether the planes are evenly spaced. The full vector stays in the tag
 * panel, since a dose grid can declare hundreds of planes.
 */
export function gridFrameOffsetSummary(offsets: readonly number[]): string | null {
	if (offsets.length === 0) return null;
	const first = offsets[0];
	const last = offsets[offsets.length - 1];
	const planes = `${offsets.length} ${offsets.length === 1 ? "plane" : "planes"}`;
	if (offsets.length === 1) return `${planes} at ${formatDeclaredNumber(first)} mm`;
	const step = offsets[1] - offsets[0];
	const uniform = offsets.every(
		(offset, index) => index === 0 || Math.abs(offset - offsets[index - 1] - step) <= 1e-3,
	);
	const spacing = uniform ? `${formatDeclaredNumber(step)} mm step` : "uneven spacing";
	return `${planes}, ${formatDeclaredNumber(first)} to ${formatDeclaredNumber(last)} mm, ${spacing}`;
}

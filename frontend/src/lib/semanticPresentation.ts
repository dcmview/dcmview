import type { CodedConceptSummary, SemanticContext } from "../generated/api-types";
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

const SEGMENT_OVERLAY_COLORS: readonly Rgb[] = [
	[255, 79, 132],
	[42, 211, 199],
	[255, 190, 92],
	[136, 132, 255],
	[114, 218, 111],
	[255, 126, 92],
];

/**
 * The color the server paints a segment's overlay with. It mirrors
 * `fallback_segment_color` in `src/semantic.rs`: a fixed palette cycled by
 * segment number, independent of the Recommended Display CIELab Value,
 * because the semantic context contract does not carry the overlay color.
 */
export function segmentOverlayColor(segmentNumber: number): Rgb {
	const index = Math.max(segmentNumber - 1, 0) % SEGMENT_OVERLAY_COLORS.length;
	return SEGMENT_OVERLAY_COLORS[index];
}

// D50 reference white of the DICOM/ICC Profile Connection Space.
const D50_WHITE = [0.96422, 1, 0.82521] as const;
// XYZ (D50) to linear sRGB, Bradford-adapted to sRGB's D65 white.
const XYZ_D50_TO_LINEAR_SRGB = [
	[3.1338561, -1.6168667, -0.4906146],
	[-0.9787684, 1.9161415, 0.033454],
	[0.0719453, -0.2289914, 1.4052427],
] as const;

/**
 * Convert a Recommended Display CIELab Value to sRGB. DICOM encodes the
 * PCS-Values as unsigned 16-bit integers: L* 0..100 and a*, b* -128..127
 * scaled over 0..0xFFFF, relative to D50. Out-of-gamut colors are clamped.
 */
export function dicomCielabToRgb(values: readonly number[] | null): Rgb | null {
	if (values === null || values.length !== 3 || values.some((value) => !Number.isFinite(value))) {
		return null;
	}
	const lightness = (values[0] * 100) / 0xffff;
	const a = (values[1] * 255) / 0xffff - 128;
	const b = (values[2] * 255) / 0xffff - 128;
	const fy = (lightness + 16) / 116;
	const inverse = (t: number) => (t ** 3 > 216 / 24389 ? t ** 3 : (116 * t - 16) / (24389 / 27));
	const xyz = [inverse(fy + a / 500), inverse(fy), inverse(fy - b / 200)].map(
		(component, axis) => component * D50_WHITE[axis],
	);
	return XYZ_D50_TO_LINEAR_SRGB.map((row) => {
		const linear = row[0] * xyz[0] + row[1] * xyz[1] + row[2] * xyz[2];
		const clamped = Math.min(Math.max(linear, 0), 1);
		const encoded = clamped <= 0.0031308 ? 12.92 * clamped : 1.055 * clamped ** (1 / 2.4) - 0.055;
		return Math.round(encoded * 255);
	}) as Rgb;
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

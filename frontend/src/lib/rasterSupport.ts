import type { FileSummary } from "../api";

const REASON_DETAIL: Record<string, string> = {
	"raster.unsupported_color": "Its color layout is not one dcmview will decode.",
	"raster.unsupported_sample_format": "Its sample format is not one dcmview will decode.",
	"raster.unsupported_compression": "Its compression is not one dcmview will decode.",
	"raster.jpeg_unsupported_process": "Its JPEG process is not one dcmview will decode.",
	"raster.too_large": "It has more pixels in a frame than dcmview will decode.",
};

/** What the tag panel is called for a file: a raster has metadata, not DICOM tags. */
export function tagPanelNames(file: Pick<FileSummary, "file_format"> | null): { title: string; panel: string } {
	return file !== null && file.file_format !== "dicom"
		? { title: "Metadata", panel: "metadata panel" }
		: { title: "DICOM tags", panel: "DICOM tag panel" };
}

/**
 * Why a raster image file cannot be drawn, from the catalog alone: the
 * server answers 422 for every frame of such a file, so nothing about its
 * frames is requested. `null` for DICOM and for a raster that can be drawn.
 */
export function unsupportedImageReason(
	file: Pick<FileSummary, "file_format" | "support_state" | "support_reason">,
): string | null {
	if (file.file_format === "dicom" || file.support_state !== "unsupported") return null;
	return REASON_DETAIL[file.support_reason ?? ""] ?? "dcmview cannot decode this file.";
}

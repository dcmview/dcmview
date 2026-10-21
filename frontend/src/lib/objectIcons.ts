import type { FileSummary } from "../api";
import type { IconName } from "./ui/icons";

const REPORT_KINDS = new Set([
	"structured_report",
	"key_object_selection",
	"encapsulated_pdf",
	"presentation_state",
	"registration",
	"real_world_value_mapping",
	"waveform",
]);

/** The line icon that names a file's object kind in the explorer. */
export function fileIcon(file: Pick<FileSummary, "object_kind" | "modality">): IconName {
	switch (file.object_kind) {
		case "image":
			return "image";
		case "whole_slide_microscopy":
			return "wsi";
		case "segmentation":
			return "seg";
		case "parametric_map":
			return "pmap";
		case "radiation_therapy":
			return file.modality === "RTDOSE" ? "dose" : "report";
		default:
			return REPORT_KINDS.has(file.object_kind) ? "report" : "file";
	}
}

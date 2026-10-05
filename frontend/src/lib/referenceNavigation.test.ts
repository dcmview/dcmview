import { describe, expect, it } from "vitest";
import type { FileSummary, ReferenceMatchSummary, ReferenceTargetSummary } from "../api";
import {
	referenceDestination,
	referenceDetails,
	referenceIdentity,
} from "./referenceNavigation";

function file(index: number, frameCount: number): FileSummary {
	return {
		index,
		path: `/data/${index}.dcm`,
		display_name: `${index}.dcm`,
		label: `${index}.dcm`,
		patient_id: "",
		patient_name: "",
		study_instance_uid: "",
		study_date: "",
		study_description: "",
		series_instance_uid: "",
		series_number: "",
		series_description: "",
		modality: "",
		instance_number: "",
		sop_instance_uid: `1.2.3.${index}`,
		sop_class_uid: "1.2.840.10008.5.1.4.1.1.2",
		object_kind: "classic_image",
		file_format: "dicom",
		raster: null,
		support_state: "renderable",
		support_reason: null,
		raw_windowing_compatible: true,
		raw_windowing_reason: null,
		presentation_layer: false,
		burned_in_annotation: false,
		has_pixels: true,
		frame_count: frameCount,
		rows: 2,
		columns: 2,
		pixel_aspect_ratio: null,
		transfer_syntax_uid: "1.2.840.10008.1.2.1",
		default_window: null,
	};
}

function match(overrides: Partial<ReferenceMatchSummary> = {}): ReferenceMatchSummary {
	return {
		file_index: 4,
		path: "/data/4.dcm",
		sop_instance_uid: "1.2.3.4",
		frame_indices: [],
		...overrides,
	};
}

const target: ReferenceTargetSummary = {
	sop_class_uid: "1.2.840.10008.5.1.4.1.1.2",
	sop_instance_uid: "1.2.3.4",
	series_instance_uid: "1.2.3",
	frame_numbers: [2, 5],
	segment_numbers: [7],
};

describe("reference navigation", () => {
	it("selects the first returned frame that is valid for the resolved local file", () => {
		expect(referenceDestination(match({ frame_indices: [9, 2, 1] }), [file(4, 3)]))
			.toMatchObject({ file: { index: 4 }, frameIndex: 2 });
	});

	it("opens an unrestricted resolved reference at its first local frame", () => {
		expect(referenceDestination(match(), [file(4, 3)]))
			.toMatchObject({ file: { index: 4 }, frameIndex: 0 });
	});

	it("keeps missing files and malformed frame restrictions inert", () => {
		expect(referenceDestination(match(), [])).toBeNull();
		expect(referenceDestination(match({ frame_indices: [-1, 3] }), [file(4, 3)]))
			.toBeNull();
	});

	it("retains declared target identity, frame, and segment metadata", () => {
		expect(referenceIdentity(target)).toBe("SOP Instance 1.2.3.4");
		expect(referenceDetails(target)).toEqual(["frame 2, 5", "segment 7"]);
		expect(referenceIdentity({ ...target, sop_instance_uid: null })).toBe("Series 1.2.3");
	});
});

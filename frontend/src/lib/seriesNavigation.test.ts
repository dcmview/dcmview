import { describe, expect, it } from "vitest";
import type { SeriesStackSummary, SeriesSummary } from "../api";
import {
	findSeriesStackForFile,
	frameAtPosition,
	framePosition,
	navigationFrameAtPosition,
	navigationFramesForFile,
	navigationTabId,
} from "./seriesNavigation";

function stack(): SeriesStackSummary {
	return {
		id: "study:s|series:x|stack:ordinary",
		kind: "ordinary",
		concatenation_uid: null,
		pyramid_uid: null,
		image_type_role: null,
		total_pixel_matrix_rows: null,
		total_pixel_matrix_columns: null,
		warnings: [],
		frames: [
			{
				virtual_index: 0,
				file_index: 7,
				frame_index: 0,
				sop_instance_uid: "a",
				instance_number: 30,
				position_along_normal_mm: 0,
			},
			{
				virtual_index: 1,
				file_index: 9,
				frame_index: 0,
				sop_instance_uid: "b",
				instance_number: 10,
				position_along_normal_mm: 5,
			},
			{
				virtual_index: 2,
				file_index: 9,
				frame_index: 1,
				sop_instance_uid: "b",
				instance_number: 10,
				position_along_normal_mm: 10,
			},
		],
	};
}

function catalog(): SeriesSummary[] {
	return [{
		id: "study:s|series:x",
		study_instance_uid: "s",
		series_instance_uid: "x",
		frame_of_reference_uids: ["for"],
		stacks: [stack()],
	}];
}

describe("series navigation", () => {
	it("locates source files in server-owned stacks", () => {
		expect(findSeriesStackForFile(catalog(), 9)?.stack.id).toContain("ordinary");
		expect(findSeriesStackForFile(catalog(), 99)).toBeNull();
		expect(navigationTabId(catalog(), 9)).toContain("ordinary");
		expect(navigationTabId(catalog(), 99)).toBe("file:99");
	});

	it("maps source frames to virtual positions and clamps selection", () => {
		const value = stack();
		expect(framePosition(value, 9, 1)).toBe(2);
		expect(framePosition(value, 9, 99)).toBe(1);
		expect(framePosition(value, 99, 0)).toBeNull();
		expect(frameAtPosition(value, -1)?.file_index).toBe(7);
		expect(frameAtPosition(value, 99)?.frame_index).toBe(1);
	});

	it("normalizes single-file and mixed-source stacks to the same frame identity", () => {
		expect(navigationFramesForFile(4, 3)).toEqual([
		{ virtual_index: 0, file_index: 4, frame_index: 0 },
		{ virtual_index: 1, file_index: 4, frame_index: 1 },
		{ virtual_index: 2, file_index: 4, frame_index: 2 },
	]);

		const mixed = stack().frames;
		expect(navigationFrameAtPosition(mixed, 0)).toMatchObject({
			file_index: 7,
			frame_index: 0,
		});
		expect(navigationFrameAtPosition(mixed, 2)).toMatchObject({
			file_index: 9,
			frame_index: 1,
		});
		expect(navigationFrameAtPosition(mixed, 99)).toMatchObject({
			file_index: 9,
			frame_index: 1,
		});
	});
});

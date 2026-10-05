import { describe, expect, it } from "vitest";
import type { EmbedRoiAnnotations } from "../../api";
import { formatRoiFrames, hitTestRoi, roiCoord, roiHandles, visibleRois } from "./roiEditing";

const annotations: EmbedRoiAnnotations = {
	num_roi: 3,
	roi_coords: [[10, 10, 50, 50], [30, 30, 70, 70], [0, 0, 5, 5]],
	roi_frames: [[0, 1], [1], [2]],
};

describe("visibleRois", () => {
	it("keeps ROIs mapped to the frame with their original index", () => {
		expect(visibleRois(annotations, 1).map((roi) => roi.index)).toEqual([0, 1]);
		expect(visibleRois(annotations, 2)).toEqual([{ index: 2, ymin: 0, xmin: 0, ymax: 5, xmax: 5, frames: [2] }]);
		expect(visibleRois(null, 0)).toEqual([]);
	});

	it("shows every ROI on every frame when no frame mapping exists", () => {
		const unmapped = { ...annotations, roi_frames: [] };
		expect(visibleRois(unmapped, 9).map((roi) => [roi.index, roi.frames])).toEqual([[0, null], [1, null], [2, null]]);
	});
});

describe("hitTestRoi", () => {
	const rois = visibleRois(annotations, 1);

	it("prefers the topmost ROI and reports interior hits without a handle", () => {
		const hit = hitTestRoi(rois, { x: 40, y: 40 }, 1);
		expect(hit?.roi.index).toBe(1);
		expect(hit?.handle).toBeNull();
		expect(hitTestRoi(rois, { x: 20, y: 20 }, 1)?.roi.index).toBe(0);
		expect(hitTestRoi(rois, { x: 90, y: 90 }, 1)).toBeNull();
	});

	it("grabs handles within about eight screen pixels", () => {
		expect(hitTestRoi(rois, { x: 78, y: 50 }, 1)).toMatchObject({ roi: { index: 1 }, handle: "e" });
		expect(hitTestRoi(rois, { x: 79, y: 50 }, 1)).toBeNull();
		// Zoomed in 4x the tolerance shrinks to its three-pixel floor.
		expect(hitTestRoi(rois, { x: 72, y: 72 }, 4)).toMatchObject({ handle: "se" });
		expect(hitTestRoi(rois, { x: 74, y: 74 }, 4)).toBeNull();
	});

	it("grabs handles within the tolerance it is given", () => {
		expect(hitTestRoi(rois, { x: 80, y: 50 }, 1, 10)).toMatchObject({ roi: { index: 1 }, handle: "e" });
		expect(hitTestRoi(rois, { x: 81, y: 50 }, 1, 10)).toBeNull();
		// Half of it at 2x.
		expect(hitTestRoi(rois, { x: 75, y: 50 }, 2, 10)).toMatchObject({ handle: "e" });
		expect(hitTestRoi(rois, { x: 76, y: 50 }, 2, 10)).toBeNull();
	});
});

describe("ROI helpers", () => {
	it("places handles on corners and edge midpoints of inverted rectangles too", () => {
		const [roi] = visibleRois({ num_roi: 1, roi_coords: [[20, 40, 0, 0]], roi_frames: [] }, 0);
		expect(roiHandles(roi).map(({ handle, x, y }) => `${handle}:${x},${y}`)).toEqual([
			"nw:0,0", "n:20,0", "ne:40,0", "e:40,10", "se:40,20", "s:20,20", "sw:0,20", "w:0,10",
		]);
		expect(roiCoord(roi)).toEqual([20, 40, 0, 0]);
	});

	it("summarizes frame scope", () => {
		expect(formatRoiFrames(null, 3)).toBe("all frames");
		expect(formatRoiFrames([0, 1, 2], 3)).toBe("all frames");
		expect(formatRoiFrames([], 3)).toBe("no frame mapping");
		expect(formatRoiFrames([1, 2], 3)).toBe("frames 1, 2");
		expect(formatRoiFrames([0, 1, 2, 3, 4, 5, 6], 9)).toBe("frames 0, 1, 2, 3, 4, 5, …");
	});
});

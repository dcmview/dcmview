import { describe, expect, it } from "vitest";
import type { EmbedRoiAnnotations } from "../api";
import {
	addRoi,
	allFrames,
	canonicalRect,
	deleteRoi,
	emptyAnnotations,
	isAllFrames,
	moveCoord,
	normalizeAnnotationsForEdit,
	resizeCoord,
	setRoiFrameScope,
	updateRoiCoord,
} from "./annotationGeometry";

function annotations(
	roi_coords: EmbedRoiAnnotations["roi_coords"],
	roi_frames: EmbedRoiAnnotations["roi_frames"] = [],
): EmbedRoiAnnotations {
	return { num_roi: roi_coords.length, roi_coords, roi_frames };
}

describe("canonicalRect", () => {
	it("orders, rounds, and clamps a dragged rectangle to the image", () => {
		expect(canonicalRect({ x: 30.6, y: 40.4 }, { x: -5, y: 10 }, 32, 24)).toEqual([10, 0, 32, 24]);
	});

	it("rejects rectangles narrower or shorter than two pixels", () => {
		expect(canonicalRect({ x: 5, y: 5 }, { x: 6.4, y: 20 }, 32, 32)).toBeNull();
		expect(canonicalRect({ x: 5, y: 5 }, { x: 20, y: 6 }, 32, 32)).toBeNull();
		expect(canonicalRect({ x: 5, y: 5 }, { x: 7, y: 7 }, 32, 32)).toEqual([5, 5, 7, 7]);
	});
});

describe("normalizeAnnotationsForEdit", () => {
	it("starts from an empty set when nothing is loaded", () => {
		expect(normalizeAnnotationsForEdit(null, 3)).toEqual(emptyAnnotations());
	});

	it("canonicalizes inverted coordinates", () => {
		const next = normalizeAnnotationsForEdit(annotations([[9, 8, 1, 2]], [[0]]), 1);
		expect(next.roi_coords).toEqual([[1, 2, 9, 8]]);
	});

	it("expands an empty frame mapping to every frame", () => {
		const next = normalizeAnnotationsForEdit(annotations([[0, 0, 4, 4], [1, 1, 5, 5]]), 3);
		expect(next.roi_frames).toEqual([[0, 1, 2], [0, 1, 2]]);
	});

	it("deduplicates, sorts, and drops out-of-range frames", () => {
		const next = normalizeAnnotationsForEdit(annotations([[0, 0, 4, 4]], [[2, 0, 2, 7, -1, 1.5]]), 3);
		expect(next.roi_frames).toEqual([[0, 2]]);
	});

	it("pads a short frame mapping so every ROI has an entry", () => {
		const next = normalizeAnnotationsForEdit(annotations([[0, 0, 4, 4], [1, 1, 5, 5]], [[1]]), 3);
		expect(next).toEqual({
			num_roi: 2,
			roi_coords: [[0, 0, 4, 4], [1, 1, 5, 5]],
			roi_frames: [[1], []],
		});
	});

	it("does not mutate its input", () => {
		const source = annotations([[9, 8, 1, 2]]);
		normalizeAnnotationsForEdit(source, 2);
		expect(source).toEqual(annotations([[9, 8, 1, 2]]));
	});
});

describe("ROI edits", () => {
	it("adds a ROI scoped to the current frame", () => {
		const next = addRoi(annotations([[0, 0, 4, 4]]), [2, 2, 6, 6], 1, 2);
		expect(next).toEqual({
			num_roi: 2,
			roi_coords: [[0, 0, 4, 4], [2, 2, 6, 6]],
			roi_frames: [[0, 1], [1]],
		});
	});

	it("adds the first ROI to a file without annotations", () => {
		expect(addRoi(null, [2, 2, 6, 6], 0, 1)).toEqual(annotations([[2, 2, 6, 6]], [[0]]));
	});

	it("replaces only the edited ROI's coordinates", () => {
		const next = updateRoiCoord(annotations([[0, 0, 4, 4], [1, 1, 5, 5]], [[0], [1]]), 1, [3, 3, 8, 8], 2);
		expect(next.roi_coords).toEqual([[0, 0, 4, 4], [3, 3, 8, 8]]);
		expect(next.roi_frames).toEqual([[0], [1]]);
	});

	it("deletes a ROI together with its frame mapping", () => {
		const next = deleteRoi(annotations([[0, 0, 4, 4], [1, 1, 5, 5]], [[0], [1]]), 0, 2);
		expect(next).toEqual(annotations([[1, 1, 5, 5]], [[1]]));
	});

	it("scopes a ROI to the current frame or to every frame", () => {
		const source = annotations([[0, 0, 4, 4], [1, 1, 5, 5]], [[0], [0]]);
		expect(setRoiFrameScope(source, 1, "current", 2, 3).roi_frames).toEqual([[0], [2]]);
		expect(setRoiFrameScope(source, 1, "all", 2, 3).roi_frames).toEqual([[0], [0, 1, 2]]);
	});
});

describe("moveCoord", () => {
	it("translates by a rounded delta", () => {
		expect(moveCoord([2, 3, 6, 9], { x: 1.6, y: -1.2 }, 20, 20)).toEqual([1, 5, 5, 11]);
	});

	it("keeps the rectangle size while clamping it inside the image", () => {
		expect(moveCoord([2, 3, 6, 9], { x: 100, y: -100 }, 20, 16)).toEqual([0, 10, 4, 16]);
	});
});

describe("resizeCoord", () => {
	it("moves only the edges named by the handle", () => {
		expect(resizeCoord([2, 2, 10, 10], "se", { x: 14, y: 12 }, 20, 20)).toEqual([2, 2, 12, 14]);
		expect(resizeCoord([2, 2, 10, 10], "n", { x: 99, y: 5 }, 20, 20)).toEqual([5, 2, 10, 10]);
		expect(resizeCoord([2, 2, 10, 10], "w", { x: 4, y: 99 }, 20, 20)).toEqual([2, 4, 10, 10]);
	});

	it("re-canonicalizes when a handle is dragged past the opposite edge", () => {
		expect(resizeCoord([2, 2, 10, 10], "nw", { x: 15, y: 16 }, 20, 20)).toEqual([10, 10, 16, 15]);
	});

	it("clamps the dragged edge to the image and rejects collapsed rectangles", () => {
		expect(resizeCoord([2, 2, 10, 10], "e", { x: 50, y: 0 }, 20, 16)).toEqual([2, 2, 10, 16]);
		expect(resizeCoord([2, 2, 10, 10], "s", { x: 0, y: 3 }, 20, 20)).toBeNull();
	});
});

describe("frame helpers", () => {
	it("lists every frame index", () => {
		expect(allFrames(0)).toEqual([]);
		expect(allFrames(3)).toEqual([0, 1, 2]);
	});

	it("recognizes an all-frames mapping", () => {
		expect(isAllFrames(null, 3)).toBe(true);
		expect(isAllFrames([0, 1, 2], 3)).toBe(true);
		expect(isAllFrames([0, 2], 3)).toBe(false);
		expect(isAllFrames([0, 1], 3)).toBe(false);
	});
});

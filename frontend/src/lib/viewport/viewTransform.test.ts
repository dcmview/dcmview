import { describe, expect, it } from "vitest";
import { imageDisplayGeometry } from "../imageGeometry";
import { DEFAULT_ORIENTATION, type ImageOrientation } from "../viewerTools";
import {
	clampZoom,
	clientToImagePoint,
	imageToViewportPoint,
	layerTransformCss,
	nextZoomStep,
	rotateClockwise,
	rotateCounterClockwise,
	flipHorizontal,
	flipVertical,
	sameTransform,
	zoomAnchor,
	zoomAroundAnchor,
	type ViewTransform,
} from "./viewTransform";

const origin = { left: 40, top: 25 };

describe("clientToImagePoint", () => {
	const transform: ViewTransform = { scale: 2.5, tx: -30, ty: 12, fit: false };

	it("maps the untransformed layer one CSS pixel per image pixel", () => {
		const geometry = imageDisplayGeometry(64, 128, 1);
		const point = clientToImagePoint(
			{ x: origin.left + 10.5, y: origin.top + 3.25 },
			origin,
			{ scale: 1, tx: 0, ty: 0, fit: false },
			DEFAULT_ORIENTATION,
			geometry,
		);
		expect(point.x).toBeCloseTo(10.5);
		expect(point.y).toBeCloseTo(3.25);
	});

	it("inverts zoom, pan, rotation, flips, and pixel aspect ratio", () => {
		const rows = 40;
		const columns = 90;
		const ratio = 1.75;
		const geometry = imageDisplayGeometry(rows, columns, ratio);
		const orientations: ImageOrientation[] = [];
		for (const rotation of [0, 90, 180, 270] as const) {
			for (const flipH of [false, true]) {
				for (const flipV of [false, true]) orientations.push({ rotation, flipH, flipV });
			}
		}
		for (const orientation of orientations) {
			for (const imagePoint of [{ x: 0, y: 0 }, { x: 89.5, y: 3 }, { x: 17.25, y: 39.9 }]) {
				const local = imageToViewportPoint(imagePoint, transform, orientation, geometry);
				const client = { x: origin.left + local.x, y: origin.top + local.y };
				const mapped = clientToImagePoint(client, origin, transform, orientation, geometry);
				expect(mapped.x, JSON.stringify(orientation)).toBeCloseTo(imagePoint.x, 6);
				expect(mapped.y, JSON.stringify(orientation)).toBeCloseTo(imagePoint.y, 6);
			}
		}
	});

	it("puts the top-left pixel at the top-right corner after a clockwise rotation", () => {
		const geometry = imageDisplayGeometry(10, 10, 1);
		const identity = { scale: 1, tx: 0, ty: 0, fit: false };
		const point = clientToImagePoint(
			{ x: origin.left + 9.5, y: origin.top + 0.5 },
			origin,
			identity,
			{ rotation: 90, flipH: false, flipV: false },
			geometry,
		);
		expect(point.x).toBeCloseTo(0.5);
		expect(point.y).toBeCloseTo(0.5);
	});

	it("maps the top-left image corner to the top-right after a clockwise rotation", () => {
		const geometry = imageDisplayGeometry(10, 20, 1);
		const point = imageToViewportPoint(
			{ x: 0, y: 0 },
			{ scale: 2, tx: 5, ty: 7, fit: false },
			{ rotation: 90, flipH: false, flipV: false },
			geometry,
		);
		// Rotated about the center (10, 5): the 20x10 layer spans x 5..15, y -5..15.
		expect(point.x).toBeCloseTo(5 + 15 * 2);
		expect(point.y).toBeCloseTo(7 - 5 * 2);
	});

	it("leaves points outside the image unclamped", () => {
		const geometry = imageDisplayGeometry(10, 10, 1);
		const point = clientToImagePoint(
			{ x: origin.left - 5, y: origin.top + 50 },
			origin,
			{ scale: 1, tx: 0, ty: 0, fit: false },
			DEFAULT_ORIENTATION,
			geometry,
		);
		expect(point).toEqual({ x: -5, y: 50 });
	});
});

describe("zooming", () => {
	it("keeps the anchored point under the cursor", () => {
		const transform: ViewTransform = { scale: 1.5, tx: 20, ty: -10, fit: true };
		const anchor = zoomAnchor(300, 200, origin, transform);
		const zoomed = zoomAroundAnchor(4, anchor, origin);
		expect(zoomed.scale).toBe(4);
		expect(origin.left + zoomed.tx + anchor.localX * zoomed.scale).toBeCloseTo(300);
		expect(origin.top + zoomed.ty + anchor.localY * zoomed.scale).toBeCloseTo(200);
	});

	it("clamps zoom to the supported range", () => {
		expect(clampZoom(0.001)).toBe(0.05);
		expect(clampZoom(1000)).toBe(64);
		const anchor = zoomAnchor(0, 0, origin, { scale: 1, tx: 0, ty: 0, fit: false });
		expect(zoomAroundAnchor(500, anchor, origin).scale).toBe(64);
	});

	it("steps between preset zoom levels", () => {
		expect(nextZoomStep(1, 1)).toBe(1.25);
		expect(nextZoomStep(1, -1)).toBe(0.75);
		expect(nextZoomStep(0.3, 1)).toBe(0.5);
		expect(nextZoomStep(0.3, -1)).toBe(0.25);
		expect(nextZoomStep(64, 1)).toBeUndefined();
		expect(nextZoomStep(0.05, -1)).toBeUndefined();
	});

	it("treats sub-threshold differences as the same transform", () => {
		const base: ViewTransform = { scale: 2, tx: 10, ty: 10, fit: false };
		expect(sameTransform(base, { ...base, tx: 10.001 })).toBe(true);
		expect(sameTransform(base, { ...base, fit: true })).toBe(false);
		expect(sameTransform(undefined, base)).toBe(false);
	});
});

describe("orientation", () => {
	it("rotates in quarter turns and toggles flips", () => {
		expect(rotateClockwise({ ...DEFAULT_ORIENTATION, rotation: 270 }).rotation).toBe(0);
		expect(rotateCounterClockwise(DEFAULT_ORIENTATION).rotation).toBe(270);
		expect(flipHorizontal(flipHorizontal(DEFAULT_ORIENTATION))).toEqual(DEFAULT_ORIENTATION);
		expect(flipVertical(DEFAULT_ORIENTATION).flipV).toBe(true);
	});

	it("adds the orientation transform only when the image is reoriented", () => {
		const geometry = imageDisplayGeometry(20, 10, 1);
		const transform = { scale: 2, tx: 3, ty: 4, fit: false };
		expect(layerTransformCss(transform, DEFAULT_ORIENTATION, geometry)).toBe("translate(3px, 4px) scale(2)");
		expect(layerTransformCss(transform, { rotation: 90, flipH: true, flipV: false }, geometry)).toBe(
			"translate(3px, 4px) scale(2) translate(5px,10px) rotate(90deg) scale(-1,1) translate(-5px,-10px)",
		);
	});
});

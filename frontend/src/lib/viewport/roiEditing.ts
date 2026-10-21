import type { EmbedRoiAnnotations } from "../../api";
import { isAllFrames, type ImagePoint, type RoiCoord, type RoiHandle } from "../annotationGeometry";
import type { ImageOrientation } from "../viewerTools";

/** A ROI shown on the current frame; `frames` is null when it applies to every frame. */
export type VisibleRoi = {
	index: number;
	ymin: number;
	xmin: number;
	ymax: number;
	xmax: number;
	frames: number[] | null;
};

export type RoiHandlePoint = { handle: RoiHandle; x: number; y: number };

export type RoiHit = { roi: VisibleRoi; handle: RoiHandle | null };

export function visibleRois(annotations: EmbedRoiAnnotations | null, frameIndex: number): VisibleRoi[] {
	if (!annotations || annotations.roi_coords.length === 0) return [];
	const appliesToAllFrames = annotations.roi_frames.length === 0;
	const visible: VisibleRoi[] = [];
	for (let idx = 0; idx < annotations.roi_coords.length; idx += 1) {
		const [ymin, xmin, ymax, xmax] = annotations.roi_coords[idx];
		const frames = appliesToAllFrames ? null : annotations.roi_frames[idx] ?? [];
		if (frames !== null && !frames.includes(frameIndex)) continue;
		visible.push({ index: idx, ymin, xmin, ymax, xmax, frames });
	}
	return visible;
}

export function roiCoord(roi: VisibleRoi): RoiCoord {
	return [roi.ymin, roi.xmin, roi.ymax, roi.xmax];
}

function roiBounds(roi: VisibleRoi): { x0: number; x1: number; y0: number; y1: number } {
	return {
		x0: Math.min(roi.xmin, roi.xmax),
		x1: Math.max(roi.xmin, roi.xmax),
		y0: Math.min(roi.ymin, roi.ymax),
		y1: Math.max(roi.ymin, roi.ymax),
	};
}

/** Corner and edge-midpoint resize handles, clockwise from the top-left. */
export function roiHandles(roi: VisibleRoi): RoiHandlePoint[] {
	const { x0, x1, y0, y1 } = roiBounds(roi);
	const cx = (x0 + x1) / 2;
	const cy = (y0 + y1) / 2;
	return [
		{ handle: "nw", x: x0, y: y0 },
		{ handle: "n", x: cx, y: y0 },
		{ handle: "ne", x: x1, y: y0 },
		{ handle: "e", x: x1, y: cy },
		{ handle: "se", x: x1, y: y1 },
		{ handle: "s", x: cx, y: y1 },
		{ handle: "sw", x: x0, y: y1 },
		{ handle: "w", x: x0, y: cy },
	];
}

/**
 * The topmost ROI under `point` and the handle it grabbed, if any. The handle
 * tolerance is about `tolerancePx` screen pixels at the current zoom `scale`.
 */
export function hitTestRoi(rois: readonly VisibleRoi[], point: ImagePoint, scale: number, tolerancePx = 8): RoiHit | null {
	const tolerance = Math.max(3, tolerancePx / Math.max(scale, 0.2));
	for (let idx = rois.length - 1; idx >= 0; idx -= 1) {
		const roi = rois[idx];
		const handle = roiHandles(roi).find((candidate) => (
			Math.abs(point.x - candidate.x) <= tolerance && Math.abs(point.y - candidate.y) <= tolerance
		));
		if (handle) return { roi, handle: handle.handle };
		const { x0, x1, y0, y1 } = roiBounds(roi);
		if (point.x >= x0 && point.x <= x1 && point.y >= y0 && point.y <= y1) {
			return { roi, handle: null };
		}
	}
	return null;
}

/** Where each handle points from the rectangle's centre, in image axes (y down). */
const HANDLE_DIRECTIONS: Record<RoiHandle, readonly [number, number]> = {
	n: [0, -1], ne: [1, -1], e: [1, 0], se: [1, 1], s: [0, 1], sw: [-1, 1], w: [-1, 0], nw: [-1, -1],
};

/**
 * The resize cursor for `handle` as the image is drawn: flipped, then
 * rotated clockwise, the way the image layer is transformed.
 */
export function handleCursor(handle: RoiHandle, orientation: ImageOrientation): string {
	let [x, y] = HANDLE_DIRECTIONS[handle];
	if (orientation.flipH) x = -x;
	if (orientation.flipV) y = -y;
	for (let turn = 0; turn < orientation.rotation / 90; turn += 1) [x, y] = [-y, x];
	if (x === 0) return "ns-resize";
	if (y === 0) return "ew-resize";
	return x === y ? "nwse-resize" : "nesw-resize";
}

export function formatRoiFrames(frames: number[] | null, frameCount: number): string {
	if (frames === null || isAllFrames(frames, frameCount)) return "all frames";
	if (frames.length === 0) return "no frame mapping";
	const preview = frames.slice(0, 6).join(", ");
	return frames.length > 6 ? `frames ${preview}, …` : `frames ${preview}`;
}

import type { EmbedRoiAnnotations } from "../../api";
import { isAllFrames, type ImagePoint, type RoiCoord, type RoiHandle } from "../annotationGeometry";

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
 * tolerance is about eight screen pixels at the current zoom `scale`.
 */
export function hitTestRoi(rois: readonly VisibleRoi[], point: ImagePoint, scale: number): RoiHit | null {
	const tolerance = Math.max(3, 8 / Math.max(scale, 0.2));
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

export function formatRoiFrames(frames: number[] | null, frameCount: number): string {
	if (frames === null || isAllFrames(frames, frameCount)) return "all frames";
	if (frames.length === 0) return "no frame mapping";
	const preview = frames.slice(0, 6).join(", ");
	return frames.length > 6 ? `frames ${preview}, …` : `frames ${preview}`;
}

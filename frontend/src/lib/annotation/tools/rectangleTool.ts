import type { EmbedRoiAnnotations } from "../../../api";
import {
	addRoi,
	canonicalRect,
	moveCoord,
	resizeCoord,
	setRoiFrameScope,
	updateRoiCoord,
	type ImagePoint,
	type RoiCoord,
	type RoiHandle,
} from "../../annotationGeometry";
import { hitTestRoi, roiCoord } from "../../viewport/roiEditing";
import type { DraftRect, Tool, ToolContext, ToolPointer } from "./tool";

type RectangleState =
	| { phase: "idle" }
	| { phase: "drawing"; start: ImagePoint; current: ImagePoint }
	| { phase: "moving"; roiIndex: number; start: ImagePoint; original: RoiCoord }
	| { phase: "resizing"; roiIndex: number; handle: RoiHandle; original: RoiCoord };

/**
 * Draws a rectangle, or moves or resizes the one under the pointer. The ROI
 * and Redact tools are two of these; `ctx.rects` decides which rectangles
 * they edit.
 */
export class RectangleTool implements Tool {
	readonly frameBound = true;
	#state: RectangleState = { phase: "idle" };
	// The file's rectangles as they were when the gesture began, for `cancel`.
	#before: { fileIndex: number; original: EmbedRoiAnnotations | null } | null = null;

	constructor(readonly id: "annotate_rect" | "redact") {}

	get draft(): DraftRect | null {
		const state = this.#state;
		return state.phase === "drawing" ? { start: state.start, current: state.current } : null;
	}

	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" | "ignore" {
		if (!ctx.rects.editable) return "ignore";
		const point = ctx.toImage(pointer.clientX, pointer.clientY);
		if (!point) return "ignore";
		this.#before = { fileIndex: ctx.file.index, original: ctx.rects.annotations };
		const hit = hitTestRoi(ctx.rects.visible, point, ctx.transform.scale);
		if (hit) {
			ctx.rects.select(hit.roi.index);
			ctx.rects.beginLiveEdit();
			const original = roiCoord(hit.roi);
			this.#state = hit.handle
				? { phase: "resizing", roiIndex: hit.roi.index, handle: hit.handle, original }
				: { phase: "moving", roiIndex: hit.roi.index, start: point, original };
			return "capture";
		}
		ctx.rects.select(null);
		this.#state = { phase: "drawing", start: point, current: point };
		return "capture";
	}

	pointerMove(pointer: ToolPointer, ctx: ToolContext): void {
		const state = this.#state;
		if (state.phase === "drawing") {
			const point = ctx.toImage(pointer.clientX, pointer.clientY);
			if (point) {
				this.#state = { ...state, current: point };
			}
			return;
		}

		const annotations = ctx.rects.annotations;
		if (state.phase === "moving" && annotations) {
			const point = ctx.toImage(pointer.clientX, pointer.clientY);
			if (!point) return;
			const moved = moveCoord(
				state.original,
				{ x: point.x - state.start.x, y: point.y - state.start.y },
				ctx.imageRows,
				ctx.imageColumns,
			);
			const next = updateRoiCoord(annotations, state.roiIndex, moved, ctx.file.frame_count);
			ctx.rects.showDraft(ctx.file.index, next);
			return;
		}

		if (state.phase === "resizing" && annotations) {
			const point = ctx.toImage(pointer.clientX, pointer.clientY);
			if (!point) return;
			const resized = resizeCoord(state.original, state.handle, point, ctx.imageRows, ctx.imageColumns);
			if (!resized) return;
			const next = updateRoiCoord(annotations, state.roiIndex, resized, ctx.file.frame_count);
			ctx.rects.showDraft(ctx.file.index, next);
		}
	}

	pointerUp(ctx: ToolContext): void {
		const state = this.#state;
		if (state.phase === "drawing") {
			const coord = canonicalRect(state.start, state.current, ctx.imageRows, ctx.imageColumns);
			if (coord) {
				const added = addRoi(ctx.rects.annotations, coord, ctx.frame, ctx.file.frame_count);
				// A ROI marks the frame it is drawn on; a redaction box covers every frame.
				const next = ctx.rects.coversAllFrames
					? setRoiFrameScope(added, added.num_roi - 1, "all", ctx.frame, ctx.file.frame_count)
					: added;
				ctx.rects.commit(next, next.num_roi - 1);
			}
		}
		const annotations = ctx.rects.annotations;
		if ((state.phase === "moving" || state.phase === "resizing") && annotations) {
			ctx.rects.commit(annotations, ctx.rects.selectedIndex);
		}
		this.reset();
	}

	cancel(ctx: ToolContext): void {
		if (this.#before?.original) {
			ctx.rects.showDraft(this.#before.fileIndex, this.#before.original);
		}
		this.reset();
	}

	reset(): void {
		this.#state = { phase: "idle" };
		this.#before = null;
	}
}

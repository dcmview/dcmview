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

/** How near a handle a press grabs it, in screen pixels: a trackpad points less precisely at the moment of clicking. */
const HANDLE_TOLERANCE_PX = { mouse: 8, trackpad: 10 } as const;

/** A press that travels less than this many screen pixels before its release is a click. */
const CLICK_TRAVEL_PX = 4;

type RectangleState =
	| { phase: "idle" }
	| {
		phase: "drawing";
		start: ImagePoint;
		current: ImagePoint;
		/** Where the first press landed, and whether the pointer has left it. */
		press: ToolPointer;
		dragged: boolean;
		/** The first corner was fixed by a click; the next click places the opposite one. */
		placing: boolean;
	}
	| { phase: "moving"; roiIndex: number; start: ImagePoint; original: RoiCoord }
	| { phase: "resizing"; roiIndex: number; handle: RoiHandle; original: RoiCoord };

/**
 * Draws a rectangle, or moves or resizes the one under the pointer. A
 * rectangle is drawn by a drag, or by two clicks: a press and release on
 * bare image that does not move fixes one corner, the rectangle follows the
 * pointer, and the next click places it. The ROI and Redact tools are two of
 * these; `ctx.rects` decides which rectangles they edit.
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

	/** One corner is fixed and the rectangle follows the pointer until the next click. */
	get armed(): boolean {
		return this.#state.phase === "drawing" && this.#state.placing;
	}

	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" | "ignore" {
		const state = this.#state;
		if (state.phase === "drawing" && state.placing) {
			// The second click: it places the corner wherever it lands, on a rectangle or not.
			const corner = ctx.toImage(pointer.clientX, pointer.clientY);
			if (corner) this.#state = { ...state, current: corner };
			return "capture";
		}
		if (!ctx.rects.editable) return "ignore";
		const point = ctx.toImage(pointer.clientX, pointer.clientY);
		if (!point) return "ignore";
		this.#before = { fileIndex: ctx.file.index, original: ctx.rects.annotations };
		const hit = hitTestRoi(ctx.rects.visible, point, ctx.transform.scale, HANDLE_TOLERANCE_PX[ctx.inputProfile]);
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
		this.#state = { phase: "drawing", start: point, current: point, press: pointer, dragged: false, placing: false };
		return "capture";
	}

	pointerMove(pointer: ToolPointer, ctx: ToolContext): void {
		const state = this.#state;
		if (state.phase === "drawing") {
			const point = ctx.toImage(pointer.clientX, pointer.clientY);
			const dragged = state.dragged || Math.hypot(pointer.clientX - state.press.clientX,
				pointer.clientY - state.press.clientY) >= CLICK_TRAVEL_PX;
			this.#state = { ...state, current: point ?? state.current, dragged };
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
			// A click, which a drag would have dropped as a sliver, fixes the first corner instead.
			if (!coord && !state.placing && !state.dragged) {
				this.#state = { ...state, placing: true };
				return;
			}
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
		// A rectangle being drawn has shown nothing through the store, and the
		// file's rectangles may have changed between its two clicks.
		if (this.#state.phase !== "drawing" && this.#before?.original) {
			ctx.rects.showDraft(this.#before.fileIndex, this.#before.original);
		}
		this.reset();
	}

	reset(): void {
		this.#state = { phase: "idle" };
		this.#before = null;
	}
}

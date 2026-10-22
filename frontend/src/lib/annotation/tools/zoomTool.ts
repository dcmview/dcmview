import type { ZoomAnchor } from "../../viewport/viewTransform";
import type { Tool, ToolContext, ToolPointer } from "./tool";

type ZoomState =
	| { phase: "idle" }
	| { phase: "zooming"; startY: number; baseScale: number; anchor: ZoomAnchor };

/**
 * Drags up to zoom in and down to zoom out, about the point the drag began
 * on. The host also starts it for a right-button drag.
 */
export class ZoomTool implements Tool {
	readonly id = "zoom";
	readonly frameBound = false;
	#state: ZoomState = { phase: "idle" };

	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" | "ignore" {
		const anchor = ctx.zoomAnchor(pointer.clientX, pointer.clientY);
		if (!anchor) return "ignore";
		this.#state = {
			phase: "zooming",
			startY: pointer.clientY,
			baseScale: ctx.transform.scale,
			anchor,
		};
		return "capture";
	}

	pointerMove(pointer: ToolPointer, ctx: ToolContext): void {
		if (this.#state.phase !== "zooming") return;
		const dy = pointer.clientY - this.#state.startY;
		const transform = ctx.zoomTransform(this.#state.baseScale * Math.exp(-dy * 0.005), this.#state.anchor);
		if (!transform) return;
		ctx.setTransform(transform);
	}

	pointerUp(): void {
		this.reset();
	}

	cancel(): void {
		this.reset();
	}

	reset(): void {
		this.#state = { phase: "idle" };
	}
}

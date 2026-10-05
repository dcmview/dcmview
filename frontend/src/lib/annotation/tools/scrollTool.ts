import type { Tool, ToolContext, ToolPointer, ToolWheel } from "./tool";

const FRAME_SCROLL_SPEED_FACTOR = 0.7;
const DRAG_PIXELS_PER_FRAME = 10 / FRAME_SCROLL_SPEED_FACTOR;

type ScrollState =
	| { phase: "idle" }
	| { phase: "scrolling"; startY: number; baseFrame: number };

/** Steps through the frames by dragging vertically or by the wheel. */
export class ScrollTool implements Tool {
	readonly id = "scroll";
	readonly frameBound = false;
	#state: ScrollState = { phase: "idle" };

	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" | "ignore" {
		if (ctx.navigation.count <= 1) return "ignore";
		this.#state = {
			phase: "scrolling",
			startY: pointer.clientY,
			baseFrame: ctx.navigation.position,
		};
		return "capture";
	}

	pointerMove(pointer: ToolPointer, ctx: ToolContext): void {
		if (this.#state.phase !== "scrolling" || ctx.navigation.count <= 1) return;
		const dy = pointer.clientY - this.#state.startY;
		const frameDelta = Math.round(dy / DRAG_PIXELS_PER_FRAME);
		ctx.navigation.go(
			Math.max(0, Math.min(ctx.navigation.count - 1, this.#state.baseFrame + frameDelta)),
		);
	}

	pointerUp(): void {
		this.reset();
	}

	wheel(wheel: ToolWheel, ctx: ToolContext): boolean {
		if (ctx.navigation.count <= 1 || wheel.dy === 0) return false;
		ctx.navigation.go(ctx.navigation.position + (wheel.dy > 0 ? 1 : -1));
		return true;
	}

	cancel(): void {
		this.reset();
	}

	reset(): void {
		this.#state = { phase: "idle" };
	}
}

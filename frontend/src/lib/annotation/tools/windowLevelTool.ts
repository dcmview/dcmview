import type { Tool, ToolContext, ToolPointer } from "./tool";

type WindowLevelState =
	| { phase: "idle" }
	| { phase: "adjusting"; startX: number; startY: number; baseCenter: number; baseWidth: number; step: number };

/** Drags horizontally for the window width and vertically for its center. */
export class WindowLevelTool implements Tool {
	readonly id = "window_level";
	readonly frameBound = false;
	#state: WindowLevelState = { phase: "idle" };

	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" | "ignore" {
		const base = ctx.window.begin();
		if (!base) return "ignore";
		this.#state = {
			phase: "adjusting",
			startX: pointer.clientX,
			startY: pointer.clientY,
			baseCenter: base.center,
			baseWidth: base.width,
			step: base.step,
		};
		return "capture";
	}

	pointerMove(pointer: ToolPointer, ctx: ToolContext): void {
		if (this.#state.phase !== "adjusting") return;
		const dx = pointer.clientX - this.#state.startX;
		const dy = pointer.clientY - this.#state.startY;
		const nextWidth = Math.max(this.#state.step, this.#state.baseWidth + dx * 4 * this.#state.step);
		const nextCenter = this.#state.baseCenter - dy * 2 * this.#state.step;
		ctx.window.preview(nextCenter, nextWidth);
	}

	pointerUp(ctx: ToolContext): void {
		if (this.#state.phase === "adjusting") ctx.window.commit();
		this.reset();
	}

	cancel(): void {
		this.reset();
	}

	reset(): void {
		this.#state = { phase: "idle" };
	}
}

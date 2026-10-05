import type { Tool, ToolContext, ToolPointer } from "./tool";

type PanState =
	| { phase: "idle" }
	| { phase: "panning"; startX: number; startY: number; baseTx: number; baseTy: number };

/** Drags the image. The host also starts it for a middle-button drag. */
export class PanTool implements Tool {
	readonly id = "pan";
	readonly frameBound = false;
	#state: PanState = { phase: "idle" };

	pointerDown(pointer: ToolPointer, ctx: ToolContext): "capture" {
		this.#state = {
			phase: "panning",
			startX: pointer.clientX,
			startY: pointer.clientY,
			baseTx: ctx.transform.tx,
			baseTy: ctx.transform.ty,
		};
		return "capture";
	}

	pointerMove(pointer: ToolPointer, ctx: ToolContext): void {
		if (this.#state.phase !== "panning") return;
		const dx = pointer.clientX - this.#state.startX;
		const dy = pointer.clientY - this.#state.startY;
		ctx.setTransform({
			...ctx.transform,
			tx: this.#state.baseTx + dx,
			ty: this.#state.baseTy + dy,
		});
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

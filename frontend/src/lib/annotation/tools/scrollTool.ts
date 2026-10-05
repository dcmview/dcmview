import type { Tool, ToolContext, ToolPointer, ToolWheel } from "./tool";

const FRAME_SCROLL_SPEED_FACTOR = 0.7;
const DRAG_PIXELS_PER_FRAME = 10 / FRAME_SCROLL_SPEED_FACTOR;

/** Two fingers travel this far for one frame. */
const TRACKPAD_PIXELS_PER_FRAME = 30;
/** This many steps, each smaller than the one before, are taken for a momentum tail... */
const TAIL_DROPS = 3;
/** ...once they have fallen to this share of the size the run began at. */
const TAIL_SHARE = 0.7;

/**
 * Turns a trackpad scroll into frame steps: one per 30 px of travel, not
 * one per event. The browser does not say when the fingers lift, so the
 * momentum the system adds afterwards is recognised by its shape: step
 * sizes that keep falling without ever rising. Stepping stops there, and
 * resumes when a step grows again or stays level for longer than a tail
 * would hold it.
 */
class TrackpadFrameSteps {
	#carry = 0;
	#direction = 0;
	#previous = 0;
	// The largest and smallest step since the sizes last rose.
	#peak = 0;
	#low = 0;
	#drops = 0;
	#level = 0;
	#coasting = false;

	/** Whole frames to step for a vertical step of `dy` pixels. */
	add(dy: number, gestureStart: boolean): number {
		const size = Math.abs(dy);
		const direction = Math.sign(dy);
		if (gestureStart || direction !== this.#direction) {
			this.#carry = 0;
			this.#direction = direction;
			this.#fingersMoved(size);
		} else if (size > this.#low * 1.15 + 0.5) {
			this.#fingersMoved(size);
		} else {
			if (size < this.#previous) {
				this.#drops += 1;
				this.#level = 0;
			} else {
				this.#level += 1;
			}
			this.#low = Math.min(this.#low, size);
			// A tail falls ever more slowly, so it holds a size of n pixels for
			// about 1/n as long as it holds one pixel. Longer than that is a hand.
			if (this.#coasting && this.#level > Math.max(6, 48 / Math.max(size, 1))) this.#fingersMoved(size);
			else if (this.#drops >= TAIL_DROPS && size <= this.#peak * TAIL_SHARE) this.#coasting = true;
		}
		this.#previous = size;
		if (this.#coasting) {
			this.#carry = 0;
			return 0;
		}
		this.#carry += dy;
		const frames = Math.trunc(this.#carry / TRACKPAD_PIXELS_PER_FRAME);
		this.#carry -= frames * TRACKPAD_PIXELS_PER_FRAME;
		return frames;
	}

	#fingersMoved(size: number): void {
		this.#peak = size;
		this.#low = size;
		this.#drops = 0;
		this.#level = 0;
		this.#coasting = false;
	}
}

type ScrollState =
	| { phase: "idle" }
	| { phase: "scrolling"; startY: number; baseFrame: number };

/**
 * Steps through the frames by dragging vertically or by the wheel: one frame
 * per notch of a mouse wheel, one per 30 px of a trackpad scroll.
 */
export class ScrollTool implements Tool {
	readonly id = "scroll";
	readonly frameBound = false;
	#state: ScrollState = { phase: "idle" };
	readonly #trackpad = new TrackpadFrameSteps();

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
		if (wheel.device === "mouse") {
			ctx.navigation.go(ctx.navigation.position + (wheel.dy > 0 ? 1 : -1));
			return true;
		}
		const frames = this.#trackpad.add(wheel.dy, wheel.gestureStart);
		const position = Math.max(0, Math.min(ctx.navigation.count - 1, ctx.navigation.position + frames));
		if (position !== ctx.navigation.position) ctx.navigation.go(position);
		return true;
	}

	cancel(): void {
		this.reset();
	}

	reset(): void {
		this.#state = { phase: "idle" };
	}
}

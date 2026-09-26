import { afterEach, describe, expect, it, vi } from "vitest";
import {
	buildCineLookahead,
	canRunCinePlayback,
	cineFrameIntervalMs,
	nextCineStep,
	runRenderPacedCine,
	waitForAbortableResult,
	waitForCineDeadline,
} from "./cinePlayback";
import { navigationFrameAtPosition, type NavigationFrameRef } from "./seriesNavigation";

describe("cine playback policy", () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it("allows playback only in the display pipeline with multiple pixel frames", () => {
		expect(canRunCinePlayback("cine", true, 2)).toBe(true);
		expect(canRunCinePlayback("diagnostic_wl", true, 2)).toBe(false);
		expect(canRunCinePlayback("cine", false, 2)).toBe(false);
		expect(canRunCinePlayback("cine", true, 1)).toBe(false);
	});

	it("wraps loop playback in both directions", () => {
		expect(nextCineStep(4, 5, "loop", 1)).toEqual({ frame: 0, direction: 1 });
		expect(nextCineStep(0, 5, "loop", -1)).toEqual({ frame: 4, direction: -1 });
	});

	it("reverses sweep playback without holding the endpoint", () => {
		expect(nextCineStep(4, 5, "sweep", 1)).toEqual({ frame: 3, direction: -1 });
		expect(nextCineStep(0, 5, "sweep", -1)).toEqual({ frame: 1, direction: 1 });
	});

	it("builds circular loop lookahead across the stack boundary", () => {
		expect(buildCineLookahead(3, 5, "loop", 1, 4)).toEqual([4, 0, 1, 2]);
	});

	it("builds direction-aware sweep lookahead", () => {
		expect(buildCineLookahead(3, 5, "sweep", 1, 4)).toEqual([4, 2, 1, 0]);
		expect(buildCineLookahead(4, 5, "sweep", 1, 4)).toEqual([3, 2, 1, 0]);
	});

	it("treats configured FPS as an interval ceiling", () => {
		expect(cineFrameIntervalMs(10)).toBe(100);
		expect(cineFrameIntervalMs(0)).toBe(1000);
	});

	it("releases abort listeners and subscriptions after normal completion", async () => {
		const ctrl = new AbortController();
		const addListener = vi.spyOn(ctrl.signal, "addEventListener");
		const removeListener = vi.spyOn(ctrl.signal, "removeEventListener");
		const cleanup = vi.fn();
		const subscription: { settle?: (value: boolean) => void } = {};
		const result = waitForAbortableResult(ctrl.signal, (finish) => {
			subscription.settle = finish;
			return cleanup;
		});

		subscription.settle?.(true);

		await expect(result).resolves.toBe(true);
		expect(cleanup).toHaveBeenCalledOnce();
		expect(removeListener).toHaveBeenCalledWith("abort", addListener.mock.calls[0][1]);
	});

	it("cleans up an abortable wait when playback stops", async () => {
		const ctrl = new AbortController();
		const cleanup = vi.fn();
		const result = waitForAbortableResult(ctrl.signal, () => cleanup);

		ctrl.abort();

		await expect(result).resolves.toBe(false);
		expect(cleanup).toHaveBeenCalledOnce();
	});

	it("releases the cine deadline listener after its timer fires", async () => {
		vi.useFakeTimers();
		const ctrl = new AbortController();
		const removeListener = vi.spyOn(ctrl.signal, "removeEventListener");
		const result = waitForCineDeadline(25, ctrl.signal);

		await vi.advanceTimersByTimeAsync(25);

		await expect(result).resolves.toBe(true);
		expect(removeListener).toHaveBeenCalledOnce();
	});

	it("waits for slow frame preparation instead of issuing catch-up frames", async () => {
		vi.useFakeTimers();
		const ctrl = new AbortController();
		const presentedAt: number[] = [];
		const playback = runRenderPacedCine({
			initialFrame: 0,
			totalFrames: 4,
			mode: "loop",
			direction: 1,
			fps: 100,
			signal: ctrl.signal,
			now: () => Date.now(),
			waitForDelay: waitForCineDeadline,
			prepareFrame: () => new Promise((resolve) => setTimeout(resolve, 25)),
			presentFrame: async () => {
				presentedAt.push(Date.now());
				if (presentedAt.length === 3) ctrl.abort();
				return true;
			},
		});

		await vi.advanceTimersByTimeAsync(200);
		await playback;

		// A 10 ms frame interval cannot outrun 25 ms preparation, and late
		// frames are not presented back to back to catch up.
		expect(presentedAt).toHaveLength(3);
		expect(presentedAt[1] - presentedAt[0]).toBe(25);
		expect(presentedAt[2] - presentedAt[1]).toBe(25);
	});

	it("paces fast preparation to the configured frame interval", async () => {
		vi.useFakeTimers();
		const ctrl = new AbortController();
		const presentedAt: number[] = [];
		const playback = runRenderPacedCine({
			initialFrame: 0,
			totalFrames: 4,
			mode: "loop",
			direction: 1,
			fps: 10,
			signal: ctrl.signal,
			now: () => Date.now(),
			waitForDelay: waitForCineDeadline,
			prepareFrame: async () => {},
			presentFrame: async () => {
				presentedAt.push(Date.now());
				if (presentedAt.length === 3) ctrl.abort();
				return true;
			},
		});

		await vi.advanceTimersByTimeAsync(500);
		await playback;

		expect(presentedAt).toHaveLength(3);
		expect(presentedAt[1] - presentedAt[0]).toBe(100);
		expect(presentedAt[2] - presentedAt[1]).toBe(100);
	});

	it("prepares logical cine positions across source-file boundaries", async () => {
		const frames: NavigationFrameRef[] = [
			{ virtual_index: 0, file_index: 7, frame_index: 0 },
			{ virtual_index: 1, file_index: 9, frame_index: 0 },
			{ virtual_index: 2, file_index: 9, frame_index: 1 },
		];
		const ctrl = new AbortController();
		const prepared: string[] = [];
		const presented: number[] = [];

		await runRenderPacedCine({
			initialFrame: 0,
			totalFrames: frames.length,
			mode: "loop",
			direction: 1,
			fps: 30,
			signal: ctrl.signal,
			now: () => 0,
			waitForDelay: async () => true,
			prepareFrame: async (position) => {
				const frame = navigationFrameAtPosition(frames, position);
				if (frame) prepared.push(`${frame.file_index}:${frame.frame_index}`);
			},
			presentFrame: async (step) => {
				presented.push(step.frame);
				if (presented.length === 3) ctrl.abort();
				return true;
			},
		});

		expect(prepared).toEqual(["9:0", "9:1", "7:0"]);
		expect(presented).toEqual([1, 2, 0]);
	});
});

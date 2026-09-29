import { afterEach, describe, expect, it, vi } from "vitest";
import type { NavigationFrameRef } from "../seriesNavigation";
import { playDisplayCine } from "./displayCine";
import { RenderedFrames } from "./renderedFrames.svelte";
import { DisplayFrameSource } from "./displayFrameSource";

const frames: NavigationFrameRef[] = [
	{ virtual_index: 0, file_index: 7, frame_index: 0 },
	{ virtual_index: 1, file_index: 9, frame_index: 0 },
	{ virtual_index: 2, file_index: 9, frame_index: 1 },
];

function fakeDisplay() {
	const fetched: string[] = [];
	return {
		fetched,
		display: {
			key: (file: number, frame: number) => `${file}:${frame}`,
			ensureFrame: vi.fn(async (file: number, frame: number) => {
				fetched.push(`${file}:${frame}`);
				return { blob: new Blob(["png"]), window: null };
			}),
			decode: vi.fn(),
		},
	};
}

afterEach(() => {
	vi.useRealTimers();
});

describe("playDisplayCine", () => {
	it("advances after returning to a cached tab and entering its playback scope", async () => {
		vi.useFakeTimers();
		const ctrl = new AbortController();
		const rendered = new RenderedFrames();
		let scope = "tab:a";
		const load = vi.fn(async () => ({ blob: new Blob(["png"]), window: null }));
		const display = new DisplayFrameSource({
			load,
			navigationScope: () => scope,
			concurrency: () => 1,
			onScopeChange: () => rendered.reset(),
		});
		await display.ensureFrame(7, 0, {});
		rendered.mark(7, 0);
		display.resetScope();
		scope = "tab:b";
		await display.ensureFrame(20, 0, {});
		rendered.mark(20, 0);
		display.resetScope();
		scope = "tab:a";
		await display.ensureFrame(7, 0, {});
		rendered.mark(7, 0);
		expect(load).toHaveBeenCalledTimes(2);

		// The viewport enters the fetch scope before starting cine. A cache
		// hit must have established it before the returning frame was drawn.
		display.enterScope({});
		const onstep = vi.fn(() => ctrl.abort());
		const playback = playDisplayCine({
			frames, startPosition: 0, direction: 1, mode: "loop", fps: 10,
			windowOptions: {}, display, rendered, signal: ctrl.signal, onstep,
		});
		await vi.advanceTimersByTimeAsync(150);
		ctrl.abort();
		await playback;
		expect(onstep).toHaveBeenCalledWith(1, 1);
		display.clear();
	});

	it("waits for the starting frame, then steps only as frames are presented", async () => {
		vi.useFakeTimers();
		const ctrl = new AbortController();
		const rendered = new RenderedFrames();
		const { display, fetched } = fakeDisplay();
		const steps: Array<[number, number]> = [];

		const playback = playDisplayCine({
			frames,
			startPosition: 0,
			direction: 1,
			mode: "loop",
			fps: 10,
			windowOptions: {},
			display,
			rendered,
			signal: ctrl.signal,
			onstep: (position, direction) => {
				steps.push([position, direction]);
				const frame = frames[position];
				// The viewport presents the frame some time after navigation.
				setTimeout(() => rendered.mark(frame.file_index, frame.frame_index), 30);
				if (steps.length === 3) ctrl.abort();
			},
		});

		await vi.advanceTimersByTimeAsync(500);
		expect(steps).toEqual([]);

		rendered.mark(7, 0);
		await vi.advanceTimersByTimeAsync(1000);
		await playback;

		expect(steps).toEqual([[1, 1], [2, 1], [0, 1]]);
		expect(fetched).toEqual(["9:0", "9:1", "7:0"]);
	});

	it("stops when a frame is never presented", async () => {
		vi.useFakeTimers();
		const ctrl = new AbortController();
		const rendered = new RenderedFrames();
		rendered.mark(7, 0);
		const onstep = vi.fn();

		const playback = playDisplayCine({
			frames,
			startPosition: 0,
			direction: 1,
			mode: "sweep",
			fps: 10,
			windowOptions: {},
			display: fakeDisplay().display,
			rendered,
			signal: ctrl.signal,
			onstep,
		});
		await vi.advanceTimersByTimeAsync(150);
		rendered.reset();
		await playback;

		expect(onstep).toHaveBeenCalledOnce();
	});
});

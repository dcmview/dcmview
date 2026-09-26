import { afterEach, describe, expect, it, vi } from "vitest";
import type { DisplayFrameWindowOptions } from "../../api";
import type { RawFrame } from "../../rawFrame";
import { navigationFramesForFile } from "../seriesNavigation";
import { DisplayFrameSource } from "./displayFrameSource";
import { RawFrameSource } from "./rawFrameSource";

function rawFrame(bitsAllocated = 8): RawFrame {
	return {
		buffer: new ArrayBuffer(4 * (bitsAllocated / 8)),
		metadata: {
			rows: 2,
			columns: 2,
			bitsAllocated,
			pixelRepresentation: 0,
			samplesPerPixel: 1,
			photometricInterpretation: "MONOCHROME2",
			rescaleSlope: 1,
			rescaleIntercept: 0,
			defaultWc: null,
			defaultWw: null,
			paddingLow: null,
			paddingHigh: null,
		},
	};
}

function abortable<Value>(signal: AbortSignal): Promise<Value> {
	return new Promise<Value>((_resolve, reject) => {
		signal.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
	});
}

async function flush(): Promise<void> {
	for (let index = 0; index < 10; index += 1) await Promise.resolve();
}

afterEach(() => {
	vi.useRealTimers();
});

describe("RawFrameSource", () => {
	it("shares one request per frame and caches only frames it is given", async () => {
		const frame = rawFrame();
		const load = vi.fn(async () => frame);
		const source = new RawFrameSource({ load, concurrency: () => 2 });

		const foreground = source.ensure(3, 1);
		expect(source.ensure(3, 1)).toBe(foreground);
		expect(source.inFlight(3, 1)).toBe(foreground);
		await foreground;
		expect(source.cached(3, 1)).toBeUndefined();

		source.store(3, 1, frame);
		await expect(source.ensure(3, 1)).resolves.toBe(frame);
		expect(load).toHaveBeenCalledOnce();
	});

	it("aborts in-flight requests and drops the cache when cleared", async () => {
		const signals: AbortSignal[] = [];
		const source = new RawFrameSource({
			load: (_file, _frame, signal) => {
				signals.push(signal);
				return abortable(signal);
			},
			concurrency: () => 2,
		});
		source.store(1, 0, rawFrame());
		const pending = source.ensure(1, 1);

		source.clear();

		await expect(pending).rejects.toMatchObject({ name: "AbortError" });
		expect(signals[0].aborted).toBe(true);
		expect(source.cached(1, 0)).toBeUndefined();
	});

	it("prefetches the ring around the position and caches only renderable frames", async () => {
		const load = vi.fn(async (_file: number, frame: number) => rawFrame(frame === 2 ? 32 : 8));
		const source = new RawFrameSource({ load, concurrency: () => 3 });
		const frames = navigationFramesForFile(5, 4);

		source.prefetch(frames, 0, 1);
		await flush();

		// The current frame is the foreground's job; the ring covers its neighbours.
		expect(load.mock.calls.map(([, frame]) => frame).sort()).toEqual([1, 2, 3]);
		expect(source.cached(5, 1)).toBeDefined();
		expect(source.cached(5, 2)).toBeUndefined();
	});

	it("abandons a replaced prefetch between batches", async () => {
		const requested: number[] = [];
		const source = new RawFrameSource({
			load: async (_file, frame) => {
				requested.push(frame);
				return rawFrame();
			},
			concurrency: () => 1,
		});
		const frames = navigationFramesForFile(0, 30);

		source.prefetch(frames, 0, 1);
		source.stopPrefetch();
		await flush();

		expect(requested).toEqual([1]);
	});
});

describe("DisplayFrameSource", () => {
	type Load = (file: number, frame: number, options: DisplayFrameWindowOptions, signal: AbortSignal) => Promise<Blob>;

	function displaySource(load: Load, scope = { value: "tab:a" }) {
		const onScopeChange = vi.fn();
		const source = new DisplayFrameSource({
			load: load as never,
			navigationScope: () => scope.value,
			concurrency: () => 2,
			onScopeChange,
		});
		return { source, onScopeChange, scope };
	}

	it("shares a request per frame and serves the cached payload afterwards", async () => {
		const load = vi.fn(async () => new Blob(["png"]));
		const { source } = displaySource(load);

		const first = source.ensureBlob(1, 0, {});
		expect(source.ensureBlob(1, 0, {})).toBe(first);
		const blob = await first;
		await expect(source.ensureBlob(1, 0, {})).resolves.toBe(blob);
		expect(load).toHaveBeenCalledOnce();
	});

	it("keeps work alive while navigating within a scope", () => {
		const signals: AbortSignal[] = [];
		const { source, onScopeChange } = displaySource((_file, _frame, _options, signal) => {
			signals.push(signal);
			return abortable(signal);
		});

		void source.ensureBlob(1, 0, {}).catch(() => {});
		void source.ensureBlob(1, 1, {}).catch(() => {});

		expect(signals.map((signal) => signal.aborted)).toEqual([false, false]);
		expect(onScopeChange).toHaveBeenCalledOnce();
	});

	it("aborts the previous scope when the window or tab changes", () => {
		const signals: AbortSignal[] = [];
		const { source, onScopeChange, scope } = displaySource((_file, _frame, _options, signal) => {
			signals.push(signal);
			return abortable(signal);
		});

		void source.ensureBlob(1, 0, {}).catch(() => {});
		void source.ensureBlob(1, 0, { wc: 40, ww: 400, windowMode: "default" }).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([true, false]);

		scope.value = "tab:b";
		void source.ensureBlob(1, 1, { wc: 40, ww: 400, windowMode: "default" }).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([true, true, false]);
		expect(onScopeChange).toHaveBeenCalledTimes(3);
	});

	it("keeps one scope for a real-world window across frames with their own conversion", () => {
		const signals: AbortSignal[] = [];
		const load = vi.fn((_file: number, _frame: number, _options: DisplayFrameWindowOptions, signal: AbortSignal) => {
			signals.push(signal);
			return abortable<Blob>(signal);
		});
		const { source, onScopeChange } = displaySource(load);
		const gray = { wc: 12, ww: 20, windowMode: "default" as const, unit: "Gy" };

		void source.ensureBlob(1, 0, gray).catch(() => {});
		void source.ensureBlob(1, 1, gray).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([false, false]);
		// The loader converts per frame, so it receives the unit.
		expect(load.mock.calls[1].slice(0, 3)).toEqual([1, 1, gray]);
		expect(source.key(1, 1, gray)).not.toBe(source.key(1, 1, { wc: 12, ww: 20, windowMode: "default" }));

		void source.ensureBlob(1, 2, { ...gray, wc: 13 }).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([true, true, false]);
		expect(onScopeChange).toHaveBeenCalledTimes(2);
	});

	it("aborts overlay fetches with their scope", () => {
		const { source } = displaySource(async () => new Blob(["png"]));
		let maskSignal!: AbortSignal;
		void source.fetchInScope("seg:4:0", {}, (signal) => {
			maskSignal = signal;
			return abortable(signal);
		}).catch(() => {});

		source.resetScope();
		expect(maskSignal.aborted).toBe(true);
	});

	it("defers idle prefetch and keeps a nearby prefetch running", async () => {
		vi.useFakeTimers();
		const requested: number[] = [];
		const { source } = displaySource(async (_file, frame) => {
			requested.push(frame);
			return new Blob(["png"]);
		});
		const frames = navigationFramesForFile(2, 8);
		source.enterScope({});

		source.startPrefetch(frames, 0, 1, {}, 3, null);
		expect(requested).toEqual([]);
		source.startPrefetch(frames, 3, 1, {}, 3, null);
		await vi.advanceTimersByTimeAsync(1);

		expect(requested.slice(0, 2)).toEqual([1, 2]);
	});

	it("starts cine prefetch without waiting for idle time", () => {
		vi.useFakeTimers();
		const requested: number[] = [];
		const { source } = displaySource(async (_file, frame) => {
			requested.push(frame);
			return new Blob(["png"]);
		});
		const frames = navigationFramesForFile(2, 4);
		source.enterScope({});

		source.startPrefetch(frames, 3, 1, {}, 3, "loop");

		expect(requested).toHaveLength(2);
		expect(requested).not.toContain(3);
	});

	it("drops cached payloads and resets rendered state when cleared", async () => {
		const load = vi.fn(async () => new Blob(["png"]));
		const { source, onScopeChange } = displaySource(load);
		await source.ensureBlob(1, 0, {});
		onScopeChange.mockClear();

		source.clear();
		await source.ensureBlob(1, 0, {});

		expect(load).toHaveBeenCalledTimes(2);
		expect(onScopeChange).toHaveBeenCalledTimes(2);
	});
});

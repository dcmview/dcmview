import { afterEach, describe, expect, it, vi } from "vitest";
import type { DisplayFrame, DisplayFrameWindowOptions } from "../../api";
import type { RawFrame } from "../../rawFrame";
import { navigationFramesForFile } from "../seriesNavigation";
import {
	DISPLAY_BITMAP_CACHE_BYTE_BUDGET,
	DISPLAY_BLOB_CACHE_BYTE_BUDGET,
	DisplayFrameSource,
	FULL_STACK_PREFETCH_DWELL_MS,
} from "./displayFrameSource";
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
	vi.unstubAllGlobals();
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
		const load = vi.fn(async (_file: number, frame: number) => rawFrame(frame === 2 ? 12 : 8));
		const source = new RawFrameSource({ load, concurrency: () => 3 });
		const frames = navigationFramesForFile(5, 4);

		source.prefetch(frames, 0, 1);
		await flush();

		// The current frame is the foreground's job; the ring covers its neighbours.
		expect(load.mock.calls.map(([, frame]) => frame).sort()).toEqual([1, 2, 3]);
		expect(source.cached(5, 1)).toBeDefined();
		expect(source.cached(5, 2)).toBeUndefined();
	});

	it("prepares the raw ring's mappings even when its samples are cached", async () => {
		const load = vi.fn(async () => rawFrame());
		const prepare = vi.fn(async (_file: number, _frame: number, _signal: AbortSignal) => {});
		const source = new RawFrameSource({ load, prepare, concurrency: () => 3 });
		for (let frame = 0; frame < 4; frame++) source.store(5, frame, rawFrame());
		source.prefetch(navigationFramesForFile(5, 4), 0, 1);
		await flush();
		expect(prepare.mock.calls.map(([, frame]) => frame).sort()).toEqual([1, 2, 3]);
		expect(load).not.toHaveBeenCalled();
	});

	it("cancels mapping preparation with its abandoned raw request", async () => {
		let signal: AbortSignal | undefined;
		const source = new RawFrameSource({ load: async () => rawFrame(), concurrency: () => 1,
			prepare: (_file, _frame, nextSignal) => { signal = nextSignal; return abortable(nextSignal); } });
		const pending = source.ensure(1, 0);
		source.abortAll();
		await expect(pending).rejects.toMatchObject({ name: "AbortError" });
		expect(signal?.aborted).toBe(true);
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
	type Load = (file: number, frame: number, options: DisplayFrameWindowOptions, signal: AbortSignal) => Promise<DisplayFrame>;

	function png(blob: Blob = new Blob(["png"])): DisplayFrame {
		return { blob, window: null, appliedWindow: null };
	}

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

	it("shares a request per frame and serves the cached payload and its window afterwards", async () => {
		const load = vi.fn(async () => ({ blob: new Blob(["png"]), window: { wc: 40, ww: 400 }, appliedWindow: "linear" as const }));
		const { source } = displaySource(load);

		const first = source.ensureFrame(1, 0, {});
		expect(source.ensureFrame(1, 0, {})).toBe(first);
		const frame = await first;
		await expect(source.ensureFrame(1, 0, {})).resolves.toEqual({ blob: frame.blob, window: { wc: 40, ww: 400 }, appliedWindow: "linear" });
		expect(load).toHaveBeenCalledOnce();
	});

	it("keeps work alive while navigating within a scope", () => {
		const signals: AbortSignal[] = [];
		const { source, onScopeChange } = displaySource((_file, _frame, _options, signal) => {
			signals.push(signal);
			return abortable(signal);
		});

		void source.ensureFrame(1, 0, {}).catch(() => {});
		void source.ensureFrame(1, 1, {}).catch(() => {});

		expect(signals.map((signal) => signal.aborted)).toEqual([false, false]);
		expect(onScopeChange).toHaveBeenCalledOnce();
	});

	it("aborts the previous scope when the window or tab changes", () => {
		const signals: AbortSignal[] = [];
		const { source, onScopeChange, scope } = displaySource((_file, _frame, _options, signal) => {
			signals.push(signal);
			return abortable(signal);
		});

		void source.ensureFrame(1, 0, {}).catch(() => {});
		void source.ensureFrame(1, 0, { wc: 40, ww: 400, windowMode: "default" }).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([true, false]);

		scope.value = "tab:b";
		void source.ensureFrame(1, 1, { wc: 40, ww: 400, windowMode: "default" }).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([true, true, false]);
		expect(onScopeChange).toHaveBeenCalledTimes(3);
	});

	it("keeps one scope for a real-world window across frames with their own conversion", () => {
		const signals: AbortSignal[] = [];
		const load = vi.fn((_file: number, _frame: number, _options: DisplayFrameWindowOptions, signal: AbortSignal) => {
			signals.push(signal);
			return abortable<DisplayFrame>(signal);
		});
		const { source, onScopeChange } = displaySource(load);
		const gray = { wc: 12, ww: 20, windowMode: "default" as const, unit: "Gy" };

		void source.ensureFrame(1, 0, gray).catch(() => {});
		void source.ensureFrame(1, 1, gray).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([false, false]);
		// The loader converts per frame, so it receives the unit.
		expect(load.mock.calls[1].slice(0, 3)).toEqual([1, 1, gray]);
		expect(source.key(1, 1, gray)).not.toBe(source.key(1, 1, { wc: 12, ww: 20, windowMode: "default" }));

		void source.ensureFrame(1, 2, { ...gray, wc: 13 }).catch(() => {});
		expect(signals.map((signal) => signal.aborted)).toEqual([true, true, false]);
		expect(onScopeChange).toHaveBeenCalledTimes(2);
	});

	it("aborts overlay fetches with their scope", () => {
		const { source } = displaySource(async () => png());
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
			return png();
		});
		const frames = navigationFramesForFile(2, 8);
		source.enterScope({});

		source.startPrefetch(frames, 0, 1, {}, 3, null);
		expect(requested).toEqual([]);
		source.startPrefetch(frames, 3, 1, {}, 3, null);
		await vi.advanceTimersByTimeAsync(1);

		expect(requested.slice(0, 2)).toEqual([1, 2]);
	});

	it("prefetches the neighbourhood first and the whole stack after the dwell", async () => {
		vi.useFakeTimers();
		const requested = new Set<number>();
		const { source } = displaySource(async (_file, frame) => {
			requested.add(frame);
			return png();
		});
		const frames = navigationFramesForFile(2, 200);
		source.enterScope({});

		source.startPrefetch(frames, 0, 1, {}, 3, null);
		await vi.advanceTimersByTimeAsync(1);
		expect(Math.max(...requested)).toBeLessThanOrEqual(48);

		await vi.advanceTimersByTimeAsync(FULL_STACK_PREFETCH_DWELL_MS);
		expect(requested.size).toBe(199);
	});

	it("aborts frame requests far from the new position but not overlay fetches", () => {
		const signals = new Map<number, AbortSignal>();
		const { source } = displaySource((_file, frame, _options, signal) => {
			signals.set(frame, signal);
			return abortable<DisplayFrame>(signal);
		});
		const frames = navigationFramesForFile(2, 200);
		void source.ensureFrame(2, 0, {}).catch(() => {});
		void source.ensureFrame(2, 150, {}).catch(() => {});
		let overlaySignal: AbortSignal | undefined;
		void source.fetchInScope("overlay", {}, (signal) => {
			overlaySignal = signal;
			return abortable<Blob>(signal);
		}).catch(() => {});

		source.abortFar(frames, 150);

		expect(signals.get(0)?.aborted).toBe(true);
		expect(signals.get(150)?.aborted).toBe(false);
		expect(overlaySignal?.aborted).toBe(false);
	});

	it("starts cine prefetch without waiting for idle time", () => {
		vi.useFakeTimers();
		const requested: number[] = [];
		const { source } = displaySource(async (_file, frame) => {
			requested.push(frame);
			return png();
		});
		const frames = navigationFramesForFile(2, 4);
		source.enterScope({});

		source.startPrefetch(frames, 3, 1, {}, 3, "loop");

		expect(requested).toHaveLength(2);
		expect(requested).not.toContain(3);
	});

	it("drops cached payloads and resets rendered state when cleared", async () => {
		const load = vi.fn(async () => png());
		const { source, onScopeChange } = displaySource(load);
		await source.ensureFrame(1, 0, {});
		onScopeChange.mockClear();

		source.clear();
		await source.ensureFrame(1, 0, {});

		expect(load).toHaveBeenCalledTimes(2);
		expect(onScopeChange).toHaveBeenCalledTimes(2);
	});

	it("returns frames over a tier's budget uncached instead of failing them", async () => {
		// An 8192 x 8192 frame decodes to 256 MiB of RGBA, twice the bitmap budget.
		const side = 8192;
		expect(side * side * 4).toBeGreaterThan(DISPLAY_BITMAP_CACHE_BYTE_BUDGET);
		const payload = png({ size: DISPLAY_BLOB_CACHE_BYTE_BUDGET + 1 } as Blob);
		const load = vi.fn(async () => payload);
		const decoded: Array<{ width: number; height: number; close: ReturnType<typeof vi.fn> }> = [];
		vi.stubGlobal("createImageBitmap", vi.fn(async () => {
			const bitmap = { width: side, height: side, close: vi.fn() };
			decoded.push(bitmap);
			return bitmap;
		}));
		const { source } = displaySource(load);

		const { blob } = await source.ensureFrame(1, 0, {});
		expect(blob).toBe(payload.blob);
		await source.ensureFrame(1, 0, {});
		expect(load).toHaveBeenCalledTimes(2);

		// Consumers of one decode share its bitmap until the last releases it.
		const key = source.key(1, 0, {});
		const [shown, prepared] = await Promise.all([source.decode(key, blob), source.decode(key, blob)]);
		expect(shown.bitmap).toBe(decoded[0]);
		expect(prepared.bitmap).toBe(decoded[0]);
		prepared.release();
		expect(decoded[0].close).not.toHaveBeenCalled();
		shown.release();
		shown.release();
		expect(decoded[0].close).toHaveBeenCalledOnce();

		const again = await source.decode(key, blob);
		expect(again.bitmap).toBe(decoded[1]);
		again.release();
	});

	it("keeps caching frames that fit and leaves cached bitmaps to the tier", async () => {
		const decode = vi.fn(async () => ({ width: 2, height: 2, close: vi.fn() }));
		vi.stubGlobal("createImageBitmap", decode);
		const { source } = displaySource(async () => png());
		const key = source.key(1, 0, {});

		const { blob } = await source.ensureFrame(1, 0, {});
		const first = await source.decode(key, blob);
		first.release();
		const second = await source.decode(key, blob);

		expect(second.bitmap).toBe(first.bitmap);
		expect(decode).toHaveBeenCalledOnce();
		expect(vi.mocked(first.bitmap.close)).not.toHaveBeenCalled();
	});
});

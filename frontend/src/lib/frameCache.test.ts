import { describe, expect, it, vi } from "vitest";
import {
	ByteBudgetLruCache,
	createDisplayFrameCaches,
	decodedBitmapBytes,
	type BitmapResource,
} from "./frameCache";

function displayFrame(blob: Blob) {
	return { blob, window: null };
}

function bitmap(width: number, height: number): BitmapResource {
	return { width, height, close: vi.fn() };
}

describe("ByteBudgetLruCache", () => {
	it("retains visited frames from different source files within one budget", () => {
		const cache = new ByteBudgetLruCache<string, { bytes: number }>({
			maxBytes: 12,
			sizeOf: (entry) => entry.bytes,
		});
		cache.set("7:0", { bytes: 4 });
		cache.set("9:0", { bytes: 4 });
		cache.set("9:1", { bytes: 4 });

		expect([...cache.keys()]).toEqual(["7:0", "9:0", "9:1"]);
		expect(cache.get("7:0")).toEqual({ bytes: 4 });
		expect([...cache.keys()]).toEqual(["9:0", "9:1", "7:0"]);
	});

	it("evicts the least-recently-used entry within its byte budget", () => {
		const disposed: string[] = [];
		const cache = new ByteBudgetLruCache<string, { id: string; bytes: number }>({
			maxBytes: 10,
			sizeOf: (entry) => entry.bytes,
			dispose: (entry) => disposed.push(entry.id),
		});
		cache.set("a", { id: "a", bytes: 4 });
		cache.set("b", { id: "b", bytes: 4 });
		cache.get("a");

		expect(cache.set("c", { id: "c", bytes: 4 })).toBe(true);
		expect([...cache.keys()]).toEqual(["a", "c"]);
		expect(disposed).toEqual(["b"]);
		expect(cache.bytes).toBe(8);
	});

	it("rejects oversized entries without replacing a cached value or disposing either", () => {
		const dispose = vi.fn();
		const cache = new ByteBudgetLruCache<string, { bytes: number }>({
			maxBytes: 4,
			sizeOf: (entry) => entry.bytes,
			dispose,
		});
		const retained = { bytes: 4 };
		cache.set("frame", retained);
		const oversized = { bytes: 5 };

		expect(cache.set("frame", oversized)).toBe(false);
		expect(cache.peek("frame")).toBe(retained);
		expect(dispose).not.toHaveBeenCalled();
	});
});

describe("display frame cache accounting", () => {
	it("accounts retained PNGs and decoded bitmaps under independent budgets", () => {
		const decoded = bitmap(4, 3);
		expect(decodedBitmapBytes(decoded)).toBe(48);

		const cache = createDisplayFrameCaches<BitmapResource>(10, 48);
		expect(cache.frames.set("one", displayFrame(new Blob([new Uint8Array(10)])))).toBe(true);
		expect(cache.bitmaps.set("one", decoded)).toBe(true);
		expect(cache.frames.bytes).toBe(10);
		expect(cache.bitmaps.bytes).toBe(48);
	});

	it("retains PNG payloads when bitmap pressure evicts decoded resources", () => {
		const first = bitmap(4, 2);
		const second = bitmap(4, 2);
		const cache = createDisplayFrameCaches<BitmapResource>(20, 32);

		cache.frames.set("first", displayFrame(new Blob([new Uint8Array(10)])));
		cache.frames.set("second", displayFrame(new Blob([new Uint8Array(10)])));
		cache.bitmaps.set("first", first);
		cache.bitmaps.set("second", second);

		expect(cache.frames.has("first")).toBe(true);
		expect(cache.frames.has("second")).toBe(true);
		expect(cache.bitmaps.has("first")).toBe(false);
		expect(first.close).toHaveBeenCalledOnce();
		expect(cache.bitmaps.has("second")).toBe(true);
		expect(second.close).not.toHaveBeenCalled();
	});

	it("closes every retained bitmap when cleared after a stress fill", () => {
		const frames = Array.from({ length: 200 }, () => bitmap(2, 2));
		const cache = createDisplayFrameCaches<BitmapResource>(4 * 200, 16 * 25);

		for (let index = 0; index < frames.length; index += 1) {
			cache.frames.set(String(index), displayFrame(new Blob([new Uint8Array(4)])));
			cache.bitmaps.set(String(index), frames[index]);
			expect(cache.bitmaps.bytes).toBeLessThanOrEqual(cache.bitmaps.maxBytes);
		}
		cache.bitmaps.clear();

		expect(cache.frames.size).toBe(200);
		expect(cache.bitmaps.bytes).toBe(0);
		expect(cache.bitmaps.size).toBe(0);
		expect(frames.every((frame) => vi.mocked(frame.close).mock.calls.length === 1)).toBe(true);
	});
});

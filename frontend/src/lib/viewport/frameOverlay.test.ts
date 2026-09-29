import { describe, expect, it, vi } from "vitest";
import * as api from "../../api";
import type { FileSummary, OverlayLegend } from "../../api";
import {
	composeOverlayFrame,
	drawOverlayLayer,
	legendColors,
	OverlayLayerCache,
	overlayLayerRequests,
	valueOverlayLayerRequest,
	type DecodedCanvasImage,
	type SegmentationOverlay,
} from "./frameOverlay";

vi.mock("../../api", async (importOriginal) => ({
	...await importOriginal<typeof import("../../api")>(),
	fetchDoseOverlayBlob: vi.fn(async () => new Blob(["dose"])),
	fetchParametricMapOverlayBlob: vi.fn(async () => new Blob(["map"])),
}));

function image(name: string, width: number, height: number): DecodedCanvasImage {
	return { source: { name } as unknown as CanvasImageSource, width, height, dispose: vi.fn() };
}

describe("frame overlays", () => {
	it("requests one mask layer per SEG frame, keyed for sharing", () => {
		const overlay: SegmentationOverlay = {
			kind: "segmentation",
			segmentationFileIndex: 4,
			segmentationFrameIndex: 2,
			sourceFileIndex: 1,
			sourceFrameIndex: 0,
			sourceFile: {} as FileSummary,
		};
		expect(overlayLayerRequests(overlay).map((layer) => layer.key)).toEqual(["seg:4:2"]);
	});

	it("draws the base at its size and stretches each layer over it in order", () => {
		const drawImage = vi.fn();
		const canvas = { width: 0, height: 0, getContext: () => ({ drawImage }) } as unknown as HTMLCanvasElement;
		const base = image("base", 64, 32);
		const mask = image("mask", 16, 8);

		composeOverlayFrame(canvas, base, [mask]);

		expect([canvas.width, canvas.height]).toEqual([64, 32]);
		expect(drawImage.mock.calls).toEqual([
			[base.source, 0, 0],
			[mask.source, 0, 0, 64, 32],
		]);
	});

	it("fails loudly when the canvas has no 2D context", () => {
		const canvas = { width: 0, height: 0, getContext: () => null } as unknown as HTMLCanvasElement;
		expect(() => composeOverlayFrame(canvas, image("base", 1, 1), [])).toThrow("2D canvas is unavailable");
	});
});

describe("value overlays", () => {
	it("caches noncoverage but retries transient failures", async () => {
		const cache = new OverlayLayerCache();
		const missing = new api.ApiError("outside", 404, "overlay_not_covering_frame");
		const load = vi.fn().mockRejectedValueOnce(missing).mockResolvedValue(new Blob(["png"]));
		for (let pass = 0; pass < 3; pass++) await expect(cache.load({ key: "missing", load })).rejects.toBe(missing);
		expect(load).toHaveBeenCalledOnce();
		cache.clear();
		await expect(cache.load({ key: "missing", load })).resolves.toBeInstanceOf(Blob);
		const failure = vi.fn().mockRejectedValueOnce(new api.ApiError("retry", 500, null)).mockResolvedValue(new Blob());
		await expect(cache.load({ key: "retry", load: failure })).rejects.toMatchObject({ status: 500 });
		await expect(cache.load({ key: "retry", load: failure })).resolves.toBeInstanceOf(Blob);
	});

	it("cancels abandoned prefetch without cancelling a foreground consumer", async () => {
		const cache = new OverlayLayerCache();
		const signals: AbortSignal[] = [];
		const load = vi.fn((signal: AbortSignal) => {
			signals.push(signal);
			return new Promise<Blob>((_resolve, reject) => signal.addEventListener("abort", () => reject(new DOMException("cancelled", "AbortError"))));
		});
		const prefetch = new AbortController();
		const foreground = new AbortController();
		const first = cache.load({ key: "shared", load }, prefetch.signal);
		const second = cache.load({ key: "shared", load }, foreground.signal);
		prefetch.abort();
		await expect(first).rejects.toMatchObject({ name: "AbortError" });
		expect(signals[0].aborted).toBe(false);
		foreground.abort();
		await expect(second).rejects.toMatchObject({ name: "AbortError" });
		expect(signals[0].aborted).toBe(true);
		const next = new AbortController();
		const third = cache.load({ key: "shared", load }, next.signal);
		expect(load).toHaveBeenCalledTimes(2);
		next.abort();
		await expect(third).rejects.toMatchObject({ name: "AbortError" });
	});

	it("keys one colorwash layer per volume and displayed frame, from the volume's endpoint", async () => {
		const signal = new AbortController().signal;
		const dose = valueOverlayLayerRequest({ kind: "rt_dose", volumeFileIndex: 9 }, 5, 2);
		const map = valueOverlayLayerRequest({ kind: "parametric_map", volumeFileIndex: 4 }, 1, 0);
		expect([dose.key, map.key]).toEqual(["rt_dose:9:5:2", "parametric_map:4:1:0"]);

		await dose.load(signal);
		await map.load(signal);
		expect(api.fetchDoseOverlayBlob).toHaveBeenCalledWith(5, 2, 9, signal);
		expect(api.fetchParametricMapOverlayBlob).toHaveBeenCalledWith(1, 0, 4, signal);
	});

	it("shares in-flight layer requests and serves revisited layers from cache", async () => {
		const cache = new OverlayLayerCache();
		const blob = new Blob(["png"], { type: "image/png" });
		const load = vi.fn(async () => blob);

		const [first, second] = await Promise.all([
			cache.load({ key: "rt_dose:9:5:0", load }),
			cache.load({ key: "rt_dose:9:5:0", load }),
		]);
		expect(await cache.load({ key: "rt_dose:9:5:0", load })).toBe(blob);
		expect([first, second]).toEqual([blob, blob]);
		expect(load).toHaveBeenCalledOnce();

		cache.clear();
		await cache.load({ key: "rt_dose:9:5:0", load });
		expect(load).toHaveBeenCalledTimes(2);
	});

	it("aborts layers of frames the viewer moved past", async () => {
		const cache = new OverlayLayerCache();
		let signal: AbortSignal | undefined;
		const pending = cache.load({
			key: "rt_dose:9:5:0",
			load: (abort) => {
				signal = abort;
				return new Promise<Blob>(() => {});
			},
		});
		void pending;
		cache.abortOthers("rt_dose:9:5:1");
		expect(signal?.aborted).toBe(true);
	});

	it("draws a layer at its own size on a cleared transparent canvas", () => {
		const clearRect = vi.fn();
		const drawImage = vi.fn();
		const canvas = { width: 0, height: 0, getContext: () => ({ clearRect, drawImage }) } as unknown as HTMLCanvasElement;
		const layer = image("dose", 10, 12);

		drawOverlayLayer(canvas, layer);

		expect([canvas.width, canvas.height]).toEqual([10, 12]);
		expect(clearRect).toHaveBeenCalledWith(0, 0, 10, 12);
		expect(drawImage).toHaveBeenCalledWith(layer.source, 0, 0);
	});

	it("turns legend color stops into CSS colors, lowest value first", () => {
		const legend = { color_stops: [[68, 1, 84], [253, 231, 37]] } as OverlayLegend;
		expect(legendColors(legend)).toEqual(["rgb(68, 1, 84)", "rgb(253, 231, 37)"]);
	});
});

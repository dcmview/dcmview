import { describe, expect, it, vi } from "vitest";
import { ApiError, type FrameValueMapping, type RawFrame } from "../../api";
import { fileSummary, rawFrame } from "../../testing/fixtures";
import { overlayValueReadout, PixelProbe, pixelReadout, type PixelReadoutInput } from "./pixelProbe.svelte";
import { RawFrameSource } from "./rawFrameSource";

const CT = fileSummary(4, { modality: "CT" });

function ctMapping(): FrameValueMapping {
	return {
		file_index: CT.index,
		frame_index: 0,
		stored_value_type: "integer",
		modality: { rescale_slope: 1, rescale_intercept: -1024, rescale_type: null, lut: null },
		real_world: [],
	};
}

function input(overrides: Partial<PixelReadoutInput> = {}): PixelReadoutInput {
	return {
		pixel: { row: 1, column: 2 },
		file: CT,
		frameIndex: 0,
		samples: { status: "ready", frame: rawFrame(4, 4, 16) },
		mapping: ctMapping(),
		mappingFailed: false,
		planarConfiguration: () => 0,
		paused: false,
		...overrides,
	};
}

describe("pixelReadout", () => {
	it("combines samples and the value mapping", () => {
		expect(pixelReadout(input())).toEqual({
			pixel: { row: 1, column: 2 },
			frameNumber: 1,
			values: {
				kind: "grayscale",
				stored: "0",
				modality: { value: "-1024", unit: "HU" },
				mapped: null,
				mappedOutOfRange: false,
				mappingSource: null,
			},
			note: null,
		});
	});

	it("keeps coordinates and explains missing values", () => {
		expect(pixelReadout(input({ paused: true }))).toMatchObject({ values: null, note: "values paused during playback" });
		expect(pixelReadout(input({ samples: { status: "unavailable", reason: "422" } })))
			.toMatchObject({ pixel: { row: 1, column: 2 }, values: null, note: "value unavailable (display only)" });
		expect(pixelReadout(input({ samples: { status: "loading" } }))).toMatchObject({ values: null, note: "reading…" });
		expect(pixelReadout(input({ mapping: null }))).toMatchObject({ values: null, note: "reading…" });
		expect(pixelReadout(input({ planarConfiguration: () => undefined }))).toMatchObject({ values: null });
	});

	it("falls back to raw-header rescale when the value mapping fails", () => {
		const readout = pixelReadout(input({ mapping: null, mappingFailed: true }));
		expect(readout.note).toBe("value mapping unavailable");
		expect(readout.values).toMatchObject({ stored: "0", modality: { value: "0", unit: "HU" } });
	});
});

describe("PixelProbe", () => {
	function source(load: (file: number, frame: number) => Promise<RawFrame>) {
		return new RawFrameSource({ load: (file, frame) => load(file, frame), concurrency: () => 1 });
	}

	it("uses the frame the renderer already holds without fetching", () => {
		const load = vi.fn();
		const probe = new PixelProbe(source(load));
		const displayed = rawFrame();

		expect(probe.track(CT, 0, displayed)).toBeUndefined();
		expect(probe.samples(CT.index, 0)).toEqual({ status: "ready", frame: displayed });
		expect(probe.samples(CT.index, 1)).toBeNull();
		expect(load).not.toHaveBeenCalled();
	});

	it("fetches once the cursor settles and marks files the raw endpoint rejects", async () => {
		vi.useFakeTimers();
		try {
			const load = vi.fn(async () => { throw new ApiError("unsupported layout", 422, "unsupported_pixel_layout"); });
			const probe = new PixelProbe(source(load));

			probe.track(CT, 0, null);
			expect(probe.samples(CT.index, 0)).toEqual({ status: "loading" });
			await vi.advanceTimersByTimeAsync(200);

			expect(load).toHaveBeenCalledOnce();
			expect(probe.samples(CT.index, 3)).toEqual({ status: "unavailable", reason: "unsupported layout" });
		} finally {
			vi.useRealTimers();
		}
	});

	it("reads a large frame one pixel at a time and answers only for that pixel", async () => {
		vi.useFakeTimers();
		try {
			const large = fileSummary(7, { modality: "CR", rows: 4096, columns: 4096 });
			const loadFrame = vi.fn(async () => rawFrame());
			const onePixel = { ...rawFrame(1, 1, 16) };
			const loadPixel = vi.fn(async () => onePixel);
			const probe = new PixelProbe(source(loadFrame), vi.fn(), loadPixel);
			const pixel = { row: 3000, column: 12 };

			probe.track(large, 0, null, { pixel });
			await vi.advanceTimersByTimeAsync(200);

			expect(loadFrame).not.toHaveBeenCalled();
			expect(loadPixel).toHaveBeenCalledWith(large.index, 0, pixel, expect.any(AbortSignal));
			const samples = probe.samples(large.index, 0);
			expect(samples).toEqual({ status: "ready", frame: onePixel, at: pixel });
			const readout = (at: typeof pixel) => pixelReadout(input({ file: large, pixel: at, samples }));
			expect(readout(pixel).values).toMatchObject({ kind: "grayscale", stored: "0" });
			expect(readout({ row: 0, column: 0 })).toMatchObject({ values: null, note: "reading…" });
		} finally {
			vi.useRealTimers();
		}
	});

	it("asks native color files for their planar configuration once", async () => {
		const loadTag = vi.fn(async () => ({ tag: "(0028,0006)", vr: "US", keyword: "PlanarConfiguration", value: { type: "number" as const, value: 1 } }));
		const probe = new PixelProbe(source(vi.fn()), loadTag);
		const color: RawFrame = { ...rawFrame(), metadata: { ...rawFrame().metadata, samplesPerPixel: 3 } };

		probe.track(CT, 0, color);
		expect(probe.planarConfiguration(CT, color)).toBeUndefined();
		await vi.waitFor(() => expect(probe.planarConfiguration(CT, color)).toBe(1));
		probe.track(CT, 1, color);
		expect(loadTag).toHaveBeenCalledOnce();
		expect(probe.planarConfiguration(CT, rawFrame())).toBe(0);
	});
});

describe("overlayValueReadout", () => {
	const pixel = { row: 1, column: 2 };

	it("reads the overlaid volume's value at the pixel, row-major", () => {
		const values = new Float32Array(12).fill(Number.NaN);
		values[1 * 4 + 2] = 16.2;
		expect(overlayValueReadout("dose", "Gy", pixel, 4, { status: "ready", values }))
			.toEqual({ label: "dose", unit: "Gy", value: "16.2", note: null });
	});

	it("says when the pixel or frame lies outside the volume", () => {
		const values = new Float32Array(12).fill(Number.NaN);
		expect(overlayValueReadout("dose", "Gy", pixel, 4, { status: "ready", values }))
			.toMatchObject({ value: null, note: "outside the volume" });
		expect(overlayValueReadout("dose", "Gy", pixel, 4, { status: "not_covering" }))
			.toMatchObject({ value: null, note: "outside the volume" });
		expect(overlayValueReadout("map", "um2/s", pixel, 4, { status: "loading" }))
			.toMatchObject({ value: null, note: "reading…" });
		expect(overlayValueReadout("map", "um2/s", pixel, 4, { status: "unavailable" }))
			.toMatchObject({ value: null, note: "unavailable" });
	});
});

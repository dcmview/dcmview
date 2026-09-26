import { describe, expect, it, vi } from "vitest";
import { ApiError, type FrameValueMapping, type RawFrame } from "../../api";
import { fileSummary, rawFrame } from "../../testing/fixtures";
import { PixelProbe, pixelReadout, type PixelReadoutInput } from "./pixelProbe.svelte";
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

import { describe, expect, it } from "vitest";
import type { RawFrame, RawFrameMetadata } from "../rawFrame";
import {
	computeFullDynamicWindow,
	computePercentileWindow,
	renderRawFrameToRgba,
	resolveDisplayWindow,
	selectWindowingPipeline,
	validateRenderableRawFrame,
} from "./rawWindowing";

type MetadataOverrides = Partial<Omit<RawFrameMetadata, "rows" | "columns">>;

function frameFromSamples(
	samples: number[],
	bitsAllocated: 8 | 16,
	pixelRepresentation: 0 | 1,
	overrides: MetadataOverrides = {},
): RawFrame {
	const bytesPerSample = bitsAllocated / 8;
	const buffer = new ArrayBuffer(samples.length * bytesPerSample);
	const view = new DataView(buffer);
	for (let index = 0; index < samples.length; index += 1) {
		if (bitsAllocated === 8 && pixelRepresentation === 1) {
			view.setInt8(index, samples[index]);
		} else if (bitsAllocated === 8) {
			view.setUint8(index, samples[index]);
		} else if (pixelRepresentation === 1) {
			view.setInt16(index * 2, samples[index], true);
		} else {
			view.setUint16(index * 2, samples[index], true);
		}
	}

	return {
		buffer,
		metadata: {
			rows: 1,
			columns: samples.length,
			bitsAllocated,
			pixelRepresentation,
			samplesPerPixel: 1,
			photometricInterpretation: "MONOCHROME2",
			rescaleSlope: 1,
			rescaleIntercept: 0,
			defaultWc: null,
			defaultWw: null,
			paddingLow: null,
			paddingHigh: null,
			...overrides,
		},
	};
}

function grayValues(rgba: Uint8ClampedArray): number[] {
	const values: number[] = [];
	for (let offset = 0; offset < rgba.length; offset += 4) {
		expect(Array.from(rgba.slice(offset, offset + 4))).toEqual([
			rgba[offset],
			rgba[offset],
			rgba[offset],
			255,
		]);
		values.push(rgba[offset]);
	}
	return values;
}

describe("renderRawFrameToRgba", () => {
	it.each([
		{
			label: "unsigned 8-bit",
			frame: frameFromSamples([0, 128, 255], 8, 0),
			wc: 127.5,
			ww: 256,
			expected: [0, 129, 255],
		},
		{
			label: "signed 8-bit",
			frame: frameFromSamples([-128, 0, 127], 8, 1),
			wc: -0.5,
			ww: 256,
			expected: [0, 129, 255],
		},
		{
			label: "unsigned 16-bit little-endian",
			frame: frameFromSamples([0, 32768, 65535], 16, 0),
			wc: 32767.5,
			ww: 65536,
			expected: [0, 128, 255],
		},
		{
			label: "signed 16-bit little-endian",
			frame: frameFromSamples([-32768, 0, 32767], 16, 1),
			wc: -0.5,
			ww: 65536,
			expected: [0, 128, 255],
		},
	])("windows $label samples across the grayscale range", ({ frame, wc, ww, expected }) => {
		expect(grayValues(renderRawFrameToRgba(frame, wc, ww))).toEqual(expected);
	});

	it("applies rescale metadata before windowing", () => {
		const frame = frameFromSamples([0, 100], 8, 0, {
			rescaleSlope: 2,
			rescaleIntercept: -100,
		});

		expect(grayValues(renderRawFrameToRgba(frame, 0, 200))).toEqual([0, 255]);
	});

	it("inverts MONOCHROME1 output using normalized photometric metadata", () => {
		const frame = frameFromSamples([0, 255], 8, 0, {
			photometricInterpretation: " monochrome1 ",
		});

		expect(grayValues(renderRawFrameToRgba(frame, 127.5, 256))).toEqual([255, 0]);
	});

	it("uses the DICOM LINEAR half-unit boundaries", () => {
		const frame = frameFromSamples([0, 1, 49, 50, 99, 100], 8, 0);

		expect(grayValues(renderRawFrameToRgba(frame, 50, 100))).toEqual([
			0,
			3,
			126,
			129,
			255,
			255,
		]);
	});

	it("draws Pixel Padding black even for MONOCHROME1", () => {
		const frame = frameFromSamples([0, 10, 100, 200], 8, 0, {
			photometricInterpretation: "MONOCHROME1",
			paddingLow: 0,
			paddingHigh: 10,
		});

		expect(grayValues(renderRawFrameToRgba(frame, 150, 100))).toEqual([0, 0, 255, 0]);
	});

	it("treats width one as a threshold at center minus one half", () => {
		const frame = frameFromSamples([49, 50], 8, 0);

		expect(grayValues(renderRawFrameToRgba(frame, 50, 1))).toEqual([0, 255]);
	});
});

describe("raw window resolution", () => {
	it("excludes Pixel Padding from automatic windows like the server", () => {
		const padded = { paddingLow: 0, paddingHigh: 1000 };
		const frame = frameFromSamples([0, 1000, 2000, 3000], 16, 0, padded);

		expect(computeFullDynamicWindow(frame)).toEqual({ wc: 2500, ww: 1000 });
		expect(computePercentileWindow(frame)).toEqual({ wc: 2500, ww: 1000 });

		const allPadding = frameFromSamples([0, 1000], 16, 0, padded);
		expect(computeFullDynamicWindow(allPadding)).toEqual({ wc: 500, ww: 1000 });
	});


	it("computes full dynamic range from signed 8-bit samples", () => {
		const frame = frameFromSamples([-128, 0, 127], 8, 1, {
			defaultWc: 40,
			defaultWw: 80,
		});

		expect(computeFullDynamicWindow(frame)).toEqual({ wc: -0.5, ww: 255 });
		expect(resolveDisplayWindow(frame, 10, 20, 30, 40, "full_dynamic")).toEqual({
			wc: -0.5,
			ww: 255,
		});
	});

	it("uses live, explicit, DICOM, then percentile windows in default mode", () => {
		const withDefault = frameFromSamples([0, 10, 20, 30], 8, 0, {
			defaultWc: 15,
			defaultWw: 30,
		});
		const withoutDefault = frameFromSamples([0, 10, 20, 30], 8, 0);

		expect(resolveDisplayWindow(withDefault, 1, 2, 3, 4, "default")).toEqual({
			wc: 1,
			ww: 2,
		});
		expect(resolveDisplayWindow(withDefault, null, null, 3, 4, "default")).toEqual({
			wc: 3,
			ww: 4,
		});
		expect(resolveDisplayWindow(withDefault, null, null, null, null, "default")).toEqual({
			wc: 15,
			ww: 30,
		});
		expect(computePercentileWindow(withoutDefault)).toEqual({ wc: 15, ww: 30 });
		expect(resolveDisplayWindow(withoutDefault, null, null, null, null, "default")).toEqual({
			wc: 15,
			ww: 30,
		});
	});
});

describe("validateRenderableRawFrame", () => {
	it("rejects unsupported layouts and short buffers before rendering", () => {
		const short = frameFromSamples([0], 16, 0);
		short.metadata.columns = 2;
		expect(validateRenderableRawFrame(short)).toBe(
			"Raw frame buffer is shorter than expected for declared metadata",
		);

		const color = frameFromSamples([0], 8, 0, { samplesPerPixel: 3 });
		expect(validateRenderableRawFrame(color)).toBe("Unsupported SamplesPerPixel: 3");

		const invalidRepresentation = frameFromSamples([0], 8, 0, { pixelRepresentation: 2 });
		expect(validateRenderableRawFrame(invalidRepresentation)).toBe(
			"Unsupported PixelRepresentation: 2",
		);
	});
});

describe("selectWindowingPipeline", () => {
	it("keeps presentation-sensitive files on server rendering", () => {
		expect(selectWindowingPipeline(true, false, false)).toBe("server_wl");
		expect(selectWindowingPipeline(true, true, true)).toBe("server_wl");
		expect(selectWindowingPipeline(true, false, true)).toBe("diagnostic_wl");
		expect(selectWindowingPipeline(false, false, false)).toBe("cine");
	});
});

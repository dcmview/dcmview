import { describe, expect, it } from "vitest";
import type { RealWorldValueMap, WindowMode } from "../generated/api-types";
import type { RawFrame, RawFrameMetadata } from "../rawFrame";
import oracle from "../../../tests/windowing-cases.json";
import {
	computeFullDynamicWindow,
	computePercentileWindow,
	mappedUnitsPerStoredUnit,
	renderRawFrameToRgba,
	resolveDisplayWindow,
	resolveMappedDisplayWindow,
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

	it("inverts MONOCHROME1 output using normalized photometric metadata", () => {
		const frame = frameFromSamples([0, 255], 8, 0, {
			photometricInterpretation: " monochrome1 ",
		});

		expect(grayValues(renderRawFrameToRgba(frame, 127.5, 256))).toEqual([255, 0]);
	});
});

type OracleCase = {
	name: string;
	stored: number[];
	rescale_slope: number;
	rescale_intercept: number;
	photometric_interpretation: string;
	dicom_window: { center: number; width: number } | null;
	padding: { value: number; range_limit: number | null } | null;
	mode: WindowMode;
	wc: number | null;
	ww: number | null;
	expected: number[];
};

// The server runs the same cases through the loader and display endpoint in
// tests/integration/windowing_oracle.rs, which also checks that the raw
// endpoint sends exactly this metadata.
describe("shared windowing oracle", () => {
	it.each(oracle.cases as OracleCase[])("$name", (oracleCase) => {
		const { padding } = oracleCase;
		const limit = padding?.range_limit ?? padding?.value ?? null;
		const frame = frameFromSamples(oracleCase.stored, 16, 0, {
			photometricInterpretation: oracleCase.photometric_interpretation,
			rescaleSlope: oracleCase.rescale_slope,
			rescaleIntercept: oracleCase.rescale_intercept,
			defaultWc: oracleCase.dicom_window?.center ?? null,
			defaultWw: oracleCase.dicom_window?.width ?? null,
			paddingLow: padding && limit !== null ? Math.min(padding.value, limit) : null,
			paddingHigh: padding && limit !== null ? Math.max(padding.value, limit) : null,
		});
		const { wc, ww } = resolveDisplayWindow(
			frame,
			null,
			null,
			oracleCase.wc,
			oracleCase.ww,
			oracleCase.mode,
		);

		expect(grayValues(renderRawFrameToRgba(frame, wc, ww))).toEqual(oracleCase.expected);
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

describe("windowing a LUT real-world mapping on the raw path", () => {
	// Stored 10..14 map through a non-linear LUT; other stored values are unmapped.
	const lut: RealWorldValueMap = {
		source: "real_world_value_mapping",
		source_file_index: null,
		label: "T1",
		first_value_mapped: 10,
		last_value_mapped: 14,
		transform: { kind: "lut", values: [0, 1, 4, 9, 16] },
		unit_label: "ms",
		units: null,
		quantity: null,
	};
	const frame = frameFromSamples([10, 11, 12, 13, 14, 3], 8, 0);

	function grays(rgba: Uint8ClampedArray): number[] {
		return Array.from({ length: rgba.length / 4 }, (_, index) => rgba[index * 4]);
	}

	it("windows the LUT's values, not the stored ones", () => {
		// Window 0..16 ms: gray follows the squared LUT values; unmapped is black.
		expect(grays(renderRawFrameToRgba(frame, 8, 16, lut))).toEqual([0, 16, 64, 143, 255, 0]);
		// Window 4..9 ms clips below and above.
		expect(grays(renderRawFrameToRgba(frame, 6.5, 5, lut))).toEqual([0, 0, 0, 255, 255, 0]);
	});

	it("resolves live, explicit, and automatic windows in mapped units", () => {
		expect(resolveMappedDisplayWindow(frame, lut, 3, 4, 1, 2, "default")).toEqual({ wc: 3, ww: 4 });
		expect(resolveMappedDisplayWindow(frame, lut, null, null, 1, 2, "default")).toEqual({ wc: 1, ww: 2 });
		expect(resolveMappedDisplayWindow(frame, lut, null, null, null, null, "full_dynamic")).toEqual({ wc: 8, ww: 16 });
		expect(resolveMappedDisplayWindow(frame, lut, 3, 4, null, null, "full_dynamic")).toEqual({ wc: 8, ww: 16 });
	});

	it("scales window drags by the LUT's mapped units per stored unit", () => {
		expect(mappedUnitsPerStoredUnit(lut)).toBe(4);
		expect(mappedUnitsPerStoredUnit({ ...lut, transform: { kind: "linear", slope: -0.5, intercept: 3 } })).toBe(0.5);
	});
});

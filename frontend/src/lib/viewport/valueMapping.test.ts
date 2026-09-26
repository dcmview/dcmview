import { describe, expect, it } from "vitest";
import type { FrameValueMapping, RawFrame, RealWorldValueMap } from "../../api";
import {
	describePixelValues,
	formatValue,
	modalityValue,
	pixelAt,
	rawHeaderValueMapping,
	realWorldValue,
	storedSamplesAt,
} from "./valueMapping";

function frame(
	buffer: ArrayBuffer,
	{
		rows = 2,
		columns = 2,
		bitsAllocated = 16,
		pixelRepresentation = 0,
		samplesPerPixel = 1,
		photometricInterpretation = "MONOCHROME2",
	} = {},
): RawFrame {
	return {
		buffer,
		metadata: {
			rows,
			columns,
			bitsAllocated,
			pixelRepresentation,
			samplesPerPixel,
			photometricInterpretation,
			rescaleSlope: 1,
			rescaleIntercept: 0,
			defaultWc: null,
			defaultWw: null,
			paddingLow: null,
			paddingHigh: null,
		},
	};
}

function mapping(overrides: Partial<FrameValueMapping> = {}): FrameValueMapping {
	return {
		file_index: 1,
		frame_index: 0,
		stored_value_type: "integer",
		modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: null },
		real_world: [],
		...overrides,
	};
}

function linearMap(slope: number, intercept: number, overrides: Partial<RealWorldValueMap> = {}): RealWorldValueMap {
	return {
		source: "real_world_value_mapping",
		label: "ADC",
		first_value_mapped: 0,
		last_value_mapped: 4095,
		transform: { kind: "linear", slope, intercept },
		unit_label: "um2/s",
		units: null,
		quantity: null,
		...overrides,
	};
}

describe("value transforms", () => {
	it("applies the Modality rescale, or its LUT clamped to the table", () => {
		const rescale = { rescale_slope: 2, rescale_intercept: -1024, rescale_type: null, lut: null };
		expect(modalityValue(1000, rescale)).toBe(976);

		const lut = { ...rescale, lut: { first_value_mapped: 10, values: [100, 200, 300] } };
		expect(modalityValue(11, lut)).toBe(200);
		expect(modalityValue(2, lut)).toBe(100);
		expect(modalityValue(99, lut)).toBe(300);
	});

	it("maps stored values linearly within the declared stored range", () => {
		const map = linearMap(0.5, -10);
		expect(realWorldValue(100, map)).toBe(40);
		expect(realWorldValue(4095, map)).toBe(2037.5);
		expect(realWorldValue(4096, map)).toBeNull();
		expect(realWorldValue(-1, map)).toBeNull();
		expect(realWorldValue(-1, linearMap(0.5, 0, { first_value_mapped: null, last_value_mapped: null }))).toBe(-0.5);
	});

	it("indexes a real-world LUT from the first mapped stored value", () => {
		const map: RealWorldValueMap = {
			...linearMap(1, 0, { first_value_mapped: 5, last_value_mapped: 7 }),
			transform: { kind: "lut", values: [1.5, 2.5, 3.5] },
		};
		expect(realWorldValue(5, map)).toBe(1.5);
		expect(realWorldValue(7, map)).toBe(3.5);
		expect(realWorldValue(8, map)).toBeNull();
		expect(realWorldValue(6.5, map)).toBeNull();
	});

	it("formats integers exactly and other values to five significant digits", () => {
		expect(formatValue(-1024)).toBe("-1024");
		expect(formatValue(0.123456)).toBe("0.12346");
		expect(formatValue(23.300000000000001)).toBe("23.3");
	});
});

describe("storedSamplesAt", () => {
	it("reads signed 16-bit and 32-bit integer samples", () => {
		const signed = new Int16Array([0, -5, 7, -32768]);
		expect(storedSamplesAt(frame(signed.buffer, { pixelRepresentation: 1 }), { row: 0, column: 1 }, "integer"))
			.toEqual([-5]);
		const wide = new Uint32Array([1, 2, 70000, 4]);
		expect(storedSamplesAt(frame(wide.buffer, { bitsAllocated: 32 }), { row: 1, column: 0 }, "integer"))
			.toEqual([70000]);
	});

	it("reads float samples when the value mapping says they are float", () => {
		const floats = new Float32Array([0.25, -1.5, 2, 3]);
		const raw = frame(floats.buffer, { bitsAllocated: 32 });
		expect(storedSamplesAt(raw, { row: 0, column: 1 }, "float32")).toEqual([-1.5]);
		const doubles = new Float64Array([0.1, 0.2, 0.3, 0.4]);
		expect(storedSamplesAt(frame(doubles.buffer, { bitsAllocated: 64 }), { row: 1, column: 1 }, "float64"))
			.toEqual([0.4]);
	});

	it("reads interleaved and planar color samples", () => {
		const interleaved = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
		const rgb = frame(interleaved.buffer, { bitsAllocated: 8, samplesPerPixel: 3, photometricInterpretation: "RGB" });
		expect(storedSamplesAt(rgb, { row: 0, column: 1 }, "integer")).toEqual([4, 5, 6]);
		expect(storedSamplesAt(rgb, { row: 0, column: 1 }, "integer", 1)).toEqual([2, 6, 10]);
	});

	it("reads native YBR_FULL_422 samples from their pixel pair", () => {
		// One row of four pixels: Y0 Y1 Cb Cr, Y2 Y3 Cb Cr.
		const bytes = new Uint8Array([10, 11, 100, 200, 12, 13, 101, 201]);
		const ybr = frame(bytes.buffer, {
			rows: 1,
			columns: 4,
			bitsAllocated: 8,
			samplesPerPixel: 3,
			photometricInterpretation: "YBR_FULL_422",
		});
		expect(storedSamplesAt(ybr, { row: 0, column: 1 }, "integer")).toEqual([11, 100, 200]);
		expect(storedSamplesAt(ybr, { row: 0, column: 2 }, "integer")).toEqual([12, 101, 201]);
	});

	it("rejects pixels and buffers outside the frame", () => {
		const short = frame(new Uint16Array([1]).buffer);
		expect(storedSamplesAt(short, { row: 1, column: 1 }, "integer")).toBeNull();
		expect(storedSamplesAt(short, { row: 2, column: 0 }, "integer")).toBeNull();
	});
});

describe("describePixelValues", () => {
	const pixel = { row: 0, column: 0 };

	it("shows CT Modality values in HU and omits a unitless identity Modality transform", () => {
		const raw = frame(new Uint16Array([1034, 0, 0, 0]).buffer);
		const ct = mapping({ modality: { rescale_slope: 1, rescale_intercept: -1024, rescale_type: null, lut: null } });
		expect(describePixelValues(raw, pixel, ct, "CT")).toEqual({
			kind: "grayscale",
			stored: "1034",
			modality: { value: "10", unit: "HU" },
			mapped: null,
			mappedOutOfRange: false,
		});
		expect(describePixelValues(raw, pixel, mapping(), "MR")).toMatchObject({ modality: null, mapped: null });
		expect(describePixelValues(raw, pixel, mapping(), "CT")).toMatchObject({ modality: { value: "1034", unit: "HU" } });
	});

	it("shows the preferred real-world value with its unit, or flags an unmapped stored value", () => {
		const raw = frame(new Uint16Array([100, 5000, 0, 0]).buffer);
		const pm = mapping({ real_world: [linearMap(0.5, -10), linearMap(1, 0, { label: "other", unit_label: "s" })] });
		expect(describePixelValues(raw, pixel, pm, "MR")).toMatchObject({
			mapped: { value: "40", unit: "um2/s", label: "ADC" },
			mappedOutOfRange: false,
		});
		expect(describePixelValues(raw, { row: 0, column: 1 }, pm, "MR")).toMatchObject({
			mapped: null,
			mappedOutOfRange: true,
		});
	});

	it("reports Dose Grid Scaling as a dose in Gy", () => {
		const raw = frame(new Uint16Array([2330, 0, 0, 0]).buffer);
		const dose = mapping({
			real_world: [linearMap(0.01, 0, {
				source: "dose_grid_scaling",
				label: "Dose",
				first_value_mapped: null,
				last_value_mapped: null,
				unit_label: "Gy",
			})],
		});
		expect(describePixelValues(raw, pixel, dose, "RTDOSE")).toMatchObject({
			stored: "2330",
			mapped: { value: "23.3", unit: "Gy", label: "Dose" },
		});
	});

	it("labels color components and palette indices", () => {
		const rgb = frame(new Uint8Array([9, 8, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0]).buffer, {
			bitsAllocated: 8,
			samplesPerPixel: 3,
			photometricInterpretation: "RGB",
		});
		expect(describePixelValues(rgb, pixel, mapping(), "OT")).toEqual({
			kind: "color",
			components: [{ label: "R", value: "9" }, { label: "G", value: "8" }, { label: "B", value: "7" }],
		});
		const palette = frame(new Uint8Array([42, 0, 0, 0]).buffer, { bitsAllocated: 8, photometricInterpretation: "PALETTE COLOR" });
		expect(describePixelValues(palette, pixel, mapping(), "US")).toEqual({ kind: "palette", index: "42" });
	});
});

describe("pixel lookup helpers", () => {
	it("floors image points to pixels inside the image", () => {
		expect(pixelAt({ x: 3.9, y: 0.1 }, 4, 4)).toEqual({ row: 0, column: 3 });
		expect(pixelAt({ x: 4, y: 0 }, 4, 4)).toBeNull();
		expect(pixelAt({ x: -0.1, y: 0 }, 4, 4)).toBeNull();
		expect(pixelAt(null, 4, 4)).toBeNull();
	});

	it("falls back to the raw headers only when they identify the sample type", () => {
		const raw = frame(new Uint16Array(4).buffer);
		raw.metadata.rescaleIntercept = -1024;
		expect(rawHeaderValueMapping(2, 1, raw)).toMatchObject({
			stored_value_type: "integer",
			modality: { rescale_slope: 1, rescale_intercept: -1024, lut: null },
			real_world: [],
		});
		expect(rawHeaderValueMapping(2, 1, frame(new Float32Array(4).buffer, { bitsAllocated: 32 }))).toBeNull();
	});
});

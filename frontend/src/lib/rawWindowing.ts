import type { RawFrame, RawFrameMetadata } from "../rawFrame";
import type { RealWorldValueMap, WindowMode } from "../generated/api-types";
import { realWorldValue } from "./viewport/valueMapping";

export type ResolvedWindow = {
	wc: number;
	ww: number;
};

export const MAX_RENDER_PIXELS = 20_000_000;

export type WindowingPipeline = "cine" | "diagnostic_wl" | "server_wl";

export function selectWindowingPipeline(
	windowLevelActive: boolean,
	requestFallback: boolean,
	presentationCompatible: boolean,
): WindowingPipeline {
	if (!windowLevelActive) return "cine";
	return requestFallback || !presentationCompatible ? "server_wl" : "diagnostic_wl";
}

const PLATFORM_LITTLE_ENDIAN = new Uint8Array(new Uint16Array([1]).buffer)[0] === 1;

type SampleReader = {
	read: (index: number) => number;
	minRaw: number;
	size: number;
};

export function validateRenderableRawFrame(
	frame: RawFrame,
	maxRenderPixels = MAX_RENDER_PIXELS,
): string | null {
	const { rows, columns, bitsAllocated, pixelRepresentation, samplesPerPixel } = frame.metadata;
	if (!Number.isInteger(rows) || !Number.isInteger(columns) || rows <= 0 || columns <= 0) {
		return "Invalid raw frame dimensions";
	}
	if (samplesPerPixel !== 1) {
		return `Unsupported SamplesPerPixel: ${samplesPerPixel}`;
	}
	if (bitsAllocated !== 8 && bitsAllocated !== 16) {
		return `Unsupported BitsAllocated for viewport: ${bitsAllocated}`;
	}
	if (pixelRepresentation !== 0 && pixelRepresentation !== 1) {
		return `Unsupported PixelRepresentation: ${pixelRepresentation}`;
	}
	const numPixels = rows * columns;
	if (!Number.isSafeInteger(numPixels) || numPixels <= 0) {
		return "Invalid raw frame pixel count";
	}
	if (numPixels > maxRenderPixels) {
		return `Frame too large to render safely (${rows}×${columns})`;
	}
	const minExpectedBytes = numPixels * (bitsAllocated / 8);
	if (frame.buffer.byteLength < minExpectedBytes) {
		return "Raw frame buffer is shorter than expected for declared metadata";
	}
	return null;
}

/**
 * Windows a raw frame into RGBA. With `valueMap`, the window applies to
 * that real-world mapping's values (a LUT mapping included) instead of
 * Modality values; stored values it does not map are drawn black.
 */
export function renderRawFrameToRgba(
	frame: RawFrame,
	wc: number,
	ww: number,
	valueMap: RealWorldValueMap | null = null,
): Uint8ClampedArray<ArrayBuffer> {
	const validationError = validateRenderableRawFrame(frame);
	if (validationError) {
		throw new Error(validationError);
	}

	const reader = createSampleReader(frame);
	const lut = buildWindowLut(frame.metadata, reader, wc, Math.max(ww, 1), valueMap);
	const isPadding = paddingPredicate(frame.metadata);
	if (isPadding) {
		// Padding is background: black after any MONOCHROME1 inversion.
		for (let index = 0; index < reader.size; index += 1) {
			if (isPadding(index + reader.minRaw)) lut[index] = 0;
		}
	}
	const numPixels = frame.metadata.rows * frame.metadata.columns;
	const output = new Uint8ClampedArray(new ArrayBuffer(numPixels * 4));

	for (let index = 0; index < numPixels; index += 1) {
		const gray = lut[reader.read(index) - reader.minRaw];
		const offset = index * 4;
		output[offset] = gray;
		output[offset + 1] = gray;
		output[offset + 2] = gray;
		output[offset + 3] = 255;
	}

	return output;
}

export function resolveDisplayWindow(
	frame: RawFrame,
	liveWc: number | null,
	liveWw: number | null,
	wc: number | null,
	ww: number | null,
	mode: WindowMode,
): ResolvedWindow {
	if (mode === "full_dynamic") {
		return computeFullDynamicWindow(frame);
	}
	if (liveWc !== null && liveWw !== null) {
		return { wc: liveWc, ww: liveWw };
	}
	if (wc !== null && ww !== null) {
		return { wc, ww };
	}
	const { defaultWc, defaultWw } = frame.metadata;
	if (defaultWc !== null && defaultWw !== null) {
		return { wc: defaultWc, ww: defaultWw };
	}
	return computePercentileWindow(frame);
}

/**
 * The window of a frame rendered in `valueMap`'s units, in the same order
 * as `resolveDisplayWindow`: full dynamic, the live or explicit window
 * (already in those units), else the 1st/99th percentile of mapped values.
 */
export function resolveMappedDisplayWindow(
	frame: RawFrame,
	valueMap: RealWorldValueMap,
	liveWc: number | null,
	liveWw: number | null,
	wc: number | null,
	ww: number | null,
	mode: WindowMode,
): ResolvedWindow {
	const step = mappedUnitsPerStoredUnit(valueMap);
	if (mode === "full_dynamic") return windowOfValues(mappedWindowRuns(frame, valueMap), false, step);
	if (liveWc !== null && liveWw !== null) return { wc: liveWc, ww: liveWw };
	if (wc !== null && ww !== null) return { wc, ww };
	return windowOfValues(mappedWindowRuns(frame, valueMap), true, step);
}

/**
 * Mapped units per stored unit across a mapping's stored range, so a
 * window drag moves a mapped window about as fast as a stored one.
 */
export function mappedUnitsPerStoredUnit(valueMap: RealWorldValueMap): number {
	const { transform } = valueMap;
	if (transform.kind === "linear") return Math.abs(transform.slope) || 1;
	const finite = transform.values.filter(Number.isFinite);
	if (finite.length < 2) return 1;
	const range = Math.max(...finite) - Math.min(...finite);
	return range > 0 ? range / (finite.length - 1) : 1;
}

// Raw frames hold 8- or 16-bit integers (validateRenderableRawFrame), so every
// stored value lies in this range.
const RAW_VALUE_OFFSET = 32768;
const RAW_VALUE_COUNT = RAW_VALUE_OFFSET + 65536;

/** How many samples hold each stored value, indexed by value + RAW_VALUE_OFFSET. */
function storedValueCounts(frame: RawFrame): Uint32Array {
	const reader = validatedSampleReader(frame);
	const { rows, columns } = frame.metadata;
	const counts = new Uint32Array(RAW_VALUE_COUNT);
	for (let index = 0; index < rows * columns; index += 1) {
		counts[reader.read(index) + RAW_VALUE_OFFSET] += 1;
	}
	return counts;
}

/**
 * The frame's values in ascending order, as (value, count) runs: each stored
 * value that occurs is mapped once by `valueOf` (null or non-finite drops it),
 * instead of mapping and sorting every sample.
 */
class ValueRuns {
	readonly total: number;
	readonly #runs: [number, number][];

	constructor(counts: Uint32Array, valueOf: (raw: number) => number | null, include: (raw: number) => boolean) {
		const runs: [number, number][] = [];
		let total = 0;
		for (let index = 0; index < counts.length; index += 1) {
			const count = counts[index];
			const raw = index - RAW_VALUE_OFFSET;
			if (count === 0 || !include(raw)) continue;
			const value = valueOf(raw);
			if (value === null || !Number.isFinite(value)) continue;
			runs.push([value, count]);
			total += count;
		}
		this.#runs = runs.sort((left, right) => left[0] - right[0]);
		this.total = total;
	}

	/** The value at `index` of the samples in ascending order. */
	at(index: number): number {
		let seen = 0;
		for (const [value, count] of this.#runs) {
			seen += count;
			if (index < seen) return value;
		}
		return this.#runs.length > 0 ? this.#runs[this.#runs.length - 1][0] : Number.NaN;
	}

	/** The 1st/99th percentile span, or the full range. */
	span(percentile: boolean): [number, number] {
		const last = this.total - 1;
		return percentile
			? [this.at(Math.floor(this.total * 0.01)), this.at(Math.min(Math.ceil(this.total * 0.99), last))]
			: [this.at(0), this.at(last)];
	}
}

function mappedWindowRuns(frame: RawFrame, valueMap: RealWorldValueMap): ValueRuns {
	const isPadding = paddingPredicate(frame.metadata);
	return new ValueRuns(storedValueCounts(frame), (raw) => realWorldValue(raw, valueMap), (raw) => !isPadding?.(raw));
}

/** A window over `values`, at least `minWidth` (one stored unit's worth) wide. */
function windowOfValues(values: ValueRuns, percentile: boolean, minWidth: number): ResolvedWindow {
	if (values.total === 0) return { wc: minWidth / 2, ww: minWidth };
	const [low, high] = values.span(percentile);
	// Mapped values can span less than one unit, so the floor is one stored
	// unit in mapped units rather than 1.
	const width = Math.max(high - low, minWidth);
	return { wc: low + width / 2, ww: width };
}

// Automatic windows scan every sample, so each frame's result is computed
// once; frames are immutable once fetched.
const fullDynamicWindows = new WeakMap<RawFrame, ResolvedWindow>();
const percentileWindows = new WeakMap<RawFrame, ResolvedWindow>();

export function computeFullDynamicWindow(frame: RawFrame): ResolvedWindow {
	const cached = fullDynamicWindows.get(frame);
	if (cached) return cached;
	const window = scanFullDynamicWindow(frame);
	fullDynamicWindows.set(frame, window);
	return window;
}

export function computePercentileWindow(frame: RawFrame): ResolvedWindow {
	const cached = percentileWindows.get(frame);
	if (cached) return cached;
	const window = scanPercentileWindow(frame);
	percentileWindows.set(frame, window);
	return window;
}

function scanFullDynamicWindow(frame: RawFrame): ResolvedWindow {
	const values = windowSourceValues(frame);
	if (values.total === 0) return { wc: 128, ww: 256 };
	const [min, max] = values.span(false);
	const width = Math.max(max - min, 1);
	return { wc: min + width / 2, ww: width };
}

function scanPercentileWindow(frame: RawFrame): ResolvedWindow {
	const [p1, p99] = windowSourceValues(frame).span(true);
	const width = Math.max(p99 - p1, 1);
	return { wc: p1 + width / 2, ww: width };
}

/** Rescaled samples for automatic windows, excluding Pixel Padding like the server. */
function windowSourceValues(frame: RawFrame): ValueRuns {
	const { rescaleSlope, rescaleIntercept } = frame.metadata;
	const counts = storedValueCounts(frame);
	const rescale = (raw: number) => raw * rescaleSlope + rescaleIntercept;
	const isPadding = paddingPredicate(frame.metadata);
	const unpadded = new ValueRuns(counts, rescale, (raw) => !isPadding?.(raw));
	// An all-padding frame falls back to every sample, as the server does.
	return unpadded.total > 0 || !isPadding ? unpadded : new ValueRuns(counts, rescale, () => true);
}

function paddingPredicate(metadata: RawFrameMetadata): ((raw: number) => boolean) | null {
	const { paddingLow, paddingHigh } = metadata;
	if (paddingLow === null || paddingHigh === null) return null;
	return (raw) => raw >= paddingLow && raw <= paddingHigh;
}

function validatedSampleReader(frame: RawFrame): SampleReader {
	const validationError = validateRenderableRawFrame(frame);
	if (validationError) {
		throw new Error(validationError);
	}
	return createSampleReader(frame);
}

function createSampleReader(frame: RawFrame): SampleReader {
	const { bitsAllocated, pixelRepresentation, rows, columns } = frame.metadata;
	const numPixels = rows * columns;
	const signed = pixelRepresentation === 1;

	if (bitsAllocated === 8 && signed) {
		const source = new Int8Array(frame.buffer, 0, numPixels);
		return { read: (index) => source[index], minRaw: -128, size: 256 };
	}
	if (bitsAllocated === 8) {
		const source = new Uint8Array(frame.buffer, 0, numPixels);
		return { read: (index) => source[index], minRaw: 0, size: 256 };
	}
	if (bitsAllocated === 16 && PLATFORM_LITTLE_ENDIAN) {
		if (signed) {
			const source = new Int16Array(frame.buffer, 0, numPixels);
			return { read: (index) => source[index], minRaw: -32768, size: 65536 };
		}
		const source = new Uint16Array(frame.buffer, 0, numPixels);
		return { read: (index) => source[index], minRaw: 0, size: 65536 };
	}

	const source = new DataView(frame.buffer);
	if (signed) {
		return {
			read: (index) => source.getInt16(index * 2, true),
			minRaw: -32768,
			size: 65536,
		};
	}
	return {
		read: (index) => source.getUint16(index * 2, true),
		minRaw: 0,
		size: 65536,
	};
}

function buildWindowLut(
	metadata: RawFrameMetadata,
	reader: SampleReader,
	wc: number,
	ww: number,
	valueMap: RealWorldValueMap | null,
): Uint8Array {
	const invert = metadata.photometricInterpretation.trim().toUpperCase() === "MONOCHROME1";
	const lut = new Uint8Array(reader.size);
	if (valueMap) {
		// Mapped values follow the linear VOI function without the integer
		// half-unit offsets, which assume Modality integers.
		const low = wc - ww / 2;
		for (let index = 0; index < reader.size; index += 1) {
			const mapped = realWorldValue(index + reader.minRaw, valueMap);
			const value = mapped === null || !Number.isFinite(mapped)
				? 0
				: Math.min(Math.max((mapped - low) / ww, 0), 1);
			const gray = Math.round(value * 255);
			lut[index] = invert ? 255 - gray : gray;
		}
		return lut;
	}
	const width = Math.max(ww, 1);
	const center = wc - 0.5;
	const low = center - (width - 1) / 2;
	const high = center + (width - 1) / 2;

	for (let index = 0; index < reader.size; index += 1) {
		const raw = index + reader.minRaw;
		const modal = raw * metadata.rescaleSlope + metadata.rescaleIntercept;
		let value: number;
		if (modal <= low) {
			value = 0;
		} else if (modal > high || width === 1) {
			value = 1;
		} else {
			value = (modal - center) / (width - 1) + 0.5;
		}
		// MONOCHROME1 inverts the quantized VOI output, like the server.
		const gray = Math.round(value * 255);
		lut[index] = invert ? 255 - gray : gray;
	}

	return lut;
}

import type { RawFrame, RawFrameMetadata } from "../rawFrame";
import type { WindowMode } from "../generated/api-types";

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

export function renderRawFrameToRgba(
	frame: RawFrame,
	wc: number,
	ww: number,
): Uint8ClampedArray<ArrayBuffer> {
	const validationError = validateRenderableRawFrame(frame);
	if (validationError) {
		throw new Error(validationError);
	}

	const reader = createSampleReader(frame);
	const lut = buildWindowLut(frame.metadata, reader, wc, Math.max(ww, 1));
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

export function computeFullDynamicWindow(frame: RawFrame): ResolvedWindow {
	const values = windowSourceValues(frame);
	let min = Infinity;
	let max = -Infinity;

	for (const value of values) {
		if (value < min) min = value;
		if (value > max) max = value;
	}

	if (!Number.isFinite(min) || !Number.isFinite(max)) {
		return { wc: 128, ww: 256 };
	}
	const width = Math.max(max - min, 1);
	return { wc: min + width / 2, ww: width };
}

export function computePercentileWindow(frame: RawFrame): ResolvedWindow {
	const values = windowSourceValues(frame);
	const numPixels = values.length;

	values.sort();
	const p1 = values[Math.floor(numPixels * 0.01)];
	const p99 = values[Math.min(Math.ceil(numPixels * 0.99), numPixels - 1)];
	const width = Math.max(p99 - p1, 1);
	return { wc: p1 + width / 2, ww: width };
}

/** Rescaled samples for automatic windows, excluding Pixel Padding like the server. */
function windowSourceValues(frame: RawFrame): Float64Array {
	const reader = validatedSampleReader(frame);
	const { rescaleSlope, rescaleIntercept, rows, columns } = frame.metadata;
	const numPixels = rows * columns;
	const isPadding = paddingPredicate(frame.metadata);
	const values = new Float64Array(numPixels);
	let count = 0;

	for (let index = 0; index < numPixels; index += 1) {
		const raw = reader.read(index);
		if (isPadding?.(raw)) continue;
		values[count] = raw * rescaleSlope + rescaleIntercept;
		count += 1;
	}
	if (count > 0 || !isPadding) return values.subarray(0, count);

	// An all-padding frame falls back to every sample, as the server does.
	for (let index = 0; index < numPixels; index += 1) {
		values[index] = reader.read(index) * rescaleSlope + rescaleIntercept;
	}
	return values;
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
): Uint8Array {
	const width = Math.max(ww, 1);
	const center = wc - 0.5;
	const low = center - (width - 1) / 2;
	const high = center + (width - 1) / 2;
	const invert = metadata.photometricInterpretation.trim().toUpperCase() === "MONOCHROME1";
	const lut = new Uint8Array(reader.size);

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
		if (invert) value = 1 - value;
		lut[index] = Math.round(value * 255);
	}

	return lut;
}

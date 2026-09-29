import type { RawFrame, RawFrameMetadata } from "../rawFrame";
import type {
	FrameValueMapping,
	ModalityValueTransform,
	RealWorldValueMap,
	VoiLookupTable,
	WindowMode,
} from "../generated/api-types";
import { realWorldValue } from "./viewport/valueMapping";

export type ResolvedWindow = {
	wc: number;
	ww: number;
	/**
	 * The VOI LUT that presents the frame instead of this window: default
	 * mode with no window requested or stored, as on the server.
	 */
	voiLut?: VoiLookupTable;
};

/**
 * What the display pipeline needs about a frame's samples beyond the raw
 * headers, from the frame's value mapping: whether 32-bit samples are
 * floats, the Modality LUT that replaces the rescale, and the VOI LUT.
 */
export type SamplePresentation = {
	/** `integer`, `float32`, or `float64`. */
	storedValueType: string;
	modality: ModalityValueTransform;
	voiLut: VoiLookupTable | null;
};

/** The presentation a value mapping describes, or the raw headers' rescale alone. */
export function samplePresentation(frame: RawFrame, mapping: FrameValueMapping | null): SamplePresentation {
	if (mapping) {
		return { storedValueType: mapping.stored_value_type, modality: mapping.modality, voiLut: mapping.voi_lut };
	}
	const { rescaleSlope, rescaleIntercept } = frame.metadata;
	return {
		storedValueType: "integer",
		modality: { rescale_slope: rescaleSlope, rescale_intercept: rescaleIntercept, rescale_type: null, lut: null },
		voiLut: null,
	};
}

export type RenderOptions = {
	/** Window this real-world mapping's values instead of Modality values. */
	valueMap?: RealWorldValueMap | null;
	/** Defaults to the raw headers' rescale for integer samples. */
	presentation?: SamplePresentation;
	/** Present Modality values with this VOI LUT instead of the window. */
	voiLut?: VoiLookupTable | null;
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
	/**
	 * For 1-, 8- and 16-bit integers, every stored value lies in
	 * [minRaw, minRaw + size), so the pipeline runs once per possible value;
	 * null for 32-bit and float samples, which are presented one by one.
	 */
	table: { minRaw: number; size: number } | null;
};

function bytesPerSample(bitsAllocated: number): number {
	return bitsAllocated === 1 ? 1 : bitsAllocated / 8;
}

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
	if (![1, 8, 16, 32, 64].includes(bitsAllocated)) {
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
	if (frame.buffer.byteLength < numPixels * bytesPerSample(bitsAllocated)) {
		return "Raw frame buffer is shorter than expected for declared metadata";
	}
	return null;
}

/**
 * Windows a raw frame into RGBA with the server's display pipeline: the
 * Modality LUT or rescale, then the LINEAR window or a VOI LUT, MONOCHROME1
 * inversion, and Pixel Padding drawn black. With `valueMap`, the window
 * applies to that real-world mapping's values (a LUT mapping included)
 * instead of Modality values; stored values it does not map take the
 * window's low end. The server's display frames with `unit` follow the same
 * rules (tests/windowing-cases.json).
 */
export function renderRawFrameToRgba(
	frame: RawFrame,
	wc: number,
	ww: number,
	options: RenderOptions = {},
): Uint8ClampedArray<ArrayBuffer> {
	const validationError = validateRenderableRawFrame(frame);
	if (validationError) {
		throw new Error(validationError);
	}
	const presentation = options.presentation ?? samplePresentation(frame, null);
	const reader = createSampleReader(frame, presentation.storedValueType);
	const gray = grayFunction(frame.metadata, presentation, wc, ww, options);
	// Padding is background: black after any MONOCHROME1 inversion.
	const isPadding = paddingPredicate(frame.metadata);
	const present = (stored: number) => (isPadding?.(stored) ? 0 : gray(stored));
	const numPixels = frame.metadata.rows * frame.metadata.columns;
	const output = new Uint8ClampedArray(new ArrayBuffer(numPixels * 4));

	const { table } = reader;
	const lut = table ? Uint8Array.from({ length: table.size }, (_, index) => present(index + table.minRaw)) : null;
	for (let index = 0; index < numPixels; index += 1) {
		const stored = reader.read(index);
		const value = lut && table ? lut[stored - table.minRaw] : present(stored);
		const offset = index * 4;
		output[offset] = value;
		output[offset + 1] = value;
		output[offset + 2] = value;
		output[offset + 3] = 255;
	}

	return output;
}

/**
 * The window a raw frame is shown with: full dynamic, the live or explicit
 * window, the DICOM window, else the 1st/99th percentile. A live or explicit
 * window on integer Modality values is reported as LINEAR applies it (and as
 * the server reports it), at least one value wide; a window carried over at a
 * sub-unit relative width therefore never reads "W: 0". Continuous values
 * keep sub-unit widths.
 */
export function resolveDisplayWindow(
	frame: RawFrame,
	liveWc: number | null,
	liveWw: number | null,
	wc: number | null,
	ww: number | null,
	mode: WindowMode,
	presentation?: SamplePresentation,
): ResolvedWindow {
	if (mode === "full_dynamic") {
		return computeFullDynamicWindow(frame, presentation);
	}
	const applied = (center: number, width: number): ResolvedWindow => ({ wc: center,
		ww: integerModality(presentation ?? samplePresentation(frame, null)) ? Math.max(width, 1) : width });
	if (liveWc !== null && liveWw !== null) {
		return applied(liveWc, liveWw);
	}
	if (wc !== null && ww !== null) {
		return applied(wc, ww);
	}
	const { defaultWc, defaultWw } = frame.metadata;
	if (defaultWc !== null && defaultWw !== null) {
		return { wc: defaultWc, ww: defaultWw };
	}
	// A VOI LUT presents the frame; the percentile window is where a drag starts.
	const window = computePercentileWindow(frame, presentation);
	return presentation?.voiLut ? { ...window, voiLut: presentation.voiLut } : window;
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

// 1-, 8- and 16-bit integer samples lie in this range.
const RAW_VALUE_OFFSET = 32768;
const RAW_VALUE_COUNT = RAW_VALUE_OFFSET + 65536;

/** How many samples hold each stored value, indexed by value + RAW_VALUE_OFFSET. */
function storedValueCounts(frame: RawFrame, reader: SampleReader): Uint32Array {
	const { rows, columns } = frame.metadata;
	const counts = new Uint32Array(RAW_VALUE_COUNT);
	for (let index = 0; index < rows * columns; index += 1) {
		counts[reader.read(index) + RAW_VALUE_OFFSET] += 1;
	}
	return counts;
}

/** A frame's values in ascending order, for automatic windows. */
interface OrderedValues {
	readonly total: number;
	/** The value at `index` of the samples in ascending order. */
	at(index: number): number;
	/** The smallest and largest value. */
	range(): [number, number];
}

/** The 1st/99th percentile span, or the full range. */
function valueSpan(values: OrderedValues, percentile: boolean): [number, number] {
	const last = values.total - 1;
	return percentile
		? [values.at(Math.floor(values.total * 0.01)), values.at(Math.min(Math.ceil(values.total * 0.99), last))]
		: values.range();
}

/**
 * The frame's values in ascending order, as (value, count) runs: each stored
 * value that occurs is mapped once by `valueOf` (null or non-finite drops it),
 * instead of mapping and sorting every sample.
 */
class ValueRuns implements OrderedValues {
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

	at(index: number): number {
		let seen = 0;
		for (const [value, count] of this.#runs) {
			seen += count;
			if (index < seen) return value;
		}
		return this.#runs.length > 0 ? this.#runs[this.#runs.length - 1][0] : Number.NaN;
	}

	range(): [number, number] {
		return [this.at(0), this.at(this.total - 1)];
	}
}

/**
 * Every value of a frame presented one sample at a time (32-bit and float
 * samples), sorted, as the server's per-sample path orders them: NaN last,
 * and ignored by the full range, which takes a minimum and maximum.
 */
class SortedValues implements OrderedValues {
	readonly total: number;
	readonly #values: Float64Array;

	constructor(values: Float64Array) {
		this.#values = values.sort();
		this.total = values.length;
	}

	at(index: number): number {
		return this.#values[Math.min(index, this.total - 1)];
	}

	range(): [number, number] {
		// f64::min and f64::max folds: comparisons with NaN are false.
		let low = Number.POSITIVE_INFINITY;
		let high = Number.NEGATIVE_INFINITY;
		for (const value of this.#values) {
			if (value < low) low = value;
			if (value > high) high = value;
		}
		return [low, high];
	}
}

function mappedWindowRuns(frame: RawFrame, valueMap: RealWorldValueMap): ValueRuns {
	const isPadding = paddingPredicate(frame.metadata);
	const counts = storedValueCounts(frame, validatedSampleReader(frame, "integer"));
	return new ValueRuns(counts, (raw) => realWorldValue(raw, valueMap), (raw) => !isPadding?.(raw));
}

/** A window over `values`, at least `minWidth` (one stored unit's worth) wide. */
function windowOfValues(values: ValueRuns, percentile: boolean, minWidth: number): ResolvedWindow {
	if (values.total === 0) return { wc: minWidth / 2, ww: minWidth };
	const [low, high] = valueSpan(values, percentile);
	// Mapped values can span less than one unit, so the floor is one stored
	// unit in mapped units rather than 1.
	const width = Math.max(high - low, minWidth);
	return { wc: low + width / 2, ww: width };
}

// Automatic windows scan every sample, so each frame's result is computed
// once per presentation; frames are immutable once fetched.
type AutomaticWindows = { modality: ModalityValueTransform | null; full?: ResolvedWindow; percentile?: ResolvedWindow };
const automaticWindows = new WeakMap<RawFrame, AutomaticWindows>();

function cachedAutomaticWindow(
	frame: RawFrame,
	presentation: SamplePresentation | undefined,
	kind: "full" | "percentile",
): ResolvedWindow {
	const modality = presentation?.modality ?? null;
	let cached = automaticWindows.get(frame);
	if (!cached || cached.modality !== modality) {
		cached = { modality };
		automaticWindows.set(frame, cached);
	}
	cached[kind] ??= automaticWindow(
		windowSourceValues(frame, presentation),
		kind === "percentile",
		integerModality(presentation ?? samplePresentation(frame, null)),
	);
	return cached[kind];
}

export function computeFullDynamicWindow(frame: RawFrame, presentation?: SamplePresentation): ResolvedWindow {
	return cachedAutomaticWindow(frame, presentation, "full");
}

export function computePercentileWindow(frame: RawFrame, presentation?: SamplePresentation): ResolvedWindow {
	return cachedAutomaticWindow(frame, presentation, "percentile");
}

function automaticWindow(values: OrderedValues, percentile: boolean, integer: boolean): ResolvedWindow {
	if (values.total === 0) return { wc: 128, ww: 256 };
	const [low, high] = valueSpan(values, percentile);
	// f64::max on the server ignores NaN; Math.max would return it.
	const span = high - low;
	const width = span > (integer ? 1 : 0) ? span : 1;
	return { wc: low + width / 2, ww: width };
}

/** Modality values for automatic windows, excluding Pixel Padding like the server. */
function windowSourceValues(frame: RawFrame, presentation = samplePresentation(frame, null)): OrderedValues {
	const reader = validatedSampleReader(frame, presentation.storedValueType);
	const modal = modalityFunction(presentation.modality);
	const isPadding = paddingPredicate(frame.metadata);
	if (!reader.table) {
		const count = frame.metadata.rows * frame.metadata.columns;
		const all = new Float64Array(count);
		const unpadded: number[] = [];
		for (let index = 0; index < count; index += 1) {
			const stored = reader.read(index);
			all[index] = modal(stored);
			if (!isPadding?.(stored)) unpadded.push(all[index]);
		}
		// An all-padding frame falls back to every sample, as the server does.
		return new SortedValues(unpadded.length > 0 || !isPadding ? Float64Array.from(unpadded) : all);
	}
	const counts = storedValueCounts(frame, reader);
	const unpadded = new ValueRuns(counts, modal, (raw) => !isPadding?.(raw));
	return unpadded.total > 0 || !isPadding ? unpadded : new ValueRuns(counts, modal, () => true);
}

function paddingPredicate(metadata: RawFrameMetadata): ((raw: number) => boolean) | null {
	const { paddingLow, paddingHigh } = metadata;
	if (paddingLow === null || paddingHigh === null) return null;
	return (raw) => raw >= paddingLow && raw <= paddingHigh;
}

function validatedSampleReader(frame: RawFrame, storedValueType: string): SampleReader {
	const validationError = validateRenderableRawFrame(frame);
	if (validationError) {
		throw new Error(validationError);
	}
	return createSampleReader(frame, storedValueType);
}

function createSampleReader(frame: RawFrame, storedValueType: string): SampleReader {
	const { bitsAllocated, pixelRepresentation, rows, columns } = frame.metadata;
	const numPixels = rows * columns;
	// One-bit samples arrive one per byte and are unsigned, as on the server.
	const signed = pixelRepresentation === 1 && bitsAllocated !== 1;
	const view = new DataView(frame.buffer);
	const typed = <T extends { [index: number]: number }>(make: () => T, fallback: (offset: number) => number) => {
		if (PLATFORM_LITTLE_ENDIAN) {
			const source = make();
			return (index: number) => source[index];
		}
		return (index: number) => fallback(index * (bitsAllocated / 8));
	};

	switch (bitsAllocated) {
		case 1:
		case 8: {
			const source = signed ? new Int8Array(frame.buffer, 0, numPixels) : new Uint8Array(frame.buffer, 0, numPixels);
			return { read: (index) => source[index], table: { minRaw: signed ? -128 : 0, size: 256 } };
		}
		case 16:
			return signed
				? {
					read: typed(() => new Int16Array(frame.buffer, 0, numPixels), (offset) => view.getInt16(offset, true)),
					table: { minRaw: -32768, size: 65536 },
				}
				: {
					read: typed(() => new Uint16Array(frame.buffer, 0, numPixels), (offset) => view.getUint16(offset, true)),
					table: { minRaw: 0, size: 65536 },
				};
		case 32:
			if (storedValueType === "float32") {
				return {
					read: typed(() => new Float32Array(frame.buffer, 0, numPixels), (offset) => view.getFloat32(offset, true)),
					table: null,
				};
			}
			return signed
				? { read: typed(() => new Int32Array(frame.buffer, 0, numPixels), (offset) => view.getInt32(offset, true)), table: null }
				: { read: typed(() => new Uint32Array(frame.buffer, 0, numPixels), (offset) => view.getUint32(offset, true)), table: null };
		case 64:
			if (storedValueType !== "float64") throw new Error("64-bit samples must be Double Float Pixel Data");
			return {
				read: typed(() => new Float64Array(frame.buffer, 0, numPixels), (offset) => view.getFloat64(offset, true)),
				table: null,
			};
		default:
			throw new Error(`Unsupported BitsAllocated for viewport: ${bitsAllocated}`);
	}
}

/** One LUT entry, indexed like the server: the value truncated, then clamped to the table. */
function lutEntry(firstValueMapped: number, values: readonly number[], value: number): number {
	const offset = (Number.isNaN(value) ? 0 : Math.trunc(value)) - firstValueMapped;
	return values[Math.min(Math.max(offset, 0), values.length - 1)];
}

/** The Modality LUT, or else the rescale, applied to one stored value. */
function modalityFunction(modality: ModalityValueTransform): (stored: number) => number {
	const { lut, rescale_slope: slope, rescale_intercept: intercept } = modality;
	if (lut && lut.values.length > 0) return (stored) => lutEntry(lut.first_value_mapped, lut.values, stored);
	return (stored) => stored * slope + intercept;
}

/** A display byte, as the server casts one: NaN is 0, and values saturate. */
function toByte(value: number): number {
	return Number.isNaN(value) ? 0 : Math.min(Math.max(Math.round(value), 0), 255);
}

/** Integer Modality values use LINEAR; all other values use a continuous window. */
function integerModality(presentation: SamplePresentation): boolean {
	const { modality, storedValueType } = presentation;
	return storedValueType === "integer" && (Boolean(modality.lut?.values.length)
		|| (Number.isInteger(modality.rescale_slope) && Number.isInteger(modality.rescale_intercept)));
}

/** The displayed byte of one stored value, before Pixel Padding. */
function grayFunction(
	metadata: RawFrameMetadata,
	presentation: SamplePresentation,
	wc: number,
	ww: number,
	{ valueMap = null, voiLut = null }: RenderOptions,
): (stored: number) => number {
	const invert = metadata.photometricInterpretation.trim().toUpperCase() === "MONOCHROME1";
	// MONOCHROME1 inverts the quantized output, like the server.
	const output = (gray: number) => (invert ? 255 - gray : gray);
	if (valueMap) {
		// Mapped values follow the linear VOI function without the integer
		// half-unit offsets, which assume Modality integers.
		const low = wc - ww / 2;
		return (stored) => {
			const mapped = realWorldValue(stored, valueMap);
			const value = mapped === null || !Number.isFinite(mapped) ? 0 : Math.min(Math.max((mapped - low) / ww, 0), 1);
			return output(toByte(value * 255));
		};
	}
	const modal = modalityFunction(presentation.modality);
	if (voiLut) {
		const max = 2 ** voiLut.bits_per_entry - 1;
		return (stored) => {
			const entry = lutEntry(voiLut.first_value_mapped, voiLut.values, modal(stored));
			return output(Math.floor((entry * 255 + Math.floor(max / 2)) / max));
		};
	}
	if (!integerModality(presentation)) {
		const low = wc - ww / 2;
		return (stored) => output(toByte(Math.min(Math.max((modal(stored) - low) / ww, 0), 1) * 255));
	}
	ww = Math.max(ww, 1);
	const center = wc - 0.5;
	const low = center - (ww - 1) / 2;
	const high = center + (ww - 1) / 2;
	return (stored) => {
		const value = modal(stored);
		if (value <= low) return output(0);
		if (value > high || ww === 1) return output(255);
		return output(toByte(((value - center) / (ww - 1) + 0.5) * 255));
	};
}

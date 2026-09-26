import type {
	FrameValueMapping,
	ModalityValueTransform,
	RawFrame,
	RealWorldValueMap,
	ValueLookupTable,
} from "../../api";

/** One image pixel, zero-based. */
export type ImagePixel = { row: number; column: number };

/** The image pixel containing an image-space point, or null outside the image. */
export function pixelAt(
	point: { x: number; y: number } | null,
	rows: number,
	columns: number,
): ImagePixel | null {
	if (!point) return null;
	const column = Math.floor(point.x);
	const row = Math.floor(point.y);
	if (row < 0 || column < 0 || row >= rows || column >= columns) return null;
	return { row, column };
}

function lookup(table: { values: readonly number[] }, index: number): number | null {
	return Number.isInteger(index) && index >= 0 && index < table.values.length ? table.values[index] : null;
}

function clampedLookup(table: ValueLookupTable, stored: number): number | null {
	if (table.values.length === 0 || !Number.isFinite(stored)) return null;
	const index = Math.min(Math.max(Math.round(stored) - table.first_value_mapped, 0), table.values.length - 1);
	return table.values[index];
}

/** The Modality transform the display pipeline applies before windowing. */
export function modalityValue(stored: number, modality: ModalityValueTransform): number | null {
	if (modality.lut) return clampedLookup(modality.lut, stored);
	return stored * modality.rescale_slope + modality.rescale_intercept;
}

/** Whether the Modality transform leaves stored values unchanged. */
export function modalityIsIdentity(modality: ModalityValueTransform): boolean {
	return modality.lut === null && modality.rescale_slope === 1 && modality.rescale_intercept === 0;
}

/**
 * A stored value in real-world units, or null when the value lies outside
 * the mapping's stored range (or a LUT has no entry for it).
 */
export function realWorldValue(stored: number, map: RealWorldValueMap): number | null {
	if (!Number.isFinite(stored)) return null;
	if (map.first_value_mapped !== null && stored < map.first_value_mapped) return null;
	if (map.last_value_mapped !== null && stored > map.last_value_mapped) return null;
	const { transform } = map;
	if (transform.kind === "linear") return stored * transform.slope + transform.intercept;
	if (!Number.isInteger(stored)) return null;
	return lookup(transform, stored - (map.first_value_mapped ?? 0));
}

/** The frame's preferred real-world mapping, if any. */
export function preferredRealWorldMap(mapping: FrameValueMapping | null): RealWorldValueMap | null {
	return mapping?.real_world[0] ?? null;
}

/**
 * Unit text for Modality values: the declared Rescale Type, or `HU` for CT,
 * whose Rescale Type defaults to Hounsfield units.
 */
export function modalityUnit(modality: ModalityValueTransform, fileModality: string): string | null {
	if (modality.rescale_type) return modality.rescale_type;
	return fileModality.trim().toUpperCase() === "CT" ? "HU" : null;
}

const NATIVE_TRANSFER_SYNTAXES = new Set([
	"1.2.840.10008.1.2",
	"1.2.840.10008.1.2.1",
	"1.2.840.10008.1.2.1.99",
	"1.2.840.10008.1.2.2",
]);

/**
 * Native raw frames keep the stored planar order and the raw headers do not
 * say which it is, so the readout reads Planar Configuration for them.
 */
export function rawColorNeedsPlanarConfiguration(transferSyntaxUid: string): boolean {
	return NATIVE_TRANSFER_SYNTAXES.has(transferSyntaxUid);
}

function bytesPerSample(bitsAllocated: number): number | null {
	if (bitsAllocated === 1 || bitsAllocated === 8) return 1;
	if (bitsAllocated === 16 || bitsAllocated === 32 || bitsAllocated === 64) return bitsAllocated / 8;
	return null;
}

function readSample(
	view: DataView,
	byteOffset: number,
	bitsAllocated: number,
	signed: boolean,
	storedValueType: string,
): number | null {
	const size = bytesPerSample(bitsAllocated);
	if (size === null || byteOffset < 0 || byteOffset + size > view.byteLength) return null;
	switch (bitsAllocated) {
		case 1:
		case 8:
			return signed ? view.getInt8(byteOffset) : view.getUint8(byteOffset);
		case 16:
			return signed ? view.getInt16(byteOffset, true) : view.getUint16(byteOffset, true);
		case 32:
			if (storedValueType === "float32") return view.getFloat32(byteOffset, true);
			return signed ? view.getInt32(byteOffset, true) : view.getUint32(byteOffset, true);
		case 64:
			return storedValueType === "float64" ? view.getFloat64(byteOffset, true) : null;
		default:
			return null;
	}
}

/**
 * The stored samples of one pixel of a raw frame (one per component), in
 * the raw endpoint's layout: little-endian, one byte per one-bit sample,
 * interleaved unless `planarConfiguration` is 1, and native YBR_FULL_422
 * kept subsampled (Y0 Y1 Cb Cr per pixel pair). `storedValueType` comes
 * from the frame's value mapping and tells float samples from integers.
 */
export function storedSamplesAt(
	frame: RawFrame,
	pixel: ImagePixel,
	storedValueType: string,
	planarConfiguration = 0,
): number[] | null {
	const { rows, columns, bitsAllocated, pixelRepresentation, samplesPerPixel } = frame.metadata;
	const size = bytesPerSample(bitsAllocated);
	if (size === null || pixel.row >= rows || pixel.column >= columns) return null;
	const view = new DataView(frame.buffer);
	const signed = pixelRepresentation === 1;
	const pixelIndex = pixel.row * columns + pixel.column;
	const pixelCount = rows * columns;

	let sampleIndices: number[];
	if (samplesPerPixel === 1) {
		sampleIndices = [pixelIndex];
	} else if (
		frame.metadata.photometricInterpretation.trim().toUpperCase() === "YBR_FULL_422"
		&& frame.buffer.byteLength === pixelCount * 2 * size
	) {
		const pairStart = pixel.row * columns * 2 + Math.floor(pixel.column / 2) * 4;
		sampleIndices = [pairStart + (pixel.column % 2), pairStart + 2, pairStart + 3];
	} else if (planarConfiguration === 1) {
		sampleIndices = Array.from({ length: samplesPerPixel }, (_, sample) => sample * pixelCount + pixelIndex);
	} else {
		sampleIndices = Array.from({ length: samplesPerPixel }, (_, sample) => pixelIndex * samplesPerPixel + sample);
	}

	const samples: number[] = [];
	for (const index of sampleIndices) {
		const value = readSample(view, index * size, bitsAllocated, signed, storedValueType);
		if (value === null) return null;
		samples.push(value);
	}
	return samples;
}

/** Component names for a color photometric interpretation. */
export function componentLabels(photometricInterpretation: string, count: number): string[] {
	const photometric = photometricInterpretation.trim().toUpperCase();
	if (photometric === "RGB" && count === 3) return ["R", "G", "B"];
	if (photometric.startsWith("YBR") && count === 3) return ["Y", "Cb", "Cr"];
	return Array.from({ length: count }, (_, index) => `C${index + 1}`);
}

/** A readout number: integers as-is, others to five significant digits. */
export function formatValue(value: number): string {
	if (!Number.isFinite(value)) return String(value);
	if (Number.isInteger(value) && Math.abs(value) < 1e9) return String(value);
	return String(Number(value.toPrecision(5)));
}

export type ValueWithUnit = { value: string; unit: string | null; label?: string | null };

/** Which real-world mapping a readout used, out of how many the frame has. */
export type MappingSource = { label: string; detail: string; count: number };

/** A short name for where a real-world mapping comes from, and a longer one. */
export function mappingSource(map: RealWorldValueMap): { label: string; detail: string } {
	switch (map.source) {
		case "dose_grid_scaling":
			return { label: "Dose Grid Scaling", detail: "RT Dose Grid Scaling" };
		case "real_world_value_mapping":
			return { label: "RWVM", detail: "Real World Value Mapping declared in this file" };
		case "rwvm_instance":
			return {
				label: "RWVM instance",
				detail: map.source_file_index === null
					? "Real World Value Mapping instance"
					: `Real World Value Mapping instance, file index ${map.source_file_index}`,
			};
		default:
			return { label: map.source, detail: map.source };
	}
}

/** What the readout shows for one pixel. */
export type PixelValues =
	| {
		kind: "grayscale";
		stored: string;
		modality: ValueWithUnit | null;
		mapped: ValueWithUnit | null;
		mappedOutOfRange: boolean;
		/** The preferred mapping's source; null without a mapping. */
		mappingSource: MappingSource | null;
	}
	| { kind: "color"; components: { label: string; value: string }[] }
	| { kind: "palette"; index: string };

/**
 * Stored, Modality, and preferred real-world values of one pixel. Modality
 * values are omitted when the transform is the identity and has no unit; a
 * real-world mapping whose stored range excludes the pixel reports
 * `mappedOutOfRange`. The preferred (first) mapping is used, and
 * `mappingSource` says which one it is.
 */
export function describePixelValues(
	frame: RawFrame,
	pixel: ImagePixel,
	mapping: FrameValueMapping,
	fileModality: string,
	planarConfiguration = 0,
): PixelValues | null {
	const samples = storedSamplesAt(frame, pixel, mapping.stored_value_type, planarConfiguration);
	if (!samples) return null;
	const photometric = frame.metadata.photometricInterpretation;
	if (samples.length > 1) {
		const labels = componentLabels(photometric, samples.length);
		return {
			kind: "color",
			components: samples.map((sample, index) => ({ label: labels[index], value: formatValue(sample) })),
		};
	}
	const stored = samples[0];
	if (photometric.trim().toUpperCase() === "PALETTE COLOR") {
		return { kind: "palette", index: formatValue(stored) };
	}

	let modality: ValueWithUnit | null = null;
	const unit = modalityUnit(mapping.modality, fileModality);
	if (unit !== null || !modalityIsIdentity(mapping.modality)) {
		const value = modalityValue(stored, mapping.modality);
		if (value !== null) modality = { value: formatValue(value), unit };
	}
	const map = preferredRealWorldMap(mapping);
	const mappedValue = map ? realWorldValue(stored, map) : null;
	return {
		kind: "grayscale",
		stored: formatValue(stored),
		modality,
		mapped: map && mappedValue !== null
			? { value: formatValue(mappedValue), unit: map.unit_label || null, label: map.label }
			: null,
		mappedOutOfRange: map !== null && mappedValue === null,
		mappingSource: map ? { ...mappingSource(map), count: mapping.real_world.length } : null,
	};
}

/**
 * A mapping built from raw-frame headers alone, for when a frame's value
 * mapping fails to load: the header rescale and no real-world mapping.
 * Null for 32-bit samples, which the headers cannot tell float from integer.
 */
export function rawHeaderValueMapping(
	fileIndex: number,
	frameIndex: number,
	frame: RawFrame,
): FrameValueMapping | null {
	const { bitsAllocated, rescaleSlope, rescaleIntercept } = frame.metadata;
	if (bitsAllocated === 32) return null;
	return {
		file_index: fileIndex,
		frame_index: frameIndex,
		stored_value_type: bitsAllocated === 64 ? "float64" : "integer",
		modality: { rescale_slope: rescaleSlope, rescale_intercept: rescaleIntercept, rescale_type: null, lut: null },
		real_world: [],
	};
}

/**
 * A linear conversion between the units the renderer windows (Modality
 * values: stored × slope + intercept) and a real-world unit. Only linear
 * Modality and real-world transforms convert a window exactly; a LUT on
 * either side has no single linear window, so it yields no scale.
 */
export type MappedWindowScale = {
	unit: string;
	label: string | null;
	toMapped: (render: number) => number;
	toRender: (mapped: number) => number;
	/** Mapped units per render unit; negative when the mapping inverts the scale. */
	ratio: number;
};

export function mappedWindowScale(mapping: FrameValueMapping | null): MappedWindowScale | null {
	const map = preferredRealWorldMap(mapping);
	if (!mapping || !map || map.transform.kind !== "linear" || mapping.modality.lut) return null;
	const { rescale_slope: slope, rescale_intercept: intercept } = mapping.modality;
	const { slope: mappedSlope, intercept: mappedIntercept } = map.transform;
	if (!Number.isFinite(slope) || slope === 0 || !Number.isFinite(mappedSlope) || mappedSlope === 0) {
		return null;
	}
	const ratio = mappedSlope / slope;
	return {
		unit: map.unit_label,
		label: map.label,
		ratio,
		toMapped: (render) => ratio * (render - intercept) + mappedIntercept,
		toRender: (mapped) => (mapped - mappedIntercept) / ratio + intercept,
	};
}

export type WindowValues = { center: number; width: number };

export function windowToMapped(window: WindowValues, scale: MappedWindowScale): WindowValues {
	return { center: scale.toMapped(window.center), width: window.width * Math.abs(scale.ratio) };
}

export function windowToRender(window: WindowValues, scale: MappedWindowScale): WindowValues {
	return { center: scale.toRender(window.center), width: window.width / Math.abs(scale.ratio) };
}

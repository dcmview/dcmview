import type { FileSummary, FilesResponse, RawFrame, SeriesCatalogResponse } from "../api";

/** A catalog entry for a single-frame 8-bit image unless overridden. */
export function fileSummary(index: number, overrides: Partial<FileSummary> = {}): FileSummary {
	return {
		index,
		path: `fixtures/image-${index}.dcm`,
		display_name: `image-${index}.dcm`,
		label: `image-${index}.dcm`,
		patient_id: "PATIENT",
		patient_name: "Test^Patient",
		study_instance_uid: "1.2.3",
		study_date: "20260101",
		study_description: "Study",
		series_instance_uid: `1.2.3.${index}`,
		series_number: String(index + 1),
		series_description: `Series ${index}`,
		modality: "CT",
		instance_number: "1",
		sop_instance_uid: `1.2.3.${index}.1`,
		sop_class_uid: "1.2.840.10008.5.1.4.1.1.2",
		object_kind: "classic_image",
		file_format: "dicom",
		raster: null,
		support_state: "renderable",
		support_reason: null,
		raw_windowing_compatible: true,
		raw_windowing_reason: null,
		presentation_layer: false,
		burned_in_annotation: false,
		has_pixels: true,
		frame_count: 1,
		rows: 64,
		columns: 64,
		pixel_aspect_ratio: null,
		transfer_syntax_uid: "1.2.840.10008.1.2.1",
		default_window: { center: 40, width: 400 },
		...overrides,
	};
}

/** A catalog entry for an 8-bit RGB PNG the server lists but does not decode. */
export function rasterSummary(index: number, overrides: Partial<FileSummary> = {}): FileSummary {
	const path = overrides.path ?? `fixtures/image-${index}.png`;
	const name = path.split("/").pop() ?? path;
	return fileSummary(index, {
		path,
		display_name: name,
		label: name,
		patient_id: "",
		patient_name: "",
		study_instance_uid: "",
		study_date: "",
		study_description: "",
		series_instance_uid: "",
		series_number: "",
		series_description: "",
		modality: "",
		instance_number: "",
		sop_instance_uid: "",
		sop_class_uid: "",
		object_kind: "image",
		file_format: "png",
		raster: {
			color_type: "rgb",
			bit_depth: 8,
			sample_format: "uint",
			has_alpha: false,
			orientation: 1,
			has_icc: false,
			pages_total: 1,
			frame_pages: [0],
			excluded_pages: [],
			excluded_pages_total: 0,
			animated: false,
			significant_bits: null,
		},
		support_state: "unsupported",
		support_reason: "raster.decode_not_available",
		transfer_syntax_uid: "",
		default_window: null,
		...overrides,
	});
}

export function filesResponse(files: FileSummary[]): FilesResponse {
	return {
		files,
		discovery: [],
		server_start_ms: 0,
		masked: false,
		scan_complete: true,
		scanned: files.length,
		skipped: 0,
		filtered: 0,
	};
}

export function emptySeriesCatalog(): SeriesCatalogResponse {
	return { series: [], scan_complete: true };
}

/** An 8-bit MONOCHROME2 raw frame; `bitsAllocated: 32` is not browser-renderable. */
export function rawFrame(rows = 64, columns = 64, bitsAllocated = 8): RawFrame {
	return {
		buffer: new ArrayBuffer(rows * columns * (bitsAllocated / 8)),
		metadata: {
			rows,
			columns,
			bitsAllocated,
			pixelRepresentation: 0,
			samplesPerPixel: 1,
			photometricInterpretation: "MONOCHROME2",
			rescaleSlope: 1,
			rescaleIntercept: 0,
			defaultWc: null,
			defaultWw: null,
			paddingLow: null,
			paddingHigh: null,
		},
	};
}

import {
	fetchDoseOverlayBlob,
	fetchDoseOverlayValues,
	fetchParametricMapOverlayBlob,
	fetchParametricMapOverlayValues,
	fetchPresentationLayerBlob,
	fetchSegmentationOverlayBlob,
	type FileSummary,
	type OverlayLegend,
} from "../../api";
import { ByteBudgetLruCache } from "../frameCache";
import { SharedRequestRegistry } from "../keyedAsyncResource";

/**
 * A SEG frame shown as its transparent mask over the one source frame it
 * validly maps to. The logical frame stays the SEG frame; geometry and the
 * base image come from the source.
 */
export type SegmentationOverlay = {
	kind: "segmentation";
	segmentationFileIndex: number;
	segmentationFrameIndex: number;
	sourceFileIndex: number;
	sourceFrameIndex: number;
	sourceFile: FileSummary;
};

/**
 * Layers composed over a source frame's display image, shown in place of
 * the active file's own frame. Value volumes that decorate the displayed
 * frame itself are `ValueOverlay`s instead.
 */
export type FrameOverlay = SegmentationOverlay;

export type ValueOverlayKind = "rt_dose" | "parametric_map";

/**
 * A value volume drawn as a translucent colorwash over the displayed frame.
 * Unlike a SEG overlay the displayed image and its render path stay as they
 * are: the layer sits above whatever the viewport renders, so window/level,
 * cine, and ROIs keep working, and opacity is applied when it is shown.
 */
export type ValueOverlay = {
	kind: ValueOverlayKind;
	volumeFileIndex: number;
	title: string;
	legend: OverlayLegend;
	/** 0..1 */
	opacity: number;
	/** False when the volume's context lists no coverage of the displayed frame. */
	coversFrame: boolean;
};

/** The colorwash of `overlay` resampled onto one displayed frame. */
export function valueOverlayLayerRequest(
	overlay: Pick<ValueOverlay, "kind" | "volumeFileIndex">,
	fileIndex: number,
	frameIndex: number,
): OverlayLayerRequest {
	const { kind, volumeFileIndex } = overlay;
	return {
		key: `${kind}:${volumeFileIndex}:${fileIndex}:${frameIndex}`,
		load: (signal) => kind === "rt_dose"
			? fetchDoseOverlayBlob(fileIndex, frameIndex, volumeFileIndex, signal)
			: fetchParametricMapOverlayBlob(fileIndex, frameIndex, volumeFileIndex, signal),
	};
}

/**
 * A frame's own shutter and overlay graphics, drawn over the frame when the
 * browser windows it (server display frames already carry them).
 */
export function presentationLayerRequest(fileIndex: number, frameIndex: number): OverlayLayerRequest {
	return {
		key: `presentation:${fileIndex}:${frameIndex}`,
		load: (signal) => fetchPresentationLayerBlob(fileIndex, frameIndex, signal),
	};
}

/**
 * The values of `overlay` resampled onto one displayed frame, row-major in
 * the legend's unit, NaN outside the volume; the readout reads them.
 */
export function valueOverlayValuesRequest(
	overlay: Pick<ValueOverlay, "kind" | "volumeFileIndex">,
	fileIndex: number,
	frameIndex: number,
): OverlayRequest<Float32Array> {
	const { kind, volumeFileIndex } = overlay;
	return {
		key: `${kind}:${volumeFileIndex}:${fileIndex}:${frameIndex}:values`,
		load: (signal) => kind === "rt_dose"
			? fetchDoseOverlayValues(fileIndex, frameIndex, volumeFileIndex, signal)
			: fetchParametricMapOverlayValues(fileIndex, frameIndex, volumeFileIndex, signal),
	};
}

/** Encoded colorwash layers kept for revisited frames. */
export const VALUE_OVERLAY_CACHE_BYTES = 32 * 1024 * 1024;

/** Value overlay payloads: one shared request per key, recent ones cached. */
export class OverlayLayerCache<Value extends Blob | Float32Array = Blob> {
	readonly #cache = new ByteBudgetLruCache<string, Value>({
		maxBytes: VALUE_OVERLAY_CACHE_BYTES,
		sizeOf: (value) => (value instanceof Blob ? value.size : value.byteLength),
	});
	readonly #requests = new SharedRequestRegistry<string, Value>();

	load({ key, load }: OverlayRequest<Value>): Promise<Value> {
		const cached = this.#cache.get(key);
		if (cached) return Promise.resolve(cached);
		return this.#requests.request(key, load).then((value) => {
			this.#cache.set(key, value);
			return value;
		});
	}

	/** Aborts layer requests other than `key`'s, such as frames scrolled past. */
	abortOthers(key: string): void {
		this.#requests.abortOthers(key);
	}

	clear(): void {
		this.#requests.abortAll();
		this.#cache.clear();
	}
}

/** CSS colors of a legend's color stops, lowest value first. */
export function legendColors(legend: OverlayLegend): string[] {
	return legend.color_stops.map(([red, green, blue]) => `rgb(${red}, ${green}, ${blue})`);
}

/** One overlay payload, shared and cached under `key`. */
export type OverlayRequest<Value> = {
	key: string;
	load: (signal: AbortSignal) => Promise<Value>;
};

/** One overlay layer image, fetched in the display fetch scope under `key`. */
export type OverlayLayerRequest = OverlayRequest<Blob>;

/** The layer images of `overlay`, bottom to top. */
export function overlayLayerRequests(overlay: FrameOverlay): OverlayLayerRequest[] {
	const { segmentationFileIndex, segmentationFrameIndex } = overlay;
	return [{
		key: `seg:${segmentationFileIndex}:${segmentationFrameIndex}`,
		load: (signal) => fetchSegmentationOverlayBlob(segmentationFileIndex, segmentationFrameIndex, signal),
	}];
}

export type DecodedCanvasImage = {
	source: CanvasImageSource;
	width: number;
	height: number;
	dispose: () => void;
};

/** Decodes a PNG blob for drawing, via ImageBitmap when available. */
export async function decodeCanvasImage(blob: Blob): Promise<DecodedCanvasImage> {
	if (typeof createImageBitmap === "function") {
		const bitmap = await createImageBitmap(blob);
		return {
			source: bitmap,
			width: bitmap.width,
			height: bitmap.height,
			dispose: () => bitmap.close(),
		};
	}

	const url = URL.createObjectURL(blob);
	const image = new Image();
	image.decoding = "async";
	try {
		await new Promise<void>((resolve, reject) => {
			image.onload = () => resolve();
			image.onerror = () => reject(new Error("overlay image decode failed"));
			image.src = url;
		});
		return {
			source: image,
			width: image.naturalWidth,
			height: image.naturalHeight,
			dispose: () => URL.revokeObjectURL(url),
		};
	} catch (error) {
		URL.revokeObjectURL(url);
		throw error;
	}
}

/** Replaces a transparent layer canvas's contents with `layer` at its own size. */
export function drawOverlayLayer(canvas: HTMLCanvasElement, layer: DecodedCanvasImage): void {
	canvas.width = layer.width;
	canvas.height = layer.height;
	const ctx = canvas.getContext("2d");
	if (!ctx) throw new Error("2D canvas is unavailable");
	ctx.clearRect(0, 0, layer.width, layer.height);
	ctx.drawImage(layer.source, 0, 0);
}

/** Draws `base` at its own size, then each layer stretched over it. */
export function composeOverlayFrame(
	canvas: HTMLCanvasElement,
	base: DecodedCanvasImage,
	layers: readonly DecodedCanvasImage[],
): void {
	canvas.width = base.width;
	canvas.height = base.height;
	const ctx = canvas.getContext("2d", { alpha: false });
	if (!ctx) throw new Error("2D canvas is unavailable");
	ctx.drawImage(base.source, 0, 0);
	for (const layer of layers) ctx.drawImage(layer.source, 0, 0, base.width, base.height);
}

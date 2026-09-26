import { fetchSegmentationOverlayBlob, type FileSummary } from "../../api";

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
 * Layers composed over a source frame's display image. Parametric Map and RT
 * Dose colorwash overlays are planned as further members of this union: each
 * names its source frame and the layer images drawn over it.
 */
export type FrameOverlay = SegmentationOverlay;

/** One overlay layer image, fetched in the display fetch scope under `key`. */
export type OverlayLayerRequest = {
	key: string;
	load: (signal: AbortSignal) => Promise<Blob>;
};

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

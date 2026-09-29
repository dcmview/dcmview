import { isApiError, type FileSummary } from "../../api";
import {
	decodeCanvasImage, OverlayLayerCache, presentationLayerRequest, valueOverlayLayerRequest,
	type DecodedCanvasImage, type ValueOverlay,
} from "./frameOverlay";

export type LayerStatus = "none" | "shown" | "not_covering" | "error";
export type PreparedLayer = { key: string; status: LayerStatus; image: DecodedCanvasImage | null };
export type PreparedFrameLayers = {
	presentation: PreparedLayer;
	value: PreparedLayer;
	dispose: () => void;
};
type LayerPayload = { key: string; status: LayerStatus; blob: Blob | null };
const NONE: LayerPayload = { key: "", status: "none", blob: null };

/** Payloads are prefetched and cached; decoded layers belong to one prepared frame. */
export class FrameLayers {
	readonly #values = new OverlayLayerCache();
	readonly #presentations = new OverlayLayerCache();

	/** Unknown coverage on a prefetched frame is answered by the overlay endpoint. */
	async value(overlay: ValueOverlay | null, fileIndex: number, frameIndex: number, covers?: boolean, signal?: AbortSignal): Promise<LayerPayload> {
		if (!overlay) return NONE;
		const request = valueOverlayLayerRequest(overlay, fileIndex, frameIndex);
		if (covers === false) return { key: request.key, status: "not_covering", blob: null };
		try {
			return { key: request.key, status: "shown", blob: await this.#values.load(request, signal) };
		} catch (error) {
			if ((error as Error).name === "AbortError") throw error;
			return { key: request.key, status: isApiError(error, "overlay_not_covering_frame") ? "not_covering" : "error", blob: null };
		}
	}

	async #presentation(file: FileSummary, frameIndex: number, raw: boolean): Promise<LayerPayload> {
		if (!raw || !file.presentation_layer) return NONE;
		const request = presentationLayerRequest(file.index, frameIndex);
		try {
			return { key: request.key, status: "shown", blob: await this.#presentations.load(request) };
		} catch (error) {
			if ((error as Error).name === "AbortError") throw error;
			return { key: request.key, status: "error", blob: null };
		}
	}

	async prepare(file: FileSummary, frameIndex: number, raw: boolean, overlay: ValueOverlay | null): Promise<PreparedFrameLayers> {
		const payloads = await Promise.all([
			this.#presentation(file, frameIndex, raw),
			this.value(overlay, file.index, frameIndex, overlay?.coversFrame),
		]);
		const [presentation, value] = await Promise.all(payloads.map(async ({ key, status, blob }): Promise<PreparedLayer> => {
			if (!blob) return { key, status, image: null };
			try { return { key, status, image: await decodeCanvasImage(blob) }; }
			catch { return { key, status: "error", image: null }; }
		}));
		return { presentation, value, dispose: () => { presentation.image?.dispose(); value.image?.dispose(); } };
	}

	clear(): void {
		this.#values.clear();
		this.#presentations.clear();
	}
}

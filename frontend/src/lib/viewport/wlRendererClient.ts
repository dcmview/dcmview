import type { RawFrame } from "../../rawFrame";
import { renderRawFrameToRgba } from "../rawWindowing";
import type {
	WlRendererRequest,
	WlRendererResponse,
	WlRendererSuccess,
} from "../workers/wlRendererProtocol";

/** Frames at least this large are windowed off the main thread. */
export const WORKER_MIN_PIXEL_THRESHOLD = 300_000;

export type WlRenderRequest = {
	frame: RawFrame;
	wc: number;
	ww: number;
	/** False once a newer frame, file, or pipeline supersedes this render. */
	isCurrent: () => boolean;
};

export type WlRendererClientOptions = {
	createWorker?: () => Worker;
	minWorkerPixels?: number;
};

type RenderedBitmap = Pick<WlRendererSuccess, "width" | "height" | "bitmap">;

function createWlWorker(): Worker {
	return new Worker(new URL("../workers/wlRenderer.worker.ts", import.meta.url), { type: "module" });
}

/** Windows a raw frame into `canvas` on the main thread. */
export function drawRawFrame(canvas: HTMLCanvasElement, frame: RawFrame, wc: number, ww: number): void {
	const { rows, columns } = frame.metadata;
	canvas.width = columns;
	canvas.height = rows;
	const ctx = canvas.getContext("2d", { alpha: false });
	if (!ctx) return;
	const imageData = ctx.createImageData(columns, rows);
	imageData.data.set(renderRawFrameToRgba(frame, wc, ww));
	ctx.putImageData(imageData, 0, 0);
}

/**
 * Client-side window/level rendering. Large frames go to a worker that keeps
 * the current frame, so a window drag sends only {wc, ww}; at most one worker
 * render is in flight and newer requests replace the queued one, so a drag
 * never builds a backlog. Small frames, and every frame after a worker
 * failure, render on the main thread.
 */
export class WlRendererClient {
	readonly #createWorker: () => Worker;
	readonly #minWorkerPixels: number;
	#worker: Worker | null = null;
	#initAttempted = false;
	#available = false;
	#messageId = 0;
	readonly #pending = new Map<number, {
		resolve: (value: RenderedBitmap) => void;
		reject: (error: Error) => void;
	}>();
	// The raw frame the worker holds; window changes send only {wc, ww}.
	#workerFrame: RawFrame | null = null;
	#workerFrameId = 0;
	#renderInFlight = false;
	#queued: WlRenderRequest | null = null;

	constructor({
		createWorker = createWlWorker,
		minWorkerPixels = WORKER_MIN_PIXEL_THRESHOLD,
	}: WlRendererClientOptions = {}) {
		this.#createWorker = createWorker;
		this.#minWorkerPixels = minWorkerPixels;
	}

	/** Renders `request` into the canvas `target` returns when it completes. */
	async render(target: () => HTMLCanvasElement | undefined, request: WlRenderRequest): Promise<void> {
		const canvas = target();
		if (!canvas) return;
		if (!this.#shouldUseWorker(request.frame)) {
			drawRawFrame(canvas, request.frame, request.wc, request.ww);
			return;
		}

		this.#queued = request;
		if (this.#renderInFlight) return;
		this.#renderInFlight = true;
		try {
			while (this.#queued) {
				const next = this.#queued;
				this.#queued = null;
				const bitmap = await this.#renderInWorker(next.frame, next.wc, next.ww);
				const current = target();
				if (!next.isCurrent() || !current) {
					bitmap.close();
					continue;
				}
				current.width = bitmap.width;
				current.height = bitmap.height;
				current.getContext("2d", { alpha: false })?.drawImage(bitmap, 0, 0);
				bitmap.close();
			}
		} catch {
			this.#available = false;
			const latest = this.#queued ?? request;
			this.#queued = null;
			const current = target();
			if (!latest.isCurrent() || !current) return;
			drawRawFrame(current, latest.frame, latest.wc, latest.ww);
		} finally {
			this.#renderInFlight = false;
		}
	}

	dispose(): void {
		this.#rejectPending("viewport disposed");
		this.#worker?.terminate();
		this.#worker = null;
		this.#workerFrame = null;
		this.#queued = null;
	}

	#shouldUseWorker(frame: RawFrame): boolean {
		const pixels = frame.metadata.rows * frame.metadata.columns;
		return pixels >= this.#minWorkerPixels && this.#ensureWorker();
	}

	#ensureWorker(): boolean {
		if (this.#initAttempted) return this.#available;
		this.#initAttempted = true;
		try {
			const worker = this.#createWorker();
			worker.onmessage = (event: MessageEvent<WlRendererResponse>) => this.#settle(event.data);
			worker.onerror = () => {
				this.#available = false;
				this.#workerFrame = null;
				this.#rejectPending("window/level worker failed");
			};
			this.#worker = worker;
			this.#available = true;
		} catch {
			this.#available = false;
			this.#worker = null;
		}
		return this.#available;
	}

	#settle(payload: WlRendererResponse): void {
		const pending = this.#pending.get(payload.id);
		if (!pending) return;
		this.#pending.delete(payload.id);
		if (payload.type === "error") {
			pending.reject(new Error(payload.message));
		} else {
			pending.resolve({ width: payload.width, height: payload.height, bitmap: payload.bitmap });
		}
	}

	#rejectPending(message: string): void {
		for (const pending of this.#pending.values()) pending.reject(new Error(message));
		this.#pending.clear();
	}

	async #renderInWorker(frame: RawFrame, wc: number, ww: number): Promise<ImageBitmap> {
		const worker = this.#worker;
		if (!worker || !this.#available) throw new Error("worker unavailable");
		if (this.#workerFrame !== frame) {
			// The main thread keeps its buffer for the raw-frame cache, so the
			// worker gets a copy, once per frame rather than once per render.
			const buffer = frame.buffer.slice(0);
			const load: WlRendererRequest = {
				type: "frame",
				frameId: ++this.#workerFrameId,
				metadata: frame.metadata,
				buffer,
			};
			worker.postMessage(load, [buffer]);
			this.#workerFrame = frame;
		}
		const id = ++this.#messageId;
		const pending = new Promise<RenderedBitmap>((resolve, reject) => {
			this.#pending.set(id, { resolve, reject });
		});
		const request: WlRendererRequest = { type: "render", id, frameId: this.#workerFrameId, wc, ww };
		worker.postMessage(request);
		return (await pending).bitmap;
	}
}

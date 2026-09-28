import { fetchRawFrame, type RawFrame } from "../../api";
import { ByteBudgetLruCache } from "../frameCache";
import { SharedRequestRegistry } from "../keyedAsyncResource";
import { buildDirectionalFrameOrder } from "../prefetchPolicy";
import { validateRenderableRawFrame } from "../rawWindowing";
import { framesNear, type NavigationFrameRef } from "../seriesNavigation";

export const RAW_CACHE_BYTE_BUDGET = 256 * 1024 * 1024;
/** Frames prefetched on each side of the current position. */
export const RAW_RING_RADIUS = 10;

export type RawFrameSourceOptions = {
	load?: (fileIndex: number, frameIndex: number, signal: AbortSignal) => Promise<RawFrame>;
	maxBytes?: number;
	/** Parallel prefetch requests, read before every batch. */
	concurrency: () => number;
};

function rawFrameKey(fileIndex: number, frameIndex: number): string {
	return `${fileIndex}:${frameIndex}`;
}

/**
 * Decoded raw samples for client-side windowing, keyed by source file/frame.
 * Foreground and prefetch consumers share one in-flight request per frame;
 * only frames the browser renderer accepts are cached, within a byte budget.
 * The pixel readout reads the samples under the cursor from here too.
 */
export class RawFrameSource {
	readonly #load: (fileIndex: number, frameIndex: number, signal: AbortSignal) => Promise<RawFrame>;
	readonly #concurrency: () => number;
	readonly #cache: ByteBudgetLruCache<string, RawFrame>;
	readonly #requests = new SharedRequestRegistry<string, RawFrame>();
	#prefetch: AbortController | null = null;

	constructor({ load = fetchRawFrame, maxBytes = RAW_CACHE_BYTE_BUDGET, concurrency }: RawFrameSourceOptions) {
		this.#load = load;
		this.#concurrency = concurrency;
		this.#cache = new ByteBudgetLruCache<string, RawFrame>({
			maxBytes,
			sizeOf: (frame) => frame.buffer.byteLength,
		});
	}

	cached(fileIndex: number, frameIndex: number): RawFrame | undefined {
		return this.#cache.get(rawFrameKey(fileIndex, frameIndex));
	}

	/** Caches a frame that passed renderer validation. */
	store(fileIndex: number, frameIndex: number, frame: RawFrame): void {
		this.#cache.set(rawFrameKey(fileIndex, frameIndex), frame);
	}

	/** The cached frame, or the shared request for it; does not cache the result. */
	ensure(fileIndex: number, frameIndex: number): Promise<RawFrame> {
		const key = rawFrameKey(fileIndex, frameIndex);
		const cached = this.#cache.get(key);
		if (cached) return Promise.resolve(cached);
		return this.#requests.request(key, (signal) => this.#load(fileIndex, frameIndex, signal));
	}

	inFlight(fileIndex: number, frameIndex: number): Promise<RawFrame> | undefined {
		return this.#requests.get(rawFrameKey(fileIndex, frameIndex));
	}

	/** Warms the ring around `position`, nearest first in `direction`, replacing any earlier prefetch. */
	prefetch(frames: readonly NavigationFrameRef[], position: number, direction: 1 | -1): void {
		this.#prefetch?.abort();
		const ctrl = new AbortController();
		this.#prefetch = ctrl;
		void this.#runPrefetch(frames, position, direction, ctrl.signal);
	}

	stopPrefetch(): void {
		this.#prefetch?.abort();
		this.#prefetch = null;
	}

	/**
	 * Aborts requests for frames beyond the prefetch ring around `position`:
	 * scrubbing past them leaves them nobody to serve, and they would hold the
	 * browser's few connections ahead of the frame now wanted.
	 */
	abortFar(frames: readonly NavigationFrameRef[], position: number): void {
		const near = framesNear(frames, position, RAW_RING_RADIUS);
		this.#requests.abortWhere((key) => !near.has(key));
	}

	/** Aborts all work; cached frames stay for a later return. */
	abortAll(): void {
		this.stopPrefetch();
		this.#requests.abortAll();
	}

	/** Aborts all work and drops every cached frame. */
	clear(): void {
		this.stopPrefetch();
		this.#requests.abortAll();
		this.#cache.clear();
	}

	async #runPrefetch(
		frames: readonly NavigationFrameRef[],
		centerPosition: number,
		direction: 1 | -1,
		signal: AbortSignal,
	): Promise<void> {
		const targets = buildDirectionalFrameOrder(centerPosition, frames.length, RAW_RING_RADIUS, direction);
		for (let i = 0; i < targets.length && !signal.aborted;) {
			const concurrency = this.#concurrency();
			const batch = targets
				.slice(i, i + concurrency)
				.map((position) => frames[position])
				.filter((frame): frame is NavigationFrameRef => frame !== undefined)
				.filter((frame) => !this.#cache.has(rawFrameKey(frame.file_index, frame.frame_index)));
			i += concurrency;
			if (batch.length === 0) continue;
			await Promise.allSettled(batch.map(async (frame) => {
				if (signal.aborted || this.#cache.has(rawFrameKey(frame.file_index, frame.frame_index))) return;
				try {
					const rawFrame = await this.ensure(frame.file_index, frame.frame_index);
					if (signal.aborted || validateRenderableRawFrame(rawFrame) !== null) return;
					this.store(frame.file_index, frame.frame_index, rawFrame);
				} catch {
					// Ignore network/decode failures during prefetch.
				}
			}));
		}
	}
}

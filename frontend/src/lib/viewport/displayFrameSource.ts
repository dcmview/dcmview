import {
	displayFrameCacheKey,
	displayFrameWindowCacheKey,
	fetchDisplayFrameBlob,
	type DisplayFrameWindowOptions,
} from "../../api";
import type { CineMode } from "../cinePlayback";
import { createDisplayFrameCaches } from "../frameCache";
import { SharedRequestRegistry } from "../keyedAsyncResource";
import { planDisplayPrefetchTargets } from "../prefetchPolicy";
import type { NavigationFrameRef } from "../seriesNavigation";
import { scheduleIdle } from "./prefetchScheduling";

export const DISPLAY_BLOB_CACHE_BYTE_BUDGET = 320 * 1024 * 1024;
export const DISPLAY_BITMAP_CACHE_BYTE_BUDGET = 128 * 1024 * 1024;
const DISPLAY_FULL_PREFETCH_BUDGET_BYTES = 320 * 1024 * 1024;
const DISPLAY_NEAR_PREFETCH_DISTANCE = 48;
/**
 * A scope is prefetched near the current frame at first and as a whole stack
 * once the viewer has stayed on it this long, or as soon as cine plays, so a
 * glance at a large stack costs its neighbourhood rather than every frame.
 */
export const FULL_STACK_PREFETCH_DWELL_MS = 1500;
const CINE_LOOKAHEAD_FRAMES = 16;
/** A still-running prefetch seeded this close to the new position is kept. */
const PREFETCH_RESEED_DISTANCE = 6;

export type DisplayFrameSourceOptions = {
	load?: typeof fetchDisplayFrameBlob;
	/** The navigation scope (open tab) whose frames are being fetched. */
	navigationScope: () => string;
	/** Parallel prefetch requests. */
	concurrency: () => number;
	/** Called when a new fetch scope starts; rendered-frame state is stale. */
	onScopeChange: () => void;
};

type PrefetchRun = { ctrl: AbortController; scopeKey: string; seedPosition: number; fullStack: boolean };

/**
 * Server-rendered display PNGs for the cine and server window/level paths.
 *
 * Requests belong to a fetch scope (navigation scope plus window options):
 * entering a new scope aborts the previous scope's requests and prefetch,
 * while navigation inside a scope never cancels reusable work. Payloads and
 * decoded bitmaps live in independent byte-budgeted LRU tiers that survive
 * scope changes until `clear`.
 */
export class DisplayFrameSource {
	readonly #load: typeof fetchDisplayFrameBlob;
	readonly #navigationScope: () => string;
	readonly #concurrency: () => number;
	readonly #onScopeChange: () => void;
	readonly #caches = createDisplayFrameCaches(DISPLAY_BLOB_CACHE_BYTE_BUDGET, DISPLAY_BITMAP_CACHE_BYTE_BUDGET);
	readonly #requests = new SharedRequestRegistry<string, Blob>();
	// Keyed by payload identity, so a refetched payload never reuses an
	// older payload's decode.
	readonly #decodes = new SharedRequestRegistry<Blob, ImageBitmap>();
	#scopeKey: string | null = null;
	#scopeEnteredAt = 0;
	#prefetch: PrefetchRun | null = null;
	#widen: ReturnType<typeof setTimeout> | null = null;
	/** Where navigation last seeded a prefetch; the widened prefetch starts there. */
	#lastSeed = 0;

	constructor({ load = fetchDisplayFrameBlob, navigationScope, concurrency, onScopeChange }: DisplayFrameSourceOptions) {
		this.#load = load;
		this.#navigationScope = navigationScope;
		this.#concurrency = concurrency;
		this.#onScopeChange = onScopeChange;
	}

	key(fileIndex: number, frameIndex: number, options: DisplayFrameWindowOptions): string {
		return displayFrameCacheKey(fileIndex, frameIndex, options);
	}

	/** Starts the fetch scope for `options`, aborting a different scope's work. */
	enterScope(options: DisplayFrameWindowOptions): void {
		const scopeKey = this.#fetchScope(options);
		if (this.#scopeKey === scopeKey) return;
		this.#requests.abortAll();
		this.stopPrefetch();
		this.#scopeKey = scopeKey;
		this.#scopeEnteredAt = performance.now();
		this.#onScopeChange();
	}

	/** The cached payload, or the scope's shared request for it. */
	ensureBlob(fileIndex: number, frameIndex: number, options: DisplayFrameWindowOptions): Promise<Blob> {
		const key = this.key(fileIndex, frameIndex, options);
		const cached = this.#caches.blobs.get(key);
		if (cached) return Promise.resolve(cached);
		const existing = this.#requests.get(key);
		if (existing) return existing;
		this.enterScope(options);
		return this.#requests.request(key, (signal) => this.#load(fileIndex, frameIndex, options, signal)
			.then((blob) => {
				if (!this.#caches.blobs.set(key, blob)) throw new Error("Display frame exceeded PNG cache budget");
				return this.#caches.blobs.get(key) ?? blob;
			}));
	}

	/**
	 * Shares an uncached fetch (such as an overlay layer) that belongs to the
	 * current scope and is aborted with it.
	 */
	fetchInScope(key: string, options: DisplayFrameWindowOptions, load: (signal: AbortSignal) => Promise<Blob>): Promise<Blob> {
		this.enterScope(options);
		return this.#requests.request(key, load);
	}

	inFlight(key: string): Promise<Blob> | undefined {
		return this.#requests.get(key);
	}

	/** Decodes `blob` into the bitmap tier, reusing a cached or in-flight decode. */
	decode(key: string, blob: Blob): Promise<ImageBitmap> {
		const cached = this.#caches.bitmaps.get(key);
		if (cached) return Promise.resolve(cached);
		return this.#decodes.request(blob, () => createImageBitmap(blob).then((bitmap) => {
			if (this.#caches.blobs.peek(key) === blob) {
				if (this.#caches.bitmaps.set(key, bitmap)) return bitmap;
				throw new Error("Decoded display frame exceeded bitmap cache budget");
			}
			bitmap.close();
			throw new Error("display image decode superseded");
		}));
	}

	/**
	 * Prefetches payloads around `position`. During cine the playback order is
	 * followed immediately; otherwise an idle-time prefetch is (re)seeded unless
	 * one for the same scope and extent is still running nearby. Before the
	 * scope's dwell has passed only the neighbourhood is fetched, and the
	 * prefetch widens to the whole stack when it does.
	 */
	startPrefetch(
		frames: readonly NavigationFrameRef[],
		position: number,
		direction: 1 | -1,
		options: DisplayFrameWindowOptions,
		currentBlobSize: number,
		cineMode: CineMode | null,
	): void {
		const dwellLeft = FULL_STACK_PREFETCH_DWELL_MS - (performance.now() - this.#scopeEnteredAt);
		this.#lastSeed = position;
		this.#seedPrefetch(frames, position, direction, options, currentBlobSize, cineMode, cineMode !== null || dwellLeft <= 0);
		if (cineMode === null && dwellLeft > 0 && this.#prefetch?.fullStack === false && this.#widen === null) {
			const scopeKey = this.#fetchScope(options);
			this.#widen = setTimeout(() => {
				this.#widen = null;
				if (this.#fetchScope(options) === scopeKey) {
					this.#seedPrefetch(frames, this.#lastSeed, direction, options, currentBlobSize, null, true);
				}
			}, dwellLeft);
		}
	}

	#seedPrefetch(
		frames: readonly NavigationFrameRef[],
		position: number,
		direction: 1 | -1,
		options: DisplayFrameWindowOptions,
		currentBlobSize: number,
		cineMode: CineMode | null,
		fullStack: boolean,
	): void {
		const scopeKey = this.#fetchScope(options);
		if (cineMode === null) {
			const running = this.#prefetch;
			if (
				running
				&& !running.ctrl.signal.aborted
				&& running.scopeKey === scopeKey
				&& running.fullStack === fullStack
				&& Math.abs(position - running.seedPosition) <= PREFETCH_RESEED_DISTANCE
			) return;
		}

		this.#prefetch?.ctrl.abort();
		const run: PrefetchRun = { ctrl: new AbortController(), scopeKey, seedPosition: position, fullStack };
		this.#prefetch = run;
		const start = () => {
			if (this.#prefetch !== run) return;
			void this.#runPrefetch(frames, position, direction, options, run.ctrl.signal, currentBlobSize, cineMode, fullStack)
				.finally(() => {
					if (this.#prefetch === run) this.#prefetch = null;
				});
		};
		if (cineMode === null) scheduleIdle(start);
		else start();
	}

	stopPrefetch(): void {
		this.#prefetch?.ctrl.abort();
		this.#prefetch = null;
		if (this.#widen !== null) clearTimeout(this.#widen);
		this.#widen = null;
	}

	/** Aborts the current scope's work; the next request starts a new scope. */
	resetScope(): void {
		this.#requests.abortAll();
		this.stopPrefetch();
		this.#scopeKey = null;
	}

	/** Aborts all work and drops every cached payload and bitmap. */
	clear(): void {
		this.resetScope();
		this.#decodes.abortAll();
		this.#caches.blobs.clear();
		this.#caches.bitmaps.clear();
		this.#onScopeChange();
	}

	#fetchScope(options: DisplayFrameWindowOptions): string {
		return `${this.#navigationScope()}:${displayFrameWindowCacheKey(options)}`;
	}

	async #runPrefetch(
		frames: readonly NavigationFrameRef[],
		startPosition: number,
		direction: 1 | -1,
		options: DisplayFrameWindowOptions,
		signal: AbortSignal,
		currentBlobSize: number,
		cineMode: CineMode | null,
		fullStack: boolean,
	): Promise<void> {
		const targets = planDisplayPrefetchTargets({
			startFrame: startPosition,
			totalFrames: frames.length,
			direction,
			currentPayloadBytes: currentBlobSize,
			fullStackBudgetBytes: fullStack ? DISPLAY_FULL_PREFETCH_BUDGET_BYTES : 0,
			nearDistance: DISPLAY_NEAR_PREFETCH_DISTANCE,
			cineMode,
			lookaheadFrames: CINE_LOOKAHEAD_FRAMES,
		});
		for (let i = 0; i < targets.length && !signal.aborted;) {
			const concurrency = this.#concurrency();
			const batch = targets.slice(i, i + concurrency);
			i += concurrency;
			await Promise.allSettled(batch.map(async (position) => {
				const frame = frames[position];
				if (!frame) return;
				const key = this.key(frame.file_index, frame.frame_index, options);
				if (signal.aborted || this.#caches.blobs.has(key)) return;
				try {
					await this.ensureBlob(frame.file_index, frame.frame_index, options);
				} catch {
					// Ignore network/decode failures during prefetch.
				}
			}));
		}
	}
}

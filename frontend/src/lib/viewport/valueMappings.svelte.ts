import { fetchFrameValueMapping, type FrameValueMapping } from "../../api";
import {
	METADATA_SETTLE_MS,
	KeyedAsyncResource,
	type AsyncResourceSnapshot,
} from "../keyedAsyncResource";

/** Frame value mappings kept; each is small unless it carries a LUT. */
export const VALUE_MAPPING_CACHE_FRAMES = 128;

type FrameKey = `${number}:${number}`;

function frameKey(fileIndex: number, frameIndex: number): FrameKey {
	return `${fileIndex}:${frameIndex}`;
}

function parseFrameKey(key: FrameKey): [number, number] {
	const [fileIndex, frameIndex] = key.split(":").map(Number);
	return [fileIndex, frameIndex];
}

/**
 * Per-frame value mappings (`value-mapping`): how stored samples convert to
 * Modality and real-world values. One shared request per frame; settled
 * values are kept reactively for the readout and mapped-unit windowing.
 */
export class ValueMappings {
	// Raw: mappings are replaced, never mutated, and their value maps are
	// posted to the W/L worker, which cannot clone proxies.
	#snapshots = $state.raw<Record<FrameKey, AsyncResourceSnapshot<FrameValueMapping> | undefined>>({});
	/** The most recent mapping loaded for each file, while a frame's own loads. */
	#latestByFile = $state.raw<Record<number, FrameValueMapping | undefined>>({});
	readonly #resource: KeyedAsyncResource<FrameKey, FrameValueMapping>;
	readonly #consumers = new Map<FrameKey, Set<symbol>>();

	constructor(load: typeof fetchFrameValueMapping = fetchFrameValueMapping) {
		this.#resource = new KeyedAsyncResource<FrameKey, FrameValueMapping>({
			load: (key, signal) => {
				const [fileIndex, frameIndex] = parseFrameKey(key);
				return load(fileIndex, frameIndex, signal);
			},
			capacity: VALUE_MAPPING_CACHE_FRAMES,
			onChange: (key, snapshot) => {
				const { [key]: _previous, ...rest } = this.#snapshots;
				this.#snapshots = snapshot.status === "idle" ? rest : { ...rest, [key]: snapshot };
				if (snapshot.status === "ready" && snapshot.value) {
					this.#latestByFile = { ...this.#latestByFile, [snapshot.value.file_index]: snapshot.value };
				}
			},
		});
	}

	/** This frame's mapping once loaded. */
	get(fileIndex: number, frameIndex: number): FrameValueMapping | null {
		const snapshot = this.#snapshots[frameKey(fileIndex, frameIndex)];
		return snapshot?.status === "ready" ? snapshot.value ?? null : null;
	}

	/** This frame's mapping, or else the last one loaded for the same file. */
	forFrame(fileIndex: number, frameIndex: number): FrameValueMapping | null {
		return this.get(fileIndex, frameIndex) ?? this.#latestByFile[fileIndex] ?? null;
	}

	/** Whether any mapping of this file has loaded. */
	knownForFile(fileIndex: number): boolean {
		return this.#latestByFile[fileIndex] !== undefined;
	}

	failed(fileIndex: number, frameIndex: number): boolean {
		return this.#snapshots[frameKey(fileIndex, frameIndex)]?.status === "error";
	}

	/** Loads this frame's mapping now. */
	ensure(fileIndex: number, frameIndex: number): void {
		void this.load(fileIndex, frameIndex);
	}

	/** Share a mapping until all its consumers cancel or the request settles. */
	load(fileIndex: number, frameIndex: number, signal?: AbortSignal): Promise<FrameValueMapping | null> {
		if (signal?.aborted) return Promise.resolve(null);
		const key = frameKey(fileIndex, frameIndex);
		const consumers = this.#consumers.get(key) ?? new Set<symbol>();
		const consumer = Symbol();
		consumers.add(consumer);
		this.#consumers.set(key, consumers);
		return new Promise((resolve) => {
			let settled = false;
			const finish = (value: FrameValueMapping | null, cancelled = false) => {
				if (settled) return;
				settled = true;
				signal?.removeEventListener("abort", abort);
				consumers.delete(consumer);
				if (consumers.size === 0) {
					this.#consumers.delete(key);
					if (cancelled && this.#resource.get(key).status === "loading") this.#resource.invalidate(key);
				}
				resolve(value);
			};
			const abort = () => finish(null, true);
			signal?.addEventListener("abort", abort, { once: true });
			void this.#resource.ensure(key).then((value) => finish(value), () => finish(null));
		});
	}

	/**
	 * Loads this frame's mapping once it stays current for the settle delay,
	 * so cine and fast scrolling skip passing frames. Returns the cleanup.
	 */
	ensureWhenSettled(fileIndex: number, frameIndex: number): (() => void) | undefined {
		const controller = new AbortController();
		const load = () => void this.load(fileIndex, frameIndex, controller.signal);
		const timer = this.get(fileIndex, frameIndex) ? (load(), undefined) : setTimeout(load, METADATA_SETTLE_MS);
		return () => {
			clearTimeout(timer);
			controller.abort();
		};
	}
}

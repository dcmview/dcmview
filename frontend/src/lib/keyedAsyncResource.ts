export type AsyncResourceStatus = "idle" | "loading" | "ready" | "error";

export type AsyncResourceSnapshot<Value> = {
	status: AsyncResourceStatus;
	value: Value | undefined;
	error: string | null;
	generation: number;
};

export type KeyedAsyncResourceOptions<Key, Value> = {
	load: (key: Key, signal: AbortSignal) => Promise<Value>;
	onChange?: (key: Key, snapshot: AsyncResourceSnapshot<Value>) => void;
	errorMessage?: (error: unknown) => string;
	/** Most keys whose settled values are kept; older ones return to idle. */
	capacity?: number;
};

type InFlight<Value> = {
	generation: number;
	promise: Promise<Value>;
	controller: AbortController;
};

const IDLE = { status: "idle", value: undefined, error: null } as const;

/**
 * How long a side-panel key must stay selected before its metadata loads.
 * Cine playback moves through slices faster than this, so passing slices
 * never cost a server request; a paused or clicked slice loads promptly.
 */
export const METADATA_SETTLE_MS = 150;

/** Settled values kept per side panel. */
export const METADATA_CACHE_FILES = 32;

function defaultErrorMessage(error: unknown): string {
	return error instanceof Error && error.message ? error.message : String(error);
}

/**
 * Keeps independent async state by logical key and rejects stale completions
 * using a monotonically increasing generation for each key.
 */
export class KeyedAsyncResource<Key, Value> {
	readonly #load: (key: Key, signal: AbortSignal) => Promise<Value>;
	readonly #onChange?: (key: Key, snapshot: AsyncResourceSnapshot<Value>) => void;
	readonly #errorMessage: (error: unknown) => string;
	readonly #capacity: number;
	// Insertion order doubles as recency: keys are re-inserted when used.
	readonly #states = new Map<Key, AsyncResourceSnapshot<Value>>();
	readonly #inFlight = new Map<Key, InFlight<Value>>();

	constructor({
		load,
		onChange,
		errorMessage = defaultErrorMessage,
		capacity = Number.POSITIVE_INFINITY,
	}: KeyedAsyncResourceOptions<Key, Value>) {
		this.#load = load;
		this.#onChange = onChange;
		this.#errorMessage = errorMessage;
		this.#capacity = capacity;
	}

	get(key: Key): AsyncResourceSnapshot<Value> {
		return this.#states.get(key) ?? {
			status: "idle",
			value: undefined,
			error: null,
			generation: 0,
		};
	}

	ensure(key: Key): Promise<Value> {
		const state = this.#states.get(key);
		if (state?.status === "ready" && state.value !== undefined) {
			this.#touch(key, state);
			return Promise.resolve(state.value);
		}
		const pending = this.#inFlight.get(key);
		if (pending) return pending.promise;
		return this.#start(key);
	}

	reload(key: Key): Promise<Value> {
		return this.#start(key);
	}

	invalidate(key: Key): void {
		this.#inFlight.get(key)?.controller.abort();
		this.#inFlight.delete(key);
		this.#setIdle(key);
	}

	/**
	 * Abort every in-flight load except `key`'s. Views that show one key at a
	 * time call this so skipped keys stop costing server work.
	 */
	abortOthers(key: Key): void {
		for (const [other, inFlight] of [...this.#inFlight]) {
			if (other === key) continue;
			inFlight.controller.abort();
			this.#inFlight.delete(other);
			this.#setIdle(other);
		}
	}

	#setIdle(key: Key): void {
		const snapshot: AsyncResourceSnapshot<Value> = { ...IDLE, generation: this.get(key).generation + 1 };
		this.#states.set(key, snapshot);
		this.#onChange?.(key, snapshot);
	}

	#touch(key: Key, snapshot: AsyncResourceSnapshot<Value>): void {
		this.#states.delete(key);
		this.#states.set(key, snapshot);
	}

	// Idle entries stay (without values) so their generation keeps rejecting
	// completions of loads that were aborted or superseded.
	#evictBeyondCapacity(settledKey: Key): void {
		const settled = [...this.#states.keys()].filter((key) => (
			this.get(key).status !== "idle" && (key === settledKey || !this.#inFlight.has(key))
		));
		for (const key of settled.slice(0, Math.max(0, settled.length - this.#capacity))) {
			this.#setIdle(key);
		}
	}

	#start(key: Key): Promise<Value> {
		const previous = this.get(key);
		const generation = previous.generation + 1;
		const loading: AsyncResourceSnapshot<Value> = {
			status: "loading",
			value: previous.value,
			error: null,
			generation,
		};
		this.#touch(key, loading);
		this.#onChange?.(key, loading);

		const controller = new AbortController();
		const promise = this.#load(key, controller.signal)
			.then((value) => {
				if (this.get(key).generation === generation) {
					const ready: AsyncResourceSnapshot<Value> = {
						status: "ready",
						value,
						error: null,
						generation,
					};
					this.#states.set(key, ready);
					this.#onChange?.(key, ready);
					this.#evictBeyondCapacity(key);
				}
				return value;
			})
			.catch((error: unknown) => {
				if (this.get(key).generation === generation) {
					const failed: AsyncResourceSnapshot<Value> = {
						status: "error",
						value: previous.value,
						error: this.#errorMessage(error),
						generation,
					};
					this.#states.set(key, failed);
					this.#onChange?.(key, failed);
				}
				throw error;
			})
			.finally(() => {
				if (this.#inFlight.get(key)?.generation === generation) {
					this.#inFlight.delete(key);
				}
			});
		this.#inFlight.set(key, { generation, promise, controller });
		return promise;
	}
}

/**
 * Keyed request sharing and cancellation for every frontend fetch path.
 *
 * `SharedRequestRegistry` is the single in-flight mechanism: one request per
 * key, shared by every consumer, aborted explicitly by key or scope.
 * `KeyedAsyncResource` layers per-key loading/ready/error state and bounded
 * retention on top of it for side-panel metadata.
 */

type PendingRequest<Value> = {
	controller: AbortController;
	promise: Promise<Value>;
};

/** Shares one in-flight request per key until it settles or is aborted. */
export class SharedRequestRegistry<Key, Value> {
	readonly #pending = new Map<Key, PendingRequest<Value>>();

	get(key: Key): Promise<Value> | undefined {
		return this.#pending.get(key)?.promise;
	}

	request(key: Key, load: (signal: AbortSignal) => Promise<Value>): Promise<Value> {
		const existing = this.#pending.get(key);
		if (existing) return existing.promise;

		const controller = new AbortController();
		const promise = load(controller.signal).finally(() => {
			if (this.#pending.get(key)?.promise === promise) {
				this.#pending.delete(key);
			}
		});
		this.#pending.set(key, { controller, promise });
		return promise;
	}

	/** Abort `key`'s request; later `request` calls start a fresh one. */
	abort(key: Key): void {
		this.#pending.get(key)?.controller.abort();
		this.#pending.delete(key);
	}

	/** Abort every request except `key`'s; returns the aborted keys. */
	abortOthers(key: Key): Key[] {
		const aborted = [...this.#pending.keys()].filter((other) => other !== key);
		for (const other of aborted) this.abort(other);
		return aborted;
	}

	abortAll(): void {
		for (const request of this.#pending.values()) {
			request.controller.abort();
		}
		this.#pending.clear();
	}
}

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
	readonly #requests = new SharedRequestRegistry<Key, Value>();

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
		return this.#requests.get(key) ?? this.#start(key);
	}

	/** Load `key` again, superseding any request already in flight. */
	reload(key: Key): Promise<Value> {
		this.#requests.abort(key);
		return this.#start(key);
	}

	invalidate(key: Key): void {
		this.#requests.abort(key);
		this.#setIdle(key);
	}

	/**
	 * Abort every in-flight load except `key`'s. Views that show one key at a
	 * time call this so skipped keys stop costing server work.
	 */
	abortOthers(key: Key): void {
		for (const other of this.#requests.abortOthers(key)) this.#setIdle(other);
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
			this.get(key).status !== "idle" && (key === settledKey || !this.#requests.get(key))
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

		return this.#requests.request(key, (signal) => this.#load(key, signal)
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
			}));
	}
}

/**
 * Loads a side panel's key once it has stayed selected for the settle delay,
 * aborting loads for keys the panel has moved past. Ready keys are served
 * immediately. Returns the cleanup for the calling effect.
 */
export function ensureWhenSettled<Key, Value>(
	resource: KeyedAsyncResource<Key, Value>,
	key: Key,
	settleMs = METADATA_SETTLE_MS,
): (() => void) | undefined {
	resource.abortOthers(key);
	if (resource.get(key).status === "ready") {
		void resource.ensure(key);
		return undefined;
	}
	const timer = setTimeout(() => void resource.ensure(key).catch(() => {}), settleMs);
	return () => clearTimeout(timer);
}

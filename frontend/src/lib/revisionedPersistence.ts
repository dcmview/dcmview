export type PersistenceStatus = "clean" | "saving" | "dirty" | "error";

export type PersistenceSnapshot<Value> = {
	value: Value;
	status: PersistenceStatus;
	error: string | null;
};

export type RevisionedPersistenceOptions<Key, Value> = {
	save: (key: Key, value: Value) => Promise<Value>;
	onChange?: (key: Key, snapshot: PersistenceSnapshot<Value>) => void;
	errorMessage?: (error: unknown) => string;
};

type PersistenceState<Value> = {
	value: Value;
	committedValue: Value;
	revision: number;
	committedRevision: number;
	inFlightRevision: number | null;
	error: string | null;
};

function defaultErrorMessage(error: unknown): string {
	return error instanceof Error && error.message ? error.message : "Failed to save";
}

/**
 * Serializes writes for each key while coalescing edits made during a request.
 *
 * A completed request only replaces the local value when it still represents
 * the newest revision. Newer local edits are retained and written next.
 */
export class RevisionedPersistenceController<Key, Value> {
	readonly #save: (key: Key, value: Value) => Promise<Value>;
	readonly #onChange?: (key: Key, snapshot: PersistenceSnapshot<Value>) => void;
	readonly #errorMessage: (error: unknown) => string;
	readonly #states = new Map<Key, PersistenceState<Value>>();

	constructor({ save, onChange, errorMessage = defaultErrorMessage }: RevisionedPersistenceOptions<Key, Value>) {
		this.#save = save;
		this.#onChange = onChange;
		this.#errorMessage = errorMessage;
	}

	initialize(key: Key, value: Value): void {
		if (this.#states.has(key)) return;
		const state: PersistenceState<Value> = {
			value,
			committedValue: value,
			revision: 0,
			committedRevision: 0,
			inFlightRevision: null,
			error: null,
		};
		this.#states.set(key, state);
		this.#emit(key, state);
	}

	get(key: Key): PersistenceSnapshot<Value> | undefined {
		const state = this.#states.get(key);
		return state ? this.#snapshot(state) : undefined;
	}

	/** Records a local edit and writes it once earlier writes settle. */
	edit(key: Key, value: Value): void {
		const state = this.#requiredState(key);
		state.value = value;
		state.revision += 1;
		state.error = null;
		this.#emit(key, state);
		this.#pump(key, state);
	}

	retry(key: Key): void {
		const state = this.#requiredState(key);
		state.error = null;
		this.#emit(key, state);
		this.#pump(key, state);
	}

	rollback(key: Key): void {
		const state = this.#requiredState(key);
		state.value = state.committedValue;
		state.revision += 1;
		state.error = null;
		if (state.inFlightRevision === null) {
			state.committedRevision = state.revision;
		}
		this.#emit(key, state);
		if (state.inFlightRevision !== null) this.#pump(key, state);
	}

	#pump(key: Key, state: PersistenceState<Value>): void {
		if (state.inFlightRevision !== null || state.error !== null) return;
		if (state.revision === state.committedRevision) return;

		const revision = state.revision;
		const value = state.value;
		state.inFlightRevision = revision;
		this.#emit(key, state);

		void this.#save(key, value)
			.then((canonicalValue) => {
				if (this.#states.get(key) !== state || state.inFlightRevision !== revision) return;
				state.committedValue = canonicalValue;
				state.committedRevision = revision;
				if (state.revision === revision) state.value = canonicalValue;
				state.error = null;
			})
			.catch((error: unknown) => {
				if (this.#states.get(key) !== state || state.inFlightRevision !== revision) return;
				state.error = this.#errorMessage(error);
			})
			.finally(() => {
				if (this.#states.get(key) !== state || state.inFlightRevision !== revision) return;
				state.inFlightRevision = null;
				this.#emit(key, state);
				if (state.error === null) this.#pump(key, state);
			});
	}

	#requiredState(key: Key): PersistenceState<Value> {
		const state = this.#states.get(key);
		if (!state) throw new Error("persistence state must be initialized before editing");
		return state;
	}

	#emit(key: Key, state: PersistenceState<Value>): void {
		this.#onChange?.(key, this.#snapshot(state));
	}

	#snapshot(state: PersistenceState<Value>): PersistenceSnapshot<Value> {
		const status: PersistenceStatus = state.error !== null
			? "error"
			: state.inFlightRevision !== null
				? "saving"
				: state.revision !== state.committedRevision
					? "dirty"
					: "clean";
		return { value: state.value, status, error: state.error };
	}
}

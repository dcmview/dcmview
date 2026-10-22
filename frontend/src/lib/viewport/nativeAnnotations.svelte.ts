import { ApiError, fetchNativeAnnotations } from "../../api";
import { showsMoreThanEmbed, type NativeAnnotationsResponse } from "../annotation/nativeAnnotations";

export type NativeAnnotationsOptions = {
	load?: (fileIndex: number, signal: AbortSignal) => Promise<NativeAnnotationsResponse>;
};

/**
 * `none`: the file has nothing native to show (the endpoint answered 404, or
 * the read shows no more than the EMBED view). `error`: the read failed; a
 * copy read before is kept.
 */
export type NativeAnnotationsStatus = "loading" | "ready" | "none" | "error";

type Entry = { status: NativeAnnotationsStatus; response: NativeAnnotationsResponse | null };

const NOTHING = new Set<string>();

function toggled(set: ReadonlySet<string>, id: string): ReadonlySet<string> {
	const next = new Set(set);
	if (!next.delete(id)) next.add(id);
	return next;
}

/**
 * Per-file native annotation records for the viewport, read through
 * `GET /api/file/{index}/annotations/native`. A file is read the first time
 * it is shown, again after each EMBED save of it completes (`refresh`), and
 * again after `forget`. A read that a later read of the same file, a
 * `forget`, or a copy at a newer store revision has overtaken is dropped.
 *
 * A file with nothing native has no copy, so the viewport draws and lists its
 * rectangles exactly as it does without native records.
 *
 * Which layers and classes are hidden is view state for the session: kept
 * here, never saved and never sent.
 */
export class NativeAnnotations {
	#entries = $state.raw<Record<number, Entry | undefined>>({});
	#hiddenLayers = $state.raw<ReadonlySet<string>>(NOTHING);
	#hiddenClasses = $state.raw<ReadonlySet<string>>(NOTHING);
	readonly #requests = new Map<number, { token: number; controller: AbortController }>();
	#nextToken = 0;
	readonly #load: (fileIndex: number, signal: AbortSignal) => Promise<NativeAnnotationsResponse>;

	constructor({ load = fetchNativeAnnotations }: NativeAnnotationsOptions = {}) {
		this.#load = load;
	}

	/** The file's records to draw and list, or `null` when it has none (yet). */
	response(fileIndex: number): NativeAnnotationsResponse | null {
		return this.#entries[fileIndex]?.response ?? null;
	}

	status(fileIndex: number): NativeAnnotationsStatus | null {
		return this.#entries[fileIndex]?.status ?? null;
	}

	get hiddenLayers(): ReadonlySet<string> {
		return this.#hiddenLayers;
	}

	get hiddenClasses(): ReadonlySet<string> {
		return this.#hiddenClasses;
	}

	toggleLayer(layerId: string): void {
		this.#hiddenLayers = toggled(this.#hiddenLayers, layerId);
	}

	toggleClass(classId: string): void {
		this.#hiddenClasses = toggled(this.#hiddenClasses, classId);
	}

	/** Reads the file the first time it is shown. */
	ensureLoaded(fileIndex: number): void {
		if (this.#entries[fileIndex] !== undefined) return;
		this.refresh(fileIndex);
	}

	/**
	 * Reads the file again, as after an EMBED save of it: the copy held is
	 * shown until the new one arrives, and a read still running is dropped.
	 */
	refresh(fileIndex: number): void {
		this.#requests.get(fileIndex)?.controller.abort();
		const token = ++this.#nextToken;
		const controller = new AbortController();
		this.#requests.set(fileIndex, { token, controller });
		const held = this.#entries[fileIndex]?.response ?? null;
		if (!held) this.#set(fileIndex, { status: "loading", response: null });
		const current = () => this.#requests.get(fileIndex)?.token === token;
		this.#load(fileIndex, controller.signal).then(
			(response) => {
				if (!current()) return;
				this.#requests.delete(fileIndex);
				const shown = this.#entries[fileIndex]?.response ?? null;
				// A copy at an older revision than the one shown says nothing newer.
				if (shown && response.revision < shown.revision) {
					this.#set(fileIndex, { status: "ready", response: shown });
					return;
				}
				this.#set(fileIndex, showsMoreThanEmbed(response)
					? { status: "ready", response }
					: { status: "none", response: null });
			},
			(error: unknown) => {
				if (!current()) return;
				this.#requests.delete(fileIndex);
				if (error instanceof ApiError && error.status === 404) {
					this.#set(fileIndex, { status: "none", response: null });
					return;
				}
				const kept = this.#entries[fileIndex]?.response ?? null;
				this.#set(fileIndex, { status: kept ? "ready" : "error", response: kept });
			},
		);
	}

	/** Drops a file's copy, to read it again the next time it is shown. */
	forget(fileIndex: number): void {
		this.#requests.get(fileIndex)?.controller.abort();
		this.#requests.delete(fileIndex);
		const { [fileIndex]: _dropped, ...rest } = this.#entries;
		this.#entries = rest;
	}

	#set(fileIndex: number, entry: Entry): void {
		this.#entries = { ...this.#entries, [fileIndex]: entry };
	}
}

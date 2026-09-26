import { fetchAnnotations, updateAnnotations, type EmbedRoiAnnotations } from "../../api";
import { KeyedAsyncResource, type AsyncResourceSnapshot } from "../keyedAsyncResource";
import {
	RevisionedPersistenceController,
	type PersistenceSnapshot,
	type PersistenceStatus,
} from "../revisionedPersistence";

export type AnnotationStoreOptions = {
	load?: (fileIndex: number) => Promise<EmbedRoiAnnotations>;
	save?: (fileIndex: number, annotations: EmbedRoiAnnotations) => Promise<EmbedRoiAnnotations>;
};

function messageOr(fallback: string): (error: unknown) => string {
	return (error) => error instanceof Error && error.message ? error.message : fallback;
}

/**
 * Per-file EMBED ROI annotations for the viewport: the server copy is loaded
 * once per file, edits are shown immediately and written through a
 * revisioned save queue, and each file remembers its selected ROI.
 */
export class AnnotationStore {
	#values = $state<Record<number, EmbedRoiAnnotations | undefined>>({});
	#loads = $state<Record<number, AsyncResourceSnapshot<EmbedRoiAnnotations> | undefined>>({});
	#saves = $state<Record<number, PersistenceSnapshot<EmbedRoiAnnotations> | undefined>>({});
	#selected = $state<Record<number, number | null | undefined>>({});
	// While a ROI of this file is being dragged, save completions must not
	// replace the live geometry under the pointer.
	#liveEditFile: number | null = null;
	readonly #loader: KeyedAsyncResource<number, EmbedRoiAnnotations>;
	readonly #persistence: RevisionedPersistenceController<number, EmbedRoiAnnotations>;

	constructor({ load = fetchAnnotations, save = updateAnnotations }: AnnotationStoreOptions = {}) {
		this.#loader = new KeyedAsyncResource<number, EmbedRoiAnnotations>({
			load: (fileIndex) => load(fileIndex),
			errorMessage: messageOr("Failed to load annotations"),
			onChange: (fileIndex, snapshot) => {
				this.#loads = { ...this.#loads, [fileIndex]: snapshot };
			},
		});
		this.#persistence = new RevisionedPersistenceController<number, EmbedRoiAnnotations>({
			save,
			errorMessage: messageOr("Failed to save annotations"),
			onChange: (fileIndex, snapshot) => {
				if (this.#liveEditFile !== fileIndex) {
					this.#values = { ...this.#values, [fileIndex]: snapshot.value };
				}
				this.#saves = { ...this.#saves, [fileIndex]: snapshot };
			},
		});
	}

	annotations(fileIndex: number): EmbedRoiAnnotations | null {
		return this.#values[fileIndex] ?? null;
	}

	/**
	 * True once the file's server copy has loaded. Editing before then would
	 * save a set built from nothing and replace the file's stored ROIs.
	 */
	ready(fileIndex: number): boolean {
		return this.#saves[fileIndex] !== undefined;
	}

	loading(fileIndex: number): boolean {
		return this.#loads[fileIndex]?.status === "loading";
	}

	/** The save error once loaded, otherwise the load error. */
	error(fileIndex: number): string | null {
		const save = this.#saves[fileIndex];
		if (save) return save.error;
		const load = this.#loads[fileIndex];
		return load?.status === "error" ? load.error : null;
	}

	saveStatus(fileIndex: number): PersistenceStatus | null {
		return this.#saves[fileIndex]?.status ?? null;
	}

	selected(fileIndex: number): number | null {
		return this.#selected[fileIndex] ?? null;
	}

	select(fileIndex: number, roiIndex: number | null): void {
		if ((this.#selected[fileIndex] ?? null) === roiIndex) return;
		this.#selected = { ...this.#selected, [fileIndex]: roiIndex };
	}

	/** Loads the file's annotations the first time it is shown. */
	ensureLoaded(fileIndex: number): void {
		if (this.ready(fileIndex) || this.#loader.get(fileIndex).status !== "idle") return;
		this.#initializeFrom(this.#loader.ensure(fileIndex), fileIndex);
	}

	retryLoad(fileIndex: number): void {
		if (this.ready(fileIndex)) return;
		this.#initializeFrom(this.#loader.reload(fileIndex), fileIndex);
	}

	/** Shows geometry that is not saved yet, such as a ROI mid-drag. */
	showDraft(fileIndex: number, annotations: EmbedRoiAnnotations): void {
		this.#values = { ...this.#values, [fileIndex]: annotations };
	}

	/** Shows and saves an edit, selecting `selectedIndex`. */
	commit(fileIndex: number, annotations: EmbedRoiAnnotations, selectedIndex: number | null): void {
		if (!this.ready(fileIndex)) return;
		this.showDraft(fileIndex, annotations);
		this.select(fileIndex, selectedIndex);
		this.#persistence.edit(fileIndex, annotations);
	}

	retrySave(fileIndex: number): void {
		if (this.ready(fileIndex)) this.#persistence.retry(fileIndex);
	}

	rollback(fileIndex: number): void {
		if (!this.ready(fileIndex)) return;
		this.#persistence.rollback(fileIndex);
		this.select(fileIndex, null);
	}

	beginLiveEdit(fileIndex: number): void {
		this.#liveEditFile = fileIndex;
	}

	endLiveEdit(): void {
		this.#liveEditFile = null;
	}

	#initializeFrom(request: Promise<EmbedRoiAnnotations>, fileIndex: number): void {
		request.then(
			(annotations) => this.#persistence.initialize(fileIndex, annotations),
			() => {},
		);
	}
}

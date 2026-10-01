import {
	fetchSemanticContext,
	type FileSummary,
	type GraphicAnnotationItemSummary,
	type PresentationStateContext,
	type SemanticContextResponse,
} from "../../api";
import {
	KeyedAsyncResource,
	METADATA_CACHE_FILES,
	type AsyncResourceSnapshot,
} from "../keyedAsyncResource";
import { isSoftcopyPresentationState } from "../semanticPresentation";
import type { NavigationFrameRef } from "../seriesNavigation";
import type { GraphicAnnotationSelection } from "../viewport/graphicAnnotations";

/** The server lists at most this many annotated frames per state. */
export const ANNOTATED_FRAME_LIMIT = 4096;

export type FrameTarget = { fileIndex: number; frameIndex: number };

/** A presentation state whose annotations can be drawn on the active tab's frames. */
export type GraphicAnnotationCandidate = {
	stateFileIndex: number;
	title: string;
	/** The state's file, for tooltips. */
	detail: string;
	/** The items with something to draw, in file order. */
	items: readonly GraphicAnnotationItemSummary[];
	/** Objects of the state that are not drawn (DISPLAY or MATRIX units, malformed). */
	skippedObjects: number;
	/** Whether the state lists this displayed frame as annotated. */
	covers: (fileIndex: number, frameIndex: number) => boolean;
};

type Coverage = {
	context: PresentationStateContext;
	frames: ReadonlySet<string>;
	files: ReadonlySet<number>;
	/** The list hit the server's cap, so unlisted frames may still be annotated. */
	truncated: boolean;
};

function frameKey(fileIndex: number, frameIndex: number): string {
	return `${fileIndex}:${frameIndex}`;
}

const coverageByResponse = new WeakMap<SemanticContextResponse, Coverage | null>();

function coverageOf(response: SemanticContextResponse): Coverage | null {
	if (coverageByResponse.has(response)) return coverageByResponse.get(response) ?? null;
	const { context } = response;
	const coverage = context.kind === "presentation_state" && context.annotated_frames.length > 0
		? {
			context,
			frames: new Set(context.annotated_frames.map((frame) => frameKey(frame.file_index, frame.frame_index))),
			files: new Set(context.annotated_frames.map((frame) => frame.file_index)),
			truncated: context.annotated_frames.length >= ANNOTATED_FRAME_LIMIT,
		}
		: null;
	coverageByResponse.set(response, coverage);
	return coverage;
}

/** The items of a state that draw something. */
export function drawableItems(context: PresentationStateContext): GraphicAnnotationItemSummary[] {
	return context.items.filter((item) => item.graphic_types.length + item.texts.length > 0);
}

/** How a state reads in a list: its Content Label and Description. */
export function presentationStateTitle(context: PresentationStateContext): string {
	const parts = [context.content_label, context.content_description].filter((part): part is string => !!part?.trim());
	return parts.length > 0 ? parts.join(" · ") : "Presentation state";
}

/**
 * Where "show on image" opens a state: the first frame of `itemIndex` when
 * given, else the state's first annotated frame.
 */
export function annotationEntryFrame(response: SemanticContextResponse, itemIndex: number | null = null): FrameTarget | null {
	const { context } = response;
	if (context.kind !== "presentation_state") return null;
	const frame = itemIndex === null
		? context.annotated_frames[0]
		: context.items.find((item) => item.index === itemIndex)?.first_frame;
	return frame ? { fileIndex: frame.file_index, frameIndex: frame.frame_index } : null;
}

export type GraphicAnnotationsOptions = {
	files: () => ReadonlyMap<number, FileSummary>;
	scanComplete: () => boolean;
	load?: (fileIndex: number, signal: AbortSignal) => Promise<SemanticContextResponse>;
};

/**
 * Graphic annotations of softcopy presentation states on the images they
 * reference. The states in the active file's study have their semantic
 * context read; a state is offered when it annotates a frame of the active
 * tab. One state is shown at a time, and its annotation items can be stepped
 * through: the stepped item is highlighted and the rest dimmed.
 */
export class GraphicAnnotations {
	/** File index of the state shown, or null when annotations are off. */
	selectedState = $state<number | null>(null);
	/** The item stepped to in the shown state, or null for every item alike. */
	selectedItem = $state<number | null>(null);
	#snapshots = $state.raw<Record<number, AsyncResourceSnapshot<SemanticContextResponse> | undefined>>({});
	/** Whether each state's context was read after discovery completed. */
	readonly #loadedComplete = new Map<number, boolean>();
	readonly #contexts: KeyedAsyncResource<number, SemanticContextResponse>;
	readonly #files: () => ReadonlyMap<number, FileSummary>;
	readonly #scanComplete: () => boolean;

	readonly #states = $derived.by(() => [...this.#files().values()].filter((file) => isSoftcopyPresentationState(file.sop_class_uid)));

	constructor({ files, scanComplete, load = (index) => fetchSemanticContext(index) }: GraphicAnnotationsOptions) {
		this.#files = files;
		this.#scanComplete = scanComplete;
		this.#contexts = new KeyedAsyncResource<number, SemanticContextResponse>({
			load,
			capacity: METADATA_CACHE_FILES,
			onChange: (index, snapshot) => {
				const { [index]: _previous, ...rest } = this.#snapshots;
				this.#snapshots = snapshot.status === "idle" ? rest : { ...rest, [index]: snapshot };
			},
		});
	}

	/**
	 * States that may annotate `fileIndex`: those of its study (PS3.3 C.11.11
	 * keeps a state and its images in one study), and the shown state, which
	 * "show on image" may have opened from another.
	 */
	#statesNear(fileIndex: number): FileSummary[] {
		const study = this.#files().get(fileIndex)?.study_instance_uid;
		return this.#states.filter((state) => (
			state.index !== fileIndex
			&& (state.index === this.selectedState || (!!study && state.study_instance_uid === study))
		));
	}

	/**
	 * Reads the contexts of the states that may annotate `fileIndex`. A context
	 * read while discovery was still running is read again once it completes,
	 * since its annotated frames can grow. Call from an effect.
	 */
	load(fileIndex: number | null): void {
		if (fileIndex === null) return;
		const complete = this.#scanComplete();
		for (const state of this.#statesNear(fileIndex)) {
			const loadedComplete = this.#loadedComplete.get(state.index);
			const status = this.#contexts.get(state.index).status;
			if (loadedComplete !== undefined && status !== "idle" && (loadedComplete || !complete)) continue;
			this.#loadedComplete.set(state.index, complete);
			const request = loadedComplete === undefined || status === "idle"
				? this.#contexts.ensure(state.index)
				: this.#contexts.reload(state.index);
			void request.catch(() => {});
		}
	}

	/** Re-read failed state contexts without changing what is shown. */
	retryFailedLoads(fileIndex: number | null): void {
		if (fileIndex === null) return;
		for (const state of this.#statesNear(fileIndex)) {
			if (this.#contexts.get(state.index).status === "error") void this.#contexts.reload(state.index).catch(() => {});
		}
	}

	/** States annotating a frame of the active tab (`frames`) or file. */
	candidatesFor(fileIndex: number, frames: readonly NavigationFrameRef[]): GraphicAnnotationCandidate[] {
		const scopeFiles = new Set(frames.map((frame) => frame.file_index)).add(fileIndex);
		const candidates: GraphicAnnotationCandidate[] = [];
		for (const state of this.#statesNear(fileIndex)) {
			const response = this.#snapshots[state.index]?.value;
			const coverage = response ? coverageOf(response) : null;
			if (!coverage) continue;
			if (!coverage.truncated && ![...coverage.files].some((file) => scopeFiles.has(file))) continue;
			const { display_units, matrix_units, malformed } = coverage.context.skipped;
			candidates.push({
				stateFileIndex: state.index,
				title: presentationStateTitle(coverage.context),
				detail: state.path,
				items: drawableItems(coverage.context),
				skippedObjects: display_units + matrix_units + malformed,
				covers: (file, frame) => coverage.truncated || coverage.frames.has(frameKey(file, frame)),
			});
		}
		// States that read alike are told apart by file name.
		return candidates.map((candidate) => (
			candidates.some((other) => other !== candidate && other.title === candidate.title)
				? { ...candidate, title: `${candidate.title} · ${candidate.detail.split(/[\\/]/).pop()}` }
				: candidate
		));
	}

	/** The shown state among `candidates`, if it is offered. */
	shown(candidates: readonly GraphicAnnotationCandidate[]): GraphicAnnotationCandidate | null {
		return candidates.find((candidate) => candidate.stateFileIndex === this.selectedState) ?? null;
	}

	/** What the viewport draws for the shown state. */
	selectionFor(candidates: readonly GraphicAnnotationCandidate[]): GraphicAnnotationSelection | null {
		const candidate = this.shown(candidates);
		return candidate ? { stateFileIndex: candidate.stateFileIndex, highlightedItem: this.selectedItem } : null;
	}

	/** Shows a state with every item alike, or turns annotations off with null. */
	select(stateFileIndex: number | null): void {
		this.selectedState = stateFileIndex;
		this.selectedItem = null;
	}

	/** Shows a state stepped to one of its items. */
	selectItem(stateFileIndex: number, itemIndex: number | null): void {
		this.selectedState = stateFileIndex;
		this.selectedItem = itemIndex;
	}

	/**
	 * Steps the shown state to its next or previous item, through "all items"
	 * at either end. Returns the frame to open for the new item: its first
	 * frame, unless it applies to every referenced image and the displayed
	 * frame (`current`) is already one of them.
	 */
	stepItem(candidates: readonly GraphicAnnotationCandidate[], step: -1 | 1, current: FrameTarget): FrameTarget | null {
		const candidate = this.shown(candidates);
		if (!candidate || candidate.items.length === 0) return null;
		// Position 0 is "all items"; item k of n is position k.
		const positions = candidate.items.length + 1;
		const position = candidate.items.findIndex((item) => item.index === this.selectedItem) + 1;
		const item = candidate.items[((position + step + positions) % positions) - 1] ?? null;
		this.selectedItem = item?.index ?? null;
		if (!item?.first_frame) return null;
		if (!item.scoped && candidate.covers(current.fileIndex, current.frameIndex)) return null;
		return { fileIndex: item.first_frame.file_index, frameIndex: item.first_frame.frame_index };
	}
}

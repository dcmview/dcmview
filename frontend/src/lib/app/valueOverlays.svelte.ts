import {
	fetchSemanticContext,
	type FileSummary,
	type OverlayLegend,
	type SemanticContextResponse,
	type SeriesSummary,
} from "../../api";
import {
	KeyedAsyncResource,
	METADATA_CACHE_FILES,
	type AsyncResourceSnapshot,
} from "../keyedAsyncResource";
import type { NavigationFrameRef } from "../seriesNavigation";
import type { ValueOverlay, ValueOverlayKind } from "../viewport/frameOverlay";

const RT_DOSE_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.481.2";
/** The server lists at most this many covered frames per volume. */
export const OVERLAY_SOURCE_FRAME_LIMIT = 4096;
export const DEFAULT_VALUE_OVERLAY_OPACITY = 0.5;

/** The value overlay a file can supply, if any. */
export function valueOverlayKind(file: FileSummary): ValueOverlayKind | null {
	if (file.sop_class_uid === RT_DOSE_SOP_CLASS_UID) return "rt_dose";
	return null;
}

/** A volume whose colorwash can be drawn on the active tab's frames. */
export type ValueOverlayCandidate = {
	kind: ValueOverlayKind;
	volumeFileIndex: number;
	title: string;
	/** The volume's file, for tooltips. */
	detail: string;
	legend: OverlayLegend;
	/** Whether the context lists this displayed frame as covered. */
	covers: (fileIndex: number, frameIndex: number) => boolean;
};

type Coverage = {
	kind: ValueOverlayKind;
	title: string;
	legend: OverlayLegend;
	frames: ReadonlySet<string>;
	files: ReadonlySet<number>;
	/** The list hit the server's cap, so unlisted frames may still be covered. */
	truncated: boolean;
};

function frameKey(fileIndex: number, frameIndex: number): string {
	return `${fileIndex}:${frameIndex}`;
}

const coverageByResponse = new WeakMap<SemanticContextResponse, Coverage | null>();

/** The eligible overlay a volume's semantic context describes. */
function coverageOf(response: SemanticContextResponse): Coverage | null {
	if (coverageByResponse.has(response)) return coverageByResponse.get(response) ?? null;
	const { context } = response;
	let coverage: Coverage | null = null;
	if (context.kind === "rt_dose" && context.overlay.eligible && context.legend) {
		const summation = context.dose_summation_type?.trim();
		coverage = {
			kind: "rt_dose",
			title: summation ? `RT Dose · ${summation}` : "RT Dose",
			legend: context.legend,
			frames: new Set(context.overlay_source_frames.map((frame) => frameKey(frame.file_index, frame.frame_index))),
			files: new Set(context.overlay_source_frames.map((frame) => frame.file_index)),
			truncated: context.overlay_source_frames.length >= OVERLAY_SOURCE_FRAME_LIMIT,
		};
	}
	coverageByResponse.set(response, coverage);
	return coverage;
}

/**
 * Where "show on source image" opens a volume: the first covered frame of
 * its declared source image, else its first covered frame.
 */
export function overlayEntryFrame(
	response: SemanticContextResponse,
): { fileIndex: number; frameIndex: number } | null {
	const { context } = response;
	if (context.kind !== "rt_dose" || !context.overlay.eligible) return null;
	const frames = context.overlay_source_frames;
	const declared = context.overlay.source_file_index;
	const entry = frames.find((frame) => frame.file_index === declared) ?? frames[0];
	return entry ? { fileIndex: entry.file_index, frameIndex: entry.frame_index } : null;
}

export type ValueOverlaysOptions = {
	files: () => ReadonlyMap<number, FileSummary>;
	series: () => readonly SeriesSummary[];
	scanComplete: () => boolean;
	load?: (fileIndex: number, signal: AbortSignal) => Promise<SemanticContextResponse>;
};

/**
 * RT Dose colorwash overlays on the images they cover. The volumes sharing
 * a Frame of Reference with the active file have their semantic context
 * read; a volume is offered when its overlay is eligible and it covers a
 * frame of the active tab. One volume is shown at a time, at a shared
 * opacity, on every tab it covers.
 */
export class ValueOverlays {
	/** File index of the volume shown, or null when overlays are off. */
	selectedVolume = $state<number | null>(null);
	/** 0..1 */
	opacity = $state(DEFAULT_VALUE_OVERLAY_OPACITY);
	#snapshots = $state<Record<number, AsyncResourceSnapshot<SemanticContextResponse> | undefined>>({});
	/** Whether each volume's context was read after discovery completed. */
	readonly #loadedComplete = new Map<number, boolean>();
	readonly #contexts: KeyedAsyncResource<number, SemanticContextResponse>;
	readonly #files: () => ReadonlyMap<number, FileSummary>;
	readonly #series: () => readonly SeriesSummary[];
	readonly #scanComplete: () => boolean;

	readonly #frameOfReferenceByFile = $derived.by(() => {
		const byFile = new Map<number, readonly string[]>();
		for (const series of this.#series()) {
			for (const stack of series.stacks) {
				for (const frame of stack.frames) byFile.set(frame.file_index, series.frame_of_reference_uids);
			}
		}
		return byFile;
	});
	readonly #volumes = $derived.by(() => (
		[...this.#files().values()].filter((file) => file.has_pixels && valueOverlayKind(file) !== null)
	));

	constructor({ files, series, scanComplete, load = (index) => fetchSemanticContext(index) }: ValueOverlaysOptions) {
		this.#files = files;
		this.#series = series;
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

	/** Volumes that share a Frame of Reference with `fileIndex`. */
	#volumesNear(fileIndex: number): FileSummary[] {
		const uids = this.#frameOfReferenceByFile.get(fileIndex);
		if (!uids || uids.length === 0) return [];
		return this.#volumes.filter((volume) => (
			volume.index !== fileIndex
			&& (this.#frameOfReferenceByFile.get(volume.index) ?? []).some((uid) => uids.includes(uid))
		));
	}

	/**
	 * Reads the contexts of the volumes that may cover `fileIndex`. A context
	 * read while discovery was still running is read again once it completes,
	 * since its covered frames can grow. Call from an effect.
	 */
	load(fileIndex: number | null): void {
		if (fileIndex === null) return;
		const complete = this.#scanComplete();
		for (const volume of this.#volumesNear(fileIndex)) {
			const loadedComplete = this.#loadedComplete.get(volume.index);
			const status = this.#contexts.get(volume.index).status;
			if (loadedComplete !== undefined && status !== "idle" && (loadedComplete || !complete)) continue;
			this.#loadedComplete.set(volume.index, complete);
			const request = loadedComplete === undefined || status === "idle"
				? this.#contexts.ensure(volume.index)
				: this.#contexts.reload(volume.index);
			void request.catch(() => {});
		}
	}

	/** Volumes covering a frame of the active tab (`frames`) or file. */
	candidatesFor(fileIndex: number, frames: readonly NavigationFrameRef[]): ValueOverlayCandidate[] {
		const scopeFiles = new Set(frames.map((frame) => frame.file_index)).add(fileIndex);
		const candidates: ValueOverlayCandidate[] = [];
		for (const volume of this.#volumesNear(fileIndex)) {
			const response = this.#snapshots[volume.index]?.value;
			const coverage = response ? coverageOf(response) : null;
			if (!coverage) continue;
			if (!coverage.truncated && ![...coverage.files].some((file) => scopeFiles.has(file))) continue;
			candidates.push({
				kind: coverage.kind,
				volumeFileIndex: volume.index,
				title: coverage.title,
				detail: volume.path,
				legend: coverage.legend,
				covers: (file, frame) => coverage.truncated || coverage.frames.has(frameKey(file, frame)),
			});
		}
		// Volumes that read alike (two PLAN doses) are told apart by file name.
		return candidates.map((candidate) => (
			candidates.some((other) => other !== candidate && other.title === candidate.title)
				? { ...candidate, title: `${candidate.title} · ${candidate.detail.split(/[\\/]/).pop()}` }
				: candidate
		));
	}

	/** The selected volume's overlay on one displayed frame, when offered. */
	overlayFor(
		candidates: readonly ValueOverlayCandidate[],
		fileIndex: number,
		frameIndex: number,
	): ValueOverlay | null {
		const candidate = candidates.find((entry) => entry.volumeFileIndex === this.selectedVolume);
		if (!candidate) return null;
		return {
			kind: candidate.kind,
			volumeFileIndex: candidate.volumeFileIndex,
			title: candidate.title,
			legend: candidate.legend,
			opacity: this.opacity,
			coversFrame: candidate.covers(fileIndex, frameIndex),
		};
	}

	/** Shows `volumeFileIndex`, or turns overlays off when it is already shown. */
	toggle(volumeFileIndex: number): void {
		this.selectedVolume = this.selectedVolume === volumeFileIndex ? null : volumeFileIndex;
	}

	select(volumeFileIndex: number | null): void {
		this.selectedVolume = volumeFileIndex;
	}

	setOpacity(opacity: number): void {
		if (Number.isFinite(opacity)) this.opacity = Math.min(1, Math.max(0, opacity));
	}
}

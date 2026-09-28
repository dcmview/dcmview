import {
	ApiError,
	fetchRawPixel,
	fetchSelectedTag,
	type FileSummary,
	type FrameValueMapping,
	type RawFrame,
	type TagNode,
} from "../../api";
import { validateRenderableRawFrame } from "../rawWindowing";
import type { RawFrameSource } from "./rawFrameSource";
import {
	describePixelValues,
	formatValue,
	rawColorNeedsPlanarConfiguration,
	rawHeaderValueMapping,
	type ImagePixel,
	type PixelValues,
} from "./valueMapping";

/** How long the cursor must rest on a frame before its samples load. */
export const PROBE_SETTLE_MS = 150;

/** Larger frames are read a pixel at a time unless their samples are already held. */
export const WHOLE_FRAME_PROBE_MAX_PIXELS = 4 * 1024 * 1024;
/**
 * Frames the raw renderer cannot window (color, 32-bit, very large) are not
 * cached, so returning to one would fetch it again; from this size they are
 * read a pixel at a time.
 */
export const UNCACHED_FRAME_PROBE_MAX_PIXELS = 512 * 512;

/** Whether the readout reads `file`'s frames one pixel at a time. */
export function probesSinglePixels(file: FileSummary): boolean {
	const pixels = file.rows * file.columns;
	return pixels > WHOLE_FRAME_PROBE_MAX_PIXELS
		|| (!file.raw_windowing_compatible && pixels > UNCACHED_FRAME_PROBE_MAX_PIXELS);
}

const PLANAR_CONFIGURATION_TAG = "(0028,0006)";

export type ProbeSamples =
	| { status: "loading" }
	/** `at`: the frame holds only this pixel, as a 1x1 frame. */
	| { status: "ready"; frame: RawFrame; at?: ImagePixel }
	| { status: "unavailable"; reason: string };

function frameKey(fileIndex: number, frameIndex: number): string {
	return `${fileIndex}:${frameIndex}`;
}

function planarConfigurationOf(node: TagNode): number {
	const { value } = node;
	if (value.type === "number") return value.value;
	if (value.type === "numbers") return value.value[0] ?? 0;
	if (value.type === "string") return Number(value.value) || 0;
	return 0;
}

/**
 * The pixel under the cursor and the raw samples of the frame it reads.
 * Samples come from the shared raw-frame source: the frame the window/level
 * renderer already holds, or one fetched once the cursor settles. A file
 * whose raw endpoint rejects its layout stays display-only for the readout.
 */
export class PixelProbe {
	/** Image pixel under the cursor; null off the image or outside the viewport. */
	pixel = $state<ImagePixel | null>(null);
	#samples = $state.raw<{ key: string; state: ProbeSamples } | null>(null);
	#unavailableFiles = $state<Record<number, string | undefined>>({});
	#planar = $state<Record<number, number | undefined>>({});
	readonly #planarRequested = new Set<number>();
	readonly #rawFrames: RawFrameSource;
	readonly #loadTag: typeof fetchSelectedTag;
	readonly #loadPixel: typeof fetchRawPixel;

	constructor(
		rawFrames: RawFrameSource,
		loadTag: typeof fetchSelectedTag = fetchSelectedTag,
		loadPixel: typeof fetchRawPixel = fetchRawPixel,
	) {
		this.#rawFrames = rawFrames;
		this.#loadTag = loadTag;
		this.#loadPixel = loadPixel;
	}

	/** Samples of this frame, when it is the one being read. */
	samples(fileIndex: number, frameIndex: number): ProbeSamples | null {
		const unavailable = this.#unavailableFiles[fileIndex];
		if (unavailable) return { status: "unavailable", reason: unavailable };
		const current = this.#samples;
		return current?.key === frameKey(fileIndex, frameIndex) ? current.state : null;
	}

	/**
	 * Planar Configuration of a native color file (0 interleaved, 1 planar);
	 * 0 for every other file, and undefined while it loads.
	 */
	planarConfiguration(file: FileSummary, frame: RawFrame): number | undefined {
		if (frame.metadata.samplesPerPixel === 1 || !rawColorNeedsPlanarConfiguration(file.transfer_syntax_uid)) {
			return 0;
		}
		return this.#planar[file.index];
	}

	/**
	 * Reads this frame's samples: `displayed` when the renderer already holds
	 * them, else a cached or settled fetch. A frame too large to fetch for one
	 * value is read at `pixel` only. Call untracked from an effect; returns
	 * the effect's cleanup.
	 */
	track(
		file: FileSummary,
		frameIndex: number,
		displayed: RawFrame | null,
		{ pixel = null }: { pixel?: ImagePixel | null } = {},
	): (() => void) | undefined {
		if (this.#unavailableFiles[file.index]) return undefined;
		const key = frameKey(file.index, frameIndex);
		const known = displayed ?? this.#rawFrames.cached(file.index, frameIndex);
		if (known) {
			this.#ready(file, key, known);
			return undefined;
		}
		if (probesSinglePixels(file)) return this.#trackPixel(file, frameIndex, key, pixel);
		const current = this.#samples;
		if (current?.key === key && current.state.status !== "loading") return undefined;
		if (current?.key !== key || current.state.status === "ready") {
			this.#samples = { key, state: { status: "loading" } };
		}

		let cancelled = false;
		const timer = setTimeout(() => {
			this.#rawFrames.ensure(file.index, frameIndex)
				.then((frame) => {
					if (validateRenderableRawFrame(frame) === null) this.#rawFrames.store(file.index, frameIndex, frame);
					if (!cancelled) this.#ready(file, key, frame);
				})
				.catch((error: unknown) => this.#failed(file, key, error, cancelled));
		}, PROBE_SETTLE_MS);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	}

	#trackPixel(file: FileSummary, frameIndex: number, key: string, pixel: ImagePixel | null): (() => void) | undefined {
		if (!pixel) return undefined;
		const current = this.#samples;
		if (
			current?.key === key
			&& current.state.status === "ready"
			&& current.state.at?.row === pixel.row
			&& current.state.at.column === pixel.column
		) return undefined;
		if (current?.key !== key || current.state.status !== "loading") {
			this.#samples = { key, state: { status: "loading" } };
		}
		const controller = new AbortController();
		const timer = setTimeout(() => {
			this.#loadPixel(file.index, frameIndex, pixel, controller.signal)
				.then((frame) => {
					this.#samples = { key, state: { status: "ready", frame, at: pixel } };
				})
				.catch((error: unknown) => this.#failed(file, key, error, controller.signal.aborted));
		}, PROBE_SETTLE_MS);
		return () => {
			clearTimeout(timer);
			controller.abort();
		};
	}

	#failed(file: FileSummary, key: string, error: unknown, cancelled: boolean): void {
		if (cancelled || (error as Error).name === "AbortError") return;
		const reason = error instanceof Error && error.message ? error.message : String(error);
		// 422 means the raw endpoint does not serve this file's layout.
		if (error instanceof ApiError && error.status === 422) {
			this.#unavailableFiles = { ...this.#unavailableFiles, [file.index]: reason };
		} else {
			this.#samples = { key, state: { status: "unavailable", reason } };
		}
	}

	#ready(file: FileSummary, key: string, frame: RawFrame): void {
		const current = this.#samples;
		if (current?.key !== key || current.state.status !== "ready" || current.state.frame !== frame) {
			this.#samples = { key, state: { status: "ready", frame } };
		}
		if (
			frame.metadata.samplesPerPixel > 1
			&& rawColorNeedsPlanarConfiguration(file.transfer_syntax_uid)
			&& !this.#planarRequested.has(file.index)
		) {
			this.#planarRequested.add(file.index);
			this.#loadTag(file.index, { path: PLANAR_CONFIGURATION_TAG })
				.then(planarConfigurationOf)
				.catch(() => 0)
				.then((planar) => {
					this.#planar = { ...this.#planar, [file.index]: planar };
				});
		}
	}
}

/** What the readout HUD shows for the pixel under the cursor. */
export type PixelReadoutModel = {
	pixel: ImagePixel;
	/** One-based frame number of the frame being read. */
	frameNumber: number;
	values: PixelValues | null;
	/** Why values are missing or partial. */
	note: string | null;
	/** The shown dose or map colorwash's value at this pixel. */
	overlay?: OverlayValueReadout | null;
};

/** A value overlay's resampled values for the displayed frame, as loaded. */
export type OverlayValueState =
	| { status: "loading" }
	| { status: "ready"; values: Float32Array }
	| { status: "not_covering" }
	| { status: "unavailable" };

export type OverlayValueReadout = {
	label: string;
	unit: string;
	/** Null when the pixel has no value; `note` says why. */
	value: string | null;
	note: string | null;
};

/** The overlaid volume's value at `pixel` of a displayed frame `columns` wide. */
export function overlayValueReadout(
	label: string,
	unit: string,
	pixel: ImagePixel,
	columns: number,
	state: OverlayValueState,
): OverlayValueReadout {
	const base = { label, unit };
	switch (state.status) {
		case "loading":
			return { ...base, value: null, note: "reading…" };
		case "not_covering":
			return { ...base, value: null, note: "outside the volume" };
		case "unavailable":
			return { ...base, value: null, note: "unavailable" };
		case "ready": {
			const value = pixel.column < columns ? state.values[pixel.row * columns + pixel.column] : undefined;
			return value === undefined || Number.isNaN(value)
				? { ...base, value: null, note: "outside the volume" }
				: { ...base, value: formatValue(value), note: null };
		}
	}
}

export type PixelReadoutInput = {
	pixel: ImagePixel;
	file: FileSummary;
	frameIndex: number;
	samples: ProbeSamples | null;
	mapping: FrameValueMapping | null;
	mappingFailed: boolean;
	planarConfiguration: (frame: RawFrame) => number | undefined;
	paused: boolean;
};

/** Combines the probe's samples and the frame's value mapping into the readout. */
export function pixelReadout(input: PixelReadoutInput): PixelReadoutModel {
	const base = { pixel: input.pixel, frameNumber: input.frameIndex + 1 };
	if (input.paused) return { ...base, values: null, note: "values paused during playback" };
	const { samples } = input;
	if (samples?.status === "unavailable") return { ...base, values: null, note: "value unavailable (display only)" };
	if (samples?.status !== "ready") return { ...base, values: null, note: "reading…" };
	// A single-pixel read answers only for the pixel it was made at.
	const { at } = samples;
	if (at && (at.row !== input.pixel.row || at.column !== input.pixel.column)) {
		return { ...base, values: null, note: "reading…" };
	}
	let mapping = input.mapping;
	let note: string | null = null;
	if (!mapping) {
		if (!input.mappingFailed) return { ...base, values: null, note: "reading…" };
		mapping = rawHeaderValueMapping(input.file.index, input.frameIndex, samples.frame);
		note = "value mapping unavailable";
		if (!mapping) return { ...base, values: null, note };
	}
	// A single pixel arrives with its samples already color-by-pixel.
	const planar = at ? 0 : input.planarConfiguration(samples.frame);
	if (planar === undefined) return { ...base, values: null, note: "reading…" };
	const values = describePixelValues(
		samples.frame,
		at ? { row: 0, column: 0 } : input.pixel,
		mapping,
		input.file.modality,
		planar,
	);
	return values ? { ...base, values, note } : { ...base, values: null, note: "value unavailable" };
}

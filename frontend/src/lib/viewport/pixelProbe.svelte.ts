import {
	ApiError,
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
	rawColorNeedsPlanarConfiguration,
	rawHeaderValueMapping,
	type ImagePixel,
	type PixelValues,
} from "./valueMapping";

/** How long the cursor must rest on a frame before its samples load. */
export const PROBE_SETTLE_MS = 150;

const PLANAR_CONFIGURATION_TAG = "(0028,0006)";

export type ProbeSamples =
	| { status: "loading" }
	| { status: "ready"; frame: RawFrame }
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
	#samples = $state<{ key: string; state: ProbeSamples } | null>(null);
	#unavailableFiles = $state<Record<number, string | undefined>>({});
	#planar = $state<Record<number, number | undefined>>({});
	readonly #planarRequested = new Set<number>();
	readonly #rawFrames: RawFrameSource;
	readonly #loadTag: typeof fetchSelectedTag;

	constructor(rawFrames: RawFrameSource, loadTag: typeof fetchSelectedTag = fetchSelectedTag) {
		this.#rawFrames = rawFrames;
		this.#loadTag = loadTag;
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
	 * them, else a cached or settled fetch. Call untracked from an effect;
	 * returns the effect's cleanup.
	 */
	track(file: FileSummary, frameIndex: number, displayed: RawFrame | null): (() => void) | undefined {
		if (this.#unavailableFiles[file.index]) return undefined;
		const key = frameKey(file.index, frameIndex);
		const known = displayed ?? this.#rawFrames.cached(file.index, frameIndex);
		if (known) {
			this.#ready(file, key, known);
			return undefined;
		}
		const current = this.#samples;
		if (current?.key === key && current.state.status !== "loading") return undefined;
		if (current?.key !== key) this.#samples = { key, state: { status: "loading" } };

		let cancelled = false;
		const timer = setTimeout(() => {
			this.#rawFrames.ensure(file.index, frameIndex)
				.then((frame) => {
					if (validateRenderableRawFrame(frame) === null) this.#rawFrames.store(file.index, frameIndex, frame);
					if (!cancelled) this.#ready(file, key, frame);
				})
				.catch((error: unknown) => {
					if (cancelled || (error as Error).name === "AbortError") return;
					const reason = error instanceof Error && error.message ? error.message : String(error);
					// 422 means the raw endpoint does not serve this file's layout.
					if (error instanceof ApiError && error.status === 422) {
						this.#unavailableFiles = { ...this.#unavailableFiles, [file.index]: reason };
					} else {
						this.#samples = { key, state: { status: "unavailable", reason } };
					}
				});
		}, PROBE_SETTLE_MS);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
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
};

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
	let mapping = input.mapping;
	let note: string | null = null;
	if (!mapping) {
		if (!input.mappingFailed) return { ...base, values: null, note: "reading…" };
		mapping = rawHeaderValueMapping(input.file.index, input.frameIndex, samples.frame);
		note = "value mapping unavailable";
		if (!mapping) return { ...base, values: null, note };
	}
	const planar = input.planarConfiguration(samples.frame);
	if (planar === undefined) return { ...base, values: null, note: "reading…" };
	const values = describePixelValues(samples.frame, input.pixel, mapping, input.file.modality, planar);
	return values ? { ...base, values, note } : { ...base, values: null, note: "value unavailable" };
}

import { fetchGraphicAnnotations, type GraphicAnnotationsResponse } from "../../api";
import { KeyedAsyncResource, type AsyncResourceSnapshot } from "../keyedAsyncResource";

/** Annotated frames kept for revisits; each is a few objects of JSON. */
const FRAME_CAPACITY = 512;

export type AnnotatedFrame = { stateFileIndex: number; fileIndex: number; frameIndex: number };

type Load = (frame: AnnotatedFrame, signal: AbortSignal) => Promise<GraphicAnnotationsResponse>;

function frameKey({ stateFileIndex, fileIndex, frameIndex }: AnnotatedFrame): string {
	return `${stateFileIndex}:${fileIndex}:${frameIndex}`;
}

/**
 * The annotations a presentation state draws on each displayed frame, read
 * once per state and frame. A frame's annotations are returned only for that
 * frame, so a frame is never shown with another's while its own load.
 */
export class GraphicAnnotationFrames {
	#snapshots = $state.raw<Record<string, AsyncResourceSnapshot<GraphicAnnotationsResponse> | undefined>>({});
	readonly #frames: KeyedAsyncResource<string, GraphicAnnotationsResponse>;

	constructor(load: Load = (frame, signal) => fetchGraphicAnnotations(frame.fileIndex, frame.frameIndex, frame.stateFileIndex, signal)) {
		this.#frames = new KeyedAsyncResource<string, GraphicAnnotationsResponse>({
			load: (key, signal) => {
				const [stateFileIndex, fileIndex, frameIndex] = key.split(":").map(Number);
				return load({ stateFileIndex, fileIndex, frameIndex }, signal);
			},
			capacity: FRAME_CAPACITY,
			onChange: (key, snapshot) => {
				const { [key]: _previous, ...rest } = this.#snapshots;
				this.#snapshots = snapshot.status === "idle" ? rest : { ...rest, [key]: snapshot };
			},
		});
	}

	/** Reads `frame`'s annotations unless they are held; frames passed by stop loading. */
	load(frame: AnnotatedFrame): void {
		const key = frameKey(frame);
		this.#frames.abortOthers(key);
		void this.#frames.ensure(key).catch(() => {});
	}

	/** Reads `frame`'s annotations again if their load failed. */
	retry(frame: AnnotatedFrame): void {
		const key = frameKey(frame);
		if (this.#frames.get(key).status === "error") void this.#frames.reload(key).catch(() => {});
	}

	/** `frame`'s annotations once read. */
	get(frame: AnnotatedFrame): GraphicAnnotationsResponse | null {
		const snapshot = this.#snapshots[frameKey(frame)];
		return snapshot?.status === "ready" ? snapshot.value ?? null : null;
	}

	failed(frame: AnnotatedFrame): boolean {
		return this.#snapshots[frameKey(frame)]?.status === "error";
	}
}

import { waitForAbortableResult } from "../cinePlayback";

type FrameRef = { fileIndex: number; frameIndex: number };
type Waiter = FrameRef & { resolve: (rendered: boolean) => void };

/**
 * Tracks which display frame the canvas last presented, so cine playback can
 * pace itself on actual presentation. `token` is exposed on the canvas as
 * `data-capture-rendered` for screenshot automation.
 */
export class RenderedFrames {
	token = $state("");
	#last: FrameRef | null = null;
	readonly #waiters = new Set<Waiter>();

	mark(fileIndex: number, frameIndex: number): void {
		this.token = `${fileIndex}:${frameIndex}`;
		this.#last = { fileIndex, frameIndex };
		for (const waiter of [...this.#waiters]) {
			if (waiter.fileIndex !== fileIndex || waiter.frameIndex !== frameIndex) continue;
			this.#waiters.delete(waiter);
			waiter.resolve(true);
		}
	}

	/** Resolves true once the frame is presented, false if aborted or reset first. */
	waitFor(fileIndex: number, frameIndex: number, signal: AbortSignal): Promise<boolean> {
		if (this.#last?.fileIndex === fileIndex && this.#last.frameIndex === frameIndex) {
			return Promise.resolve(true);
		}
		return waitForAbortableResult(signal, (settle) => {
			const waiter = { fileIndex, frameIndex, resolve: settle };
			this.#waiters.add(waiter);
			return () => this.#waiters.delete(waiter);
		});
	}

	/** The canvas was cleared; the last presented frame still counts for waiters. */
	clearToken(): void {
		this.token = "";
	}

	/** Forgets the presented frame and releases every waiter unrendered. */
	reset(): void {
		for (const waiter of this.#waiters) waiter.resolve(false);
		this.#waiters.clear();
		this.#last = null;
		this.token = "";
	}
}

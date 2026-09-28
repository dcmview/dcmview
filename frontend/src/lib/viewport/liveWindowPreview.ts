export type PreviewWindow = { wc: number; ww: number };

export type LiveWindowPreviewOptions<Frame> = {
	/** Fetches the server-windowed frame for one window. */
	load: (window: PreviewWindow, signal: AbortSignal) => Promise<Frame>;
	/** Draws a preview that arrived while the drag is still live. */
	show: (frame: Frame) => Promise<void> | void;
};

/**
 * Server-windowed previews while a window/level drag moves over a frame the
 * browser cannot window: one request in flight at a time, only the newest
 * window queued behind it (older ones are dropped unsent), and the request
 * in flight aborted when the drag ends. Previews reach the server marked as
 * such, so they are never cached there.
 */
export class LiveWindowPreview<Frame> {
	readonly #load: LiveWindowPreviewOptions<Frame>["load"];
	readonly #show: LiveWindowPreviewOptions<Frame>["show"];
	#inFlight: AbortController | null = null;
	#queued: PreviewWindow | null = null;

	constructor({ load, show }: LiveWindowPreviewOptions<Frame>) {
		this.#load = load;
		this.#show = show;
	}

	request(window: PreviewWindow): void {
		if (this.#inFlight) {
			this.#queued = window;
			return;
		}
		void this.#send(window);
	}

	/** Drops the queued window and aborts the one in flight. */
	stop(): void {
		this.#queued = null;
		this.#inFlight?.abort();
		this.#inFlight = null;
	}

	async #send(window: PreviewWindow): Promise<void> {
		const controller = new AbortController();
		this.#inFlight = controller;
		try {
			const frame = await this.#load(window, controller.signal);
			if (!controller.signal.aborted) await this.#show(frame);
		} catch {
			// A failed preview is skipped; the settled window is fetched on release.
		}
		if (this.#inFlight !== controller) return;
		this.#inFlight = null;
		const next = this.#queued;
		this.#queued = null;
		if (next) void this.#send(next);
	}
}

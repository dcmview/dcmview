import { describe, expect, it, vi } from "vitest";
import { LiveWindowPreview, type PreviewWindow } from "./liveWindowPreview";

function deferred() {
	let resolve!: (blob: Blob) => void;
	const promise = new Promise<Blob>((done) => { resolve = done; });
	return { promise, resolve };
}

describe("LiveWindowPreview", () => {
	it("keeps one request in flight and sends only the newest queued window after it", async () => {
		const pending: { window: PreviewWindow; reply: ReturnType<typeof deferred> }[] = [];
		const show = vi.fn();
		const preview = new LiveWindowPreview({
			load: (window) => {
				const reply = deferred();
				pending.push({ window, reply });
				return reply.promise;
			},
			show,
		});

		preview.request({ wc: 1, ww: 10 });
		preview.request({ wc: 2, ww: 10 });
		preview.request({ wc: 3, ww: 10 });
		expect(pending.map(({ window }) => window.wc)).toEqual([1]);

		pending[0].reply.resolve(new Blob(["1"]));
		await vi.waitFor(() => expect(pending.map(({ window }) => window.wc)).toEqual([1, 3]));
		expect(show).toHaveBeenCalledOnce();
	});

	it("aborts the request in flight and drops the queue when stopped", async () => {
		const signals: AbortSignal[] = [];
		const show = vi.fn();
		const load = vi.fn((_window: PreviewWindow, signal: AbortSignal) => {
			signals.push(signal);
			return new Promise<Blob>((_, reject) => signal.addEventListener("abort", () => reject(new Error("aborted"))));
		});
		const preview = new LiveWindowPreview({ load, show });

		preview.request({ wc: 1, ww: 10 });
		preview.request({ wc: 2, ww: 10 });
		preview.stop();
		await Promise.resolve();

		expect(signals[0].aborted).toBe(true);
		expect(load).toHaveBeenCalledOnce();
		expect(show).not.toHaveBeenCalled();
	});
});

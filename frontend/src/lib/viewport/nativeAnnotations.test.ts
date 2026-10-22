import { describe, expect, it, vi } from "vitest";
import { ApiError } from "../../api";
import type { NativeAnnotationsResponse } from "../annotation/nativeAnnotations";
import { nativeLayer, nativeRecord, nativeResponse } from "../../testing/nativeAnnotations";
import { NativeAnnotations, RETAINED_FILES } from "./nativeAnnotations.svelte";

async function flush(): Promise<void> {
	for (let index = 0; index < 6; index += 1) await Promise.resolve();
}

function deferred<Value>() {
	let resolve!: (value: Value) => void;
	let reject!: (error: unknown) => void;
	const promise = new Promise<Value>((res, rej) => {
		resolve = res;
		reject = rej;
	});
	return { promise, resolve, reject };
}

/** What a session without a document reads: EMBED rectangles under the implicit schema. */
function embedOnly(revision = 1): NativeAnnotationsResponse {
	return {
		schema: { schema_id: "implicit", schema_version: 1, fields: [], classes: [
			{ id: "roi", name: "ROI", geometry: ["rect"], attributes: [], deprecated: false },
		] },
		layers: [nativeLayer("L-user", "Default", null)],
		annotations: [nativeRecord("r0", "roi", { type: "rect", x0: 1, y0: 1, x1: 5, y1: 5 }, { embed_index: 0 })],
		labels: [],
		revision,
	};
}

describe("NativeAnnotations", () => {
	it("reads a file once when it is shown", async () => {
		const load = vi.fn(async (_file: number, _signal: AbortSignal) => nativeResponse());
		const store = new NativeAnnotations({ load });

		store.ensureLoaded(2);
		expect(store.status(2)).toBe("loading");
		store.ensureLoaded(2);
		await flush();

		expect(load).toHaveBeenCalledOnce();
		expect(load.mock.calls[0][0]).toBe(2);
		expect(store.status(2)).toBe("ready");
		expect(store.response(2)?.revision).toBe(7);
	});

	it("has nothing native for a 404, an empty read, or one the EMBED view already shows", async () => {
		const load = vi.fn()
			.mockRejectedValueOnce(new ApiError("not found", 404, null))
			.mockResolvedValueOnce(nativeResponse({ annotations: [], labels: [] }))
			.mockResolvedValueOnce(embedOnly());
		const store = new NativeAnnotations({ load });

		for (const file of [0, 1, 2]) store.ensureLoaded(file);
		await flush();

		for (const file of [0, 1, 2]) {
			expect(store.status(file)).toBe("none");
			expect(store.response(file)).toBeNull();
		}
	});

	it("shows an implicit-schema read with more than EMBED rectangles", async () => {
		const withLine = embedOnly();
		withLine.annotations.push(nativeRecord("l0", "roi", { type: "line", points: [{ x: 0, y: 0 }, { x: 2, y: 2 }] }));
		const store = new NativeAnnotations({ load: vi.fn(async () => withLine) });

		store.ensureLoaded(0);
		await flush();

		expect(store.response(0)).toBe(withLine);
	});

	it("shows EMBED rectangles in a layer with its own color", async () => {
		const colored = embedOnly();
		colored.layers = [nativeLayer("L-user", "Reader", "#00ff00")];
		const store = new NativeAnnotations({ load: vi.fn(async () => colored) });

		store.ensureLoaded(0);
		await flush();

		expect(store.response(0)).toBe(colored);
	});

	it("reads the file again after a save, keeping the copy shown until the new one arrives", async () => {
		const next = deferred<NativeAnnotationsResponse>();
		const load = vi.fn()
			.mockResolvedValueOnce(nativeResponse({ revision: 3 }))
			.mockReturnValueOnce(next.promise);
		const store = new NativeAnnotations({ load });
		store.ensureLoaded(1);
		await flush();

		store.refresh(1);
		expect(store.response(1)?.revision).toBe(3);
		next.resolve(nativeResponse({ revision: 4 }));
		await flush();

		expect(load).toHaveBeenCalledTimes(2);
		expect(store.response(1)?.revision).toBe(4);
	});

	it("drops a read that a later one overtook, and aborts it", async () => {
		const first = deferred<NativeAnnotationsResponse>();
		const second = deferred<NativeAnnotationsResponse>();
		const signals: AbortSignal[] = [];
		const load = vi.fn((_file: number, signal: AbortSignal) => {
			signals.push(signal);
			return signals.length === 1 ? first.promise : second.promise;
		});
		const store = new NativeAnnotations({ load });

		store.ensureLoaded(1);
		store.refresh(1);
		expect(signals[0].aborted).toBe(true);
		second.resolve(nativeResponse({ revision: 9 }));
		await flush();
		first.resolve(nativeResponse({ revision: 8 }));
		await flush();

		expect(store.response(1)?.revision).toBe(9);
	});

	it("keeps the copy shown over one read at an older revision", async () => {
		const load = vi.fn()
			.mockResolvedValueOnce(nativeResponse({ revision: 5 }))
			.mockResolvedValueOnce(nativeResponse({ revision: 4, annotations: [] }));
		const store = new NativeAnnotations({ load });
		store.ensureLoaded(0);
		await flush();

		store.refresh(0);
		await flush();

		expect(store.response(0)?.revision).toBe(5);
		expect(store.response(0)?.annotations.length).toBeGreaterThan(0);
	});

	it("keeps files apart: a read of one file never lands on another", async () => {
		const reads: Record<number, NativeAnnotationsResponse> = { 0: nativeResponse({ revision: 1 }), 1: embedOnly(2) };
		const store = new NativeAnnotations({ load: vi.fn(async (file: number) => reads[file]) });

		store.ensureLoaded(0);
		store.ensureLoaded(1);
		await flush();

		expect(store.response(0)?.revision).toBe(1);
		expect(store.response(1)).toBeNull();
	});

	it("forgets a file's copy, drops its read in flight, and reads it again when shown", async () => {
		const inFlight = deferred<NativeAnnotationsResponse>();
		const load = vi.fn()
			.mockResolvedValueOnce(nativeResponse({ revision: 1 }))
			.mockReturnValueOnce(inFlight.promise)
			.mockResolvedValueOnce(nativeResponse({ revision: 3 }));
		const store = new NativeAnnotations({ load });
		store.ensureLoaded(0);
		await flush();

		store.refresh(0);
		store.forget(0);
		expect(store.response(0)).toBeNull();
		expect(store.status(0)).toBeNull();
		inFlight.resolve(nativeResponse({ revision: 2 }));
		await flush();
		expect(store.response(0)).toBeNull();

		store.ensureLoaded(0);
		await flush();
		expect(load).toHaveBeenCalledTimes(3);
		expect(store.response(0)?.revision).toBe(3);
	});

	it("keeps an earlier copy when a read fails, and shows nothing when the first one does", async () => {
		const load = vi.fn()
			.mockRejectedValueOnce(new ApiError("boom", 500, null))
			.mockResolvedValueOnce(nativeResponse({ revision: 2 }))
			.mockRejectedValueOnce(new ApiError("boom", 500, null));
		const store = new NativeAnnotations({ load });

		store.ensureLoaded(0);
		await flush();
		expect(store.status(0)).toBe("error");
		expect(store.response(0)).toBeNull();

		store.refresh(0);
		await flush();
		store.refresh(0);
		await flush();
		expect(store.response(0)?.revision).toBe(2);
	});

	it("keeps copies of the files read last only, and reads an evicted one again when shown", async () => {
		const load = vi.fn(async (file: number) => nativeResponse({ revision: file }));
		const store = new NativeAnnotations({ load });

		for (let file = 0; file <= RETAINED_FILES; file += 1) store.ensureLoaded(file);
		await flush();

		expect(store.response(0)).toBeNull();
		expect(store.response(1)?.revision).toBe(1);
		expect(store.response(RETAINED_FILES)?.revision).toBe(RETAINED_FILES);
		store.ensureLoaded(0);
		await flush();
		expect(load).toHaveBeenCalledTimes(RETAINED_FILES + 2);
		expect(store.response(0)?.revision).toBe(0);
	});

	it("keeps the copy of a file shown again before it would be evicted", async () => {
		const load = vi.fn(async (file: number) => nativeResponse({ revision: file }));
		const store = new NativeAnnotations({ load });

		store.ensureLoaded(0);
		for (let file = 1; file < RETAINED_FILES; file += 1) store.ensureLoaded(file);
		store.ensureLoaded(0);
		store.ensureLoaded(RETAINED_FILES);
		await flush();

		expect(store.response(0)?.revision).toBe(0);
		expect(store.response(1)).toBeNull();
	});

	it("aborts the read of a file it evicts", () => {
		const signals: AbortSignal[] = [];
		const store = new NativeAnnotations({ load: vi.fn((_file: number, signal: AbortSignal) => {
			signals.push(signal);
			return new Promise<NativeAnnotationsResponse>(() => {});
		}) });

		for (let file = 0; file <= RETAINED_FILES; file += 1) store.ensureLoaded(file);

		expect(signals[0].aborted).toBe(true);
		expect(signals.slice(1).every((signal) => !signal.aborted)).toBe(true);
	});

	it("toggles layer and class visibility as view state", () => {
		const load = vi.fn();
		const store = new NativeAnnotations({ load });

		store.toggleLayer("L-model");
		store.toggleClass("mass");
		expect([...store.hiddenLayers]).toEqual(["L-model"]);
		expect([...store.hiddenClasses]).toEqual(["mass"]);
		store.toggleLayer("L-model");
		expect(store.hiddenLayers.size).toBe(0);
		expect(load).not.toHaveBeenCalled();
	});
});

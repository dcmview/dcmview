import { describe, expect, it, vi } from "vitest";
import type { EmbedRoiAnnotations } from "../../api";
import { AnnotationStore } from "./annotationStore.svelte";

function set(...coords: EmbedRoiAnnotations["roi_coords"]): EmbedRoiAnnotations {
	return { num_roi: coords.length, roi_coords: coords, roi_frames: coords.map(() => [0]) };
}

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

describe("AnnotationStore", () => {
	it("loads a file once and becomes editable only after it loads", async () => {
		const load = vi.fn(async () => set([1, 1, 5, 5]));
		const save = vi.fn(async (_file: number, value: EmbedRoiAnnotations) => value);
		const store = new AnnotationStore({ load, save });

		store.commit(3, set([0, 0, 2, 2]), 0);
		expect(save).not.toHaveBeenCalled();

		store.ensureLoaded(3);
		expect(store.loading(3)).toBe(true);
		store.ensureLoaded(3);
		await flush();

		expect(load).toHaveBeenCalledOnce();
		expect(store.ready(3)).toBe(true);
		expect(store.annotations(3)).toEqual(set([1, 1, 5, 5]));
	});

	it("reports a load failure and retries only on request", async () => {
		const load = vi.fn()
			.mockRejectedValueOnce(new Error("offline"))
			.mockResolvedValueOnce(set());
		const store = new AnnotationStore({ load, save: vi.fn() });

		store.ensureLoaded(1);
		await flush();
		expect(store.error(1)).toBe("offline");
		store.ensureLoaded(1);
		expect(load).toHaveBeenCalledOnce();

		store.retryLoad(1);
		await flush();
		expect(store.error(1)).toBeNull();
		expect(store.ready(1)).toBe(true);
	});

	it("saves commits, tracks selection, and surfaces save errors", async () => {
		const save = vi.fn().mockRejectedValueOnce(new Error("disk full"));
		const store = new AnnotationStore({ load: async () => set(), save });
		store.ensureLoaded(2);
		await flush();

		store.commit(2, set([0, 0, 4, 4]), 0);
		expect(store.selected(2)).toBe(0);
		expect(store.saveStatus(2)).toBe("saving");
		await flush();
		expect(store.error(2)).toBe("disk full");

		store.rollback(2);
		expect(store.annotations(2)).toEqual(set());
		expect(store.selected(2)).toBeNull();
	});

	it("keeps a dragged ROI's live geometry while an earlier save completes", async () => {
		const pending = deferred<EmbedRoiAnnotations>();
		const store = new AnnotationStore({ load: async () => set(), save: () => pending.promise });
		store.ensureLoaded(4);
		await flush();
		store.commit(4, set([0, 0, 4, 4]), 0);

		store.beginLiveEdit(4);
		store.showDraft(4, set([2, 2, 6, 6]));
		pending.resolve(set([0, 0, 4, 4]));
		await flush();
		expect(store.annotations(4)).toEqual(set([2, 2, 6, 6]));
		store.endLiveEdit();
	});
});

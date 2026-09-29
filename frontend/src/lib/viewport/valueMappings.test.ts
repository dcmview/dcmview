import { describe, expect, it, vi } from "vitest";
import type { FrameValueMapping } from "../../api";
import { ValueMappings } from "./valueMappings.svelte";

const mapping: FrameValueMapping = {
	file_index: 1, frame_index: 0, stored_value_type: "integer",
	modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: null },
	real_world: [], voi_lut: null,
};

describe("ValueMappings request ownership", () => {
	it("cancels a mapping only after its last consumer leaves", async () => {
		let requestSignal: AbortSignal | undefined;
		const load = vi.fn(async (_file: number, _frame: number, signal?: AbortSignal) => {
			requestSignal = signal;
			return new Promise<FrameValueMapping>(() => {});
		});
		const mappings = new ValueMappings(load);
		const first = new AbortController(), second = new AbortController();
		const a = mappings.load(1, 0, first.signal), b = mappings.load(1, 0, second.signal);
		expect(load).toHaveBeenCalledOnce();
		first.abort();
		expect(requestSignal?.aborted).toBe(false);
		second.abort();
		expect(requestSignal?.aborted).toBe(true);
		expect(await Promise.all([a, b])).toEqual([null, null]);
		expect(mappings.failed(1, 0)).toBe(false);
	});

	it("retains a settled mapping and ignores a later consumer cancellation", async () => {
		const load = vi.fn(async () => mapping);
		const mappings = new ValueMappings(load);
		const controller = new AbortController();
		expect(await mappings.load(1, 0, controller.signal)).toEqual(mapping);
		controller.abort();
		expect(await mappings.load(1, 0)).toEqual(mapping);
		expect(load).toHaveBeenCalledOnce();
	});
});

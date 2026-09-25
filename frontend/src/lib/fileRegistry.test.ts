import { describe, expect, it } from "vitest";
import { indexFilesById, resolveFilesById, reuseUnchangedEntries } from "./fileRegistry";

describe("file registry", () => {
	it("resolves non-dense identifiers independently of array position", () => {
		const files = [
			{ index: 42, label: "forty-two" },
			{ index: 3, label: "three" },
			{ index: 900, label: "nine-hundred" },
		];
		const byId = indexFilesById(files);

		expect(byId.get(3)?.label).toBe("three");
		expect(byId.get(42)?.label).toBe("forty-two");
		expect(byId.get(0)).toBeUndefined();
	});

	it("preserves requested tab order when the source array is reordered", () => {
		const files = [
			{ index: 8, label: "eight" },
			{ index: 2, label: "two" },
			{ index: 5, label: "five" },
		];
		const resolved = resolveFilesById(indexFilesById(files), [5, 8, 2]);
		expect(resolved.map((file) => file.label)).toEqual(["five", "eight", "two"]);
	});

	it("rejects duplicate identifiers at the contract boundary", () => {
		expect(() => indexFilesById([{ index: 5 }, { index: 5 }]))
			.toThrow("duplicate file index 5");
	});

	it("keeps unchanged entries identical across catalog polls", () => {
		const previous = [
			{ index: 0, frames: [0], label: "stable" },
			{ index: 1, frames: [0], label: "growing" },
		];
		const next = [
			{ index: 0, frames: [0], label: "stable" },
			{ index: 1, frames: [0, 1], label: "growing" },
			{ index: 2, frames: [0], label: "new" },
		];

		const merged = reuseUnchangedEntries(previous, next, (entry) => entry.index);

		expect(merged[0]).toBe(previous[0]);
		expect(merged[1]).toBe(next[1]);
		expect(merged[2]).toBe(next[2]);
	});
});

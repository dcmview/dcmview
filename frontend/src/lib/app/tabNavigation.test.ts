import { describe, expect, it, vi } from "vitest";
import type { FileSummary, SeriesSummary } from "../../api";
import { TabNavigation } from "./tabNavigation.svelte";

function file(index: number, frameCount = 1): FileSummary {
	return { index, frame_count: frameCount } as FileSummary;
}

/** A series whose single stack holds one frame from each file. */
function stackSeries(id: string, fileIndexes: number[]): SeriesSummary {
	return {
		stacks: [{
			id,
			frames: fileIndexes.map((file_index, virtual_index) => ({ virtual_index, file_index, frame_index: 0 })),
		}],
	} as unknown as SeriesSummary;
}

function navigation(files: FileSummary[], series: SeriesSummary[] = []) {
	const catalog = { files: new Map(files.map((entry) => [entry.index, entry])), series };
	const ontabchange = vi.fn();
	const onfilechange = vi.fn();
	const tabs = new TabNavigation({
		series: () => catalog.series,
		files: () => catalog.files,
		ontabchange,
		onfilechange,
	});
	return { tabs, catalog, ontabchange, onfilechange };
}

describe("TabNavigation", () => {
	it("opens a lone multiframe file as its own tab and steps through its frames", () => {
		const { tabs } = navigation([file(3, 4)]);
		tabs.open(3);

		expect(tabs.activeTabId).toBe("file:3");
		expect(tabs.scopeKey).toBe("file:3");
		expect(tabs.frames).toHaveLength(4);
		tabs.setStackPosition(9);
		expect([tabs.currentFrame, tabs.stackPosition]).toEqual([3, 3]);
		expect(tabs.tabs[0]).toMatchObject({ currentFrame: 3, stackPosition: 3 });
	});

	it("keeps one tab per stack and restores where each tab was left", () => {
		const { tabs, ontabchange, onfilechange } = navigation(
			[file(1), file(2), file(3), file(9, 5)],
			[stackSeries("stack:a", [1, 2, 3])],
		);
		tabs.open(1);
		tabs.setStackPosition(2);
		expect(tabs.activeFileIndex).toBe(3);

		tabs.open(9);
		tabs.setStackPosition(4);
		tabs.activate(3);

		expect(tabs.tabs.map((tab) => tab.id)).toEqual(["stack:a", "file:9"]);
		expect([tabs.activeTabId, tabs.activeFileIndex, tabs.stackPosition]).toEqual(["stack:a", 3, 2]);
		tabs.activate(9);
		expect([tabs.currentFrame, tabs.stackPosition]).toEqual([4, 4]);
		expect(ontabchange).toHaveBeenCalledTimes(4);
		expect(onfilechange.mock.calls.map(([index]) => index)).toEqual([1, 3, 9, 3, 9]);
	});

	it("reopening a stack from another file jumps to that file", () => {
		const { tabs } = navigation([file(1), file(2), file(3)], [stackSeries("stack:a", [1, 2, 3])]);
		tabs.open(1);
		tabs.open(3);
		expect(tabs.tabs).toHaveLength(1);
		expect([tabs.activeFileIndex, tabs.stackPosition]).toEqual([3, 2]);
	});

	it("activates the neighbour when the active tab closes", () => {
		const { tabs } = navigation([file(1), file(2), file(3)]);
		tabs.open(1);
		tabs.open(2);
		tabs.open(3);
		tabs.activate(2);

		tabs.close(2);
		expect(tabs.activeFileIndex).toBe(3);
		tabs.close(1);
		expect(tabs.activeFileIndex).toBe(3);
		tabs.close(3);
		expect([tabs.activeTabId, tabs.activeFileIndex, tabs.scopeKey]).toEqual([null, null, ""]);
	});

	it("opens reference targets at their frame and ignores invalid frames", () => {
		const { tabs } = navigation([file(1, 3), file(2, 2)]);
		tabs.open(1);

		tabs.openReference(2, 5);
		expect(tabs.activeFileIndex).toBe(1);
		tabs.openReference(2, 1);
		expect([tabs.activeTabId, tabs.currentFrame]).toEqual(["file:2", 1]);
	});

	it("opens the first file once and retargets a tab whose file joins a stack", () => {
		const { tabs, catalog, ontabchange } = navigation([file(1), file(2)]);
		tabs.syncCatalog(1);
		tabs.syncCatalog(1);
		expect(tabs.tabs.map((tab) => tab.id)).toEqual(["file:1"]);

		catalog.series = [stackSeries("stack:a", [2, 1])];
		ontabchange.mockClear();
		tabs.syncCatalog(1);

		expect(tabs.tabs.map((tab) => tab.id)).toEqual(["stack:a"]);
		expect([tabs.activeTabId, tabs.stackPosition]).toEqual(["stack:a", 1]);
		expect(tabs.frameCounts.get(1)).toBe(2);
		expect(ontabchange).toHaveBeenCalledOnce();
	});
});

import { afterEach, describe, expect, it, vi } from "vitest";
import * as api from "../../api";
import type { FileSummary, FilesResponse } from "../../api";
import { Catalog } from "./catalog.svelte";

vi.mock("../../api", () => ({
	fetchFiles: vi.fn(),
	fetchSeries: vi.fn(),
}));

function filesResponse(count: number, scanComplete: boolean): FilesResponse {
	return {
		files: Array.from({ length: count }, (_, index) => ({ index }) as FileSummary),
		discovery: [],
		server_start_ms: 0,
		scan_complete: scanComplete,
		scanned: count,
		skipped: 0,
		filtered: 0,
	} as FilesResponse;
}

async function settle(ms: number): Promise<void> {
	await vi.advanceTimersByTimeAsync(ms);
}

describe("Catalog.poll", () => {
	it("refreshes references for arrivals and completion, not filtered progress", () => {
		const catalog = new Catalog();
		const series = { series: [], scan_complete: false };
		catalog.apply(filesResponse(1, false), series);
		const revision = catalog.referenceRevision;
		catalog.apply({ ...filesResponse(1, false), scanned: 200, skipped: 100, filtered: 99 }, series);
		expect(catalog.referenceRevision).toBe(revision);
		expect(catalog.files?.filtered).toBe(99);
		catalog.apply(filesResponse(2, false), series);
		expect(catalog.referenceRevision).not.toBe(revision);
		const arrived = catalog.referenceRevision;
		catalog.apply(filesResponse(2, true), { ...series, scan_complete: true });
		expect(catalog.referenceRevision).not.toBe(arrived);
	});

	afterEach(() => {
		vi.useRealTimers();
		vi.resetAllMocks();
	});

	it("fetches the series catalog and notifies only when the scan moved", async () => {
		vi.useFakeTimers();
		const files = vi.mocked(api.fetchFiles);
		const series = vi.mocked(api.fetchSeries);
		series.mockResolvedValue({ series: [], scan_complete: false });
		files.mockResolvedValue(filesResponse(1, false));
		const onupdate = vi.fn();
		const stop = new Catalog().poll(onupdate);

		await settle(0);
		expect(onupdate).toHaveBeenCalledTimes(1);
		// The scan has not moved: no series request, no update, and the next
		// poll waits longer.
		await settle(500);
		expect(files).toHaveBeenCalledTimes(2);
		expect(series).toHaveBeenCalledTimes(1);
		expect(onupdate).toHaveBeenCalledTimes(1);
		await settle(500);
		expect(files).toHaveBeenCalledTimes(2);

		files.mockResolvedValue(filesResponse(2, true));
		series.mockResolvedValue({ series: [], scan_complete: true });
		await settle(500);
		expect(series).toHaveBeenCalledTimes(2);
		expect(onupdate).toHaveBeenCalledTimes(2);

		// Complete: polling stops.
		await settle(5000);
		expect(files).toHaveBeenCalledTimes(3);
		stop();
	});
});

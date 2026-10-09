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
		masked: false,
		scan_complete: scanComplete,
		scanned: count,
		skipped: 0,
		filtered: 0,
		revision: count,
		reset: false,
		more: false,
		keys_hashing: 0,
		rekeys: [],
	} as FilesResponse;
}

async function settle(ms: number): Promise<void> {
	await vi.advanceTimersByTimeAsync(ms);
}

describe("Catalog path display", () => {
	const named = (masked: boolean): FilesResponse => ({
		...filesResponse(0, true),
		masked,
		files: [{ index: 0, path: "/data/MRN-1/scan.dcm", display_name: masked ? "File 1" : "scan.dcm" } as FileSummary],
	});
	const series = { series: [], scan_complete: true };

	it("shows real paths in an unmasked session", () => {
		const catalog = new Catalog();
		catalog.apply(named(false), series);

		expect(catalog.pathsShown).toBe(true);
		expect(catalog.filesById.get(0)).toBe(catalog.files?.files[0]);
	});

	it("shows a masked session's paths only while the directory tree is showing", () => {
		const catalog = new Catalog();
		catalog.apply(named(true), series);

		expect(catalog.masked).toBe(true);
		expect(catalog.filesById.get(0)?.path).toBe("File 1");
		expect(catalog.files?.files[0].path).toBe("/data/MRN-1/scan.dcm");
		const hidden = catalog.filesById.get(0);

		catalog.directoryView = true;
		expect(catalog.filesById.get(0)?.path).toBe("/data/MRN-1/scan.dcm");

		catalog.directoryView = false;
		expect(catalog.filesById.get(0)).toBe(hidden);
	});
});

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

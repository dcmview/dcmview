import { afterEach, describe, expect, it, vi } from "vitest";
import {
	ApiError,
	displayFrameCacheKey,
	displayFrameWindowCacheKey,
	fetchDoseOverlayBlob,
	fetchDisplayFrameBlob,
	fetchDoseOverlayValues,
	fetchFiles,
	fetchRawFrame,
	fetchSelectedTag,
	frameUrl,
	isApiError,
	onReachabilityChange,
	parseRawFrameMetadata,
	UNREACHABLE_STATUS,
	updateAnnotations,
} from "./api";
import { RAW_FRAME_HEADERS } from "./generated/api-types";

function completeRawHeaders(): Headers {
	return new Headers({
		[RAW_FRAME_HEADERS.rows]: "512",
		[RAW_FRAME_HEADERS.columns]: "256",
		[RAW_FRAME_HEADERS.bitsAllocated]: "16",
		[RAW_FRAME_HEADERS.pixelRepresentation]: "1",
		[RAW_FRAME_HEADERS.samplesPerPixel]: "1",
		[RAW_FRAME_HEADERS.photometricInterpretation]: "MONOCHROME1",
		[RAW_FRAME_HEADERS.rescaleSlope]: "2.5",
		[RAW_FRAME_HEADERS.rescaleIntercept]: "-1024",
		[RAW_FRAME_HEADERS.defaultWc]: "40",
		[RAW_FRAME_HEADERS.defaultWw]: "80",
		[RAW_FRAME_HEADERS.paddingLow]: "-2000",
		[RAW_FRAME_HEADERS.paddingHigh]: "-1000",
	});
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe("parseRawFrameMetadata", () => {
	it("maps the generated raw-frame header contract", () => {
		expect(parseRawFrameMetadata(completeRawHeaders())).toEqual({
			rows: 512,
			columns: 256,
			bitsAllocated: 16,
			pixelRepresentation: 1,
			samplesPerPixel: 1,
			photometricInterpretation: "MONOCHROME1",
			rescaleSlope: 2.5,
			rescaleIntercept: -1024,
			defaultWc: 40,
			defaultWw: 80,
			paddingLow: -2000,
			paddingHigh: -1000,
		});
	});

	it("represents absent optional window and padding headers as null", () => {
		const headers = completeRawHeaders();
		headers.delete(RAW_FRAME_HEADERS.defaultWc);
		headers.delete(RAW_FRAME_HEADERS.defaultWw);
		headers.delete(RAW_FRAME_HEADERS.paddingLow);
		headers.delete(RAW_FRAME_HEADERS.paddingHigh);

		expect(parseRawFrameMetadata(headers)).toMatchObject({
			defaultWc: null,
			defaultWw: null,
			paddingLow: null,
			paddingHigh: null,
		});
	});

	it("rejects missing or partially numeric required headers", () => {
		const missing = completeRawHeaders();
		missing.delete(RAW_FRAME_HEADERS.rows);
		expect(() => parseRawFrameMetadata(missing)).toThrow(
			`raw frame response missing required header ${RAW_FRAME_HEADERS.rows}`,
		);

		const malformed = completeRawHeaders();
		malformed.set(RAW_FRAME_HEADERS.columns, "256px");
		expect(() => parseRawFrameMetadata(malformed)).toThrow(
			`raw frame response has invalid integer header ${RAW_FRAME_HEADERS.columns}`,
		);
	});
});

describe("display frame cache keys", () => {
	it("canonicalizes absent window values and includes the window mode", () => {
		expect(displayFrameWindowCacheKey()).toBe("default:none:none");
		expect(displayFrameWindowCacheKey({ wc: null, ww: undefined })).toBe(
			"default:none:none",
		);
		expect(displayFrameCacheKey(2, 7, { windowMode: "full_dynamic" })).toBe(
			"2:7:full_dynamic:none:none",
		);
		expect(
			displayFrameCacheKey(2, 7, {
				windowMode: "full_dynamic",
				wc: 40,
				ww: 80,
			}),
		).toBe("2:7:full_dynamic:none:none");
	});

	it("does not conflate distinct window parameters sent to the backend", () => {
		const first = displayFrameCacheKey(0, 0, { wc: 1.00001, ww: 2.00001 });
		const second = displayFrameCacheKey(0, 0, { wc: 1.00002, ww: 2.00002 });

		expect(first).not.toBe(second);
	});
});

describe("display frame URLs", () => {
	it("sends explicit windows only outside full-dynamic mode", () => {
		expect(frameUrl(2, 7)).toBe("/api/file/2/frame/7");
		expect(frameUrl(2, 7, { wc: 40, ww: 80, windowMode: "default" })).toBe(
			"/api/file/2/frame/7?wc=40&ww=80",
		);
		expect(frameUrl(2, 7, { wc: 40, ww: 80, windowMode: "full_dynamic" })).toBe(
			"/api/file/2/frame/7?mode=full_dynamic",
		);
		expect(frameUrl(2, 7, { wc: 1.5, ww: 3, windowMode: "default", unit: "SUV" })).toBe(
			"/api/file/2/frame/7?wc=1.5&ww=3&unit=SUV",
		);
		expect(frameUrl(2, 7, { wc: 1.5, ww: 3, windowMode: "full_dynamic", unit: "SUV" })).toBe(
			"/api/file/2/frame/7?mode=full_dynamic",
		);
		expect(frameUrl(2, 7, { wc: 40, ww: 80, preview: true })).toBe(
			"/api/file/2/frame/7?wc=40&ww=80&preview=true",
		);
	});
});

function jsonResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { "Content-Type": "application/json" },
	});
}

describe("fetch wrappers", () => {
	it("surfaces the JSON error envelope message", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn().mockResolvedValue(
				jsonResponse({ code: "not_found", error: "file index out of range" }, 404),
			),
		);

		await expect(fetchFiles()).rejects.toThrow("file index out of range");
	});

	it("keeps the envelope's status and code for callers that branch on them", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn().mockResolvedValue(
				jsonResponse({ code: "overlay_not_covering_frame", error: "beyond the dose grid" }, 404),
			),
		);

		const error = await fetchDoseOverlayBlob(5, 0, 9).catch((caught: unknown) => caught);

		expect(error).toBeInstanceOf(ApiError);
		expect(error).toMatchObject({ status: 404, code: "overlay_not_covering_frame" });
		expect(isApiError(error, "overlay_not_covering_frame")).toBe(true);
		expect(isApiError(error, "not_found")).toBe(false);
	});

	it("falls back to the HTTP status when the error body is not an envelope", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn().mockResolvedValue(new Response("gateway down", { status: 502 })),
		);

		await expect(fetchFiles()).rejects.toThrow("HTTP 502");
	});

	it("reports an unreachable server once and its return", async () => {
		const changes: boolean[] = [];
		const unsubscribe = onReachabilityChange((reachable) => changes.push(reachable));
		vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("Failed to fetch")));

		const error = await fetchFiles().catch((caught: unknown) => caught);
		await fetchFiles().catch(() => {});
		expect(error).toMatchObject({ status: UNREACHABLE_STATUS, code: null });
		expect((error as Error).message).toContain("not reachable");

		vi.stubGlobal("fetch", vi.fn().mockResolvedValue(jsonResponse({ files: [] }, 200)));
		await fetchFiles();
		expect(changes).toEqual([false, true]);
		unsubscribe();
	});

	it("sends a real-world window with its unit", async () => {
		const fetchMock = vi.fn().mockResolvedValue(new Response(new Blob(), { status: 200 }));
		vi.stubGlobal("fetch", fetchMock);
		await fetchDisplayFrameBlob(1, 0, { wc: 12, ww: 20, unit: "Gy" });
		expect(fetchMock.mock.calls[0][0]).toBe("/api/file/1/frame/0?wc=12&ww=20&unit=Gy");
		expect(displayFrameWindowCacheKey({ wc: 12, ww: 20, windowMode: "default", unit: "Gy" }))
			.toBe("default:12:20:Gy");
	});

	it("reads overlay values as little-endian f32 samples", async () => {
		const bytes = new DataView(new ArrayBuffer(12));
		bytes.setFloat32(0, 16.2, true);
		bytes.setFloat32(4, Number.NaN, true);
		bytes.setFloat32(8, -1.5, true);
		const fetchMock = vi.fn().mockResolvedValue(new Response(bytes.buffer, { status: 200 }));
		vi.stubGlobal("fetch", fetchMock);

		const values = await fetchDoseOverlayValues(5, 0, 9);

		expect(fetchMock.mock.calls[0][0]).toBe("/api/file/5/frame/0/dose-overlay/values?dose=9");
		expect(values[0]).toBeCloseTo(16.2, 5);
		expect(Number.isNaN(values[1])).toBe(true);
		expect(values[2]).toBe(-1.5);
	});

	it("forwards the abort signal and parses raw-frame headers", async () => {
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(new Uint8Array([1, 2]), { status: 200, headers: completeRawHeaders() }),
		);
		vi.stubGlobal("fetch", fetchMock);
		const controller = new AbortController();

		const frame = await fetchRawFrame(4, 2, controller.signal);

		expect(frame.buffer.byteLength).toBe(2);
		expect(frame.metadata.photometricInterpretation).toBe("MONOCHROME1");
		expect(fetchMock).toHaveBeenCalledWith("/api/file/4/frame/2/raw", {
			method: "GET",
			signal: controller.signal,
		});
	});

	it("sends annotation edits as a JSON PUT", async () => {
		const annotations = {
			num_roi: 1,
			roi_coords: [[1, 2, 3, 4] as [number, number, number, number]],
			roi_frames: [[0]],
		};
		const fetchMock = vi.fn().mockResolvedValue(jsonResponse(annotations));
		vi.stubGlobal("fetch", fetchMock);

		await expect(updateAnnotations(7, annotations)).resolves.toEqual(annotations);
		expect(fetchMock).toHaveBeenCalledWith("/api/file/7/annotations", {
			method: "PUT",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(annotations),
		});
	});

	it("encodes selective tag paths and sequence pages", async () => {
		const fetchMock = vi.fn().mockResolvedValue(
			jsonResponse({ tag: "(0008,2218)", vr: "SQ", keyword: "", value: { type: "sequence", items: [] } }),
		);
		vi.stubGlobal("fetch", fetchMock);

		await fetchSelectedTag(4, { path: "(0008,2218)/69/(0008,0100)", offset: 2, limit: 8 });

		expect(fetchMock.mock.calls[0][0]).toBe(
			"/api/file/4/tags/select?path=%280008%2C2218%29%2F69%2F%280008%2C0100%29&offset=2&limit=8",
		);
	});
});

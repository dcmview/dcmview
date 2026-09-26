import { afterEach, describe, expect, it, vi } from "vitest";
import {
	annotationsExportUrl,
	displayFrameCacheKey,
	displayFrameWindowCacheKey,
	fetchFiles,
	fetchReferences,
	fetchSegmentationOverlayBlob,
	fetchSeries,
	fetchSelectedTag,
	frameUrl,
	parseRawFrameMetadata,
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
	it("uses the generated route and query contract", () => {
		expect(frameUrl(2, 7)).toBe("/api/file/2/frame/7");
		expect(frameUrl(2, 7, 40, 80, "default")).toBe(
			"/api/file/2/frame/7?wc=40&ww=80",
		);
		expect(frameUrl(2, 7, 40, 80, "full_dynamic")).toBe(
			"/api/file/2/frame/7?mode=full_dynamic",
		);
	});

	it("anchors the annotations export link to the declared GET endpoint", () => {
		expect(annotationsExportUrl()).toBe("/api/annotations/export.csv");
	});
});

describe("generated endpoint fetch wrappers", () => {
	it("uses the declared GET operation and inferred files response", async () => {
		const payload = { files: [], scan_complete: true };
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify(payload), {
				status: 200,
				headers: { "Content-Type": "application/json" },
			}),
		);
		vi.stubGlobal("fetch", fetchMock);

		await expect(fetchFiles()).resolves.toEqual(payload);
		expect(fetchMock).toHaveBeenCalledWith("/api/files", { method: "GET" });
	});

	it("uses the declared series catalog endpoint", async () => {
		const payload = { series: [], scan_complete: true };
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify(payload), {
				status: 200,
				headers: { "Content-Type": "application/json" },
			}),
		);
		vi.stubGlobal("fetch", fetchMock);

		await expect(fetchSeries()).resolves.toEqual(payload);
		expect(fetchMock).toHaveBeenCalledWith("/api/series", { method: "GET" });
	});

	it("uses the declared file reference endpoint", async () => {
		const payload = {
			source_file_index: 7,
			source_sop_instance_uid: "1.2.3",
			references: [],
		};
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify(payload), {
				status: 200,
				headers: { "Content-Type": "application/json" },
			}),
		);
		vi.stubGlobal("fetch", fetchMock);

		await expect(fetchReferences(7)).resolves.toEqual(payload);
		expect(fetchMock).toHaveBeenCalledWith("/api/file/7/references", { method: "GET" });
	});

	it("uses the declared segmentation overlay endpoint", async () => {
		const payload = new Blob(["png"], { type: "image/png" });
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(payload, {
				status: 200,
				headers: { "Content-Type": "image/png" },
			}),
		);
		vi.stubGlobal("fetch", fetchMock);
		const controller = new AbortController();

		await expect(fetchSegmentationOverlayBlob(4, 2, controller.signal)).resolves.toBeInstanceOf(Blob);
		expect(fetchMock).toHaveBeenCalledWith("/api/file/4/frame/2/segmentation-overlay", {
			signal: controller.signal,
		});
	});

	it("uses the annotation update verb, body, and inferred response contract", async () => {
		const annotations = {
			num_roi: 1,
			roi_coords: [[1, 2, 3, 4] as [number, number, number, number]],
			roi_frames: [[0]],
		};
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify(annotations), {
				status: 200,
				headers: { "Content-Type": "application/json" },
			}),
		);
		vi.stubGlobal("fetch", fetchMock);

		await expect(updateAnnotations(7, annotations)).resolves.toEqual(annotations);
		expect(fetchMock).toHaveBeenCalledWith("/api/file/7/annotations", {
			method: "PUT",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(annotations),
		});
	});

	it("encodes selective tag paths and sequence pages", async () => {
		const node = {
			tag: "(0008,2218)",
			vr: "SQ",
			keyword: "AnatomicRegionSequence",
			value: { type: "sequence", items: [], truncated: true, total: 70 },
		};
		const fetchMock = vi.fn().mockResolvedValue(
			new Response(JSON.stringify(node), {
				status: 200,
				headers: { "Content-Type": "application/json" },
			}),
		);
		vi.stubGlobal("fetch", fetchMock);

		await expect(
			fetchSelectedTag(4, { path: "(0008,2218)/69/(0008,0100)", offset: 2, limit: 8 }),
		).resolves.toEqual(node);
		expect(fetchMock).toHaveBeenCalledWith(
			"/api/file/4/tags/select?path=%280008%2C2218%29%2F69%2F%280008%2C0100%29&offset=2&limit=8",
			{ method: "GET", signal: undefined },
		);
	});

	it("rejects a successful status that differs from the endpoint contract", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn().mockResolvedValue(
				new Response("{}", {
					status: 201,
					headers: { "Content-Type": "application/json" },
				}),
			),
		);

		await expect(fetchFiles()).rejects.toThrow("HTTP 201");
	});
});

// @vitest-environment happy-dom
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import { fileSummary, rawFrame } from "../testing/fixtures";
import ImageViewport from "./ImageViewport.svelte";
import { navigationFramesForFile } from "./seriesNavigation";
import type { ActiveTool } from "./viewerTools";
import { ViewStates } from "./viewport/viewStates.svelte";

vi.mock("../api", async (importOriginal) => ({
	...await importOriginal<typeof import("../api")>(),
	fetchAnnotations: vi.fn(async () => ({ num_roi: 0, roi_coords: [], roi_frames: [] })),
	fetchDisplayFrameBlob: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
	fetchRawFrame: vi.fn(),
	fetchFrameValueMapping: vi.fn(),
	updateAnnotations: vi.fn(),
}));

const fetchDisplayFrameBlob = vi.mocked(api.fetchDisplayFrameBlob);
const fetchRawFrame = vi.mocked(api.fetchRawFrame);
const fetchFrameValueMapping = vi.mocked(api.fetchFrameValueMapping);

function renderViewport({
	activeTool = "pan" as ActiveTool,
	file = fileSummary(5),
	windowCenter = null as number | null,
	windowWidth = null as number | null,
} = {}) {
	return render(ImageViewport, {
		activeFile: file,
		currentFrame: 0,
		windowCenter,
		windowWidth,
		activeTool,
		windowMode: "default",
		viewStates: new ViewStates(),
		cinePlaying: false,
		cineFps: 10,
		cineMode: "loop",
		cineDirection: 1,
		navigationFrameCount: file.frame_count,
		navigationFrames: navigationFramesForFile(file.index, file.frame_count),
		navigationScopeKey: `file:${file.index}`,
		navigationPosition: 0,
		onnavigationchange: vi.fn(),
		onreset: vi.fn(),
		onmanualwindowlevel: vi.fn(),
	});
}

beforeEach(() => {
	fetchDisplayFrameBlob.mockClear();
	fetchRawFrame.mockReset();
	fetchRawFrame.mockResolvedValue(rawFrame());
	fetchFrameValueMapping.mockReset();
});

describe("ImageViewport window/level path", () => {
	it("shows server-rendered PNGs outside the window/level tool", async () => {
		renderViewport({ activeTool: "pan" });

		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
		expect(fetchRawFrame).not.toHaveBeenCalled();
	});

	it("requests the display window the viewer selected", async () => {
		renderViewport({ activeTool: "pan", windowCenter: 60, windowWidth: 400 });

		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(
			5,
			0,
			{ wc: 60, ww: 400, windowMode: "default" },
			expect.any(AbortSignal),
		));
	});

	it("windows raw samples in the browser for compatible files", async () => {
		renderViewport({ activeTool: "window_level" });

		await waitFor(() => expect(fetchRawFrame).toHaveBeenCalledWith(5, 0, expect.any(AbortSignal)));
		expect(fetchDisplayFrameBlob).not.toHaveBeenCalled();
		expect(screen.queryByText("server presentation retained")).toBeNull();
	});

	it("keeps server presentation for files the raw renderer cannot window", async () => {
		renderViewport({
			activeTool: "window_level",
			file: fileSummary(5, { raw_windowing_compatible: false, raw_windowing_reason: "32-bit float" }),
		});

		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalled());
		expect(fetchRawFrame).not.toHaveBeenCalled();
		expect(screen.getByText("server presentation retained").getAttribute("title")).toBe("32-bit float");
	});

	it("falls back to server presentation when a raw frame is not renderable", async () => {
		fetchRawFrame.mockResolvedValue(rawFrame(64, 64, 32));
		renderViewport({ activeTool: "window_level" });

		await waitFor(() => expect(fetchRawFrame).toHaveBeenCalledOnce());
		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
	});

	it("shows a placeholder instead of fetching frames for metadata-only objects", async () => {
		renderViewport({ file: fileSummary(2, { has_pixels: false }) });

		expect(screen.getByText("No pixel data")).toBeTruthy();
		await Promise.resolve();
		expect(fetchDisplayFrameBlob).not.toHaveBeenCalled();
		expect(fetchRawFrame).not.toHaveBeenCalled();
	});
});

describe("ImageViewport pixel readout", () => {
	function ctFrame(): api.RawFrame {
		const frame = rawFrame(64, 64, 16);
		new Uint16Array(frame.buffer)[20 * 64 + 10] = 1100;
		return frame;
	}

	it("reads stored and Modality values under the cursor", async () => {
		fetchRawFrame.mockResolvedValue(ctFrame());
		fetchFrameValueMapping.mockResolvedValue({
			file_index: 5,
			frame_index: 0,
			stored_value_type: "integer",
			modality: { rescale_slope: 1, rescale_intercept: -1024, rescale_type: null, lut: null },
			real_world: [],
		});
		renderViewport();
		const viewport = await screen.findByRole("application");

		// happy-dom lays the unfitted image out at the viewport origin, one pixel per pixel.
		await fireEvent.pointerMove(viewport, { clientX: 10.5, clientY: 20.5 });

		const readout = await screen.findByRole("status", { name: "Pixel value under cursor" });
		expect(readout.textContent).toContain("row 20 · col 10 · frame 1");
		await waitFor(() => expect(readout.textContent).toContain("stored 1100"));
		expect(readout.textContent).toContain("modality 76 HU");
		expect(fetchRawFrame).toHaveBeenCalledWith(5, 0, expect.any(AbortSignal));
		expect(fetchFrameValueMapping).toHaveBeenCalledWith(5, 0, expect.any(AbortSignal));

		await fireEvent.pointerLeave(viewport);
		expect(screen.queryByRole("status", { name: "Pixel value under cursor" })).toBeNull();
	});

	it("keeps coordinates when the raw endpoint cannot serve the file", async () => {
		fetchRawFrame.mockRejectedValue(new api.ApiError("unsupported layout", 422, "unsupported_pixel_layout"));
		fetchFrameValueMapping.mockResolvedValue({
			file_index: 5,
			frame_index: 0,
			stored_value_type: "integer",
			modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: null },
			real_world: [],
		});
		renderViewport();
		const viewport = await screen.findByRole("application");

		await fireEvent.pointerMove(viewport, { clientX: 3, clientY: 4 });

		const readout = await screen.findByRole("status", { name: "Pixel value under cursor" });
		await waitFor(() => expect(readout.textContent).toContain("value unavailable (display only)"));
		expect(readout.textContent).toContain("row 4 · col 3");
	});
});

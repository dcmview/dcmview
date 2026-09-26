// @vitest-environment happy-dom
import { render, screen, waitFor } from "@testing-library/svelte";
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
	updateAnnotations: vi.fn(),
}));

const fetchDisplayFrameBlob = vi.mocked(api.fetchDisplayFrameBlob);
const fetchRawFrame = vi.mocked(api.fetchRawFrame);

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

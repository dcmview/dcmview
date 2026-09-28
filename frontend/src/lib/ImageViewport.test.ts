// @vitest-environment happy-dom
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import { fileSummary, rawFrame } from "../testing/fixtures";
import ImageViewport from "./ImageViewport.svelte";
import * as frameOverlay from "./viewport/frameOverlay";
import { navigationFramesForFile } from "./seriesNavigation";
import type { ActiveTool } from "./viewerTools";
import { ViewStates } from "./viewport/viewStates.svelte";

vi.mock("../api", async (importOriginal) => ({
	...await importOriginal<typeof import("../api")>(),
	fetchAnnotations: vi.fn(async () => ({ num_roi: 0, roi_coords: [], roi_frames: [] })),
	fetchDisplayFrameBlob: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
	fetchRawFrame: vi.fn(),
	fetchFrameValueMapping: vi.fn(),
	fetchDoseOverlayBlob: vi.fn(),
	fetchDoseOverlayValues: vi.fn(),
	updateAnnotations: vi.fn(),
}));

// happy-dom cannot decode PNGs or draw on a canvas; the layer is recorded.
vi.mock("./viewport/frameOverlay", async (importOriginal) => ({
	...await importOriginal<typeof import("./viewport/frameOverlay")>(),
	decodeCanvasImage: vi.fn(async () => ({ source: {}, width: 64, height: 64, dispose: vi.fn() })),
	drawOverlayLayer: vi.fn(),
}));

const fetchDisplayFrameBlob = vi.mocked(api.fetchDisplayFrameBlob);
const fetchRawFrame = vi.mocked(api.fetchRawFrame);
const fetchFrameValueMapping = vi.mocked(api.fetchFrameValueMapping);
const fetchDoseOverlayBlob = vi.mocked(api.fetchDoseOverlayBlob);
const drawOverlayLayer = vi.mocked(frameOverlay.drawOverlayLayer);

function identityMapping(fileIndex = 5): api.FrameValueMapping {
	return {
		file_index: fileIndex,
		frame_index: 0,
		stored_value_type: "integer",
		modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: null },
		real_world: [],
		voi_lut: null,
	};
}

function adcMapping(): api.FrameValueMapping {
	return {
		...identityMapping(),
		real_world: [{
			source: "real_world_value_mapping",
			source_file_index: null,
			label: "ADC",
			first_value_mapped: 0,
			last_value_mapped: 4095,
			transform: { kind: "linear", slope: 0.5, intercept: -10 },
			unit_label: "um2/s",
			units: null,
			quantity: null,
		}],
	};
}

function renderViewport({
	activeTool = "pan" as ActiveTool,
	file = fileSummary(5),
	windowCenter = null as number | null,
	windowWidth = null as number | null,
	windowUnit = null as string | null,
	onmanualwindowlevel = vi.fn(),
	valueOverlay = null as frameOverlay.ValueOverlay | null,
} = {}) {
	return render(ImageViewport, {
		activeFile: file,
		currentFrame: 0,
		windowCenter,
		windowWidth,
		windowUnit,
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
		onmanualwindowlevel,
		valueOverlay,
	});
}

beforeEach(() => {
	fetchDisplayFrameBlob.mockClear();
	fetchRawFrame.mockReset();
	fetchRawFrame.mockResolvedValue(rawFrame());
	fetchFrameValueMapping.mockReset();
	fetchFrameValueMapping.mockResolvedValue(identityMapping());
	fetchDoseOverlayBlob.mockReset();
	fetchDoseOverlayBlob.mockResolvedValue(new Blob(["png"], { type: "image/png" }));
	drawOverlayLayer.mockClear();
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

	it("falls back to server presentation only when the raw endpoint refuses the layout", async () => {
		fetchRawFrame.mockRejectedValue(new api.ApiError("unsupported layout", 422, "unsupported_pixel_layout"));
		renderViewport({ activeTool: "window_level" });
		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
	});

	it("reports a failed raw request without giving up client windowing", async () => {
		fetchRawFrame.mockRejectedValue(new TypeError("Failed to fetch"));
		renderViewport({ activeTool: "window_level" });

		expect(await screen.findByText("Failed to fetch")).toBeTruthy();
		expect(fetchDisplayFrameBlob).not.toHaveBeenCalled();
		expect(screen.queryByText("server presentation retained")).toBeNull();
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
			voi_lut: null,
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
			voi_lut: null,
		});
		renderViewport();
		const viewport = await screen.findByRole("application");

		await fireEvent.pointerMove(viewport, { clientX: 3, clientY: 4 });

		const readout = await screen.findByRole("status", { name: "Pixel value under cursor" });
		await waitFor(() => expect(readout.textContent).toContain("value unavailable (display only)"));
		expect(readout.textContent).toContain("row 4 · col 3");
	});
});

function lutMapping(): api.FrameValueMapping {
	return {
		...identityMapping(),
		real_world: [{
			source: "real_world_value_mapping",
			source_file_index: null,
			label: "T1",
			first_value_mapped: 0,
			last_value_mapped: 3,
			transform: { kind: "lut", values: [0, 10, 40, 90] },
			unit_label: "ms",
			units: null,
			quantity: null,
		}],
	};
}

describe("ImageViewport window/level in real-world units", () => {
	it("windows a LUT mapping's values on the raw path and reports drags in its unit", async () => {
		fetchFrameValueMapping.mockResolvedValue(lutMapping());
		const onmanualwindowlevel = vi.fn();
		renderViewport({ activeTool: "window_level", onmanualwindowlevel });
		const viewport = await screen.findByRole("application");
		// The all-zero frame maps to 0 ms everywhere, so its automatic window
		// is one stored unit (30 ms) wide.
		await screen.findByText("W: 30 · C: 15 ms");
		expect(screen.getByRole("figure", { name: "T1: 0 to 30 ms" })).toBeTruthy();

		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });

		// 30 ms per stored unit: 10 px widen the window by 10 × 4 × 30 ms.
		expect(onmanualwindowlevel).toHaveBeenCalledWith(15, 1230, "ms");
	});

	it("windows a linear mapping behind a Modality LUT directly on the raw path", async () => {
		fetchFrameValueMapping.mockResolvedValue({
			...adcMapping(),
			modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: { first_value_mapped: 0, values: [0, 7] } },
		});
		renderViewport({ activeTool: "window_level" });

		// mapped = 0.5 × stored − 10: the all-zero frame is −10 um2/s everywhere,
		// so its automatic window is one stored unit (0.5 um2/s) wide.
		await screen.findByText("W: 0.5 · C: -9.75 um2/s");
	});

	it("keeps a still frame with a LUT-unit window on the raw path in any tool", async () => {
		fetchFrameValueMapping.mockResolvedValue(lutMapping());
		renderViewport({ activeTool: "pan", windowCenter: 40, windowWidth: 80, windowUnit: "ms" });

		await waitFor(() => expect(fetchRawFrame).toHaveBeenCalledWith(5, 0, expect.any(AbortSignal)));
		await screen.findByText("W: 80 · C: 40 ms");
		expect(fetchDisplayFrameBlob).not.toHaveBeenCalled();
	});

	it("converts a real-world window to stored units before requesting the frame", async () => {
		fetchFrameValueMapping.mockResolvedValue(adcMapping());
		renderViewport({ windowCenter: 40, windowWidth: 100, windowUnit: "um2/s" });

		// mapped = 0.5 × stored − 10, so C 40 / W 100 um2/s is C 100 / W 200 stored.
		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(
			5,
			0,
			{ wc: 100, ww: 200, windowMode: "default" },
			expect.any(AbortSignal),
		));
		expect(fetchDisplayFrameBlob).toHaveBeenCalledOnce();
		expect(screen.getByText(/W: 100 · C: 40 um2\/s/)).toBeTruthy();
		expect(screen.getByRole("figure", { name: "ADC: -10 to 90 um2/s" })).toBeTruthy();
	});

	it("converts the window through each frame's own mapping as frames change", async () => {
		// Frame f maps stored values with slope (f + 1) / 2 um2/s.
		fetchFrameValueMapping.mockImplementation(async (fileIndex, frameIndex) => {
			const base = adcMapping();
			const [map] = base.real_world;
			return {
				...base,
				file_index: fileIndex,
				frame_index: frameIndex,
				real_world: [{ ...map, transform: { kind: "linear", slope: (frameIndex + 1) / 2, intercept: 0 } }],
			};
		});
		const file = fileSummary(5, { frame_count: 3 });
		const view = renderViewport({ file, windowCenter: 30, windowWidth: 60, windowUnit: "um2/s" });

		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(
			5, 0, { wc: 60, ww: 120, windowMode: "default" }, expect.any(AbortSignal),
		));
		await view.rerender({ currentFrame: 2, navigationPosition: 2 });
		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(
			5, 2, { wc: 20, ww: 40, windowMode: "default" }, expect.any(AbortSignal),
		));
		expect(fetchFrameValueMapping).toHaveBeenCalledWith(5, 2, expect.any(AbortSignal));
	});

	it("shows the default window on files without that unit", async () => {
		renderViewport({ windowCenter: 40, windowWidth: 100, windowUnit: "Gy" });

		// The server shows the frame's default window for a unit it lacks.
		await waitFor(() => expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(
			5,
			0,
			{ wc: 40, ww: 100, windowMode: "default", unit: "Gy" },
			expect.any(AbortSignal),
		));
		expect(screen.getByText("W: 400 · C: 40")).toBeTruthy();
		expect(screen.queryByRole("figure")).toBeNull();
	});

	it("reports a window/level drag in the mapping's unit", async () => {
		fetchFrameValueMapping.mockResolvedValue(adcMapping());
		const onmanualwindowlevel = vi.fn();
		renderViewport({ activeTool: "window_level", onmanualwindowlevel });
		const viewport = await screen.findByRole("application");
		// The all-zero raw frame's automatic window, C 0.5 / W 1 stored, in um2/s.
		await screen.findByText(/W: 0.5 · C: -9.75 um2\/s/);

		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });

		// Stored W 1 + 10 px × 4 = 41 → 20.5 um2/s; C 0.5 stored → -9.75 um2/s.
		expect(onmanualwindowlevel).toHaveBeenCalledWith(-9.75, 20.5, "um2/s");
	});
});

describe("ImageViewport value overlays", () => {
	const legend: api.OverlayLegend = {
		unit_label: "Gy",
		units: null,
		min_value: 0,
		max_value: 23.3,
		transparent_at_or_below: 0,
		colormap: "viridis",
		color_stops: [[68, 1, 84], [253, 231, 37]],
	};

	function dose(overrides: Partial<frameOverlay.ValueOverlay> = {}): frameOverlay.ValueOverlay {
		return { kind: "rt_dose", volumeFileIndex: 9, title: "RT Dose", legend, opacity: 0.4, coversFrame: true, ...overrides };
	}

	function layerCanvas(): HTMLCanvasElement | null {
		return document.querySelector(".value-overlay-canvas");
	}

	it("draws the dose colorwash over the frame at the chosen opacity, with a Gy legend", async () => {
		renderViewport({ valueOverlay: dose() });

		await waitFor(() => expect(drawOverlayLayer).toHaveBeenCalledOnce());
		expect(fetchDoseOverlayBlob).toHaveBeenCalledWith(5, 0, 9, expect.any(AbortSignal));
		// The image underneath keeps its own render path and window.
		expect(fetchDisplayFrameBlob).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal));
		await waitFor(() => expect(layerCanvas()?.hidden).toBe(false));
		expect(layerCanvas()?.style.opacity).toBe("0.4");
		const legendFigure = screen.getByRole("figure", { name: "RT Dose: 0 to 23.3 Gy" });
		expect(legendFigure.textContent).toContain("≤ 0 Gy transparent");
	});

	it("skips frames the dose context does not cover", async () => {
		renderViewport({ valueOverlay: dose({ coversFrame: false }) });

		expect(await screen.findByText("Not covering this frame")).toBeTruthy();
		expect(fetchDoseOverlayBlob).not.toHaveBeenCalled();
		expect(layerCanvas()?.hidden).toBe(true);
	});

	it("hides the layer when the server says the grid misses the frame", async () => {
		fetchDoseOverlayBlob.mockRejectedValue(
			new api.ApiError("beyond the dose grid", 404, "overlay_not_covering_frame"),
		);
		renderViewport({ valueOverlay: dose() });

		expect(await screen.findByText("Not covering this frame")).toBeTruthy();
		expect(drawOverlayLayer).not.toHaveBeenCalled();
		expect(layerCanvas()?.hidden).toBe(true);
	});

	it("adds the overlaid dose under the cursor to the readout", async () => {
		const values = new Float32Array(64 * 64).fill(Number.NaN);
		values[20 * 64 + 10] = 16.2;
		vi.mocked(api.fetchDoseOverlayValues).mockResolvedValue(values);
		renderViewport({ valueOverlay: dose() });
		const viewport = await screen.findByRole("application");

		await fireEvent.pointerMove(viewport, { clientX: 10.5, clientY: 20.5 });

		const readout = await screen.findByRole("status", { name: "Pixel value under cursor" });
		await waitFor(() => expect(readout.textContent).toContain("dose 16.2 Gy"));
		expect(api.fetchDoseOverlayValues).toHaveBeenCalledWith(5, 0, 9, expect.any(AbortSignal));

		await fireEvent.pointerMove(viewport, { clientX: 3.5, clientY: 4.5 });
		await waitFor(() => expect(readout.textContent).toContain("outside the volume"));
		expect(api.fetchDoseOverlayValues).toHaveBeenCalledOnce();
	});

	it("says when a layer fails for another reason", async () => {
		fetchDoseOverlayBlob.mockRejectedValue(new api.ApiError("mapping unavailable", 422, "semantic_mapping_unavailable"));
		renderViewport({ valueOverlay: dose() });

		expect(await screen.findByText("Overlay unavailable for this frame")).toBeTruthy();
	});
});

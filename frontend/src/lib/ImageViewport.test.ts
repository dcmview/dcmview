// @vitest-environment happy-dom
import { act, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import { fileSummary, rawFrame } from "../testing/fixtures";
import ImageViewport from "./ImageViewport.svelte";
import * as frameOverlay from "./viewport/frameOverlay";
import { navigationFramesForFile } from "./seriesNavigation";
import type { ActiveTool } from "./viewerTools";
import { ViewStates } from "./viewport/viewStates.svelte";
import { reactiveProps } from "../testing/reactiveProps.svelte";
import type { ComponentProps } from "svelte";

vi.mock("../api", async (importOriginal) => ({
	...await importOriginal<typeof import("../api")>(),
	fetchAnnotations: vi.fn(async () => ({ num_roi: 0, roi_coords: [], roi_frames: [] })),
	fetchDisplayFrame: vi.fn(async () => ({ blob: new Blob(["png"], { type: "image/png" }), window: null, appliedWindow: null })),
	fetchRawFrame: vi.fn(),
	fetchFrameValueMapping: vi.fn(),
	fetchSelectedTag: vi.fn(),
	fetchDoseOverlayBlob: vi.fn(),
	fetchSegmentationOverlayBlob: vi.fn(async () => new Blob(["seg"])),
	fetchDoseOverlayValues: vi.fn(),
	fetchPresentationLayerBlob: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
	updateAnnotations: vi.fn(),
}));

// happy-dom cannot decode PNGs or draw on a canvas; the layer is recorded.
vi.mock("./viewport/frameOverlay", async (importOriginal) => ({
	...await importOriginal<typeof import("./viewport/frameOverlay")>(),
	decodeCanvasImage: vi.fn(async () => ({ source: {}, width: 64, height: 64, dispose: vi.fn() })),
	drawOverlayLayer: vi.fn(),
}));

const fetchDisplayFrame = vi.mocked(api.fetchDisplayFrame);
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
	overlay = null as frameOverlay.FrameOverlay | null,
} = {}) {
	const state = reactiveProps<ComponentProps<typeof ImageViewport>>({
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
		overlay,
	});
	const view = render(ImageViewport, { props: state.props });
	return { ...view, rerender: async (props: Partial<ComponentProps<typeof ImageViewport>>) => act(() => state.update(props)) };
}

beforeEach(() => {
	fetchDisplayFrame.mockReset();
	fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"], { type: "image/png" }), window: null, appliedWindow: null });
	fetchRawFrame.mockReset();
	fetchRawFrame.mockResolvedValue(rawFrame());
	fetchFrameValueMapping.mockReset();
	fetchFrameValueMapping.mockResolvedValue(identityMapping());
	vi.mocked(api.fetchSelectedTag).mockResolvedValue({ tag: "(0028,0004)", keyword: "PhotometricInterpretation", vr: "CS", value: { type: "string", value: "MONOCHROME2" } });
	fetchDoseOverlayBlob.mockReset();
	fetchDoseOverlayBlob.mockResolvedValue(new Blob(["png"], { type: "image/png" }));
	drawOverlayLayer.mockClear();
});

describe("ImageViewport window/level path", () => {
	it("shows server-rendered PNGs outside the window/level tool", async () => {
		renderViewport({ activeTool: "pan" });

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
		expect(fetchRawFrame).not.toHaveBeenCalled();
	});

	it("shows the window the server rendered a frame with when it chose it", async () => {
		fetchDisplayFrame.mockResolvedValueOnce({ blob: new Blob(["png"]), window: { wc: 1499.5, ww: 2970 }, appliedWindow: "linear" });
		renderViewport({ file: fileSummary(5, { default_window: null }) });

		await screen.findByText("W: 2970 · C: 1500");
	});

	it("shows no window for a frame rendered without a linear one", async () => {
		renderViewport({ file: fileSummary(5, { default_window: null }) });

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledOnce());
		expect(screen.queryByText(/W: /)).toBeNull();
	});

	it("starts a server-windowed drag from the window the frame was rendered with", async () => {
		fetchDisplayFrame.mockResolvedValueOnce({ blob: new Blob(["png"]), window: { wc: 1499.5, ww: 2970 }, appliedWindow: "linear" });
		const onmanualwindowlevel = vi.fn();
		renderViewport({
			activeTool: "window_level",
			file: fileSummary(5, { default_window: null, raw_windowing_compatible: false, raw_windowing_reason: "display shutter" }),
			onmanualwindowlevel,
		});
		const viewport = await screen.findByRole("application");
		await screen.findByText("W: 2970 · C: 1500");

		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });

		expect(onmanualwindowlevel).toHaveBeenCalledWith(1499.5, 3010, null);
	});

	it("requests the display window the viewer selected", async () => {
		renderViewport({ activeTool: "pan", windowCenter: 60, windowWidth: 400 });

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
			5,
			0,
			{ wc: 60, ww: 400, windowMode: "default" },
			expect.any(AbortSignal),
		));
	});

	it("windows raw samples in the browser for compatible files", async () => {
		renderViewport({ activeTool: "window_level" });

		await waitFor(() => expect(fetchRawFrame).toHaveBeenCalledWith(5, 0, expect.any(AbortSignal)));
		expect(fetchDisplayFrame).not.toHaveBeenCalled();
		expect(screen.queryByText("server presentation retained")).toBeNull();
	});

	it("keeps server presentation for files the raw renderer cannot window", async () => {
		renderViewport({
			activeTool: "window_level",
			file: fileSummary(5, { raw_windowing_compatible: false, raw_windowing_reason: "32-bit float" }),
		});

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalled());
		expect(fetchRawFrame).not.toHaveBeenCalled();
		expect(screen.getByText("server presentation retained").getAttribute("title")).toBe("32-bit float");
	});

	it("draws the frame's shutter and overlays over the browser-windowed image", async () => {
		renderViewport({ activeTool: "window_level", file: fileSummary(5, { presentation_layer: true }) });

		await waitFor(() => expect(api.fetchPresentationLayerBlob).toHaveBeenCalledWith(5, 0, expect.any(AbortSignal)));
		await waitFor(() => expect(drawOverlayLayer).toHaveBeenCalled());
		expect(fetchDisplayFrame).not.toHaveBeenCalled();
	});

	it("keeps server windowing when the file's value mapping cannot load", async () => {
		fetchFrameValueMapping.mockRejectedValue(new Error("mapping unavailable"));
		renderViewport({ activeTool: "window_level" });

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
	});

	it("previews a drag on frames too large for the browser with server windows", async () => {
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 70, ww: 400 }, appliedWindow: "linear" });
		renderViewport({ activeTool: "window_level", file: fileSummary(5, { rows: 5000, columns: 5000 }) });
		await screen.findByText("W: 400 · C: 70");
		const viewport = await screen.findByRole("application");

		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
			5,
			0,
			expect.objectContaining({ windowMode: "default", preview: true }),
			expect.any(AbortSignal),
		));
		expect(fetchRawFrame).not.toHaveBeenCalled();
	});

	it("falls back to server presentation when a raw frame is not renderable", async () => {
		fetchRawFrame.mockResolvedValue(rawFrame(64, 64, 12));
		renderViewport({ activeTool: "window_level" });

		await waitFor(() => expect(fetchRawFrame).toHaveBeenCalledOnce());
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
	});

	it("falls back to server presentation only when the raw endpoint refuses the layout", async () => {
		fetchRawFrame.mockRejectedValue(new api.ApiError("unsupported layout", 422, "unsupported_pixel_layout"));
		renderViewport({ activeTool: "window_level" });
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal)));
	});

	it("reports a failed raw request without giving up client windowing", async () => {
		fetchRawFrame.mockRejectedValue(new TypeError("Failed to fetch"));
		renderViewport({ activeTool: "window_level" });

		expect(await screen.findByText("Failed to fetch")).toBeTruthy();
		expect(fetchDisplayFrame).not.toHaveBeenCalled();
		expect(screen.queryByText("server presentation retained")).toBeNull();
	});

	it("shows a placeholder instead of fetching frames for metadata-only objects", async () => {
		renderViewport({ file: fileSummary(2, { has_pixels: false }) });

		expect(screen.getByText("No pixel data")).toBeTruthy();
		await Promise.resolve();
		expect(fetchDisplayFrame).not.toHaveBeenCalled();
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
		expect(fetchDisplayFrame).not.toHaveBeenCalled();
	});

	it("labels a LUT-unit window the server applied in its unit", async () => {
		fetchFrameValueMapping.mockResolvedValue(lutMapping());
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: null, appliedWindow: "real_world" });
		// Too large for the browser, so the server windows it in its unit.
		renderViewport({ file: fileSummary(5, { rows: 5000, columns: 5000 }), windowCenter: 40, windowWidth: 80, windowUnit: "ms" });

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
			5, 0, { wc: 40, ww: 80, windowMode: "default", unit: "ms" }, expect.any(AbortSignal),
		));
		// The applied-window header confirms that the requested unit was used.
		await screen.findByText("W: 80 · C: 40 ms");
	});

	it("labels a server VOI fallback instead of claiming that a requested LUT unit applied", async () => {
		fetchFrameValueMapping.mockResolvedValue(lutMapping());
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: null, appliedWindow: "voi_lut" });
		renderViewport({ file: fileSummary(5, { rows: 5000, columns: 5000 }), windowCenter: 40, windowWidth: 80, windowUnit: "ms" });
		await screen.findByText("VOI LUT");
		expect(screen.queryByText(/W: .*ms/)).toBeNull();
	});

	it("shows the window the server fell back to when a LUT-unit window could not apply", async () => {
		fetchFrameValueMapping.mockResolvedValue(lutMapping());
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 1.5, ww: 3 }, appliedWindow: "linear" });
		renderViewport({ file: fileSummary(5, { rows: 5000, columns: 5000 }), windowCenter: 40, windowWidth: 80, windowUnit: "ms" });

		await screen.findByText("W: 3 · C: 2");
		expect(screen.queryByText(/ms$/)).toBeNull();
	});

	it("converts a real-world window to an exact equivalent before requesting the frame", async () => {
		fetchFrameValueMapping.mockResolvedValue(adcMapping());
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 100.5, ww: 201 }, appliedWindow: "linear" });
		renderViewport({ windowCenter: 40, windowWidth: 100, windowUnit: "um2/s" });

		// C 100.5 / W 201 cancels integer LINEAR offsets for the physical C 40 / W 100.
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
			5,
			0,
			{ wc: 100.5, ww: 201, windowMode: "default" },
			expect.any(AbortSignal),
		));
		expect(fetchDisplayFrame).toHaveBeenCalledOnce();
		expect(screen.getByText(/W: 100 · C: 40 um2\/s/)).toBeTruthy();
		expect(screen.getByRole("figure", { name: "ADC: -10 to 90 um2/s" })).toBeTruthy();
	});

	it("shows a mapped file's automatic window in its unit without fetching raw samples", async () => {
		fetchFrameValueMapping.mockResolvedValue(adcMapping());
		fetchDisplayFrame.mockResolvedValueOnce({ blob: new Blob(["png"]), window: { wc: 100, ww: 200 }, appliedWindow: "linear" });
		renderViewport({ file: fileSummary(5, { default_window: null }) });

		// mapped = 0.5 × stored − 10: C 100 / W 200 stored is C 40 / W 100 um2/s.
		await screen.findByText(/W: 100 · C: 40 um2\/s/);
		expect(screen.getByRole("figure", { name: "ADC: -10 to 90 um2/s" })).toBeTruthy();
		expect(fetchRawFrame).not.toHaveBeenCalled();
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

		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
			5, 0, { wc: 60.5, ww: 121, windowMode: "default" }, expect.any(AbortSignal),
		));
		await view.rerender({ currentFrame: 2, navigationPosition: 2 });
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
			5, 2, { wc: 20.5, ww: 41, windowMode: "default" }, expect.any(AbortSignal),
		));
		expect(fetchFrameValueMapping).toHaveBeenCalledWith(5, 2, expect.any(AbortSignal));
	});

	it("shows the default window on files without that unit", async () => {
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 40, ww: 400 }, appliedWindow: "linear" });
		renderViewport({ windowCenter: 40, windowWidth: 100, windowUnit: "Gy" });

		// The server shows the frame's default window for a unit it lacks,
		// and reports it.
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(
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
		expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, {}, expect.any(AbortSignal));
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

// F11 suspects are reproduced before changing their resolution paths.
describe("ImageViewport window presentation consistency", () => {
	it("releases a manual window so a later preset and reset take effect", async () => {
		const { rerender } = renderViewport({ activeTool: "window_level" });
		await screen.findByText("W: 1 · C: 1");
		const viewport = screen.getByRole("application");
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { pointerId: 1 });
		await rerender({ windowCenter: 40, windowWidth: 80 });
		await screen.findByText("W: 80 · C: 40");
		await rerender({ windowCenter: null, windowWidth: null });
		await screen.findByText("W: 1 · C: 1");
	});

	it("starts a server LUT-unit drag in the displayed unit", async () => {
		fetchFrameValueMapping.mockResolvedValue(lutMapping());
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: null, appliedWindow: "real_world" });
		const onmanualwindowlevel = vi.fn();
		renderViewport({ activeTool: "window_level", file: fileSummary(5, { rows: 5000, columns: 5000 }),
			windowCenter: 40, windowWidth: 80, windowUnit: "ms", onmanualwindowlevel });
		await screen.findByText("W: 80 · C: 40 ms");
		const viewport = screen.getByRole("application");
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		expect(onmanualwindowlevel).toHaveBeenCalledWith(40, 1280, "ms");
		expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, expect.objectContaining({ unit: "ms", preview: true }), expect.any(AbortSignal));
	});

	it("inverts a mapped MONOCHROME1 legend with the pixels", async () => {
		const frame = rawFrame();
		frame.metadata.photometricInterpretation = "MONOCHROME1";
		fetchRawFrame.mockResolvedValue(frame);
		fetchFrameValueMapping.mockResolvedValue(adcMapping());
		renderViewport({ activeTool: "window_level" });
		const legend = await screen.findByRole("figure", { name: /ADC:/ });
		await waitFor(() => expect(legend.querySelector(".bar")?.getAttribute("style")).toContain("#fff, #000"));
	});

	it("inverts a mapped MONOCHROME1 legend on the display path without downloading raw samples", async () => {
		fetchFrameValueMapping.mockResolvedValue(adcMapping());
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 100, ww: 200 }, appliedWindow: "linear" });
		vi.mocked(api.fetchSelectedTag).mockResolvedValue({ tag: "(0028,0004)", keyword: "PhotometricInterpretation", vr: "CS", value: { type: "string", value: "MONOCHROME1" } });
		renderViewport();
		const legend = await screen.findByRole("figure", { name: /ADC:/ });
		await waitFor(() => expect(legend.querySelector(".bar")?.getAttribute("style")).toContain("#fff, #000"));
		expect(fetchRawFrame).not.toHaveBeenCalled();
	});

	it("does not preview or label a color frame that starts on server windowing", async () => {
		renderViewport({ activeTool: "window_level", file: fileSummary(5, { raw_windowing_compatible: false }) });
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledOnce());
		const viewport = screen.getByRole("application");
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		expect(fetchDisplayFrame).toHaveBeenCalledOnce();
		await waitFor(() => expect(screen.queryByText(/W: /)).toBeNull());
	});
});


describe("F11 remaining confirmations", () => {
	it("keeps a loaded SEG layer across catalog replacement and presets", async () => {
		const load = vi.mocked(api.fetchSegmentationOverlayBlob);
		load.mockClear();
		const file = fileSummary(5);
		const overlay: frameOverlay.FrameOverlay = { kind: "segmentation", segmentationFileIndex: 5,
			segmentationFrameIndex: 0, sourceFileIndex: 6, sourceFrameIndex: 0, sourceFile: fileSummary(6) };
		const { rerender } = renderViewport({ file, overlay });
		await waitFor(() => expect(load).toHaveBeenCalledOnce());
		await rerender({ activeFile: { ...file }, overlay: { ...overlay, sourceFile: { ...overlay.sourceFile } } });
		await rerender({ windowCenter: 40, windowWidth: 80 });
		expect(load).toHaveBeenCalledOnce();
	});

	it("aborts unit-window mapping prefetch when its display scope is abandoned", async () => {
		let pendingSignal: AbortSignal | undefined;
		fetchFrameValueMapping.mockImplementation(async (fileIndex, frameIndex, signal) => {
			if (fileIndex === 5 && frameIndex > 0) {
				pendingSignal = signal;
				return new Promise(() => {});
			}
			return { ...lutMapping(), file_index: fileIndex, frame_index: frameIndex };
		});
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: null, appliedWindow: "real_world" });
		const { rerender } = renderViewport({ file: fileSummary(5, { rows: 5000, columns: 5000, frame_count: 2 }),
			windowCenter: 40, windowWidth: 80, windowUnit: "ms" });
		await waitFor(() => expect(pendingSignal).toBeDefined());
		expect(pendingSignal?.aborted).toBe(false);
		await rerender({ windowUnit: null, windowCenter: 40, windowWidth: 80 });
		expect(pendingSignal?.aborted).toBe(true);
	});

	it("does not attribute held raw samples to a frame still loading", async () => {
		const frame = rawFrame(); new Uint8Array(frame.buffer).fill(123);
		fetchRawFrame.mockImplementation(async (_file, index) => index === 0 ? frame : new Promise(() => {}));
		fetchFrameValueMapping.mockImplementation(async (file, index) => ({ ...identityMapping(file), frame_index: index }));
		const { rerender } = renderViewport({ activeTool: "window_level", file: fileSummary(5, { frame_count: 2 }) });
		await fireEvent.pointerMove(screen.getByRole("application"), { clientX: 10.5, clientY: 20.5 });
		const readout = await screen.findByRole("status", { name: "Pixel value under cursor" });
		await waitFor(() => expect(readout.textContent).toContain("stored 123"));
		await rerender({ currentFrame: 1, navigationPosition: 1 });
		await waitFor(() => expect(fetchFrameValueMapping).toHaveBeenCalledWith(5, 1, expect.any(AbortSignal)));
		expect(readout.textContent).toContain("frame 2");
		expect(readout.textContent).not.toContain("stored 123");
	});
});


describe("ImageViewport frame presentation", () => {
	function canvasContext() {
		const context = { clearRect: vi.fn(), putImageData: vi.fn(), drawImage: vi.fn(),
			createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }) };
		const spy = vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
		return { context, restore: () => spy.mockRestore() };
	}

	it.each(["pan", "window_level"] as const)("holds pixels, colorwash, and label until the next colorwash is ready with %s", async (activeTool) => {
		const { context, restore } = canvasContext();
		vi.stubGlobal("createImageBitmap", vi.fn(async () => ({ width: 64, height: 64, close: vi.fn() })));
		const painted = activeTool === "pan" ? context.drawImage : context.putImageData;
		let finish!: (blob: Blob) => void;
		fetchDoseOverlayBlob.mockImplementation(async (file) => file === 5 ? new Blob(["first"]) : new Promise((resolve) => { finish = resolve; }));
		fetchFrameValueMapping.mockImplementation(async (file) => identityMapping(file));
		const valueOverlay: frameOverlay.ValueOverlay = { kind: "rt_dose", volumeFileIndex: 9, title: "Dose", opacity: 0.4, coversFrame: true,
			legend: { unit_label: "Gy", units: null, min_value: 0, max_value: 10, transparent_at_or_below: 0,
				colormap: "viridis", color_stops: [[68, 1, 84], [253, 231, 37]] } };
		try {
			const { rerender } = renderViewport({ activeTool, valueOverlay });
			await waitFor(() => expect(painted).toHaveBeenCalledOnce());
			const layer = document.querySelector<HTMLCanvasElement>(".value-overlay-canvas")!;
			expect(layer.hidden).toBe(false);
			expect(screen.getByText("image 1 / 1")).toBeTruthy();
			await rerender({ activeFile: fileSummary(6), navigationPosition: 1, navigationFrameCount: 2 });
			await waitFor(() => expect(fetchDoseOverlayBlob).toHaveBeenCalledWith(6, 0, 9, expect.any(AbortSignal)));
			expect(painted).toHaveBeenCalledOnce();
			expect(drawOverlayLayer).toHaveBeenCalledOnce();
			expect(layer.hidden).toBe(false);
			expect(screen.getByText("image 1 / 1")).toBeTruthy();
			await act(() => finish(new Blob(["second"])));
			await screen.findByText("image 2 / 2");
			expect(painted).toHaveBeenCalledTimes(2);
			expect(drawOverlayLayer).toHaveBeenCalledTimes(2);
			expect(layer.hidden).toBe(false);
		} finally { restore(); vi.unstubAllGlobals(); }
	});

	it("keeps the base image and reports a failed presentation layer", async () => {
		const { context, restore } = canvasContext();
		vi.mocked(api.fetchPresentationLayerBlob).mockRejectedValueOnce(new api.ApiError("injected failure", 500, "pixel_decode_failed"));
		try {
			renderViewport({ activeTool: "window_level", file: fileSummary(5, { presentation_layer: true }) });
			await screen.findByText("Presentation layer unavailable");
			expect(context.putImageData).toHaveBeenCalledOnce();
			expect(document.querySelector(".dicom-canvas")).toBeTruthy();
			expect(screen.queryByText("injected failure")).toBeNull();
		} finally { restore(); }
	});

	it("waits for a float64 frame's own mapping after displaying float32", async () => {
		const { context, restore } = canvasContext();
		let finish!: (mapping: api.FrameValueMapping) => void;
		fetchRawFrame.mockImplementation(async (file) => rawFrame(64, 64, file === 5 ? 32 : 64));
		fetchFrameValueMapping.mockImplementation(async (file) => file === 5
			? { ...identityMapping(file), stored_value_type: "float32" }
			: new Promise((resolve) => { finish = resolve; }));
		try {
			const { rerender } = renderViewport({ activeTool: "window_level" });
			await waitFor(() => expect(context.putImageData).toHaveBeenCalledOnce());
			await rerender({ activeFile: fileSummary(6), navigationPosition: 1, navigationFrameCount: 2 });
			await waitFor(() => expect(fetchFrameValueMapping).toHaveBeenCalledWith(6, 0, expect.any(AbortSignal)));
			expect(context.putImageData).toHaveBeenCalledOnce();
			await act(() => finish({ ...identityMapping(6), stored_value_type: "float64" }));
			await screen.findByText("image 2 / 2");
			expect(context.putImageData).toHaveBeenCalledTimes(2);
		} finally { restore(); }
	});

	it("keeps the painted image while the next file's raw frame is pending", async () => {
		const context = {
			clearRect: vi.fn(), putImageData: vi.fn(), drawImage: vi.fn(),
			createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
		};
		const canvas = vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
		try {
			fetchRawFrame.mockImplementation(async (file) => file === 5 ? rawFrame() : new Promise(() => {}));
			fetchFrameValueMapping.mockImplementation(async (file) => identityMapping(file));
			const { rerender } = renderViewport({ activeTool: "window_level" });
			await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
			context.clearRect.mockClear();
			await rerender({ activeFile: fileSummary(6) });
			await waitFor(() => expect(fetchRawFrame).toHaveBeenCalledWith(6, 0, expect.any(AbortSignal)));
			expect(context.clearRect).not.toHaveBeenCalled();
		} finally { canvas.mockRestore(); }
	});
});


describe("W/L cine", () => {
	it("plays display frames with the current window and returns to raw windowing when paused", async () => {
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 40, ww: 80 }, appliedWindow: "linear" });
		const onmanualwindowlevel = vi.fn();
		const { rerender } = renderViewport({ activeTool: "window_level", file: fileSummary(5, { frame_count: 3 }),
			windowCenter: 40, windowWidth: 80, onmanualwindowlevel });
		await screen.findByText("W: 80 · C: 40");
		await waitFor(() => expect(fetchRawFrame).toHaveBeenCalled());
		await rerender({ cinePlaying: true });
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(5, 0, { wc: 40, ww: 80, windowMode: "default" }, expect.any(AbortSignal)));
		await rerender({ cinePlaying: false });
		await screen.findByText("W: 80 · C: 40");
		const viewport = screen.getByRole("application");
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		expect(onmanualwindowlevel).toHaveBeenCalled();
		expect(fetchDisplayFrame.mock.calls.some((call) => call[2]?.preview)).toBe(false);
	});
});

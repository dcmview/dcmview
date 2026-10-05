// @vitest-environment happy-dom
import { act, createEvent, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import { fileSummary, rawFrame } from "../testing/fixtures";
import ImageViewport from "./ImageViewport.svelte";
import * as frameOverlay from "./viewport/frameOverlay";
import { navigationFramesForFile } from "./seriesNavigation";
import { TOOL_ORDER, type ActiveTool } from "./viewerTools";
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
	fetchGraphicAnnotations: vi.fn(),
	fetchSegmentationOverlayBlob: vi.fn(async () => new Blob(["seg"])),
	fetchDoseOverlayValues: vi.fn(),
	fetchPresentationLayerBlob: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
	updateAnnotations: vi.fn(),
	fetchRedactions: vi.fn(),
	updateRedactions: vi.fn(),
	applyRedactionsToSeries: vi.fn(),
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
	graphicAnnotation = null as ComponentProps<typeof ImageViewport>["graphicAnnotation"],
	onnavigationchange = vi.fn() as (position: number) => void,
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
		onnavigationchange,
		onreset: vi.fn(),
		onmanualwindowlevel,
		valueOverlay,
		overlay,
		graphicAnnotation,
	});
	const view = render(ImageViewport, { props: state.props });
	return { ...view, rerender: async (props: Partial<ComponentProps<typeof ImageViewport>>) => act(() => state.update(props)) };
}

beforeEach(() => {
	vi.mocked(api.fetchAnnotations).mockReset().mockResolvedValue({ num_roi: 0, roi_coords: [], roi_frames: [] });
	vi.mocked(api.updateAnnotations).mockReset().mockImplementation(async (_file, annotations) => annotations);
	vi.mocked(api.fetchRedactions).mockReset().mockResolvedValue({ num_roi: 0, roi_coords: [], roi_frames: [] });
	vi.mocked(api.applyRedactionsToSeries).mockReset().mockResolvedValue({ file_indices: [6] });
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

describe("ImageViewport redaction boxes", () => {
	it("lists ROIs, not redaction boxes, outside the Redact tool", async () => {
		renderViewport({ activeTool: "pan" });

		await screen.findByText("ROIs 0 / 0");
		expect(api.fetchRedactions).not.toHaveBeenCalled();
	});

	it("edits the file's redaction boxes with the Redact tool", async () => {
		vi.mocked(api.fetchRedactions).mockResolvedValue({ num_roi: 1, roi_coords: [[0, 0, 8, 32]], roi_frames: [] });
		renderViewport({ activeTool: "redact" });

		await screen.findByText("Redactions 1 / 1");
		expect(api.fetchRedactions).toHaveBeenCalledWith(5);
		expect(screen.getByText("[0, 0, 8, 32]")).toBeTruthy();
		expect(screen.getByText("all frames")).toBeTruthy();
	});

	it("copies the boxes to the series and fetches the frame again", async () => {
		vi.mocked(api.fetchRedactions).mockResolvedValue({ num_roi: 1, roi_coords: [[0, 0, 8, 32]], roi_frames: [] });
		renderViewport({ activeTool: "redact" });
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledOnce());

		await fireEvent.click(await screen.findByRole("button", { name: "Apply to series" }));

		await waitFor(() => expect(api.applyRedactionsToSeries).toHaveBeenCalledWith(5));
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledTimes(2));
	});

	it("offers no series copy for a file without boxes", async () => {
		renderViewport({ activeTool: "redact" });

		await screen.findByText("No redactions for this frame");
		expect(screen.queryByRole("button", { name: "Apply to series" })).toBeNull();
	});
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

		await screen.findByText("W: 3 · C: 1.5");
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
		await screen.findByText("W: 1 · C: 0.5");
		const viewport = screen.getByRole("application");
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { pointerId: 1 });
		await rerender({ windowCenter: 40, windowWidth: 80 });
		await screen.findByText("W: 80 · C: 40");
		await rerender({ windowCenter: null, windowWidth: null });
		await screen.findByText("W: 1 · C: 0.5");
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


function canvasContext() {
	const context = { clearRect: vi.fn(), putImageData: vi.fn(), drawImage: vi.fn(),
		createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }) };
	const spy = vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
	return { context, restore: () => spy.mockRestore() };
}

describe("ImageViewport frame presentation", () => {
	it("presents a display image before its delayed mapping and then updates its legend", async () => {
		const { context, restore } = canvasContext();
		vi.stubGlobal("createImageBitmap", vi.fn(async () => ({ width: 64, height: 64, close: vi.fn() })));
		let finish!: (mapping: api.FrameValueMapping) => void;
		fetchFrameValueMapping.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 100, ww: 200 }, appliedWindow: "linear" });
		renderViewport();
		await waitFor(() => expect(context.drawImage).toHaveBeenCalled());
		expect(screen.queryByRole("figure", { name: /ADC:/ })).toBeNull();
		await waitFor(() => expect(finish).toBeDefined());
		await act(() => finish(adcMapping()));
		await screen.findByRole("figure", { name: /ADC:/ });
		await screen.findByText("W: 100 · C: 40 um2/s");
		expect(fetchRawFrame).not.toHaveBeenCalled();
		restore();
	});

	it.each([false, true])("drags a small float window on its own scale (identity RWVM: %s)", async (mapped) => {
		const { context, restore } = canvasContext();
		const frame = rawFrame(64, 64, 32);
		new Float32Array(frame.buffer).set(Array.from({ length: 4096 }, (_, i) => 0.0005 + (i % 28) * 0.0001));
		fetchRawFrame.mockResolvedValue(frame);
		fetchFrameValueMapping.mockResolvedValue({ ...identityMapping(), stored_value_type: "float32",
			real_world: mapped ? [{ ...adcMapping().real_world[0], unit_label: "mm2/s", transform: { kind: "linear", slope: 1, intercept: 0 } }] : [] });
		const onmanualwindowlevel = vi.fn();
		try {
			renderViewport({ activeTool: "window_level", file: fileSummary(5, { default_window: null }), onmanualwindowlevel });
			await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
			const viewport = screen.getByRole("application");
			for (const dx of [1, -1000]) {
				await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
				await fireEvent.pointerMove(viewport, { clientX: 10 + dx, clientY: 11, pointerId: 1 });
				await fireEvent.pointerUp(viewport, { clientX: 10 + dx, clientY: 11, pointerId: 1 });
				const [center, width, unit] = onmanualwindowlevel.mock.lastCall!;
				expect(center).toBeGreaterThan(0.001);
				expect(center).toBeLessThan(0.003);
				expect(width).toBeGreaterThan(0);
				expect(width).toBeLessThan(dx === 1 ? 0.01 : 0.0001);
				expect(unit).toBe(mapped ? "mm2/s" : null);
			}
		} finally { restore(); }
	});

	it("drags a fractionally rescaled window by one stored unit", async () => {
		const { context, restore } = canvasContext();
		const frame = rawFrame(64, 64, 16);
		new Uint16Array(frame.buffer).set(Array.from({ length: 4096 }, (_, i) => i % 101));
		fetchRawFrame.mockResolvedValue(frame);
		fetchFrameValueMapping.mockResolvedValue({ ...identityMapping(),
			modality: { rescale_slope: 0.0001, rescale_intercept: 0, rescale_type: null, lut: null } });
		const onmanualwindowlevel = vi.fn();
		try {
			renderViewport({ activeTool: "window_level", file: fileSummary(5, { default_window: null }), onmanualwindowlevel });
			await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
			const viewport = screen.getByRole("application");
			for (const [dx, maximum] of [[1, 0.02], [-1000, 0.0001]]) {
				await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
				await fireEvent.pointerMove(viewport, { clientX: 10 + dx, clientY: 10, pointerId: 1 });
				await fireEvent.pointerUp(viewport, { clientX: 10 + dx, clientY: 10, pointerId: 1 });
				const [center, width] = onmanualwindowlevel.mock.lastCall!;
				expect(center).toBeGreaterThan(0);
				expect(center).toBeLessThan(0.01);
				expect(width).toBeGreaterThan(0);
				expect(width).toBeLessThanOrEqual(maximum);
			}
		} finally { restore(); }
	});

	it("keeps an integer drag's one-unit minimum", async () => {
		const { context, restore } = canvasContext();
		const onmanualwindowlevel = vi.fn();
		try {
			renderViewport({ activeTool: "window_level", onmanualwindowlevel });
			await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
			const viewport = screen.getByRole("application");
			await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
			await fireEvent.pointerMove(viewport, { clientX: -1000, clientY: 10, pointerId: 1 });
			await fireEvent.pointerUp(viewport, { pointerId: 1 });
			expect(onmanualwindowlevel.mock.lastCall?.[1]).toBe(1);
		} finally { restore(); }
	});

	it("shows a carried sub-unit window on integer samples as the one unit it applies", async () => {
		const { context, restore } = canvasContext();
		try {
			renderViewport({ activeTool: "window_level", windowCenter: 40, windowWidth: 0.3 });
			await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
			await screen.findByText("W: 1 · C: 40");
		} finally { restore(); }
	});

	it.each(["next file", "next frame", "mid-drag file"])("does not edit held ROIs while the %s is pending", async (destination) => {
		const { context, restore } = canvasContext();
		const changesFile = destination !== "next frame";
		vi.stubGlobal("createImageBitmap", vi.fn(async () => ({ width: 64, height: 64, close: vi.fn() })));
		let finish!: (frame: api.DisplayFrame) => void;
		fetchDisplayFrame.mockImplementation(async (file, frame) => file === 5 && frame === 0
			? { blob: new Blob(["first"]), window: null, appliedWindow: null }
			: new Promise((resolve) => { finish = resolve; }));
		fetchFrameValueMapping.mockImplementation(async (file, frame) => ({ ...identityMapping(file), frame_index: frame }));
		vi.mocked(api.fetchAnnotations).mockImplementation(async (file) => ({ num_roi: 1,
			roi_coords: [file === 5 ? [2, 2, 20, 20] : [30, 30, 50, 50]], roi_frames: [] }));
		try {
			const view = renderViewport({ activeTool: "annotate_rect", file: fileSummary(5, { frame_count: 2 }) });
			await waitFor(() => expect(context.drawImage).toHaveBeenCalledOnce());
			await fireEvent.click(await screen.findByRole("button", { name: "#1" }));
			if (destination === "mid-drag file") {
				await fireEvent.pointerDown(screen.getByRole("application"), { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
			}
			await view.rerender(changesFile
				? { activeFile: fileSummary(6), currentFrame: 0, navigationPosition: 1 }
				: { currentFrame: 1, navigationPosition: 1 });
			await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalledWith(changesFile ? 6 : 5,
				changesFile ? 0 : 1, expect.anything(), expect.any(AbortSignal)));
			const viewport = screen.getByRole("application");
			if (destination !== "mid-drag file") await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
			await fireEvent.pointerMove(viewport, { clientX: 15, clientY: 15, pointerId: 1 });
			await fireEvent.pointerUp(viewport, { clientX: 15, clientY: 15, pointerId: 1 });
			await fireEvent.click(screen.getByRole("button", { name: "#1" }));
			await act(() => view.component.deleteSelectedRoi());
			for (const name of ["Current", "All", "Delete"]) {
				const button = screen.queryByRole("button", { name });
				if (button) await fireEvent.click(button);
			}
			expect(api.updateAnnotations).not.toHaveBeenCalled();
			expect(context.drawImage).toHaveBeenCalledOnce();
			await act(() => finish({ blob: new Blob(["next"]), window: null, appliedWindow: null }));
			await waitFor(() => expect(context.drawImage).toHaveBeenCalledTimes(2));
			await fireEvent.click(screen.getByRole("button", { name: "#1" }));
			await act(() => view.component.deleteSelectedRoi());
			await waitFor(() => expect(api.updateAnnotations).toHaveBeenCalledWith(changesFile ? 6 : 5,
				{ num_roi: 0, roi_coords: [], roi_frames: [] }));
		} finally { restore(); vi.unstubAllGlobals(); }
	});

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
	it.each([
		["automatic", null, null, null, (frame: number) => `W: ${100 * (frame + 1)} · C: ${50 * (frame + 1)} um2/s`],
		["explicit", 30, 60, "um2/s", () => "W: 60 · C: 30 um2/s"],
	])("keeps each frame's real-world unit on the HUD through cine (%s window)", async (_name, windowCenter, windowWidth, windowUnit, expected) => {
		// Frame f maps stored values with slope (f + 1) / 2 um2/s; the server
		// applies stored C 100 / W 200 unless a unit window is converted.
		fetchFrameValueMapping.mockImplementation(async (fileIndex, frameIndex) => {
			const base = adcMapping();
			return { ...base, file_index: fileIndex, frame_index: frameIndex,
				real_world: [{ ...base.real_world[0], transform: { kind: "linear", slope: (frameIndex + 1) / 2, intercept: 0 } }] };
		});
		fetchDisplayFrame.mockImplementation(async (_file, _frame, options) => ({ blob: new Blob(["png"]),
			window: options?.wc != null && options.ww != null ? { wc: options.wc, ww: options.ww } : { wc: 100, ww: 200 }, appliedWindow: "linear" }));
		const context = { clearRect: vi.fn(), putImageData: vi.fn(), drawImage: vi.fn() };
		const canvas = vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
		vi.stubGlobal("createImageBitmap", vi.fn(async () => ({ width: 64, height: 64, close: vi.fn() })));
		const shown: Array<[number, string]> = [];
		let frame = 0;
		const hud = () => [...document.querySelectorAll(".hud .overlay span")].map((span) => span.textContent?.trim()).find((text) => text?.startsWith("W:")) ?? "";
		const view = renderViewport({ file: fileSummary(5, { frame_count: 4 }), windowCenter, windowWidth, windowUnit,
			onnavigationchange: (position) => {
				// Cine steps only once the previous frame is presented.
				shown.push([frame, hud()]);
				frame = position;
				void view.rerender({ currentFrame: position, navigationPosition: position });
			} });
		await screen.findByText(expected(0));
		await view.rerender({ cineFps: 60, cinePlaying: true });
		await waitFor(() => expect(shown.length).toBeGreaterThanOrEqual(9), { timeout: 3000 });
		await view.rerender({ cinePlaying: false });
		expect(shown).toEqual(shown.map(([index]) => [index, expected(index)]));
		expect(new Set(shown.map(([index]) => index))).toEqual(new Set([0, 1, 2, 3]));
		canvas.mockRestore();
	});

	it("pauses with pending cine metadata and resumes a drawable raw frame", async () => {
		const context = { createImageData: (w: number, h: number) => ({ data: new Uint8ClampedArray(w * h * 4) }),
			putImageData: vi.fn(), drawImage: vi.fn(), clearRect: vi.fn() };
		const canvas = vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(context as unknown as CanvasRenderingContext2D);
		vi.stubGlobal("createImageBitmap", vi.fn(async () => ({ width: 64, height: 64, close: vi.fn() })));
		const pending = new Map<number, AbortSignal>();
		const mapping = (file: number, frame: number) => ({ ...identityMapping(file), frame_index: frame,
			real_world: [{ ...adcMapping().real_world[0], unit_label: "HU", transform: { kind: "linear" as const, slope: 1, intercept: 0 } }] });
		fetchFrameValueMapping.mockImplementation(async (file, frame, signal) => {
			if (frame < 12) return mapping(file, frame);
			if (frame === 35) { await new Promise((resolve) => setTimeout(resolve, 30)); return mapping(file, frame); }
			pending.set(frame, signal!);
			return new Promise((_resolve, reject) => signal!.addEventListener("abort", () => reject(new DOMException("cancelled", "AbortError")), { once: true }));
		});
		fetchDisplayFrame.mockResolvedValue({ blob: new Blob(["png"]), window: { wc: 40, ww: 80 }, appliedWindow: "linear" });
		const onmanualwindowlevel = vi.fn();
		const { rerender } = renderViewport({ activeTool: "window_level", file: fileSummary(5, { frame_count: 300 }),
			windowCenter: 40, windowWidth: 80, windowUnit: "HU", onmanualwindowlevel });
		await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
		await rerender({ cinePlaying: true });
		await waitFor(() => expect(pending.size).toBeGreaterThan(0));
		await rerender({ currentFrame: 35, navigationPosition: 35 });
		context.putImageData.mockClear();
		await rerender({ cinePlaying: false });
		await waitFor(() => expect(context.putImageData).toHaveBeenCalled());
		const viewport = screen.getByRole("application");
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 10, clientY: 10, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 20, clientY: 10, pointerId: 1 });
		expect(onmanualwindowlevel).toHaveBeenCalledWith(40, 120, "HU");
		canvas.mockRestore();
	});

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

describe("ImageViewport graphic annotations", () => {
	const fetchGraphicAnnotations = vi.mocked(api.fetchGraphicAnnotations);
	const annotations = (text: string): api.GraphicAnnotationsResponse => ({
		layers: [{ name: "SHAPES", order: 1, description: null, color: [255, 212, 0] }],
		graphics: [
			{ item: 0, layer: "SHAPES", graphic_type: "circle", points: [[20, 30], [30, 30]], filled: false },
			{ item: 1, layer: "MARKS", graphic_type: "polyline", points: [[0, 0], [64, 0], [64, 64], [0, 0]], filled: true },
			{ item: 1, layer: "MARKS", graphic_type: "point", points: [[10.5, 12.5]], filled: false },
		],
		texts: [{ item: 0, layer: "SHAPES", text, bounding_box: [10, 40, 30, 46], justification: "center", anchor: null, anchor_visible: false }],
		skipped: { display_units: 0, matrix_units: 0, malformed: 0, masked_text: 0 },
	});

	beforeEach(() => {
		fetchGraphicAnnotations.mockReset();
		fetchGraphicAnnotations.mockImplementation(async (_file, frame) => annotations(`frame ${frame}`));
	});

	it("draws nothing and asks for nothing without a shown state", async () => {
		const { container } = renderViewport();
		await waitFor(() => expect(fetchDisplayFrame).toHaveBeenCalled());

		expect(fetchGraphicAnnotations).not.toHaveBeenCalled();
		expect(container.querySelector(".graphic-annotations")).toBeNull();
	});

	it("draws the shown state's objects in image pixels over the displayed frame", async () => {
		const { container } = renderViewport({ graphicAnnotation: { stateFileIndex: 9, highlightedItem: null } });
		await waitFor(() => expect(container.querySelector(".graphic-annotations ellipse")).not.toBeNull());

		expect(fetchGraphicAnnotations).toHaveBeenCalledWith(5, 0, 9, expect.anything());
		const layer = container.querySelector(".graphic-annotations");
		expect(layer?.getAttribute("viewBox")).toBe("0 0 64 64");
		const circle = layer?.querySelector("ellipse");
		expect([circle?.getAttribute("cx"), circle?.getAttribute("cy"), circle?.getAttribute("rx")]).toEqual(["20", "30", "10"]);
		expect(circle?.getAttribute("style")).toContain("rgb(255, 212, 0)");
		const border = layer?.querySelector("path");
		expect(border?.getAttribute("d")).toBe("M0 0L64 0L64 64Z");
		expect(border?.classList.contains("filled")).toBe(true);
		// Points and text are drawn at screen size outside the scaled layer.
		expect(container.querySelector(".graphic-annotation-labels .point")).not.toBeNull();
		expect(container.querySelector(".graphic-annotation-labels text")?.textContent).toBe("frame 0");
	});

	it("highlights the stepped item and dims the others", async () => {
		const { container, rerender } = renderViewport({ graphicAnnotation: { stateFileIndex: 9, highlightedItem: null } });
		await waitFor(() => expect(container.querySelector(".graphic-annotations ellipse")).not.toBeNull());
		expect(container.querySelectorAll(".highlighted, .dimmed")).toHaveLength(0);

		await rerender({ graphicAnnotation: { stateFileIndex: 9, highlightedItem: 0 } });
		expect(container.querySelector(".graphic-annotations ellipse")?.classList.contains("highlighted")).toBe(true);
		expect(container.querySelector(".graphic-annotations path")?.classList.contains("dimmed")).toBe(true);
		expect(container.querySelector(".graphic-annotation-labels .point")?.classList.contains("dimmed")).toBe(true);
		expect(container.querySelector(".graphic-annotation-labels g")?.classList.contains("highlighted")).toBe(true);
		// Stepping re-reads nothing.
		expect(fetchGraphicAnnotations).toHaveBeenCalledTimes(1);
	});

	it("reads each displayed frame's own annotations and removes them when turned off", async () => {
		const file = fileSummary(5, { frame_count: 3 });
		const { container, rerender } = renderViewport({ file, graphicAnnotation: { stateFileIndex: 9, highlightedItem: null } });
		await waitFor(() => expect(container.querySelector(".graphic-annotation-labels text")?.textContent).toBe("frame 0"));

		await rerender({ currentFrame: 2, navigationPosition: 2 });
		await waitFor(() => expect(container.querySelector(".graphic-annotation-labels text")?.textContent).toBe("frame 2"));
		expect(fetchGraphicAnnotations).toHaveBeenLastCalledWith(5, 2, 9, expect.anything());

		await rerender({ graphicAnnotation: null });
		expect(container.querySelector(".graphic-annotations")).toBeNull();
		expect(container.querySelector(".graphic-annotation-labels")).toBeNull();
	});
});

// The gestures every tool shares, which the tool host handles before any tool.
describe("ImageViewport shared gestures", () => {
	const noRois: api.EmbedRoiAnnotations = { num_roi: 0, roi_coords: [], roi_frames: [] };
	const oneRoi: api.EmbedRoiAnnotations = { num_roi: 1, roi_coords: [[10, 10, 30, 30]], roi_frames: [] };
	const otherFileRoi: api.EmbedRoiAnnotations = { num_roi: 1, roi_coords: [[40, 40, 60, 60]], roi_frames: [] };

	let restoreCanvas = () => {};
	beforeEach(() => {
		restoreCanvas = canvasContext().restore;
		vi.stubGlobal("createImageBitmap", vi.fn(async () => ({ width: 64, height: 64, close: vi.fn() })));
	});
	afterEach(() => {
		restoreCanvas();
		vi.unstubAllGlobals();
	});

	type Wheel = { deltaX?: number; deltaY?: number; deltaMode?: number; ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean; at?: number };
	/**
	 * A wheel event at client (20, 30), `at` milliseconds into the test when given.
	 * happy-dom's WheelEvent lacks the MouseEvent fields browsers give it.
	 */
	function wheelAt(viewport: HTMLElement, { ctrlKey = false, metaKey = false, altKey = false, at, ...deltas }: Wheel) {
		const event = createEvent.wheel(viewport, deltas);
		Object.defineProperties(event, {
			clientX: { value: 20 }, clientY: { value: 30 }, ctrlKey: { value: ctrlKey }, metaKey: { value: metaKey },
			altKey: { value: altKey },
			...(at === undefined ? {} : { timeStamp: { value: 10_000 + at } }),
		});
		return fireEvent(viewport, event);
	}

	/** The zoom and pan the image layer is drawn with. */
	function shownTransform() {
		const style = document.querySelector(".image-layer")?.getAttribute("style") ?? "";
		const match = /translate\((\S+)px, (\S+)px\) scale\((\S+?)\)/.exec(style);
		if (!match) throw new Error(`no view transform in "${style}"`);
		return { tx: Number(match[1]), ty: Number(match[2]), scale: Number(match[3]) };
	}

	function expectTransform(expected: { tx: number; ty: number; scale: number }) {
		const shown = shownTransform();
		expect(shown.scale).toBeCloseTo(expected.scale, 3);
		expect(shown.tx).toBeCloseTo(expected.tx, 1);
		expect(shown.ty).toBeCloseTo(expected.ty, 1);
	}

	/** Resolves once the frame is on screen, which is when a tool can act on it. */
	async function shown(fileIndex: number, frameIndex: number) {
		await waitFor(() => expect(document.querySelector(".dicom-canvas")?.getAttribute("data-capture-rendered"))
			.toBe(`${fileIndex}:${frameIndex}`));
	}

	const draft = () => document.querySelector(".roi-rect.draft");
	const coords = () => [...document.querySelectorAll(".roi-coords")].map((item) => item.textContent);
	const windowHud = () => [...document.querySelectorAll(".hud .overlay span")]
		.map((span) => span.textContent?.trim()).find((text) => text?.startsWith("W:"));

	/** A three-frame file (5) with `rois` in `activeTool`, ready for that tool's own gesture; file 6 has its own ROI. */
	async function renderReady(activeTool: ActiveTool, rois = noRois) {
		vi.mocked(api.fetchAnnotations).mockImplementation(async (file) => file === 6 ? otherFileRoi : rois);
		const onnavigationchange = vi.fn();
		const onmanualwindowlevel = vi.fn();
		const view = renderViewport({ activeTool, file: fileSummary(5, { frame_count: 3 }), onnavigationchange, onmanualwindowlevel });
		const viewport = await screen.findByRole("application");
		await shown(5, 0);
		await screen.findByText(activeTool === "redact" ? "Redactions 0 / 0" : `ROIs ${rois.num_roi} / ${rois.num_roi}`);
		if (activeTool === "window_level") await waitFor(() => expect(windowHud()).toBe("W: 1 · C: 0.5"));
		return { ...view, viewport, onnavigationchange, onmanualwindowlevel };
	}

	it.each([
		...TOOL_ORDER.map((tool) => [tool, "middle", 1, true] as const),
		...TOOL_ORDER.map((tool) => [tool, "right", 2, false] as const),
		["pan", "left", 0, true] as const,
	])("%s tool: a %s-button drag pans or does nothing, and never starts another tool's gesture", async (tool, _name, button, pans) => {
		const { viewport, onnavigationchange, onmanualwindowlevel } = await renderReady(tool);
		const moved = pans ? { tx: 15, ty: -6, scale: 1 } : { tx: 0, ty: 0, scale: 1 };

		await fireEvent.pointerDown(viewport, { button, clientX: 10, clientY: 10, pointerId: 1 });
		// The pointer belongs to the viewport for as long as the drag lasts.
		expect(viewport.hasPointerCapture(1)).toBe(pans);
		await fireEvent.pointerMove(viewport, { clientX: 25, clientY: 4, pointerId: 1 });
		expectTransform(moved);
		expect(draft()).toBeNull();
		await fireEvent.pointerUp(viewport, { button, clientX: 25, clientY: 4, pointerId: 1 });
		expect(viewport.hasPointerCapture(1)).toBe(false);
		await fireEvent.pointerMove(viewport, { clientX: 60, clientY: 60, pointerId: 1 });

		expectTransform(moved);
		expect(onnavigationchange).not.toHaveBeenCalled();
		expect(onmanualwindowlevel).not.toHaveBeenCalled();
		expect(api.updateAnnotations).not.toHaveBeenCalled();
		expect(api.updateRedactions).not.toHaveBeenCalled();
		if (tool === "window_level") expect(windowHud()).toBe("W: 1 · C: 0.5");
	});

	// Zooming keeps the image point under the pointer (20, 30) in place.
	const zoomedAbout = (scale: number) => ({ scale, tx: 20 - 20 * scale, ty: 30 - 30 * scale });
	it.each([
		["a mouse wheel notch zooms about the pointer", "pan", { deltaY: -100 }, zoomedAbout(Math.exp(0.25)), null],
		["a line-mode wheel zooms however small its step", "pan", { deltaY: 1, deltaMode: 1 }, zoomedAbout(Math.exp(-0.04)), null],
		["a small pixel step pans", "pan", { deltaY: 20 }, { scale: 1, tx: 0, ty: -20 }, null],
		["a step with a horizontal part pans both ways", "pan", { deltaX: 30, deltaY: 80 }, { scale: 1, tx: -30, ty: -80 }, null],
		["Ctrl plus wheel zooms as a pinch", "pan", { deltaY: -10, ctrlKey: true }, zoomedAbout(Math.exp(0.1)), null],
		["Meta plus wheel zooms as a pinch", "pan", { deltaY: -10, metaKey: true }, zoomedAbout(Math.exp(0.1)), null],
		["the Scroll tool steps to the next frame", "scroll", { deltaY: 100 }, { scale: 1, tx: 0, ty: 0 }, 1],
	] as const)("wheel: %s", async (_name, tool, wheel, expected, steppedTo) => {
		const { viewport, onnavigationchange } = await renderReady(tool);

		await wheelAt(viewport, wheel);

		expectTransform(expected);
		if (steppedTo === null) expect(onnavigationchange).not.toHaveBeenCalled();
		else expect(onnavigationchange.mock.calls).toEqual([[steppedTo]]);
	});

	it.each([
		["Ctrl", { deltaY: -10, ctrlKey: true }],
		["Meta", { deltaY: -10, metaKey: true }],
	] as const)("wheel: a pinch (%s plus wheel) zooms in the Scroll tool and steps no frame", async (_name, wheel) => {
		const { viewport, onnavigationchange } = await renderReady("scroll");

		await wheelAt(viewport, wheel);

		expectTransform(zoomedAbout(Math.exp(0.1)));
		expect(onnavigationchange).not.toHaveBeenCalled();
	});

	// Wheel events under 150 ms apart are one gesture, which keeps the device it began as;
	// the session then follows what its gestures showed (annotation-tools-ux.md 3.5).
	it.each([
		["after a wheel notch, a small pixel step zooms too",
			[{ deltaY: -100, at: 0 }, { deltaY: -20, at: 400 }], zoomedAbout(Math.exp(0.3))],
		["a swipe keeps panning through a notch-sized step",
			[{ deltaY: 20, at: 0 }, { deltaY: 30, at: 16 }, { deltaY: 100, at: 32 }], { scale: 1, tx: 0, ty: -150 }],
		["a wheel that only moves sideways pans",
			[{ deltaY: -100, at: 0 }, { deltaX: 100, at: 400 }], { ...zoomedAbout(Math.exp(0.25)), tx: zoomedAbout(Math.exp(0.25)).tx - 100 }],
	] as const)("wheel sequence: %s", async (_name, wheels, expected) => {
		const { viewport, onnavigationchange } = await renderReady("pan");

		for (const wheel of wheels) await wheelAt(viewport, wheel);

		expectTransform(expected);
		expect(onnavigationchange).not.toHaveBeenCalled();
	});

	it.each([
		["draw", "frame"], ["draw", "file"], ["move", "frame"], ["move", "file"],
	] as const)("drops a rectangle %s when the %s changes under it", async (gesture, change) => {
		const { viewport, rerender } = await renderReady("annotate_rect", gesture === "move" ? oneRoi : noRois);
		const before = gesture === "move" ? ["[10, 10, 30, 30]"] : [];
		const after = change === "file" ? ["[40, 40, 60, 60]"] : before;

		// From (20, 20): inside the ROI when there is one, on bare image otherwise.
		await fireEvent.pointerDown(viewport, { button: 0, clientX: 20, clientY: 20, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 25, clientY: 25, pointerId: 1 });
		if (gesture === "draw") expect(draft()).not.toBeNull();
		else expect(coords()).toEqual(["[15, 15, 35, 35]"]);

		await rerender(change === "file"
			? { activeFile: fileSummary(6, { frame_count: 3 }) }
			: { currentFrame: 1, navigationPosition: 1 });
		await (change === "file" ? shown(6, 0) : shown(5, 1));
		await waitFor(() => expect(coords()).toHaveLength(after.length));
		await fireEvent.pointerMove(viewport, { clientX: 45, clientY: 45, pointerId: 1 });

		expect(draft()).toBeNull();
		expect(coords()).toEqual(after);
		await fireEvent.pointerUp(viewport, { clientX: 45, clientY: 45, pointerId: 1 });
		expect(draft()).toBeNull();
		expect(coords()).toEqual(after);
		expect(api.updateAnnotations).not.toHaveBeenCalled();

		// The frame the gesture began on shows what it showed before.
		await rerender(change === "file"
			? { activeFile: fileSummary(5, { frame_count: 3 }) }
			: { currentFrame: 0, navigationPosition: 0 });
		await shown(5, 0);
		await waitFor(() => expect(coords()).toEqual(before));
		expect(api.updateAnnotations).not.toHaveBeenCalled();
	});

	it("does not land a rectangle on the new frame when released without another move", async () => {
		const { viewport, rerender } = await renderReady("annotate_rect", noRois);

		await fireEvent.pointerDown(viewport, { button: 0, clientX: 20, clientY: 20, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 35, clientY: 35, pointerId: 1 });
		expect(draft()).not.toBeNull();

		await rerender({ currentFrame: 1, navigationPosition: 1 });
		await shown(5, 1);
		await fireEvent.pointerUp(viewport, { clientX: 35, clientY: 35, pointerId: 1 });

		expect(draft()).toBeNull();
		expect(coords()).toEqual([]);
		expect(api.updateAnnotations).not.toHaveBeenCalled();
	});

	it.each([
		["a rectangle being drawn", "annotate_rect", noRois, false],
		["a ROI being moved", "annotate_rect", oneRoi, false],
		["a ROI being moved, after a second pointer began a middle-button pan", "annotate_rect", oneRoi, true],
		["a window/level drag", "window_level", noRois, false],
		["a pan", "pan", noRois, false],
	] as const)("pointercancel ends %s without committing it", async (_name, tool, rois, secondPointer) => {
		const { viewport, onmanualwindowlevel } = await renderReady(tool, rois);

		await fireEvent.pointerDown(viewport, { button: 0, clientX: 20, clientY: 20, pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 25, clientY: 25, pointerId: 1 });
		if (tool === "annotate_rect" && rois.num_roi === 0) expect(draft()).not.toBeNull();
		if (rois.num_roi === 1) expect(coords()).toEqual(["[15, 15, 35, 35]"]);
		if (tool === "pan") expectTransform({ tx: 5, ty: 5, scale: 1 });
		if (tool === "window_level") await waitFor(() => expect(windowHud()).toMatch(/^W: 21 /));
		if (secondPointer) await fireEvent.pointerDown(viewport, { button: 1, clientX: 40, clientY: 40, pointerId: 2 });

		await fireEvent.pointerCancel(viewport, { pointerId: 1 });
		await fireEvent.pointerMove(viewport, { clientX: 50, clientY: 50, pointerId: 1 });
		await fireEvent.pointerUp(viewport, { clientX: 50, clientY: 50, pointerId: 1 });

		expect(draft()).toBeNull();
		expect(coords()).toEqual(rois.num_roi === 1 ? ["[10, 10, 30, 30]"] : []);
		expect(api.updateAnnotations).not.toHaveBeenCalled();
		expect(onmanualwindowlevel).not.toHaveBeenCalled();
		// A pan is not undone, but it stops following the pointer.
		expectTransform(tool === "pan" ? { tx: 5, ty: 5, scale: 1 } : { tx: 0, ty: 0, scale: 1 });
		if (tool === "window_level") await waitFor(() => expect(windowHud()).toBe("W: 1 · C: 0.5"));
	});
});

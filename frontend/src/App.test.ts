// @vitest-environment happy-dom
import { act, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import App from "./App.svelte";
import type { SemanticContextResponse, SeriesSummary } from "./api";
import { emptySeriesCatalog, fileSummary, filesResponse, rawFrame } from "./testing/fixtures";

vi.mock("./api", async (importOriginal) => ({
	...await importOriginal<typeof import("./api")>(),
	fetchFiles: vi.fn(),
	fetchHealth: vi.fn(),
	onReachabilityChange: vi.fn(() => () => {}),
	fetchSeries: vi.fn(),
	fetchTags: vi.fn(async () => []),
	fetchReferences: vi.fn(async (fileIndex: number) => ({
		source_file_index: fileIndex,
		source_sop_instance_uid: "",
		references: [],
	})),
	fetchAnnotations: vi.fn(async () => ({ num_roi: 0, roi_coords: [], roi_frames: [] })),
	updateAnnotations: vi.fn(),
	fetchDisplayFrame: vi.fn(async (_file: number, _frame: number, options: api.DisplayFrameWindowOptions = {}) => ({
		blob: new Blob(["png"], { type: "image/png" }),
		window: { wc: options.wc ?? 40, ww: options.ww ?? 400 }, appliedWindow: "linear",
	})),
	fetchRawFrame: vi.fn(async () => rawFrame()),
	fetchFrameValueMapping: vi.fn(async (fileIndex: number, frameIndex: number) => ({
		file_index: fileIndex,
		frame_index: frameIndex,
		stored_value_type: "integer",
		modality: { rescale_slope: 1, rescale_intercept: 0, rescale_type: null, lut: null },
		real_world: [],
	})),
	fetchSemanticContext: vi.fn(),
	fetchDoseOverlayBlob: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
}));

// happy-dom cannot decode PNGs or draw on a canvas; the layer is recorded.
vi.mock("./lib/viewport/frameOverlay", async (importOriginal) => ({
	...await importOriginal<typeof import("./lib/viewport/frameOverlay")>(),
	decodeCanvasImage: vi.fn(async () => ({ source: {}, width: 64, height: 64, dispose: vi.fn() })),
	drawOverlayLayer: vi.fn(),
}));

const files = [
	fileSummary(0, { label: "first.dcm", path: "first.dcm" }),
	fileSummary(1, { label: "second.dcm", path: "second.dcm", frame_count: 3 }),
];

beforeEach(() => {
	vi.mocked(api.fetchFiles).mockResolvedValue(filesResponse(files));
	vi.mocked(api.fetchSeries).mockResolvedValue(emptySeriesCatalog());
});

async function renderApp() {
	const view = render(App);
	// The catalog opens the first file on load.
	await screen.findByTitle("Fit to height");
	return view;
}

function zoomLabel(): string {
	return screen.getByTitle("Fit to height").textContent ?? "";
}

function activeTool(): string {
	return document.querySelector('.toolbar button[aria-pressed="true"]')?.textContent?.trim() ?? "";
}

function tabButton(path: string): HTMLElement {
	const button = document.querySelector<HTMLElement>(`.tab[title="${path}"] .tab-main`);
	if (!button) throw new Error(`no open tab for ${path}`);
	return button;
}

async function openFromExplorer(fileIndex: number) {
	const button = document.querySelector<HTMLElement>(`[data-capture-file-index="${fileIndex}"]`);
	if (!button) throw new Error(`file ${fileIndex} is not in the explorer`);
	await fireEvent.click(button);
}

describe("App", () => {
	it("keeps each tab's zoom when switching between tabs", async () => {
		await renderApp();
		expect(zoomLabel()).toBe("100%");
		await fireEvent.click(screen.getByRole("button", { name: "+" }));
		expect(zoomLabel()).toBe("125%");

		await openFromExplorer(1);
		await waitFor(() => expect(document.querySelectorAll(".tab")).toHaveLength(2));
		expect(zoomLabel()).toBe("100%");
		await fireEvent.click(screen.getByRole("button", { name: "−" }));
		expect(zoomLabel()).toBe("75%");

		await fireEvent.click(tabButton("first.dcm"));
		expect(zoomLabel()).toBe("125%");
		await fireEvent.click(tabButton("second.dcm"));
		expect(zoomLabel()).toBe("75%");
	});

	it("forgets a closed tab's zoom and orientation while retaining other open tabs", async () => {
		await renderApp();
		const initial = document.querySelector<HTMLElement>(".image-layer")!.style.transform;
		await fireEvent.click(screen.getByRole("button", { name: "+" }));
		await fireEvent.click(screen.getByRole("button", { name: "Flip horizontal" }));
		await fireEvent.click(screen.getByRole("button", { name: "Rotate 90° clockwise" }));
		expect(document.querySelector<HTMLElement>(".image-layer")!.style.transform).not.toBe(initial);
		await openFromExplorer(1);
		await fireEvent.click(screen.getByRole("button", { name: "−" }));
		await fireEvent.click(document.querySelector<HTMLElement>('.tab[title="first.dcm"] .close')!);
		expect(zoomLabel()).toBe("75%");
		await openFromExplorer(0);
		expect(zoomLabel()).toBe("100%");
		expect(document.querySelector<HTMLElement>(".image-layer")!.style.transform).toBe(initial);
		await fireEvent.click(tabButton("second.dcm"));
		expect(zoomLabel()).toBe("75%");
		await fireEvent.click(document.querySelector<HTMLElement>('.tab[title="second.dcm"] .close')!);
		await openFromExplorer(1);
		expect(zoomLabel()).toBe("100%");
	});

	it("switches tools from the keyboard but not while typing", async () => {
		await renderApp();
		expect(activeTool()).toBe("Pan");

		const filter = screen.getByLabelText("Filter tags");
		filter.focus();
		await fireEvent.keyDown(filter, { key: "w" });
		expect(activeTool()).toBe("Pan");

		const editable = document.createElement("div");
		editable.contentEditable = "true";
		document.body.append(editable);
		await fireEvent.keyDown(editable, { key: "z" });
		expect(activeTool()).toBe("Pan");
		editable.remove();

		await fireEvent.keyDown(document.body, { key: "w" });
		expect(activeTool()).toBe("W/L");
		await fireEvent.keyDown(document.body, { key: "R" });
		expect(activeTool()).toBe("ROI");
	});

	it("steps frames of a multi-frame tab with the arrow keys, not while typing", async () => {
		await renderApp();
		await openFromExplorer(1);
		await screen.findByText("image 1 / 3", { selector: ".slider span" });

		await fireEvent.keyDown(document.body, { key: "ArrowRight" });
		await screen.findByText("image 2 / 3", { selector: ".slider span" });
		await fireEvent.keyDown(screen.getByLabelText("Filter tags"), { key: "]" });
		expect(screen.getByText("image 2 / 3", { selector: ".slider span" })).toBeTruthy();
		await fireEvent.keyDown(document.body, { key: "[" });
		await screen.findByText("image 1 / 3", { selector: ".slider span" });
	});

	it("resets window/level and preset from the toolbar", async () => {
		await renderApp();
		const presets = document.querySelector<HTMLSelectElement>(".toolbar select");
		if (!presets) throw new Error("no preset select");

		presets.value = "brain";
		await fireEvent.change(presets);
		await screen.findByText("W: 80 · C: 40");

		await fireEvent.click(screen.getByRole("button", { name: "Reset view" }));
		await screen.findByText("W: 400 · C: 40");
		expect(presets.value).toBe("default");
	});
});

describe("App value overlays", () => {
	const DOSE = fileSummary(2, {
		label: "dose.dcm",
		path: "dose.dcm",
		modality: "RTDOSE",
		sop_class_uid: "1.2.840.10008.5.1.4.1.1.481.2",
		object_kind: "radiation_therapy",
		frame_count: 3,
	});

	function frameSeries(id: string, fileIndexes: number[]): SeriesSummary {
		return {
			id,
			study_instance_uid: "1.2.3",
			series_instance_uid: id,
			frame_of_reference_uids: ["1.2.3.for"],
			stacks: [{
				id,
				kind: "ordinary",
				concatenation_uid: null,
				pyramid_uid: null,
				image_type_role: null,
				total_pixel_matrix_rows: null,
				total_pixel_matrix_columns: null,
				frames: fileIndexes.map((file_index, virtual_index) => ({
					virtual_index,
					file_index,
					frame_index: 0,
					sop_instance_uid: "",
					instance_number: null,
					position_along_normal_mm: null,
				})),
				warnings: [],
			}],
		};
	}

	function doseContext(): SemanticContextResponse {
		return {
			source_file_index: DOSE.index,
			default_mode: "pixel_preview",
			pixel_preview_preserves_stored_values: true,
			context: {
				kind: "rt_dose",
				dose_grid_scaling: 0.01,
				scaling_status: "available",
				displayed_value_kind: "stored",
				dose_units: "GY",
				dose_type: "PHYSICAL",
				dose_summation_type: "PLAN",
				geometry: {
					frame_of_reference_uid: "1.2.3.for",
					image_position_patient: null,
					image_orientation_patient: null,
					pixel_spacing: null,
					grid_frame_offsets: [],
				},
				references: [],
				overlay: { eligible: true, reason: "covers", source_file_index: 0, mapped_source_count: 1 },
				overlay_source_frames: [{ file_index: 0, frame_index: 0, sop_instance_uid: "ct" }],
				legend: {
					unit_label: "Gy",
					units: null,
					min_value: 0,
					max_value: 23.3,
					transparent_at_or_below: 0,
					colormap: "viridis",
					color_stops: [[68, 1, 84], [253, 231, 37]],
				},
				clinical_use_warning: "Not for clinical use.",
			},
		};
	}

	it("offers a covering RT Dose over the image and draws it when switched on", async () => {
		vi.mocked(api.fetchFiles).mockResolvedValue(filesResponse([...files, DOSE]));
		vi.mocked(api.fetchSeries).mockResolvedValue({
			series: [frameSeries("ct", [0]), frameSeries("dose", [2])],
			scan_complete: true,
		});
		vi.mocked(api.fetchSemanticContext).mockResolvedValue(doseContext());
		await renderApp();

		const toggle = await screen.findByRole("button", { name: "RT Dose · PLAN" });
		expect(api.fetchSemanticContext).toHaveBeenCalledWith(DOSE.index);
		expect(api.fetchDoseOverlayBlob).not.toHaveBeenCalled();

		await fireEvent.click(toggle);
		await waitFor(() => expect(api.fetchDoseOverlayBlob).toHaveBeenCalledWith(0, 0, DOSE.index, expect.any(AbortSignal)));
		expect(toggle.getAttribute("aria-pressed")).toBe("true");
		expect(await screen.findByRole("figure", { name: "RT Dose · PLAN: 0 to 23.3 Gy" })).toBeTruthy();
	});
});


describe("server retry", () => {
	async function disconnect() {
		const calls = vi.mocked(api.onReachabilityChange).mock.calls;
		const listen = calls[calls.length - 1][0];
		await act(() => listen(false));
	}

	it("reloads instead of reusing file indices when health reports another server", async () => {
		const reload = vi.spyOn(window.location, "reload").mockImplementation(() => {});
		try {
			await renderApp();
			vi.mocked(api.fetchHealth).mockResolvedValue({ status: "ok", viewer: { name: "dcmview", version: "test", build_target: "test", build_profile: "test" }, file_count: 1, server_start_ms: 999, masked: false });
			await disconnect();
			await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
			await waitFor(() => expect(reload).toHaveBeenCalledOnce());
		} finally { reload.mockRestore(); }
	});

	it("retries a failed frame on the same server and removes its error placeholder", async () => {
		vi.mocked(api.fetchDisplayFrame).mockRejectedValueOnce(new api.ApiError("frame was unreachable", 0, null));
		await renderApp();
		await screen.findByText("frame was unreachable");
		vi.mocked(api.fetchHealth).mockResolvedValue({ status: "ok", viewer: { name: "dcmview", version: "test", build_target: "test", build_profile: "test" }, file_count: 2, server_start_ms: 0, masked: false });
		await disconnect();
		await fireEvent.click(screen.getByRole("button", { name: "Retry" }));
		await waitFor(() => expect(screen.queryByText("frame was unreachable")).toBeNull());
		await waitFor(() => expect(document.querySelector(".dicom-canvas")?.getAttribute("data-capture-rendered")).toBe("0:0"));
	});
});

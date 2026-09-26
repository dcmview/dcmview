// @vitest-environment happy-dom
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import App from "./App.svelte";
import { emptySeriesCatalog, fileSummary, filesResponse, rawFrame } from "./testing/fixtures";

vi.mock("./api", async (importOriginal) => ({
	...await importOriginal<typeof import("./api")>(),
	fetchFiles: vi.fn(),
	fetchSeries: vi.fn(),
	fetchTags: vi.fn(async () => []),
	fetchReferences: vi.fn(async (fileIndex: number) => ({
		source_file_index: fileIndex,
		source_sop_instance_uid: "",
		references: [],
	})),
	fetchAnnotations: vi.fn(async () => ({ num_roi: 0, roi_coords: [], roi_frames: [] })),
	updateAnnotations: vi.fn(),
	fetchDisplayFrameBlob: vi.fn(async () => new Blob(["png"], { type: "image/png" })),
	fetchRawFrame: vi.fn(async () => rawFrame()),
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
	return document.querySelector(".toolbar button.active")?.textContent?.trim() ?? "";
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

	it("switches tools from the keyboard but not while typing", async () => {
		await renderApp();
		expect(activeTool()).toBe("Pan");

		const filter = screen.getByPlaceholderText("filter tags...");
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
		expect(activeTool()).toBe("WL");
		await fireEvent.keyDown(document.body, { key: "R" });
		expect(activeTool()).toBe("ROI");
	});

	it("steps frames of a multi-frame tab with the arrow keys, not while typing", async () => {
		await renderApp();
		await openFromExplorer(1);
		await screen.findByText("image 1 / 3", { selector: ".slider span" });

		await fireEvent.keyDown(document.body, { key: "ArrowRight" });
		await screen.findByText("image 2 / 3", { selector: ".slider span" });
		await fireEvent.keyDown(screen.getByPlaceholderText("filter tags..."), { key: "]" });
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

		await fireEvent.click(screen.getByRole("button", { name: "Reset" }));
		await screen.findByText("W: 400 · C: 40");
		expect(presets.value).toBe("default");
	});
});

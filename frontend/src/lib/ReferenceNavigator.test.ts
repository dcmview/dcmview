// @vitest-environment happy-dom
import { act, fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import { beforeEach, expect, it, vi } from "vitest";
import * as api from "../api";
import { fileSummary } from "../testing/fixtures";
import { reactiveProps } from "../testing/reactiveProps.svelte";
import ReferenceNavigator from "./ReferenceNavigator.svelte";
import type { ComponentProps } from "svelte";

vi.mock("../api", async (original) => ({ ...await original<typeof import("../api")>(), fetchReferences: vi.fn() }));

beforeEach(() => {
	vi.mocked(api.fetchReferences).mockReset();
});

function referencesTo(source: number, target: api.FileSummary, matched = false): api.ReferenceCatalogResponse {
	return { source_file_index: source, source_sop_instance_uid: String(source), references: [{
		relationship: "source", target: { sop_class_uid: null, sop_instance_uid: target.sop_instance_uid,
			series_instance_uid: null, frame_numbers: [], segment_numbers: [] },
		matches: matched ? [{ file_index: target.index, path: target.path,
			sop_instance_uid: target.sop_instance_uid, frame_indices: [0] }] : [],
	}] };
}

it("refreshes references as discovery moves and completes without changing the active file", async () => {
	const load = vi.mocked(api.fetchReferences);
	const target = fileSummary(6);
	load.mockResolvedValue(referencesTo(5, target));
	const state = reactiveProps<ComponentProps<typeof ReferenceNavigator>>({ fileIndex: 5, files: [fileSummary(5), target],
		scanProgress: "2|2|false", onopenreference: vi.fn() });
	render(ReferenceNavigator, { props: state.props });
	await fireEvent.click(await screen.findByRole("button", { name: "References (1)" }));
	await screen.findByText("unresolved");
	await act(() => state.update({ scanProgress: "2|3|false" }));
	await waitFor(() => expect(load).toHaveBeenCalledTimes(2));
	load.mockResolvedValue(referencesTo(5, target, true));
	await act(() => state.update({ scanProgress: "2|3|true" }));
	await screen.findByRole("button", { name: "Open image-6.dcm · frame 1" });
	expect(load).toHaveBeenCalledTimes(3);
	await act(() => state.update({ files: [fileSummary(5), { ...target }] }));
	expect(load).toHaveBeenCalledTimes(3);
});

it("keeps its toolbar slot, hidden, while the active file links to nothing", async () => {
	vi.mocked(api.fetchReferences).mockResolvedValue({ source_file_index: 5, source_sop_instance_uid: "5", references: [] });
	const { container } = render(ReferenceNavigator, { props: { fileIndex: 5, files: [fileSummary(5)],
		scanProgress: "1|1|true", onopenreference: vi.fn() } });
	await waitFor(() => expect(api.fetchReferences).toHaveBeenCalled());
	const slot = container.querySelector(".reference-navigator");
	expect(slot?.classList.contains("hidden")).toBe(true);
	expect(slot?.getAttribute("aria-hidden")).toBe("true");
	expect(screen.getByRole("button", { hidden: true, name: "References" }).hasAttribute("disabled")).toBe(true);
});

it("stays shown while the next linked file loads, then follows that file", async () => {
	const load = vi.mocked(api.fetchReferences);
	const target = fileSummary(9);
	let resolveNext: (value: api.ReferenceCatalogResponse) => void = () => {};
	load.mockResolvedValueOnce(referencesTo(5, target))
		.mockImplementationOnce(() => new Promise((resolve) => { resolveNext = resolve; }));
	const state = reactiveProps<ComponentProps<typeof ReferenceNavigator>>({ fileIndex: 5,
		files: [fileSummary(5), fileSummary(6), target], scanProgress: "3|3|true", onopenreference: vi.fn() });
	const { container } = render(ReferenceNavigator, { props: state.props });
	await screen.findByRole("button", { name: "References (1)" });
	await act(() => state.update({ fileIndex: 6 }));
	await waitFor(() => expect(load).toHaveBeenCalledTimes(2));
	expect(container.querySelector(".reference-navigator.hidden")).toBeNull();
	expect(screen.getByRole("button", { name: "References (1)" })).toBeTruthy();
	await act(() => resolveNext({ source_file_index: 6, source_sop_instance_uid: "6", references: [] }));
	await waitFor(() => expect(container.querySelector(".reference-navigator.hidden")).not.toBeNull());
});

it("closes on Escape and after opening a reference", async () => {
	const target = fileSummary(6);
	vi.mocked(api.fetchReferences).mockResolvedValue(referencesTo(5, target, true));
	const onopenreference = vi.fn();
	render(ReferenceNavigator, { props: { fileIndex: 5, files: [fileSummary(5), target],
		scanProgress: "2|2|true", onopenreference } });
	const toggle = await screen.findByRole("button", { name: "References (1)" });
	await fireEvent.click(toggle);
	expect(screen.getByRole("dialog", { name: "DICOM references" })).toBeTruthy();
	await fireEvent.keyDown(document, { key: "Escape" });
	expect(screen.queryByRole("dialog")).toBeNull();
	expect(document.activeElement).toBe(toggle);
	await fireEvent.click(toggle);
	await fireEvent.click(screen.getByRole("button", { name: "Open image-6.dcm · frame 1" }));
	expect(onopenreference).toHaveBeenCalledWith(6, 0);
	expect(screen.queryByRole("dialog")).toBeNull();
});

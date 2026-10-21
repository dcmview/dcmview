// @vitest-environment happy-dom
import { fireEvent, render, screen, within } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";
import type { FileSummary } from "../api";
import { fileSummary, rasterSummary } from "../testing/fixtures";
import FileNavigator from "./FileNavigator.svelte";

/** Two DICOM files (one without any identity) beside three rasters in three folders. */
function mixedCatalog(): FileSummary[] {
	return [
		fileSummary(0, { path: "/data/scans/ct.dcm", display_name: "ct.dcm" }),
		fileSummary(1, {
			path: "/data/scans/anonymous.dcm",
			display_name: "anonymous.dcm",
			patient_id: "",
			patient_name: "",
			study_instance_uid: "",
			series_instance_uid: "",
			sop_instance_uid: "",
		}),
		rasterSummary(2, { path: "/data/scans/a.png" }),
		rasterSummary(3, { path: "/data/scans/sub/b.tif", file_format: "tiff" }),
		rasterSummary(4, { path: "/data/other/c.jpg", file_format: "jpeg" }),
	];
}

/** A masked session's catalog: synthetic display names, real paths. */
function maskedCatalog(): FileSummary[] {
	return mixedCatalog().map((file) => ({ ...file, display_name: `File ${file.index + 1}` }));
}

function renderNavigator(files: FileSummary[], masked = false, onnavigationorderchange = vi.fn()) {
	return render(FileNavigator, {
		props: { files, masked, activeFileIndex: null, collapsed: false, onopenfile: vi.fn(), onnavigationorderchange },
	});
}

function texts(root: Element, selector: string): string[] {
	return [...root.querySelectorAll(selector)].map((element) => element.textContent?.trim() ?? "");
}

/** The tooltips of the rows that open a file, in order: the path, or the display name where paths are hidden. */
function listedFiles(root: HTMLElement): string[] {
	return within(root).queryAllByRole("button")
		.filter((button) => button.closest('[role="tree"]') && !button.hasAttribute("aria-expanded"))
		.map((button) => button.title);
}

function imageGroup(): HTMLElement {
	return screen.getByRole("button", { name: /^Images, / }).closest("section")!;
}

// Not "data": the markup has data- attributes.
const REAL_NAMES = /scans|\bother\b|a\.png|b\.tif|c\.jpg|\.dcm/;

describe("FileNavigator image files", () => {
	it.each([
		{
			name: "groups rasters by folder under Images and keeps every DICOM file in the clinical tree",
			masked: false,
			folders: ["data", "other", "scans", "sub"],
			images: ["/data/other/c.jpg", "/data/scans/sub/b.tif", "/data/scans/a.png"],
			clinical: ["/data/scans/ct.dcm", "/data/scans/anonymous.dcm"],
		},
		{
			name: "lists a masked session's rasters by display name, without folders",
			masked: true,
			folders: [],
			images: ["File 3", "File 4", "File 5"],
			clinical: ["File 1", "File 2"],
		},
	])("$name", ({ masked, folders, images, clinical }) => {
		const { container } = renderNavigator(masked ? maskedCatalog() : mixedCatalog(), masked);
		const group = imageGroup();

		expect(within(group).getByRole("button", { name: "Images, 3 images, expanded" })).toBeTruthy();
		expect(texts(group, ".folder-row .directory-label")).toEqual(folders);
		expect(listedFiles(group)).toEqual(images);
		// Rasters have no patient: the clinical tree holds exactly the DICOM
		// files, the one with empty UIDs included, ahead of the Images group.
		expect(listedFiles(container)).toEqual([...clinical, ...images]);
		expect(screen.getByRole("tree", { name: "File hierarchy" })).toBeTruthy();
		// Two patients (one unnamed), then the Images group.
		expect(container.querySelectorAll(".study-tree > .tree-group")).toHaveLength(3);
		if (masked) expect(container.innerHTML).not.toMatch(REAL_NAMES);
	});

	it("continues the Up/Down file order from the last DICOM file into Images", () => {
		const order = vi.fn();
		renderNavigator(mixedCatalog(), false, order);
		// The clinical tree's files, then the Images group in its folder order.
		expect(order).toHaveBeenLastCalledWith([0, 1, 4, 3, 2]);
	});

	it.each([
		{ query: "format:png", masked: false, shown: ["/data/scans/a.png"] },
		{ query: "format:jpg", masked: false, shown: ["/data/other/c.jpg"] },
		{ query: "format:dicom", masked: false, shown: ["/data/scans/ct.dcm", "/data/scans/anonymous.dcm"] },
		// Unscoped, a format name matches rasters only: "dicom" is not a way to match every DICOM file.
		{ query: "tiff", masked: false, shown: ["/data/scans/sub/b.tif"] },
		{ query: "com", masked: false, shown: [] },
		{ query: "scans/sub", masked: false, shown: ["/data/scans/sub/b.tif"] },
		// The Study view shows a raster's folders in Images and a DICOM file's name only.
		{ query: "scans", masked: false, shown: ["/data/scans/sub/b.tif", "/data/scans/a.png"] },
		{ query: ".dcm", masked: false, shown: ["/data/scans/ct.dcm", "/data/scans/anonymous.dcm"] },
		// Every raster is under /data: a folder they all share matches nothing.
		{ query: "data", masked: false, shown: [] },
		{ query: "format:png", masked: true, shown: ["File 3"] },
		// A masked Study view hides paths, so they match nothing either.
		{ query: "sub", masked: true, shown: [] },
	])("filters by format and path: $query (masked: $masked)", async ({ query, masked, shown }) => {
		const { container } = renderNavigator(masked ? maskedCatalog() : mixedCatalog(), masked);

		await fireEvent.input(screen.getByLabelText("Filter file hierarchy"), { target: { value: query } });

		expect(listedFiles(container)).toEqual(shown);
		expect(screen.getByText(`showing ${shown.length} of 5 images`)).toBeTruthy();
		// Filtering a masked session reveals no real name, tooltips included.
		if (masked) expect(container.innerHTML).not.toMatch(REAL_NAMES);
	});
});

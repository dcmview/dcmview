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

function renderNavigator(files: FileSummary[], masked = false) {
	return render(FileNavigator, {
		props: { files, masked, activeFileIndex: null, collapsed: false, onopenfile: vi.fn() },
	});
}

function texts(root: Element, selector: string): string[] {
	return [...root.querySelectorAll(selector)].map((element) => element.textContent?.trim() ?? "");
}

/** The names of the listed files, clinical tree first. */
function shownFileNames(root: Element): string[] {
	return texts(root, ".file-row .node-label, .directory-file .directory-label");
}

function imageGroup(): HTMLElement {
	return screen.getByRole("button", { name: /^Images, / }).closest("section")!;
}

describe("FileNavigator image files", () => {
	it.each([
		{
			name: "groups rasters by folder under Images and keeps every DICOM file in the clinical tree",
			masked: false,
			folders: ["data", "other", "scans", "sub"],
			images: ["c.jpg", "b.tif", "a.png"],
			clinical: ["#1 anonymous.dcm", "#1 ct.dcm"],
		},
		{
			name: "lists a masked session's rasters by display name, without folders",
			masked: true,
			folders: [],
			images: ["File 3", "File 4", "File 5"],
			clinical: ["#1 File 2", "#1 File 1"],
		},
	])("$name", ({ masked, folders, images, clinical }) => {
		const { container } = renderNavigator(masked ? maskedCatalog() : mixedCatalog(), masked);
		const group = imageGroup();

		expect(within(group).getByRole("button", { name: "Images, 3 images, expanded" })).toBeTruthy();
		expect(texts(group, ".folder-row .directory-label")).toEqual(folders);
		expect(shownFileNames(group)).toEqual(images);
		// Rasters have no patient: the clinical tree holds exactly the DICOM
		// files, the one with empty UIDs included.
		expect(shownFileNames(container).filter((name) => !images.includes(name)).sort()).toEqual([...clinical].sort());
		// Two patients (one unnamed), then the Images group.
		expect(container.querySelectorAll(".study-tree > .tree-group")).toHaveLength(3);
		if (masked) {
			expect(container.innerHTML).not.toMatch(/scans|other|a\.png|b\.tif|c\.jpg|\.dcm/);
		}
	});

	it.each([
		{ query: "format:png", masked: false, shown: ["a.png"] },
		{ query: "format:jpg", masked: false, shown: ["c.jpg"] },
		{ query: "format:dicom", masked: false, shown: ["#1 anonymous.dcm", "#1 ct.dcm"] },
		{ query: "scans/sub", masked: false, shown: ["b.tif"] },
		{ query: "tiff", masked: false, shown: ["b.tif"] },
		// Every file is under /data: a shared parent folder matches nothing.
		{ query: "data/", masked: false, shown: ["c.jpg", "a.png", "b.tif", "#1 anonymous.dcm", "#1 ct.dcm"] },
		{ query: "format:png", masked: true, shown: ["File 3"] },
		// A masked study view hides paths, so they match nothing either.
		{ query: "sub", masked: true, shown: [] },
	])("filters by format and path: $query (masked: $masked)", async ({ query, masked, shown }) => {
		const { container } = renderNavigator(masked ? maskedCatalog() : mixedCatalog(), masked);

		await fireEvent.input(screen.getByLabelText("Filter file hierarchy"), { target: { value: query } });

		expect(shownFileNames(container).sort()).toEqual([...shown].sort());
		expect(screen.getByText(`showing ${shown.length} of 5 images`)).toBeTruthy();
	});
});

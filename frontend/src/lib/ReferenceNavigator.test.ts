// @vitest-environment happy-dom
import { act, render, screen, waitFor } from "@testing-library/svelte";
import { expect, it, vi } from "vitest";
import * as api from "../api";
import { fileSummary } from "../testing/fixtures";
import { reactiveProps } from "../testing/reactiveProps.svelte";
import ReferenceNavigator from "./ReferenceNavigator.svelte";
import type { ComponentProps } from "svelte";

vi.mock("../api", async (original) => ({ ...await original<typeof import("../api")>(), fetchReferences: vi.fn() }));

it("refreshes references as discovery moves and completes without changing the active file", async () => {
	const load = vi.mocked(api.fetchReferences);
	const target = fileSummary(6);
	const response: api.ReferenceCatalogResponse = { source_file_index: 5, source_sop_instance_uid: "5", references: [{
		relationship: "source", target: { sop_class_uid: null, sop_instance_uid: target.sop_instance_uid,
			series_instance_uid: null, frame_numbers: [], segment_numbers: [] }, matches: [],
	}] };
	load.mockResolvedValue(response);
	const state = reactiveProps<ComponentProps<typeof ReferenceNavigator>>({ fileIndex: 5, files: [fileSummary(5), target],
		scanProgress: "2|2|false", onopenreference: vi.fn() });
	render(ReferenceNavigator, { props: state.props });
	await screen.findByText("unresolved");
	await act(() => state.update({ scanProgress: "2|3|false" }));
	await waitFor(() => expect(load).toHaveBeenCalledTimes(2));
	load.mockResolvedValue({ ...response, references: [{ ...response.references[0], matches: [{ file_index: 6,
		path: target.path, sop_instance_uid: target.sop_instance_uid, frame_indices: [0] }] }] });
	await act(() => state.update({ scanProgress: "2|3|true" }));
	await screen.findByRole("button", { name: "Open image-6.dcm · frame 1" });
	expect(load).toHaveBeenCalledTimes(3);
	await act(() => state.update({ files: [fileSummary(5), { ...target }] }));
	expect(load).toHaveBeenCalledTimes(3);
});

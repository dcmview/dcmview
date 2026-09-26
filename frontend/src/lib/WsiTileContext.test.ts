// @vitest-environment happy-dom
import { fireEvent, render, screen, within } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import type { WsiFrameContextResponse } from "../generated/api-types";
import { fileSummary } from "../testing/fixtures";
import WsiTileContext from "./WsiTileContext.svelte";

vi.mock("../api", async (importOriginal) => ({
	...await importOriginal<typeof import("../api")>(),
	fetchWsiFrameContext: vi.fn(),
}));

const fetchWsiFrameContext = vi.mocked(api.fetchWsiFrameContext);

const wsi = (index: number, label: string) => fileSummary(index, {
	label,
	modality: "SM",
	sop_class_uid: "1.2.840.10008.5.1.4.1.1.77.1.6",
	object_kind: "whole_slide_microscopy",
	frame_count: 4,
});
const LEVEL_0 = wsi(0, "level-0.dcm");
const LEVEL_1 = wsi(1, "level-1.dcm");
const LABEL = wsi(2, "label.dcm");
const OVERVIEW = wsi(3, "overview.dcm");

function wsiContext(overrides: Partial<WsiFrameContextResponse> = {}): WsiFrameContextResponse {
	return {
		source_file_index: LEVEL_0.index,
		frame_index: 0,
		tile_frame_path: "/api/file/0/frame/0",
		positioning_status: "positioned",
		position_source: "tiled_full",
		tiling_status: "full",
		total_pixel_matrix: { rows: 512, columns: 512 },
		tile_rectangle: { x: 0, y: 0, width: 256, height: 256 },
		tile_row: 0,
		tile_column: 0,
		pyramid_uid: "1.2.3.pyramid",
		pyramid_level: 0,
		optical_path: null,
		focal_plane: null,
		image_type_role: "VOLUME",
		companions: [
			{ file_index: LEVEL_1.index, sop_instance_uid: LEVEL_1.sop_instance_uid, image_type_role: "VOLUME", pyramid_uid: "1.2.3.pyramid" },
			{ file_index: LABEL.index, sop_instance_uid: LABEL.sop_instance_uid, image_type_role: "LABEL", pyramid_uid: null },
			{ file_index: OVERVIEW.index, sop_instance_uid: OVERVIEW.sop_instance_uid, image_type_role: "OVERVIEW", pyramid_uid: null },
			{ file_index: 99, sop_instance_uid: "1.2.3.gone", image_type_role: "THUMBNAIL", pyramid_uid: null },
		],
		companions_truncated: false,
		relationships: [{
			relationship: "source_image",
			target: {
				sop_class_uid: null,
				sop_instance_uid: OVERVIEW.sop_instance_uid,
				series_instance_uid: null,
				frame_numbers: [],
				segment_numbers: [],
			},
			matches: [{ file_index: OVERVIEW.index, path: OVERVIEW.path, sop_instance_uid: OVERVIEW.sop_instance_uid, frame_indices: [] }],
		}],
		relationships_truncated: false,
		reconstruction_claimed: false,
		warnings: [],
		...overrides,
	};
}

function renderContext() {
	const onopenreference = vi.fn();
	render(WsiTileContext, {
		fileIndex: LEVEL_0.index,
		frame: 0,
		files: [LEVEL_0, LEVEL_1, LABEL, OVERVIEW],
		onopenreference,
	});
	return { onopenreference };
}

beforeEach(() => {
	fetchWsiFrameContext.mockReset();
});

describe("WsiTileContext companions and relationships", () => {
	it("groups companions by slide role and opens loaded ones", async () => {
		fetchWsiFrameContext.mockResolvedValue(wsiContext());
		const { onopenreference } = renderContext();

		const companions = await screen.findByLabelText("Slide companions");
		const rows = [...companions.querySelectorAll(".companion-row")].map((row) => row.textContent?.replace(/\s+/g, " ").trim());
		expect(rows).toEqual([
			"Label Open label.dcm · 4 frames",
			"Overview Open overview.dcm · 4 frames",
			"Thumbnail local target unavailable",
			"Pyramid levels Open level-1.dcm · 4 frames",
		]);

		await fireEvent.click(within(companions).getByRole("button", { name: "Open level-1.dcm · 4 frames" }));
		expect(onopenreference).toHaveBeenCalledWith(LEVEL_1.index, 0);
	});

	it("lists relationships through the shared reference edge", async () => {
		fetchWsiFrameContext.mockResolvedValue(wsiContext());
		const { onopenreference } = renderContext();

		const relationships = await screen.findByLabelText("Slide relationships");
		expect(within(relationships).getByText("source_image")).toBeTruthy();
		await fireEvent.click(within(relationships).getByRole("button", { name: "Open overview.dcm · frame 1" }));
		expect(onopenreference).toHaveBeenCalledWith(OVERVIEW.index, 0);
	});

	it("states when nothing is linked and when lists are truncated", async () => {
		fetchWsiFrameContext.mockResolvedValue(wsiContext({ companions: [], relationships: [], relationships_truncated: true }));
		renderContext();

		expect(await screen.findByText("No companion instances share this slide's pyramid or container")).toBeTruthy();
		expect(screen.getByText("No typed references")).toBeTruthy();
		expect(screen.getByText("Relationships (first 0)")).toBeTruthy();
	});
});

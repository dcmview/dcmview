// @vitest-environment happy-dom
import { fireEvent, render, screen } from "@testing-library/svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "../api";
import type { RtDoseContext, SemanticContextResponse } from "../generated/api-types";
import { fileSummary } from "../testing/fixtures";
import SemanticContextPanel from "./SemanticContextPanel.svelte";

vi.mock("../api", async (importOriginal) => ({
	...await importOriginal<typeof import("../api")>(),
	fetchSemanticContext: vi.fn(),
}));

const fetchSemanticContext = vi.mocked(api.fetchSemanticContext);

const RT_DOSE = fileSummary(3, {
	label: "dose.dcm",
	modality: "RTDOSE",
	sop_class_uid: "1.2.840.10008.5.1.4.1.1.481.2",
	object_kind: "radiation_therapy",
	frame_count: 4,
	rows: 32,
	columns: 48,
});
const CT = fileSummary(7, { label: "ct-slice.dcm" });

function rtDoseContext(overrides: Partial<RtDoseContext> = {}): SemanticContextResponse {
	return {
		source_file_index: RT_DOSE.index,
		default_mode: "pixel_preview",
		pixel_preview_preserves_stored_values: true,
		context: {
			kind: "rt_dose",
			dose_grid_scaling: 0.0025,
			scaling_status: "available",
			displayed_value_kind: "mapped",
			dose_units: "GY",
			dose_type: "PHYSICAL",
			dose_summation_type: "PLAN",
			geometry: {
				frame_of_reference_uid: "1.2.826.0.1.99",
				image_position_patient: [-120.5, -80, 12],
				image_orientation_patient: [1, 0, 0, 0, 1, 0],
				pixel_spacing: [2.5, 2.5],
				grid_frame_offsets: [0, 3, 6, 9],
			},
			references: [
				{
					relationship: "referenced_rt_plan",
					target: {
						sop_class_uid: "1.2.840.10008.5.1.4.1.1.481.5",
						sop_instance_uid: "1.2.3.plan",
						series_instance_uid: null,
						frame_numbers: [],
						segment_numbers: [],
					},
					matches: [],
				},
				{
					relationship: "source_image",
					target: {
						sop_class_uid: CT.sop_class_uid,
						sop_instance_uid: CT.sop_instance_uid,
						series_instance_uid: null,
						frame_numbers: [],
						segment_numbers: [],
					},
					matches: [{ file_index: CT.index, path: CT.path, sop_instance_uid: CT.sop_instance_uid, frame_indices: [0] }],
				},
			],
			overlay: { eligible: false, reason: "not drawn", source_file_index: null, mapped_source_count: 0 },
			clinical_use_warning: "Not for clinical use.",
			...overrides,
		},
	};
}

function renderPanel(file = RT_DOSE) {
	const onopenreference = vi.fn();
	render(SemanticContextPanel, {
		fileIndex: file.index,
		currentFrame: 0,
		files: [RT_DOSE, CT],
		onopenreference,
	});
	return { onopenreference };
}

async function showSemanticContext() {
	const button = await screen.findByRole("button", { name: "Semantic Context" });
	await vi.waitFor(() => expect(button).not.toHaveProperty("disabled", true));
	await fireEvent.click(button);
}

beforeEach(() => {
	fetchSemanticContext.mockReset();
});

describe("SemanticContextPanel RT Dose section", () => {
	it("shows the dose grid geometry", async () => {
		fetchSemanticContext.mockResolvedValue(rtDoseContext());
		renderPanel();
		await showSemanticContext();

		expect(screen.getByText("48 × 32 × 4")).toBeTruthy();
		expect(screen.getByText("2.5 \\ 2.5 mm")).toBeTruthy();
		expect(screen.getByText("4 planes, 0 to 9 mm, 3 mm step")).toBeTruthy();
		expect(screen.getByText("-120.5 \\ -80 \\ 12")).toBeTruthy();
		expect(screen.getByText("1 \\ 0 \\ 0 \\ 0 \\ 1 \\ 0")).toBeTruthy();
		expect(screen.getByText("1.2.826.0.1.99")).toBeTruthy();
		expect(screen.queryByText(/Dose Grid Scaling is/)).toBeNull();
	});

	it("lists references and opens the ones that resolve to a loaded file", async () => {
		fetchSemanticContext.mockResolvedValue(rtDoseContext());
		const { onopenreference } = renderPanel();
		await showSemanticContext();

		expect(screen.getByText("referenced_rt_plan")).toBeTruthy();
		expect(screen.getByText("unresolved")).toBeTruthy();
		await fireEvent.click(screen.getByRole("button", { name: "Open ct-slice.dcm · frame 1" }));
		expect(onopenreference).toHaveBeenCalledWith(CT.index, 0);
	});

	it("reports missing scaling and absent geometry explicitly", async () => {
		fetchSemanticContext.mockResolvedValue(rtDoseContext({
			dose_grid_scaling: null,
			scaling_status: "missing_or_malformed",
			displayed_value_kind: "stored",
			geometry: {
				frame_of_reference_uid: null,
				image_position_patient: null,
				image_orientation_patient: null,
				pixel_spacing: null,
				grid_frame_offsets: [],
			},
			references: [],
		}));
		renderPanel();
		await showSemanticContext();

		expect(screen.getByText(/Dose Grid Scaling is missing or malformed/)).toBeTruthy();
		expect(screen.getAllByText("Not declared").length).toBeGreaterThanOrEqual(5);
		expect(screen.getByText("No plan, structure set, or image references are declared.")).toBeTruthy();
	});
});

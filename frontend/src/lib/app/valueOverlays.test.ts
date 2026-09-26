import { describe, expect, it, vi } from "vitest";
import type {
	FileSummary,
	OverlayLegend,
	RtDoseContext,
	SemanticContextResponse,
	SeriesSummary,
} from "../../api";
import { fileSummary } from "../../testing/fixtures";
import {
	OVERLAY_SOURCE_FRAME_LIMIT,
	overlayEntryFrame,
	ValueOverlays,
} from "./valueOverlays.svelte";

const RT_DOSE_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.481.2";
const LEGEND: OverlayLegend = {
	unit_label: "Gy",
	units: null,
	min_value: 0,
	max_value: 23.3,
	transparent_at_or_below: 0,
	colormap: "viridis",
	color_stops: [[68, 1, 84], [253, 231, 37]],
};

function dose(index: number): FileSummary {
	return fileSummary(index, {
		path: `plan/dose-${index}.dcm`,
		modality: "RTDOSE",
		sop_class_uid: RT_DOSE_SOP_CLASS_UID,
		object_kind: "radiation_therapy",
		frame_count: 3,
	});
}

function doseContext(index: number, overrides: Partial<RtDoseContext> = {}): SemanticContextResponse {
	return {
		source_file_index: index,
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
			overlay: { eligible: true, reason: "covers", source_file_index: null, mapped_source_count: 2 },
			overlay_source_frames: [
				{ file_index: 1, frame_index: 0, sop_instance_uid: "ct.1" },
				{ file_index: 2, frame_index: 0, sop_instance_uid: "ct.2" },
			],
			legend: LEGEND,
			clinical_use_warning: "Not for clinical use.",
			...overrides,
		},
	};
}

function series(id: string, frameOfReference: string, fileIndexes: number[]): SeriesSummary {
	return {
		id,
		frame_of_reference_uids: [frameOfReference],
		stacks: [{
			id,
			frames: fileIndexes.map((file_index, virtual_index) => ({ virtual_index, file_index, frame_index: 0 })),
		}],
	} as unknown as SeriesSummary;
}

const CT_FRAMES = [1, 2, 3].map((file_index, virtual_index) => ({ virtual_index, file_index, frame_index: 0 }));

function overlays(contexts: Record<number, SemanticContextResponse>, { complete = true } = {}) {
	const files = new Map([1, 2, 3].map((index) => [index, fileSummary(index)]));
	for (const index of [7, 8, 9]) files.set(index, dose(index));
	const state = { complete };
	const load = vi.fn(async (index: number) => contexts[index]);
	const controller = new ValueOverlays({
		files: () => files,
		series: () => [
			series("ct", "1.2.3.for", [1, 2, 3]),
			series("dose", "1.2.3.for", [7, 8]),
			series("other", "9.9.9.for", [9]),
		],
		scanComplete: () => state.complete,
		load,
	});
	return { controller, load, state };
}

describe("ValueOverlays", () => {
	it("reads only the volumes that share the displayed image's Frame of Reference", async () => {
		const { controller, load } = overlays({ 7: doseContext(7), 8: doseContext(8) });

		controller.load(1);
		controller.load(1);

		expect(load.mock.calls.map(([index]) => index)).toEqual([7, 8]);
		await vi.waitFor(() => expect(controller.candidatesFor(1, CT_FRAMES)).toHaveLength(2));
	});

	it("offers eligible volumes covering the tab and tells alike ones apart", async () => {
		const { controller } = overlays({
			7: doseContext(7),
			8: doseContext(8, {
				overlay: { eligible: false, reason: "no dose", source_file_index: null, mapped_source_count: 0 },
				legend: null,
			}),
		});
		controller.load(1);
		await vi.waitFor(() => expect(controller.candidatesFor(1, CT_FRAMES)).toHaveLength(1));
		expect(controller.candidatesFor(1, CT_FRAMES)[0]).toMatchObject({
			kind: "rt_dose",
			volumeFileIndex: 7,
			title: "RT Dose · PLAN",
			legend: LEGEND,
		});

		const both = overlays({ 7: doseContext(7), 8: doseContext(8) });
		both.controller.load(1);
		await vi.waitFor(() => expect(both.controller.candidatesFor(1, CT_FRAMES)).toHaveLength(2));
		expect(both.controller.candidatesFor(1, CT_FRAMES).map((candidate) => candidate.title))
			.toEqual(["RT Dose · PLAN · dose-7.dcm", "RT Dose · PLAN · dose-8.dcm"]);
	});

	it("shows the selected volume with its opacity and whether it covers the frame", async () => {
		const { controller } = overlays({ 7: doseContext(7), 8: doseContext(8) });
		controller.load(1);
		await vi.waitFor(() => expect(controller.candidatesFor(1, CT_FRAMES)).toHaveLength(2));
		const candidates = controller.candidatesFor(1, CT_FRAMES);

		expect(controller.overlayFor(candidates, 1, 0)).toBeNull();
		controller.toggle(7);
		controller.setOpacity(1.4);
		expect(controller.overlayFor(candidates, 1, 0)).toMatchObject({
			kind: "rt_dose",
			volumeFileIndex: 7,
			opacity: 1,
			coversFrame: true,
		});
		// File 3 is in the stack but outside the dose grid.
		expect(controller.overlayFor(candidates, 3, 0)).toMatchObject({ coversFrame: false });
		controller.toggle(7);
		expect(controller.overlayFor(candidates, 1, 0)).toBeNull();
	});

	it("treats a capped coverage list as possibly covering unlisted frames", async () => {
		const frames = Array.from({ length: OVERLAY_SOURCE_FRAME_LIMIT }, (_, frame_index) => (
			{ file_index: 40, frame_index, sop_instance_uid: "big" }
		));
		const { controller } = overlays({ 7: doseContext(7, { overlay_source_frames: frames }) });
		controller.load(1);
		await vi.waitFor(() => expect(controller.candidatesFor(1, CT_FRAMES)).toHaveLength(1));
		expect(controller.candidatesFor(1, CT_FRAMES)[0].covers(3, 0)).toBe(true);
	});

	it("reads a context again once discovery completes", async () => {
		const { controller, load, state } = overlays({ 7: doseContext(7), 8: doseContext(8) }, { complete: false });
		controller.load(1);
		await vi.waitFor(() => expect(controller.candidatesFor(1, CT_FRAMES)).toHaveLength(2));
		controller.load(1);
		expect(load).toHaveBeenCalledTimes(2);

		state.complete = true;
		controller.load(1);
		controller.load(1);
		expect(load).toHaveBeenCalledTimes(4);
	});
});

describe("overlayEntryFrame", () => {
	it("opens the declared source image, else the first covered frame", () => {
		expect(overlayEntryFrame(doseContext(7))).toEqual({ fileIndex: 1, frameIndex: 0 });
		expect(overlayEntryFrame(doseContext(7, {
			overlay: { eligible: true, reason: "covers", source_file_index: 2, mapped_source_count: 2 },
		}))).toEqual({ fileIndex: 2, frameIndex: 0 });
		expect(overlayEntryFrame(doseContext(7, {
			overlay: { eligible: false, reason: "no", source_file_index: null, mapped_source_count: 0 },
		}))).toBeNull();
	});
});

function parametricMapContext(index: number, eligible = true): SemanticContextResponse {
	return {
		source_file_index: index,
		default_mode: "pixel_preview",
		pixel_preview_preserves_stored_values: true,
		context: {
			kind: "parametric_map",
			stored_value_type: "integer",
			displayed_value_kind: "stored",
			mappings: [{
				source: "embedded",
				source_sop_instance_uid: null,
				label: "ADC",
				first_value_mapped: 0,
				last_value_mapped: 4095,
				slope: 0.5,
				intercept: -10,
				lut_data: [],
				lut_data_truncated: false,
				units: { value: "um2/s", scheme: "UCUM", meaning: "um2/s" },
				quantity: { value: "113041", scheme: "DCM", meaning: "Apparent Diffusion Coefficient" },
				derivation: null,
			}],
			mapping_status: "mapping_available",
			source_references: [],
			warnings: [],
			overlay: eligible
				? { eligible: true, reason: "2 local image frame(s) lie within it", source_file_index: null, mapped_source_count: 2 }
				: { eligible: false, reason: "mappings use different units", source_file_index: null, mapped_source_count: 0 },
			overlay_source_frames: eligible ? [{ file_index: 1, frame_index: 0, sop_instance_uid: "mr.1" }] : [],
			legend: eligible
				? { ...LEGEND, unit_label: "um2/s", min_value: 0, max_value: 665, transparent_at_or_below: null }
				: null,
		},
	};
}

describe("ValueOverlays Parametric Maps", () => {
	it("offers an eligible map on its source images, titled by its quantity", async () => {
		const files = new Map<number, FileSummary>([
			[1, fileSummary(1, { modality: "MR" })],
			[4, fileSummary(4, { modality: "MR", object_kind: "parametric_map", frame_count: 2 })],
		]);
		const controller = new ValueOverlays({
			files: () => files,
			series: () => [series("mr", "2.2.for", [1]), series("pm", "2.2.for", [4])],
			scanComplete: () => true,
			load: async (index) => parametricMapContext(index),
		});

		controller.load(1);
		await vi.waitFor(() => expect(controller.candidatesFor(1, [])).toHaveLength(1));
		const [candidate] = controller.candidatesFor(1, []);
		expect(candidate).toMatchObject({ kind: "parametric_map", volumeFileIndex: 4, title: "Parametric Map · ADC" });
		expect(candidate.legend.unit_label).toBe("um2/s");
		controller.select(4);
		expect(controller.overlayFor([candidate], 1, 0)).toMatchObject({ kind: "parametric_map", coversFrame: true });
		expect(overlayEntryFrame(parametricMapContext(4))).toEqual({ fileIndex: 1, frameIndex: 0 });
		expect(overlayEntryFrame(parametricMapContext(4, false))).toBeNull();
	});
});

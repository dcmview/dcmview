import { describe, expect, it, vi } from "vitest";
import type {
	FileSummary,
	GraphicAnnotationItemSummary,
	PresentationStateContext,
	SemanticContextResponse,
} from "../../api";
import { fileSummary } from "../../testing/fixtures";
import {
	ANNOTATED_FRAME_LIMIT,
	annotationEntryFrame,
	GraphicAnnotations,
	presentationStateTitle,
} from "./graphicAnnotations.svelte";

const GSPS_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.11.1";

function state(index: number, overrides: Partial<FileSummary> = {}): FileSummary {
	return fileSummary(index, {
		path: `study/state-${index}.dcm`,
		modality: "PR",
		sop_class_uid: GSPS_SOP_CLASS_UID,
		object_kind: "presentation_state",
		has_pixels: false,
		...overrides,
	});
}

function item(index: number, file: number, frame: number, overrides: Partial<GraphicAnnotationItemSummary> = {}): GraphicAnnotationItemSummary {
	return {
		index,
		layer: "SHAPES",
		graphic_types: ["ellipse"],
		texts: [],
		scoped: true,
		first_frame: { file_index: file, frame_index: frame, sop_instance_uid: `image.${file}` },
		frame_count: 1,
		...overrides,
	};
}

function stateContext(index: number, overrides: Partial<PresentationStateContext> = {}): SemanticContextResponse {
	const items = overrides.items ?? [item(0, 1, 0), item(1, 1, 2)];
	return {
		source_file_index: index,
		default_mode: "pixel_preview",
		pixel_preview_preserves_stored_values: true,
		context: {
			kind: "presentation_state",
			content_label: "ROIS",
			content_description: "Reader marks",
			content_creator_name: null,
			presentation_creation_date: null,
			layers: [],
			items,
			annotated_frames: items.flatMap((entry) => entry.first_frame ? [entry.first_frame] : []),
			skipped: { display_units: 0, matrix_units: 0, malformed: 0, masked_text: 0 },
			references: [],
			...overrides,
		},
	};
}

function controller(files: FileSummary[], contexts: Record<number, SemanticContextResponse>, scanComplete = () => true) {
	const load = vi.fn(async (index: number) => contexts[index]);
	const annotations = new GraphicAnnotations({
		files: () => new Map(files.map((file) => [file.index, file])),
		scanComplete,
		load,
	});
	return { annotations, load };
}

const image = fileSummary(1, { frame_count: 3 });
const frames = [0, 1, 2].map((frame) => ({ virtual_index: frame, file_index: 1, frame_index: frame }));

describe("GraphicAnnotations", () => {
	it("offers the states of the image's study that annotate a frame of the tab", async () => {
		const files = [image, state(5), state(6), state(7, { study_instance_uid: "9.9.9" }), fileSummary(8)];
		const { annotations, load } = controller(files, {
			5: stateContext(5),
			// Annotates another image only.
			6: stateContext(6, { items: [item(0, 8, 0)] }),
			7: stateContext(7),
		});
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));

		// The state of another study is never read.
		expect(load.mock.calls.map(([index]) => index).sort()).toEqual([5, 6]);
		const [candidate] = annotations.candidatesFor(1, frames);
		expect(candidate).toMatchObject({ stateFileIndex: 5, title: "ROIS · Reader marks", detail: "study/state-5.dcm" });
		expect(candidate.covers(1, 0)).toBe(true);
		expect(candidate.covers(1, 1)).toBe(false);
	});

	it("draws nothing until a state is chosen, then draws it with every item alike", async () => {
		const { annotations } = controller([image, state(5)], { 5: stateContext(5) });
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));
		const candidates = annotations.candidatesFor(1, frames);

		expect(annotations.selectionFor(candidates)).toBeNull();
		annotations.select(5);
		expect(annotations.selectionFor(candidates)).toEqual({ stateFileIndex: 5, highlightedItem: null });
		annotations.select(null);
		expect(annotations.selectionFor(candidates)).toBeNull();
	});

	it("steps through items via 'all items' and opens each scoped item's first frame", async () => {
		const { annotations } = controller([image, state(5)], { 5: stateContext(5) });
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));
		const candidates = annotations.candidatesFor(1, frames);
		annotations.select(5);
		const current = { fileIndex: 1, frameIndex: 0 };

		expect(annotations.stepItem(candidates, 1, current)).toEqual({ fileIndex: 1, frameIndex: 0 });
		expect(annotations.selectedItem).toBe(0);
		expect(annotations.stepItem(candidates, 1, current)).toEqual({ fileIndex: 1, frameIndex: 2 });
		expect(annotations.selectedItem).toBe(1);
		// Past the last item is "all items", which moves nowhere.
		expect(annotations.stepItem(candidates, 1, current)).toBeNull();
		expect(annotations.selectedItem).toBeNull();
		// Backwards from "all items" is the last item.
		expect(annotations.stepItem(candidates, -1, current)).toEqual({ fileIndex: 1, frameIndex: 2 });
		expect(annotations.selectedItem).toBe(1);
	});

	it("keeps the displayed frame for an item that applies to every referenced image", async () => {
		const everywhere = item(0, 1, 0, { scoped: false, frame_count: 3 });
		const { annotations } = controller([image, state(5)], {
			5: stateContext(5, {
				items: [everywhere],
				annotated_frames: frames.map((frame) => ({ file_index: 1, frame_index: frame.frame_index, sop_instance_uid: "image.1" })),
			}),
		});
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));
		annotations.select(5);

		expect(annotations.stepItem(annotations.candidatesFor(1, frames), 1, { fileIndex: 1, frameIndex: 2 })).toBeNull();
		expect(annotations.selectedItem).toBe(0);
	});

	it("skips items with nothing to draw and reports skipped objects", async () => {
		const { annotations } = controller([image, state(5)], {
			5: stateContext(5, {
				items: [item(0, 1, 0, { graphic_types: [], texts: [] }), item(1, 1, 0, { graphic_types: [], texts: ["Mass"] })],
				skipped: { display_units: 2, matrix_units: 0, malformed: 1, masked_text: 0 },
			}),
		});
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));
		const [candidate] = annotations.candidatesFor(1, frames);

		expect(candidate.items.map((entry) => entry.index)).toEqual([1]);
		expect(candidate.skippedObjects).toBe(3);
	});

	it("keeps offering a shown state from another study", async () => {
		const { annotations } = controller([image, state(7, { study_instance_uid: "9.9.9" })], { 7: stateContext(7) });
		annotations.selectItem(7, 1);
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));

		expect(annotations.selectionFor(annotations.candidatesFor(1, frames))).toEqual({ stateFileIndex: 7, highlightedItem: 1 });
	});

	it("re-reads a state read during discovery once the scan completes", async () => {
		let complete = false;
		const { annotations, load } = controller([image, state(5)], { 5: stateContext(5) }, () => complete);
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));
		annotations.load(1);
		expect(load).toHaveBeenCalledTimes(1);

		complete = true;
		annotations.load(1);
		annotations.load(1);
		expect(load).toHaveBeenCalledTimes(2);
	});

	it("treats a capped frame list as possibly covering any frame", async () => {
		const many = Array.from({ length: ANNOTATED_FRAME_LIMIT }, (_, frame) => ({ file_index: 9, frame_index: frame, sop_instance_uid: "image.9" }));
		const { annotations } = controller([image, state(5)], { 5: stateContext(5, { annotated_frames: many }) });
		annotations.load(1);
		await vi.waitFor(() => expect(annotations.candidatesFor(1, frames)).toHaveLength(1));

		expect(annotations.candidatesFor(1, frames)[0].covers(1, 1)).toBe(true);
	});
});

describe("presentation state helpers", () => {
	it("titles a state by its label and description", () => {
		const { context } = stateContext(5);
		if (context.kind !== "presentation_state") throw new Error("expected a presentation state");
		expect(presentationStateTitle(context)).toBe("ROIS · Reader marks");
		expect(presentationStateTitle({ ...context, content_label: null, content_description: " " })).toBe("Presentation state");
	});

	it("enters a state at its first annotated frame or at an item's first frame", () => {
		const response = stateContext(5);
		expect(annotationEntryFrame(response)).toEqual({ fileIndex: 1, frameIndex: 0 });
		expect(annotationEntryFrame(response, 1)).toEqual({ fileIndex: 1, frameIndex: 2 });
		expect(annotationEntryFrame(response, 9)).toBeNull();
	});
});

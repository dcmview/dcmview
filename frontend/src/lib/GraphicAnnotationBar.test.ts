// @vitest-environment happy-dom
import { fireEvent, render, screen } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";
import type { GraphicAnnotationItemSummary } from "../api";
import type { GraphicAnnotationCandidate } from "./app/graphicAnnotations.svelte";
import GraphicAnnotationBar from "./GraphicAnnotationBar.svelte";

function item(index: number, overrides: Partial<GraphicAnnotationItemSummary> = {}): GraphicAnnotationItemSummary {
	return { index, layer: "SHAPES", graphic_types: ["ellipse"], texts: [], scoped: true, first_frame: null, frame_count: 1, ...overrides };
}

function candidate(stateFileIndex: number, title: string, overrides: Partial<GraphicAnnotationCandidate> = {}): GraphicAnnotationCandidate {
	return {
		stateFileIndex,
		title,
		detail: `study/state-${stateFileIndex}.dcm`,
		items: [item(0), item(2, { graphic_types: ["circle"], texts: ["Mass\nupper outer"] })],
		skippedObjects: 0,
		covers: () => true,
		...overrides,
	};
}

function renderBar(props: Partial<{ selectedState: number | null; selectedItem: number | null; coversFrame: boolean; candidates: GraphicAnnotationCandidate[] }> = {}) {
	const ontoggle = vi.fn();
	const onstep = vi.fn();
	render(GraphicAnnotationBar, {
		candidates: [candidate(5, "ROIS · Reader marks"), candidate(6, "BOXES")],
		selectedState: null,
		selectedItem: null,
		coversFrame: true,
		ontoggle,
		onstep,
		...props,
	});
	return { ontoggle, onstep };
}

describe("GraphicAnnotationBar", () => {
	it("offers each state and no item stepper until one is shown", async () => {
		const { ontoggle } = renderBar();

		const rois = screen.getByRole("button", { name: "ROIS · Reader marks" });
		expect(rois.getAttribute("aria-pressed")).toBe("false");
		expect(screen.queryByRole("group", { name: "Annotation item" })).toBeNull();
		await fireEvent.click(rois);
		expect(ontoggle).toHaveBeenCalledWith(5);
	});

	it("steps through the shown state's items and names the current one", async () => {
		const { onstep } = renderBar({ selectedState: 5 });

		expect(screen.getByRole("button", { name: "ROIS · Reader marks" }).getAttribute("aria-pressed")).toBe("true");
		expect(screen.getByText("All 2 items")).toBeTruthy();
		await fireEvent.click(screen.getByRole("button", { name: "Next annotation item" }));
		await fireEvent.click(screen.getByRole("button", { name: "Previous annotation item" }));
		expect(onstep.mock.calls).toEqual([[1], [-1]]);
	});

	it("counts the stepped item by position and names it by its text, else its graphic types", () => {
		renderBar({ selectedState: 5, selectedItem: 2 });
		expect(screen.getByText("Item 2 of 2 · Mass")).toBeTruthy();
	});

	it("says when the shown state is not on the frame and when objects are not drawn", () => {
		renderBar({
			selectedState: 6,
			coversFrame: false,
			candidates: [candidate(6, "BOXES", { skippedObjects: 2 })],
		});

		expect(screen.getByText("Not on this frame")).toBeTruthy();
		expect(screen.getByText("2 objects not drawn")).toBeTruthy();
	});
});

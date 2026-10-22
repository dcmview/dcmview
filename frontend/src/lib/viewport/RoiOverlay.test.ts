// @vitest-environment happy-dom
import { render } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import { DEFAULT_ORIENTATION } from "../viewerTools";
import RoiLabels from "./RoiLabels.svelte";
import RoiOverlay from "./RoiOverlay.svelte";
import type { VisibleRoi } from "./roiEditing";

const rois: VisibleRoi[] = [
	{ index: 0, ymin: 20, xmin: 10, ymax: 60, xmax: 40, frames: null },
	{ index: 1, ymin: 5, xmin: 5, ymax: 15, xmax: 15, frames: null },
];
const overlay = { rois, rows: 100, columns: 100, scale: 1, pixelAspectRatio: 1 };
const labels = {
	rois,
	selectedIndex: null,
	transform: { scale: 1, tx: 0, ty: 0, fit: false },
	orientation: DEFAULT_ORIENTATION,
	geometry: { width: 100, height: 100, centerX: 50, centerY: 50, pixelAspectRatio: 1 },
};

describe("RoiOverlay", () => {
	it("outlines every ROI without native records", () => {
		const { container } = render(RoiOverlay, { ...overlay, selectedIndex: null, draft: null });

		expect(container.querySelectorAll(".roi-rect")).toHaveLength(2);
	});

	it("leaves the outline of a ROI the native overlay draws, keeping the draft", () => {
		const { container } = render(RoiOverlay, {
			...overlay,
			selectedIndex: null,
			draft: [70, 70, 80, 80],
			nativeDrawn: new Set([0]),
		});

		const outlines = [...container.querySelectorAll(".roi-rect")];
		expect(outlines.map((rect) => rect.getAttribute("x"))).toEqual(["5", "70"]);
		expect(outlines[1].classList.contains("draft")).toBe(true);
	});

	it("keeps the selection highlight and handles of a natively drawn ROI", () => {
		const { container } = render(RoiOverlay, { ...overlay, selectedIndex: 0, draft: null, nativeDrawn: new Set([0, 1]) });

		const outlines = [...container.querySelectorAll(".roi-rect")];
		expect(outlines.map((rect) => rect.getAttribute("x"))).toEqual(["10"]);
		expect(outlines[0].closest("g")?.classList.contains("selected")).toBe(true);
		expect(container.querySelectorAll(".roi-handle")).toHaveLength(8);
	});
});

describe("RoiLabels", () => {
	it("numbers every ROI without native records", () => {
		const { container } = render(RoiLabels, labels);

		expect([...container.querySelectorAll(".roi-label")].map((label) => label.textContent)).toEqual(["#1", "#2"]);
	});

	it("does not number a ROI the native labels name", () => {
		const { container } = render(RoiLabels, { ...labels, nativeDrawn: new Set([0]) });

		expect([...container.querySelectorAll(".roi-label")].map((label) => label.textContent)).toEqual(["#2"]);
	});
});

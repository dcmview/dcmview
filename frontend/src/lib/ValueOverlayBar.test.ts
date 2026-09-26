// @vitest-environment happy-dom
import { fireEvent, render, screen } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";
import type { OverlayLegend } from "../api";
import type { ValueOverlayCandidate } from "./app/valueOverlays.svelte";
import ValueOverlayBar from "./ValueOverlayBar.svelte";

const legend = { unit_label: "Gy" } as OverlayLegend;

function candidate(volumeFileIndex: number, title: string): ValueOverlayCandidate {
	return {
		kind: "rt_dose",
		volumeFileIndex,
		title,
		detail: `plan/dose-${volumeFileIndex}.dcm`,
		legend,
		covers: () => true,
	};
}

function renderBar(selectedVolume: number | null, coversFrame = true) {
	const ontoggle = vi.fn();
	const onopacity = vi.fn();
	render(ValueOverlayBar, {
		candidates: [candidate(7, "RT Dose · PLAN"), candidate(8, "RT Dose · BEAM")],
		selectedVolume,
		opacity: 0.5,
		coversFrame,
		ontoggle,
		onopacity,
	});
	return { ontoggle, onopacity };
}

describe("ValueOverlayBar", () => {
	it("toggles volumes and leaves opacity disabled while none is shown", async () => {
		const { ontoggle } = renderBar(null);

		const plan = screen.getByRole("button", { name: "RT Dose · PLAN" });
		expect(plan.getAttribute("aria-pressed")).toBe("false");
		expect(plan.getAttribute("title")).toBe("plan/dose-7.dcm");
		expect(screen.getByRole("slider")).toHaveProperty("disabled", true);

		await fireEvent.click(plan);
		expect(ontoggle).toHaveBeenCalledWith(7);
	});

	it("sets the shown volume's opacity from the slider", async () => {
		const { onopacity } = renderBar(8);

		expect(screen.getByRole("button", { name: "RT Dose · BEAM" }).getAttribute("aria-pressed")).toBe("true");
		const slider = screen.getByRole("slider") as HTMLInputElement;
		expect(slider.disabled).toBe(false);
		expect(slider.value).toBe("50");
		expect(screen.getByText("50%")).toBeTruthy();

		slider.value = "80";
		await fireEvent.input(slider);
		expect(onopacity).toHaveBeenCalledWith(0.8);
	});

	it("notes a displayed frame the shown volume does not cover", () => {
		renderBar(7, false);
		expect(screen.getByText("Not covering this frame")).toBeTruthy();
	});
});

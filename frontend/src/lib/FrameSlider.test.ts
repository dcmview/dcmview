// @vitest-environment happy-dom
import { fireEvent, render, screen } from "@testing-library/svelte";
import { expect, it, vi } from "vitest";
import FrameSlider from "./FrameSlider.svelte";

it("exposes the visible capture scrubber and stops playback when seeking", async () => {
	const move = vi.fn();
	render(FrameSlider, { props: { totalFrames: 50, currentPosition: 0, onpositionchange: move,
		cinePlaying: true, cineFps: 10, cineMode: "loop", cineDirection: 1 } });
	const slider = screen.getByRole("slider", { name: "Image position" });
	expect(slider.hasAttribute("hidden")).toBe(false);
	expect(slider.getAttribute("aria-valuetext")).toBe("Image 1 of 50");
	await fireEvent.input(slider, { target: { value: "49" } });
	expect(move).toHaveBeenCalledWith(49);
	expect(screen.getByRole("button", { name: "Play cine" })).toBeTruthy();
	expect(document.querySelector("[data-capture-position]")).toBeNull();
});

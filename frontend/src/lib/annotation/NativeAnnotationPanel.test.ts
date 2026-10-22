// @vitest-environment happy-dom
import { fireEvent, render, screen, within } from "@testing-library/svelte";
import { describe, expect, it, vi } from "vitest";
import { nativeResponse } from "../../testing/nativeAnnotations";
import type { NativeAnnotationsResponse } from "./nativeAnnotations";
import NativeAnnotationPanel from "./NativeAnnotationPanel.svelte";

function renderPanel(props: Partial<{ response: NativeAnnotationsResponse; frame: number; hiddenLayers: Set<string>; hiddenClasses: Set<string> }> = {}) {
	const onlayertoggle = vi.fn();
	const onclasstoggle = vi.fn();
	render(NativeAnnotationPanel, {
		response: nativeResponse(),
		frame: 0,
		hiddenLayers: new Set<string>(),
		hiddenClasses: new Set<string>(),
		onlayertoggle,
		onclasstoggle,
		...props,
	});
	return { onlayertoggle, onclasstoggle };
}

describe("NativeAnnotationPanel", () => {
	it.each([" ", "Enter"])("keeps %j on a toggle from the window's shortcuts", async (key) => {
		renderPanel();
		const onWindowKey = vi.fn();
		window.addEventListener("keydown", onWindowKey);

		await fireEvent.keyDown(screen.getByRole("button", { name: /^Mass,/ }), { key });
		await fireEvent.keyDown(screen.getByRole("button", { name: /^Reader,/ }), { key });
		await fireEvent.keyDown(screen.getByRole("button", { name: /^Mass,/ }), { key: "ArrowRight" });

		window.removeEventListener("keydown", onWindowKey);
		expect(onWindowKey.mock.calls.map(([event]) => event.key)).toEqual(["ArrowRight"]);
	});

	it("lists each class with its color and count, masks included and noted", () => {
		renderPanel();

		const classes = screen.getByRole("list", { name: "Classes" });
		const buttons = within(classes).getAllByRole("button");
		expect(buttons.map((button) => button.getAttribute("aria-label"))).toEqual([
			"Mass, 4 shapes, 1 not drawn",
			"Calcification, 2 shapes",
			"Clip, 1 shape",
		]);
		expect((buttons[0].querySelector(".swatch") as HTMLElement).style.getPropertyValue("--swatch")).toBe("#e4572e");
		expect(within(classes).getByText("1 not drawn")).toBeTruthy();
	});

	it("toggles classes and layers from the keyboard-reachable buttons", async () => {
		const { onlayertoggle, onclasstoggle } = renderPanel({ hiddenLayers: new Set(["L-model"]) });

		const mass = screen.getByRole("button", { name: /^Mass/ });
		expect(mass.getAttribute("aria-pressed")).toBe("true");
		await fireEvent.click(mass);
		expect(onclasstoggle).toHaveBeenCalledWith("mass");

		const model = screen.getByRole("button", { name: "Model v2, 1 shape, read-only" });
		expect(model.getAttribute("aria-pressed")).toBe("false");
		expect(model.getAttribute("title")).toBe("Show Model v2");
		await fireEvent.click(model);
		expect(onlayertoggle).toHaveBeenCalledWith("L-model");
		expect(screen.getByRole("button", { name: "Review, 0 shapes" })).toBeTruthy();
	});

	it("marks hidden classes as not pressed", () => {
		renderPanel({ hiddenClasses: new Set(["calc"]) });
		expect(screen.getByRole("button", { name: "Calcification, 2 shapes" }).getAttribute("aria-pressed")).toBe("false");
	});

	it("shows the labels that apply to the shown frame, grouped by level", () => {
		renderPanel({ frame: 1, hiddenLayers: new Set(["L-model"]) });

		expect(screen.getAllByRole("heading", { level: 4 }).map((heading) => heading.textContent)).toEqual(["Study", "File", "Frame"]);
		const entries = (title: string) => [...screen.getByRole("heading", { level: 4, name: title }).nextElementSibling!.querySelectorAll(".native-label")]
			.map((entry) => [entry.querySelector("dt")!.textContent, entry.querySelector("dd")!.textContent!.trim()]);
		expect(entries("Study")).toEqual([["Density", "Heterogeneously dense"]]);
		expect(entries("File")).toEqual([["Artifact", "No"]]);
		expect(entries("Frame")).toEqual([["Artifact", "No"]]);
		// The model layer's thickness label is hidden with its layer.
		expect(screen.queryByText("Thickness")).toBeNull();
	});

	it("shows a score after a label's value", () => {
		renderPanel();
		expect(screen.getByText("Thickness").nextElementSibling!.textContent!.trim()).toBe("4.5 mm 0.60");
	});

	it("says when the file has no annotations", () => {
		renderPanel({ response: nativeResponse({ annotations: [], labels: [] }) });
		expect(screen.getByText("No annotations on this file")).toBeTruthy();
		expect(screen.queryByRole("heading", { name: "Labels" })).toBeNull();
	});
});

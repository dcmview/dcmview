// @vitest-environment happy-dom
import { render } from "@testing-library/svelte";
import { describe, expect, it } from "vitest";
import { nativeResponse } from "../../testing/nativeAnnotations";
import { DEFAULT_ORIENTATION } from "../viewerTools";
import { visibleShapes } from "./display";
import NativeAnnotationLabels from "./NativeAnnotationLabels.svelte";
import NativeAnnotationOverlay from "./NativeAnnotationOverlay.svelte";

const shapes = visibleShapes(nativeResponse(), { frame: 0 });
const geometry = { width: 100, height: 100, centerX: 50, centerY: 50, pixelAspectRatio: 1 };

describe("NativeAnnotationOverlay", () => {
	it("draws every shape but points in image pixels, colored, with a constant on-screen stroke", () => {
		const { container } = render(NativeAnnotationOverlay, { shapes, rows: 100, columns: 120, scale: 4 });

		const svg = container.querySelector("svg")!;
		expect(svg.getAttribute("viewBox")).toBe("0 0 120 100");
		expect(svg.style.getPropertyValue("--annotation-px")).toBe("0.25px");
		const drawn = [...svg.querySelectorAll(".native-shape")];
		expect(drawn.map((element) => element.getAttribute("data-id"))).toEqual(["a-rect", "a-ellipse", "a-polyline", "a-line"]);
		expect(drawn[0].getAttribute("d")).toBe("M10 20L40 20L40 60L10 60Z");
		expect((drawn[0] as SVGElement).style.getPropertyValue("--annotation-color")).toBe("#e4572e");
		expect(drawn[1].getAttribute("transform")).toBe("rotate(90 80 80)");
		expect((drawn[1] as SVGElement).style.getPropertyValue("--annotation-color")).toBe("#7b61ff");
	});
});

describe("NativeAnnotationLabels", () => {
	it("marks points unscaled and tags each shape with its class in its color", () => {
		const { container } = render(NativeAnnotationLabels, {
			shapes,
			transform: { scale: 2, tx: 10, ty: 0, fit: false },
			orientation: DEFAULT_ORIENTATION,
			geometry,
		});

		const marks = [...container.querySelectorAll(".point")];
		// The point at (30, 30) lands at (70, 60) on screen, with 5px arms at any zoom.
		expect(marks.map((mark) => mark.getAttribute("d"))).toEqual(["M65 60H75M70 55V65"]);
		const tags = [...container.querySelectorAll(".tag")];
		expect(tags.map((tag) => tag.textContent)).toEqual(["Mass", "Mass 0.87", "Calcification", "Calcification", "Clip"]);
		// The rectangle's top-left (10, 20) is at (30, 40): its tag sits just above.
		expect([tags[0].getAttribute("x"), tags[0].getAttribute("y")]).toEqual(["32", "36"]);
		expect((tags[1] as SVGElement).style.getPropertyValue("--annotation-color")).toBe("#7b61ff");
		// A tag on a shape at the viewport top moves inside it.
		expect(tags[3].getAttribute("y")).toBe("12");
	});
});

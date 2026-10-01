import { describe, expect, it } from "vitest";
import { graphicShape, itemEmphasis, layerColor } from "./graphicAnnotations";

describe("graphicShape", () => {
	it("draws a circle from its centre and a point on it", () => {
		expect(graphicShape({ graphic_type: "circle", points: [[200, 40], [222, 40]] }))
			.toEqual({ kind: "ellipse", cx: 200, cy: 40, rx: 22, ry: 22, rotation: 0 });
	});

	it("takes an ellipse's first two points as its major axis, wherever it points", () => {
		// Major axis along (4, 3)/5 with semi-axes 30 and 15, as in the golden fixture.
		const shape = graphicShape({ graphic_type: "ellipse", points: [[106, 27], [154, 63], [139, 33], [121, 57]] });
		expect(shape).toMatchObject({ kind: "ellipse", cx: 130, cy: 45, rx: 30, ry: 15 });
		expect(shape.kind === "ellipse" && shape.rotation).toBeCloseTo(36.8699, 3);

		// A "major" axis that is vertical and shorter than the other still rotates the ellipse.
		expect(graphicShape({ graphic_type: "ellipse", points: [[50, 30], [50, 50], [20, 40], [80, 40]] }))
			.toMatchObject({ cx: 50, cy: 40, rx: 10, ry: 30, rotation: 90 });
	});

	it("closes a polyline only when its last point repeats its first", () => {
		expect(graphicShape({ graphic_type: "polyline", points: [[0, 0], [4, 0], [4, 3], [0, 0]] }))
			.toEqual({ kind: "path", d: "M0 0L4 0L4 3Z", closed: true });
		expect(graphicShape({ graphic_type: "polyline", points: [[0, 0], [4, 0], [4, 3]] }))
			.toEqual({ kind: "path", d: "M0 0L4 0L4 3", closed: false });
		// Two coincident points are a degenerate open line, not a closed shape.
		expect(graphicShape({ graphic_type: "polyline", points: [[1, 1], [1, 1]] }))
			.toMatchObject({ closed: false });
	});

	it("passes an interpolated curve through every point", () => {
		const points: [number, number][] = [[0, 0], [10, 5], [20, 0], [30, 5]];
		const shape = graphicShape({ graphic_type: "interpolated", points });
		if (shape.kind !== "path") throw new Error("expected a path");
		expect(shape.closed).toBe(false);
		expect(shape.d.startsWith("M0 0C")).toBe(true);
		// Each cubic segment ends on the next declared point.
		const ends = [...shape.d.matchAll(/C[^C]*? ([-\d.]+) ([-\d.]+)(?=C|Z|$)/g)].map((match) => [Number(match[1]), Number(match[2])]);
		expect(ends).toEqual(points.slice(1));
	});

	it("closes an interpolated curve that returns to its first point", () => {
		const shape = graphicShape({ graphic_type: "interpolated", points: [[0, 0], [10, 0], [10, 10], [0, 10], [0, 0]] });
		if (shape.kind !== "path") throw new Error("expected a path");
		expect(shape.closed).toBe(true);
		expect(shape.d.endsWith("0 0Z")).toBe(true);
		expect(shape.d.match(/C/g)).toHaveLength(4);
	});

	it("keeps a point's sub-pixel position", () => {
		expect(graphicShape({ graphic_type: "point", points: [[120.5, 150.5]] })).toEqual({ kind: "point", x: 120.5, y: 150.5 });
	});
});

describe("layer colors and emphasis", () => {
	const layers = [
		{ name: "SHAPES", order: 1, description: null, color: [255, 212, 0] as [number, number, number] },
		{ name: "MARKS", order: 2, description: null, color: null },
	];

	it("uses a layer's recommended color and falls back for layers without one", () => {
		expect(layerColor(layers, "SHAPES")).toBe("rgb(255, 212, 0)");
		expect(layerColor(layers, "MARKS")).toBeNull();
		expect(layerColor(layers, "UNDECLARED")).toBeNull();
	});

	it("highlights the stepped item and dims the rest", () => {
		expect(itemEmphasis(2, null)).toBe("normal");
		expect(itemEmphasis(2, 2)).toBe("highlighted");
		expect(itemEmphasis(1, 2)).toBe("dimmed");
	});
});

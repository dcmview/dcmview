import { describe, expect, it } from "vitest";
import type { Geometry } from "../../generated/annotation-types";
import { nativeLayer, nativeRecord, nativeResponse } from "../../testing/nativeAnnotations";
import {
	FALLBACK_ANNOTATION_COLOR,
	annotationColor,
	annotationText,
	appliesToFrame,
	classColor,
	classLegend,
	formatLabelValue,
	geometryBounds,
	geometryShape,
	labelGroups,
	layerEntries,
	nativeView,
	safeColor,
	targetLevel,
	visibleShapes,
} from "./display";

describe("geometryShape", () => {
	it("draws a point, and lines and polylines as open paths", () => {
		expect(geometryShape({ type: "point", x: 3, y: 4 })).toEqual({ kind: "point", x: 3, y: 4 });
		expect(geometryShape({ type: "line", points: [{ x: 1, y: 2 }, { x: 3, y: 4 }] }))
			.toEqual({ kind: "path", d: "M1 2L3 4", closed: false });
		expect(geometryShape({ type: "polyline", points: [{ x: 0, y: 0 }, { x: 5, y: 5 }, { x: 10, y: 0 }] }))
			.toEqual({ kind: "path", d: "M0 0L5 5L10 0", closed: false });
	});

	it("keeps a polyline open even when its last point repeats its first", () => {
		expect(geometryShape({ type: "polyline", points: [{ x: 0, y: 0 }, { x: 4, y: 0 }, { x: 4, y: 3 }, { x: 0, y: 0 }] }))
			.toEqual({ kind: "path", d: "M0 0L4 0L4 3L0 0", closed: false });
	});

	it("does not draw a geometry type it does not know", () => {
		const future = { type: "spline", points: [] } as unknown as Geometry;
		expect(geometryShape(future)).toBeNull();
		expect(geometryBounds(future)).toBeNull();
	});

	it("closes a polygon whether or not its last point repeats its first", () => {
		const closed = { kind: "path", d: "M50 50L70 50L60 70Z", closed: true };
		expect(geometryShape({ type: "polygon", points: [{ x: 50, y: 50 }, { x: 70, y: 50 }, { x: 60, y: 70 }] })).toEqual(closed);
		expect(geometryShape({ type: "polygon", points: [{ x: 50, y: 50 }, { x: 70, y: 50 }, { x: 60, y: 70 }, { x: 50, y: 50 }] })).toEqual(closed);
	});

	it("draws a rectangle as a closed path on its corners", () => {
		expect(geometryShape({ type: "rect", x0: 10, y0: 20, x1: 40, y1: 60 }))
			.toEqual({ kind: "path", d: "M10 20L40 20L40 60L10 60Z", closed: true });
	});

	it("takes an ellipse's angle as its rotation in degrees", () => {
		expect(geometryShape({ type: "ellipse", cx: 80, cy: 80, rx: 10, ry: 5, angle: 30 }))
			.toEqual({ kind: "ellipse", cx: 80, cy: 80, rx: 10, ry: 5, rotation: 30 });
	});

	it("does not draw a mask", () => {
		expect(geometryShape({ type: "mask", encoding: "tiles-v1", tile: 64, depth: 1, frames: {} })).toBeNull();
	});
});

describe("geometryBounds", () => {
	it("bounds points, rectangles and point lists", () => {
		expect(geometryBounds({ type: "point", x: 3, y: 4 })).toEqual({ x0: 3, y0: 4, x1: 3, y1: 4 });
		expect(geometryBounds({ type: "rect", x0: 10, y0: 20, x1: 40, y1: 60 })).toEqual({ x0: 10, y0: 20, x1: 40, y1: 60 });
		expect(geometryBounds({ type: "polygon", points: [{ x: 50, y: 50 }, { x: 70, y: 52 }, { x: 60, y: 70 }] }))
			.toEqual({ x0: 50, y0: 50, x1: 70, y1: 70 });
	});

	it("bounds a rotated ellipse by its rotated axes", () => {
		const upright = geometryBounds({ type: "ellipse", cx: 80, cy: 80, rx: 10, ry: 5, angle: 90 })!;
		expect(upright.x0).toBeCloseTo(75);
		expect(upright.x1).toBeCloseTo(85);
		expect(upright.y0).toBeCloseTo(70);
		expect(upright.y1).toBeCloseTo(90);
		expect(geometryBounds({ type: "ellipse", cx: 0, cy: 0, rx: 10, ry: 10, angle: 45 })!.x1).toBeCloseTo(10);
	});
});

describe("appliesToFrame", () => {
	it("includes every frame for all and only the listed ones for a set", () => {
		expect(appliesToFrame("all", 12)).toBe(true);
		expect(appliesToFrame({ set: [0, 2] }, 2)).toBe(true);
		expect(appliesToFrame({ set: [0, 2] }, 1)).toBe(false);
	});
});

describe("annotationColor", () => {
	const classDef = nativeResponse().schema.classes[0];

	it("takes the layer's color, else the class's, else the fallback", () => {
		expect(annotationColor(nativeLayer("L", "L", "#7b61ff"), classDef)).toBe("#7b61ff");
		expect(annotationColor(nativeLayer("L", "L", null), classDef)).toBe("#e4572e");
		expect(annotationColor(nativeLayer("L", "L", null), { ...classDef, color: undefined })).toBe(FALLBACK_ANNOTATION_COLOR);
		expect(annotationColor(undefined, undefined)).toBe(FALLBACK_ANNOTATION_COLOR);
	});

	it("passes on only #RRGGBB colors", () => {
		expect(safeColor("#A1b2C3")).toBe("#A1b2C3");
		for (const color of ["url(https://example.invalid/x)", "#abc", "red", "#1234567", " #123456", "", null, undefined]) {
			expect(safeColor(color)).toBeNull();
		}
		expect(annotationColor(nativeLayer("L", "L", "url(https://example.invalid/x)"), classDef)).toBe("#e4572e");
		expect(annotationColor(undefined, { ...classDef, color: "var(--x)" })).toBe(FALLBACK_ANNOTATION_COLOR);
		expect(classColor({ ...classDef, color: "url(x)" })).toBe(FALLBACK_ANNOTATION_COLOR);
	});
});

describe("annotationText", () => {
	const { schema } = nativeResponse();

	it("names a record by its class, then its score", () => {
		expect(annotationText(schema, { class: "mass" })).toBe("Mass");
		expect(annotationText(schema, { class: "mass", score: 0.874 })).toBe("Mass 0.87");
		expect(annotationText(schema, { class: "mass", score: 0 })).toBe("Mass 0.00");
	});

	it("falls back to the class id the schema does not hold", () => {
		expect(annotationText(schema, { class: "unknown" })).toBe("unknown");
	});
});

describe("visibleShapes", () => {
	it("draws the frame's vector records in order with their colors and tags", () => {
		const shapes = visibleShapes(nativeResponse(), { frame: 0 });
		expect(shapes.map((shape) => shape.id)).toEqual(["a-rect", "a-ellipse", "a-point", "a-polyline", "a-line"]);
		expect(shapes[0]).toMatchObject({ color: "#e4572e", text: "Mass", embedIndex: 0, bounds: { x0: 10, y0: 20, x1: 40, y1: 60 } });
		expect(shapes[1]).toMatchObject({ color: "#7b61ff", text: "Mass 0.87", layer: "L-model", embedIndex: null });
		expect(shapes[4]).toMatchObject({ color: FALLBACK_ANNOTATION_COLOR, text: "Clip" });
	});

	it("keeps a record scoped to a frame set to those frames", () => {
		expect(visibleShapes(nativeResponse(), { frame: 1 }).map((shape) => shape.id)).toContain("a-polygon");
	});

	it("leaves out hidden layers and classes", () => {
		const ids = visibleShapes(nativeResponse(), {
			frame: 0,
			hiddenLayers: new Set(["L-model"]),
			hiddenClasses: new Set(["calc"]),
		}).map((shape) => shape.id);
		expect(ids).toEqual(["a-rect", "a-line"]);
	});
});

describe("classLegend", () => {
	it("counts every record of each class, masks included, in schema order", () => {
		expect(classLegend(nativeResponse())).toEqual([
			{ id: "mass", name: "Mass", color: "#e4572e", count: 4, notDrawn: 1 },
			{ id: "calc", name: "Calcification", color: "#29a3a3", count: 2, notDrawn: 0 },
			{ id: "clip", name: "Clip", color: FALLBACK_ANNOTATION_COLOR, count: 1, notDrawn: 0 },
		]);
	});

	it("counts a geometry type it does not know as not drawn", () => {
		const future = { type: "spline", points: [] } as unknown as Geometry;
		expect(classLegend(nativeResponse({ annotations: [nativeRecord("f", "clip", future)] })))
			.toEqual([{ id: "clip", name: "Clip", color: FALLBACK_ANNOTATION_COLOR, count: 1, notDrawn: 1 }]);
		expect(visibleShapes(nativeResponse({ annotations: [nativeRecord("f", "clip", future)] }), { frame: 0 })).toEqual([]);
	});

	it("leaves out unused classes and lists ids the schema lacks last", () => {
		const legend = classLegend(nativeResponse({
			annotations: [
				nativeRecord("x", "other", { type: "point", x: 1, y: 1 }),
				nativeRecord("y", "calc", { type: "point", x: 1, y: 1 }),
			],
		}));
		expect(legend.map((entry) => [entry.id, entry.name, entry.color])).toEqual([
			["calc", "Calcification", "#29a3a3"],
			["other", "other", FALLBACK_ANNOTATION_COLOR],
		]);
	});
});

describe("layerEntries", () => {
	it("drops a layer color that is not #RRGGBB", () => {
		const layers = [nativeLayer("L-user", "Reader", "url(https://example.invalid/x)")];
		expect(layerEntries(nativeResponse({ layers }))[0].color).toBeNull();
	});

	it("lists every session layer with the file's records in it", () => {
		expect(layerEntries(nativeResponse())).toEqual([
			{ id: "L-user", name: "Reader", color: null, readonly: false, count: 6 },
			{ id: "L-model", name: "Model v2", color: "#7b61ff", readonly: true, count: 1 },
			{ id: "L-empty", name: "Review", color: null, readonly: false, count: 0 },
		]);
	});
});

describe("labels", () => {
	it("names each target level", () => {
		expect(targetLevel({ patient: "P" })).toBe("patient");
		expect(targetLevel({ study: "1" })).toBe("study");
		expect(targetLevel({ series: "1" })).toBe("series");
		expect(targetLevel({ file: "k" })).toBe("file");
		expect(targetLevel({ frame: "k", index: 0 })).toBe("frame");
		expect(targetLevel({ folder: "a/b", root: "r" })).toBe("folder");
	});

	it("writes values in words", () => {
		const { fields } = nativeResponse().schema;
		expect(formatLabelValue(fields[0], "c")).toBe("Heterogeneously dense");
		expect(formatLabelValue({ ...fields[0], type: "multi_category", options: fields[0].type === "category" ? fields[0].options : [] }, ["a", "c", "z"]))
			.toBe("Fatty, Heterogeneously dense, z");
		expect(formatLabelValue(fields[1], true)).toBe("Yes");
		expect(formatLabelValue(fields[2], 4.5)).toBe("4.5 mm");
		expect(formatLabelValue(undefined, "free text")).toBe("free text");
	});

	it("groups labels broadest first and keeps only the shown frame's frame labels", () => {
		const groups = labelGroups(nativeResponse(), 0);
		expect(groups.map((group) => group.title)).toEqual(["Study", "File", "Frame"]);
		expect(groups[0].labels).toEqual([{ id: "l-density", field: "Density", value: "Heterogeneously dense", score: null, layer: "L-user" }]);
		expect(groups[1].labels.map((label) => [label.field, label.value, label.score])).toEqual([["Artifact", "No", null], ["Thickness", "4.5 mm", 0.6]]);
		expect(groups[2].labels.map((label) => label.id)).toEqual(["l-frame0"]);
	});

	it("keeps every frame label without a frame and drops hidden layers", () => {
		expect(labelGroups(nativeResponse()).find((group) => group.level === "frame")!.labels).toHaveLength(2);
		const file = labelGroups(nativeResponse(), 0, new Set(["L-model"])).find((group) => group.level === "file")!;
		expect(file.labels.map((label) => label.id)).toEqual(["l-artifact"]);
	});
});

describe("nativeView", () => {
	// The fixture's a-rect (10, 20)-(40, 60) is EMBED ROI 0: [20, 10, 60, 40].
	const embedRect = { index: 0, ymin: 20, xmin: 10, ymax: 60, xmax: 40 };

	it("draws a record for the EMBED rectangle it matches and marks that rectangle as drawn", () => {
		const view = nativeView(nativeResponse(), { frame: 0 }, [embedRect]);

		expect(view.shapes.map((shape) => shape.id)).toContain("a-rect");
		expect([...view.embedDrawn]).toEqual([0]);
	});

	it("matches a fractional rectangle as the EMBED view rounds it, outward", () => {
		const response = nativeResponse({ annotations: [
			nativeRecord("f", "mass", { type: "rect", x0: 10.4, y0: 20.6, x1: 39.2, y1: 59.1 }, { embed_index: 0 }),
		] });

		expect([...nativeView(response, { frame: 0 }, [embedRect]).embedDrawn]).toEqual([0]);
	});

	it("leaves the EMBED overlay a rectangle the native copy is behind on", () => {
		const moved = { ...embedRect, xmin: 12, xmax: 42 };
		for (const rects of [[moved], [], [{ ...embedRect, index: 1 }]]) {
			const view = nativeView(nativeResponse(), { frame: 0 }, rects);
			expect(view.shapes.map((shape) => shape.id)).not.toContain("a-rect");
			expect(view.embedDrawn.size).toBe(0);
			expect(view.embedHidden.size).toBe(0);
		}
	});

	it("marks a hidden record's rectangle hidden, not drawn", () => {
		const view = nativeView(nativeResponse(), { frame: 0, hiddenClasses: new Set(["mass"]) }, [embedRect]);

		expect(view.shapes.map((shape) => shape.id)).not.toContain("a-rect");
		expect(view.embedDrawn.size).toBe(0);
		expect([...view.embedHidden]).toEqual([0]);
	});

	it("draws the records the EMBED view does not show first, then those that stand for its rectangles", () => {
		const view = nativeView(nativeResponse(), { frame: 0 }, [embedRect]);

		expect(view.shapes.map((shape) => shape.id)).toEqual(["a-ellipse", "a-point", "a-polyline", "a-line", "a-rect"]);
	});

	it("draws records the EMBED view does not show whatever its rectangles", () => {
		const view = nativeView(nativeResponse(), { frame: 0 }, []);

		expect(view.shapes.map((shape) => shape.id)).toEqual(["a-ellipse", "a-point", "a-polyline", "a-line"]);
	});
});

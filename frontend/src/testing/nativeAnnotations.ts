import type { Geometry, Label, LabelTarget, Layer } from "../generated/annotation-types";
import type { NativeAnnotationRecord, NativeAnnotationsResponse } from "../lib/annotation/nativeAnnotations";

const STAMP = { rev: 1, created_by: "user:owner", created_at: "2026-10-01T00:00:00.000Z", modified_by: "user:owner", modified_at: "2026-10-01T00:00:00.000Z" };
export const FILE_KEY = "sop:1.2.3.0.1";

/** A record on every frame of the fixture file, in layer L-user, unless overridden. */
export function nativeRecord(id: string, classId: string, geometry: Geometry, overrides: Partial<NativeAnnotationRecord> = {}): NativeAnnotationRecord {
	return { id, file: FILE_KEY, frames: "all", layer: "L-user", class: classId, geometry, attributes: {}, extensions: {}, embed_index: null, ...STAMP, ...overrides };
}

export function nativeLayer(id: string, name: string, color: string | null, overrides: Partial<Layer> = {}): Layer {
	return { id, name, kind: "user", exclusive_masks: false, color, readonly: false, source: {}, rev: 1, ...overrides };
}

export function nativeLabel(id: string, target: LabelTarget, field: string, value: Label["value"], overrides: Partial<Label> = {}): Label {
	return { id, target, field, value, layer: "L-user", extensions: {}, ...STAMP, ...overrides };
}

/**
 * Three classes (mass and calcification with colors, clip without), a user
 * layer without a color and a model layer with one, every vector shape type,
 * a mask, and labels on the study, the file and two frames.
 */
export function nativeResponse(overrides: Partial<NativeAnnotationsResponse> = {}): NativeAnnotationsResponse {
	return {
		schema: {
			schema_id: "demo",
			schema_version: 1,
			classes: [
				{ id: "mass", name: "Mass", color: "#e4572e", geometry: ["polygon", "ellipse", "rect", "mask"], attributes: [], deprecated: false },
				{ id: "calc", name: "Calcification", color: "#29a3a3", geometry: ["point", "polyline"], attributes: [], deprecated: false },
				{ id: "clip", name: "Clip", geometry: ["line", "point"], attributes: [], deprecated: false },
			],
			fields: [
				{ id: "density", name: "Density", applies_to: ["study"], required: false, type: "category", ordered: true, options: [
					{ id: "a", name: "Fatty", deprecated: false },
					{ id: "c", name: "Heterogeneously dense", deprecated: false },
				] },
				{ id: "artifact", name: "Artifact", applies_to: ["file", "frame"], required: false, type: "boolean" },
				{ id: "thickness", name: "Thickness", applies_to: ["file"], required: false, type: "number", integer: false, unit: "mm" },
			],
		},
		layers: [
			nativeLayer("L-user", "Reader", null),
			nativeLayer("L-model", "Model v2", "#7b61ff", { kind: "model", readonly: true }),
			nativeLayer("L-empty", "Review", null, { kind: "review" }),
		],
		annotations: [
			nativeRecord("a-rect", "mass", { type: "rect", x0: 10, y0: 20, x1: 40, y1: 60 }, { embed_index: 0 }),
			nativeRecord("a-polygon", "mass", { type: "polygon", points: [{ x: 50, y: 50 }, { x: 70, y: 50 }, { x: 60, y: 70 }] }, { frames: { set: [1] } }),
			nativeRecord("a-ellipse", "mass", { type: "ellipse", cx: 80, cy: 80, rx: 10, ry: 5, angle: 90 }, { layer: "L-model", score: 0.874 }),
			nativeRecord("a-point", "calc", { type: "point", x: 30, y: 30 }),
			nativeRecord("a-polyline", "calc", { type: "polyline", points: [{ x: 0, y: 0 }, { x: 5, y: 5 }, { x: 10, y: 0 }] }),
			nativeRecord("a-line", "clip", { type: "line", points: [{ x: 1, y: 2 }, { x: 3, y: 4 }] }),
			nativeRecord("a-mask", "mass", { type: "mask", encoding: "tiles-v1", tile: 64, depth: 1, frames: { "0": { "0,0": "AA==" } } }, { frames: { set: [0] } }),
		],
		labels: [
			nativeLabel("l-density", { study: "1.2.3" }, "density", "c"),
			nativeLabel("l-artifact", { file: FILE_KEY }, "artifact", false),
			nativeLabel("l-thick", { file: FILE_KEY }, "thickness", 4.5, { layer: "L-model", score: 0.6 }),
			nativeLabel("l-frame0", { frame: FILE_KEY, index: 0 }, "artifact", true),
			nativeLabel("l-frame1", { frame: FILE_KEY, index: 1 }, "artifact", false),
		],
		revision: 7,
		...overrides,
	};
}

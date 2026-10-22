import type {
	ClassDef,
	FieldDef,
	FrameScope,
	Geometry,
	Label,
	LabelSchema,
	LabelTarget,
	LabelValue,
	Layer,
	TargetKind,
} from "../../generated/annotation-types";
import { graphicShape, type GraphicShape } from "../viewport/graphicAnnotations";
import type { NativeAnnotationRecord, NativeAnnotationsResponse } from "./nativeAnnotations";

/**
 * The color of a shape whose layer and class name none. A fixed value rather
 * than a theme token so the legend swatch and the drawn shape always agree.
 */
export const FALLBACK_ANNOTATION_COLOR = "#ffc145";

/** An axis-aligned box in image pixel coordinates (corner origin). */
export type ImageBounds = { x0: number; y0: number; x1: number; y1: number };

/** One record ready to draw on the shown frame. */
export type NativeShape = {
	id: string;
	/** Image pixel coordinates; the viewBox is `0 0 columns rows`. */
	shape: GraphicShape;
	/** Where the class-name tag is placed from: the shape's bounding box. */
	bounds: ImageBounds;
	color: string;
	/** The class name, then the score when the record has one. */
	text: string;
	classId: string;
	layer: string;
	embedIndex: number | null;
};

type Vertex = [number, number];

function toPoints(points: readonly { x: number; y: number }[]): Vertex[] {
	return points.map(({ x, y }): Vertex => [x, y]);
}

function closeRing(points: Vertex[]): Vertex[] {
	if (points.length === 0) return points;
	const [first, last] = [points[0], points[points.length - 1]];
	return first[0] === last[0] && first[1] === last[1] ? points : [...points, first];
}

/**
 * The drawable form of a vector geometry, in image pixel coordinates, or
 * `null` for a mask or a geometry type this viewer does not know, which it
 * does not draw. A polygon and a rectangle are closed paths; a line and a
 * polyline are always open ones. An ellipse's `angle` (degrees) becomes its
 * rotation.
 */
export function geometryShape(geometry: Geometry): GraphicShape | null {
	switch (geometry.type) {
		case "point":
			return { kind: "point", x: geometry.x, y: geometry.y };
		case "line":
		case "polyline":
			// Open even when the last point repeats the first: the model's polygon is the closed form.
			return { kind: "path", d: geometry.points.map(({ x, y }, index) => `${index === 0 ? "M" : "L"}${x} ${y}`).join(""), closed: false };
		case "polygon":
			return graphicShape({ graphic_type: "polyline", points: closeRing(toPoints(geometry.points)) });
		case "rect": {
			const { x0, y0, x1, y1 } = geometry;
			return graphicShape({ graphic_type: "polyline", points: [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]] });
		}
		case "ellipse":
			return { kind: "ellipse", cx: geometry.cx, cy: geometry.cy, rx: geometry.rx, ry: geometry.ry, rotation: geometry.angle };
		case "mask":
			return null;
		default:
			// A geometry type newer than this viewer is counted but not drawn.
			return null;
	}
}

/** The bounding box of a vector geometry, or `null` for a mask. */
export function geometryBounds(geometry: Geometry): ImageBounds | null {
	const ofPoints = (points: readonly { x: number; y: number }[]): ImageBounds => ({
		x0: Math.min(...points.map((point) => point.x)),
		y0: Math.min(...points.map((point) => point.y)),
		x1: Math.max(...points.map((point) => point.x)),
		y1: Math.max(...points.map((point) => point.y)),
	});
	switch (geometry.type) {
		case "point":
			return { x0: geometry.x, y0: geometry.y, x1: geometry.x, y1: geometry.y };
		case "line":
		case "polyline":
		case "polygon":
			return ofPoints(geometry.points);
		case "rect":
			return { x0: geometry.x0, y0: geometry.y0, x1: geometry.x1, y1: geometry.y1 };
		case "ellipse": {
			const radians = (geometry.angle * Math.PI) / 180;
			const [cos, sin] = [Math.cos(radians), Math.sin(radians)];
			const halfWidth = Math.hypot(geometry.rx * cos, geometry.ry * sin);
			const halfHeight = Math.hypot(geometry.rx * sin, geometry.ry * cos);
			return {
				x0: geometry.cx - halfWidth,
				y0: geometry.cy - halfHeight,
				x1: geometry.cx + halfWidth,
				y1: geometry.cy + halfHeight,
			};
		}
		case "mask":
		default:
			return null;
	}
}

/** Whether a record's frame scope includes a zero-based frame index. */
export function appliesToFrame(frames: FrameScope, frame: number): boolean {
	return frames === "all" || frames.set.includes(frame);
}

const HEX_COLOR = /^#[0-9a-f]{6}$/i;

/**
 * A stored color when it is `#RRGGBB`, else `null`. Colors reach CSS custom
 * properties, so anything else (a `url(...)` among them) is never passed on.
 */
export function safeColor(color: string | null | undefined): string | null {
	return typeof color === "string" && HEX_COLOR.test(color) ? color : null;
}

/** A shape's color: its layer's, else its class's, else the fixed fallback. */
export function annotationColor(layer: Layer | undefined, classDef: ClassDef | undefined): string {
	return safeColor(layer?.color) ?? safeColor(classDef?.color) ?? FALLBACK_ANNOTATION_COLOR;
}

/** A class's own color, as the legend shows it: its color, else the fallback. */
export function classColor(classDef: ClassDef | undefined): string {
	return safeColor(classDef?.color) ?? FALLBACK_ANNOTATION_COLOR;
}

/** A class's name, or its id when the schema does not hold it. */
export function className(schema: LabelSchema, classId: string): string {
	return schema.classes.find((entry) => entry.id === classId)?.name ?? classId;
}

/** A score as it is shown after a name: two decimals. */
export function formatScore(score: number): string {
	return score.toFixed(2);
}

/** The tag a shape is named by: its class name, then its score when present. */
export function annotationText(schema: LabelSchema, record: Pick<NativeAnnotationRecord, "class" | "score">): string {
	const name = className(schema, record.class);
	return record.score === undefined || record.score === null ? name : `${name} ${formatScore(record.score)}`;
}

export type NativeDisplayFilter = {
	/** The zero-based frame shown. */
	frame: number;
	/** Layer ids the viewer has hidden. */
	hiddenLayers?: ReadonlySet<string>;
	/** Class ids the viewer has hidden. */
	hiddenClasses?: ReadonlySet<string>;
};

/**
 * The records to draw on the shown frame, in response order, without hidden
 * layers or classes and without records that are not drawn.
 */
export function visibleShapes(
	response: Pick<NativeAnnotationsResponse, "schema" | "layers" | "annotations">,
	{ frame, hiddenLayers, hiddenClasses }: NativeDisplayFilter,
): NativeShape[] {
	const layers = new Map(response.layers.map((layer) => [layer.id, layer]));
	const classes = new Map(response.schema.classes.map((entry) => [entry.id, entry]));
	const shapes: NativeShape[] = [];
	for (const record of response.annotations) {
		if (hiddenLayers?.has(record.layer) || hiddenClasses?.has(record.class)) continue;
		if (!appliesToFrame(record.frames, frame)) continue;
		const shape = geometryShape(record.geometry);
		const bounds = geometryBounds(record.geometry);
		if (!shape || !bounds) continue;
		shapes.push({
			id: record.id,
			shape,
			bounds,
			color: annotationColor(layers.get(record.layer), classes.get(record.class)),
			text: annotationText(response.schema, record),
			classId: record.class,
			layer: record.layer,
			embedIndex: record.embed_index,
		});
	}
	return shapes;
}

/** A rectangle the EMBED view shows on the current frame, at its index in that view. */
export type EmbedRect = { index: number; ymin: number; xmin: number; ymax: number; xmax: number };

export type NativeView = {
	/** The records to draw, as `visibleShapes` gives them, less those out of date (below). */
	shapes: NativeShape[];
	/**
	 * EMBED indices whose rectangle a native record stands for on this frame,
	 * hidden or not: the EMBED overlay does not outline or number them.
	 */
	embedDrawn: ReadonlySet<number>;
};

/** A rectangle as the EMBED view writes it: `[y0, x0, y1, x1]`, rounded outward and not below 0. */
function embedBox(geometry: Geometry): [number, number, number, number] | null {
	if (geometry.type !== "rect") return null;
	return [
		Math.max(0, Math.floor(geometry.y0)),
		Math.max(0, Math.floor(geometry.x0)),
		Math.max(0, Math.ceil(geometry.y1)),
		Math.max(0, Math.ceil(geometry.x1)),
	];
}

/**
 * What the native overlay draws beside the EMBED rectangles on the same
 * frame. The EMBED view is the live copy of the rectangles it shows: a
 * record with an `embed_index` stands for the rectangle at that index only
 * while that rectangle is on this frame with the box the record reads as.
 * Otherwise the native copy is behind an edit (a rectangle being dragged,
 * one saved and not read back yet, a deleted one shifting the indices), and
 * the record is left out, so the EMBED overlay draws that rectangle as it
 * always has and nothing is drawn twice.
 */
export function nativeView(
	response: Pick<NativeAnnotationsResponse, "schema" | "layers" | "annotations">,
	{ frame, hiddenLayers, hiddenClasses }: NativeDisplayFilter,
	embedRects: readonly EmbedRect[],
): NativeView {
	const records = new Map(response.annotations.map((record) => [record.id, record]));
	const rects = new Map(embedRects.map((rect) => [rect.index, rect]));
	const embedDrawn = new Set<number>();
	const shapes: NativeShape[] = [];
	for (const shape of visibleShapes(response, { frame })) {
		if (shape.embedIndex !== null) {
			const box = embedBox(records.get(shape.id)!.geometry);
			const rect = rects.get(shape.embedIndex);
			const current = box !== null && rect !== undefined
				&& rect.ymin === box[0] && rect.xmin === box[1] && rect.ymax === box[2] && rect.xmax === box[3];
			if (!current) continue;
			embedDrawn.add(shape.embedIndex);
		}
		if (hiddenLayers?.has(shape.layer) || hiddenClasses?.has(shape.classId)) continue;
		shapes.push(shape);
	}
	return { shapes, embedDrawn };
}

export type ClassLegendEntry = {
	id: string;
	name: string;
	color: string;
	/** Records of this class in the file, on every frame and layer, masks included. */
	count: number;
	/** How many of `count` are not drawn: masks, and geometry types this viewer does not know. */
	notDrawn: number;
};

/**
 * One entry for each class the file's records use: the schema's classes in
 * schema order, then ids the schema does not hold in first-seen order.
 */
export function classLegend(response: Pick<NativeAnnotationsResponse, "schema" | "annotations">): ClassLegendEntry[] {
	const counts = new Map<string, { count: number; notDrawn: number }>();
	for (const record of response.annotations) {
		const entry = counts.get(record.class) ?? { count: 0, notDrawn: 0 };
		entry.count += 1;
		if (geometryShape(record.geometry) === null) entry.notDrawn += 1;
		counts.set(record.class, entry);
	}
	const known = response.schema.classes.filter((entry) => counts.has(entry.id));
	const knownIds = new Set(known.map((entry) => entry.id));
	return [
		...known.map((entry) => ({ id: entry.id, name: entry.name, color: classColor(entry), ...counts.get(entry.id)! })),
		...[...counts.entries()]
			.filter(([id]) => !knownIds.has(id))
			.map(([id, entry]) => ({ id, name: id, color: FALLBACK_ANNOTATION_COLOR, ...entry })),
	];
}

export type LayerEntry = {
	id: string;
	name: string;
	/** The layer's own color, or `null` when its shapes take their class colors. */
	color: string | null;
	readonly: boolean;
	/** Records of this file in the layer. */
	count: number;
};

/** Every session layer in response order, with how many of the file's records it holds. */
export function layerEntries(response: Pick<NativeAnnotationsResponse, "layers" | "annotations">): LayerEntry[] {
	const counts = new Map<string, number>();
	for (const record of response.annotations) counts.set(record.layer, (counts.get(record.layer) ?? 0) + 1);
	return response.layers.map((layer) => ({
		id: layer.id,
		name: layer.name,
		color: safeColor(layer.color),
		readonly: layer.readonly,
		count: counts.get(layer.id) ?? 0,
	}));
}

/** The levels labels are grouped by, broadest first. */
export const TARGET_LEVELS: readonly TargetKind[] = ["patient", "study", "series", "file", "frame", "folder"];

const LEVEL_TITLES: Record<TargetKind, string> = {
	patient: "Patient",
	study: "Study",
	series: "Series",
	file: "File",
	frame: "Frame",
	folder: "Folder",
};

/** The kind of thing a label is set on. */
export function targetLevel(target: LabelTarget): TargetKind {
	if ("patient" in target) return "patient";
	if ("study" in target) return "study";
	if ("series" in target) return "series";
	if ("frame" in target) return "frame";
	if ("file" in target) return "file";
	return "folder";
}

/** A label value in words: option names for categories, Yes or No, a number with its unit. */
export function formatLabelValue(field: FieldDef | undefined, value: LabelValue): string {
	const optionName = (id: string) => (
		field && (field.type === "category" || field.type === "multi_category")
			? field.options.find((option) => option.id === id)?.name ?? id
			: id
	);
	if (typeof value === "boolean") return value ? "Yes" : "No";
	if (Array.isArray(value)) return value.map(optionName).join(", ");
	if (typeof value === "number") return field?.type === "number" && field.unit ? `${value} ${field.unit}` : String(value);
	return optionName(value);
}

export type LabelEntry = {
	id: string;
	/** The field's name, or its id when the schema does not hold it. */
	field: string;
	value: string;
	score: number | null;
	layer: string;
};

export type LabelGroup = { level: TargetKind; title: string; labels: LabelEntry[] };

/**
 * Labels grouped by target level, broadest first, leaving out empty levels.
 * With `frame` given, a frame label is kept only when it is on that frame.
 */
export function labelGroups(
	response: Pick<NativeAnnotationsResponse, "schema" | "labels">,
	frame?: number,
	hiddenLayers?: ReadonlySet<string>,
): LabelGroup[] {
	const fields = new Map(response.schema.fields.map((field) => [field.id, field]));
	const byLevel = new Map<TargetKind, LabelEntry[]>();
	for (const label of response.labels) {
		if (hiddenLayers?.has(label.layer)) continue;
		if (frame !== undefined && "frame" in label.target && label.target.index !== frame) continue;
		const level = targetLevel(label.target);
		const field = fields.get(label.field);
		const entries = byLevel.get(level) ?? [];
		entries.push(labelEntry(label, field));
		byLevel.set(level, entries);
	}
	return TARGET_LEVELS
		.filter((level) => byLevel.has(level))
		.map((level) => ({ level, title: LEVEL_TITLES[level], labels: byLevel.get(level)! }));
}

function labelEntry(label: Label, field: FieldDef | undefined): LabelEntry {
	return {
		id: label.id,
		field: field?.name ?? label.field,
		value: formatLabelValue(field, label.value),
		score: label.score ?? null,
		layer: label.layer,
	};
}

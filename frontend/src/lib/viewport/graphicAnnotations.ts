import type { GraphicLayerSummary, GraphicObjectSummary } from "../../api";

/**
 * One softcopy presentation state's annotations on the displayed frames.
 * `highlightedItem` is the Graphic Annotation Sequence item being stepped to:
 * it is drawn emphasized and the other items dimmed. `null` draws every item
 * alike.
 */
export type GraphicAnnotationSelection = {
	stateFileIndex: number;
	highlightedItem: number | null;
};

export type Point = readonly [number, number];

/** What an SVG element needs to draw one graphic object in image pixels. */
export type GraphicShape =
	| { kind: "path"; d: string; closed: boolean }
	| { kind: "ellipse"; cx: number; cy: number; rx: number; ry: number; rotation: number }
	| { kind: "point"; x: number; y: number };

function samePoint(a: Point, b: Point): boolean {
	return a[0] === b[0] && a[1] === b[1];
}

/** A polyline is closed when its last point repeats its first (PS3.3 C.10.5). */
function isClosed(points: readonly Point[]): boolean {
	return points.length > 2 && samePoint(points[0], points[points.length - 1]);
}

function polylinePath(points: readonly Point[]): string {
	const closed = isClosed(points);
	const vertices = closed ? points.slice(0, -1) : points;
	const segments = vertices.map(([x, y], index) => `${index === 0 ? "M" : "L"}${x} ${y}`).join("");
	return closed ? `${segments}Z` : segments;
}

/**
 * A smooth curve through every point: a uniform Catmull-Rom spline as cubic
 * Béziers. The standard leaves the interpolation to the viewer and only
 * requires the curve to pass through the points. A closed curve wraps its
 * tangents around; an open one ends with its end segments' own direction.
 */
function interpolatedPath(points: readonly Point[]): string {
	const closed = isClosed(points);
	const vertices = closed ? points.slice(0, -1) : points;
	const count = vertices.length;
	if (count < 3) return polylinePath(points);
	const at = (index: number): Point => (
		closed ? vertices[((index % count) + count) % count] : vertices[Math.min(count - 1, Math.max(0, index))]
	);
	let d = `M${vertices[0][0]} ${vertices[0][1]}`;
	for (let index = 0; index < (closed ? count : count - 1); index += 1) {
		const [before, from, to, after] = [at(index - 1), at(index), at(index + 1), at(index + 2)];
		const c1 = [from[0] + (to[0] - before[0]) / 6, from[1] + (to[1] - before[1]) / 6];
		const c2 = [to[0] - (after[0] - from[0]) / 6, to[1] - (after[1] - from[1]) / 6];
		d += `C${c1[0]} ${c1[1]} ${c2[0]} ${c2[1]} ${to[0]} ${to[1]}`;
	}
	return closed ? `${d}Z` : d;
}

function distance(a: Point, b: Point): number {
	return Math.hypot(b[0] - a[0], b[1] - a[1]);
}

/** The drawable form of a graphic object, in image pixel coordinates. */
export function graphicShape({ graphic_type, points }: Pick<GraphicObjectSummary, "graphic_type" | "points">): GraphicShape {
	switch (graphic_type) {
		case "point":
			return { kind: "point", x: points[0][0], y: points[0][1] };
		case "polyline":
			return { kind: "path", d: polylinePath(points), closed: isClosed(points) };
		case "interpolated":
			return { kind: "path", d: interpolatedPath(points), closed: isClosed(points) };
		case "circle": {
			const radius = distance(points[0], points[1]);
			return { kind: "ellipse", cx: points[0][0], cy: points[0][1], rx: radius, ry: radius, rotation: 0 };
		}
		case "ellipse": {
			// Major-axis endpoints, then minor-axis endpoints.
			const [majorStart, majorEnd, minorStart, minorEnd] = points;
			return {
				kind: "ellipse",
				cx: (majorStart[0] + majorEnd[0]) / 2,
				cy: (majorStart[1] + majorEnd[1]) / 2,
				rx: distance(majorStart, majorEnd) / 2,
				ry: distance(minorStart, minorEnd) / 2,
				rotation: (Math.atan2(majorEnd[1] - majorStart[1], majorEnd[0] - majorStart[0]) * 180) / Math.PI,
			};
		}
	}
}

/** A layer's recommended color as CSS, or null to use the theme's annotation color. */
export function layerColor(layers: readonly GraphicLayerSummary[], name: string): string | null {
	const color = layers.find((layer) => layer.name === name)?.color;
	return color ? `rgb(${color[0]}, ${color[1]}, ${color[2]})` : null;
}

/** How an object is drawn while stepping through a state's items. */
export function itemEmphasis(item: number, highlightedItem: number | null): "normal" | "highlighted" | "dimmed" {
	if (highlightedItem === null) return "normal";
	return item === highlightedItem ? "highlighted" : "dimmed";
}

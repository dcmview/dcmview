<script lang="ts">
	import type { ImageDisplayGeometry } from "../imageGeometry";
	import type { ImageOrientation } from "../viewerTools";
	import { imageToViewportPoint, type ViewTransform } from "../viewport/viewTransform";
	import type { NativeShape } from "./display";

	let {
		shapes,
		transform,
		orientation,
		geometry,
	}: {
		/** The records drawn on the shown frame, from `visibleShapes`. */
		shapes: readonly NativeShape[];
		transform: ViewTransform;
		orientation: ImageOrientation;
		geometry: ImageDisplayGeometry;
	} = $props();

	// Screen pixels; the tag is 11px tall.
	const POINT_ARM = 5;
	const ABOVE = 4;
	const INSIDE = 12;
	const POINT_GAP = 8;

	const toViewport = (x: number, y: number) => imageToViewportPoint({ x, y }, transform, orientation, geometry);

	const points = $derived(shapes.flatMap((shape) => (
		shape.shape.kind === "point" ? [{ id: shape.id, color: shape.color, ...toViewport(shape.shape.x, shape.shape.y) }] : []
	)));

	/**
	 * A point's tag sits to the right of its mark. Any other shape's tag sits
	 * just above its bounding box's on-screen top-left corner, or just inside
	 * it when the viewport top leaves no room.
	 */
	const tags = $derived(shapes.map((shape) => {
		if (shape.shape.kind === "point") {
			const at = toViewport(shape.shape.x, shape.shape.y);
			return { id: shape.id, text: shape.text, color: shape.color, x: at.x + POINT_GAP, y: at.y + 4 };
		}
		const { x0, y0, x1, y1 } = shape.bounds;
		const corners = [toViewport(x0, y0), toViewport(x1, y0), toViewport(x0, y1), toViewport(x1, y1)];
		const left = Math.min(...corners.map((corner) => corner.x));
		const top = Math.min(...corners.map((corner) => corner.y));
		return { id: shape.id, text: shape.text, color: shape.color, x: left + 2, y: top - ABOVE < INSIDE ? top + INSIDE : top - ABOVE };
	}));
</script>

<!-- Drawn in unscaled viewport pixels so point marks and tags keep their size and stay upright at any zoom or orientation. -->
<svg class="native-annotation-labels" aria-hidden="true">
	{#each points as point (point.id)}
		<path
			class="point"
			style:--annotation-color={point.color}
			d={`M${point.x - POINT_ARM} ${point.y}H${point.x + POINT_ARM}M${point.x} ${point.y - POINT_ARM}V${point.y + POINT_ARM}`}
		></path>
	{/each}
	{#each tags as tag (tag.id)}
		<text class="tag" style:--annotation-color={tag.color} x={tag.x} y={tag.y}>{tag.text}</text>
	{/each}
</svg>

<style>
	.native-annotation-labels {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: hidden;
		pointer-events: none;
	}

	.point {
		fill: none;
		stroke: var(--annotation-color);
		stroke-width: 1.4;
	}

	.tag {
		fill: var(--annotation-color);
		stroke: var(--viewport);
		stroke-width: 2.4;
		paint-order: stroke;
		font-size: 11px;
		font-family: var(--font-ui);
	}
</style>

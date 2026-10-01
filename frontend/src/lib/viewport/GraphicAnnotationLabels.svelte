<script lang="ts">
	import type { GraphicAnnotationsResponse } from "../../api";
	import type { ImageDisplayGeometry } from "../imageGeometry";
	import type { ImageOrientation } from "../viewerTools";
	import { itemEmphasis, layerColor } from "./graphicAnnotations";
	import { imageToViewportPoint, type ViewTransform } from "./viewTransform";

	let {
		annotations,
		highlightedItem,
		transform,
		orientation,
		geometry,
	}: {
		annotations: GraphicAnnotationsResponse;
		/** The annotation item stepped to; the others are dimmed. */
		highlightedItem: number | null;
		transform: ViewTransform;
		orientation: ImageOrientation;
		geometry: ImageDisplayGeometry;
	} = $props();

	// Screen pixels.
	const LINE_HEIGHT = 14;
	const FIRST_BASELINE = 11;
	const POINT_ARM = 5;
	const ANCHOR_GAP = 8;

	const toViewport = (point: readonly number[]) => imageToViewportPoint({ x: point[0], y: point[1] }, transform, orientation, geometry);

	const points = $derived(annotations.graphics
		.filter((graphic) => graphic.graphic_type === "point")
		.map((graphic) => ({
			...toViewport(graphic.points[0]),
			color: layerColor(annotations.layers, graphic.layer),
			emphasis: itemEmphasis(graphic.item, highlightedItem),
		})));

	/**
	 * Text is upright at a fixed size. In a bounding box it starts at the box's
	 * on-screen top edge, justified between its on-screen sides; without one it
	 * sits beside its anchor point. A visible anchor is joined to the text by a
	 * line to the nearest point of the box.
	 */
	const texts = $derived(annotations.texts.map((text) => {
		const anchor = text.anchor ? toViewport(text.anchor) : null;
		const lines = text.text.split("\n");
		let x: number;
		let top: number;
		let align: "start" | "middle" | "end" = "start";
		let leader: { x: number; y: number } | null = null;
		if (text.bounding_box) {
			const [x0, y0, x1, y1] = text.bounding_box;
			const corners = [[x0, y0], [x1, y0], [x0, y1], [x1, y1]].map(toViewport);
			const left = Math.min(...corners.map((corner) => corner.x));
			const right = Math.max(...corners.map((corner) => corner.x));
			top = Math.min(...corners.map((corner) => corner.y));
			const bottom = Math.max(...corners.map((corner) => corner.y));
			align = text.justification === "center" ? "middle" : text.justification === "right" ? "end" : "start";
			x = align === "middle" ? (left + right) / 2 : align === "end" ? right : left;
			if (anchor) leader = { x: Math.min(right, Math.max(left, anchor.x)), y: Math.min(bottom, Math.max(top, anchor.y)) };
		} else {
			// The server sends text with a box or an anchor.
			x = (anchor?.x ?? 0) + ANCHOR_GAP;
			top = (anchor?.y ?? 0) - (lines.length * LINE_HEIGHT) / 2;
			if (anchor) leader = { x, y: anchor.y };
		}
		return {
			lines,
			x,
			top,
			align,
			leader: anchor && leader && text.anchor_visible ? { from: anchor, to: leader } : null,
			color: layerColor(annotations.layers, text.layer),
			emphasis: itemEmphasis(text.item, highlightedItem),
		};
	}));
</script>

<!-- Drawn in unscaled viewport pixels so text and point marks keep their size and stay upright at any zoom or orientation. -->
<svg class="graphic-annotation-labels" aria-hidden="true">
	{#each points as point, index (index)}
		<path
			class="point {point.emphasis}"
			style:--annotation-color={point.color}
			d={`M${point.x - POINT_ARM} ${point.y}H${point.x + POINT_ARM}M${point.x} ${point.y - POINT_ARM}V${point.y + POINT_ARM}`}
		></path>
	{/each}
	{#each texts as text, index (index)}
		<g class={text.emphasis} style:--annotation-color={text.color}>
			{#if text.leader}
				<line class="leader" x1={text.leader.from.x} y1={text.leader.from.y} x2={text.leader.to.x} y2={text.leader.to.y}></line>
			{/if}
			<text class="label" text-anchor={text.align}>
				{#each text.lines as line, lineIndex (lineIndex)}
					<tspan x={text.x} y={text.top + FIRST_BASELINE + lineIndex * LINE_HEIGHT}>{line}</tspan>
				{/each}
			</text>
		</g>
	{/each}
</svg>

<style>
	.graphic-annotation-labels {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: hidden;
		pointer-events: none;
	}

	.point,
	.leader {
		fill: none;
		stroke: var(--annotation-color, var(--graphic-annotation));
		stroke-width: 1.4;
	}

	.label {
		fill: var(--annotation-color, var(--graphic-annotation));
		stroke: var(--viewport);
		stroke-width: 2.4;
		paint-order: stroke;
		font-size: 12px;
		font-family: var(--font-ui);
	}

	.highlighted.point,
	.highlighted .leader {
		stroke-width: 2.4;
	}

	.highlighted .label {
		font-weight: 600;
	}

	.dimmed {
		opacity: 0.3;
	}
</style>

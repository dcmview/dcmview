<script lang="ts">
	import type { GraphicAnnotationsResponse } from "../../api";
	import { graphicShape, itemEmphasis, layerColor } from "./graphicAnnotations";

	let {
		annotations,
		highlightedItem,
		rows,
		columns,
		scale,
	}: {
		annotations: GraphicAnnotationsResponse;
		/** The annotation item stepped to; the others are dimmed. */
		highlightedItem: number | null;
		rows: number;
		columns: number;
		/** View zoom applied to the layer by CSS, which non-scaling strokes do not undo. */
		scale: number;
	} = $props();

	// Points keep a fixed on-screen size, so GraphicAnnotationLabels draws them.
	const shapes = $derived(annotations.graphics
		.filter((graphic) => graphic.graphic_type !== "point")
		.map((graphic) => ({
			shape: graphicShape(graphic),
			filled: graphic.filled,
			color: layerColor(annotations.layers, graphic.layer),
			emphasis: itemEmphasis(graphic.item, highlightedItem),
		})));
</script>

<!--
	A presentation state's graphic objects, in image pixel coordinates inside
	the transformed image layer: PS3.3 C.10.5 PIXEL units put 0\0 at the
	top-left corner of the top-left pixel, which is the viewBox origin. Stroke
	widths are divided by the view scale so they stay constant on screen.
-->
<svg
	class="graphic-annotations"
	style:--annotation-px={`${1 / scale}px`}
	viewBox={`0 0 ${columns} ${rows}`}
	preserveAspectRatio="none"
	aria-hidden="true"
>
	{#each shapes as { shape, filled, color, emphasis }, index (index)}
		{#if shape.kind === "path"}
			<path
				class="graphic {emphasis}"
				class:filled={filled && shape.closed}
				style:--annotation-color={color}
				d={shape.d}
			></path>
		{:else if shape.kind === "ellipse"}
			<ellipse
				class="graphic {emphasis}"
				class:filled
				style:--annotation-color={color}
				cx={shape.cx}
				cy={shape.cy}
				rx={shape.rx}
				ry={shape.ry}
				transform={`rotate(${shape.rotation} ${shape.cx} ${shape.cy})`}
			></ellipse>
		{/if}
	{/each}
</svg>

<style>
	.graphic-annotations {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: visible;
		pointer-events: none;
	}

	.graphic {
		fill: none;
		stroke: var(--annotation-color, var(--graphic-annotation));
		stroke-width: calc(1.4 * var(--annotation-px));
		stroke-linejoin: round;
		vector-effect: non-scaling-stroke;
	}

	/* A filled graphic stays translucent so the image under it can still be read. */
	.graphic.filled {
		fill: var(--annotation-color, var(--graphic-annotation));
		fill-opacity: 0.35;
	}

	.graphic.highlighted {
		stroke-width: calc(2.4 * var(--annotation-px));
	}

	.graphic.dimmed {
		opacity: 0.3;
	}
</style>

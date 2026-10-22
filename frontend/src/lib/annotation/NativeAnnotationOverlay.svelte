<script lang="ts">
	import type { NativeShape } from "./display";

	let {
		shapes,
		rows,
		columns,
		scale,
	}: {
		/** The records to draw on the shown frame, from `visibleShapes`. */
		shapes: readonly NativeShape[];
		rows: number;
		columns: number;
		/** View zoom applied to the layer by CSS, which non-scaling strokes do not undo. */
		scale: number;
	} = $props();

	// Points keep a fixed on-screen size, so NativeAnnotationLabels draws them.
	const drawn = $derived(shapes.filter((shape) => shape.shape.kind !== "point"));
</script>

<!--
	Native annotation records in image pixel coordinates inside the
	transformed image layer: the model's corner origin is the viewBox origin.
	Stroke widths are divided by the view scale so they stay constant on screen.
-->
<svg
	class="native-annotations"
	style:--annotation-px={`${1 / scale}px`}
	viewBox={`0 0 ${columns} ${rows}`}
	preserveAspectRatio="none"
	aria-hidden="true"
>
	{#each drawn as { id, shape, color } (id)}
		{#if shape.kind === "path"}
			<path class="native-shape" data-id={id} style:--annotation-color={color} d={shape.d}></path>
		{:else if shape.kind === "ellipse"}
			<ellipse
				class="native-shape"
				data-id={id}
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
	.native-annotations {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: visible;
		pointer-events: none;
	}

	.native-shape {
		fill: none;
		stroke: var(--annotation-color);
		stroke-width: calc(1.4 * var(--annotation-px));
		stroke-linejoin: round;
		vector-effect: non-scaling-stroke;
	}
</style>

<script lang="ts">
	import type { RoiCoord } from "../annotationGeometry";
	import { roiHandles, type VisibleRoi } from "./roiEditing";

	let {
		rois,
		selectedIndex,
		draft,
		rows,
		columns,
		scale,
		pixelAspectRatio,
	}: {
		rois: readonly VisibleRoi[];
		selectedIndex: number | null;
		/** The rectangle being drawn, as [ymin, xmin, ymax, xmax]. */
		draft: RoiCoord | null;
		rows: number;
		columns: number;
		/** View zoom applied to the layer by CSS, which non-scaling strokes do not undo. */
		scale: number;
		pixelAspectRatio: number;
	} = $props();

	const HANDLE_RADIUS = 4;
</script>

<!--
	Drawn in image pixel coordinates inside the transformed image layer; RoiLabels
	draws the labels unscaled. Stroke and handle sizes are divided by the view
	scale so they stay constant on screen; non-scaling-stroke still undoes the
	viewBox's pixel-aspect stretch.
-->
<svg
	class="roi-overlay"
	style:--roi-px={`${1 / scale}px`}
	viewBox={`0 0 ${columns} ${rows}`}
	preserveAspectRatio="none"
	aria-hidden="true"
>
	{#each rois as roi (roi.index)}
		<g class:selected={selectedIndex === roi.index}>
			<rect
				class="roi-rect"
				x={Math.min(roi.xmin, roi.xmax)}
				y={Math.min(roi.ymin, roi.ymax)}
				width={Math.max(1, Math.abs(roi.xmax - roi.xmin))}
				height={Math.max(1, Math.abs(roi.ymax - roi.ymin))}
			></rect>
			{#if selectedIndex === roi.index}
				{#each roiHandles(roi) as handle}
					<ellipse
						class="roi-handle"
						cx={handle.x}
						cy={handle.y}
						rx={HANDLE_RADIUS / scale}
						ry={HANDLE_RADIUS / (scale * pixelAspectRatio)}
					></ellipse>
				{/each}
			{/if}
		</g>
	{/each}
	{#if draft}
		<rect
			class="roi-rect draft"
			x={draft[1]}
			y={draft[0]}
			width={Math.max(1, draft[3] - draft[1])}
			height={Math.max(1, draft[2] - draft[0])}
		></rect>
	{/if}
</svg>

<style>
	.roi-overlay {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		pointer-events: none;
	}

	/* Outlines only; saved, selected and draft differ by line style as well as colour. */
	.roi-rect {
		fill: none;
		stroke: var(--roi);
		stroke-width: calc(1.2 * var(--roi-px));
		vector-effect: non-scaling-stroke;
	}

	.roi-overlay g.selected .roi-rect {
		stroke: var(--roi-selected);
		stroke-width: calc(1.6 * var(--roi-px));
	}

	.roi-rect.draft {
		stroke: var(--roi-draft);
		stroke-dasharray: calc(5 * var(--roi-px)) calc(4 * var(--roi-px));
	}

	.roi-handle {
		fill: var(--viewport);
		stroke: var(--roi-selected);
		stroke-width: calc(1.4 * var(--roi-px));
		vector-effect: non-scaling-stroke;
	}
</style>

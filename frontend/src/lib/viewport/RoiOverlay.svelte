<script lang="ts">
	import type { RoiCoord } from "../annotationGeometry";
	import { roiHandles, type VisibleRoi } from "./roiEditing";

	let {
		rois,
		selectedIndex,
		draft,
		rows,
		columns,
	}: {
		rois: readonly VisibleRoi[];
		selectedIndex: number | null;
		/** The rectangle being drawn, as [ymin, xmin, ymax, xmax]. */
		draft: RoiCoord | null;
		rows: number;
		columns: number;
	} = $props();
</script>

<!-- Drawn in image pixel coordinates inside the transformed image layer; RoiLabels draws the labels unscaled. -->
<svg
	class="roi-overlay"
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
					<circle class="roi-handle" cx={handle.x} cy={handle.y} r={4}></circle>
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
		stroke-width: 1.2;
		vector-effect: non-scaling-stroke;
	}

	.roi-overlay g.selected .roi-rect {
		stroke: var(--roi-selected);
		stroke-width: 1.6;
	}

	.roi-rect.draft {
		stroke: var(--roi-draft);
		stroke-dasharray: 5 4;
	}

	.roi-handle {
		fill: var(--viewport);
		stroke: var(--roi-selected);
		stroke-width: 1.4;
		vector-effect: non-scaling-stroke;
	}
</style>

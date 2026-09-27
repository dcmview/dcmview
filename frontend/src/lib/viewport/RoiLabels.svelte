<script lang="ts">
	import type { ImageDisplayGeometry } from "../imageGeometry";
	import type { ImageOrientation } from "../viewerTools";
	import type { VisibleRoi } from "./roiEditing";
	import { imageToViewportPoint, type ViewTransform } from "./viewTransform";

	let {
		rois,
		selectedIndex,
		transform,
		orientation,
		geometry,
	}: {
		rois: readonly VisibleRoi[];
		selectedIndex: number | null;
		transform: ViewTransform;
		orientation: ImageOrientation;
		geometry: ImageDisplayGeometry;
	} = $props();

	// Baseline offsets in screen pixels; the label is 11px tall.
	const ABOVE = 4;
	const INSIDE = 12;

	/** Each label sits just above its ROI's on-screen top-left corner, or just inside it when the viewport top leaves no room. */
	const labels = $derived(rois.map((roi) => {
		const corners = [
			{ x: roi.xmin, y: roi.ymin },
			{ x: roi.xmax, y: roi.ymin },
			{ x: roi.xmin, y: roi.ymax },
			{ x: roi.xmax, y: roi.ymax },
		].map((corner) => imageToViewportPoint(corner, transform, orientation, geometry));
		const left = Math.min(...corners.map((corner) => corner.x));
		const top = Math.min(...corners.map((corner) => corner.y));
		return {
			index: roi.index,
			x: left + 2,
			y: top - ABOVE < INSIDE ? top + INSIDE : top - ABOVE,
		};
	}));
</script>

<!-- Drawn in unscaled viewport pixels so labels keep their size and stay upright at any zoom or orientation. -->
<svg class="roi-labels" aria-hidden="true">
	{#each labels as label (label.index)}
		<text class="roi-label" class:selected={selectedIndex === label.index} x={label.x} y={label.y}>#{label.index + 1}</text>
	{/each}
</svg>

<style>
	.roi-labels {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		overflow: hidden;
		pointer-events: none;
	}

	.roi-label {
		fill: var(--roi);
		stroke: var(--viewport);
		stroke-width: 2.4;
		paint-order: stroke;
		font-size: 11px;
		font-family: var(--font-mono);
	}

	.roi-label.selected {
		fill: var(--roi-selected);
	}
</style>

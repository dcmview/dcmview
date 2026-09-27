<script lang="ts">
	import type { PixelReadoutModel } from "./pixelProbe.svelte";
	import type { ValueWithUnit } from "./valueMapping";

	let { readout }: { readout: PixelReadoutModel } = $props();

	function withUnit({ value, unit }: ValueWithUnit): string {
		return unit ? `${value} ${unit}` : value;
	}
</script>

<div class="pixel-readout" role="status" aria-label="Pixel value under cursor">
	<span class="coordinates">
		row {readout.pixel.row} · col {readout.pixel.column} · frame {readout.frameNumber}
	</span>
	{#if readout.values?.kind === "grayscale"}
		<span><span class="label">stored</span> {readout.values.stored}</span>
		{#if readout.values.modality}
			<span><span class="label">modality</span> {withUnit(readout.values.modality)}</span>
		{/if}
		{#if readout.values.mapped}
			<span class="mapped">
				<span class="label">{readout.values.mapped.label ?? "mapped"}</span> {withUnit(readout.values.mapped)}
			</span>
		{:else if readout.values.mappedOutOfRange}
			<span class="note">outside mapped range</span>
		{/if}
		{#if readout.values.mappingSource}
			{@const source = readout.values.mappingSource}
			<span class="note" title={source.detail}>
				via {source.label}{source.count > 1 ? ` (1 of ${source.count})` : ""}
			</span>
		{/if}
	{:else if readout.values?.kind === "color"}
		<span>
			<span class="label">stored</span>
			{readout.values.components.map((component) => `${component.label} ${component.value}`).join(" · ")}
		</span>
	{:else if readout.values?.kind === "palette"}
		<span><span class="label">palette index</span> {readout.values.index}</span>
	{/if}
	{#if readout.note}
		<span class="note">{readout.note}</span>
	{/if}
	{#if readout.overlay}
		<span class="overlay-value">
			<span class="label">{readout.overlay.label}</span>
			{#if readout.overlay.value !== null}
				{readout.overlay.value} {readout.overlay.unit}
			{:else}
				<span class="note">{readout.overlay.note}</span>
			{/if}
		</span>
	{/if}
</div>

<style>
	.pixel-readout {
		display: flex;
		flex-wrap: wrap;
		gap: 3px 10px;
		max-width: min(36rem, calc(100vw - 2rem));
		padding: 6px 9px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
		color: var(--text);
		font: var(--t-mono);
		font-variant-numeric: tabular-nums;
		pointer-events: none;
	}

	.coordinates,
	.label,
	.note {
		color: var(--ink-muted);
	}

	.label {
		font: 400 11px/16px var(--font-ui);
	}

	.mapped,
	.overlay-value {
		font-weight: 600;
	}
</style>

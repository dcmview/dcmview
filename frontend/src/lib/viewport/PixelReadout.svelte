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
</div>

<style>
	.pixel-readout {
		display: flex;
		flex-wrap: wrap;
		gap: 0.2rem 0.75rem;
		max-width: min(36rem, calc(100vw - 2rem));
		font-size: 0.78rem;
		font-family: var(--font-mono);
		padding: 0.34rem 0.55rem;
		background: var(--surface-hud);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
		color: var(--text-primary);
		pointer-events: none;
	}
	.coordinates,
	.label,
	.note {
		color: var(--text-muted);
	}
	.mapped {
		color: var(--accent-text);
	}
</style>

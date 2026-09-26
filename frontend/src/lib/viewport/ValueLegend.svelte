<script lang="ts">
	import { formatValue } from "./valueMapping";

	let {
		title,
		unit,
		low,
		high,
		colors,
		caption = null,
	}: {
		title: string;
		unit: string;
		/** Value at the bottom of the bar. */
		low: number;
		/** Value at the top of the bar. */
		high: number;
		/** Evenly spaced CSS colors from bottom to top. */
		colors: readonly string[];
		caption?: string | null;
	} = $props();

	const gradient = $derived(`linear-gradient(to top, ${colors.join(", ")})`);
</script>

<figure class="value-legend" aria-label={`${title}: ${formatValue(low)} to ${formatValue(high)} ${unit}`}>
	<figcaption {title}>{title}</figcaption>
	<div class="scale">
		<span class="bar" style:background-image={gradient}></span>
		<span class="ticks" aria-hidden="true">
			<span>{formatValue(high)}</span>
			<span>{formatValue((low + high) / 2)}</span>
			<span>{formatValue(low)}</span>
		</span>
	</div>
	<span class="unit">{unit}</span>
	{#if caption}
		<span class="caption">{caption}</span>
	{/if}
</figure>

<style>
	.value-legend {
		display: grid;
		gap: 0.25rem;
		margin: 0;
		width: max-content;
		max-width: 7.5rem;
		padding: 0.4rem 0.5rem;
		font-size: 0.72rem;
		background: var(--surface-hud);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
		color: var(--text-secondary);
		pointer-events: none;
	}
	figcaption {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-primary);
	}
	.scale {
		display: flex;
		gap: 0.4rem;
		height: 7rem;
	}
	.bar {
		flex: 0 0 0.7rem;
		border: 1px solid var(--border-strong);
		border-radius: 2px;
	}
	.ticks {
		display: flex;
		flex-direction: column;
		justify-content: space-between;
		font-family: var(--font-mono);
	}
	.unit,
	.caption {
		color: var(--text-muted);
	}
	.unit {
		font-family: var(--font-mono);
	}
	@media (max-height: 640px), (max-width: 519px) {
		.scale {
			height: 4.5rem;
		}
	}
</style>

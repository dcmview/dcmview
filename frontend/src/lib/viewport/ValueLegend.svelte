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
		justify-items: center;
		gap: 4px;
		width: max-content;
		max-width: 7.5rem;
		margin: 0;
		padding: 8px 8px 6px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
		color: var(--text);
		font: 400 10px/12px var(--font-mono);
		font-variant-numeric: tabular-nums;
		pointer-events: none;
	}

	figcaption {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--ink-muted);
		font: 600 10px/12px var(--font-ui);
		letter-spacing: 0.05em;
		text-transform: uppercase;
	}

	.scale {
		display: flex;
		gap: 6px;
		height: 104px;
	}

	.bar {
		flex: 0 0 10px;
		border: 1px solid var(--line);
		border-radius: 2px;
	}

	.ticks {
		display: flex;
		flex-direction: column;
		justify-content: space-between;
	}

	.unit,
	.caption {
		color: var(--ink-muted);
	}

	.caption {
		font-family: var(--font-ui);
	}

	@media (max-height: 640px), (max-width: 519px) {
		.scale {
			height: 4.5rem;
		}
	}
</style>

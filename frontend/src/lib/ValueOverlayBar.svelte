<script lang="ts">
	import type { ValueOverlayCandidate } from "./app/valueOverlays.svelte";
	import StatusBadge from "./ui/StatusBadge.svelte";

	let {
		candidates,
		selectedVolume,
		opacity,
		coversFrame,
		ontoggle,
		onopacity,
	}: {
		candidates: readonly ValueOverlayCandidate[];
		selectedVolume: number | null;
		/** 0..1 */
		opacity: number;
		/** Whether the shown volume covers the displayed frame. */
		coversFrame: boolean;
		ontoggle: (volumeFileIndex: number) => void;
		onopacity: (opacity: number) => void;
	} = $props();

	const shown = $derived(candidates.some((candidate) => candidate.volumeFileIndex === selectedVolume));
	const percent = $derived(Math.round(opacity * 100));
</script>

<section class="value-overlay-bar" aria-label="Value overlays">
	<span class="heading">Overlay</span>
	<div class="choices" role="group" aria-label="Overlay volume">
		{#each candidates as candidate (candidate.volumeFileIndex)}
			<button
				type="button"
				class:active={candidate.volumeFileIndex === selectedVolume}
				aria-pressed={candidate.volumeFileIndex === selectedVolume}
				title={candidate.detail}
				onclick={() => ontoggle(candidate.volumeFileIndex)}
			>
				{candidate.title}
			</button>
		{/each}
	</div>
	<label class="opacity">
		Opacity
		<input
			type="range"
			min="10"
			max="100"
			step="5"
			value={percent}
			disabled={!shown}
			oninput={(event) => onopacity(Number(event.currentTarget.value) / 100)}
		/>
		<span class="percent">{percent}%</span>
	</label>
	{#if shown && !coversFrame}
		<StatusBadge status="partial">Not covering this frame</StatusBadge>
	{/if}
</section>

<style>
	.value-overlay-bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 10px;
		min-width: 0;
		color: var(--ink-muted);
		font: var(--t-meta);
	}

	.heading {
		font: var(--t-micro);
		letter-spacing: 0.06em;
		text-transform: uppercase;
	}

	.choices {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
	}

	button {
		height: 24px;
		padding: 0 8px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--paper);
		color: var(--ink-muted);
		font: 500 12px/16px var(--font-ui);
		white-space: nowrap;
		cursor: pointer;
	}

	button:hover {
		border-color: var(--subtle);
		color: var(--text);
	}

	button.active {
		border-color: var(--selection-edge);
		background: var(--selection-fill);
		color: var(--text);
	}

	button:focus-visible,
	input:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	.opacity {
		display: flex;
		align-items: center;
		gap: 8px;
	}

	input {
		width: 7rem;
		accent-color: var(--accent);
	}

	input:disabled {
		opacity: 0.42;
	}

	.percent {
		min-width: 2.6rem;
		color: var(--text);
		font: var(--t-mono);
		font-variant-numeric: tabular-nums;
	}
</style>

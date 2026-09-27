<script lang="ts">
	import type { ValueOverlayCandidate } from "./app/valueOverlays.svelte";

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
		<span class="note">Not covering this frame</span>
	{/if}
</section>

<style>
	.value-overlay-bar {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 12px;
		padding: 6px 12px;
		border-bottom: 1px solid var(--border-subtle);
		background: var(--surface-chrome);
		color: var(--text-secondary);
		font-size: 12px;
	}
	.heading {
		color: var(--text-primary);
		font-weight: 600;
	}
	.choices {
		display: flex;
		flex-wrap: wrap;
		gap: 4px;
	}
	button {
		border: 1px solid var(--border-strong);
		border-radius: 4px;
		padding: 4px 9px;
		color: var(--text-secondary);
		background: var(--surface-control);
		font: inherit;
		cursor: pointer;
	}
	button:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}
	button.active {
		color: var(--surface-root);
		background: var(--surface-control-active);
	}
	button:focus-visible,
	input:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}
	.opacity {
		display: flex;
		align-items: center;
		gap: 6px;
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
		font-family: var(--font-mono);
	}
	.note {
		color: var(--text-muted);
	}
</style>

<script lang="ts">
	import type { CineDirection, CineMode } from "./cinePlayback";

	let {
		totalFrames,
		currentPosition,
		onpositionchange,
		cinePlaying = $bindable(),
		cineFps = $bindable(),
		cineMode = $bindable(),
		cineDirection = $bindable(),
	}: {
		totalFrames: number;
		currentPosition: number;
		onpositionchange: (position: number) => void;
		cinePlaying: boolean;
		cineFps: number;
		cineMode: CineMode;
		cineDirection: CineDirection;
	} = $props();

	const FPS_OPTIONS = [1, 5, 10, 15, 24];

	/** Stops cine and moves one image back or forward, clamped to the stack. */
	export function step(delta: -1 | 1) {
		if (totalFrames <= 1) {
			return;
		}
		cinePlaying = false;
		onpositionchange(Math.max(0, Math.min(totalFrames - 1, currentPosition + delta)));
	}

	export function togglePlay() {
		if (totalFrames <= 1) return;
		if (!cinePlaying) {
			cineDirection = 1;
		}
		cinePlaying = !cinePlaying;
	}

	$effect(() => {
		if (currentPosition >= totalFrames && totalFrames > 0) {
			onpositionchange(0);
		}
		if (totalFrames <= 1) {
			cinePlaying = false;
		}
	});
</script>

{#if totalFrames > 1}
	<div class="slider">
		<input
			type="range"
			hidden
			data-capture-position
			min="0"
			max={Math.max(0, totalFrames - 1)}
			value={currentPosition}
			oninput={(event) => onpositionchange(Number(event.currentTarget.value))}
		/>
		<button type="button" onclick={() => step(-1)} aria-label="Previous image">◀</button>
		<span>image {currentPosition + 1} / {totalFrames}</span>
		<button type="button" onclick={() => step(1)} aria-label="Next image">▶</button>
		<button type="button" class="play" onclick={togglePlay} aria-label={cinePlaying ? "Pause cine" : "Play cine"}>
			{cinePlaying ? "⏸" : "▶"}
		</button>
		<select class="fps-select" bind:value={cineFps}>
			{#each FPS_OPTIONS as f}
				<option value={f}>{f} fps</option>
			{/each}
		</select>
		<button type="button" class="mode-toggle" onclick={() => cineMode = cineMode === "loop" ? "sweep" : "loop"}>
			{cineMode === "sweep" ? 'Sweep' : 'Loop'}
		</button>
	</div>
{/if}

<style>
	.slider {
		display: flex;
		flex-wrap: wrap;
		gap: 0.42rem 0.55rem;
		align-items: center;
		min-width: 0;
		padding: 0.55rem 0.85rem;
		background: var(--surface-chrome);
		border-top: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: 0.84rem;
	}
	button {
		min-height: var(--control-height);
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		padding: 0.22rem 0.65rem;
		border-radius: var(--radius-control);
		cursor: pointer;
		font: inherit;
	}
	.play {
		margin-left: 0.25rem;
		border-color: rgba(10, 132, 255, 0.42);
		color: var(--text-primary);
	}
	.fps-select {
		min-height: var(--control-height);
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		color: var(--text-primary);
		padding: 0.22rem 1.55rem 0.22rem 0.5rem;
		border-radius: var(--radius-control);
		font-size: inherit;
	}
	button:hover,
	.fps-select:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}
	button:focus-visible,
	.fps-select:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
	}
	.mode-toggle {
		font-size: 0.85em;
	}
</style>

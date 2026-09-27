<script lang="ts">
	import { MAX_ZOOM, MIN_ZOOM } from "./viewTransform";

	let {
		scale,
		onstep,
		onfit,
	}: {
		scale: number;
		/** Zoom to the next preset level out (-1) or in (1). */
		onstep: (direction: 1 | -1) => void;
		onfit: () => void;
	} = $props();
</script>

<div class="zoom-controls">
	<button type="button" onclick={() => onstep(-1)} disabled={scale <= MIN_ZOOM}>−</button>
	<button type="button" class="zoom-level" onclick={onfit} title="Fit to height">{Math.round(scale * 100)}%</button>
	<button type="button" onclick={() => onstep(1)} disabled={scale >= MAX_ZOOM}>+</button>
</div>

<style>
	.zoom-controls {
		position: absolute;
		right: 12px;
		bottom: 12px;
		display: flex;
		align-items: center;
		padding: 2px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
	}

	button {
		display: grid;
		place-items: center;
		min-width: 24px;
		height: 22px;
		padding: 0 6px;
		border: 0;
		border-radius: var(--radius-sm);
		background: none;
		color: var(--text);
		font: 400 14px/1 var(--font-ui);
		cursor: pointer;
	}

	button:hover:not(:disabled) {
		background: var(--row-hover);
	}

	button:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: -2px;
	}

	button:disabled {
		color: var(--subtle);
		cursor: default;
	}

	.zoom-level {
		min-width: 3.2rem;
		border-radius: 0;
		border-inline: 1px solid var(--line);
		font: 400 11px/1 var(--font-mono);
		font-variant-numeric: tabular-nums;
	}
</style>

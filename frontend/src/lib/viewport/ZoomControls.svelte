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
		right: 0.75rem;
		bottom: 0.75rem;
		display: flex;
		align-items: center;
		gap: 0;
		background: var(--surface-hud);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-panel);
		overflow: hidden;
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
	}
	button {
		background: none;
		border: none;
		color: var(--text-secondary);
		padding: 0.3rem 0.55rem;
		font-size: 0.95rem;
		cursor: pointer;
		line-height: 1;
	}
	button:hover:not(:disabled) {
		background: var(--surface-hover-overlay);
		color: var(--text-primary);
	}
	button:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: -2px;
	}
	button:disabled {
		color: var(--text-disabled);
		cursor: default;
	}
	.zoom-level {
		padding: 0.3rem 0.4rem;
		font-size: 0.78rem;
		font-family: var(--font-mono);
		color: var(--text-secondary);
		min-width: 3.2rem;
		text-align: center;
		cursor: pointer;
		border-left: 1px solid var(--border-subtle);
		border-right: 1px solid var(--border-subtle);
	}
	.zoom-level:hover {
		color: var(--text-primary);
	}
</style>

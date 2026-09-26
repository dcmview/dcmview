<script lang="ts">
	import type { PersistenceStatus } from "../revisionedPersistence";
	import { formatRoiFrames, type VisibleRoi } from "./roiEditing";

	let {
		rois,
		totalCount,
		frameCount,
		selectedIndex,
		loading,
		error,
		ready,
		saveStatus,
		onselect,
		onscope,
		ondelete,
		onretryload,
		onretrysave,
		onrevert,
	}: {
		/** ROIs shown on the current frame. */
		rois: readonly VisibleRoi[];
		/** ROIs across all frames, once annotations have loaded. */
		totalCount: number | null;
		frameCount: number;
		selectedIndex: number | null;
		loading: boolean;
		error: string | null;
		ready: boolean;
		saveStatus: PersistenceStatus | null;
		onselect: (roiIndex: number) => void;
		onscope: (scope: "current" | "all") => void;
		ondelete: () => void;
		onretryload: () => void;
		onretrysave: () => void;
		onrevert: () => void;
	} = $props();

	const countLabel = $derived(totalCount === null ? String(rois.length) : `${rois.length} / ${totalCount}`);
</script>

<div class="roi-list">
	<div class="roi-list-title">
		<span>ROIs {countLabel}</span>
		{#if saveStatus === "saving"}
			<span class="roi-save-status">saving…</span>
		{:else if saveStatus === "dirty"}
			<span class="roi-save-status">unsaved</span>
		{/if}
	</div>
	{#if loading}
		<div class="roi-list-status">Loading annotations…</div>
	{:else if error}
		<div class="roi-list-status error">
			<span>{error}</span>
			{#if saveStatus === "error"}
				<div class="roi-error-actions">
					<button type="button" onclick={onretrysave}>Retry</button>
					<button type="button" onclick={onrevert}>Revert</button>
				</div>
			{:else if !ready}
				<div class="roi-error-actions">
					<button type="button" onclick={onretryload}>Retry</button>
				</div>
			{/if}
		</div>
	{:else if rois.length === 0}
		<div class="roi-list-status">No ROIs for this frame</div>
	{:else}
		<ul>
			{#each rois as roi (roi.index)}
				<li class:selected={selectedIndex === roi.index}>
					<button type="button" class="roi-select" onclick={() => onselect(roi.index)}>
						<span class="roi-id">#{roi.index + 1}</span>
					</button>
					<span class="roi-coords">[{roi.ymin}, {roi.xmin}, {roi.ymax}, {roi.xmax}]</span>
					<span class="roi-frames">{formatRoiFrames(roi.frames, frameCount)}</span>
					{#if selectedIndex === roi.index}
						<div class="roi-actions">
							<button type="button" onclick={() => onscope("current")}>Current</button>
							<button type="button" onclick={() => onscope("all")}>All</button>
							<button type="button" class="danger" onclick={ondelete}>Delete</button>
						</div>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.roi-list {
		position: absolute;
		right: 0.75rem;
		top: 0.75rem;
		max-width: min(48ch, 46%);
		max-height: 38%;
		overflow: auto;
		font-size: 0.72rem;
		padding: 0.5rem 0.55rem;
		background: var(--surface-hud);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-panel);
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
		z-index: 2;
		scrollbar-width: thin;
	}
	.roi-list-title {
		display: flex;
		justify-content: space-between;
		gap: 0.75rem;
		font-weight: 600;
		margin-bottom: 0.25rem;
		color: var(--text-primary);
	}
	.roi-save-status {
		color: var(--text-muted);
		font-weight: 400;
	}
	.roi-list-status {
		color: var(--text-muted);
	}
	.roi-list-status.error {
		color: var(--danger);
	}
	.roi-error-actions {
		display: flex;
		gap: 0.25rem;
		margin-top: 0.35rem;
	}
	.roi-error-actions button {
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.68rem;
		padding: 0.15rem 0.35rem;
	}
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
		display: grid;
		gap: 0.2rem;
	}
	li {
		display: grid;
		gap: 0.1rem;
		padding: 0.18rem 0;
		border-top: 1px solid var(--border-subtle);
	}
	li.selected {
		background: var(--accent-soft);
		margin-inline: -0.25rem;
		padding-inline: 0.25rem;
		border-radius: 4px;
	}
	li:first-child {
		border-top: none;
		padding-top: 0;
	}
	.roi-select {
		width: fit-content;
		background: none;
		border: none;
		color: inherit;
		padding: 0;
		cursor: pointer;
	}
	.roi-select:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
		border-radius: 3px;
	}
	.roi-id {
		font-weight: 600;
		color: var(--accent-text);
	}
	.roi-coords,
	.roi-frames {
		font-family: var(--font-mono);
		line-height: 1.25;
		color: var(--text-secondary);
	}
	.roi-actions {
		display: flex;
		gap: 0.25rem;
		margin-top: 0.15rem;
	}
	.roi-actions button {
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.68rem;
		padding: 0.15rem 0.35rem;
	}
	.roi-actions button:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}
	.roi-actions button:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
	}
	.roi-actions button.danger {
		color: var(--danger-text);
	}
</style>

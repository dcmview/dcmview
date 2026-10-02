<script lang="ts">
	import type { PersistenceStatus } from "../revisionedPersistence";
	import { formatRoiFrames, type VisibleRoi } from "./roiEditing";

	let {
		rois,
		noun = "ROI",
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
		onapplytoseries,
	}: {
		/** What a rectangle is called: "ROI", or "redaction". */
		noun?: string;
		onapplytoseries?: () => void;
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
		<span>{noun[0].toUpperCase()}{noun.slice(1)}s {countLabel}</span>
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
		<div class="roi-list-status">No {noun}s for this frame</div>
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
	{#if onapplytoseries && totalCount}
		<div class="roi-actions">
			<button
				type="button"
				disabled={saveStatus !== "clean"}
				title="Copy this file's boxes to every file of the series with the same image size"
				onclick={onapplytoseries}
			>Apply to series</button>
		</div>
	{/if}
</div>

<style>
	.roi-list {
		position: absolute;
		right: 12px;
		top: 12px;
		z-index: 2;
		max-width: min(48ch, 46%);
		max-height: 38%;
		overflow: auto;
		padding: 8px 10px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
		color: var(--text);
		font: 400 11px/16px var(--font-ui);
		scrollbar-width: thin;
	}

	.roi-list-title {
		display: flex;
		justify-content: space-between;
		gap: 12px;
		margin-bottom: 4px;
		padding-bottom: 6px;
		border-bottom: 1px solid var(--line);
		font: 600 12px/16px var(--font-ui);
	}

	.roi-save-status {
		color: var(--ink-muted);
		font: 400 11px/16px var(--font-mono);
	}

	.roi-list-status {
		color: var(--ink-muted);
	}

	.roi-list-status.error {
		color: var(--red-text);
	}

	.roi-error-actions,
	.roi-actions {
		display: flex;
		gap: 4px;
		margin-top: 4px;
	}

	.roi-error-actions button,
	.roi-actions button {
		height: 22px;
		padding: 0 8px;
		border: 1px solid var(--ink);
		border-radius: var(--radius-sm);
		background: linear-gradient(var(--control-top), var(--control-bot));
		box-shadow: var(--elev-control-secondary);
		color: var(--text);
		font: 500 11px/1 var(--font-ui);
		cursor: pointer;
	}

	.roi-error-actions button:hover,
	.roi-actions button:hover {
		background: var(--control-top);
	}

	.roi-error-actions button:focus-visible,
	.roi-actions button:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	.roi-actions button.danger {
		color: var(--red-text);
	}

	ul {
		display: grid;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	li {
		display: grid;
		gap: 2px;
		padding: 3px 0;
		border-top: 1px solid var(--line);
	}

	li:first-child {
		border-top: none;
		padding-top: 0;
	}

	li.selected {
		margin-inline: -4px;
		padding-inline: 4px;
		border: 1px solid var(--selection-edge);
		border-radius: var(--radius-sm);
		background: var(--selection-fill);
	}

	.roi-select {
		width: fit-content;
		padding: 0;
		border: none;
		background: none;
		color: inherit;
		font: inherit;
		cursor: pointer;
	}

	.roi-select:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
		border-radius: var(--radius-sm);
	}

	.roi-id {
		font-weight: 600;
	}

	.roi-coords,
	.roi-frames {
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
		font-variant-numeric: tabular-nums;
	}
</style>

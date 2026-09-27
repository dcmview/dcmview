<script lang="ts">
	import type { FileSummary } from "../api";
	import Icon from "./ui/Icon.svelte";

	let {
		openFiles,
		frameCounts,
		activeFileIndex,
		activePosition,
		onactivate,
		onclose,
	}: {
		openFiles: FileSummary[];
		frameCounts: ReadonlyMap<number, number>;
		activeFileIndex: number | null;
		/** Zero-based position of the active tab in its stack or frames. */
		activePosition: number;
		onactivate: (index: number) => void;
		onclose: (index: number) => void;
	} = $props();

	function basename(path: string): string {
		return path.split(/[\\/]/).pop() || path;
	}

	function tabLabel(file: FileSummary): string {
		const instance = file.instance_number.trim();
		const base = basename(file.path);
		return instance ? `#${instance} ${base}` : base;
	}

	/** The active tab shows where it is in its images; the others how many they hold. */
	function tabDetail(file: FileSummary): string {
		if (!file.has_pixels) return "tags";
		const count = frameCounts.get(file.index) ?? file.frame_count;
		return file.index === activeFileIndex ? `${activePosition + 1}/${count}` : `${count} img`;
	}

	function closeTab(event: MouseEvent, index: number) {
		event.stopPropagation();
		onclose(index);
	}
</script>

<nav class="open-tabs" aria-label="Open images">
	{#if openFiles.length === 0}
		<div class="empty-tabs">No open images</div>
	{:else}
		{#each openFiles as file (file.index)}
			<div
				class="tab"
				class:active={file.index === activeFileIndex}
				aria-current={file.index === activeFileIndex ? "page" : undefined}
				title={file.path}
			>
				<button
					type="button"
					class="tab-main"
					onclick={() => onactivate(file.index)}
				>
					<span class="tab-label">{tabLabel(file)}</span>
					<span class="tab-detail">{tabDetail(file)}</span>
				</button>
				<button
					type="button"
					class="close"
					onclick={(event) => closeTab(event, file.index)}
					aria-label={`Close ${tabLabel(file)}`}
				>
					<Icon name="close" size={12} />
				</button>
			</div>
		{/each}
	{/if}
</nav>

<style>
	.open-tabs {
		display: flex;
		align-items: stretch;
		min-width: 0;
		height: 100%;
		overflow-x: auto;
		scrollbar-width: thin;
	}

	.empty-tabs {
		align-self: center;
		padding: 0 14px;
		color: var(--ink-muted);
		font: var(--t-ui);
		white-space: nowrap;
	}

	.tab {
		position: relative;
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		min-width: 9rem;
		max-width: 16rem;
		border-right: 1px solid var(--line);
		color: var(--ink-muted);
	}

	.tab:hover {
		background: var(--row-hover);
		color: var(--text);
	}

	.tab.active {
		margin-bottom: -1px;
		background: var(--paper);
		color: var(--text);
	}

	.tab.active::before {
		content: "";
		position: absolute;
		inset: 0 0 auto;
		height: 2px;
		background: var(--accent);
	}

	.tab-main {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		gap: 8px;
		min-width: 0;
		height: 100%;
		padding: 0 4px 0 14px;
		border: 0;
		background: transparent;
		color: inherit;
		font: var(--t-ui);
		cursor: pointer;
	}

	.tab.active .tab-main {
		font-weight: 600;
	}

	.tab-label {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		text-align: left;
	}

	.tab-detail {
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
		font-variant-numeric: tabular-nums;
	}

	.close {
		display: grid;
		place-items: center;
		width: 20px;
		height: 20px;
		margin-right: 6px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--ink-muted);
		cursor: pointer;
	}

	.close:hover {
		background: var(--row-hover);
		color: var(--text);
	}

	.tab-main:focus-visible,
	.close:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: -2px;
	}
</style>

<script lang="ts">
	import type { FileSummary } from "../api";

	let {
		openFiles,
		frameCounts,
		activeFileIndex,
		onactivate,
		onclose,
	}: {
		openFiles: FileSummary[];
		frameCounts: ReadonlyMap<number, number>;
		activeFileIndex: number | null;
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
				title={file.path}
			>
				<button
					type="button"
					class="tab-main"
					onclick={() => onactivate(file.index)}
				>
					<span class="tab-label">{tabLabel(file)}</span>
					<span class="tab-detail">{file.has_pixels ? `${frameCounts.get(file.index) ?? file.frame_count}i` : "tags"}</span>
				</button>
				<button
					type="button"
					class="close"
					onclick={(event) => closeTab(event, file.index)}
					aria-label={`Close ${tabLabel(file)}`}
				>
					x
				</button>
			</div>
		{/each}
	{/if}
</nav>

<style>
	.open-tabs {
		display: flex;
		align-items: end;
		gap: 0.2rem;
		min-width: 0;
		overflow-x: auto;
		padding-top: 0.3rem;
		scrollbar-width: thin;
	}

	.empty-tabs {
		color: var(--text-muted);
		font-size: 0.84rem;
		padding: 0.35rem 0.25rem;
		white-space: nowrap;
	}

	.tab {
		display: grid;
		grid-template-columns: minmax(5rem, 1fr) auto;
		align-items: center;
		min-width: 9rem;
		max-width: 16rem;
		height: 2rem;
		border: 1px solid transparent;
		border-bottom: 0;
		border-radius: var(--radius-control) var(--radius-control) 0 0;
		background: rgba(255, 255, 255, 0.035);
		color: var(--text-secondary);
		overflow: hidden;
	}

	.tab:hover {
		background: rgba(255, 255, 255, 0.07);
		color: var(--text-primary);
	}

	.tab.active {
		background: var(--surface-panel);
		border-color: var(--border-subtle);
		color: var(--text-primary);
		box-shadow: inset 0 2px 0 var(--accent);
	}

	.tab-main {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		gap: 0.4rem;
		min-width: 0;
		height: 100%;
		border: 0;
		background: transparent;
		color: inherit;
		cursor: pointer;
		padding: 0 0.35rem 0 0.65rem;
	}

	.tab-label {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		text-align: left;
		font-size: 0.82rem;
	}

	.tab-detail {
		color: var(--text-muted);
		font-size: 0.72rem;
	}

	.close {
		display: grid;
		place-items: center;
		width: 1.25rem;
		height: 1.25rem;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: var(--text-muted);
		cursor: pointer;
		font-size: 0.78rem;
	}

	.close:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}

	.tab-main:focus-visible,
	.close:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}
</style>

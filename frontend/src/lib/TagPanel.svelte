<script lang="ts">
	import { fetchTags, type TagNode } from "../api";
	import {
		ensureWhenSettled,
		KeyedAsyncResource,
		METADATA_CACHE_FILES,
		type AsyncResourceSnapshot,
	} from "./keyedAsyncResource";
	import {
		flattenTagRows,
		isSequenceTag,
		tagValueDisplay,
		tagValueToCopyText,
		type FlatTagRow,
	} from "./tagRows";

	type ColumnKey = "tag" | "keyword" | "vr";

	type ColumnResizeState = {
		pointerId: number;
		column: ColumnKey;
		startX: number;
		startWidth: number;
	};

	const TAG_COLUMN_DEFAULT_PX = 128;
	const KEYWORD_COLUMN_DEFAULT_PX = 136;
	const VR_COLUMN_DEFAULT_PX = 64;

	const TAG_COLUMN_MIN_PX = 88;
	const TAG_COLUMN_MAX_PX = 260;
	const KEYWORD_COLUMN_MIN_PX = 100;
	const KEYWORD_COLUMN_MAX_PX = 320;
	const VR_COLUMN_MIN_PX = 52;
	const VR_COLUMN_MAX_PX = 140;

	let { fileIndex }: { fileIndex: number } = $props();

	let filter = $state("");
	let tagResourcesByFile = $state<Record<number, AsyncResourceSnapshot<TagNode[]> | undefined>>({});
	let expandedSequences = $state<Set<string>>(new Set());
	let expandedLongValues = $state<Set<string>>(new Set());
	let copiedKey = $state<string | null>(null);
	let tagColumnWidthPx = $state(TAG_COLUMN_DEFAULT_PX);
	let keywordColumnWidthPx = $state(KEYWORD_COLUMN_DEFAULT_PX);
	let vrColumnWidthPx = $state(VR_COLUMN_DEFAULT_PX);
	let columnResizeState = $state<ColumnResizeState | null>(null);
	const tagResources = new KeyedAsyncResource<number, TagNode[]>({
		load: fetchTags,
		capacity: METADATA_CACHE_FILES,
		onChange: (fileIndex, snapshot) => {
			const { [fileIndex]: _previous, ...rest } = tagResourcesByFile;
			tagResourcesByFile = snapshot.status === "idle" ? rest : { ...rest, [fileIndex]: snapshot };
		},
	});

	const tableColumns = $derived(
		`${tagColumnWidthPx}px ${keywordColumnWidthPx}px ${vrColumnWidthPx}px minmax(0, 1fr)`,
	);
	const activeTagResource = $derived(tagResourcesByFile[fileIndex]);
	// A file waiting out the settle delay counts as loading.
	const loading = $derived(!activeTagResource || activeTagResource.status === "loading");
	const error = $derived(activeTagResource?.error ?? null);

	$effect(() => ensureWhenSettled(tagResources, fileIndex));

	function retryTags() {
		void tagResources.reload(fileIndex).catch(() => {});
	}

	function toggleSequence(key: string) {
		const next = new Set(expandedSequences);
		if (next.has(key)) {
			next.delete(key);
		} else {
			next.add(key);
		}
		expandedSequences = next;
	}

	function toggleLongValue(key: string) {
		const next = new Set(expandedLongValues);
		if (next.has(key)) {
			next.delete(key);
		} else {
			next.add(key);
		}
		expandedLongValues = next;
	}

	function getColumnWidth(column: ColumnKey): number {
		switch (column) {
			case "tag":
				return tagColumnWidthPx;
			case "keyword":
				return keywordColumnWidthPx;
			case "vr":
				return vrColumnWidthPx;
		}
	}

	function clampColumnWidth(column: ColumnKey, width: number): number {
		switch (column) {
			case "tag":
				return Math.min(TAG_COLUMN_MAX_PX, Math.max(TAG_COLUMN_MIN_PX, width));
			case "keyword":
				return Math.min(KEYWORD_COLUMN_MAX_PX, Math.max(KEYWORD_COLUMN_MIN_PX, width));
			case "vr":
				return Math.min(VR_COLUMN_MAX_PX, Math.max(VR_COLUMN_MIN_PX, width));
		}
	}

	function setColumnWidth(column: ColumnKey, width: number) {
		if (column === "tag") {
			tagColumnWidthPx = width;
			return;
		}
		if (column === "keyword") {
			keywordColumnWidthPx = width;
			return;
		}
		vrColumnWidthPx = width;
	}

	function startColumnResize(column: ColumnKey, event: PointerEvent) {
		if (event.button !== 0) {
			return;
		}

		const handle = event.currentTarget as HTMLElement;
		handle.setPointerCapture(event.pointerId);
		columnResizeState = {
			pointerId: event.pointerId,
			column,
			startX: event.clientX,
			startWidth: getColumnWidth(column),
		};
		event.preventDefault();
	}

	function moveColumnResize(event: PointerEvent) {
		if (!columnResizeState || columnResizeState.pointerId !== event.pointerId) {
			return;
		}

		const delta = event.clientX - columnResizeState.startX;
		const nextWidth = clampColumnWidth(
			columnResizeState.column,
			columnResizeState.startWidth + delta,
		);
		setColumnWidth(columnResizeState.column, nextWidth);
	}

	function endColumnResize(event: PointerEvent) {
		const handle = event.currentTarget as HTMLElement;
		if (handle.hasPointerCapture(event.pointerId)) {
			handle.releasePointerCapture(event.pointerId);
		}

		if (columnResizeState?.pointerId === event.pointerId) {
			columnResizeState = null;
		}
	}

	function cancelColumnResize() {
		columnResizeState = null;
	}

	async function copyRow(row: FlatTagRow) {
		const text = `${row.node.tag}  ${row.node.keyword}  =  ${tagValueToCopyText(row.node.value)}`;
		try {
			await navigator.clipboard.writeText(text);
			copiedKey = row.key;
			setTimeout(() => {
				if (copiedKey === row.key) {
					copiedKey = null;
				}
			}, 1500);
		} catch {
			copiedKey = null;
		}
	}

	const visibleRows = $derived.by(() => {
		const source = activeTagResource?.value ?? [];
		return flattenTagRows(source, `f${fileIndex}`, expandedSequences, filter);
	});
</script>

<aside class="panel">
	<header>
		<h2>DICOM Tags</h2>
		<input bind:value={filter} placeholder="filter tags..." />
	</header>
	{#if error}
		<div class="error">
			<span>{error}</span>
			<button type="button" onclick={retryTags}>Retry</button>
		</div>
	{:else if loading}
		<p class="loading">Loading tags…</p>
	{:else}
		<div class="table" style={`--tag-grid-columns:${tableColumns};`}>
			<div class="header-row row-grid" role="row">
				<div class="header-cell resizable">
					<span>Tag</span>
					<button
						type="button"
						class="column-resizer"
						class:dragging={columnResizeState?.column === "tag"}
						aria-label="Resize tag column"
						onpointerdown={(event) => startColumnResize("tag", event)}
						onpointermove={moveColumnResize}
						onpointerup={endColumnResize}
						onpointercancel={cancelColumnResize}
					></button>
				</div>
				<div class="header-cell resizable">
					<span>Keyword</span>
					<button
						type="button"
						class="column-resizer"
						class:dragging={columnResizeState?.column === "keyword"}
						aria-label="Resize keyword column"
						onpointerdown={(event) => startColumnResize("keyword", event)}
						onpointermove={moveColumnResize}
						onpointerup={endColumnResize}
						onpointercancel={cancelColumnResize}
					></button>
				</div>
				<div class="header-cell resizable">
					<span>VR</span>
					<button
						type="button"
						class="column-resizer"
						class:dragging={columnResizeState?.column === "vr"}
						aria-label="Resize VR column"
						onpointerdown={(event) => startColumnResize("vr", event)}
						onpointermove={moveColumnResize}
						onpointerup={endColumnResize}
						onpointercancel={cancelColumnResize}
					></button>
				</div>
				<div class="header-cell">Value</div>
			</div>
			{#each visibleRows as row}
				<div
					class="row row-grid"
					role="button"
					tabindex="0"
					onclick={() => copyRow(row)}
					onkeydown={(event) => {
						if (event.key === "Enter" || event.key === " ") {
							event.preventDefault();
							void copyRow(row);
						}
					}}
				>
					<div class="tag-cell" style={`--depth:${row.depth}`}>
						{#if isSequenceTag(row.node)}
							<button
								type="button"
								class="chevron"
								onclick={(event) => { event.stopPropagation(); toggleSequence(row.key); }}
							>
								{expandedSequences.has(row.key) ? "▼" : "▶"}
							</button>
						{/if}
						<span>{row.node.tag}</span>
					</div>
					<div class="keyword-cell">{row.node.keyword}</div>
					<div class="vr-cell">{row.node.vr}</div>
					<div class:binary={row.node.value.type === "binary"} class="value-cell">
						<button
							type="button"
							class="value-toggle"
							onclick={(event) => {
								event.stopPropagation();
								if (row.node.value.type === "string" && row.node.value.value.length > 80) {
									toggleLongValue(row.key);
								}
							}}
						>
							{tagValueDisplay(row, expandedLongValues.has(row.key))}
						</button>
						{#if copiedKey === row.key}
							<span class="copied">Copied ✓</span>
						{/if}
					</div>
				</div>
			{/each}
		</div>
	{/if}
</aside>

<style>
	.panel {
		background: var(--surface-panel);
		display: grid;
		grid-template-rows: auto 1fr;
		height: 100%;
		min-height: 0;
	}

	header {
		padding: 0.7rem;
		border-bottom: 1px solid var(--border-subtle);
	}

	h2 {
		margin: 0 0 0.5rem 0;
		color: var(--text-secondary);
		font-size: 0.84rem;
		font-weight: 650;
	}

	input {
		width: 100%;
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		color: var(--text-primary);
		padding: 0.42rem 0.6rem;
		border-radius: var(--radius-control);
		font: inherit;
		font-size: 0.82rem;
	}

	input::placeholder {
		color: var(--text-muted);
	}

	input:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	.table {
		overflow: auto;
		min-width: 0;
		min-height: 0;
		font-family: var(--font-mono);
		font-size: 0.8rem;
		scrollbar-width: thin;
	}

	.row-grid {
		display: grid;
		grid-template-columns: var(--tag-grid-columns);
		gap: 0.5rem;
		align-items: center;
		min-width: 0;
	}

	.header-row {
		position: sticky;
		top: 0;
		z-index: 2;
		padding: 0.42rem 0.75rem;
		background: color-mix(in srgb, var(--surface-panel) 94%, black);
		border-bottom: 1px solid var(--border-subtle);
	}

	.header-cell {
		position: relative;
		min-width: 0;
		color: var(--text-muted);
		font-size: 0.72rem;
		font-weight: 600;
		letter-spacing: 0.03em;
		text-transform: uppercase;
		user-select: none;
	}

	.header-cell.resizable {
		padding-right: 0.45rem;
	}

	.column-resizer {
		position: absolute;
		right: -0.35rem;
		top: -0.35rem;
		bottom: -0.35rem;
		width: 0.75rem;
		border: 0;
		padding: 0;
		margin: 0;
		background: transparent;
		cursor: col-resize;
		touch-action: none;
	}

	.column-resizer::after {
		content: "";
		position: absolute;
		left: 50%;
		top: 0.2rem;
		bottom: 0.2rem;
		width: 1px;
		background: var(--border-subtle);
		transform: translateX(-50%);
	}

	.column-resizer.dragging::after {
		background: var(--accent);
	}

	.row {
		padding: 0.36rem 0.75rem;
		border-bottom: 1px solid rgba(255, 255, 255, 0.045);
		color: inherit;
		text-align: left;
	}

	.row:hover {
		background: rgba(255, 255, 255, 0.045);
	}

	.row:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: -2px;
	}

	.row > div {
		min-width: 0;
	}

	.tag-cell {
		display: flex;
		gap: 0.35rem;
		align-items: center;
		padding-left: calc(var(--depth) * 0.9rem);
	}

	.tag-cell span,
	.keyword-cell,
	.vr-cell {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chevron {
		cursor: pointer;
		color: var(--accent);
		font-size: 0.75rem;
		border: 0;
		padding: 0;
		background: transparent;
	}

	.value-cell {
		position: relative;
		min-width: 0;
		padding-right: 4.4rem;
	}

	.value-toggle {
		display: block;
		width: 100%;
		min-width: 0;
		border: 0;
		background: transparent;
		padding: 0;
		margin: 0;
		color: inherit;
		font: inherit;
		text-align: left;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.binary {
		color: var(--text-muted);
	}

	.copied {
		position: absolute;
		right: 0;
		top: 50%;
		transform: translateY(-50%);
		color: var(--accent);
		font-size: 0.72rem;
		white-space: nowrap;
		max-width: 4rem;
		overflow: hidden;
		text-overflow: ellipsis;
		pointer-events: none;
	}

	.error,
	.loading {
		padding: 0.75rem;
		color: var(--text-muted);
	}

	.error {
		display: grid;
		justify-items: start;
		gap: 0.45rem;
		color: var(--danger);
	}

	.error button {
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		color: var(--text-secondary);
		cursor: pointer;
		font: inherit;
		padding: 0.25rem 0.55rem;
	}
</style>

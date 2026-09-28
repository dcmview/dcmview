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
	import Button from "./ui/Button.svelte";
	import Icon from "./ui/Icon.svelte";
	import SearchField from "./ui/SearchField.svelte";

	type ColumnKey = "tag" | "keyword" | "vr";

	type ColumnResizeState = {
		pointerId: number;
		column: ColumnKey;
		startX: number;
		startWidth: number;
	};

	const TAG_COLUMN_DEFAULT_PX = 84;
	const KEYWORD_COLUMN_DEFAULT_PX = 140;
	const VR_COLUMN_DEFAULT_PX = 28;

	const TAG_COLUMN_MIN_PX = 72;
	const TAG_COLUMN_MAX_PX = 260;
	const KEYWORD_COLUMN_MIN_PX = 80;
	const KEYWORD_COLUMN_MAX_PX = 320;
	const VR_COLUMN_MIN_PX = 28;
	const VR_COLUMN_MAX_PX = 140;

	let { fileIndex }: { fileIndex: number } = $props();

	let filter = $state("");
	let tagResourcesByFile = $state.raw<Record<number, AsyncResourceSnapshot<TagNode[]> | undefined>>({});
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
		// Keyword gives way before Value: Value always keeps at least 96px on screen.
		`${tagColumnWidthPx}px minmax(0, ${keywordColumnWidthPx}px) ${vrColumnWidthPx}px minmax(96px, 1fr)`,
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
		<h2>DICOM tags</h2>
		<SearchField bind:value={filter} placeholder="keyword, tag or value" aria-label="Filter tags" />
	</header>
	{#if error}
		<div class="error">
			<span>{error}</span>
			<Button icon="reset" onclick={retryTags}>Retry</Button>
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
					class:nested={row.depth > 0}
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
								aria-label={expandedSequences.has(row.key) ? "Collapse sequence" : "Expand sequence"}
								aria-expanded={expandedSequences.has(row.key)}
								onclick={(event) => { event.stopPropagation(); toggleSequence(row.key); }}
							>
								<Icon name={expandedSequences.has(row.key) ? "chevron-down" : "chevron-right"} size={12} />
							</button>
						{/if}
						<span>{row.node.tag}</span>
					</div>
					<div class="keyword-cell">{row.node.keyword}</div>
					<div class="vr-cell">{row.node.vr}</div>
					<div
						class:note={row.node.value.type === "binary" || row.node.value.type === "sequence"}
						class:value-error={row.node.value.type === "error"}
						class="value-cell"
					>
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
							<span class="copied"><Icon name="check" size={12} />Copied</span>
						{/if}
					</div>
				</div>
			{/each}
		</div>
	{/if}
</aside>

<style>
	.panel {
		display: grid;
		grid-template-rows: auto 1fr;
		height: 100%;
		min-height: 0;
		background: var(--paper);
	}

	header {
		display: grid;
		gap: 8px;
		padding: 10px 10px 10px 12px;
		border-bottom: 1px solid var(--line);
	}

	h2 {
		margin: 0;
		padding-right: 36px;
		color: var(--text);
		font: var(--t-title);
	}

	.table {
		overflow: auto;
		min-width: 0;
		min-height: 0;
		font: var(--t-mono);
		scrollbar-width: thin;
	}

	.row-grid {
		display: grid;
		grid-template-columns: var(--tag-grid-columns);
		gap: 8px;
		align-items: center;
		min-width: 0;
		padding: 0 12px;
	}

	.header-row {
		position: sticky;
		top: 0;
		z-index: 2;
		height: 26px;
		background: var(--surface);
		border-bottom: 1px solid var(--line);
	}

	.header-cell {
		position: relative;
		min-width: 0;
		overflow: hidden;
		color: var(--ink-muted);
		font: var(--t-micro);
		letter-spacing: 0.06em;
		text-transform: uppercase;
		white-space: nowrap;
		user-select: none;
	}

	.header-cell.resizable {
		overflow: visible;
		padding-right: 6px;
	}

	.column-resizer {
		position: absolute;
		right: -6px;
		top: -6px;
		bottom: -6px;
		width: 12px;
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
		top: 4px;
		bottom: 4px;
		width: 1px;
		background: var(--line);
		transform: translateX(-50%);
	}

	.column-resizer.dragging::after {
		background: var(--accent);
	}

	.row {
		min-height: var(--row-h);
		border-bottom: 1px solid var(--surface);
		color: var(--text);
		text-align: left;
		cursor: pointer;
	}

	.row:hover {
		background: var(--row-hover);
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
		gap: 4px;
		align-items: center;
		padding-left: calc(var(--depth) * 12px);
		color: var(--ink-muted);
		font-variant-numeric: tabular-nums;
	}

	.nested .tag-cell {
		box-shadow: inset 1px 0 var(--line);
		padding-left: calc(var(--depth) * 12px + 6px);
	}

	.tag-cell span,
	.keyword-cell,
	.vr-cell {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.vr-cell {
		color: var(--ink-muted);
		font-size: 11px;
	}

	.chevron {
		display: grid;
		place-items: center;
		flex: none;
		width: 14px;
		height: 14px;
		border: 0;
		border-radius: var(--radius-sm);
		padding: 0;
		background: transparent;
		color: var(--ink-muted);
		cursor: pointer;
	}

	.chevron:hover {
		color: var(--text);
	}

	.value-cell {
		position: relative;
		min-width: 0;
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
		cursor: inherit;
	}

	.note {
		color: var(--ink-muted);
	}

	.value-error {
		display: flex;
		align-items: center;
		gap: 4px;
		color: var(--red-text);
	}

	.value-error::before {
		content: "";
		flex: none;
		width: 12px;
		height: 12px;
		background: var(--status-negative);
		mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='black' stroke-width='1.6' stroke-linecap='round'%3E%3Cpath d='M5 5l6 6M11 5l-6 6'/%3E%3C/svg%3E") center / contain no-repeat;
		-webkit-mask: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 16 16' fill='none' stroke='black' stroke-width='1.6' stroke-linecap='round'%3E%3Cpath d='M5 5l6 6M11 5l-6 6'/%3E%3C/svg%3E") center / contain no-repeat;
	}

	/* Copy confirmation floats over the value instead of reserving room for it. */
	.copied {
		position: absolute;
		right: 0;
		top: 50%;
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 0 6px;
		transform: translateY(-50%);
		border-radius: var(--radius-sm);
		background: var(--paper);
		box-shadow: -8px 0 8px var(--paper);
		color: var(--text);
		font: 600 11px/18px var(--font-ui);
		white-space: nowrap;
		pointer-events: none;
		animation: copied-pop var(--settle) var(--ease-spring);
	}

	.copied :global(.icon) {
		stroke: var(--status-positive);
	}

	@keyframes copied-pop {
		from { opacity: 0; transform: translateY(-50%) scale(0.6); }
		to { opacity: 1; transform: translateY(-50%) scale(1); }
	}

	.error,
	.loading {
		padding: 12px;
		color: var(--ink-muted);
		font: var(--t-meta);
	}

	.error {
		display: grid;
		justify-items: start;
		gap: 8px;
		color: var(--red-text);
	}
</style>

<script lang="ts">
	import type { FileSummary } from "../api";
	import {
		activeDirectoryPathKeys,
		activeStudyPathKeys,
		buildDirectoryTree,
		buildFileTree,
		directoryFileOrder,
		fileAriaLabel,
		fileMatchesFilter,
		nodeAriaLabel,
		patientDetailWithCounts,
		seriesDetailWithCounts,
		studyDetailWithCounts,
		studyFileOrder,
		tierLabel,
		type NavKind,
		type DirectoryNode,
	} from "./fileTree";
	import Button from "./ui/Button.svelte";
	import SearchField from "./ui/SearchField.svelte";
	import SegmentedControl from "./ui/SegmentedControl.svelte";

	let {
		files,
		activeFileIndex,
		scanComplete = true,
		collapsed = $bindable(),
		onopenfile,
		onnavigationorderchange,
	}: {
		files: FileSummary[];
		activeFileIndex: number | null;
		scanComplete?: boolean;
		collapsed: boolean;
		onopenfile: (index: number) => void;
		onnavigationorderchange?: (order: number[]) => void;
	} = $props();

	const LARGE_TREE_COLLAPSE_THRESHOLD = 500;
	let collapsedNodes = $state<Record<string, boolean>>({});
	let filterQuery = $state("");
	let viewMode = $state<"study" | "directory">("study");
	const VIEW_OPTIONS: { value: "study" | "directory"; label: string }[] = [
		{ value: "study", label: "Study" },
		{ value: "directory", label: "Directory" },
	];

	function defaultCollapsed(key: string): boolean {
		if (filterActive) {
			return false;
		}
		const scaleCollapseActive = !scanComplete || files.length > LARGE_TREE_COLLAPSE_THRESHOLD;
		if (!scaleCollapseActive) {
			return false;
		}
		if (!key.includes("/")) {
			return tree.length > 1;
		}
		return key.includes("/study:") || key.includes("/series:");
	}

	function isCollapsed(key: string): boolean {
		if (filterActive) {
			return false;
		}
		return collapsedNodes[key] ?? defaultCollapsed(key);
	}

	function toggleNode(key: string) {
		collapsedNodes = { ...collapsedNodes, [key]: !isCollapsed(key) };
	}

	function directoryFileCount(node: DirectoryNode): number {
		return node.kind === "file"
			? 1
			: node.children.reduce((total, child) => total + directoryFileCount(child), 0);
	}

	const filterActive = $derived(filterQuery.trim().length > 0);
	const filteredFiles = $derived.by(() => {
		if (!filterActive) return files;
		return files.filter((file) => fileMatchesFilter(file, filterQuery));
	});

	const tree = $derived(buildFileTree(filteredFiles));
	const directoryTree = $derived(buildDirectoryTree(filteredFiles));
	const activeStudyPath = $derived(activeStudyPathKeys(tree, activeFileIndex));
	const activeDirectoryPath = $derived(activeDirectoryPathKeys(directoryTree, activeFileIndex));
	const navigationOrder = $derived(
		viewMode === "study" ? studyFileOrder(tree) : directoryFileOrder(directoryTree),
	);

	$effect(() => {
		onnavigationorderchange?.(navigationOrder);
	});
</script>

{#snippet nodeContent(kind: NavKind, label: string, detail: string)}
	<span class="kind-badge">{tierLabel(kind)}</span>
	<span class="node-text">
		<span class="node-label">{label}</span>
		{#if detail}<span class="node-detail">{detail}</span>{/if}
	</span>
{/snippet}

{#snippet directoryNodes(nodes: DirectoryNode[], depth: number)}
	{#each nodes as node}
		{#if node.kind === "folder"}
			<button
				type="button"
				class="directory-row folder-row"
				class:active-path={activeDirectoryPath.has(node.key)}
				style:--depth={depth}
				aria-expanded={!isCollapsed(node.key)}
				onclick={() => toggleNode(node.key)}
			>
				<span class="folder-icon" aria-hidden="true"></span>
				<span class="directory-label">{node.label}</span>
				<span class="directory-count">{directoryFileCount(node)}</span>
				<span class="folder-twisty">{isCollapsed(node.key) ? "▶" : "▼"}</span>
			</button>
			{#if !isCollapsed(node.key)}
				{@render directoryNodes(node.children, depth + 1)}
			{/if}
		{:else}
			<button
				type="button"
				class="directory-row directory-file"
				class:active={node.file.index === activeFileIndex}
				aria-current={node.file.index === activeFileIndex ? "true" : undefined}
				style:--depth={depth}
				title={node.file.path}
				onclick={() => onopenfile(node.file.index)}
			>
				<span class="file-icon" aria-hidden="true">DCM</span>
				<span class="directory-text">
					<span class="directory-label">{node.label}</span>
					<span class="directory-detail">{node.detail}</span>
				</span>
			</button>
		{/if}
	{/each}
{/snippet}

<aside class="navigator" class:collapsed>
	<div class="navigator-header">
		{#if !collapsed}
			<div class="header-copy"><strong>Explorer</strong><span>{files.length} images</span></div>
		{/if}
		<span class="collapse-button">
			<Button
				variant="ghost"
				icon="panel-left"
				onclick={() => collapsed = !collapsed}
				aria-label={collapsed ? "Expand file navigator" : "Collapse file navigator"}
				aria-expanded={!collapsed}
			/>
		</span>
	</div>

	{#if !collapsed}
		<div class="view-switch">
			<SegmentedControl
				fill
				label="Explorer organization"
				options={VIEW_OPTIONS}
				value={viewMode}
				onchange={(mode) => viewMode = mode}
			/>
		</div>
		<div class="navigator-filter">
			<SearchField
				bind:value={filterQuery}
				placeholder="patient, study, series, modality"
				aria-label="Filter file hierarchy"
			/>
			{#if filterActive}
				<div class="filter-result">showing {filteredFiles.length} of {files.length} images</div>
			{/if}
			{#if !scanComplete}
				<div class="scan-progress">indexed {files.length} file{files.length === 1 ? "" : "s"}...</div>
			{/if}
		</div>
		{#if viewMode === "study"}
		<div class="tree study-tree" role="tree" aria-label="DICOM file hierarchy">
			{#each tree as patient}
				{@const patientDetail = patientDetailWithCounts(patient)}
				<section class="tree-group">
					<button
						type="button"
						class="tree-header depth-0"
						class:active-path={activeStudyPath.has(patient.key)}
						aria-label={nodeAriaLabel(patient.kind, patient.label, patientDetail, isCollapsed(patient.key))}
						aria-expanded={!isCollapsed(patient.key)}
						onclick={() => toggleNode(patient.key)}
					>
						<span class="twisty">{isCollapsed(patient.key) ? "▶" : "▼"}</span>
						{@render nodeContent(patient.kind, patient.label, patientDetail)}
					</button>
					{#if !isCollapsed(patient.key)}
						{#each patient.studies as study}
							{@const studyDetail = studyDetailWithCounts(study)}
							<div class="study-sibling">
							<button
								type="button"
								class="tree-header depth-1"
								class:active-path={activeStudyPath.has(study.key)}
								aria-label={nodeAriaLabel(study.kind, study.label, studyDetail, isCollapsed(study.key))}
								aria-expanded={!isCollapsed(study.key)}
								onclick={() => toggleNode(study.key)}
							>
								<span class="twisty">{isCollapsed(study.key) ? "▶" : "▼"}</span>
								{@render nodeContent(study.kind, study.label, studyDetail)}
							</button>
							{#if !isCollapsed(study.key)}
								{#each study.series as series}
									{@const seriesDetail = seriesDetailWithCounts(series)}
									<div class="series-sibling">
									<button
										type="button"
										class="tree-header depth-2"
										class:active-path={activeStudyPath.has(series.key)}
										aria-label={nodeAriaLabel(series.kind, series.label, seriesDetail, isCollapsed(series.key))}
										aria-expanded={!isCollapsed(series.key)}
										onclick={() => toggleNode(series.key)}
									>
										<span class="twisty">{isCollapsed(series.key) ? "▶" : "▼"}</span>
										{@render nodeContent(series.kind, series.label, seriesDetail)}
									</button>
									{#if !isCollapsed(series.key)}
										{#each series.files as item}
											<button
												type="button"
												data-capture-file-index={item.file.index}
												class="file-row depth-3"
												class:active={item.file.index === activeFileIndex}
												aria-current={item.file.index === activeFileIndex ? "true" : undefined}
												onclick={() => onopenfile(item.file.index)}
												title={item.file.path}
												aria-label={fileAriaLabel(item)}
											>
												{@render nodeContent(item.kind, item.label, item.detail)}
											</button>
										{/each}
									{/if}
									</div>
								{/each}
							{/if}
							</div>
						{/each}
					{/if}
				</section>
			{/each}
		</div>
		{:else}
			<div class="tree directory-tree" role="tree" aria-label="Directory file hierarchy">
				{@render directoryNodes(directoryTree, 0)}
			</div>
		{/if}
	{/if}
</aside>

<style>
	.navigator {
		display: grid;
		grid-template-rows: auto auto auto 1fr;
		min-width: 0;
		min-height: 0;
		background: var(--surface-panel);
		border-right: 1px solid var(--border-subtle);
		overflow: hidden;
	}

	.navigator.collapsed {
		background: var(--surface-chrome);
	}

	.navigator-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem;
		min-height: 2.5rem;
		padding: 0.55rem 0.65rem;
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: 0.82rem;
		font-weight: 650;
	}

	.header-copy { display: grid; gap: 0.08rem; }
	.header-copy strong { color: var(--text-primary); font-size: 0.82rem; }
	.header-copy span { color: var(--text-muted); font-size: 0.64rem; font-weight: 500; }

	.view-switch {
		padding: 10px 10px 0;
	}


	.tree-header:focus-visible,
	.file-row:focus-visible,
	.directory-row:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: -2px;
	}

	.navigator-filter {
		display: grid;
		gap: 0.35rem;
		padding: 0.5rem 0.65rem;
		border-bottom: 1px solid var(--border-subtle);
	}


	.filter-result,
	.scan-progress {
		color: var(--text-muted);
		font-size: 0.72rem;
		line-height: 1.25;
	}

	.tree {
		overflow: auto;
		padding: 0.4rem 0;
		scrollbar-width: thin;
	}

	.study-tree { padding: 0.5rem 0.55rem 0.8rem; }
	.study-tree .tree-group {
		margin-bottom: 0.55rem;
		padding-bottom: 0.34rem;
		border: 1px solid var(--border-subtle);
		border-left: 3px solid var(--border-subtle);
		border-radius: 0.52rem;
		background: transparent;
		overflow: hidden;
	}

	.tree-group,
	.tree-header,
	.file-row {
		min-width: 0;
	}

	.tree-header,
	.file-row {
		width: 100%;
		border: 0;
		background: transparent;
		color: var(--text-secondary);
		text-align: left;
		cursor: pointer;
	}

	.tree-header {
		display: grid;
		grid-template-columns: 1.1rem 3.35rem minmax(0, 1fr);
		align-items: start;
		gap: 0.35rem;
		padding-top: 0.28rem;
		padding-bottom: 0.28rem;
		font-size: 0.81rem;
	}

	.file-row {
		display: grid;
		grid-template-columns: 3.35rem minmax(0, 1fr);
		align-items: start;
		gap: 0.35rem;
		padding-top: 0.26rem;
		padding-bottom: 0.26rem;
		font-size: 0.8rem;
	}

	.tree-header:hover,
	.file-row:hover {
		background: rgba(255, 255, 255, 0.05);
	}
	.study-tree .depth-0 {
		min-height: 2.75rem;
		padding-top: 0.42rem;
		padding-bottom: 0.42rem;
		background: transparent;
	}
	.study-tree .depth-0.active-path {
		background: color-mix(in srgb, var(--accent) 6%, transparent);
	}

	.study-sibling {
		margin: 0 0.34rem;
		border: 1px solid var(--border-subtle);
		border-left: 3px solid var(--border-subtle);
		border-radius: 0.36rem;
		background: transparent;
		overflow: hidden;
	}
	.study-sibling + .study-sibling { margin-top: 0.34rem; }

	.series-sibling {
		margin: 0 0.32rem 0.3rem;
		border: 1px solid var(--border-subtle);
		border-left: 3px solid var(--border-subtle);
		border-radius: 0.3rem;
		background: transparent;
		overflow: hidden;
	}
	.series-sibling + .series-sibling { margin-top: 0.3rem; }
	.study-tree .depth-1,
	.study-tree .depth-2 { border-left: 0; }
	.study-tree .depth-1.active-path {
		background: color-mix(in srgb, var(--accent) 9%, transparent);
	}
	.study-tree .depth-2.active-path {
		background: color-mix(in srgb, var(--accent) 12%, transparent);
	}
	.study-tree .tree-header.active-path .node-label {
		color: var(--text-primary);
	}

	.file-row.active {
		background: var(--accent-soft);
		color: var(--text-primary);
		box-shadow: inset 3px 0 0 var(--accent);
	}

	.depth-0 { padding-left: 0.48rem; }
	.depth-1 { padding: 0.36rem 0.5rem; }
	.depth-2 { padding: 0.32rem 0.42rem; }
	.depth-3 { padding-left: 2.62rem; padding-right: 0.5rem; }

	.twisty {
		align-self: center;
		color: var(--text-muted);
		font-size: 0.72rem;
		line-height: 1.35;
	}

	.kind-badge {
		display: block;
		align-self: center;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-muted);
		font-size: 0.6rem;
		font-weight: 700;
		letter-spacing: 0.04em;
		line-height: 1.45;
		text-transform: uppercase;
	}

	.node-text {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.node-text {
		display: grid;
		gap: 0.04rem;
		line-height: 1.25;
	}

	.node-label {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-secondary);
	}

	.file-row.active .node-label {
		color: var(--text-primary);
	}

	.node-detail {
		color: var(--text-muted);
		font-size: 0.72rem;
	}

	.directory-tree { padding: 0.45rem 0.55rem 0.8rem; }
	.directory-row {
		display: grid;
		align-items: center;
		gap: 0.35rem;
		width: 100%;
		min-height: 2rem;
		padding-left: calc(0.3rem + var(--depth) * 0.72rem);
		border: 0;
		border-radius: 0.3rem;
		background: transparent;
		color: var(--text-secondary);
		font: 0.77rem var(--font-ui);
		text-align: left;
		cursor: pointer;
	}
	.directory-row:hover { background: rgba(255, 255, 255, 0.05); }
	.directory-row.active-path {
		background: color-mix(in srgb, var(--accent) 9%, transparent);
		color: var(--text-primary);
		box-shadow: inset 2px 0 0 color-mix(in srgb, var(--accent) 58%, transparent);
	}
	.directory-row.active { background: var(--accent-soft); box-shadow: inset 3px 0 var(--accent); color: var(--text-primary); }
	.folder-row {
		grid-template-columns: 1rem minmax(0, 1fr) auto 0.65rem;
		padding-top: 0.25rem;
		padding-right: 0.35rem;
		padding-bottom: 0.25rem;
	}
	.directory-file {
		grid-template-columns: 1.75rem minmax(0, 1fr);
		align-items: center;
		padding-top: 0.34rem;
		padding-right: 0.4rem;
		padding-bottom: 0.34rem;
		padding-left: calc(0.42rem + var(--depth) * 0.72rem);
	}
	.folder-twisty { color: var(--text-muted); font-size: 0.55rem; text-align: center; }
	.folder-icon {
		position: relative;
		display: block;
		width: 0.92rem;
		height: 0.65rem;
		/* Account for the tab extending above the icon's layout box. */
		transform: translateY(0.1rem);
		border-radius: 0.12rem;
		background: #72879e;
	}
	.folder-icon::before { content: ""; position: absolute; left: 0.08rem; top: -0.2rem; width: 0.42rem; height: 0.25rem; border-radius: 0.1rem 0.1rem 0 0; background: #72879e; }
	.file-icon {
		display: grid;
		place-items: center;
		width: 1.55rem;
		height: 1.1rem;
		border: 1px solid rgba(126, 179, 236, 0.28);
		border-radius: 0.18rem;
		background: rgba(64, 124, 186, 0.13);
		color: #8cb9e9;
		font: 800 0.46rem var(--font-ui);
		letter-spacing: 0.03em;
	}
	.directory-label { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.directory-text { display: grid; gap: 0.04rem; min-width: 0; line-height: 1.25; }
	.directory-detail { color: var(--text-muted); font-size: 0.68rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.folder-row .directory-label { font-weight: 600; }
	.directory-count { min-width: 1.25rem; padding: 0.08rem 0.28rem; border-radius: 99px; background: rgba(255,255,255,0.06); color: var(--text-muted); font-size: 0.62rem; text-align: center; }
</style>

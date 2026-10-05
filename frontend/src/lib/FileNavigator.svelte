<script lang="ts">
	import type { FileSummary } from "../api";
	import {
		activeDirectoryPathKeys,
		activeStudyPathKeys,
		buildDirectoryTree,
		buildFileTree,
		buildImageGroup,
		clinicalFiles,
		directoryFileOrder,
		fileAriaLabel,
		fileKindLabel,
		filterFiles,
		imageGroupAriaLabel,
		nodeAriaLabel,
		patientDetailWithCounts,
		seriesDetailWithCounts,
		studyDetailWithCounts,
		studyFileOrder,
		type DirectoryNode,
	} from "./fileTree";
	import { fileIcon } from "./objectIcons";
	import { unsupportedImageReason } from "./rasterSupport";
	import Button from "./ui/Button.svelte";
	import Icon from "./ui/Icon.svelte";
	import StatusBadge from "./ui/StatusBadge.svelte";
	import type { IconName } from "./ui/icons";
	import SearchField from "./ui/SearchField.svelte";
	import SegmentedControl from "./ui/SegmentedControl.svelte";

	let {
		files,
		activeFileIndex,
		scanComplete = true,
		masked = false,
		viewMode = $bindable("study"),
		collapsed = $bindable(),
		onopenfile,
		onnavigationorderchange,
	}: {
		files: FileSummary[];
		activeFileIndex: number | null;
		scanComplete?: boolean;
		/** A masked session: only the directory tree shows real file names. */
		masked?: boolean;
		viewMode?: "study" | "directory";
		collapsed: boolean;
		onopenfile: (index: number) => void;
		onnavigationorderchange?: (order: number[]) => void;
	} = $props();

	const LARGE_TREE_COLLAPSE_THRESHOLD = 500;
	let collapsedNodes = $state<Record<string, boolean>>({});
	let filterQuery = $state("");
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
			return tree.length + (imageGroup ? 1 : 0) > 1;
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
	// A masked session shows real folder and file names only in the directory
	// tree, so only there do they label, group or match anything.
	const pathsShown = $derived(!masked || viewMode === "directory");
	const filteredFiles = $derived.by(() => {
		if (!filterActive) return files;
		return filterFiles(files, filterQuery, { searchPaths: pathsShown });
	});

	// Study view: DICOM in the clinical tree, raster files in "Images" after it.
	const tree = $derived(buildFileTree(clinicalFiles(filteredFiles)));
	const imageGroup = $derived(buildImageGroup(filteredFiles, { folders: !masked }));
	const directoryTree = $derived(buildDirectoryTree(filteredFiles));
	const activeStudyPath = $derived(activeStudyPathKeys(tree, activeFileIndex));
	const imageGroupOrder = $derived(directoryFileOrder(imageGroup?.children ?? []));
	const activeDirectoryPath = $derived(activeDirectoryPathKeys(
		viewMode === "study" ? imageGroup?.children ?? [] : directoryTree,
		activeFileIndex,
	));
	const navigationOrder = $derived(
		viewMode === "study" ? [...studyFileOrder(tree), ...imageGroupOrder] : directoryFileOrder(directoryTree),
	);

	$effect(() => {
		onnavigationorderchange?.(navigationOrder);
	});
</script>

{#snippet twisty(collapsedNode: boolean)}
	<span class="twisty"><Icon name={collapsedNode ? "chevron-right" : "chevron-down"} size={14} /></span>
{/snippet}

{#snippet fileState(file: FileSummary, detail: string)}
	{#if file.support_state === "unsupported"}
		<StatusBadge status="negative" title={unsupportedImageReason(file) ?? file.support_reason ?? undefined}>Unsupported</StatusBadge>
	{:else if !file.has_pixels}
		<StatusBadge status="unknown">No pixels</StatusBadge>
	{:else if detail}
		<span class="node-detail">{detail}</span>
	{/if}
{/snippet}

{#snippet nodeContent(icon: IconName, label: string, detail: string, file?: FileSummary)}
	<span class="tier"><Icon name={icon} size={14} /></span>
	<span class="node-text">
		<span class="node-label">{label}</span>
		{#if file}
			{@render fileState(file, detail)}
		{:else if detail}
			<span class="node-detail">{detail}</span>
		{/if}
	</span>
{/snippet}

{#snippet directoryNodes(nodes: DirectoryNode[], depth: number)}
	{#each nodes as node}
		{#if node.kind === "folder"}
			{@const holdsActive = node.children.some((child) => child.kind === "file" && child.file.index === activeFileIndex)}
			<div class="folder" class:root={depth === 0}>
				<button
					type="button"
					class="directory-row folder-row"
					class:active-path={activeDirectoryPath.has(node.key)}
					aria-expanded={!isCollapsed(node.key)}
					onclick={() => toggleNode(node.key)}
				>
					{@render twisty(isCollapsed(node.key))}
					<span class="tier"><Icon name="directory" size={14} /></span>
					<span class="directory-label">{node.label}</span>
					<span class="directory-count">{directoryFileCount(node)}</span>
				</button>
				{#if !isCollapsed(node.key)}
					<div class="directory-children" class:current={holdsActive} role="group">
						{@render directoryNodes(node.children, depth + 1)}
					</div>
				{/if}
			</div>
		{:else}
			<button
				type="button"
				class="directory-row directory-file"
				class:active={node.file.index === activeFileIndex}
				class:unsupported={node.file.support_state === "unsupported"}
				class:dim={!node.file.has_pixels}
				aria-current={node.file.index === activeFileIndex ? "true" : undefined}
				title={pathsShown ? node.file.path : node.file.display_name}
				onclick={() => onopenfile(node.file.index)}
			>
				<span class="tier"><Icon name={fileIcon(node.file)} size={14} /></span>
				<span class="directory-label">{node.label}</span>
				{#if fileKindLabel(node.file)}<span class="modality">{fileKindLabel(node.file)}</span>{/if}
				<span class="directory-detail">{@render fileState(node.file, node.detail)}</span>
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
				placeholder="patient, study, series, modality, format"
				aria-label="Filter file hierarchy"
			/>
			{#if filterActive}
				<div class="filter-result">showing {filteredFiles.length} of {files.length} images</div>
			{/if}
			{#if !scanComplete}
				<div class="scan-progress">indexed {files.length} file{files.length === 1 ? "" : "s"}...</div>
			{/if}
			{#if masked && viewMode === "directory"}
				<div class="unmasked-names" role="note">
					<StatusBadge status="partial">Not masked</StatusBadge>
					<span>Folder and file names are shown as they are on disk.</span>
				</div>
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
						{@render twisty(isCollapsed(patient.key))}
						{@render nodeContent("patient", patient.label, patientDetail)}
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
								{@render twisty(isCollapsed(study.key))}
								{@render nodeContent("study", study.label, studyDetail)}
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
										{@render twisty(isCollapsed(series.key))}
										{@render nodeContent("series", series.label, seriesDetail)}
									</button>
									{#if !isCollapsed(series.key)}
										<div class="series-files" role="group">
										{#each series.files as item}
											<button
												type="button"
												data-capture-file-index={item.file.index}
												class="file-row depth-3"
												class:active={item.file.index === activeFileIndex}
												class:unsupported={item.file.support_state === "unsupported"}
												class:dim={!item.file.has_pixels}
												aria-current={item.file.index === activeFileIndex ? "true" : undefined}
												onclick={() => onopenfile(item.file.index)}
												title={masked ? item.file.display_name : item.file.path}
												aria-label={fileAriaLabel(item)}
											>
												{@render nodeContent(fileIcon(item.file), item.label, item.detail, item.file)}
											</button>
										{/each}
										</div>
									{/if}
									</div>
								{/each}
							{/if}
							</div>
						{/each}
					{/if}
				</section>
			{/each}
			{#if imageGroup}
				<section class="tree-group">
					<button
						type="button"
						class="tree-header depth-0"
						class:active-path={activeFileIndex !== null && imageGroupOrder.includes(activeFileIndex)}
						aria-label={imageGroupAriaLabel(imageGroup, isCollapsed(imageGroup.key))}
						aria-expanded={!isCollapsed(imageGroup.key)}
						onclick={() => toggleNode(imageGroup.key)}
					>
						{@render twisty(isCollapsed(imageGroup.key))}
						{@render nodeContent("image", imageGroup.label, imageGroup.detail)}
					</button>
					{#if !isCollapsed(imageGroup.key)}
						<div class="image-folders" role="group">
							{@render directoryNodes(imageGroup.children, 1)}
						</div>
					{/if}
				</section>
			{/if}
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
		background: var(--paper);
		border-right: 1px solid var(--line);
		overflow: hidden;
	}

	.navigator.collapsed {
		background: var(--surface);
	}

	.navigator-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		min-height: 44px;
		padding: 8px 8px 6px 12px;
		box-sizing: border-box;
	}

	.header-copy {
		display: flex;
		align-items: baseline;
		gap: 8px;
		min-width: 0;
	}

	.header-copy strong {
		color: var(--text);
		font: var(--t-title);
	}

	.header-copy span {
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
		white-space: nowrap;
	}

	.view-switch {
		padding: 0 10px;
	}

	.navigator-filter {
		display: grid;
		gap: 6px;
		padding: 8px 10px 10px;
		border-bottom: 1px solid var(--line);
	}

	.filter-result,
	.scan-progress {
		color: var(--ink-muted);
		font: var(--t-meta);
		font-size: 11px;
	}

	.unmasked-names {
		display: flex;
		align-items: center;
		gap: 8px;
		color: var(--ink-muted);
		font: var(--t-meta);
		font-size: 11px;
	}

	.tree {
		display: flex;
		flex-direction: column;
		gap: 10px;
		overflow: auto;
		padding: 10px;
		scrollbar-width: thin;
	}

	/* Encapsulation shows the DICOM hierarchy: patient card, study box, then rows. */
	.tree-group {
		flex: none;
		border: 1px solid var(--ink);
		border-radius: var(--radius-lg);
		background: var(--paper);
		padding-bottom: 6px;
	}

	.study-sibling {
		margin: 0 6px;
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		padding-bottom: 4px;
	}

	.study-sibling + .study-sibling {
		margin-top: 6px;
	}

	.series-sibling {
		margin: 0 4px;
	}

	.image-folders {
		display: flex;
		flex-direction: column;
		margin: 0 6px;
	}

	.series-files {
		margin: 0 0 2px 13px;
		padding-left: 8px;
		border-left: 1px solid var(--line);
	}

	.tree-header,
	.file-row,
	.directory-row {
		display: grid;
		align-items: center;
		gap: 6px;
		width: 100%;
		min-width: 0;
		padding: 3px 8px 3px 6px;
		border: 1px solid transparent;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text);
		font: var(--t-ui);
		text-align: left;
		cursor: pointer;
	}

	.tree-header {
		grid-template-columns: 14px 14px minmax(0, 1fr);
		min-height: 32px;
	}

	.file-row {
		grid-template-columns: 14px minmax(0, 1fr);
		min-height: 32px;
	}

	.tree-header:hover,
	.file-row:hover,
	.directory-row:hover {
		background: var(--row-hover);
	}

	.tree-header:focus-visible,
	.file-row:focus-visible,
	.directory-row:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: -2px;
	}

	.depth-0 .node-label {
		font-weight: 700;
	}

	.depth-1 .node-label {
		font-weight: 600;
	}

	.tree-header.active-path .node-label,
	.folder-row.active-path .directory-label {
		font-weight: 700;
	}

	.file-row.active,
	.directory-row.active {
		border-color: var(--selection-edge);
		background: var(--selection-fill);
	}

	.file-row.unsupported,
	.directory-row.unsupported,
	.file-row.unsupported:hover,
	.directory-row.unsupported:hover {
		border: 1px dashed var(--red);
		background: var(--red-wash);
	}

	/* Every raster is unsupported for now: the open one still reads as selected. */
	.file-row.unsupported.active,
	.directory-row.unsupported.active {
		background: var(--selection-fill);
	}

	.dim .node-label,
	.dim .directory-label {
		color: var(--ink-muted);
	}

	.twisty,
	.tier {
		display: grid;
		place-items: center;
		align-self: start;
		height: 18px;
		color: var(--ink-muted);
	}

	.node-text {
		display: grid;
		justify-items: start;
		gap: 2px;
		min-width: 0;
	}

	.node-label {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.file-row .node-label {
		font: var(--t-mono);
		line-height: 18px;
	}

	.node-detail {
		max-width: 100%;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
		font-variant-numeric: tabular-nums;
	}

	/* Directory mode: folders are rows on one rail per level; the rail holding the open file is ink. */
	.folder.root {
		flex: none;
		border: 1px solid var(--ink);
		border-radius: var(--radius-lg);
		background: var(--paper);
		padding-bottom: 4px;
	}

	.folder-row {
		grid-template-columns: 14px 14px minmax(0, 1fr) auto;
		min-height: 28px;
	}

	.folder-row .directory-label {
		font: var(--t-mono);
		line-height: 18px;
	}

	.folder.root > .folder-row .directory-label {
		font-weight: 700;
	}

	.directory-children {
		display: flex;
		flex-direction: column;
		margin: 0 0 2px 13px;
		padding-left: 8px;
		border-left: 1px solid var(--line);
	}

	.directory-children.current {
		border-left-color: var(--ink);
	}

	.directory-count {
		color: var(--ink-muted);
		font: 400 11px/14px var(--font-mono);
		font-variant-numeric: tabular-nums;
	}

	.directory-file {
		grid-template-columns: 14px minmax(0, 1fr) auto;
		min-height: 36px;
		row-gap: 2px;
	}

	.directory-file .tier {
		grid-row: 1 / span 2;
	}

	.directory-file .directory-label {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font: var(--t-mono);
	}

	.directory-detail {
		grid-column: 2 / span 2;
		display: flex;
		min-width: 0;
	}

	.modality {
		padding: 0 4px;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		color: var(--ink-2);
		font: 500 10px/14px var(--font-mono);
		letter-spacing: 0.02em;
	}
</style>

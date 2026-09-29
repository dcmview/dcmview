<script lang="ts">
	import { onMount } from "svelte";
	import { annotationsExportUrl, fetchHealth, onReachabilityChange, type SemanticContextResponse } from "./api";
	import FileNavigator from "./lib/FileNavigator.svelte";
	import FrameSlider from "./lib/FrameSlider.svelte";
	import ImageViewport from "./lib/ImageViewport.svelte";
	import OpenImageTabs from "./lib/OpenImageTabs.svelte";
	import ReferenceNavigator from "./lib/ReferenceNavigator.svelte";
	import SemanticContextPanel from "./lib/SemanticContextPanel.svelte";
	import {
		segmentationOverlaySelection,
		supportsSemanticContext,
		type SemanticMode,
	} from "./lib/semanticPresentation";
	import StatusBar from "./lib/StatusBar.svelte";
	import TagPanel from "./lib/TagPanel.svelte";
	import ValueOverlayBar from "./lib/ValueOverlayBar.svelte";
	import ViewerToolbar from "./lib/ViewerToolbar.svelte";
	import Button from "./lib/ui/Button.svelte";
	import WsiTileContext from "./lib/WsiTileContext.svelte";
	import { Catalog } from "./lib/app/catalog.svelte";
	import {
		SidebarLayout,
		TAG_PANEL_MAX_WIDTH_PX,
		TAG_PANEL_MIN_WIDTH_PX,
	} from "./lib/app/sidebarLayout.svelte";
	import { TabNavigation } from "./lib/app/tabNavigation.svelte";
	import { overlayEntryFrame, ValueOverlays } from "./lib/app/valueOverlays.svelte";
	import { WindowSettings } from "./lib/app/windowSettings.svelte";
	import type { CineDirection, CineMode } from "./lib/cinePlayback";
	import { resolveFilesById } from "./lib/fileRegistry";
	import { adjacentFileIndex } from "./lib/fileTree";
	import { REPEAT_INTERVAL_MS, RepeatThrottle, shortcutFor } from "./lib/keyboardShortcuts";
	import type { ActiveTool } from "./lib/viewerTools";
	import type { FrameOverlay } from "./lib/viewport/frameOverlay";
	import { ViewStates } from "./lib/viewport/viewStates.svelte";
	import {
		flipHorizontal,
		flipVertical,
		rotateClockwise,
		rotateCounterClockwise,
	} from "./lib/viewport/viewTransform";

	// App owns the shared root state; the controllers below hold its parts
	// and the components receive what they need as props.
	const catalog = new Catalog();
	const windowSettings = new WindowSettings((fileIndex) => catalog.filesById.get(fileIndex)?.default_window);
	const tabs = new TabNavigation({
		series: () => catalog.series?.series ?? [],
		files: () => catalog.filesById,
		ontabchange: resetCine,
		// A manual window follows the user onto the next file of a stack.
		onfilechange: (fileIndex) => windowSettings.followFile(fileIndex),
	});
	const layout = new SidebarLayout();
	// Zoom, pan, and orientation per open tab: the viewport zooms and pans,
	// the toolbar reorients.
	const viewStates = new ViewStates();
	const keyRepeats = new RepeatThrottle();
	// Value colorwashes (RT Dose, Parametric Map) over the images they cover.
	const valueOverlays = new ValueOverlays({
		files: () => catalog.filesById,
		series: () => catalog.series?.series ?? [],
		scanComplete: () => (catalog.files?.scan_complete ?? false) && (catalog.series?.scan_complete ?? false),
	});

	let activeTool = $state<ActiveTool>("pan");
	let cinePlaying = $state(false);
	let cineFps = $state(10);
	let cineMode = $state<CineMode>("loop");
	let cineDirection = $state<CineDirection>(1);
	let fileNavigationOrder = $state<number[]>([]);
	let semanticMode = $state<SemanticMode>("pixel_preview");
	let semanticResponse = $state<SemanticContextResponse | null>(null);
	let viewport = $state<ReturnType<typeof ImageViewport>>();
	let frameSlider = $state<ReturnType<typeof FrameSlider>>();
	let tagPanel = $state<ReturnType<typeof TagPanel>>();
	let references = $state<ReturnType<typeof ReferenceNavigator>>();
	let semanticPanel = $state<ReturnType<typeof SemanticContextPanel>>();
	let wsiContext = $state<ReturnType<typeof WsiTileContext>>();

	const activeFile = $derived(tabs.activeFile);
	const frameOverlay = $derived.by<FrameOverlay | null>(() => {
		if (semanticMode !== "semantic_context") return null;
		if (!semanticResponse || semanticResponse.source_file_index !== tabs.activeFileIndex) return null;
		const selection = segmentationOverlaySelection(semanticResponse, tabs.currentFrame);
		if (!selection) return null;
		const sourceFile = catalog.filesById.get(selection.sourceFileIndex);
		if (!sourceFile) return null;
		return { kind: "segmentation", ...selection, sourceFile };
	});
	const overlayCandidates = $derived(
		tabs.activeFileIndex === null ? [] : valueOverlays.candidatesFor(tabs.activeFileIndex, tabs.frames),
	);
	const valueOverlay = $derived(
		frameOverlay || tabs.activeFileIndex === null
			? null
			: valueOverlays.overlayFor(overlayCandidates, tabs.activeFileIndex, tabs.currentFrame),
	);
	const openTabFiles = $derived(resolveFilesById(catalog.filesById, tabs.tabs.map((tab) => tab.fileIndex)));

	/** A different tab starts paused and playing forward. */
	function resetCine() {
		cinePlaying = false;
		cineDirection = 1;
	}

	/** Opens a volume's source image with its colorwash shown. */
	function showValueOverlay(response: SemanticContextResponse) {
		const entry = overlayEntryFrame(response);
		if (!entry) return;
		valueOverlays.select(response.source_file_index);
		tabs.openReference(entry.fileIndex, entry.frameIndex);
	}

	function closeTab(fileIndex: number): void {
		const scope = tabs.tabs.find((tab) => tab.fileIndex === fileIndex)?.id;
		tabs.close(fileIndex);
		if (scope) viewStates.forget(scope);
	}

	function openFileFromNavigator(fileIndex: number) {
		tabs.open(fileIndex);
		layout.fileOpenedFromExplorer();
	}

	function updateFileNavigationOrder(order: number[]) {
		if (
			order.length === fileNavigationOrder.length
			&& order.every((fileIndex, position) => fileNavigationOrder[position] === fileIndex)
		) return;
		fileNavigationOrder = order;
	}

	function resetViewport() {
		if (tabs.activeFileIndex === null) return;
		windowSettings.reset();
		viewport?.resetView();
		viewStates.resetOrientation(tabs.scopeKey);
	}

	function reorient(change: typeof flipHorizontal) {
		if (tabs.activeFileIndex === null) return;
		viewStates.updateOrientation(tabs.scopeKey, change);
	}

	function exportAnnotations() {
		const link = document.createElement("a");
		link.href = annotationsExportUrl();
		link.download = "dcmview-annotations.csv";
		document.body.appendChild(link);
		link.click();
		link.remove();
	}

	/** The single global keyboard dispatcher; bindings live in keyboardShortcuts.ts. */
	function handleWindowKeydown(event: KeyboardEvent) {
		const action = shortcutFor(event, {
			drawerOpen: layout.compactDrawer !== null,
			multiFrame: activeFile !== null && tabs.frames.length > 1,
			roiToolActive: activeFile !== null && activeTool === "annotate_rect",
		});
		if (!action) return;
		switch (action.type) {
			case "close-drawer":
				event.preventDefault();
				layout.closeDrawer();
				return;
			case "select-adjacent-file": {
				const adjacent = adjacentFileIndex(fileNavigationOrder, tabs.activeFileIndex, action.step);
				if (adjacent === null) return;
				event.preventDefault();
				if (!keyRepeats.allow(event, REPEAT_INTERVAL_MS[action.type])) return;
				cinePlaying = false;
				tabs.open(adjacent);
				return;
			}
			case "select-tool":
				activeTool = action.tool;
				return;
			case "step-frame":
				event.preventDefault();
				if (!keyRepeats.allow(event, REPEAT_INTERVAL_MS[action.type])) return;
				frameSlider?.step(action.step);
				return;
			case "toggle-cine":
				event.preventDefault();
				frameSlider?.togglePlay();
				return;
			case "delete-roi":
				event.preventDefault();
				viewport?.deleteSelectedRoi();
				return;
		}
	}

	$effect(() => valueOverlays.load(tabs.activeFileIndex));

	let stopPolling: (() => void) | null = null;
	function loadCatalog(): void {
		stopPolling?.();
		catalog.loadError = null;
		stopPolling = catalog.poll(() => tabs.syncCatalog(catalog.files?.files[0]?.index ?? null));
	}
	async function retryServer(): Promise<void> {
		try {
			const health = await fetchHealth();
			if (catalog.files && health.server_start_ms !== catalog.files.server_start_ms) {
				window.location.reload();
				return;
			}
			loadCatalog();
			void viewport?.retryFailedLoads();
			tagPanel?.retryFailedLoads();
			references?.retryFailedLoads();
			semanticPanel?.retryFailedLoads();
			wsiContext?.retryFailedLoads();
			valueOverlays.retryFailedLoads(tabs.activeFileIndex);
		} catch { /* Reachability stays failed; Retry remains available. */ }
	}

	onMount(() => {
		loadCatalog();
		return () => stopPolling?.();
	});

	// One place says the server is gone, instead of every panel's own error;
	// cine stops rather than failing frame by frame.
	let serverReachable = $state(true);
	onMount(() => onReachabilityChange((reachable) => {
		serverReachable = reachable;
		if (!reachable) cinePlaying = false;
	}));
</script>

<svelte:window onkeydown={handleWindowKeydown} onresize={() => layout.viewportResized()} />

{#if catalog.loadError}
	<main class="error">
		<p>{catalog.loadError}</p>
		<Button onclick={loadCatalog}>Retry</Button>
	</main>
{:else if !catalog.files}
	<main class="loading">Loading dcmview…</main>
{:else}
	<main
		class="layout"
		style={`--file-nav-width:${layout.fileNavigatorWidthPx}px; --tag-panel-width:${layout.tagPanelWidth}px;`}
	>
		<header class="topbar">
			<img
				class="brand-mark"
				src="/assets/dcmview-icon.png"
				alt="dcmview"
			/>
			<span class="compact-sidebar-button explorer-drawer-button">
				<Button
					icon="panel-left"
					bind:element={layout.explorerButton}
					onclick={() => layout.toggleDrawer("explorer")}
					aria-label="Toggle Explorer drawer"
					aria-controls="file-navigator-panel"
					aria-expanded={layout.compactDrawer === "explorer"}
				>
					Explorer
				</Button>
			</span>
			<OpenImageTabs
				openFiles={openTabFiles}
				frameCounts={tabs.frameCounts}
				activeFileIndex={tabs.activeFileIndex}
				activePosition={tabs.stackPosition}
				onactivate={(fileIndex) => tabs.activate(fileIndex)}
				onclose={closeTab}
			/>
			<span class="compact-sidebar-button tags-drawer-button">
				<Button
					icon="panel-right"
					bind:element={layout.tagsButton}
					onclick={() => layout.toggleDrawer("tags")}
					aria-label="Toggle Tags drawer"
					aria-controls="tag-panel"
					aria-expanded={layout.compactDrawer === "tags"}
				>
					Tags
				</Button>
			</span>
		</header>
		<ViewerToolbar
			bind:activeTool
			selectedPresetId={windowSettings.presetId}
			onpresetchange={(presetId) => windowSettings.selectPreset(presetId)}
			onreset={resetViewport}
			onflipH={() => reorient(flipHorizontal)}
			onflipV={() => reorient(flipVertical)}
			onrotateCW={() => reorient(rotateClockwise)}
			onrotateCCW={() => reorient(rotateCounterClockwise)}
			onexportAnnotations={exportAnnotations}
		/>
		{#if layout.compactDrawer !== null}
			<button
				type="button"
				class="drawer-backdrop"
				onclick={() => layout.closeDrawer()}
				aria-label="Close sidebar drawer"
			></button>
		{/if}
		<section class="workspace">
			<div
				id="file-navigator-panel"
				class="file-navigator-shell"
				class:compact-open={layout.compactDrawer === "explorer"}
				bind:this={layout.explorerDrawer}
				tabindex="-1"
				role={layout.compactDrawer === "explorer" ? "dialog" : undefined}
				aria-modal={layout.compactDrawer === "explorer" ? "true" : undefined}
				aria-label="Explorer"
				onkeydown={(event) => layout.trapDrawerFocus(event)}
			>
				<FileNavigator
					files={catalog.files.files}
					activeFileIndex={tabs.activeFileIndex}
					scanComplete={catalog.files.scan_complete}
					bind:collapsed={layout.fileNavigatorCollapsed}
					onopenfile={openFileFromNavigator}
					onnavigationorderchange={updateFileNavigationOrder}
				/>
			</div>
			<section class="viewer-column">
				{#if activeFile === null}
					<div class="empty-viewer">Open a file from the sidebar</div>
				{:else}
					<div class="viewer-context">
						<div class="context-strip">
							{#if overlayCandidates.length > 0 && !frameOverlay}
								<ValueOverlayBar
									candidates={overlayCandidates}
									selectedVolume={valueOverlays.selectedVolume}
									opacity={valueOverlays.opacity}
									coversFrame={valueOverlay?.coversFrame ?? false}
									ontoggle={(volumeFileIndex) => valueOverlays.toggle(volumeFileIndex)}
									onopacity={(opacity) => valueOverlays.setOpacity(opacity)}
								/>
							{/if}
							<ReferenceNavigator bind:this={references}
								scanProgress={[catalog.files.files.length, catalog.files.scanned, catalog.files.skipped, catalog.files.filtered, catalog.files.scan_complete].join("|")}
								fileIndex={activeFile.index}
								files={catalog.files.files}
								onopenreference={(fileIndex, frameIndex) => tabs.openReference(fileIndex, frameIndex)}
							/>
						</div>
						{#if supportsSemanticContext(activeFile.object_kind, activeFile.sop_class_uid)}
							<SemanticContextPanel bind:this={semanticPanel}
								fileIndex={activeFile.index}
								currentFrame={tabs.currentFrame}
								files={catalog.files.files}
								onopenreference={(fileIndex, frameIndex) => tabs.openReference(fileIndex, frameIndex)}
								onmodechange={(mode) => { semanticMode = mode; }}
								oncontextchange={(response) => { semanticResponse = response; }}
								onshowoverlay={showValueOverlay}
							/>
						{/if}
						{#if activeFile.object_kind === "whole_slide_microscopy"}
							<WsiTileContext bind:this={wsiContext}
								fileIndex={activeFile.index}
								frame={tabs.currentFrame}
								files={catalog.files.files}
								onopenreference={(fileIndex, frameIndex) => tabs.openReference(fileIndex, frameIndex)}
							/>
						{/if}
					</div>
					<ImageViewport
						bind:this={viewport}
						{activeFile}
						currentFrame={tabs.currentFrame}
						windowCenter={windowSettings.center}
						windowWidth={windowSettings.width}
						windowUnit={windowSettings.unit}
						windowMode={windowSettings.mode}
						{activeTool}
						{viewStates}
						overlay={frameOverlay}
						{valueOverlay}
						bind:cinePlaying
						{cineFps}
						{cineMode}
						bind:cineDirection
						navigationFrameCount={tabs.frames.length}
						navigationFrames={tabs.frames}
						navigationScopeKey={tabs.scopeKey}
						navigationPosition={tabs.stackPosition}
						onnavigationchange={(position) => tabs.setStackPosition(position)}
						onreset={resetViewport}
						onmanualwindowlevel={(center, width, unit) => windowSettings.recordManual(tabs.activeFileIndex, center, width, unit)}
					/>
					<FrameSlider
						bind:this={frameSlider}
						totalFrames={tabs.frames.length}
						currentPosition={tabs.stackPosition}
						onpositionchange={(position) => tabs.setStackPosition(position)}
						bind:cinePlaying
						bind:cineFps
						bind:cineMode
						bind:cineDirection
					/>
				{/if}
			</section>
			<aside
				id="tag-panel"
				class="tag-panel-shell"
				class:collapsed={layout.tagPanelCollapsed}
				class:compact-open={layout.compactDrawer === "tags"}
				bind:this={layout.tagsDrawer}
				tabindex="-1"
				role={layout.compactDrawer === "tags" ? "dialog" : undefined}
				aria-modal={layout.compactDrawer === "tags" ? "true" : undefined}
				aria-label="DICOM tags"
				onkeydown={(event) => layout.trapDrawerFocus(event)}
			>
				<div
					class="sidebar-handle"
					class:dragging={layout.resizing !== null}
					class:disabled={layout.tagPanelCollapsed}
					role="separator"
					aria-label="Resize DICOM tag panel"
					aria-orientation="vertical"
					aria-valuemin={TAG_PANEL_MIN_WIDTH_PX}
					aria-valuemax={TAG_PANEL_MAX_WIDTH_PX}
					aria-valuenow={layout.tagPanelWidthPx}
					onpointerdown={(event) => layout.startTagPanelResize(event)}
					onpointermove={(event) => layout.moveTagPanelResize(event)}
					onpointerup={(event) => layout.endTagPanelResize(event)}
					onpointercancel={() => layout.cancelTagPanelResize()}
				></div>
				<span class="panel-toggle">
					<Button
						variant="ghost"
						icon="panel-right"
						onclick={() => layout.toggleTagPanel()}
						aria-label={layout.tagPanelCollapsed ? "Expand DICOM tag panel" : "Collapse DICOM tag panel"}
						aria-expanded={!layout.tagPanelCollapsed}
					/>
				</span>
				{#if !layout.tagPanelCollapsed}
					{#if activeFile === null}
						<div class="tag-empty">No file selected</div>
					{:else}
						<TagPanel bind:this={tagPanel} fileIndex={activeFile.index} />
					{/if}
				{/if}
			</aside>
		</section>
		<StatusBar
			serverStartMs={catalog.files.server_start_ms}
			fileCount={catalog.files.files.length}
			reachable={serverReachable}
			onretry={() => void retryServer()}
		/>
	</main>
{/if}

<style>
	:global(*) {
		box-sizing: border-box;
	}

	:global(html),
	:global(body) {
		margin: 0;
		padding: 0;
		width: 100%;
		height: 100%;
		overflow: hidden;
		font-family: var(--font-ui);
		background: var(--canvas);
		color: var(--text);
		-webkit-font-smoothing: antialiased;
		text-rendering: optimizeLegibility;
	}

	.layout {
		display: grid;
		grid-template-rows: auto auto 1fr auto;
		height: 100vh;
		width: 100%;
		overflow: hidden;
		background: var(--canvas);
	}

	.topbar {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		align-items: stretch;
		height: var(--tab-h);
		background: var(--surface);
		border-bottom: 1px solid var(--line);
	}

	.compact-sidebar-button {
		display: none;
		align-self: center;
	}

	.file-navigator-shell:focus-visible,
	.tag-panel-shell:focus-visible {
		outline: 2px solid var(--focus-ring);
		outline-offset: 2px;
	}

	.drawer-backdrop {
		position: fixed;
		inset: 0;
		z-index: 30;
		border: 0;
		background: var(--scrim);
		cursor: default;
	}

	.brand-mark {
		align-self: center;
		display: block;
		width: 22px;
		height: 22px;
		margin: 0 12px;
		border-radius: var(--radius-sm);
	}

	.workspace {
		display: grid;
		grid-template-columns: var(--file-nav-width) minmax(0, 1fr) var(--tag-panel-width);
		grid-template-rows: 1fr;
		min-height: 0;
	}

	.file-navigator-shell {
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.file-navigator-shell :global(.navigator) {
		width: 100%;
		height: 100%;
	}

	.viewer-column {
		display: grid;
		grid-template-rows: auto minmax(0, 1fr) auto;
		min-width: 0;
		min-height: 0;
		background: var(--viewport);
	}

	.viewer-context {
		min-width: 0;
		background: var(--paper);
	}

	/* Overlay choice and references share one row; with neither, the row is gone. */
	.context-strip {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px 16px;
		min-height: 38px;
		padding: 6px 10px;
		box-sizing: border-box;
		border-bottom: 1px solid var(--line);
	}

	.context-strip:not(:has(*)) {
		display: none;
	}

	.empty-viewer,
	.tag-empty {
		display: grid;
		place-content: center;
		color: var(--ink-muted);
	}

	.empty-viewer {
		min-height: 0;
		background: var(--viewport);
	}

	.tag-empty {
		height: 100%;
		font-size: 0.85rem;
	}

	.tag-panel-shell {
		position: relative;
		background: var(--paper);
		border-left: 1px solid var(--line);
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.tag-panel-shell.collapsed {
		background: var(--surface);
	}

	.sidebar-handle {
		position: absolute;
		left: 0;
		top: 0;
		bottom: 0;
		width: 10px;
		transform: translateX(-50%);
		cursor: col-resize;
		touch-action: none;
		z-index: 5;
	}

	.sidebar-handle::after {
		content: "";
		position: absolute;
		left: 50%;
		top: 0;
		bottom: 0;
		width: 1px;
		background: var(--line);
		transform: translateX(-50%);
	}

	.sidebar-handle.dragging::after {
		background: var(--accent);
	}

	.sidebar-handle.disabled {
		cursor: default;
		pointer-events: none;
	}

	.panel-toggle {
		position: absolute;
		top: 8px;
		right: 8px;
		z-index: 6;
	}

	.loading,
	.error {
		display: grid;
		place-content: center;
		justify-items: center;
		gap: 12px;
		height: 100vh;
		background: var(--canvas);
		color: var(--text);
	}

	@media (max-width: 979px) {
		.workspace {
			grid-template-columns: var(--file-nav-width) minmax(0, 1fr);
		}

		.tags-drawer-button {
			display: block;
		}

		.tag-panel-shell {
			position: fixed;
			top: 0;
			right: 0;
			bottom: 0;
			z-index: 40;
			width: min(420px, 90vw);
			visibility: hidden;
			transform: translateX(100%);
			transition: transform var(--settle) var(--ease-standard), visibility 0s linear var(--settle);
			box-shadow: var(--elev-overlay);
		}

		.tag-panel-shell.compact-open {
			visibility: visible;
			transform: translateX(0);
			transition-delay: 0s;
		}

		.tag-panel-shell .sidebar-handle,
		.tag-panel-shell .panel-toggle {
			display: none;
		}
	}

	@media (max-width: 519px) {
		.topbar {
			grid-template-columns: auto minmax(0, 1fr) auto;
			gap: 0.25rem;
		}

		.brand-mark {
			display: none;
		}

		.explorer-drawer-button {
			display: block;
		}

		.workspace {
			grid-template-columns: minmax(0, 1fr);
		}

		.file-navigator-shell {
			position: fixed;
			top: 0;
			left: 0;
			bottom: 0;
			z-index: 40;
			width: min(276px, 90vw);
			visibility: hidden;
			transform: translateX(-100%);
			transition: transform var(--settle) var(--ease-standard), visibility 0s linear var(--settle);
			box-shadow: var(--elev-overlay);
		}

		.file-navigator-shell.compact-open {
			visibility: visible;
			transform: translateX(0);
			transition-delay: 0s;
		}

		.file-navigator-shell :global(.collapse-button) {
			display: none;
		}
	}
</style>

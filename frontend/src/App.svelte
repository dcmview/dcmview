<script lang="ts">
	import { onMount } from "svelte";
	import { annotationsExportUrl, type SemanticContextResponse } from "./api";
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
	import { shortcutFor } from "./lib/keyboardShortcuts";
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
	// Value colorwashes (RT Dose) over the images they cover.
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
				cinePlaying = false;
				tabs.open(adjacent);
				return;
			}
			case "select-tool":
				activeTool = action.tool;
				return;
			case "step-frame":
				event.preventDefault();
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

	onMount(() => catalog.poll(() => tabs.syncCatalog(catalog.files?.files[0]?.index ?? null)));
</script>

<svelte:window onkeydown={handleWindowKeydown} onresize={() => layout.viewportResized()} />

{#if catalog.loadError}
	<main class="error">{catalog.loadError}</main>
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
			<button
				type="button"
				class="compact-sidebar-button explorer-drawer-button"
				bind:this={layout.explorerButton}
				onclick={() => layout.toggleDrawer("explorer")}
				aria-label="Toggle Explorer drawer"
				aria-controls="file-navigator-panel"
				aria-expanded={layout.compactDrawer === "explorer"}
			>
				Explorer
			</button>
			<OpenImageTabs
				openFiles={openTabFiles}
				frameCounts={tabs.frameCounts}
				activeFileIndex={tabs.activeFileIndex}
				onactivate={(fileIndex) => tabs.activate(fileIndex)}
				onclose={(fileIndex) => tabs.close(fileIndex)}
			/>
			<button
				type="button"
				class="compact-sidebar-button tags-drawer-button"
				bind:this={layout.tagsButton}
				onclick={() => layout.toggleDrawer("tags")}
				aria-label="Toggle Tags drawer"
				aria-controls="tag-panel"
				aria-expanded={layout.compactDrawer === "tags"}
			>
				Tags
			</button>
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
						<ReferenceNavigator
							fileIndex={activeFile.index}
							files={catalog.files.files}
							onopenreference={(fileIndex, frameIndex) => tabs.openReference(fileIndex, frameIndex)}
						/>
						{#if supportsSemanticContext(activeFile.object_kind, activeFile.sop_class_uid)}
							<SemanticContextPanel
								fileIndex={activeFile.index}
								currentFrame={tabs.currentFrame}
								files={catalog.files.files}
								onopenreference={(fileIndex, frameIndex) => tabs.openReference(fileIndex, frameIndex)}
								onmodechange={(mode) => { semanticMode = mode; }}
								oncontextchange={(response) => { semanticResponse = response; }}
								onshowoverlay={showValueOverlay}
							/>
						{/if}
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
						{#if activeFile.object_kind === "whole_slide_microscopy"}
							<WsiTileContext
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
				<button
					type="button"
					class="panel-toggle"
					onclick={() => layout.toggleTagPanel()}
					aria-label={layout.tagPanelCollapsed ? "Expand DICOM tag panel" : "Collapse DICOM tag panel"}
					aria-expanded={!layout.tagPanelCollapsed}
				>
					{layout.tagPanelCollapsed ? "◀" : "▶"}
				</button>
				{#if !layout.tagPanelCollapsed}
					{#if activeFile === null}
						<div class="tag-empty">No file selected</div>
					{:else}
						<TagPanel fileIndex={activeFile.index} />
					{/if}
				{/if}
			</aside>
		</section>
		<StatusBar
			serverStartMs={catalog.files.server_start_ms}
			fileCount={catalog.files.files.length}
		/>
	</main>
{/if}

<style>
	:global(:root) {
		--font-ui: -apple-system, BlinkMacSystemFont, "SF Pro Text", "Segoe UI", system-ui, sans-serif;
		--font-mono: "SF Mono", "JetBrains Mono", ui-monospace, monospace;
		--surface-root: #151516;
		--surface-viewport: #080809;
		--surface-chrome: #202124;
		--surface-panel: #252629;
		--surface-panel-alt: #2b2c30;
		--surface-control: #303136;
		--surface-control-hover: #393a40;
		--surface-control-active: #e7e7ea;
		--border-subtle: rgba(255, 255, 255, 0.08);
		--border-strong: rgba(255, 255, 255, 0.14);
		--text-primary: #f2f2f3;
		--text-secondary: #c7c7cc;
		--text-muted: #8e8e93;
		--text-inverse: #1d1d1f;
		--accent: #0a84ff;
		--accent-soft: rgba(10, 132, 255, 0.16);
		--danger: #ff6961;
		--accent-text: #9fcbff;
		--danger-text: #ffb0b0;
		--success-text: #8bd5a1;
		--text-disabled: rgba(255, 255, 255, 0.22);
		--surface-hud: rgba(28, 28, 30, 0.78);
		--surface-hover-overlay: rgba(255, 255, 255, 0.08);
		--viewport-glow: rgba(255, 255, 255, 0.025);
		--spinner-track: rgba(142, 142, 147, 0.24);
		--label-halo: rgba(0, 0, 0, 0.75);
		/* ROI annotations drawn over the image. */
		--roi-stroke: #ff7373;
		--roi-fill: rgba(255, 115, 115, 0.12);
		--roi-label: #ffdede;
		--roi-selected-stroke: #4a9eff;
		--roi-selected-fill: rgba(74, 158, 255, 0.16);
		--roi-selected-label: #c8ddff;
		--roi-draft-stroke: #ffd45c;
		--roi-draft-fill: rgba(255, 212, 92, 0.14);
		--roi-handle-outline: #101820;
		--radius-control: 7px;
		--radius-panel: 8px;
		--control-height: 1.75rem;
		--shadow-hud: 0 12px 30px rgba(0, 0, 0, 0.28);
		--focus-ring: 0 0 0 2px rgba(10, 132, 255, 0.48);
		color-scheme: dark;
	}

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
		background: var(--surface-root);
		color: var(--text-primary);
		-webkit-font-smoothing: antialiased;
		text-rendering: optimizeLegibility;
	}

	.layout {
		display: grid;
		grid-template-rows: auto auto 1fr auto;
		height: 100vh;
		width: 100%;
		overflow: hidden;
		background: var(--surface-root);
	}

	.topbar {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		align-items: end;
		gap: 0.8rem;
		min-height: 2.6rem;
		background: var(--surface-chrome);
		padding: 0 0.7rem;
		border-bottom: 1px solid var(--border-subtle);
	}

	.compact-sidebar-button {
		display: none;
		align-self: center;
		height: var(--control-height);
		padding: 0 0.65rem;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		background: var(--surface-control);
		color: var(--text-secondary);
		font: inherit;
		font-size: 0.74rem;
		cursor: pointer;
	}

	.compact-sidebar-button:hover,
	.compact-sidebar-button[aria-expanded="true"] {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}

	.compact-sidebar-button:focus-visible,
	.file-navigator-shell:focus-visible,
	.tag-panel-shell:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
	}

	.drawer-backdrop {
		position: fixed;
		inset: 0;
		z-index: 30;
		border: 0;
		background: rgba(0, 0, 0, 0.52);
		cursor: default;
	}

	.brand-mark {
		align-self: center;
		display: block;
		width: 1.55rem;
		height: 1.55rem;
		border-radius: 0.28rem;
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
		background: var(--surface-viewport);
	}

	.viewer-context {
		min-width: 0;
	}

	.empty-viewer,
	.tag-empty {
		display: grid;
		place-content: center;
		color: var(--text-muted);
	}

	.empty-viewer {
		min-height: 0;
		background: var(--surface-viewport);
	}

	.tag-empty {
		height: 100%;
		font-size: 0.85rem;
	}

	.tag-panel-shell {
		position: relative;
		background: var(--surface-panel);
		border-left: 1px solid var(--border-subtle);
		min-width: 0;
		min-height: 0;
		overflow: hidden;
	}

	.tag-panel-shell.collapsed {
		background: var(--surface-chrome);
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
		background: var(--border-subtle);
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
		top: 0.6rem;
		right: 0.45rem;
		display: grid;
		place-items: center;
		width: 1.5rem;
		height: 1.5rem;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		background: var(--surface-control);
		color: var(--text-secondary);
		cursor: pointer;
		z-index: 6;
	}

	.panel-toggle:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}

	.panel-toggle:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
	}

	.loading,
	.error {
		display: grid;
		place-content: center;
		height: 100vh;
		background: var(--surface-root);
		color: var(--text-secondary);
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
			width: min(360px, 90vw);
			visibility: hidden;
			transform: translateX(100%);
			transition: transform 150ms ease, visibility 0s linear 150ms;
			box-shadow: -12px 0 30px rgba(0, 0, 0, 0.34);
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
			width: min(300px, 90vw);
			visibility: hidden;
			transform: translateX(-100%);
			transition: transform 150ms ease, visibility 0s linear 150ms;
			box-shadow: 12px 0 30px rgba(0, 0, 0, 0.34);
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

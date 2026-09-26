<script lang="ts">
	import { onMount, tick } from "svelte";
	import {
		annotationsExportUrl,
		fetchFiles,
		fetchSeries,
		type FilesResponse,
		type SeriesCatalogResponse,
		type SeriesStackSummary,
		type SemanticContextResponse,
		type WindowMode,
		type WindowPreset,
	} from "./api";
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
	import ViewerToolbar from "./lib/ViewerToolbar.svelte";
	import WsiTileContext from "./lib/WsiTileContext.svelte";
	import type { CineDirection, CineMode } from "./lib/cinePlayback";
	import { indexFilesById, resolveFilesById, reuseUnchangedEntries } from "./lib/fileRegistry";
	import { focusTrapTarget } from "./lib/focusTrap";
	import { adjacentFileIndex } from "./lib/fileTree";
	import {
		findSeriesStackForFile,
		frameAtPosition,
		framePosition,
		navigationFrameAtPosition,
		navigationFramesForFile,
		navigationTabId,
		type NavigationFrameRef,
	} from "./lib/seriesNavigation";
	import { WL_PRESETS, type ActiveTool } from "./lib/viewerTools";
	import type { FrameOverlay } from "./lib/viewport/frameOverlay";
	import { ViewStates } from "./lib/viewport/viewStates.svelte";
	import {
		flipHorizontal,
		flipVertical,
		rotateClockwise,
		rotateCounterClockwise,
	} from "./lib/viewport/viewTransform";

	const TAG_PANEL_DEFAULT_WIDTH_PX = 360;
	const TAG_PANEL_MIN_WIDTH_PX = 260;
	const TAG_PANEL_MAX_WIDTH_PX = 720;
	const TAG_PANEL_COLLAPSED_WIDTH_PX = 44;
	const FILE_NAV_WIDTH_PX = 300;
	const FILE_NAV_COLLAPSED_WIDTH_PX = 44;
	const DRAWER_FOCUSABLE_SELECTOR = [
		'a[href]',
		'button:not([disabled])',
		'input:not([disabled])',
		'select:not([disabled])',
		'textarea:not([disabled])',
		'[tabindex]:not([tabindex="-1"])',
	].join(',');

	type SidebarResizeState = {
		pointerId: number;
		startX: number;
		startWidth: number;
	};

	type OpenTabState = {
		id: string;
		fileIndex: number;
		currentFrame: number;
		stackPosition: number;
	};

	type ManualWindowAdjustment = {
		centerOffsetRatio: number;
		widthRatio: number;
	};

	type CompactDrawer = "explorer" | "tags";

	let filesResponse = $state<FilesResponse | null>(null);
	let seriesResponse = $state<SeriesCatalogResponse | null>(null);
	let loadError = $state<string | null>(null);

	let openTabs = $state<OpenTabState[]>([]);
	let activeTabId = $state<string | null>(null);
	let activeFileIndex = $state<number | null>(null);
	let currentFrame = $state(0);
	let stackPosition = $state(0);
	let cinePlaying = $state(false);
	let cineFps = $state(10);
	let cineMode = $state<CineMode>("loop");
	let cineDirection = $state<CineDirection>(1);
	let lastCineStackId = $state<string | null>(null);
	let windowCenter = $state<number | null>(null);
	let windowWidth = $state<number | null>(null);
	let activeTool = $state<ActiveTool>('pan');
	let windowMode = $state<WindowMode>('default');
	let selectedPresetId = $state('default');
	let lastAppliedPresetId = 'default';
	let manualWindowAdjustment = $state<ManualWindowAdjustment | null>(null);
	let lastWindowFileIndex = $state<number | null>(null);
	let resetCount = $state(0);
	// Zoom, pan, and orientation per open tab; ImageViewport reads and zooms,
	// the toolbar reorients.
	const viewStates = new ViewStates();
	let fileNavigatorCollapsed = $state(false);
	let tagPanelWidthPx = $state(clampTagPanelWidth(TAG_PANEL_DEFAULT_WIDTH_PX));
	let tagPanelCollapsed = $state(false);
	let sidebarResizeState = $state<SidebarResizeState | null>(null);
	let compactDrawer = $state<CompactDrawer | null>(null);
	let explorerDrawerButton = $state<HTMLButtonElement | null>(null);
	let tagsDrawerButton = $state<HTMLButtonElement | null>(null);
	let explorerDrawerElement = $state<HTMLDivElement | null>(null);
	let tagsDrawerElement = $state<HTMLElement | null>(null);
	let fileNavigationOrder = $state<number[]>([]);
	let semanticMode = $state<SemanticMode>("pixel_preview");
	let semanticResponse = $state<SemanticContextResponse | null>(null);

	const filesById = $derived(indexFilesById(filesResponse?.files ?? []));
	const activeFile = $derived(
		activeFileIndex === null ? null : filesById.get(activeFileIndex) ?? null,
	);
	const activeLocatedStack = $derived(
		activeFileIndex === null
			? null
			: findSeriesStackForFile(seriesResponse?.series ?? [], activeFileIndex),
	);
	const activeStack = $derived(activeLocatedStack?.stack ?? null);
	const navigationFrames = $derived.by<readonly NavigationFrameRef[]>(() => {
		if (activeStack) return activeStack.frames;
		if (activeFile) return navigationFramesForFile(activeFile.index, activeFile.frame_count);
		return [];
	});
	const navigationFrameCount = $derived(navigationFrames.length);
	const navigationScopeKey = $derived(activeTabId ?? (activeFile ? `file:${activeFile.index}` : ""));
	const frameOverlay = $derived.by<FrameOverlay | null>(() => {
		if (semanticMode !== "semantic_context") return null;
		if (!semanticResponse || semanticResponse.source_file_index !== activeFileIndex) return null;
		const selection = segmentationOverlaySelection(semanticResponse, currentFrame);
		if (!selection) return null;
		const sourceFile = filesById.get(selection.sourceFileIndex);
		if (!sourceFile) return null;
		return { kind: "segmentation", ...selection, sourceFile };
	});
	const openTabFiles = $derived(resolveFilesById(filesById, openTabs.map((tab) => tab.fileIndex)));
	const openTabFrameCounts = $derived(new Map(
		openTabs.map((tab) => [
			tab.fileIndex,
			stackById(tab.id)?.frames.length ?? filesById.get(tab.fileIndex)?.frame_count ?? 0,
		]),
	));
	const fileNavigatorWidthPx = $derived(fileNavigatorCollapsed ? FILE_NAV_COLLAPSED_WIDTH_PX : FILE_NAV_WIDTH_PX);
	const tagPanelWidth = $derived(tagPanelCollapsed ? TAG_PANEL_COLLAPSED_WIDTH_PX : tagPanelWidthPx);

	function clampTagPanelWidth(width: number): number {
		return Math.min(TAG_PANEL_MAX_WIDTH_PX, Math.max(TAG_PANEL_MIN_WIDTH_PX, width));
	}

	function defaultTabState(fileIndex: number): OpenTabState {
		const located = findSeriesStackForFile(seriesResponse?.series ?? [], fileIndex);
		const position = located ? framePosition(located.stack, fileIndex, 0) ?? 0 : 0;
		const frame = located ? frameAtPosition(located.stack, position) : null;
		return {
			id: navigationTabId(seriesResponse?.series ?? [], fileIndex),
			fileIndex: frame?.file_index ?? fileIndex,
			currentFrame: frame?.frame_index ?? 0,
			stackPosition: position,
		};
	}

	function saveActiveTabState() {
		if (activeTabId === null || activeFileIndex === null) return;
		const tabId = activeTabId;
		const fileIndex = activeFileIndex;
		openTabs = openTabs.map((tab) => tab.id === tabId
			? {
				...tab,
				fileIndex,
				currentFrame,
				stackPosition,
			}
			: tab);
	}

	function loadTabState(tab: OpenTabState | null) {
		if (!tab) {
			activeTabId = null;
			activeFileIndex = null;
			currentFrame = 0;
			stackPosition = 0;
			return;
		}

		activeTabId = tab.id;
		activeFileIndex = tab.fileIndex;
		currentFrame = tab.currentFrame;
		stackPosition = tab.stackPosition;
	}

	function activateOpenTab(fileIndex: number) {
		const target = openTabs.find((tab) => tab.fileIndex === fileIndex);
		if (!target) return;
		if (activeTabId !== target.id) {
			saveActiveTabState();
		}
		loadTabState(target);
	}

	function stackById(id: string | null): SeriesStackSummary | null {
		if (id === null) return null;
		for (const series of seriesResponse?.series ?? []) {
			const stack = series.stacks.find((candidate) => candidate.id === id);
			if (stack) return stack;
		}
		return null;
	}

	function setStackPosition(position: number) {
		const frame = navigationFrameAtPosition(navigationFrames, position);
		if (!frame) return;
		stackPosition = frame.virtual_index;
		activeFileIndex = frame.file_index;
		currentFrame = frame.frame_index;
		if (activeTabId !== null) {
			openTabs = openTabs.map((tab) => tab.id === activeTabId
				? {
					...tab,
					fileIndex: frame.file_index,
					currentFrame: frame.frame_index,
					stackPosition: frame.virtual_index,
				}
				: tab);
		}
	}

	function openOrActivateFile(fileIndex: number) {
		const id = navigationTabId(seriesResponse?.series ?? [], fileIndex);
		const existing = openTabs.find((tab) => tab.id === id);
		if (existing) {
			if (activeTabId !== existing.id) saveActiveTabState();
			loadTabState(existing);
			const stack = stackById(id);
			const position = stack ? framePosition(stack, fileIndex, 0) : null;
			if (position !== null) setStackPosition(position);
			return;
		}

		saveActiveTabState();
		const next = defaultTabState(fileIndex);
		openTabs = [...openTabs, next];
		loadTabState(next);
	}

	function openFileFromNavigator(fileIndex: number) {
		openOrActivateFile(fileIndex);
		if (window.matchMedia("(max-width: 519px)").matches) {
			closeCompactDrawer();
		}
	}

	function updateFileNavigationOrder(order: number[]) {
		if (
			order.length === fileNavigationOrder.length
			&& order.every((fileIndex, position) => fileNavigationOrder[position] === fileIndex)
		) return;
		fileNavigationOrder = order;
	}

	function openReferenceTarget(fileIndex: number, frameIndex: number) {
		const file = filesById.get(fileIndex);
		if (
			!file
			|| !Number.isInteger(frameIndex)
			|| frameIndex < 0
			|| frameIndex >= file.frame_count
		) return;

		openOrActivateFile(fileIndex);
		const stack = stackById(activeTabId);
		const position = stack ? framePosition(stack, fileIndex, frameIndex) : null;
		if (position !== null) {
			setStackPosition(position);
			return;
		}

		activeFileIndex = fileIndex;
		currentFrame = frameIndex;
		stackPosition = frameIndex;
		if (activeTabId !== null) {
			openTabs = openTabs.map((tab) => tab.id === activeTabId
				? { ...tab, fileIndex, currentFrame: frameIndex, stackPosition: frameIndex }
				: tab);
		}
	}

	function compactDrawerTrigger(drawer: CompactDrawer): HTMLButtonElement | null {
		return drawer === "explorer" ? explorerDrawerButton : tagsDrawerButton;
	}

	function handleSemanticModeChange(mode: SemanticMode) {
		semanticMode = mode;
	}

	function handleSemanticContextChange(response: SemanticContextResponse | null) {
		semanticResponse = response;
	}

	function compactDrawerElement(drawer: CompactDrawer): HTMLElement | null {
		return drawer === "explorer" ? explorerDrawerElement : tagsDrawerElement;
	}

	function closeCompactDrawer(restoreFocus = true) {
		const closingDrawer = compactDrawer;
		if (closingDrawer === null) return;
		compactDrawer = null;
		if (restoreFocus) {
			void tick().then(() => compactDrawerTrigger(closingDrawer)?.focus());
		}
	}

	function toggleCompactDrawer(drawer: CompactDrawer) {
		if (compactDrawer === drawer) {
			closeCompactDrawer(false);
			return;
		}
		if (drawer === "explorer") {
			fileNavigatorCollapsed = false;
		} else {
			tagPanelCollapsed = false;
		}
		compactDrawer = drawer;
		void tick().then(() => compactDrawerElement(drawer)?.focus());
	}

	function handleCompactDrawerKeydown(event: KeyboardEvent) {
		if (event.key !== "Tab" || compactDrawer === null) return;
		const container = compactDrawerElement(compactDrawer);
		if (!container) return;
		const focusable = Array.from(
			container.querySelectorAll<HTMLElement>(DRAWER_FOCUSABLE_SELECTOR),
		).filter((element) => element.getClientRects().length > 0 && getComputedStyle(element).visibility !== "hidden");
		const activeIndex = focusable.indexOf(document.activeElement as HTMLElement);
		const target = focusTrapTarget(activeIndex, focusable.length, event.shiftKey);
		if (target === null) return;
		event.preventDefault();
		if (target === "container") {
			container.focus();
		} else if (target === "first") {
			focusable[0]?.focus();
		} else {
			focusable[focusable.length - 1]?.focus();
		}
	}

	function closeOpenTab(fileIndex: number) {
		const closingIndex = openTabs.findIndex((tab) => tab.fileIndex === fileIndex);
		if (closingIndex === -1) return;
		const closingId = openTabs[closingIndex]?.id;

		const wasActive = activeTabId === closingId;
		const remaining = openTabs.filter((tab) => tab.id !== closingId);
		openTabs = remaining;

		if (!wasActive) return;

		const replacement = remaining[Math.min(closingIndex, remaining.length - 1)] ?? null;
		loadTabState(replacement);
	}

	function resetViewport() {
		if (activeFileIndex === null) return;
		manualWindowAdjustment = null;
		windowCenter = null;
		windowWidth = null;
		windowMode = 'default';
		selectedPresetId = 'default';
		resetCount += 1;
		viewStates.resetOrientation(navigationScopeKey);
	}

	function reorient(change: typeof flipHorizontal) {
		if (activeFileIndex === null) return;
		viewStates.updateOrientation(navigationScopeKey, change);
	}

	function exportAnnotations() {
		const link = document.createElement('a');
		link.href = annotationsExportUrl();
		link.download = 'dcmview-annotations.csv';
		document.body.appendChild(link);
		link.click();
		link.remove();
	}

	function toggleTagPanel() {
		tagPanelCollapsed = !tagPanelCollapsed;
	}

	function startTagPanelResize(event: PointerEvent) {
		if (tagPanelCollapsed || event.button !== 0) {
			return;
		}

		const handle = event.currentTarget as HTMLElement;
		handle.setPointerCapture(event.pointerId);
		sidebarResizeState = {
			pointerId: event.pointerId,
			startX: event.clientX,
			startWidth: tagPanelWidthPx,
		};
		event.preventDefault();
	}

	function moveTagPanelResize(event: PointerEvent) {
		if (!sidebarResizeState || sidebarResizeState.pointerId !== event.pointerId) {
			return;
		}

		const delta = sidebarResizeState.startX - event.clientX;
		tagPanelWidthPx = clampTagPanelWidth(sidebarResizeState.startWidth + delta);
	}

	function endTagPanelResize(event: PointerEvent) {
		const handle = event.currentTarget as HTMLElement;
		if (handle.hasPointerCapture(event.pointerId)) {
			handle.releasePointerCapture(event.pointerId);
		}

		if (sidebarResizeState?.pointerId === event.pointerId) {
			sidebarResizeState = null;
		}
	}

	function cancelTagPanelResize() {
		sidebarResizeState = null;
	}

	function fileByIndex(fileIndex: number): FilesResponse["files"][number] | null {
		return filesById.get(fileIndex) ?? null;
	}

	function applyCatalogResponses(files: FilesResponse, series: SeriesCatalogResponse) {
		seriesResponse = {
			...series,
			series: reuseUnchangedEntries(seriesResponse?.series, series.series, (entry) => entry.id),
		};
		filesResponse = {
			...files,
			files: reuseUnchangedEntries(filesResponse?.files, files.files, (entry) => entry.index),
		};
		if (activeFileIndex === null && openTabs.length === 0 && files.files.length > 0) {
			openOrActivateFile(files.files[0].index);
			return;
		}
		if (activeFileIndex !== null && activeTabId !== null) {
			const located = findSeriesStackForFile(series.series, activeFileIndex);
			if (located && activeTabId !== located.stack.id) {
				const previousId = activeTabId;
				activeTabId = located.stack.id;
				openTabs = openTabs.map((tab) => tab.id === previousId
					? { ...tab, id: located.stack.id }
					: tab);
			}
			const position = located
				? framePosition(located.stack, activeFileIndex, currentFrame)
				: null;
			if (position !== null) {
				stackPosition = position;
				openTabs = openTabs.map((tab) => tab.id === activeTabId
					? { ...tab, stackPosition: position }
					: tab);
			}
		}
	}

	function defaultWindowForFile(fileIndex: number): WindowPreset | null {
		const window = fileByIndex(fileIndex)?.default_window ?? null;
		if (!window || !Number.isFinite(window.center) || !Number.isFinite(window.width) || window.width <= 0) {
			return null;
		}
		return window;
	}

	function resolveManualWindowForFile(fileIndex: number): WindowPreset | null {
		if (!manualWindowAdjustment) return null;
		const base = defaultWindowForFile(fileIndex);
		if (!base) return null;
		return {
			center: base.center + manualWindowAdjustment.centerOffsetRatio * base.width,
			width: Math.max(1, manualWindowAdjustment.widthRatio * base.width),
		};
	}

	function recordManualWindowLevel(center: number, width: number) {
		if (activeFileIndex === null || !Number.isFinite(center) || !Number.isFinite(width) || width <= 0) {
			return;
		}
		const base = defaultWindowForFile(activeFileIndex);
		if (!base) {
			manualWindowAdjustment = null;
			return;
		}
		manualWindowAdjustment = {
			centerOffsetRatio: (center - base.center) / base.width,
			widthRatio: width / base.width,
		};
		windowMode = 'default';
		selectedPresetId = 'default';
		lastAppliedPresetId = 'default';
	}

	function applyWindowPreset(presetId: string) {
		manualWindowAdjustment = null;
		const preset = WL_PRESETS.find(p => p.id === presetId);
		if (!preset) return;
		if (preset.wc !== undefined && preset.ww !== undefined) {
			windowCenter = preset.wc;
			windowWidth = preset.ww;
			windowMode = 'default';
		} else {
			windowCenter = null;
			windowWidth = null;
			windowMode = preset.mode ?? 'default';
		}
	}

	$effect(() => {
		const presetId = selectedPresetId;
		if (presetId === lastAppliedPresetId) return;
		lastAppliedPresetId = presetId;
		applyWindowPreset(presetId);
	});

	$effect(() => {
		const fileIndex = activeFileIndex;
		if (fileIndex === lastWindowFileIndex) return;
		lastWindowFileIndex = fileIndex;
		if (fileIndex === null || !manualWindowAdjustment) return;
		const resolved = resolveManualWindowForFile(fileIndex);
		if (!resolved) return;
		windowCenter = resolved.center;
		windowWidth = resolved.width;
		windowMode = 'default';
	});

	$effect(() => {
		const stackId = activeTabId;
		if (stackId === lastCineStackId) return;
		lastCineStackId = stackId;
		cinePlaying = false;
		cineDirection = 1;
	});

	$effect(() => {
		const handleKey = (event: KeyboardEvent) => {
			if (event.key === "Escape" && compactDrawer !== null) {
				event.preventDefault();
				closeCompactDrawer();
				return;
			}
			const target = event.target as HTMLElement | null;
			if (
				target
				&& (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))
			) return;
			if (
				!event.altKey
				&& !event.ctrlKey
				&& !event.metaKey
				&& !event.shiftKey
				&& (event.key === "ArrowUp" || event.key === "ArrowDown")
			) {
				const adjacent = adjacentFileIndex(
					fileNavigationOrder,
					activeFileIndex,
					event.key === "ArrowUp" ? -1 : 1,
				);
				if (adjacent !== null) {
					event.preventDefault();
					cinePlaying = false;
					openOrActivateFile(adjacent);
				}
				return;
			}
			switch (event.key.toLowerCase()) {
				case 'w': activeTool = 'window_level'; break;
				case 'p': activeTool = 'pan'; break;
				case 'z': activeTool = 'zoom'; break;
				case 's': activeTool = 'scroll'; break;
				case 'r': activeTool = 'annotate_rect'; break;
			}
		};
		window.addEventListener('keydown', handleKey);
		return () => window.removeEventListener('keydown', handleKey);
	});

	$effect(() => {
		const handleResize = () => {
			if (compactDrawer === "explorer" && !window.matchMedia("(max-width: 519px)").matches) {
				closeCompactDrawer(false);
			}
			if (compactDrawer === "tags" && !window.matchMedia("(max-width: 979px)").matches) {
				closeCompactDrawer(false);
			}
		};
		window.addEventListener("resize", handleResize);
		return () => window.removeEventListener("resize", handleResize);
	});

	onMount(() => {
		let cancelled = false;
		let pollTimer: number | null = null;

		const pollCatalog = async () => {
			try {
				const [files, series] = await Promise.all([fetchFiles(), fetchSeries()]);
				if (cancelled) return;
				applyCatalogResponses(files, series);
				if (!files.scan_complete || !series.scan_complete) {
					pollTimer = window.setTimeout(pollCatalog, 500);
				}
			} catch (error) {
				if (cancelled) return;
				if (!filesResponse) {
					loadError = error instanceof Error ? error.message : String(error);
					return;
				}
				pollTimer = window.setTimeout(pollCatalog, 1000);
			}
		};

		pollCatalog();
		return () => {
			cancelled = true;
			if (pollTimer !== null) {
				window.clearTimeout(pollTimer);
			}
		};
	});
</script>

{#if loadError}
	<main class="error">{loadError}</main>
{:else if !filesResponse}
	<main class="loading">Loading dcmview…</main>
{:else}
	<main
		class="layout"
		style={`--file-nav-width:${fileNavigatorWidthPx}px; --tag-panel-width:${tagPanelWidth}px;`}
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
				bind:this={explorerDrawerButton}
				onclick={() => toggleCompactDrawer("explorer")}
				aria-label="Toggle Explorer drawer"
				aria-controls="file-navigator-panel"
				aria-expanded={compactDrawer === "explorer"}
			>
				Explorer
			</button>
			<OpenImageTabs
				openFiles={openTabFiles}
				frameCounts={openTabFrameCounts}
				activeFileIndex={activeFileIndex}
				onactivate={activateOpenTab}
				onclose={closeOpenTab}
			/>
			<button
				type="button"
				class="compact-sidebar-button tags-drawer-button"
				bind:this={tagsDrawerButton}
				onclick={() => toggleCompactDrawer("tags")}
				aria-label="Toggle Tags drawer"
				aria-controls="tag-panel"
				aria-expanded={compactDrawer === "tags"}
			>
				Tags
			</button>
		</header>
		<ViewerToolbar
			bind:activeTool
			bind:selectedPresetId
			onreset={resetViewport}
			onflipH={() => reorient(flipHorizontal)}
			onflipV={() => reorient(flipVertical)}
			onrotateCW={() => reorient(rotateClockwise)}
			onrotateCCW={() => reorient(rotateCounterClockwise)}
			onexportAnnotations={exportAnnotations}
		/>
		{#if compactDrawer !== null}
			<button
				type="button"
				class="drawer-backdrop"
				onclick={() => closeCompactDrawer()}
				aria-label="Close sidebar drawer"
			></button>
		{/if}
		<section class="workspace">
			<div
				id="file-navigator-panel"
				class="file-navigator-shell"
				class:compact-open={compactDrawer === "explorer"}
				bind:this={explorerDrawerElement}
				tabindex="-1"
				role={compactDrawer === "explorer" ? "dialog" : undefined}
				aria-modal={compactDrawer === "explorer" ? "true" : undefined}
				aria-label="Explorer"
				onkeydown={handleCompactDrawerKeydown}
			>
				<FileNavigator
					files={filesResponse.files}
					activeFileIndex={activeFileIndex}
					scanComplete={filesResponse.scan_complete}
					bind:collapsed={fileNavigatorCollapsed}
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
							files={filesResponse.files}
							onopenreference={openReferenceTarget}
						/>
						{#if supportsSemanticContext(activeFile.object_kind, activeFile.sop_class_uid)}
							<SemanticContextPanel
								fileIndex={activeFile.index}
								{currentFrame}
								onmodechange={handleSemanticModeChange}
								oncontextchange={handleSemanticContextChange}
							/>
						{/if}
						{#if activeFile.object_kind === "whole_slide_microscopy"}
							<WsiTileContext fileIndex={activeFile.index} frame={currentFrame} />
						{/if}
					</div>
					<ImageViewport
						{activeFile}
						bind:currentFrame
						bind:windowCenter
						bind:windowWidth
						activeTool={activeTool}
						windowMode={windowMode}
						resetCount={resetCount}
						selectedPresetId={selectedPresetId}
						{viewStates}
						overlay={frameOverlay}
						bind:cinePlaying
						{cineFps}
						{cineMode}
						bind:cineDirection
						navigationFrameCount={navigationFrameCount}
						navigationFrames={navigationFrames}
						navigationScopeKey={navigationScopeKey}
						navigationPosition={stackPosition}
						onnavigationchange={setStackPosition}
						onreset={resetViewport}
						onmanualwindowlevel={recordManualWindowLevel}
					/>
					<FrameSlider
						totalFrames={navigationFrameCount}
						currentPosition={stackPosition}
						onpositionchange={setStackPosition}
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
				class:collapsed={tagPanelCollapsed}
				class:compact-open={compactDrawer === "tags"}
				bind:this={tagsDrawerElement}
				tabindex="-1"
				role={compactDrawer === "tags" ? "dialog" : undefined}
				aria-modal={compactDrawer === "tags" ? "true" : undefined}
				aria-label="DICOM tags"
				onkeydown={handleCompactDrawerKeydown}
			>
				<div
					class="sidebar-handle"
					class:dragging={sidebarResizeState !== null}
					class:disabled={tagPanelCollapsed}
					role="separator"
					aria-label="Resize DICOM tag panel"
					aria-orientation="vertical"
					aria-valuemin={TAG_PANEL_MIN_WIDTH_PX}
					aria-valuemax={TAG_PANEL_MAX_WIDTH_PX}
					aria-valuenow={tagPanelWidthPx}
					onpointerdown={startTagPanelResize}
					onpointermove={moveTagPanelResize}
					onpointerup={endTagPanelResize}
					onpointercancel={cancelTagPanelResize}
				></div>
				<button
					type="button"
					class="panel-toggle"
					onclick={toggleTagPanel}
					aria-label={tagPanelCollapsed ? "Expand DICOM tag panel" : "Collapse DICOM tag panel"}
					aria-expanded={!tagPanelCollapsed}
				>
					{tagPanelCollapsed ? "◀" : "▶"}
				</button>
				{#if !tagPanelCollapsed}
					{#if activeFile === null}
						<div class="tag-empty">No file selected</div>
					{:else}
						<TagPanel fileIndex={activeFile.index} />
					{/if}
				{/if}
			</aside>
		</section>
		<StatusBar
			serverStartMs={filesResponse.server_start_ms}
			fileCount={filesResponse.files.length}
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

<script lang="ts">
	import { tick, untrack } from "svelte";
	import {
		ApiError,
		fetchDisplayFrame,
		fetchSelectedTag,
		isApiError,
		type DisplayFrame,
		type DisplayFrameWindowOptions,
		type FileSummary,
		type FrameValueMapping,
		type RawFrame,
		type WindowMode,
			applyRedactionsToSeries,
		fetchRedactions,
		updateRedactions,
	} from "../api";
	import type { WindowDragBase } from "./annotation/tools/tool";
	import {
		canonicalRect,
		deleteRoi,
		setRoiFrameScope,
		type ImagePoint,
	} from "./annotationGeometry";
	import { canRunCinePlayback, type CineDirection, type CineMode } from "./cinePlayback";
	import { fitImageToViewportHeight, imageDisplayGeometry } from "./imageGeometry";
	import {
		hasIntegerModality,
		mappedUnitsPerStoredUnit,
		MAX_RENDER_PIXELS,
		samplePresentation,
		selectWindowingPipeline,
		validateRenderableRawFrame,
	} from "./rawWindowing";
	import { trackForegroundRequest } from "./requestIndicator";
	import type { NavigationFrameRef } from "./seriesNavigation";
	import type { ActiveTool } from "./viewerTools";
	import { AnnotationStore } from "./viewport/annotationStore.svelte";
	import { playDisplayCine } from "./viewport/displayCine";
	import { LiveWindowPreview } from "./viewport/liveWindowPreview";
	import { DisplayFrameSource } from "./viewport/displayFrameSource";
	import {
		composeOverlayFrame,
		decodeCanvasImage,
		drawOverlayLayer,
		legendColors,
		OverlayLayerCache,
		overlayLayerRequests,
		valueOverlayValuesRequest,
		type FrameOverlay,
		type ValueOverlay,
	} from "./viewport/frameOverlay";
	import {
		observePrefetchConcurrency,
		PREFETCH_CONCURRENCY,
		scheduleIdle,
	} from "./viewport/prefetchScheduling";
	import PixelReadout from "./viewport/PixelReadout.svelte";
	import {
		overlayValueReadout,
		PixelProbe,
		pixelReadout,
		probesSinglePixels,
		type OverlayValueState,
	} from "./viewport/pixelProbe.svelte";
	import GraphicAnnotationLabels from "./viewport/GraphicAnnotationLabels.svelte";
	import GraphicAnnotationOverlay from "./viewport/GraphicAnnotationOverlay.svelte";
	import { GraphicAnnotationFrames } from "./viewport/graphicAnnotationFrames.svelte";
	import type { GraphicAnnotationSelection } from "./viewport/graphicAnnotations";
	import { RawFrameSource } from "./viewport/rawFrameSource";
	import { RenderedFrames } from "./viewport/renderedFrames.svelte";
	import { resolveWindow, type WindowResolution } from "./viewport/resolveWindow";
	import { FrameLayers, type PreparedFrameLayers, type LayerStatus } from "./viewport/frameLayers";
	import RoiList from "./viewport/RoiList.svelte";
	import RoiLabels from "./viewport/RoiLabels.svelte";
	import RoiOverlay from "./viewport/RoiOverlay.svelte";
	import { visibleRois as roisOnFrame } from "./viewport/roiEditing";
	import { isViewportChromeTarget, ToolHost } from "./viewport/ToolHost.svelte";
	import type { ViewStates } from "./viewport/viewStates.svelte";
	import {
		clientToImagePoint,
		layerTransformCss,
		MIN_ZOOM,
		nextZoomStep,
		zoomAnchor,
		zoomAroundAnchor,
		type LayerOrigin,
		type ViewTransform,
		type ZoomAnchor,
	} from "./viewport/viewTransform";
	import ValueLegend from "./viewport/ValueLegend.svelte";
	import {
		formatValue,
		formatWindow,
		frameDisplayWindowOptions,
		mappedWindowScale,
		pixelAt,
		windowToRender,
	} from "./viewport/valueMapping";
	import { ValueMappings } from "./viewport/valueMappings.svelte";
	import { WlRendererClient } from "./viewport/wlRendererClient";
	import ZoomControls from "./viewport/ZoomControls.svelte";

	type PipelineMode = "cine" | "diagnostic_wl" | "server_wl" | "overlay";

	let {
		activeFile,
		currentFrame,
		windowCenter,
		windowWidth,
		windowUnit = null,
		activeTool,
		windowMode,
		viewStates,
		onreset,
		onmanualwindowlevel,
		cinePlaying = $bindable(),
		cineFps,
		cineMode,
		cineDirection = $bindable(),
		navigationFrameCount,
		navigationFrames,
		navigationScopeKey,
		navigationPosition,
		onnavigationchange,
		overlay = null,
		valueOverlay = null,
		graphicAnnotation = null,
	}: {
		activeFile: FileSummary;
		currentFrame: number;
		windowCenter: number | null;
		windowWidth: number | null;
		/** Real-world unit of the window; null for the rendered (Modality) scale. */
		windowUnit?: string | null;
		activeTool: ActiveTool;
		windowMode: WindowMode;
		viewStates: ViewStates;
		onreset: () => void;
		/**
		 * A window/level drag ended at this window: in the frame's real-world
		 * `unit` when it has a linear mapping, else on the rendered scale.
		 */
		onmanualwindowlevel: (center: number, width: number, unit: string | null) => void;
		cinePlaying: boolean;
		cineFps: number;
		cineMode: CineMode;
		cineDirection: CineDirection;
		navigationFrameCount: number;
		navigationFrames: readonly NavigationFrameRef[];
		navigationScopeKey: string;
		navigationPosition: number;
		onnavigationchange: (position: number) => void;
		overlay?: FrameOverlay | null;
		/** A colorwash drawn over the displayed frame; ignored under a SEG overlay. */
		valueOverlay?: ValueOverlay | null;
		/** A presentation state's annotations drawn over the displayed frame; ignored under a SEG overlay. */
		graphicAnnotation?: GraphicAnnotationSelection | null;
	} = $props();

	let loading = $state(false);
	let loadError = $state<string | null>(null);
	let liveWindowCenter = $state<number | null>(null);
	let liveWindowWidth = $state<number | null>(null);
	// The unit of the window a window/level drag began from.
	let liveWindowUnit = $state<string | null>(null);
	let viewportEl: HTMLElement | undefined = $state();
	let viewportSize = $state({ width: 0, height: 0 });
	let canvasEl: HTMLCanvasElement | undefined = $state();
	// Raw, not a deep proxy: the frame is posted to the W/L worker, and a
	// proxied metadata object cannot be structured-cloned.
	type FrameTarget = {
		file: FileSummary; frameIndex: number; position: number; totalFrames: number; scope: string;
		imageFile: FileSummary; imageFrameIndex: number; segmentation: boolean;
	};
	type PreparedRaw = { target: FrameTarget; frame: RawFrame; mapping: FrameValueMapping; layers: PreparedFrameLayers; generation: number };
	type PresentedFrame = {
		target: FrameTarget; raw: RawFrame | null; mapping: FrameValueMapping | null; window: WindowResolution;
		valueOverlay: ValueOverlay | null; presentation: LayerStatus;
	};
	let preparedRaw = $state.raw<PreparedRaw | null>(null);
	let presented = $state.raw<PresentedFrame | null>(null);
	const presentedMatchesActive = $derived(presented?.target.file.index === activeFile.index
		&& presented?.target.frameIndex === currentFrame && presented?.target.scope === navigationScopeKey);
	const currentRawFrame = $derived(preparedRaw?.frame ?? null);
	const currentRawFrameKey = $derived(preparedRaw ? `${preparedRaw.target.file.index}:${preparedRaw.target.frameIndex}` : "");
	const rawMatchesRequest = $derived(preparedRaw?.target.file.index === activeFile.index
		&& preparedRaw?.target.frameIndex === currentFrame && preparedRaw?.target.scope === navigationScopeKey);
	let displayPhotometric = $state.raw<{ fileIndex: number; value: string } | null>(null);
	// The window the server rendered the displayed PNG with, if linear, and
	// whether that PNG was requested in a real-world unit.
	let shownDisplay = $state.raw<Pick<DisplayFrame, "window" | "appliedWindow"> | null>(null);
	let shownUnitRequest = $state(false);
	let rawWindowLevelFallbackByFile = $state<Record<number, boolean>>({});
	// Color files: neither path windows them, so a drag sends no previews.
	let colorFiles = $state<Record<number, boolean>>({});
	const annotations = new AnnotationStore();
	// Redaction boxes are edited like ROIs. The server blanks them in the
	// frames it sends, so a saved change reloads the frames on screen.
	const redactions = new AnnotationStore({
		load: fetchRedactions,
		save: async (fileIndex, boxes) => {
			const saved = await updateRedactions(fileIndex, boxes);
			reloadFrames();
			return saved;
		},
	});
	const redacting = $derived(activeTool === "redact");
	/** The rectangles the rectangle tool edits and the viewport outlines. */
	const edited = $derived(redacting ? redactions : annotations);

	let prefetchConcurrency = $state(PREFETCH_CONCURRENCY);
	const rendered = new RenderedFrames();
	const rawFrames = new RawFrameSource({
		concurrency: () => prefetchConcurrency,
		prepare: (fileIndex, frameIndex, signal) => Promise.all([
			valueMappings.load(fileIndex, frameIndex, signal),
			warmValueLayer(fileIndex, frameIndex, signal),
		]),
	});
	const displayFrames = new DisplayFrameSource({
		load: loadDisplayFrame,
		prepare: (fileIndex, frameIndex, signal) => warmValueLayer(fileIndex, frameIndex, signal),
		// Prefetched and cine frames carry their own mapping, so the HUD and
		// legend keep a real-world unit on every frame; a foreground frame is
		// presented first and its HUD follows its mapping.
		loadMetadata: (fileIndex, frameIndex, signal) => valueMappings.load(fileIndex, frameIndex, signal),
		navigationScope: () => navigationScopeKey,
		concurrency: () => prefetchConcurrency,
		onScopeChange: () => rendered.reset(),
	});
	const valueMappings = new ValueMappings();
	const frameLayers = new FrameLayers();
	const segmentationLayers = new OverlayLayerCache();
	let presentationLayerCanvas: HTMLCanvasElement | undefined = $state();
	const overlayValues = new OverlayLayerCache<Float32Array>();
	let overlayValueState = $state<{ key: string; state: OverlayValueState } | null>(null);
	let valueOverlayCanvas: HTMLCanvasElement | undefined = $state();
	type ValueOverlayState = { key: string; status: "loading" | "shown" | "not_covering" | "error" };
	let valueOverlayState = $state<ValueOverlayState | null>(null);
	const probe = new PixelProbe(rawFrames);
	let probeClientPoint: { x: number; y: number } | null = null;
	let probeAnimationFrame = 0;
	let retainedScopeKey = "";
	let requestGeneration = 0;
	let lastFrameForDirection = 0;
	let frameDirection: 1 | -1 = 1;
	let wlRenderGeneration = 0;

	const wlRenderer = new WlRendererClient();

	// Zoom, pan, and orientation belong to the open tab (navigation scope).
	const activeTransform = $derived(viewStates.transform(activeFile ? navigationScopeKey : ""));
	const orientation = $derived(viewStates.orientation(navigationScopeKey));
	// Pointer and wheel gestures. The host and its tools read the viewport
	// through these members at the moment of each event.
	const tools = new ToolHost({
		get file() { return activeFile; },
		get frame() { return currentFrame; },
		get imageRows() { return imageRows; },
		get imageColumns() { return imageColumns; },
		get transform() { return activeTransform; },
		setTransform: (transform) => updateTransform(transform),
		toImage: pointFromPointer,
		zoomAnchor: zoomAnchorFromClient,
		zoomTransform: zoomTransformForAnchor,
		navigation: {
			get count() { return navigationFrameCount; },
			get position() { return navigationPosition; },
			go(position) {
				cinePlaying = false;
				onnavigationchange(position);
			},
		},
		window: {
			begin: beginWindowDrag,
			preview(center, width) {
				liveWindowCenter = center;
				liveWindowWidth = width;
			},
			commit() {
				if (liveWindowCenter !== null && liveWindowWidth !== null) {
					onmanualwindowlevel(liveWindowCenter, liveWindowWidth, liveWindowUnit);
				}
			},
		},
		rects: {
			get editable() { return !overlay && presentedMatchesActive && annotationsReady; },
			get visible() { return visibleRois; },
			get annotations() { return activeAnnotations; },
			get selectedIndex() { return selectedRoiIndex; },
			get coversAllFrames() { return redacting; },
			select: setSelectedRoi,
			beginLiveEdit: () => edited.beginLiveEdit(activeFile.index),
			showDraft: (fileIndex, annotations) => edited.showDraft(fileIndex, annotations),
			commit: (annotations, selectedIndex) => edited.commit(activeFile.index, annotations, selectedIndex),
		},
		get activeTool() { return activeTool; },
		get displayedFrameIsCurrent() { return presentedMatchesActive; },
		get viewportHeight() { return viewportSize.height; },
		zoomAt,
		scheduleProbe,
		gestureEnded,
	});
	const isDragging = $derived(tools.dragging);
	const windowDragging = $derived(tools.capturedTool === "window_level");
	// The displayed frame's value mapping (or the file's latest while that
	// frame's loads) decides whether the window is in real-world units.
	const frameMapping = $derived(rawMatchesRequest ? preparedRaw!.mapping : valueMappings.forFrame(activeFile.index, currentFrame));
	// A real-world mapping with no linear stored window (a LUT mapping, or
	// any mapping behind a Modality LUT) is windowed directly by the raw
	// renderer, from its automatic window or one set in its unit. A window
	// set on the stored scale (a preset) stays stored.
	const directMap = $derived.by(() => {
		const map = overlay ? null : frameMapping?.real_world[0];
		return map && mappedWindowScale(frameMapping) === null ? map : null;
	});
	const directWindowing = $derived(
		directMap !== null && (windowUnit === directMap.unit_label || (windowUnit === null && windowCenter === null)),
	);
	const pipelineMode = $derived.by<PipelineMode>(() => {
		if (overlay) return "overlay";
		if (cinePlaying) return "cine";
		// Frames over the browser's limit stay on the server without first
		// downloading their samples.
		const rawFallback = (rawWindowLevelFallbackByFile[activeFile.index] ?? false)
			|| activeFile.rows * activeFile.columns > MAX_RENDER_PIXELS;
		// Stills keep a window set in such a unit on the raw path, whatever
		// the tool; cine plays display frames the server windows in that unit
		// (see frameDisplayWindowOptions).
		if (directWindowing && windowUnit !== null && !cinePlaying && !rawFallback && activeFile.raw_windowing_compatible) {
			return "diagnostic_wl";
		}
		return selectWindowingPipeline(activeTool === "window_level", rawFallback, activeFile.raw_windowing_compatible);
	});

	// The raw renderer reads what the raw headers cannot say (float samples,
	// Modality and VOI LUTs) from the file's value mapping, and waits for it.
	const rawPresentation = $derived(preparedRaw ? samplePresentation(preparedRaw.frame, preparedRaw.mapping) : null);

	// A window in real-world units converts through that mapping to the
	// Modality scale that both render paths window.
	const mappedScale = $derived(overlay ? null : mappedWindowScale(frameMapping));
	const renderWindow = $derived.by(() => {
		if (overlay || windowCenter === null || windowWidth === null || windowUnit === null) {
			return { center: windowCenter, width: windowWidth, pending: false };
		}
		if (mappedScale?.unit === windowUnit) {
			return { ...windowToRender({ center: windowCenter, width: windowWidth }, mappedScale), pending: false };
		}
		// A frame without that unit falls back to its own default window.
		const known = frameMapping !== null || valueMappings.failed(activeFile.index, currentFrame);
		return { center: null, width: null, pending: !known };
	});
	const renderWindowCenter = $derived(renderWindow.center);
	const renderWindowWidth = $derived(renderWindow.width);
	const renderWindowPending = $derived(renderWindow.pending);

	const resolvedWindow = $derived(resolveWindow({
		raw: pipelineMode === "diagnostic_wl" && rawMatchesRequest ? currentRawFrame : null,
		mapping: overlay ? null : frameMapping,
		requested: windowCenter !== null && windowWidth !== null
			? { window: { wc: windowCenter, ww: windowWidth }, unit: windowUnit } : null,
		live: windowDragging && liveWindowCenter !== null && liveWindowWidth !== null
			? { window: { wc: liveWindowCenter, ww: liveWindowWidth }, unit: liveWindowUnit } : null,
		mode: windowMode,
		defaultWindow: overlay?.sourceFile.default_window ?? activeFile.default_window,
		server: shownDisplay,
		unitRequest: shownUnitRequest,
	}));
	const displayWindow = $derived(resolvedWindow.window);
	const hudWindow = $derived(presented?.window ?? resolvedWindow);
	// Convert only for the worker; HUD, drag and legend share the resolved unit.
	const rawRenderWindow = $derived.by(() => {
		if (!displayWindow) return null;
		if (resolvedWindow.unit && mappedScale?.unit === resolvedWindow.unit) {
			const converted = windowToRender({ center: displayWindow.wc, width: displayWindow.ww }, mappedScale, resolvedWindow.source === "live" || resolvedWindow.source === "explicit" && windowUnit !== null);
			return { wc: converted.center, ww: converted.width };
		}
		return displayWindow;
	});
	const windowLegend = $derived.by(() => {
		const { window, unit } = hudWindow;
		if (!window || !unit) return null;
		return { center: window.wc, width: window.ww, low: window.wc - window.ww / 2,
			high: window.wc + window.ww / 2, unit,
			label: (presented?.mapping ?? frameMapping)?.real_world[0]?.label ?? null };
	});
	const windowColors = $derived.by(() => {
		const fileIndex = presented?.target.imageFile.index ?? activeFile.index;
		const raw = presented?.raw ?? (rawMatchesRequest ? currentRawFrame : null);
		const photometric = raw?.metadata.photometricInterpretation
			?? (displayPhotometric?.fileIndex === fileIndex ? displayPhotometric.value : "");
		const mono1 = photometric.trim().toUpperCase() === "MONOCHROME1";
		const scale = mappedWindowScale(presented?.mapping ?? frameMapping);
		return mono1 !== (scale !== null && scale.ratio < 0) ? ["#fff", "#000"] : ["#000", "#fff"];
	});

	// The colorwash layer depends only on which volume is shown and whether
	// it covers the frame; opacity is applied to the drawn layer.
	const shownValueOverlay = $derived(overlay || !activeFile.has_pixels ? null : valueOverlay);
	const valueOverlayVolume = $derived(
		shownValueOverlay ? `${shownValueOverlay.kind}:${shownValueOverlay.volumeFileIndex}` : null,
	);
	const valueOverlayCovers = $derived(shownValueOverlay?.coversFrame ?? false);
	const displayedValueOverlay = $derived(presented ? presented.valueOverlay : shownValueOverlay);
	const valueOverlayCaption = $derived.by(() => {
		if (!displayedValueOverlay) return null;
		const { legend } = displayedValueOverlay;
		if (valueOverlayState?.status === "not_covering") return "Not covering this frame";
		if (valueOverlayState?.status === "error") return "Overlay unavailable for this frame";
		return legend.transparent_at_or_below === null
			? null
			: `≤ ${formatValue(legend.transparent_at_or_below)} ${legend.unit_label} transparent`;
	});

	const activeAnnotations = $derived(edited.annotations(activeFile.index));
	const annotationsReady = $derived(edited.ready(activeFile.index));
	const selectedRoiIndex = $derived(edited.selected(activeFile.index));
	const imageRows = $derived(presented?.raw?.metadata.rows ?? presented?.target.imageFile.rows ?? activeFile.rows);
	const imageColumns = $derived(presented?.raw?.metadata.columns ?? presented?.target.imageFile.columns ?? activeFile.columns);
	const displayGeometry = $derived(imageDisplayGeometry(imageRows, imageColumns,
		presented?.target.imageFile.pixel_aspect_ratio ?? overlay?.sourceFile.pixel_aspect_ratio ?? activeFile.pixel_aspect_ratio));

	const transformCss = $derived(layerTransformCss(activeTransform, orientation, displayGeometry));
	const annotationFrames = new GraphicAnnotationFrames();
	// The frame on screen, which trails the requested one while it loads.
	const annotatedFrame = $derived(graphicAnnotation && !overlay ? {
		stateFileIndex: graphicAnnotation.stateFileIndex,
		fileIndex: presented?.target.imageFile.index ?? activeFile.index,
		frameIndex: presented?.target.imageFrameIndex ?? currentFrame,
	} : null);
	const shownAnnotations = $derived(annotatedFrame ? annotationFrames.get(annotatedFrame) : null);
	$effect(() => {
		if (annotatedFrame) annotationFrames.load(annotatedFrame);
	});
	const visibleRois = $derived(
		overlay ? [] : roisOnFrame(edited.annotations(presented?.target.file.index ?? activeFile.index), presented?.target.frameIndex ?? currentFrame),
	);
	const draftRoi = $derived(
		tools.draft
			? canonicalRect(tools.draft.start, tools.draft.current, imageRows, imageColumns)
			: null,
	);

	// The readout reads the image on screen: a SEG overlay's source frame,
	// otherwise the active file's frame.
	const probeTarget = $derived(presented
		? { file: presented.target.imageFile, frameIndex: presented.target.imageFrameIndex }
		: overlay ? { file: overlay.sourceFile, frameIndex: overlay.sourceFrameIndex }
		: { file: activeFile, frameIndex: currentFrame });

	const probing = $derived(probe.pixel !== null);
	const readout = $derived.by(() => {
		const pixel = probe.pixel;
		if (!pixel) return null;
		const { file, frameIndex } = probeTarget;
		const values = pixelReadout({
			pixel,
			file,
			frameIndex,
			samples: probe.samples(file.index, frameIndex),
			mapping: valueMappings.get(file.index, frameIndex),
			mappingFailed: valueMappings.failed(file.index, frameIndex),
			planarConfiguration: (frame) => probe.planarConfiguration(file, frame),
			paused: cinePlaying,
		});
		if (!displayedValueOverlay || cinePlaying) return values;
		const key = valueOverlayValuesRequest(displayedValueOverlay, file.index, frameIndex).key;
		const state: OverlayValueState = valueOverlayState?.status === "not_covering"
			? { status: "not_covering" }
			: overlayValueState?.key === key ? overlayValueState.state : { status: "loading" };
		const label = displayedValueOverlay.kind === "rt_dose" ? "dose" : "map";
		return {
			...values,
			overlay: overlayValueReadout(label, displayedValueOverlay.legend.unit_label, pixel, file.columns, state),
		};
	});

	function warmValueLayer(fileIndex: number, frameIndex: number, signal?: AbortSignal) {
		const covers = fileIndex === activeFile.index && frameIndex === currentFrame ? shownValueOverlay?.coversFrame : undefined;
		return frameLayers.value(shownValueOverlay, fileIndex, frameIndex, covers, signal);
	}

	function frameTarget(file: FileSummary, frameIndex: number, segmentation: FrameOverlay | null = null): FrameTarget {
		return { file, frameIndex, position: navigationPosition, totalFrames: navigationFrameCount, scope: navigationScopeKey,
			imageFile: segmentation?.sourceFile ?? file, imageFrameIndex: segmentation?.sourceFrameIndex ?? frameIndex,
			segmentation: segmentation !== null };
	}

	function commitFrame(target: FrameTarget, window: WindowResolution, layers: PreparedFrameLayers,
		raw: RawFrame | null, mapping: FrameValueMapping | null, value: ValueOverlay | null): void {
		if (presentationLayerCanvas && layers.presentation.image) drawOverlayLayer(presentationLayerCanvas, layers.presentation.image);
		if (valueOverlayCanvas && layers.value.image) drawOverlayLayer(valueOverlayCanvas, layers.value.image);
		valueOverlayState = layers.value.status === "none" ? null : { key: layers.value.key, status: layers.value.status };
		presented = { target, window, raw, mapping, valueOverlay: value, presentation: layers.presentation.status };
		rendered.mark(target.file.index, target.frameIndex);
		loading = false;
		loadError = null;
	}

	function setSelectedRoi(index: number | null) {
		if (index !== null && !presentedMatchesActive) return;
		edited.select(activeFile.index, index);
	}

	/** Image coordinates under a client point; the seam for cursor-driven tools. */
	function imagePointAt(clientX: number, clientY: number): ImagePoint | null {
		const origin = imageLayoutOrigin();
		if (!origin) return null;
		return clientToImagePoint(
			{ x: clientX, y: clientY },
			origin,
			activeTransform,
			orientation,
			displayGeometry,
		);
	}

	/** Moves the readout to a client point on the next animation frame. */
	function scheduleProbe(clientX: number, clientY: number): void {
		probeClientPoint = { x: clientX, y: clientY };
		if (probeAnimationFrame !== 0) return;
		probeAnimationFrame = requestAnimationFrame(() => {
			probeAnimationFrame = 0;
			const point = probeClientPoint ? imagePointAt(probeClientPoint.x, probeClientPoint.y) : null;
			probe.pixel = pixelAt(point, imageRows, imageColumns);
		});
	}

	function stopProbe(): void {
		probeClientPoint = null;
		if (probeAnimationFrame !== 0) cancelAnimationFrame(probeAnimationFrame);
		probeAnimationFrame = 0;
		probe.pixel = null;
	}

	function pointFromPointer(clientX: number, clientY: number): ImagePoint | null {
		const point = imagePointAt(clientX, clientY);
		if (!point) return null;
		return {
			x: Math.min(imageColumns, Math.max(0, point.x)),
			y: Math.min(imageRows, Math.max(0, point.y)),
		};
	}

	/** Escape: cancels a rectangle between its two clicks. False when there is none. */
	export function cancelPlacement(): boolean {
		return tools.cancelPlacement();
	}

	/** Deletes the selected ROI on this file; App's keyboard dispatcher calls it. */
	export function deleteSelectedRoi() {
		if (!presentedMatchesActive || selectedRoiIndex === null || !activeAnnotations) return;
		const next = deleteRoi(activeAnnotations, selectedRoiIndex, activeFile.frame_count);
		edited.commit(activeFile.index, next, null);
	}

	function setSelectedScope(scope: "current" | "all") {
		if (!presentedMatchesActive || selectedRoiIndex === null || !activeAnnotations) return;
		const next = setRoiFrameScope(activeAnnotations, selectedRoiIndex, scope, currentFrame, activeFile.frame_count);
		edited.commit(activeFile.index, next, selectedRoiIndex);
	}

	function clearCanvas(): void {
		rendered.clearToken();
		if (!canvasEl) return;
		const ctx = canvasEl.getContext("2d", { alpha: false });
		if (!ctx) return;
		ctx.clearRect(0, 0, canvasEl.width, canvasEl.height);
	}

	function invalidateWindowLevelRenders(): void {
		wlRenderGeneration += 1;
	}

	/**
	 * A display frame request. A real-world window stays in its unit in the
	 * fetch scope and cache key, and each frame (current, prefetched, or
	 * played by cine) is converted through its own linear mapping here, or
	 * requested in that unit.
	 */
	async function loadDisplayFrame(
		fileIndex: number,
		frameIndex: number,
		options: DisplayFrameWindowOptions = {},
		signal?: AbortSignal,
	): Promise<DisplayFrame> {
		if (!options.unit) return fetchDisplayFrame(fileIndex, frameIndex, options, signal);
		const mapping = await valueMappings.load(fileIndex, frameIndex, signal);
		signal?.throwIfAborted();
		return fetchDisplayFrame(fileIndex, frameIndex, frameDisplayWindowOptions(options, mapping), signal);
	}

	/**
	 * Whether a display request goes out with `unit`: a real-world window the
	 * frame's own linear mapping cannot convert (see `loadDisplayFrame`).
	 */
	function sendsUnit(fileIndex: number, frameIndex: number, options: DisplayFrameWindowOptions): boolean {
		if (!options.unit) return false;
		return Boolean(frameDisplayWindowOptions(options, valueMappings.forFrame(fileIndex, frameIndex)).unit);
	}

	function currentDisplayWindowOptions(): DisplayFrameWindowOptions {
		if (pipelineMode === "overlay") return {};
		if (windowUnit !== null && windowCenter !== null && windowWidth !== null) {
			return { wc: windowCenter, ww: windowWidth, windowMode: "default", unit: windowUnit };
		}
		if (renderWindowCenter !== null && renderWindowWidth !== null) {
			return { wc: renderWindowCenter, ww: renderWindowWidth, windowMode: "default" };
		}
		if (windowMode === "full_dynamic") {
			return { windowMode: "full_dynamic" };
		}
		return {};
	}

	function usesDisplayPipeline(): boolean {
		return pipelineMode !== "diagnostic_wl";
	}

	async function drawDisplayBlob(key: string, blob: Blob, generation: number): Promise<void> {
		if (!canvasEl || !usesDisplayPipeline()) return;
		const ctx = canvasEl.getContext("2d", { alpha: false });
		if (!ctx) return;

		if (typeof createImageBitmap === "function") {
			const { bitmap, release } = await displayFrames.decode(key, blob);
			try {
				if (generation !== requestGeneration || !canvasEl || !usesDisplayPipeline()) return;
				canvasEl.width = bitmap.width;
				canvasEl.height = bitmap.height;
				ctx.drawImage(bitmap, 0, 0);
			} finally {
				release();
			}
			return;
		}

		const fallbackUrl = URL.createObjectURL(blob);
		try {
			const img = new Image();
			img.decoding = "async";
			const loaded = new Promise<void>((resolve, reject) => {
				img.onload = () => resolve();
				img.onerror = () => reject(new Error("display image decode failed"));
			});
			img.src = fallbackUrl;
			await loaded;
			if (generation !== requestGeneration || !canvasEl || !usesDisplayPipeline()) return;
			canvasEl.width = img.naturalWidth;
			canvasEl.height = img.naturalHeight;
			ctx.drawImage(img, 0, 0);
		} finally {
			URL.revokeObjectURL(fallbackUrl);
		}
	}

	/**
	 * Server-windowed previews of a drag over a frame the browser does not
	 * window; the settled window is fetched as usual on release.
	 */
	const livePreview = new LiveWindowPreview({
		load: ({ wc, ww }, signal) => loadDisplayFrame(
			activeFile.index,
			currentFrame,
			{ wc, ww, windowMode: "default", unit: windowDragging ? liveWindowUnit : null, preview: true },
			signal,
		),
		show: drawPreviewFrame,
	});

	async function drawPreviewFrame({ blob, window, appliedWindow }: DisplayFrame): Promise<void> {
		const isLive = () => windowDragging && pipelineMode === "server_wl" && !!canvasEl;
		if (!isLive()) return;
		const image = await decodeCanvasImage(blob);
		try {
			if (!isLive() || !canvasEl) return;
			const ctx = canvasEl.getContext("2d", { alpha: false });
			canvasEl.width = image.width;
			canvasEl.height = image.height;
			ctx?.drawImage(image.source, 0, 0);
			shownDisplay = { window, appliedWindow };
			shownUnitRequest = windowDragging && !!liveWindowUnit && !mappedScale;
			if (presented) presented = { ...presented, window: resolvedWindow };
		} finally {
			image.dispose();
		}
	}

	function prefetchRawRing(direction: 1 | -1): void {
		const prefetchScope = retainedScopeKey;
		const frames = navigationFrames;
		const position = navigationPosition;
		scheduleIdle(() => {
			if (destroyed || retainedScopeKey !== prefetchScope || pipelineMode !== "diagnostic_wl") return;
			rawFrames.prefetch(frames, position, direction);
		});
	}

	async function loadRawFrameAndRender(fileIndex: number, frameIndex: number, generation: number, direction: 1 | -1): Promise<void> {
		const target = frameTarget(activeFile, frameIndex);
		const value = shownValueOverlay;
		let layers: PreparedFrameLayers | null = null;
		let retired = false;
		const layerRequest = frameLayers.prepare(target.file, frameIndex, true, value).then((prepared) => {
			if (retired) prepared.dispose(); else layers = prepared;
			return prepared;
		});
		try {
			const rawRequest = rawFrames.ensure(fileIndex, frameIndex);
			const pending = Promise.all([rawRequest, valueMappings.load(fileIndex, frameIndex), layerRequest]);
			trackForegroundRequest(pending, () => generation === requestGeneration, (pending) => { loading = pending; });
			const [frame, mapping, preparedLayers] = await pending;
			layers = preparedLayers;
			if (generation !== requestGeneration || pipelineMode !== "diagnostic_wl") return;
			if (validateRenderableRawFrame(frame) || !mapping) {
				if (frame.metadata.samplesPerPixel !== 1) colorFiles = { ...colorFiles, [fileIndex]: true };
				rawWindowLevelFallbackByFile = { ...rawWindowLevelFallbackByFile, [fileIndex]: true };
				return;
			}
			rawFrames.store(fileIndex, frameIndex, frame);
			preparedRaw = { target, frame, mapping, layers, generation };
			layers = null; // Owned by preparedRaw until its replacement is drawn/discarded.
			prefetchRawRing(direction);
		} catch (error) {
			if ((error as Error).name === "AbortError" || generation !== requestGeneration) return;
			loading = false;
			if (error instanceof ApiError && error.status === 422) {
				rawWindowLevelFallbackByFile = { ...rawWindowLevelFallbackByFile, [fileIndex]: true };
			} else loadError = (error as Error).message || "Failed to load frame";
		} finally { retired = true; layers?.dispose(); }
	}

	async function loadDisplayFrameAndRender(
		fileIndex: number,
		frameIndex: number,
		generation: number,
		direction: 1 | -1,
	): Promise<void> {
		const windowOptions = currentDisplayWindowOptions();
		const cacheKey = displayFrames.key(fileIndex, frameIndex, windowOptions);
		const target = frameTarget(activeFile, frameIndex);
		const value = shownValueOverlay;
		let layers: PreparedFrameLayers | null = null;
		let retired = false;
		const layerRequest = frameLayers.prepare(target.file, frameIndex, false, value).then((prepared) => {
			if (retired) prepared.dispose(); else layers = prepared;
			return prepared;
		});
		try {
			const frameRequest = displayFrames.ensureFrame(fileIndex, frameIndex, windowOptions);
			trackForegroundRequest(
				displayFrames.inFlight(cacheKey),
				() => generation === requestGeneration,
				(pending) => { loading = pending; },
			);
			const [{ blob, window, appliedWindow }, preparedLayers] = await Promise.all([frameRequest, layerRequest]);
			layers = preparedLayers;
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			loading = false;
			loadError = null;
			await drawDisplayBlob(cacheKey, blob, generation);
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			shownDisplay = { window, appliedWindow };
			shownUnitRequest = sendsUnit(fileIndex, frameIndex, windowOptions);
			const mapping = valueMappings.get(fileIndex, frameIndex);
			const windowResolution = resolveWindow({ raw: null, mapping, mode: windowMode,
				requested: windowOptions.wc != null && windowOptions.ww != null ? { window: { wc: windowOptions.wc, ww: windowOptions.ww }, unit: windowOptions.unit ?? null } : null,
				live: null, defaultWindow: target.file.default_window, server: { window, appliedWindow }, unitRequest: shownUnitRequest });
			commitFrame(target, windowResolution, layers, null, mapping, value);

			displayFrames.startPrefetch(
				navigationFrames,
				navigationPosition,
				direction,
				windowOptions,
				blob.size,
				cinePlaying ? cineMode : null,
			);
		} catch (error) {
			if ((error as Error).name === "AbortError") return;
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			loading = false;
			loadError = (error as Error).message || "Failed to load frame";
			cinePlaying = false;
		} finally { retired = true; layers?.dispose(); }
	}

	/** Composes an overlay's layers over its source frame's display image. */
	async function loadOverlayAndRender(overlay: FrameOverlay, generation: number): Promise<void> {
		// Overlays sit on the source's default presentation.
		const windowOptions: DisplayFrameWindowOptions = {};
		const target = frameTarget(activeFile, currentFrame, overlay);
		displayFrames.enterScope(windowOptions);
		loading = true;
		try {
			const [source, ...layerBlobs] = await Promise.all([
				displayFrames.ensureFrame(overlay.sourceFileIndex, overlay.sourceFrameIndex, windowOptions),
				...overlayLayerRequests(overlay).map((request) => {
					segmentationLayers.abortOthers(request.key);
					return segmentationLayers.load(request);
				}),
			]);
			const [base, ...layers] = await Promise.all([source.blob, ...layerBlobs].map(decodeCanvasImage));
			try {
				if (generation !== requestGeneration || pipelineMode !== "overlay" || !canvasEl) return;
				composeOverlayFrame(canvasEl, base, layers);
				loading = false;
				loadError = null;
				shownDisplay = { window: source.window, appliedWindow: source.appliedWindow };
				shownUnitRequest = false;
				presented = { target, raw: null, mapping: null, valueOverlay: null, presentation: "none",
					window: resolveWindow({ raw: null, mapping: null, requested: null, live: null, mode: "default",
						defaultWindow: overlay.sourceFile.default_window, server: source, unitRequest: false }) };
				valueOverlayState = null;
				rendered.mark(target.file.index, target.frameIndex);
			} finally {
				base.dispose();
				for (const layer of layers) layer.dispose();
			}
		} catch (error) {
			if ((error as Error).name === "AbortError") return;
			if (generation !== requestGeneration || pipelineMode !== "overlay") return;
			loading = false;
			loadError = (error as Error).message || "Failed to load segmentation overlay";
			cinePlaying = false;
		}
	}

	function updateTransform(transform: Omit<ViewTransform, "fit">, fit = false) {
		viewStates.setTransform(navigationScopeKey, transform, fit);
	}

	function fitTransformForViewport(): ViewTransform | null {
		const fit = fitImageToViewportHeight(
			displayGeometry,
			viewportSize.width,
			viewportSize.height,
			MIN_ZOOM,
		);
		if (!fit) return null;
		return {
			...fit,
			fit: true,
		};
	}

	function fitActiveImageToViewport(): void {
		if (!activeFile) return;
		const transform = fitTransformForViewport();
		if (!transform) return;
		updateTransform(transform, true);
	}

	function imageLayoutOrigin(): LayerOrigin | null {
		if (!viewportEl || displayGeometry.width <= 0 || displayGeometry.height <= 0) return null;
		const rect = viewportEl.getBoundingClientRect();
		return {
			left: rect.left,
			top: rect.top,
		};
	}

	/** Direction of travel for prefetch ordering: cine's, else the last frame step's. */
	function frameDirectionTo(frame: number): 1 | -1 {
		if (cinePlaying) {
			frameDirection = cineDirection;
		} else {
			if (frame > lastFrameForDirection) frameDirection = 1;
			if (frame < lastFrameForDirection) frameDirection = -1;
		}
		lastFrameForDirection = frame;
		return frameDirection;
	}

	$effect(() => {
		if (!activeFile?.has_pixels) return;
		const existing = viewStates.storedTransform(navigationScopeKey);
		if (!existing || existing.fit) fitActiveImageToViewport();
	});

	$effect(() => {
		if (!viewportEl) return;
		const element = viewportEl;
		const updateViewportSize = () => {
			const rect = element.getBoundingClientRect();
			viewportSize = { width: rect.width, height: rect.height };
		};
		updateViewportSize();
		const observer = new ResizeObserver(updateViewportSize);
		observer.observe(element);
		return () => observer.disconnect();
	});

	$effect(() => {
		const fileIndex = activeFile.index;
		const store = edited;
		untrack(() => store.ensureLoaded(fileIndex));
	});

	$effect(() => {
		const nextScope = navigationScopeKey;
		if (!nextScope || nextScope === retainedScopeKey) return;
		retainedScopeKey = nextScope;
		invalidateWindowLevelRenders();
		// The frame caches are byte-budgeted and keyed by file, so a tab's
		// frames stay for a return; only the previous tab's work stops.
		rawFrames.abortAll();
		displayFrames.resetScope();
	});

	// A rectangle left between its two clicks ends with the tool, file or frame it began on.
	$effect(() => {
		void activeTool;
		void activeFile?.index;
		void currentFrame;
		untrack(() => tools.shownChanged());
	});

	// An explicit selection supersedes an in-progress local drag as well as
	// its last preview. A released drag is already cleared by the tool host.
	$effect(() => {
		void windowCenter;
		void windowWidth;
		void windowUnit;
		void windowMode;
		untrack(() => {
			if (windowDragging) tools.endGesture();
		});
	});

	// Display PNGs contain no raw headers. Read just the polarity tag when a
	// mapped legend needs it, rather than downloading a frame's samples.
	$effect(() => {
		if (!resolvedWindow.unit || pipelineMode === "diagnostic_wl") return;
		const fileIndex = activeFile.index;
		if (untrack(() => displayPhotometric?.fileIndex) === fileIndex) return;
		const controller = new AbortController();
		void fetchSelectedTag(fileIndex, { path: "(0028,0004)" }, controller.signal)
			.then(({ value }) => {
				if (!controller.signal.aborted && value.type === "string") {
					displayPhotometric = { fileIndex, value: value.value };
				}
			}).catch(() => {});
		return () => controller.abort();
	});

	$effect(() => {
		const prepared = preparedRaw;
		return () => prepared?.layers.dispose();
	});

	// A window/level drag on a server-windowed frame shows server previews.
	$effect(() => {
		const wc = liveWindowCenter;
		const ww = liveWindowWidth;
		if (pipelineMode !== "server_wl" || !windowDragging || wc === null || ww === null) return;
		if (resolvedWindow.source === "color" || colorFiles[activeFile.index]) return;
		untrack(() => livePreview.request({ wc, ww }));
	});

	$effect(() => {
		if (!activeFile) return;
		void activeFile.index;
		livePreview.stop();
		invalidateWindowLevelRenders();
		shownDisplay = null;
		shownUnitRequest = false;
		liveWindowCenter = null;
		liveWindowWidth = null;
		untrack(() => setSelectedRoi(null));
		// Keep the previous pixels until the replacement is ready to draw.
	});

	$effect(() => {
		const mode = pipelineMode;
		// Aborting display requests updates mapping snapshots synchronously.
		// Those snapshots must not become dependencies of this mode transition:
		// their next completion would invalidate the newly prepared raw frame.
		untrack(() => {
			requestGeneration += 1;
			if (mode !== "diagnostic_wl") {
				rawFrames.stopPrefetch();
				invalidateWindowLevelRenders();
				liveWindowCenter = null;
				liveWindowWidth = null;
			} else {
				displayFrames.resetScope();
			}
		});
	});

	$effect(() => {
		const playing = cinePlaying;
		const fps = cineFps;
		const playbackMode = cineMode;
		const mode = pipelineMode;
		const frames = navigationFrames;
		const scopeKey = navigationScopeKey;
		const windowOptions = currentDisplayWindowOptions();
		if (!scopeKey) return;
		if (mode === "overlay") {
			if (playing) cinePlaying = false;
			return;
		}
		const canPlay = canRunCinePlayback(
			mode === "cine" ? "cine" : "diagnostic_wl",
			true,
			frames.length,
		);
		if (playing && !canPlay) {
			cinePlaying = false;
			return;
		}
		if (!playing || !canPlay) return;

		const ctrl = new AbortController();
		displayFrames.enterScope(windowOptions);
		playDisplayCine({
			frames,
			startPosition: untrack(() => navigationPosition),
			direction: untrack(() => cineDirection),
			mode: playbackMode,
			fps,
			windowOptions,
			display: displayFrames,
			rendered,
			signal: ctrl.signal,
			onstep: (position, direction) => {
				cineDirection = direction;
				onnavigationchange(position);
			},
		}).catch((error) => {
			if (ctrl.signal.aborted || (error as Error).name === "AbortError") return;
			loadError = (error as Error).message || "Failed to prepare cine frame";
			cinePlaying = false;
		});

		return () => ctrl.abort();
	});

	$effect(() => {
		const frameIndex = currentFrame;
		const direction = untrack(() => frameDirectionTo(frameIndex));
		if (!activeFile?.has_pixels) {
			preparedRaw = null;
			presented = null;
			loading = false;
			loadError = null;
			clearCanvas();
			return;
		}

		const mode = pipelineMode;
		const activeOverlay = overlay;
		void valueOverlayVolume;
		void valueOverlayCovers;
		const fileIndex = activeFile.index;
		const generation = ++requestGeneration;
		if (mode !== "diagnostic_wl" && mode !== "overlay") {
			// Server-rendered frames bake in the window, so window changes refetch.
			// A real-world window is converted per frame by loadDisplayFrame.
			void windowCenter;
			void windowWidth;
			void windowUnit;
			void windowMode;
		}

		loadError = null;
		untrack(() => {
			rawFrames.abortFar(navigationFrames, navigationPosition);
			displayFrames.abortFar(navigationFrames, navigationPosition);
		});
		untrack(() => {
			if (mode === "overlay" && activeOverlay) void loadOverlayAndRender(activeOverlay, generation);
			else if (mode === "diagnostic_wl") void loadRawFrameAndRender(fileIndex, frameIndex, generation, direction);
			else void loadDisplayFrameAndRender(fileIndex, frameIndex, generation, direction);
		});
	});

	$effect(() => {
		const presentation = rawPresentation;
		if (pipelineMode !== "diagnostic_wl" || !rawMatchesRequest || !preparedRaw || !currentRawFrame || !canvasEl || renderWindowPending || !presentation || !rawRenderWindow) {
			return;
		}
		// displayWindow already resolved this frame's window; window changes do
		// not invalidate in-flight renders, only frame, file, and mode changes do.
		const prepared = preparedRaw;
		const frame = currentRawFrame;
		const resolution = resolvedWindow;
		const value = shownValueOverlay;
		const { wc, ww, voiLut } = rawRenderWindow;
		const valueMap = directWindowing ? directMap : null;
		const generation = wlRenderGeneration;
		void wlRenderer.render(() => canvasEl, {
			frame,
			wc,
			ww,
			options: { valueMap, presentation, voiLut },
			isCurrent: () => generation === wlRenderGeneration
				&& prepared === preparedRaw && prepared.generation === requestGeneration
				&& pipelineMode === "diagnostic_wl",
			onRendered: () => commitFrame(prepared.target, resolution, prepared.layers, frame, prepared.mapping, value),
		});
	});

	// The displayed frame's value mapping decides whether the window is shown
	// in real-world units. It loads once the frame settles, or at once when a
	// real-world window is waiting to be converted or the raw renderer (which
	// windows a LUT mapping's values) shows the frame.
	$effect(() => {
		if (!activeFile.has_pixels || overlay) return;
		const fileIndex = activeFile.index;
		const frameIndex = currentFrame;
		if (renderWindowPending || pipelineMode === "diagnostic_wl") {
			untrack(() => valueMappings.ensure(fileIndex, frameIndex));
			return;
		}
		return untrack(() => valueMappings.ensureWhenSettled(fileIndex, frameIndex));
	});

	// A display PNG is already windowed and can be presented before metadata
	// settles. Update only its matching HUD/legend when its own mapping arrives;
	// never hold scrolling pixels behind a metadata round trip.
	$effect(() => {
		const frame = presented;
		if (!frame || frame.raw || !presentedMatchesActive || !usesDisplayPipeline()) return;
		const mapping = valueMappings.get(activeFile.index, currentFrame);
		if (!mapping || mapping === frame.mapping) return;
		const window = resolvedWindow;
		untrack(() => { presented = { ...frame, mapping, window }; });
	});

	// Without any value mapping of the file, the raw renderer cannot tell
	// how to present its samples, so the file keeps server windowing.
	$effect(() => {
		const fileIndex = activeFile.index;
		if (pipelineMode !== "diagnostic_wl" || valueMappings.knownForFile(fileIndex)) return;
		if (!valueMappings.failed(fileIndex, currentFrame)) return;
		untrack(() => {
			rawWindowLevelFallbackByFile = { ...rawWindowLevelFallbackByFile, [fileIndex]: true };
		});
	});

	// Samples and the value mapping load once the cursor rests on a frame;
	// cine playback skips them.
	$effect(() => {
		if (!probing || cinePlaying || !activeFile.has_pixels) return;
		const { file, frameIndex } = probeTarget;
		const displayed = presented?.raw ?? (currentRawFrameKey === `${file.index}:${frameIndex}` ? currentRawFrame : null);
		// A frame read one pixel at a time follows the cursor.
		const pixel = probesSinglePixels(file) ? probe.pixel : null;
		return untrack(() => {
			valueMappings.ensure(file.index, frameIndex);
			return probe.track(file, frameIndex, displayed, { pixel });
		});
	});

	// While the cursor is on the image, the shown colorwash's values for the
	// displayed frame load once, for the readout.
	$effect(() => {
		const shown = displayedValueOverlay;
		const covers = valueOverlayState?.status === "shown";
		const { file, frameIndex } = probeTarget;
		const fileIndex = file.index;
		if (!probing || cinePlaying || !covers || !shown) return;
		const request = valueOverlayValuesRequest(shown, fileIndex, frameIndex);
		const { key } = request;
		let current = true;
		if (untrack(() => overlayValueState?.key) !== key) overlayValueState = { key, state: { status: "loading" } };
		overlayValues.abortOthers(key);
		overlayValues.load(request)
			.then((values) => {
				if (current) overlayValueState = { key, state: { status: "ready", values } };
			})
			.catch((error: unknown) => {
				if (!current || (error as Error).name === "AbortError") return;
				overlayValueState = {
					key,
					state: { status: isApiError(error, "overlay_not_covering_frame") ? "not_covering" : "unavailable" },
				};
			});
		return () => {
			current = false;
		};
	});

	$effect(() => observePrefetchConcurrency((concurrency) => { prefetchConcurrency = concurrency; }));

	// Idle work scheduled before unmount must not restart prefetches after it.
	let destroyed = false;
	$effect(() => {
		return () => {
			destroyed = true;
			requestGeneration += 1;
			invalidateWindowLevelRenders();
			stopProbe();
			frameLayers.clear();
			segmentationLayers.clear();
			overlayValues.clear();
			rawFrames.clear();
			displayFrames.clear();
			livePreview.stop();
			wlRenderer.dispose();
		};
	});

	/** Fetches the frames again after the server's copy of them changed (redaction boxes). */
	function reloadFrames(): void {
		frameLayers.clear();
		rawFrames.clear();
		displayFrames.clear();
		invalidateWindowLevelRenders();
		if (!activeFile.has_pixels) return;
		const generation = ++requestGeneration;
		const fileIndex = activeFile.index;
		if (pipelineMode === "overlay" && overlay) void loadOverlayAndRender(overlay, generation);
		else if (pipelineMode === "diagnostic_wl") void loadRawFrameAndRender(fileIndex, currentFrame, generation, frameDirection);
		else void loadDisplayFrameAndRender(fileIndex, currentFrame, generation, frameDirection);
	}

	let seriesRedactionError = $state<string | null>(null);
	/** Copies this file's redaction boxes to the same-sized files of its series. */
	async function applyRedactionsToSeriesFiles(): Promise<void> {
		seriesRedactionError = null;
		try {
			const { file_indices } = await applyRedactionsToSeries(activeFile.index);
			for (const fileIndex of file_indices) redactions.forget(fileIndex);
			reloadFrames();
		} catch (error) {
			seriesRedactionError = error instanceof Error ? error.message : "Failed to apply redaction boxes to the series";
		}
	}

	/** Retry failed requests without discarding successful frames or unsaved ROIs. */
	export async function retryFailedLoads(): Promise<void> {
		const fileIndex = activeFile.index;
		const frameIndex = currentFrame;
		if (edited.error(fileIndex)) {
			if (edited.ready(fileIndex)) edited.retrySave(fileIndex);
			else edited.retryLoad(fileIndex);
		}
		if (annotatedFrame) annotationFrames.retry(annotatedFrame);
		const mappingFailed = valueMappings.failed(fileIndex, frameIndex);
		if (!loadError && !mappingFailed && presented?.presentation !== "error" && valueOverlayState?.status !== "error") return;
		loadError = null;
		const generation = ++requestGeneration;
		if (mappingFailed) {
			await valueMappings.load(fileIndex, frameIndex);
			if (generation !== requestGeneration) return;
			const { [fileIndex]: _fallback, ...rest } = rawWindowLevelFallbackByFile;
			rawWindowLevelFallbackByFile = rest;
		}
		// An error placeholder removes the canvas; restore it before drawing.
		await tick();
		if (generation !== requestGeneration || !activeFile.has_pixels) return;
		if (pipelineMode === "overlay" && overlay) await loadOverlayAndRender(overlay, generation);
		else if (pipelineMode === "diagnostic_wl") await loadRawFrameAndRender(fileIndex, frameIndex, generation, frameDirection);
		else await loadDisplayFrameAndRender(fileIndex, frameIndex, generation, frameDirection);
	}

	/** Refits the image and drops any in-progress window/level or drag. */
	export function resetView(): void {
		invalidateWindowLevelRenders();
		fitActiveImageToViewport();
		liveWindowCenter = null;
		liveWindowWidth = null;
		tools.endGesture();
	}

	function zoomAnchorFromClient(clientX: number, clientY: number): ZoomAnchor | null {
		const origin = imageLayoutOrigin();
		return origin ? zoomAnchor(clientX, clientY, origin, activeTransform) : null;
	}

	function zoomTransformForAnchor(newScale: number, anchor: ZoomAnchor): Omit<ViewTransform, "fit"> | null {
		const origin = imageLayoutOrigin();
		return origin ? zoomAroundAnchor(newScale, anchor, origin) : null;
	}

	function zoomAt(newScale: number, clientX: number, clientY: number) {
		if (!activeFile || !canvasEl) return;
		const anchor = zoomAnchorFromClient(clientX, clientY);
		if (!anchor) return;
		const transform = zoomTransformForAnchor(newScale, anchor);
		if (!transform) return;
		updateTransform(transform);
	}

	function windowDragStep(baseWindow: { wc: number; ww: number }): number {
		if (frameMapping && frameMapping.stored_value_type !== "integer") {
			// Continuous samples have no one-unit quantum. Use the data range,
			// independent of the manual window, to keep subsequent drags stable.
			const automatic = resolveWindow({
				raw: rawMatchesRequest ? currentRawFrame : null,
				mapping: frameMapping, requested: null, live: null,
				mode: "full_dynamic", defaultWindow: activeFile.default_window,
				server: shownDisplay, unitRequest: false,
			}).window ?? baseWindow;
			return Math.max(Number.MIN_VALUE, Math.abs(automatic.wc) * Number.EPSILON, automatic.ww / 256);
		}
		if (resolvedWindow.unit && frameMapping?.real_world[0]) {
			return mappedScale ? Math.abs(mappedScale.ratio) : mappedUnitsPerStoredUnit(frameMapping.real_world[0]);
		}
		// One stored unit: a fractional rescale steps by its slope, not by whole Modality units.
		if (frameMapping && !hasIntegerModality(frameMapping.stored_value_type, frameMapping.modality)) {
			const slope = Math.abs(frameMapping.modality.rescale_slope);
			if (Number.isFinite(slope) && slope > 0) return slope;
		}
		return 1;
	}

	/** Starts a window/level drag's live window at the displayed one, when this frame can be windowed now. */
	function beginWindowDrag(): WindowDragBase | null {
		if (overlay) return null;
		if (presented && (presented.target.file.index !== activeFile.index || presented.target.frameIndex !== currentFrame)) return null;
		if (resolvedWindow.source === "color" || (pipelineMode === "diagnostic_wl" ? !currentRawFrame : !shownDisplay)) return null;
		const baseWindow = displayWindow ?? { wc: 0, ww: 1 };
		const step = windowDragStep(baseWindow);
		liveWindowUnit = resolvedWindow.unit;
		liveWindowCenter = baseWindow.wc;
		liveWindowWidth = baseWindow.ww;
		return { center: baseWindow.wc, width: baseWindow.ww, step };
	}

	/** The tool host ended a gesture: drop the live window and its previews. */
	function gestureEnded() {
		liveWindowCenter = null;
		liveWindowWidth = null;
		liveWindowUnit = null;
		livePreview.stop();
		edited.endLiveEdit();
	}

	function onPointerMove(event: PointerEvent) {
		if (activeFile?.has_pixels && !isViewportChromeTarget(event.target)) {
			scheduleProbe(event.clientX, event.clientY);
		} else if (!tools.dragging) {
			stopProbe();
		}
		tools.pointerMove(event);
	}

	function onContextMenu(event: MouseEvent) {
		event.preventDefault();
	}

	function zoomToLevel(level: number) {
		if (!activeFile || !activeFile.has_pixels) return;
		const rect = viewportEl?.getBoundingClientRect();
		const cx = rect ? rect.left + rect.width / 2 : 0;
		const cy = rect ? rect.top + rect.height / 2 : 0;
		zoomAt(level, cx, cy);
	}

	function stepZoom(direction: 1 | -1) {
		if (!activeFile) return;
		const level = nextZoomStep(activeTransform.scale, direction);
		if (level !== undefined) zoomToLevel(level);
	}
</script>

<section
	bind:this={viewportEl}
	class="viewport"
	data-theme="dark"
	class:dragging={isDragging}
	data-tool={activeTool}
	role="application"
	onwheel={(event) => tools.wheel(event)}
	onpointerdown={(event) => tools.pointerDown(event)}
	onpointermove={onPointerMove}
	onpointerup={(event) => tools.pointerUp(event)}
	onpointercancel={(event) => tools.pointerCancel(event)}
	onpointerleave={stopProbe}
	oncontextmenu={onContextMenu}
	ondblclick={onreset}
>
	{#if !activeFile}
		<div class="placeholder">No file selected</div>
	{:else if !activeFile.has_pixels}
		<div class="placeholder">No pixel data</div>
	{:else if loadError}
		<div class="placeholder">{loadError}</div>
	{:else}
		{#if loading}
			<div class="frame-request-indicator" role="status">
				<span class="loading-wheel" aria-hidden="true"></span>
				<span class="visually-hidden">Loading frame</span>
			</div>
		{/if}
		<div
			class="image-layer"
			style={`transform:${transformCss}; width:${Math.max(displayGeometry.width, 1)}px; height:${Math.max(displayGeometry.height, 1)}px;`}
		>
			<canvas
				bind:this={canvasEl}
				class="dicom-canvas"
				data-capture-rendered={rendered.token}
			></canvas>
			<canvas bind:this={presentationLayerCanvas} class="layer-canvas" hidden={presented?.presentation !== "shown"} aria-hidden="true"></canvas>
			{#if valueOverlayVolume || displayedValueOverlay}
				<canvas
					bind:this={valueOverlayCanvas}
					class="value-overlay-canvas"
					hidden={valueOverlayState?.status !== "shown"}
					style:opacity={shownValueOverlay?.opacity ?? displayedValueOverlay?.opacity ?? 0}
					aria-hidden="true"
				></canvas>
			{/if}
			{#if shownAnnotations && imageColumns > 0 && imageRows > 0}
				<GraphicAnnotationOverlay
					annotations={shownAnnotations}
					highlightedItem={graphicAnnotation?.highlightedItem ?? null}
					rows={imageRows}
					columns={imageColumns}
					scale={activeTransform.scale}
				/>
			{/if}
			{#if !overlay && imageColumns > 0 && imageRows > 0}
				<RoiOverlay
					rois={visibleRois}
					selectedIndex={selectedRoiIndex}
					draft={draftRoi}
					rows={imageRows}
					columns={imageColumns}
					scale={activeTransform.scale}
					pixelAspectRatio={displayGeometry.pixelAspectRatio}
				/>
			{/if}
		</div>
		{#if shownAnnotations && imageColumns > 0 && imageRows > 0}
			<GraphicAnnotationLabels
				annotations={shownAnnotations}
				highlightedItem={graphicAnnotation?.highlightedItem ?? null}
				transform={activeTransform}
				{orientation}
				geometry={displayGeometry}
			/>
		{/if}
		{#if !overlay && imageColumns > 0 && imageRows > 0}
			<RoiLabels
				rois={visibleRois}
				selectedIndex={selectedRoiIndex}
				transform={activeTransform}
				{orientation}
				geometry={displayGeometry}
			/>
		{/if}
		<div class="hud">
			{#if readout}
				<PixelReadout {readout} />
			{/if}
			<div class="overlay">
				{#if presented?.target.segmentation}
					<span>SEG overlay {presented.target.frameIndex + 1} / {presented.target.file.frame_count}</span>
					<span>source frame {presented.target.imageFrameIndex + 1}</span>
				{:else}
					<span>image {(presented?.target.position ?? navigationPosition) + 1} / {presented?.target.totalFrames ?? navigationFrameCount}</span>
					<span>source frame {(presented?.target.frameIndex ?? currentFrame) + 1} / {presented?.target.file.frame_count ?? activeFile.frame_count}</span>
				{/if}
				{#if windowLegend}
					<span class="mapped-window">
						W: {formatValue(windowLegend.width)} · C: {formatValue(windowLegend.center)} {windowLegend.unit}
					</span>
				{:else if hudWindow.source === "voi_lut"}
					<span>VOI LUT</span>
				{:else if hudWindow.window}
					{@const shown = formatWindow(hudWindow.window)}
					<span>W: {shown.width} · C: {shown.center}</span>
				{/if}
				{#if presented?.presentation === "error"}
					<span>Presentation layer unavailable</span>
				{/if}
				{#if activeTool === "window_level" && !activeFile.raw_windowing_compatible}
					<span class="presentation-path" title={activeFile.raw_windowing_reason ?? undefined}>server presentation retained</span>
				{/if}
			</div>
		</div>
		{#if !overlay}
			<RoiList
				noun={redacting ? "redaction" : "ROI"}
				onapplytoseries={redacting ? () => void applyRedactionsToSeriesFiles() : undefined}
				rois={visibleRois}
				totalCount={activeAnnotations?.num_roi ?? null}
				frameCount={activeFile.frame_count}
				selectedIndex={selectedRoiIndex}
				loading={edited.loading(activeFile.index)}
				error={(redacting ? seriesRedactionError : null) ?? edited.error(activeFile.index)}
				ready={annotationsReady}
				saveStatus={edited.saveStatus(activeFile.index)}
				onselect={setSelectedRoi}
				onscope={setSelectedScope}
				ondelete={deleteSelectedRoi}
				onretryload={() => edited.retryLoad(activeFile.index)}
				onretrysave={() => edited.retrySave(activeFile.index)}
				onrevert={() => edited.rollback(activeFile.index)}
			/>
		{/if}
		{#if windowLegend || displayedValueOverlay}
			<div class="legends">
				{#if displayedValueOverlay}
					<ValueLegend
						title={displayedValueOverlay.title}
						unit={displayedValueOverlay.legend.unit_label}
						low={displayedValueOverlay.legend.min_value}
						high={displayedValueOverlay.legend.max_value}
						colors={legendColors(displayedValueOverlay.legend)}
						caption={valueOverlayCaption}
					/>
				{/if}
				{#if windowLegend}
					<ValueLegend
						title={windowLegend.label ?? "Window"}
						unit={windowLegend.unit}
						low={windowLegend.low}
						high={windowLegend.high}
						colors={windowColors}
					/>
				{/if}
			</div>
		{/if}
		<ZoomControls scale={activeTransform.scale} onstep={stepZoom} onfit={fitActiveImageToViewport} />
	{/if}
</section>

<style>
	.viewport {
		position: relative;
		display: grid;
		place-items: center;
		background: var(--viewport);
		min-height: 0;
		overflow: hidden;
		user-select: none;
		touch-action: none;
	}
	.viewport[data-tool="window_level"] { cursor: crosshair; }
	.viewport[data-tool="pan"] { cursor: grab; }
	.viewport[data-tool="pan"]:active { cursor: grabbing; }
	.viewport[data-tool="zoom"] { cursor: zoom-in; }
	.viewport[data-tool="scroll"] { cursor: ns-resize; }
	.viewport[data-tool="annotate_rect"],
	.viewport[data-tool="redact"] { cursor: crosshair; }
	.viewport.dragging { cursor: grabbing; }
	.image-layer {
		position: absolute;
		left: 0;
		top: 0;
		transform-origin: 0 0;
		transition: transform 0.03s linear;
	}
	.layer-canvas,
	.value-overlay-canvas {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		image-rendering: pixelated;
		pointer-events: none;
	}
	.layer-canvas[hidden],
	.value-overlay-canvas[hidden] {
		display: none;
	}
	.dicom-canvas {
		display: block;
		width: 100%;
		height: 100%;
		image-rendering: pixelated;
	}
	.placeholder {
		color: var(--ink-muted);
		font: var(--t-ui);
	}

	.frame-request-indicator {
		position: absolute;
		top: 12px;
		left: 12px;
		z-index: 2;
		display: grid;
		place-items: center;
		width: 28px;
		height: 28px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
	}

	.loading-wheel {
		width: 14px;
		height: 14px;
		box-sizing: border-box;
		border: 2px solid var(--track);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: frame-request-spin 0.78s linear infinite;
	}

	.visually-hidden {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	@keyframes frame-request-spin {
		to { transform: rotate(360deg); }
	}

	@media (prefers-reduced-motion: reduce) {
		.loading-wheel {
			animation-duration: 1.8s;
		}
	}

	.legends {
		position: absolute;
		right: 12px;
		top: 50%;
		transform: translateY(-50%);
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 10px;
	}

	.hud {
		position: absolute;
		left: 12px;
		bottom: 12px;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 6px;
		max-width: calc(100% - 9.5rem);
		pointer-events: none;
	}

	.overlay {
		display: flex;
		flex-wrap: wrap;
		gap: 3px 12px;
		padding: 6px 9px;
		background: var(--paper);
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		box-shadow: var(--elev-overlay);
		color: var(--text);
		font: var(--t-mono);
		font-variant-numeric: tabular-nums;
		pointer-events: auto;
	}

	.presentation-path {
		color: var(--ink-muted);
		font-family: var(--font-ui);
		font-size: 11px;
	}
</style>

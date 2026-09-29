<script lang="ts">
	import { untrack } from "svelte";
	import {
		ApiError,
		fetchDisplayFrame,
		fetchSelectedTag,
		isApiError,
		type DisplayFrame,
		type DisplayFrameWindowOptions,
		type FileSummary,
		type RawFrame,
		type WindowMode,
	} from "../api";
	import {
		addRoi,
		canonicalRect,
		deleteRoi,
		moveCoord,
		normalizeAnnotationsForEdit,
		resizeCoord,
		setRoiFrameScope,
		updateRoiCoord,
		type ImagePoint,
		type RoiCoord,
		type RoiHandle,
	} from "./annotationGeometry";
	import { canRunCinePlayback, type CineDirection, type CineMode } from "./cinePlayback";
	import { fitImageToViewportHeight, imageDisplayGeometry } from "./imageGeometry";
	import {
		mappedUnitsPerStoredUnit,
		MAX_RENDER_PIXELS,
		samplePresentation,
		selectWindowingPipeline,
		validateRenderableRawFrame,
		type ResolvedWindow,
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
		presentationLayerRequest,
		valueOverlayLayerRequest,
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
	import { RawFrameSource } from "./viewport/rawFrameSource";
	import { RenderedFrames } from "./viewport/renderedFrames.svelte";
	import { resolveWindow } from "./viewport/resolveWindow";
	import RoiList from "./viewport/RoiList.svelte";
	import RoiLabels from "./viewport/RoiLabels.svelte";
	import RoiOverlay from "./viewport/RoiOverlay.svelte";
	import { hitTestRoi, roiCoord, visibleRois as roisOnFrame } from "./viewport/roiEditing";
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
		frameDisplayWindowOptions,
		mappedWindowScale,
		pixelAt,
		windowToRender,
	} from "./viewport/valueMapping";
	import { ValueMappings } from "./viewport/valueMappings.svelte";
	import { WlRendererClient } from "./viewport/wlRendererClient";
	import ZoomControls from "./viewport/ZoomControls.svelte";

	type PipelineMode = "cine" | "diagnostic_wl" | "server_wl" | "overlay";
	type DragState =
		| { mode: "pan"; startX: number; startY: number; baseTx: number; baseTy: number }
		| { mode: "wl"; startX: number; startY: number; baseCenter: number; baseWidth: number; step: number; unit: string | null }
		| { mode: "zoom_drag"; startY: number; baseScale: number; anchor: ZoomAnchor }
		| { mode: "scroll_drag"; startY: number; baseFrame: number }
		| { mode: "draw_roi"; start: ImagePoint; current: ImagePoint }
		| { mode: "move_roi"; roiIndex: number; start: ImagePoint; original: RoiCoord }
		| { mode: "resize_roi"; roiIndex: number; handle: RoiHandle; original: RoiCoord }
		| null;

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
	} = $props();

	let dragState = $state<DragState>(null);
	let loading = $state(false);
	let loadError = $state<string | null>(null);
	let liveWindowCenter = $state<number | null>(null);
	let liveWindowWidth = $state<number | null>(null);
	let viewportEl: HTMLElement | undefined = $state();
	let viewportSize = $state({ width: 0, height: 0 });
	let canvasEl: HTMLCanvasElement | undefined = $state();
	// Raw, not a deep proxy: the frame is posted to the W/L worker, and a
	// proxied metadata object cannot be structured-cloned.
	let currentRawFrame = $state.raw<RawFrame | null>(null);
	let currentRawFrameKey = $state("");
	let displayPhotometric = $state.raw<{ fileIndex: number; value: string } | null>(null);
	// The window the server rendered the displayed PNG with, if linear, and
	// whether that PNG was requested in a real-world unit.
	let shownDisplay = $state.raw<Pick<DisplayFrame, "window" | "appliedWindow"> | null>(null);
	let shownUnitRequest = $state(false);
	let rawWindowLevelFallbackByFile = $state<Record<number, boolean>>({});
	// Color files: neither path windows them, so a drag sends no previews.
	let colorFiles = $state<Record<number, boolean>>({});
	const annotations = new AnnotationStore();

	let prefetchConcurrency = $state(PREFETCH_CONCURRENCY);
	const rendered = new RenderedFrames();
	const rawFrames = new RawFrameSource({
		concurrency: () => prefetchConcurrency,
		prepare: (fileIndex, frameIndex, signal) => valueMappings.load(fileIndex, frameIndex, signal),
	});
	const displayFrames = new DisplayFrameSource({
		load: loadDisplayFrame,
		navigationScope: () => navigationScopeKey,
		concurrency: () => prefetchConcurrency,
		onScopeChange: () => rendered.reset(),
	});
	const valueMappings = new ValueMappings();
	const overlayLayers = new OverlayLayerCache();
	const presentationLayers = new OverlayLayerCache();
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

	const FRAME_SCROLL_SPEED_FACTOR = 0.7;
	const DRAG_PIXELS_PER_FRAME = 10 / FRAME_SCROLL_SPEED_FACTOR;
	const TRACKPAD_WHEEL_DELTA_THRESHOLD = 50;
	const MOUSE_WHEEL_ZOOM_SENSITIVITY = 0.0025;
	const PINCH_ZOOM_SENSITIVITY = 0.01;
	// Zoom, pan, and orientation belong to the open tab (navigation scope).
	const activeTransform = $derived(viewStates.transform(activeFile ? navigationScopeKey : ""));
	const orientation = $derived(viewStates.orientation(navigationScopeKey));
	const isDragging = $derived(dragState !== null);
	// The displayed frame's value mapping (or the file's latest while that
	// frame's loads) decides whether the window is in real-world units.
	const frameMapping = $derived(valueMappings.forFrame(activeFile.index, currentFrame));
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
	const rawPresentation = $derived.by(() => {
		if (pipelineMode !== "diagnostic_wl" || !currentRawFrame) return null;
		const mapping = valueMappings.forFrame(activeFile.index, currentFrame);
		return mapping ? samplePresentation(currentRawFrame, mapping) : null;
	});
	// A browser-windowed frame gets its shutter and overlay graphics drawn over it.
	const showsPresentationLayer = $derived(pipelineMode === "diagnostic_wl" && activeFile.presentation_layer);

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
		raw: pipelineMode === "diagnostic_wl" ? currentRawFrame : null,
		mapping: overlay ? null : frameMapping,
		requested: windowCenter !== null && windowWidth !== null
			? { window: { wc: windowCenter, ww: windowWidth }, unit: windowUnit } : null,
		live: dragState?.mode === "wl" && liveWindowCenter !== null && liveWindowWidth !== null
			? { window: { wc: liveWindowCenter, ww: liveWindowWidth }, unit: dragState.unit } : null,
		mode: windowMode,
		defaultWindow: overlay?.sourceFile.default_window ?? activeFile.default_window,
		server: shownDisplay,
		unitRequest: shownUnitRequest,
	}));
	const displayWindow = $derived(resolvedWindow.window);
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
		if (!displayWindow || !resolvedWindow.unit) return null;
		return {
			center: displayWindow.wc, width: displayWindow.ww,
			low: displayWindow.wc - displayWindow.ww / 2,
			high: displayWindow.wc + displayWindow.ww / 2,
			unit: resolvedWindow.unit, label: frameMapping?.real_world[0]?.label ?? null,
		};
	});
	const windowColors = $derived.by(() => {
		const photometric = currentRawFrameKey === `${activeFile.index}:${currentFrame}` && currentRawFrame
			? currentRawFrame.metadata.photometricInterpretation
			: displayPhotometric?.fileIndex === activeFile.index ? displayPhotometric.value : "";
		const mono1 = photometric.trim().toUpperCase() === "MONOCHROME1";
		const decreasing = mappedScale !== null && mappedScale.ratio < 0;
		return mono1 !== decreasing ? ["#fff", "#000"] : ["#000", "#fff"];
	});

	// The colorwash layer depends only on which volume is shown and whether
	// it covers the frame; opacity is applied to the drawn layer.
	const shownValueOverlay = $derived(overlay || !activeFile.has_pixels ? null : valueOverlay);
	const valueOverlayVolume = $derived(
		shownValueOverlay ? `${shownValueOverlay.kind}:${shownValueOverlay.volumeFileIndex}` : null,
	);
	const valueOverlayCovers = $derived(shownValueOverlay?.coversFrame ?? false);
	const valueOverlayCaption = $derived.by(() => {
		if (!shownValueOverlay) return null;
		const { legend } = shownValueOverlay;
		if (!valueOverlayCovers || valueOverlayState?.status === "not_covering") return "Not covering this frame";
		if (valueOverlayState?.status === "error") return "Overlay unavailable for this frame";
		return legend.transparent_at_or_below === null
			? null
			: `≤ ${formatValue(legend.transparent_at_or_below)} ${legend.unit_label} transparent`;
	});

	const activeAnnotations = $derived(annotations.annotations(activeFile.index));
	const annotationsReady = $derived(annotations.ready(activeFile.index));
	const selectedRoiIndex = $derived(annotations.selected(activeFile.index));
	const imageRows = $derived(
		pipelineMode === "overlay" && overlay
			? overlay.sourceFile.rows
			: pipelineMode === "diagnostic_wl" && currentRawFrame
			? currentRawFrame.metadata.rows
			: activeFile?.rows ?? 0,
	);
	const imageColumns = $derived(
		pipelineMode === "overlay" && overlay
			? overlay.sourceFile.columns
			: pipelineMode === "diagnostic_wl" && currentRawFrame
			? currentRawFrame.metadata.columns
			: activeFile?.columns ?? 0,
	);
	const displayGeometry = $derived(
		imageDisplayGeometry(
			imageRows,
			imageColumns,
			overlay?.sourceFile.pixel_aspect_ratio ?? activeFile?.pixel_aspect_ratio,
		),
	);
	const transformCss = $derived(layerTransformCss(activeTransform, orientation, displayGeometry));
	const visibleRois = $derived(
		overlay ? [] : roisOnFrame(activeAnnotations, currentFrame),
	);
	const draftRoi = $derived(
		dragState?.mode === "draw_roi"
			? canonicalRect(dragState.start, dragState.current, imageRows, imageColumns)
			: null,
	);

	// The readout reads the image on screen: a SEG overlay's source frame,
	// otherwise the active file's frame.
	const probeTarget = $derived(
		overlay
			? { file: overlay.sourceFile, frameIndex: overlay.sourceFrameIndex }
			: { file: activeFile, frameIndex: currentFrame },
	);
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
		if (!shownValueOverlay || cinePlaying) return values;
		const key = valueOverlayValuesRequest(shownValueOverlay, activeFile.index, currentFrame).key;
		const state: OverlayValueState = !valueOverlayCovers
			? { status: "not_covering" }
			: overlayValueState?.key === key ? overlayValueState.state : { status: "loading" };
		const label = shownValueOverlay.kind === "rt_dose" ? "dose" : "map";
		return {
			...values,
			overlay: overlayValueReadout(label, shownValueOverlay.legend.unit_label, pixel, activeFile.columns, state),
		};
	});

	function setSelectedRoi(index: number | null) {
		annotations.select(activeFile.index, index);
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

	function pointFromPointer(event: PointerEvent): ImagePoint | null {
		const point = imagePointAt(event.clientX, event.clientY);
		if (!point) return null;
		return {
			x: Math.min(imageColumns, Math.max(0, point.x)),
			y: Math.min(imageRows, Math.max(0, point.y)),
		};
	}

	/** Deletes the selected ROI on this file; App's keyboard dispatcher calls it. */
	export function deleteSelectedRoi() {
		if (selectedRoiIndex === null || !activeAnnotations) return;
		const next = deleteRoi(activeAnnotations, selectedRoiIndex, activeFile.frame_count);
		annotations.commit(activeFile.index, next, null);
	}

	function setSelectedScope(scope: "current" | "all") {
		if (selectedRoiIndex === null || !activeAnnotations) return;
		const next = setRoiFrameScope(activeAnnotations, selectedRoiIndex, scope, currentFrame, activeFile.frame_count);
		annotations.commit(activeFile.index, next, selectedRoiIndex);
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
			{ wc, ww, windowMode: "default", unit: dragState?.mode === "wl" ? dragState.unit : null, preview: true },
			signal,
		),
		show: drawPreviewFrame,
	});

	async function drawPreviewFrame({ blob, window, appliedWindow }: DisplayFrame): Promise<void> {
		const isLive = () => dragState?.mode === "wl" && pipelineMode === "server_wl" && !!canvasEl;
		if (!isLive()) return;
		const image = await decodeCanvasImage(blob);
		try {
			if (!isLive() || !canvasEl) return;
			const ctx = canvasEl.getContext("2d", { alpha: false });
			canvasEl.width = image.width;
			canvasEl.height = image.height;
			ctx?.drawImage(image.source, 0, 0);
			shownDisplay = { window, appliedWindow };
			shownUnitRequest = dragState?.mode === "wl" && !!dragState.unit && !mappedScale;
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

	async function loadRawFrameAndRender(
		fileIndex: number,
		frameIndex: number,
		generation: number,
		direction: 1 | -1,
	): Promise<void> {
		const cached = rawFrames.cached(fileIndex, frameIndex);
		if (cached) {
			currentRawFrame = cached;
			currentRawFrameKey = `${fileIndex}:${frameIndex}`;
			loading = false;
			loadError = null;
			prefetchRawRing(direction);
			return;
		}

		try {
			const rawFrameRequest = rawFrames.ensure(fileIndex, frameIndex);
			trackForegroundRequest(
				rawFrames.inFlight(fileIndex, frameIndex),
				() => generation === requestGeneration,
				(pending) => { loading = pending; },
			);
			const rawFrame = await rawFrameRequest;
			if (generation !== requestGeneration || pipelineMode !== "diagnostic_wl") return;
			loading = false;
			const validationError = validateRenderableRawFrame(rawFrame);
			if (validationError) {
				currentRawFrame = null;
				if (rawFrame.metadata.samplesPerPixel !== 1) colorFiles = { ...colorFiles, [fileIndex]: true };
				rawWindowLevelFallbackByFile = {
					...rawWindowLevelFallbackByFile,
					[fileIndex]: true,
				};
				return;
			}
			rawFrames.store(fileIndex, frameIndex, rawFrame);
			currentRawFrame = rawFrame;
			currentRawFrameKey = `${fileIndex}:${frameIndex}`;
			loading = false;
			loadError = null;
			prefetchRawRing(direction);
		} catch (error) {
			if ((error as Error).name === "AbortError") {
				if (generation === requestGeneration) loading = false;
				return;
			}
			if (generation !== requestGeneration || pipelineMode !== "diagnostic_wl") return;
			loading = false;
			currentRawFrame = null;
			// Only a layout the raw endpoint cannot serve moves the file to
			// server windowing for good; a dropped request or a failed frame is
			// this frame's error, and the next frame tries raw samples again.
			if (error instanceof ApiError && error.status === 422) {
				rawWindowLevelFallbackByFile = {
					...rawWindowLevelFallbackByFile,
					[fileIndex]: true,
				};
			} else {
				loadError = (error as Error).message || "Failed to load frame";
			}
		}
	}

	async function loadDisplayFrameAndRender(
		fileIndex: number,
		frameIndex: number,
		generation: number,
		direction: 1 | -1,
	): Promise<void> {
		const windowOptions = currentDisplayWindowOptions();
		const cacheKey = displayFrames.key(fileIndex, frameIndex, windowOptions);
		try {
			const frameRequest = displayFrames.ensureFrame(fileIndex, frameIndex, windowOptions);
			trackForegroundRequest(
				displayFrames.inFlight(cacheKey),
				() => generation === requestGeneration,
				(pending) => { loading = pending; },
			);
			const { blob, window, appliedWindow } = await frameRequest;
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			loading = false;
			loadError = null;
			await drawDisplayBlob(cacheKey, blob, generation);
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			shownDisplay = { window, appliedWindow };
			shownUnitRequest = sendsUnit(fileIndex, frameIndex, windowOptions);
			rendered.mark(fileIndex, frameIndex);

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
		}
	}

	/** Composes an overlay's layers over its source frame's display image. */
	async function loadOverlayAndRender(overlay: FrameOverlay, generation: number): Promise<void> {
		// Overlays sit on the source's default presentation.
		const windowOptions: DisplayFrameWindowOptions = {};
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
				rendered.mark(activeFile.index, currentFrame);
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

	function isViewportChromeTarget(target: EventTarget | null): boolean {
		return target instanceof Element && !!target.closest(".zoom-controls, .roi-list");
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
		untrack(() => annotations.ensureLoaded(fileIndex));
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

	// An explicit selection supersedes an in-progress local drag as well as
	// its last preview. A released drag is already cleared by endDrag.
	$effect(() => {
		void windowCenter;
		void windowWidth;
		void windowUnit;
		void windowMode;
		untrack(() => {
			if (dragState?.mode === "wl") endDrag();
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

	// A window/level drag on a server-windowed frame shows server previews.
	$effect(() => {
		const wc = liveWindowCenter;
		const ww = liveWindowWidth;
		if (pipelineMode !== "server_wl" || dragState?.mode !== "wl" || wc === null || ww === null) return;
		if (resolvedWindow.source === "color" || colorFiles[activeFile.index]) return;
		untrack(() => livePreview.request({ wc, ww }));
	});

	$effect(() => {
		if (!activeFile) return;
		void activeFile.index;
		livePreview.stop();
		invalidateWindowLevelRenders();
		currentRawFrame = null;
		shownDisplay = null;
		shownUnitRequest = false;
		liveWindowCenter = null;
		liveWindowWidth = null;
		untrack(() => setSelectedRoi(null));
		// Keep the previous pixels until the replacement is ready to draw.
	});

	$effect(() => {
		const mode = pipelineMode;
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
			currentRawFrame = null;
			loading = false;
			loadError = null;
			clearCanvas();
			return;
		}

		const mode = pipelineMode;
		const activeOverlay = overlay;
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
		if (mode === "overlay" && activeOverlay) {
			void loadOverlayAndRender(activeOverlay, generation);
		} else if (mode === "diagnostic_wl") {
			void loadRawFrameAndRender(fileIndex, frameIndex, generation, direction);
		} else {
			void loadDisplayFrameAndRender(fileIndex, frameIndex, generation, direction);
		}
	});

	$effect(() => {
		const presentation = rawPresentation;
		if (pipelineMode !== "diagnostic_wl" || !currentRawFrame || !canvasEl || renderWindowPending || !presentation || !rawRenderWindow) {
			return;
		}
		// displayWindow already resolved this frame's window; window changes do
		// not invalidate in-flight renders, only frame, file, and mode changes do.
		const frame = currentRawFrame;
		const { wc, ww, voiLut } = rawRenderWindow;
		const valueMap = directWindowing ? directMap : null;
		const generation = wlRenderGeneration;
		void wlRenderer.render(() => canvasEl, {
			frame,
			wc,
			ww,
			options: { valueMap, presentation, voiLut },
			isCurrent: () => generation === wlRenderGeneration
				&& frame === currentRawFrame
				&& pipelineMode === "diagnostic_wl",
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

	// Draws the displayed frame's shutter and overlay graphics over the
	// browser-windowed image; the last layer stays until the next one loads,
	// as the image does.
	$effect(() => {
		const canvas = presentationLayerCanvas;
		if (!showsPresentationLayer || !canvas) return;
		const request = presentationLayerRequest(activeFile.index, currentFrame);
		let current = true;
		presentationLayers.abortOthers(request.key);
		presentationLayers.load(request)
			.then(decodeCanvasImage)
			.then((layer) => {
				try {
					if (current) drawOverlayLayer(canvas, layer);
				} finally {
					layer.dispose();
				}
			})
			.catch((error: unknown) => {
				if (!current || (error as Error).name === "AbortError") return;
				loadError = (error as Error).message || "Failed to load the frame's shutter and overlays";
			});
		return () => {
			current = false;
		};
	});

	// Samples and the value mapping load once the cursor rests on a frame;
	// cine playback skips them.
	$effect(() => {
		if (!probing || cinePlaying || !activeFile.has_pixels) return;
		const { file, frameIndex } = probeTarget;
		const displayed = pipelineMode === "diagnostic_wl" && currentRawFrameKey === `${file.index}:${frameIndex}`
			? currentRawFrame : null;
		// A frame read one pixel at a time follows the cursor.
		const pixel = probesSinglePixels(file) ? probe.pixel : null;
		return untrack(() => {
			valueMappings.ensure(file.index, frameIndex);
			return probe.track(file, frameIndex, displayed, { pixel });
		});
	});

	// Fetches the displayed frame's colorwash and draws it on its own canvas.
	// A frame the volume does not reach shows no layer and a legend note.
	$effect(() => {
		const volume = valueOverlayVolume;
		const covers = valueOverlayCovers;
		const canvas = valueOverlayCanvas;
		const fileIndex = activeFile.index;
		const frameIndex = currentFrame;
		const shown = untrack(() => shownValueOverlay);
		if (!volume || !shown) {
			valueOverlayState = null;
			return;
		}
		const request = valueOverlayLayerRequest(shown, fileIndex, frameIndex);
		const key = request.key;
		if (!covers) {
			valueOverlayState = { key, status: "not_covering" };
			return;
		}
		if (!canvas) return;
		let current = true;
		valueOverlayState = { key, status: "loading" };
		overlayLayers.abortOthers(key);
		overlayLayers.load(request)
			.then(decodeCanvasImage)
			.then((layer) => {
				try {
					if (!current) return;
					drawOverlayLayer(canvas, layer);
					valueOverlayState = { key, status: "shown" };
				} finally {
					layer.dispose();
				}
			})
			.catch((error: unknown) => {
				if (!current || (error as Error).name === "AbortError") return;
				valueOverlayState = {
					key,
					status: isApiError(error, "overlay_not_covering_frame") ? "not_covering" : "error",
				};
			});
		return () => {
			current = false;
		};
	});

	// While the cursor is on the image, the shown colorwash's values for the
	// displayed frame load once, for the readout.
	$effect(() => {
		const volume = valueOverlayVolume;
		const covers = valueOverlayCovers;
		const fileIndex = activeFile.index;
		const frameIndex = currentFrame;
		const shown = untrack(() => shownValueOverlay);
		if (!probing || cinePlaying || !volume || !covers || !shown) return;
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
			stopProbe();
			overlayLayers.clear();
			presentationLayers.clear();
			segmentationLayers.clear();
			overlayValues.clear();
			rawFrames.clear();
			displayFrames.clear();
			livePreview.stop();
			wlRenderer.dispose();
		};
	});

	/** Refits the image and drops any in-progress window/level or drag. */
	export function resetView(): void {
		invalidateWindowLevelRenders();
		fitActiveImageToViewport();
		liveWindowCenter = null;
		liveWindowWidth = null;
		endDrag();
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

	function startZoomDrag(event: PointerEvent): DragState {
		const anchor = zoomAnchorFromClient(event.clientX, event.clientY);
		if (!anchor) return null;
		return {
			mode: "zoom_drag",
			startY: event.clientY,
			baseScale: activeTransform.scale,
			anchor,
		};
	}

	function applyZoomDrag(drag: Extract<NonNullable<DragState>, { mode: "zoom_drag" }>, clientY: number) {
		if (!activeFile) return;
		const dy = clientY - drag.startY;
		const transform = zoomTransformForAnchor(drag.baseScale * Math.exp(-dy * 0.005), drag.anchor);
		if (!transform) return;
		updateTransform(transform);
	}

	function wheelDeltaPixels(event: WheelEvent): { dx: number; dy: number } {
		if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) {
			return { dx: event.deltaX * 16, dy: event.deltaY * 16 };
		}
		if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) {
			const page = viewportSize.height || window.innerHeight || 800;
			return { dx: event.deltaX * page, dy: event.deltaY * page };
		}
		return { dx: event.deltaX, dy: event.deltaY };
	}

	function isLikelyTouchpadWheel(event: WheelEvent, dx: number, dy: number): boolean {
		if (event.deltaMode !== WheelEvent.DOM_DELTA_PIXEL) return false;
		return Math.abs(dx) > 0 || Math.abs(dy) < TRACKPAD_WHEEL_DELTA_THRESHOLD;
	}

	function zoomByWheelDelta(deltaY: number, clientX: number, clientY: number, sensitivity: number) {
		if (deltaY === 0) return;
		zoomAt(activeTransform.scale * Math.exp(-deltaY * sensitivity), clientX, clientY);
	}

	function onWheel(event: WheelEvent) {
		if (!activeFile || !activeFile.has_pixels) return;
		if (isViewportChromeTarget(event.target)) return;
		event.preventDefault();

		const { dx, dy } = wheelDeltaPixels(event);
		if (activeTool === "scroll" && navigationFrameCount > 1 && dy !== 0) {
			cinePlaying = false;
			onnavigationchange(navigationPosition + (dy > 0 ? 1 : -1));
			return;
		}
		if (event.ctrlKey || event.metaKey) {
			zoomByWheelDelta(dy, event.clientX, event.clientY, PINCH_ZOOM_SENSITIVITY);
			return;
		}

		if (isLikelyTouchpadWheel(event, dx, dy)) {
			updateTransform({
				...activeTransform,
				tx: activeTransform.tx - dx,
				ty: activeTransform.ty - dy,
			});
			return;
		}

		zoomByWheelDelta(dy, event.clientX, event.clientY, MOUSE_WHEEL_ZOOM_SENSITIVITY);
		scheduleProbe(event.clientX, event.clientY);
	}

	function onPointerDown(event: PointerEvent) {
		if (!activeFile || !activeFile.has_pixels) return;
		if (isViewportChromeTarget(event.target)) return;

		if (event.button === 1) {
			event.preventDefault();
			(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
			dragState = {
				mode: "pan",
				startX: event.clientX,
				startY: event.clientY,
				baseTx: activeTransform.tx,
				baseTy: activeTransform.ty,
			};
			return;
		}

		if (event.button === 2) {
			event.preventDefault();
			return;
		}

		if (event.button === 0) {
			if (overlay && (activeTool === "window_level" || activeTool === "annotate_rect")) {
				return;
			}
			let nextDragState: DragState = null;
			switch (activeTool) {
				case "window_level": {
					if (resolvedWindow.source === "color" || (pipelineMode === "diagnostic_wl" ? !currentRawFrame : !shownDisplay)) break;
					const baseWindow = displayWindow ?? { wc: 0, ww: 1 };
					nextDragState = {
						mode: "wl",
						startX: event.clientX,
						startY: event.clientY,
						baseCenter: baseWindow.wc,
						baseWidth: baseWindow.ww,
						// A LUT window drags in mapped units, scaled to move like a stored one.
						step: resolvedWindow.unit && frameMapping?.real_world[0]
							? mappedScale ? Math.abs(mappedScale.ratio) : mappedUnitsPerStoredUnit(frameMapping.real_world[0])
							: 1,
						unit: resolvedWindow.unit,
					};
					liveWindowCenter = baseWindow.wc;
					liveWindowWidth = baseWindow.ww;
					break;
				}
				case "pan":
					nextDragState = {
						mode: "pan",
						startX: event.clientX,
						startY: event.clientY,
						baseTx: activeTransform.tx,
						baseTy: activeTransform.ty,
					};
					break;
				case "zoom":
					nextDragState = startZoomDrag(event);
					break;
				case "scroll":
					if (navigationFrameCount > 1) {
						nextDragState = {
							mode: "scroll_drag",
							startY: event.clientY,
							baseFrame: navigationPosition,
						};
					}
					break;
				case "annotate_rect": {
					if (!annotationsReady) break;
					const point = pointFromPointer(event);
					if (!point) break;
					event.preventDefault();
					const hit = hitTestRoi(visibleRois, point, activeTransform.scale);
					if (hit) {
						setSelectedRoi(hit.roi.index);
						annotations.beginLiveEdit(activeFile.index);
						const original = roiCoord(hit.roi);
						nextDragState = hit.handle
							? { mode: "resize_roi", roiIndex: hit.roi.index, handle: hit.handle, original }
							: { mode: "move_roi", roiIndex: hit.roi.index, start: point, original };
						break;
					}
					setSelectedRoi(null);
					nextDragState = { mode: "draw_roi", start: point, current: point };
					break;
				}
			}
			if (nextDragState) {
				event.preventDefault();
				(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
				dragState = nextDragState;
			}
		}
	}

	function onPointerMove(event: PointerEvent) {
		if (activeFile?.has_pixels && !isViewportChromeTarget(event.target)) {
			scheduleProbe(event.clientX, event.clientY);
		} else if (!dragState) {
			stopProbe();
		}
		if (!activeFile || !dragState) return;

		if (dragState.mode === "pan") {
			const dx = event.clientX - dragState.startX;
			const dy = event.clientY - dragState.startY;
			updateTransform({
				...activeTransform,
				tx: dragState.baseTx + dx,
				ty: dragState.baseTy + dy,
			});
			return;
		}

		if (dragState.mode === "wl") {
			const dx = event.clientX - dragState.startX;
			const dy = event.clientY - dragState.startY;
			const nextWidth = Math.max(dragState.step, dragState.baseWidth + dx * 4 * dragState.step);
			const nextCenter = dragState.baseCenter - dy * 2 * dragState.step;
			liveWindowCenter = nextCenter;
			liveWindowWidth = nextWidth;
			return;
		}

		if (dragState.mode === "zoom_drag") {
			applyZoomDrag(dragState, event.clientY);
			return;
		}

		if (dragState.mode === "scroll_drag" && navigationFrameCount > 1) {
			const dy = event.clientY - dragState.startY;
			const frameDelta = Math.round(dy / DRAG_PIXELS_PER_FRAME);
			cinePlaying = false;
			onnavigationchange(
				Math.max(0, Math.min(navigationFrameCount - 1, dragState.baseFrame + frameDelta)),
			);
			return;
		}

		if (dragState.mode === "draw_roi") {
			const point = pointFromPointer(event);
			if (point) {
				dragState = { ...dragState, current: point };
			}
			return;
		}

		if (dragState.mode === "move_roi" && activeAnnotations) {
			const point = pointFromPointer(event);
			if (!point) return;
			const moved = moveCoord(
				dragState.original,
				{ x: point.x - dragState.start.x, y: point.y - dragState.start.y },
				imageRows,
				imageColumns,
			);
			const next = updateRoiCoord(activeAnnotations, dragState.roiIndex, moved, activeFile.frame_count);
			annotations.showDraft(activeFile.index, next);
			return;
		}

		if (dragState.mode === "resize_roi" && activeAnnotations) {
			const point = pointFromPointer(event);
			if (!point) return;
			const resized = resizeCoord(dragState.original, dragState.handle, point, imageRows, imageColumns);
			if (!resized) return;
			const next = updateRoiCoord(activeAnnotations, dragState.roiIndex, resized, activeFile.frame_count);
			annotations.showDraft(activeFile.index, next);
		}
	}

	function onPointerUp(event: PointerEvent) {
		const target = event.currentTarget as HTMLElement;
		if (target.hasPointerCapture(event.pointerId)) {
			target.releasePointerCapture(event.pointerId);
		}
		if (dragState?.mode === "wl" && liveWindowCenter !== null && liveWindowWidth !== null) {
			onmanualwindowlevel(liveWindowCenter, liveWindowWidth, dragState.unit);
		}
		if (dragState?.mode === "draw_roi") {
			const coord = canonicalRect(dragState.start, dragState.current, imageRows, imageColumns);
			if (coord) {
				const next = addRoi(activeAnnotations, coord, currentFrame, activeFile.frame_count);
				annotations.commit(activeFile.index, next, next.num_roi - 1);
			}
		}
		if ((dragState?.mode === "move_roi" || dragState?.mode === "resize_roi") && activeAnnotations) {
			annotations.commit(activeFile.index, activeAnnotations, selectedRoiIndex);
		}
		endDrag();
	}

	function onPointerCancel() {
		if (dragState?.mode === "move_roi" || dragState?.mode === "resize_roi") {
			const editable = normalizeAnnotationsForEdit(activeAnnotations, activeFile.frame_count);
			const next = updateRoiCoord(editable, dragState.roiIndex, dragState.original, activeFile.frame_count);
			annotations.showDraft(activeFile.index, next);
		}
		endDrag();
	}

	function endDrag() {
		liveWindowCenter = null;
		liveWindowWidth = null;
		livePreview.stop();
		dragState = null;
		annotations.endLiveEdit();
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
	onwheel={onWheel}
	onpointerdown={onPointerDown}
	onpointermove={onPointerMove}
	onpointerup={onPointerUp}
	onpointercancel={onPointerCancel}
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
			{#if showsPresentationLayer}
				<canvas bind:this={presentationLayerCanvas} class="layer-canvas" aria-hidden="true"></canvas>
			{/if}
			{#if valueOverlayVolume}
				<canvas
					bind:this={valueOverlayCanvas}
					class="value-overlay-canvas"
					hidden={valueOverlayState?.status !== "shown"}
					style:opacity={shownValueOverlay?.opacity ?? 0}
					aria-hidden="true"
				></canvas>
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
				{#if overlay?.kind === "segmentation"}
					<span>SEG overlay {overlay.segmentationFrameIndex + 1} / {activeFile.frame_count}</span>
					<span>source frame {overlay.sourceFrameIndex + 1}</span>
				{:else}
					<span>image {navigationPosition + 1} / {navigationFrameCount}</span>
					<span>source frame {currentFrame + 1} / {activeFile.frame_count}</span>
				{/if}
				{#if windowLegend}
					<span class="mapped-window">
						W: {formatValue(windowLegend.width)} · C: {formatValue(windowLegend.center)} {windowLegend.unit}
					</span>
				{:else if resolvedWindow.source === "voi_lut"}
					<span>VOI LUT</span>
				{:else if displayWindow}
					<span>W: {Math.round(displayWindow.ww)} · C: {Math.round(displayWindow.wc)}</span>
				{/if}
				{#if activeTool === "window_level" && !activeFile.raw_windowing_compatible}
					<span class="presentation-path" title={activeFile.raw_windowing_reason ?? undefined}>server presentation retained</span>
				{/if}
			</div>
		</div>
		{#if !overlay}
			<RoiList
				rois={visibleRois}
				totalCount={activeAnnotations?.num_roi ?? null}
				frameCount={activeFile.frame_count}
				selectedIndex={selectedRoiIndex}
				loading={annotations.loading(activeFile.index)}
				error={annotations.error(activeFile.index)}
				ready={annotationsReady}
				saveStatus={annotations.saveStatus(activeFile.index)}
				onselect={setSelectedRoi}
				onscope={setSelectedScope}
				ondelete={deleteSelectedRoi}
				onretryload={() => annotations.retryLoad(activeFile.index)}
				onretrysave={() => annotations.retrySave(activeFile.index)}
				onrevert={() => annotations.rollback(activeFile.index)}
			/>
		{/if}
		{#if windowLegend || shownValueOverlay}
			<div class="legends">
				{#if shownValueOverlay}
					<ValueLegend
						title={shownValueOverlay.title}
						unit={shownValueOverlay.legend.unit_label}
						low={shownValueOverlay.legend.min_value}
						high={shownValueOverlay.legend.max_value}
						colors={legendColors(shownValueOverlay.legend)}
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
	.viewport[data-tool="annotate_rect"] { cursor: crosshair; }
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

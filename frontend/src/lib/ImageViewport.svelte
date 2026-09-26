<script lang="ts">
	import { untrack } from "svelte";
	import {
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
	import { fitImageToViewportHeight, imageDisplayGeometry } from "./imageGeometry";
	import type { ViewStates } from "./viewport/viewStates.svelte";
	import {
		clientToImagePoint,
		layerTransformCss,
		MAX_ZOOM,
		MIN_ZOOM,
		nextZoomStep,
		zoomAnchor,
		zoomAroundAnchor,
		type LayerOrigin,
		type ViewTransform,
		type ZoomAnchor,
	} from "./viewport/viewTransform";
	import {
		canRunCinePlayback,
		runRenderPacedCine,
		waitForCineDeadline,
		type CineDirection,
		type CineMode,
	} from "./cinePlayback";
	import { trackForegroundRequest } from "./requestIndicator";
	import {
		resolveDisplayWindow,
		selectWindowingPipeline,
		validateRenderableRawFrame,
	} from "./rawWindowing";
	import type { ActiveTool } from "./viewerTools";
	import { navigationFrameAtPosition, type NavigationFrameRef } from "./seriesNavigation";
	import { AnnotationStore } from "./viewport/annotationStore.svelte";
	import { DisplayFrameSource } from "./viewport/displayFrameSource";
	import {
		composeOverlayFrame,
		decodeCanvasImage,
		overlayLayerRequests,
		type FrameOverlay,
	} from "./viewport/frameOverlay";
	import {
		observePrefetchConcurrency,
		PREFETCH_CONCURRENCY,
		scheduleIdle,
	} from "./viewport/prefetchScheduling";
	import { RawFrameSource } from "./viewport/rawFrameSource";
	import { RenderedFrames } from "./viewport/renderedFrames.svelte";
	import RoiList from "./viewport/RoiList.svelte";
	import RoiOverlay from "./viewport/RoiOverlay.svelte";
	import { hitTestRoi, roiCoord, visibleRois as roisOnFrame } from "./viewport/roiEditing";
	import { WlRendererClient } from "./viewport/wlRendererClient";

	type PipelineMode = "cine" | "diagnostic_wl" | "server_wl" | "overlay";
	type DragState =
		| { mode: "pan"; startX: number; startY: number; baseTx: number; baseTy: number }
		| { mode: "wl"; startX: number; startY: number; baseCenter: number; baseWidth: number }
		| { mode: "zoom_drag"; startY: number; baseScale: number; anchor: ZoomAnchor }
		| { mode: "scroll_drag"; startY: number; baseFrame: number }
		| { mode: "draw_roi"; start: ImagePoint; current: ImagePoint }
		| { mode: "move_roi"; roiIndex: number; start: ImagePoint; original: RoiCoord }
		| { mode: "resize_roi"; roiIndex: number; handle: RoiHandle; original: RoiCoord }
		| null;

	let {
		activeFile,
		currentFrame = $bindable(),
		windowCenter = $bindable(),
		windowWidth = $bindable(),
		activeTool,
		windowMode,
		selectedPresetId,
		resetCount,
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
	}: {
		activeFile: FileSummary;
		currentFrame: number;
		windowCenter: number | null;
		windowWidth: number | null;
		activeTool: ActiveTool;
		windowMode: WindowMode;
		selectedPresetId: string;
		resetCount: number;
		viewStates: ViewStates;
		onreset?: () => void;
		onmanualwindowlevel?: (center: number, width: number) => void;
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
	} = $props();

	let dragState = $state<DragState>(null);
	let loading = $state(false);
	let loadError = $state<string | null>(null);
	let liveWindowCenter = $state<number | null>(null);
	let liveWindowWidth = $state<number | null>(null);
	let viewportEl: HTMLElement | undefined = $state();
	let viewportSize = $state({ width: 0, height: 0 });
	let canvasEl: HTMLCanvasElement | undefined = $state();
	let currentRawFrame = $state<RawFrame | null>(null);
	let rawWindowLevelFallbackByFile = $state<Record<number, boolean>>({});
	const annotations = new AnnotationStore();

	let prefetchConcurrency = $state(PREFETCH_CONCURRENCY);
	const rendered = new RenderedFrames();
	const rawFrames = new RawFrameSource({ concurrency: () => prefetchConcurrency });
	const displayFrames = new DisplayFrameSource({
		navigationScope: () => navigationScopeKey,
		concurrency: () => prefetchConcurrency,
		onScopeChange: () => rendered.reset(),
	});
	let retainedScopeKey = "";
	let lastHandledResetCount = 0;
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
	const zoomPercent = $derived(Math.round(activeTransform.scale * 100));
	const isDragging = $derived(dragState !== null);
	const pipelineMode = $derived.by<PipelineMode>(() => overlay
		? "overlay"
		: selectWindowingPipeline(
			activeTool === "window_level",
			rawWindowLevelFallbackByFile[activeFile.index] ?? false,
			activeFile.raw_windowing_compatible,
		));

	const displayWindow = $derived(
		pipelineMode === "overlay"
			? overlay?.sourceFile.default_window
				? {
					wc: overlay.sourceFile.default_window.center,
					ww: overlay.sourceFile.default_window.width,
				}
				: { wc: 0, ww: 1 }
			: pipelineMode === "diagnostic_wl" && currentRawFrame
			? resolveDisplayWindow(
				currentRawFrame,
				liveWindowCenter,
				liveWindowWidth,
				windowCenter,
				windowWidth,
				windowMode,
			)
			: liveWindowCenter !== null && liveWindowWidth !== null
				? { wc: liveWindowCenter, ww: liveWindowWidth }
				: windowCenter !== null && windowWidth !== null
				? { wc: windowCenter, ww: windowWidth }
				: activeFile?.default_window
					? { wc: activeFile.default_window.center, ww: activeFile.default_window.width }
					: { wc: 0, ww: 1 },
	);
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

	function pointFromPointer(event: PointerEvent): ImagePoint | null {
		const point = imagePointAt(event.clientX, event.clientY);
		if (!point) return null;
		return {
			x: Math.min(imageColumns, Math.max(0, point.x)),
			y: Math.min(imageRows, Math.max(0, point.y)),
		};
	}

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

	function currentDisplayWindowOptions(): DisplayFrameWindowOptions {
		if (pipelineMode === "overlay") return {};
		if (windowCenter !== null && windowWidth !== null) {
			return { wc: windowCenter, ww: windowWidth, windowMode: "default" };
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
			const bitmap = await displayFrames.decode(key, blob);
			if (generation !== requestGeneration || !canvasEl || !usesDisplayPipeline()) return;
			canvasEl.width = bitmap.width;
			canvasEl.height = bitmap.height;
			ctx.drawImage(bitmap, 0, 0);
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

	function prefetchRawRing(direction: 1 | -1): void {
		const prefetchScope = retainedScopeKey;
		const frames = navigationFrames;
		const position = navigationPosition;
		scheduleIdle(() => {
			if (retainedScopeKey !== prefetchScope || pipelineMode !== "diagnostic_wl") return;
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
				rawWindowLevelFallbackByFile = {
					...rawWindowLevelFallbackByFile,
					[fileIndex]: true,
				};
				return;
			}
			rawFrames.store(fileIndex, frameIndex, rawFrame);
			currentRawFrame = rawFrame;
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
			rawWindowLevelFallbackByFile = {
				...rawWindowLevelFallbackByFile,
				[fileIndex]: true,
			};
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
			const blobRequest = displayFrames.ensureBlob(fileIndex, frameIndex, windowOptions);
			trackForegroundRequest(
				displayFrames.inFlight(cacheKey),
				() => generation === requestGeneration,
				(pending) => { loading = pending; },
			);
			const blob = await blobRequest;
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			loading = false;
			loadError = null;
			await drawDisplayBlob(cacheKey, blob, generation);
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
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
			const blobs = await Promise.all([
				displayFrames.ensureBlob(overlay.sourceFileIndex, overlay.sourceFrameIndex, windowOptions),
				...overlayLayerRequests(overlay).map(({ key, load }) => displayFrames.fetchInScope(key, windowOptions, load)),
			]);
			const [base, ...layers] = await Promise.all(blobs.map(decodeCanvasImage));
			try {
				if (generation !== requestGeneration || pipelineMode !== "overlay" || !canvasEl) return;
				composeOverlayFrame(canvasEl, base, layers);
				loading = false;
				loadError = null;
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

	$effect(() => {
		const current = currentFrame;
		if (cinePlaying) {
			frameDirection = cineDirection;
		} else {
			if (current > lastFrameForDirection) frameDirection = 1;
			if (current < lastFrameForDirection) frameDirection = -1;
		}
		lastFrameForDirection = current;
	});

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
		rawFrames.clear();
		displayFrames.clear();
	});

	$effect(() => {
		if (!activeFile) return;
		void activeFile.index;
		invalidateWindowLevelRenders();
		currentRawFrame = null;
		liveWindowCenter = null;
		liveWindowWidth = null;
		untrack(() => setSelectedRoi(null));
		clearCanvas();
	});

	$effect(() => {
		const handleKey = (event: KeyboardEvent) => {
			const target = event.target as HTMLElement | null;
			if (target && ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName)) return;
			if (activeTool !== "annotate_rect") return;
			if (event.key === "Delete" || event.key === "Backspace") {
				event.preventDefault();
				deleteSelectedRoi();
			}
		};
		window.addEventListener("keydown", handleKey);
		return () => window.removeEventListener("keydown", handleKey);
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
		const totalFrames = frames.length;
		let scheduledPosition = untrack(() => navigationPosition);
		let direction = untrack(() => cineDirection);
		displayFrames.enterScope(windowOptions);

		void (async () => {
			const initialFrame = navigationFrameAtPosition(frames, scheduledPosition);
			if (!initialFrame) return;
			if (!await rendered.waitFor(
				initialFrame.file_index,
				initialFrame.frame_index,
				ctrl.signal,
			)) return;
			await runRenderPacedCine({
				initialFrame: scheduledPosition,
				totalFrames,
				mode: playbackMode,
				direction,
				fps,
				signal: ctrl.signal,
				now: () => performance.now(),
				waitForDelay: waitForCineDeadline,
				prepareFrame: (position) => {
					const frame = navigationFrameAtPosition(frames, position);
					if (!frame) return Promise.reject(new Error("logical cine frame is unavailable"));
					return displayFrames.ensureBlob(frame.file_index, frame.frame_index, windowOptions)
						.then((blob) => typeof createImageBitmap === "function"
							? displayFrames.decode(displayFrames.key(frame.file_index, frame.frame_index, windowOptions), blob)
							: undefined);
				},
				presentFrame: async (step, signal) => {
					const frame = navigationFrameAtPosition(frames, step.frame);
					if (!frame) return false;
					direction = step.direction;
					scheduledPosition = step.frame;
					cineDirection = direction;
					onnavigationchange(scheduledPosition);
					return rendered.waitFor(frame.file_index, frame.frame_index, signal);
				},
			});
		})().catch((error) => {
			if (ctrl.signal.aborted || (error as Error).name === "AbortError") return;
			loadError = (error as Error).message || "Failed to prepare cine frame";
			cinePlaying = false;
		});

		return () => ctrl.abort();
	});

	$effect(() => {
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
		const frameIndex = currentFrame;
		const generation = ++requestGeneration;
		const modeWc = mode !== "diagnostic_wl" ? windowCenter : null;
		const modeWw = mode !== "diagnostic_wl" ? windowWidth : null;
		const modePreset = mode !== "diagnostic_wl" ? selectedPresetId : "";
		const modeWindowMode = mode !== "diagnostic_wl" ? windowMode : "default";
		void modeWc;
		void modeWw;
		void modePreset;
		void modeWindowMode;

		loadError = null;
		if (mode === "overlay" && activeOverlay) {
			void loadOverlayAndRender(activeOverlay, generation);
		} else if (mode === "diagnostic_wl") {
			void loadRawFrameAndRender(fileIndex, frameIndex, generation, frameDirection);
		} else {
			void loadDisplayFrameAndRender(fileIndex, frameIndex, generation, frameDirection);
		}
	});

	$effect(() => {
		if (pipelineMode !== "diagnostic_wl" || !currentRawFrame || !canvasEl) return;
		// displayWindow already resolved this frame's window; window changes do
		// not invalidate in-flight renders, only frame, file, and mode changes do.
		const frame = currentRawFrame;
		const { wc, ww } = displayWindow;
		const generation = wlRenderGeneration;
		void wlRenderer.render(() => canvasEl, {
			frame,
			wc,
			ww,
			isCurrent: () => generation === wlRenderGeneration
				&& frame === currentRawFrame
				&& pipelineMode === "diagnostic_wl",
		});
	});

	$effect(() => observePrefetchConcurrency((concurrency) => { prefetchConcurrency = concurrency; }));

	$effect(() => {
		return () => {
			rawFrames.clear();
			displayFrames.clear();
			wlRenderer.dispose();
		};
	});

	$effect(() => {
		if (resetCount === lastHandledResetCount) return;
		lastHandledResetCount = resetCount;
		if (resetCount === 0) return;
		invalidateWindowLevelRenders();
		fitActiveImageToViewport();
		liveWindowCenter = null;
		liveWindowWidth = null;
		endDrag();
	});

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
					if (pipelineMode === "diagnostic_wl" && !currentRawFrame) break;
					const baseWindow = pipelineMode === "diagnostic_wl" && currentRawFrame
						? resolveDisplayWindow(
							currentRawFrame,
							liveWindowCenter,
							liveWindowWidth,
							windowCenter,
							windowWidth,
							windowMode,
						)
						: displayWindow;
					nextDragState = {
						mode: "wl",
						startX: event.clientX,
						startY: event.clientY,
						baseCenter: baseWindow.wc,
						baseWidth: baseWindow.ww,
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
			const nextWidth = Math.max(1, dragState.baseWidth + dx * 4);
			const nextCenter = dragState.baseCenter - dy * 2;
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
			windowCenter = liveWindowCenter;
			windowWidth = liveWindowWidth;
			onmanualwindowlevel?.(liveWindowCenter, liveWindowWidth);
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
		dragState = null;
		annotations.endLiveEdit();
	}

	function onContextMenu(event: MouseEvent) {
		event.preventDefault();
	}

	function resetViewport() {
		if (!activeFile) return;
		fitActiveImageToViewport();
		windowCenter = activeFile.default_window?.center ?? null;
		windowWidth = activeFile.default_window?.width ?? null;
		liveWindowCenter = null;
		liveWindowWidth = null;
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
	class:dragging={isDragging}
	data-tool={activeTool}
	role="application"
	onwheel={onWheel}
	onpointerdown={onPointerDown}
	onpointermove={onPointerMove}
	onpointerup={onPointerUp}
	onpointercancel={onPointerCancel}
	oncontextmenu={onContextMenu}
	ondblclick={() => { if (onreset) { onreset(); } else { resetViewport(); } }}
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
			{#if !overlay && imageColumns > 0 && imageRows > 0}
				<RoiOverlay
					rois={visibleRois}
					selectedIndex={selectedRoiIndex}
					draft={draftRoi}
					rows={imageRows}
					columns={imageColumns}
				/>
			{/if}
		</div>
		<div class="overlay">
			{#if overlay?.kind === "segmentation"}
				<span>SEG overlay {overlay.segmentationFrameIndex + 1} / {activeFile.frame_count}</span>
				<span>source frame {overlay.sourceFrameIndex + 1}</span>
			{:else}
				<span>image {navigationPosition + 1} / {navigationFrameCount}</span>
				<span>source frame {currentFrame + 1} / {activeFile.frame_count}</span>
			{/if}
			<span>W: {Math.round(displayWindow.ww)} · C: {Math.round(displayWindow.wc)}</span>
			{#if activeTool === "window_level" && !activeFile.raw_windowing_compatible}
				<span class="presentation-path" title={activeFile.raw_windowing_reason ?? undefined}>server presentation retained</span>
			{/if}
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
		<div class="zoom-controls">
			<button type="button" onclick={() => stepZoom(-1)} disabled={activeTransform.scale <= MIN_ZOOM}>−</button>
			<button type="button" class="zoom-level" onclick={fitActiveImageToViewport} title="Fit to height">{zoomPercent}%</button>
			<button type="button" onclick={() => stepZoom(1)} disabled={activeTransform.scale >= MAX_ZOOM}>+</button>
		</div>
	{/if}
</section>

<style>
	.viewport {
		position: relative;
		display: grid;
		place-items: center;
		background:
			radial-gradient(circle at center, rgba(255, 255, 255, 0.025), transparent 58%),
			var(--surface-viewport);
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
	.dicom-canvas {
		display: block;
		width: 100%;
		height: 100%;
		image-rendering: pixelated;
	}
	.placeholder {
		color: var(--text-muted);
	}
	.frame-request-indicator {
		position: absolute;
		top: 0.75rem;
		left: 0.75rem;
		z-index: 2;
		display: grid;
		place-items: center;
		width: 1.8rem;
		height: 1.8rem;
		background: rgba(28, 28, 30, 0.72);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		backdrop-filter: blur(14px);
	}
	.loading-wheel {
		width: 0.9rem;
		height: 0.9rem;
		box-sizing: border-box;
		border: 2px solid rgba(142, 142, 147, 0.24);
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
	.overlay {
		position: absolute;
		left: 0.75rem;
		bottom: 0.75rem;
		display: flex;
		gap: 0.75rem;
		font-size: 0.78rem;
		padding: 0.34rem 0.55rem;
		background: rgba(28, 28, 30, 0.74);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
		color: var(--text-secondary);
	}
	.zoom-controls {
		position: absolute;
		right: 0.75rem;
		bottom: 0.75rem;
		display: flex;
		align-items: center;
		gap: 0;
		background: rgba(28, 28, 30, 0.78);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-panel);
		overflow: hidden;
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
	}
	.zoom-controls button {
		background: none;
		border: none;
		color: var(--text-secondary);
		padding: 0.3rem 0.55rem;
		font-size: 0.95rem;
		cursor: pointer;
		line-height: 1;
	}
	.zoom-controls button:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.08);
		color: var(--text-primary);
	}
	.zoom-controls button:focus-visible {
		outline: none;
		box-shadow: inset var(--focus-ring);
	}
	.zoom-controls button:disabled {
		color: rgba(255, 255, 255, 0.22);
		cursor: default;
	}
	.zoom-controls .zoom-level {
		padding: 0.3rem 0.4rem;
		font-size: 0.78rem;
		font-family: var(--font-mono);
		color: var(--text-secondary);
		min-width: 3.2rem;
		text-align: center;
		cursor: pointer;
		border-left: 1px solid var(--border-subtle);
		border-right: 1px solid var(--border-subtle);
	}
	.zoom-controls .zoom-level:hover {
		color: var(--text-primary);
	}
</style>

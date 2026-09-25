<script lang="ts">
	import { untrack } from "svelte";
	import {
		displayFrameCacheKey,
		displayFrameWindowCacheKey,
		fetchAnnotations,
		fetchDisplayFrameBlob,
		fetchRawFrame,
		fetchSegmentationOverlayBlob,
		updateAnnotations,
		type DisplayFrameWindowOptions,
		type EmbedRoiAnnotations,
		type FileSummary,
		type RawFrame,
		type WindowMode,
	} from "../api";
	import {
		addRoi,
		canonicalRect,
		deleteRoi,
		isAllFrames,
		moveCoord,
		normalizeAnnotationsForEdit,
		resizeCoord,
		setRoiFrameScope,
		updateRoiCoord,
		type ImagePoint,
		type RoiCoord,
		type RoiHandle,
	} from "./annotationGeometry";
	import {
		ByteBudgetLruCache,
		createDisplayFrameCaches,
	} from "./frameCache";
	import { fitImageToViewportHeight, imageDisplayGeometry } from "./imageGeometry";
	import {
		canRunCinePlayback,
		runRenderPacedCine,
		waitForAbortableResult,
		waitForCineDeadline,
		type CineDirection,
		type CineMode,
	} from "./cinePlayback";
	import {
		buildDirectionalFrameOrder,
		planDisplayPrefetchTargets,
	} from "./prefetchPolicy";
	import {
		RevisionedPersistenceController,
		type PersistenceSnapshot,
	} from "./revisionedPersistence";
	import { trackForegroundRequest } from "./requestIndicator";
	import { SharedRequestRegistry } from "./sharedRequestRegistry";
	import {
		renderRawFrameToRgba,
		resolveDisplayWindow,
		selectWindowingPipeline,
		validateRenderableRawFrame,
	} from "./rawWindowing";
	import { DEFAULT_ORIENTATION, type ActiveTool, type ImageOrientation } from "./viewerTools";
	import { navigationFrameAtPosition, type NavigationFrameRef } from "./seriesNavigation";
	import type {
		WlRendererRequest,
		WlRendererResponse,
		WlRendererSuccess,
	} from "./workers/wlRendererProtocol";

	type PipelineMode = "cine" | "diagnostic_wl" | "server_wl" | "segmentation_overlay";
	type SegmentationOverlay = {
		segmentationFileIndex: number;
		segmentationFrameIndex: number;
		sourceFileIndex: number;
		sourceFrameIndex: number;
		sourceFile: FileSummary;
	};
	type TransformState = { scale: number; tx: number; ty: number; fit: boolean };
	type ZoomAnchor = {
		clientX: number;
		clientY: number;
		localX: number;
		localY: number;
	};
	type DragState =
		| { mode: "pan"; startX: number; startY: number; baseTx: number; baseTy: number }
		| { mode: "wl"; startX: number; startY: number; baseCenter: number; baseWidth: number }
		| { mode: "zoom_drag"; startY: number; baseScale: number; anchor: ZoomAnchor }
		| { mode: "scroll_drag"; startY: number; baseFrame: number }
		| { mode: "draw_roi"; start: ImagePoint; current: ImagePoint }
		| { mode: "move_roi"; roiIndex: number; start: ImagePoint; original: RoiCoord }
		| { mode: "resize_roi"; roiIndex: number; handle: RoiHandle; original: RoiCoord }
		| null;

	type VisibleRoi = {
		index: number;
		ymin: number;
		xmin: number;
		ymax: number;
		xmax: number;
		frames: number[] | null;
	};
	type WlRenderedFrame = Pick<WlRendererSuccess, "width" | "height" | "bitmap">;

	const RAW_CACHE_BYTE_BUDGET = 256 * 1024 * 1024;
	const DISPLAY_BLOB_CACHE_BYTE_BUDGET = 320 * 1024 * 1024;
	const DISPLAY_BITMAP_CACHE_BYTE_BUDGET = 128 * 1024 * 1024;

	let {
		activeFile,
		currentFrame = $bindable(),
		windowCenter = $bindable(),
		windowWidth = $bindable(),
		activeTool,
		windowMode,
		selectedPresetId,
		resetCount,
		orientation = DEFAULT_ORIENTATION,
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
		segmentationOverlay = null,
	}: {
		activeFile: FileSummary;
		currentFrame: number;
		windowCenter: number | null;
		windowWidth: number | null;
		activeTool: ActiveTool;
		windowMode: WindowMode;
		selectedPresetId: string;
		resetCount: number;
		orientation?: ImageOrientation;
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
		segmentationOverlay?: SegmentationOverlay | null;
	} = $props();

	let transformsByFile = $state<Record<number, TransformState>>({});
	let dragState = $state<DragState>(null);
	let loading = $state(false);
	let loadError = $state<string | null>(null);
	let liveWindowCenter = $state<number | null>(null);
	let liveWindowWidth = $state<number | null>(null);
	let viewportEl: HTMLElement | undefined = $state();
	let viewportSize = $state({ width: 0, height: 0 });
	let canvasEl: HTMLCanvasElement | undefined = $state();
	let renderedFrameToken = $state("");
	let roiSvgEl: SVGSVGElement | undefined = $state();
	let currentRawFrame = $state<RawFrame | null>(null);
	let rawWindowLevelFallbackByFile = $state<Record<number, boolean>>({});
	let annotationsByFile = $state<Record<number, EmbedRoiAnnotations | undefined>>({});
	let annotationErrorsByFile = $state<Record<number, string | null | undefined>>({});
	let annotationLoadingByFile = $state<Record<number, boolean | undefined>>({});
	let annotationPersistenceByFile = $state<
		Record<number, PersistenceSnapshot<EmbedRoiAnnotations> | undefined>
	>({});
	let selectedRoiByFile = $state<Record<number, number | null | undefined>>({});
	let annotationRequestedByFile: Record<number, boolean> = {};
	const annotationPersistence = new RevisionedPersistenceController<number, EmbedRoiAnnotations>({
		save: updateAnnotations,
		errorMessage: (error) =>
			error instanceof Error && error.message ? error.message : "Failed to save annotations",
		onChange: (fileIndex, snapshot) => {
			const editingRoi = activeFile?.index === fileIndex
				&& (dragState?.mode === "move_roi" || dragState?.mode === "resize_roi");
			if (!editingRoi) {
				annotationsByFile = {
					...annotationsByFile,
					[fileIndex]: snapshot.value,
				};
			}
			annotationErrorsByFile = {
				...annotationErrorsByFile,
				[fileIndex]: snapshot.error,
			};
			annotationPersistenceByFile = {
				...annotationPersistenceByFile,
				[fileIndex]: snapshot,
			};
		},
	});

	let rawPrefetchCtrl: AbortController | null = null;
	let displayPrefetchCtrl: AbortController | null = null;
	let displayScopeCtrl: AbortController | null = null;
	let displayFetchScopeKey = "";
	let displayPrefetchSeedFrame: number | null = null;
	let displayPrefetchScopeKey = "";
	const rawFrameCache = new ByteBudgetLruCache<string, RawFrame>({
		maxBytes: RAW_CACHE_BYTE_BUDGET,
		sizeOf: (frame) => frame.buffer.byteLength,
	});
	const rawRequests = new SharedRequestRegistry<string, RawFrame>();
	const displayFrameCaches = createDisplayFrameCaches(
		DISPLAY_BLOB_CACHE_BYTE_BUDGET,
		DISPLAY_BITMAP_CACHE_BYTE_BUDGET,
	);
	const displayDecodePromises = new Map<
		string,
		{ blob: Blob; promise: Promise<ImageBitmap> }
	>();
	const displayNetworkPromises = new Map<string, Promise<Blob>>();
	const displayFetchPromises = new Map<string, Promise<Blob>>();
	let lastRenderedDisplayFrame: { fileIndex: number; frameIndex: number } | null = null;
	const displayRenderWaiters = new Set<{
		fileIndex: number;
		frameIndex: number;
		resolve: (rendered: boolean) => void;
	}>();
	let retainedScopeKey = "";
	let lastHandledResetCount = 0;
	let requestGeneration = 0;
	let lastFrameForDirection = 0;
	let frameDirection: 1 | -1 = 1;
	let wlRenderGeneration = 0;

	let wlWorker: Worker | null = null;
	let workerInitAttempted = false;
	let workerAvailable = false;
	let workerMessageId = 0;
	let pendingWorkerResponses = new Map<number, {
		resolve: (value: WlRenderedFrame) => void;
		reject: (error: Error) => void;
	}>();

	const MIN_ZOOM = 0.05;
	const MAX_ZOOM = 64;
	const ZOOM_STEPS = [0.05, 0.1, 0.2, 0.25, 0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64];
	const DEFAULT_TRANSFORM: TransformState = { scale: 1, tx: 0, ty: 0, fit: false };
	const RAW_RING_RADIUS = 10;
	const DISPLAY_FULL_PREFETCH_BUDGET_BYTES = 320 * 1024 * 1024;
	const DISPLAY_NEAR_PREFETCH_DISTANCE = 48;
	const WORKER_MIN_PIXEL_THRESHOLD = 300_000;
	const PREFETCH_CONCURRENCY = 3;
	const CINE_LOOKAHEAD_FRAMES = 16;
	const PREFETCH_RESEED_DISTANCE = 6;
	let prefetchConcurrency = $state(PREFETCH_CONCURRENCY);
	const FRAME_SCROLL_SPEED_FACTOR = 0.7;
	const DRAG_PIXELS_PER_FRAME = 10 / FRAME_SCROLL_SPEED_FACTOR;
	const TRACKPAD_WHEEL_DELTA_THRESHOLD = 50;
	const MOUSE_WHEEL_ZOOM_SENSITIVITY = 0.0025;
	const PINCH_ZOOM_SENSITIVITY = 0.01;
	const activeTransform = $derived(activeFile ? transformsByFile[activeFile.index] ?? DEFAULT_TRANSFORM : DEFAULT_TRANSFORM);
	const transformCss = $derived.by(() => {
		const { tx, ty, scale } = activeTransform;
		let css = `translate(${tx}px, ${ty}px) scale(${scale})`;
		const { flipH, flipV, rotation } = orientation;
		if (rotation !== 0 || flipH || flipV) {
			const cx = displayGeometry.centerX;
			const cy = displayGeometry.centerY;
			const sx = flipH ? -1 : 1;
			const sy = flipV ? -1 : 1;
			css += ` translate(${cx}px,${cy}px) rotate(${rotation}deg) scale(${sx},${sy}) translate(${-cx}px,${-cy}px)`;
		}
		return css;
	});
	const zoomPercent = $derived(Math.round(activeTransform.scale * 100));
	const isDragging = $derived(dragState !== null);
	const pipelineMode = $derived.by<PipelineMode>(() => segmentationOverlay
		? "segmentation_overlay"
		: selectWindowingPipeline(
			activeTool === "window_level",
			rawWindowLevelFallbackByFile[activeFile.index] ?? false,
			activeFile.raw_windowing_compatible,
		));

	const displayWindow = $derived(
		pipelineMode === "segmentation_overlay"
			? segmentationOverlay?.sourceFile.default_window
				? {
					wc: segmentationOverlay.sourceFile.default_window.center,
					ww: segmentationOverlay.sourceFile.default_window.width,
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
	const activeAnnotations = $derived(activeFile ? annotationsByFile[activeFile.index] ?? null : null);
	const activeAnnotationError = $derived(activeFile ? annotationErrorsByFile[activeFile.index] ?? null : null);
	const activeAnnotationLoading = $derived(activeFile ? annotationLoadingByFile[activeFile.index] ?? false : false);
	const activeAnnotationPersistence = $derived(
		activeFile ? annotationPersistenceByFile[activeFile.index] ?? null : null,
	);
	// Persistence state exists only once the file's server-side annotations
	// have loaded. Editing before then would save a set built from nothing and
	// replace the file's stored ROIs.
	const annotationsReady = $derived(activeAnnotationPersistence !== null);
	const selectedRoiIndex = $derived(activeFile ? selectedRoiByFile[activeFile.index] ?? null : null);
	const imageRows = $derived(
		pipelineMode === "segmentation_overlay" && segmentationOverlay
			? segmentationOverlay.sourceFile.rows
			: pipelineMode === "diagnostic_wl" && currentRawFrame
			? currentRawFrame.metadata.rows
			: activeFile?.rows ?? 0,
	);
	const imageColumns = $derived(
		pipelineMode === "segmentation_overlay" && segmentationOverlay
			? segmentationOverlay.sourceFile.columns
			: pipelineMode === "diagnostic_wl" && currentRawFrame
			? currentRawFrame.metadata.columns
			: activeFile?.columns ?? 0,
	);
	const displayGeometry = $derived(
		imageDisplayGeometry(
			imageRows,
			imageColumns,
			segmentationOverlay?.sourceFile.pixel_aspect_ratio ?? activeFile?.pixel_aspect_ratio,
		),
	);
	const visibleRois = $derived(
		segmentationOverlay ? [] : deriveVisibleRois(activeAnnotations, currentFrame),
	);
	const draftRoi = $derived(
		dragState?.mode === "draw_roi"
			? canonicalRect(dragState.start, dragState.current, imageRows, imageColumns)
			: null,
	);
	const roiListCountLabel = $derived(
		activeAnnotations ? `${visibleRois.length} / ${activeAnnotations.num_roi}` : String(visibleRois.length),
	);

	function deriveVisibleRois(annotations: EmbedRoiAnnotations | null, frameIndex: number): VisibleRoi[] {
		if (!annotations || annotations.roi_coords.length === 0) return [];
		const appliesToAllFrames = annotations.roi_frames.length === 0;
		const visible: VisibleRoi[] = [];
		for (let idx = 0; idx < annotations.roi_coords.length; idx += 1) {
			const [ymin, xmin, ymax, xmax] = annotations.roi_coords[idx];
			const frames = appliesToAllFrames ? null : annotations.roi_frames[idx] ?? [];
			if (frames !== null && !frames.includes(frameIndex)) continue;
			visible.push({ index: idx, ymin, xmin, ymax, xmax, frames });
		}
		return visible;
	}

	function formatRoiFrames(frames: number[] | null): string {
		if (frames === null || isAllFrames(frames, activeFile?.frame_count ?? 0)) return "all frames";
		if (frames.length === 0) return "no frame mapping";
		const preview = frames.slice(0, 6).join(", ");
		return frames.length > 6 ? `frames ${preview}, …` : `frames ${preview}`;
	}

	function setSelectedRoi(index: number | null) {
		if (!activeFile) return;
		if ((selectedRoiByFile[activeFile.index] ?? null) === index) return;
		selectedRoiByFile = {
			...selectedRoiByFile,
			[activeFile.index]: index,
		};
	}

	function setAnnotationsForFile(fileIndex: number, annotations: EmbedRoiAnnotations) {
		annotationsByFile = {
			...annotationsByFile,
			[fileIndex]: annotations,
		};
	}

	function currentEditableAnnotations(): EmbedRoiAnnotations {
		return normalizeAnnotationsForEdit(activeAnnotations, activeFile?.frame_count ?? 0);
	}

	function commitAnnotations(annotations: EmbedRoiAnnotations, selectedIndex: number | null = selectedRoiIndex) {
		if (!activeFile || !annotationPersistence.get(activeFile.index)) return;
		setAnnotationsForFile(activeFile.index, annotations);
		setSelectedRoi(selectedIndex);
		annotationPersistence.edit(activeFile.index, annotations);
	}

	function retryAnnotationLoad() {
		if (!activeFile || annotationPersistence.get(activeFile.index)) return;
		loadAnnotations(activeFile.index);
	}

	function retryAnnotationSave() {
		if (!activeFile || !annotationPersistence.get(activeFile.index)) return;
		annotationPersistence.retry(activeFile.index);
	}

	function rollbackAnnotationSave() {
		if (!activeFile || !annotationPersistence.get(activeFile.index)) return;
		annotationPersistence.rollback(activeFile.index);
		setSelectedRoi(null);
	}

	function pointFromPointer(event: PointerEvent): ImagePoint | null {
		if (!roiSvgEl) return null;
		const matrix = roiSvgEl.getScreenCTM();
		if (!matrix) return null;
		const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse());
		return {
			x: Math.min(imageColumns, Math.max(0, point.x)),
			y: Math.min(imageRows, Math.max(0, point.y)),
		};
	}

	function hitTestRoi(point: ImagePoint): { roi: VisibleRoi; handle: RoiHandle | null } | null {
		const tolerance = Math.max(3, 8 / Math.max(activeTransform.scale, 0.2));
		for (let idx = visibleRois.length - 1; idx >= 0; idx -= 1) {
			const roi = visibleRois[idx];
			const handle = hitTestHandle(roi, point, tolerance);
			if (handle) return { roi, handle };
			const x0 = Math.min(roi.xmin, roi.xmax);
			const x1 = Math.max(roi.xmin, roi.xmax);
			const y0 = Math.min(roi.ymin, roi.ymax);
			const y1 = Math.max(roi.ymin, roi.ymax);
			if (point.x >= x0 && point.x <= x1 && point.y >= y0 && point.y <= y1) {
				return { roi, handle: null };
			}
		}
		return null;
	}

	function hitTestHandle(roi: VisibleRoi, point: ImagePoint, tolerance: number): RoiHandle | null {
		for (const handle of roiHandles(roi)) {
			if (Math.abs(point.x - handle.x) <= tolerance && Math.abs(point.y - handle.y) <= tolerance) {
				return handle.handle;
			}
		}
		return null;
	}

	function roiHandles(roi: VisibleRoi): Array<{ handle: RoiHandle; x: number; y: number }> {
		const x0 = Math.min(roi.xmin, roi.xmax);
		const x1 = Math.max(roi.xmin, roi.xmax);
		const y0 = Math.min(roi.ymin, roi.ymax);
		const y1 = Math.max(roi.ymin, roi.ymax);
		const cx = (x0 + x1) / 2;
		const cy = (y0 + y1) / 2;
		return [
			{ handle: "nw", x: x0, y: y0 },
			{ handle: "n", x: cx, y: y0 },
			{ handle: "ne", x: x1, y: y0 },
			{ handle: "e", x: x1, y: cy },
			{ handle: "se", x: x1, y: y1 },
			{ handle: "s", x: cx, y: y1 },
			{ handle: "sw", x: x0, y: y1 },
			{ handle: "w", x: x0, y: cy },
		];
	}

	function deleteSelectedRoi() {
		if (!activeFile || selectedRoiIndex === null || !activeAnnotations) return;
		const next = deleteRoi(activeAnnotations, selectedRoiIndex, activeFile.frame_count);
		commitAnnotations(next, null);
	}

	function setSelectedScope(scope: "current" | "all") {
		if (!activeFile || selectedRoiIndex === null || !activeAnnotations) return;
		const next = setRoiFrameScope(activeAnnotations, selectedRoiIndex, scope, currentFrame, activeFile.frame_count);
		commitAnnotations(next, selectedRoiIndex);
	}

	function ensureWlWorker(): boolean {
		if (workerInitAttempted) {
			return workerAvailable;
		}
		workerInitAttempted = true;
		try {
			wlWorker = new Worker(new URL("./workers/wlRenderer.worker.ts", import.meta.url), { type: "module" });
			wlWorker.onmessage = (event: MessageEvent<WlRendererResponse>) => {
				const payload = event.data;
				if (payload.type === "error") {
					const pending = pendingWorkerResponses.get(payload.id);
					if (!pending) return;
					pendingWorkerResponses.delete(payload.id);
					pending.reject(new Error(payload.message));
					return;
				}
				const pending = pendingWorkerResponses.get(payload.id);
				if (!pending) return;
				pendingWorkerResponses.delete(payload.id);
				pending.resolve({ width: payload.width, height: payload.height, bitmap: payload.bitmap });
			};
			wlWorker.onerror = () => {
				workerAvailable = false;
			};
			workerAvailable = true;
			return true;
		} catch {
			workerAvailable = false;
			wlWorker = null;
			return false;
		}
	}

	function shouldUseWorker(frame: RawFrame): boolean {
		const pixels = frame.metadata.rows * frame.metadata.columns;
		return pixels >= WORKER_MIN_PIXEL_THRESHOLD && ensureWlWorker();
	}

	async function renderWithWorker(frame: RawFrame, wc: number, ww: number): Promise<ImageBitmap> {
		if (!wlWorker || !workerAvailable) {
			throw new Error("worker unavailable");
		}
		const id = ++workerMessageId;
		const copiedBuffer = frame.buffer.slice(0);
		const pending = new Promise<WlRenderedFrame>((resolve, reject) => {
			pendingWorkerResponses.set(id, { resolve, reject });
		});
		const request: WlRendererRequest = {
			type: "render",
			id,
			metadata: frame.metadata,
			wc,
			ww,
			buffer: copiedBuffer,
		};
		wlWorker.postMessage(request, [copiedBuffer]);
		const result = await pending;
		return result.bitmap;
	}

	function clearCanvas(): void {
		renderedFrameToken = "";
		if (!canvasEl) return;
		const ctx = canvasEl.getContext("2d", { alpha: false });
		if (!ctx) return;
		ctx.clearRect(0, 0, canvasEl.width, canvasEl.height);
	}

	function invalidateWindowLevelRenders(): void {
		wlRenderGeneration += 1;
	}

	function rawFrameCacheKey(fileIndex: number, frameIndex: number): string {
		return `${fileIndex}:${frameIndex}`;
	}

	function clearRawFrameCache(): void {
		rawRequests.abortAll();
		rawFrameCache.clear();
	}

	function getCachedRawFrame(fileIndex: number, frameIndex: number): RawFrame | undefined {
		return rawFrameCache.get(rawFrameCacheKey(fileIndex, frameIndex));
	}

	function cacheRawFrame(fileIndex: number, frameIndex: number, frame: RawFrame): void {
		rawFrameCache.set(rawFrameCacheKey(fileIndex, frameIndex), frame);
	}

	function ensureRawFrame(fileIndex: number, frameIndex: number): Promise<RawFrame> {
		const key = rawFrameCacheKey(fileIndex, frameIndex);
		const cached = rawFrameCache.get(key);
		if (cached) return Promise.resolve(cached);
		return rawRequests.request(key, (signal) => fetchRawFrame(fileIndex, frameIndex, signal));
	}

	function clearDisplayCache(): void {
		for (const waiter of displayRenderWaiters) waiter.resolve(false);
		displayRenderWaiters.clear();
		displayFrameCaches.blobs.clear();
		displayFrameCaches.bitmaps.clear();
		displayDecodePromises.clear();
		displayNetworkPromises.clear();
		displayFetchPromises.clear();
		lastRenderedDisplayFrame = null;
		renderedFrameToken = "";
	}

	function getCachedDisplayBlob(key: string): Blob | undefined {
		return displayFrameCaches.blobs.get(key);
	}

	function cacheDisplayBlob(key: string, blob: Blob): Blob | null {
		return displayFrameCaches.blobs.set(key, blob) ? blob : null;
	}

	function currentDisplayWindowOptions(): DisplayFrameWindowOptions {
		if (pipelineMode === "segmentation_overlay") return {};
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

	function buildDisplayKey(fileIndex: number, frameIndex: number, options: DisplayFrameWindowOptions): string {
		return displayFrameCacheKey(fileIndex, frameIndex, options);
	}

	function displayPrefetchScope(options: DisplayFrameWindowOptions): string {
		return `${navigationScopeKey}:${displayFrameWindowCacheKey(options)}`;
	}

	function ensureDisplayFetchScope(options: DisplayFrameWindowOptions): AbortSignal {
		const scopeKey = displayPrefetchScope(options);
		if (displayScopeCtrl && displayFetchScopeKey === scopeKey) {
			return displayScopeCtrl.signal;
		}
		displayScopeCtrl?.abort();
		for (const waiter of displayRenderWaiters) waiter.resolve(false);
		displayRenderWaiters.clear();
		displayScopeCtrl = new AbortController();
		displayFetchScopeKey = scopeKey;
		displayNetworkPromises.clear();
		displayFetchPromises.clear();
		lastRenderedDisplayFrame = null;
		renderedFrameToken = "";
		return displayScopeCtrl.signal;
	}

	async function ensureDisplayFrameBlob(
		fileIndex: number,
		frameIndex: number,
		windowOptions: DisplayFrameWindowOptions,
	): Promise<Blob> {
		const key = buildDisplayKey(fileIndex, frameIndex, windowOptions);
		const cached = getCachedDisplayBlob(key);
		if (cached) return cached;
		const existing = displayFetchPromises.get(key);
		if (existing) return existing;

		const signal = ensureDisplayFetchScope(windowOptions);
		const networkRequest = fetchDisplayFrameBlob(fileIndex, frameIndex, windowOptions, signal);
		displayNetworkPromises.set(key, networkRequest);
		void networkRequest.then(
			() => {
				if (displayNetworkPromises.get(key) === networkRequest) {
					displayNetworkPromises.delete(key);
				}
			},
			() => {
				if (displayNetworkPromises.get(key) === networkRequest) {
					displayNetworkPromises.delete(key);
				}
			},
		);
		const request = networkRequest
			.then((blob) => {
				const retained = cacheDisplayBlob(key, blob);
				if (!retained) throw new Error("Display frame exceeded PNG cache budget");
				return getCachedDisplayBlob(key) ?? retained;
			})
			.finally(() => {
				if (displayFetchPromises.get(key) === request) {
					displayFetchPromises.delete(key);
				}
			});
		displayFetchPromises.set(key, request);
		return request;
	}

	function markDisplayFrameRendered(fileIndex: number, frameIndex: number): void {
		renderedFrameToken = `${fileIndex}:${frameIndex}`;
		lastRenderedDisplayFrame = { fileIndex, frameIndex };
		for (const waiter of [...displayRenderWaiters]) {
			if (waiter.fileIndex !== fileIndex || waiter.frameIndex !== frameIndex) continue;
			displayRenderWaiters.delete(waiter);
			waiter.resolve(true);
		}
	}

	function waitForDisplayFrameRendered(
		fileIndex: number,
		frameIndex: number,
		signal: AbortSignal,
	): Promise<boolean> {
		if (
			lastRenderedDisplayFrame?.fileIndex === fileIndex
			&& lastRenderedDisplayFrame.frameIndex === frameIndex
		) return Promise.resolve(true);
		return waitForAbortableResult(signal, (settle) => {
			const waiter = { fileIndex, frameIndex, resolve: settle };
			displayRenderWaiters.add(waiter);
			return () => displayRenderWaiters.delete(waiter);
		});
	}

	function renderRawFrameOnMainThread(
		canvas: HTMLCanvasElement,
		frame: RawFrame,
		wc: number,
		ww: number,
	): void {
		const { rows, columns } = frame.metadata;
		canvas.width = columns;
		canvas.height = rows;
		const ctx = canvas.getContext("2d", { alpha: false });
		if (!ctx) return;
		const imageData = ctx.createImageData(columns, rows);
		imageData.data.set(renderRawFrameToRgba(frame, wc, ww));
		ctx.putImageData(imageData, 0, 0);
	}

	async function renderDiagnosticFrame(frame: RawFrame, wc: number, ww: number, generation: number): Promise<void> {
		if (!canvasEl) return;
		if (shouldUseWorker(frame)) {
			try {
				const bitmap = await renderWithWorker(frame, wc, ww);
				if (generation !== wlRenderGeneration || !canvasEl || pipelineMode !== "diagnostic_wl") {
					bitmap.close();
					return;
				}
				canvasEl.width = bitmap.width;
				canvasEl.height = bitmap.height;
				const ctx = canvasEl.getContext("2d", { alpha: false });
				ctx?.drawImage(bitmap, 0, 0);
				bitmap.close();
				return;
			} catch {
				workerAvailable = false;
			}
		}
		renderRawFrameOnMainThread(canvasEl, frame, wc, ww);
	}

	function startDisplayDecode(key: string, blob: Blob): Promise<ImageBitmap> {
		const cached = displayFrameCaches.bitmaps.get(key);
		if (cached) return Promise.resolve(cached);
		const pending = displayDecodePromises.get(key);
		if (pending?.blob === blob) return pending.promise;

		const promise = createImageBitmap(blob)
			.then((bitmap) => {
				if (displayFrameCaches.blobs.peek(key) === blob) {
					if (displayFrameCaches.bitmaps.set(key, bitmap)) return bitmap;
					throw new Error("Decoded display frame exceeded bitmap cache budget");
				}
				bitmap.close();
				throw new Error("display image decode superseded");
			})
			.finally(() => {
				if (displayDecodePromises.get(key)?.promise === promise) {
					displayDecodePromises.delete(key);
				}
			});
		displayDecodePromises.set(key, { blob, promise });
		return promise;
	}

	async function drawDisplayBlob(key: string, blob: Blob, generation: number): Promise<void> {
		if (!canvasEl || !usesDisplayPipeline()) return;
		const ctx = canvasEl.getContext("2d", { alpha: false });
		if (!ctx) return;

		if (typeof createImageBitmap === "function") {
			const bitmap = await startDisplayDecode(key, blob);
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

	function scheduleIdleOrImmediate(fn: () => void, timeout = 200): void {
		if (typeof requestIdleCallback === "function") {
			requestIdleCallback(fn, { timeout });
		} else {
			setTimeout(fn, 0);
		}
	}

	function derivePrefetchConcurrency(): number {
		const conn = (navigator as { connection?: { saveData?: boolean; effectiveType?: string } }).connection;
		if (!conn) return PREFETCH_CONCURRENCY;
		if (conn.saveData) return 1;
		const type = conn.effectiveType ?? "";
		if (type === "slow-2g" || type === "2g") return 1;
		if (type === "3g") return 2;
		return 4;
	}

	async function runRawPrefetch(
		frames: readonly NavigationFrameRef[],
		centerPosition: number,
		direction: 1 | -1,
		signal: AbortSignal,
	): Promise<void> {
		const targets = buildDirectionalFrameOrder(
			centerPosition,
			frames.length,
			RAW_RING_RADIUS,
			direction,
		);
		for (let i = 0; i < targets.length && !signal.aborted; i += prefetchConcurrency) {
			const batch = targets
				.slice(i, i + prefetchConcurrency)
				.map((position) => frames[position])
				.filter((frame): frame is NavigationFrameRef => frame !== undefined)
				.filter((frame) => !rawFrameCache.has(rawFrameCacheKey(frame.file_index, frame.frame_index)));
			if (batch.length === 0) continue;
			await Promise.allSettled(
				batch.map(async (frame) => {
					const key = rawFrameCacheKey(frame.file_index, frame.frame_index);
					if (signal.aborted || rawFrameCache.has(key)) return;
					try {
						const rawFrame = await ensureRawFrame(frame.file_index, frame.frame_index);
						if (signal.aborted) return;
						if (validateRenderableRawFrame(rawFrame) !== null) return;
						cacheRawFrame(frame.file_index, frame.frame_index, rawFrame);
					} catch {
						// Ignore network/decode failures during prefetch.
					}
				}),
			);
		}
	}

	async function runDisplayPrefetch(
		frames: readonly NavigationFrameRef[],
		startPosition: number,
		direction: 1 | -1,
		windowOptions: DisplayFrameWindowOptions,
		signal: AbortSignal,
		currentBlobSize: number,
		playbackMode: CineMode | null = null,
	): Promise<void> {
		const targets = planDisplayPrefetchTargets({
			startFrame: startPosition,
			totalFrames: frames.length,
			direction,
			currentPayloadBytes: currentBlobSize,
			fullStackBudgetBytes: DISPLAY_FULL_PREFETCH_BUDGET_BYTES,
			nearDistance: DISPLAY_NEAR_PREFETCH_DISTANCE,
			cineMode: playbackMode,
			lookaheadFrames: CINE_LOOKAHEAD_FRAMES,
		});
		for (let i = 0; i < targets.length && !signal.aborted; i += prefetchConcurrency) {
			const batch = targets.slice(i, i + prefetchConcurrency);
			await Promise.allSettled(
				batch.map(async (position) => {
					const frame = frames[position];
					if (!frame) return;
					const key = buildDisplayKey(frame.file_index, frame.frame_index, windowOptions);
					if (signal.aborted || displayFrameCaches.blobs.has(key)) return;
					try {
						await ensureDisplayFrameBlob(frame.file_index, frame.frame_index, windowOptions);
					} catch {
						// Ignore network/decode failures during prefetch.
					}
				}),
			);
		}
	}

function startDisplayPrefetch(
	frames: readonly NavigationFrameRef[],
	position: number,
	direction: 1 | -1,
	windowOptions: DisplayFrameWindowOptions,
	currentBlobSize: number,
): void {
	const scopeKey = displayPrefetchScope(windowOptions);

	if (cinePlaying) {
		// Playback follows its explicit loop/sweep order and reuses shared in-flight work.
		displayPrefetchCtrl?.abort();
		const ctrl = new AbortController();
		displayPrefetchCtrl = ctrl;
		displayPrefetchScopeKey = scopeKey;
		displayPrefetchSeedFrame = position;
		void runDisplayPrefetch(
			frames,
			position,
			direction,
			windowOptions,
			ctrl.signal,
			currentBlobSize,
			cineMode,
		).finally(() => {
			if (displayPrefetchCtrl === ctrl) {
				displayPrefetchCtrl = null;
				displayPrefetchScopeKey = "";
				displayPrefetchSeedFrame = null;
			}
		});
		return;
	}

	const shouldReusePrefetch =
		displayPrefetchCtrl !== null &&
		!displayPrefetchCtrl.signal.aborted &&
		displayPrefetchScopeKey === scopeKey &&
		displayPrefetchSeedFrame !== null &&
		Math.abs(position - displayPrefetchSeedFrame) <= PREFETCH_RESEED_DISTANCE;
	if (shouldReusePrefetch) return;

	displayPrefetchCtrl?.abort();
	const ctrl = new AbortController();
	displayPrefetchCtrl = ctrl;
	displayPrefetchScopeKey = scopeKey;
	displayPrefetchSeedFrame = position;

	scheduleIdleOrImmediate(() => {
		if (displayPrefetchCtrl !== ctrl) return;
		void runDisplayPrefetch(
			frames,
			position,
			direction,
			windowOptions,
			ctrl.signal,
			currentBlobSize,
		).finally(() => {
			if (displayPrefetchCtrl === ctrl) {
				displayPrefetchCtrl = null;
				displayPrefetchScopeKey = "";
				displayPrefetchSeedFrame = null;
			}
		});
	});
}

	async function loadRawFrameAndRender(
		fileIndex: number,
		frameIndex: number,
		generation: number,
		direction: 1 | -1,
	): Promise<void> {
		const cached = getCachedRawFrame(fileIndex, frameIndex);
		if (cached) {
			currentRawFrame = cached;
			loading = false;
			loadError = null;
			const prefetchScope = retainedScopeKey;
			const frames = navigationFrames;
			const position = navigationPosition;
			scheduleIdleOrImmediate(() => {
				if (retainedScopeKey !== prefetchScope || pipelineMode !== "diagnostic_wl") return;
				rawPrefetchCtrl?.abort();
				rawPrefetchCtrl = new AbortController();
				void runRawPrefetch(frames, position, direction, rawPrefetchCtrl.signal);
			});
			return;
		}

		try {
			const key = rawFrameCacheKey(fileIndex, frameIndex);
			const rawFrameRequest = ensureRawFrame(fileIndex, frameIndex);
			trackForegroundRequest(
				rawRequests.get(key),
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
			cacheRawFrame(fileIndex, frameIndex, rawFrame);
			currentRawFrame = rawFrame;
			loading = false;
			loadError = null;

			const prefetchScope = retainedScopeKey;
			const frames = navigationFrames;
			const position = navigationPosition;
			scheduleIdleOrImmediate(() => {
				if (retainedScopeKey !== prefetchScope || pipelineMode !== "diagnostic_wl") return;
				rawPrefetchCtrl?.abort();
				rawPrefetchCtrl = new AbortController();
				void runRawPrefetch(frames, position, direction, rawPrefetchCtrl.signal);
			});
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
		const cacheKey = buildDisplayKey(fileIndex, frameIndex, windowOptions);
		try {
			const blobRequest = ensureDisplayFrameBlob(fileIndex, frameIndex, windowOptions);
			trackForegroundRequest(
				displayNetworkPromises.get(cacheKey),
				() => generation === requestGeneration,
				(pending) => { loading = pending; },
			);
			const blob = await blobRequest;
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			loading = false;
			loadError = null;
			await drawDisplayBlob(cacheKey, blob, generation);
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			markDisplayFrameRendered(fileIndex, frameIndex);

			startDisplayPrefetch(
				navigationFrames,
				navigationPosition,
				direction,
				windowOptions,
				blob.size,
			);
		} catch (error) {
			if ((error as Error).name === "AbortError") return;
			if (generation !== requestGeneration || !usesDisplayPipeline()) return;
			loading = false;
			loadError = (error as Error).message || "Failed to load frame";
			cinePlaying = false;
		}
	}

	type DecodedCanvasImage = {
		source: CanvasImageSource;
		width: number;
		height: number;
		dispose: () => void;
	};

	async function decodeCanvasImage(blob: Blob): Promise<DecodedCanvasImage> {
		if (typeof createImageBitmap === "function") {
			const bitmap = await createImageBitmap(blob);
			return {
				source: bitmap,
				width: bitmap.width,
				height: bitmap.height,
				dispose: () => bitmap.close(),
			};
		}

		const url = URL.createObjectURL(blob);
		const image = new Image();
		image.decoding = "async";
		try {
			await new Promise<void>((resolve, reject) => {
				image.onload = () => resolve();
				image.onerror = () => reject(new Error("overlay image decode failed"));
				image.src = url;
			});
			return {
				source: image,
				width: image.naturalWidth,
				height: image.naturalHeight,
				dispose: () => URL.revokeObjectURL(url),
			};
		} catch (error) {
			URL.revokeObjectURL(url);
			throw error;
		}
	}

	async function loadSegmentationOverlayAndRender(
		overlay: SegmentationOverlay,
		generation: number,
	): Promise<void> {
		const windowOptions: DisplayFrameWindowOptions = {};
		const signal = ensureDisplayFetchScope(windowOptions);
		loading = true;
		try {
			const [sourceBlob, maskBlob] = await Promise.all([
				ensureDisplayFrameBlob(
					overlay.sourceFileIndex,
					overlay.sourceFrameIndex,
					windowOptions,
				),
				fetchSegmentationOverlayBlob(
					overlay.segmentationFileIndex,
					overlay.segmentationFrameIndex,
					signal,
				),
			]);
			const [sourceImage, maskImage] = await Promise.all([
				decodeCanvasImage(sourceBlob),
				decodeCanvasImage(maskBlob),
			]);
			try {
				if (
					generation !== requestGeneration
					|| pipelineMode !== "segmentation_overlay"
					|| !canvasEl
				) return;
				canvasEl.width = sourceImage.width;
				canvasEl.height = sourceImage.height;
				const ctx = canvasEl.getContext("2d", { alpha: false });
				if (!ctx) throw new Error("2D canvas is unavailable");
				ctx.drawImage(sourceImage.source, 0, 0);
				ctx.drawImage(maskImage.source, 0, 0, sourceImage.width, sourceImage.height);
				loading = false;
				loadError = null;
				markDisplayFrameRendered(activeFile.index, currentFrame);
			} finally {
				sourceImage.dispose();
				maskImage.dispose();
			}
		} catch (error) {
			if ((error as Error).name === "AbortError") return;
			if (generation !== requestGeneration || pipelineMode !== "segmentation_overlay") return;
			loading = false;
			loadError = (error as Error).message || "Failed to load segmentation overlay";
			cinePlaying = false;
		}
	}

	function sameTransform(a: TransformState | undefined, b: TransformState): boolean {
		return !!a
			&& a.fit === b.fit
			&& Math.abs(a.scale - b.scale) < 0.0001
			&& Math.abs(a.tx - b.tx) < 0.01
			&& Math.abs(a.ty - b.ty) < 0.01;
	}

	function updateTransform(index: number, transform: Omit<TransformState, "fit"> | TransformState, fit = false) {
		const next = { scale: transform.scale, tx: transform.tx, ty: transform.ty, fit };
		if (sameTransform(transformsByFile[index], next)) return;
		transformsByFile = {
			...transformsByFile,
			[index]: next,
		};
	}

	function clampZoom(scale: number): number {
		return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, scale));
	}

	function fitTransformForViewport(): TransformState | null {
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
		updateTransform(activeFile.index, transform, true);
	}

	function imageLayoutOrigin(): { left: number; top: number } | null {
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
		const existing = transformsByFile[activeFile.index];
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
		if (!activeFile) return;
		const fileIndex = activeFile.index;
		if (annotationsByFile[fileIndex] !== undefined || annotationRequestedByFile[fileIndex]) {
			return;
		}
		loadAnnotations(fileIndex);
	});

	function loadAnnotations(fileIndex: number) {
		// Direct mutation — annotationRequestedByFile is not $state, so this
		// does not trigger an effect re-run and will not fire the cleanup.
		annotationRequestedByFile[fileIndex] = true;
		annotationLoadingByFile = {
			...annotationLoadingByFile,
			[fileIndex]: true,
		};
		annotationErrorsByFile = {
			...annotationErrorsByFile,
			[fileIndex]: null,
		};

		void fetchAnnotations(fileIndex)
			.then((annotations) => {
				annotationPersistence.initialize(fileIndex, annotations);
			})
			.catch((error) => {
				annotationErrorsByFile = {
					...annotationErrorsByFile,
					[fileIndex]: (error as Error).message || "Failed to load annotations",
				};
			})
			.finally(() => {
				annotationLoadingByFile = {
					...annotationLoadingByFile,
					[fileIndex]: false,
				};
			});
	}

	$effect(() => {
		const nextScope = navigationScopeKey;
		if (!nextScope || nextScope === retainedScopeKey) return;
		retainedScopeKey = nextScope;
		invalidateWindowLevelRenders();
		rawPrefetchCtrl?.abort();
		displayScopeCtrl?.abort();
		displayScopeCtrl = null;
		displayFetchScopeKey = "";
		displayPrefetchCtrl?.abort();
		displayPrefetchCtrl = null;
		displayPrefetchScopeKey = "";
		displayPrefetchSeedFrame = null;
		clearRawFrameCache();
		clearDisplayCache();
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
			rawPrefetchCtrl?.abort();
			rawPrefetchCtrl = null;
			invalidateWindowLevelRenders();
			liveWindowCenter = null;
			liveWindowWidth = null;
		} else {
			displayScopeCtrl?.abort();
			displayScopeCtrl = null;
			displayFetchScopeKey = "";
			displayPrefetchCtrl?.abort();
			displayPrefetchCtrl = null;
			displayPrefetchScopeKey = "";
			displayPrefetchSeedFrame = null;
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
		if (mode === "segmentation_overlay") {
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
		ensureDisplayFetchScope(windowOptions);

		void (async () => {
			const initialFrame = navigationFrameAtPosition(frames, scheduledPosition);
			if (!initialFrame) return;
			if (!await waitForDisplayFrameRendered(
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
					return ensureDisplayFrameBlob(frame.file_index, frame.frame_index, windowOptions)
						.then((blob) => typeof createImageBitmap === "function"
							? startDisplayDecode(buildDisplayKey(frame.file_index, frame.frame_index, windowOptions), blob)
							: undefined);
				},
				presentFrame: async (step, signal) => {
					const frame = navigationFrameAtPosition(frames, step.frame);
					if (!frame) return false;
					direction = step.direction;
					scheduledPosition = step.frame;
					cineDirection = direction;
					onnavigationchange(scheduledPosition);
					return waitForDisplayFrameRendered(frame.file_index, frame.frame_index, signal);
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
		const overlay = segmentationOverlay;
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
		if (mode === "segmentation_overlay" && overlay) {
			void loadSegmentationOverlayAndRender(overlay, generation);
		} else if (mode === "diagnostic_wl") {
			void loadRawFrameAndRender(fileIndex, frameIndex, generation, frameDirection);
		} else {
			void loadDisplayFrameAndRender(fileIndex, frameIndex, generation, frameDirection);
		}
	});

	$effect(() => {
		if (pipelineMode !== "diagnostic_wl" || !currentRawFrame || !canvasEl) return;
		const window = resolveDisplayWindow(
			currentRawFrame,
			liveWindowCenter,
			liveWindowWidth,
			windowCenter,
			windowWidth,
			windowMode,
		);
		const generation = ++wlRenderGeneration;
		void renderDiagnosticFrame(currentRawFrame, window.wc, window.ww, generation);
	});

	$effect(() => {
		prefetchConcurrency = derivePrefetchConcurrency();
		const conn = (navigator as { connection?: { addEventListener: (t: string, fn: () => void) => void; removeEventListener: (t: string, fn: () => void) => void } }).connection;
		if (!conn) return;
		const update = () => { prefetchConcurrency = derivePrefetchConcurrency(); };
		conn.addEventListener("change", update);
		return () => conn.removeEventListener("change", update);
	});

	$effect(() => {
		return () => {
			rawPrefetchCtrl?.abort();
			displayScopeCtrl?.abort();
			displayPrefetchCtrl?.abort();
			displayPrefetchCtrl = null;
			displayPrefetchScopeKey = "";
			displayPrefetchSeedFrame = null;
			clearRawFrameCache();
			clearDisplayCache();
			for (const pending of pendingWorkerResponses.values()) {
				pending.reject(new Error("viewport disposed"));
			}
			pendingWorkerResponses.clear();
			wlWorker?.terminate();
			wlWorker = null;
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
		dragState = null;
	});

	function zoomAnchorFromClient(clientX: number, clientY: number): ZoomAnchor | null {
		const origin = imageLayoutOrigin();
		if (!origin) return null;
		const { scale, tx, ty } = activeTransform;
		return {
			clientX,
			clientY,
			localX: (clientX - origin.left - tx) / scale,
			localY: (clientY - origin.top - ty) / scale,
		};
	}

	function zoomTransformForAnchor(newScale: number, anchor: ZoomAnchor): Omit<TransformState, "fit"> | null {
		const origin = imageLayoutOrigin();
		if (!origin) return null;
		const clamped = clampZoom(newScale);
		return {
			scale: clamped,
			tx: anchor.clientX - origin.left - anchor.localX * clamped,
			ty: anchor.clientY - origin.top - anchor.localY * clamped,
		};
	}

	function zoomAt(newScale: number, clientX: number, clientY: number) {
		if (!activeFile || !canvasEl) return;
		const anchor = zoomAnchorFromClient(clientX, clientY);
		if (!anchor) return;
		const transform = zoomTransformForAnchor(newScale, anchor);
		if (!transform) return;
		updateTransform(activeFile.index, transform);
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
		updateTransform(activeFile.index, transform);
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
			updateTransform(activeFile.index, {
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
			if (segmentationOverlay && (activeTool === "window_level" || activeTool === "annotate_rect")) {
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
					const hit = hitTestRoi(point);
					if (hit) {
						setSelectedRoi(hit.roi.index);
						const original: RoiCoord = [hit.roi.ymin, hit.roi.xmin, hit.roi.ymax, hit.roi.xmax];
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
			updateTransform(activeFile.index, {
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
			setAnnotationsForFile(activeFile.index, next);
			return;
		}

		if (dragState.mode === "resize_roi" && activeAnnotations) {
			const point = pointFromPointer(event);
			if (!point) return;
			const resized = resizeCoord(dragState.original, dragState.handle, point, imageRows, imageColumns);
			if (!resized) return;
			const next = updateRoiCoord(activeAnnotations, dragState.roiIndex, resized, activeFile.frame_count);
			setAnnotationsForFile(activeFile.index, next);
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
			if (coord && activeFile) {
				const next = addRoi(activeAnnotations, coord, currentFrame, activeFile.frame_count);
				commitAnnotations(next, next.num_roi - 1);
			}
		}
		if ((dragState?.mode === "move_roi" || dragState?.mode === "resize_roi") && activeAnnotations) {
			commitAnnotations(activeAnnotations, selectedRoiIndex);
		}
		dragState = null;
	}

	function onPointerCancel() {
		if ((dragState?.mode === "move_roi" || dragState?.mode === "resize_roi") && activeFile) {
			const next = updateRoiCoord(currentEditableAnnotations(), dragState.roiIndex, dragState.original, activeFile.frame_count);
			setAnnotationsForFile(activeFile.index, next);
		}
		dragState = null;
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
		const current = activeTransform.scale;
		if (direction > 0) {
			const next = ZOOM_STEPS.find((step) => step > current + 0.001);
			if (next !== undefined) zoomToLevel(next);
		} else {
			const previous = [...ZOOM_STEPS].reverse().find((step) => step < current - 0.001);
			if (previous !== undefined) zoomToLevel(previous);
		}
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
				data-capture-rendered={renderedFrameToken}
			></canvas>
			{#if !segmentationOverlay && imageColumns > 0 && imageRows > 0}
				<svg
					bind:this={roiSvgEl}
					class="roi-overlay"
					viewBox={`0 0 ${imageColumns} ${imageRows}`}
					preserveAspectRatio="none"
					aria-hidden="true"
				>
					{#each visibleRois as roi (roi.index)}
						<g class:selected={selectedRoiIndex === roi.index}>
							<rect
								class="roi-rect"
								x={Math.min(roi.xmin, roi.xmax)}
								y={Math.min(roi.ymin, roi.ymax)}
								width={Math.max(1, Math.abs(roi.xmax - roi.xmin))}
								height={Math.max(1, Math.abs(roi.ymax - roi.ymin))}
							></rect>
							<text
								class="roi-label"
								x={Math.min(roi.xmin, roi.xmax) + 3}
								y={Math.max(10, Math.min(roi.ymin, roi.ymax) - 4)}
							>#{roi.index + 1}</text>
							{#if selectedRoiIndex === roi.index}
								{#each roiHandles(roi) as handle}
									<circle class="roi-handle" cx={handle.x} cy={handle.y} r={4}></circle>
								{/each}
							{/if}
						</g>
					{/each}
					{#if draftRoi}
						<rect
							class="roi-rect draft"
							x={draftRoi[1]}
							y={draftRoi[0]}
							width={Math.max(1, draftRoi[3] - draftRoi[1])}
							height={Math.max(1, draftRoi[2] - draftRoi[0])}
						></rect>
					{/if}
				</svg>
			{/if}
		</div>
		<div class="overlay">
			{#if segmentationOverlay}
				<span>SEG overlay {segmentationOverlay.segmentationFrameIndex + 1} / {activeFile.frame_count}</span>
				<span>source frame {segmentationOverlay.sourceFrameIndex + 1}</span>
			{:else}
				<span>image {navigationPosition + 1} / {navigationFrameCount}</span>
				<span>source frame {currentFrame + 1} / {activeFile.frame_count}</span>
			{/if}
			<span>W: {Math.round(displayWindow.ww)} · C: {Math.round(displayWindow.wc)}</span>
			{#if activeTool === "window_level" && !activeFile.raw_windowing_compatible}
				<span class="presentation-path" title={activeFile.raw_windowing_reason ?? undefined}>server presentation retained</span>
			{/if}
		</div>
		{#if !segmentationOverlay}<div class="roi-list">
			<div class="roi-list-title">
				<span>ROIs {roiListCountLabel}</span>
				{#if activeAnnotationPersistence?.status === "saving"}
					<span class="roi-save-status">saving…</span>
				{:else if activeAnnotationPersistence?.status === "dirty"}
					<span class="roi-save-status">unsaved</span>
				{/if}
			</div>
			{#if activeAnnotationLoading}
				<div class="roi-list-status">Loading annotations…</div>
			{:else if activeAnnotationError}
				<div class="roi-list-status error">
					<span>{activeAnnotationError}</span>
					{#if activeAnnotationPersistence?.status === "error"}
						<div class="roi-error-actions">
							<button type="button" onclick={retryAnnotationSave}>Retry</button>
							<button type="button" onclick={rollbackAnnotationSave}>Revert</button>
						</div>
					{:else if !annotationsReady}
						<div class="roi-error-actions">
							<button type="button" onclick={retryAnnotationLoad}>Retry</button>
						</div>
					{/if}
				</div>
			{:else if visibleRois.length === 0}
				<div class="roi-list-status">No ROIs for this frame</div>
			{:else}
				<ul>
					{#each visibleRois as roi (roi.index)}
						<li class:selected={selectedRoiIndex === roi.index}>
							<button type="button" class="roi-select" onclick={() => setSelectedRoi(roi.index)}>
								<span class="roi-id">#{roi.index + 1}</span>
							</button>
							<span class="roi-coords">[{roi.ymin}, {roi.xmin}, {roi.ymax}, {roi.xmax}]</span>
							<span class="roi-frames">{formatRoiFrames(roi.frames)}</span>
							{#if selectedRoiIndex === roi.index}
								<div class="roi-actions">
									<button type="button" onclick={() => setSelectedScope("current")}>Current</button>
									<button type="button" onclick={() => setSelectedScope("all")}>All</button>
									<button type="button" class="danger" onclick={deleteSelectedRoi}>Delete</button>
								</div>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</div>{/if}
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
	.roi-overlay {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		pointer-events: none;
	}
	.roi-rect {
		fill: rgba(255, 115, 115, 0.12);
		stroke: #ff7373;
		stroke-width: 1.2;
		vector-effect: non-scaling-stroke;
	}
	.roi-overlay g.selected .roi-rect {
		fill: rgba(74, 158, 255, 0.16);
		stroke: #4a9eff;
		stroke-width: 1.6;
	}
	.roi-rect.draft {
		fill: rgba(255, 212, 92, 0.14);
		stroke: #ffd45c;
		stroke-dasharray: 5 4;
	}
	.roi-label {
		fill: #ffdede;
		stroke: rgba(0, 0, 0, 0.75);
		stroke-width: 2.4;
		paint-order: stroke;
		font-size: 11px;
		font-family: ui-monospace, monospace;
		vector-effect: non-scaling-stroke;
	}
	.roi-overlay g.selected .roi-label {
		fill: #c8ddff;
	}
	.roi-handle {
		fill: #4a9eff;
		stroke: #101820;
		stroke-width: 1;
		vector-effect: non-scaling-stroke;
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
	.roi-list {
		position: absolute;
		right: 0.75rem;
		top: 0.75rem;
		max-width: min(48ch, 46%);
		max-height: 38%;
		overflow: auto;
		font-size: 0.72rem;
		padding: 0.5rem 0.55rem;
		background: rgba(28, 28, 30, 0.78);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-panel);
		box-shadow: var(--shadow-hud);
		backdrop-filter: blur(16px);
		z-index: 2;
		scrollbar-width: thin;
	}
	.roi-list-title {
		display: flex;
		justify-content: space-between;
		gap: 0.75rem;
		font-weight: 600;
		margin-bottom: 0.25rem;
		color: var(--text-primary);
	}
	.roi-save-status {
		color: var(--text-muted);
		font-weight: 400;
	}
	.roi-list-status {
		color: var(--text-muted);
	}
	.roi-list-status.error {
		color: var(--danger);
	}
	.roi-error-actions {
		display: flex;
		gap: 0.25rem;
		margin-top: 0.35rem;
	}
	.roi-error-actions button {
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.68rem;
		padding: 0.15rem 0.35rem;
	}
	.roi-list ul {
		margin: 0;
		padding: 0;
		list-style: none;
		display: grid;
		gap: 0.2rem;
	}
	.roi-list li {
		display: grid;
		gap: 0.1rem;
		padding: 0.18rem 0;
		border-top: 1px solid rgba(255, 255, 255, 0.08);
	}
	.roi-list li.selected {
		background: var(--accent-soft);
		margin-inline: -0.25rem;
		padding-inline: 0.25rem;
		border-radius: 4px;
	}
	.roi-list li:first-child {
		border-top: none;
		padding-top: 0;
	}
	.roi-select {
		width: fit-content;
		background: none;
		border: none;
		color: inherit;
		padding: 0;
		cursor: pointer;
	}
	.roi-select:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
		border-radius: 3px;
	}
	.roi-id {
		font-weight: 600;
		color: #9fcbff;
	}
	.roi-coords,
	.roi-frames {
		font-family: var(--font-mono);
		line-height: 1.25;
		color: var(--text-secondary);
	}
	.roi-actions {
		display: flex;
		gap: 0.25rem;
		margin-top: 0.15rem;
	}
	.roi-actions button {
		background: var(--surface-control);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-control);
		color: var(--text-secondary);
		cursor: pointer;
		font-size: 0.68rem;
		padding: 0.15rem 0.35rem;
	}
	.roi-actions button:hover {
		background: var(--surface-control-hover);
		color: var(--text-primary);
	}
	.roi-actions button:focus-visible {
		outline: none;
		box-shadow: var(--focus-ring);
	}
	.roi-actions button.danger {
		color: #ffb0b0;
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

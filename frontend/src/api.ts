import type {
	ApiErrorCode,
	DoseOverlayQuery,
	EmbedRoiAnnotations,
	ErrorResponse,
	FilesResponse,
	FrameQuery,
	FrameValueMapping,
	FrameWindowApplied,
	GraphicAnnotationsQuery,
	GraphicAnnotationsResponse,
	HealthResponse,
	ParametricMapOverlayQuery,
	PixelQuery,
	RawFrameMetadata,
	RedactionSeriesResponse,
	ReferenceCatalogResponse,
	SemanticContextResponse,
	SeriesCatalogResponse,
	TagNode,
	TagQuery,
	WindowMode,
	WsiFrameContextResponse,
} from "./generated/api-types";
import { API_ENDPOINTS, API_RESPONSE_HEADERS, DISPLAY_FRAME_HEADERS, RAW_FRAME_HEADERS } from "./generated/api-types";
import type { RawFrame } from "./rawFrame";

export type {
	ApiErrorCode,
	EmbedRoiAnnotations,
	ErrorResponse,
	FileSummary,
	FilesResponse,
	FrameQuery,
	FrameInfo,
	FrameValueMapping,
	FrameWindowApplied,
	GraphicAnnotationItemSummary,
	GraphicAnnotationsResponse,
	GraphicLayerSummary,
	GraphicObjectSummary,
	HealthResponse,
	ModalityValueTransform,
	OverlayLegend,
	RawFrameMetadata,
	RealWorldValueMap,
	RealWorldValueTransform,
	RedactionSeriesResponse,
	ReferenceCatalogResponse,
	ReferenceMatchSummary,
	ReferenceSummary,
	ReferenceTargetSummary,
	SemanticContext,
	SemanticContextResponse,
	SegmentationContext,
	ParametricMapContext,
	PresentationStateContext,
	RtDoseContext,
	SeriesCatalogResponse,
	SeriesSummary,
	SeriesStackSummary,
	FrameRefSummary,
	SeriesWarningSummary,
	TagNode,
	TagQuery,
	TagValue,
	TextObjectSummary,
	ValueLookupTable,
	WindowMode,
	WindowPreset,
	WsiFrameContextResponse,
	WsiTileRectangle,
	WsiTotalPixelMatrix,
} from "./generated/api-types";
export type { RawFrame } from "./rawFrame";

type Endpoint = (typeof API_ENDPOINTS)[keyof typeof API_ENDPOINTS];
type PathParams = { index?: number; frame?: number };

/**
 * Fills `{index}`/`{frame}` in a generated path and appends defined query values.
 * The result is relative to the viewer page (`api/...`), so a reverse proxy can
 * serve the viewer under a path prefix.
 */
function endpointUrl(
	endpoint: Endpoint,
	params: PathParams = {},
	query?: FrameQuery | TagQuery | DoseOverlayQuery | ParametricMapOverlayQuery | GraphicAnnotationsQuery | PixelQuery,
): string {
	const path = endpoint.path.slice(1).replace(/\{(\w+)\}/g, (_, name: string) => {
		const value = params[name as keyof PathParams];
		if (value === undefined) {
			throw new Error(`missing API path parameter ${name} for ${endpoint.path}`);
		}
		return encodeURIComponent(String(value));
	});
	const search = new URLSearchParams();
	for (const [name, value] of Object.entries(query ?? {})) {
		if (value !== undefined) {
			search.set(name, String(value));
		}
	}
	const encoded = search.toString();
	return encoded.length > 0 ? `${path}?${encoded}` : path;
}

/** A non-2xx API response: the server's message, status, and stable error `code`. */
export class ApiError extends Error {
	readonly status: number;
	readonly code: ApiErrorCode | null;

	constructor(message: string, status: number, code: ApiErrorCode | null) {
		super(message);
		this.name = "ApiError";
		this.status = status;
		this.code = code;
	}
}

/** True when `error` is an API response carrying `code`. */
export function isApiError(error: unknown, code: ApiErrorCode): boolean {
	return error instanceof ApiError && error.code === code;
}

async function readServerError(response: Response): Promise<Partial<ErrorResponse>> {
	try {
		return (await response.json()) as Partial<ErrorResponse>;
	} catch {
		return {};
	}
}

/** Status of an `ApiError` for a request that never reached the server. */
export const UNREACHABLE_STATUS = 0;
const UNREACHABLE_MESSAGE = "dcmview is not reachable: the viewer process may have stopped";

let catalogServerInstance: string | null = null;
let serverReplaced = false;
const restartListeners = new Set<() => void>();

/** A changed server invalidates every file index and cache in the loaded page. */
export function onServerRestart(listener: () => void): () => void {
	restartListeners.add(listener);
	if (serverReplaced) listener();
	return () => restartListeners.delete(listener);
}

function checkServerInstance(instance: string | null): void {
	if (catalogServerInstance === null || instance === null || instance === catalogServerInstance && !serverReplaced) return;
	if (!serverReplaced) {
		serverReplaced = true;
		for (const listener of restartListeners) listener();
	}
	// Do not let a response from another catalog reach an index-keyed cache.
	throw new DOMException("dcmview server was replaced", "AbortError");
}

let serverReachable = true;
const reachabilityListeners = new Set<(reachable: boolean) => void>();

/** Calls `listener` whenever requests start or stop reaching the server. Returns unsubscribe. */
export function onReachabilityChange(listener: (reachable: boolean) => void): () => void {
	reachabilityListeners.add(listener);
	return () => reachabilityListeners.delete(listener);
}

function setReachable(reachable: boolean): void {
	if (reachable === serverReachable) return;
	serverReachable = reachable;
	for (const listener of reachabilityListeners) listener(reachable);
}

/**
 * Sends one request and turns non-2xx responses into an `ApiError`. A request
 * that cannot reach the server at all (the process stopped, or the tunnel
 * closed) becomes one with `UNREACHABLE_STATUS` and a plain message instead
 * of the browser's bare "Failed to fetch".
 */
async function send(endpoint: Endpoint, url: string, init: RequestInit = {}): Promise<Response> {
	let response: Response;
	try {
		response = await fetch(url, { ...init, method: endpoint.method });
	} catch (error) {
		if ((error as Error).name === "AbortError") throw error;
		setReachable(false);
		throw new ApiError(UNREACHABLE_MESSAGE, UNREACHABLE_STATUS, null);
	}
	checkServerInstance(response.headers.get(API_RESPONSE_HEADERS.serverInstance));
	setReachable(true);
	if (!response.ok) {
		const body = await readServerError(response);
		const message = typeof body.error === "string" && body.error.length > 0
			? body.error
			: `HTTP ${response.status}: ${endpoint.method} ${url} failed`;
		const code = typeof body.code === "string" ? body.code : null;
		throw new ApiError(message, response.status, code);
	}
	return response;
}

async function getJson<T>(endpoint: Endpoint, url: string, signal?: AbortSignal): Promise<T> {
	const response = await send(endpoint, url, { signal });
	return (await response.json()) as T;
}

export function fetchHealth(): Promise<HealthResponse> {
	return getJson(API_ENDPOINTS.health, endpointUrl(API_ENDPOINTS.health));
}

export async function fetchFiles(): Promise<FilesResponse> {
	const files = await getJson<FilesResponse>(API_ENDPOINTS.files, endpointUrl(API_ENDPOINTS.files));
	if (Number.isFinite(files.server_start_ms)) {
		const instance = String(files.server_start_ms);
		checkServerInstance(instance);
		catalogServerInstance ??= instance;
	}
	return files;
}

export function fetchSeries(): Promise<SeriesCatalogResponse> {
	return getJson(API_ENDPOINTS.series, endpointUrl(API_ENDPOINTS.series));
}

export function fetchReferences(
	fileIndex: number,
	signal?: AbortSignal,
): Promise<ReferenceCatalogResponse> {
	const endpoint = API_ENDPOINTS.fileReferences;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex }), signal);
}

export function fetchSemanticContext(fileIndex: number): Promise<SemanticContextResponse> {
	const endpoint = API_ENDPOINTS.fileSemanticContext;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex }));
}

/** A grayscale frame's display shutter and overlay graphics, transparent elsewhere. */
export async function fetchPresentationLayerBlob(
	fileIndex: number,
	frame: number,
	signal?: AbortSignal,
): Promise<Blob> {
	const endpoint = API_ENDPOINTS.filePresentationLayer;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }), { signal });
	return response.blob();
}

export async function fetchSegmentationOverlayBlob(
	fileIndex: number,
	frame: number,
	signal?: AbortSignal,
): Promise<Blob> {
	const endpoint = API_ENDPOINTS.fileSegmentationOverlay;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }), {
		signal,
	});
	return response.blob();
}

/** Colorwash PNG of RT Dose `doseFileIndex` resampled onto one displayed frame. */
export async function fetchDoseOverlayBlob(
	fileIndex: number,
	frame: number,
	doseFileIndex: number,
	signal?: AbortSignal,
): Promise<Blob> {
	const endpoint = API_ENDPOINTS.fileDoseOverlay;
	const url = endpointUrl(endpoint, { index: fileIndex, frame }, { dose: doseFileIndex });
	const response = await send(endpoint, url, { signal });
	return response.blob();
}

/** Colorwash PNG of Parametric Map `mapFileIndex` resampled onto one displayed frame. */
export async function fetchParametricMapOverlayBlob(
	fileIndex: number,
	frame: number,
	mapFileIndex: number,
	signal?: AbortSignal,
): Promise<Blob> {
	const endpoint = API_ENDPOINTS.fileParametricMapOverlay;
	const url = endpointUrl(endpoint, { index: fileIndex, frame }, { map: mapFileIndex });
	const response = await send(endpoint, url, { signal });
	return response.blob();
}

/** Little-endian `f32` values of a values response, one per displayed pixel. */
/** The annotations softcopy presentation state `stateFileIndex` draws on one image frame. */
export function fetchGraphicAnnotations(
	fileIndex: number,
	frame: number,
	stateFileIndex: number,
	signal?: AbortSignal,
): Promise<GraphicAnnotationsResponse> {
	const endpoint = API_ENDPOINTS.fileGraphicAnnotations;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }, { state: stateFileIndex }), signal);
}

async function overlayValues(response: Response): Promise<Float32Array> {
	const buffer = await response.arrayBuffer();
	if (buffer.byteLength % 4 !== 0) throw new Error("overlay values are not whole f32 samples");
	const view = new DataView(buffer);
	const values = new Float32Array(buffer.byteLength / 4);
	for (let index = 0; index < values.length; index += 1) values[index] = view.getFloat32(index * 4, true);
	return values;
}

/** RT Dose `doseFileIndex` resampled onto one displayed frame, in Gy (NaN outside). */
export async function fetchDoseOverlayValues(
	fileIndex: number,
	frame: number,
	doseFileIndex: number,
	signal?: AbortSignal,
): Promise<Float32Array> {
	const endpoint = API_ENDPOINTS.fileDoseOverlayValues;
	const url = endpointUrl(endpoint, { index: fileIndex, frame }, { dose: doseFileIndex });
	return overlayValues(await send(endpoint, url, { signal }));
}

/** Parametric Map `mapFileIndex` resampled onto one displayed frame, in its unit. */
export async function fetchParametricMapOverlayValues(
	fileIndex: number,
	frame: number,
	mapFileIndex: number,
	signal?: AbortSignal,
): Promise<Float32Array> {
	const endpoint = API_ENDPOINTS.fileParametricMapOverlayValues;
	const url = endpointUrl(endpoint, { index: fileIndex, frame }, { map: mapFileIndex });
	return overlayValues(await send(endpoint, url, { signal }));
}

export function fetchFrameValueMapping(
	fileIndex: number,
	frame: number,
	signal?: AbortSignal,
): Promise<FrameValueMapping> {
	const endpoint = API_ENDPOINTS.fileValueMapping;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }), signal);
}

export function fetchWsiFrameContext(
	fileIndex: number,
	frame: number,
): Promise<WsiFrameContextResponse> {
	const endpoint = API_ENDPOINTS.fileWsiContext;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }));
}

export function fetchTags(fileIndex: number, signal?: AbortSignal): Promise<TagNode[]> {
	const endpoint = API_ENDPOINTS.fileTags;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex }), signal);
}

export function fetchSelectedTag(
	fileIndex: number,
	query: TagQuery,
	signal?: AbortSignal,
): Promise<TagNode> {
	const endpoint = API_ENDPOINTS.fileTagSelect;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex }, query), signal);
}

export function fetchAnnotations(fileIndex: number): Promise<EmbedRoiAnnotations> {
	const endpoint = API_ENDPOINTS.fileAnnotationsGet;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex }));
}

export async function updateAnnotations(
	fileIndex: number,
	annotations: EmbedRoiAnnotations,
): Promise<EmbedRoiAnnotations> {
	const endpoint = API_ENDPOINTS.fileAnnotationsUpdate;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex }), {
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(annotations),
	});
	return (await response.json()) as EmbedRoiAnnotations;
}

export function annotationsExportUrl(): string {
	return endpointUrl(API_ENDPOINTS.annotationsExport);
}

export function fetchRedactions(fileIndex: number): Promise<EmbedRoiAnnotations> {
	const endpoint = API_ENDPOINTS.fileRedactionsGet;
	return getJson(endpoint, endpointUrl(endpoint, { index: fileIndex }));
}

/** Replaces the file's redaction boxes, which the server then applies to its frames. */
export async function updateRedactions(
	fileIndex: number,
	boxes: EmbedRoiAnnotations,
): Promise<EmbedRoiAnnotations> {
	const endpoint = API_ENDPOINTS.fileRedactionsUpdate;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex }), {
		headers: { "Content-Type": "application/json" },
		body: JSON.stringify(boxes),
	});
	return (await response.json()) as EmbedRoiAnnotations;
}

/** Copies the file's redaction boxes to the same-sized files of its series. */
export async function applyRedactionsToSeries(fileIndex: number): Promise<RedactionSeriesResponse> {
	const endpoint = API_ENDPOINTS.fileRedactionsApplyToSeries;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex }));
	return (await response.json()) as RedactionSeriesResponse;
}

export function frameUrl(
	fileIndex: number,
	frame: number,
	{ wc, ww, windowMode, unit, preview }: DisplayFrameWindowOptions = {},
): string {
	// Full-dynamic windowing ignores explicit values, so they are not sent.
	const window: FrameQuery =
		windowMode === "full_dynamic"
			? { mode: "full_dynamic" }
			: { wc: wc ?? undefined, ww: ww ?? undefined, unit: unit ?? undefined };
	const query: FrameQuery = preview ? { ...window, preview: true } : window;
	return endpointUrl(API_ENDPOINTS.fileFrame, { index: fileIndex, frame }, query);
}

export interface DisplayFrameWindowOptions {
	wc?: number | null;
	ww?: number | null;
	windowMode?: WindowMode | null;
	/**
	 * Real-world unit of `wc`/`ww`. The viewer converts such a window
	 * through each frame's own linear mapping where it can
	 * (`frameDisplayWindowOptions`); otherwise the server windows the frame's
	 * preferred mapping in this unit.
	 */
	unit?: string | null;
	/** A window/level drag preview, which the server does not cache. */
	preview?: boolean;
}

export function displayFrameWindowCacheKey(
	options: DisplayFrameWindowOptions = {},
): string {
	if (options.windowMode === "full_dynamic") {
		return "full_dynamic:none:none";
	}
	const wc = options.wc === null || options.wc === undefined ? "none" : String(options.wc);
	const ww = options.ww === null || options.ww === undefined ? "none" : String(options.ww);
	const mode = options.windowMode ?? "default";
	return options.unit ? `${mode}:${wc}:${ww}:${options.unit}` : `${mode}:${wc}:${ww}`;
}

export function displayFrameCacheKey(
	fileIndex: number,
	frame: number,
	options: DisplayFrameWindowOptions = {},
): string {
	return `${fileIndex}:${frame}:${displayFrameWindowCacheKey(options)}`;
}

/** A display PNG and the window the server rendered it with. */
export type DisplayFrame = {
	blob: Blob;
	/**
	 * The linear window applied, in Modality values; null for color frames,
	 * frames presented through a VOI LUT, and a window applied in a
	 * real-world `unit`. A `unit` request that reports one was shown with
	 * that default window instead.
	 */
	window: { wc: number; ww: number } | null;
	/** The server's explicit presentation kind; null for color or older servers. */
	appliedWindow: FrameWindowApplied | null;
};

export async function fetchDisplayFrame(
	fileIndex: number,
	frame: number,
	options: DisplayFrameWindowOptions = {},
	signal?: AbortSignal,
): Promise<DisplayFrame> {
	const url = frameUrl(fileIndex, frame, options);
	const response = await send(API_ENDPOINTS.fileFrame, url, { signal });
	const kind = response.headers.get(DISPLAY_FRAME_HEADERS.windowApplied);
	const appliedWindow = kind === "linear" || kind === "real_world" || kind === "voi_lut" ? kind : null;
	return { blob: await response.blob(), window: parseDisplayWindow(response.headers), appliedWindow };
}

function parseDisplayWindow(headers: Headers): DisplayFrame["window"] {
	const wc = Number(headers.get(DISPLAY_FRAME_HEADERS.windowCenter) ?? Number.NaN);
	const ww = Number(headers.get(DISPLAY_FRAME_HEADERS.windowWidth) ?? Number.NaN);
	return Number.isFinite(wc) && Number.isFinite(ww) ? { wc, ww } : null;
}

function requiredHeader(headers: Headers, name: string): string {
	const value = headers.get(name);
	if (value === null || value.trim() === "") {
		throw new Error(`raw frame response missing required header ${name}`);
	}
	return value;
}

function parseRequiredIntHeader(headers: Headers, name: string): number {
	const value = Number(requiredHeader(headers, name));
	if (!Number.isInteger(value)) {
		throw new Error(`raw frame response has invalid integer header ${name}`);
	}
	return value;
}

function parseRequiredFloatHeader(headers: Headers, name: string): number {
	const value = Number(requiredHeader(headers, name));
	if (!Number.isFinite(value)) {
		throw new Error(`raw frame response has invalid numeric header ${name}`);
	}
	return value;
}

function parseOptionalFloatHeader(headers: Headers, name: string): number | null {
	const raw = headers.get(name);
	if (raw === null || raw.trim() === "") {
		return null;
	}
	const value = Number(raw);
	if (!Number.isFinite(value)) {
		throw new Error(`raw frame response has invalid numeric header ${name}`);
	}
	return value;
}

export function parseRawFrameMetadata(headers: Headers): RawFrameMetadata {
	return {
		rows: parseRequiredIntHeader(headers, RAW_FRAME_HEADERS.rows),
		columns: parseRequiredIntHeader(headers, RAW_FRAME_HEADERS.columns),
		bitsAllocated: parseRequiredIntHeader(headers, RAW_FRAME_HEADERS.bitsAllocated),
		pixelRepresentation: parseRequiredIntHeader(headers, RAW_FRAME_HEADERS.pixelRepresentation),
		samplesPerPixel: parseRequiredIntHeader(headers, RAW_FRAME_HEADERS.samplesPerPixel),
		photometricInterpretation: requiredHeader(headers, RAW_FRAME_HEADERS.photometricInterpretation),
		rescaleSlope: parseRequiredFloatHeader(headers, RAW_FRAME_HEADERS.rescaleSlope),
		rescaleIntercept: parseRequiredFloatHeader(headers, RAW_FRAME_HEADERS.rescaleIntercept),
		defaultWc: parseOptionalFloatHeader(headers, RAW_FRAME_HEADERS.defaultWc),
		defaultWw: parseOptionalFloatHeader(headers, RAW_FRAME_HEADERS.defaultWw),
		paddingLow: parseOptionalFloatHeader(headers, RAW_FRAME_HEADERS.paddingLow),
		paddingHigh: parseOptionalFloatHeader(headers, RAW_FRAME_HEADERS.paddingHigh),
	};
}

export async function fetchRawFrame(
	fileIndex: number,
	frame: number,
	signal?: AbortSignal,
): Promise<RawFrame> {
	const endpoint = API_ENDPOINTS.fileRawFrame;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }), {
		signal,
	});
	const buffer = await response.arrayBuffer();
	return { metadata: parseRawFrameMetadata(response.headers), buffer };
}

/**
 * One pixel of a raw frame as a 1x1 raw frame, its samples in
 * color-by-pixel order: the readout's source where a whole frame is too
 * large to fetch for one value.
 */
export async function fetchRawPixel(
	fileIndex: number,
	frame: number,
	pixel: PixelQuery,
	signal?: AbortSignal,
): Promise<RawFrame> {
	const endpoint = API_ENDPOINTS.fileRawPixel;
	const response = await send(endpoint, endpointUrl(endpoint, { index: fileIndex, frame }, pixel), {
		signal,
	});
	const buffer = await response.arrayBuffer();
	return { metadata: parseRawFrameMetadata(response.headers), buffer };
}

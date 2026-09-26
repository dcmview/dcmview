import type {
	ApiErrorCode,
	DoseOverlayQuery,
	EmbedRoiAnnotations,
	ErrorResponse,
	FilesResponse,
	FrameQuery,
	FrameValueMapping,
	ParametricMapOverlayQuery,
	RawFrameMetadata,
	ReferenceCatalogResponse,
	SemanticContextResponse,
	SeriesCatalogResponse,
	TagNode,
	TagQuery,
	WindowMode,
	WsiFrameContextResponse,
} from "./generated/api-types";
import { API_ENDPOINTS, RAW_FRAME_HEADERS } from "./generated/api-types";
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
	HealthResponse,
	ModalityValueTransform,
	OverlayLegend,
	RawFrameMetadata,
	RealWorldValueMap,
	RealWorldValueTransform,
	ReferenceCatalogResponse,
	ReferenceMatchSummary,
	ReferenceSummary,
	ReferenceTargetSummary,
	SemanticContext,
	SemanticContextResponse,
	SegmentationContext,
	ParametricMapContext,
	RtDoseContext,
	SeriesCatalogResponse,
	SeriesSummary,
	SeriesStackSummary,
	FrameRefSummary,
	SeriesWarningSummary,
	TagNode,
	TagQuery,
	TagValue,
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

/** Fills `{index}`/`{frame}` in a generated path and appends defined query values. */
function endpointUrl(
	endpoint: Endpoint,
	params: PathParams = {},
	query?: FrameQuery | TagQuery | DoseOverlayQuery | ParametricMapOverlayQuery,
): string {
	const path = endpoint.path.replace(/\{(\w+)\}/g, (_, name: string) => {
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

/** Sends one request and turns non-2xx responses into an `ApiError`. */
async function send(endpoint: Endpoint, url: string, init: RequestInit = {}): Promise<Response> {
	const response = await fetch(url, { ...init, method: endpoint.method });
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

export function fetchFiles(): Promise<FilesResponse> {
	return getJson(API_ENDPOINTS.files, endpointUrl(API_ENDPOINTS.files));
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

export function frameUrl(
	fileIndex: number,
	frame: number,
	wc?: number | null,
	ww?: number | null,
	windowMode?: WindowMode | null,
): string {
	// Full-dynamic windowing ignores explicit values, so they are not sent.
	const query: FrameQuery =
		windowMode === "full_dynamic"
			? { mode: "full_dynamic" }
			: { wc: wc ?? undefined, ww: ww ?? undefined };
	return endpointUrl(API_ENDPOINTS.fileFrame, { index: fileIndex, frame }, query);
}

export interface DisplayFrameWindowOptions {
	wc?: number | null;
	ww?: number | null;
	windowMode?: WindowMode | null;
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
	return `${mode}:${wc}:${ww}`;
}

export function displayFrameCacheKey(
	fileIndex: number,
	frame: number,
	options: DisplayFrameWindowOptions = {},
): string {
	return `${fileIndex}:${frame}:${displayFrameWindowCacheKey(options)}`;
}

export async function fetchDisplayFrameBlob(
	fileIndex: number,
	frame: number,
	options: DisplayFrameWindowOptions = {},
	signal?: AbortSignal,
): Promise<Blob> {
	const url = frameUrl(fileIndex, frame, options.wc, options.ww, options.windowMode);
	const response = await send(API_ENDPOINTS.fileFrame, url, { signal });
	return response.blob();
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

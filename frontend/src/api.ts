import type {
	ApiEndpointParams,
	ApiEndpointQuery,
	ApiEndpointRequest,
	ApiEndpointResponse,
	EmbedRoiAnnotations,
	ErrorResponse,
	FilesResponse,
	FrameInfo,
	JsonApiEndpointId,
	RawFrameMetadata,
	ReferenceCatalogResponse,
	SemanticContextResponse,
	SeriesCatalogResponse,
	TagNode,
	TagQuery,
	WindowMode,
	WsiFrameContextResponse,
} from "./generated/api-types";
import {
	API_ENDPOINTS,
	FRAME_QUERY_KEYS,
	RAW_FRAME_HEADERS,
	apiEndpointPath,
	getApiEndpointPath,
} from "./generated/api-types";
import type { RawFrame } from "./rawFrame";

export type {
	ApiEndpointId,
	ApiEndpointError,
	ApiEndpointParams,
	ApiEndpointQuery,
	ApiEndpointRequest,
	ApiEndpointResponse,
	ApiEndpointResponseHeaders,
	ApiEndpointTypes,
	EmbedRoiAnnotations,
	ErrorResponse,
	FileSummary,
	FilesResponse,
	FrameQuery,
	FrameInfo,
	HealthResponse,
	RawFrameMetadata,
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
	WindowMode,
	WindowPreset,
	WsiFrameContextResponse,
	WsiTileRectangle,
	WsiTotalPixelMatrix,
} from "./generated/api-types";
export type { RawFrame } from "./rawFrame";

async function readServerError(response: Response): Promise<string | null> {
	try {
		const body = (await response.json()) as Partial<ErrorResponse>;
		return typeof body.error === "string" && body.error.length > 0 ? body.error : null;
	} catch {
		return null;
	}
}

async function responseError(response: Response, fallback: string): Promise<Error> {
	const serverMessage = await readServerError(response);
	return new Error(serverMessage ?? `HTTP ${response.status}: ${fallback}`);
}

type JsonRequestArguments<Id extends JsonApiEndpointId> = [
	ApiEndpointRequest<Id>,
] extends [never]
	? []
	: [body: ApiEndpointRequest<Id>];

async function requestJsonEndpoint<Id extends JsonApiEndpointId>(
	id: Id,
	params: ApiEndpointParams<Id>,
	...requestArguments: JsonRequestArguments<Id>
): Promise<ApiEndpointResponse<Id>> {
	const endpoint = API_ENDPOINTS[id];
	const path = apiEndpointPath(id, params);
	const init: RequestInit = { method: endpoint.method };
	if (requestArguments.length > 0) {
		if (endpoint.requestMediaType === null) {
			throw new Error(`endpoint ${id} does not declare a request media type`);
		}
		init.headers = { "Content-Type": endpoint.requestMediaType };
		init.body = JSON.stringify(requestArguments[0]);
	}
	const response = await fetch(path, init);
	if (response.status !== endpoint.successStatus) {
		throw await responseError(response, `request failed: ${path}`);
	}
	return (await response.json()) as ApiEndpointResponse<Id>;
}

export function fetchFiles(): Promise<FilesResponse> {
	return requestJsonEndpoint("files", {});
}

export function fetchSeries(): Promise<SeriesCatalogResponse> {
	return requestJsonEndpoint("series", {});
}

export function fetchReferences(fileIndex: number): Promise<ReferenceCatalogResponse> {
	return requestJsonEndpoint("fileReferences", { index: fileIndex });
}

export function fetchSemanticContext(fileIndex: number): Promise<SemanticContextResponse> {
	return requestJsonEndpoint("fileSemanticContext", { index: fileIndex });
}

export async function fetchSegmentationOverlayBlob(
	fileIndex: number,
	frame: number,
	signal?: AbortSignal,
): Promise<Blob> {
	const path = apiEndpointPath("fileSegmentationOverlay", { index: fileIndex, frame });
	const response = await fetch(path, { signal });
	if (response.status !== API_ENDPOINTS.fileSegmentationOverlay.successStatus) {
		throw await responseError(response, `request failed: ${path}`);
	}
	return response.blob();
}

export function fetchWsiFrameContext(
	fileIndex: number,
	frame: number,
): Promise<WsiFrameContextResponse> {
	return requestJsonEndpoint("fileWsiContext", { index: fileIndex, frame });
}

export function fetchFrameInfo(fileIndex: number): Promise<FrameInfo> {
	return requestJsonEndpoint("fileInfo", { index: fileIndex });
}

export function fetchTags(fileIndex: number): Promise<TagNode[]> {
	return requestJsonEndpoint("fileTags", { index: fileIndex });
}

export function fetchAnnotations(fileIndex: number): Promise<EmbedRoiAnnotations> {
	return requestJsonEndpoint("fileAnnotationsGet", { index: fileIndex });
}

export function updateAnnotations(
	fileIndex: number,
	annotations: EmbedRoiAnnotations,
): Promise<EmbedRoiAnnotations> {
	return requestJsonEndpoint(
		"fileAnnotationsUpdate",
		{ index: fileIndex },
		annotations,
	);
}

export function annotationsExportUrl(): string {
	return getApiEndpointPath("annotationsExport", {});
}

export function frameUrl(
	fileIndex: number,
	frame: number,
	wc?: number | null,
	ww?: number | null,
	windowMode?: WindowMode | null,
): string {
	const path = apiEndpointPath("fileFrame", { index: fileIndex, frame });
	const query: ApiEndpointQuery<"fileFrame"> = {};
	if (windowMode !== "full_dynamic") {
		if (wc !== undefined && wc !== null) {
			query[FRAME_QUERY_KEYS.windowCenter] = wc;
		}
		if (ww !== undefined && ww !== null) {
			query[FRAME_QUERY_KEYS.windowWidth] = ww;
		}
	}
	if (windowMode === "full_dynamic") {
		query[FRAME_QUERY_KEYS.mode] = "full_dynamic";
	}
	const encoded = new URLSearchParams(
		Object.entries(query).map(([name, value]) => [name, String(value)]),
	).toString();
	return encoded.length > 0 ? `${path}?${encoded}` : path;
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
	const endpoint = API_ENDPOINTS.fileFrame;
	const response = await fetch(
		frameUrl(fileIndex, frame, options.wc, options.ww, options.windowMode),
		{ method: endpoint.method, signal },
	);
	if (response.status !== endpoint.successStatus) {
		throw await responseError(response, "display frame fetch failed");
	}
	return (await response.blob()) as ApiEndpointResponse<"fileFrame">;
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
	const response = await fetch(
		apiEndpointPath("fileRawFrame", { index: fileIndex, frame }),
		{ method: endpoint.method, signal },
	);
	if (response.status !== endpoint.successStatus) {
		throw await responseError(response, "raw frame fetch failed");
	}
	const buffer = (await response.arrayBuffer()) as ApiEndpointResponse<"fileRawFrame">;
	const metadata: RawFrameMetadata = parseRawFrameMetadata(response.headers);
	return { metadata, buffer };
}

export async function fetchSelectedTag(
	fileIndex: number,
	query: TagQuery,
	signal?: AbortSignal,
): Promise<TagNode> {
	const endpoint = API_ENDPOINTS.fileTagSelect;
	const parameters = new URLSearchParams({ path: query.path });
	if (query.offset !== undefined) parameters.set("offset", String(query.offset));
	if (query.limit !== undefined) parameters.set("limit", String(query.limit));
	const response = await fetch(
		`${getApiEndpointPath("fileTagSelect", { index: fileIndex })}?${parameters}`,
		{ method: endpoint.method, signal },
	);
	if (response.status !== endpoint.successStatus) {
		throw await responseError(response, "selective tag fetch failed");
	}
	return (await response.json()) as TagNode;
}

use serde::{Deserialize, Serialize};

pub const API_PREFIX: &str = "/api";

pub const CACHE_HEADER: &str = "X-Cache";
pub const CACHE_HIT: &str = "HIT";
pub const CACHE_MISS: &str = "MISS";
pub const EXPORT_CONTENT_DISPOSITION_HEADER: &str = "Content-Disposition";
pub const EXPORT_CONTENT_DISPOSITION_VALUE: &str =
    "attachment; filename=\"dcmview-annotations.csv\"";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiMethod {
    Get,
    Put,
}

impl ApiMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiResponseHeadersKind {
    None,
    Cache,
    RawFrame,
    Export,
}

pub trait ApiResponseHeadersSpec {
    const KIND: ApiResponseHeadersKind;
}

#[derive(Debug, Clone, Copy)]
pub struct NoResponseHeaders;

impl ApiResponseHeadersSpec for NoResponseHeaders {
    const KIND: ApiResponseHeadersKind = ApiResponseHeadersKind::None;
}

#[derive(Debug, Clone, Copy)]
pub struct CacheResponseHeaders;

impl ApiResponseHeadersSpec for CacheResponseHeaders {
    const KIND: ApiResponseHeadersKind = ApiResponseHeadersKind::Cache;
}

#[derive(Debug, Clone, Copy)]
pub struct RawFrameResponseHeaders;

impl ApiResponseHeadersSpec for RawFrameResponseHeaders {
    const KIND: ApiResponseHeadersKind = ApiResponseHeadersKind::RawFrame;
}

#[derive(Debug, Clone, Copy)]
pub struct ExportResponseHeaders;

impl ApiResponseHeadersSpec for ExportResponseHeaders {
    const KIND: ApiResponseHeadersKind = ApiResponseHeadersKind::Export;
}

#[derive(Debug, Clone, Copy)]
pub struct NoQuery;

#[derive(Debug, Clone, Copy)]
pub struct NoRequest;

#[derive(Debug, Clone, Copy)]
pub struct BlobBody;

#[derive(Debug, Clone, Copy)]
pub struct ArrayBufferBody;

pub trait ApiEndpointSpec {
    type Query;
    type Request;
    type Response;
    type ResponseHeaders: ApiResponseHeadersSpec;
    type Error;

    const CONTRACT: ApiEndpointContract;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiEndpointContract {
    pub operation: ApiOperation,
    pub id: &'static str,
    pub method: ApiMethod,
    pub path: &'static str,
    pub query_type: &'static str,
    pub request_type: &'static str,
    pub request_media_type: Option<&'static str>,
    pub response_type: &'static str,
    pub response_media_type: &'static str,
    pub response_headers_type: &'static str,
    pub response_headers: ApiResponseHeadersKind,
    pub error_type: &'static str,
    pub success_status: u16,
}

macro_rules! define_api_endpoints {
    (
        $(
            $constant:ident => {
                operation: $operation:ident,
                id: $id:literal,
                method: $method:ident,
                path: $path:literal,
                query_type: $query_type:ty,
                request_type: $request_type:ty,
                request_media_type: $request_media_type:expr,
                response_type: $response_type:ty,
                response_media_type: $response_media_type:literal,
                response_headers_type: $response_headers_type:ty,
                error_type: $error_type:ty,
                success_status: $success_status:literal
            }
        ),+ $(,)?
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ApiOperation {
            $($operation),+
        }

        $(
            #[derive(Debug, Clone, Copy)]
            pub struct $operation;

            impl ApiEndpointSpec for $operation {
                type Query = $query_type;
                type Request = $request_type;
                type Response = $response_type;
                type ResponseHeaders = $response_headers_type;
                type Error = $error_type;

                const CONTRACT: ApiEndpointContract = ApiEndpointContract {
                    operation: ApiOperation::$operation,
                    id: $id,
                    method: ApiMethod::$method,
                    path: $path,
                    query_type: stringify!($query_type),
                    request_type: stringify!($request_type),
                    request_media_type: $request_media_type,
                    response_type: stringify!($response_type),
                    response_media_type: $response_media_type,
                    response_headers_type: stringify!($response_headers_type),
                    response_headers:
                        <$response_headers_type as ApiResponseHeadersSpec>::KIND,
                    error_type: stringify!($error_type),
                    success_status: $success_status,
                };
            }

            pub const $constant: ApiEndpointContract =
                <$operation as ApiEndpointSpec>::CONTRACT;
        )+

        pub const API_ENDPOINTS: &[ApiEndpointContract] = &[$($constant),+];
    };
}

define_api_endpoints! {
    API_ENDPOINT_HEALTH => {
        operation: Health,
        id: "health",
        method: Get,
        path: "/health",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: HealthResponse,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILES => {
        operation: Files,
        id: "files",
        method: Get,
        path: "/files",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: FilesResponse,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_SERIES => {
        operation: Series,
        id: "series",
        method: Get,
        path: "/series",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: SeriesCatalogResponse,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_INFO => {
        operation: FileInfo,
        id: "fileInfo",
        method: Get,
        path: "/file/{index}/info",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: FrameInfo,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_REFERENCES => {
        operation: FileReferences,
        id: "fileReferences",
        method: Get,
        path: "/file/{index}/references",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: ReferenceCatalogResponse,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_SEMANTIC_CONTEXT => {
        operation: FileSemanticContext,
        id: "fileSemanticContext",
        method: Get,
        path: "/file/{index}/semantic-context",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: SemanticContextResponse,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_SEGMENTATION_OVERLAY => {
        operation: FileSegmentationOverlay,
        id: "fileSegmentationOverlay",
        method: Get,
        path: "/file/{index}/frame/{frame}/segmentation-overlay",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: BlobBody,
        response_media_type: "image/png",
        response_headers_type: CacheResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_WSI_CONTEXT => {
        operation: FileWsiContext,
        id: "fileWsiContext",
        method: Get,
        path: "/file/{index}/frame/{frame}/wsi-context",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: WsiFrameContextResponse,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_FRAME => {
        operation: FileFrame,
        id: "fileFrame",
        method: Get,
        path: "/file/{index}/frame/{frame}",
        query_type: FrameQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: BlobBody,
        response_media_type: "image/png",
        response_headers_type: CacheResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_RAW_FRAME => {
        operation: FileRawFrame,
        id: "fileRawFrame",
        method: Get,
        path: "/file/{index}/frame/{frame}/raw",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: ArrayBufferBody,
        response_media_type: "application/octet-stream",
        response_headers_type: RawFrameResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_TAGS => {
        operation: FileTags,
        id: "fileTags",
        method: Get,
        path: "/file/{index}/tags",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: Vec<TagNode>,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_TAG_SELECT => {
        operation: FileTagSelect,
        id: "fileTagSelect",
        method: Get,
        path: "/file/{index}/tags/select",
        query_type: TagQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: TagNode,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_ANNOTATIONS_GET => {
        operation: FileAnnotationsGet,
        id: "fileAnnotationsGet",
        method: Get,
        path: "/file/{index}/annotations",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: EmbedRoiAnnotations,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_FILE_ANNOTATIONS_UPDATE => {
        operation: FileAnnotationsUpdate,
        id: "fileAnnotationsUpdate",
        method: Put,
        path: "/file/{index}/annotations",
        query_type: NoQuery,
        request_type: EmbedRoiAnnotations,
        request_media_type: Some("application/json"),
        response_type: EmbedRoiAnnotations,
        response_media_type: "application/json",
        response_headers_type: NoResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
    API_ENDPOINT_ANNOTATIONS_EXPORT => {
        operation: AnnotationsExport,
        id: "annotationsExport",
        method: Get,
        path: "/annotations/export.csv",
        query_type: NoQuery,
        request_type: NoRequest,
        request_media_type: None,
        response_type: BlobBody,
        response_media_type: "text/csv; charset=utf-8",
        response_headers_type: ExportResponseHeaders,
        error_type: ErrorResponse,
        success_status: 200
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameQueryParameter {
    pub client_key: &'static str,
    pub wire_name: &'static str,
}

macro_rules! define_frame_query_parameters {
    (
        $(
            $constant:ident => {
                client_key: $client_key:literal,
                wire_name: $wire_name:literal
            }
        ),+ $(,)?
    ) => {
        $(pub const $constant: &str = $wire_name;)+

        pub const FRAME_QUERY_PARAMETERS: &[FrameQueryParameter] = &[
            $(FrameQueryParameter {
                client_key: $client_key,
                wire_name: $wire_name,
            }),+
        ];
    };
}

define_frame_query_parameters! {
    FRAME_QUERY_WINDOW_CENTER => {
        client_key: "windowCenter",
        wire_name: "wc"
    },
    FRAME_QUERY_WINDOW_WIDTH => {
        client_key: "windowWidth",
        wire_name: "ww"
    },
    FRAME_QUERY_MODE => {
        client_key: "mode",
        wire_name: "mode"
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFrameHeaderContract {
    pub field: &'static str,
    pub name: &'static str,
}

macro_rules! define_raw_frame_headers {
    (
        $(
            $constant:ident => {
                field: $field:literal,
                name: $name:literal
            }
        ),+ $(,)?
    ) => {
        $(pub const $constant: &str = $name;)+

        pub const RAW_FRAME_HEADERS: &[RawFrameHeaderContract] = &[
            $(RawFrameHeaderContract {
                field: $field,
                name: $name,
            }),+
        ];
    };
}

define_raw_frame_headers! {
    RAW_FRAME_HEADER_ROWS => {
        field: "rows",
        name: "X-Frame-Rows"
    },
    RAW_FRAME_HEADER_COLUMNS => {
        field: "columns",
        name: "X-Frame-Columns"
    },
    RAW_FRAME_HEADER_BITS_ALLOCATED => {
        field: "bitsAllocated",
        name: "X-Frame-Bits-Allocated"
    },
    RAW_FRAME_HEADER_PIXEL_REPRESENTATION => {
        field: "pixelRepresentation",
        name: "X-Frame-Pixel-Representation"
    },
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL => {
        field: "samplesPerPixel",
        name: "X-Frame-Samples-Per-Pixel"
    },
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION => {
        field: "photometricInterpretation",
        name: "X-Frame-Photometric-Interpretation"
    },
    RAW_FRAME_HEADER_RESCALE_SLOPE => {
        field: "rescaleSlope",
        name: "X-Frame-Rescale-Slope"
    },
    RAW_FRAME_HEADER_RESCALE_INTERCEPT => {
        field: "rescaleIntercept",
        name: "X-Frame-Rescale-Intercept"
    },
    RAW_FRAME_HEADER_DEFAULT_WC => {
        field: "defaultWc",
        name: "X-Frame-Default-Wc"
    },
    RAW_FRAME_HEADER_DEFAULT_WW => {
        field: "defaultWw",
        name: "X-Frame-Default-Ww"
    },
    RAW_FRAME_HEADER_PADDING_LOW => {
        field: "paddingLow",
        name: "X-Frame-Padding-Low"
    },
    RAW_FRAME_HEADER_PADDING_HIGH => {
        field: "paddingHigh",
        name: "X-Frame-Padding-High"
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WindowPreset {
    pub center: f64,
    pub width: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileSummary {
    pub index: usize,
    pub path: String,
    pub label: String,
    pub patient_id: String,
    pub patient_name: String,
    pub study_instance_uid: String,
    pub study_date: String,
    pub study_description: String,
    pub series_instance_uid: String,
    pub series_number: String,
    pub series_description: String,
    pub modality: String,
    pub instance_number: String,
    pub sop_instance_uid: String,
    pub sop_class_uid: String,
    pub object_kind: String,
    pub support_state: SupportState,
    pub support_reason: Option<String>,
    /// Whether client-side raw windowing preserves every declared presentation transform.
    pub raw_windowing_compatible: bool,
    /// Stable explanation when the frontend must retain the server-rendered presentation path.
    pub raw_windowing_reason: Option<String>,
    pub has_pixels: bool,
    pub frame_count: u32,
    pub rows: u32,
    pub columns: u32,
    /// Effective physical row-to-column pixel extent ratio.
    pub pixel_aspect_ratio: Option<f64>,
    pub transfer_syntax_uid: String,
    pub default_window: Option<WindowPreset>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilesResponse {
    pub files: Vec<FileSummary>,
    pub discovery: Vec<DiscoveryResult>,
    pub server_start_ms: u64,
    pub scan_complete: bool,
    pub scanned: usize,
    pub skipped: usize,
    pub filtered: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesCatalogResponse {
    pub series: Vec<SeriesSummary>,
    pub scan_complete: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceCatalogResponse {
    pub source_file_index: usize,
    pub source_sop_instance_uid: String,
    pub references: Vec<ReferenceSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceSummary {
    pub relationship: String,
    pub target: ReferenceTargetSummary,
    pub matches: Vec<ReferenceMatchSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceTargetSummary {
    pub sop_class_uid: Option<String>,
    pub sop_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
    /// DICOM-declared, one-based frame numbers.
    pub frame_numbers: Vec<u32>,
    pub segment_numbers: Vec<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReferenceMatchSummary {
    pub file_index: usize,
    pub path: String,
    pub sop_instance_uid: String,
    /// Validated, zero-based frame indices suitable for viewer navigation.
    pub frame_indices: Vec<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticContextResponse {
    pub source_file_index: usize,
    /// The normal frame endpoints remain the default and are never semantically transformed.
    pub default_mode: String,
    pub pixel_preview_preserves_stored_values: bool,
    pub context: SemanticContext,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticContext {
    Segmentation(SegmentationContext),
    ParametricMap(ParametricMapContext),
    RtDose(Box<RtDoseContext>),
    NotApplicable { reason: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct CodedConceptSummary {
    pub value: String,
    pub scheme: String,
    pub meaning: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SegmentationContext {
    pub segmentation_type: Option<String>,
    pub segmentation_fractional_type: Option<String>,
    pub maximum_fractional_value: Option<u32>,
    pub segments: Vec<SegmentSummary>,
    pub frame_mappings: Vec<SegmentFrameMapping>,
    pub references: Vec<ReferenceSummary>,
    pub overlay: OverlayEligibility,
}

#[derive(Debug, Clone, Serialize)]
pub struct SegmentSummary {
    pub number: u16,
    pub label: Option<String>,
    pub description: Option<String>,
    pub property_category: Option<CodedConceptSummary>,
    pub property_type: Option<CodedConceptSummary>,
    pub algorithm_type: Option<String>,
    pub algorithm_name: Option<String>,
    pub recommended_display_cielab: Option<Vec<u16>>,
    pub recommended_display_grayscale: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SegmentFrameMapping {
    /// Zero-based frame index in the segmentation object.
    pub frame_index: u32,
    pub segment_number: Option<u16>,
    pub source_sop_instance_uid: Option<String>,
    /// DICOM-declared, one-based source frame numbers.
    pub source_frame_numbers: Vec<u32>,
    pub source_file_indices: Vec<usize>,
    /// Normalized local source frames resolved from declared identity or geometry.
    pub source_frames: Vec<ResolvedSegmentSourceFrame>,
    /// `explicit_derivation` or `declared_source_geometry` when resolved.
    pub mapping_method: Option<String>,
    /// `resolved`, `missing`, or `ambiguous`.
    pub mapping_status: String,
    pub mapping_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedSegmentSourceFrame {
    pub file_index: usize,
    /// Zero-based frame index in the source object.
    pub frame_index: u32,
    pub sop_instance_uid: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayEligibility {
    pub eligible: bool,
    pub reason: String,
    pub source_file_index: Option<usize>,
    pub mapped_source_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ParametricMapContext {
    pub stored_value_type: String,
    pub displayed_value_kind: String,
    pub mappings: Vec<RealWorldValueMappingSummary>,
    pub mapping_status: String,
    pub source_references: Vec<ReferenceSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RealWorldValueMappingSummary {
    pub source: String,
    pub source_sop_instance_uid: Option<String>,
    pub label: Option<String>,
    pub first_value_mapped: Option<f64>,
    pub last_value_mapped: Option<f64>,
    pub slope: Option<f64>,
    pub intercept: Option<f64>,
    pub lut_data: Vec<f64>,
    pub lut_data_truncated: bool,
    pub units: Option<CodedConceptSummary>,
    pub quantity: Option<CodedConceptSummary>,
    pub derivation: Option<CodedConceptSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RtDoseContext {
    pub dose_grid_scaling: Option<f64>,
    pub scaling_status: String,
    pub displayed_value_kind: String,
    pub dose_units: Option<String>,
    pub dose_type: Option<String>,
    pub dose_summation_type: Option<String>,
    pub geometry: DoseGridGeometry,
    pub references: Vec<ReferenceSummary>,
    pub overlay: OverlayEligibility,
    pub clinical_use_warning: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoseGridGeometry {
    pub frame_of_reference_uid: Option<String>,
    pub image_position_patient: Option<[f64; 3]>,
    pub image_orientation_patient: Option<[f64; 6]>,
    pub pixel_spacing: Option<[f64; 2]>,
    pub grid_frame_offsets: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsiFrameContextResponse {
    pub source_file_index: usize,
    pub frame_index: u32,
    pub tile_frame_path: String,
    pub positioning_status: String,
    pub position_source: String,
    pub tiling_status: String,
    pub total_pixel_matrix: Option<WsiTotalPixelMatrix>,
    pub tile_rectangle: Option<WsiTileRectangle>,
    /// Zero-based tile row within the matrix grid.
    pub tile_row: Option<u64>,
    /// Zero-based tile column within the matrix grid.
    pub tile_column: Option<u64>,
    pub pyramid_uid: Option<String>,
    /// Zero is the highest-resolution declared matrix in this pyramid.
    pub pyramid_level: Option<u32>,
    pub optical_path: Option<WsiOpticalPath>,
    pub focal_plane: Option<WsiFocalPlane>,
    pub image_type_role: Option<String>,
    pub companions: Vec<WsiCompanionSummary>,
    pub companions_truncated: bool,
    pub relationships: Vec<ReferenceSummary>,
    pub relationships_truncated: bool,
    pub reconstruction_claimed: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsiTotalPixelMatrix {
    pub rows: u64,
    pub columns: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsiTileRectangle {
    /// Zero-based column offset in the Total Pixel Matrix.
    pub x: u64,
    /// Zero-based row offset in the Total Pixel Matrix.
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsiOpticalPath {
    pub index: Option<u32>,
    pub identifier: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsiFocalPlane {
    pub index: Option<u32>,
    pub z_offset_slide: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsiCompanionSummary {
    pub file_index: usize,
    pub sop_instance_uid: String,
    pub image_type_role: Option<String>,
    pub pyramid_uid: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesSummary {
    pub id: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub frame_of_reference_uids: Vec<String>,
    pub stacks: Vec<SeriesStackSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesStackSummary {
    pub id: String,
    pub kind: String,
    pub concatenation_uid: Option<String>,
    pub pyramid_uid: Option<String>,
    pub image_type_role: Option<String>,
    pub total_pixel_matrix_rows: Option<u32>,
    pub total_pixel_matrix_columns: Option<u32>,
    pub frames: Vec<FrameRefSummary>,
    pub warnings: Vec<SeriesWarningSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameRefSummary {
    pub virtual_index: usize,
    pub file_index: usize,
    pub frame_index: u32,
    pub source_path: String,
    pub sop_instance_uid: String,
    pub instance_number: Option<i32>,
    pub position_along_normal_mm: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesWarningSummary {
    pub code: String,
    pub message: String,
    pub file_indices: Vec<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameInfo {
    pub frame_count: u32,
    pub rows: u32,
    pub columns: u32,
    pub transfer_syntax_uid: String,
    pub has_pixels: bool,
    pub sop_class_uid: String,
    pub object_kind: String,
    pub support_state: SupportState,
    pub support_reason: Option<String>,
    pub default_window: Option<WindowPreset>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportState {
    Renderable,
    MetadataOnly,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveryResult {
    pub path: String,
    pub disposition: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    #[default]
    Default,
    FullDynamic,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagNode {
    pub tag: String,
    pub vr: String,
    pub keyword: String,
    pub value: TagValue,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TagValue {
    String {
        value: String,
    },
    Number {
        value: f64,
    },
    Numbers {
        value: Vec<f64>,
        #[serde(skip_serializing_if = "is_false")]
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<usize>,
    },
    Binary {
        length: usize,
    },
    Sequence {
        items: Vec<Vec<TagNode>>,
        #[serde(skip_serializing_if = "is_false")]
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        total: Option<usize>,
    },
    Error {
        message: String,
    },
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiErrorCode {
    InvalidPath,
    InvalidQuery,
    InvalidJson,
    BadRequest,
    NotFound,
    RouteNotFound,
    AssetNotFound,
    MethodNotAllowed,
    NoPixelData,
    FrameOutOfRange,
    InvalidWindow,
    UnsupportedTransferSyntax,
    UnsupportedPixelLayout,
    SemanticMappingUnavailable,
    PixelDecodeFailed,
    InternalError,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub code: ApiErrorCode,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawFrameMetadata {
    pub rows: u32,
    pub columns: u32,
    pub bits_allocated: u32,
    pub pixel_representation: u32,
    pub samples_per_pixel: u32,
    pub photometric_interpretation: String,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    pub default_wc: Option<f64>,
    pub default_ww: Option<f64>,
    /// Inclusive stored-value range of Pixel Padding, which the client must
    /// exclude from automatic windows and draw as black background.
    pub padding_low: Option<f64>,
    pub padding_high: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewerIdentity {
    pub name: &'static str,
    pub version: &'static str,
    pub build_git_sha: &'static str,
    pub build_target: &'static str,
    pub build_profile: &'static str,
}

impl ViewerIdentity {
    pub const fn current() -> Self {
        Self {
            name: "dcmview",
            version: env!("CARGO_PKG_VERSION"),
            build_git_sha: env!("DCMVIEW_BUILD_GIT_SHA"),
            build_target: env!("DCMVIEW_BUILD_TARGET"),
            build_profile: env!("DCMVIEW_BUILD_PROFILE"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub viewer: ViewerIdentity,
    pub file_count: usize,
    pub server_start_ms: u64,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct FrameQuery {
    pub wc: Option<f64>,
    pub ww: Option<f64>,
    pub mode: Option<WindowMode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TagQuery {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct EmbedRoiAnnotations {
    pub num_roi: usize,
    pub roi_coords: Vec<[u32; 4]>,
    pub roi_frames: Vec<Vec<u32>>,
}

impl EmbedRoiAnnotations {
    pub fn empty() -> Self {
        Self {
            num_roi: 0,
            roi_coords: Vec::new(),
            roi_frames: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ApiEndpointSpec, ApiMethod, ApiOperation, ApiResponseHeadersKind, BlobBody,
        CacheResponseHeaders, EmbedRoiAnnotations, ErrorResponse, FileFrame,
        FileInfo as FileInfoEndpoint, FrameInfo, FrameQuery, NoQuery, NoRequest, NoResponseHeaders,
        RawFrameMetadata, RawFrameResponseHeaders, API_ENDPOINTS, API_PREFIX,
        FRAME_QUERY_PARAMETERS, RAW_FRAME_HEADERS,
    };
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn derived_contract_registries_are_complete_unique_and_well_formed() {
        assert_eq!(API_PREFIX, "/api");
        assert!(API_ENDPOINTS
            .iter()
            .all(|endpoint| endpoint.path.starts_with('/')));
        assert_eq!(
            API_ENDPOINTS
                .iter()
                .map(|endpoint| endpoint.id)
                .collect::<HashSet<_>>()
                .len(),
            API_ENDPOINTS.len()
        );
        assert_eq!(
            API_ENDPOINTS
                .iter()
                .map(|endpoint| endpoint.operation)
                .collect::<HashSet<_>>()
                .len(),
            API_ENDPOINTS.len()
        );
        assert_eq!(
            API_ENDPOINTS
                .iter()
                .map(|endpoint| (endpoint.method.as_str(), endpoint.path))
                .collect::<HashSet<_>>()
                .len(),
            API_ENDPOINTS.len()
        );
        for endpoint in API_ENDPOINTS {
            assert_eq!(
                endpoint.request_type != "NoRequest",
                endpoint.request_media_type.is_some(),
                "{} request type/media declarations differ",
                endpoint.id
            );
            if endpoint.method == ApiMethod::Get {
                assert_eq!(
                    endpoint.request_type, "NoRequest",
                    "{} declares a body-bearing GET",
                    endpoint.id
                );
            }
            assert_eq!(endpoint.error_type, "ErrorResponse");
            assert!((200..300).contains(&endpoint.success_status));
        }
        let frame = API_ENDPOINTS
            .iter()
            .find(|endpoint| endpoint.operation == ApiOperation::FileFrame)
            .expect("file frame endpoint");
        assert_eq!(frame.query_type, "FrameQuery");
        assert_eq!(frame.response_headers, ApiResponseHeadersKind::Cache);
        let raw = API_ENDPOINTS
            .iter()
            .find(|endpoint| endpoint.operation == ApiOperation::FileRawFrame)
            .expect("raw frame endpoint");
        assert_eq!(raw.response_headers_type, "RawFrameResponseHeaders");
        assert_eq!(raw.response_headers, ApiResponseHeadersKind::RawFrame);
        assert!(RAW_FRAME_HEADERS
            .iter()
            .all(|header| header.name.starts_with("X-Frame-")));
        assert_eq!(
            RAW_FRAME_HEADERS
                .iter()
                .map(|header| header.field)
                .collect::<HashSet<_>>()
                .len(),
            RAW_FRAME_HEADERS.len()
        );
        assert_eq!(
            RAW_FRAME_HEADERS
                .iter()
                .map(|header| header.name)
                .collect::<HashSet<_>>()
                .len(),
            RAW_FRAME_HEADERS.len()
        );
        assert_eq!(
            FRAME_QUERY_PARAMETERS
                .iter()
                .map(|parameter| parameter.client_key)
                .collect::<HashSet<_>>()
                .len(),
            FRAME_QUERY_PARAMETERS.len()
        );
        assert_eq!(
            FRAME_QUERY_PARAMETERS
                .iter()
                .map(|parameter| parameter.wire_name)
                .collect::<HashSet<_>>()
                .len(),
            FRAME_QUERY_PARAMETERS.len()
        );
    }

    #[test]
    fn typed_endpoint_specs_bind_handler_facing_contract_types() {
        fn assert_file_info<Spec>()
        where
            Spec: ApiEndpointSpec<
                Query = NoQuery,
                Request = NoRequest,
                Response = FrameInfo,
                ResponseHeaders = NoResponseHeaders,
                Error = ErrorResponse,
            >,
        {
        }

        fn assert_file_frame<Spec>()
        where
            Spec: ApiEndpointSpec<
                Query = FrameQuery,
                Request = NoRequest,
                Response = BlobBody,
                ResponseHeaders = CacheResponseHeaders,
                Error = ErrorResponse,
            >,
        {
        }

        fn assert_raw_headers<Spec>()
        where
            Spec: ApiEndpointSpec<ResponseHeaders = RawFrameResponseHeaders, Error = ErrorResponse>,
        {
        }

        assert_file_info::<FileInfoEndpoint>();
        assert_file_frame::<FileFrame>();
        assert_raw_headers::<super::FileRawFrame>();
    }

    #[test]
    fn specialized_info_endpoint_uses_the_canonical_transfer_syntax_field() {
        let value = serde_json::to_value(FrameInfo {
            frame_count: 1,
            rows: 2,
            columns: 3,
            transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
            has_pixels: true,
            sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
            object_kind: "classic_image".to_string(),
            support_state: super::SupportState::Renderable,
            support_reason: None,
            default_window: None,
        })
        .expect("serialize frame info");

        assert_eq!(value["transfer_syntax_uid"], json!("1.2.840.10008.1.2.1"));
        assert!(value.get("transfer_syntax").is_none());
    }

    #[test]
    fn raw_metadata_uses_frontend_camel_case_names_when_serialized() {
        let value = serde_json::to_value(RawFrameMetadata {
            rows: 2,
            columns: 3,
            bits_allocated: 16,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            default_wc: None,
            default_ww: None,
            padding_low: None,
            padding_high: None,
        })
        .expect("serialize raw metadata");

        assert_eq!(value["bitsAllocated"], json!(16));
        assert_eq!(value["samplesPerPixel"], json!(1));
        assert!(value.get("bits_allocated").is_none());
    }

    #[test]
    fn empty_annotation_payload_is_canonical() {
        let empty = EmbedRoiAnnotations::empty();
        assert_eq!(empty.num_roi, 0);
        assert!(empty.roi_coords.is_empty());
        assert!(empty.roi_frames.is_empty());
    }
}

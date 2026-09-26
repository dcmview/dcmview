//! Canonical HTTP contract: endpoint facts and every serialized wire type.
//!
//! The router, handlers, and `tests/integration/api_contract.rs` read the
//! endpoint table below. `frontend/src/generated/api-types.ts` is generated
//! from this module by `cargo run --example generate_api_types`; wire types
//! derive `ts_rs::TS` so their TypeScript follows their serde attributes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const API_PREFIX: &str = "/api";

pub const JSON_MEDIA_TYPE: &str = "application/json";
pub const PNG_MEDIA_TYPE: &str = "image/png";
pub const OCTET_STREAM_MEDIA_TYPE: &str = "application/octet-stream";
pub const CSV_MEDIA_TYPE: &str = "text/csv; charset=utf-8";

pub const CACHE_HEADER: &str = "X-Cache";
pub const CACHE_HIT: &str = "HIT";
pub const CACHE_MISS: &str = "MISS";
pub const EXPORT_CONTENT_DISPOSITION_HEADER: &str = "Content-Disposition";
pub const EXPORT_CONTENT_DISPOSITION_VALUE: &str =
    "attachment; filename=\"dcmview-annotations.csv\"";

pub const RAW_FRAME_HEADER_ROWS: &str = "X-Frame-Rows";
pub const RAW_FRAME_HEADER_COLUMNS: &str = "X-Frame-Columns";
pub const RAW_FRAME_HEADER_BITS_ALLOCATED: &str = "X-Frame-Bits-Allocated";
pub const RAW_FRAME_HEADER_PIXEL_REPRESENTATION: &str = "X-Frame-Pixel-Representation";
pub const RAW_FRAME_HEADER_SAMPLES_PER_PIXEL: &str = "X-Frame-Samples-Per-Pixel";
pub const RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION: &str = "X-Frame-Photometric-Interpretation";
pub const RAW_FRAME_HEADER_RESCALE_SLOPE: &str = "X-Frame-Rescale-Slope";
pub const RAW_FRAME_HEADER_RESCALE_INTERCEPT: &str = "X-Frame-Rescale-Intercept";
pub const RAW_FRAME_HEADER_DEFAULT_WC: &str = "X-Frame-Default-Wc";
pub const RAW_FRAME_HEADER_DEFAULT_WW: &str = "X-Frame-Default-Ww";
pub const RAW_FRAME_HEADER_PADDING_LOW: &str = "X-Frame-Padding-Low";
pub const RAW_FRAME_HEADER_PADDING_HIGH: &str = "X-Frame-Padding-High";

/// Raw-frame response header carrying each serialized [`RawFrameMetadata`]
/// field, keyed by that field's JSON name. The two padding headers are sent
/// only when the file declares Pixel Padding, and the default window pair only
/// when a default window exists.
pub const RAW_FRAME_HEADERS: &[(&str, &str)] = &[
    ("rows", RAW_FRAME_HEADER_ROWS),
    ("columns", RAW_FRAME_HEADER_COLUMNS),
    ("bitsAllocated", RAW_FRAME_HEADER_BITS_ALLOCATED),
    ("pixelRepresentation", RAW_FRAME_HEADER_PIXEL_REPRESENTATION),
    ("samplesPerPixel", RAW_FRAME_HEADER_SAMPLES_PER_PIXEL),
    (
        "photometricInterpretation",
        RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION,
    ),
    ("rescaleSlope", RAW_FRAME_HEADER_RESCALE_SLOPE),
    ("rescaleIntercept", RAW_FRAME_HEADER_RESCALE_INTERCEPT),
    ("defaultWc", RAW_FRAME_HEADER_DEFAULT_WC),
    ("defaultWw", RAW_FRAME_HEADER_DEFAULT_WW),
    ("paddingLow", RAW_FRAME_HEADER_PADDING_LOW),
    ("paddingHigh", RAW_FRAME_HEADER_PADDING_HIGH),
];

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

/// Contract-specific response headers an endpoint sends on success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseHeaders {
    None,
    /// [`CACHE_HEADER`] with [`CACHE_HIT`] or [`CACHE_MISS`].
    Cache,
    /// [`CACHE_HEADER`] plus [`RAW_FRAME_HEADERS`].
    RawFrame,
    /// [`EXPORT_CONTENT_DISPOSITION_HEADER`].
    Export,
}

/// One HTTP endpoint. Every endpoint answers errors with [`ErrorResponse`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    /// Stable camelCase key of this endpoint in the generated TypeScript table.
    pub id: &'static str,
    pub method: ApiMethod,
    /// Axum route relative to [`API_PREFIX`]; `{name}` segments are path parameters.
    pub path: &'static str,
    pub response_media_type: &'static str,
    pub response_headers: ResponseHeaders,
    pub success_status: u16,
}

pub mod endpoints {
    use super::{
        ApiMethod, Endpoint, ResponseHeaders, CSV_MEDIA_TYPE, JSON_MEDIA_TYPE,
        OCTET_STREAM_MEDIA_TYPE, PNG_MEDIA_TYPE,
    };

    const fn json(id: &'static str, method: ApiMethod, path: &'static str) -> Endpoint {
        Endpoint {
            id,
            method,
            path,
            response_media_type: JSON_MEDIA_TYPE,
            response_headers: ResponseHeaders::None,
            success_status: 200,
        }
    }

    const fn binary(
        id: &'static str,
        path: &'static str,
        response_media_type: &'static str,
        response_headers: ResponseHeaders,
    ) -> Endpoint {
        Endpoint {
            id,
            method: ApiMethod::Get,
            path,
            response_media_type,
            response_headers,
            success_status: 200,
        }
    }

    /// `HealthResponse`.
    pub const HEALTH: Endpoint = json("health", ApiMethod::Get, "/health");
    /// `FilesResponse`.
    pub const FILES: Endpoint = json("files", ApiMethod::Get, "/files");
    /// `SeriesCatalogResponse`.
    pub const SERIES: Endpoint = json("series", ApiMethod::Get, "/series");
    /// `FrameInfo`.
    pub const FILE_INFO: Endpoint = json("fileInfo", ApiMethod::Get, "/file/{index}/info");
    /// `ReferenceCatalogResponse`.
    pub const FILE_REFERENCES: Endpoint =
        json("fileReferences", ApiMethod::Get, "/file/{index}/references");
    /// `SemanticContextResponse`.
    pub const FILE_SEMANTIC_CONTEXT: Endpoint = json(
        "fileSemanticContext",
        ApiMethod::Get,
        "/file/{index}/semantic-context",
    );
    /// Transparent PNG overlay of a segmentation frame on its source frame.
    pub const FILE_SEGMENTATION_OVERLAY: Endpoint = binary(
        "fileSegmentationOverlay",
        "/file/{index}/frame/{frame}/segmentation-overlay",
        PNG_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// `FrameValueMapping`.
    pub const FILE_VALUE_MAPPING: Endpoint = json(
        "fileValueMapping",
        ApiMethod::Get,
        "/file/{index}/frame/{frame}/value-mapping",
    );
    /// `WsiFrameContextResponse`.
    pub const FILE_WSI_CONTEXT: Endpoint = json(
        "fileWsiContext",
        ApiMethod::Get,
        "/file/{index}/frame/{frame}/wsi-context",
    );
    /// Windowed display PNG; query `FrameQuery`.
    pub const FILE_FRAME: Endpoint = binary(
        "fileFrame",
        "/file/{index}/frame/{frame}",
        PNG_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// Decoded little-endian samples with `RawFrameMetadata` in headers.
    pub const FILE_RAW_FRAME: Endpoint = binary(
        "fileRawFrame",
        "/file/{index}/frame/{frame}/raw",
        OCTET_STREAM_MEDIA_TYPE,
        ResponseHeaders::RawFrame,
    );
    /// `TagNode[]`.
    pub const FILE_TAGS: Endpoint = json("fileTags", ApiMethod::Get, "/file/{index}/tags");
    /// One `TagNode`; query `TagQuery`.
    pub const FILE_TAG_SELECT: Endpoint =
        json("fileTagSelect", ApiMethod::Get, "/file/{index}/tags/select");
    /// `EmbedRoiAnnotations`.
    pub const FILE_ANNOTATIONS_GET: Endpoint = json(
        "fileAnnotationsGet",
        ApiMethod::Get,
        "/file/{index}/annotations",
    );
    /// JSON `EmbedRoiAnnotations` in and out.
    pub const FILE_ANNOTATIONS_UPDATE: Endpoint = json(
        "fileAnnotationsUpdate",
        ApiMethod::Put,
        "/file/{index}/annotations",
    );
    /// EMBED-style CSV of every in-memory annotation.
    pub const ANNOTATIONS_EXPORT: Endpoint = binary(
        "annotationsExport",
        "/annotations/export.csv",
        CSV_MEDIA_TYPE,
        ResponseHeaders::Export,
    );

    pub const ALL: &[Endpoint] = &[
        HEALTH,
        FILES,
        SERIES,
        FILE_INFO,
        FILE_REFERENCES,
        FILE_SEMANTIC_CONTEXT,
        FILE_SEGMENTATION_OVERLAY,
        FILE_VALUE_MAPPING,
        FILE_WSI_CONTEXT,
        FILE_FRAME,
        FILE_RAW_FRAME,
        FILE_TAGS,
        FILE_TAG_SELECT,
        FILE_ANNOTATIONS_GET,
        FILE_ANNOTATIONS_UPDATE,
        ANNOTATIONS_EXPORT,
    ];
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
pub struct WindowPreset {
    pub center: f64,
    pub width: f64,
}

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
pub struct FilesResponse {
    pub files: Vec<FileSummary>,
    pub discovery: Vec<DiscoveryResult>,
    pub server_start_ms: u64,
    pub scan_complete: bool,
    pub scanned: usize,
    pub skipped: usize,
    pub filtered: usize,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SeriesCatalogResponse {
    pub series: Vec<SeriesSummary>,
    pub scan_complete: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReferenceCatalogResponse {
    pub source_file_index: usize,
    pub source_sop_instance_uid: String,
    pub references: Vec<ReferenceSummary>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReferenceSummary {
    pub relationship: String,
    pub target: ReferenceTargetSummary,
    pub matches: Vec<ReferenceMatchSummary>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReferenceTargetSummary {
    pub sop_class_uid: Option<String>,
    pub sop_instance_uid: Option<String>,
    pub series_instance_uid: Option<String>,
    /// DICOM-declared, one-based frame numbers.
    pub frame_numbers: Vec<u32>,
    pub segment_numbers: Vec<u16>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ReferenceMatchSummary {
    pub file_index: usize,
    pub path: String,
    pub sop_instance_uid: String,
    /// Validated, zero-based frame indices suitable for viewer navigation.
    pub frame_indices: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SemanticContextResponse {
    pub source_file_index: usize,
    /// The normal frame endpoints remain the default and are never semantically transformed.
    pub default_mode: String,
    pub pixel_preview_preserves_stored_values: bool,
    pub context: SemanticContext,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticContext {
    Segmentation(SegmentationContext),
    ParametricMap(ParametricMapContext),
    RtDose(Box<RtDoseContext>),
    NotApplicable { reason: String },
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct CodedConceptSummary {
    pub value: String,
    pub scheme: String,
    pub meaning: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SegmentationContext {
    pub segmentation_type: Option<String>,
    pub segmentation_fractional_type: Option<String>,
    pub maximum_fractional_value: Option<u32>,
    pub segments: Vec<SegmentSummary>,
    pub frame_mappings: Vec<SegmentFrameMapping>,
    pub references: Vec<ReferenceSummary>,
    pub overlay: OverlayEligibility,
}

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
pub struct ResolvedSegmentSourceFrame {
    pub file_index: usize,
    /// Zero-based frame index in the source object.
    pub frame_index: u32,
    pub sop_instance_uid: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct OverlayEligibility {
    pub eligible: bool,
    pub reason: String,
    pub source_file_index: Option<usize>,
    pub mapped_source_count: usize,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ParametricMapContext {
    pub stored_value_type: String,
    /// What the display and raw frames carry: `stored` values (after any
    /// Modality rescale). Mapped units are converted client-side.
    pub displayed_value_kind: String,
    pub mappings: Vec<RealWorldValueMappingSummary>,
    pub mapping_status: String,
    pub source_references: Vec<ReferenceSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
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

/// How one frame's stored samples, as served by the raw-frame endpoint,
/// convert to modality and real-world values.
#[derive(Debug, Clone, Serialize, TS)]
pub struct FrameValueMapping {
    pub file_index: usize,
    /// Zero-based frame index.
    pub frame_index: u32,
    /// `integer`, `float32`, or `float64` stored samples.
    pub stored_value_type: String,
    pub modality: ModalityValueTransform,
    /// Conversions of stored values into real-world units that apply to this
    /// frame, the preferred one first. Empty when none is declared.
    pub real_world: Vec<RealWorldValueMap>,
}

/// The Modality transform the display pipeline applies to stored values
/// before windowing: the LUT when present, otherwise the rescale.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ModalityValueTransform {
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    pub rescale_type: Option<String>,
    pub lut: Option<ValueLookupTable>,
}

/// `value = values[clamp(stored - first_value_mapped, 0, values.length - 1)]`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct ValueLookupTable {
    pub first_value_mapped: f64,
    pub values: Vec<f64>,
}

/// One validated stored-to-real-world conversion. Stored values outside
/// `first_value_mapped..=last_value_mapped` have no mapped value.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RealWorldValueMap {
    /// `real_world_value_mapping` (a declared RWVM item) or
    /// `dose_grid_scaling` (RT Dose: `mapped = stored * DoseGridScaling`).
    pub source: String,
    pub label: Option<String>,
    /// Inclusive stored-value range; an absent bound is unbounded.
    pub first_value_mapped: Option<f64>,
    pub last_value_mapped: Option<f64>,
    pub transform: RealWorldValueTransform,
    /// Short unit text for readouts and legends, such as `Gy` or a UCUM code.
    pub unit_label: String,
    pub units: Option<CodedConceptSummary>,
    pub quantity: Option<CodedConceptSummary>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RealWorldValueTransform {
    /// `mapped = stored * slope + intercept`.
    Linear { slope: f64, intercept: f64 },
    /// `mapped = values[stored - first_value_mapped]`.
    Lut { values: Vec<f64> },
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct RtDoseContext {
    pub dose_grid_scaling: Option<f64>,
    pub scaling_status: String,
    /// What the display and raw frames carry: `stored` values (after any
    /// Modality rescale). Mapped units are converted client-side.
    pub displayed_value_kind: String,
    pub dose_units: Option<String>,
    pub dose_type: Option<String>,
    pub dose_summation_type: Option<String>,
    pub geometry: DoseGridGeometry,
    pub references: Vec<ReferenceSummary>,
    pub overlay: OverlayEligibility,
    pub clinical_use_warning: String,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct DoseGridGeometry {
    pub frame_of_reference_uid: Option<String>,
    pub image_position_patient: Option<[f64; 3]>,
    pub image_orientation_patient: Option<[f64; 6]>,
    pub pixel_spacing: Option<[f64; 2]>,
    pub grid_frame_offsets: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
pub struct WsiTotalPixelMatrix {
    pub rows: u64,
    pub columns: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WsiTileRectangle {
    /// Zero-based column offset in the Total Pixel Matrix.
    pub x: u64,
    /// Zero-based row offset in the Total Pixel Matrix.
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WsiOpticalPath {
    pub index: Option<u32>,
    pub identifier: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WsiFocalPlane {
    pub index: Option<u32>,
    pub z_offset_slide: Option<f64>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct WsiCompanionSummary {
    pub file_index: usize,
    pub sop_instance_uid: String,
    pub image_type_role: Option<String>,
    pub pyramid_uid: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SeriesSummary {
    pub id: String,
    pub study_instance_uid: String,
    pub series_instance_uid: String,
    pub frame_of_reference_uids: Vec<String>,
    pub stacks: Vec<SeriesStackSummary>,
}

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
pub struct FrameRefSummary {
    pub virtual_index: usize,
    pub file_index: usize,
    pub frame_index: u32,
    pub source_path: String,
    pub sop_instance_uid: String,
    pub instance_number: Option<i32>,
    pub position_along_normal_mm: Option<f64>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct SeriesWarningSummary {
    pub code: String,
    pub message: String,
    pub file_indices: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SupportState {
    Renderable,
    MetadataOnly,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct DiscoveryResult {
    pub path: String,
    pub disposition: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Default, TS)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    #[default]
    Default,
    FullDynamic,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct TagNode {
    pub tag: String,
    pub vr: String,
    pub keyword: String,
    pub value: TagValue,
}

#[derive(Debug, Clone, Serialize, TS)]
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
        #[serde(default, skip_serializing_if = "is_false")]
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        total: Option<usize>,
    },
    Binary {
        length: usize,
    },
    Sequence {
        items: Vec<Vec<TagNode>>,
        #[serde(default, skip_serializing_if = "is_false")]
        truncated: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        total: Option<usize>,
    },
    Error {
        message: String,
    },
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
pub struct ErrorResponse {
    pub code: ApiErrorCode,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
pub struct HealthResponse {
    pub status: &'static str,
    pub viewer: ViewerIdentity,
    pub file_count: usize,
    pub server_start_ms: u64,
}

/// Display-frame query. Explicit `wc`/`ww` must be sent together;
/// `mode=full_dynamic` ignores them.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[ts(optional_fields)]
pub struct FrameQuery {
    pub wc: Option<f64>,
    pub ww: Option<f64>,
    pub mode: Option<WindowMode>,
}

/// Selective tag query: `path` addresses one element, and `offset`/`limit`
/// page the items of a sequence at that path.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(optional_fields)]
pub struct TagQuery {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, TS)]
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
    use super::{endpoints, FrameInfo, RawFrameMetadata, RAW_FRAME_HEADERS};
    use serde_json::json;
    use std::collections::HashSet;

    #[test]
    fn endpoint_table_is_unique_and_well_formed() {
        let all = endpoints::ALL;
        let ids = all
            .iter()
            .map(|endpoint| endpoint.id)
            .collect::<HashSet<_>>();
        let routes = all
            .iter()
            .map(|endpoint| (endpoint.method, endpoint.path))
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), all.len());
        assert_eq!(routes.len(), all.len());
        for endpoint in all {
            assert!(endpoint.path.starts_with('/'), "{}", endpoint.id);
            assert!(
                (200..300).contains(&endpoint.success_status),
                "{}",
                endpoint.id
            );
        }
    }

    #[test]
    fn raw_frame_headers_cover_exactly_the_serialized_metadata_fields() {
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
        let serialized = value
            .as_object()
            .expect("metadata object")
            .keys()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let fields = RAW_FRAME_HEADERS
            .iter()
            .map(|(field, _)| *field)
            .collect::<HashSet<_>>();
        let names = RAW_FRAME_HEADERS
            .iter()
            .map(|(_, name)| *name)
            .collect::<HashSet<_>>();

        assert_eq!(serialized, fields);
        assert_eq!(names.len(), RAW_FRAME_HEADERS.len());
        assert!(names.iter().all(|name| name.starts_with("X-Frame-")));
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
}

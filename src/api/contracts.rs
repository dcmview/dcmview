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
pub const JPEG_MEDIA_TYPE: &str = "image/jpeg";
pub const OCTET_STREAM_MEDIA_TYPE: &str = "application/octet-stream";
pub const CSV_MEDIA_TYPE: &str = "text/csv; charset=utf-8";

/// Unix-millisecond server start identity on every API response, including errors.
pub const SERVER_INSTANCE_HEADER: &str = "X-Server-Instance";
pub const API_RESPONSE_HEADERS: &[(&str, &str)] = &[("serverInstance", SERVER_INSTANCE_HEADER)];

/// Scheme of the `Authorization` header every `/api` request carries unless
/// the process runs with `--no-token`: `Authorization: Bearer <token>`.
pub const AUTHORIZATION_SCHEME: &str = "Bearer";
/// Value of `WWW-Authenticate` on a 401.
pub const UNAUTHORIZED_CHALLENGE: &str = "Bearer";
// The launch and startup contract lives in the `dcmview-protocol` crate so
// other crates can use it without the viewer. Re-exported here so existing
// `crate::api::contracts` paths keep working.
pub use dcmview_protocol::{
    launch_url, StartupEvent, STARTUP_PROTOCOL, TOKEN_ENV_VAR, TOKEN_FRAGMENT_PARAM,
};

/// Request header (`X-Dcmview-Background: 1`) marking a request the page
/// makes on its own, such as polling or prefetch. Such a request is served
/// normally but does not count as activity for `--timeout`, so an open tab
/// alone does not keep an idle process alive. Sent only where a supervising
/// parent asks for it; a standalone viewer's own polling does not send it.
pub const BACKGROUND_REQUEST_HEADER: &str = "X-Dcmview-Background";

/// The file's key (`FileSummary::file_key`, written in full) on a display
/// frame and a raw frame response. Sent whenever the file has a
/// key when the response is built and absent while it has none, so a viewer
/// showing a file whose key was pending learns the key from the next frame
/// it loads, without waiting for a catalog poll. A masked session sends the
/// key as its catalog shows it.
pub const FILE_KEY_HEADER: &str = "X-File-Key";

/// Seconds a client is told to wait, in the standard `Retry-After` header,
/// before repeating a request answered `503 decode_busy`. A fixed number: a
/// decode has no deadline to derive a better one from.
pub const DECODE_BUSY_RETRY_AFTER_SECONDS: u32 = 1;

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

pub const DISPLAY_FRAME_HEADER_WINDOW_CENTER: &str = "X-Frame-Window-Center";
pub const DISPLAY_FRAME_HEADER_WINDOW_WIDTH: &str = "X-Frame-Window-Width";
pub const DISPLAY_FRAME_HEADER_WINDOW_APPLIED: &str = "X-Frame-Window-Applied";

/// How a grayscale display PNG was presented, reported in its applied-window header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FrameWindowApplied {
    Linear,
    RealWorld,
    VoiLut,
}

impl FrameWindowApplied {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::RealWorld => "real_world",
            Self::VoiLut => "voi_lut",
        }
    }
}

/// Display-frame response headers, keyed by their name in the generated
/// TypeScript table: the linear window the PNG was presented with, as center
/// and width in Modality values. Both are sent for grayscale frames windowed
/// linearly (requested, DICOM, or automatic) and neither for color frames or
/// frames presented through a VOI LUT. The applied kind is sent for every
/// grayscale frame, and omitted for color frames.
pub const DISPLAY_FRAME_HEADERS: &[(&str, &str)] = &[
    ("windowCenter", DISPLAY_FRAME_HEADER_WINDOW_CENTER),
    ("windowWidth", DISPLAY_FRAME_HEADER_WINDOW_WIDTH),
    ("windowApplied", DISPLAY_FRAME_HEADER_WINDOW_APPLIED),
];

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

/// Thumbnail response header naming the step that produced the image, as a
/// [`ThumbnailSource`] value. Diagnostic only: every source yields the same
/// presentation.
pub const THUMBNAIL_HEADER_SOURCE: &str = "X-Thumbnail-Source";
/// `Cache-Control` of a thumbnail. File indexes are valid within one server
/// process only, so a browser must not keep a thumbnail across a restart.
pub const THUMBNAIL_CACHE_CONTROL: &str = "no-store";

/// Thumbnail response headers, keyed by their name in the generated
/// TypeScript table. Both are sent on every thumbnail, with [`CACHE_HEADER`].
pub const THUMBNAIL_HEADERS: &[(&str, &str)] = &[
    ("source", THUMBNAIL_HEADER_SOURCE),
    ("cacheControl", "Cache-Control"),
];

/// The longest-edge sizes a thumbnail is rendered at, ascending. A requested
/// `size` is snapped up to the next bucket so cache keys stay few; the client
/// scales down.
pub const THUMBNAIL_SIZE_BUCKETS: [u32; 4] = [128, 256, 512, 1024];
/// The bucket of a request that names no `size`.
pub const THUMBNAIL_DEFAULT_SIZE: u32 = 256;

/// The step that produced a thumbnail, reported in
/// [`THUMBNAIL_HEADER_SOURCE`]. `ThumbnailCache` and `FullDecode` are sent
/// today; the others are reserved for the cheaper sources that follow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailSource {
    ThumbnailCache,
    DisplayCache,
    RawCache,
    ReducedDecode,
    FullDecode,
}

impl ThumbnailSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThumbnailCache => "thumbnail_cache",
            Self::DisplayCache => "display_cache",
            Self::RawCache => "raw_cache",
            Self::ReducedDecode => "reduced_decode",
            Self::FullDecode => "full_decode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiMethod {
    Get,
    Put,
    Post,
}

impl ApiMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
        }
    }
}

/// Endpoint-specific headers on success, in addition to [`API_RESPONSE_HEADERS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseHeaders {
    None,
    /// [`CACHE_HEADER`] with [`CACHE_HIT`] or [`CACHE_MISS`].
    Cache,
    /// [`CACHE_HEADER`] plus, when the frame has a linear window,
    /// [`DISPLAY_FRAME_HEADERS`].
    DisplayFrame,
    /// [`CACHE_HEADER`] plus [`RAW_FRAME_HEADERS`].
    RawFrame,
    /// [`CACHE_HEADER`] plus [`THUMBNAIL_HEADERS`].
    Thumbnail,
    /// [`EXPORT_CONTENT_DISPOSITION_HEADER`].
    Export,
}

/// One HTTP endpoint. Every response carries [`API_RESPONSE_HEADERS`], and
/// every endpoint answers errors with [`ErrorResponse`].
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
        ApiMethod, Endpoint, ResponseHeaders, CSV_MEDIA_TYPE, JPEG_MEDIA_TYPE, JSON_MEDIA_TYPE,
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
    /// RGBA PNG of the display shutter fill and overlay graphics a grayscale
    /// display frame carries, opaque where drawn and transparent elsewhere;
    /// fully transparent when the file declares neither.
    pub const FILE_PRESENTATION_LAYER: Endpoint = binary(
        "filePresentationLayer",
        "/file/{index}/frame/{frame}/presentation-layer",
        PNG_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// PNG colorwash of an RT Dose grid resampled onto the path's frame;
    /// query `DoseOverlayQuery`.
    pub const FILE_DOSE_OVERLAY: Endpoint = binary(
        "fileDoseOverlay",
        "/file/{index}/frame/{frame}/dose-overlay",
        PNG_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// The dose-overlay's values: one little-endian `f32` per pixel of the
    /// path's frame, row-major, in the dose's unit (NaN outside the grid);
    /// query `DoseOverlayQuery`.
    pub const FILE_DOSE_OVERLAY_VALUES: Endpoint = binary(
        "fileDoseOverlayValues",
        "/file/{index}/frame/{frame}/dose-overlay/values",
        OCTET_STREAM_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// PNG colorwash of a Parametric Map's mapped values resampled onto the
    /// path's frame; query `ParametricMapOverlayQuery`.
    pub const FILE_PARAMETRIC_MAP_OVERLAY: Endpoint = binary(
        "fileParametricMapOverlay",
        "/file/{index}/frame/{frame}/parametric-map-overlay",
        PNG_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// The parametric-map-overlay's mapped values, laid out like
    /// [`FILE_DOSE_OVERLAY_VALUES`]; query `ParametricMapOverlayQuery`.
    pub const FILE_PARAMETRIC_MAP_OVERLAY_VALUES: Endpoint = binary(
        "fileParametricMapOverlayValues",
        "/file/{index}/frame/{frame}/parametric-map-overlay/values",
        OCTET_STREAM_MEDIA_TYPE,
        ResponseHeaders::Cache,
    );
    /// `GraphicAnnotationsResponse`: the graphic and text objects a softcopy
    /// presentation state draws on the path's frame; query
    /// `GraphicAnnotationsQuery`.
    pub const FILE_GRAPHIC_ANNOTATIONS: Endpoint = json(
        "fileGraphicAnnotations",
        ApiMethod::Get,
        "/file/{index}/frame/{frame}/graphic-annotations",
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
        ResponseHeaders::DisplayFrame,
    );
    /// Decoded little-endian samples with `RawFrameMetadata` in headers.
    pub const FILE_RAW_FRAME: Endpoint = binary(
        "fileRawFrame",
        "/file/{index}/frame/{frame}/raw",
        OCTET_STREAM_MEDIA_TYPE,
        ResponseHeaders::RawFrame,
    );
    /// One pixel of the raw frame as a 1x1 raw frame: its stored samples in
    /// color-by-pixel order, with `RawFrameMetadata` in headers; query
    /// `PixelQuery`. The readout uses it where a whole frame is too large to
    /// fetch for one value.
    pub const FILE_RAW_PIXEL: Endpoint = binary(
        "fileRawPixel",
        "/file/{index}/frame/{frame}/raw/pixel",
        OCTET_STREAM_MEDIA_TYPE,
        ResponseHeaders::RawFrame,
    );
    /// Small lossy JPEG preview of the frame for the gallery; query
    /// `ThumbnailQuery`. The whole frame in the stored pixel grid, resampled
    /// to its physical aspect and fitted inside the size bucket, never
    /// cropped, rotated, flipped or enlarged. It has the frame's default
    /// presentation without the display shutter and overlay graphics, and
    /// the frame's redaction boxes painted black. A masked session withholds
    /// it for the files whose display frames it withholds.
    pub const FILE_THUMBNAIL: Endpoint = binary(
        "fileThumbnail",
        "/file/{index}/frame/{frame}/thumbnail",
        JPEG_MEDIA_TYPE,
        ResponseHeaders::Thumbnail,
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
    /// The file's redaction boxes, as `EmbedRoiAnnotations`.
    pub const FILE_REDACTIONS_GET: Endpoint = json(
        "fileRedactionsGet",
        ApiMethod::Get,
        "/file/{index}/redactions",
    );
    /// JSON `EmbedRoiAnnotations` in and out: replaces the file's redaction
    /// boxes, which both frame endpoints then apply.
    pub const FILE_REDACTIONS_UPDATE: Endpoint = json(
        "fileRedactionsUpdate",
        ApiMethod::Put,
        "/file/{index}/redactions",
    );
    /// `RedactionSeriesResponse`: copies the file's redaction boxes to every
    /// file of its series with the same rows and columns. Takes no body.
    pub const FILE_REDACTIONS_APPLY_TO_SERIES: Endpoint = json(
        "fileRedactionsApplyToSeries",
        ApiMethod::Put,
        "/file/{index}/redactions/series",
    );
    /// EMBED-style CSV of every in-memory annotation.
    pub const ANNOTATIONS_EXPORT: Endpoint = binary(
        "annotationsExport",
        "/annotations/export.csv",
        CSV_MEDIA_TYPE,
        ResponseHeaders::Export,
    );
    /// One annotation operation; query `AnnotationOpQuery`. The body is one
    /// `OpEnvelope` of the annotation model as JSON, at most [`super::ANNOTATION_OP_MAX_BYTES`]
    /// long, and the answer is an `AnnotationOpResponse`. See that type for
    /// the statuses.
    pub const ANNOTATION_OPS: Endpoint = json("annotationOps", ApiMethod::Post, "/annotations/ops");

    /// The endpoints that decode or render pixels under the decode memory
    /// budget (`pixels::DecodeScheduler`). Each may answer
    /// `503 decode_busy` with `Retry-After` when the budget is in use and
    /// its queue is full. Each but the semantic context, which reports a
    /// frame it cannot decode in the overlay's eligibility, may answer
    /// `422 decode_memory_exceeded` for a frame whose decode needs more
    /// than the budget. No other endpoint answers either.
    pub const DECODING: &[Endpoint] = &[
        FILE_SEMANTIC_CONTEXT,
        FILE_FRAME,
        FILE_RAW_FRAME,
        FILE_RAW_PIXEL,
        FILE_THUMBNAIL,
        FILE_PRESENTATION_LAYER,
        FILE_SEGMENTATION_OVERLAY,
        FILE_DOSE_OVERLAY,
        FILE_DOSE_OVERLAY_VALUES,
        FILE_PARAMETRIC_MAP_OVERLAY,
        FILE_PARAMETRIC_MAP_OVERLAY_VALUES,
    ];

    pub const ALL: &[Endpoint] = &[
        HEALTH,
        FILES,
        SERIES,
        FILE_INFO,
        FILE_REFERENCES,
        FILE_SEMANTIC_CONTEXT,
        FILE_SEGMENTATION_OVERLAY,
        FILE_PRESENTATION_LAYER,
        FILE_DOSE_OVERLAY,
        FILE_DOSE_OVERLAY_VALUES,
        FILE_PARAMETRIC_MAP_OVERLAY,
        FILE_PARAMETRIC_MAP_OVERLAY_VALUES,
        FILE_GRAPHIC_ANNOTATIONS,
        FILE_VALUE_MAPPING,
        FILE_WSI_CONTEXT,
        FILE_FRAME,
        FILE_RAW_FRAME,
        FILE_RAW_PIXEL,
        FILE_THUMBNAIL,
        FILE_TAGS,
        FILE_TAG_SELECT,
        FILE_ANNOTATIONS_GET,
        FILE_ANNOTATIONS_UPDATE,
        FILE_REDACTIONS_GET,
        FILE_REDACTIONS_UPDATE,
        FILE_REDACTIONS_APPLY_TO_SERIES,
        ANNOTATIONS_EXPORT,
        ANNOTATION_OPS,
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
    /// The name the viewer shows for the file outside the directory tree: its
    /// file name, or a synthetic `File N` in a masked session.
    pub display_name: String,
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
    /// The object family: a DICOM family from the SOP Class, or `image` for a
    /// raster file.
    pub object_kind: String,
    /// What the file is, detected from its content and never from its name.
    /// A raster (anything but `dicom`) has empty DICOM identity strings and
    /// an empty `transfer_syntax_uid`.
    pub file_format: FileFormat,
    /// What discovery read from a raster file's header; `null` for DICOM.
    pub raster: Option<RasterSummary>,
    pub support_state: SupportState,
    pub support_reason: Option<String>,
    /// Whether client-side raw windowing preserves every declared presentation
    /// transform. Always `true` now that the value mapping carries the
    /// Modality and VOI LUTs and `presentation-layer` the shutter and overlays.
    pub raw_windowing_compatible: bool,
    /// Explanation when the frontend must retain the server-rendered
    /// presentation path; always `null` (see `raw_windowing_compatible`).
    pub raw_windowing_reason: Option<String>,
    /// Whether grayscale display frames carry a display shutter or overlay
    /// graphics, which `presentation-layer` draws for a raw-rendered frame.
    pub presentation_layer: bool,
    /// Whether the file declares burned-in annotation, which display masking
    /// cannot hide.
    pub burned_in_annotation: bool,
    pub has_pixels: bool,
    pub frame_count: u32,
    pub rows: u32,
    pub columns: u32,
    /// Effective physical row-to-column pixel extent ratio.
    pub pixel_aspect_ratio: Option<f64>,
    pub transfer_syntax_uid: String,
    pub default_window: Option<WindowPreset>,
    /// The file's stable key (`docs/design/annotation-model.md` 1.3):
    /// `sop:<SOP Instance UID>` or `b3:<64 lowercase hex>`, the BLAKE3 digest
    /// of the file's bytes.
    ///
    /// - Left out when the key is `sop:` followed by this entry's
    ///   `sop_instance_uid`, which is the case for a DICOM file whose UID no
    ///   other loaded file is known to contradict. A reader rebuilds it.
    /// - `null` while the file has no key: a raster, a DICOM file without a
    ///   usable UID, or one whose UID another loaded file with different
    ///   bytes also carries, until its bytes have been hashed. Hashing starts
    ///   once a frame of the file has been served. `key_error` says when it
    ///   failed.
    /// - Otherwise the key in full.
    ///
    /// A key is at most 132 bytes. This is the key as it stands, which may
    /// not be settled: a `sop:` key is replaced, once, by a `b3:` key when
    /// the file turns out to share its UID with different bytes, and
    /// `rekeys` in [`FilesResponse`] reports each replacement. A `b3:` key,
    /// and a `sop:` key the server has returned for a write or an export,
    /// never change. A file found after the key of its UID was settled has
    /// `null` here until it has been compared with the first file that
    /// carries the UID. In a masked session a `sop:` key is built from the
    /// masked UID, so the rule for leaving it out is unchanged and no real
    /// UID is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file_key: Option<Option<String>>,
    /// The `index` of the first loaded file that holds the same key, when
    /// that is another file: the two are one image under two paths and share
    /// annotations. Left out otherwise. Two DICOM files with one UID and one
    /// size are aliases without their bytes having been compared; comparing
    /// them, which happens before a key of theirs is written down, may give
    /// both a `b3:` key instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub alias_of: Option<usize>,
    /// Why the file's bytes could not be hashed for its key. Left out when
    /// hashing has not failed. A file with `file_key: null` and an error
    /// stays without a key until hashing is asked for again and succeeds.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub key_error: Option<FileKeyError>,
}

/// Why a whole-file digest could not be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileKeyError {
    /// The file could not be opened or read.
    Unreadable,
    /// The file is not the one discovery saw: its length differs, or it
    /// changed while it was being read.
    Changed,
}

/// One file's key replaced by another (`docs/design/annotation-model.md`
/// 1.7). A client applies it in one step to everything it holds under
/// `old_key` for the file `index`: records, queued operations, history and
/// selection. For the rest of the session the server accepts `old_key` as
/// naming the first file that held it, which can be another file than
/// `index`, so what is held under `old_key` for other files stays. A key
/// the server has returned for a write or an export is never replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct FileRekey {
    /// The catalog revision at which the key changed.
    pub revision: u64,
    pub index: usize,
    /// The key the file had, in full.
    pub old_key: String,
    /// The key the file has now, in full.
    pub new_key: String,
}

/// Query of `GET /api/files`.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(optional_fields)]
pub struct FilesQuery {
    /// A `revision` from an earlier response. The response then lists only
    /// the entries added or changed after it. Absent or 0 lists every entry.
    /// A value above the catalog's current revision is answered with every
    /// entry and `reset: true`.
    pub since: Option<u64>,
    /// The most entries to return, at least 1. Absent returns all of them.
    /// 0 is `400 invalid_query`.
    pub limit: Option<u32>,
}

/// The container format of a discovered file, detected from its content
/// (`docs/design/image-formats.md` section 3). The serialized names are also
/// the values of `--formats` and of the `format` scan filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    #[default]
    Dicom,
    Png,
    Jpeg,
    Tiff,
    Webp,
}

impl FileFormat {
    /// Every format, in the order lists of formats are printed.
    pub const ALL: [Self; 5] = [Self::Dicom, Self::Png, Self::Jpeg, Self::Tiff, Self::Webp];

    /// The serialized name: `dicom`, `png`, `jpeg`, `tiff` or `webp`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dicom => "dicom",
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Tiff => "tiff",
            Self::Webp => "webp",
        }
    }

    /// Whether this is a raster image format, that is, anything but DICOM.
    pub const fn is_raster(self) -> bool {
        !matches!(self, Self::Dicom)
    }
}

/// How a raster file stores colour, before any expansion on decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RasterColorType {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
    /// Indexed colour; decoded frames hold the expanded colours.
    Palette,
    /// CMYK or YCCK JPEG; decoded frames hold an approximate sRGB conversion.
    Cmyk,
    /// A stored layout none of the above describes and the viewer does not
    /// decode: CIELab, more than four bands, or an extra sample that is not
    /// alpha. The file is listed with `support_reason`
    /// `raster.unsupported_color`.
    Other,
}

/// The numeric kind of a raster file's stored samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RasterSampleFormat {
    Uint,
    Int,
    Float,
}

/// Why a TIFF page is not one of the file's frames: the first property, in
/// this order, in which it differs from page 0, or the subfile flag that
/// marks it as not a full image
/// (`docs/design/image-formats.md` section 2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RasterPageDifference {
    /// `NewSubfileType` bit 0: a reduced-resolution copy, such as a thumbnail.
    ReducedResolution,
    /// `NewSubfileType` bit 2: a transparency mask.
    Mask,
    Width,
    Height,
    SamplesPerPixel,
    SampleFormat,
    BitsPerSample,
    Photometric,
    /// The extra-sample (alpha) type.
    Alpha,
    Orientation,
}

/// A TIFF page left out of the frame map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub struct RasterExcludedPage {
    /// Zero-based IFD index in the file's page chain.
    pub page: u32,
    pub differs: RasterPageDifference,
}

/// What discovery read from a raster file's header. Every field holds for
/// every frame of the file. Nothing here is decoded from pixel data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct RasterSummary {
    pub color_type: RasterColorType,
    /// Bits per sample as stored in the file (1, 2, 4, 8, 16, 32 or 64).
    /// Raw frames serve low-bit samples one byte each, at this depth's values.
    pub bit_depth: u32,
    pub sample_format: RasterSampleFormat,
    /// Whether the file carries an alpha channel or palette transparency.
    pub has_alpha: bool,
    /// The EXIF or TIFF orientation, 1 to 8; 1 when the file declares none or
    /// an invalid one. `rows`, `columns`, raw samples and every coordinate
    /// stay in the stored pixel grid: this is a display hint only.
    pub orientation: u8,
    /// Whether the file embeds an ICC profile.
    pub has_icc: bool,
    /// Pages (IFDs) in the file: 1 for PNG, JPEG and WebP, at most 65,535
    /// for TIFF (a file with more is not listed).
    pub pages_total: u32,
    /// The zero-based page (IFD index) of each frame, in frame order. Its
    /// length is the file's `frame_count`; `[0]` for PNG, JPEG and WebP.
    pub frame_pages: Vec<u32>,
    /// The first pages that are not frames, in page order: at most 16, so a
    /// file with many excluded pages does not grow every catalog response.
    pub excluded_pages: Vec<RasterExcludedPage>,
    /// How many pages are not frames; `pages_total` is this plus the length
    /// of `frame_pages`.
    pub excluded_pages_total: u32,
    /// An animated PNG or WebP; only its first frame is a frame here.
    pub animated: bool,
    /// PNG `sBIT`: the original precision of each stored channel, in file
    /// order. Informational only: it never narrows a window or value range.
    /// `null` when the file has no `sBIT` chunk.
    pub significant_bits: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct FilesResponse {
    /// With neither `since` nor `limit`, every entry in index order.
    /// Otherwise the entries added or changed after `since` (after 0 when it
    /// is absent), least recently changed first, each as it is now and each
    /// once, and at most `limit` of them. An entry that changes while a
    /// client pages through the list is listed again on a later page, so
    /// applying every page by `index` ends with the current catalog.
    pub files: Vec<FileSummary>,
    pub discovery: Vec<DiscoveryResult>,
    pub server_start_ms: u64,
    /// Whether this session masks patient identifiers (`--mask`).
    pub masked: bool,
    pub scan_complete: bool,
    pub scanned: usize,
    pub skipped: usize,
    pub filtered: usize,
    /// The catalog revision this response is complete up to: pass it as
    /// `since` to receive what changed afterwards. It starts at 0 and rises
    /// by one for every entry added and every entry whose `file_key`,
    /// `alias_of` or `key_error` changes. When `more` is `true` it is the
    /// revision of the last entry listed, not the catalog's latest.
    pub revision: u64,
    /// `true` when `since` was above the catalog's revision, which means it
    /// came from another process: the client drops the entries it holds and
    /// takes this response, which then lists from the start, as the catalog.
    pub reset: bool,
    /// `true` when `limit` cut the list short: ask again with
    /// `since=<revision>` for the rest.
    pub more: bool,
    /// Files whose bytes are being hashed for a key now, or are queued for
    /// it. A client that shows pending keys polls while this is above zero.
    pub keys_hashing: usize,
    /// The key replacements after `since` and up to `revision`, oldest
    /// first. A file's key is replaced at most once, so the list never holds
    /// more entries than there are files.
    pub rekeys: Vec<FileRekey>,
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
    ParametricMap(Box<ParametricMapContext>),
    RtDose(Box<RtDoseContext>),
    PresentationState(Box<PresentationStateContext>),
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
    // Interpretation warnings; empty when the declared samples need no fallback.
    pub warnings: Vec<String>,
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
    /// The sRGB color the segmentation overlay paints this segment with.
    pub display_color: [u8; 3],
    /// Where `display_color` comes from: `recommended_cielab` (the
    /// Recommended Display CIELab Value, D50 CIELab converted to sRGB),
    /// `recommended_grayscale` (the Recommended Display Grayscale Value as a
    /// gray level), or `palette` (a fixed palette cycled by segment number,
    /// when neither is declared).
    pub display_color_source: String,
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
    /// Eligibility of the mapped-value colorwash on local image frames that
    /// share the map's Frame of Reference and lie within its frames.
    pub overlay: OverlayEligibility,
    /// The covered local image frames in file and frame order, at most 4096.
    pub overlay_source_frames: Vec<ResolvedSegmentSourceFrame>,
    /// Color bar over the mapped values of every frame; present when the
    /// overlay is eligible.
    pub legend: Option<OverlayLegend>,
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
    /// The VOI LUT the display path presents Modality values with in default
    /// mode when no window is requested and no DICOM window is stored.
    pub voi_lut: Option<VoiLookupTable>,
}

/// A VOI LUT: `values[clamp(trunc(modality) - first_value_mapped, 0,
/// values.length - 1)]`, an output of `bits_per_entry` (8 or 16) bits that
/// scales to 8 bits as `(output * 255 + max / 2) / max` in integers, where
/// `max = 2^bits_per_entry - 1`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct VoiLookupTable {
    pub first_value_mapped: i32,
    pub bits_per_entry: u16,
    pub values: Vec<u16>,
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
    /// `real_world_value_mapping` (an RWVM item declared in the file),
    /// `dose_grid_scaling` (RT Dose: `mapped = stored * DoseGridScaling`), or
    /// `rwvm_instance` (an item of a separate Real World Value Mapping
    /// instance that references this frame's image).
    pub source: String,
    /// File index of the RWVM instance when `source` is `rwvm_instance`.
    pub source_file_index: Option<usize>,
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
    /// Eligibility of the dose colorwash on local image frames that share the
    /// dose's Frame of Reference and lie within its grid.
    pub overlay: OverlayEligibility,
    /// The covered local image frames in file and frame order, at most 4096.
    pub overlay_source_frames: Vec<ResolvedSegmentSourceFrame>,
    /// Color bar of the dose colorwash; present when the overlay is eligible.
    pub legend: Option<OverlayLegend>,
    pub clinical_use_warning: String,
}

/// A Grayscale or Color Softcopy Presentation State's graphic annotations
/// (PS3.3 C.10.5) and the local image frames they are drawn on. Only the
/// annotations are applied: the state's window, shutter, displayed area and
/// spatial transformation are not.
#[derive(Debug, Clone, Serialize, TS)]
pub struct PresentationStateContext {
    pub content_label: Option<String>,
    pub content_description: Option<String>,
    pub content_creator_name: Option<String>,
    pub presentation_creation_date: Option<String>,
    /// In Graphic Layer Order.
    pub layers: Vec<GraphicLayerSummary>,
    /// The Graphic Annotation Sequence items in file order, at most 4096.
    pub items: Vec<GraphicAnnotationItemSummary>,
    /// The local image frames with a drawable annotation, in file and frame
    /// order, at most 4096.
    pub annotated_frames: Vec<ResolvedSegmentSourceFrame>,
    /// Objects of every item that are not drawn.
    pub skipped: SkippedGraphicObjects,
    pub references: Vec<ReferenceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
pub struct GraphicLayerSummary {
    pub name: String,
    pub order: Option<i32>,
    pub description: Option<String>,
    /// sRGB of the layer's Recommended Display CIELab Value, else of its
    /// Recommended Display Grayscale Value; `null` when it declares neither.
    pub color: Option<[u8; 3]>,
}

/// One Graphic Annotation Sequence item of a presentation state.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphicAnnotationItemSummary {
    /// Position in the Graphic Annotation Sequence, zero-based.
    pub index: usize,
    pub layer: String,
    /// Graphic Type of each drawable graphic object.
    pub graphic_types: Vec<GraphicType>,
    /// Text of each drawable text object.
    pub texts: Vec<String>,
    /// Whether the item names its images in a Referenced Image Sequence;
    /// otherwise it applies to every image and frame the state references.
    pub scoped: bool,
    /// The first local image frame the item applies to.
    pub first_frame: Option<ResolvedSegmentSourceFrame>,
    /// How many local image frames the item applies to.
    pub frame_count: usize,
}

/// Graphic and text objects that are not drawn, by reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, TS)]
pub struct SkippedGraphicObjects {
    /// DISPLAY units: positioned in the displayed area, which is not applied.
    pub display_units: usize,
    /// MATRIX units: positioned in a tiled image's total pixel matrix.
    pub matrix_units: usize,
    /// Unknown type or units, or point data that does not fit the type.
    pub malformed: usize,
    /// Text objects, which a masked session does not show.
    pub masked_text: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GraphicType {
    Point,
    Polyline,
    Interpolated,
    Circle,
    Ellipse,
}

/// The annotations one presentation state draws on one image frame. Every
/// coordinate is a PIXEL-unit `[column, row]` position in the image, where
/// `[0, 0]` is the top-left corner of the top-left pixel and
/// `[columns, rows]` the bottom-right corner of the bottom-right pixel.
#[derive(Debug, Clone, Serialize, TS)]
pub struct GraphicAnnotationsResponse {
    /// The state's layers, in Graphic Layer Order.
    pub layers: Vec<GraphicLayerSummary>,
    pub graphics: Vec<GraphicObjectSummary>,
    pub texts: Vec<TextObjectSummary>,
    /// Objects of the items applying to this frame that are not drawn.
    pub skipped: SkippedGraphicObjects,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
pub struct GraphicObjectSummary {
    /// The Graphic Annotation Sequence item it belongs to, zero-based.
    pub item: usize,
    pub layer: String,
    pub graphic_type: GraphicType,
    /// One point; the vertices of a polyline or the points an interpolated
    /// curve passes through (closed when the last equals the first); a
    /// circle's centre and a point on it; or an ellipse's two major-axis
    /// endpoints followed by its two minor-axis endpoints.
    pub points: Vec<[f64; 2]>,
    pub filled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
pub struct TextObjectSummary {
    /// The Graphic Annotation Sequence item it belongs to, zero-based.
    pub item: usize,
    pub layer: String,
    /// Lines are separated by `\n`.
    pub text: String,
    /// `[left, top, right, bottom]` of the bounding box.
    pub bounding_box: Option<[f64; 4]>,
    /// `left`, `center`, or `right` within the bounding box.
    pub justification: Option<TextJustification>,
    pub anchor: Option<[f64; 2]>,
    /// Whether a line joins the text to its anchor point.
    pub anchor_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TextJustification {
    Left,
    Center,
    Right,
}

/// Color bar of a value overlay. The overlay PNG colors a value `v` at
/// position `(v - min_value) / (max_value - min_value)`, clamped to `0..=1`,
/// along `color_stops`, which are evenly spaced and interpolated linearly in
/// RGB. Colored pixels are opaque, so the viewer applies overlay opacity.
#[derive(Debug, Clone, Serialize, TS)]
pub struct OverlayLegend {
    /// Short unit text, such as `Gy`, `RELATIVE`, or a UCUM code.
    pub unit_label: String,
    pub units: Option<CodedConceptSummary>,
    pub min_value: f64,
    pub max_value: f64,
    /// Values at or below this are transparent, as are pixels outside the
    /// overlay grid.
    pub transparent_at_or_below: Option<f64>,
    /// Name of the fixed colormap, `viridis`.
    pub colormap: String,
    pub color_stops: Vec<[u8; 3]>,
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
    /// A value overlay's planes do not reach the requested frame.
    OverlayNotCoveringFrame,
    PixelDecodeFailed,
    /// The decode needs more memory than is free and too many requests are
    /// already waiting for it. Status 503 with `Retry-After:`
    /// [`DECODE_BUSY_RETRY_AFTER_SECONDS`]; the same request succeeds once
    /// running decodes finish. Only the endpoints of
    /// [`endpoints::DECODING`] answer it.
    DecodeBusy,
    /// The decode needs more memory than the whole decode memory budget
    /// (`--decode-memory`), or, for a thumbnail, than the share of it
    /// thumbnails may use. Status 422; repeating the request cannot succeed
    /// in this session. The file is not unsupported: a larger budget decodes
    /// it. Only the endpoints of [`endpoints::DECODING`] answer it.
    DecodeMemoryExceeded,
    /// Content a masked session (`--mask`) withholds.
    Masked,
    /// An annotation operation was based on a revision its target has left.
    /// Status 409, on `endpoints::ANNOTATION_OPS` only, in an
    /// [`AnnotationOpResponse`] whose `result` holds the current state.
    /// Nothing was applied.
    AnnotationConflict,
    /// An annotation operation broke a rule of the annotation model or of
    /// the store. Status 422, on `endpoints::ANNOTATION_OPS` only, in an
    /// [`AnnotationOpResponse`] whose `result` lists the violations.
    /// Nothing was applied.
    AnnotationInvalid,
    /// An annotation write named a file that has no key and cannot be given
    /// one: its bytes, or those of the first file loaded with its SOP
    /// Instance UID, could not be read or have changed since discovery
    /// (`FileSummary::key_error`). Status 422. Nothing was written.
    FileKeyUnavailable,
    /// An annotation operation named a file by a key that is not the
    /// settled key of one file: files that shared the key turned out to
    /// differ, or the key was replaced (`FilesResponse::rekeys`). Status
    /// 409, a plain `ErrorResponse`. Nothing was applied; the catalog now
    /// shows the file's key, and the same operation under that key is
    /// applied.
    FileKeyReplaced,
    /// An annotation operation named a file by a `sop:` key that more than
    /// one loaded file carries the UID of, without saying which file it
    /// was drawn on (`AnnotationOpQuery::file`). Status 409, a plain
    /// `ErrorResponse`. Nothing was applied.
    FileKeyAmbiguous,
    /// A request body longer than its endpoint reads. Status 413.
    PayloadTooLarge,
    /// The request lacks the session's bearer token. Status 401 with
    /// `WWW-Authenticate:` [`UNAUTHORIZED_CHALLENGE`]. Answered for every
    /// path under [`API_PREFIX`], declared or not, before routing.
    Unauthorized,
    InternalError,
}

#[derive(Debug, Clone, Serialize, TS)]
pub struct ErrorResponse {
    pub code: ApiErrorCode,
    pub error: String,
}

/// The query of `endpoints::ANNOTATION_OPS`.
///
/// `file` is the catalog index of the file the operation was drawn on. It
/// stands beside the envelope and is no part of the annotation model: the
/// envelope names a file by key, and a key this viewer shows may be shared
/// by several loaded files (one SOP Instance UID) until they are compared.
/// With `file`, every file key in the envelope must be the settled key of
/// that one file, or the request is `409 file_key_replaced`. The record an
/// operation changes must be on the file the operation names, for every
/// kind of operation (`409 annotation_conflict` otherwise), so a request
/// for one file neither records anything on another nor changes a record
/// of another. The
/// viewer's page always sends it. Without it, a `sop:` key whose UID more
/// than one loaded file carries is `409 file_key_ambiguous`.
#[derive(Debug, Clone, Default, Deserialize, TS)]
#[ts(optional_fields)]
pub struct AnnotationOpQuery {
    pub file: Option<usize>,
}

/// The longest body `endpoints::ANNOTATION_OPS` reads, in bytes: the
/// annotation model's bound on one operation envelope, 16,777,216.
pub const ANNOTATION_OP_MAX_BYTES: usize = dcmview_annotation::limits::MAX_ENVELOPE_BYTES;

/// The answer to one annotation operation (`endpoints::ANNOTATION_OPS`).
///
/// | Status | `result.status` | `code` | Meaning |
/// |---|---|---|---|
/// | 200 | `ok` | absent | Applied, or applied earlier under the same `op_id`: `result.revs` holds the new revision of every record and layer it changed. |
/// | 409 | `conflict` | `annotation_conflict` | `result.current` is what the operation should have been based on. |
/// | 422 | `invalid` | `annotation_invalid` | `result.violations` says which rules were broken. |
///
/// An envelope is applied whole or not at all, a `batch` included, so with
/// 409 and 422 nothing changed. The two refusals are also error envelopes:
/// they carry `code` and `error` as an `ErrorResponse` does.
///
/// Any other failure is a plain `ErrorResponse`: 400 `invalid_json` for a
/// body that is not an envelope, 413 `payload_too_large`, 422
/// `file_key_unavailable`, 409 `file_key_replaced`, 409
/// `file_key_ambiguous`.
///
/// Every file key in the request and in `result` is in the form this
/// session sends keys (`FileSummary::file_key`): a masked session reads and
/// writes keys built from masked UIDs.
#[derive(Debug, Clone, Serialize, TS)]
pub struct AnnotationOpResponse {
    /// The annotation model's `ApplyResult`
    /// (`frontend/src/generated/annotation-types.ts`).
    #[ts(type = "ApplyResult")]
    pub result: dcmview_annotation::ApplyResult,
    /// The store's revision as this request left it: the number of
    /// envelopes it has applied. It rises by one for each envelope that changed
    /// something, and is unchanged by a refusal and by a repeated `op_id`.
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<ApiErrorCode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
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
    pub build_target: &'static str,
    pub build_profile: &'static str,
}

impl ViewerIdentity {
    pub const fn current() -> Self {
        Self {
            name: "dcmview",
            version: env!("CARGO_PKG_VERSION"),
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
    /// Whether this session masks patient identifiers (`--mask`).
    pub masked: bool,
}

/// Display-frame query. Explicit `wc`/`ww` must be sent together;
/// `mode=full_dynamic` ignores them (and `unit`).
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(optional_fields)]
pub struct FrameQuery {
    pub wc: Option<f64>,
    pub ww: Option<f64>,
    pub mode: Option<WindowMode>,
    /// The real-world unit `wc`/`ww` are in, which requires both. The window
    /// then applies to the values of the frame's preferred real-world
    /// mapping (the first of its value mapping's `real_world`) when it has this
    /// `unit_label`, as the viewer's raw renderer windows them; a frame whose
    /// preferred mapping has another unit, or whose samples are not 8- or
    /// 16-bit (or one-bit) integers, is shown with its default window.
    pub unit: Option<String>,
    /// `true` for a window/level drag preview: served from the display cache
    /// when present, otherwise rendered without being cached.
    pub preview: Option<bool>,
}

/// Thumbnail query.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
#[ts(optional_fields)]
pub struct ThumbnailQuery {
    /// Longest edge wanted, in device pixels: 1 to the largest of
    /// [`THUMBNAIL_SIZE_BUCKETS`], snapped up to the next bucket; absent
    /// means [`THUMBNAIL_DEFAULT_SIZE`]. Anything else is `400
    /// invalid_query`.
    pub size: Option<u32>,
    /// `default` (when absent) for the frame's default presentation, or
    /// `full_dynamic`. A thumbnail takes no explicit window.
    pub window_mode: Option<WindowMode>,
}

/// Raw-pixel query: the zero-based image row and column.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
pub struct PixelQuery {
    pub row: u32,
    pub column: u32,
}

/// Dose-overlay query: the RT Dose object drawn on the path's frame.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
pub struct DoseOverlayQuery {
    pub dose: usize,
}

/// Parametric-map-overlay query: the Parametric Map drawn on the path's frame.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
pub struct ParametricMapOverlayQuery {
    pub map: usize,
}

/// Graphic-annotations query: the softcopy presentation state whose
/// annotations are drawn on the path's frame.
#[derive(Debug, Clone, Copy, Deserialize, TS)]
pub struct GraphicAnnotationsQuery {
    pub state: usize,
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

/// The files a file's redaction boxes were copied to.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RedactionSeriesResponse {
    pub file_indices: Vec<usize>,
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

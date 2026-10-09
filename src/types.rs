pub use crate::api::contracts::{
    ErrorResponse, FileFormat, FileSummary, FilesResponse, FrameInfo, RawFrameMetadata, TagNode,
    TagValue, WindowMode, WindowPreset, RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_COLUMNS,
    RAW_FRAME_HEADER_DEFAULT_WC, RAW_FRAME_HEADER_DEFAULT_WW,
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, RAW_FRAME_HEADER_PIXEL_REPRESENTATION,
    RAW_FRAME_HEADER_RESCALE_INTERCEPT, RAW_FRAME_HEADER_RESCALE_SLOPE, RAW_FRAME_HEADER_ROWS,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
};
use crate::api::contracts::{
    RasterColorType, RasterExcludedPage, RasterSampleFormat, RasterSummary,
};
use std::path::PathBuf;

pub type PatientPosition = [f64; 3];
pub type PatientOrientation = [f64; 6];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePixelDataKind {
    Integer,
    Float32,
    Float64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DicomLut {
    pub first_mapped_value: i32,
    pub bits_per_entry: u16,
    pub entries: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlayPlane {
    pub group: u16,
    pub rows: u32,
    pub columns: u32,
    /// One-based row and column origin in the displayed image.
    pub origin: [i32; 2],
    pub overlay_type: String,
    pub number_of_frames: u32,
    /// One-based source image frame to which the first overlay frame applies.
    pub image_frame_origin: u32,
    /// Overlay Data words in DICOM's least-significant-bit-first pixel order.
    pub data: Vec<u16>,
}

/// One Shutter Shape (0018,1600) of a display shutter, as its opening: the
/// part of the image the shape leaves visible. Coordinates are one-based image
/// `[row, column]` pixel positions, and a pixel on an edge is inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShutterShape {
    /// Inclusive column (vertical edge) and row (horizontal edge) bounds.
    Rectangular {
        left_vertical_edge: i32,
        right_vertical_edge: i32,
        upper_horizontal_edge: i32,
        lower_horizontal_edge: i32,
    },
    /// Radius counted in pixels along the row direction, so non-square pixels
    /// still give a physically circular opening.
    Circular { center: [i32; 2], radius: i32 },
    /// Implicitly closed polygon of at least three vertices.
    Polygonal { vertices: Vec<[i32; 2]> },
    /// The overlay plane named by Shutter Overlay Group; its set bits are
    /// occluded. The plane is not also drawn as a visible overlay.
    Bitmap(OverlayPlane),
}

/// Display Shutter (PS3.3 C.7.6.11) and Bitmap Display Shutter (C.7.6.15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayShutter {
    /// A pixel stays visible only inside every shape's opening.
    pub shapes: Vec<ShutterShape>,
    /// Unsigned 16-bit P-value used outside the opening on grayscale frames,
    /// and on color frames without `presentation_color_cielab`.
    pub presentation_value: u16,
    /// Shutter Presentation Color CIELab Value as DICOM PCS-values (L*, a*,
    /// b* scaled to 0-FFFFH), used outside the opening on color frames.
    pub presentation_color_cielab: Option<[u16; 3]>,
}

#[derive(Debug, Clone, Default)]
pub struct PresentationMetadata {
    pub overlay_planes: Vec<OverlayPlane>,
    /// The shutter of every frame without its own: the Frame Display Shutter
    /// of the Shared Functional Groups, else the Display Shutter modules.
    pub display_shutter: Option<DisplayShutter>,
    /// Frame Display Shutters of the Per-frame Functional Groups, by
    /// zero-based frame; empty when no frame declares one.
    pub frame_display_shutters: Vec<Option<DisplayShutter>>,
}

impl PresentationMetadata {
    /// The shutter that applies to one zero-based frame.
    pub fn display_shutter_for_frame(&self, frame: u32) -> Option<&DisplayShutter> {
        self.frame_display_shutters
            .get(frame as usize)
            .and_then(Option::as_ref)
            .or(self.display_shutter.as_ref())
    }

    pub fn has_display_shutter(&self) -> bool {
        self.display_shutter.is_some() || self.frame_display_shutters.iter().any(Option::is_some)
    }
}

#[derive(Debug, Clone, Default)]
pub struct NativePixelMetadata {
    pub planar_configuration: Option<u32>,
    pub bits_stored: Option<u32>,
    pub high_bit: Option<u32>,
    pub pixel_data_kind: Option<NativePixelDataKind>,
    pub pixel_spacing: Option<[f64; 2]>,
    pub pixel_aspect_ratio: Option<[u32; 2]>,
    /// Relative physical row and column extents, normalized so column is 1.0.
    /// Pixel Spacing is authoritative when valid; Pixel Aspect Ratio is fallback.
    pub normalized_pixel_aspect: Option<[f64; 2]>,
    pub modality_lut: Option<DicomLut>,
    pub voi_lut: Option<DicomLut>,
    /// Inclusive stored-value range of Pixel Padding, read at discovery so no
    /// frame request has to parse the header again for it.
    pub pixel_padding: Option<[f64; 2]>,
}

impl NativePixelMetadata {
    pub fn effective_pixel_aspect_ratio(&self) -> Option<f64> {
        let [row, column] = self.normalized_pixel_aspect?;
        let ratio = row / column;
        (ratio.is_finite() && ratio > 0.0).then_some(ratio)
    }
}

/// What discovery read from a raster file's header
/// (`docs/design/image-formats.md` section 5.1). The fields hold for every
/// frame, which the page rule of section 2.4 guarantees. Nothing here is
/// decoded from pixel data, and no profile or pixel bytes are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterMetadata {
    pub color_type: RasterColorType,
    /// Bits per sample as stored in the file.
    pub bit_depth: u32,
    pub sample_format: RasterSampleFormat,
    /// An alpha channel, or palette transparency (PNG `tRNS`).
    pub has_alpha: bool,
    /// TIFF `ExtraSamples` 1 (premultiplied colour). Always `false` for PNG,
    /// JPEG and WebP, whose alpha is unassociated.
    pub alpha_associated: bool,
    /// EXIF or TIFF orientation, 1 to 8; 1 when absent or out of range.
    pub orientation: u8,
    pub has_icc: bool,
    /// Pages (IFDs) in the file; 1 for PNG, JPEG and WebP.
    pub pages_total: u32,
    /// The zero-based IFD index of each frame; never empty.
    pub frame_pages: Vec<u32>,
    /// The file's length in bytes when discovery listed it. A decode's
    /// reservation of decode memory is computed from it, so a decoder
    /// refuses a file that has grown past it since.
    pub file_length: u64,
    /// TIFF: the file offset of each frame's IFD, in frame order, so a frame
    /// request seeks to its page instead of walking the chain to it. Empty
    /// for PNG, JPEG and WebP. A decoder checks what it finds there against
    /// the entry: the file may have changed since it was listed.
    pub frame_offsets: Vec<u64>,
    /// The first [`RASTER_EXCLUDED_PAGES_LISTED`] pages left out of the frame
    /// map, in page order.
    pub excluded_pages: Vec<RasterExcludedPage>,
    /// How many pages are left out of the frame map, listed or not.
    pub excluded_pages_total: u32,
    /// Why the viewer does not decode this file, from its header alone;
    /// `None` for a file whose frames decode. The catalog reports it as
    /// `support_reason` and the frame endpoints answer it without reading
    /// the file.
    pub unsupported: Option<RasterUnsupported>,
    /// Animated PNG (`acTL`) or animated WebP.
    pub animated: bool,
    /// PNG `sBIT`, one entry per stored channel; informational only.
    pub significant_bits: Option<Vec<u8>>,
    /// Notes for the file report, such as a page whose ICC profile differs
    /// from page 0's or a page chain that could not be read to its end. At
    /// most [`RASTER_WARNINGS_MAX`] entries; when more arose, the last entry
    /// says how many are not shown.
    pub warnings: Vec<String>,
}

/// How many excluded pages a raster's metadata lists; the rest are counted.
pub const RASTER_EXCLUDED_PAGES_LISTED: usize = 16;

/// How many warnings a raster's metadata keeps.
pub const RASTER_WARNINGS_MAX: usize = 16;

/// The most pixels one raster frame may have and be decoded: 16,384 x
/// 16,384 (`docs/design/image-formats.md` section 2.3). It depends on nothing
/// about the host. A frame is decoded whole, so this is what bounds the
/// memory one decode can take; a larger file is listed and not decoded.
pub const RASTER_MAX_FRAME_PIXELS: u64 = 268_435_456;

/// Why the viewer lists a raster file and does not decode it
/// (`docs/design/image-formats.md` sections 2.3 and 11), decided from the
/// header at discovery. When several apply, the first in this order is
/// reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterUnsupported {
    /// The colour layout. TIFF: CIELab, more than four bands, an extra
    /// sample that is not alpha (`color_type` `Other`); palette; CMYK; gray
    /// with alpha; YCbCr; more than one sample stored as separate planes.
    Color,
    /// The sample format at this depth. TIFF: 16-bit float; 1-, 2- and 4-bit
    /// samples; 64-bit integers; colour that is not 8- or 16-bit unsigned.
    SampleFormat,
    /// TIFF compression other than none, LZW, Deflate or PackBits.
    Compression,
    /// A JPEG that is not 8-bit baseline, extended sequential or progressive
    /// Huffman: 12-bit, lossless, hierarchical or arithmetic-coded.
    JpegProcess,
    /// More than [`RASTER_MAX_FRAME_PIXELS`] pixels in a frame.
    TooLarge,
    /// A PNG, JPEG or WebP file longer than the bytes a decode of it may
    /// read (`pixels::raster_read_budget`: 64 MiB plus four times the
    /// frame). These are decoded from front to back, so the decoder would
    /// refuse every frame; the length is known at discovery, and the file
    /// is listed as not decoded instead. Never a TIFF, which is read a page
    /// at a time and may be any length.
    FileLength,
}

impl RasterMetadata {
    /// The part of the metadata the catalog reports.
    pub fn summary(&self) -> RasterSummary {
        RasterSummary {
            color_type: self.color_type,
            bit_depth: self.bit_depth,
            sample_format: self.sample_format,
            has_alpha: self.has_alpha,
            orientation: self.orientation,
            has_icc: self.has_icc,
            pages_total: self.pages_total,
            frame_pages: self.frame_pages.clone(),
            excluded_pages: self.excluded_pages.clone(),
            excluded_pages_total: self.excluded_pages_total,
            animated: self.animated,
            significant_bits: self.significant_bits.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub index: usize,
    pub path: PathBuf,
    /// The file's length in bytes when discovery opened it. Two files with
    /// one SOP Instance UID and different lengths cannot be the same bytes,
    /// and a whole-file digest is refused when the length has changed.
    pub size_bytes: u64,
    /// The file's modification time from the same `stat` as `size_bytes`.
    /// A whole-file digest is refused when it has changed, which is how a
    /// file rewritten with other bytes of the same length is told from the
    /// one discovery saw. `None` when the platform reports none, and for an
    /// entry that was not read from a file.
    pub modified: Option<std::time::SystemTime>,
    /// The container format, from the file's content. For a raster the DICOM
    /// identity strings, `sop_class_uid` and `transfer_syntax_uid` are empty.
    pub format: FileFormat,
    /// Present exactly when `format` is a raster format.
    pub raster: Option<Box<RasterMetadata>>,
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
    pub series_metadata: Box<SeriesMetadata>,
    pub has_pixels: bool,
    pub frame_count: u32,
    pub rows: u32,
    pub columns: u32,
    pub bits_allocated: u32,
    pub pixel_representation: u32,
    pub samples_per_pixel: u32,
    pub photometric_interpretation: String,
    pub rescale_slope: f64,
    pub rescale_intercept: f64,
    pub transfer_syntax_uid: String,
    pub default_window: Option<WindowPreset>,
}

#[derive(Debug, Clone, Default)]
pub struct SeriesMetadata {
    /// Discovery-time verdict for a binary-valued FRACTIONAL SEG; retains its declared maximum.
    pub binary_fractional_seg_maximum: Option<u32>,
    pub native_pixel: NativePixelMetadata,
    pub presentation: PresentationMetadata,
    pub frame_of_reference_uid: String,
    pub image_position_patient: Option<PatientPosition>,
    pub image_orientation_patient: Option<PatientOrientation>,
    pub frame_image_positions_patient: Vec<Option<PatientPosition>>,
    pub frame_image_orientations_patient: Vec<Option<PatientOrientation>>,
    pub frame_pixel_spacings: Vec<Option<[f64; 2]>>,
    pub concatenation_uid: Option<String>,
    pub in_concatenation_number: Option<u32>,
    pub in_concatenation_total_number: Option<u32>,
    pub concatenation_frame_offset_number: Option<u32>,
    pub sop_instance_uid_of_concatenation_source: Option<String>,
    pub image_type: Vec<String>,
    /// Burned In Annotation (0028,0301) is `YES`: the pixels are declared to
    /// carry text that may identify the patient.
    pub burned_in_annotation: bool,
    pub pyramid_uid: Option<String>,
    pub dimension_organization_type: Option<String>,
    pub dimension_organization_uids: Vec<String>,
    pub image_orientation_slide: Option<[f64; 6]>,
    pub total_pixel_matrix_rows: Option<u32>,
    pub total_pixel_matrix_columns: Option<u32>,
    pub total_pixel_matrix_focal_planes: Option<u32>,
    pub number_of_optical_paths: Option<u32>,
    pub container_identifier: Option<String>,
    pub specimen_uids: Vec<String>,
    pub optical_path_identifiers: Vec<String>,
}

impl FileEntry {
    pub fn frame_image_position_patient(&self, frame: u32) -> Option<PatientPosition> {
        frame_geometry_value(
            &self.series_metadata.frame_image_positions_patient,
            self.series_metadata.image_position_patient,
            frame,
        )
    }

    pub fn frame_image_orientation_patient(&self, frame: u32) -> Option<PatientOrientation> {
        frame_geometry_value(
            &self.series_metadata.frame_image_orientations_patient,
            self.series_metadata.image_orientation_patient,
            frame,
        )
    }

    pub fn frame_pixel_spacing(&self, frame: u32) -> Option<[f64; 2]> {
        frame_geometry_value(
            &self.series_metadata.frame_pixel_spacings,
            self.series_metadata.native_pixel.pixel_spacing,
            frame,
        )
    }

    pub fn raw_metadata(
        &self,
        rows: u32,
        columns: u32,
        bits_allocated: u32,
        samples_per_pixel: u32,
    ) -> RawFrameMetadata {
        RawFrameMetadata {
            rows,
            columns,
            bits_allocated,
            pixel_representation: self.pixel_representation,
            samples_per_pixel,
            photometric_interpretation: self.photometric_interpretation.clone(),
            rescale_slope: self.rescale_slope,
            rescale_intercept: self.rescale_intercept,
            default_wc: self.default_window.map(|window| window.center),
            default_ww: self.default_window.map(|window| window.width),
            padding_low: None,
            padding_high: None,
        }
    }
}

fn frame_geometry_value<const N: usize>(
    frame_values: &[Option<[f64; N]>],
    top_level_value: Option<[f64; N]>,
    frame: u32,
) -> Option<[f64; N]> {
    frame_values
        .get(frame as usize)
        .copied()
        .flatten()
        .or(top_level_value)
}

impl From<&FileEntry> for FileSummary {
    fn from(value: &FileEntry) -> Self {
        let support = crate::pixels::classify_pixel_support(value);
        Self {
            index: value.index,
            path: value.path.display().to_string(),
            display_name: value
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            label: value.label.clone(),
            patient_id: value.patient_id.clone(),
            patient_name: value.patient_name.clone(),
            study_instance_uid: value.study_instance_uid.clone(),
            study_date: value.study_date.clone(),
            study_description: value.study_description.clone(),
            series_instance_uid: value.series_instance_uid.clone(),
            series_number: value.series_number.clone(),
            series_description: value.series_description.clone(),
            modality: value.modality.clone(),
            instance_number: value.instance_number.clone(),
            sop_instance_uid: value.sop_instance_uid.clone(),
            sop_class_uid: value.sop_class_uid.clone(),
            object_kind: crate::object_kind::classify_file(value).to_string(),
            file_format: value.format,
            raster: value.raster.as_deref().map(RasterMetadata::summary),
            support_state: support.state,
            support_reason: support.reason_id().map(ToString::to_string),
            // The browser reproduces every presentation transform: Modality
            // and VOI LUTs from the value mapping, shutter and overlays from
            // the presentation layer.
            raw_windowing_compatible: true,
            raw_windowing_reason: None,
            presentation_layer: !value.series_metadata.presentation.overlay_planes.is_empty()
                || value.series_metadata.presentation.has_display_shutter(),
            burned_in_annotation: value.series_metadata.burned_in_annotation,
            has_pixels: value.has_pixels,
            frame_count: value.frame_count,
            rows: value.rows,
            columns: value.columns,
            pixel_aspect_ratio: value
                .series_metadata
                .native_pixel
                .effective_pixel_aspect_ratio(),
            transfer_syntax_uid: value.transfer_syntax_uid.clone(),
            default_window: value.default_window,
            // The catalog fills these from its key table; an entry on its
            // own has the key its UID implies.
            file_key: None,
            alias_of: None,
            key_error: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameCacheKey {
    pub file_index: usize,
    pub frame: u32,
    pub window_center_bits: Option<u64>,
    pub window_width_bits: Option<u64>,
    pub window_mode: WindowMode,
    /// The real-world unit an explicit window is in; `None` for Modality
    /// values.
    pub window_unit: Option<String>,
    /// Revision of the redaction boxes painted on the frame; 0 without any.
    pub redaction: u64,
}

impl FrameCacheKey {
    pub fn new(
        file_index: usize,
        frame: u32,
        window_center: Option<f64>,
        window_width: Option<f64>,
        window_mode: WindowMode,
        window_unit: Option<&str>,
    ) -> Self {
        let (window_center, window_width) = match window_mode {
            WindowMode::Default => (window_center, window_width),
            WindowMode::FullDynamic => (None, None),
        };
        Self {
            file_index,
            frame,
            window_center_bits: window_center.map(f64::to_bits),
            window_width_bits: window_width.map(f64::to_bits),
            window_mode,
            window_unit: window_center.and(window_unit).map(str::to_string),
            redaction: 0,
        }
    }
}

/// A cached thumbnail: one frame at one size bucket and window mode. Every
/// thumbnail source yields the same presentation, so the key has no source.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbnailCacheKey {
    pub file_index: usize,
    pub frame: u32,
    /// One of `contracts::THUMBNAIL_SIZE_BUCKETS`.
    pub bucket: u32,
    pub window_mode: WindowMode,
    /// Revision of the redaction boxes painted on the frame; 0 without any.
    /// A change to a file's boxes moves the revision, so a thumbnail
    /// rendered before the change is never served after it.
    pub redaction: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowRequest {
    center: Option<f64>,
    width: Option<f64>,
    mode: WindowMode,
}

impl WindowRequest {
    pub fn new(
        center: Option<f64>,
        width: Option<f64>,
        mode: WindowMode,
    ) -> Result<Self, WindowRequestError> {
        match (center, width) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => {
                return Err(WindowRequestError::IncompletePair);
            }
            (Some(center), Some(width)) => {
                if !center.is_finite() {
                    return Err(WindowRequestError::NonFiniteCenter);
                }
                if !width.is_finite() {
                    return Err(WindowRequestError::NonFiniteWidth);
                }
                if width <= 0.0 {
                    return Err(WindowRequestError::NonPositiveWidth);
                }
            }
        }

        // Signed zero renders identically. Preserve sub-unit widths: their
        // meaning depends on the samples and mapping, unknown at validation.
        Ok(Self {
            center: center.map(|center| center + 0.0),
            width,
            mode,
        })
    }

    pub fn center(self) -> Option<f64> {
        self.center
    }

    pub fn width(self) -> Option<f64> {
        self.width
    }

    pub fn mode(self) -> WindowMode {
        self.mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRequestError {
    IncompletePair,
    NonFiniteCenter,
    NonFiniteWidth,
    NonPositiveWidth,
}

impl std::fmt::Display for WindowRequestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::IncompletePair => "window center and width must be provided together",
            Self::NonFiniteCenter => "window center must be finite",
            Self::NonFiniteWidth => "window width must be finite",
            Self::NonPositiveWidth => "window width must be greater than zero",
        })
    }
}

impl std::error::Error for WindowRequestError {}

#[cfg(test)]
mod tests {
    use super::{
        frame_geometry_value, FrameCacheKey, NativePixelMetadata, WindowMode, WindowRequest,
        WindowRequestError,
    };

    #[test]
    fn frame_geometry_prefers_frame_value_and_falls_back_to_top_level() {
        let top_level = Some([1.0, 2.0, 3.0]);
        let per_frame = vec![Some([4.0, 5.0, 6.0]), None];

        assert_eq!(frame_geometry_value(&per_frame, top_level, 0), per_frame[0]);
        assert_eq!(frame_geometry_value(&per_frame, top_level, 1), top_level);
        assert_eq!(frame_geometry_value(&per_frame, top_level, 2), top_level);
        assert_eq!(frame_geometry_value::<3>(&[], None, 0), None);
    }

    #[test]
    fn effective_pixel_aspect_ratio_requires_positive_finite_geometry() {
        let mut metadata = NativePixelMetadata {
            normalized_pixel_aspect: Some([2.0, 1.0]),
            ..Default::default()
        };
        assert_eq!(metadata.effective_pixel_aspect_ratio(), Some(2.0));

        metadata.normalized_pixel_aspect = Some([f64::INFINITY, 1.0]);
        assert_eq!(metadata.effective_pixel_aspect_ratio(), None);
        metadata.normalized_pixel_aspect = Some([1.0, 0.0]);
        assert_eq!(metadata.effective_pixel_aspect_ratio(), None);
    }

    #[test]
    fn frame_cache_key_distinguishes_absent_and_explicit_window_params() {
        let default_window = FrameCacheKey::new(0, 0, None, None, WindowMode::Default, None);
        let explicit = FrameCacheKey::new(0, 0, Some(0.0), Some(1.0), WindowMode::Default, None);
        let mapped =
            FrameCacheKey::new(0, 0, Some(0.0), Some(1.0), WindowMode::Default, Some("Gy"));

        assert_ne!(default_window, explicit);
        assert_ne!(explicit, mapped);
        assert_eq!(explicit.window_center_bits, Some(0));
        assert_eq!(explicit.window_width_bits, Some(1.0_f64.to_bits()));
        assert_eq!(
            FrameCacheKey::new(0, 0, None, None, WindowMode::Default, Some("Gy")),
            default_window,
            "a unit without a window is the default window"
        );
    }

    #[test]
    fn full_dynamic_cache_key_ignores_explicit_window_values() {
        let first = FrameCacheKey::new(0, 0, Some(10.0), Some(20.0), WindowMode::FullDynamic, None);
        let second = FrameCacheKey::new(
            0,
            0,
            Some(30.0),
            Some(40.0),
            WindowMode::FullDynamic,
            Some("Gy"),
        );

        assert_eq!(first, second);
        assert_eq!(first.window_center_bits, None);
        assert_eq!(first.window_width_bits, None);
    }

    #[test]
    fn window_request_requires_a_finite_positive_pair() {
        assert_eq!(
            WindowRequest::new(Some(10.0), None, WindowMode::Default),
            Err(WindowRequestError::IncompletePair)
        );
        assert_eq!(
            WindowRequest::new(Some(f64::INFINITY), Some(20.0), WindowMode::Default),
            Err(WindowRequestError::NonFiniteCenter)
        );
        assert_eq!(
            WindowRequest::new(Some(10.0), Some(f64::NAN), WindowMode::Default),
            Err(WindowRequestError::NonFiniteWidth)
        );
        assert_eq!(
            WindowRequest::new(Some(10.0), Some(0.0), WindowMode::Default),
            Err(WindowRequestError::NonPositiveWidth)
        );
        assert!(WindowRequest::new(Some(10.0), Some(20.0), WindowMode::Default).is_ok());
    }

    #[test]
    fn window_requests_that_render_alike_share_a_key() {
        let key = |center: f64, width: f64| {
            let request = WindowRequest::new(Some(center), Some(width), WindowMode::Default)
                .expect("valid window");
            FrameCacheKey::new(
                0,
                0,
                request.center(),
                request.width(),
                request.mode(),
                None,
            )
        };
        assert_eq!(key(-0.0, 0.25), key(0.0, 0.25));
        assert_ne!(key(0.0, 0.25), key(0.0, 1.0));
        assert_ne!(key(0.0, 2.0), key(0.0, 1.0));
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedWindow {
    pub center: f64,
    pub width: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawFrameCacheKey {
    pub file_index: usize,
    pub frame: u32,
}

/// An encoded overlay drawn on one displayed frame: a whole RT Dose or
/// Parametric Map volume, or one SEG frame on its resolved source frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OverlayCacheKey {
    pub overlay_file_index: usize,
    /// The SEG frame drawn; `None` for value overlays, which sample the
    /// whole volume. A presentation layer (a file's own shutter and overlay
    /// graphics) is keyed as a file overlaying its own frame.
    pub overlay_frame: Option<u32>,
    pub target_file_index: usize,
    pub target_frame: u32,
    pub encoding: OverlayEncoding,
    /// For a value overlay, how many files were registered when the legend
    /// it is colored by was read ([`ValueRangeCacheKey::file_set`]): the
    /// file set decides the volume's mapping and so its scale. `None` for a
    /// SEG overlay and a presentation layer, which have no legend.
    pub file_set: Option<usize>,
}

/// The range of an RT Dose or Parametric Map's mapped values, which its
/// legend spans: one per object and file set, since the Real World Value
/// Mapping instances of the file set decide the mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ValueRangeCacheKey {
    pub file_index: usize,
    /// How many files were registered when the range was asked for. The
    /// registry only grows, so its length names the file set.
    pub file_set: usize,
}

/// How an overlay is sent: a colored PNG, or a value overlay's resampled
/// values as little-endian `f32`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverlayEncoding {
    Png,
    Values,
}

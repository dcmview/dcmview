pub use crate::api::contracts::{
    ErrorResponse, FileSummary, FilesResponse, FrameInfo, RawFrameMetadata, TagNode, TagValue,
    WindowMode, WindowPreset, RAW_FRAME_HEADER_BITS_ALLOCATED, RAW_FRAME_HEADER_COLUMNS,
    RAW_FRAME_HEADER_DEFAULT_WC, RAW_FRAME_HEADER_DEFAULT_WW,
    RAW_FRAME_HEADER_PHOTOMETRIC_INTERPRETATION, RAW_FRAME_HEADER_PIXEL_REPRESENTATION,
    RAW_FRAME_HEADER_RESCALE_INTERCEPT, RAW_FRAME_HEADER_RESCALE_SLOPE, RAW_FRAME_HEADER_ROWS,
    RAW_FRAME_HEADER_SAMPLES_PER_PIXEL,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectangularDisplayShutter {
    /// One-based inclusive image column bounds.
    pub left_vertical_edge: i32,
    pub right_vertical_edge: i32,
    /// One-based inclusive image row bounds.
    pub upper_horizontal_edge: i32,
    pub lower_horizontal_edge: i32,
    /// Unsigned 16-bit P-value used outside the shutter opening.
    pub presentation_value: u16,
}

#[derive(Debug, Clone, Default)]
pub struct PresentationMetadata {
    pub overlay_planes: Vec<OverlayPlane>,
    pub rectangular_shutter: Option<RectangularDisplayShutter>,
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
}

impl NativePixelMetadata {
    pub fn effective_pixel_aspect_ratio(&self) -> Option<f64> {
        let [row, column] = self.normalized_pixel_aspect?;
        let ratio = row / column;
        (ratio.is_finite() && ratio > 0.0).then_some(ratio)
    }
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub index: usize,
    pub path: PathBuf,
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
        let raw_windowing_reason = raw_windowing_incompatibility(&value.series_metadata);
        Self {
            index: value.index,
            path: value.path.display().to_string(),
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
            object_kind: crate::object_kind::classify_sop_class(&value.sop_class_uid).to_string(),
            support_state: match support.state {
                crate::pixels::PixelSupportState::Renderable => {
                    crate::api::contracts::SupportState::Renderable
                }
                crate::pixels::PixelSupportState::MetadataOnly => {
                    crate::api::contracts::SupportState::MetadataOnly
                }
                crate::pixels::PixelSupportState::Unsupported => {
                    crate::api::contracts::SupportState::Unsupported
                }
            },
            support_reason: support.reason_id().map(ToString::to_string),
            raw_windowing_compatible: raw_windowing_reason.is_none(),
            raw_windowing_reason: raw_windowing_reason.map(ToString::to_string),
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
        }
    }
}

fn raw_windowing_incompatibility(metadata: &SeriesMetadata) -> Option<&'static str> {
    if metadata.native_pixel.modality_lut.is_some() {
        Some("client raw windowing is disabled because a Modality LUT is declared")
    } else if metadata.native_pixel.voi_lut.is_some() {
        Some("client raw windowing is disabled because a VOI LUT is declared")
    } else if !metadata.presentation.overlay_planes.is_empty() {
        Some("client raw windowing is disabled because an overlay plane is declared")
    } else if metadata.presentation.rectangular_shutter.is_some() {
        Some("client raw windowing is disabled because a display shutter is declared")
    } else {
        None
    }
}

#[cfg(test)]
mod raw_windowing_tests {
    use super::{raw_windowing_incompatibility, DicomLut, OverlayPlane, SeriesMetadata};

    #[test]
    fn disables_raw_windowing_for_unrepresented_presentation_semantics() {
        let mut metadata = SeriesMetadata::default();
        assert_eq!(raw_windowing_incompatibility(&metadata), None);

        metadata.native_pixel.voi_lut = Some(DicomLut {
            first_mapped_value: 0,
            bits_per_entry: 8,
            entries: vec![0, 255],
        });
        assert!(raw_windowing_incompatibility(&metadata)
            .expect("VOI LUT reason")
            .contains("VOI LUT"));

        metadata.native_pixel.voi_lut = None;
        metadata.presentation.overlay_planes.push(OverlayPlane {
            group: 0x6000,
            rows: 1,
            columns: 1,
            origin: [1, 1],
            overlay_type: "G".to_string(),
            number_of_frames: 1,
            image_frame_origin: 1,
            data: vec![1],
        });
        assert!(raw_windowing_incompatibility(&metadata)
            .expect("overlay reason")
            .contains("overlay"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameCacheKey {
    pub file_index: usize,
    pub frame: u32,
    pub window_center_bits: Option<u64>,
    pub window_width_bits: Option<u64>,
    pub window_mode: WindowMode,
}

impl FrameCacheKey {
    pub fn new(
        file_index: usize,
        frame: u32,
        window_center: Option<f64>,
        window_width: Option<f64>,
        window_mode: WindowMode,
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
        }
    }
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

        Ok(Self {
            center,
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
        let default_window = FrameCacheKey::new(0, 0, None, None, WindowMode::Default);
        let explicit = FrameCacheKey::new(0, 0, Some(0.0), Some(1.0), WindowMode::Default);

        assert_ne!(default_window, explicit);
        assert_eq!(explicit.window_center_bits, Some(0));
        assert_eq!(explicit.window_width_bits, Some(1.0_f64.to_bits()));
    }

    #[test]
    fn full_dynamic_cache_key_ignores_explicit_window_values() {
        let first = FrameCacheKey::new(0, 0, Some(10.0), Some(20.0), WindowMode::FullDynamic);
        let second = FrameCacheKey::new(0, 0, Some(30.0), Some(40.0), WindowMode::FullDynamic);

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
}

#[derive(Debug, Clone)]
pub struct LoadReport {
    pub files: Vec<FileEntry>,
    pub skipped: usize,
    pub filtered: usize,
    pub searched_recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferSyntaxClass {
    Jpeg,
    JpegLossless,
    Jpeg2000,
    JpegXl,
    Uncompressed,
    JpegLs,
    Rle,
    Unsupported,
}

#[derive(Debug, Clone)]
pub struct ResolvedWindow {
    pub center: f64,
    pub width: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawFrameCacheKey {
    pub file_index: usize,
    pub frame: u32,
}

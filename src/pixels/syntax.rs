use crate::types::{FileEntry, TransferSyntaxClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelSupportState {
    Renderable,
    MetadataOnly,
    Unsupported,
}

impl PixelSupportState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Renderable => "renderable",
            Self::MetadataOnly => "metadata_only",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelSupportReason {
    PixelDataAbsentOrUnrecognized,
    RleLosslessNotSupported,
    JpegLsNotSupported,
    JpegXlNotSupported,
    DeflatedImageFrameNotSupported,
    TransferSyntaxNotSupported,
    InvalidGeometry,
    BitPackedPixelsNotSupported,
    NumericPrecisionNotSupported,
    SamplesPerPixelNotSupported,
    GenericColorRenderingOnly,
    PaletteColorNotSupported,
    PhotometricInterpretationNotSupported,
}

impl PixelSupportReason {
    /// Stable, machine-readable reason identifier for compatibility evidence.
    pub const fn id(self) -> &'static str {
        match self {
            Self::PixelDataAbsentOrUnrecognized => "pixel_data.absent_or_unrecognized",
            Self::RleLosslessNotSupported => "transfer_syntax.rle_lossless_not_supported",
            Self::JpegLsNotSupported => "transfer_syntax.jpeg_ls_not_supported",
            Self::JpegXlNotSupported => "transfer_syntax.jpeg_xl_not_supported",
            Self::DeflatedImageFrameNotSupported => {
                "transfer_syntax.deflated_image_frame_not_supported"
            }
            Self::TransferSyntaxNotSupported => "transfer_syntax.not_supported",
            Self::InvalidGeometry => "pixel_layout.invalid_geometry",
            Self::BitPackedPixelsNotSupported => "pixel_layout.bit_packed_not_supported",
            Self::NumericPrecisionNotSupported => "pixel_layout.numeric_precision_not_supported",
            Self::SamplesPerPixelNotSupported => "pixel_layout.samples_per_pixel_not_supported",
            Self::GenericColorRenderingOnly => "pixel_layout.generic_color_rendering_only",
            Self::PaletteColorNotSupported => "pixel_layout.palette_color_not_supported",
            Self::PhotometricInterpretationNotSupported => {
                "pixel_layout.photometric_interpretation_not_supported"
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelSupport {
    pub state: PixelSupportState,
    pub reason: Option<PixelSupportReason>,
}

impl PixelSupport {
    const fn renderable() -> Self {
        Self {
            state: PixelSupportState::Renderable,
            reason: None,
        }
    }

    const fn metadata_only(reason: PixelSupportReason) -> Self {
        Self {
            state: PixelSupportState::MetadataOnly,
            reason: Some(reason),
        }
    }

    const fn unsupported(reason: PixelSupportReason) -> Self {
        Self {
            state: PixelSupportState::Unsupported,
            reason: Some(reason),
        }
    }

    pub const fn reason_id(self) -> Option<&'static str> {
        match self.reason {
            Some(reason) => Some(reason.id()),
            None => None,
        }
    }
}

pub fn classify_transfer_syntax(uid: &str) -> TransferSyntaxClass {
    match uid {
        // JPEG Baseline is qualified; Extended 12-bit remains controlled unsupported.
        "1.2.840.10008.1.2.4.50" => TransferSyntaxClass::Jpeg,
        // JPEG Lossless: browsers cannot decode — must be decoded server-side
        "1.2.840.10008.1.2.4.57" | "1.2.840.10008.1.2.4.70" => TransferSyntaxClass::JpegLossless,
        // Only the lossless JPEG 2000 process is qualified.
        "1.2.840.10008.1.2.4.90" => TransferSyntaxClass::Jpeg2000,
        "1.2.840.10008.1.2.4.110" => TransferSyntaxClass::JpegXl,
        "1.2.840.10008.1.2"
        | "1.2.840.10008.1.2.1"
        | "1.2.840.10008.1.2.2"
        | "1.2.840.10008.1.2.1.99" => TransferSyntaxClass::Uncompressed,
        "1.2.840.10008.1.2.4.80" => TransferSyntaxClass::JpegLs,
        "1.2.840.10008.1.2.5" => TransferSyntaxClass::Rle,
        _ => TransferSyntaxClass::Unsupported,
    }
}

/// Describe the viewer's current display capability without changing decode routing.
///
/// This classifier describes the layouts that the active display pipeline handles;
/// semantic interpretation such as segmentation or parametric mapping is separate.
pub fn classify_pixel_support(file: &FileEntry) -> PixelSupport {
    if !file.has_pixels {
        return PixelSupport::metadata_only(PixelSupportReason::PixelDataAbsentOrUnrecognized);
    }

    if file.transfer_syntax_uid == super::deflated_frame::DEFLATED_IMAGE_FRAME_UID {
        return if file.rows > 0
            && file.columns > 0
            && file.bits_allocated == 1
            && file.pixel_representation == 0
            && file.samples_per_pixel == 1
            && matches!(
                file.photometric_interpretation.trim(),
                "MONOCHROME1" | "MONOCHROME2"
            ) {
            PixelSupport::renderable()
        } else {
            PixelSupport::unsupported(PixelSupportReason::BitPackedPixelsNotSupported)
        };
    }

    let syntax_class = classify_transfer_syntax(&file.transfer_syntax_uid);
    match syntax_class {
        TransferSyntaxClass::Unsupported => {
            return PixelSupport::unsupported(unsupported_transfer_syntax_reason(
                &file.transfer_syntax_uid,
            ));
        }
        TransferSyntaxClass::Rle
        | TransferSyntaxClass::JpegLs
        | TransferSyntaxClass::Jpeg
        | TransferSyntaxClass::JpegLossless
        | TransferSyntaxClass::Jpeg2000
        | TransferSyntaxClass::JpegXl
        | TransferSyntaxClass::Uncompressed => {}
    }

    if file.rows == 0 || file.columns == 0 {
        return PixelSupport::unsupported(PixelSupportReason::InvalidGeometry);
    }

    if file.samples_per_pixel == 0 {
        return PixelSupport::unsupported(PixelSupportReason::SamplesPerPixelNotSupported);
    }

    let pixel_kind = file
        .series_metadata
        .native_pixel
        .pixel_data_kind
        .unwrap_or(crate::types::NativePixelDataKind::Integer);
    let supported_precision = match syntax_class {
        TransferSyntaxClass::Uncompressed => match pixel_kind {
            crate::types::NativePixelDataKind::Integer => {
                matches!(file.bits_allocated, 1 | 8 | 16 | 32)
            }
            crate::types::NativePixelDataKind::Float32 => file.bits_allocated == 32,
            crate::types::NativePixelDataKind::Float64 => file.bits_allocated == 64,
        },
        _ => {
            pixel_kind == crate::types::NativePixelDataKind::Integer
                && matches!(file.bits_allocated, 8 | 16)
        }
    };
    if !supported_precision {
        return PixelSupport::unsupported(if file.bits_allocated == 1 {
            PixelSupportReason::BitPackedPixelsNotSupported
        } else {
            PixelSupportReason::NumericPrecisionNotSupported
        });
    }

    let photometric = file.photometric_interpretation.trim().to_ascii_uppercase();
    match (file.samples_per_pixel, photometric.as_str()) {
        (1, "MONOCHROME1" | "MONOCHROME2") => PixelSupport::renderable(),
        (1, "PALETTE COLOR")
            if matches!(
                syntax_class,
                TransferSyntaxClass::Rle | TransferSyntaxClass::Uncompressed
            ) && file.bits_allocated == 8 =>
        {
            PixelSupport::renderable()
        }
        (1, "PALETTE COLOR") => {
            PixelSupport::unsupported(PixelSupportReason::PaletteColorNotSupported)
        }
        (3, color) if renders_color(syntax_class, color, file.bits_allocated) => {
            PixelSupport::renderable()
        }
        (3, "RGB" | "YBR_FULL" | "YBR_FULL_422" | "YBR_ICT" | "YBR_RCT") => {
            PixelSupport::unsupported(PixelSupportReason::GenericColorRenderingOnly)
        }
        (1, _) => {
            PixelSupport::unsupported(PixelSupportReason::PhotometricInterpretationNotSupported)
        }
        _ => PixelSupport::unsupported(PixelSupportReason::SamplesPerPixelNotSupported),
    }
}

/// Three-sample layouts each display decoder actually converts to RGB.
///
/// This must stay in step with the color arms of the codec modules; a layout
/// listed here but rejected by its decoder would be advertised as renderable
/// and then fail on every frame request.
fn renders_color(syntax_class: TransferSyntaxClass, photometric: &str, bits: u32) -> bool {
    match syntax_class {
        // dicom-pixeldata converts baseline YBR to RGB; native.rs expands 4:2:2.
        TransferSyntaxClass::Jpeg | TransferSyntaxClass::Uncompressed => {
            bits == 8 && matches!(photometric, "RGB" | "YBR_FULL" | "YBR_FULL_422")
        }
        // rle.rs normalizes RGB and YBR_FULL byte planes only.
        TransferSyntaxClass::Rle => bits == 8 && matches!(photometric, "RGB" | "YBR_FULL"),
        // Lossless JPEG and JPEG XL decode without a color transform.
        TransferSyntaxClass::JpegLossless | TransferSyntaxClass::JpegXl => {
            bits == 8 && photometric == "RGB"
        }
        // OpenJPEG applies the inverse RCT/ICT, so every JPEG 2000 color
        // photometric decodes to RGB components.
        TransferSyntaxClass::Jpeg2000 => {
            matches!(bits, 8 | 16) && matches!(photometric, "RGB" | "YBR_RCT" | "YBR_ICT")
        }
        TransferSyntaxClass::JpegLs | TransferSyntaxClass::Unsupported => false,
    }
}

fn unsupported_transfer_syntax_reason(uid: &str) -> PixelSupportReason {
    match uid {
        "1.2.840.10008.1.2.4.81" => PixelSupportReason::JpegLsNotSupported,
        "1.2.840.10008.1.2.4.111" | "1.2.840.10008.1.2.4.112" => {
            PixelSupportReason::JpegXlNotSupported
        }
        "1.2.840.10008.1.2.8.1" => PixelSupportReason::DeflatedImageFrameNotSupported,
        _ => PixelSupportReason::TransferSyntaxNotSupported,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_pixel_support, PixelSupportReason, PixelSupportState};
    use crate::types::{FileEntry, NativePixelDataKind};
    use std::path::PathBuf;

    fn file(transfer_syntax_uid: &str) -> FileEntry {
        FileEntry {
            index: 0,
            path: PathBuf::from("fixture.dcm"),
            label: String::new(),
            patient_id: String::new(),
            patient_name: String::new(),
            study_instance_uid: String::new(),
            study_date: String::new(),
            study_description: String::new(),
            series_instance_uid: String::new(),
            series_number: String::new(),
            series_description: String::new(),
            modality: String::new(),
            instance_number: String::new(),
            sop_instance_uid: String::new(),
            sop_class_uid: "1.2.840.10008.5.1.4.1.1.7".to_string(),
            series_metadata: Default::default(),
            has_pixels: true,
            frame_count: 1,
            rows: 2,
            columns: 2,
            bits_allocated: 16,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            transfer_syntax_uid: transfer_syntax_uid.to_string(),
            default_window: None,
        }
    }

    #[test]
    fn current_monochrome_decode_paths_are_renderable() {
        for uid in [
            "1.2.840.10008.1.2",
            "1.2.840.10008.1.2.1",
            "1.2.840.10008.1.2.2",
            "1.2.840.10008.1.2.1.99",
            "1.2.840.10008.1.2.4.50",
            "1.2.840.10008.1.2.4.70",
            "1.2.840.10008.1.2.4.90",
            "1.2.840.10008.1.2.4.80",
        ] {
            let support = classify_pixel_support(&file(uid));
            assert_eq!(support.state, PixelSupportState::Renderable, "{uid}");
            assert_eq!(support.reason_id(), None, "{uid}");
        }
    }

    #[test]
    fn absent_or_unrecognized_pixel_elements_are_metadata_only() {
        let mut entry = file("1.2.840.10008.1.2.1");
        entry.has_pixels = false;
        entry.bits_allocated = 64;

        let support = classify_pixel_support(&entry);
        assert_eq!(support.state, PixelSupportState::MetadataOnly);
        assert_eq!(
            support.reason,
            Some(PixelSupportReason::PixelDataAbsentOrUnrecognized)
        );
        assert_eq!(
            support.reason_id(),
            Some("pixel_data.absent_or_unrecognized")
        );
    }

    #[test]
    fn disabled_codecs_have_transfer_syntax_reasons() {
        let cases = [
            (
                "1.2.840.10008.1.2.4.81",
                PixelSupportReason::JpegLsNotSupported,
                "transfer_syntax.jpeg_ls_not_supported",
            ),
            (
                "1.2.840.10008.1.2.4.111",
                PixelSupportReason::JpegXlNotSupported,
                "transfer_syntax.jpeg_xl_not_supported",
            ),
            (
                "1.2.840.10008.1.2.4.51",
                PixelSupportReason::TransferSyntaxNotSupported,
                "transfer_syntax.not_supported",
            ),
            (
                "1.2.840.10008.1.2.4.91",
                PixelSupportReason::TransferSyntaxNotSupported,
                "transfer_syntax.not_supported",
            ),
            (
                "9.9.9",
                PixelSupportReason::TransferSyntaxNotSupported,
                "transfer_syntax.not_supported",
            ),
        ];

        for (uid, reason, reason_id) in cases {
            let support = classify_pixel_support(&file(uid));
            assert_eq!(support.state, PixelSupportState::Unsupported, "{uid}");
            assert_eq!(support.reason, Some(reason), "{uid}");
            assert_eq!(support.reason_id(), Some(reason_id), "{uid}");
        }
    }

    #[test]
    fn only_the_bounded_deflated_binary_layout_is_renderable() {
        let mut entry = file("1.2.840.10008.1.2.8.1");
        entry.bits_allocated = 1;
        assert_eq!(
            classify_pixel_support(&entry).state,
            PixelSupportState::Renderable
        );

        entry.bits_allocated = 8;
        let unsupported = classify_pixel_support(&entry);
        assert_eq!(unsupported.state, PixelSupportState::Unsupported);
        assert_eq!(
            unsupported.reason,
            Some(PixelSupportReason::BitPackedPixelsNotSupported)
        );
    }

    #[test]
    fn supported_rle_layouts_are_renderable() {
        for (samples_per_pixel, photometric) in [
            (1, "MONOCHROME1"),
            (1, "MONOCHROME2"),
            (1, "PALETTE COLOR"),
            (3, "RGB"),
            (3, "YBR_FULL"),
        ] {
            let mut entry = file("1.2.840.10008.1.2.5");
            entry.bits_allocated = 8;
            entry.samples_per_pixel = samples_per_pixel;
            entry.photometric_interpretation = photometric.to_string();

            let support = classify_pixel_support(&entry);
            assert_eq!(
                support.state,
                PixelSupportState::Renderable,
                "{photometric}"
            );
            assert_eq!(support.reason, None, "{photometric}");
        }

        let mut unsupported = file("1.2.840.10008.1.2.5");
        unsupported.bits_allocated = 16;
        unsupported.samples_per_pixel = 3;
        unsupported.photometric_interpretation = "RGB".to_string();
        assert_eq!(
            classify_pixel_support(&unsupported).reason,
            Some(PixelSupportReason::GenericColorRenderingOnly)
        );
    }

    #[test]
    fn color_support_matches_each_decoders_accepted_photometrics() {
        let color = |uid: &str, bits: u32, photometric: &str| {
            let mut entry = file(uid);
            entry.bits_allocated = bits;
            entry.samples_per_pixel = 3;
            entry.photometric_interpretation = photometric.to_string();
            classify_pixel_support(&entry).state
        };
        // rle.rs and jpegxl.rs reject these, so they must not be advertised.
        assert_eq!(
            color("1.2.840.10008.1.2.5", 8, "YBR_FULL_422"),
            PixelSupportState::Unsupported
        );
        assert_eq!(
            color("1.2.840.10008.1.2.4.110", 8, "YBR_FULL"),
            PixelSupportState::Unsupported
        );
        // jpeg2000.rs renders three-component codestreams as RGB.
        for photometric in ["RGB", "YBR_RCT", "YBR_ICT"] {
            assert_eq!(
                color("1.2.840.10008.1.2.4.90", 8, photometric),
                PixelSupportState::Renderable,
                "{photometric}"
            );
        }
    }

    #[test]
    fn only_lossless_jpeg_xl_rgb_is_declared_renderable() {
        let mut lossless = file("1.2.840.10008.1.2.4.110");
        lossless.bits_allocated = 8;
        lossless.samples_per_pixel = 3;
        lossless.photometric_interpretation = "RGB".to_string();
        assert_eq!(
            classify_pixel_support(&lossless).state,
            PixelSupportState::Renderable
        );

        for uid in ["1.2.840.10008.1.2.4.111", "1.2.840.10008.1.2.4.112"] {
            let support = classify_pixel_support(&file(uid));
            assert_eq!(support.state, PixelSupportState::Unsupported, "{uid}");
            assert_eq!(support.reason, Some(PixelSupportReason::JpegXlNotSupported));
        }
    }

    #[test]
    fn baseline_jpeg_color_layouts_are_renderable_after_rgb_decode() {
        for photometric in ["RGB", "YBR_FULL", "YBR_FULL_422"] {
            let mut entry = file("1.2.840.10008.1.2.4.50");
            entry.bits_allocated = 8;
            entry.samples_per_pixel = 3;
            entry.photometric_interpretation = photometric.to_string();

            let support = classify_pixel_support(&entry);
            assert_eq!(
                support.state,
                PixelSupportState::Renderable,
                "{photometric}"
            );
            assert_eq!(support.reason, None, "{photometric}");
        }

        let mut palette = file("1.2.840.10008.1.2.4.50");
        palette.bits_allocated = 8;
        palette.photometric_interpretation = "PALETTE COLOR".to_string();
        assert_eq!(
            classify_pixel_support(&palette).reason,
            Some(PixelSupportReason::PaletteColorNotSupported)
        );
    }

    #[test]
    fn supported_uncompressed_color_layouts_are_renderable() {
        for (samples_per_pixel, photometric) in [
            (1, "PALETTE COLOR"),
            (3, "RGB"),
            (3, "YBR_FULL"),
            (3, "YBR_FULL_422"),
        ] {
            let mut entry = file("1.2.840.10008.1.2.1");
            entry.bits_allocated = 8;
            entry.samples_per_pixel = samples_per_pixel;
            entry.photometric_interpretation = photometric.to_string();

            let support = classify_pixel_support(&entry);
            assert_eq!(
                support.state,
                PixelSupportState::Renderable,
                "{photometric}"
            );
            assert_eq!(support.reason, None, "{photometric}");
        }
    }

    #[test]
    fn unsupported_numeric_layouts_have_pixel_layout_reasons() {
        for (bits_allocated, expected_reason, reason_id) in [
            (
                24,
                PixelSupportReason::NumericPrecisionNotSupported,
                "pixel_layout.numeric_precision_not_supported",
            ),
            (
                64,
                PixelSupportReason::NumericPrecisionNotSupported,
                "pixel_layout.numeric_precision_not_supported",
            ),
        ] {
            let mut entry = file("1.2.840.10008.1.2.1");
            entry.bits_allocated = bits_allocated;

            let support = classify_pixel_support(&entry);
            assert_eq!(support.state, PixelSupportState::Unsupported);
            assert_eq!(support.reason, Some(expected_reason));
            assert_eq!(support.reason_id(), Some(reason_id));
        }
    }

    #[test]
    fn supported_uncompressed_numeric_kinds_are_renderable() {
        for (kind, bits_allocated) in [
            (NativePixelDataKind::Integer, 1),
            (NativePixelDataKind::Integer, 32),
            (NativePixelDataKind::Float32, 32),
            (NativePixelDataKind::Float64, 64),
        ] {
            let mut entry = file("1.2.840.10008.1.2.1");
            entry.bits_allocated = bits_allocated;
            entry.series_metadata.native_pixel.pixel_data_kind = Some(kind);

            let support = classify_pixel_support(&entry);
            assert_eq!(support.state, PixelSupportState::Renderable, "{kind:?}");
            assert_eq!(support.reason, None, "{kind:?}");
        }
    }

    #[test]
    fn invalid_geometry_and_component_counts_are_layout_gaps() {
        let mut invalid_geometry = file("1.2.840.10008.1.2.1");
        invalid_geometry.rows = 0;
        assert_eq!(
            classify_pixel_support(&invalid_geometry).reason,
            Some(PixelSupportReason::InvalidGeometry)
        );

        let mut unsupported_components = file("1.2.840.10008.1.2.1");
        unsupported_components.samples_per_pixel = 4;
        assert_eq!(
            classify_pixel_support(&unsupported_components).reason,
            Some(PixelSupportReason::SamplesPerPixelNotSupported)
        );
    }

    #[test]
    fn state_names_are_stable() {
        assert_eq!(PixelSupportState::Renderable.as_str(), "renderable");
        assert_eq!(PixelSupportState::MetadataOnly.as_str(), "metadata_only");
        assert_eq!(PixelSupportState::Unsupported.as_str(), "unsupported");
    }
}

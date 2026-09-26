use crate::api::contracts::SupportState;
use crate::types::{FileEntry, NativePixelDataKind};

/// The decoder that handles one transfer syntax's pixel data.
///
/// This table is the single source for both frame dispatch (`service.rs`) and
/// the support state reported to clients (`classify_pixel_support`), so a
/// syntax cannot be advertised under one decoder and routed to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// Implicit/Explicit VR Little Endian, Explicit VR Big Endian, and
    /// Deflated Explicit VR Little Endian.
    Native,
    /// Deflated Image Frame Compression, limited to one-bit monochrome frames.
    DeflatedImageFrame,
    JpegBaseline,
    JpegLossless,
    /// Only the lossless JPEG 2000 process.
    Jpeg2000,
    /// Only the lossless JPEG-LS process.
    JpegLs,
    /// Only JPEG XL Lossless.
    JpegXl,
    Rle,
}

/// What a codec's decoded three-sample frame holds for a photometric
/// interpretation, which fixes the conversion to display RGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorSamples {
    /// Red, green, and blue samples; any YCbCr in the codestream has already
    /// been converted by the codec library.
    Rgb,
    /// Full-resolution Y, Cb, Cr samples still to be converted with the
    /// YBR_FULL equations of PS3.3 C.7.6.3.1.2.
    YbrFull,
}

pub fn codec_for_syntax(uid: &str) -> Option<Codec> {
    match uid {
        "1.2.840.10008.1.2"
        | "1.2.840.10008.1.2.1"
        | "1.2.840.10008.1.2.2"
        | "1.2.840.10008.1.2.1.99" => Some(Codec::Native),
        super::deflated_frame::DEFLATED_IMAGE_FRAME_UID => Some(Codec::DeflatedImageFrame),
        // Extended 12-bit (.51) stays a controlled unsupported syntax.
        "1.2.840.10008.1.2.4.50" => Some(Codec::JpegBaseline),
        "1.2.840.10008.1.2.4.57" | "1.2.840.10008.1.2.4.70" => Some(Codec::JpegLossless),
        "1.2.840.10008.1.2.4.90" => Some(Codec::Jpeg2000),
        "1.2.840.10008.1.2.4.80" => Some(Codec::JpegLs),
        "1.2.840.10008.1.2.4.110" => Some(Codec::JpegXl),
        "1.2.840.10008.1.2.5" => Some(Codec::Rle),
        _ => None,
    }
}

impl Codec {
    /// The three-sample layouts this codec displays, and what its decoder
    /// yields for each. Display decoders convert color through this table, so
    /// every layout it lists renders; a layout it omits is reported
    /// unsupported.
    pub(crate) fn color_samples(
        self,
        photometric: &str,
        bits_allocated: u32,
    ) -> Option<ColorSamples> {
        use ColorSamples::{Rgb, YbrFull};
        match (self, bits_allocated, photometric) {
            (Self::Native, 8, "RGB") => Some(Rgb),
            // native_layout.rs expands 4:2:2 chroma to full resolution first.
            (Self::Native, 8, "YBR_FULL" | "YBR_FULL_422") => Some(YbrFull),
            // The JPEG decoder applies the codestream's YCbCr transform itself.
            (Self::JpegBaseline, 8, "RGB" | "YBR_FULL" | "YBR_FULL_422") => Some(Rgb),
            // The lossless process has no color transform: jpeg-decoder
            // returns the stored components, so YBR_FULL is converted here.
            (Self::JpegLossless, 8, "RGB") => Some(Rgb),
            (Self::JpegLossless, 8, "YBR_FULL") => Some(YbrFull),
            // OpenJPEG applies the inverse RCT/ICT itself.
            (Self::Jpeg2000, 8 | 16, "RGB" | "YBR_RCT" | "YBR_ICT") => Some(Rgb),
            (Self::JpegXl, 8, "RGB") => Some(Rgb),
            (Self::Rle, 8, "RGB") => Some(Rgb),
            // PS3.5 Table 8.2.2-1 does not pair RLE with YBR_FULL_422, but files
            // transcoded from JPEG keep that label over full-resolution Y, Cb,
            // and Cr segments; the segment-length check rejects anything else.
            (Self::Rle, 8, "YBR_FULL" | "YBR_FULL_422") => Some(YbrFull),
            _ => None,
        }
    }

    /// Whether this codec displays PALETTE COLOR indices through the lookup tables.
    pub(crate) fn displays_palette(self, bits_allocated: u32) -> bool {
        matches!(self, Self::Native | Self::Rle) && bits_allocated == 8
    }

    fn supports_precision(self, kind: NativePixelDataKind, bits_allocated: u32) -> bool {
        match (self, kind) {
            (Self::Native, NativePixelDataKind::Integer) => {
                matches!(bits_allocated, 1 | 8 | 16 | 32)
            }
            (Self::Native, NativePixelDataKind::Float32) => bits_allocated == 32,
            (Self::Native, NativePixelDataKind::Float64) => bits_allocated == 64,
            (_, NativePixelDataKind::Integer) => matches!(bits_allocated, 8 | 16),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelSupportReason {
    PixelDataAbsentOrUnrecognized,
    JpegLsNotSupported,
    JpegXlNotSupported,
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
            Self::JpegLsNotSupported => "transfer_syntax.jpeg_ls_not_supported",
            Self::JpegXlNotSupported => "transfer_syntax.jpeg_xl_not_supported",
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
    pub state: SupportState,
    pub reason: Option<PixelSupportReason>,
}

impl PixelSupport {
    const fn renderable() -> Self {
        Self {
            state: SupportState::Renderable,
            reason: None,
        }
    }

    const fn metadata_only(reason: PixelSupportReason) -> Self {
        Self {
            state: SupportState::MetadataOnly,
            reason: Some(reason),
        }
    }

    const fn unsupported(reason: PixelSupportReason) -> Self {
        Self {
            state: SupportState::Unsupported,
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

/// Describe the viewer's display capability for a file from the codec table.
///
/// Semantic interpretation such as segmentation or parametric mapping is
/// separate.
pub fn classify_pixel_support(file: &FileEntry) -> PixelSupport {
    if !file.has_pixels {
        return PixelSupport::metadata_only(PixelSupportReason::PixelDataAbsentOrUnrecognized);
    }
    let Some(codec) = codec_for_syntax(&file.transfer_syntax_uid) else {
        return PixelSupport::unsupported(unsupported_transfer_syntax_reason(
            &file.transfer_syntax_uid,
        ));
    };

    if codec == Codec::DeflatedImageFrame {
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
        .unwrap_or(NativePixelDataKind::Integer);
    if !codec.supports_precision(pixel_kind, file.bits_allocated) {
        return PixelSupport::unsupported(if file.bits_allocated == 1 {
            PixelSupportReason::BitPackedPixelsNotSupported
        } else {
            PixelSupportReason::NumericPrecisionNotSupported
        });
    }

    let photometric = file.photometric_interpretation.trim().to_ascii_uppercase();
    match (file.samples_per_pixel, photometric.as_str()) {
        (1, "MONOCHROME1" | "MONOCHROME2") => PixelSupport::renderable(),
        (1, "PALETTE COLOR") if codec.displays_palette(file.bits_allocated) => {
            PixelSupport::renderable()
        }
        (1, "PALETTE COLOR") => {
            PixelSupport::unsupported(PixelSupportReason::PaletteColorNotSupported)
        }
        (3, color) if codec.color_samples(color, file.bits_allocated).is_some() => {
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

fn unsupported_transfer_syntax_reason(uid: &str) -> PixelSupportReason {
    match uid {
        "1.2.840.10008.1.2.4.81" => PixelSupportReason::JpegLsNotSupported,
        "1.2.840.10008.1.2.4.111" | "1.2.840.10008.1.2.4.112" => {
            PixelSupportReason::JpegXlNotSupported
        }
        _ => PixelSupportReason::TransferSyntaxNotSupported,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_pixel_support, PixelSupportReason, SupportState};
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
            assert_eq!(support.state, SupportState::Renderable, "{uid}");
            assert_eq!(support.reason_id(), None, "{uid}");
        }
    }

    #[test]
    fn absent_or_unrecognized_pixel_elements_are_metadata_only() {
        let mut entry = file("1.2.840.10008.1.2.1");
        entry.has_pixels = false;
        entry.bits_allocated = 64;

        let support = classify_pixel_support(&entry);
        assert_eq!(support.state, SupportState::MetadataOnly);
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
            assert_eq!(support.state, SupportState::Unsupported, "{uid}");
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
            SupportState::Renderable
        );

        entry.bits_allocated = 8;
        let unsupported = classify_pixel_support(&entry);
        assert_eq!(unsupported.state, SupportState::Unsupported);
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
            assert_eq!(support.state, SupportState::Renderable, "{photometric}");
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
        // RLE YBR_FULL_422 segments are full resolution and display as YBR_FULL.
        assert_eq!(
            color("1.2.840.10008.1.2.5", 8, "YBR_FULL_422"),
            SupportState::Renderable
        );
        assert_eq!(
            color("1.2.840.10008.1.2.5", 16, "YBR_FULL"),
            SupportState::Unsupported
        );
        assert_eq!(
            color("1.2.840.10008.1.2.4.70", 8, "YBR_FULL"),
            SupportState::Renderable
        );
        // jpegxl.rs rejects this, so it must not be advertised.
        assert_eq!(
            color("1.2.840.10008.1.2.4.110", 8, "YBR_FULL"),
            SupportState::Unsupported
        );
        // jpeg2000.rs renders three-component codestreams as RGB.
        for photometric in ["RGB", "YBR_RCT", "YBR_ICT"] {
            assert_eq!(
                color("1.2.840.10008.1.2.4.90", 8, photometric),
                SupportState::Renderable,
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
            SupportState::Renderable
        );

        for uid in ["1.2.840.10008.1.2.4.111", "1.2.840.10008.1.2.4.112"] {
            let support = classify_pixel_support(&file(uid));
            assert_eq!(support.state, SupportState::Unsupported, "{uid}");
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
            assert_eq!(support.state, SupportState::Renderable, "{photometric}");
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
            assert_eq!(support.state, SupportState::Renderable, "{photometric}");
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
            assert_eq!(support.state, SupportState::Unsupported);
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
            assert_eq!(support.state, SupportState::Renderable, "{kind:?}");
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
        let name = |state| serde_json::to_value(state).unwrap();
        assert_eq!(name(SupportState::Renderable), "renderable");
        assert_eq!(name(SupportState::MetadataOnly), "metadata_only");
        assert_eq!(name(SupportState::Unsupported), "unsupported");
    }
}

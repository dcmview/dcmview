use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use std::sync::Arc;
use tokio::task;

use super::codestream::{self, CodestreamKind, Structure};
use super::encapsulated::read_encapsulated_fragment_blocking;
use super::error::{PixelError, PixelResult};
use super::header::open_header;
use super::icc::select_icc_profile;
use super::reduced::{Reduction, ThumbnailRender};
use super::render::{
    render_windowed_luminance, DisplayBuffer, DisplayPng, LuminanceRenderOptions, StoredSamples,
};

pub(crate) async fn render_jp2_fragment(
    file: Arc<FileEntry>,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<DisplayBuffer> {
    task::spawn_blocking(move || {
        render_jp2_fragment_blocking(&file, frame, requested_wc, requested_ww, window_mode)
    })
    .await
    .context("jp2 fragment decode task failed")?
}

pub(crate) async fn decode_jp2_fragment_to_png(
    file: Arc<FileEntry>,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<DisplayPng> {
    task::spawn_blocking(move || {
        decode_jp2_fragment_to_png_blocking(&file, frame, requested_wc, requested_ww, window_mode)
    })
    .await
    .context("jp2 fragment decode task failed")?
}

/// Renders one JPEG 2000 frame for its thumbnail: its default presentation
/// in `window_mode`, decoded at a reduced resolution when `reduction` asks
/// for one and the frame allows it, in full otherwise.
///
/// The fragment is read from the file once and checked once
/// (`codestream::checked`) whichever decode follows; a frame the check
/// refuses is an error and reaches no decoder. Then:
///
/// 1. The reduction applied is `reduction.shift`, or the fewest
///    decomposition levels the codestream declares if that is less
///    (`Structure::Jpeg2000::levels`): the decoder can halve the image once
///    for each level of every tile and component and no further. With no
///    `reduction`, or no level, or an image that does not start at the
///    origin of its reference grid (`Structure::Jpeg2000::at_origin`; the
///    decoder does not place such an image correctly at a reduced
///    resolution), the frame is decoded in full.
/// 2. Otherwise [`decode_reduced`] is tried once. An image it returns is
///    rendered, and [`ThumbnailRender::shift`] says what was applied.
/// 3. When it returns nothing, the frame is decoded in full exactly as a
///    display frame is, after the reduced attempt has given back all it
///    held, and `shift` is 0.
///
/// So a frame costs at most one reduced decode and one full decode, never
/// both at once, and every frame that has a thumbnail from a full decode
/// has one here.
pub(crate) async fn render_jp2_thumbnail(
    file: Arc<FileEntry>,
    frame: u32,
    window_mode: WindowMode,
    reduction: Option<Reduction>,
) -> Result<ThumbnailRender> {
    task::spawn_blocking(move || {
        let fragment = read_encapsulated_fragment_blocking(&file.path, frame)?;
        let checked = checked_codestream(&file, &fragment)?;
        let (image, shift) = decode_for_thumbnail(&file, &checked, reduction, decode_reduced)?;
        let buffer = render_jp2_image(&file, frame, &image, None, None, window_mode)?;
        Ok(ThumbnailRender { buffer, shift })
    })
    .await
    .context("jp2 thumbnail decode task failed")?
}

/// Steps 1 to 3 of [`render_jp2_thumbnail`]: the image and the reduction
/// that was applied to it. `reduce` is [`decode_reduced`]; it is a
/// parameter so that the fallback can be shown for an attempt that fails.
fn decode_for_thumbnail(
    file: &FileEntry,
    checked: &CheckedCodestream<'_>,
    reduction: Option<Reduction>,
    reduce: impl FnOnce(&FileEntry, &CheckedCodestream<'_>, Reduction) -> Option<jpeg2k::Image>,
) -> Result<(jpeg2k::Image, u8)> {
    let shift = reduction.map_or(0, |reduction| reduction.shift.min(checked.levels));
    let reduced = if shift == 0 {
        None
    } else {
        reduce(file, checked, Reduction { shift })
    };
    Ok(match reduced {
        Some(image) => (image, shift),
        None => (decode_full(checked)?, 0),
    })
}

fn render_jp2_fragment_blocking(
    file: &FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<DisplayBuffer> {
    let fragment = read_encapsulated_fragment_blocking(&file.path, frame)?;

    let jp2_image = decode_checked(file, &fragment)?;

    render_jp2_image(
        file,
        frame,
        &jp2_image,
        requested_wc,
        requested_ww,
        window_mode,
    )
}

/// Presents a decoded JPEG 2000 image, at whatever resolution it was
/// decoded: the buffer has the image's own rows and columns.
fn render_jp2_image(
    file: &FileEntry,
    frame: u32,
    jp2_image: &jpeg2k::Image,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<DisplayBuffer> {
    let comps = jp2_image.components();
    if comps.is_empty() {
        return Err(anyhow!("JP2 image has no components"));
    }

    if comps.len() == 1 {
        // Grayscale shares the presentation pipeline used by every other
        // syntax: Modality LUT/rescale, VOI LUT/window, padding, inversion,
        // shutter, and overlays.
        let data = comps[0].data();
        let options = LuminanceRenderOptions {
            frame,
            rows: comps[0].height(),
            columns: comps[0].width(),
            requested_wc,
            requested_ww,
            window_mode,
        };
        // Components that fit the declared 8- or 16-bit layout take the
        // table path; anything else is windowed per sample, as before.
        if let Ok((bytes, bits_allocated)) = encode_raw_jp2_samples(
            data,
            comps[0].precision(),
            comps[0].is_signed(),
            file.pixel_representation,
        ) {
            return render_windowed_luminance(
                file,
                StoredSamples::Integer {
                    bytes: &bytes,
                    bits_allocated,
                    signed: file.pixel_representation == 1,
                },
                options,
            );
        }
        let stored_samples: Vec<f64> = data.iter().map(|&value| f64::from(value)).collect();
        render_windowed_luminance(file, StoredSamples::Values(&stored_samples), options)
    } else if comps.len() == 3 {
        // RGB — rare in medical imaging but handle it
        let width = comps[0].width();
        let height = comps[0].height();
        let precision = comps[0].precision();
        if precision <= 8 {
            let r = comps[0].data_u8();
            let g = comps[1].data_u8();
            let b = comps[2].data_u8();
            let interleaved: Vec<u8> = r
                .zip(g)
                .zip(b)
                .flat_map(|((rv, gv), bv)| [rv, gv, bv])
                .collect();
            // Only 8-bit color carries an ICC profile into the PNG.
            let icc_profile = select_icc_profile(&*open_header(&file.path)?);
            DisplayBuffer::rgb8(interleaved, width, height, icc_profile)
                .context("JP2 decode failed: png encoding failed")
        } else if precision <= 16 {
            let r = comps[0].data_u16();
            let g = comps[1].data_u16();
            let b = comps[2].data_u16();
            let interleaved: Vec<u16> = r
                .zip(g)
                .zip(b)
                .flat_map(|((rv, gv), bv)| [rv, gv, bv])
                .collect();
            let full_scale = u16::try_from((1_u32 << precision) - 1).unwrap_or(u16::MAX);
            DisplayBuffer::rgb16(interleaved, full_scale, width, height)
        } else {
            Err(anyhow!("unsupported JP2 component layout"))
        }
    } else {
        Err(anyhow!("unsupported JP2 component layout"))
    }
}

/// Decodes one frame's codestream once it is known to declare the entry's
/// image. The decoder sizes everything from the codestream, so this is the
/// only place a JPEG 2000 frame may be handed to it; a JP2 file is reduced
/// to its codestream first, so none of its other boxes takes part.
fn decode_checked(file: &FileEntry, fragment: &[u8]) -> Result<jpeg2k::Image> {
    decode_full(&checked_codestream(file, fragment)?)
}

/// The codestream of one frame that `codestream::checked` has accepted for
/// its entry. Only [`checked_codestream`] makes one, and the decoders take
/// nothing else, so no JPEG 2000 decode can skip the check.
struct CheckedCodestream<'a> {
    /// The bare codestream: the fragment, or the contents of the codestream
    /// box of a JP2 file.
    bytes: &'a [u8],
    /// How many times the decoder may halve the image:
    /// `Structure::Jpeg2000::levels`, the fewest decomposition levels any
    /// coding style of the codestream declares, or 0 for an image that is
    /// not `at_origin`.
    levels: u8,
}

fn checked_codestream<'a>(file: &FileEntry, fragment: &'a [u8]) -> Result<CheckedCodestream<'a>> {
    let declared = codestream::checked(file, CodestreamKind::Jpeg2000, fragment)?;
    let Structure::Jpeg2000 {
        codestream,
        levels,
        at_origin,
        ..
    } = declared.structure
    else {
        return Err(anyhow!("JP2 fragment was not read as JPEG 2000"));
    };
    let bytes = fragment
        .get(codestream)
        .context("JP2 codestream lies outside its fragment")?;
    Ok(CheckedCodestream {
        bytes,
        levels: if at_origin { levels } else { 0 },
    })
}

fn decode_full(checked: &CheckedCodestream<'_>) -> Result<jpeg2k::Image> {
    jpeg2k::Image::from_bytes(checked.bytes)
        .map_err(anyhow::Error::from)
        .context("failed to decode JP2 fragment")
}

/// Decodes `checked` with each axis divided by `2^reduction.shift`, or
/// returns `None` when that does not give `file`'s frame at that reduction.
///
/// The caller passes a shift of at least 1 and at most `checked.levels`.
/// The decoder is asked for it with
/// `jpeg2k::Image::from_bytes_with(checked.bytes,
/// jpeg2k::DecodeParameters::new().reduce(shift))`, the same decoder and
/// the same bytes as a full decode: it then stops the inverse wavelet
/// transform `shift` levels early, and allocates its sample buffers at the
/// reduced size. `None`, never an error and never a panic, when:
///
/// - the decoder returns an error (the caller decodes in full instead);
/// - the image does not have `file.samples_per_pixel` components;
/// - any component's width and height are not the same as the first's, or
///   are not accepted by [`Reduction::accepts`] for `file`;
/// - any component's sample count is not its width times its height.
///
/// The image is dropped before `None` is returned, so the full decode that
/// follows starts with nothing held.
fn decode_reduced(
    file: &FileEntry,
    checked: &CheckedCodestream<'_>,
    reduction: Reduction,
) -> Option<jpeg2k::Image> {
    let image = jpeg2k::Image::from_bytes_with(
        checked.bytes,
        jpeg2k::DecodeParameters::new().reduce(u32::from(reduction.shift)),
    )
    .ok()?;
    let components = image.components();
    if components.len() != usize::try_from(file.samples_per_pixel).ok()? {
        return None;
    }
    let first = components.first()?;
    let (width, height) = (first.width(), first.height());
    if !reduction.accepts(file, width, height) {
        return None;
    }
    let samples = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    if components.iter().any(|component| {
        component.width() != width
            || component.height() != height
            || component.data().len() != samples
    }) {
        return None;
    }
    Some(image)
}

fn decode_jp2_fragment_to_png_blocking(
    file: &FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<DisplayPng> {
    render_jp2_fragment_blocking(file, frame, requested_wc, requested_ww, window_mode)?
        .into_display_png(file, frame)
}

pub(crate) async fn decode_raw_jp2_samples(
    file: Arc<FileEntry>,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || decode_raw_jp2_samples_blocking(&file, frame))
        .await
        .map_err(|error| PixelError::raw_decode(anyhow!("raw JP2 decode task failed: {error}")))?
}

pub(super) fn decode_raw_jp2_samples_blocking(
    file: &FileEntry,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    let fragment =
        read_encapsulated_fragment_blocking(&file.path, frame).map_err(PixelError::raw_decode)?;

    decode_raw_fragment(file, &fragment)
}

pub(super) fn decode_raw_fragment(
    file: &FileEntry,
    fragment: &[u8],
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    let jp2_image = decode_checked(file, fragment).map_err(PixelError::raw_decode)?;

    let comps = jp2_image.components();
    if comps.is_empty() {
        return Err(PixelError::raw_decode(anyhow!(
            "JP2 image has no components"
        )));
    }

    // Only grayscale (single component) is supported for the raw path.
    if comps.len() != 1 {
        return Err(PixelError::UnsupportedLayout(format!(
            "raw JPEG 2000 requires one component, decoded {}",
            comps.len()
        )));
    }

    let width = comps[0].width();
    let height = comps[0].height();
    let precision = comps[0].precision();

    let (sample_bytes, bits_allocated) = encode_raw_jp2_samples(
        comps[0].data(),
        precision,
        comps[0].is_signed(),
        file.pixel_representation,
    )?;

    let metadata = file.raw_metadata(height, width, bits_allocated, 1);
    Ok((Bytes::from(sample_bytes), metadata))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RawJp2SampleLayout {
    Unsigned8,
    Signed8,
    Unsigned16,
    Signed16,
}

impl RawJp2SampleLayout {
    fn bits_allocated(self) -> u32 {
        match self {
            Self::Unsigned8 | Self::Signed8 => 8,
            Self::Unsigned16 | Self::Signed16 => 16,
        }
    }

    fn is_signed(self) -> bool {
        matches!(self, Self::Signed8 | Self::Signed16)
    }
}

fn raw_jp2_sample_layout(
    precision: u32,
    component_signed: bool,
    pixel_representation: u32,
) -> PixelResult<RawJp2SampleLayout> {
    if !(1..=16).contains(&precision) {
        return Err(PixelError::UnsupportedLayout(format!(
            "raw JPEG 2000 component precision {precision} is unsupported; expected 1-16 bits"
        )));
    }

    let dicom_signed = match pixel_representation {
        0 => false,
        1 => true,
        value => {
            return Err(PixelError::UnsupportedLayout(format!(
                "raw JPEG 2000 PixelRepresentation {value} is unsupported; expected 0 or 1"
            )));
        }
    };
    if component_signed != dicom_signed {
        let component_kind = if component_signed {
            "signed"
        } else {
            "unsigned"
        };
        let dicom_kind = if dicom_signed { "signed" } else { "unsigned" };
        return Err(PixelError::UnsupportedLayout(format!(
            "raw JPEG 2000 signedness mismatch: component is {component_kind}, \
             PixelRepresentation declares {dicom_kind}"
        )));
    }

    Ok(match (precision <= 8, component_signed) {
        (true, false) => RawJp2SampleLayout::Unsigned8,
        (true, true) => RawJp2SampleLayout::Signed8,
        (false, false) => RawJp2SampleLayout::Unsigned16,
        (false, true) => RawJp2SampleLayout::Signed16,
    })
}

fn encode_raw_jp2_samples(
    samples: &[i32],
    precision: u32,
    component_signed: bool,
    pixel_representation: u32,
) -> PixelResult<(Vec<u8>, u32)> {
    let layout = raw_jp2_sample_layout(precision, component_signed, pixel_representation)?;
    let (minimum, maximum) = if layout.is_signed() {
        let magnitude = 1_i32 << (precision - 1);
        (-magnitude, magnitude - 1)
    } else {
        (0, (1_i32 << precision) - 1)
    };

    let bytes_per_sample = (layout.bits_allocated() / 8) as usize;
    let mut encoded = Vec::with_capacity(samples.len().saturating_mul(bytes_per_sample));
    for &sample in samples {
        if !(minimum..=maximum).contains(&sample) {
            let sample_kind = if layout.is_signed() {
                "signed"
            } else {
                "unsigned"
            };
            return Err(PixelError::UnsupportedLayout(format!(
                "raw JPEG 2000 sample {sample} is outside the {sample_kind} {precision}-bit \
                 range {minimum}..={maximum}"
            )));
        }

        match layout {
            RawJp2SampleLayout::Unsigned8 => encoded.push(sample as u8),
            RawJp2SampleLayout::Signed8 => encoded.push((sample as i8) as u8),
            RawJp2SampleLayout::Unsigned16 => {
                encoded.extend_from_slice(&(sample as u16).to_le_bytes());
            }
            RawJp2SampleLayout::Signed16 => {
                encoded.extend_from_slice(&(sample as i16).to_le_bytes());
            }
        }
    }

    Ok((encoded, layout.bits_allocated()))
}

#[cfg(test)]
mod tests {
    use super::{
        checked_codestream, decode_for_thumbnail, decode_reduced, encode_raw_jp2_samples,
        raw_jp2_sample_layout, RawJp2SampleLayout,
    };
    use crate::pixels::reduced::Reduction;
    use crate::pixels::PixelError;
    use crate::types::FileEntry;

    /// A reduced attempt that returns nothing is followed by the full
    /// decode of the same checked codestream, and reported as a full
    /// decode. No frame an encoder writes makes the linked decoder fail
    /// reduced and succeed in full, so the attempt is replaced here.
    #[test]
    fn a_reduced_attempt_that_fails_is_followed_by_the_full_decode() {
        let frame = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/reduced-decode/gray8-512x384.j2k"),
        )
        .expect("the fixture");
        let file = FileEntry {
            format: Default::default(),
            raster: None,
            size_bytes: 0,
            modified: None,
            index: 0,
            path: "unread.dcm".into(),
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
            sop_class_uid: String::new(),
            series_metadata: Default::default(),
            has_pixels: true,
            frame_count: 1,
            rows: 384,
            columns: 512,
            bits_allocated: 8,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            transfer_syntax_uid: "1.2.840.10008.1.2.4.90".to_string(),
            default_window: None,
        };
        let checked = checked_codestream(&file, &frame).expect("an honest frame");
        let asked = Some(Reduction { shift: 1 });
        let size = |image: &jpeg2k::Image| (image.width(), image.height());

        let (image, shift) =
            decode_for_thumbnail(&file, &checked, asked, decode_reduced).expect("reduced");
        assert_eq!((size(&image), shift), ((256, 192), 1));

        let mut attempts = 0;
        let (image, shift) = decode_for_thumbnail(&file, &checked, asked, |_, _, reduction| {
            attempts += 1;
            assert_eq!(reduction.shift, 1);
            None
        })
        .expect("the full decode");
        assert_eq!((size(&image), shift, attempts), ((512, 384), 0, 1));
    }

    #[test]
    fn raw_sample_layout_accepts_supported_precision_and_signedness_boundaries() {
        let cases = [
            (1, false, 0, RawJp2SampleLayout::Unsigned8),
            (1, true, 1, RawJp2SampleLayout::Signed8),
            (8, false, 0, RawJp2SampleLayout::Unsigned8),
            (8, true, 1, RawJp2SampleLayout::Signed8),
            (9, false, 0, RawJp2SampleLayout::Unsigned16),
            (9, true, 1, RawJp2SampleLayout::Signed16),
            (16, false, 0, RawJp2SampleLayout::Unsigned16),
            (16, true, 1, RawJp2SampleLayout::Signed16),
        ];

        for (precision, component_signed, pixel_representation, expected) in cases {
            assert_eq!(
                raw_jp2_sample_layout(precision, component_signed, pixel_representation)
                    .expect("supported raw JPEG 2000 layout"),
                expected
            );
        }
    }

    #[test]
    fn raw_sample_layout_rejects_out_of_range_precisions() {
        for precision in [0, 17] {
            let error = raw_jp2_sample_layout(precision, false, 0)
                .expect_err("unsupported sample precision");
            assert!(
                matches!(error, PixelError::UnsupportedLayout(_)),
                "precision {precision} should be an unsupported layout: {error}"
            );
            assert!(
                error.to_string().contains(&precision.to_string()),
                "precision should appear in the error: {error}"
            );
        }
    }

    #[test]
    fn raw_sample_layout_rejects_invalid_or_mismatched_signedness() {
        for (component_signed, pixel_representation) in [(false, 1), (true, 0)] {
            let error = raw_jp2_sample_layout(9, component_signed, pixel_representation)
                .expect_err("signedness mismatch");
            assert!(
                matches!(error, PixelError::UnsupportedLayout(_)),
                "signedness mismatch should be an unsupported layout: {error}"
            );
            assert!(error.to_string().contains("signedness mismatch"));
        }

        let error = raw_jp2_sample_layout(9, false, 2).expect_err("invalid PixelRepresentation");
        assert!(
            matches!(error, PixelError::UnsupportedLayout(_)),
            "invalid PixelRepresentation should be an unsupported layout: {error}"
        );
        assert!(error.to_string().contains("PixelRepresentation 2"));
    }

    #[test]
    fn raw_unsigned_source_codes_remain_exact_at_9_and_16_bits() {
        let (nine_bit, bits_allocated) =
            encode_raw_jp2_samples(&[0, 1, 256, 511], 9, false, 0).expect("9-bit unsigned samples");
        assert_eq!(bits_allocated, 16);
        assert_eq!(nine_bit, [0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0xff, 0x01]);

        let (sixteen_bit, bits_allocated) =
            encode_raw_jp2_samples(&[0, 1, 32_768, 65_535], 16, false, 0)
                .expect("16-bit unsigned samples");
        assert_eq!(bits_allocated, 16);
        assert_eq!(
            sixteen_bit,
            [0x00, 0x00, 0x01, 0x00, 0x00, 0x80, 0xff, 0xff]
        );
    }

    #[test]
    fn raw_signed_source_codes_remain_exact_twos_complement_at_9_and_16_bits() {
        let (nine_bit, bits_allocated) =
            encode_raw_jp2_samples(&[-256, -1, 0, 255], 9, true, 1).expect("9-bit signed samples");
        assert_eq!(bits_allocated, 16);
        assert_eq!(nine_bit, [0x00, 0xff, 0xff, 0xff, 0x00, 0x00, 0xff, 0x00]);

        let (sixteen_bit, bits_allocated) =
            encode_raw_jp2_samples(&[-32_768, -1, 0, 32_767], 16, true, 1)
                .expect("16-bit signed samples");
        assert_eq!(bits_allocated, 16);
        assert_eq!(
            sixteen_bit,
            [0x00, 0x80, 0xff, 0xff, 0x00, 0x00, 0xff, 0x7f]
        );
    }

    #[test]
    fn raw_sample_conversion_rejects_values_outside_declared_precision() {
        let cases = [
            (&[-1][..], 9, false, 0),
            (&[512][..], 9, false, 0),
            (&[-257][..], 9, true, 1),
            (&[256][..], 9, true, 1),
        ];

        for (samples, precision, component_signed, pixel_representation) in cases {
            let error =
                encode_raw_jp2_samples(samples, precision, component_signed, pixel_representation)
                    .expect_err("sample outside declared range");
            assert!(
                matches!(error, PixelError::UnsupportedLayout(_)),
                "out-of-range sample should be an unsupported layout: {error}"
            );
            assert!(error.to_string().contains("outside"));
        }
    }
}

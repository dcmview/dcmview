use crate::api::contracts::RawFrameMetadata;
use crate::semantic::SegmentationOverlayPlan;
use bytes::Bytes;
use image::{ExtendedColorType, ImageEncoder};

use super::color::png_encoder;

use super::{PixelError, PixelResult};

/// Inspect the entire object on a discovery worker, holding at most one decoded
/// frame. A later fractional sample prevents fallback even on binary edge slices.
/// Decode failures must never turn a partially inspected object into a mask.
pub(crate) fn segmentation_has_only_binary_samples(
    file: &crate::types::FileEntry,
    object: &dicom_object::DefaultDicomObject,
    check_active: &impl Fn() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    use super::syntax::{codec_for_syntax, Codec};
    use anyhow::{ensure, Context};
    if !file.has_pixels
        || file.frame_count == 0
        || file.samples_per_pixel != 1
        || !matches!(file.bits_allocated, 1 | 8)
        || file.pixel_representation != 0
    {
        return Ok(false);
    }
    let codec = codec_for_syntax(&file.transfer_syntax_uid).context("unsupported SEG codec")?;
    let expected = usize::try_from(u64::from(file.rows) * u64::from(file.columns))?;
    ensure!(expected > 0, "empty SEG frame");
    if matches!(codec, Codec::Native) {
        return native_binary_samples(file, check_active);
    }
    let mut frames = super::encapsulated::EncapsulatedFrames::open(&file.path, check_active)?;
    let mut object = object.clone();
    // PixelDecoder builds presentation vectors from every functional group on
    // each call. This object is used only to decode stored samples; none of
    // those per-frame rescale/VOI vectors participates in the binary verdict.
    // Keep them out of the decoder view so encapsulated scans are linear too.
    object.remove_element(dicom_dictionary_std::tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE);
    object.put(dicom_core::DataElement::new(
        dicom_dictionary_std::tags::NUMBER_OF_FRAMES,
        dicom_core::VR::IS,
        "1",
    ));
    object.remove_element(dicom_dictionary_std::tags::EXTENDED_OFFSET_TABLE);
    object.remove_element(dicom_dictionary_std::tags::EXTENDED_OFFSET_TABLE_LENGTHS);
    for frame in 0..file.frame_count {
        check_active()?;
        let encoded = frames.read_frame(frame)?;
        if !matches!(codec, Codec::Rle | Codec::Jpeg2000) {
            object.put(dicom_core::DataElement::new(
                dicom_dictionary_std::tags::PIXEL_DATA,
                dicom_core::VR::OB,
                dicom_core::value::PixelFragmentSequence::new(
                    Vec::<u32>::new(),
                    vec![encoded.to_vec()],
                ),
            ));
        }
        let samples: Bytes = match codec {
            Codec::Native => unreachable!("native samples use the streaming path"),
            Codec::Raster => unreachable!("a transfer syntax never selects the raster codec"),
            Codec::Rle => super::rle::decode_fragment(file, &encoded)?.into(),
            Codec::Jpeg2000 => {
                let (bytes, metadata) = super::jpeg2000::decode_raw_fragment(file, &encoded)?;
                ensure!(
                    metadata.rows == file.rows
                        && metadata.columns == file.columns
                        && metadata.bits_allocated <= 8,
                    "unexpected decoded SEG layout"
                );
                bytes
            }
            Codec::DeflatedImageFrame => super::deflated_frame::decode_object(file, &object, 0)?
                .samples
                .into(),
            Codec::JpegBaseline | Codec::JpegLossless | Codec::JpegLs | Codec::JpegXl => {
                let decoded = super::pixeldata_frame::decode_object(file, &object, 0, "SEG")?;
                ensure!(
                    decoded.rows == file.rows
                        && decoded.columns == file.columns
                        && decoded.bits_allocated == 8
                        && decoded.samples_per_pixel == 1,
                    "unexpected decoded SEG layout"
                );
                decoded.bytes.into()
            }
        };
        ensure!(samples.len() == expected, "incomplete SEG frame");
        if samples.iter().any(|value| *value > 1) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Scan native samples as a bounded stream, including a single inflation for a
/// deflated dataset. Pixel padding beyond the declared frames is not a sample.
fn native_binary_samples(
    file: &crate::types::FileEntry,
    check_active: &impl Fn() -> anyhow::Result<()>,
) -> anyhow::Result<bool> {
    use anyhow::{ensure, Context};
    use dicom_dictionary_std::{tags, uids};
    use dicom_encoding::TransferSyntaxIndex;
    use dicom_object::FileMetaTable;
    use dicom_parser::dataset::{read::DataSetReader, DataToken};
    use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
    use std::io::{BufReader, Read, Seek, SeekFrom};
    let mut input = BufReader::new(std::fs::File::open(&file.path)?);
    input.seek(SeekFrom::Start(128))?;
    let meta = FileMetaTable::from_reader(&mut input)?;
    let syntax = TransferSyntaxRegistry
        .get(meta.transfer_syntax())
        .context("unknown SEG transfer syntax")?;
    let mut stream: Box<dyn Read> =
        if file.transfer_syntax_uid == uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN {
            Box::new(flate2::read::DeflateDecoder::new(input))
        } else {
            Box::new(input)
        };
    let available = {
        let tokens = DataSetReader::new_with_ts(&mut stream, syntax)?;
        let mut depth = 0_usize;
        let mut available = None;
        for token in tokens {
            check_active()?;
            match token? {
                DataToken::SequenceStart { .. } | DataToken::PixelSequenceStart => depth += 1,
                DataToken::SequenceEnd => depth = depth.saturating_sub(1),
                DataToken::ElementHeader(header)
                    if depth == 0 && header.tag == tags::PIXEL_DATA =>
                {
                    available = header.len.get();
                    break;
                }
                _ => {}
            }
        }
        u64::from(available.context("missing native SEG pixel data")?)
    };
    let samples = u64::from(file.rows)
        .checked_mul(u64::from(file.columns))
        .and_then(|n| n.checked_mul(u64::from(file.frame_count)))
        .context("SEG sample count overflow")?;
    let mut remaining = if file.bits_allocated == 1 {
        samples.div_ceil(8)
    } else {
        samples
    };
    ensure!(remaining <= available, "incomplete SEG samples");
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        check_active()?;
        let length = remaining.min(buffer.len() as u64) as usize;
        let bytes = &mut buffer[..length];
        stream.read_exact(bytes).context("truncated SEG samples")?;
        if file.bits_allocated == 8 {
            super::stored_bits::canonicalize_integer_samples(
                bytes,
                8,
                file.series_metadata.native_pixel.bits_stored,
                file.series_metadata.native_pixel.high_bit,
                false,
            )?;
            if bytes.iter().any(|value| *value > 1) {
                return Ok(false);
            }
        }
        remaining -= length as u64;
    }
    Ok(true)
}

pub fn encode_segmentation_overlay_png(
    samples: &Bytes,
    metadata: &RawFrameMetadata,
    plan: &SegmentationOverlayPlan,
    target_rows: u32,
    target_columns: u32,
) -> PixelResult<Bytes> {
    if metadata.samples_per_pixel != 1 || metadata.bits_allocated > 8 {
        return Err(PixelError::UnsupportedLayout(
            "SEG overlay requires one expanded 8-bit-or-smaller sample per pixel".to_string(),
        ));
    }
    let sample_count = usize::try_from(u64::from(metadata.rows) * u64::from(metadata.columns))
        .map_err(|_| PixelError::UnsupportedLayout("SEG frame dimensions overflow".to_string()))?;
    if samples.len() != sample_count {
        return Err(PixelError::UnsupportedLayout(format!(
            "SEG raw frame has {} bytes for {sample_count} pixels",
            samples.len()
        )));
    }
    let output_len = usize::try_from(
        u64::from(target_rows)
            .checked_mul(u64::from(target_columns))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                PixelError::UnsupportedLayout("overlay dimensions overflow".to_string())
            })?,
    )
    .map_err(|_| PixelError::UnsupportedLayout("overlay dimensions overflow".to_string()))?;
    let mut rgba = vec![0_u8; output_len];
    let maximum_fractional = match plan.segmentation_type.as_str() {
        "BINARY" => None,
        "FRACTIONAL" => Some(
            plan.maximum_fractional_value
                .filter(|value| *value > 0 && *value <= 255)
                .ok_or_else(|| {
                    PixelError::UnsupportedLayout(
                        "fractional SEG overlay requires Maximum Fractional Value in 1..=255"
                            .to_string(),
                    )
                })?,
        ),
        other => {
            return Err(PixelError::UnsupportedLayout(format!(
                "SEG overlay does not support segmentation type {other}"
            )))
        }
    };

    for target_row in 0..target_rows {
        for target_column in 0..target_columns {
            let [source_row, source_column] = plan
                .target_to_segmentation
                .map(f64::from(target_row), f64::from(target_column));
            let source_row = source_row.round();
            let source_column = source_column.round();
            if source_row < 0.0
                || source_column < 0.0
                || source_row >= f64::from(metadata.rows)
                || source_column >= f64::from(metadata.columns)
            {
                continue;
            }
            let source_index =
                source_row as usize * metadata.columns as usize + source_column as usize;
            let value = samples[source_index];
            let alpha = maximum_fractional.map_or_else(
                || if value == 0 { 0 } else { 178 },
                |maximum| ((u32::from(value).min(maximum) * 204) / maximum) as u8,
            );
            if alpha == 0 {
                continue;
            }
            let target_index =
                (target_row as usize * target_columns as usize + target_column as usize) * 4;
            rgba[target_index..target_index + 3].copy_from_slice(&plan.color);
            rgba[target_index + 3] = alpha;
        }
    }

    let mut encoded = Vec::new();
    png_encoder(&mut encoded)
        .write_image(&rgba, target_columns, target_rows, ExtendedColorType::Rgba8)
        .map_err(|error| PixelError::frame_decode(error.into()))?;
    Ok(Bytes::from(encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::PixelAffineTransform;

    fn metadata(rows: u32, columns: u32) -> RawFrameMetadata {
        RawFrameMetadata {
            rows,
            columns,
            bits_allocated: 1,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            default_wc: None,
            default_ww: None,
            padding_low: None,
            padding_high: None,
        }
    }

    fn plan(transform: PixelAffineTransform) -> SegmentationOverlayPlan {
        SegmentationOverlayPlan {
            segmentation_file_index: 0,
            segmentation_frame_index: 0,
            source_file_index: 1,
            source_frame_index: 0,
            target_to_segmentation: transform,
            segmentation_type: "BINARY".to_string(),
            maximum_fractional_value: None,
            color: [255, 79, 132],
        }
    }

    #[test]
    fn resamples_binary_mask_into_target_grid_with_transparency() {
        let transform = PixelAffineTransform {
            source_origin: [0.0, 0.0],
            source_step_for_target_row: [1.0, 0.0],
            source_step_for_target_column: [0.0, 1.0],
        };
        let png = encode_segmentation_overlay_png(
            &Bytes::from_static(&[0, 1, 1, 0]),
            &metadata(2, 2),
            &plan(transform),
            2,
            2,
        )
        .expect("overlay PNG");
        let decoded = image::load_from_memory(&png)
            .expect("decode PNG")
            .to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(decoded.get_pixel(1, 0).0, [255, 79, 132, 178]);
        assert_eq!(decoded.get_pixel(0, 1).0, [255, 79, 132, 178]);
    }
}

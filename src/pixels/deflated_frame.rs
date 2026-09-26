use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use dicom_object::open_file;
use dicom_pixeldata::PixelDecoder;
use tokio::task;

use super::error::{PixelError, PixelResult};
use super::render::{encode_windowed_luminance_png, LuminanceRenderOptions};

pub(crate) const DEFLATED_IMAGE_FRAME_UID: &str = "1.2.840.10008.1.2.8.1";

struct DecodedBinaryFrame {
    samples: Vec<u8>,
    rows: u32,
    columns: u32,
}

pub(crate) async fn decode_deflated_binary_frame_to_png(
    file: FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> PixelResult<Bytes> {
    validate_binary_layout(&file)
        .map_err(|error| PixelError::UnsupportedLayout(error.to_string()))?;
    task::spawn_blocking(move || {
        let decoded = decode_binary_frame(&file, frame).map_err(PixelError::frame_decode)?;
        let samples = decoded
            .samples
            .iter()
            .map(|sample| f64::from(*sample))
            .collect::<Vec<_>>();
        encode_windowed_luminance_png(
            &file,
            &samples,
            LuminanceRenderOptions {
                frame,
                rows: decoded.rows,
                columns: decoded.columns,
                requested_wc,
                requested_ww,
                window_mode,
            },
        )
        .map_err(PixelError::frame_decode)
    })
    .await
    .map_err(|error| {
        PixelError::frame_decode(anyhow!("Deflated Image Frame decode task failed: {error}"))
    })?
}

pub(crate) async fn decode_raw_deflated_binary_frame(
    file: FileEntry,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    validate_binary_layout(&file)
        .map_err(|error| PixelError::UnsupportedLayout(error.to_string()))?;
    task::spawn_blocking(move || {
        let decoded = decode_binary_frame(&file, frame).map_err(PixelError::raw_decode)?;
        let metadata = file.raw_metadata(decoded.rows, decoded.columns, 1, 1);
        // Match the native one-bit raw contract: BitsAllocated remains 1,
        // while each response byte is one canonical decoded sample (0 or 1).
        Ok((Bytes::from(decoded.samples), metadata))
    })
    .await
    .map_err(|error| {
        PixelError::raw_decode(anyhow!(
            "raw Deflated Image Frame decode task failed: {error}"
        ))
    })?
}

fn decode_binary_frame(file: &FileEntry, frame: u32) -> Result<DecodedBinaryFrame> {
    let object = open_file(&file.path).with_context(|| {
        format!(
            "failed to open Deflated Image Frame DICOM: {}",
            file.path.display()
        )
    })?;
    let decoded = object
        .decode_pixel_data_frame(frame)
        .context("Deflated Image Frame adapter decode failed")?;

    if decoded.rows() != file.rows || decoded.columns() != file.columns {
        return Err(anyhow!(
            "decoded Deflated Image Frame geometry {}x{} does not match catalog {}x{}",
            decoded.columns(),
            decoded.rows(),
            file.columns,
            file.rows
        ));
    }
    if decoded.bits_allocated() != 1 || decoded.samples_per_pixel() != 1 {
        return Err(anyhow!(
            "decoded Deflated Image Frame layout requires one one-bit sample per pixel"
        ));
    }

    // dicom-pixeldata's adapter output is the native bit-packed frame. Avoid
    // DecodedPixelData::frame_data here: it currently sizes one-bit frames as
    // one byte per sample and rejects the correctly packed adapter buffer.
    let pixel_count = usize::try_from(decoded.rows())?
        .checked_mul(usize::try_from(decoded.columns())?)
        .context("Deflated Image Frame geometry overflow")?;
    let packed = decoded.data().to_vec();
    let samples = unpack_one_bit_frame(&packed, pixel_count)?;
    Ok(DecodedBinaryFrame {
        samples,
        rows: decoded.rows(),
        columns: decoded.columns(),
    })
}

fn validate_binary_layout(file: &FileEntry) -> Result<()> {
    if file.transfer_syntax_uid != DEFLATED_IMAGE_FRAME_UID {
        return Err(anyhow!(
            "unexpected transfer syntax {} for Deflated Image Frame decoder",
            file.transfer_syntax_uid
        ));
    }
    if file.bits_allocated != 1 || file.samples_per_pixel != 1 || file.pixel_representation != 0 {
        return Err(anyhow!(
            "Deflated Image Frame viewer path requires unsigned one-bit single-sample pixels"
        ));
    }
    if !matches!(
        file.photometric_interpretation.trim(),
        "MONOCHROME1" | "MONOCHROME2"
    ) {
        return Err(anyhow!(
            "Deflated Image Frame viewer path does not support PhotometricInterpretation {}",
            file.photometric_interpretation
        ));
    }
    Ok(())
}

fn unpack_one_bit_frame(packed: &[u8], pixel_count: usize) -> Result<Vec<u8>> {
    let expected_len = pixel_count
        .checked_add(7)
        .map(|bits| bits / 8)
        .context("Deflated Image Frame size overflow")?;
    if packed.len() != expected_len {
        return Err(anyhow!(
            "decoded Deflated Image Frame length {} does not match expected packed length {expected_len}",
            packed.len()
        ));
    }
    Ok(packed
        .iter()
        .flat_map(|byte| (0..8).map(move |bit| (byte >> bit) & 1))
        .take(pixel_count)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::unpack_one_bit_frame;
    use crate::api::contracts::WindowMode;
    use crate::loader::test_entry;
    use crate::pixels::{
        load_frame, load_raw_frame, new_cache, new_raw_cache, FrameRequest, RawFrameRequest,
    };

    #[test]
    fn raw_one_bit_contract_expands_lsb_first_samples_to_one_byte_each() {
        assert_eq!(unpack_one_bit_frame(&[0b1001], 4).unwrap(), [1, 0, 0, 1]);
        assert_eq!(unpack_one_bit_frame(&[0b0110], 4).unwrap(), [0, 1, 1, 0]);
        assert!(unpack_one_bit_frame(&[], 4).is_err());
        assert!(unpack_one_bit_frame(&[0, 0], 4).is_err());
    }

    #[tokio::test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    async fn prepared_two_frame_seg_matches_packed_hash_inputs_and_display_masks() {
        let root = std::env::var_os("DCMVIEW_PREPARED_CORPUS")
            .map(std::path::PathBuf::from)
            .expect("set DCMVIEW_PREPARED_CORPUS to the generated suite directory");
        let path = root
            .join("extended-deflate")
            .join("derived/seg/binary_multiframe_deflated_image_frame/instance.dcm");
        let file = test_entry(&path);

        let raw_cache = new_raw_cache();
        let display_cache = new_cache();
        for (frame, samples, pixels) in [
            (0, [1_u8, 0, 0, 1], [255_u8, 0, 0, 255]),
            (1, [0_u8, 1, 1, 0], [0_u8, 255, 255, 0]),
        ] {
            let raw = load_raw_frame(file.clone(), raw_cache.clone(), RawFrameRequest { frame })
                .await
                .expect("decode prepared raw frame");
            assert_eq!(raw.body.as_ref(), samples);
            assert_eq!(raw.metadata.bits_allocated, 1);

            let display = load_frame(
                file.clone(),
                display_cache.clone(),
                FrameRequest {
                    frame,
                    window_center: None,
                    window_width: None,
                    window_mode: WindowMode::Default,
                },
            )
            .await
            .expect("render prepared SEG frame");
            let rendered = image::load_from_memory(&display.body)
                .expect("decode display PNG")
                .to_luma8();
            assert_eq!(rendered.into_raw(), pixels);
        }
    }
}

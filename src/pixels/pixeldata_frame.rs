//! Frames decoded by dicom-pixeldata: JPEG Baseline and Lossless, JPEG-LS
//! and JPEG XL. Each codec's display and raw paths share this decode and
//! differ only in how they present the samples.

use crate::types::FileEntry;
use anyhow::{Context, Result};
use dicom_pixeldata::PixelDecoder;

use super::encapsulated::open_for_frame_decode;
use super::icc::select_icc_profile;

/// One decoded frame, color-by-pixel, samples little endian.
pub(super) struct DecodedFrame {
    pub(super) bytes: Vec<u8>,
    pub(super) rows: u32,
    pub(super) columns: u32,
    pub(super) bits_allocated: u32,
    pub(super) samples_per_pixel: u32,
    pub(super) icc_profile: Option<Vec<u8>>,
}

/// Decodes only `frame` of `file`; `codec` names the codec in errors.
pub(super) fn decode_frame(file: &FileEntry, frame: u32, codec: &str) -> Result<DecodedFrame> {
    let (object, frame_in_object) = open_for_frame_decode(file, frame)?;
    // The result holds just the requested frame, at index 0.
    let decoded = object
        .decode_pixel_data_frame(frame_in_object)
        .with_context(|| format!("{codec} frame decode failed"))?;
    let bits_allocated = u32::from(decoded.bits_allocated());
    let frame_bytes = decoded
        .frame_data(0)
        .with_context(|| format!("decoded {codec} frame is incomplete"))?;
    // dicom-pixeldata hands back samples in host byte order.
    let bytes = if bits_allocated == 16 {
        frame_bytes
            .chunks_exact(2)
            .flat_map(|sample| u16::from_ne_bytes([sample[0], sample[1]]).to_le_bytes())
            .collect()
    } else {
        frame_bytes.to_vec()
    };
    Ok(DecodedFrame {
        bytes,
        rows: decoded.rows(),
        columns: decoded.columns(),
        bits_allocated,
        samples_per_pixel: u32::from(decoded.samples_per_pixel()),
        icc_profile: select_icc_profile(&object),
    })
}

//! Frames decoded by dicom-pixeldata: JPEG Baseline and Lossless, JPEG-LS
//! and JPEG XL. Each codec's display and raw paths share this decode and
//! differ only in how they present the samples.

use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use dicom_dictionary_std::tags;
use dicom_pixeldata::PixelDecoder;

use super::codestream::{self, CodestreamKind};
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
    decode_object(file, &object, frame_in_object, codec)
}

/// Decodes the one frame `object` holds, as its only fragment.
///
/// The adapters and the codec libraries behind them size their buffers from
/// the data set and from the codestream, so nothing reaches them until the
/// data set is known to say what the entry says and the codestream is known
/// to declare the entry's image (`codestream::checked`). That also makes
/// the adapters' own copies safe: the JPEG adapter copies what its decoder
/// returns into a buffer sized from the data set, and the two now have the
/// same length.
pub(super) fn decode_object(
    file: &FileEntry,
    object: &dicom_object::DefaultDicomObject,
    frame_in_object: u32,
    codec: &str,
) -> Result<DecodedFrame> {
    let kind = codestream::kind_of(file)
        .with_context(|| format!("{codec} frames carry no codestream header"))?;
    codestream::header_agrees(file, object)?;
    let fragments = object
        .element(tags::PIXEL_DATA)
        .ok()
        .and_then(|element| element.fragments())
        .with_context(|| format!("{codec} pixel data is not encapsulated"))?;
    let [encoded] = fragments else {
        return Err(anyhow!(
            "{codec} frame was not prepared as a single fragment"
        ));
    };
    if frame_in_object != 0 {
        return Err(anyhow!("{codec} frame was not prepared as frame 0"));
    }
    codestream::checked(file, kind, encoded)?;
    if kind == CodestreamKind::JpegXl {
        // Decoded here, not by the adapter: only this decoder can be told
        // how much it may allocate.
        let bytes = codestream::decode_jpeg_xl(file, encoded)
            .with_context(|| format!("{codec} frame decode failed"))?;
        return Ok(DecodedFrame {
            bytes,
            rows: file.rows,
            columns: file.columns,
            bits_allocated: file.bits_allocated,
            samples_per_pixel: file.samples_per_pixel,
            icc_profile: select_icc_profile(object),
        });
    }
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
        icc_profile: select_icc_profile(object),
    })
}

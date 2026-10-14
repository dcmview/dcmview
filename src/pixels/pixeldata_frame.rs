//! Frames decoded by dicom-pixeldata: JPEG Baseline and Lossless, JPEG-LS
//! and JPEG XL. Each codec's display and raw paths share this decode and
//! differ only in how they present the samples.

use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use dicom_dictionary_std::tags;
use dicom_pixeldata::PixelDecoder;

use super::codestream::{self, CodestreamKind, Declared, Structure};
use super::encapsulated::open_for_frame_decode;
use super::icc::select_icc_profile;
use super::reduced::{Reduction, JPEG_MAX_REDUCTION_SHIFT};

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

/// Decodes only `frame` of `file` for its thumbnail: at a reduced
/// resolution when `reduction` asks for one and the frame allows it, in
/// full otherwise. Returns the frame and the reduction that was applied (0
/// for a full decode); a reduced frame's `rows` and `columns` are its own.
///
/// The frame is read from the file once and checked once
/// ([`checked_fragment`]) whichever decode follows; a frame the check
/// refuses is an error and reaches no decoder. With a `reduction`,
/// [`decode_scaled_jpeg`] is tried once; when it returns nothing the frame
/// is decoded in full exactly as [`decode_object`] does, after the reduced
/// attempt has given back all it held. So a frame costs at most one reduced
/// decode and one full decode, never both at once.
pub(super) fn decode_frame_for_thumbnail(
    file: &FileEntry,
    frame: u32,
    codec: &str,
    reduction: Option<Reduction>,
) -> Result<(DecodedFrame, u8)> {
    let (object, frame_in_object) = open_for_frame_decode(file, frame)?;
    let checked = checked_fragment(file, &object, frame_in_object, codec)?;
    if let Some(reduction) = reduction {
        if let Some((bytes, columns, rows)) = decode_scaled_jpeg(file, &checked, reduction) {
            return Ok((
                DecodedFrame {
                    bytes,
                    rows,
                    columns,
                    bits_allocated: file.bits_allocated,
                    samples_per_pixel: file.samples_per_pixel,
                    icc_profile: None,
                },
                reduction.shift,
            ));
        }
    }
    decode_checked_object(file, &object, &checked, frame_in_object, codec).map(|frame| (frame, 0))
}

/// One frame's encoded bytes, once the data set that holds them is known to
/// say what the entry says and the codestream is known to declare the
/// entry's image. Only [`checked_fragment`] makes one, and the decoders
/// below take nothing else.
struct CheckedFragment<'a> {
    kind: CodestreamKind,
    encoded: &'a [u8],
    declared: Declared,
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
    let checked = checked_fragment(file, object, frame_in_object, codec)?;
    decode_checked_object(file, object, &checked, frame_in_object, codec)
}

/// The checks every decode of `object`'s one frame starts with:
/// `codestream::header_agrees` for the data set, then `codestream::checked`
/// for its only fragment.
fn checked_fragment<'a>(
    file: &FileEntry,
    object: &'a dicom_object::DefaultDicomObject,
    frame_in_object: u32,
    codec: &str,
) -> Result<CheckedFragment<'a>> {
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
    let declared = codestream::checked(file, kind, encoded)?;
    Ok(CheckedFragment {
        kind,
        encoded,
        declared,
    })
}

/// Decodes a JPEG frame with its blocks inverted at `1 / 2^reduction.shift`
/// of their size, or returns `None` when that does not give `file`'s frame
/// at that reduction. On success: the samples, color-by-pixel and one byte
/// each, then the columns and rows of the reduced frame.
///
/// `None`, never an error and never a panic, unless all of these hold:
///
/// 1. `checked.declared.structure` is `Structure::Jpeg` with `process`
///    `0xC0`, `0xC1` or `0xC2` (the DCT processes; the lossless process has
///    no blocks to scale), and `reduction.shift` is 1, 2 or 3
///    (`JPEG_MAX_REDUCTION_SHIFT`).
/// 2. `columns = ceil(file.columns / 2^shift)` and
///    `rows = ceil(file.rows / 2^shift)` each fit 16 bits, and
///    `columns * rows * file.samples_per_pixel` does not overflow.
/// 3. `jpeg_decoder::Decoder::new(checked.encoded)`, the decoder and the
///    bytes a full decode uses, is asked for that size
///    (`scale(columns, rows)`) and answers exactly `(columns, rows)`: the
///    decoder picks the scale itself, and for some sizes it picks another.
///    It is also told that many bytes of output at most
///    (`set_max_decoding_buffer_size`), which is a second check on the
///    result and not what bounds the decode: the decoder consults it only
///    when it assembles its output, after its planes are allocated. Those
///    are sized from the frame header, which `checked_fragment` has held to
///    the entry.
/// 4. `decode()` succeeds and returns exactly
///    `columns * rows * file.samples_per_pixel` bytes.
///
/// The samples are what a full decode returns for the same frame, at the
/// reduced size: the decoder applies the same colour transform either way.
/// Whatever the decoder holds is dropped before `None` is returned, so the
/// full decode that follows starts with nothing held.
fn decode_scaled_jpeg(
    file: &FileEntry,
    checked: &CheckedFragment<'_>,
    reduction: Reduction,
) -> Option<(Vec<u8>, u32, u32)> {
    if !matches!(
        checked.declared.structure,
        Structure::Jpeg {
            process: 0xC0..=0xC2,
            ..
        }
    ) || !(1..=JPEG_MAX_REDUCTION_SHIFT).contains(&reduction.shift)
    {
        return None;
    }
    let divisor = 1_u64 << reduction.shift;
    let columns = u16::try_from(u64::from(file.columns).div_ceil(divisor)).ok()?;
    let rows = u16::try_from(u64::from(file.rows).div_ceil(divisor)).ok()?;
    let bytes = usize::from(columns)
        .checked_mul(usize::from(rows))?
        .checked_mul(usize::try_from(file.samples_per_pixel).ok()?)?;
    let mut decoder = jpeg_decoder::Decoder::new(checked.encoded);
    decoder.set_max_decoding_buffer_size(bytes);
    if decoder.scale(columns, rows).ok()? != (columns, rows) {
        return None;
    }
    let decoded = decoder.decode().ok()?;
    if decoded.len() != bytes {
        return None;
    }
    Some((decoded, u32::from(columns), u32::from(rows)))
}

fn decode_checked_object(
    file: &FileEntry,
    object: &dicom_object::DefaultDicomObject,
    checked: &CheckedFragment<'_>,
    frame_in_object: u32,
    codec: &str,
) -> Result<DecodedFrame> {
    let (kind, encoded) = (checked.kind, checked.encoded);
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

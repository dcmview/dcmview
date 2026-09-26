use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::{FileEntry, NativePixelDataKind};
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use dicom_dictionary_std::uids;
use dicom_encoding::TransferSyntaxIndex;
use dicom_object::{open_file, FileMetaTable};
use dicom_parser::dataset::lazy_read::LazyDataSetReader;
use dicom_parser::dataset::LazyDataToken;
use dicom_parser::StatefulDecode;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use image::{ImageBuffer, ImageFormat, Luma};
use std::fs::File;
use std::io::{BufReader, Cursor, Seek, SeekFrom};
use tokio::task;

use super::color::{encode_rgb8_png_with_icc, rgb8_interleaved, ybr_full_to_rgb8};
use super::header::open_header;
use super::icc::select_icc_profile;
use super::native_layout::{native_pixel_element_tag, NativeByteOrder, NativeFrameLayout};
use super::overlay::apply_overlay_planes;
use super::palette::palette_indices_to_rgb8;
use super::render::apply_monochrome1_inversion;
use super::shutter::apply_rectangular_shutter;
use super::stored_bits::canonicalize_integer_samples;
use super::window::{
    apply_modality_transform, apply_padding_background, apply_voi_lut_if_selected, apply_window,
    exclude_padding_samples, read_pixel_padding_range, resolve_window_with_mode,
};

pub(crate) async fn decode_uncompressed_to_png(
    file: FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<Bytes> {
    task::spawn_blocking(move || {
        decode_uncompressed_to_png_blocking(&file, frame, requested_wc, requested_ww, window_mode)
    })
    .await
    .context("uncompressed decode task failed")?
}

fn decode_uncompressed_to_png_blocking(
    file: &FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<Bytes> {
    let object = open_header(&file.path)?;

    let rows = file.rows;
    let columns = file.columns;
    let samples_per_pixel = file.samples_per_pixel.max(1);
    let bits_allocated = file.bits_allocated;
    let mut frame_bytes = read_native_frame(file, frame)?.display_frame()?;
    if native_pixel_data_kind(file) == NativePixelDataKind::Integer && bits_allocated > 1 {
        canonicalize_integer_samples(
            &mut frame_bytes,
            bits_allocated,
            file.series_metadata.native_pixel.bits_stored,
            file.series_metadata.native_pixel.high_bit,
            file.pixel_representation == 1,
        )
        .context("frame decode failed: invalid native stored-bit layout")?;
    }
    let pixel_count = usize::try_from(rows)
        .ok()
        .and_then(|rows| {
            usize::try_from(columns)
                .ok()
                .and_then(|columns| rows.checked_mul(columns))
        })
        .ok_or_else(|| anyhow!("frame decode failed: invalid image geometry"))?;
    let photometric = file.photometric_interpretation.trim().to_ascii_uppercase();
    if bits_allocated == 8 {
        let rgb = match (samples_per_pixel, photometric.as_str()) {
            (3, "RGB") => Some(rgb8_interleaved(&frame_bytes, pixel_count, 0)?),
            (3, "YBR_FULL" | "YBR_FULL_422") => {
                Some(ybr_full_to_rgb8(&frame_bytes, pixel_count, 0)?)
            }
            (1, "PALETTE COLOR") => Some(palette_indices_to_rgb8(
                &file.path,
                &frame_bytes,
                bits_allocated,
            )?),
            _ => None,
        };
        if let Some(rgb) = rgb {
            let icc_profile = select_icc_profile(&object);
            return encode_rgb8_png_with_icc(rgb, columns, rows, icc_profile)
                .context("frame decode failed: color PNG encoding failed");
        }
    }
    if samples_per_pixel != 1 || !matches!(photometric.as_str(), "MONOCHROME1" | "MONOCHROME2") {
        return Err(anyhow!(
            "frame decode failed: unsupported native layout SamplesPerPixel {samples_per_pixel}, PhotometricInterpretation {}",
            file.photometric_interpretation
        ));
    }

    let signed = file.pixel_representation == 1;
    // dicom-object normalizes primitive pixel bytes to host order for native pixel data.
    // Decode from the normalized byte representation directly.
    let raw_samples = decode_numeric_samples(
        &frame_bytes,
        bits_allocated,
        signed,
        false,
        native_pixel_data_kind(file),
    )?;
    let padding_mask = read_pixel_padding_range(&object, native_pixel_data_kind(file))
        .map(|padding| padding.mask(&raw_samples));
    let rescaled = apply_modality_transform(
        &raw_samples,
        file.series_metadata.native_pixel.modality_lut.as_ref(),
        file.rescale_slope,
        file.rescale_intercept,
    );

    let luminance_samples = if samples_per_pixel > 1 {
        rescaled
            .chunks(samples_per_pixel as usize)
            .map(|chunk| chunk[0])
            .collect::<Vec<_>>()
    } else {
        rescaled
    };
    let unpadded_samples = padding_mask
        .as_deref()
        .map(|mask| exclude_padding_samples(&luminance_samples, mask));
    let window_source = unpadded_samples
        .as_deref()
        .filter(|samples| !samples.is_empty())
        .unwrap_or(&luminance_samples);

    let mut windowed = if let Some(values) = apply_voi_lut_if_selected(
        window_mode,
        requested_wc,
        requested_ww,
        file.default_window,
        file.series_metadata.native_pixel.voi_lut.as_ref(),
        &luminance_samples,
    ) {
        values
    } else {
        let resolved_window = resolve_window_with_mode(
            window_mode,
            requested_wc,
            requested_ww,
            file.default_window,
            window_source,
        )
        .ok_or_else(|| anyhow!("frame decode failed: could not resolve window"))?;
        apply_window(
            &luminance_samples,
            resolved_window.center,
            resolved_window.width.max(1.0),
        )
    };
    apply_monochrome1_inversion(&mut windowed, &file.photometric_interpretation);
    // Padding is background: black whatever the photometric interpretation.
    if let Some(mask) = padding_mask.as_deref() {
        apply_padding_background(&mut windowed, mask);
    }
    apply_rectangular_shutter(
        &mut windowed,
        rows,
        columns,
        file.series_metadata.presentation.rectangular_shutter,
    );
    apply_overlay_planes(
        &mut windowed,
        rows,
        columns,
        frame,
        &file.series_metadata.presentation.overlay_planes,
    );

    let image = ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(columns, rows, windowed)
        .ok_or_else(|| anyhow!("frame decode failed: windowed buffer size mismatch"))?;
    let mut encoded = Cursor::new(Vec::<u8>::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut encoded, ImageFormat::Png)
        .context("frame decode failed: png encoding failed")?;

    Ok(Bytes::from(encoded.into_inner()))
}

fn native_frame_layout(file: &FileEntry) -> NativeFrameLayout<'_> {
    NativeFrameLayout {
        rows: file.rows,
        columns: file.columns,
        samples_per_pixel: file.samples_per_pixel.max(1),
        bits_allocated: file.bits_allocated,
        planar_configuration: file.series_metadata.native_pixel.planar_configuration,
        photometric_interpretation: &file.photometric_interpretation,
        byte_order: NativeByteOrder::LittleEndian,
    }
}

// Retired, but still readable; dicom-dictionary-std marks the constant deprecated.
const EXPLICIT_VR_BIG_ENDIAN: &str = "1.2.840.10008.1.2.2";

/// Stored bytes for one native frame, plus the layout that interprets them.
struct NativeFrameSource<'a> {
    bytes: Vec<u8>,
    frame_in_bytes: u32,
    layout: NativeFrameLayout<'a>,
}

impl NativeFrameSource<'_> {
    fn raw_frame(&self) -> Result<Vec<u8>> {
        self.layout
            .extract_raw_frame(&self.bytes, self.frame_in_bytes)
            .context("frame decode failed: invalid native frame layout")
    }

    fn display_frame(&self) -> Result<Vec<u8>> {
        self.layout
            .extract_display_frame(&self.bytes, self.frame_in_bytes)
            .context("frame decode failed: invalid native frame layout")
    }
}

/// Reads one native frame without loading the rest of the Pixel Data element.
///
/// Byte-aligned frames in an undeflated data set are read by seeking straight
/// to the frame inside the top-level pixel element, so each request costs one
/// frame of I/O. Deflated data sets cannot be seeked and one-bit frames need
/// not start on a byte boundary; both are small in practice and fall back to
/// reading the whole element.
fn read_native_frame(file: &FileEntry, frame: u32) -> Result<NativeFrameSource<'_>> {
    let layout = native_frame_layout(file);
    let seekable = file.transfer_syntax_uid != uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN
        && layout.bits_allocated != 1;
    if seekable {
        let byte_order = if file.transfer_syntax_uid == EXPLICIT_VR_BIG_ENDIAN {
            NativeByteOrder::BigEndian
        } else {
            NativeByteOrder::LittleEndian
        };
        let bytes = read_native_frame_bytes(file, layout, frame)?;
        return Ok(NativeFrameSource {
            bytes,
            frame_in_bytes: 0,
            layout: NativeFrameLayout {
                byte_order,
                ..layout
            },
        });
    }

    let object = open_file(&file.path).with_context(|| {
        format!(
            "failed to open DICOM for native pixel read: {}",
            file.path.display()
        )
    })?;
    // dicom-object normalizes native primitive values to host order. The
    // supported release hosts are little-endian, matching the raw API.
    let bytes = object
        .element(native_pixel_element_tag(native_pixel_data_kind(file)))
        .context("frame decode failed: missing native pixel data element")?
        .to_bytes()
        .context("frame decode failed: pixel bytes unavailable")?
        .into_owned();
    Ok(NativeFrameSource {
        bytes,
        frame_in_bytes: frame,
        layout,
    })
}

/// Returns the stored bytes of `frame`, in the file's byte order, by locating
/// the top-level native pixel element and seeking past the preceding frames.
fn read_native_frame_bytes(
    file: &FileEntry,
    layout: NativeFrameLayout<'_>,
    frame: u32,
) -> Result<Vec<u8>> {
    let frame_len = layout
        .stored_frame_bytes()
        .context("frame decode failed: invalid native frame layout")?;
    let start = usize::try_from(frame)
        .ok()
        .and_then(|frame| frame.checked_mul(frame_len))
        .context("frame decode failed: frame offset overflowed")?;
    let end = start
        .checked_add(frame_len)
        .context("frame decode failed: frame offset overflowed")?;

    let mut reader = BufReader::new(
        File::open(&file.path)
            .with_context(|| format!("failed to open {}", file.path.display()))?,
    );
    reader.seek(SeekFrom::Start(128))?;
    let meta = FileMetaTable::from_reader(&mut reader)
        .with_context(|| format!("failed to read file meta: {}", file.path.display()))?;
    let transfer_syntax = TransferSyntaxRegistry
        .get(meta.transfer_syntax())
        .with_context(|| format!("unknown transfer syntax {}", meta.transfer_syntax()))?;
    let mut parser = LazyDataSetReader::new_with_ts(reader, transfer_syntax)
        .context("failed to start DICOM data set parser")?;

    let pixel_tag = native_pixel_element_tag(native_pixel_data_kind(file));
    // Nested pixel elements (for example an Icon Image Sequence) must not be
    // mistaken for the image's own pixel data.
    let mut sequence_depth = 0_usize;
    while let Some(token) = parser.advance() {
        match token.context("failed to parse DICOM data set")? {
            LazyDataToken::SequenceStart { .. } | LazyDataToken::PixelSequenceStart => {
                sequence_depth += 1;
            }
            LazyDataToken::SequenceEnd => sequence_depth = sequence_depth.saturating_sub(1),
            LazyDataToken::LazyValue { header, decoder }
                if sequence_depth == 0 && header.tag == pixel_tag =>
            {
                let available = header
                    .len
                    .get()
                    .and_then(|length| usize::try_from(length).ok())
                    .context("frame decode failed: native pixel data has undefined length")?;
                if end > available {
                    return Err(anyhow!(
                        "frame decode failed: native pixel data frame {frame} extends beyond {available} source bytes"
                    ));
                }
                let value_start = decoder.position();
                decoder.seek(value_start + start as u64)?;
                let mut bytes = Vec::with_capacity(frame_len);
                decoder.read_to_vec(u32::try_from(frame_len)?, &mut bytes)?;
                return Ok(bytes);
            }
            // The lazy reader leaves unread values in the stream; step over
            // them so the next token starts at the following header.
            LazyDataToken::LazyValue { header, decoder } => {
                let length = header
                    .len
                    .get()
                    .context("element with undefined length has no skippable value")?;
                decoder.skip_bytes(length)?;
            }
            LazyDataToken::LazyItemValue { len, decoder } => decoder.skip_bytes(len)?,
            _ => {}
        }
    }
    Err(anyhow!(
        "frame decode failed: missing native pixel data element"
    ))
}

fn native_pixel_data_kind(file: &FileEntry) -> NativePixelDataKind {
    file.series_metadata
        .native_pixel
        .pixel_data_kind
        .unwrap_or(NativePixelDataKind::Integer)
}

fn decode_numeric_samples(
    frame_slice: &[u8],
    bits_allocated: u32,
    signed: bool,
    big_endian: bool,
    kind: NativePixelDataKind,
) -> Result<Vec<f64>> {
    match (kind, bits_allocated, signed) {
        (NativePixelDataKind::Float32, 32, _) => Ok(frame_slice
            .chunks_exact(4)
            .map(|chunk| {
                let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
                (if big_endian {
                    f32::from_be_bytes(bytes)
                } else {
                    f32::from_le_bytes(bytes)
                }) as f64
            })
            .collect()),
        (NativePixelDataKind::Float64, 64, _) => Ok(frame_slice
            .chunks_exact(8)
            .map(|chunk| {
                let bytes = [
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ];
                if big_endian {
                    f64::from_be_bytes(bytes)
                } else {
                    f64::from_le_bytes(bytes)
                }
            })
            .collect()),
        (NativePixelDataKind::Integer, 1, false) => {
            Ok(frame_slice.iter().map(|value| f64::from(*value)).collect())
        }
        (NativePixelDataKind::Integer, 8, false) => {
            Ok(frame_slice.iter().map(|value| *value as f64).collect())
        }
        (NativePixelDataKind::Integer, 8, true) => Ok(frame_slice
            .iter()
            .map(|value| (*value as i8) as f64)
            .collect()),
        (NativePixelDataKind::Integer, 16, false) => {
            let mut out = Vec::with_capacity(frame_slice.len() / 2);
            for chunk in frame_slice.chunks_exact(2) {
                let value = if big_endian {
                    u16::from_be_bytes([chunk[0], chunk[1]])
                } else {
                    u16::from_le_bytes([chunk[0], chunk[1]])
                };
                out.push(value as f64);
            }
            Ok(out)
        }
        (NativePixelDataKind::Integer, 16, true) => {
            let mut out = Vec::with_capacity(frame_slice.len() / 2);
            for chunk in frame_slice.chunks_exact(2) {
                let value = if big_endian {
                    i16::from_be_bytes([chunk[0], chunk[1]])
                } else {
                    i16::from_le_bytes([chunk[0], chunk[1]])
                };
                out.push(value as f64);
            }
            Ok(out)
        }
        (NativePixelDataKind::Integer, 32, false) => Ok(frame_slice
            .chunks_exact(4)
            .map(|chunk| {
                let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
                (if big_endian {
                    u32::from_be_bytes(bytes)
                } else {
                    u32::from_le_bytes(bytes)
                }) as f64
            })
            .collect()),
        (NativePixelDataKind::Integer, 32, true) => Ok(frame_slice
            .chunks_exact(4)
            .map(|chunk| {
                let bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
                (if big_endian {
                    i32::from_be_bytes(bytes)
                } else {
                    i32::from_le_bytes(bytes)
                }) as f64
            })
            .collect()),
        _ => Err(anyhow!(
            "frame decode failed: unsupported native sample kind {kind:?} with BitsAllocated {bits_allocated}"
        )),
    }
}

pub(crate) async fn read_raw_uncompressed(
    file: FileEntry,
    frame: u32,
) -> Result<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || read_raw_uncompressed_blocking(&file, frame))
        .await
        .context("raw uncompressed read task failed")?
}

fn read_raw_uncompressed_blocking(
    file: &FileEntry,
    frame: u32,
) -> Result<(Bytes, RawFrameMetadata)> {
    let rows = file.rows;
    let columns = file.columns;
    let samples_per_pixel = file.samples_per_pixel.max(1);
    let bits_allocated = file.bits_allocated;
    let mut frame_bytes = read_native_frame(file, frame)?.raw_frame()?;
    if native_pixel_data_kind(file) == NativePixelDataKind::Integer && bits_allocated > 1 {
        canonicalize_integer_samples(
            &mut frame_bytes,
            bits_allocated,
            file.series_metadata.native_pixel.bits_stored,
            file.series_metadata.native_pixel.high_bit,
            file.pixel_representation == 1,
        )
        .context("frame decode failed: invalid native stored-bit layout")?;
    }

    let metadata = file.raw_metadata(rows, columns, bits_allocated, samples_per_pixel);
    Ok((Bytes::from(frame_bytes), metadata))
}

#[cfg(test)]
mod tests {
    use super::decode_numeric_samples;
    use crate::types::NativePixelDataKind;

    #[test]
    fn decodes_one_bit_and_32_bit_integer_samples() {
        assert_eq!(
            decode_numeric_samples(&[1, 0, 1], 1, false, false, NativePixelDataKind::Integer,)
                .unwrap(),
            [1.0, 0.0, 1.0]
        );
        let unsigned = [0_u32, 65_535, 2_147_483_648, u32::MAX]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            decode_numeric_samples(&unsigned, 32, false, false, NativePixelDataKind::Integer,)
                .unwrap(),
            [0.0, 65_535.0, 2_147_483_648.0, 4_294_967_295.0]
        );
    }

    #[test]
    fn decodes_float_and_double_float_samples() {
        let floats = [-256.0_f32, 0.5, 512.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            decode_numeric_samples(&floats, 32, false, false, NativePixelDataKind::Float32,)
                .unwrap(),
            [-256.0, 0.5, 512.25]
        );
        let doubles = [-256.0_f64, 0.5, 511.750_000_001_862_65]
            .into_iter()
            .flat_map(f64::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(
            decode_numeric_samples(&doubles, 64, false, false, NativePixelDataKind::Float64,)
                .unwrap(),
            [-256.0, 0.5, 511.750_000_001_862_65]
        );
    }
}

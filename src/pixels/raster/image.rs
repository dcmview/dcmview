//! The image crate leaves JPEG and WebP samples in the stored orientation.
use super::{checked_profile, raster_decode_heap_limit, reader::Reader, RasterFrame};
use crate::api::contracts::{FileFormat, RasterSampleFormat};
use crate::types::FileEntry;
use anyhow::{ensure, Context, Result};
use image::{ColorType, ImageDecoder, Limits};

pub(super) fn decode(
    file: &FileEntry,
    source: &mut Reader<'_>,
    length: u64,
    expected: u64,
) -> Result<RasterFrame> {
    match file.format {
        FileFormat::Jpeg => read(
            file,
            image::codecs::jpeg::JpegDecoder::new(source)?,
            length,
            expected,
        ),
        FileFormat::Webp => read(
            file,
            image::codecs::webp::WebPDecoder::new(source)?,
            length,
            expected,
        ),
        _ => anyhow::bail!("invalid image codec"),
    }
}

fn read(
    file: &FileEntry,
    mut decoder: impl ImageDecoder,
    length: u64,
    expected: u64,
) -> Result<RasterFrame> {
    let raster = file.raster.as_ref().context("missing raster metadata")?;
    let color = match file.samples_per_pixel {
        1 => ColorType::L8,
        3 => ColorType::Rgb8,
        4 => ColorType::Rgba8,
        _ => anyhow::bail!("invalid image catalog color"),
    };
    ensure!(
        decoder.dimensions() == (file.columns, file.rows)
            && decoder.color_type() == color
            && file.bits_allocated == 8
            && raster.bit_depth == 8
            && raster.sample_format == RasterSampleFormat::Uint,
        "image header differs from catalog"
    );
    let mut limits = Limits::default();
    limits.max_image_width = Some(file.columns);
    limits.max_image_height = Some(file.rows);
    limits.max_alloc = raster_decode_heap_limit(file, length);
    decoder.set_limits(limits)?;
    let profile = decoder.icc_profile()?;
    let icc_profile = checked_profile(file, profile.as_deref());
    drop(profile);
    let mut bytes = vec![0; usize::try_from(expected)?];
    decoder.read_image(&mut bytes)?;
    Ok(RasterFrame { bytes, icc_profile })
}

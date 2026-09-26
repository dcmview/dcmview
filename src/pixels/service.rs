use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::{
    FileEntry, FrameCacheKey, NativePixelDataKind, RawFrameCacheKey, WindowRequest,
};
use bytes::Bytes;
use std::sync::{Arc, Mutex};

use super::cache::{FrameCache, RawFrameCache};
use super::deflated_frame::{
    decode_deflated_binary_frame_to_png, decode_raw_deflated_binary_frame,
};
use super::error::{PixelError, PixelResult};
use super::header::open_header;
use super::jpeg::{
    decode_compressed_frame_to_png, decode_raw_jpeg_lossless, read_raw_jpeg_samples,
};
use super::jpeg2000::{decode_jp2_fragment_to_png, decode_raw_jp2_samples};
use super::jpegls::{decode_jpeg_ls_to_png, decode_raw_jpeg_ls};
use super::jpegxl::{decode_jpeg_xl_to_png, decode_raw_jpeg_xl};
use super::native::{decode_uncompressed_to_png, read_raw_uncompressed};
use super::rle::{decode_raw_rle, decode_rle_to_png};
use super::syntax::{codec_for_syntax, Codec};
use super::window::read_pixel_padding_range;

#[derive(Debug, Clone)]
pub struct RawFrameRequest {
    pub frame: u32,
}

#[derive(Debug, Clone)]
pub struct RawFrameResponse {
    pub body: Bytes,
    pub metadata: RawFrameMetadata,
    pub cache_hit: bool,
}

pub async fn load_raw_frame(
    file: FileEntry,
    cache: Arc<Mutex<RawFrameCache>>,
    request: RawFrameRequest,
) -> PixelResult<RawFrameResponse> {
    if !file.has_pixels {
        return Err(PixelError::NoPixelData);
    }
    if request.frame >= file.frame_count {
        return Err(PixelError::FrameOutOfRange);
    }

    let codec = codec_or_unsupported(&file)?;

    let key = RawFrameCacheKey {
        file_index: file.index,
        frame: request.frame,
    };

    if let Ok(mut lock) = cache.lock() {
        if let Some((bytes, meta)) = lock.get(&key) {
            return Ok(RawFrameResponse {
                body: bytes,
                metadata: meta,
                cache_hit: true,
            });
        }
    }

    let (body, mut metadata) = match codec {
        Codec::DeflatedImageFrame => {
            decode_raw_deflated_binary_frame(file.clone(), request.frame).await?
        }
        Codec::JpegBaseline => read_raw_jpeg_samples(file.clone(), request.frame)
            .await
            .map_err(PixelError::raw_decode)?,
        Codec::JpegLossless => decode_raw_jpeg_lossless(file.clone(), request.frame).await?,
        Codec::Jpeg2000 => decode_raw_jp2_samples(file.clone(), request.frame).await?,
        Codec::JpegXl => decode_raw_jpeg_xl(file.clone(), request.frame).await?,
        Codec::JpegLs => decode_raw_jpeg_ls(file.clone(), request.frame).await?,
        Codec::Native => read_raw_uncompressed(file.clone(), request.frame)
            .await
            .map_err(PixelError::raw_decode)?,
        Codec::Rle => decode_raw_rle(file.clone(), request.frame).await?,
    };
    if let Some((low, high)) = raw_padding_bounds(&file, &metadata).await {
        metadata.padding_low = Some(low);
        metadata.padding_high = Some(high);
    }

    if let Ok(mut lock) = cache.lock() {
        lock.insert(key, (body.clone(), metadata.clone()));
    }

    Ok(RawFrameResponse {
        body,
        metadata,
        cache_hit: false,
    })
}

/// Pixel Padding bounds for a grayscale integer raw frame, so client-side
/// windowing can exclude padding exactly as the display path does.
async fn raw_padding_bounds(file: &FileEntry, metadata: &RawFrameMetadata) -> Option<(f64, f64)> {
    let integer_pixels = matches!(
        file.series_metadata.native_pixel.pixel_data_kind,
        None | Some(NativePixelDataKind::Integer)
    );
    if metadata.samples_per_pixel != 1 || !integer_pixels {
        return None;
    }
    let path = file.path.clone();
    tokio::task::spawn_blocking(move || {
        let object = open_header(&path).ok()?;
        read_pixel_padding_range(&object, NativePixelDataKind::Integer).map(|range| range.bounds())
    })
    .await
    .ok()
    .flatten()
}

#[derive(Debug, Clone)]
pub struct FrameRequest {
    pub frame: u32,
    pub window_center: Option<f64>,
    pub window_width: Option<f64>,
    pub window_mode: WindowMode,
}

#[derive(Debug, Clone)]
pub struct FrameResponse {
    pub body: Bytes,
    pub content_type: &'static str,
    pub cache_hit: bool,
}

pub async fn load_frame(
    file: FileEntry,
    cache: Arc<Mutex<FrameCache>>,
    request: FrameRequest,
) -> PixelResult<FrameResponse> {
    if !file.has_pixels {
        return Err(PixelError::NoPixelData);
    }
    if request.frame >= file.frame_count {
        return Err(PixelError::FrameOutOfRange);
    }

    let window = WindowRequest::new(
        request.window_center,
        request.window_width,
        request.window_mode,
    )
    .map_err(|error| PixelError::InvalidWindow(error.to_string()))?;
    let codec = codec_or_unsupported(&file)?;
    let key = FrameCacheKey::new(
        file.index,
        request.frame,
        window.center(),
        window.width(),
        window.mode(),
    );

    if let Ok(mut lock) = cache.lock() {
        if let Some(bytes) = lock.get(&key) {
            return Ok(FrameResponse {
                body: bytes,
                content_type: "image/png",
                cache_hit: true,
            });
        }
    }

    let (frame, center, width, mode) = (
        request.frame,
        window.center(),
        window.width(),
        window.mode(),
    );
    let body = match codec {
        Codec::DeflatedImageFrame => {
            decode_deflated_binary_frame_to_png(file.clone(), frame, center, width, mode).await?
        }
        Codec::JpegBaseline | Codec::JpegLossless => {
            decode_compressed_frame_to_png(codec, file.clone(), frame, center, width, mode)
                .await
                .map_err(PixelError::frame_decode)?
        }
        Codec::Jpeg2000 => decode_jp2_fragment_to_png(file.clone(), frame, center, width, mode)
            .await
            .map_err(PixelError::frame_decode)?,
        Codec::JpegXl => decode_jpeg_xl_to_png(file.clone(), frame, center, width, mode).await?,
        Codec::JpegLs => decode_jpeg_ls_to_png(file.clone(), frame, center, width, mode).await?,
        Codec::Native => decode_uncompressed_to_png(file.clone(), frame, center, width, mode)
            .await
            .map_err(PixelError::frame_decode)?,
        Codec::Rle => decode_rle_to_png(file.clone(), frame, center, width, mode).await?,
    };

    if let Ok(mut lock) = cache.lock() {
        lock.insert(key, body.clone());
    }

    Ok(FrameResponse {
        body,
        content_type: "image/png",
        cache_hit: false,
    })
}

fn codec_or_unsupported(file: &FileEntry) -> PixelResult<Codec> {
    codec_for_syntax(&file.transfer_syntax_uid)
        .ok_or_else(|| PixelError::UnsupportedTransferSyntax(file.transfer_syntax_uid.clone()))
}

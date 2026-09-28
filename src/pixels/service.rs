use crate::api::contracts::{RawFrameMetadata, SupportState, WindowMode};
use crate::types::{
    FileEntry, FrameCacheKey, NativePixelDataKind, RawFrameCacheKey, WindowRequest,
};
use bytes::Bytes;
use futures::FutureExt;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Semaphore;

use super::cache::{BudgetedLru, FrameBody, FrameCache, InFlight, RawFrameCache};
use super::deflated_frame::{
    decode_deflated_binary_frame_to_png, decode_raw_deflated_binary_frame,
};
use super::error::{PixelError, PixelResult};
use super::jpeg::{
    decode_compressed_frame_to_png, decode_raw_jpeg_lossless, read_raw_jpeg_samples,
};
use super::jpeg2000::{decode_jp2_fragment_to_png, decode_raw_jp2_samples};
use super::jpegls::{decode_jpeg_ls_to_png, decode_raw_jpeg_ls};
use super::jpegxl::{decode_jpeg_xl_to_png, decode_raw_jpeg_xl};
use super::native::{decode_uncompressed_to_png, read_raw_uncompressed};
use super::rle::{decode_raw_rle, decode_rle_to_png};
use super::syntax::{classify_pixel_support, codec_for_syntax, Codec, PixelSupportReason};

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
    file: Arc<FileEntry>,
    cache: Arc<Mutex<RawFrameCache>>,
    request: RawFrameRequest,
) -> PixelResult<RawFrameResponse> {
    if !file.has_pixels {
        return Err(PixelError::NoPixelData {
            file_index: file.index,
        });
    }
    PixelError::ensure_frame(request.frame, file.frame_count)?;

    let codec = codec_or_unsupported(&file)?;
    reject_unsupported_layout(&file, FrameKind::Raw)?;

    let key = RawFrameCacheKey {
        file_index: file.index,
        frame: request.frame,
    };

    let frame = request.frame;
    let ((body, metadata), cache_hit) = cached_or_decoded(&cache, key, async move {
        let (body, mut metadata) = match codec {
            Codec::DeflatedImageFrame => {
                decode_raw_deflated_binary_frame(file.clone(), frame).await?
            }
            Codec::JpegBaseline => read_raw_jpeg_samples(file.clone(), frame)
                .await
                .map_err(PixelError::raw_decode)?,
            Codec::JpegLossless => decode_raw_jpeg_lossless(file.clone(), frame).await?,
            Codec::Jpeg2000 => decode_raw_jp2_samples(file.clone(), frame).await?,
            Codec::JpegXl => decode_raw_jpeg_xl(file.clone(), frame).await?,
            Codec::JpegLs => decode_raw_jpeg_ls(file.clone(), frame).await?,
            Codec::Native => read_raw_uncompressed(file.clone(), frame)
                .await
                .map_err(PixelError::raw_decode)?,
            Codec::Rle => decode_raw_rle(file.clone(), frame).await?,
        };
        if let Some([low, high]) = raw_padding_bounds(&file, &metadata) {
            metadata.padding_low = Some(low);
            metadata.padding_high = Some(high);
        }
        Ok((body, metadata))
    })
    .await?;

    Ok(RawFrameResponse {
        body,
        metadata,
        cache_hit,
    })
}

/// One pixel of a raw frame as a 1x1 raw frame: its samples in color-by-pixel
/// order. The raw frame keeps the stored layout (planar native color, and
/// native YBR_FULL_422 as Y0 Y1 Cb Cr per pixel pair), which is resolved here
/// the way the viewer's readout resolves it. `None` outside the frame.
pub fn raw_pixel(
    file: &FileEntry,
    raw: &RawFrameResponse,
    row: u32,
    column: u32,
) -> Option<(Bytes, RawFrameMetadata)> {
    let metadata = &raw.metadata;
    if row >= metadata.rows || column >= metadata.columns {
        return None;
    }
    let size = match metadata.bits_allocated {
        1 | 8 => 1,
        bits @ (16 | 32 | 64) => (bits / 8) as usize,
        _ => return None,
    };
    let columns = metadata.columns as usize;
    let pixel_count = metadata.rows as usize * columns;
    let pixel = row as usize * columns + column as usize;
    let samples = metadata.samples_per_pixel as usize;
    let subsampled = metadata
        .photometric_interpretation
        .trim()
        .eq_ignore_ascii_case("YBR_FULL_422")
        && raw.body.len() == pixel_count * 2 * size;
    let planar = file.series_metadata.native_pixel.planar_configuration == Some(1)
        && codec_for_syntax(&file.transfer_syntax_uid) == Some(Codec::Native);
    let indices: Vec<usize> = if samples == 1 {
        vec![pixel]
    } else if subsampled {
        let pair = row as usize * columns * 2 + (column as usize / 2) * 4;
        vec![pair + column as usize % 2, pair + 2, pair + 3]
    } else if planar {
        (0..samples)
            .map(|sample| sample * pixel_count + pixel)
            .collect()
    } else {
        (0..samples)
            .map(|sample| pixel * samples + sample)
            .collect()
    };
    let mut body = Vec::with_capacity(indices.len() * size);
    for index in &indices {
        body.extend_from_slice(raw.body.get(index * size..(index + 1) * size)?);
    }
    let pixel_metadata = RawFrameMetadata {
        rows: 1,
        columns: 1,
        samples_per_pixel: indices.len() as u32,
        ..metadata.clone()
    };
    Some((Bytes::from(body), pixel_metadata))
}

/// Pixel Padding bounds for a grayscale integer raw frame, so client-side
/// windowing can exclude padding exactly as the display path does.
fn raw_padding_bounds(file: &FileEntry, metadata: &RawFrameMetadata) -> Option<[f64; 2]> {
    let native = &file.series_metadata.native_pixel;
    let integer_pixels = matches!(
        native.pixel_data_kind,
        None | Some(NativePixelDataKind::Integer)
    );
    (metadata.samples_per_pixel == 1 && integer_pixels)
        .then_some(native.pixel_padding)
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
    file: Arc<FileEntry>,
    cache: Arc<Mutex<FrameCache>>,
    request: FrameRequest,
) -> PixelResult<FrameResponse> {
    if !file.has_pixels {
        return Err(PixelError::NoPixelData {
            file_index: file.index,
        });
    }
    PixelError::ensure_frame(request.frame, file.frame_count)?;

    let window = WindowRequest::new(
        request.window_center,
        request.window_width,
        request.window_mode,
    )
    .map_err(|error| PixelError::InvalidWindow(error.to_string()))?;
    let codec = codec_or_unsupported(&file)?;
    reject_unsupported_layout(&file, FrameKind::Display)?;
    let key = FrameCacheKey::new(
        file.index,
        request.frame,
        window.center(),
        window.width(),
        window.mode(),
    );

    let (frame, center, width, mode) = (
        request.frame,
        window.center(),
        window.width(),
        window.mode(),
    );
    let (body, cache_hit) = cached_or_decoded(&cache, key, async move {
        Ok(match codec {
            Codec::DeflatedImageFrame => {
                decode_deflated_binary_frame_to_png(file, frame, center, width, mode).await?
            }
            Codec::JpegBaseline | Codec::JpegLossless => {
                decode_compressed_frame_to_png(codec, file, frame, center, width, mode)
                    .await
                    .map_err(PixelError::frame_decode)?
            }
            Codec::Jpeg2000 => decode_jp2_fragment_to_png(file, frame, center, width, mode)
                .await
                .map_err(PixelError::frame_decode)?,
            Codec::JpegXl => decode_jpeg_xl_to_png(file, frame, center, width, mode).await?,
            Codec::JpegLs => decode_jpeg_ls_to_png(file, frame, center, width, mode).await?,
            Codec::Native => decode_uncompressed_to_png(file, frame, center, width, mode)
                .await
                .map_err(PixelError::frame_decode)?,
            Codec::Rle => decode_rle_to_png(file, frame, center, width, mode).await?,
        })
    })
    .await?;

    Ok(FrameResponse {
        body,
        content_type: "image/png",
        cache_hit,
    })
}

/// Decodes run on the blocking pool; bounding them to the core count keeps
/// concurrent requests from multiplying peak memory (one large frame can take
/// hundreds of MB while it decodes) without speeding anything up.
static DECODE_PERMITS: LazyLock<Semaphore> = LazyLock::new(|| {
    Semaphore::new(std::thread::available_parallelism().map_or(4, |cores| cores.get()))
});

/// The cached value for `key` (`true`), or the result of `decode` (`false`).
///
/// A request for a key already being decoded awaits that decode and counts as
/// a hit. The decode runs as its own task and caches its result there, so a
/// client that disconnects mid-decode (a slider drag, a held arrow key) still
/// leaves the finished frame cached for the request that follows.
async fn cached_or_decoded<K, V>(
    cache: &Arc<Mutex<BudgetedLru<K, V>>>,
    key: K,
    decode: impl Future<Output = PixelResult<V>> + Send + 'static,
) -> PixelResult<(V, bool)>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: FrameBody + Send + Sync + 'static,
{
    let (flight, cache_hit) = {
        let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
        if let Some(value) = lock.get(&key) {
            return Ok((value, true));
        }
        match lock.in_flight(&key) {
            Some(flight) => (flight, true),
            None => {
                let flight = spawn_decode(cache, key.clone(), decode);
                lock.start_flight(key, flight.clone());
                (flight, false)
            }
        }
    };
    flight
        .await
        .map(|value| (value, cache_hit))
        .map_err(|error| error.duplicate())
}

/// Runs `decode` as its own task, which caches the result and clears the
/// in-flight entry whether or not anyone is still waiting for it.
fn spawn_decode<K, V>(
    cache: &Arc<Mutex<BudgetedLru<K, V>>>,
    key: K,
    decode: impl Future<Output = PixelResult<V>> + Send + 'static,
) -> InFlight<V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: FrameBody + Send + Sync + 'static,
{
    let (task_cache, task_key) = (Arc::clone(cache), key.clone());
    let task = tokio::spawn(async move {
        let result = match DECODE_PERMITS.acquire().await {
            Ok(_permit) => decode.await,
            Err(_) => Err(PixelError::frame_decode(anyhow::anyhow!(
                "decoder shut down"
            ))),
        };
        if let Ok(mut lock) = task_cache.lock() {
            lock.finish_flight(&task_key);
            if let Ok(value) = &result {
                lock.insert(task_key, value.clone());
            }
        }
        result
    });
    let cache = Arc::clone(cache);
    async move {
        match task.await {
            Ok(result) => result.map_err(Arc::new),
            Err(join_error) => {
                // A panicked decode never reached its own cleanup.
                if let Ok(mut lock) = cache.lock() {
                    lock.finish_flight(&key);
                }
                Err(Arc::new(PixelError::frame_decode(anyhow::anyhow!(
                    "decode task failed: {join_error}"
                ))))
            }
        }
    }
    .boxed()
    .shared()
}

fn cache_poisoned() -> PixelError {
    PixelError::frame_decode(anyhow::anyhow!("frame cache lock poisoned"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Display,
    Raw,
}

/// Reports a layout the catalog marks unsupported as such, with the catalog's
/// reason, before any decode is attempted. Raw frames serve samples for some
/// layouts display cannot present (palette indices, other photometric
/// interpretations, color the display path rejects), so for them only the
/// reasons that break sample reads too apply.
fn reject_unsupported_layout(file: &FileEntry, kind: FrameKind) -> PixelResult<()> {
    let support = classify_pixel_support(file);
    let Some(reason) = support.reason else {
        return Ok(());
    };
    let applies = support.state == SupportState::Unsupported
        && (kind == FrameKind::Display
            || matches!(
                reason,
                PixelSupportReason::InvalidGeometry
                    | PixelSupportReason::NumericPrecisionNotSupported
            ));
    if applies {
        return Err(PixelError::UnsupportedLayout(reason.id().to_string()));
    }
    Ok(())
}

fn codec_or_unsupported(file: &FileEntry) -> PixelResult<Codec> {
    codec_for_syntax(&file.transfer_syntax_uid)
        .ok_or_else(|| PixelError::UnsupportedTransferSyntax(file.transfer_syntax_uid.clone()))
}

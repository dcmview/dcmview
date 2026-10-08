use crate::api::contracts::{
    RawFrameMetadata, RealWorldValueMap, SupportState, ThumbnailSource, WindowMode,
};
use crate::types::{
    FileEntry, FrameCacheKey, NativePixelDataKind, RawFrameCacheKey, ResolvedWindow,
    ThumbnailCacheKey, WindowRequest,
};
use anyhow::Context;
use bytes::Bytes;
use futures::FutureExt;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use super::cache::{BudgetedLru, FrameBody, FrameCache, InFlight, RawFrameCache, ThumbnailCache};
use super::deflated_frame::{
    decode_deflated_binary_frame_to_png, decode_raw_deflated_binary_frame,
    render_deflated_binary_frame,
};
use super::error::{PixelError, PixelResult};
use super::jpeg::{
    decode_compressed_frame_to_png, decode_raw_jpeg_lossless, read_raw_jpeg_samples,
    render_compressed_frame,
};
use super::jpeg2000::{decode_jp2_fragment_to_png, decode_raw_jp2_samples, render_jp2_fragment};
use super::jpegls::{decode_jpeg_ls_to_png, decode_raw_jpeg_ls, render_jpeg_ls};
use super::jpegxl::{decode_jpeg_xl_to_png, decode_raw_jpeg_xl, render_jpeg_xl};
use super::native::{decode_uncompressed_to_png, read_raw_uncompressed, render_uncompressed};
use super::redaction::{redact_png, redact_raw, RawLayout, Redaction};
use super::render::{
    encode_real_world_windowed_png, encode_windowed_luminance_png, AppliedWindow, DisplayBuffer,
    DisplayPng, LuminanceRenderOptions, StoredSamples,
};
use super::rle::{decode_raw_rle, decode_rle_to_png, render_rle};
use super::schedule::{decode_scheduler, DecodeClass, DecodePermit};
use super::syntax::{classify_pixel_support, codec_for_syntax, Codec, PixelSupportReason};
use super::thumbnail::{encode_thumbnail_jpeg, ThumbnailRequest, ThumbnailResponse};

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

/// [`load_raw_frame`] with the frame's redaction boxes filled. The cache
/// keeps the decoded samples, which the display path also reads; the boxes
/// are filled in a copy for each response.
pub async fn load_redacted_raw_frame(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<RawFrameCache>>,
    request: RawFrameRequest,
    redaction: &Redaction,
) -> PixelResult<RawFrameResponse> {
    let mut raw = load_raw_frame(file.clone(), cache, request).await?;
    if !redaction.is_empty() {
        raw.body = redact_raw(&file, &raw.body, &raw.metadata, &redaction.boxes);
    }
    Ok(raw)
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
    if file.format.is_raster() {
        return Err(PixelError::UnsupportedLayout(
            classify_pixel_support(&file)
                .reason_id()
                .unwrap_or_default()
                .to_string(),
        ));
    }

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
    let layout = RawLayout::of(file, metadata, raw.body.len())?;
    let size = layout.size;
    let mut body = Vec::with_capacity(3 * size);
    let mut samples = 0;
    for index in layout.sample_indices(row as usize, column as usize) {
        body.extend_from_slice(raw.body.get(index * size..(index + 1) * size)?);
        samples += 1;
    }
    let pixel_metadata = RawFrameMetadata {
        rows: 1,
        columns: 1,
        samples_per_pixel: samples,
        ..metadata.clone()
    };
    Some((Bytes::from(body), pixel_metadata))
}

/// Pixel Padding bounds (the float padding attributes for float pixel data)
/// for a grayscale raw frame, so client-side windowing can exclude padding
/// exactly as the display path does.
fn raw_padding_bounds(file: &FileEntry, metadata: &RawFrameMetadata) -> Option<[f64; 2]> {
    (metadata.samples_per_pixel == 1)
        .then_some(file.series_metadata.native_pixel.pixel_padding)
        .flatten()
}

#[derive(Debug, Clone)]
pub struct FrameRequest {
    pub frame: u32,
    pub window_center: Option<f64>,
    pub window_width: Option<f64>,
    pub window_mode: WindowMode,
    /// The mapping whose values an explicit window is in (the frame's
    /// preferred real-world mapping, in the requested unit); `None` windows
    /// Modality values.
    pub real_world: Option<RealWorldValueMap>,
    /// A window/level drag preview: served from the display cache when
    /// present, otherwise rendered for this request alone and not cached.
    pub preview: bool,
}

#[derive(Debug, Clone)]
pub struct FrameResponse {
    pub body: Bytes,
    pub content_type: &'static str,
    /// The presentation the frame was actually rendered with.
    pub window: AppliedWindow,
    pub cache_hit: bool,
}

pub async fn load_frame(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<FrameCache>>,
    raw_cache: Arc<Mutex<RawFrameCache>>,
    request: FrameRequest,
) -> PixelResult<FrameResponse> {
    load_redacted_frame(file, cache, raw_cache, request, Redaction::default()).await
}

/// [`load_frame`] with the frame's redaction boxes painted black. The boxes'
/// revision is part of the cache key, so a frame rendered before a box was
/// drawn is never served after it.
pub async fn load_redacted_frame(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<FrameCache>>,
    raw_cache: Arc<Mutex<RawFrameCache>>,
    request: FrameRequest,
    redaction: Redaction,
) -> PixelResult<FrameResponse> {
    if !file.has_pixels {
        return Err(PixelError::NoPixelData {
            file_index: file.index,
        });
    }
    PixelError::ensure_frame(request.frame, file.frame_count)?;
    if file.format.is_raster() {
        return Err(PixelError::UnsupportedLayout(
            classify_pixel_support(&file)
                .reason_id()
                .unwrap_or_default()
                .to_string(),
        ));
    }

    let window = WindowRequest::new(
        request.window_center,
        request.window_width,
        request.window_mode,
    )
    .map_err(|error| PixelError::InvalidWindow(error.to_string()))?;
    let codec = codec_or_unsupported(&file)?;
    reject_unsupported_layout(&file, FrameKind::Display)?;
    let mut display = DisplayWindow {
        frame: request.frame,
        center: window.center(),
        width: window.width(),
        mode: window.mode(),
        redaction: if redaction.is_empty() {
            0
        } else {
            redaction.revision
        },
    };
    let boxes = Arc::new(redaction.boxes);

    let real_world_window = display
        .center
        .zip(display.width)
        .filter(|_| display.mode == WindowMode::Default);
    if let (Some(map), Some(window)) = (request.real_world, real_world_window) {
        let key = display.cache_key(&file, Some(&map.unit_label));
        if let Some(display) = cache.lock().map_err(|_| cache_poisoned())?.get(&key) {
            return Ok(FrameResponse::png(display, true));
        }
        // A real-world window is applied to decoded integer samples, so
        // JPEG 2000 decodes through the raw tier here too.
        let raw = raw_samples_for_display(&file, codec, &raw_cache, display.frame, true).await;
        if let Some((raw, layout)) = raw {
            let render = window_real_world_samples(file, raw, layout, map, window, display.frame);
            let render = redacted(render, boxes);
            let (display, cache_hit) =
                cached_or_rendered(&cache, key, request.preview, render).await?;
            return Ok(FrameResponse::png(display, cache_hit));
        }
        // Samples a real-world window cannot apply to show the default
        // window, as a frame without a mapping in that unit does, and report
        // it, so the viewer can tell its window was not applied.
        display.center = None;
        display.width = None;
    }

    // Only this frame's integer Modality path proves sub-unit widths render
    // alike. Unit windows above retain their exact widths and separate keys.
    if let (Some(center), Some(width)) = (display.center, display.width) {
        display.width = Some(
            super::window::WindowFunction::for_file(&file)
                .applied(ResolvedWindow { center, width })
                .width,
        );
    }

    let key = display.cache_key(&file, None);

    let cached = cache.lock().map_err(|_| cache_poisoned())?.get(&key);
    let (display, cache_hit) = match cached {
        Some(display) => (display, true),
        None => match raw_samples_for_display(&file, codec, &raw_cache, display.frame, false).await
        {
            Some((raw, layout)) => {
                let render = redacted(window_raw_samples(file, raw, layout, display), boxes);
                cached_or_rendered(&cache, key, request.preview, render).await?
            }
            None => {
                let render = redacted(decode_display_frame(codec, file, display), boxes);
                cached_or_rendered(&cache, key, request.preview, render).await?
            }
        },
    };

    Ok(FrameResponse::png(display, cache_hit))
}

/// The gallery thumbnail of one frame: a small JPEG of the whole frame with
/// the redaction boxes painted black.
///
/// Errors as [`load_redacted_frame`] does for the same file and frame: no
/// pixel data, frame out of range, unsupported transfer syntax, a layout the
/// display path does not present, a failed decode.
///
/// The cache key is the file, frame, bucket, window mode and the revision of
/// `redaction` (0 when it has no box for this frame), so a thumbnail
/// rendered before a file's boxes changed is never served after the change.
///
/// 1. A thumbnail the cache holds is returned as it is: source
///    `ThumbnailCache`, a cache hit.
/// 2. A render of the same key already under way is awaited instead of
///    repeated, and counts as a cache hit with source `ThumbnailCache`.
/// 3. Otherwise the request waits for a [`DecodeClass::Background`] permit
///    of [`decode_scheduler`] inside this future, so a request that is
///    dropped while it waits starts no work. Once it holds the permit, the
///    render runs as its own task, keeps the permit until it ends, and
///    caches its result whether or not the request is still there. Source
///    `FullDecode`, a cache miss.
///
/// The render decodes the frame with its codec's display renderer at the
/// default window of `request.window_mode` (no explicit center or width, no
/// real-world unit), paints `redaction.boxes` on the buffer
/// ([`DisplayBuffer::redact`]) and encodes it
/// (`thumbnail::encode_thumbnail_jpeg`, with the file's effective pixel
/// aspect ratio). It does not draw the display shutter or overlay planes.
///
/// It never reads or writes the display cache and never writes the raw
/// cache: those hold the viewer's working set, which a gallery scroll must
/// not evict. In this version it does not read the raw cache either.
///
/// [`DecodeClass::Background`]: super::schedule::DecodeClass::Background
/// [`decode_scheduler`]: super::schedule::decode_scheduler
/// [`DisplayBuffer::redact`]: super::render::DisplayBuffer::redact
pub async fn load_thumbnail(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<ThumbnailCache>>,
    request: ThumbnailRequest,
    redaction: Redaction,
) -> PixelResult<ThumbnailResponse> {
    if !file.has_pixels {
        return Err(PixelError::NoPixelData {
            file_index: file.index,
        });
    }
    PixelError::ensure_frame(request.frame, file.frame_count)?;
    let codec = codec_or_unsupported(&file)?;
    reject_unsupported_layout(&file, FrameKind::Display)?;
    let key = ThumbnailCacheKey {
        file_index: file.index,
        frame: request.frame,
        bucket: request.bucket,
        window_mode: request.window_mode,
        redaction: if redaction.is_empty() {
            0
        } else {
            redaction.revision
        },
    };
    let flight = {
        let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
        if let Some(body) = lock.get(&key) {
            return Ok(thumbnail_response(body, true));
        }
        lock.in_flight(&key)
    };
    let (flight, cache_hit) = match flight {
        Some(flight) => (flight, true),
        None => {
            let permit = decode_scheduler().acquire(DecodeClass::Background).await;
            // Another request may have started or finished this key while
            // we queued. Claim a flight only while holding the cache lock.
            let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
            if let Some(body) = lock.get(&key) {
                return Ok(thumbnail_response(body, true));
            }
            match lock.in_flight(&key) {
                Some(flight) => {
                    drop(permit);
                    (flight, true)
                }
                None => {
                    let render = async move {
                        let mut buffer = render_display_frame(
                            codec,
                            file.clone(),
                            DisplayWindow {
                                frame: request.frame,
                                center: None,
                                width: None,
                                mode: request.window_mode,
                                redaction: 0,
                            },
                        )
                        .await?;
                        tokio::task::spawn_blocking(move || {
                            buffer.redact(&redaction.boxes);
                            encode_thumbnail_jpeg(
                                buffer,
                                request.bucket,
                                file.series_metadata
                                    .native_pixel
                                    .effective_pixel_aspect_ratio(),
                            )
                        })
                        .await
                        .context("thumbnail encoding task failed")
                        .and_then(|result| result)
                        .map_err(PixelError::frame_decode)
                    };
                    let flight = spawn_decode_with_permit(
                        &cache,
                        key.clone(),
                        std::future::ready(permit),
                        render,
                    );
                    lock.start_flight(key, flight.clone());
                    (flight, false)
                }
            }
        }
    };
    let body = flight.await.map_err(|error| error.duplicate())?;
    Ok(thumbnail_response(body, cache_hit))
}

fn thumbnail_response(body: Bytes, cache_hit: bool) -> ThumbnailResponse {
    ThumbnailResponse {
        body,
        cache_hit,
        source: if cache_hit {
            ThumbnailSource::ThumbnailCache
        } else {
            ThumbnailSource::FullDecode
        },
    }
}

impl FrameResponse {
    fn png(display: DisplayPng, cache_hit: bool) -> Self {
        Self {
            body: display.png,
            content_type: "image/png",
            window: display.window,
            cache_hit,
        }
    }
}

#[derive(Clone, Copy)]
struct DisplayWindow {
    frame: u32,
    center: Option<f64>,
    width: Option<f64>,
    mode: WindowMode,
    /// Revision of the redaction boxes painted on the frame; 0 without any.
    redaction: u64,
}

impl DisplayWindow {
    fn cache_key(self, file: &FileEntry, unit: Option<&str>) -> FrameCacheKey {
        FrameCacheKey {
            redaction: self.redaction,
            ..FrameCacheKey::new(
                file.index,
                self.frame,
                self.center,
                self.width,
                self.mode,
                unit,
            )
        }
    }
}

/// `render`'s frame with `boxes` painted black.
async fn redacted(
    render: impl Future<Output = PixelResult<DisplayPng>>,
    boxes: Arc<Vec<[u32; 4]>>,
) -> PixelResult<DisplayPng> {
    let mut display = render.await?;
    if boxes.is_empty() {
        return Ok(display);
    }
    display.png = tokio::task::spawn_blocking(move || redact_png(&display.png, &boxes))
        .await
        .context("redaction task failed")
        .and_then(|result| result)
        .map_err(PixelError::frame_decode)?;
    Ok(display)
}

/// Decodes and presents one display frame with the codec's own decoder.
async fn decode_display_frame(
    codec: Codec,
    file: Arc<FileEntry>,
    window: DisplayWindow,
) -> PixelResult<DisplayPng> {
    let DisplayWindow {
        frame,
        center,
        width,
        mode,
        ..
    } = window;
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
}

/// Renders the same codec paths without presentation graphics or encoding.
async fn render_display_frame(
    codec: Codec,
    file: Arc<FileEntry>,
    window: DisplayWindow,
) -> PixelResult<DisplayBuffer> {
    let DisplayWindow {
        frame,
        center,
        width,
        mode,
        ..
    } = window;
    Ok(match codec {
        Codec::DeflatedImageFrame => {
            render_deflated_binary_frame(file, frame, center, width, mode).await?
        }
        Codec::JpegBaseline | Codec::JpegLossless => {
            render_compressed_frame(codec, file, frame, center, width, mode)
                .await
                .map_err(PixelError::frame_decode)?
        }
        Codec::Jpeg2000 => render_jp2_fragment(file, frame, center, width, mode)
            .await
            .map_err(PixelError::frame_decode)?,
        Codec::JpegXl => render_jpeg_xl(file, frame, center, width, mode).await?,
        Codec::JpegLs => render_jpeg_ls(file, frame, center, width, mode).await?,
        Codec::Native => render_uncompressed(file, frame, center, width, mode)
            .await
            .map_err(PixelError::frame_decode)?,
        Codec::Rle => render_rle(file, frame, center, width, mode).await?,
    })
}

/// Presents raw-tier samples exactly as the codec's display decoder presents
/// the same decoded frame.
async fn window_raw_samples(
    file: Arc<FileEntry>,
    (bytes, metadata): (Bytes, RawFrameMetadata),
    (bits_allocated, signed): (u32, bool),
    window: DisplayWindow,
) -> PixelResult<DisplayPng> {
    tokio::task::spawn_blocking(move || {
        encode_windowed_luminance_png(
            &file,
            StoredSamples::Integer {
                bytes: &bytes,
                bits_allocated,
                signed,
            },
            LuminanceRenderOptions {
                frame: window.frame,
                rows: metadata.rows,
                columns: metadata.columns,
                requested_wc: window.center,
                requested_ww: window.width,
                window_mode: window.mode,
            },
        )
    })
    .await
    .context("display windowing task failed")
    .and_then(|result| result)
    .map_err(PixelError::frame_decode)
}

/// Presents raw-tier samples windowed over `map`'s real-world values.
async fn window_real_world_samples(
    file: Arc<FileEntry>,
    (bytes, metadata): (Bytes, RawFrameMetadata),
    (bits_allocated, signed): (u32, bool),
    map: RealWorldValueMap,
    window: (f64, f64),
    frame: u32,
) -> PixelResult<DisplayPng> {
    tokio::task::spawn_blocking(move || {
        encode_real_world_windowed_png(
            &file,
            &bytes,
            bits_allocated,
            signed,
            &map,
            window,
            frame,
            (metadata.rows, metadata.columns),
        )
    })
    .await
    .context("display windowing task failed")
    .and_then(|result| result)
    .map_err(PixelError::frame_decode)
}

/// The decoded samples of a display frame from the raw tier, so a frame is
/// decoded once whatever windows it is displayed with. Codecs whose raw decode
/// succeeds whenever their grayscale display decode does are decoded through
/// the raw cache (and fill it); JPEG 2000's raw path rejects components that
/// do not fit the declared layout, which display still windows per sample, so
/// unless `decode_jpeg2000` it only reuses a frame the raw cache already holds
/// rather than risk a second decode per request. `None` means decode for
/// display as before.
async fn raw_samples_for_display(
    file: &Arc<FileEntry>,
    codec: Codec,
    raw_cache: &Arc<Mutex<RawFrameCache>>,
    frame: u32,
    decode_jpeg2000: bool,
) -> Option<((Bytes, RawFrameMetadata), (u32, bool))> {
    if !displays_grayscale(file, codec) {
        return None;
    }
    let raw = if codec == Codec::Jpeg2000 && !decode_jpeg2000 {
        let key = RawFrameCacheKey {
            file_index: file.index,
            frame,
        };
        raw_cache.lock().ok()?.get(&key)?
    } else {
        let raw = load_raw_frame(file.clone(), raw_cache.clone(), RawFrameRequest { frame })
            .await
            .ok()?;
        (raw.body, raw.metadata)
    };
    let layout = display_integer_layout(file, codec, &raw.1)?;
    Some((raw, layout))
}

/// Whether the file's display decode takes the grayscale integer path, judged
/// from the catalog before decoding (`display_integer_layout` confirms it
/// against the decoded frame).
fn displays_grayscale(file: &FileEntry, codec: Codec) -> bool {
    let monochrome = matches!(
        file.photometric_interpretation
            .trim()
            .to_ascii_uppercase()
            .as_str(),
        "MONOCHROME1" | "MONOCHROME2"
    );
    file.samples_per_pixel == 1
        && match codec {
            Codec::Native => {
                monochrome
                    && matches!(
                        file.series_metadata.native_pixel.pixel_data_kind,
                        None | Some(NativePixelDataKind::Integer)
                    )
                    && matches!(file.bits_allocated, 1 | 8 | 16)
            }
            Codec::Rle => monochrome && matches!(file.pixel_representation, 0 | 1),
            Codec::DeflatedImageFrame
            | Codec::JpegBaseline
            | Codec::JpegLossless
            | Codec::JpegLs
            | Codec::JpegXl
            | Codec::Jpeg2000 => true,
        }
}

/// The container width and signedness the codec's display decoder windows a
/// decoded grayscale frame with, when those samples are exactly the raw
/// body; `None` when display would not window this raw frame as integers.
///
/// Each arm mirrors that decoder's `StoredSamples::Integer` call; the raw
/// metadata cannot be used as is, because it describes the wire (JPEG
/// Baseline reports canonical unsigned samples, one-bit frames keep
/// BitsAllocated 1 over one byte per sample).
fn display_integer_layout(
    file: &FileEntry,
    codec: Codec,
    metadata: &RawFrameMetadata,
) -> Option<(u32, bool)> {
    if metadata.samples_per_pixel != 1 {
        return None;
    }
    let signed = file.pixel_representation == 1;
    match (codec, metadata.bits_allocated) {
        (Codec::Native | Codec::DeflatedImageFrame, 1) => Some((8, false)),
        (Codec::DeflatedImageFrame, _) => None,
        (Codec::JpegXl, 8) => Some((8, false)),
        (_, bits @ (8 | 16)) => Some((bits, signed)),
        _ => None,
    }
}

/// The display frame for `key`: shared and cached like any frame, or for a
/// preview rendered for this request alone. A drag sends one preview per
/// window it passes through; caching them would evict the frames cine and
/// the settled window need, and no other request will ask for them.
async fn cached_or_rendered(
    cache: &Arc<Mutex<FrameCache>>,
    key: FrameCacheKey,
    preview: bool,
    render: impl Future<Output = PixelResult<DisplayPng>> + Send + 'static,
) -> PixelResult<(DisplayPng, bool)> {
    if !preview {
        return cached_or_decoded(cache, key, render).await;
    }
    let _permit = decode_scheduler().acquire(DecodeClass::Interactive).await;
    Ok((render.await?, false))
}

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
    spawn_decode_with_permit(
        cache,
        key,
        decode_scheduler().acquire(DecodeClass::Interactive),
        decode,
    )
}

// The permit future is awaited in the detached task for viewer work; for
// thumbnails it is an already granted permit moved out of the request.
fn spawn_decode_with_permit<K, V>(
    cache: &Arc<Mutex<BudgetedLru<K, V>>>,
    key: K,
    permit: impl Future<Output = DecodePermit> + Send + 'static,
    decode: impl Future<Output = PixelResult<V>> + Send + 'static,
) -> InFlight<V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: FrameBody + Send + Sync + 'static,
{
    let (task_cache, task_key) = (Arc::clone(cache), key.clone());
    let task = tokio::spawn(async move {
        let _permit = permit.await;
        // Cleanup belongs to the task too: even an abandoned request and a
        // panicked render must leave no in-flight entry behind.
        let result = std::panic::AssertUnwindSafe(decode)
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                Err(PixelError::frame_decode(anyhow::anyhow!(
                    "decode task panicked"
                )))
            });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::new_raw_cache;

    /// Every committed fixture whose display frame is windowed from the raw
    /// tier renders exactly what its codec's display decoder renders.
    #[tokio::test]
    async fn raw_tier_display_matches_the_codec_display_decoder() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut compared = 0;
        for path in std::fs::read_dir(fixtures).expect("fixture directory") {
            let path = path.expect("fixture entry").path();
            if path.extension().is_none_or(|extension| extension != "dcm") {
                continue;
            }
            let file = Arc::new(crate::loader::test_entry(&path));
            let Some(codec) = codec_for_syntax(&file.transfer_syntax_uid) else {
                continue;
            };
            if !file.has_pixels || reject_unsupported_layout(&file, FrameKind::Display).is_err() {
                continue;
            }
            let raw_cache = new_raw_cache();
            let _ = load_raw_frame(
                file.clone(),
                raw_cache.clone(),
                RawFrameRequest { frame: 0 },
            )
            .await;
            let Some((raw, layout)) =
                raw_samples_for_display(&file, codec, &raw_cache, 0, false).await
            else {
                continue;
            };
            for (center, width, mode) in [
                (None, None, WindowMode::Default),
                (Some(-600.0), Some(1500.0), WindowMode::Default),
                (None, None, WindowMode::FullDynamic),
            ] {
                let window = DisplayWindow {
                    frame: 0,
                    center,
                    width,
                    mode,
                    redaction: 0,
                };
                let decoded = decode_display_frame(codec, file.clone(), window)
                    .await
                    .expect("display decode");
                let windowed = window_raw_samples(file.clone(), raw.clone(), layout, window)
                    .await
                    .expect("raw-tier display");
                // The same image, reporting the same window.
                assert_eq!(
                    (decoded.png, decoded.window),
                    (windowed.png, windowed.window),
                    "{}",
                    path.display()
                );
            }
            compared += 1;
        }
        assert!(compared >= 15, "only {compared} fixtures took the raw tier");
    }
}

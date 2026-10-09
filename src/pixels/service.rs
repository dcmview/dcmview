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

use super::admission::{self, DecodeWork};
use super::cache::{
    BudgetedLru, Flight, FlightWaiters, FrameBody, FrameCache, RawFrameCache, ThumbnailCache,
};
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
use super::raster::{decode_raster_to_png, decode_raw_raster, render_raster};
use super::redaction::{redact_png, redact_raw, RawLayout, Redaction};
use super::render::{
    encode_real_world_windowed_png, encode_windowed_luminance_png, AppliedWindow, DisplayBuffer,
    DisplayPng, LuminanceRenderOptions, StoredSamples,
};
use super::rle::{decode_raw_rle, decode_rle_to_png, render_rle};
use super::schedule::{DecodeClass, DecodePermit, DecodeScheduler};
use super::syntax::{classify_pixel_support, codec_for_file, Codec, PixelSupportReason};
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
///
/// The copy is as large as the frame, so it is made under its own permit
/// ([`DecodeWork::RawRedaction`]), taken after the decode's has been
/// returned. The request waits for that permit itself, so one that is
/// dropped while it waits makes no copy; once the copy has begun it holds
/// the permit until it is done, whether or not the request is still there.
pub async fn load_redacted_raw_frame(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<RawFrameCache>>,
    request: RawFrameRequest,
    redaction: &Redaction,
) -> PixelResult<RawFrameResponse> {
    let scheduler = scheduler_of(&cache)?;
    let mut raw = load_raw_frame(file.clone(), cache, request).await?;
    if !redaction.is_empty() {
        let permit = admission::admit(
            &scheduler,
            DecodeClass::Interactive,
            &file,
            DecodeWork::RawRedaction,
        )
        .await?;
        let (body, metadata, boxes) = (raw.body, raw.metadata.clone(), redaction.boxes.clone());
        raw.body = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            redact_raw(&file, &body, &metadata, &boxes)
        })
        .await
        .map_err(|error| {
            PixelError::raw_decode(anyhow::anyhow!("raw redaction task failed: {error}"))
        })?;
    }
    Ok(raw)
}

/// Who holds the permit a raw decode runs under.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RawAdmission {
    /// The decode admits itself as interactive [`DecodeWork::RawFrame`].
    Own,
    /// The caller holds a permit whose estimate includes this decode
    /// ([`DecodeWork::DisplayFrame`]) and awaits it, so the decode takes
    /// none. Such a caller must never wait for work that may itself be
    /// waiting for a permit, or two display frames could each hold the
    /// permit the other's raw decode needs: it reads the cache, and
    /// otherwise decodes, without joining a decode already announced.
    Covered,
}

pub async fn load_raw_frame(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<RawFrameCache>>,
    request: RawFrameRequest,
) -> PixelResult<RawFrameResponse> {
    raw_frame(file, cache, request, RawAdmission::Own).await
}

async fn raw_frame(
    file: Arc<FileEntry>,
    cache: Arc<Mutex<RawFrameCache>>,
    request: RawFrameRequest,
    admission: RawAdmission,
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
    let decoded_file = file.clone();
    let decode = async move {
        let file = decoded_file;
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
            Codec::Raster => decode_raw_raster(file.clone(), frame).await?,
        };
        if let Some([low, high]) = raw_padding_bounds(&file, &metadata) {
            metadata.padding_low = Some(low);
            metadata.padding_high = Some(high);
        }
        Ok((body, metadata))
    };
    let ((body, metadata), cache_hit) = match admission {
        RawAdmission::Own => {
            cached_or_decoded(&cache, key, file, DecodeWork::RawFrame, decode).await?
        }
        RawAdmission::Covered => covered_decode(&cache, key, decode).await?,
    };

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
        // A real-world window is applied to decoded integer samples, so
        // JPEG 2000 decodes through the raw tier here too. Samples a
        // real-world window cannot apply to show the default window, as a
        // frame without a mapping in that unit does, and report it, so the
        // viewer can tell its window was not applied.
        let render = {
            let (file, raw_cache) = (file.clone(), raw_cache.clone());
            async move {
                match raw_samples_for_display(&file, codec, &raw_cache, display.frame, true).await {
                    Some((raw, layout)) => {
                        window_real_world_samples(file, raw, layout, map, window, display.frame)
                            .await
                    }
                    None => {
                        let display = DisplayWindow {
                            center: None,
                            width: None,
                            ..display
                        };
                        render_display(file, codec, raw_cache, display).await
                    }
                }
            }
        };
        let (display, cache_hit) =
            cached_or_rendered(&cache, key, request.preview, file, redacted(render, boxes)).await?;
        return Ok(FrameResponse::png(display, cache_hit));
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
    let render = redacted(
        render_display(file.clone(), codec, raw_cache, display),
        boxes,
    );
    let (display, cache_hit) =
        cached_or_rendered(&cache, key, request.preview, file, render).await?;

    Ok(FrameResponse::png(display, cache_hit))
}

/// One display frame, from the raw tier when it holds or can decode the
/// frame's samples and otherwise from the codec's display decoder. It runs
/// under the permit of the display frame it is for
/// ([`DecodeWork::DisplayFrame`]), whose estimate includes the decode.
async fn render_display(
    file: Arc<FileEntry>,
    codec: Codec,
    raw_cache: Arc<Mutex<RawFrameCache>>,
    display: DisplayWindow,
) -> PixelResult<DisplayPng> {
    match raw_samples_for_display(&file, codec, &raw_cache, display.frame, false).await {
        Some((raw, layout)) => window_raw_samples(file, raw, layout, display).await,
        None => decode_display_frame(codec, file, display).await,
    }
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
///    of the cache's scheduler for [`DecodeWork::Thumbnail`] inside this
///    future, so a request that is dropped while it waits starts no work
///    and reserves nothing. A refusal is this request's error
///    ([`PixelError::DecodeBusy`], [`PixelError::DecodeMemoryExceeded`]):
///    nothing was started, so no other request shares it. Once it holds the
///    permit, the render runs as its own task, keeps the permit until it
///    ends, and caches its result whether or not the request is still
///    there. Source `FullDecode`, a cache miss.
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
    let (flight, scheduler) = {
        let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
        if let Some(body) = lock.get(&key) {
            return Ok(thumbnail_response(body, true));
        }
        (lock.join_flight(&key), lock.scheduler())
    };
    let (flight, cache_hit) = match flight {
        Some(flight) => (flight, true),
        None => {
            let permit = admission::admit(
                &scheduler,
                DecodeClass::Background,
                &file,
                DecodeWork::Thumbnail,
            )
            .await?;
            // Another request may have started or finished this key while
            // we queued. Claim a flight only while holding the cache lock.
            let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
            if let Some(body) = lock.get(&key) {
                return Ok(thumbnail_response(body, true));
            }
            match lock.join_flight(&key) {
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
                        std::future::ready(Ok(Some(permit))),
                        render,
                    );
                    (lock.start_flight(key, flight), false)
                }
            }
        }
    };
    let body = flight.result().await.map_err(|error| error.duplicate())?;
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
        Codec::Raster => decode_raster_to_png(file, frame, center, width, mode).await?,
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
        Codec::Raster => render_raster(file, frame, center, width, mode).await?,
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
///
/// The caller holds the display frame's permit, so the raw decode takes none
/// ([`RawAdmission::Covered`]).
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
        let raw = raw_frame(
            file.clone(),
            raw_cache.clone(),
            RawFrameRequest { frame },
            RawAdmission::Covered,
        )
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
            // Gray with alpha has two samples and is flattened by its own
            // renderer; wider and floating-point gray is windowed per sample.
            Codec::Raster => {
                monochrome
                    && matches!(
                        file.series_metadata.native_pixel.pixel_data_kind,
                        None | Some(NativePixelDataKind::Integer)
                    )
                    && matches!(file.bits_allocated, 8 | 16)
            }
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
///
/// Either way `render` runs under one interactive permit for
/// [`DecodeWork::DisplayFrame`] on `file`.
///
/// A preview the cache holds is served from it. Otherwise the request
/// waits for its permit itself, so a preview that is dropped while it waits
/// (the drag moved on) renders nothing and reserves nothing. Once it holds
/// the permit, the render runs as its own task and keeps the permit until
/// it ends: a request dropped after that point no longer frees the permit
/// or the memory while its blocking work is still running.
async fn cached_or_rendered(
    cache: &Arc<Mutex<FrameCache>>,
    key: FrameCacheKey,
    preview: bool,
    file: Arc<FileEntry>,
    render: impl Future<Output = PixelResult<DisplayPng>> + Send + 'static,
) -> PixelResult<(DisplayPng, bool)> {
    if !preview {
        return cached_or_decoded(cache, key, file, DecodeWork::DisplayFrame, render).await;
    }
    let scheduler = {
        let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
        if let Some(display) = lock.get(&key) {
            return Ok((display, true));
        }
        lock.scheduler()
    };
    let permit = admission::admit(
        &scheduler,
        DecodeClass::Interactive,
        &file,
        DecodeWork::DisplayFrame,
    )
    .await?;
    let task = tokio::spawn(async move {
        let _permit = permit;
        std::panic::AssertUnwindSafe(render).catch_unwind().await
    });
    match task.await {
        Ok(Ok(result)) => Ok((result?, false)),
        Ok(Err(_)) => Err(PixelError::frame_decode(anyhow::anyhow!(
            "preview task panicked"
        ))),
        Err(join_error) => Err(PixelError::frame_decode(anyhow::anyhow!(
            "preview task failed: {join_error}"
        ))),
    }
}

/// The cached value for `key` (`true`), or the result of `decode` (`false`).
///
/// A request for a key already being decoded awaits that decode and counts as
/// a hit. The decode runs as its own task and caches its result there, so a
/// client that disconnects mid-decode (a slider drag, a held arrow key) still
/// leaves the finished frame cached for the request that follows.
///
/// The task first waits for an interactive permit for `work` on `file` from
/// the cache's scheduler. A refusal is the decode's result, for every
/// request that shared it, and nothing is cached. While it waits, the
/// decode is only as wanted as its requests: when the last request waiting
/// for it is dropped, it leaves the scheduler's queue and is never started,
/// and a later request for `key` announces a new decode.
async fn cached_or_decoded<K, V>(
    cache: &Arc<Mutex<BudgetedLru<K, V>>>,
    key: K,
    file: Arc<FileEntry>,
    work: DecodeWork,
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
        match lock.join_flight(&key) {
            Some(flight) => (flight, true),
            None => {
                let scheduler = lock.scheduler();
                let permit = async move {
                    admission::admit(&scheduler, DecodeClass::Interactive, &file, work)
                        .await
                        .map(Some)
                };
                let flight = spawn_decode_with_permit(cache, key.clone(), permit, decode);
                (lock.start_flight(key, flight), false)
            }
        }
    };
    flight
        .result()
        .await
        .map(|value| (value, cache_hit))
        .map_err(|error| error.duplicate())
}

/// [`cached_or_decoded`] for a caller that already holds a permit covering
/// `decode` ([`RawAdmission::Covered`]): no permit is taken, and nothing is
/// awaited that could be waiting for one. A cached value is returned; with
/// no decode announced for `key`, this one is announced (so requests for
/// the same key share it) and cached; with one announced, which may not
/// have been admitted yet, `decode` runs for this caller alone and its
/// result is cached.
async fn covered_decode<K, V>(
    cache: &Arc<Mutex<BudgetedLru<K, V>>>,
    key: K,
    decode: impl Future<Output = PixelResult<V>> + Send + 'static,
) -> PixelResult<(V, bool)>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: FrameBody + Send + Sync + 'static,
{
    let flight = {
        let mut lock = cache.lock().map_err(|_| cache_poisoned())?;
        if let Some(value) = lock.get(&key) {
            return Ok((value, true));
        }
        if lock.has_flight(&key) {
            Err(decode)
        } else {
            let flight =
                spawn_decode_with_permit(cache, key.clone(), std::future::ready(Ok(None)), decode);
            Ok(lock.start_flight(key.clone(), flight))
        }
    };
    match flight {
        Ok(flight) => flight
            .result()
            .await
            .map(|value| (value, false))
            .map_err(|error| error.duplicate()),
        Err(decode) => {
            let value = decode.await?;
            if let Ok(mut lock) = cache.lock() {
                lock.insert(key, value.clone());
            }
            Ok((value, false))
        }
    }
}

/// Runs `decode` as its own task once `permit` resolves, holding what it
/// resolves to until the decode ends. The task caches the result and clears
/// the in-flight entry whether or not anyone is still waiting for it. A
/// refused permit is the result, and `decode` is never polled.
///
/// The permit future is awaited in the detached task for viewer work; for
/// thumbnails it is an already granted permit moved out of the request, and
/// for a decode its caller's permit covers it is no permit at all.
///
/// While the permit future is pending, the task also watches the requests
/// waiting for the flight ([`BudgetedLru::join_flight`]). When the last of
/// them is dropped before the permit is granted, the flight is abandoned:
/// its in-flight entry is removed, the permit future is dropped, which
/// gives up its place in the scheduler's queue, and `decode` is never
/// polled. A decode whose permit has been granted runs to its end and is
/// cached whoever is still waiting.
fn spawn_decode_with_permit<K, V>(
    cache: &Arc<Mutex<BudgetedLru<K, V>>>,
    key: K,
    permit: impl Future<Output = PixelResult<Option<DecodePermit>>> + Send + 'static,
    decode: impl Future<Output = PixelResult<V>> + Send + 'static,
) -> Flight<V>
where
    K: Hash + Eq + Clone + Send + 'static,
    V: FrameBody + Send + Sync + 'static,
{
    let waiters = Arc::new(FlightWaiters::default());
    let (task_cache, task_key, task_waiters) =
        (Arc::clone(cache), key.clone(), Arc::clone(&waiters));
    let task = tokio::spawn(async move {
        let admitted = {
            let mut permit = std::pin::pin!(permit);
            loop {
                tokio::select! {
                    // A permit that is ready is taken, wanted or not: the
                    // scheduler has already granted it.
                    biased;
                    admitted = &mut permit => break Some(admitted),
                    () = task_waiters.none_left() => {
                        // Requests join under the cache lock, so under it
                        // "nobody is waiting" stays true once seen.
                        // A poisoned cache serves nobody: give up as well.
                        let abandoned = task_cache.lock().map_or(true, |mut lock| {
                            lock.abandon_flight_if_unwanted(&task_key, &task_waiters)
                        });
                        if abandoned {
                            break None;
                        }
                    }
                }
            }
        };
        let Some(admitted) = admitted else {
            // Nobody holds this flight any longer, so nobody reads this.
            return Err(PixelError::DecodeBusy);
        };
        // Cleanup belongs to the task too: even an abandoned request, a
        // refused permit and a panicked render must leave no in-flight
        // entry behind.
        let result = match admitted {
            Ok(permit) => {
                let result = std::panic::AssertUnwindSafe(decode)
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        Err(PixelError::frame_decode(anyhow::anyhow!(
                            "decode task panicked"
                        )))
                    });
                drop(permit);
                result
            }
            Err(refusal) => Err(refusal),
        };
        if let Ok(mut lock) = task_cache.lock() {
            lock.finish_flight(&task_key, &task_waiters);
            if let Ok(value) = &result {
                lock.insert(task_key, value.clone());
            }
        }
        result
    });
    let (cache, flight_waiters) = (Arc::clone(cache), Arc::clone(&waiters));
    let decode = async move {
        match task.await {
            Ok(result) => result.map_err(Arc::new),
            Err(join_error) => {
                // A panicked decode never reached its own cleanup.
                if let Ok(mut lock) = cache.lock() {
                    lock.finish_flight(&key, &flight_waiters);
                }
                Err(Arc::new(PixelError::frame_decode(anyhow::anyhow!(
                    "decode task failed: {join_error}"
                ))))
            }
        }
    }
    .boxed()
    .shared();
    Flight::new(decode, waiters)
}

/// The scheduler that admits the decodes filling `cache`.
fn scheduler_of<K, V>(cache: &Arc<Mutex<BudgetedLru<K, V>>>) -> PixelResult<Arc<DecodeScheduler>>
where
    K: Hash + Eq,
    V: FrameBody,
{
    Ok(cache.lock().map_err(|_| cache_poisoned())?.scheduler())
}

/// Runs `draw`, which draws and encodes one presentation layer of `file`,
/// on the blocking pool under an interactive permit for
/// [`DecodeWork::PresentationLayer`].
///
/// The request waits for the permit itself, so one that is dropped while it
/// waits draws nothing. The permit then belongs to the blocking work and is
/// held until `draw` returns, whether or not the request is still there.
pub async fn draw_presentation_layer<T: Send + 'static>(
    scheduler: &Arc<DecodeScheduler>,
    file: &FileEntry,
    draw: impl FnOnce() -> T + Send + 'static,
) -> PixelResult<T> {
    let permit = admission::admit(
        scheduler,
        DecodeClass::Interactive,
        file,
        DecodeWork::PresentationLayer,
    )
    .await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        draw()
    })
    .await
    .map_err(|error| {
        PixelError::frame_decode(anyhow::anyhow!("presentation layer task failed: {error}"))
    })
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
    // No tier serves a raster the catalog reports as unsupported: its reason
    // is why nothing decodes it.
    let applies = support.state == SupportState::Unsupported
        && (kind == FrameKind::Display
            || file.format.is_raster()
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
    codec_for_file(file)
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
            let Some(codec) = codec_for_file(&file) else {
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

    /// A gray raster frame is the same image, with the same window, whether
    /// it is windowed from the raw tier (the viewer's display frames) or
    /// rendered by the raster decoder itself (thumbnails).
    #[tokio::test]
    async fn a_gray_raster_renders_alike_from_the_raw_tier_and_from_its_own_decode() {
        use image::{ExtendedColorType, ImageEncoder};
        use tiff::encoder::{colortype, TiffEncoder};

        let dir = tempfile::tempdir().expect("temp dir");
        let ramp8: Vec<u8> = (0..64).map(|index| index * 4).collect();
        let ramp16: Vec<u8> = (0..64_u16)
            .flat_map(|index| (index * 1000).to_ne_bytes())
            .collect();
        for (name, color, data) in [
            ("gray8.png", ExtendedColorType::L8, &ramp8),
            ("gray16.png", ExtendedColorType::L16, &ramp16),
        ] {
            let mut bytes = Vec::new();
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(data, 8, 8, color)
                .expect("encode PNG");
            std::fs::write(dir.path().join(name), bytes).expect("write PNG");
        }
        // Signed samples, shown inverted: WhiteIsZero.
        let signed: Vec<i16> = (0..64).map(|index| (index - 32) * 500).collect();
        let mut bytes = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = TiffEncoder::new(&mut bytes).expect("TIFF encoder");
            let mut image = encoder
                .new_image::<colortype::GrayI16>(8, 8)
                .expect("TIFF page");
            image
                .encoder()
                .write_tag(tiff::tags::Tag::PhotometricInterpretation, 0_u16)
                .expect("TIFF tag");
            image.write_data(&signed).expect("TIFF samples");
        }
        std::fs::write(dir.path().join("int16.tif"), bytes.into_inner()).expect("write TIFF");

        for name in ["gray8.png", "gray16.png", "int16.tif"] {
            let file = Arc::new(crate::loader::test_entry(&dir.path().join(name)));
            let codec = codec_for_file(&file).expect("a raster has a codec");
            assert_eq!(codec, Codec::Raster, "{name}");
            let raw_cache = new_raw_cache();
            let (raw, layout) = raw_samples_for_display(&file, codec, &raw_cache, 0, false)
                .await
                .unwrap_or_else(|| panic!("{name} takes the raw tier"));
            for (center, width, mode) in [
                (None, None, WindowMode::Default),
                (Some(100.0), Some(5000.0), WindowMode::Default),
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
                assert_eq!(
                    (decoded.png, decoded.window),
                    (windowed.png, windowed.window),
                    "{name}"
                );
            }
        }
    }
}

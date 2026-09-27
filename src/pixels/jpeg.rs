use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use dicom_pixeldata::PixelDecoder;
use tokio::task;

use super::color::color_samples_to_rgb8;
use super::encapsulated::open_for_frame_decode;
use super::error::{PixelError, PixelResult};
use super::icc::select_icc_profile;
use super::render::{
    encode_rgb8_display_png, encode_windowed_luminance_png, LuminanceRenderOptions, StoredSamples,
};
use super::syntax::{Codec, ColorSamples};

/// Displays one JPEG Baseline or JPEG Lossless frame decoded by dicom-pixeldata.
pub(crate) async fn decode_compressed_frame_to_png(
    codec: Codec,
    file: FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<Bytes> {
    task::spawn_blocking(move || {
        decode_compressed_frame_to_png_blocking(
            codec,
            &file,
            frame,
            requested_wc,
            requested_ww,
            window_mode,
        )
    })
    .await
    .context("compressed decode task failed")?
}

fn decode_compressed_frame_to_png_blocking(
    codec: Codec,
    file: &FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> Result<Bytes> {
    let (obj, frame_in_object) = open_for_frame_decode(file, frame)?;
    // Decode only the requested frame; the result holds that frame at index 0.
    let decoded = obj
        .decode_pixel_data_frame(frame_in_object)
        .context("JPEG frame decode failed")?;
    if decoded.samples_per_pixel() == 3 {
        if decoded.bits_allocated() != 8 {
            return Err(anyhow!(
                "unsupported color BitsAllocated {}",
                decoded.bits_allocated()
            ));
        }
        let decoded_frame = decoded.frame_data(0)?;
        let photometric = file.photometric_interpretation.trim().to_ascii_uppercase();
        // The table names the layouts whose samples still need converting;
        // any other three-sample frame is displayed as the decoder produced it.
        let rgb = match codec.color_samples(&photometric, 8) {
            Some(samples @ ColorSamples::YbrFull) => {
                let pixel_count = decoded_frame.len() / 3;
                color_samples_to_rgb8(samples, decoded_frame, pixel_count, 0)?
            }
            Some(ColorSamples::Rgb) | None => decoded_frame.to_vec(),
        };
        return encode_rgb8_display_png(
            file,
            frame,
            rgb,
            decoded.columns(),
            decoded.rows(),
            select_icc_profile(&obj),
        )
        .context("color PNG encoding failed");
    }
    if decoded.samples_per_pixel() != 1 {
        return Err(anyhow!(
            "unsupported SamplesPerPixel {}",
            decoded.samples_per_pixel()
        ));
    }

    let bits_allocated = u32::from(decoded.bits_allocated());
    if !matches!(bits_allocated, 8 | 16) {
        return Err(anyhow!("unsupported BitsAllocated {bits_allocated}"));
    }
    // Decoded samples are in host byte order, little endian on every
    // supported host.
    encode_windowed_luminance_png(
        file,
        StoredSamples::Integer {
            bytes: decoded.frame_data(0)?,
            bits_allocated,
            signed: file.pixel_representation == 1,
        },
        LuminanceRenderOptions {
            frame,
            rows: decoded.rows(),
            columns: decoded.columns(),
            requested_wc,
            requested_ww,
            window_mode,
        },
    )
}

pub(crate) async fn read_raw_jpeg_samples(
    file: FileEntry,
    frame: u32,
) -> Result<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || read_raw_jpeg_samples_blocking(&file, frame))
        .await
        .context("raw JPEG sample read task failed")?
}

fn read_raw_jpeg_samples_blocking(
    file: &FileEntry,
    frame: u32,
) -> Result<(Bytes, RawFrameMetadata)> {
    let (obj, frame_in_object) = open_for_frame_decode(file, frame)?;
    // The transfer-syntax adapter assembles every fragment belonging to the
    // requested frame using the Basic Offset Table before decoding. A frame is
    // not required to have a one-to-one relationship with a fragment.
    let decoded = obj
        .decode_pixel_data_frame(frame_in_object)
        .context("JPEG decode failed for raw samples")?;
    let bits_allocated = decoded.bits_allocated() as u32;
    let samples_per_pixel = decoded.samples_per_pixel() as u32;
    let photometric_interpretation = match samples_per_pixel {
        1 => "MONOCHROME2",
        3 => "RGB",
        value => {
            return Err(anyhow!(
                "raw JPEG does not support decoded SamplesPerPixel {value}"
            ));
        }
    };
    let decoded_frame = decoded.frame_data(0)?;
    let samples = match (bits_allocated, samples_per_pixel) {
        (8, 1 | 3) => Bytes::copy_from_slice(decoded_frame),
        (16, 1) => Bytes::from(
            decoded_frame
                .chunks_exact(2)
                .flat_map(|sample| u16::from_ne_bytes([sample[0], sample[1]]).to_le_bytes())
                .collect::<Vec<_>>(),
        ),
        _ => {
            return Err(anyhow!(
                "raw JPEG does not support decoded BitsAllocated {bits_allocated} with SamplesPerPixel {samples_per_pixel}"
            ));
        }
    };
    let mut metadata = file.raw_metadata(
        decoded.rows(),
        decoded.columns(),
        bits_allocated,
        samples_per_pixel,
    );
    // JPEG Baseline output from the decoder is canonical unsigned,
    // color-by-pixel RGB (or grayscale), regardless of stored DICOM layout.
    metadata.pixel_representation = 0;
    metadata.photometric_interpretation = photometric_interpretation.to_string();
    Ok((samples, metadata))
}

pub(crate) async fn decode_raw_jpeg_lossless(
    file: FileEntry,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || decode_raw_jpeg_lossless_blocking(&file, frame))
        .await
        .map_err(|error| {
            PixelError::raw_decode(anyhow!("raw JPEG Lossless decode task failed: {error}"))
        })?
}

fn decode_raw_jpeg_lossless_blocking(
    file: &FileEntry,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    let (obj, frame_in_object) =
        open_for_frame_decode(file, frame).map_err(PixelError::raw_decode)?;

    let decoded = obj
        .decode_pixel_data_frame(frame_in_object)
        .context("JPEG frame decode failed")
        .map_err(PixelError::raw_decode)?;
    if decoded.samples_per_pixel() != 1 {
        return Err(PixelError::UnsupportedLayout(format!(
            "raw JPEG Lossless requires one sample per pixel, decoded {}",
            decoded.samples_per_pixel()
        )));
    }

    let bits_allocated = decoded.bits_allocated() as u32;
    let sample_bytes = match bits_allocated {
        8 => Bytes::copy_from_slice(
            decoded
                .frame_data(0)
                .map_err(anyhow::Error::from)
                .map_err(PixelError::raw_decode)?,
        ),
        16 => {
            let bytes = decoded
                .frame_data_ow(0)
                .map_err(anyhow::Error::from)
                .map_err(PixelError::raw_decode)?
                .into_iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>();
            Bytes::from(bytes)
        }
        _ => {
            return Err(PixelError::UnsupportedLayout(format!(
                "raw JPEG Lossless does not support BitsAllocated {bits_allocated}"
            )));
        }
    };

    let metadata = file.raw_metadata(decoded.rows(), decoded.columns(), bits_allocated, 1);
    Ok((sample_bytes, metadata))
}

#[cfg(test)]
mod tests {
    use super::{decode_compressed_frame_to_png_blocking, read_raw_jpeg_samples_blocking};
    use crate::api::contracts::WindowMode;
    use crate::pixels::Codec;
    use crate::types::FileEntry;
    use dicom_core::{value::PixelFragmentSequence, DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    use image::{codecs::jpeg::JpegEncoder, RgbImage};
    use tempfile::tempdir;

    #[test]
    fn raw_baseline_rgb_assembles_multifragment_frame_as_interleaved_rgb() {
        let source = RgbImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, // red
                0, 255, 0, // green
                0, 0, 255, // blue
                255, 255, 255, // white
            ],
        )
        .unwrap();
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&source)
            .unwrap();
        if !jpeg.len().is_multiple_of(2) {
            jpeg.push(0);
        }
        let split = (jpeg.len() / 2) & !1;
        let fragments = vec![jpeg[..split].to_vec(), jpeg[split..].to_vec()];

        let mut obj = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.9001"),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
            DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
            DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
            DataElement::new(
                tags::PIXEL_REPRESENTATION,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(3_u16)),
            DataElement::new(
                tags::PHOTOMETRIC_INTERPRETATION,
                VR::CS,
                PrimitiveValue::from("RGB"),
            ),
            DataElement::new(
                tags::PLANAR_CONFIGURATION,
                VR::US,
                PrimitiveValue::from(0_u16),
            ),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "1"),
        ]);
        obj.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new(vec![0], fragments),
        ));

        let directory = tempdir().unwrap();
        let path = directory.path().join("multifragment-rgb-baseline.dcm");
        obj.with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(uids::JPEG_BASELINE8_BIT)
                .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.9001"),
        )
        .unwrap()
        .write_to_file(&path)
        .unwrap();

        let file = FileEntry {
            index: 0,
            path,
            label: "fixture".to_string(),
            patient_id: String::new(),
            patient_name: String::new(),
            study_instance_uid: String::new(),
            study_date: String::new(),
            study_description: String::new(),
            series_instance_uid: String::new(),
            series_number: String::new(),
            series_description: String::new(),
            modality: "OT".to_string(),
            instance_number: "1".to_string(),
            sop_instance_uid: "2.25.9001".to_string(),
            sop_class_uid: uids::SECONDARY_CAPTURE_IMAGE_STORAGE.to_string(),
            series_metadata: Default::default(),
            has_pixels: true,
            frame_count: 1,
            rows: 2,
            columns: 2,
            bits_allocated: 8,
            pixel_representation: 0,
            samples_per_pixel: 3,
            photometric_interpretation: "RGB".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            transfer_syntax_uid: uids::JPEG_BASELINE8_BIT.to_string(),
            default_window: None,
        };

        let (body, metadata) = read_raw_jpeg_samples_blocking(&file, 0).unwrap();
        assert_eq!(body.len(), 12);
        assert_eq!(metadata.rows, 2);
        assert_eq!(metadata.columns, 2);
        assert_eq!(metadata.bits_allocated, 8);
        assert_eq!(metadata.samples_per_pixel, 3);
        assert_eq!(metadata.pixel_representation, 0);
        assert_eq!(metadata.photometric_interpretation, "RGB");

        let pixels = body.chunks_exact(3).collect::<Vec<_>>();
        assert!(pixels[0][0] > pixels[0][1] && pixels[0][0] > pixels[0][2]);
        assert!(pixels[1][1] > pixels[1][0] && pixels[1][1] > pixels[1][2]);
        assert!(pixels[2][2] > pixels[2][0] && pixels[2][2] > pixels[2][1]);
        assert!(pixels[3].iter().all(|value| *value > 200));

        let png = decode_compressed_frame_to_png_blocking(
            Codec::JpegBaseline,
            &file,
            0,
            None,
            None,
            WindowMode::Default,
        )
        .unwrap();
        let display = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(display.dimensions(), (2, 2));
        let display_pixels = display.into_raw();
        let display_pixels = display_pixels.chunks_exact(3).collect::<Vec<_>>();
        assert!(display_pixels[0][0] > display_pixels[0][1]);
        assert!(display_pixels[1][1] > display_pixels[1][0]);
        assert!(display_pixels[2][2] > display_pixels[2][0]);
        assert!(display_pixels[3].iter().all(|value| *value > 200));
    }
}

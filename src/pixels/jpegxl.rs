use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::FileEntry;
use anyhow::{anyhow, Result};
use bytes::Bytes;
use tokio::task;

use super::color::color_samples_to_rgb8;
use super::error::{PixelError, PixelResult};
use super::pixeldata_frame::{self, DecodedFrame};
use super::render::{
    encode_rgb8_display_png, encode_windowed_luminance_png, LuminanceRenderOptions, StoredSamples,
};
use super::syntax::{Codec, ColorSamples};

pub(crate) async fn decode_jpeg_xl_to_png(
    file: FileEntry,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> PixelResult<Bytes> {
    task::spawn_blocking(move || {
        let decoded = decode_frame(&file, frame).map_err(PixelError::frame_decode)?;
        match (decoded.bits_allocated, decoded.samples_per_pixel) {
            (8, 3) => {
                let photometric = file.photometric_interpretation.trim().to_ascii_uppercase();
                let samples = Codec::JpegXl.color_samples(&photometric, 8).ok_or_else(|| {
                    PixelError::UnsupportedLayout(format!(
                        "JPEG XL Lossless display does not support PhotometricInterpretation {}",
                        file.photometric_interpretation
                    ))
                })?;
                let pixel_count = decoded.bytes.len() / 3;
                let rgb = color_samples_to_rgb8(samples, &decoded.bytes, pixel_count, 0)
                    .map_err(PixelError::frame_decode)?;
                encode_rgb8_display_png(
                    &file,
                    frame,
                    rgb,
                    decoded.columns,
                    decoded.rows,
                    decoded.icc_profile,
                )
                    .map_err(PixelError::frame_decode)
            }
            (8, 1) => {
                let samples = StoredSamples::Integer {
                    bytes: &decoded.bytes,
                    bits_allocated: 8,
                    signed: false,
                };
                encode_monochrome(
                    &file,
                    samples,
                    frame,
                    decoded.rows,
                    decoded.columns,
                    requested_wc,
                    requested_ww,
                    window_mode,
                )
            }
            (16, 1) => {
                let samples = StoredSamples::Integer {
                    bytes: &decoded.bytes,
                    bits_allocated: 16,
                    signed: file.pixel_representation == 1,
                };
                encode_monochrome(
                    &file,
                    samples,
                    frame,
                    decoded.rows,
                    decoded.columns,
                    requested_wc,
                    requested_ww,
                    window_mode,
                )
            }
            (bits, samples) => Err(PixelError::UnsupportedLayout(format!(
                "JPEG XL Lossless display does not support BitsAllocated {bits} with SamplesPerPixel {samples}"
            ))),
        }
    })
    .await
    .map_err(|error| PixelError::frame_decode(anyhow!("JPEG XL decode task failed: {error}")))?
}

pub(crate) async fn decode_raw_jpeg_xl(
    file: FileEntry,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || {
        let decoded = decode_frame(&file, frame).map_err(PixelError::raw_decode)?;
        match (decoded.bits_allocated, decoded.samples_per_pixel) {
            (8, 1 | 3) | (16, 1) => {}
            (bits, samples) => {
                return Err(PixelError::UnsupportedLayout(format!(
                    "raw JPEG XL Lossless does not support BitsAllocated {bits} with SamplesPerPixel {samples}"
                )));
            }
        }
        let mut metadata = file.raw_metadata(
            decoded.rows,
            decoded.columns,
            decoded.bits_allocated,
            decoded.samples_per_pixel,
        );
        metadata.pixel_representation = if decoded.samples_per_pixel == 3 {
            0
        } else {
            file.pixel_representation
        };
        // Three-sample frames are returned as decoded: RGB, or the stored
        // Y, Cb, Cr channels of a YBR_FULL frame.
        metadata.photometric_interpretation = if decoded.samples_per_pixel == 3 {
            let photometric = file.photometric_interpretation.trim().to_ascii_uppercase();
            match Codec::JpegXl.color_samples(&photometric, decoded.bits_allocated) {
                Some(ColorSamples::YbrFull) => "YBR_FULL".to_string(),
                _ => "RGB".to_string(),
            }
        } else {
            file.photometric_interpretation.clone()
        };
        Ok((Bytes::from(decoded.bytes), metadata))
    })
    .await
    .map_err(|error| PixelError::raw_decode(anyhow!("raw JPEG XL decode task failed: {error}")))?
}

fn decode_frame(file: &FileEntry, frame: u32) -> Result<DecodedFrame> {
    pixeldata_frame::decode_frame(file, frame, "JPEG XL Lossless")
}

#[allow(clippy::too_many_arguments)]
fn encode_monochrome(
    file: &FileEntry,
    samples: StoredSamples<'_>,
    frame: u32,
    rows: u32,
    columns: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> PixelResult<Bytes> {
    encode_windowed_luminance_png(
        file,
        samples,
        LuminanceRenderOptions {
            frame,
            rows,
            columns,
            requested_wc,
            requested_ww,
            window_mode,
        },
    )
    .map_err(PixelError::frame_decode)
}

#[cfg(test)]
mod tests {
    use super::{decode_jpeg_xl_to_png, decode_raw_jpeg_xl};
    use crate::api::contracts::WindowMode;
    use crate::types::FileEntry;
    use dicom_core::{value::PixelFragmentSequence, DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    use tempfile::tempdir;

    const JPEG_XL_LOSSLESS_UID: &str = "1.2.840.10008.1.2.4.110";
    const RGB_QUADRANTS: [u8; 12] = [
        255, 0, 0, // red
        0, 255, 0, // green
        0, 0, 255, // blue
        255, 255, 255, // white
    ];
    const RGB_QUADRANTS_JXL: &[u8] = &[
        0xff, 0x0a, 0x08, 0x00, 0x02, 0x80, 0x48, 0x08, 0x02, 0x01, 0x00, 0xcc, 0x02, 0x4b, 0x18,
        0x9b, 0x9c, 0x71, 0x84, 0x03, 0x38, 0x80, 0x03, 0x38, 0x20, 0x4a, 0xc0, 0x39, 0x05, 0x01,
        0x00, 0x20, 0x44, 0x80, 0x08, 0x10, 0x01, 0x22, 0x40, 0x84, 0xff, 0xf7, 0xef, 0xf9, 0xef,
        0xa1, 0x31, 0xe7, 0x9c, 0x6b, 0xed, 0x73, 0x6f, 0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55,
        0x55, 0x55, 0xd5, 0xff, 0xff, 0xff, 0x73, 0xef, 0xeb, 0xee, 0xee, 0xee, 0x86, 0xff, 0xf7,
        0xef, 0xf9, 0xef, 0xa1, 0x31, 0xe7, 0x9c, 0x6b, 0xed, 0x73, 0x6f, 0x92, 0x24, 0x09, 0x01,
        0x55, 0x55, 0x55, 0x55, 0x55, 0xd5, 0xff, 0xff, 0xff, 0x73, 0xef, 0xeb, 0xee, 0xee, 0xee,
        0x86, 0xff, 0xf7, 0xef, 0xf9, 0xef, 0xa1, 0x31, 0xe7, 0x9c, 0x6b, 0xed, 0x73, 0x6f, 0x92,
        0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5, 0xff, 0xff, 0xff, 0x73, 0xef, 0xeb,
        0xee, 0xee, 0xee, 0x86, 0xff, 0xf7, 0xef, 0xf9, 0xef, 0xa1, 0x31, 0xe7, 0x9c, 0x6b, 0xed,
        0x73, 0x6f, 0x92, 0x24, 0x09, 0x01, 0x55, 0x55, 0x55, 0x55, 0x55, 0xd5, 0xff, 0xff, 0xff,
        0x73, 0xef, 0xeb, 0xee, 0xee, 0xee, 0x3e, 0x00, 0xc7, 0xbf, 0x00, 0x3c, 0x00, 0x1e, 0xfe,
        0x8f, 0xfe, 0xe7, 0xfe, 0x87, 0xff, 0xd5, 0x7f, 0xf2, 0xe3, 0xd1, 0x0f,
    ];

    fn write_fixture(path: &std::path::Path) -> FileEntry {
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.9110"),
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
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new(vec![0], vec![RGB_QUADRANTS_JXL.to_vec()]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(JPEG_XL_LOSSLESS_UID)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.9110"),
            )
            .unwrap()
            .write_to_file(path)
            .unwrap();

        FileEntry {
            index: 0,
            path: path.to_path_buf(),
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
            sop_instance_uid: "2.25.9110".to_string(),
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
            transfer_syntax_uid: JPEG_XL_LOSSLESS_UID.to_string(),
            default_window: None,
        }
    }

    #[tokio::test]
    async fn lossless_rgb_preserves_all_channels_for_raw_and_display() {
        let directory = tempdir().unwrap();
        let file = write_fixture(&directory.path().join("rgb-lossless-jxl.dcm"));

        let (raw, metadata) = decode_raw_jpeg_xl(file.clone(), 0).await.unwrap();
        assert_eq!(raw.as_ref(), RGB_QUADRANTS);
        assert_eq!(metadata.rows, 2);
        assert_eq!(metadata.columns, 2);
        assert_eq!(metadata.bits_allocated, 8);
        assert_eq!(metadata.samples_per_pixel, 3);
        assert_eq!(metadata.pixel_representation, 0);
        assert_eq!(metadata.photometric_interpretation, "RGB");

        let png = decode_jpeg_xl_to_png(file, 0, None, None, WindowMode::Default)
            .await
            .unwrap();
        let rendered = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(rendered.dimensions(), (2, 2));
        assert_eq!(rendered.into_raw(), RGB_QUADRANTS);
    }
}

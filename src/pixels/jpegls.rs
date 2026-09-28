use crate::api::contracts::{RawFrameMetadata, WindowMode};
use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use std::sync::Arc;
use tokio::task;

use super::error::{PixelError, PixelResult};
use super::pixeldata_frame::{self, DecodedFrame};
use super::render::{encode_windowed_luminance_png, LuminanceRenderOptions, StoredSamples};

pub(crate) async fn decode_jpeg_ls_to_png(
    file: Arc<FileEntry>,
    frame: u32,
    requested_wc: Option<f64>,
    requested_ww: Option<f64>,
    window_mode: WindowMode,
) -> PixelResult<Bytes> {
    task::spawn_blocking(move || {
        let decoded = decode_frame(&file, frame).map_err(PixelError::frame_decode)?;
        if !matches!(decoded.bits_allocated, 8 | 16) {
            return Err(PixelError::UnsupportedLayout(format!(
                "JPEG-LS Lossless display does not support BitsAllocated {}",
                decoded.bits_allocated
            )));
        }
        encode_windowed_luminance_png(
            &file,
            StoredSamples::Integer {
                bytes: &decoded.bytes,
                bits_allocated: decoded.bits_allocated,
                signed: file.pixel_representation == 1,
            },
            LuminanceRenderOptions {
                frame,
                rows: decoded.rows,
                columns: decoded.columns,
                requested_wc,
                requested_ww,
                window_mode,
            },
        )
        .map_err(PixelError::frame_decode)
    })
    .await
    .map_err(|error| PixelError::frame_decode(anyhow!("JPEG-LS decode task failed: {error}")))?
}

pub(crate) async fn decode_raw_jpeg_ls(
    file: Arc<FileEntry>,
    frame: u32,
) -> PixelResult<(Bytes, RawFrameMetadata)> {
    task::spawn_blocking(move || {
        let decoded = decode_frame(&file, frame).map_err(PixelError::raw_decode)?;
        let metadata = file.raw_metadata(decoded.rows, decoded.columns, decoded.bits_allocated, 1);
        Ok((Bytes::from(decoded.bytes), metadata))
    })
    .await
    .map_err(|error| PixelError::raw_decode(anyhow!("raw JPEG-LS decode task failed: {error}")))?
}

fn decode_frame(file: &FileEntry, frame: u32) -> Result<DecodedFrame> {
    let decoded = pixeldata_frame::decode_frame(file, frame, "JPEG-LS Lossless")?;
    if !matches!(decoded.bits_allocated, 8 | 16) {
        return Err(anyhow!(
            "JPEG-LS Lossless does not support decoded BitsAllocated {}",
            decoded.bits_allocated
        ));
    }
    if decoded.samples_per_pixel != 1 {
        return Err(anyhow!(
            "JPEG-LS Lossless requires one decoded sample per pixel, found {}",
            decoded.samples_per_pixel
        ));
    }
    let expected_len = usize::try_from(decoded.rows)?
        .checked_mul(usize::try_from(decoded.columns)?)
        .and_then(|pixels| pixels.checked_mul((decoded.bits_allocated / 8) as usize))
        .context("decoded JPEG-LS frame size overflow")?;
    if decoded.bytes.len() != expected_len {
        return Err(anyhow!(
            "decoded JPEG-LS frame length {} does not match expected {expected_len}",
            decoded.bytes.len()
        ));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use crate::api::contracts::WindowMode;
    use crate::pixels::{
        load_frame, load_raw_frame, new_cache, new_raw_cache, FrameRequest, RawFrameRequest,
    };
    use crate::types::FileEntry;
    use dicom_core::{value::PixelFragmentSequence, DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    use tempfile::tempdir;

    const JPEG_LS_LOSSLESS_UID: &str = "1.2.840.10008.1.2.4.80";
    const EXPECTED_SAMPLES: [u8; 4] = [0, 85, 170, 255];
    const JPEG_LS_FRAME: &[u8] = &[
        0xff, 0xd8, 0xff, 0xf7, 0x00, 0x0b, 0x08, 0x00, 0x02, 0x00, 0x02, 0x01, 0x01, 0x11, 0x00,
        0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00, 0xd4, 0x00,
        0x00, 0x00, 0xd5, 0x00, 0x00, 0x00, 0xd4, 0x80, 0xff, 0xd9,
    ];

    fn write_fixture(path: &std::path::Path) -> FileEntry {
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.9080"),
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
            DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
            DataElement::new(
                tags::PHOTOMETRIC_INTERPRETATION,
                VR::CS,
                PrimitiveValue::from("MONOCHROME2"),
            ),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "1"),
        ]);
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new(vec![0], vec![JPEG_LS_FRAME.to_vec()]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(JPEG_LS_LOSSLESS_UID)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.9080"),
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
            sop_instance_uid: "2.25.9080".to_string(),
            sop_class_uid: uids::SECONDARY_CAPTURE_IMAGE_STORAGE.to_string(),
            series_metadata: Default::default(),
            has_pixels: true,
            frame_count: 1,
            rows: 2,
            columns: 2,
            bits_allocated: 8,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            transfer_syntax_uid: JPEG_LS_LOSSLESS_UID.to_string(),
            default_window: None,
        }
    }

    #[tokio::test]
    async fn lossless_grayscale_routes_exact_samples_to_raw_and_display() {
        let directory = tempdir().unwrap();
        let file = write_fixture(&directory.path().join("mono2-lossless-jpegls.dcm"));

        let raw = load_raw_frame(
            file.clone().into(),
            new_raw_cache(),
            RawFrameRequest { frame: 0 },
        )
        .await
        .unwrap();
        assert_eq!(raw.body.as_ref(), EXPECTED_SAMPLES);
        assert_eq!(raw.metadata.rows, 2);
        assert_eq!(raw.metadata.columns, 2);
        assert_eq!(raw.metadata.bits_allocated, 8);
        assert_eq!(raw.metadata.samples_per_pixel, 1);
        assert_eq!(raw.metadata.pixel_representation, 0);
        assert_eq!(raw.metadata.photometric_interpretation, "MONOCHROME2");

        let display = load_frame(
            file.into(),
            new_cache(),
            new_raw_cache(),
            FrameRequest {
                frame: 0,
                window_center: None,
                window_width: None,
                window_mode: WindowMode::FullDynamic,
                real_world: None,
                preview: false,
            },
        )
        .await
        .unwrap();
        let rendered = image::load_from_memory(&display.body).unwrap().to_luma8();
        assert_eq!(rendered.dimensions(), (2, 2));
        assert_eq!(rendered.into_raw(), [0, 85, 171, 255]);
    }
}

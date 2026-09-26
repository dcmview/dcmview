use super::support;
use dcmview::pixels::{
    load_frame, load_raw_frame, new_cache, new_raw_cache, FrameRequest, RawFrameRequest,
};
use dcmview::types::WindowPreset;
use image::ImageFormat;
use tempfile::tempdir;

#[tokio::test]
async fn decodes_uncompressed_png_and_tracks_window_cache_keys() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("uncompressed-le.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000, 500, 1500, 2500, 3500],
        Some("1500"),
        Some("3000"),
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 2);
    entry.rows = 2;
    entry.columns = 2;
    entry.default_window = Some(WindowPreset {
        center: 1500.0,
        width: 3000.0,
    });

    let cache = new_cache();

    let first = load_frame(
        entry.clone(),
        cache.clone(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("first uncompressed frame");
    assert_eq!(first.content_type, "image/png");
    assert!(!first.cache_hit);

    let first_image = image::load_from_memory_with_format(first.body.as_ref(), ImageFormat::Png)
        .expect("valid png")
        .to_luma8();
    assert_eq!(first_image.width(), 2);
    assert_eq!(first_image.height(), 2);

    let second = load_frame(
        entry.clone(),
        cache.clone(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("second uncompressed frame");
    assert!(second.cache_hit);

    let overridden = load_frame(
        entry,
        cache,
        FrameRequest {
            frame: 0,
            window_center: Some(800.0),
            window_width: Some(1000.0),
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("window override frame");
    assert!(!overridden.cache_hit);
    assert_ne!(first.body.as_ref(), overridden.body.as_ref());
}

#[tokio::test]
async fn native_overlay_composites_after_windowing_without_changing_raw_samples() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("overlay.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.1",
        2,
        2,
        vec![0, 1000, 2000, 3000],
        Some("1500"),
        Some("3000"),
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 2;
    entry.columns = 2;
    entry.default_window = Some(WindowPreset {
        center: 1500.0,
        width: 3000.0,
    });
    entry
        .series_metadata
        .presentation
        .overlay_planes
        .push(dcmview::types::OverlayPlane {
            group: 0x6000,
            rows: 2,
            columns: 2,
            origin: [1, 1],
            overlay_type: "G".to_string(),
            number_of_frames: 1,
            image_frame_origin: 1,
            data: vec![0x0009],
        });

    let display = load_frame(
        entry.clone(),
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("overlay display frame");
    let raw = load_raw_frame(entry, new_raw_cache(), RawFrameRequest { frame: 0 })
        .await
        .expect("overlay raw frame");

    assert_eq!(
        raw.body.as_ref(),
        &[0, 0, 0xE8, 0x03, 0xD0, 0x07, 0xB8, 0x0B],
        "overlay compositing must not alter raw source samples"
    );
    let pixels = image::load_from_memory_with_format(display.body.as_ref(), ImageFormat::Png)
        .expect("valid overlay PNG")
        .to_luma8()
        .into_raw();
    assert_eq!(pixels, [255, 85, 170, 255]);
}

#[tokio::test]
#[ignore = "requires the independently generated prepared DICOM corpus"]
async fn prepared_native_overlay_composites_after_luts_and_preserves_raw_frame() {
    let path =
        support::prepared_corpus_case("classic/cr/overlay_modality_voi_explicit_le/instance.dcm");
    let report = support::discover(
        &[path],
        dcmview::loader::DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover prepared CR");
    let entry = report.files.into_iter().next().expect("prepared CR entry");

    let display = load_frame(
        entry.clone(),
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("prepared overlay display");
    let raw = load_raw_frame(entry, new_raw_cache(), RawFrameRequest { frame: 0 })
        .await
        .expect("prepared overlay raw frame");

    assert_eq!(raw.body.as_ref(), &[0, 1, 2, 3]);
    let pixels = image::load_from_memory_with_format(display.body.as_ref(), ImageFormat::Png)
        .expect("valid prepared overlay PNG")
        .to_luma8()
        .into_raw();
    assert_eq!(pixels, [255, 255, 255, 255]);
}

#[tokio::test]
async fn native_rectangular_shutter_applies_after_monochrome1_and_preserves_raw_samples() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("shutter-monochrome1.dcm");
    support::write_uncompressed_u16_dicom_with_photometric(
        &path,
        "1.2.840.10008.1.2.1",
        (3, 3),
        vec![0, 500, 1000, 1500, 2000, 2500, 3000, 3500, 4095],
        "MONOCHROME1",
        Some("2048"),
        Some("4096"),
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.1", 1);
    entry.rows = 3;
    entry.columns = 3;
    entry.photometric_interpretation = "MONOCHROME1".to_string();
    entry.default_window = Some(WindowPreset {
        center: 2048.0,
        width: 4096.0,
    });
    entry.series_metadata.presentation.display_shutter = Some(dcmview::types::DisplayShutter {
        shapes: vec![dcmview::types::ShutterShape::Rectangular {
            left_vertical_edge: 2,
            right_vertical_edge: 2,
            upper_horizontal_edge: 2,
            lower_horizontal_edge: 2,
        }],
        presentation_value: 0,
    });

    let display = load_frame(
        entry.clone(),
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("shutter display frame");
    let raw = load_raw_frame(entry, new_raw_cache(), RawFrameRequest { frame: 0 })
        .await
        .expect("shutter raw frame");

    assert_eq!(raw.body.len(), 18);
    assert_eq!(&raw.body[..4], &[0, 0, 0xF4, 0x01]);
    let pixels = image::load_from_memory_with_format(display.body.as_ref(), ImageFormat::Png)
        .expect("valid shutter PNG")
        .to_luma8()
        .into_raw();
    assert_eq!(pixels, [0, 0, 0, 0, 130, 0, 0, 0, 0]);
}

#[tokio::test]
#[ignore = "requires the independently generated prepared DICOM corpus"]
async fn prepared_native_full_frame_shutter_preserves_windowed_pixels_and_raw_frame() {
    let path = support::prepared_corpus_case(
        "classic/dx/display_shutter_mono2_u16_explicit_le/instance.dcm",
    );
    let report = support::discover(
        &[path],
        dcmview::loader::DiscoverOptions {
            recursive: false,
            filters: Vec::new(),
        },
    )
    .await
    .expect("discover prepared DX");
    let entry = report.files.into_iter().next().expect("prepared DX entry");

    let display = load_frame(
        entry.clone(),
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("prepared shutter display");
    let raw = load_raw_frame(entry, new_raw_cache(), RawFrameRequest { frame: 0 })
        .await
        .expect("prepared shutter raw frame");

    assert_eq!(
        raw.body.as_ref(),
        &[0x00, 0x00, 0x00, 0x04, 0x00, 0x08, 0xff, 0x0f]
    );
    let pixels = image::load_from_memory_with_format(display.body.as_ref(), ImageFormat::Png)
        .expect("valid prepared shutter PNG")
        .to_luma8()
        .into_raw();
    assert_eq!(pixels, [0, 64, 128, 255]);
}

#[tokio::test]
async fn applies_big_endian_byte_order_for_uncompressed_pixels() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("uncompressed-be.dcm");
    support::write_uncompressed_u16_dicom(
        &path,
        "1.2.840.10008.1.2.2",
        1,
        2,
        vec![256, 1],
        Some("128"),
        Some("256"),
    );

    let mut entry = support::file_entry(path, "1.2.840.10008.1.2.2", 1);
    entry.rows = 1;
    entry.columns = 2;
    entry.default_window = Some(WindowPreset {
        center: 128.0,
        width: 256.0,
    });

    let response = load_frame(
        entry,
        new_cache(),
        FrameRequest {
            frame: 0,
            window_center: None,
            window_width: None,
            window_mode: dcmview::types::WindowMode::Default,
        },
    )
    .await
    .expect("big-endian frame decode");

    let image = image::load_from_memory_with_format(response.body.as_ref(), ImageFormat::Png)
        .expect("valid png")
        .to_luma8();
    let first = image.get_pixel(0, 0).0[0];
    let second = image.get_pixel(1, 0).0[0];
    assert!(
        first > second,
        "expected first pixel to remain brighter after BE decode"
    );
}

use dicom_dictionary_std::tags;
use dicom_object::InMemDicomObject;

const ICC_HEADER_LEN: usize = 128;
const MAX_ICC_PROFILE_LEN: usize = 16 * 1024 * 1024;

/// Return an ICC profile only when it applies unambiguously to every possible
/// optical path represented by this object.
pub(super) fn select_icc_profile(object: &InMemDicomObject) -> Option<Vec<u8>> {
    let top_level = object.get(tags::ICC_PROFILE).and_then(element_profile);

    let optical_paths = object
        .get(tags::OPTICAL_PATH_SEQUENCE)
        .and_then(|element| element.items());

    let Some(optical_paths) = optical_paths else {
        return top_level;
    };
    if optical_paths.is_empty() {
        return top_level;
    }

    let nested = optical_paths
        .iter()
        .map(|item| item.get(tags::ICC_PROFILE).and_then(element_profile))
        .collect::<Option<Vec<_>>>()?;

    select_identical_profiles(top_level.into_iter().chain(nested))
}

fn element_profile<I: dicom_core::header::HasLength, P>(
    element: &dicom_core::DataElement<I, P>,
) -> Option<Vec<u8>> {
    let bytes = element.value().to_bytes().ok()?;
    normalize_profile(bytes.as_ref())
}

fn normalize_profile(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < ICC_HEADER_LEN || bytes.len() > MAX_ICC_PROFILE_LEN {
        return None;
    }
    let declared_len = u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
    if !(ICC_HEADER_LEN..=MAX_ICC_PROFILE_LEN).contains(&declared_len) {
        return None;
    }
    let padding_is_valid = bytes.len() == declared_len
        || (bytes.len() == declared_len.checked_add(1)? && bytes[declared_len] == 0);
    if !padding_is_valid || bytes.get(36..40)? != b"acsp" {
        return None;
    }
    Some(bytes[..declared_len].to_vec())
}

fn select_identical_profiles(mut profiles: impl Iterator<Item = Vec<u8>>) -> Option<Vec<u8>> {
    let selected = profiles.next()?;
    profiles
        .all(|profile| profile == selected)
        .then_some(selected)
}

#[cfg(test)]
mod tests {
    use super::{normalize_profile, select_icc_profile, select_identical_profiles};
    use crate::api::contracts::WindowMode;
    use dicom_core::{value::DataSetSequence, DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::tags;
    use dicom_object::{open_file, InMemDicomObject};
    use image::{codecs::png::PngDecoder, ImageDecoder};
    use std::io::Cursor;

    fn profile(marker: u8) -> Vec<u8> {
        let mut profile = vec![0; 128];
        profile[..4].copy_from_slice(&128_u32.to_be_bytes());
        profile[36..40].copy_from_slice(b"acsp");
        profile[64] = marker;
        profile
    }

    #[test]
    fn validates_icc_header_length_signature_and_dicom_padding() {
        let valid = profile(1);
        assert_eq!(normalize_profile(&valid), Some(valid.clone()));

        let mut padded = valid.clone();
        padded.push(0);
        assert_eq!(normalize_profile(&padded), Some(valid));

        let mut wrong_signature = profile(1);
        wrong_signature[36..40].copy_from_slice(b"nope");
        assert!(normalize_profile(&wrong_signature).is_none());

        let mut wrong_length = profile(1);
        wrong_length[..4].copy_from_slice(&127_u32.to_be_bytes());
        assert!(normalize_profile(&wrong_length).is_none());
    }

    #[test]
    fn selects_only_identical_optical_path_profiles() {
        let first = profile(1);
        assert_eq!(
            select_identical_profiles([first.clone(), first.clone()].into_iter()),
            Some(first.clone())
        );
        assert!(select_identical_profiles([first, profile(2)].into_iter()).is_none());
        assert!(select_identical_profiles(Vec::<Vec<u8>>::new().into_iter()).is_none());
    }

    fn object_with_profiles(
        top_level: Option<Vec<u8>>,
        optical_paths: Vec<Option<Vec<u8>>>,
    ) -> InMemDicomObject {
        let mut object = InMemDicomObject::new_empty();
        if let Some(profile) = top_level {
            object.put(DataElement::new(
                tags::ICC_PROFILE,
                VR::OB,
                PrimitiveValue::U8(profile.into()),
            ));
        }
        if !optical_paths.is_empty() {
            let items: Vec<InMemDicomObject> = optical_paths
                .into_iter()
                .map(|profile| {
                    InMemDicomObject::from_element_iter(profile.map(|profile| {
                        DataElement::new(
                            tags::ICC_PROFILE,
                            VR::OB,
                            PrimitiveValue::U8(profile.into()),
                        )
                    }))
                })
                .collect();
            object.put(DataElement::new(
                tags::OPTICAL_PATH_SEQUENCE,
                VR::SQ,
                DataSetSequence::from(items),
            ));
        }
        object
    }

    #[test]
    fn extracts_top_level_or_complete_identical_optical_path_profiles() {
        let first = profile(1);
        assert_eq!(
            select_icc_profile(&object_with_profiles(Some(first.clone()), Vec::new())),
            Some(first.clone())
        );
        assert_eq!(
            select_icc_profile(&object_with_profiles(
                None,
                vec![Some(first.clone()), Some(first.clone())]
            )),
            Some(first)
        );
    }

    #[test]
    fn omits_ambiguous_or_incomplete_optical_path_profiles() {
        let first = profile(1);
        let second = profile(2);
        assert!(select_icc_profile(&object_with_profiles(
            None,
            vec![Some(first.clone()), Some(second.clone())]
        ))
        .is_none());
        assert!(
            select_icc_profile(&object_with_profiles(None, vec![Some(first.clone()), None]))
                .is_none()
        );
        assert!(
            select_icc_profile(&object_with_profiles(Some(first), vec![Some(second)])).is_none()
        );
    }

    #[tokio::test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    async fn prepared_top_level_and_optical_path_profiles_are_preserved_in_display_pngs() {
        // `None` is the case's own file; a codec root holds the same case
        // re-encoded, and exists only in the per-profile prepared layout.
        let cases = [
            (
                None,
                "vl/photo/rgb_icc_profile_explicit_le",
                0_u32,
                [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
            ),
            (
                None,
                "vl/wsi/tiled_full_small",
                0,
                [255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
            ),
            (
                None,
                "vl/wsi/tiled_sparse_small",
                0,
                [255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0],
            ),
            (
                None,
                "vl/wsi/multiple_optical_paths",
                4,
                [0, 255, 255, 0, 255, 255, 0, 255, 255, 0, 255, 255],
            ),
            (
                Some("extended-jpegxl"),
                "vl/photo/rgb_icc_profile_explicit_le",
                0,
                [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
            ),
            (
                Some("extended-jpeg2000"),
                "vl/photo/rgb_icc_profile_explicit_le",
                0,
                [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255],
            ),
        ];

        let mut expected_profile = None;
        for (codec_root, case, frame, expected_pixels) in cases {
            let relative = format!("{case}/instance.dcm");
            let path = match codec_root {
                None => crate::loader::prepared_corpus_case(&relative),
                Some(codec_root) => {
                    let root = crate::loader::prepared_corpus_root().join(codec_root);
                    if !root.is_dir() {
                        eprintln!("{codec_root}: not in this corpus; skipping {case}");
                        continue;
                    }
                    root.join(relative)
                }
            };
            let object = open_file(&path).expect("open prepared ICC object");
            let profile = select_icc_profile(&object).expect("unambiguous prepared ICC profile");
            assert_eq!(profile.len(), 736);
            assert_eq!(&profile[36..40], b"acsp");
            if let Some(expected) = &expected_profile {
                assert_eq!(&profile, expected, "prepared profiles differ for {case}");
            } else {
                expected_profile = Some(profile.clone());
            }

            let entry = crate::loader::test_entry(&path);
            let response = super::super::service::load_frame(
                entry,
                super::super::cache::new_cache(),
                super::super::service::FrameRequest {
                    frame,
                    window_center: None,
                    window_width: None,
                    window_mode: WindowMode::Default,
                },
            )
            .await
            .expect("render prepared ICC object");
            let png = response.body;
            let mut decoder =
                PngDecoder::new(Cursor::new(png.clone())).expect("decode display PNG");
            assert_eq!(decoder.icc_profile().expect("read PNG ICC"), Some(profile));
            let pixels = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .expect("load display PNG pixels")
                .to_rgb8()
                .into_raw();
            assert_eq!(pixels, expected_pixels, "display pixels changed for {case}");
        }
    }
}

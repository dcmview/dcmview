use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use dicom_core::value::PixelFragmentSequence;
use dicom_core::{DataElement, PrimitiveValue, VR};
use dicom_dictionary_std::tags;
use dicom_object::{collector::DicomCollector, open_file, DefaultDicomObject, InMemDicomObject};

use super::header::open_header;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;

/// The file's data set, ready to decode `frame`, and the index to decode.
///
/// For a multi-frame object this keeps the header plus only the requested
/// frame's encoded bytes as a one-frame pixel sequence, so an uncached frame
/// request reads one frame from disk rather than every frame in the file.
/// Single-frame objects are opened whole, which reads the same bytes.
pub(crate) fn open_for_frame_decode(
    file: &FileEntry,
    frame: u32,
) -> Result<(DefaultDicomObject, u32)> {
    if file.frame_count <= 1 {
        let object = open_file(&file.path)
            .with_context(|| format!("failed to open DICOM: {}", file.path.display()))?;
        return Ok((object, frame));
    }
    let mut object = open_header(&file.path)?;
    let encoded = read_encapsulated_fragment_blocking(&file.path, frame)?;
    object.put(DataElement::new(
        tags::NUMBER_OF_FRAMES,
        VR::IS,
        PrimitiveValue::from("1"),
    ));
    object.remove_element(tags::EXTENDED_OFFSET_TABLE);
    object.remove_element(tags::EXTENDED_OFFSET_TABLE_LENGTHS);
    object.put(DataElement::new(
        tags::PIXEL_DATA,
        VR::OB,
        PixelFragmentSequence::new(Vec::<u32>::new(), vec![encoded.to_vec()]),
    ));
    Ok((object, 0))
}

pub(crate) fn read_encapsulated_fragment_blocking(path: &PathBuf, frame: u32) -> Result<Bytes> {
    let mut collector = DicomCollector::open_file(path).with_context(|| {
        format!(
            "failed to open DICOM for collector access: {}",
            path.display()
        )
    })?;
    let transfer_syntax = collector
        .read_file_meta()?
        .transfer_syntax
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let mut dataset = InMemDicomObject::new_empty();
    collector.read_dataset_up_to_pixeldata(&mut dataset)?;
    let frame_count = dataset
        .get(tags::NUMBER_OF_FRAMES)
        .and_then(|element| element.to_int::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    if frame >= frame_count {
        return Err(anyhow!("frame out of range"));
    }

    let extended_offsets = read_u64_values(&dataset, tags::EXTENDED_OFFSET_TABLE);
    let extended_lengths = read_u64_values(&dataset, tags::EXTENDED_OFFSET_TABLE_LENGTHS);
    let mut basic_offsets = Vec::<u32>::new();
    collector.read_basic_offset_table(&mut basic_offsets)?;

    let offsets = if !extended_offsets.is_empty() {
        validate_offset_table("Extended Offset Table", &extended_offsets, frame_count)?;
        Some(extended_offsets)
    } else if basic_offsets.is_empty() {
        None
    } else {
        let offsets = basic_offsets.into_iter().map(u64::from).collect::<Vec<_>>();
        validate_offset_table("Basic Offset Table", &offsets, frame_count)?;
        Some(offsets)
    };

    if let Some(offsets) = offsets.as_deref() {
        let index = usize::try_from(frame).context("frame index overflow")?;
        let start = offsets[index];
        let end = offsets.get(index + 1).copied();
        let encoded_length = (!extended_lengths.is_empty())
            .then(|| extended_lengths.get(index).copied())
            .flatten();
        return read_frame_at_offsets(&mut collector, start, end, encoded_length);
    }

    read_frame_without_offsets(&mut collector, frame, frame_count, &transfer_syntax)
}

fn read_u64_values(dataset: &InMemDicomObject, tag: dicom_core::Tag) -> Vec<u64> {
    dataset
        .get(tag)
        .and_then(|element| element.to_multi_int::<u64>().ok())
        .unwrap_or_default()
}

fn validate_offset_table(name: &str, offsets: &[u64], frame_count: u32) -> Result<()> {
    if offsets.len() != usize::try_from(frame_count)? {
        return Err(anyhow!(
            "{name} contains {} frame offsets, expected {frame_count}",
            offsets.len()
        ));
    }
    if offsets.first() != Some(&0) || offsets.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(anyhow!(
            "{name} offsets must begin at zero and increase strictly"
        ));
    }
    Ok(())
}

fn read_frame_at_offsets(
    collector: &mut DicomCollector<BufReader<File>>,
    target_start: u64,
    target_end: Option<u64>,
    encoded_length: Option<u64>,
) -> Result<Bytes> {
    let mut item_offset = 0_u64;
    let mut frame_data = Vec::new();
    let mut fragment = Vec::new();
    let mut found_start = false;

    loop {
        fragment.clear();
        let Some(length) = collector.read_next_fragment(&mut fragment)? else {
            break;
        };
        if item_offset == target_start {
            found_start = true;
        }
        if item_offset > target_start && !found_start {
            return Err(anyhow!("encapsulated frame offset is not an item boundary"));
        }
        if target_end.is_some_and(|end| item_offset >= end) {
            break;
        }
        if found_start {
            frame_data.extend_from_slice(&fragment);
        }
        item_offset = item_offset
            .checked_add(8)
            .and_then(|offset| offset.checked_add(u64::from(length)))
            .context("encapsulated item offset overflow")?;
    }

    if !found_start {
        return Err(anyhow!("encapsulated frame offset is out of range"));
    }
    if let Some(end) = target_end {
        if item_offset != end {
            return Err(anyhow!("encapsulated frame end is not an item boundary"));
        }
    }
    if frame_data.is_empty() {
        return Err(anyhow!("encapsulated frame contains no data"));
    }
    if let Some(encoded_length) = encoded_length {
        let encoded_length = usize::try_from(encoded_length)
            .context("Extended Offset Table frame length exceeds addressable memory")?;
        if frame_data.len() < encoded_length {
            return Err(anyhow!(
                "encapsulated frame is shorter than its Extended Offset Table length"
            ));
        }
        frame_data.truncate(encoded_length);
    }
    Ok(Bytes::from(frame_data))
}

fn read_frame_without_offsets(
    collector: &mut DicomCollector<BufReader<File>>,
    target_frame: u32,
    frame_count: u32,
    transfer_syntax: &str,
) -> Result<Bytes> {
    let one_fragment_per_frame = transfer_syntax == dicom_dictionary_std::uids::RLE_LOSSLESS;
    let mut current_frame = 0_u32;
    let mut frame_data = Vec::new();
    let mut fragment = Vec::new();

    loop {
        fragment.clear();
        if collector.read_next_fragment(&mut fragment)?.is_none() {
            break;
        }
        if current_frame == target_frame {
            frame_data.extend_from_slice(&fragment);
        }
        let frame_complete = one_fragment_per_frame || compressed_frame_ends_here(&fragment);
        if frame_complete {
            if current_frame == target_frame {
                return Ok(Bytes::from(frame_data));
            }
            current_frame = current_frame
                .checked_add(1)
                .context("encapsulated frame count overflow")?;
        }
    }

    if frame_count == 1 && target_frame == 0 && !frame_data.is_empty() {
        return Ok(Bytes::from(frame_data));
    }
    Err(anyhow!(
        "could not determine encapsulated frame boundaries without an offset table"
    ))
}

fn compressed_frame_ends_here(fragment: &[u8]) -> bool {
    let payload = fragment.strip_suffix(&[0]).unwrap_or(fragment);
    payload.ends_with(&[0xFF, 0xD9])
}

#[cfg(test)]
mod tests {
    use super::read_encapsulated_fragment_blocking;
    use dicom_core::{value::PixelFragmentSequence, DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    use tempfile::tempdir;

    #[test]
    fn basic_offsets_assemble_multifragment_frames_for_random_access() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("bot-multifragment.dcm");
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.71001"),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
        ]);
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new(
                vec![0, 20],
                vec![
                    b"aa".to_vec(),
                    b"bb".to_vec(),
                    b"cc".to_vec(),
                    b"dd".to_vec(),
                ],
            ),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax("1.2.840.10008.1.2.4.90")
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.71001"),
            )
            .unwrap()
            .write_to_file(&path)
            .unwrap();

        assert_eq!(
            read_encapsulated_fragment_blocking(&path, 1)
                .unwrap()
                .as_ref(),
            b"ccdd"
        );
        assert_eq!(
            read_encapsulated_fragment_blocking(&path, 0)
                .unwrap()
                .as_ref(),
            b"aabb"
        );
    }

    #[test]
    fn empty_offsets_use_end_markers_and_preserve_odd_length_padding() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("empty-bot-padding.dcm");
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.71002"),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
        ]);
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new_fragments(vec![
                vec![1, 2],
                vec![3, 0xFF, 0xD9, 0],
                vec![4, 5],
                vec![6, 0xFF, 0xD9, 0],
            ]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax("1.2.840.10008.1.2.4.90")
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.71002"),
            )
            .unwrap()
            .write_to_file(&path)
            .unwrap();

        assert_eq!(
            read_encapsulated_fragment_blocking(&path, 1)
                .unwrap()
                .as_ref(),
            &[4, 5, 6, 0xFF, 0xD9, 0]
        );
    }

    #[test]
    fn extended_offsets_take_precedence_for_multifragment_frames() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("eot-multifragment.dcm");
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.71003"),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
            DataElement::new(
                tags::EXTENDED_OFFSET_TABLE,
                VR::OV,
                PrimitiveValue::U64(vec![0, 20].into()),
            ),
            DataElement::new(
                tags::EXTENDED_OFFSET_TABLE_LENGTHS,
                VR::OV,
                PrimitiveValue::U64(vec![4, 4].into()),
            ),
        ]);
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new_fragments(vec![
                b"aa".to_vec(),
                b"bb".to_vec(),
                b"cc".to_vec(),
                b"dd".to_vec(),
            ]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax("1.2.840.10008.1.2.4.90")
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.71003"),
            )
            .unwrap()
            .write_to_file(&path)
            .unwrap();

        assert_eq!(
            read_encapsulated_fragment_blocking(&path, 1)
                .unwrap()
                .as_ref(),
            b"ccdd"
        );
    }

    #[test]
    fn empty_basic_offsets_use_one_rle_fragment_per_frame() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("empty-bot-rle.dcm");
        let mut object = InMemDicomObject::from_element_iter([
            DataElement::new(
                tags::SOP_CLASS_UID,
                VR::UI,
                uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
            ),
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.71004"),
            DataElement::new(tags::NUMBER_OF_FRAMES, VR::IS, "2"),
        ]);
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OB,
            PixelFragmentSequence::new_fragments(vec![b"first".to_vec(), b"second".to_vec()]),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::RLE_LOSSLESS)
                    .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.71004"),
            )
            .unwrap()
            .write_to_file(&path)
            .unwrap();

        assert_eq!(
            read_encapsulated_fragment_blocking(&path, 1)
                .unwrap()
                .as_ref(),
            b"second"
        );
    }
}

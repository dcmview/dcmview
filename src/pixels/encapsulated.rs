use crate::types::FileEntry;
use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use dicom_core::value::PixelFragmentSequence;
use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
use dicom_dictionary_std::tags;
use dicom_encoding::TransferSyntaxIndex;
use dicom_object::{open_file, DefaultDicomObject, FileMetaTable};
use dicom_parser::dataset::lazy_read::LazyDataSetReader;
use dicom_parser::dataset::LazyDataToken;
use dicom_parser::StatefulDecode;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;

use super::header::open_header;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
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

const ITEM: Tag = Tag(0xFFFE, 0xE000);
const SEQUENCE_DELIMITER: Tag = Tag(0xFFFE, 0xE0DD);

/// The encoded bytes of one frame of an encapsulated Pixel Data element.
///
/// The header is walked once, keeping only Number of Frames and the Extended
/// Offset Table. With a valid offset table (Extended, else Basic) the reader
/// seeks to the frame's first item and reads just that frame's fragments;
/// without one it steps over item headers, reading only each fragment's last
/// bytes to find where a compressed frame ends (RLE has one fragment per
/// frame), so earlier frames are never read into memory.
pub(crate) fn read_encapsulated_fragment_blocking(path: &PathBuf, frame: u32) -> Result<Bytes> {
    let file =
        File::open(path).with_context(|| format!("failed to open DICOM: {}", path.display()))?;
    let mut reader = BufReader::new(file);
    reader.seek(SeekFrom::Start(128))?;
    let meta = FileMetaTable::from_reader(&mut reader)
        .with_context(|| format!("failed to read file meta: {}", path.display()))?;
    let transfer_syntax_uid = meta
        .transfer_syntax()
        .trim_end_matches('\0')
        .trim()
        .to_string();
    let transfer_syntax = TransferSyntaxRegistry
        .get(&transfer_syntax_uid)
        .with_context(|| format!("unknown transfer syntax {transfer_syntax_uid}"))?;
    let header = walk_to_pixel_sequence(&mut reader, transfer_syntax)?;
    let frame_count = header.frame_count.max(1);
    if frame >= frame_count {
        return Err(anyhow!("frame out of range"));
    }

    let basic_offsets = read_basic_offset_table(&mut reader)?;
    let first_fragment = reader.stream_position()?;
    let offsets = if !header.extended_offsets.is_empty() {
        validate_offset_table(
            "Extended Offset Table",
            &header.extended_offsets,
            frame_count,
        )?;
        Some(header.extended_offsets)
    } else if basic_offsets.is_empty() {
        None
    } else {
        validate_offset_table("Basic Offset Table", &basic_offsets, frame_count)?;
        Some(basic_offsets)
    };

    if let Some(offsets) = offsets.as_deref() {
        let index = usize::try_from(frame).context("frame index overflow")?;
        let start = offsets[index];
        let end = offsets.get(index + 1).copied();
        let encoded_length = header.extended_lengths.get(index).copied();
        reader.seek(SeekFrom::Start(
            first_fragment
                .checked_add(start)
                .context("encapsulated item offset overflow")?,
        ))?;
        return read_frame_at_offset(&mut reader, start, end, encoded_length);
    }

    let one_fragment_per_frame = transfer_syntax_uid == dicom_dictionary_std::uids::RLE_LOSSLESS;
    read_frame_without_offsets(&mut reader, frame, frame_count, one_fragment_per_frame)
}

struct EncapsulatedHeader {
    frame_count: u32,
    extended_offsets: Vec<u64>,
    extended_lengths: Vec<u64>,
}

/// Parses the data set up to the top-level encapsulated Pixel Data element,
/// leaving the reader just after its header. Only Number of Frames and the
/// Extended Offset Table values are read.
fn walk_to_pixel_sequence(
    reader: &mut BufReader<File>,
    transfer_syntax: &dicom_encoding::TransferSyntax,
) -> Result<EncapsulatedHeader> {
    let mut header = EncapsulatedHeader {
        frame_count: 1,
        extended_offsets: Vec::new(),
        extended_lengths: Vec::new(),
    };
    let mut parser = LazyDataSetReader::new_with_ts(&mut *reader, transfer_syntax)?;
    let mut depth = 0_usize;
    while let Some(token) = parser.advance() {
        match token? {
            LazyDataToken::PixelSequenceStart if depth == 0 => return Ok(header),
            LazyDataToken::SequenceStart { .. } | LazyDataToken::PixelSequenceStart => depth += 1,
            LazyDataToken::SequenceEnd => depth = depth.saturating_sub(1),
            LazyDataToken::LazyItemValue { len, decoder } => decoder.skip_bytes(len)?,
            LazyDataToken::LazyValue {
                header: element,
                decoder,
            } => {
                let length = element.len.get().context("undefined element length")?;
                let keep = depth == 0
                    && matches!(
                        element.tag,
                        tags::NUMBER_OF_FRAMES
                            | tags::EXTENDED_OFFSET_TABLE
                            | tags::EXTENDED_OFFSET_TABLE_LENGTHS
                    );
                if !keep {
                    decoder.skip_bytes(length)?;
                    continue;
                }
                let mut value = Vec::new();
                decoder.read_to_vec(length, &mut value)?;
                match element.tag {
                    tags::NUMBER_OF_FRAMES => {
                        header.frame_count = std::str::from_utf8(&value)
                            .ok()
                            .and_then(|text| text.trim_matches(['\0', ' ']).parse().ok())
                            .unwrap_or(1);
                    }
                    tags::EXTENDED_OFFSET_TABLE => header.extended_offsets = u64_values(&value),
                    _ => header.extended_lengths = u64_values(&value),
                }
            }
            _ => {}
        }
    }
    Err(anyhow!("no encapsulated pixel data element"))
}

/// Encapsulated transfer syntaxes are all little endian.
fn u64_values(bytes: &[u8]) -> Vec<u64> {
    bytes
        .chunks_exact(8)
        .map(|value| u64::from_le_bytes(value.try_into().unwrap_or_default()))
        .collect()
}

/// The next item header: its tag and length.
fn read_item_header(reader: &mut BufReader<File>) -> Result<(Tag, u32)> {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header)?;
    let tag = Tag(
        u16::from_le_bytes([header[0], header[1]]),
        u16::from_le_bytes([header[2], header[3]]),
    );
    Ok((
        tag,
        u32::from_le_bytes([header[4], header[5], header[6], header[7]]),
    ))
}

fn read_basic_offset_table(reader: &mut BufReader<File>) -> Result<Vec<u64>> {
    let (tag, length) = read_item_header(reader)?;
    if tag != ITEM {
        return Err(anyhow!(
            "encapsulated pixel data does not start with an item"
        ));
    }
    let mut table = vec![0_u8; usize::try_from(length)?];
    reader.read_exact(&mut table)?;
    Ok(table
        .chunks_exact(4)
        .map(|offset| {
            u64::from(u32::from_le_bytes([
                offset[0], offset[1], offset[2], offset[3],
            ]))
        })
        .collect())
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

/// Reads the fragments from the reader's position (the frame's first item,
/// at `start` bytes past the first fragment) up to `end`, or the sequence
/// delimiter for the last frame.
fn read_frame_at_offset(
    reader: &mut BufReader<File>,
    start: u64,
    end: Option<u64>,
    encoded_length: Option<u64>,
) -> Result<Bytes> {
    let mut item_offset = start;
    let mut frame_data = Vec::new();
    while end.is_none_or(|end| item_offset < end) {
        let (tag, length) = match read_item_header(reader) {
            Ok(header) => header,
            Err(_) if end.is_none() => break,
            Err(error) => return Err(error),
        };
        match tag {
            ITEM => {}
            SEQUENCE_DELIMITER if end.is_none() => break,
            _ if item_offset == start => {
                return Err(anyhow!("encapsulated frame offset is out of range"));
            }
            _ => return Err(anyhow!("encapsulated frame end is not an item boundary")),
        }
        let fragment_start = frame_data.len();
        frame_data.resize(fragment_start + usize::try_from(length)?, 0);
        reader
            .read_exact(&mut frame_data[fragment_start..])
            .context("encapsulated fragment is truncated")?;
        item_offset = item_offset
            .checked_add(8)
            .and_then(|offset| offset.checked_add(u64::from(length)))
            .context("encapsulated item offset overflow")?;
    }
    if end.is_some_and(|end| item_offset != end) {
        return Err(anyhow!("encapsulated frame end is not an item boundary"));
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
    reader: &mut BufReader<File>,
    target_frame: u32,
    frame_count: u32,
    one_fragment_per_frame: bool,
) -> Result<Bytes> {
    let mut current_frame = 0_u32;
    let mut frame_data = Vec::new();
    loop {
        // The sequence delimiter (or the end of the file) ends the frames.
        let Ok((ITEM, length)) = read_item_header(reader) else {
            break;
        };
        let length = usize::try_from(length)?;
        let frame_complete = if current_frame == target_frame {
            let fragment_start = frame_data.len();
            frame_data.resize(fragment_start + length, 0);
            reader
                .read_exact(&mut frame_data[fragment_start..])
                .context("encapsulated fragment is truncated")?;
            one_fragment_per_frame || compressed_frame_ends_here(&frame_data[fragment_start..])
        } else if one_fragment_per_frame {
            reader.seek_relative(i64::try_from(length)?)?;
            true
        } else {
            // Only the fragment's end can mark a frame end: skip to it.
            let tail = length.min(3);
            reader.seek_relative(i64::try_from(length - tail)?)?;
            let mut last = [0_u8; 3];
            reader
                .read_exact(&mut last[..tail])
                .context("encapsulated fragment is truncated")?;
            compressed_frame_ends_here(&last[..tail])
        };
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

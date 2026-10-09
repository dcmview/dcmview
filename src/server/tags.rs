use crate::api::contracts::{TagNode, TagValue};
use anyhow::{anyhow, bail, Context, Result};
use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom_core::header::HasLength;
use dicom_core::Tag;
use dicom_core::VR;
use dicom_dictionary_std::{tags, StandardDataDictionary};
use dicom_encoding::text::{SpecificCharacterSet, TextCodec};
use dicom_encoding::TransferSyntaxIndex;
use dicom_object::{open_file, FileMetaTable, InMemDicomObject, OpenFileOptions};
use dicom_parser::dataset::lazy_read::LazyDataSetReader;
use dicom_parser::dataset::read::DataSetReader;
use dicom_parser::dataset::{DataToken, LazyDataToken};
use dicom_parser::StatefulDecode;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// Float Pixel Data (7FE0,0008), the first standard pixel element. Tag reads
/// parse the data set only up to here and describe what follows from element
/// headers, so large pixel payloads are never read.
const FIRST_PIXEL_ELEMENT: Tag = tags::FLOAT_PIXEL_DATA;

const TAG_TEXT_PREVIEW_LIMIT: usize = 256;
const TAG_NUMERIC_VALUE_LIMIT: usize = 128;
const TAG_SEQUENCE_ITEM_LIMIT: usize = 64;
const TAG_SEQUENCE_DEPTH_LIMIT: usize = 4;
pub(crate) const TAG_SELECT_DEFAULT_LIMIT: usize = 64;
pub(crate) const TAG_SELECT_MAX_LIMIT: usize = 256;

pub(crate) fn build_tag_tree(path: &Path) -> Result<Vec<TagNode>> {
    if let Some(trailing) = trailing_element_summaries(path) {
        let object = open_tag_header(path)?;
        let text_codec = declared_text_codec(&object);
        let mut nodes = serialize_object_tags(&object, 0, text_codec.as_ref());
        nodes.extend(trailing);
        return Ok(nodes);
    }
    let object = open_full(path)?;
    let text_codec = declared_text_codec(&object);
    Ok(serialize_object_tags(&object, 0, text_codec.as_ref()))
}

/// Why a tag selection failed: a bad request, or a file that could not be read.
#[derive(Debug, thiserror::Error)]
pub(crate) enum TagSelectError {
    #[error("{0:#}")]
    Invalid(anyhow::Error),
    #[error("{0:#}")]
    Read(anyhow::Error),
}

pub(crate) fn build_selected_tag(
    path: &Path,
    selector: &str,
    offset: usize,
    limit: usize,
) -> std::result::Result<TagNode, TagSelectError> {
    check_page_limit(limit)?;
    let steps = parse_tag_selector(selector).map_err(TagSelectError::Invalid)?;
    let selects_pixel_or_later =
        matches!(steps.first(), Some(TagPathStep::Tag(tag)) if *tag >= FIRST_PIXEL_ELEMENT);
    let object = if selects_pixel_or_later {
        open_full(path)
    } else {
        open_tag_header(path)
    }
    .map_err(TagSelectError::Read)?;
    let text_codec = declared_text_codec(&object);
    select_from_object(&object, &steps, offset, limit, text_codec.as_ref())
        .map_err(TagSelectError::Invalid)
}

fn check_page_limit(limit: usize) -> std::result::Result<(), TagSelectError> {
    if limit == 0 || limit > TAG_SELECT_MAX_LIMIT {
        return Err(TagSelectError::Invalid(anyhow!(
            "tag page limit must be between 1 and {TAG_SELECT_MAX_LIMIT}"
        )));
    }
    Ok(())
}

/// The longest selector a raster's metadata tree is searched for.
const RASTER_SELECTOR_MAX_BYTES: usize = 256;

/// Selects one node of a raster image file's metadata tree, `nodes`, which
/// is the tree `/tags` answers with (masked, in a masked session): nothing
/// is read from the file, and what is selected is what the tree shows.
///
/// `selector` names a node by the path to it: steps separated by `/`, at
/// most `pixels::RASTER_TAGS_MAX_DEPTH` of them and at most 256 bytes in
/// all. A step is a node's `tag`, optionally followed by `.` and its
/// `keyword`, optionally followed by `[n]`: the `n`-th node, from zero,
/// among those of that level that match; without it, the first. So
/// `PNG:IHDR.Width`, `PNG:tEXt[2]`, `EXIF/GPS/0x0002` and
/// `TIFF:page 1/0x0100`. Every step but the last must name a group.
///
/// A group is returned with its children, and `offset` and `limit` page its
/// items as they page a DICOM sequence's (a group has one item). A leaf
/// takes no `offset`.
///
/// Every failure is `TagSelectError::Invalid` (the request names nothing
/// that exists): a malformed selector, a step that matches no node, a step
/// through a leaf, a limit outside 1 to 256.
pub(crate) fn select_raster_node(
    nodes: &[TagNode],
    selector: &str,
    offset: usize,
    limit: usize,
) -> std::result::Result<TagNode, TagSelectError> {
    check_page_limit(limit)?;
    select_raster_step(nodes, selector, offset, limit).map_err(TagSelectError::Invalid)
}

fn select_raster_step(
    nodes: &[TagNode],
    selector: &str,
    offset: usize,
    limit: usize,
) -> Result<TagNode> {
    if selector.len() > RASTER_SELECTOR_MAX_BYTES {
        bail!("metadata path is longer than {RASTER_SELECTOR_MAX_BYTES} bytes");
    }
    let steps = selector
        .split('/')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .collect::<Vec<_>>();
    if steps.is_empty() || steps.len() > crate::pixels::RASTER_TAGS_MAX_DEPTH {
        bail!(
            "metadata path must have 1 to {} steps",
            crate::pixels::RASTER_TAGS_MAX_DEPTH
        );
    }
    let mut level = nodes;
    for (index, step) in steps.iter().enumerate() {
        let (name, nth) = match step.strip_suffix(']').map(|step| step.rsplit_once('[')) {
            Some(Some((name, nth))) => (
                name,
                nth.parse::<usize>()
                    .map_err(|_| anyhow!("invalid index in metadata path step `{step}`"))?,
            ),
            Some(None) => bail!("invalid metadata path step `{step}`"),
            None => (*step, 0),
        };
        let (tag, keyword) = match name.split_once('.') {
            Some((tag, keyword)) => (tag, Some(keyword)),
            None => (name, None),
        };
        let node = level
            .iter()
            .filter(|node| node.tag == tag && keyword.is_none_or(|keyword| node.keyword == keyword))
            .nth(nth)
            .ok_or_else(|| anyhow!("metadata path step `{step}` not found"))?;
        let last = index + 1 == steps.len();
        match &node.value {
            TagValue::Sequence { items, .. } if last => {
                let total = items.len();
                let items = items
                    .iter()
                    .skip(offset)
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>();
                let truncated = offset > 0 || offset.saturating_add(items.len()) < total;
                return Ok(TagNode {
                    value: TagValue::Sequence {
                        items,
                        truncated,
                        total: truncated.then_some(total),
                    },
                    ..node.clone()
                });
            }
            TagValue::Sequence { items, .. } => {
                level = items.first().map_or(&[][..], Vec::as_slice);
            }
            _ if last => {
                if offset != 0 {
                    bail!("tag page offset applies only to sequence values");
                }
                return Ok(node.clone());
            }
            _ => bail!("metadata path step `{step}` is not a group"),
        }
    }
    unreachable!("the last step returns")
}

fn open_full(path: &Path) -> Result<InMemDicomObject<StandardDataDictionary>> {
    Ok(open_file(path)
        .with_context(|| format!("failed to open DICOM for tags: {}", path.display()))?
        .into_inner())
}

fn open_tag_header(path: &Path) -> Result<InMemDicomObject<StandardDataDictionary>> {
    Ok(OpenFileOptions::new()
        .read_until(FIRST_PIXEL_ELEMENT)
        .open_file(path)
        .with_context(|| format!("failed to open DICOM for tags: {}", path.display()))?
        .into_inner())
}

/// Nodes for the top-level elements from Float Pixel Data onward, built from
/// element headers alone.
///
/// Pixel values are never read: when parsing reaches a top-level pixel
/// element, the parser is dropped and the file is seeked past its value (or
/// across its fragment item headers), then parsing resumes with a fresh parser
/// there. The parser tracks positions itself, so seeking under a live one
/// would desynchronize them.
///
/// Returns `None` when those elements cannot be described without reading
/// them (a deflated data set, or a trailing element that is not a binary
/// value), so the caller falls back to reading the whole file.
fn trailing_element_summaries(path: &Path) -> Option<Vec<TagNode>> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    reader.seek(SeekFrom::Start(128)).ok()?;
    let meta = FileMetaTable::from_reader(&mut reader).ok()?;
    let transfer_syntax = TransferSyntaxRegistry.get(meta.transfer_syntax())?;
    if transfer_syntax.uid() == dicom_dictionary_std::uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN {
        return deflated_trailing_summaries(reader, transfer_syntax);
    }

    let mut nodes = Vec::new();
    loop {
        let next = next_pixel_element(&mut reader, transfer_syntax)?;
        let resume_at = match next {
            None => return Some(nodes),
            Some(PixelElement::Native {
                tag,
                vr,
                length,
                value_end,
            }) => {
                nodes.push(binary_summary_node(tag, vr, length as usize));
                value_end
            }
            Some(PixelElement::Encapsulated) => {
                let (length, end) = skip_pixel_fragments(&mut reader)?;
                nodes.push(binary_summary_node(tags::PIXEL_DATA, VR::OB, length));
                end
            }
        };
        reader.seek(SeekFrom::Start(resume_at)).ok()?;
    }
}

/// `trailing_element_summaries` for a deflated data set, which cannot be
/// seeked: the inflated stream is parsed up to each top-level element from
/// Float Pixel Data on, whose value is inflated into a sink rather than kept.
fn deflated_trailing_summaries(
    reader: BufReader<File>,
    transfer_syntax: &dicom_encoding::TransferSyntax,
) -> Option<Vec<TagNode>> {
    let mut inflated = flate2::read::DeflateDecoder::new(reader);
    let mut nodes = Vec::new();
    loop {
        let next = {
            // Stops at the element's header, before its value is read.
            let tokens = DataSetReader::new_with_ts(&mut inflated, transfer_syntax).ok()?;
            let mut depth = 0_usize;
            let mut next = None;
            for token in tokens {
                match token.ok()? {
                    DataToken::SequenceStart { tag, .. } => {
                        if depth == 0 && tag >= FIRST_PIXEL_ELEMENT {
                            return None;
                        }
                        depth += 1;
                    }
                    DataToken::PixelSequenceStart => return None,
                    DataToken::SequenceEnd => depth = depth.saturating_sub(1),
                    DataToken::ElementHeader(header)
                        if depth == 0 && header.tag >= FIRST_PIXEL_ELEMENT =>
                    {
                        if !is_binary_vr(header.vr) {
                            return None;
                        }
                        next = Some((header.tag, header.vr, header.len.get()?));
                        break;
                    }
                    _ => {}
                }
            }
            next
        };
        let Some((tag, vr, length)) = next else {
            return Some(nodes);
        };
        nodes.push(binary_summary_node(tag, vr, length as usize));
        std::io::copy(
            &mut (&mut inflated).take(u64::from(length)),
            &mut std::io::sink(),
        )
        .ok()?;
    }
}

/// A top-level pixel element whose header was just read.
enum PixelElement {
    Native {
        tag: Tag,
        vr: VR,
        length: u32,
        value_end: u64,
    },
    Encapsulated,
}

/// Parses from the reader's position to the next top-level pixel element,
/// leaving the reader just after its header. `None` at the end of the file.
fn next_pixel_element(
    reader: &mut BufReader<File>,
    transfer_syntax: &dicom_encoding::TransferSyntax,
) -> Option<Option<PixelElement>> {
    let mut parser = LazyDataSetReader::new_with_ts(&mut *reader, transfer_syntax).ok()?;
    let mut depth = 0_usize;
    let element = loop {
        let Some(token) = parser.advance() else {
            break None;
        };
        match token.ok()? {
            LazyDataToken::PixelSequenceStart if depth == 0 => {
                break Some(PixelElement::Encapsulated)
            }
            LazyDataToken::SequenceStart { tag, .. } => {
                if depth == 0 && tag >= FIRST_PIXEL_ELEMENT {
                    return None;
                }
                depth += 1;
            }
            LazyDataToken::PixelSequenceStart => depth += 1,
            LazyDataToken::SequenceEnd => depth = depth.saturating_sub(1),
            LazyDataToken::LazyItemValue { len, decoder } => decoder.skip_bytes(len).ok()?,
            LazyDataToken::LazyValue { header, decoder } => {
                let length = header.len.get()?;
                if depth == 0 && header.tag >= FIRST_PIXEL_ELEMENT {
                    if !is_binary_vr(header.vr) {
                        return None;
                    }
                    let value_end = decoder.position().checked_add(u64::from(length))?;
                    break Some(PixelElement::Native {
                        tag: header.tag,
                        vr: header.vr,
                        length,
                        value_end,
                    });
                }
                decoder.skip_bytes(length).ok()?;
            }
            _ => {}
        }
    };
    Some(element)
}

/// Walks an encapsulated pixel element's item headers from just after its
/// header, seeking over every fragment. Returns the fragment bytes, excluding
/// the Basic Offset Table item, and the position after the sequence
/// delimiter. Encapsulated transfer syntaxes are all little endian.
fn skip_pixel_fragments(reader: &mut BufReader<File>) -> Option<(usize, u64)> {
    const ITEM: Tag = Tag(0xFFFE, 0xE000);
    const SEQUENCE_DELIMITER: Tag = Tag(0xFFFE, 0xE0DD);
    let mut length = 0_usize;
    let mut items = 0_usize;
    loop {
        let mut header = [0_u8; 8];
        reader.read_exact(&mut header).ok()?;
        let tag = Tag(
            u16::from_le_bytes([header[0], header[1]]),
            u16::from_le_bytes([header[2], header[3]]),
        );
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        match tag {
            ITEM if len != u32::MAX => {
                if items > 0 {
                    length = length.checked_add(len as usize)?;
                }
                items += 1;
                reader.seek_relative(i64::from(len)).ok()?;
            }
            SEQUENCE_DELIMITER => {
                return Some((length, reader.stream_position().ok()?));
            }
            _ => return None,
        }
    }
}

fn is_binary_vr(vr: VR) -> bool {
    matches!(
        vr,
        VR::OB | VR::OW | VR::OF | VR::OD | VR::OL | VR::OV | VR::UN
    )
}

fn binary_summary_node(tag: Tag, vr: VR, length: usize) -> TagNode {
    TagNode {
        tag: format!("({:04X},{:04X})", tag.0, tag.1),
        vr: format!("{vr}"),
        keyword: StandardDataDictionary
            .by_tag(tag)
            .map(|entry| entry.alias().to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        value: TagValue::Binary { length },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagPathStep {
    Tag(Tag),
    Item(usize),
}

fn parse_tag_selector(selector: &str) -> Result<Vec<TagPathStep>> {
    let parts = selector
        .split('/')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() || parts.len() % 2 == 0 {
        bail!("tag path must alternate tag/item and end with a tag");
    }
    let mut steps = Vec::with_capacity(parts.len());
    for (index, part) in parts.into_iter().enumerate() {
        if index % 2 == 0 {
            steps.push(TagPathStep::Tag(parse_tag(part)?));
        } else {
            let item = part
                .parse::<usize>()
                .map_err(|_| anyhow!("invalid sequence item index `{part}`"))?;
            steps.push(TagPathStep::Item(item));
        }
    }
    Ok(steps)
}

fn parse_tag(raw: &str) -> Result<Tag> {
    let normalized = raw.trim_matches(|character| character == '(' || character == ')');
    let (group, element) = normalized
        .split_once(',')
        .ok_or_else(|| anyhow!("invalid tag `{raw}`; expected (GGGG,EEEE)"))?;
    let group =
        u16::from_str_radix(group, 16).map_err(|_| anyhow!("invalid tag group in `{raw}`"))?;
    let element =
        u16::from_str_radix(element, 16).map_err(|_| anyhow!("invalid tag element in `{raw}`"))?;
    Ok(Tag(group, element))
}

fn select_from_object(
    object: &InMemDicomObject<StandardDataDictionary>,
    steps: &[TagPathStep],
    offset: usize,
    limit: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> Result<TagNode> {
    let TagPathStep::Tag(tag) = steps[0] else {
        bail!("tag path must begin with a tag");
    };
    let element = object
        .get(tag)
        .ok_or_else(|| anyhow!("tag ({:04X},{:04X}) not found", tag.0, tag.1))?;
    if steps.len() == 1 {
        return serialize_selected_element(element, offset, limit, text_codec);
    }
    let TagPathStep::Item(item_index) = steps[1] else {
        bail!("tag path must include a sequence item index after a tag");
    };
    let items = element
        .items()
        .ok_or_else(|| anyhow!("tag ({:04X},{:04X}) is not a sequence", tag.0, tag.1))?;
    let item = items.get(item_index).ok_or_else(|| {
        anyhow!(
            "sequence item {item_index} out of range for ({:04X},{:04X})",
            tag.0,
            tag.1
        )
    })?;
    select_from_object(item, &steps[2..], offset, limit, text_codec)
}

fn serialize_selected_element(
    element: &dicom_object::mem::InMemElement<StandardDataDictionary>,
    offset: usize,
    limit: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> Result<TagNode> {
    if format!("{}", element.header().vr()) != "SQ" {
        if offset != 0 {
            bail!("tag page offset applies only to sequence values");
        }
        return Ok(serialize_element(element, 0, text_codec));
    }
    let items = element
        .items()
        .ok_or_else(|| anyhow!("sequence item decoding failed"))?;
    let tag = element.header().tag;
    let tag_repr = format!("({:04X},{:04X})", tag.0, tag.1);
    let total = items.len();
    let serialized_items = items
        .iter()
        .skip(offset)
        .take(limit)
        .map(|item| serialize_object_tags(item, 1, text_codec))
        .collect::<Vec<_>>();
    let truncated = offset > 0 || offset.saturating_add(serialized_items.len()) < total;
    Ok(TagNode {
        tag: tag_repr,
        vr: "SQ".to_string(),
        keyword: StandardDataDictionary
            .by_tag(tag)
            .map(|entry| entry.alias().to_string())
            .unwrap_or_else(|| "Unknown".to_string()),
        value: TagValue::Sequence {
            items: serialized_items,
            truncated,
            total: truncated.then_some(total),
        },
    })
}

fn serialize_object_tags(
    object: &InMemDicomObject<StandardDataDictionary>,
    depth: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> Vec<TagNode> {
    object
        .iter()
        .map(|element| serialize_element(element, depth, text_codec))
        .collect()
}

fn serialize_element(
    element: &dicom_object::mem::InMemElement<StandardDataDictionary>,
    depth: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> TagNode {
    let tag = element.header().tag;
    let tag_repr = format!("({:04X},{:04X})", tag.0, tag.1);
    let vr_repr = format!("{}", element.header().vr());
    let keyword = StandardDataDictionary
        .by_tag(tag)
        .map(|entry| entry.alias().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    let value = serialize_tag_value(element, tag_repr.as_str(), &vr_repr, depth, text_codec);

    TagNode {
        tag: tag_repr,
        vr: vr_repr,
        keyword,
        value,
    }
}

fn serialize_tag_value(
    element: &dicom_object::mem::InMemElement<StandardDataDictionary>,
    tag_repr: &str,
    vr_repr: &str,
    depth: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> TagValue {
    if tag_repr == "(7FE0,0010)" {
        return binary_value_from_element(element);
    }

    if vr_repr == "SQ" {
        return match element.items() {
            Some(items) => serialize_sequence_items(items, depth, text_codec),
            None => TagValue::Error {
                message: "sequence item decoding failed".to_string(),
            },
        };
    }

    if matches!(vr_repr, "OB" | "OW" | "OD" | "OF" | "UN" | "OL") {
        return binary_value_from_element(element);
    }

    let mut string_value = match element.to_str() {
        Ok(value) => value.to_string(),
        Err(error) => {
            return TagValue::Error {
                message: format!("value serialization failed: {error}"),
            };
        }
    };
    string_value = decode_escaped_text(string_value, text_codec);

    if is_numeric_vr(vr_repr) {
        let mut numbers = string_value
            .split('\\')
            .filter_map(|part| part.trim().parse::<f64>().ok())
            .collect::<Vec<_>>();
        if numbers.is_empty() {
            TagValue::Error {
                message: "numeric conversion failed".to_string(),
            }
        } else if numbers.len() == 1 {
            TagValue::Number { value: numbers[0] }
        } else {
            let total = numbers.len();
            let truncated = total > TAG_NUMERIC_VALUE_LIMIT;
            numbers.truncate(TAG_NUMERIC_VALUE_LIMIT);
            TagValue::Numbers {
                value: numbers,
                truncated,
                total: truncated.then_some(total),
            }
        }
    } else {
        TagValue::String {
            value: format_tag_text_preview(&string_value),
        }
    }
}

fn serialize_sequence_items(
    items: &[InMemDicomObject<StandardDataDictionary>],
    depth: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> TagValue {
    let total = items.len();
    let depth_limited = depth >= TAG_SEQUENCE_DEPTH_LIMIT;
    let item_limited = total > TAG_SEQUENCE_ITEM_LIMIT;

    if depth_limited {
        return TagValue::Sequence {
            items: Vec::new(),
            truncated: true,
            total: Some(total),
        };
    }

    let serialized_items = items
        .iter()
        .take(TAG_SEQUENCE_ITEM_LIMIT)
        .map(|item| serialize_object_tags(item, depth + 1, text_codec))
        .collect();

    TagValue::Sequence {
        items: serialized_items,
        truncated: item_limited,
        total: item_limited.then_some(total),
    }
}

fn declared_text_codec(
    object: &InMemDicomObject<StandardDataDictionary>,
) -> Option<SpecificCharacterSet> {
    let declaration = object.get(tags::SPECIFIC_CHARACTER_SET)?.to_str().ok()?;
    declaration
        .split(['\\', ';'])
        .map(str::trim)
        .filter(|component| !component.is_empty())
        .find_map(SpecificCharacterSet::from_code)
}

fn decode_escaped_text(value: String, text_codec: Option<&SpecificCharacterSet>) -> String {
    if !value.contains('\u{1b}') {
        return value;
    }
    text_codec
        .and_then(|codec| codec.decode(value.as_bytes()).ok())
        .unwrap_or(value)
}

fn format_tag_text_preview(raw: &str) -> String {
    let normalized = raw.replace('\\', "; ");
    let mut chars = normalized.chars();
    let preview: String = chars.by_ref().take(TAG_TEXT_PREVIEW_LIMIT).collect();
    if chars.next().is_some() {
        format!("{preview}…")
    } else {
        preview
    }
}

fn binary_value_from_element(
    element: &dicom_object::mem::InMemElement<StandardDataDictionary>,
) -> TagValue {
    if let Some(length) = element.header().length().get() {
        return TagValue::Binary {
            length: length as usize,
        };
    }

    if let Some(fragments) = element.fragments() {
        let length = fragments.iter().map(|fragment| fragment.len()).sum();
        return TagValue::Binary { length };
    }

    match element.to_bytes() {
        Ok(bytes) => TagValue::Binary {
            length: bytes.len(),
        },
        Err(error) => TagValue::Error {
            message: format!("binary serialization failed: {error}"),
        },
    }
}

fn is_numeric_vr(vr_repr: &str) -> bool {
    matches!(
        vr_repr,
        "US" | "SS" | "UL" | "SL" | "FL" | "FD" | "DS" | "IS"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deflated_tag_tree_matches_a_full_read_without_keeping_pixels() {
        use dicom_core::{DataElement, PrimitiveValue};
        use dicom_object::meta::FileMetaTableBuilder;

        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("deflated.dcm");
        InMemDicomObject::from_element_iter([
            DataElement::new(tags::SOP_INSTANCE_UID, VR::UI, "2.25.9"),
            DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(2_u16)),
            DataElement::new(
                tags::PIXEL_DATA,
                VR::OW,
                PrimitiveValue::U16(vec![7; 4].into()),
            ),
            // A trailing element must still be described after the pixels.
            DataElement::new(
                Tag(0xFFFC, 0xFFFC),
                VR::OB,
                PrimitiveValue::from(vec![0_u8; 6]),
            ),
        ])
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax(dicom_dictionary_std::uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(
                    dicom_dictionary_std::uids::SECONDARY_CAPTURE_IMAGE_STORAGE,
                )
                .media_storage_sop_instance_uid("2.25.9"),
        )
        .expect("file meta")
        .write_to_file(&path)
        .expect("write deflated file");

        let full = open_full(&path).expect("full read");
        assert!(trailing_element_summaries(&path).is_some());
        assert_eq!(
            serde_json::to_value(build_tag_tree(&path).expect("tag tree")).unwrap(),
            serde_json::to_value(serialize_object_tags(&full, 0, None)).unwrap()
        );
    }

    #[test]
    fn header_only_tag_tree_matches_a_full_read_for_every_fixture() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut compared = 0;
        for entry in std::fs::read_dir(fixtures).expect("fixture directory") {
            let path = entry.expect("fixture entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("dcm") {
                continue;
            }
            let full = open_full(&path).expect("full read");
            let expected = serialize_object_tags(&full, 0, declared_text_codec(&full).as_ref());
            assert!(
                trailing_element_summaries(&path).is_some(),
                "{} should not need a full read",
                path.display()
            );
            assert_eq!(
                serde_json::to_value(build_tag_tree(&path).expect("tag tree")).unwrap(),
                serde_json::to_value(expected).unwrap(),
                "{}",
                path.display()
            );
            compared += 1;
        }
        assert!(
            compared >= 6,
            "native, encapsulated, and no-pixel fixtures are covered"
        );
    }

    #[test]
    fn decodes_iso_2022_person_name_extension_sequences() {
        let codec =
            SpecificCharacterSet::from_code("ISO 2022 IR 87").expect("ISO 2022 IR 87 codec");
        let encoded = concat!(
            "Yamada^Tarou=\u{1b}$B;3ED\u{1b}(B^",
            "\u{1b}$BB@O:\u{1b}(B=",
            "\u{1b}$B$d$^$@\u{1b}(B^",
            "\u{1b}$B$?$m$&\u{1b}(B"
        );

        assert_eq!(
            decode_escaped_text(encoded.to_string(), Some(&codec)),
            "Yamada^Tarou=山田^太郎=やまだ^たろう"
        );
    }

    #[test]
    fn preserves_text_when_no_extension_sequence_is_present() {
        assert_eq!(
            decode_escaped_text("Doe^Jane".to_string(), None),
            "Doe^Jane"
        );
    }
}

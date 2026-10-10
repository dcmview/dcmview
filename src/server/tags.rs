use crate::api::contracts::{TagNode, TagValue};
use crate::data_set::{
    read_for_tags, shown_by_length, TagCut, TagDataSet, TagExtent, DATA_SET_MAX_DEPTH,
};
use anyhow::{anyhow, bail, Context, Result};
use dicom_core::dictionary::{DataDictionary, DataDictionaryEntry};
use dicom_core::header::HasLength;
use dicom_core::value::{PrimitiveValue, Value};
use dicom_core::Tag;
use dicom_dictionary_std::{tags, StandardDataDictionary};
use dicom_encoding::text::{SpecificCharacterSet, TextCodec};
use dicom_object::InMemDicomObject;
use std::fs::File;
use std::io::{BufReader, Seek, SeekFrom};
use std::path::Path;

/// Float Pixel Data (7FE0,0008), the first standard pixel element. A
/// selection before it is read from the data set before it alone.
const FIRST_PIXEL_ELEMENT: Tag = tags::FLOAT_PIXEL_DATA;

const TAG_TEXT_PREVIEW_LIMIT: usize = 256;
const TAG_NUMERIC_VALUE_LIMIT: usize = 128;
const TAG_SEQUENCE_ITEM_LIMIT: usize = 64;
const TAG_SEQUENCE_DEPTH_LIMIT: usize = 4;
pub(crate) const TAG_SELECT_DEFAULT_LIMIT: usize = 64;
pub(crate) const TAG_SELECT_MAX_LIMIT: usize = 256;

/// The tag tree of a DICOM file, from one bounded read of its data set
/// (`data_set::read_for_tags`): no pixel value, no bulk binary value and no
/// value over `DATA_SET_VALUE_MAX_BYTES` is read, and each of those is shown
/// by its declared length.
///
/// The second member is a `Note` leaf, as a raster's tree has them, when
/// the tree does not reach the end of the data set. It is the viewer's own
/// text and goes behind the nodes after they are masked.
pub(crate) fn build_tag_tree(path: &Path) -> Result<(Vec<TagNode>, Option<TagNode>)> {
    let data_set = read_data_set(path, TagExtent::Whole)?;
    let text_codec = declared_text_codec(&data_set.object);
    let mut nodes = serialize_object_tags(&data_set.object, 0, text_codec.as_ref());
    if let Some(node) = nodes.iter_mut().find(|node| node.tag == PIXEL_DATA_TAG) {
        show_fragment_bytes(node, &data_set);
    }
    if let Some(node) = too_deep_node(&mut nodes, &data_set) {
        node.value = too_deep_value();
    }
    Ok((nodes, data_set.cut.map(cut_note)))
}

const PIXEL_DATA_TAG: &str = "(7FE0,0010)";

/// Gives top-level encapsulated Pixel Data the length of its fragments,
/// which an element's own length cannot hold past 4 GiB.
fn show_fragment_bytes(node: &mut TagNode, data_set: &TagDataSet) {
    if let Some(bytes) = data_set.pixel_fragment_bytes {
        node.value = TagValue::Binary {
            length: usize::try_from(bytes).unwrap_or(usize::MAX),
        };
    }
}

/// The leaf that says why a tree ends before its data set does.
fn cut_note(cut: TagCut) -> TagNode {
    let why = match cut {
        TagCut::InsideValue => {
            "the tree ends here: the data set ends inside the last element's value, \
             or is deflated and larger than is read"
                .to_string()
        }
        TagCut::TooDeep => {
            format!("the tree ends here: sequences nest deeper than {DATA_SET_MAX_DEPTH}")
        }
        TagCut::NotAnElement => {
            "the tree ends here: what follows in the file is not a data element".to_string()
        }
    };
    TagNode {
        tag: "Note".to_string(),
        vr: String::new(),
        keyword: String::new(),
        value: TagValue::String { value: why },
    }
}

/// The node of the top-level element in which sequences nest past the
/// limit, where the tree ends.
fn too_deep_node<'a>(nodes: &'a mut [TagNode], data_set: &TagDataSet) -> Option<&'a mut TagNode> {
    let tag = data_set.too_deep?;
    let tag = format!("({:04X},{:04X})", tag.0, tag.1);
    nodes.iter_mut().rfind(|node| node.tag == tag)
}

/// What the element the tree ends in shows instead of its items.
fn too_deep_value() -> TagValue {
    TagValue::Error {
        message: format!("sequences nest deeper than {DATA_SET_MAX_DEPTH}; the tree ends here"),
    }
}

fn read_data_set(path: &Path, extent: TagExtent) -> Result<TagDataSet> {
    let open = || -> Result<TagDataSet> {
        let file = File::open(path)?;
        let length = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(128))?;
        read_for_tags(&mut reader, length, extent)
    };
    open().with_context(|| format!("failed to open DICOM for tags: {}", path.display()))
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
    let extent = match steps.first() {
        Some(TagPathStep::Tag(tag)) if *tag >= FIRST_PIXEL_ELEMENT => TagExtent::Whole,
        _ => TagExtent::BeforePixelData,
    };
    let data_set = read_data_set(path, extent).map_err(TagSelectError::Read)?;
    let text_codec = declared_text_codec(&data_set.object);
    let mut node = select_from_object(&data_set.object, &steps, offset, limit, text_codec.as_ref())
        .map_err(TagSelectError::Invalid)?;
    if steps.len() == 1 && node.tag == PIXEL_DATA_TAG {
        show_fragment_bytes(&mut node, &data_set);
    }
    if steps.len() == 1 && too_deep_node(std::slice::from_mut(&mut node), &data_set).is_some() {
        node.value = too_deep_value();
    }
    Ok(node)
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

    let value = serialize_tag_value(element, &vr_repr, depth, text_codec);

    TagNode {
        tag: tag_repr,
        vr: vr_repr,
        keyword,
        value,
    }
}

fn serialize_tag_value(
    element: &dicom_object::mem::InMemElement<StandardDataDictionary>,
    vr_repr: &str,
    depth: usize,
    text_codec: Option<&SpecificCharacterSet>,
) -> TagValue {
    if vr_repr == "SQ" {
        return match element.items() {
            Some(items) => serialize_sequence_items(items, depth, text_codec),
            None => TagValue::Error {
                message: "sequence item decoding failed".to_string(),
            },
        };
    }

    // A value the bounded read did not keep is empty at its declared length.
    let not_read = matches!(element.value(), Value::Primitive(PrimitiveValue::Empty))
        && element
            .header()
            .length()
            .get()
            .is_some_and(|length| length > 0);
    if shown_by_length(element.header().tag, element.header().vr()) || not_read {
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

/// An element shown by its length: the declared one, which the bounded
/// read gives every element (encapsulated Pixel Data has the length of its
/// fragments), else the length of the fragments it holds.
fn binary_value_from_element(
    element: &dicom_object::mem::InMemElement<StandardDataDictionary>,
) -> TagValue {
    let length = match element.header().length().get() {
        Some(length) => length as usize,
        None => element
            .fragments()
            .map_or(0, |fragments| fragments.iter().map(Vec::len).sum()),
    };
    TagValue::Binary { length }
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

    /// The tree dicom-object's own whole-file read gives, which the
    /// bounded read must equal for every honest file.
    fn full_read_tree(path: &Path) -> serde_json::Value {
        let full = dicom_object::open_file(path)
            .expect("full read")
            .into_inner();
        let codec = declared_text_codec(&full);
        serde_json::to_value(serialize_object_tags(&full, 0, codec.as_ref())).unwrap()
    }

    #[test]
    fn deflated_tag_tree_matches_a_full_read_without_keeping_pixels() {
        use dicom_core::{DataElement, VR};
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

        assert_eq!(
            serde_json::to_value(build_tag_tree(&path).expect("tag tree").0).unwrap(),
            full_read_tree(&path)
        );
    }

    #[test]
    fn bounded_tag_tree_matches_a_full_read_for_every_fixture() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let mut compared = 0;
        for entry in std::fs::read_dir(fixtures).expect("fixture directory") {
            let path = entry.expect("fixture entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("dcm") {
                continue;
            }
            assert_eq!(
                serde_json::to_value(build_tag_tree(&path).expect("tag tree").0).unwrap(),
                full_read_tree(&path),
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

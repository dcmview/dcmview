//! Lenient readers for attribute values in a decoded DICOM dataset.
//!
//! The loader, reference, semantic, and WSI paths all read attributes through
//! these helpers so trimming, multi-value splitting, and sequence access follow
//! one convention. Readers never fail: an absent element, a value that cannot
//! be rendered as text, or an unparsable number yields `None` or an empty
//! collection.

use dicom_core::Tag;
use dicom_object::InMemDicomObject;
use std::borrow::Cow;
use std::str::FromStr;

fn text(object: &InMemDicomObject, tag: Tag) -> Option<Cow<'_, str>> {
    object.element(tag).ok()?.to_str().ok()
}

/// The whole value, trimmed; `None` when absent or blank.
pub(crate) fn read_string(object: &InMemDicomObject, tag: Tag) -> Option<String> {
    let value = text(object, tag)?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Every backslash-separated value, trimmed. Blank values keep their position.
pub(crate) fn read_strings(object: &InMemDicomObject, tag: Tag) -> Vec<String> {
    text(object, tag)
        .map(|value| {
            value
                .split('\\')
                .map(|part| part.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The first backslash-separated value, trimmed. Unlike [`read_string`], a
/// present but blank value is `Some("")`.
pub(crate) fn read_first_string(object: &InMemDicomObject, tag: Tag) -> Option<String> {
    Some(text(object, tag)?.split('\\').next()?.trim().to_string())
}

/// The first backslash-separated value parsed as `T`.
pub(crate) fn read_number<T: FromStr>(object: &InMemDicomObject, tag: Tag) -> Option<T> {
    text(object, tag)?.split('\\').next()?.trim().parse().ok()
}

/// Every backslash-separated value that parses as `T`; malformed values are
/// skipped.
pub(crate) fn read_numbers<T: FromStr>(object: &InMemDicomObject, tag: Tag) -> Vec<T> {
    text(object, tag)
        .map(|value| {
            value
                .split('\\')
                .filter_map(|part| part.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The items of a sequence attribute; empty when absent or not a sequence.
pub(crate) fn sequence_items(object: &InMemDicomObject, tag: Tag) -> &[InMemDicomObject] {
    object
        .element(tag)
        .ok()
        .and_then(|element| element.items())
        .unwrap_or_default()
}

/// One item of a sequence attribute.
pub(crate) fn sequence_item(
    object: &InMemDicomObject,
    tag: Tag,
    index: usize,
) -> Option<&InMemDicomObject> {
    sequence_items(object, tag).get(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::{DataElement, VR};
    use dicom_dictionary_std::tags;

    fn object(value: &str) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([DataElement::new(
            tags::REFERENCED_FRAME_NUMBER,
            VR::IS,
            value,
        )])
    }

    #[test]
    fn string_readers_differ_only_in_blank_and_multi_value_handling() {
        let blank = object("  ");
        assert_eq!(read_string(&blank, tags::REFERENCED_FRAME_NUMBER), None);
        assert_eq!(
            read_first_string(&blank, tags::REFERENCED_FRAME_NUMBER).as_deref(),
            Some("")
        );
        let multi = object(" 2 \\\\ 4 ");
        assert_eq!(
            read_string(&multi, tags::REFERENCED_FRAME_NUMBER).as_deref(),
            Some("2 \\\\ 4")
        );
        assert_eq!(
            read_strings(&multi, tags::REFERENCED_FRAME_NUMBER),
            ["2", "", "4"]
        );
        assert_eq!(read_string(&multi, tags::SOP_CLASS_UID), None);
    }

    #[test]
    fn number_readers_take_the_first_value_or_every_parsable_value() {
        let values = object("bad\\3\\ 5");
        assert_eq!(
            read_number::<u32>(&values, tags::REFERENCED_FRAME_NUMBER),
            None
        );
        assert_eq!(
            read_numbers::<u32>(&values, tags::REFERENCED_FRAME_NUMBER),
            [3, 5]
        );
        assert_eq!(
            read_number::<u32>(&object(" 7\\bad"), tags::REFERENCED_FRAME_NUMBER),
            Some(7)
        );
    }
}

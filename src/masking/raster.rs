//! What a masked session shows of a raster image file's metadata tree
//! (`pixels::read_raster_tags`, and the `File` and `Note` leaves the server
//! adds).
//!
//! The rule is an allowlist. A value is shown only when its place in the
//! tree is listed below; everything else shows `[masked]`: every tag this
//! file does not name, every tag a file invents, and all text of the file.
//! What is listed describes the pixel grid and its encoding and nothing
//! about who made the image, where, when or with what: so GPS, dates and
//! times, device make, model and serial numbers, software and host names,
//! owner, artist, copyright, description and comment fields, PNG text
//! chunks, XMP, maker notes, embedded thumbnails, profile names and
//! descriptions, and document and page names are all masked. Dates are
//! masked, not shifted: a raster has no patient whose offset would apply.
//!
//! A listed value must also look like what the list expects. Apart from the
//! leaves the server composes itself, a listed value is kept only when it
//! is a number, a list of numbers, a problem stated in fixed words, or text
//! made of digits and number punctuation (a rational, a version). So a
//! reader that put text of the file where a number belongs would still not
//! get it past a masked session.

use super::{masked_value, MASKED};
use crate::api::contracts::{TagNode, TagValue};

/// What kind of value a listed place may show.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Numbers, or text made of digits and number punctuation.
    Numeric,
    /// Text the server composed from the catalog entry or from fixed words.
    Composed,
}

/// The top-level leaves a masked session shows, as `(tag, keyword)`.
const LEAVES: &[(&str, &str, Shown)] = &[
    ("File", "Format", Shown::Composed),
    ("File", "Size", Shown::Numeric),
    ("File", "Pages", Shown::Numeric),
    ("File", "Frames", Shown::Numeric),
    ("File", "ExcludedPages", Shown::Numeric),
    ("File", "ExcludedPage", Shown::Composed),
    ("Note", "", Shown::Composed),
    ("PNG:IHDR", "Width", Shown::Numeric),
    ("PNG:IHDR", "Height", Shown::Numeric),
    ("PNG:IHDR", "BitDepth", Shown::Numeric),
    ("PNG:IHDR", "ColorType", Shown::Numeric),
    ("PNG:IHDR", "Compression", Shown::Numeric),
    ("PNG:IHDR", "Filter", Shown::Numeric),
    ("PNG:IHDR", "Interlace", Shown::Numeric),
    ("PNG:pHYs", "PixelsPerUnitX", Shown::Numeric),
    ("PNG:pHYs", "PixelsPerUnitY", Shown::Numeric),
    ("PNG:pHYs", "Unit", Shown::Numeric),
    ("PNG:gAMA", "Gamma", Shown::Numeric),
    ("PNG:cHRM", "Chromaticities", Shown::Numeric),
    ("PNG:sRGB", "RenderingIntent", Shown::Numeric),
    ("PNG:sBIT", "SignificantBits", Shown::Numeric),
    ("PNG:acTL", "Frames", Shown::Numeric),
    ("PNG:acTL", "Plays", Shown::Numeric),
    ("JPEG:JFIF", "Version", Shown::Numeric),
    ("JPEG:JFIF", "Units", Shown::Numeric),
    ("JPEG:JFIF", "XDensity", Shown::Numeric),
    ("JPEG:JFIF", "YDensity", Shown::Numeric),
    ("JPEG:Adobe", "Version", Shown::Numeric),
    ("JPEG:Adobe", "Transform", Shown::Numeric),
    ("WEBP:VP8X", "Flags", Shown::Numeric),
    ("WEBP:VP8X", "CanvasWidth", Shown::Numeric),
    ("WEBP:VP8X", "CanvasHeight", Shown::Numeric),
    ("WEBP:VP8", "Width", Shown::Numeric),
    ("WEBP:VP8", "Height", Shown::Numeric),
    ("WEBP:VP8L", "Width", Shown::Numeric),
    ("WEBP:VP8L", "Height", Shown::Numeric),
    ("WEBP:ANIM", "LoopCount", Shown::Numeric),
    ("ICC", "Size", Shown::Numeric),
    ("ICC", "Version", Shown::Numeric),
];

/// The leaves of a JPEG frame header (`JPEG:SOF0` to `JPEG:SOF15`).
const FRAME_HEADER_LEAVES: &[&str] = &["Precision", "Height", "Width", "Components"];

/// The entries of an image directory (a TIFF page, `IFD0`, `IFD1`) a masked
/// session shows: the layout of the samples and where they lie. Sorted.
const IMAGE_ENTRIES: &[u16] = &[
    254, 255, 256, 257, 258, 259, 262, 266, 273, 274, 277, 278, 279, 280, 281, 282, 283, 284, 296,
    297, 317, 318, 319, 320, 322, 323, 324, 325, 338, 339, 529, 530, 531, 532, 34665, 34853,
];

/// The entries of an EXIF directory a masked session shows: the colour
/// space and the pixel dimensions. Exposure settings are not listed.
const EXIF_ENTRIES: &[u16] = &[40961, 40962, 40963, 40965];

/// Masks a raster's metadata tree in place. Groups stay, with their names,
/// which are dcmview's own; a leaf keeps its `tag`, `keyword` and `vr`,
/// which never hold bytes of the file, and its value only when listed.
pub(super) fn mask_tree(nodes: &mut [TagNode]) {
    for node in nodes {
        if is_group(node) {
            let entries = match node.tag.as_str() {
                "EXIF" => None,
                tag if tag.starts_with("TIFF:page ") => Some(IMAGE_ENTRIES),
                _ => Some(&[][..]),
            };
            match entries {
                // The EXIF block: only its directories, by name.
                None => mask_children(node, None),
                Some(entries) => mask_children(node, Some(entries)),
            }
        } else if !top_level_leaf_is_shown(node) {
            node.value = masked_value();
        }
    }
}

fn is_group(node: &TagNode) -> bool {
    matches!(node.value, TagValue::Sequence { .. })
}

/// Masks the children of a group: its leaves by `entries` (`None` shows
/// none), and the directories inside it by their own lists.
fn mask_children(group: &mut TagNode, entries: Option<&[u16]>) {
    let TagValue::Sequence { items, .. } = &mut group.value else {
        return;
    };
    for child in items.iter_mut().flatten() {
        if is_group(child) {
            let entries = match child.tag.as_str() {
                "IFD0" | "IFD1" => IMAGE_ENTRIES,
                "Exif" => EXIF_ENTRIES,
                // GPS, interoperability and anything unnamed: nothing.
                _ => &[],
            };
            mask_directory(child, entries);
        } else if !entries.is_some_and(|entries| entry_is_shown(child, entries)) {
            child.value = masked_value();
        }
    }
}

/// Masks a directory's entries by `entries`. Nothing lies deeper; a group
/// found here is masked whole.
fn mask_directory(directory: &mut TagNode, entries: &[u16]) {
    let TagValue::Sequence { items, .. } = &mut directory.value else {
        return;
    };
    for entry in items.iter_mut().flatten() {
        if is_group(entry) || !entry_is_shown(entry, entries) {
            entry.value = masked_value();
        }
    }
}

fn entry_is_shown(node: &TagNode, entries: &[u16]) -> bool {
    let number = node
        .tag
        .strip_prefix("0x")
        .filter(|digits| digits.len() == 4)
        .and_then(|digits| u16::from_str_radix(digits, 16).ok());
    number.is_some_and(|number| entries.binary_search(&number).is_ok())
        && looks_as_listed(&node.value, Shown::Numeric)
}

fn top_level_leaf_is_shown(node: &TagNode) -> bool {
    let listed = LEAVES
        .iter()
        .find(|(tag, keyword, _)| *tag == node.tag && *keyword == node.keyword)
        .map(|(_, _, shown)| *shown)
        .or_else(|| {
            let index = node.tag.strip_prefix("JPEG:SOF")?.parse::<u8>().ok()?;
            (index < 16 && FRAME_HEADER_LEAVES.contains(&node.keyword.as_str()))
                .then_some(Shown::Numeric)
        });
    listed.is_some_and(|shown| looks_as_listed(&node.value, shown))
}

fn looks_as_listed(value: &TagValue, shown: Shown) -> bool {
    match value {
        TagValue::Number { .. } | TagValue::Numbers { .. } | TagValue::Error { .. } => true,
        TagValue::String { value } => {
            shown == Shown::Composed
                || value == MASKED
                || value
                    .chars()
                    .all(|character| character.is_ascii_digit() || "/,.- …".contains(character))
        }
        TagValue::Binary { .. } | TagValue::Sequence { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_lists_are_sorted_for_binary_search() {
        for list in [IMAGE_ENTRIES, EXIF_ENTRIES] {
            assert!(list.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }
}

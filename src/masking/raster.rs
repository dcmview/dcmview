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
//! get it past a masked session. Of a directory entry, text is kept only
//! when the entry's type is a rational: a file can write any tag number
//! with a text type, and a date or a telephone number is digits and
//! punctuation too.
//!
//! A listed directory entry must also be what its field is. A file can
//! write any tag number with any type and count, and as often as it
//! likes, so a listed entry is shown only when it is the first entry of
//! its tag in its directory, has a type the field is defined with (TIFF
//! 6.0 and EXIF), and holds no more numbers than the field does: one for
//! a size or a scalar field, the field's own fixed count otherwise, and
//! four for a field with a number for each sample. Anything else under a
//! listed tag is masked whole. The offsets and byte counts of strips and
//! tiles are shown only when there is one of them, which is all a page
//! stored in one strip or tile has; a colour map has no small count, so
//! it is not listed.
//!
//! What a masked tree shows of a directory is therefore one value for each
//! listed field, of the field's own type and count, and nothing else. Each
//! of those is still a number the file chose: masking is a display aid for
//! honest files, not a guarantee against a file built to carry something
//! in those numbers.

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

/// The most numbers a field with one for each sample may show: dcmview
/// renders at most four bands.
const PER_SAMPLE: usize = 4;

/// The TIFF types a listed entry may have to be shown, as TIFF 6.0 and
/// EXIF define its field. In a TIFF page, where the file may be a BigTIFF,
/// `LONG8` counts as `LONG` and `IFD8` as `IFD`; in an EXIF block, which is
/// never one, they are not shown.
#[derive(Clone, Copy)]
enum Types {
    Short,
    Long,
    ShortOrLong,
    Rational,
    /// A pointer to another directory: `LONG` or `IFD`.
    Pointer,
}

impl Types {
    fn hold(self, vr: &str, big: bool) -> bool {
        let vr = match vr {
            "LONG8" if big => "LONG",
            "IFD8" if big => "IFD",
            vr => vr,
        };
        match self {
            Self::Short => vr == "SHORT",
            Self::Long => vr == "LONG",
            Self::ShortOrLong => matches!(vr, "SHORT" | "LONG"),
            Self::Rational => vr == "RATIONAL",
            Self::Pointer => matches!(vr, "LONG" | "IFD"),
        }
    }
}

/// A listed directory entry: its tag, the most numbers or rationals it may
/// hold to be shown, and the types it may have.
type Entry = (u16, usize, Types);

/// The entries of an image directory (a TIFF page, `IFD0`, `IFD1`) a masked
/// session shows: the layout of the samples and where they lie. Sorted.
const IMAGE_ENTRIES: &[Entry] = &[
    (254, 1, Types::Long),           // NewSubfileType
    (255, 1, Types::Short),          // SubfileType
    (256, 1, Types::ShortOrLong),    // ImageWidth
    (257, 1, Types::ShortOrLong),    // ImageLength
    (258, PER_SAMPLE, Types::Short), // BitsPerSample
    (259, 1, Types::Short),          // Compression
    (262, 1, Types::Short),          // PhotometricInterpretation
    (266, 1, Types::Short),          // FillOrder
    (273, 1, Types::ShortOrLong),    // StripOffsets, of one strip
    (274, 1, Types::Short),          // Orientation
    (277, 1, Types::Short),          // SamplesPerPixel
    (278, 1, Types::ShortOrLong),    // RowsPerStrip
    (279, 1, Types::ShortOrLong),    // StripByteCounts, of one strip
    (280, PER_SAMPLE, Types::Short), // MinSampleValue
    (281, PER_SAMPLE, Types::Short), // MaxSampleValue
    (282, 1, Types::Rational),       // XResolution
    (283, 1, Types::Rational),       // YResolution
    (284, 1, Types::Short),          // PlanarConfiguration
    (296, 1, Types::Short),          // ResolutionUnit
    (297, 2, Types::Short),          // PageNumber: page, and of how many
    (317, 1, Types::Short),          // Predictor
    (318, 2, Types::Rational),       // WhitePoint: x and y
    (319, 6, Types::Rational),       // PrimaryChromaticities
    (322, 1, Types::ShortOrLong),    // TileWidth
    (323, 1, Types::ShortOrLong),    // TileLength
    (324, 1, Types::Long),           // TileOffsets, of one tile
    (325, 1, Types::ShortOrLong),    // TileByteCounts, of one tile
    (338, PER_SAMPLE, Types::Short), // ExtraSamples
    (339, PER_SAMPLE, Types::Short), // SampleFormat
    (529, 3, Types::Rational),       // YCbCrCoefficients
    (530, 2, Types::Short),          // YCbCrSubSampling
    (531, 1, Types::Short),          // YCbCrPositioning
    (532, 6, Types::Rational),       // ReferenceBlackWhite
    (34665, 1, Types::Pointer),      // the EXIF directory
    (34853, 1, Types::Pointer),      // the GPS directory
];

/// The entries of an EXIF directory a masked session shows, as
/// [`IMAGE_ENTRIES`]: the colour space, the pixel dimensions and the
/// pointer to the interoperability directory. Exposure settings are not
/// listed.
const EXIF_ENTRIES: &[Entry] = &[
    (40961, 1, Types::Short),       // ColorSpace
    (40962, 1, Types::ShortOrLong), // PixelXDimension
    (40963, 1, Types::ShortOrLong), // PixelYDimension
    (40965, 1, Types::Pointer),     // the interoperability directory
];

/// The listed entries of one directory, and which of them it has shown or
/// masked so far: only the first entry of a tag is ever shown.
struct Listed {
    entries: &'static [Entry],
    /// Whether the directory is a TIFF file's own, which may be a BigTIFF.
    big: bool,
    seen: Vec<u16>,
}

impl Listed {
    fn new(entries: &'static [Entry], big: bool) -> Self {
        Self {
            entries,
            big,
            seen: Vec::new(),
        }
    }

    /// Whether the entry `node`, the next of its directory, is shown.
    fn shows(&mut self, node: &TagNode) -> bool {
        let Some((tag, most, types)) = node
            .tag
            .strip_prefix("0x")
            .filter(|digits| digits.len() == 4)
            .and_then(|digits| u16::from_str_radix(digits, 16).ok())
            .and_then(|number| {
                let index = self.entries.binary_search_by_key(&number, |entry| entry.0);
                index.ok().map(|index| self.entries[index])
            })
        else {
            return false;
        };
        let first = !self.seen.contains(&tag);
        if first {
            self.seen.push(tag);
        }
        // Text is shown only where this module's reader composed it from
        // numbers, which is a rational.
        let text = matches!(node.value, TagValue::String { .. });
        first
            && types.hold(&node.vr, self.big)
            && text == matches!(types, Types::Rational)
            && numbers_in(&node.value) <= most
            && looks_as_listed(&node.value, Shown::Numeric)
    }
}

/// Masks a raster's metadata tree in place. Groups stay, with their names,
/// which are dcmview's own; a leaf keeps its `tag`, `keyword` and `vr`,
/// which never hold bytes of the file, and its value only when listed.
pub(super) fn mask_tree(nodes: &mut [TagNode]) {
    for node in nodes {
        if is_group(node) {
            match node.tag.as_str() {
                // The EXIF block: only its directories, by name.
                "EXIF" => mask_children(node, None, false),
                tag if tag.starts_with("TIFF:page ") => {
                    mask_children(node, Some(IMAGE_ENTRIES), true);
                }
                _ => mask_children(node, Some(&[]), false),
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
/// none), and the directories inside it by their own lists. `big` says the
/// group is a page of a TIFF file.
fn mask_children(group: &mut TagNode, entries: Option<&'static [Entry]>, big: bool) {
    let TagValue::Sequence { items, .. } = &mut group.value else {
        return;
    };
    let mut listed = Listed::new(entries.unwrap_or(&[]), big);
    for child in items.iter_mut().flatten() {
        if is_group(child) {
            let entries = match child.tag.as_str() {
                "IFD0" | "IFD1" => IMAGE_ENTRIES,
                "Exif" => EXIF_ENTRIES,
                // GPS, interoperability and anything unnamed: nothing.
                _ => &[],
            };
            mask_directory(child, Listed::new(entries, big));
        } else if !listed.shows(child) {
            child.value = masked_value();
        }
    }
}

/// Masks a directory's entries by `listed`. Nothing lies deeper; a group
/// found here is masked whole.
fn mask_directory(directory: &mut TagNode, mut listed: Listed) {
    let TagValue::Sequence { items, .. } = &mut directory.value else {
        return;
    };
    for entry in items.iter_mut().flatten() {
        if is_group(entry) || !listed.shows(entry) {
            entry.value = masked_value();
        }
    }
}

/// How many numbers, or rationals, a directory entry's value holds, as the
/// file states it: what counts is the entry's count, not how many of its
/// numbers the tree shows.
fn numbers_in(value: &TagValue) -> usize {
    match value {
        TagValue::Number { .. } => 1,
        TagValue::Numbers { value, total, .. } => total.unwrap_or(0).max(value.len()),
        // Rationals, as the reader joins them; a cut list ends with `…`.
        TagValue::String { value } if value.contains('…') => usize::MAX,
        TagValue::String { value } => value.split(',').count(),
        // A problem is fixed words, and the others are never shown.
        TagValue::Error { .. } | TagValue::Binary { .. } | TagValue::Sequence { .. } => 0,
    }
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
            assert!(list.windows(2).all(|pair| pair[0].0 < pair[1].0));
        }
    }
}

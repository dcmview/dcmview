//! File formats discovery recognizes: the `--formats` selection and the
//! content signatures of the raster formats
//! (`docs/design/image-formats.md` section 3).

use crate::types::FileFormat;
use std::str::FromStr;

/// The formats a discovery loads from the directories it walks (`--formats`).
///
/// The selection never applies to a file named as an input path itself:
/// such a file is loaded in whatever supported format it has. A file of a
/// recognized format outside the selection is skipped with
/// `DiscoveryReason::FormatNotSelected`, without its header being parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatSelection {
    /// Indexed like [`FileFormat::ALL`].
    selected: [bool; FileFormat::ALL.len()],
}

impl FormatSelection {
    /// Every format: the default, and what applies to an explicit input file.
    pub const fn all() -> Self {
        Self {
            selected: [true; FileFormat::ALL.len()],
        }
    }

    /// Exactly `formats`; repeats are harmless.
    pub fn only(formats: &[FileFormat]) -> Self {
        let mut selected = [false; FileFormat::ALL.len()];
        for format in formats {
            selected[slot(*format)] = true;
        }
        Self { selected }
    }

    pub fn contains(self, format: FileFormat) -> bool {
        self.selected[slot(format)]
    }

    /// Whether nothing is excluded, as when `--formats` is not given.
    pub fn is_all(self) -> bool {
        self.selected.iter().all(|selected| *selected)
    }
}

fn slot(format: FileFormat) -> usize {
    FileFormat::ALL
        .iter()
        .position(|candidate| *candidate == format)
        .expect("FileFormat::ALL lists every format")
}

impl Default for FormatSelection {
    fn default() -> Self {
        Self::all()
    }
}

/// The selected names in [`FileFormat::ALL`] order, comma-separated, in the
/// form [`FromStr`] reads back: `dicom,png`.
impl std::fmt::Display for FormatSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = FileFormat::ALL
            .into_iter()
            .filter(|format| self.contains(*format))
            .map(FileFormat::as_str)
            .collect::<Vec<_>>();
        formatter.write_str(&names.join(","))
    }
}

/// Parses the value of `--formats`: a comma-separated list of
/// [`FileFormat::as_str`] names.
///
/// - Names match case-insensitively; whitespace around a name is ignored; a
///   name may repeat.
/// - Only the five names are accepted: no aliases (`jpg`, `tif`), no `all`.
/// - An empty value, an empty item (`dicom,,png`, a trailing comma) or an
///   unknown name is an error. The message names the offending item and lists
///   the accepted names in [`FileFormat::ALL`] order; clap prints it as the
///   flag's value error.
impl FromStr for FormatSelection {
    type Err = String;

    fn from_str(_raw: &str) -> Result<Self, Self::Err> {
        todo!("FMT1: parse the --formats list")
    }
}

/// The raster format whose signature `prefix` starts with, if any.
///
/// `prefix` is the start of the file: up to 132 bytes, fewer for a shorter
/// file. The caller has already ruled out DICOM, whose `DICM` at offset 128
/// wins over any signature in the preamble (a DICOM WSI "dual-personality"
/// file starts with a TIFF header and stays DICOM). Never returns
/// [`FileFormat::Dicom`]. A signature match says nothing about whether the
/// rest of the header parses.
pub(super) fn sniff_raster_format(prefix: &[u8]) -> Option<FileFormat> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff];
    // Classic TIFF (42) and BigTIFF (43), in either byte order.
    const TIFF: [&[u8]; 4] = [b"II\x2a\x00", b"MM\x00\x2a", b"II\x2b\x00", b"MM\x00\x2b"];

    if prefix.starts_with(PNG) {
        Some(FileFormat::Png)
    } else if prefix.starts_with(JPEG) {
        Some(FileFormat::Jpeg)
    } else if TIFF.iter().any(|magic| prefix.starts_with(magic)) {
        Some(FileFormat::Tiff)
    } else if prefix.len() >= 12 && prefix.starts_with(b"RIFF") && &prefix[8..12] == b"WEBP" {
        Some(FileFormat::Webp)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{sniff_raster_format, FormatSelection};
    use crate::types::FileFormat;

    #[test]
    fn signatures_select_each_raster_format_and_nothing_else() {
        let cases: &[(&[u8], Option<FileFormat>)] = &[
            (b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR", Some(FileFormat::Png)),
            (b"\xff\xd8\xff\xe0\0\x10JFIF", Some(FileFormat::Jpeg)),
            (b"II\x2a\0\x08\0\0\0", Some(FileFormat::Tiff)),
            (b"MM\0\x2a\0\0\0\x08", Some(FileFormat::Tiff)),
            (b"II\x2b\0\x08\0\0\0", Some(FileFormat::Tiff)),
            (b"MM\0\x2b\0\x08\0\0", Some(FileFormat::Tiff)),
            (b"RIFF\x24\0\0\0WEBPVP8 ", Some(FileFormat::Webp)),
            (b"RIFF\x24\0\0\0WAVEfmt ", None),
            (b"RIFF\x24\0\0\0WEB", None),
            (b"\x89PNG", None),
            (b"GIF89a", None),
            (b"plain text", None),
            (b"", None),
        ];
        for (prefix, expected) in cases {
            assert_eq!(sniff_raster_format(prefix), *expected, "{prefix:?}");
        }
    }

    #[test]
    fn formats_list_parses_names_and_rejects_everything_else() {
        let parsed = |raw: &str| raw.parse::<FormatSelection>();
        assert_eq!(
            parsed("dicom"),
            Ok(FormatSelection::only(&[FileFormat::Dicom]))
        );
        assert_eq!(
            parsed(" PNG , dicom,png "),
            Ok(FormatSelection::only(&[FileFormat::Dicom, FileFormat::Png]))
        );
        assert_eq!(
            parsed("dicom,png,jpeg,tiff,webp"),
            Ok(FormatSelection::all())
        );
        // What is printed parses back to the same selection.
        let subset = FormatSelection::only(&[FileFormat::Tiff, FileFormat::Jpeg]);
        assert_eq!(subset.to_string(), "jpeg,tiff");
        assert_eq!(parsed(&subset.to_string()), Ok(subset));

        for rejected in ["", " ", "dicom,", "dicom,,png", "jpg", "tif", "all", "gif"] {
            // The message lists the accepted names, whatever its wording.
            let error = parsed(rejected).expect_err(rejected);
            for name in FileFormat::ALL.map(FileFormat::as_str) {
                assert!(error.contains(name), "{rejected:?}: {error}");
            }
        }
    }
}

use anyhow::{Context, Result};
use dicom_core::Tag;
use dicom_object::{DefaultDicomObject, OpenFileOptions};
use std::path::Path;

/// Float Pixel Data (7FE0,0008) is the first standard pixel element, so
/// stopping here reads every presentation attribute without any pixel values.
const FIRST_PIXEL_ELEMENT: Tag = Tag(0x7FE0, 0x0008);

/// Opens the data set up to, but excluding, its pixel data.
///
/// Frame requests use this for padding, ICC, and palette attributes so that
/// looking them up does not read every frame of a multi-frame object.
pub(crate) fn open_header(path: &Path) -> Result<DefaultDicomObject> {
    OpenFileOptions::new()
        .read_until(FIRST_PIXEL_ELEMENT)
        .open_file(path)
        .with_context(|| format!("failed to read DICOM header: {}", path.display()))
}

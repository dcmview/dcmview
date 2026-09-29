use super::metadata::{
    normalize_pixel_aspect, read_exact_f64s, read_frame_patient_geometry, read_lut_sequence,
    read_positive_f64_pair, read_positive_u32_pair, read_presentation_metadata,
    read_sequence_strings,
};
use super::DiscoveryReason;
use crate::api::contracts::WindowPreset;
use crate::dicom_values::{read_first_string, read_number, read_strings, sequence_items};
use crate::pixels::{read_pixel_padding_range, NativeByteOrder, NativeFrameLayout};
use crate::types::{FileEntry, NativePixelDataKind, NativePixelMetadata, SeriesMetadata};
use anyhow::{bail, Context, Result};
use dicom_core::value::{DataSetSequence, InMemFragment, PixelFragmentSequence, Value};
use dicom_core::VR;
use dicom_dictionary_std::{tags, uids};
use dicom_encoding::text::SpecificCharacterSet;
use dicom_encoding::{Codec, TransferSyntax, TransferSyntaxIndex};
use dicom_object::mem::InMemElement;
use dicom_object::{FileMetaTable, InMemDicomObject};
use dicom_parser::dataset::read::{DataSetReader, Result as ParserResult};
use dicom_parser::dataset::DataToken;
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

pub(super) enum EntryInspection {
    Selected(Box<FileEntry>),
    Skipped(DiscoveryReason),
}

#[cfg(test)]
pub(super) fn build_entry(path: &Path) -> Result<EntryInspection> {
    build_entry_selected(path, &|_| true, &|| Ok(()))
}

pub(super) fn build_entry_selected(
    path: &Path,
    selected: &impl Fn(&FileEntry) -> bool,
    check_active: &impl Fn() -> Result<()>,
) -> Result<EntryInspection> {
    let DiscoveryHeader {
        object: obj,
        odd_item_length,
        pixels: pixel_header,
    } = match read_discovery_header(path)? {
        HeaderRead::NotPart10 => {
            return Ok(EntryInspection::Skipped(
                DiscoveryReason::MissingPart10Preamble,
            ))
        }
        HeaderRead::ParseFailed => {
            return Ok(EntryInspection::Skipped(DiscoveryReason::DicomParseFailed))
        }
        HeaderRead::Read(header) => *header,
    };
    if odd_item_length || !valid_character_set(&obj) {
        return Ok(EntryInspection::Skipped(DiscoveryReason::DicomParseFailed));
    }

    let transfer_syntax_uid = obj.meta().transfer_syntax().to_string();
    let patient_id = read_first_string(&obj, tags::PATIENT_ID).unwrap_or_default();
    let patient_name = read_first_string(&obj, tags::PATIENT_NAME).unwrap_or_default();
    let modality = read_first_string(&obj, tags::MODALITY).unwrap_or_default();
    let sop_instance_uid = read_first_string(&obj, tags::SOP_INSTANCE_UID).unwrap_or_default();
    let sop_class_uid = read_first_string(&obj, tags::SOP_CLASS_UID).unwrap_or_default();
    if sop_class_uid == uids::MEDIA_STORAGE_DIRECTORY_STORAGE {
        return Ok(EntryInspection::Skipped(
            DiscoveryReason::UnsupportedMediaDirectory,
        ));
    }
    let study_instance_uid = read_first_string(&obj, tags::STUDY_INSTANCE_UID).unwrap_or_default();
    let study_date = read_first_string(&obj, tags::STUDY_DATE).unwrap_or_default();
    let study_description = read_first_string(&obj, tags::STUDY_DESCRIPTION).unwrap_or_default();
    let series_instance_uid =
        read_first_string(&obj, tags::SERIES_INSTANCE_UID).unwrap_or_default();
    let series_number = read_first_string(&obj, tags::SERIES_NUMBER).unwrap_or_default();
    let series_description = read_first_string(&obj, tags::SERIES_DESCRIPTION).unwrap_or_default();
    let instance_number = read_first_string(&obj, tags::INSTANCE_NUMBER).unwrap_or_default();
    let rows = read_number::<u32>(&obj, tags::ROWS).unwrap_or(0);
    let columns = read_number::<u32>(&obj, tags::COLUMNS).unwrap_or(0);
    let bits_allocated = read_number::<u32>(&obj, tags::BITS_ALLOCATED).unwrap_or(8);
    let samples_per_pixel = read_number::<u32>(&obj, tags::SAMPLES_PER_PIXEL)
        .unwrap_or(1)
        .max(1);
    let photometric_interpretation = read_first_string(&obj, tags::PHOTOMETRIC_INTERPRETATION)
        .unwrap_or_else(|| "MONOCHROME2".to_string());
    let pixel_header =
        pixel_header.with_context(|| format!("failed to parse {}", path.display()))?;
    let pixel_data_kind = pixel_header.as_ref().map(|header| header.kind);
    let has_pixels = pixel_header.is_some();
    let frame_count = present_frame_count(
        read_number::<u32>(&obj, tags::NUMBER_OF_FRAMES).unwrap_or(1),
        sequence_items(&obj, tags::PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE).len(),
        pixel_header.as_ref(),
        NativeFrameLayout {
            rows,
            columns,
            samples_per_pixel,
            bits_allocated,
            planar_configuration: None,
            photometric_interpretation: &photometric_interpretation,
            byte_order: NativeByteOrder::LittleEndian,
        },
    );
    let frame_of_reference_uid =
        read_first_string(&obj, tags::FRAME_OF_REFERENCE_UID).unwrap_or_default();
    let image_position_patient = read_exact_f64s(&obj, tags::IMAGE_POSITION_PATIENT);
    let image_orientation_patient = read_exact_f64s(&obj, tags::IMAGE_ORIENTATION_PATIENT);
    let top_level_pixel_spacing = read_positive_f64_pair(&obj, tags::PIXEL_SPACING);
    let (
        frame_image_positions_patient,
        frame_image_orientations_patient,
        frame_pixel_spacings,
        shared_pixel_spacing,
    ) = read_frame_patient_geometry(
        &obj,
        frame_count,
        image_position_patient,
        image_orientation_patient,
        top_level_pixel_spacing,
    );
    let concatenation_uid = read_first_string(&obj, tags::CONCATENATION_UID);
    let in_concatenation_number = read_number::<u32>(&obj, tags::IN_CONCATENATION_NUMBER);
    let in_concatenation_total_number =
        read_number::<u32>(&obj, tags::IN_CONCATENATION_TOTAL_NUMBER);
    let concatenation_frame_offset_number =
        read_number::<u32>(&obj, tags::CONCATENATION_FRAME_OFFSET_NUMBER);
    let sop_instance_uid_of_concatenation_source =
        read_first_string(&obj, tags::SOP_INSTANCE_UID_OF_CONCATENATION_SOURCE);
    let image_type = read_strings(&obj, tags::IMAGE_TYPE);
    let pyramid_uid = read_first_string(&obj, tags::PYRAMID_UID);
    let dimension_organization_type = read_first_string(&obj, tags::DIMENSION_ORGANIZATION_TYPE);
    let dimension_organization_uids = read_sequence_strings(
        &obj,
        tags::DIMENSION_ORGANIZATION_SEQUENCE,
        tags::DIMENSION_ORGANIZATION_UID,
    );
    let image_orientation_slide = read_exact_f64s(&obj, tags::IMAGE_ORIENTATION_SLIDE);
    let total_pixel_matrix_rows = read_number::<u32>(&obj, tags::TOTAL_PIXEL_MATRIX_ROWS);
    let total_pixel_matrix_columns = read_number::<u32>(&obj, tags::TOTAL_PIXEL_MATRIX_COLUMNS);
    let total_pixel_matrix_focal_planes =
        read_number::<u32>(&obj, tags::TOTAL_PIXEL_MATRIX_FOCAL_PLANES);
    let number_of_optical_paths = read_number::<u32>(&obj, tags::NUMBER_OF_OPTICAL_PATHS);
    let container_identifier = read_first_string(&obj, tags::CONTAINER_IDENTIFIER);
    let specimen_uids = read_sequence_strings(
        &obj,
        tags::SPECIMEN_DESCRIPTION_SEQUENCE,
        tags::SPECIMEN_UID,
    );
    let optical_path_identifiers = read_sequence_strings(
        &obj,
        tags::OPTICAL_PATH_SEQUENCE,
        tags::OPTICAL_PATH_IDENTIFIER,
    );
    let planar_configuration = read_number::<u32>(&obj, tags::PLANAR_CONFIGURATION);
    let bits_stored = read_number::<u32>(&obj, tags::BITS_STORED);
    let high_bit = read_number::<u32>(&obj, tags::HIGH_BIT);
    let pixel_spacing = top_level_pixel_spacing.or(shared_pixel_spacing);
    let pixel_aspect_ratio = read_positive_u32_pair(&obj, tags::PIXEL_ASPECT_RATIO);
    let normalized_pixel_aspect = normalize_pixel_aspect(pixel_spacing, pixel_aspect_ratio);
    let modality_lut = read_lut_sequence(&obj, tags::MODALITY_LUT_SEQUENCE);
    let voi_lut = read_lut_sequence(&obj, tags::VOILUT_SEQUENCE);
    let presentation = read_presentation_metadata(&obj, frame_count);
    let pixel_representation = read_number::<u32>(&obj, tags::PIXEL_REPRESENTATION).unwrap_or(0);
    let rescale_slope = read_number::<f64>(&obj, tags::RESCALE_SLOPE).unwrap_or(1.0);
    let rescale_intercept = read_number::<f64>(&obj, tags::RESCALE_INTERCEPT).unwrap_or(0.0);
    let default_window = match (
        read_number::<f64>(&obj, tags::WINDOW_CENTER),
        read_number::<f64>(&obj, tags::WINDOW_WIDTH),
    ) {
        (Some(center), Some(width)) => Some(WindowPreset { center, width }),
        _ => None,
    };

    let fallback_label = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string_lossy().to_string());
    let label = build_label(&patient_id, &modality, &study_date, &fallback_label);

    let mut entry = FileEntry {
        index: 0,
        path: path.to_path_buf(),
        label,
        patient_id,
        patient_name,
        study_instance_uid,
        study_date,
        study_description,
        series_instance_uid,
        series_number,
        series_description,
        modality,
        instance_number,
        sop_instance_uid,
        sop_class_uid,
        series_metadata: Box::new(SeriesMetadata {
            binary_fractional_seg_maximum: None,
            native_pixel: NativePixelMetadata {
                planar_configuration,
                bits_stored,
                high_bit,
                pixel_data_kind,
                pixel_spacing,
                pixel_aspect_ratio,
                normalized_pixel_aspect,
                modality_lut,
                voi_lut,
                pixel_padding: pixel_data_kind
                    .and_then(|kind| read_pixel_padding_range(&obj, kind))
                    .map(|range| range.bounds().into()),
            },
            presentation,
            frame_of_reference_uid,
            image_position_patient,
            image_orientation_patient,
            frame_image_positions_patient,
            frame_image_orientations_patient,
            frame_pixel_spacings,
            concatenation_uid,
            in_concatenation_number,
            in_concatenation_total_number,
            concatenation_frame_offset_number,
            sop_instance_uid_of_concatenation_source,
            image_type,
            pyramid_uid,
            dimension_organization_type,
            dimension_organization_uids,
            image_orientation_slide,
            total_pixel_matrix_rows,
            total_pixel_matrix_columns,
            total_pixel_matrix_focal_planes,
            number_of_optical_paths,
            container_identifier,
            specimen_uids,
            optical_path_identifiers,
        }),
        has_pixels,
        frame_count,
        rows,
        columns,
        bits_allocated,
        pixel_representation,
        samples_per_pixel,
        photometric_interpretation,
        rescale_slope,
        rescale_intercept,
        transfer_syntax_uid,
        default_window,
    };
    // Discovery already runs on blocking workers. Retain only the verdict,
    // never the decoded masks, and finish before publishing this entry.
    if selected(&entry)
        && entry.sop_class_uid == uids::SEGMENTATION_STORAGE
        && read_first_string(&obj, tags::SEGMENTATION_TYPE).as_deref() == Some("FRACTIONAL")
        && read_number::<u32>(&obj, tags::NUMBER_OF_FRAMES).unwrap_or(1) == entry.frame_count
    {
        if let Some(maximum) =
            read_number::<u32>(&obj, tags::MAXIMUM_FRACTIONAL_VALUE).filter(|maximum| *maximum > 1)
        {
            check_active()?;
            match crate::pixels::segmentation_has_only_binary_samples(&entry, &obj, check_active) {
                Ok(true) => entry.series_metadata.binary_fractional_seg_maximum = Some(maximum),
                Ok(false) => {}
                Err(error) => tracing::warn!(path = %path.display(), %error,
                    "could not inspect fractional SEG samples; retaining declared interpretation"),
            }
        }
    }
    check_active()?;
    Ok(EntryInspection::Selected(Box::new(entry)))
}

fn valid_character_set(object: &dicom_object::DefaultDicomObject) -> bool {
    object
        .get(tags::SPECIFIC_CHARACTER_SET)
        .and_then(|element| element.to_str().ok())
        .map(|value| valid_specific_character_set(&value))
        .unwrap_or(true)
}

fn valid_specific_character_set(value: &str) -> bool {
    value.split('\\').all(|component| {
        let component = component.trim();
        component.is_empty() || SpecificCharacterSet::from_code(component).is_some()
    })
}

/// What discovery reads from one file: the data set up to, but excluding,
/// Float Pixel Data, and the top-level pixel element's header.
struct DiscoveryHeader {
    object: dicom_object::DefaultDicomObject,
    /// A sequence item before the pixel data declares an odd length, which a
    /// conformant data set never does.
    odd_item_length: bool,
    /// The top-level pixel element, or why the file could not be walked to it.
    pixels: Result<Option<PixelDataHeader>>,
}

enum HeaderRead {
    NotPart10,
    ParseFailed,
    Read(Box<DiscoveryHeader>),
}

/// Opens and parses `path` once for everything discovery needs.
///
/// The metadata object is what `OpenFileOptions::read_until(FLOAT_PIXEL_DATA)`
/// builds: float and double-float pixel data precede Pixel Data, so stopping
/// at Float Pixel Data keeps every pixel payload out of the scan. That call
/// drops the element it stopped at and reads through its own buffer, so the
/// object is built here from the same parser's tokens instead, and the parse
/// carries on from the stop token to the top-level pixel element's header.
fn read_discovery_header(path: &Path) -> Result<HeaderRead> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let file_length = file
        .metadata()
        .with_context(|| format!("failed to stat {}", path.display()))?
        .len();
    let mut reader = BufReader::new(file);
    let mut preamble = [0_u8; 132];
    match reader.read_exact(&mut preamble) {
        Ok(()) if &preamble[128..132] == b"DICM" => {}
        Ok(()) => return Ok(HeaderRead::NotPart10),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            return Ok(HeaderRead::NotPart10)
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()))
        }
    }
    // The file meta reader expects the magic code.
    reader
        .seek_relative(-4)
        .with_context(|| format!("failed to seek {}", path.display()))?;
    let Ok(meta) = FileMetaTable::from_reader(&mut reader) else {
        return Ok(HeaderRead::ParseFailed);
    };
    let Some(transfer_syntax) = TransferSyntaxRegistry.get(meta.transfer_syntax()) else {
        return Ok(HeaderRead::ParseFailed);
    };
    // Deflated data sets are parsed through the syntax's own adapter, as
    // dicom-object does.
    Ok(match transfer_syntax.codec() {
        Codec::Dataset(Some(adapter)) => parse_data_set(
            adapter.adapt_reader(Box::new(reader)),
            transfer_syntax,
            meta,
            file_length,
        ),
        Codec::Dataset(None) => HeaderRead::ParseFailed,
        _ => parse_data_set(reader, transfer_syntax, meta, file_length),
    })
}

/// Builds the discovery header from the data set after the file meta.
fn parse_data_set(
    source: impl Read,
    transfer_syntax: &TransferSyntax,
    meta: FileMetaTable,
    file_length: u64,
) -> HeaderRead {
    let Ok(mut tokens) = DataSetReader::new_with_ts(source, transfer_syntax) else {
        return HeaderRead::ParseFailed;
    };
    let mut odd_item_length = false;
    let Ok((elements, stop)) = read_elements(&mut tokens, false, &mut odd_item_length) else {
        return HeaderRead::ParseFailed;
    };
    let pixels = match stop {
        Some(stop) => top_level_pixel_element(std::iter::once(Ok(stop)).chain(tokens)),
        None => Ok(None),
    };
    HeaderRead::Read(Box::new(DiscoveryHeader {
        object: InMemDicomObject::from_element_iter(elements).with_exact_meta(meta),
        odd_item_length,
        pixels: pixels.map(|element| {
            element.map(|(kind, native_length)| PixelDataHeader {
                kind,
                native_length,
                file_length,
            })
        }),
    }))
}

/// Builds data set elements from parser tokens the way dicom-object's
/// builder does. At the top level it stops at the first element from Float
/// Pixel Data on and returns that token unread; inside an item it reads to
/// the item's end, nested pixel data included.
fn read_elements<I>(
    tokens: &mut I,
    in_item: bool,
    odd_item_length: &mut bool,
) -> Result<(Vec<InMemElement>, Option<DataToken>)>
where
    I: Iterator<Item = ParserResult<DataToken>>,
{
    let mut elements = Vec::new();
    while let Some(token) = tokens.next() {
        let token = token?;
        let tag = match &token {
            DataToken::ElementHeader(header) => Some(header.tag),
            DataToken::SequenceStart { tag, .. } => Some(*tag),
            DataToken::PixelSequenceStart => Some(tags::PIXEL_DATA),
            _ => None,
        };
        if !in_item && tag.is_some_and(|tag| tag >= tags::FLOAT_PIXEL_DATA) {
            return Ok((elements, Some(token)));
        }
        let element = match token {
            DataToken::ElementHeader(header) => {
                match tokens.next().context("element has no value")?? {
                    DataToken::PrimitiveValue(value) => InMemElement::new_with_len(
                        header.tag,
                        header.vr,
                        header.len,
                        Value::Primitive(value),
                    ),
                    token => bail!("unexpected token {token:?} for {}", header.tag),
                }
            }
            DataToken::SequenceStart { tag, len } => {
                let items = read_items(tokens, odd_item_length)?;
                InMemElement::new_with_len(
                    tag,
                    VR::SQ,
                    len,
                    Value::Sequence(DataSetSequence::new(items, len)),
                )
            }
            DataToken::PixelSequenceStart => {
                InMemElement::new(tags::PIXEL_DATA, VR::OB, read_fragments(tokens)?)
            }
            DataToken::ItemEnd if in_item => return Ok((elements, None)),
            token => bail!("unexpected token {token:?}"),
        };
        elements.push(element);
    }
    Ok((elements, None))
}

fn read_items<I>(tokens: &mut I, odd_item_length: &mut bool) -> Result<Vec<InMemDicomObject>>
where
    I: Iterator<Item = ParserResult<DataToken>>,
{
    let mut items = Vec::new();
    while let Some(token) = tokens.next() {
        match token? {
            DataToken::ItemStart { len } => {
                *odd_item_length |= len.get().is_some_and(|length| length % 2 != 0);
                let (elements, _) = read_elements(tokens, true, odd_item_length)?;
                items.push(InMemDicomObject::from_element_iter(elements));
            }
            DataToken::SequenceEnd => return Ok(items),
            token => bail!("unexpected token {token:?} in a sequence"),
        }
    }
    bail!("data set ended inside a sequence")
}

/// Encapsulated pixel data nested in an item, collected as dicom-object does.
fn read_fragments<I>(tokens: &mut I) -> Result<Value<InMemDicomObject, InMemFragment>>
where
    I: Iterator<Item = ParserResult<DataToken>>,
{
    let mut offset_table = None;
    let mut fragments = Vec::new();
    for token in tokens {
        match token? {
            DataToken::OffsetTable(table) => offset_table = Some(table),
            DataToken::ItemValue(data) => fragments.push(data),
            DataToken::ItemStart { .. } | DataToken::ItemEnd => {}
            DataToken::SequenceEnd => break,
            token => bail!("unexpected token {token:?} in pixel data"),
        }
    }
    Ok(Value::PixelSequence(PixelFragmentSequence::new(
        offset_table.unwrap_or_default(),
        fragments,
    )))
}

/// The top-level pixel element as discovery sees it, without reading pixel
/// values.
struct PixelDataHeader {
    kind: NativePixelDataKind,
    /// Value length of a native element; `None` for encapsulated pixel data.
    native_length: Option<u32>,
    file_length: u64,
}

/// `NumberOfFrames` bounded by what the file can hold: the Per-frame
/// Functional Groups items present and the pixel data present. Discovery
/// sizes per-frame geometry, and the catalog per-frame navigation, from this
/// count, so a corrupt or hostile header must not size them beyond the file.
/// Frames past the data would fail to decode anyway. At least one frame is
/// kept so a file whose data is short still reports its decode error.
fn present_frame_count(
    declared: u32,
    per_frame_items: usize,
    pixels: Option<&PixelDataHeader>,
    layout: NativeFrameLayout<'_>,
) -> u32 {
    let mut bound = u64::from(declared);
    if per_frame_items > 0 {
        bound = bound.min(per_frame_items as u64);
    }
    bound = match pixels {
        // No pixel data holds no frames.
        None => bound.min(1),
        Some(PixelDataHeader {
            native_length: Some(length),
            ..
        }) => layout
            .frame_capacity(u64::from(*length))
            .map_or(bound, |capacity| bound.min(capacity)),
        // Every encapsulated frame takes at least one 8-byte item header.
        Some(PixelDataHeader { file_length, .. }) => bound.min(file_length / 8),
    };
    u32::try_from(bound.max(1)).unwrap_or(u32::MAX)
}

/// The kind and native length of the data set's own pixel element, if any.
///
/// Counts only top-level elements, so a pixel element nested in a sequence
/// (an Icon Image Sequence, for example) or pixel-tag bytes inside another
/// value never make a no-pixel object look like an image.
fn top_level_pixel_element(
    tokens: impl Iterator<Item = ParserResult<DataToken>>,
) -> Result<Option<(NativePixelDataKind, Option<u32>)>> {
    let mut depth = 0_usize;
    for token in tokens {
        match token? {
            DataToken::PixelSequenceStart if depth == 0 => {
                return Ok(Some((NativePixelDataKind::Integer, None)));
            }
            DataToken::SequenceStart { .. } | DataToken::PixelSequenceStart => depth += 1,
            DataToken::SequenceEnd => depth = depth.saturating_sub(1),
            DataToken::ElementHeader(header) if depth == 0 => {
                let kind = match header.tag {
                    tags::PIXEL_DATA => NativePixelDataKind::Integer,
                    tags::FLOAT_PIXEL_DATA => NativePixelDataKind::Float32,
                    tags::DOUBLE_FLOAT_PIXEL_DATA => NativePixelDataKind::Float64,
                    _ => continue,
                };
                return Ok(Some((kind, header.len.get())));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn build_label(patient_id: &str, modality: &str, study_date: &str, fallback: &str) -> String {
    let mut fields = Vec::new();
    if !patient_id.is_empty() {
        fields.push(patient_id);
    }
    if !modality.is_empty() {
        fields.push(modality);
    }
    if !study_date.is_empty() {
        fields.push(study_date);
    }

    if fields.is_empty() {
        fallback.to_string()
    } else {
        fields.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fractional_seg_inspection_filters_before_samples_and_honours_cancellation() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-seg-binary-valued-fractional.dcm");
        let checks = std::cell::Cell::new(0);
        let selected = std::cell::Cell::new(false);
        let check = || {
            assert!(selected.get(), "filter must run before sample inspection");
            checks.set(checks.get() + 1);
            Ok(())
        };
        let EntryInspection::Selected(entry) = super::build_entry_selected(
            &fixture,
            &|_| {
                selected.set(true);
                false
            },
            &check,
        )
        .unwrap() else {
            panic!("fixture");
        };
        assert_eq!(
            checks.get(),
            1,
            "filtered objects must not walk pixel samples"
        );
        assert_eq!(entry.series_metadata.binary_fractional_seg_maximum, None);
        checks.set(0);
        let error = super::build_entry_selected(&fixture, &|_| true, &|| {
            checks.set(checks.get() + 1);
            anyhow::ensure!(checks.get() < 10, "cancel sample inspection");
            Ok(())
        })
        .err()
        .expect("cancelled inspection must not publish an entry");
        assert!(error.to_string().contains("cancel sample inspection"));
    }

    #[test]
    fn fractional_seg_streams_native_deflated_and_encapsulated_samples() {
        use std::io::Write;
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-seg-binary-valued-fractional.dcm");
        let directory = tempdir().unwrap();
        let path = directory.path().join("seg.dcm");
        for syntax in [
            uids::EXPLICIT_VR_LITTLE_ENDIAN,
            uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN,
            uids::RLE_LOSSLESS,
            uids::JPEG_BASELINE8_BIT,
            "1.2.840.10008.1.2.8.1",
        ] {
            for last in [1_u8, 2] {
                let mut object = dicom_object::open_file(&fixture).unwrap();
                object.update_meta(|meta| meta.transfer_syntax = syntax.to_owned());
                if syntax == uids::RLE_LOSSLESS {
                    let fragments = [1, last].map(|value| {
                        let mut encoded = vec![0; 64];
                        encoded[..4].copy_from_slice(&1_u32.to_le_bytes());
                        encoded[4..8].copy_from_slice(&64_u32.to_le_bytes());
                        encoded.extend_from_slice(&[3, 0, 1, 0, value, 0x80]);
                        encoded
                    });
                    object.put(DataElement::new(
                        tags::PIXEL_DATA,
                        VR::OB,
                        PixelFragmentSequence::new_fragments(fragments.to_vec()),
                    ));
                } else if syntax == uids::JPEG_BASELINE8_BIT {
                    let fragments = [0, if last == 1 { 0 } else { 128 }].map(|value| {
                        let mut bytes = Vec::new();
                        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 100)
                            .encode(&[value; 4], 2, 2, image::ExtendedColorType::L8)
                            .unwrap();
                        if bytes.len() % 2 != 0 {
                            bytes.push(0);
                        }
                        bytes
                    });
                    object.put(DataElement::new(
                        tags::PIXEL_DATA,
                        VR::OB,
                        PixelFragmentSequence::new_fragments(fragments.to_vec()),
                    ));
                } else if syntax == "1.2.840.10008.1.2.8.1" {
                    for (tag, value) in [
                        (tags::BITS_ALLOCATED, 1_u16),
                        (tags::BITS_STORED, 1),
                        (tags::HIGH_BIT, 0),
                    ] {
                        object.put(DataElement::new(tag, VR::US, PrimitiveValue::from(value)));
                    }
                    let fragments = [0b0101, 0b1010].map(|value| {
                        let mut encoded = flate2::write::DeflateEncoder::new(
                            Vec::new(),
                            flate2::Compression::default(),
                        );
                        encoded.write_all(&[value]).unwrap();
                        let mut bytes = encoded.finish().unwrap();
                        if bytes.len() % 2 != 0 {
                            bytes.push(0);
                        }
                        bytes
                    });
                    object.put(DataElement::new(
                        tags::PIXEL_DATA,
                        VR::OB,
                        PixelFragmentSequence::new_fragments(fragments.to_vec()),
                    ));
                } else {
                    object.put(DataElement::new(
                        tags::PIXEL_DATA,
                        VR::OB,
                        PrimitiveValue::U8(vec![0, 1, 1, 0, 0, 1, 0, last].into()),
                    ));
                }
                object.write_to_file(&path).unwrap();
                let entry = crate::loader::test_entry(&path);
                let expected = if last == 1 || syntax == "1.2.840.10008.1.2.8.1" {
                    Some(255)
                } else {
                    None
                };
                assert_eq!(
                    entry.series_metadata.binary_fractional_seg_maximum, expected,
                    "{syntax}, last {last}"
                );
            }
        }
    }

    #[test]
    fn fractional_seg_inspection_is_cached_and_requires_all_declared_frames() {
        use dicom_core::{DataElement, PrimitiveValue, VR};
        use dicom_dictionary_std::tags;
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-seg-binary-valued-fractional.dcm");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seg.dcm");
        for (maximum, samples, expected) in [
            (255_u16, vec![0, 1, 1, 0, 1, 0, 0, 1], Some(255)),
            (255, vec![0; 8], Some(255)),
            (1, vec![0, 1, 1, 0, 1, 0, 0, 1], None),
            (255, vec![0, 1, 1, 0, 0, 0, 0, 2], None),
            (255, vec![0, 1, 1, 0], None),
        ] {
            let mut object = dicom_object::open_file(&fixture).unwrap();
            object.put(DataElement::new(
                tags::MAXIMUM_FRACTIONAL_VALUE,
                VR::US,
                PrimitiveValue::from(maximum),
            ));
            object.put(DataElement::new(
                tags::PIXEL_DATA,
                VR::OB,
                PrimitiveValue::U8(samples.into()),
            ));
            object.write_to_file(&path).unwrap();
            let entry = crate::loader::test_entry(&path);
            assert_eq!(
                entry.series_metadata.binary_fractional_seg_maximum,
                expected
            );
            std::fs::remove_file(&path).unwrap();
            // A clone retains the verdict without any file access or lazy initialization.
            assert_eq!(
                entry.clone().series_metadata.binary_fractional_seg_maximum,
                expected
            );
        }
        std::fs::copy(&fixture, &path).unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(file.metadata().unwrap().len() - 2).unwrap();
        let entry = crate::loader::test_entry(&path);
        assert_eq!(
            entry.series_metadata.binary_fractional_seg_maximum, None,
            "a truncated final frame cannot promote the complete-looking first frame"
        );
    }

    use super::super::test_fixtures::base_object;
    use super::{
        build_entry, read_discovery_header, valid_specific_character_set, EntryInspection,
        HeaderRead,
    };
    use dicom_core::value::PixelFragmentSequence;
    use dicom_core::{DataElement, PrimitiveValue, Tag, VR};
    use dicom_dictionary_std::{tags, uids};
    use dicom_object::meta::FileMetaTableBuilder;
    use dicom_object::{InMemDicomObject, OpenFileOptions};
    use std::path::Path;
    use tempfile::tempdir;

    use crate::types::NativePixelDataKind;

    #[test]
    fn validates_each_component_of_multi_valued_character_sets() {
        assert!(valid_specific_character_set(
            "\\ISO 2022 IR 87\\ISO 2022 IR 13"
        ));
        assert!(!valid_specific_character_set(
            "ISO_IR 100\\DTS_UNKNOWN_CHARACTER_SET"
        ));
    }

    #[test]
    #[ignore = "requires the independently generated prepared DICOM corpus"]
    fn recognizes_prepared_deflated_image_frame_segmentation_metadata() {
        let path = crate::loader::prepared_corpus_case(
            "derived/seg/binary_multiframe_deflated_image_frame/instance.dcm",
        );

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect prepared SEG")
        else {
            panic!("prepared SEG should be selected");
        };

        assert_eq!(file.sop_class_uid, "1.2.840.10008.5.1.4.1.1.66.4");
        assert_eq!(file.transfer_syntax_uid, "1.2.840.10008.1.2.8.1");
        assert_eq!(file.modality, "SEG");
        assert_eq!(file.frame_count, 2);
        assert_eq!((file.rows, file.columns), (2, 2));
        assert_eq!(file.bits_allocated, 1);
        assert!(file.has_pixels);
    }

    #[test]
    fn bounds_frame_count_by_the_pixel_data_present() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("absurd-frames.dcm");
        let mut object = base_object();
        object.put(DataElement::new(
            tags::NUMBER_OF_FRAMES,
            VR::IS,
            u32::MAX.to_string(),
        ));
        // Enhanced objects always carry a Shared Functional Groups item, which
        // used to size per-frame geometry by the declared frame count.
        object.put(DataElement::new(
            tags::SHARED_FUNCTIONAL_GROUPS_SEQUENCE,
            VR::SQ,
            dicom_core::value::DataSetSequence::from(vec![
                dicom_object::InMemDicomObject::from_element_iter([DataElement::new(
                    tags::PLANE_ORIENTATION_SEQUENCE,
                    VR::SQ,
                    dicom_core::value::DataSetSequence::from(vec![
                        dicom_object::InMemDicomObject::from_element_iter([DataElement::new(
                            tags::IMAGE_ORIENTATION_PATIENT,
                            VR::DS,
                            "1\\0\\0\\0\\1\\0",
                        )]),
                    ]),
                )]),
            ]),
        ));
        // Three whole 2x2 16-bit frames and half of a fourth.
        object.put(DataElement::new(
            tags::PIXEL_DATA,
            VR::OW,
            PrimitiveValue::U16(vec![0_u16; 14].into()),
        ));
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                    .media_storage_sop_class_uid(uids::ENHANCED_CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta")
            .write_to_file(&path)
            .expect("write fixture");

        let EntryInspection::Selected(file) = build_entry(&path).expect("inspect fixture") else {
            panic!("fixture should be selected");
        };
        assert_eq!(file.frame_count, 3);
        assert_eq!(
            file.series_metadata.frame_image_orientations_patient.len(),
            3
        );
    }

    fn write_object(path: &Path, object: InMemDicomObject, transfer_syntax: &str) {
        object
            .with_meta(
                FileMetaTableBuilder::new()
                    .transfer_syntax(transfer_syntax)
                    .media_storage_sop_class_uid(uids::ENHANCED_CT_IMAGE_STORAGE)
                    .media_storage_sop_instance_uid("2.25.300"),
            )
            .expect("file meta")
            .write_to_file(path)
            .expect("write fixture");
    }

    /// The single parse builds exactly the object dicom-object's
    /// `read_until(FLOAT_PIXEL_DATA)` builds, and finds the pixel element the
    /// former second walk found, for every kind of pixel data and syntax.
    #[test]
    fn one_parse_matches_read_until_and_finds_each_pixel_element() {
        use NativePixelDataKind::{Float32, Float64, Integer};
        let native = || {
            DataElement::new(
                tags::PIXEL_DATA,
                VR::OW,
                PrimitiveValue::U16(vec![1_u16; 8].into()),
            )
        };
        let fragments = || {
            DataElement::new(
                tags::PIXEL_DATA,
                VR::OB,
                PixelFragmentSequence::new(
                    Vec::<u32>::new(),
                    vec![vec![0xff_u8, 0xd8, 0xff, 0xd9]],
                ),
            )
        };
        let icons = |pixel_data| {
            DataElement::new(
                tags::ICON_IMAGE_SEQUENCE,
                VR::SQ,
                dicom_core::value::DataSetSequence::from(vec![
                    InMemDicomObject::from_element_iter([
                        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(1_u16)),
                        pixel_data,
                    ]),
                ]),
            )
        };
        let padding = || {
            DataElement::new(
                tags::DATA_SET_TRAILING_PADDING,
                VR::OB,
                PrimitiveValue::from(vec![0_u8; 4]),
            )
        };
        let float = DataElement::new(
            tags::FLOAT_PIXEL_DATA,
            VR::OF,
            PrimitiveValue::F32(vec![0.0_f32, 1.0].into()),
        );
        let double = DataElement::new(
            tags::DOUBLE_FLOAT_PIXEL_DATA,
            VR::OD,
            PrimitiveValue::F64(vec![0.0_f64, 1.0].into()),
        );
        // Little-endian (7FE0,0010) bytes inside an unrelated value.
        let tag_bytes = || {
            DataElement::new(
                Tag(0x0009, 0x1010),
                VR::OB,
                PrimitiveValue::from(vec![0xe0_u8, 0x7f, 0x10, 0x00]),
            )
        };
        let (le, jpeg) = (uids::EXPLICIT_VR_LITTLE_ENDIAN, uids::JPEG_BASELINE8_BIT);
        let words = Some((Integer, Some(16)));
        let cases = [
            ("explicit", le, vec![native()], words),
            (
                "implicit",
                uids::IMPLICIT_VR_LITTLE_ENDIAN,
                vec![native()],
                words,
            ),
            ("big-endian", "1.2.840.10008.1.2.2", vec![native()], words),
            (
                "deflated",
                uids::DEFLATED_EXPLICIT_VR_LITTLE_ENDIAN,
                vec![native()],
                words,
            ),
            ("float", le, vec![float], Some((Float32, Some(8)))),
            ("double", le, vec![double], Some((Float64, Some(16)))),
            (
                "encapsulated",
                jpeg,
                vec![fragments()],
                Some((Integer, None)),
            ),
            ("none", le, vec![], None),
            ("padding only", le, vec![padding()], None),
            (
                "icon and tag bytes",
                le,
                vec![icons(native()), tag_bytes()],
                None,
            ),
            (
                "icon, pixels, padding",
                le,
                vec![icons(native()), native(), padding()],
                words,
            ),
            (
                "encapsulated icon",
                jpeg,
                vec![icons(fragments()), fragments()],
                Some((Integer, None)),
            ),
        ];

        let directory = tempdir().expect("temp directory");
        for (name, transfer_syntax, elements, expected_pixels) in cases {
            let path = directory.path().join(format!("{name}.dcm"));
            let mut object = base_object();
            for element in elements {
                object.put(element);
            }
            write_object(&path, object, transfer_syntax);

            let expected = OpenFileOptions::new()
                .read_until(tags::FLOAT_PIXEL_DATA)
                .open_file(&path)
                .expect("dicom-object reads the fixture");
            let HeaderRead::Read(header) = read_discovery_header(&path).expect("read header")
            else {
                panic!("{name} should parse");
            };
            // dicom-core never finds two undefined lengths equal, so compare
            // the full debug form, which also shows every item's length.
            assert_eq!(
                format!("{:?}", *header.object),
                format!("{:?}", *expected),
                "{name}"
            );
            assert_eq!(
                header.object.meta().transfer_syntax(),
                expected.meta().transfer_syntax()
            );
            assert!(!header.odd_item_length, "{name}");
            let pixels = header
                .pixels
                .expect("walk to the pixel element")
                .map(|pixels| (pixels.kind, pixels.native_length));
            assert_eq!(pixels, expected_pixels, "{name}");
        }
    }

    /// Each way a file can fail keeps its own outcome: no Part 10 preamble,
    /// an unparsable header (including an odd item length), or a data set
    /// that breaks after the header, which fails the inspection.
    #[test]
    fn classifies_files_that_cannot_be_inspected() {
        let directory = tempdir().expect("temp directory");
        let part10 = |name: &str, data_set: &[u8]| {
            let meta = FileMetaTableBuilder::new()
                .transfer_syntax(uids::EXPLICIT_VR_LITTLE_ENDIAN)
                .media_storage_sop_class_uid(uids::SECONDARY_CAPTURE_IMAGE_STORAGE)
                .media_storage_sop_instance_uid("2.25.302")
                .build()
                .expect("file meta");
            let mut bytes = vec![0_u8; 128];
            bytes.extend_from_slice(b"DICM");
            meta.write(&mut bytes).expect("write file meta");
            bytes.extend_from_slice(data_set);
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).expect("write fixture");
            path
        };
        // (0040,0275) SQ, undefined length, holding one 11-byte item: an SH
        // element with an odd, three-byte value.
        let mut odd_item = vec![
            0x40, 0x00, 0x75, 0x02, b'S', b'Q', 0, 0, 0xff, 0xff, 0xff, 0xff,
        ];
        odd_item.extend_from_slice(&[0xfe, 0xff, 0x00, 0xe0, 11, 0, 0, 0]);
        odd_item.extend_from_slice(&[0x08, 0x00, 0x00, 0x01, b'S', b'H', 3, 0, b'A', b'B', b'C']);
        odd_item.extend_from_slice(&[0xfe, 0xff, 0xdd, 0xe0, 0, 0, 0, 0]);
        // (0008,0020) DA declaring 100 bytes of which the file holds two.
        let cut_value = [0x08, 0x00, 0x20, 0x00, b'D', b'A', 100, 0, b'2', b'0'];
        // Trailing padding that declares more bytes than the file holds.
        let truncated_tail = [0xfc, 0xff, 0xfc, 0xff, b'O', b'B', 0, 0, 100, 0, 0, 0, 0, 0];

        let short = directory.path().join("short.dcm");
        std::fs::write(&short, b"DICM").expect("write fixture");
        let skipped = |path: &Path| match build_entry(path).expect("inspect") {
            EntryInspection::Skipped(reason) => reason.code(),
            EntryInspection::Selected(_) => "selected",
        };
        assert_eq!(skipped(&short), "missing_part10_preamble");
        assert_eq!(
            skipped(&part10("cut-value.dcm", &cut_value)),
            "dicom_parse_failed"
        );
        assert_eq!(
            skipped(&part10("odd-item.dcm", &odd_item)),
            "dicom_parse_failed"
        );
        assert!(build_entry(&part10("truncated-tail.dcm", &truncated_tail)).is_err());
    }
}

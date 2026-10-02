//! Graphic annotations of Grayscale and Color Softcopy Presentation States.
//!
//! A presentation state's Graphic Annotation Sequence (PS3.3 C.10.5) holds
//! graphic and text objects to draw on the images it references. This module
//! reads the PIXEL-unit objects and decides which local image frames each
//! annotation item applies to. Nothing else in the state is applied: its
//! window, shutter, displayed area and spatial transformation are ignored,
//! which is why DISPLAY-unit objects (positioned in the displayed area) are
//! counted but not drawn.
//!
//! An item applies to the images of its own Referenced Image Sequence, or,
//! when it has none, to every image of the state's Referenced Series
//! Sequence. A reference without frame numbers covers every frame.

use crate::api::contracts::{
    GraphicAnnotationItemSummary, GraphicAnnotationsResponse, GraphicLayerSummary,
    GraphicObjectSummary, GraphicType, PresentationStateContext, ResolvedSegmentSourceFrame,
    SkippedGraphicObjects, TextJustification, TextObjectSummary,
};
use crate::dicom_values::{read_number, read_numbers, read_string, sequence_items};
use crate::pixels::{cielab_to_srgb8, open_header};
use crate::references::ResolvedReferenceEdge;
use crate::types::FileEntry;
use anyhow::{Context, Result};
use dicom_core::Tag;
use dicom_dictionary_std::{tags, uids};
use dicom_object::InMemDicomObject;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

const MAX_ANNOTATION_ITEMS: usize = 4_096;
const MAX_OBJECTS_PER_ITEM: usize = 4_096;
const MAX_ANNOTATED_FRAMES: usize = 4_096;

/// Whether `sop_class_uid` is a presentation state whose graphic annotations
/// are read here. Blending states compose images and are not.
pub fn has_graphic_annotations(sop_class_uid: &str) -> bool {
    matches!(
        sop_class_uid,
        uids::GRAYSCALE_SOFTCOPY_PRESENTATION_STATE_STORAGE
            | uids::COLOR_SOFTCOPY_PRESENTATION_STATE_STORAGE
    )
}

/// An image reference: every frame when `frame_numbers` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ImageTarget {
    sop_instance_uid: String,
    /// DICOM-declared, one-based.
    frame_numbers: Vec<u32>,
}

impl ImageTarget {
    fn covers(&self, sop_instance_uid: &str, frame_index: u32) -> bool {
        self.sop_instance_uid == sop_instance_uid
            && (self.frame_numbers.is_empty()
                || self
                    .frame_numbers
                    .iter()
                    .any(|number| number.checked_sub(1) == Some(frame_index)))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct AnnotationItem {
    layer: String,
    /// The item's own Referenced Image Sequence; `None` when it has none.
    targets: Option<Vec<ImageTarget>>,
    graphics: Vec<GraphicObjectSummary>,
    texts: Vec<TextObjectSummary>,
    skipped: SkippedGraphicObjects,
}

impl AnnotationItem {
    fn drawable(&self) -> bool {
        !self.graphics.is_empty() || !self.texts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct PresentationState {
    layers: Vec<GraphicLayerSummary>,
    items: Vec<AnnotationItem>,
    /// The images of the Referenced Series Sequence.
    series_targets: Vec<ImageTarget>,
}

impl PresentationState {
    fn read(object: &InMemDicomObject) -> Self {
        let series_targets = sequence_items(object, tags::REFERENCED_SERIES_SEQUENCE)
            .iter()
            .flat_map(image_targets)
            .collect();
        let items = sequence_items(object, tags::GRAPHIC_ANNOTATION_SEQUENCE)
            .iter()
            .take(MAX_ANNOTATION_ITEMS)
            .enumerate()
            .map(|(index, item)| annotation_item(index, item))
            .collect();
        let mut layers = sequence_items(object, tags::GRAPHIC_LAYER_SEQUENCE)
            .iter()
            .filter_map(graphic_layer)
            .collect::<Vec<_>>();
        layers.sort_by_key(|layer| layer.order.unwrap_or(i32::MAX));
        Self {
            layers,
            items,
            series_targets,
        }
    }

    fn targets<'a>(&'a self, item: &'a AnnotationItem) -> &'a [ImageTarget] {
        item.targets.as_deref().unwrap_or(&self.series_targets)
    }

    fn applies(&self, item: &AnnotationItem, sop_instance_uid: &str, frame_index: u32) -> bool {
        self.targets(item)
            .iter()
            .any(|target| target.covers(sop_instance_uid, frame_index))
    }
}

fn image_targets(object: &InMemDicomObject) -> impl Iterator<Item = ImageTarget> + '_ {
    sequence_items(object, tags::REFERENCED_IMAGE_SEQUENCE)
        .iter()
        .filter_map(|reference| {
            Some(ImageTarget {
                sop_instance_uid: read_string(reference, tags::REFERENCED_SOP_INSTANCE_UID)?,
                frame_numbers: read_numbers(reference, tags::REFERENCED_FRAME_NUMBER),
            })
        })
}

fn graphic_layer(layer: &InMemDicomObject) -> Option<GraphicLayerSummary> {
    let cielab = read_numbers::<u16>(layer, tags::GRAPHIC_LAYER_RECOMMENDED_DISPLAY_CIE_LAB_VALUE);
    let grayscale = read_number::<u16>(
        layer,
        tags::GRAPHIC_LAYER_RECOMMENDED_DISPLAY_GRAYSCALE_VALUE,
    );
    let color = match (cielab.as_slice(), grayscale) {
        (&[l, a, b], _) => Some(cielab_to_srgb8([l, a, b])),
        // A P-Value from 0 (black) to FFFFH (white).
        (_, Some(gray)) => Some([(f64::from(gray) * 255.0 / 65_535.0).round() as u8; 3]),
        _ => None,
    };
    Some(GraphicLayerSummary {
        name: read_string(layer, tags::GRAPHIC_LAYER)?,
        order: read_number(layer, tags::GRAPHIC_LAYER_ORDER),
        description: read_string(layer, tags::GRAPHIC_LAYER_DESCRIPTION),
        color,
    })
}

/// Why an object is not drawn.
enum Skip {
    DisplayUnits,
    MatrixUnits,
    Malformed,
}

impl SkippedGraphicObjects {
    fn count(&mut self, skip: Skip) {
        match skip {
            Skip::DisplayUnits => self.display_units += 1,
            Skip::MatrixUnits => self.matrix_units += 1,
            Skip::Malformed => self.malformed += 1,
        }
    }

    fn add(&mut self, other: Self) {
        self.display_units += other.display_units;
        self.matrix_units += other.matrix_units;
        self.malformed += other.malformed;
        self.masked_text += other.masked_text;
    }
}

fn annotation_item(index: usize, item: &InMemDicomObject) -> AnnotationItem {
    let layer = read_string(item, tags::GRAPHIC_LAYER).unwrap_or_default();
    let mut skipped = SkippedGraphicObjects::default();
    let mut graphics = Vec::new();
    for object in sequence_items(item, tags::GRAPHIC_OBJECT_SEQUENCE)
        .iter()
        .take(MAX_OBJECTS_PER_ITEM)
    {
        match graphic_object(index, &layer, object) {
            Ok(graphic) => graphics.push(graphic),
            Err(skip) => skipped.count(skip),
        }
    }
    let mut texts = Vec::new();
    for object in sequence_items(item, tags::TEXT_OBJECT_SEQUENCE)
        .iter()
        .take(MAX_OBJECTS_PER_ITEM)
    {
        match text_object(index, &layer, object) {
            Ok(text) => texts.push(text),
            Err(skip) => skipped.count(skip),
        }
    }
    AnnotationItem {
        layer,
        targets: item
            .get(tags::REFERENCED_IMAGE_SEQUENCE)
            .map(|_| image_targets(item).collect()),
        graphics,
        texts,
        skipped,
    }
}

/// PIXEL units, or the reason an object in other units is skipped.
fn pixel_units(object: &InMemDicomObject, tag: Tag) -> Result<(), Skip> {
    match read_string(object, tag).as_deref() {
        Some("PIXEL") => Ok(()),
        Some("DISPLAY") => Err(Skip::DisplayUnits),
        Some("MATRIX") => Err(Skip::MatrixUnits),
        _ => Err(Skip::Malformed),
    }
}

/// `[column, row]` points; `None` unless every value is finite and paired.
fn points(object: &InMemDicomObject, tag: Tag) -> Option<Vec<[f64; 2]>> {
    let values = read_numbers::<f64>(object, tag);
    let declared = object.get(tag)?.to_str().ok()?.split('\\').count();
    (values.len() == declared
        && values.len().is_multiple_of(2)
        && values.iter().all(|value| value.is_finite()))
    .then(|| {
        values
            .chunks_exact(2)
            .map(|pair| [pair[0], pair[1]])
            .collect()
    })
}

fn point(object: &InMemDicomObject, tag: Tag) -> Option<[f64; 2]> {
    match points(object, tag)?.as_slice() {
        &[point] => Some(point),
        _ => None,
    }
}

fn graphic_object(
    item: usize,
    layer: &str,
    object: &InMemDicomObject,
) -> Result<GraphicObjectSummary, Skip> {
    pixel_units(object, tags::GRAPHIC_ANNOTATION_UNITS)?;
    let points = points(object, tags::GRAPHIC_DATA).ok_or(Skip::Malformed)?;
    let (graphic_type, valid) = match read_string(object, tags::GRAPHIC_TYPE).as_deref() {
        Some("POINT") => (GraphicType::Point, points.len() == 1),
        Some("POLYLINE") => (GraphicType::Polyline, points.len() >= 2),
        Some("INTERPOLATED") => (GraphicType::Interpolated, points.len() >= 2),
        Some("CIRCLE") => (GraphicType::Circle, points.len() == 2),
        Some("ELLIPSE") => (GraphicType::Ellipse, points.len() == 4),
        _ => return Err(Skip::Malformed),
    };
    if !valid {
        return Err(Skip::Malformed);
    }
    Ok(GraphicObjectSummary {
        item,
        layer: layer.to_string(),
        graphic_type,
        points,
        filled: read_string(object, tags::GRAPHIC_FILLED).as_deref() == Some("Y"),
    })
}

fn text_object(
    item: usize,
    layer: &str,
    object: &InMemDicomObject,
) -> Result<TextObjectSummary, Skip> {
    let has_box = object
        .get(tags::BOUNDING_BOX_TOP_LEFT_HAND_CORNER)
        .is_some();
    let has_anchor = object.get(tags::ANCHOR_POINT).is_some();
    if has_box {
        pixel_units(object, tags::BOUNDING_BOX_ANNOTATION_UNITS)?;
    }
    if has_anchor {
        pixel_units(object, tags::ANCHOR_POINT_ANNOTATION_UNITS)?;
    }
    // The corners follow the text's reading direction, which is not
    // reproduced: the box is reported by its extent.
    let bounding_box = if has_box {
        let [x0, y0] =
            point(object, tags::BOUNDING_BOX_TOP_LEFT_HAND_CORNER).ok_or(Skip::Malformed)?;
        let [x1, y1] =
            point(object, tags::BOUNDING_BOX_BOTTOM_RIGHT_HAND_CORNER).ok_or(Skip::Malformed)?;
        Some([x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)])
    } else {
        None
    };
    let anchor = if has_anchor {
        Some(point(object, tags::ANCHOR_POINT).ok_or(Skip::Malformed)?)
    } else {
        None
    };
    let text = object
        .get(tags::UNFORMATTED_TEXT_VALUE)
        .and_then(|element| element.to_str().ok())
        .map(|text| text.replace("\r\n", "\n").trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or(Skip::Malformed)?;
    if bounding_box.is_none() && anchor.is_none() {
        return Err(Skip::Malformed);
    }
    let justification =
        match read_string(object, tags::BOUNDING_BOX_TEXT_HORIZONTAL_JUSTIFICATION).as_deref() {
            Some("LEFT") => Some(TextJustification::Left),
            Some("CENTER") => Some(TextJustification::Center),
            Some("RIGHT") => Some(TextJustification::Right),
            _ => None,
        };
    Ok(TextObjectSummary {
        item,
        layer: layer.to_string(),
        text,
        bounding_box,
        justification: bounding_box.and(justification),
        anchor,
        anchor_visible: read_string(object, tags::ANCHOR_POINT_VISIBILITY).as_deref() == Some("Y"),
    })
}

/// Local image files by SOP Instance UID.
type ImagesByUid<'a> = HashMap<&'a str, Vec<&'a FileEntry>>;

/// The local image frames `item` applies to, in file and frame order. Walks
/// the item's references rather than every loaded frame, so a large scan
/// costs no more than the state references.
fn item_frames(
    state: &PresentationState,
    item: &AnnotationItem,
    images: &ImagesByUid<'_>,
) -> BTreeSet<(usize, u32)> {
    let mut frames = BTreeSet::new();
    for target in state.targets(item) {
        for file in images
            .get(target.sop_instance_uid.as_str())
            .into_iter()
            .flatten()
        {
            if target.frame_numbers.is_empty() {
                frames.extend((0..file.frame_count).map(|frame| (file.index, frame)));
            } else {
                frames.extend(
                    target
                        .frame_numbers
                        .iter()
                        .filter_map(|number| number.checked_sub(1))
                        .filter(|frame| *frame < file.frame_count)
                        .map(|frame| (file.index, frame)),
                );
            }
        }
    }
    frames
}

/// The semantic context of a softcopy presentation state.
pub fn presentation_state_context(
    object: &InMemDicomObject,
    files: &[Arc<FileEntry>],
    resolved: &[ResolvedReferenceEdge],
) -> PresentationStateContext {
    let state = PresentationState::read(object);
    let mut images = ImagesByUid::new();
    for file in files.iter().filter(|file| file.has_pixels) {
        images
            .entry(file.sop_instance_uid.as_str())
            .or_default()
            .push(file);
    }
    let sop_instance_uids = files
        .iter()
        .map(|file| (file.index, file.sop_instance_uid.as_str()))
        .collect::<HashMap<_, _>>();
    let frame = |(file_index, frame_index): (usize, u32)| ResolvedSegmentSourceFrame {
        file_index,
        frame_index,
        sop_instance_uid: sop_instance_uids
            .get(&file_index)
            .map(|uid| uid.to_string())
            .unwrap_or_default(),
    };
    let mut annotated = BTreeSet::new();
    let mut skipped = SkippedGraphicObjects::default();
    let items = state
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            skipped.add(item.skipped);
            let frames = item_frames(&state, item, &images);
            let summary = GraphicAnnotationItemSummary {
                index,
                layer: item.layer.clone(),
                graphic_types: item
                    .graphics
                    .iter()
                    .map(|graphic| graphic.graphic_type)
                    .collect(),
                texts: item.texts.iter().map(|text| text.text.clone()).collect(),
                scoped: item.targets.is_some(),
                first_frame: frames.first().copied().map(frame),
                frame_count: frames.len(),
            };
            if item.drawable() {
                annotated.extend(frames);
            }
            summary
        })
        .collect();
    PresentationStateContext {
        content_label: read_string(object, tags::CONTENT_LABEL),
        content_description: read_string(object, tags::CONTENT_DESCRIPTION),
        content_creator_name: read_string(object, tags::CONTENT_CREATOR_NAME),
        presentation_creation_date: read_string(object, tags::PRESENTATION_CREATION_DATE),
        layers: state.layers,
        items,
        annotated_frames: annotated
            .into_iter()
            .take(MAX_ANNOTATED_FRAMES)
            .map(frame)
            .collect(),
        skipped,
        references: resolved
            .iter()
            .map(ResolvedReferenceEdge::summary)
            .collect(),
    }
}

/// The annotations `state_file` draws on one frame of `target`; empty when
/// none of its items applies to that frame.
pub fn graphic_annotations(
    state_file: &FileEntry,
    target: &FileEntry,
    frame_index: u32,
) -> Result<GraphicAnnotationsResponse> {
    let object = open_header(&state_file.path).context("failed to open presentation state")?;
    let state = PresentationState::read(&object);
    let mut response = GraphicAnnotationsResponse {
        layers: Vec::new(),
        graphics: Vec::new(),
        texts: Vec::new(),
        skipped: SkippedGraphicObjects::default(),
    };
    for item in state
        .items
        .iter()
        .filter(|item| state.applies(item, &target.sop_instance_uid, frame_index))
    {
        response.graphics.extend(item.graphics.iter().cloned());
        response.texts.extend(item.texts.iter().cloned());
        response.skipped.add(item.skipped);
    }
    response.layers = state.layers;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dicom_core::value::DataSetSequence;
    use dicom_core::{DataElement, PrimitiveValue, VR};

    fn sequence(tag: Tag, items: Vec<InMemDicomObject>) -> DataElement<InMemDicomObject> {
        DataElement::new(tag, VR::SQ, DataSetSequence::from(items))
    }

    fn reference(uid: &str, frames: Option<&str>) -> InMemDicomObject {
        let mut item = InMemDicomObject::from_element_iter([DataElement::new(
            tags::REFERENCED_SOP_INSTANCE_UID,
            VR::UI,
            uid,
        )]);
        if let Some(frames) = frames {
            item.put(DataElement::new(
                tags::REFERENCED_FRAME_NUMBER,
                VR::IS,
                frames,
            ));
        }
        item
    }

    fn graphic(graphic_type: &str, units: &str, data: &[f32]) -> InMemDicomObject {
        InMemDicomObject::from_element_iter([
            DataElement::new(tags::GRAPHIC_ANNOTATION_UNITS, VR::CS, units),
            DataElement::new(
                tags::GRAPHIC_DATA,
                VR::FL,
                PrimitiveValue::F32(data.to_vec().into()),
            ),
            DataElement::new(tags::GRAPHIC_TYPE, VR::CS, graphic_type),
        ])
    }

    fn item(
        references: Option<Vec<InMemDicomObject>>,
        graphics: Vec<InMemDicomObject>,
    ) -> InMemDicomObject {
        let mut item = InMemDicomObject::from_element_iter([
            DataElement::new(tags::GRAPHIC_LAYER, VR::CS, "LAYER"),
            sequence(tags::GRAPHIC_OBJECT_SEQUENCE, graphics),
        ]);
        if let Some(references) = references {
            item.put(sequence(tags::REFERENCED_IMAGE_SEQUENCE, references));
        }
        item
    }

    fn state(items: Vec<InMemDicomObject>) -> PresentationState {
        let series = InMemDicomObject::from_element_iter([sequence(
            tags::REFERENCED_IMAGE_SEQUENCE,
            vec![reference("1.2.a", None), reference("1.2.b", Some("2"))],
        )]);
        PresentationState::read(&InMemDicomObject::from_element_iter([
            sequence(tags::REFERENCED_SERIES_SEQUENCE, vec![series]),
            sequence(tags::GRAPHIC_ANNOTATION_SEQUENCE, items),
        ]))
    }

    #[test]
    fn scoped_item_applies_to_its_own_images_and_frames_only() {
        let circle = || graphic("CIRCLE", "PIXEL", &[1.0, 2.0, 3.0, 2.0]);
        let state = state(vec![
            item(Some(vec![reference("1.2.a", Some("1\\3"))]), vec![circle()]),
            item(Some(vec![reference("1.2.c", None)]), vec![circle()]),
        ]);
        let applies =
            |item: usize, uid: &str, frame: u32| state.applies(&state.items[item], uid, frame);

        assert!(applies(0, "1.2.a", 0));
        assert!(!applies(0, "1.2.a", 1));
        assert!(applies(0, "1.2.a", 2));
        assert!(!applies(0, "1.2.b", 0));
        // An image missing from the Referenced Series Sequence is honored.
        assert!(applies(1, "1.2.c", 7));
    }

    #[test]
    fn unscoped_item_applies_to_the_referenced_series_images() {
        let state = state(vec![item(
            None,
            vec![graphic("POINT", "PIXEL", &[1.5, 2.5])],
        )]);
        let applies = |uid: &str, frame: u32| state.applies(&state.items[0], uid, frame);

        assert!(applies("1.2.a", 0));
        assert!(applies("1.2.a", 9));
        assert!(applies("1.2.b", 1));
        assert!(!applies("1.2.b", 0));
        assert!(!applies("1.2.c", 0));
    }

    #[test]
    fn an_empty_referenced_image_sequence_applies_to_nothing() {
        let state = state(vec![item(
            Some(Vec::new()),
            vec![graphic("POINT", "PIXEL", &[1.5, 2.5])],
        )]);

        assert!(!state.applies(&state.items[0], "1.2.a", 0));
    }

    #[test]
    fn graphics_keep_their_points_and_count_what_is_skipped() {
        let state = state(vec![item(
            None,
            vec![
                graphic(
                    "ELLIPSE",
                    "PIXEL",
                    &[0.0, 1.0, 4.0, 1.0, 2.0, 0.5, 2.0, 1.5],
                ),
                graphic("POLYLINE", "PIXEL", &[0.0, 0.0, 2.0, 0.0, 0.0, 0.0]),
                graphic(
                    "ELLIPSE",
                    "DISPLAY",
                    &[0.1, 0.5, 0.9, 0.5, 0.5, 0.2, 0.5, 0.8],
                ),
                graphic("CIRCLE", "MATRIX", &[1.0, 1.0, 2.0, 1.0]),
                // A circle needs exactly two points; an odd value count has no pairs.
                graphic("CIRCLE", "PIXEL", &[1.0, 1.0]),
                graphic("POLYLINE", "PIXEL", &[1.0, 1.0, 2.0]),
                graphic("POINT", "PIXEL", &[f32::NAN, 1.0]),
                graphic("SQUARE", "PIXEL", &[1.0, 1.0]),
            ],
        )]);
        let item = &state.items[0];

        assert_eq!(
            item.graphics
                .iter()
                .map(|graphic| graphic.graphic_type)
                .collect::<Vec<_>>(),
            [GraphicType::Ellipse, GraphicType::Polyline]
        );
        assert_eq!(
            item.graphics[0].points,
            [[0.0, 1.0], [4.0, 1.0], [2.0, 0.5], [2.0, 1.5]]
        );
        assert_eq!(
            item.skipped,
            SkippedGraphicObjects {
                display_units: 1,
                matrix_units: 1,
                malformed: 4,
                masked_text: 0,
            }
        );
    }

    #[test]
    fn text_needs_a_pixel_unit_box_or_anchor() {
        let text = |elements: Vec<DataElement<InMemDicomObject>>| {
            let mut object = InMemDicomObject::from_element_iter([DataElement::new(
                tags::UNFORMATTED_TEXT_VALUE,
                VR::ST,
                "First\r\nSecond",
            )]);
            for element in elements {
                object.put(element);
            }
            text_object(3, "LAYER", &object).ok()
        };
        let floats = |tag, values: [f32; 2]| {
            DataElement::new(tag, VR::FL, PrimitiveValue::F32(values.to_vec().into()))
        };
        let units = |tag, value: &str| DataElement::new(tag, VR::CS, value);

        // Corners given bottom-right first are reported by extent.
        let boxed = text(vec![
            units(tags::BOUNDING_BOX_ANNOTATION_UNITS, "PIXEL"),
            floats(tags::BOUNDING_BOX_TOP_LEFT_HAND_CORNER, [9.0, 8.0]),
            floats(tags::BOUNDING_BOX_BOTTOM_RIGHT_HAND_CORNER, [1.0, 2.0]),
            units(tags::BOUNDING_BOX_TEXT_HORIZONTAL_JUSTIFICATION, "RIGHT"),
        ])
        .expect("boxed text");
        assert_eq!(boxed.text, "First\nSecond");
        assert_eq!(boxed.bounding_box, Some([1.0, 2.0, 9.0, 8.0]));
        assert_eq!(boxed.justification, Some(TextJustification::Right));
        assert_eq!((boxed.anchor, boxed.anchor_visible), (None, false));

        let anchored = text(vec![
            units(tags::ANCHOR_POINT_ANNOTATION_UNITS, "PIXEL"),
            floats(tags::ANCHOR_POINT, [4.5, 6.5]),
            units(tags::ANCHOR_POINT_VISIBILITY, "Y"),
        ])
        .expect("anchored text");
        assert_eq!(anchored.anchor, Some([4.5, 6.5]));
        assert!(anchored.anchor_visible);
        assert_eq!(anchored.bounding_box, None);

        assert!(text(Vec::new()).is_none());
        assert!(text(vec![
            units(tags::ANCHOR_POINT_ANNOTATION_UNITS, "DISPLAY"),
            floats(tags::ANCHOR_POINT, [0.5, 0.5]),
        ])
        .is_none());
    }

    #[test]
    fn layers_are_ordered_and_colored_from_their_recommended_values() {
        let layer = |name: &str, order: &str| {
            InMemDicomObject::from_element_iter([
                DataElement::new(tags::GRAPHIC_LAYER, VR::CS, name),
                DataElement::new(tags::GRAPHIC_LAYER_ORDER, VR::IS, order),
            ])
        };
        let mut gray = layer("GRAY", "2");
        gray.put(DataElement::new(
            tags::GRAPHIC_LAYER_RECOMMENDED_DISPLAY_GRAYSCALE_VALUE,
            VR::US,
            PrimitiveValue::from(0xFFFF_u16),
        ));
        let mut lab = layer("LAB", "1");
        // L* 100, a* 0, b* 0: white.
        lab.put(DataElement::new(
            tags::GRAPHIC_LAYER_RECOMMENDED_DISPLAY_CIE_LAB_VALUE,
            VR::US,
            PrimitiveValue::U16(vec![65_535, 32_896, 32_896].into()),
        ));
        let state = PresentationState::read(&InMemDicomObject::from_element_iter([sequence(
            tags::GRAPHIC_LAYER_SEQUENCE,
            vec![layer("PLAIN", "3"), gray, lab],
        )]));

        assert_eq!(
            state
                .layers
                .iter()
                .map(|layer| (layer.name.as_str(), layer.color))
                .collect::<Vec<_>>(),
            [
                ("LAB", Some([255, 255, 255])),
                ("GRAY", Some([255, 255, 255])),
                ("PLAIN", None),
            ]
        );
    }
}

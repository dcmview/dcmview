//! The EMBED compatibility view: ROI rows as rectangle records and back.
//!
//! The EMBED endpoints (`GET` and `PUT /api/file/{index}/annotations`,
//! `GET /api/annotations/export.csv`) and the `--annotations` CSV keep the
//! shape and the bytes they had before the store held records
//! (`docs/design/annotation-model.md` 9.1, 9.2). This module is the mapping
//! between the two, as pure functions: it holds no state and touches neither
//! the store nor the registry. `server::annotations` puts them together.
//!
//! What a box is as a rectangle, and what rectangles are as boxes, is the
//! EMBED adapter's rule (`dcmview_adapters::embed`), so that the viewer's
//! view and every other reader and writer of the format agree; it is
//! repeated here because the view depends on every word of it. This module
//! adds the operations.
//!
//! # A ROI as a record
//!
//! - `[ymin, xmin, ymax, xmax]` is the rectangle `x0 = xmin`, `y0 = ymin`,
//!   `x1 = xmax`, `y1 = ymax`, each number exactly as written: a box past
//!   the image edge, with no area, or with its corners out of order is the
//!   record it reads as, and nothing is clamped, reordered or rounded.
//! - The record's class is `IMPLICIT_CLASS_ID` (`roi`) and its layer the
//!   store's default layer.
//! - A row whose `roi_frames` is empty gives every ROI of the row
//!   `FrameScope::All`. Otherwise ROI `i` has
//!   `FrameScope::from_written(roi_frames[i])`: the list as written, order
//!   and repeats included, and an empty list for one ROI stays an empty
//!   set.
//!
//! # Records as ROIs
//!
//! [`rois_of`] is the inverse, and the one place that decides what the
//! EMBED endpoints and the CSV show for a list of records:
//!
//! - `num_roi` is the number of records, and `roi_coords[i]` is record
//!   `i`'s rectangle as `[y0, x0, y1, x1]`. A coordinate that is not a whole
//!   number is rounded outward, the first two down and the last two up
//!   (`docs/design/annotation-model.md` 9.3), and each is then brought into
//!   `0..=u32::MAX`.
//! - `roi_frames` is empty when every record is `FrameScope::All`.
//!   Otherwise it has one list per record: `FrameScope::written` for a set,
//!   and every frame index of the file, ascending, for a record that is
//!   `All` (`docs/design/output-adapters.md` 6.2).
//!
//! Rows the mapping read come back from it unchanged, which is what keeps
//! the export byte for byte what it was.
//!
//! # The order of a view
//!
//! A file's view lists the rows the `--annotations` import read for that
//! file first, in the CSV's order, and then every other record in creation
//! order (`MemoryBackend::embed_records`). The import's rows may become
//! records long after other records were made, for example when a
//! rectangle is drawn on a byte-identical copy of the file; they still
//! stand first, because the CSV was read before anything was drawn.

use crate::api::contracts::EmbedRoiAnnotations;
use dcmview_adapters::embed::{annotation_of, rectangle, scope_at};
use dcmview_annotation::{Annotation, Author, FileKey, FrameScope, LayerId, Op, Patch, Timestamp};

/// What the EMBED endpoints show for `records`, the view of one file in
/// view order, by the rules in the module documentation. `frame_count` is
/// the file's.
pub(crate) fn rois_of<'a>(
    records: impl IntoIterator<Item = &'a Annotation>,
    frame_count: u32,
) -> EmbedRoiAnnotations {
    let rois = dcmview_adapters::embed::rois_of(records, frame_count);
    EmbedRoiAnnotations {
        num_roi: rois.roi_coords.len(),
        roi_coords: rois.roi_coords,
        roi_frames: rois.roi_frames,
    }
}

/// One `CreateAnnotation` for each ROI of `rows`, in their order, by "A ROI
/// as a record", each under a new id (`dcmview_annotation::new_id`). `author` and `stamp`
/// fill the record's metadata, which the store then writes itself.
///
/// `rows` is taken as it is: `num_roi` is not read, and a ROI is made for
/// each entry of `roi_coords`.
pub(crate) fn rows_as_creates(
    rows: &EmbedRoiAnnotations,
    file: &FileKey,
    layer: &LayerId,
    author: &Author,
    stamp: &Timestamp,
) -> Vec<Op> {
    rows.roi_coords
        .iter()
        .enumerate()
        .map(|(i, coords)| {
            create_roi(
                *coords,
                scope_at(&rows.roi_frames, i),
                file,
                layer,
                author,
                stamp,
            )
        })
        .collect()
}

/// The operations that make a file's EMBED view show `wanted`, given the
/// records it shows now (`current`, in view order). Empty when the view
/// already shows it.
///
/// ROIs are matched to records by position, since the EMBED shape has no
/// ids. With `shown = rois_of(current, frame_count)`:
///
/// 1. For each position both have, in order: nothing when
///    `shown.roi_coords[i] == wanted.roi_coords[i]` and the frame scope
///    `wanted` gives ROI `i` ("A ROI as a record") is the record's own;
///    otherwise one `UpdateAnnotation` of that record, with its `file`,
///    its `rev` as `base_rev`, and `before` and `after` holding `geometry`
///    when the coordinates differ and `frames` when the scopes do.
///
///    The scopes are compared, not the lists shown, so that the view after
///    the save shows exactly `wanted`: a record for every frame that the
///    view listed frame by frame, because another record of the view named
///    some, becomes a record for those frames when the save names them.
/// 2. Then, for each record past the end of `wanted`, in order: one
///    `DeleteAnnotation` with its `rev` and the record as `snapshot`.
/// 3. Then, for each ROI past the end of `current`, in order: one
///    `CreateAnnotation` as [`rows_as_creates`] makes it.
///
/// A record that is not touched keeps its id and its `rev`, so a client
/// that holds it by id is not disturbed by a save of the ROI beside it.
pub(crate) fn replacement_ops(
    current: &[Annotation],
    wanted: &EmbedRoiAnnotations,
    frame_count: u32,
    layer: &LayerId,
    author: &Author,
    stamp: &Timestamp,
    file: &FileKey,
) -> Vec<Op> {
    let shown = rois_of(current, frame_count);
    let mut ops = Vec::new();
    for (i, (record, coords)) in current.iter().zip(&wanted.roi_coords).enumerate() {
        let mut before = Patch::default();
        let mut after = Patch::default();
        if shown.roi_coords.get(i) != Some(coords) {
            before.geometry = Some(record.geometry.clone());
            after.geometry = Some(rectangle(*coords));
        }
        let frames = scope_at(&wanted.roi_frames, i);
        if frames != record.frames {
            before.frames = Some(record.frames.clone());
            after.frames = Some(frames);
        }
        if after.geometry.is_some() || after.frames.is_some() {
            ops.push(Op::UpdateAnnotation {
                id: record.id,
                file: record.file.clone(),
                base_rev: record.meta.rev,
                before: Box::new(before),
                after: Box::new(after),
            });
        }
    }
    ops.extend(
        current
            .iter()
            .skip(wanted.roi_coords.len())
            .map(|record| Op::DeleteAnnotation {
                id: record.id,
                base_rev: record.meta.rev,
                snapshot: Box::new(record.clone()),
            }),
    );
    ops.extend(
        wanted
            .roi_coords
            .iter()
            .enumerate()
            .skip(current.len())
            .map(|(i, coords)| {
                create_roi(
                    *coords,
                    scope_at(&wanted.roi_frames, i),
                    file,
                    layer,
                    author,
                    stamp,
                )
            }),
    );
    ops
}

fn create_roi(
    coords: [u32; 4],
    frames: FrameScope,
    file: &FileKey,
    layer: &LayerId,
    author: &Author,
    stamp: &Timestamp,
) -> Op {
    Op::CreateAnnotation {
        annotation: Box::new(annotation_of(coords, frames, file, layer, author, stamp)),
    }
}

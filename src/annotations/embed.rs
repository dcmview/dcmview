//! The EMBED compatibility view: ROI rows as rectangle records and back.
//!
//! The EMBED endpoints (`GET` and `PUT /api/file/{index}/annotations`,
//! `GET /api/annotations/export.csv`) and the `--annotations` CSV keep the
//! shape and the bytes they had before the store held records
//! (`docs/design/annotation-model.md` 9.1, 9.2). This module is the mapping
//! between the two, as pure functions: it holds no state and touches neither
//! the store nor the registry. `server::annotations` puts them together.
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

#![expect(dead_code, reason = "nothing calls into the store yet")]

use crate::api::contracts::EmbedRoiAnnotations;
use anyhow::Result;
use dcmview_annotation::{Annotation, Author, FileKey, LayerId, Op, Timestamp};

/// What the EMBED endpoints show for `records`, the view of one file in
/// view order, by the rules in the module documentation. `frame_count` is
/// the file's.
pub(crate) fn rois_of(records: &[Annotation], frame_count: u32) -> EmbedRoiAnnotations {
    let _ = (records, frame_count);
    todo!("records as EMBED ROIs")
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
    let _ = (rows, file, layer, author, stamp);
    todo!("EMBED ROIs as create operations")
}

/// The operations that make a file's EMBED view show `wanted`, given the
/// records it shows now (`current`, in view order). Empty when the view
/// already shows it.
///
/// ROIs are matched to records by position, since the EMBED shape has no
/// ids. With `shown = rois_of(current, frame_count)`:
///
/// 1. For each position both have, in order: nothing when
///    `shown.roi_coords[i] == wanted.roi_coords[i]` and the frames agree;
///    otherwise one `UpdateAnnotation` of that record, with its `file`,
///    its `rev` as `base_rev`, and `before` and `after` holding `geometry`
///    when the coordinates differ and `frames` when the frames do. The
///    frames agree when both lists are empty, or both hold a list for `i`
///    and the lists are equal, or the scope `wanted` gives ROI `i` ("A ROI
///    as a record") is the record's own.
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
    let _ = (current, wanted, frame_count, layer, author, stamp, file);
    todo!("the operations a replaced ROI list amounts to")
}

/// The EMBED CSV of `rows`, each a path and that file's ROIs, in the order
/// given: the header `anon_dicom_path,num_ROI,ROI_coords,ROI_frames`, then
/// one record per row with `num_roi` in decimal and `roi_coords` and
/// `roi_frames` as compact JSON (`serde_json::to_string`), written by a
/// `csv::Writer` with its default settings (a field is quoted only when it
/// needs to be, and records end with a line feed). A row is written as
/// given, one with no ROI included; the caller leaves those out.
pub(crate) fn write_embed_csv(rows: &[(String, EmbedRoiAnnotations)]) -> Result<String> {
    let _ = rows;
    todo!("the EMBED CSV of some rows")
}

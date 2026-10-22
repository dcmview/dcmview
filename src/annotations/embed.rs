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
use std::collections::HashMap;

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
/// The EMBED shape has no ids, so each ROI of `wanted` is given a record of
/// `current` to become, or none, by [`pairing`]: a record keeps its id,
/// class, layer, attributes and history when its ROI is still in the list,
/// wherever the ROIs around it went. With `shown = rois_of(current,
/// frame_count)`:
///
/// 1. For each record given a ROI `j`, in view order: nothing when
///    `shown.roi_coords[i] == wanted.roi_coords[j]` and the frame scope
///    `wanted` gives ROI `j` ("A ROI as a record") is the record's own;
///    otherwise one `UpdateAnnotation` of that record, with its `file`,
///    its `rev` as `base_rev`, and `before` and `after` holding `geometry`
///    when the coordinates differ and `frames` when the scopes do.
///
///    The scopes are compared, not the lists shown, so that the view after
///    the save shows exactly `wanted`: a record for every frame that the
///    view listed frame by frame, because another record of the view named
///    some, becomes a record for those frames when the save names them.
/// 2. Then, for each record given no ROI, in view order: one
///    `DeleteAnnotation` with its `rev` and the record as `snapshot`.
/// 3. Then, for each ROI given no record, in order: one `CreateAnnotation`
///    as [`rows_as_creates`] makes it. These are always the last ROIs of
///    `wanted`, and a new record stands last in the view, so the view after
///    the save lists the ROIs in the order `wanted` has them.
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
    let scopes: Vec<FrameScope> = (0..wanted.roi_coords.len())
        .map(|j| scope_at(&wanted.roi_frames, j))
        .collect();
    // Each ROI as a number that two ROIs share when they are the same
    // rectangle over the same frames, so the pairing compares numbers.
    let mut shapes = HashMap::new();
    let mut shape = |coords: [u32; 4], frames| {
        let next = shapes.len();
        *shapes
            .entry((edges(coords), covered(frames, frame_count)))
            .or_insert(next)
    };
    let had: Vec<usize> = shown
        .roi_coords
        .iter()
        .zip(current)
        .map(|(coords, record)| shape(*coords, &record.frames))
        .collect();
    let has: Vec<usize> = wanted
        .roi_coords
        .iter()
        .zip(&scopes)
        .map(|(coords, frames)| shape(*coords, frames))
        .collect();
    let given = pairing(had.len(), has.len(), |i, j| had[i] == has[j]);
    let mut ops = Vec::new();
    let mut deletes = Vec::new();
    let mut kept = vec![false; wanted.roi_coords.len()];
    for (i, (record, roi)) in current.iter().zip(&given).enumerate() {
        let Some(j) = *roi else {
            deletes.push(Op::DeleteAnnotation {
                id: record.id,
                base_rev: record.meta.rev,
                snapshot: Box::new(record.clone()),
            });
            continue;
        };
        kept[j] = true;
        let coords = wanted.roi_coords[j];
        let mut before = Patch::default();
        let mut after = Patch::default();
        if shown.roi_coords[i] != coords {
            before.geometry = Some(record.geometry.clone());
            after.geometry = Some(rectangle(coords));
        }
        if scopes[j] != record.frames {
            before.frames = Some(record.frames.clone());
            after.frames = Some(scopes[j].clone());
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
    ops.append(&mut deletes);
    ops.extend(
        wanted
            .roi_coords
            .iter()
            .zip(scopes)
            .zip(kept)
            .filter(|(_, kept)| !kept)
            .map(|((coords, frames), _)| create_roi(*coords, frames, file, layer, author, stamp)),
    );
    ops
}

/// A box's edges in order, whichever corner it names first: a box the
/// import read with its corners out of order is the rectangle a client
/// sends back in order.
fn edges([y0, x0, y1, x1]: [u32; 4]) -> [u32; 4] {
    [y0.min(y1), x0.min(x1), y0.max(y1), x0.max(x1)]
}

/// The frames a scope covers in a file of `frame_count` frames, `None` for
/// every frame: `All`, or a set of each frame. A list as written covers the
/// set it names.
fn covered(frames: &FrameScope, frame_count: u32) -> Option<&[u32]> {
    match frames {
        FrameScope::All => None,
        FrameScope::Set(frames) => {
            let every = frames.set.len() == frame_count as usize
                && frames.set.iter().zip(0..).all(|(&frame, n)| frame == n);
            (!every).then_some(frames.set.as_slice())
        }
    }
}

/// The most cells of the table [`pairing`] fills, about a thousand records
/// against a thousand ROIs.
const MAX_PAIRING_CELLS: usize = 1 << 20;

/// Which ROI of `wanted` (`m` of them) each record of `current` (`n`, in
/// view order) becomes, or `None` for a record to delete. A ROI no record
/// becomes is created. `same(i, j)` says whether record `i` already is ROI
/// `j`: the same rectangle over the same frames.
///
/// The pairing keeps the order of both lists (record `i` before record `k`
/// become ROIs `j` before `l`), and the ROIs no record becomes are the last
/// of `wanted`, since a record made now stands last in the view. Of the
/// pairings that do, it is the one that:
///
/// 1. pairs the most records with a ROI they already are, so that a record
///    whose ROI is still in the list keeps it; then
/// 2. pairs the most of the remaining records and ROIs with each other, a
///    record becoming a ROI it is not (a move, a resize, a change of
///    frames) rather than one being deleted and another created; then
/// 3. of what is left equal, pairs earlier records first: at the first
///    record where two such pairings differ, the one that pairs it.
///
/// A save after one edit in the viewer therefore keeps every other record:
/// a deleted ROI's record is deleted, a moved, resized or rescoped ROI's
/// record is updated, and a drawn ROI, which the viewer appends, is
/// created. Two records that are one rectangle over the same frames are
/// one ROI to a save: when one of them goes, which is kept follows from
/// the order alone, not from which the client meant.
///
/// The run at the start of both lists whose records already are their
/// ROIs is paired first, without a table, which gives the same pairing.
/// The rest is paired through a table of its records by its ROIs, which
/// holds the rules exactly. When that table would have more than
/// [`MAX_PAIRING_CELLS`] cells, the pairing is no longer the best one but
/// still keeps both orders: the run at the end of both lists whose records
/// already are their ROIs is paired too, when `wanted` is not longer, and
/// the part in between is paired through a table in which no ROI is
/// created, or, when that too is larger, by position, its records past the
/// end of its ROIs being deleted. So a save that changes one place of a
/// long list still keeps every other record.
fn pairing(n: usize, m: usize, same: impl Fn(usize, usize) -> bool) -> Vec<Option<usize>> {
    let mut given = vec![None; n];
    let start = (0..n.min(m)).take_while(|&k| same(k, k)).count();
    for (k, roi) in given.iter_mut().enumerate().take(start) {
        *roi = Some(k);
    }
    let fits =
        |rows: usize, columns: usize| (rows + 1).saturating_mul(columns + 1) <= MAX_PAIRING_CELLS;
    let (rows, columns) = (n - start, m - start);
    if fits(rows, columns) {
        through_table(&mut given, start, rows, columns, true, &same);
        return given;
    }
    let end = if m <= n {
        (0..m - start)
            .take_while(|&k| same(n - 1 - k, m - 1 - k))
            .count()
    } else {
        0
    };
    for (k, roi) in given.iter_mut().rev().enumerate().take(end) {
        *roi = Some(m - 1 - k);
    }
    let (rows, columns) = (rows - end, columns - end);
    if fits(rows, columns) {
        // A ROI created here would stand after the run at the end.
        through_table(&mut given, start, rows, columns, end == 0, &same);
    } else {
        let middle = start..start + rows.min(columns);
        for (roi, k) in given[middle.clone()].iter_mut().zip(middle) {
            *roi = Some(k);
        }
    }
    given
}

/// Pairs records `start..start + rows` with ROIs `start..start + columns`
/// in `given` by [`pairing`]'s rules, creating ROIs only when `creates`.
fn through_table(
    given: &mut [Option<usize>],
    start: usize,
    rows: usize,
    columns: usize,
    creates: bool,
    same: &impl Fn(usize, usize) -> bool,
) {
    // best[i * width + j]: the best (kept, paired) for the records from
    // `i` and the ROIs from `j`, `None` when they cannot be paired in
    // order. Stopping (deleting the records left, creating the ROIs left)
    // is open only when `creates`.
    let width = columns + 1;
    let same = |i: usize, j: usize| same(start + i, start + j);
    let mut best: Vec<Option<(u32, u32)>> = vec![None; (rows + 1) * width];
    for i in (0..=rows).rev() {
        for j in (0..=columns).rev() {
            best[i * width + j] = choices(&best, width, rows, columns, creates, &same, i, j)
                .into_iter()
                .flatten()
                .max();
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < rows {
        let here = best[i * width + j];
        let [diagonal, skip, _] = choices(&best, width, rows, columns, creates, &same, i, j);
        if diagonal.is_some() && diagonal == here {
            given[start + i] = Some(start + j);
            j += 1;
        } else if skip.is_none() || skip != here {
            break;
        }
        i += 1;
    }
}

/// The three ways on from records `i` and ROIs `j` in [`pairing`]'s table:
/// pair them, delete record `i`, or stop.
#[allow(clippy::too_many_arguments)]
fn choices(
    best: &[Option<(u32, u32)>],
    width: usize,
    rows: usize,
    columns: usize,
    creates: bool,
    same: &impl Fn(usize, usize) -> bool,
    i: usize,
    j: usize,
) -> [Option<(u32, u32)>; 3] {
    let diagonal = (i < rows && j < columns)
        .then(|| best[(i + 1) * width + j + 1])
        .flatten()
        .map(|(kept, paired)| {
            if same(i, j) {
                (kept + 1, paired)
            } else {
                (kept, paired + 1)
            }
        });
    let skip = (i < rows).then(|| best[(i + 1) * width + j]).flatten();
    let stop = (creates || j == columns).then_some((0, 0));
    [diagonal, skip, stop]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotations::memory::{Checking, MemoryBackend, MemoryConfig, ViewCheck, Write};
    use dcmview_annotation::{new_id, ApplyResult, ImageSize, OpEnvelope};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    const FRAMES: u32 = 4;

    fn author() -> Author {
        Author::parse("user:test").expect("author")
    }

    fn stamp() -> Timestamp {
        serde_json::from_value(serde_json::json!("2026-10-20T08:00:00.000Z")).expect("stamp")
    }

    fn key() -> FileKey {
        FileKey::parse("sop:1.2.3").expect("key")
    }

    fn sizes() -> BTreeMap<FileKey, ImageSize> {
        BTreeMap::from([(
            key(),
            ImageSize {
                columns: 100,
                rows: 100,
                frames: FRAMES,
            },
        )])
    }

    fn rois(coords: &[[u32; 4]], frames: &[&[u32]]) -> EmbedRoiAnnotations {
        EmbedRoiAnnotations {
            num_roi: coords.len(),
            roi_coords: coords.to_vec(),
            roi_frames: frames.iter().map(|frames| frames.to_vec()).collect(),
        }
    }

    fn run(backend: &MemoryBackend, ops: Vec<Op>, view: Option<&[Annotation]>) -> ApplyResult {
        let author = author();
        let key = key();
        let envelope = OpEnvelope {
            op_id: new_id(),
            actor: author.clone(),
            ts: stamp(),
            op: Op::Batch { ops },
        };
        backend
            .transact(
                &envelope,
                &sizes(),
                Write {
                    checking: Checking::AsLoaded,
                    author: &author,
                    embed_slot: Some(0),
                    import: false,
                    view: view.map(|records| ViewCheck {
                        key: &key,
                        slot: 0,
                        records,
                    }),
                    bindings: None,
                },
            )
            .expect("store")
            .result
    }

    /// A store whose view of file 0 shows `seed`, each record of its own
    /// class and, from the second on, of a layer of its own, so that a
    /// record given another's ROI shows.
    fn seeded(seed: &EmbedRoiAnnotations) -> MemoryBackend {
        let backend = MemoryBackend::new(MemoryConfig {
            author: author(),
            schema: None,
        });
        let mut ops = Vec::new();
        for (n, coords) in seed.roi_coords.iter().enumerate() {
            let mut layer = backend.default_layer().clone();
            if n > 0 {
                let layer_value =
                    serde_json::json!({ "id": format!("L-{n}"), "name": "Layer", "kind": "user" });
                ops.push(Op::CreateLayer {
                    layer: serde_json::from_value(layer_value).expect("layer"),
                });
                layer = LayerId::parse(&format!("L-{n}")).expect("layer id");
            }
            let mut record = annotation_of(
                *coords,
                scope_at(&seed.roi_frames, n),
                &key(),
                &layer,
                &author(),
                &stamp(),
            );
            record.class = format!("class-{n}");
            ops.push(Op::CreateAnnotation {
                annotation: Box::new(record),
            });
        }
        assert!(matches!(run(&backend, ops, None), ApplyResult::Ok { .. }));
        backend
    }

    fn records(backend: &MemoryBackend) -> Vec<Annotation> {
        backend.embed_records(&key(), 0).expect("view")
    }

    fn view(backend: &MemoryBackend) -> EmbedRoiAnnotations {
        rois_of(&records(backend), FRAMES)
    }

    /// Who a ROI of the view is: its record's id, class and layer.
    type Who = (Uuid, String, String);

    fn who(backend: &MemoryBackend) -> Vec<Who> {
        records(backend)
            .into_iter()
            .map(|record| (record.id, record.class, record.layer.as_str().to_string()))
            .collect()
    }

    /// Saves `wanted` as the endpoint does, and returns the operations.
    fn save(backend: &MemoryBackend, wanted: &EmbedRoiAnnotations) -> Vec<Op> {
        let current = records(backend);
        let ops = replacement_ops(
            &current,
            wanted,
            FRAMES,
            backend.default_layer(),
            &author(),
            &stamp(),
            &key(),
        );
        if !ops.is_empty() {
            let result = run(backend, ops.clone(), Some(&current));
            assert!(matches!(result, ApplyResult::Ok { .. }), "{result:?}");
        }
        assert_eq!(
            &view(backend),
            wanted,
            "the view shows what was saved, in its order"
        );
        ops
    }

    /// The inverse of a save's batch, as a client undoing it sends it: each
    /// operation's inverse, last first, against the revision it left.
    fn undo(backend: &MemoryBackend, ops: Vec<Op>) {
        let inverse = ops
            .into_iter()
            .rev()
            .map(|op| match op {
                Op::CreateAnnotation { annotation } => Op::DeleteAnnotation {
                    id: annotation.id,
                    base_rev: 1,
                    snapshot: annotation,
                },
                Op::UpdateAnnotation {
                    id,
                    file,
                    base_rev,
                    before,
                    after,
                } => Op::UpdateAnnotation {
                    id,
                    file,
                    base_rev: base_rev + 1,
                    before: after,
                    after: before,
                },
                Op::DeleteAnnotation {
                    id,
                    base_rev,
                    snapshot,
                } => Op::RestoreAnnotation {
                    id,
                    base_rev: base_rev + 1,
                    snapshot,
                },
                other => panic!("a save made {other:?}"),
            })
            .collect();
        let result = run(backend, inverse, None);
        assert!(matches!(result, ApplyResult::Ok { .. }), "{result:?}");
    }

    /// Saves `wanted` over `seed` and checks that undoing the save gives
    /// back the records and the view as they were. Returns the operations
    /// and who the view's ROIs were before and after the save.
    fn saved(
        seed: &EmbedRoiAnnotations,
        wanted: &EmbedRoiAnnotations,
    ) -> (Vec<Op>, Vec<Who>, Vec<Who>) {
        let backend = seeded(seed);
        let before = who(&backend);
        let shapes: Vec<_> = records(&backend)
            .into_iter()
            .map(|record| (record.id, record.geometry, record.frames))
            .collect();
        let ops = save(&backend, wanted);
        let after = who(&backend);
        undo(&backend, ops.clone());
        assert_eq!(
            who(&backend),
            before,
            "undo gives back every record in its place"
        );
        assert_eq!(
            records(&backend)
                .into_iter()
                .map(|record| (record.id, record.geometry, record.frames))
                .collect::<Vec<_>>(),
            shapes
        );
        assert_eq!(&view(&backend), seed);
        (ops, before, after)
    }

    fn kinds(ops: &[Op]) -> Vec<(&'static str, Option<Uuid>)> {
        ops.iter()
            .map(|op| match op {
                Op::CreateAnnotation { .. } => ("create", None),
                Op::UpdateAnnotation { id, .. } => ("update", Some(*id)),
                Op::DeleteAnnotation { id, .. } => ("delete", Some(*id)),
                _ => ("other", None),
            })
            .collect()
    }

    const A: [u32; 4] = [10, 10, 20, 20];
    const B: [u32; 4] = [30, 30, 40, 40];
    const C: [u32; 4] = [50, 50, 60, 60];
    const D: [u32; 4] = [70, 70, 80, 80];

    /// Deleting one ROI deletes its record, wherever it stood, and every
    /// other record keeps its ROI, class and layer. Before, deleting any
    /// but the last moved the next rectangle into the deleted record.
    #[test]
    fn deleting_a_roi_deletes_its_own_record() {
        let seed = rois(&[A, B, C], &[&[0], &[1], &[2]]);
        for gone in 0..3 {
            let mut wanted = seed.clone();
            wanted.roi_coords.remove(gone);
            wanted.roi_frames.remove(gone);
            wanted.num_roi = 2;
            let (ops, mut before, after) = saved(&seed, &wanted);
            let deleted = before.remove(gone);
            assert_eq!(
                kinds(&ops),
                [("delete", Some(deleted.0))],
                "deleting ROI {gone}"
            );
            assert_eq!(after, before, "deleting ROI {gone}");
        }
    }

    /// Of two records that are one rectangle over the same frames, a save
    /// that keeps one of them keeps one, here the first, and the ROIs
    /// around them keep their records.
    #[test]
    fn deleting_one_of_two_equal_rois_keeps_one_and_the_others() {
        let seed = rois(&[A, B, B, C], &[]);
        let wanted = rois(&[A, B, C], &[]);
        let (ops, before, after) = saved(&seed, &wanted);
        assert_eq!(kinds(&ops), [("delete", Some(before[2].0))]);
        assert_eq!(
            after,
            [before[0].clone(), before[1].clone(), before[3].clone()]
        );
    }

    /// Moving or resizing one ROI updates its record's geometry alone.
    #[test]
    fn moving_a_roi_updates_its_own_record() {
        let seed = rois(&[A, B, C], &[]);
        let wanted = rois(&[A, D, C], &[]);
        let (ops, before, after) = saved(&seed, &wanted);
        assert_eq!(kinds(&ops), [("update", Some(before[1].0))]);
        let Op::UpdateAnnotation { after: patch, .. } = &ops[0] else {
            unreachable!()
        };
        assert!(patch.geometry.is_some() && patch.frames.is_none());
        assert_eq!(after, before);
    }

    /// Changing one ROI's frames, as the viewer does, which then lists
    /// every ROI's frames, updates that record's frames alone: a record for
    /// every frame is the ROI listed with every frame.
    #[test]
    fn changing_a_rois_frames_updates_its_own_record() {
        let seed = rois(&[A, B, C], &[&[0, 1, 2, 3], &[1], &[0, 1, 2, 3]]);
        let wanted = rois(&[A, B, C], &[&[0, 1, 2, 3], &[0, 1, 2, 3], &[0, 1, 2, 3]]);
        let (ops, before, after) = saved(&seed, &wanted);
        assert_eq!(kinds(&ops), [("update", Some(before[1].0))]);
        assert_eq!(after, before);

        let all = rois(&[A, B, C], &[]);
        let (ops, before, after) = saved(
            &all,
            &rois(&[A, B, C], &[&[0, 1, 2, 3], &[2], &[0, 1, 2, 3]]),
        );
        assert_eq!(
            kinds(&ops),
            [
                ("update", Some(before[0].0)),
                ("update", Some(before[1].0)),
                ("update", Some(before[2].0))
            ],
            "records for every frame become records for the frames named"
        );
        assert_eq!(after, before);
    }

    /// A drawn ROI, which the viewer appends, is created; the records
    /// before it are untouched.
    #[test]
    fn adding_a_roi_creates_a_record_last() {
        let seed = rois(&[A, B], &[&[0], &[1]]);
        let wanted = rois(&[A, B, C], &[&[0], &[1], &[2]]);
        let (ops, before, after) = saved(&seed, &wanted);
        assert_eq!(kinds(&ops), [("create", None)]);
        assert_eq!(after[..2], before[..]);
        assert_eq!(after[2].1, "roi");
    }

    /// A record the import read with its corners out of order and its
    /// frames as written is the ROI the viewer sends back in order: it
    /// keeps its record when the ROI before it is deleted.
    #[test]
    fn a_roi_sent_back_in_order_keeps_its_record() {
        let seed = rois(&[A, [40, 40, 30, 30], C], &[&[0], &[2, 1, 1], &[3]]);
        let wanted = rois(&[B, C], &[&[1, 2], &[3]]);
        let (ops, before, after) = saved(&seed, &wanted);
        assert_eq!(
            kinds(&ops),
            [("update", Some(before[1].0)), ("delete", Some(before[0].0))]
        );
        assert_eq!(after, before[1..]);
    }

    /// Edits the viewer coalesced into one save while another was in
    /// flight: each kept ROI keeps its record, and what changed is paired
    /// in order.
    #[test]
    fn a_save_of_several_edits_keeps_every_record_still_in_it() {
        let seed = rois(&[A, B, C], &[]);
        // The first deleted, the last moved, one drawn.
        let (ops, before, after) = saved(&seed, &rois(&[B, D, A], &[]));
        assert_eq!(
            kinds(&ops),
            [
                ("update", Some(before[2].0)),
                ("delete", Some(before[0].0)),
                ("create", None)
            ]
        );
        assert_eq!(after[..2], [before[1].clone(), before[2].clone()]);

        // A ROI put before the last can only be created last, so the last
        // record becomes it and the last ROI is created: the view keeps
        // the order of the save.
        let (ops, before, after) = saved(&seed, &rois(&[A, B, D, C], &[]));
        assert_eq!(
            kinds(&ops),
            [("update", Some(before[2].0)), ("create", None)]
        );
        assert_eq!(after[..3], before[..]);

        // Two moved: each record becomes the ROI in its place.
        let (ops, before, after) = saved(&seed, &rois(&[D, B, [1, 1, 2, 2]], &[]));
        assert_eq!(
            kinds(&ops),
            [("update", Some(before[0].0)), ("update", Some(before[2].0))]
        );
        assert_eq!(after, before);
    }

    /// Saving what the view shows changes nothing, and an empty save
    /// deletes every record.
    #[test]
    fn saving_the_view_changes_nothing_and_an_empty_save_deletes_all() {
        let seed = rois(&[A, B, C], &[&[0], &[1], &[2]]);
        let (ops, _, _) = saved(&seed, &seed);
        assert!(ops.is_empty());
        let (ops, before, _) = saved(&seed, &rois(&[], &[]));
        assert_eq!(
            kinds(&ops),
            before
                .iter()
                .map(|who| ("delete", Some(who.0)))
                .collect::<Vec<_>>()
        );
    }

    /// The pairing in order: equal ROIs are kept, the rest paired in order
    /// with extra records deleted and extra ROIs created at the end.
    #[test]
    fn records_are_paired_in_order_and_kept_where_equal() {
        let paired = |current: &[u32], wanted: &[u32]| {
            pairing(current.len(), wanted.len(), |i, j| current[i] == wanted[j])
        };
        assert_eq!(paired(&[1, 2, 3], &[1, 3]), [Some(0), None, Some(1)]);
        assert_eq!(paired(&[1, 2, 3], &[2, 3]), [None, Some(0), Some(1)]);
        assert_eq!(
            paired(&[1, 2, 2, 3], &[2, 3]),
            [None, Some(0), None, Some(1)]
        );
        assert_eq!(paired(&[1, 0, 0], &[0]), [None, Some(0), None]);
        assert_eq!(
            paired(&[3, 2, 1, 0], &[2, 1, 3, 0]),
            [None, Some(0), Some(1), Some(2)]
        );
        assert_eq!(paired(&[1, 2, 3], &[4, 5]), [Some(0), Some(1), None]);
        assert_eq!(paired(&[1, 2, 3], &[3, 1]), [None, None, Some(0)]);
        assert_eq!(paired(&[1, 2], &[1, 9, 2]), [Some(0), Some(1)]);
        assert_eq!(
            paired(&[1, 2, 3, 4], &[2, 9, 4]),
            [None, Some(0), Some(1), Some(2)]
        );
        assert_eq!(paired(&[], &[1]), []);
        assert_eq!(paired(&[1], &[]), [None]);
    }

    /// Whether `given` keeps both orders and leaves only the last of `m`
    /// ROIs unpaired.
    fn in_order(given: &[Option<usize>], m: usize) -> bool {
        let paired: Vec<usize> = given.iter().flatten().copied().collect();
        paired.iter().copied().eq(0..paired.len()) && paired.len() <= m
    }

    /// Every pairing of up to five records with up to five ROIs over three
    /// shapes is the one the rules name, found by trying every pairing that
    /// keeps both orders.
    #[test]
    fn the_pairing_is_the_best_by_its_rules() {
        fn lists(len: usize) -> Vec<Vec<u8>> {
            (0..3usize.pow(len as u32))
                .map(|mut n| {
                    (0..len)
                        .map(|_| {
                            let digit = (n % 3) as u8;
                            n /= 3;
                            digit
                        })
                        .collect()
                })
                .collect()
        }
        // Records paired, as a bit set, with the first ROIs in order.
        let best = |current: &[u8], wanted: &[u8]| {
            (0u32..1 << current.len())
                .filter(|chosen| chosen.count_ones() as usize <= wanted.len())
                .map(|chosen| {
                    let mut next = 0;
                    let given: Vec<Option<usize>> = (0..current.len())
                        .map(|i| {
                            (chosen & 1 << i != 0).then(|| {
                                next += 1;
                                next - 1
                            })
                        })
                        .collect();
                    let kept = (0..current.len())
                        .filter(|&i| given[i].is_some_and(|j| current[i] == wanted[j]))
                        .count();
                    ((kept, next - kept), given)
                })
                .max()
                .expect("a pairing")
                .1
        };
        for n in 0..=5 {
            for m in 0..=5 {
                for current in lists(n) {
                    for wanted in lists(m) {
                        let given = pairing(n, m, |i, j| current[i] == wanted[j]);
                        assert!(in_order(&given, m));
                        assert_eq!(given, best(&current, &wanted), "{current:?} to {wanted:?}");
                    }
                }
            }
        }
    }

    /// A long list with one ROI deleted near its start keeps every other
    /// record, and one too long for the table keeps both orders: no ROI is
    /// created before the run at its end.
    #[test]
    fn a_long_list_keeps_its_records_and_its_order() {
        let given = pairing(2_000, 1_999, |i, j| i == j + 1);
        assert_eq!(given[0], None);
        assert!(given[1..].iter().zip(0..).all(|(roi, j)| *roi == Some(j)));

        // Two swapped before 1,100 kept: either keeping one of them and
        // creating the other would put it after the kept run.
        let shape = |k: usize| match k {
            0 => 1,
            1 => 0,
            k => k,
        };
        let given = pairing(1_102, 1_102, |i, j| i == shape(j));
        assert!(in_order(&given, 1_102));
        assert_eq!(given[..2], [Some(0), Some(1)]);
        assert!(given[2..].iter().zip(2..).all(|(roi, j)| *roi == Some(j)));
    }

    /// The viewer lists every frame for a record that covers every frame:
    /// that record is still the ROI it was, so deleting the ROI before it
    /// keeps it, and its frames become the frames listed.
    #[test]
    fn a_roi_listed_with_every_frame_keeps_its_record() {
        let seed = rois(&[A, B, C], &[]);
        let every: &[u32] = &[0, 1, 2, 3];
        let (ops, before, after) = saved(&seed, &rois(&[A, C], &[every, every]));
        assert_eq!(
            kinds(&ops),
            [
                ("update", Some(before[0].0)),
                ("update", Some(before[2].0)),
                ("delete", Some(before[1].0))
            ]
        );
        assert_eq!(after, [before[0].clone(), before[2].clone()]);
    }

    /// A save whose changed part is too large for the table is paired by
    /// position there, and still keeps the runs at its start and end.
    #[test]
    fn a_large_changed_part_is_paired_by_position() {
        let n = 1_100;
        let given = pairing(n + 2, n + 1, |i, j| {
            (i == 0 && j == 0) || (i == n + 1 && j == n)
        });
        assert_eq!(given[0], Some(0));
        assert_eq!(given[n + 1], Some(n));
        assert!(given[1..n].iter().zip(1..).all(|(roi, j)| *roi == Some(j)));
        assert_eq!(given[n], None);
    }
}

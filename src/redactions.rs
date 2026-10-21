//! Redaction boxes: rectangles drawn by hand over pixels that must not be
//! shown (burned-in text), kept per file in memory for the session. They are
//! never written anywhere; the frame endpoints apply them.

use crate::annotations::canonicalize_annotations;
use crate::api::contracts::EmbedRoiAnnotations;
use crate::pixels::Redaction;
use crate::types::FileEntry;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Boxes have the shape and validation of ROI annotations: `roi_coords` are
/// `[row0, column0, row1, column1]`, and `roi_frames` is either empty (every
/// box covers every frame) or lists each box's zero-based frames.
///
/// Redaction boxes are not annotations (`docs/design/annotation-model.md`,
/// re-baseline amendment of 2026-10-05). This store shares nothing with the
/// annotation store but the wire shape and its validation: the boxes are
/// kept here by file index, have no file key, record or revision of the
/// annotation model, are changed by no operation, and appear in no snapshot
/// or export.
#[derive(Debug, Clone)]
pub struct RedactionStore {
    /// Each file's boxes with their revision, under one lock: a frame is
    /// never rendered from one file state and cached under the revision of
    /// another.
    files: Arc<Mutex<Files>>,
}

#[derive(Debug, Default)]
struct Files {
    /// The last revision given out. Revisions are drawn from one counter,
    /// so one is never reused; display frames are cached under them.
    latest: u64,
    by_file: HashMap<usize, FileBoxes>,
}

#[derive(Debug)]
struct FileBoxes {
    revision: u64,
    boxes: EmbedRoiAnnotations,
}

impl Default for RedactionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RedactionStore {
    pub fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(Files::default())),
        }
    }

    pub fn get(&self, file_index: usize) -> Result<EmbedRoiAnnotations> {
        let files = self
            .files
            .lock()
            .map_err(|_| anyhow!("redaction store lock poisoned"))?;
        Ok(files
            .by_file
            .get(&file_index)
            .map_or_else(EmbedRoiAnnotations::empty, |file| file.boxes.clone()))
    }

    /// Replaces the file's boxes after validating them against its image
    /// size and frame count; returns them in canonical form. An invalid
    /// replacement changes neither the boxes nor their revision.
    pub fn replace_for_file(
        &self,
        file: &FileEntry,
        boxes: EmbedRoiAnnotations,
    ) -> Result<EmbedRoiAnnotations> {
        let canonical = canonicalize_annotations(boxes, file.rows, file.columns, file.frame_count)?;
        let mut files = self
            .files
            .lock()
            .map_err(|_| anyhow!("redaction store lock poisoned"))?;
        files.latest += 1;
        let revision = files.latest;
        files.by_file.insert(
            file.index,
            FileBoxes {
                revision,
                boxes: canonical.clone(),
            },
        );
        Ok(canonical)
    }

    /// The boxes that apply to one frame of a file.
    pub fn for_frame(&self, file_index: usize, frame: u32) -> Result<Redaction> {
        let files = self
            .files
            .lock()
            .map_err(|_| anyhow!("redaction store lock poisoned"))?;
        let Some(stored) = files.by_file.get(&file_index) else {
            return Ok(Redaction::default());
        };
        let boxes = stored
            .boxes
            .roi_coords
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                stored
                    .boxes
                    .roi_frames
                    .get(*index)
                    .is_none_or(|frames| frames.contains(&frame))
            })
            .map(|(_, coords)| *coords)
            .collect();
        Ok(Redaction {
            revision: stored.revision,
            boxes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> FileEntry {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/golden-uncompressed-u16-multiframe.dcm");
        crate::loader::test_entry(&fixture)
    }

    fn boxes(coords: Vec<[u32; 4]>, frames: Vec<Vec<u32>>) -> EmbedRoiAnnotations {
        EmbedRoiAnnotations {
            num_roi: coords.len(),
            roi_coords: coords,
            roi_frames: frames,
        }
    }

    #[test]
    fn a_file_without_boxes_has_no_redaction() {
        let store = RedactionStore::new();
        assert_eq!(
            store.for_frame(0, 0).expect("redaction"),
            Redaction::default()
        );
    }

    #[test]
    fn boxes_cover_every_frame_unless_they_list_frames() {
        let store = RedactionStore::new();
        let file = file();
        assert!(file.frame_count >= 2, "fixture is multi-frame");

        store
            .replace_for_file(&file, boxes(vec![[0, 0, 1, 1]], Vec::new()))
            .expect("store boxes");
        assert_eq!(
            store.for_frame(file.index, 0).expect("frame 0").boxes,
            [[0, 0, 1, 1]]
        );
        assert_eq!(
            store.for_frame(file.index, 1).expect("frame 1").boxes,
            [[0, 0, 1, 1]]
        );

        store
            .replace_for_file(
                &file,
                boxes(vec![[0, 0, 1, 1], [1, 1, 2, 2]], vec![vec![0], vec![0, 1]]),
            )
            .expect("store scoped boxes");
        assert_eq!(
            store.for_frame(file.index, 0).expect("frame 0").boxes,
            [[0, 0, 1, 1], [1, 1, 2, 2]]
        );
        assert_eq!(
            store.for_frame(file.index, 1).expect("frame 1").boxes,
            [[1, 1, 2, 2]]
        );
    }

    #[test]
    fn every_change_takes_a_new_revision_and_removal_leaves_no_boxes() {
        let store = RedactionStore::new();
        let file = file();

        store
            .replace_for_file(&file, boxes(vec![[0, 0, 1, 1]], Vec::new()))
            .expect("store boxes");
        let first = store.for_frame(file.index, 0).expect("first");
        store
            .replace_for_file(&file, boxes(Vec::new(), Vec::new()))
            .expect("remove boxes");
        let removed = store.for_frame(file.index, 0).expect("removed");

        assert!(removed.is_empty());
        assert!(removed.revision > first.revision);
        assert!(store
            .replace_for_file(&file, boxes(vec![[0, 0, 9999, 1]], Vec::new()))
            .is_err());
    }

    /// Frames are cached under the revision, so a revision must always name
    /// one set of boxes, however a read interleaves with a change.
    #[test]
    fn a_revision_never_names_two_sets_of_boxes() {
        let store = RedactionStore::new();
        let file = file();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let (store, stop, index) = (store.clone(), stop.clone(), file.index);
                std::thread::spawn(move || {
                    let mut seen: HashMap<u64, Vec<[u32; 4]>> = HashMap::new();
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let redaction = store.for_frame(index, 0).expect("read boxes");
                        let earlier = seen
                            .entry(redaction.revision)
                            .or_insert_with(|| redaction.boxes.clone());
                        assert_eq!(*earlier, redaction.boxes, "revision {}", redaction.revision);
                    }
                })
            })
            .collect();

        for round in 0..2_000 {
            let coords = if round % 2 == 0 {
                vec![[0, 0, 2, 2]]
            } else {
                vec![[0, 0, 2, 2], [2, 2, 4, 4]]
            };
            store
                .replace_for_file(&file, boxes(coords, vec![]))
                .expect("replace boxes");
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for reader in readers {
            reader.join().expect("reader saw one box set per revision");
        }
    }
}

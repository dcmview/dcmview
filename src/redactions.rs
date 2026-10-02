//! Redaction boxes: rectangles drawn by hand over pixels that must not be
//! shown (burned-in text), kept per file in memory for the session. They are
//! never written anywhere; the frame endpoints apply them.

use crate::annotations::AnnotationStore;
use crate::api::contracts::EmbedRoiAnnotations;
use crate::pixels::Redaction;
use crate::types::FileEntry;
use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Boxes have the shape and validation of ROI annotations: `roi_coords` are
/// `[row0, column0, row1, column1]`, and `roi_frames` is either empty (every
/// box covers every frame) or lists each box's zero-based frames.
#[derive(Debug, Clone)]
pub struct RedactionStore {
    boxes: AnnotationStore,
    /// The revision of each file's boxes, drawn from one counter so a
    /// revision is never reused; display frames are cached under it.
    revisions: Arc<Mutex<Revisions>>,
}

#[derive(Debug, Default)]
struct Revisions {
    latest: u64,
    by_file: HashMap<usize, u64>,
}

impl Default for RedactionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RedactionStore {
    pub fn new() -> Self {
        Self {
            boxes: AnnotationStore::empty(),
            revisions: Arc::new(Mutex::new(Revisions::default())),
        }
    }

    pub fn get(&self, file_index: usize) -> Result<EmbedRoiAnnotations> {
        self.boxes.get(file_index)
    }

    /// Replaces the file's boxes after validating them against its image
    /// size and frame count; returns them in canonical form.
    pub fn replace_for_file(
        &self,
        file: &FileEntry,
        boxes: EmbedRoiAnnotations,
    ) -> Result<EmbedRoiAnnotations> {
        // The revision moves first: a frame rendered while the boxes change
        // is then cached under a revision no later request asks for.
        {
            let mut revisions = self
                .revisions
                .lock()
                .map_err(|_| anyhow!("redaction store lock poisoned"))?;
            revisions.latest += 1;
            let revision = revisions.latest;
            revisions.by_file.insert(file.index, revision);
        }
        self.boxes.replace_for_file(file, boxes)
    }

    /// The boxes that apply to one frame of a file.
    pub fn for_frame(&self, file_index: usize, frame: u32) -> Result<Redaction> {
        let revision = self
            .revisions
            .lock()
            .map_err(|_| anyhow!("redaction store lock poisoned"))?
            .by_file
            .get(&file_index)
            .copied();
        let Some(revision) = revision else {
            return Ok(Redaction::default());
        };
        let stored = self.boxes.get(file_index)?;
        let boxes = stored
            .roi_coords
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                stored
                    .roi_frames
                    .get(*index)
                    .is_none_or(|frames| frames.contains(&frame))
            })
            .map(|(_, coords)| *coords)
            .collect();
        Ok(Redaction { revision, boxes })
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
}

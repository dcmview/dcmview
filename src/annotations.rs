//! Annotations: the store behind the annotation endpoints, and the EMBED
//! CSV the viewer reads with `--annotations` and writes on export.
//!
//! The model (what an annotation, a label, a layer and an operation are, and
//! the rules each must meet) is the `dcmview-annotation` crate. This module
//! holds state and applies operations to it
//! (`docs/design/annotation-model.md` 7.3, 7.4):
//!
//! | Module | Holds |
//! |---|---|
//! | `backend` | [`AnnotationBackend`]: the three things a store does, whatever it is (memory here, a hub or a sidecar file later), and [`Snapshot`] |
//! | `memory` | [`MemoryBackend`]: the default store. Records by id, in server memory for the session; one transaction per envelope |
//! | `embed` | The EMBED compatibility view: ROI rows as rectangle records and back, the operations a replaced ROI list amounts to, and the CSV export |
//! | `store` | [`AnnotationStore`]: what a session holds, the backend and the state of the `--annotations` import |
//! | this file | The EMBED CSV reader ([`AnnotationSource`]) and the check a replaced ROI list must pass ([`canonicalize_annotations`]) |
//!
//! Nothing here knows the file registry: records are addressed by file key
//! and the caller says which files exist and how large they are. Joining a
//! request to the registry (settling a file's key, translating the keys a
//! masked session shows) is `server::annotations`.
//!
//! Redaction boxes are not annotations and are not in this store
//! (`crate::redactions`).

mod backend;
pub(crate) mod embed;
pub(crate) mod memory;
mod store;

pub use crate::api::contracts::EmbedRoiAnnotations;
pub use backend::{AnnotationBackend, BackendError, Snapshot};
pub use memory::{
    MemoryBackend, MemoryConfig, DEFAULT_LAYER_ID, DEFAULT_LAYER_NAME, REMEMBERED_OPS,
    REMEMBERED_REVS,
};
pub use store::{session_author, AnnotationStore, EMBED_IMPORT_AUTHOR};

use crate::types::FileEntry;
use anyhow::{anyhow, bail, Context, Result};
use csv::{ReaderBuilder, StringRecord};
use std::collections::HashMap;
use std::env;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PathKey(PathBuf);

/// The ROI rows an EMBED CSV holds for the loaded files, by file index, as
/// the CSV wrote them.
pub type AnnotationIndexMap = HashMap<usize, EmbedRoiAnnotations>;

#[derive(Debug, Clone)]
pub struct AnnotationSource {
    csv_path: PathBuf,
    working_directory: PathBuf,
    indexes: ColumnIndexes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnotationLoadReport {
    pub matched_rows: usize,
    pub unmatched_rows: usize,
}

#[derive(Debug, Clone)]
struct ColumnIndexes {
    path: usize,
    roi_coords: usize,
    num_roi: Option<usize>,
    roi_frames: Option<usize>,
}

impl AnnotationSource {
    pub fn from_path(csv_path: &Path) -> Result<Self> {
        let working_directory =
            env::current_dir().context("failed to resolve current directory")?;
        let csv_path = absolute_path(csv_path, &working_directory);
        let indexes = read_column_indexes(&csv_path)?;
        Ok(Self {
            csv_path,
            working_directory,
            indexes,
        })
    }

    pub fn load_for_files(&self, files: &[Arc<FileEntry>]) -> Result<AnnotationIndexMap> {
        self.load_for_files_with_check(files, || Ok(()))
            .map(|(map, _)| map)
    }

    pub fn load_for_files_with_check<F>(
        &self,
        files: &[Arc<FileEntry>],
        mut check_active: F,
    ) -> Result<(AnnotationIndexMap, AnnotationLoadReport)>
    where
        F: FnMut() -> Result<()>,
    {
        let file_lookup = build_file_lookup(files, &self.working_directory)?;
        let mut reader = ReaderBuilder::new()
            .flexible(false)
            .from_path(&self.csv_path)
            .with_context(|| {
                format!(
                    "failed to open annotations CSV: {}",
                    self.csv_path.display()
                )
            })?;
        let mut annotations_by_file = HashMap::new();
        let mut matched_at_row = HashMap::<usize, usize>::new();
        let mut matched_rows = 0_usize;
        let mut unmatched_rows = 0_usize;

        for (idx, row_result) in reader.records().enumerate() {
            check_active()?;
            let row_number = idx + 2;
            let row = row_result
                .with_context(|| format!("annotations CSV row {row_number} could not be parsed"))?;
            let path_key =
                parse_path_key(&row, &self.indexes, row_number, &self.working_directory)?;
            let Some(file_targets) = matching_file_targets(&file_lookup, &path_key) else {
                unmatched_rows += 1;
                continue;
            };

            let annotations = parse_annotations(&row, &self.indexes, row_number)?;
            for &(file_index, frame_count) in file_targets {
                if let Some(previous_row) = matched_at_row.insert(file_index, row_number) {
                    bail!(
                        "annotations CSV row {row_number}: duplicate matching anon_dicom_path for loaded file (already matched at row {previous_row})"
                    );
                }
                validate_frames_in_range(&annotations, frame_count, row_number)?;
                annotations_by_file.insert(file_index, annotations.clone());
            }
            matched_rows += 1;
        }
        check_active()?;

        Ok((
            annotations_by_file,
            AnnotationLoadReport {
                matched_rows,
                unmatched_rows,
            },
        ))
    }
}

pub fn load_annotations_for_files(
    csv_path: &Path,
    files: &[Arc<FileEntry>],
) -> Result<AnnotationIndexMap> {
    AnnotationSource::from_path(csv_path)?.load_for_files(files)
}

fn read_column_indexes(csv_path: &Path) -> Result<ColumnIndexes> {
    let mut reader = ReaderBuilder::new()
        .flexible(false)
        .from_path(csv_path)
        .with_context(|| format!("failed to open annotations CSV: {}", csv_path.display()))?;

    let headers = reader
        .headers()
        .with_context(|| {
            format!(
                "failed to read annotations CSV header: {}",
                csv_path.display()
            )
        })?
        .clone();
    build_column_indexes(&headers)
}

fn build_column_indexes(headers: &StringRecord) -> Result<ColumnIndexes> {
    let find = |name: &str| headers.iter().position(|h| h == name);

    let path = find("anon_dicom_path")
        .ok_or_else(|| anyhow!("annotations CSV missing required column `anon_dicom_path`"))?;
    let roi_coords = find("ROI_coords")
        .ok_or_else(|| anyhow!("annotations CSV missing required column `ROI_coords`"))?;

    Ok(ColumnIndexes {
        path,
        roi_coords,
        num_roi: find("num_ROI"),
        roi_frames: find("ROI_frames"),
    })
}

fn parse_path_key(
    row: &StringRecord,
    indexes: &ColumnIndexes,
    row_number: usize,
    working_directory: &Path,
) -> Result<PathKey> {
    let raw_path = row.get(indexes.path).ok_or_else(|| {
        anyhow!("annotations CSV row {row_number}: missing value for `anon_dicom_path`")
    })?;
    if raw_path.trim().is_empty() {
        bail!("annotations CSV row {row_number}: anon_dicom_path must not be empty");
    }
    Ok(PathKey(absolute_path(
        Path::new(raw_path.trim()),
        working_directory,
    )))
}

fn parse_annotations(
    row: &StringRecord,
    indexes: &ColumnIndexes,
    row_number: usize,
) -> Result<EmbedRoiAnnotations> {
    let raw_roi_coords = row.get(indexes.roi_coords).ok_or_else(|| {
        anyhow!("annotations CSV row {row_number}: missing value for `ROI_coords`")
    })?;
    let roi_coords = parse_roi_coords(raw_roi_coords.trim(), row_number)?;

    let num_roi = if let Some(num_roi_idx) = indexes.num_roi {
        let raw = row.get(num_roi_idx).ok_or_else(|| {
            anyhow!("annotations CSV row {row_number}: missing value for `num_ROI`")
        })?;
        let parsed = raw.trim().parse::<usize>().map_err(|error| {
            anyhow!("annotations CSV row {row_number}: num_ROI must be an integer: {error}")
        })?;
        if parsed != roi_coords.len() {
            bail!(
                "annotations CSV row {row_number}: len(ROI_coords) must equal num_ROI ({} != {})",
                roi_coords.len(),
                parsed
            );
        }
        parsed
    } else {
        roi_coords.len()
    };

    let roi_frames = if let Some(roi_frames_idx) = indexes.roi_frames {
        let raw = row.get(roi_frames_idx).ok_or_else(|| {
            anyhow!("annotations CSV row {row_number}: missing value for `ROI_frames`")
        })?;
        let frames = parse_roi_frames(raw.trim(), row_number)?;
        if !frames.is_empty() && frames.len() != num_roi {
            bail!(
				"annotations CSV row {row_number}: len(ROI_frames) must equal num_ROI when ROI_frames is not empty ({} != {})",
				frames.len(),
				num_roi
			);
        }
        frames
    } else {
        vec![]
    };

    Ok(EmbedRoiAnnotations {
        num_roi,
        roi_coords,
        roi_frames,
    })
}

fn parse_roi_coords(raw: &str, row_number: usize) -> Result<Vec<[u32; 4]>> {
    let parsed: Vec<Vec<u32>> = serde_json::from_str(raw).map_err(|error| {
		anyhow!(
			"annotations CSV row {row_number}: ROI_coords must be a JSON list of [ymin, xmin, ymax, xmax] arrays: {error}"
		)
	})?;

    let mut coords = Vec::with_capacity(parsed.len());
    for (idx, coord) in parsed.into_iter().enumerate() {
        if coord.len() != 4 {
            bail!(
				"annotations CSV row {row_number}: ROI_coords[{idx}] must contain exactly 4 integers [ymin, xmin, ymax, xmax]"
			);
        }
        coords.push([coord[0], coord[1], coord[2], coord[3]]);
    }

    Ok(coords)
}

fn parse_roi_frames(raw: &str, row_number: usize) -> Result<Vec<Vec<u32>>> {
    serde_json::from_str(raw).map_err(|error| {
        anyhow!(
			"annotations CSV row {row_number}: ROI_frames must be a JSON list of frame-index lists: {error}"
		)
    })
}

fn build_file_lookup(
    files: &[Arc<FileEntry>],
    working_directory: &Path,
) -> Result<HashMap<PathKey, Vec<(usize, u32)>>> {
    let mut lookup = HashMap::<PathKey, Vec<(usize, u32)>>::new();
    for file in files {
        let target = (file.index, file.frame_count);
        let lexical = PathKey(absolute_path(&file.path, working_directory));
        push_unique_target(&mut lookup, lexical, target);
        if let Ok(canonical) = file.path.canonicalize() {
            push_unique_target(&mut lookup, PathKey(normalize_path(&canonical)), target);
        }
    }
    Ok(lookup)
}

fn push_unique_target(
    lookup: &mut HashMap<PathKey, Vec<(usize, u32)>>,
    key: PathKey,
    target: (usize, u32),
) {
    let targets = lookup.entry(key).or_default();
    if !targets.contains(&target) {
        targets.push(target);
    }
}

fn matching_file_targets<'a>(
    lookup: &'a HashMap<PathKey, Vec<(usize, u32)>>,
    path_key: &PathKey,
) -> Option<&'a Vec<(usize, u32)>> {
    lookup.get(path_key).or_else(|| {
        path_key
            .0
            .canonicalize()
            .ok()
            .and_then(|canonical| lookup.get(&PathKey(normalize_path(&canonical))))
    })
}

fn validate_frames_in_range(
    annotations: &EmbedRoiAnnotations,
    frame_count: u32,
    row_number: usize,
) -> Result<()> {
    if annotations.roi_frames.is_empty() {
        return Ok(());
    }

    for (roi_idx, frames) in annotations.roi_frames.iter().enumerate() {
        for frame in frames {
            if *frame >= frame_count {
                bail!(
					"annotations CSV row {row_number}: ROI_frames[{roi_idx}] contains frame {frame}, but matched DICOM has {frame_count} frame(s)"
				);
            }
        }
    }

    Ok(())
}

pub fn canonicalize_annotations(
    annotations: EmbedRoiAnnotations,
    rows: u32,
    columns: u32,
    frame_count: u32,
) -> Result<EmbedRoiAnnotations> {
    if rows == 0 || columns == 0 {
        bail!("annotations cannot be edited for files without image dimensions");
    }

    let mut roi_coords = Vec::with_capacity(annotations.roi_coords.len());
    for (idx, [ymin, xmin, ymax, xmax]) in annotations.roi_coords.into_iter().enumerate() {
        let y0 = ymin.min(ymax);
        let y1 = ymin.max(ymax);
        let x0 = xmin.min(xmax);
        let x1 = xmin.max(xmax);

        if y0 == y1 || x0 == x1 {
            bail!("ROI_coords[{idx}] must describe a non-empty rectangle");
        }
        if y1 > rows || x1 > columns {
            bail!(
				"ROI_coords[{idx}] exceeds image bounds: [{y0}, {x0}, {y1}, {x1}] outside {rows}x{columns}"
			);
        }

        roi_coords.push([y0, x0, y1, x1]);
    }

    let num_roi = roi_coords.len();
    let roi_frames = if annotations.roi_frames.is_empty() {
        Vec::new()
    } else {
        if annotations.roi_frames.len() != num_roi {
            bail!(
                "len(ROI_frames) must equal ROI count when ROI_frames is not empty ({} != {})",
                annotations.roi_frames.len(),
                num_roi
            );
        }
        for (roi_idx, frames) in annotations.roi_frames.iter().enumerate() {
            for frame in frames {
                if *frame >= frame_count {
                    bail!(
                        "ROI_frames[{roi_idx}] contains frame {frame}, but DICOM has {frame_count} frame(s)"
                    );
                }
            }
        }
        annotations.roi_frames
    };

    Ok(EmbedRoiAnnotations {
        num_roi,
        roi_coords,
        roi_frames,
    })
}

fn absolute_path(path: &Path, working_directory: &Path) -> PathBuf {
    if path.is_absolute() {
        normalize_path(path)
    } else {
        normalize_path(&working_directory.join(path))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        canonicalize_annotations, load_annotations_for_files, AnnotationSource, EmbedRoiAnnotations,
    };
    use crate::types::FileEntry;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    #[test]
    fn maps_valid_annotations_to_matching_files() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched_file = dir.path().join("matched.dcm");
        let unmatched_file = dir.path().join("unmatched.dcm");

        write_csv(
			&csv_path,
			&format!(
				"anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{matched},2,\"[[10,20,30,40],[50,60,70,80]]\",\"[[0,1],[2]]\"\n",
				matched = matched_file.display(),
			),
		);

        let files = vec![
            file_entry(0, matched_file.clone(), 3),
            file_entry(1, unmatched_file.clone(), 1),
        ];
        let mapped =
            load_annotations_for_files(&csv_path, &files).expect("annotations should parse");

        assert_eq!(mapped.len(), 1);
        assert_eq!(
            mapped.get(&0),
            Some(&EmbedRoiAnnotations {
                num_roi: 2,
                roi_coords: vec![[10, 20, 30, 40], [50, 60, 70, 80]],
                roi_frames: vec![vec![0, 1], vec![2]],
            })
        );
        assert!(!mapped.contains_key(&1));
    }

    #[test]
    fn accepts_empty_roi_frames_for_non_dbt_rows() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched_file = dir.path().join("matched.dcm");

        write_csv(
            &csv_path,
            &format!(
				"anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{matched},1,\"[[1,2,3,4]]\",\"[]\"\n",
				matched = matched_file.display(),
			),
        );

        let files = vec![file_entry(0, matched_file.clone(), 42)];
        let mapped =
            load_annotations_for_files(&csv_path, &files).expect("annotations should parse");
        assert_eq!(
            mapped.get(&0).map(|value| value.roi_frames.clone()),
            Some(vec![])
        );
    }

    #[test]
    fn accepts_csv_without_num_roi_column() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched_file = dir.path().join("matched.dcm");

        write_csv(
			&csv_path,
			&format!(
				"anon_dicom_path,ROI_coords,ROI_frames\n{matched},\"[[10,20,30,40],[50,60,70,80]]\",\"[[0],[1]]\"\n",
				matched = matched_file.display(),
			),
		);

        let files = vec![file_entry(0, matched_file.clone(), 3)];
        let mapped = load_annotations_for_files(&csv_path, &files)
            .expect("annotations should parse without num_ROI");
        assert_eq!(mapped.get(&0).map(|a| a.num_roi), Some(2));
    }

    #[test]
    fn accepts_csv_without_roi_frames_column() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched_file = dir.path().join("matched.dcm");

        write_csv(
            &csv_path,
            &format!(
                "anon_dicom_path,ROI_coords\n{matched},\"[[1,2,3,4]]\"\n",
                matched = matched_file.display(),
            ),
        );

        let files = vec![file_entry(0, matched_file.clone(), 10)];
        let mapped = load_annotations_for_files(&csv_path, &files)
            .expect("annotations should parse without ROI_frames");
        assert_eq!(mapped.get(&0).map(|a| a.roi_frames.clone()), Some(vec![]));
    }

    #[test]
    fn parsed_source_matches_files_and_counts_unmatched_rows() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched_file = dir.path().join("matched.dcm");
        let unmatched_file = dir.path().join("unmatched.dcm");

        write_csv(
            &csv_path,
            &format!(
                "anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{matched},1,\"[[1,2,3,4]]\",\"[[0]]\"\n{unmatched},1,\"[[5,6,7,8]]\",\"[[0]]\"\n",
                matched = matched_file.display(),
                unmatched = unmatched_file.display(),
            ),
        );

        let source = AnnotationSource::from_path(&csv_path).expect("source parses");
        let matched = file_entry(0, matched_file.clone(), 1);

        let (mapped, report) = source
            .load_for_files_with_check(&[matched], || Ok(()))
            .expect("single-file match succeeds");
        let annotations = mapped.get(&0).expect("matched annotations");

        assert_eq!(annotations.roi_coords, vec![[1, 2, 3, 4]]);
        assert_eq!(report.matched_rows, 1);
        assert_eq!(report.unmatched_rows, 1);
    }

    #[test]
    fn errors_when_required_column_is_missing() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        write_csv(
            &csv_path,
            "anon_dicom_path,num_ROI,ROI_frames\n/path/one.dcm,1,\"[]\"\n",
        );

        let error =
            load_annotations_for_files(&csv_path, &[]).expect_err("missing header should fail");
        assert!(error
            .to_string()
            .contains("missing required column `ROI_coords`"));
    }

    #[test]
    fn errors_when_num_roi_and_coords_count_do_not_align() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched = PathBuf::from("/path/one.dcm");
        write_csv(
			&csv_path,
			"anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n/path/one.dcm,2,\"[[1,2,3,4]]\",\"[]\"\n",
		);

        let error = load_annotations_for_files(&csv_path, &[file_entry(0, matched, 1)])
            .expect_err("mismatched ROI count should fail");
        assert!(error
            .to_string()
            .contains("len(ROI_coords) must equal num_ROI"));
    }

    #[test]
    fn errors_when_roi_frames_length_does_not_match_num_roi() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched = PathBuf::from("/path/one.dcm");
        write_csv(
			&csv_path,
			"anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n/path/one.dcm,2,\"[[1,2,3,4],[5,6,7,8]]\",\"[[0]]\"\n",
		);

        let error = load_annotations_for_files(&csv_path, &[file_entry(0, matched, 1)])
            .expect_err("mismatched frame groups should fail");
        assert!(error
            .to_string()
            .contains("len(ROI_frames) must equal num_ROI when ROI_frames is not empty"));
    }

    #[test]
    fn errors_when_frame_index_exceeds_matched_file_frame_count() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let matched_file = dir.path().join("matched.dcm");
        write_csv(
            &csv_path,
            &format!(
				"anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{matched},1,\"[[1,2,3,4]]\",\"[[3]]\"\n",
				matched = matched_file.display(),
			),
        );

        let files = vec![file_entry(0, matched_file, 3)];
        let error = load_annotations_for_files(&csv_path, &files)
            .expect_err("out-of-range frame should fail");
        assert!(error.to_string().contains("contains frame 3"));
        assert!(error.to_string().contains("3 frame(s)"));
    }

    #[test]
    fn errors_on_duplicate_anon_dicom_paths() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let duplicate_path = dir.path().join("dup.dcm");
        write_csv(
			&csv_path,
			&format!(
				"anon_dicom_path,num_ROI,ROI_coords,ROI_frames\n{path},1,\"[[1,2,3,4]]\",\"[]\"\n{path},1,\"[[5,6,7,8]]\",\"[]\"\n",
				path = duplicate_path.display(),
			),
		);

        let error = load_annotations_for_files(&csv_path, &[file_entry(0, duplicate_path, 1)])
            .expect_err("duplicate path should fail");
        assert!(error
            .to_string()
            .contains("duplicate matching anon_dicom_path"));
    }

    #[test]
    fn matches_relative_csv_path_to_absolute_loaded_path() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let relative = PathBuf::from("dataset").join("case.dcm");
        let absolute = std::env::current_dir()
            .expect("current directory")
            .join(&relative);
        write_csv(
            &csv_path,
            &format!(
                "anon_dicom_path,ROI_coords\n{},\"[[1,2,3,4]]\"\n",
                relative.display()
            ),
        );

        let mapped = load_annotations_for_files(&csv_path, &[file_entry(0, absolute, 1)])
            .expect("relative path should match absolute file");

        assert_eq!(mapped.get(&0).map(|value| value.num_roi), Some(1));
    }

    #[test]
    fn matches_absolute_csv_path_to_relative_loaded_path_with_parent_components() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let relative = PathBuf::from("dataset")
            .join("nested")
            .join("..")
            .join("case.dcm");
        let absolute = std::env::current_dir()
            .expect("current directory")
            .join("dataset")
            .join("case.dcm");
        write_csv(
            &csv_path,
            &format!(
                "anon_dicom_path,ROI_coords\n{},\"[[1,2,3,4]]\"\n",
                absolute.display()
            ),
        );

        let mapped = load_annotations_for_files(&csv_path, &[file_entry(0, relative, 1)])
            .expect("absolute path should match normalized relative file");

        assert_eq!(mapped.get(&0).map(|value| value.num_roi), Some(1));
    }

    #[test]
    fn ignores_malformed_roi_payload_for_unmatched_rows() {
        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        write_csv(
            &csv_path,
            "anon_dicom_path,ROI_coords\n/unmatched/file.dcm,not-json\n",
        );

        let mapped = load_annotations_for_files(&csv_path, &[])
            .expect("unmatched payload should not be parsed");

        assert!(mapped.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn matches_real_csv_path_to_symlinked_loaded_file() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let real_path = dir.path().join("real.dcm");
        let symlink_path = dir.path().join("linked.dcm");
        fs::write(&real_path, []).expect("create target file");
        symlink(&real_path, &symlink_path).expect("create symlink");
        let canonical_real_path = real_path.canonicalize().expect("canonical target path");
        write_csv(
            &csv_path,
            &format!(
                "anon_dicom_path,ROI_coords\n{},\"[[1,2,3,4]]\"\n",
                canonical_real_path.display()
            ),
        );

        let mapped = load_annotations_for_files(&csv_path, &[file_entry(0, symlink_path, 1)])
            .expect("canonical file alias should match");

        assert_eq!(mapped.get(&0).map(|value| value.num_roi), Some(1));
    }

    #[cfg(unix)]
    #[test]
    fn matches_symlinked_csv_path_to_real_loaded_file() {
        use std::os::unix::fs::symlink;

        let dir = tempdir().expect("temp dir");
        let csv_path = dir.path().join("annotations.csv");
        let real_path = dir.path().join("real.dcm");
        let symlink_path = dir.path().join("linked.dcm");
        fs::write(&real_path, []).expect("create target file");
        symlink(&real_path, &symlink_path).expect("create symlink");
        write_csv(
            &csv_path,
            &format!(
                "anon_dicom_path,ROI_coords\n{},\"[[1,2,3,4]]\"\n",
                symlink_path.display()
            ),
        );

        let mapped = load_annotations_for_files(&csv_path, &[file_entry(0, real_path, 1)])
            .expect("symlinked CSV alias should match canonical loaded file");

        assert_eq!(mapped.get(&0).map(|value| value.num_roi), Some(1));
    }

    #[test]
    fn canonicalizes_coords_and_derives_roi_count() {
        let annotations = EmbedRoiAnnotations {
            num_roi: 99,
            roi_coords: vec![[30, 40, 10, 20]],
            roi_frames: vec![vec![0, 1]],
        };

        let canonical =
            canonicalize_annotations(annotations, 100, 100, 2).expect("canonical annotations");

        assert_eq!(
            canonical,
            EmbedRoiAnnotations {
                num_roi: 1,
                roi_coords: vec![[10, 20, 30, 40]],
                roi_frames: vec![vec![0, 1]],
            }
        );
    }

    #[test]
    fn rejects_edit_coords_outside_image_bounds() {
        let annotations = EmbedRoiAnnotations {
            num_roi: 1,
            roi_coords: vec![[0, 0, 2, 1]],
            roi_frames: vec![],
        };

        let error = canonicalize_annotations(annotations, 1, 1, 1).expect_err("bounds should fail");

        assert!(error.to_string().contains("exceeds image bounds"));
    }

    fn file_entry(index: usize, path: PathBuf, frame_count: u32) -> Arc<FileEntry> {
        Arc::new(FileEntry {
            format: Default::default(),
            raster: None,
            size_bytes: 0,
            modified: None,
            index,
            path,
            label: "fixture".to_string(),
            patient_id: String::new(),
            patient_name: String::new(),
            study_instance_uid: String::new(),
            study_date: String::new(),
            study_description: String::new(),
            series_instance_uid: String::new(),
            series_number: String::new(),
            series_description: String::new(),
            modality: String::new(),
            instance_number: String::new(),
            sop_instance_uid: String::new(),
            sop_class_uid: "1.2.840.10008.5.1.4.1.1.2".to_string(),
            series_metadata: Default::default(),
            has_pixels: true,
            frame_count,
            rows: 1,
            columns: 1,
            bits_allocated: 8,
            pixel_representation: 0,
            samples_per_pixel: 1,
            photometric_interpretation: "MONOCHROME2".to_string(),
            rescale_slope: 1.0,
            rescale_intercept: 0.0,
            transfer_syntax_uid: "1.2.840.10008.1.2.1".to_string(),
            default_window: None,
        })
    }

    fn write_csv(path: &Path, content: &str) {
        fs::write(path, content).expect("write csv");
    }
}

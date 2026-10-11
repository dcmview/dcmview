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
//! | this file | The `--annotations` file ([`AnnotationInput`]): which format it is, the EMBED CSV matched to the loaded files by path ([`AnnotationSource`]), and the check a replaced ROI list must pass ([`canonicalize_annotations`]) |
//!
//! Nothing here knows the file registry: records are addressed by file key
//! and the caller says which files exist and how large they are. Joining a
//! request to the registry (settling a file's key, translating the keys a
//! masked session shows) is `server::annotations`.
//!
//! The formats themselves (reading and writing EMBED ROI CSV and the native
//! JSON and JSON Lines documents, and the report each import and export
//! leaves) are the `dcmview-adapters` crate. This module and
//! `server::annotations` hand it the session's files and store.
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
    MemoryBackend, MemoryConfig, DEFAULT_LAYER_ID, DEFAULT_LAYER_NAME, DELETED_BYTES,
    REMEMBERED_OPS, REMEMBERED_REVS,
};
pub use store::{
    default_document_budget, session_author, AnnotationStore, DownloadHold, ImportStatus,
    DOWNLOAD_BUDGET_BYTES, EMBED_IMPORT_AUTHOR, NATIVE_IMPORT_AUTHOR,
};

use crate::types::FileEntry;
use anyhow::{bail, Context, Result};
use dcmview_adapters::{FileResolver, Registry, Report, Resolved};
use dcmview_annotation::ImageSize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PathKey(PathBuf);

/// A loaded file as a row of an EMBED CSV can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathTarget {
    index: usize,
    size: ImageSize,
    size_bytes: u64,
}

/// The ROI rows an EMBED CSV holds for the loaded files, by file index, as
/// the CSV wrote them.
pub type AnnotationIndexMap = HashMap<usize, EmbedRoiAnnotations>;

/// The format of an `--annotations` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnnotationFormat {
    /// EMBED ROI CSV (`dcmview_adapters::embed`).
    Embed,
    /// A `dcmview.annotations` document, as JSON or as JSON Lines
    /// (`dcmview_adapters::native`).
    Native,
}

/// The `--annotations` file of a session, checked as far as it can be
/// before the viewer serves.
#[derive(Debug, Clone)]
pub enum AnnotationInput {
    Embed(AnnotationSource),
    Native(NativeSource),
}

impl AnnotationInput {
    /// Opens the `--annotations` file at `path`.
    ///
    /// The format is `format` when one is given (`--annotations-format`).
    /// Otherwise the start of the file decides
    /// (`dcmview_adapters::Registry::sniff` over the built-in adapters, on
    /// at most `dcmview_adapters::limits::SNIFF_BYTES`): a file the native
    /// adapter is certain of (it starts with `{`, after an optional byte
    /// order mark and whitespace, and names the format
    /// `"dcmview.annotations"` in those first bytes) is
    /// [`AnnotationFormat::Native`], and every other file is
    /// [`AnnotationFormat::Embed`]. So anything that is not recognizably a
    /// native document, a JSON file of another kind included, is read as
    /// the EMBED CSV it always was and stops the process as one does, with
    /// status 1 and the message about its columns. A document whose
    /// `format` member is not in its first 4,096 bytes needs
    /// `--annotations-format native`.
    ///
    /// An EMBED CSV has its header checked here
    /// ([`AnnotationSource::from_path`]). A native file is only opened: it
    /// is read after discovery, and what is wrong with it fails the import
    /// then, with the viewer serving.
    pub fn from_path(path: &Path, format: Option<AnnotationFormat>) -> Result<Self> {
        let format = match format {
            Some(format) => format,
            None => sniff_format(path)?,
        };
        match format {
            AnnotationFormat::Embed => AnnotationSource::from_path(path).map(Self::Embed),
            AnnotationFormat::Native => NativeSource::from_path(path).map(Self::Native),
        }
    }
}

fn sniff_format(path: &Path) -> Result<AnnotationFormat> {
    let mut head = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(dcmview_adapters::limits::SNIFF_BYTES as u64)
                .read_to_end(&mut head)
        })
        .with_context(|| format!("failed to open annotations file: {}", path.display()))?;
    // Only a file that says it is a document is one. A file that merely
    // starts like JSON is read as the CSV it always was, and fails as one.
    let native = Registry::builtin()
        .sniff(&head)
        .is_some_and(|(adapter, confidence)| {
            adapter.info().id == dcmview_adapters::native::NATIVE_ADAPTER_ID
                && confidence == dcmview_adapters::Confidence::Certain
        });
    Ok(if native {
        AnnotationFormat::Native
    } else {
        AnnotationFormat::Embed
    })
}

/// A native `--annotations` file: where it is. It is read by
/// `server::annotations::import_native`.
#[derive(Debug, Clone)]
pub struct NativeSource {
    path: PathBuf,
}

impl NativeSource {
    /// Checks that the file at `path` can be opened and remembers its
    /// absolute path.
    pub fn from_path(path: &Path) -> Result<Self> {
        let working_directory =
            env::current_dir().context("failed to resolve current directory")?;
        let path = absolute_path(path, &working_directory);
        File::open(&path)
            .with_context(|| format!("failed to open annotations file: {}", path.display()))?;
        Ok(Self { path })
    }

    /// The file's absolute path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// An EMBED CSV whose header was checked.
#[derive(Debug, Clone)]
pub struct AnnotationSource {
    csv_path: PathBuf,
    working_directory: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnotationLoadReport {
    pub matched_rows: usize,
    pub unmatched_rows: usize,
}

/// What an EMBED CSV holds for the loaded files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationRead {
    /// The rows by file index, as the import reads them
    /// (`dcmview_adapters::embed::read_rows`, "Lenient coordinates").
    pub rows: AnnotationIndexMap,
    /// What the import did, corrected and kept as read.
    pub report: Report,
}

impl AnnotationSource {
    /// Opens the CSV at `csv_path` and checks its header
    /// (`dcmview_adapters::embed::check_header`): the file must exist and
    /// hold the columns `anon_dicom_path` and `ROI_coords`.
    pub fn from_path(csv_path: &Path) -> Result<Self> {
        let working_directory =
            env::current_dir().context("failed to resolve current directory")?;
        let csv_path = absolute_path(csv_path, &working_directory);
        let mut file = File::open(&csv_path)
            .with_context(|| format!("failed to open annotations CSV: {}", csv_path.display()))?;
        dcmview_adapters::embed::check_header(&mut file)?;
        Ok(Self {
            csv_path,
            working_directory,
        })
    }

    pub fn load_for_files(&self, files: &[Arc<FileEntry>]) -> Result<AnnotationIndexMap> {
        self.load_for_files_with_check(files, || Ok(()))
            .map(|(map, _)| map)
    }

    pub fn load_for_files_with_check<F>(
        &self,
        files: &[Arc<FileEntry>],
        check_active: F,
    ) -> Result<(AnnotationIndexMap, AnnotationLoadReport)>
    where
        F: FnMut() -> Result<()>,
    {
        let check_active = RefCell::new(check_active);
        let stopped = RefCell::new(None);
        let read = self.read_for_files(files, &|| match (check_active.borrow_mut())() {
            Ok(()) => true,
            Err(error) => {
                *stopped.borrow_mut() = Some(error);
                false
            }
        });
        if let Some(error) = stopped.into_inner() {
            return Err(error);
        }
        let read = read?;
        let report = AnnotationLoadReport {
            matched_rows: read
                .report
                .matched_by
                .map_or(0, |matched| matched.path as usize),
            unmatched_rows: read.report.counts.unmatched as usize,
        };
        Ok((read.rows, report))
    }

    /// Reads the CSV for `files`: the rows whose path names one of them,
    /// by file index, and the import's report. All or nothing.
    ///
    /// A row names a file by its normalized absolute path, a relative one
    /// resolved against the working directory the viewer was started in,
    /// or by the canonical form of either side (a link and its target are
    /// one file). A path can name several loaded files; each gets the
    /// row. Reading, the lenient coordinates and every bound are
    /// `dcmview_adapters::embed::read_rows`; its error is returned with its
    /// own text. `keep_going` is asked for each row; `false` ends the read.
    pub fn read_for_files(
        &self,
        files: &[Arc<FileEntry>],
        keep_going: &dyn Fn() -> bool,
    ) -> Result<AnnotationRead> {
        let lookup = FileLookup::new(files, self.working_directory.clone())?;
        let mut file = File::open(&self.csv_path).with_context(|| {
            format!(
                "failed to open annotations CSV: {}",
                self.csv_path.display()
            )
        })?;
        let read = dcmview_adapters::embed::read_rows(&mut file, &lookup, keep_going)?;
        let rows = read
            .files
            .into_iter()
            .map(|file| {
                (
                    file.slot,
                    EmbedRoiAnnotations {
                        num_roi: file.rois.roi_coords.len(),
                        roi_coords: file.rois.roi_coords,
                        roi_frames: file.rois.roi_frames,
                    },
                )
            })
            .collect();
        Ok(AnnotationRead {
            rows,
            report: read.report,
        })
    }
}

pub fn load_annotations_for_files(
    csv_path: &Path,
    files: &[Arc<FileEntry>],
) -> Result<AnnotationIndexMap> {
    AnnotationSource::from_path(csv_path)?.load_for_files(files)
}

/// The loaded files by the paths that name them: each by its normalized
/// absolute path and by the canonical form of that path, so a link and its
/// target are one file.
pub(crate) struct FileLookup {
    targets: HashMap<PathKey, Vec<PathTarget>>,
    /// What a relative path is resolved against.
    base: PathBuf,
}

impl FileLookup {
    /// A lookup over `files`, in which a relative path is taken below
    /// `base`.
    pub(crate) fn new(files: &[Arc<FileEntry>], base: PathBuf) -> Result<Self> {
        Ok(Self {
            targets: build_file_lookup(files, &base)?,
            base,
        })
    }

    fn targets(&self, path: &str) -> &[PathTarget] {
        let key = PathKey(absolute_path(Path::new(path), &self.base));
        matching_file_targets(&self.targets, &key).map_or(&[], Vec::as_slice)
    }

    /// The catalog indexes of the files `path` names, in the order the
    /// files were loaded; empty when it names none.
    pub(crate) fn indexes(&self, path: &str) -> Vec<usize> {
        self.targets(path)
            .iter()
            .map(|target| target.index)
            .collect()
    }
}

impl FileResolver for FileLookup {
    fn by_path(&self, path: &str, out: &mut Vec<Resolved>) {
        out.extend(self.targets(path).iter().map(|target| Resolved {
            slot: target.index,
            // The rows are staged by index: no key is settled for them.
            key: None,
            size: target.size,
            size_bytes: target.size_bytes,
        }));
    }
}

/// The loaded files by the end of their paths: which of them a relative
/// path of a document (`FileRef::path`) may name.
///
/// A document writes a file's path below the deepest directory that held
/// every file of the session that wrote it ([`common_directory`]). The
/// session that reads it may have loaded a subfolder of that directory, a
/// parent of it, or the same files somewhere else, so the path is not
/// resolved against any directory: a file is named by it when the file's
/// own path **ends with every component of it**. `sub/case/lcc.dcm` names
/// `/data/sub/case/lcc.dcm` and `/mnt/copy/sub/case/lcc.dcm`, and not
/// `/data/other/case/lcc.dcm`. One component is enough to name a file,
/// and several files can be named; a path only finds candidates, and what
/// is placed on which of them is decided by comparing the file with the
/// reference's key (`server::annotations::import_native`).
pub(crate) struct PathSuffixes<'a> {
    files: &'a [Arc<FileEntry>],
    /// Places in `files`, ordered by their paths read from the last
    /// component to the first, so the files a suffix names are neighbours.
    order: Vec<usize>,
    examined: std::cell::Cell<u64>,
}

impl<'a> PathSuffixes<'a> {
    /// A lookup over `files`. It holds one number a file and is built by
    /// sorting them once.
    pub(crate) fn new(files: &'a [Arc<FileEntry>]) -> Self {
        let mut order: Vec<usize> = (0..files.len()).collect();
        order.sort_by(|a, b| {
            files[*a]
                .path
                .components()
                .rev()
                .cmp(files[*b].path.components().rev())
        });
        Self {
            files,
            order,
            examined: std::cell::Cell::new(0),
        }
    }

    /// The catalog indexes of the files whose path ends with `path`, in
    /// ascending order. Empty for a path that is absolute, is empty, or
    /// steps upwards (`..`); `.` components are passed over.
    ///
    /// **Cost.** A lookup compares the path with at most
    /// `2 + log2(files)` files to find where its files start, and then
    /// with each file it returns and one more. It never visits the other
    /// files that share a file name with it, so looking up every file of a
    /// tree whose files are all named alike is not quadratic.
    #[cfg(test)]
    pub(crate) fn indexes(&self, path: &str) -> Vec<usize> {
        self.matching(path, usize::MAX).unwrap_or_default()
    }

    fn matching(&self, path: &str, limit: usize) -> Option<Vec<usize>> {
        let mut wanted = Vec::new();
        for component in Path::new(path).components() {
            match component {
                Component::CurDir => {}
                Component::Normal(part) => wanted.push(Component::Normal(part)),
                _ => return Some(Vec::new()),
            }
        }
        if wanted.is_empty() {
            return Some(Vec::new());
        }
        // How the end of a file's path stands to the wanted components.
        let compare = |place: &usize| {
            self.examined.set(self.examined.get() + 1);
            let mut theirs = self.files[*place].path.components().rev();
            for part in wanted.iter().rev() {
                match theirs.next() {
                    None => return std::cmp::Ordering::Less,
                    Some(component) => match component.cmp(part) {
                        std::cmp::Ordering::Equal => {}
                        other => return other,
                    },
                }
            }
            std::cmp::Ordering::Equal
        };
        let start = self
            .order
            .partition_point(|place| compare(place) == std::cmp::Ordering::Less);
        let mut indexes: Vec<usize> = self.order[start..]
            .iter()
            .take(limit.saturating_add(1))
            .take_while(|place| compare(place) == std::cmp::Ordering::Equal)
            .map(|place| self.files[*place].index)
            .collect();
        if indexes.len() > limit {
            return None;
        }
        indexes.sort_unstable();
        Some(indexes)
    }

    /// [`PathSuffixes::indexes`] when the path names at most `limit` files,
    /// and `None` when it names more, found at the cost of `limit` files.
    pub(crate) fn at_most(&self, path: &str, limit: usize) -> Option<Vec<usize>> {
        self.matching(path, limit)
    }

    /// How many files' paths the lookups so far were compared with.
    #[cfg(test)]
    pub(crate) fn examined(&self) -> u64 {
        self.examined.get()
    }
}

/// The directory a document's file paths are relative to: the deepest
/// directory that contains every one of `files`, by the components of the
/// paths as the registry holds them (`Path::components`, nothing resolved).
/// With one file it is that file's directory. `None` when `files` is empty
/// or the paths share no first component.
pub(crate) fn common_directory(files: &[Arc<FileEntry>]) -> Option<PathBuf> {
    let mut files = files.iter();
    let mut common: Vec<_> = files.next()?.path.parent()?.components().collect();
    for file in files {
        let parent = file.path.parent()?;
        let shared = common
            .iter()
            .copied()
            .zip(parent.components())
            .take_while(|(a, b)| a == b)
            .count();
        common.truncate(shared);
        if common.is_empty() {
            return None;
        }
    }
    if common.is_empty() {
        None
    } else {
        Some(common.into_iter().collect())
    }
}

/// How a document names the loaded files (`FileRef::path`): never by an
/// absolute path, and never by a path that starts above the folder the
/// files were loaded from.
///
/// Every file's path is first made absolute against `base` (the working
/// directory), without resolving links. The root is the deepest directory
/// that holds every file ([`common_directory`] of those paths), **unless**
/// that directory is the filesystem root, a drive, or a directory above the
/// home directory: then the files share nothing worth naming, and there is
/// no root. The home directory itself, and anything below it, is a root.
///
/// A file's path is its components below the root, each as text
/// (`to_string_lossy`), joined with `/`. Without a root, or for a file that
/// is not below it, the path is the file's name alone. A reader finds a
/// file by the end of its path ([`PathSuffixes`]) and compares its bytes
/// or its key, so a name is enough to find it again.
pub(crate) struct DocumentPaths {
    root: Option<PathBuf>,
    base: PathBuf,
}

impl DocumentPaths {
    /// The paths of `files`, with relative ones below `base` and `home`
    /// the home directory when there is one.
    pub(crate) fn new(files: &[Arc<FileEntry>], base: &Path, home: Option<&Path>) -> Self {
        let mut common: Option<Vec<Component<'_>>> = None;
        let absolute: Vec<PathBuf> = files
            .iter()
            .map(|file| absolute_path(&file.path, base))
            .collect();
        for path in &absolute {
            let parent: Vec<_> = path
                .parent()
                .map(|parent| parent.components().collect())
                .unwrap_or_default();
            common = Some(match common {
                None => parent,
                Some(mut common) => {
                    let shared = common
                        .iter()
                        .zip(&parent)
                        .take_while(|(a, b)| a == b)
                        .count();
                    common.truncate(shared);
                    common
                }
            });
        }
        let root = common
            .filter(|common| {
                common
                    .iter()
                    .any(|part| matches!(part, Component::Normal(_)))
            })
            .map(|common| common.into_iter().collect::<PathBuf>())
            .filter(|root| {
                // Above the home directory by the paths as written, or by
                // where they lead: a home directory reached through a link
                // is the same directory.
                let above = |home: &Path, root: &Path| home != root && home.starts_with(root);
                let led =
                    |path: &Path| path.canonicalize().unwrap_or_else(|_| normalize_path(path));
                home.is_none_or(|home| {
                    !above(&normalize_path(home), root) && !above(&led(home), &led(root))
                })
            });
        Self {
            root,
            base: base.to_path_buf(),
        }
    }

    /// The paths of a session's files: relative ones below the working
    /// directory, and the home directory as the environment gives it.
    pub(crate) fn of_session(files: &[Arc<FileEntry>]) -> Self {
        let base = std::env::current_dir().unwrap_or_default();
        Self::new(files, &base, std::env::home_dir().as_deref())
    }

    /// `path` as a document names it.
    pub(crate) fn path(&self, path: &Path) -> String {
        let absolute = absolute_path(path, &self.base);
        let below = self
            .root
            .as_deref()
            .and_then(|root| absolute.strip_prefix(root).ok())
            .map(|relative| {
                relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|relative| !relative.is_empty());
        below.unwrap_or_else(|| {
            path.file_name().map_or_else(
                || "file".to_string(),
                |name| name.to_string_lossy().into_owned(),
            )
        })
    }
}

fn build_file_lookup(
    files: &[Arc<FileEntry>],
    working_directory: &Path,
) -> Result<HashMap<PathKey, Vec<PathTarget>>> {
    let mut lookup = HashMap::<PathKey, Vec<PathTarget>>::new();
    for file in files {
        let target = PathTarget {
            index: file.index,
            size: ImageSize {
                columns: file.columns,
                rows: file.rows,
                frames: file.frame_count,
            },
            size_bytes: file.size_bytes,
        };
        let lexical = PathKey(absolute_path(&file.path, working_directory));
        push_unique_target(&mut lookup, lexical, target);
        if let Ok(canonical) = file.path.canonicalize() {
            push_unique_target(&mut lookup, PathKey(normalize_path(&canonical)), target);
        }
    }
    Ok(lookup)
}

fn push_unique_target(
    lookup: &mut HashMap<PathKey, Vec<PathTarget>>,
    key: PathKey,
    target: PathTarget,
) {
    let targets = lookup.entry(key).or_default();
    if !targets.contains(&target) {
        targets.push(target);
    }
}

fn matching_file_targets<'a>(
    lookup: &'a HashMap<PathKey, Vec<PathTarget>>,
    path_key: &PathKey,
) -> Option<&'a Vec<PathTarget>> {
    lookup.get(path_key).or_else(|| {
        path_key
            .0
            .canonicalize()
            .ok()
            .and_then(|canonical| lookup.get(&PathKey(normalize_path(&canonical))))
    })
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

    #[test]
    fn the_common_directory_is_the_deepest_one_that_holds_every_file() {
        let at = |paths: &[&str]| {
            let files: Vec<Arc<FileEntry>> = paths
                .iter()
                .enumerate()
                .map(|(index, path)| file_entry(index, PathBuf::from(path), 1))
                .collect();
            super::common_directory(&files)
        };
        assert_eq!(at(&[]), None);
        assert_eq!(at(&["/data/a/one.dcm"]), Some(PathBuf::from("/data/a")));
        assert_eq!(
            at(&[
                "/data/a/one.dcm",
                "/data/a/deep/two.dcm",
                "/data/b/three.dcm"
            ]),
            Some(PathBuf::from("/data"))
        );
        // Whole components are compared, not characters.
        assert_eq!(
            at(&["/data/ab/one.dcm", "/data/abc/two.dcm"]),
            Some(PathBuf::from("/data"))
        );
        // A directory is not below itself: a file never counts as one.
        assert_eq!(
            at(&["/data/a/one.dcm", "/data/a/one.dcm"]),
            Some(PathBuf::from("/data/a"))
        );
        // Relative and absolute paths share no first component.
        assert_eq!(at(&["/data/one.dcm", "data/one.dcm"]), None);
    }

    #[test]
    fn a_document_path_is_written_below_the_root_with_slashes() {
        let root = PathBuf::from("/data").join("set");
        let files = vec![
            file_entry(0, root.join("case 1").join("lcc.dcm"), 1),
            file_entry(1, root.join("rcc.dcm"), 1),
        ];
        let names = super::DocumentPaths::new(&files, Path::new("/work"), None);
        assert_eq!(names.path(&files[0].path), "case 1/lcc.dcm");
        // Not below the root: the file's name.
        assert_eq!(names.path(Path::new("/other/mlo.dcm")), "mlo.dcm");
    }

    #[test]
    fn a_relative_path_names_the_files_whose_path_ends_with_it() {
        let files = vec![
            file_entry(0, PathBuf::from("/data/sub/case/lcc.dcm"), 1),
            file_entry(1, PathBuf::from("/data/other/case/lcc.dcm"), 1),
            file_entry(2, PathBuf::from("/mnt/copy/sub/case/lcc.dcm"), 1),
            file_entry(3, PathBuf::from("/data/sub/case/xlcc.dcm"), 1),
        ];
        let suffixes = super::PathSuffixes::new(&files);
        assert_eq!(suffixes.indexes("sub/case/lcc.dcm"), vec![0, 2]);
        assert_eq!(suffixes.indexes("./case/./lcc.dcm"), vec![0, 1, 2]);
        assert_eq!(suffixes.indexes("lcc.dcm"), vec![0, 1, 2]);
        // Whole components: a name is not the end of a longer name, nor a
        // folder the end of a longer folder.
        assert_eq!(suffixes.indexes("ub/case/lcc.dcm"), Vec::<usize>::new());
        assert_eq!(suffixes.indexes("data/sub/case/lcc.dcm"), vec![0]);
        // Longer than the file's own path, absolute, upwards or empty.
        for path in [
            "x/data/sub/case/lcc.dcm",
            "/data/sub/case/lcc.dcm",
            "sub/../sub/case/lcc.dcm",
            "",
            ".",
        ] {
            assert!(suffixes.indexes(path).is_empty(), "{path}");
        }
    }

    /// Looking up every file of a tree whose files are all named alike
    /// compares each path with a few files, not with every file of that
    /// name: counted, for 4,096 files.
    #[test]
    fn looking_up_files_that_share_a_name_is_not_quadratic() {
        const FILES: usize = 4_096;
        let files: Vec<_> = (0..FILES)
            .map(|n| {
                file_entry(
                    n,
                    PathBuf::from(format!("/data/patient-{n}/series/image.dcm")),
                    1,
                )
            })
            .collect();
        let suffixes = super::PathSuffixes::new(&files);
        for n in 0..FILES {
            assert_eq!(
                suffixes.indexes(&format!("patient-{n}/series/image.dcm")),
                vec![n]
            );
        }
        // 12 halvings, the file itself and the one after it: within 16 a
        // lookup, where visiting every file of the name is 4,096.
        assert!(
            suffixes.examined() <= 16 * FILES as u64,
            "{} comparisons for {FILES} lookups",
            suffixes.examined()
        );
        // A path that names them all costs one comparison each, and gives
        // them in index order.
        let before = suffixes.examined();
        assert_eq!(
            suffixes.indexes("series/image.dcm"),
            (0..FILES).collect::<Vec<_>>()
        );
        assert!(suffixes.examined() - before <= FILES as u64 + 16);
        // Asked for at most a few, a path that names more names none, at
        // the cost of those few.
        let before = suffixes.examined();
        assert_eq!(suffixes.at_most("series/image.dcm", 64), None);
        assert!(suffixes.examined() - before <= 64 + 16);
        assert_eq!(
            suffixes.at_most("patient-7/series/image.dcm", 1),
            Some(vec![7])
        );
    }

    #[test]
    fn a_document_never_names_a_file_by_an_absolute_path() {
        let paths = |files: &[&str], base: &str, home: Option<&str>| -> Vec<String> {
            let files: Vec<_> = files
                .iter()
                .enumerate()
                .map(|(index, path)| file_entry(index, PathBuf::from(path), 1))
                .collect();
            let names = super::DocumentPaths::new(&files, Path::new(base), home.map(Path::new));
            files.iter().map(|file| names.path(&file.path)).collect()
        };
        let home = Some("/home/reader");
        // One tree: below the deepest folder that holds every file.
        assert_eq!(
            paths(
                &["/data/set/a/one.dcm", "/data/set/b/two.dcm"],
                "/work",
                home
            ),
            ["a/one.dcm", "b/two.dcm"]
        );
        // A relative and an absolute argument for one tree are one tree.
        assert_eq!(
            paths(
                &["set/a/one.dcm", "/work/set/b/two.dcm", "./set/c.dcm"],
                "/work",
                home
            ),
            ["a/one.dcm", "b/two.dcm", "c.dcm"]
        );
        // Two trees that share only the root: file names.
        assert_eq!(
            paths(&["/alpha/a/one.dcm", "/beta/b/two.dcm"], "/work", home),
            ["one.dcm", "two.dcm"]
        );
        assert_eq!(
            paths(&["/alpha/a/one.dcm", "/beta/b/two.dcm"], "/work", None),
            ["one.dcm", "two.dcm"]
        );
        // Two trees that share only a directory above the home directory:
        // a path below it would start with the user's name.
        assert_eq!(
            paths(
                &["/home/reader/x/one.dcm", "/home/other/y/two.dcm"],
                "/work",
                home
            ),
            ["one.dcm", "two.dcm"]
        );
        // The home directory itself is a root, and so is anything below.
        assert_eq!(
            paths(
                &["/home/reader/x/one.dcm", "/home/reader/y/two.dcm"],
                "/work",
                home
            ),
            ["x/one.dcm", "y/two.dcm"]
        );
        // One file: its name.
        assert_eq!(paths(&["/one.dcm"], "/work", home), ["one.dcm"]);
        assert_eq!(paths(&["/data/one.dcm"], "/work", None), ["one.dcm"]);
        for path in paths(&["rel/one.dcm", "/abs/two.dcm", "/"], "/", None) {
            assert!(
                !Path::new(&path).is_absolute() && !path.is_empty(),
                "{path}"
            );
        }
    }

    /// A home directory that is reached through a link is still the home
    /// directory: files in two users' folders, loaded by the path the link
    /// leads to, share only a directory above it.
    #[cfg(unix)]
    #[test]
    fn a_linked_home_directory_is_the_directory_it_leads_to() {
        let dir = tempdir().expect("temp dir");
        let base = dir.path().canonicalize().expect("canonical");
        let users = base.join("volume").join("users");
        fs::create_dir_all(users.join("reader").join("x")).expect("folders");
        fs::create_dir_all(users.join("other").join("y")).expect("folders");
        fs::create_dir_all(base.join("home")).expect("folder");
        let home = base.join("home").join("reader");
        std::os::unix::fs::symlink(users.join("reader"), &home).expect("link");
        let files = vec![
            file_entry(0, users.join("reader").join("x").join("one.dcm"), 1),
            file_entry(1, users.join("other").join("y").join("two.dcm"), 1),
        ];
        let names = super::DocumentPaths::new(&files, &base, Some(&home));
        assert_eq!(names.path(&files[0].path), "one.dcm");
        assert_eq!(names.path(&files[1].path), "two.dcm");
        // Below the home directory itself, by either path, there is a root.
        let files = vec![
            file_entry(0, users.join("reader").join("x").join("one.dcm"), 1),
            file_entry(1, users.join("reader").join("two.dcm"), 1),
        ];
        let names = super::DocumentPaths::new(&files, &base, Some(&home));
        assert_eq!(names.path(&files[0].path), "x/one.dcm");
    }

    #[test]
    fn a_lookup_takes_a_relative_path_below_its_base() {
        let base = PathBuf::from("/data/set");
        let files = vec![
            file_entry(0, base.join("a").join("one.dcm"), 1),
            file_entry(1, base.join("b").join("one.dcm"), 1),
        ];
        let lookup = super::FileLookup::new(&files, base.clone()).expect("lookup");
        assert_eq!(lookup.indexes("b/one.dcm"), vec![1]);
        assert_eq!(lookup.indexes("/data/set/a/../a/one.dcm"), vec![0]);
        assert!(lookup.indexes("c/one.dcm").is_empty());
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

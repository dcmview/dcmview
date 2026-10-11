# dcmview Annotation Reference

`dcmview` can load, edit, and export rectangular ROI annotations in an
EMBED-style CSV format, and can write and reload everything a session holds
as a `dcmview.annotations` document. Annotations are kept in memory only: the
input file and source DICOM files are never modified.

`dcmview` is intended for research and development inspection, not clinical
diagnosis. Annotation CSVs can contain local paths and case identifiers; do not
share them publicly unless they are fully de-identified and approved for public
use.

## Loading Annotations

Pass an annotation file with `--annotations`:

```bash
dcmview --annotations ./embed_annotations.csv ./study_dir
dcmview --annotations ./dcmview-annotations.json ./study_dir
```

The file is an EMBED-style CSV (below) or a
[`dcmview.annotations` document](#documents), as JSON or JSON Lines. The
start of the file decides which, whatever it is named: a file that starts
with `{` (after an optional byte order mark) and names the
`dcmview.annotations` format in its first 4,096 bytes is read as a
document, and every other file as a CSV. A file that is neither stops the
viewer with exit status 1 before it serves, as a CSV without the required
columns always has; a document that turns out not to be loadable does not
stop the viewer, and `GET /api/annotations/import` says why it failed.
`--annotations-format embed` or `--annotations-format native` reads it as the
format named instead.

Python wrapper usage:

```python
from dcmview_py import view

view("./study_dir", annotations="./embed_annotations.csv")
```

Viewer edits replace the in-memory annotations for the current file. Use
**Export ROIs** in the viewer to download a fresh CSV generated from current
in-memory state.

## CSV Columns

Required columns:

| Column | Meaning |
|---|---|
| `anon_dicom_path` | Path to the DICOM file the row describes. |
| `ROI_coords` | JSON array of rectangular boxes. |

Optional columns:

| Column | Meaning |
|---|---|
| `num_ROI` | Number of ROIs in the row. When present, it must equal the number of coordinate boxes. |
| `ROI_frames` | JSON array of frame-index lists. When omitted or `[]`, ROIs apply to all frames. |

Extra columns are ignored when loading and are not preserved when exporting.

## Coordinate Format

`ROI_coords` is a JSON array of `[ymin, xmin, ymax, xmax]` boxes:

```json
[[120, 340, 220, 430], [400, 510, 480, 590]]
```

Coordinates are image pixel indices: non-negative whole numbers. Two kinds
of value are corrected on load, and each corrected box is listed in the
[import's report](#the-imports-report): a negative coordinate is read as 0,
and a coordinate that is not a whole number is rounded to the nearest pixel
(halves away from zero). A whole number written as `20.0` or `2e1` is that
number, and `-0` and `-0.0` are 0 and are not listed.

A box should be inside image bounds with `ymax`/`xmax` greater than
`ymin`/`xmin`. Loading does not enforce this: such a box is kept and
exported exactly as written and listed in the report, but the viewer refuses
to save an edit to a file whose boxes break either rule.

`ROI_frames` is a JSON array with one frame-index list per ROI:

```json
[[0, 1, 2], [5, 6]]
```

Frame indices are zero-based and must be less than `NumberOfFrames`. Empty
frame lists mean the ROI applies to all frames.

JSON-valued fields that contain a comma must be CSV-quoted. A value without
one, such as `[]` or `[[0]]`, needs no quotes, and export writes it unquoted.

## Example

```csv
anon_dicom_path,num_ROI,ROI_coords,ROI_frames
/path/to/dbt_case.dcm,2,"[[120,340,220,430],[400,510,480,590]]","[[0,1,2],[5,6]]"
/path/to/ffdm_case.dcm,1,"[[80,150,190,260]]",[]
```

Matching uses normalized absolute path equality against loaded DICOM paths.
Relative paths are resolved from the directory where `dcmview` was started;
`.` and `..` components are normalized. Loaded files also receive a
best-effort canonical alias so a relative or symlinked CLI path can match the
corresponding absolute CSV path. If a CSV row does not match any loaded DICOM
path, that row is ignored without parsing its ROI payload.

Annotation CSV ingestion runs after DICOM discovery on a blocking worker and
does not delay server startup or image rendering. Annotation reads and export
wait for the single streaming CSV pass to finish. Viewer edits remain available
while the CSV is loading and are never overwritten by the later import.

## Validation Errors

`dcmview` validates the CSV header at startup and validates matching rows during
background ingestion. Common failures include:

- Missing `anon_dicom_path` or `ROI_coords`.
- Invalid JSON in `ROI_coords` or `ROI_frames`.
- `num_ROI` not matching the number of coordinate boxes.
- A coordinate that is not a number, or is above 4,294,967,295.
- A frame index outside the file's frame range.
- A different number of `ROI_coords` and `ROI_frames` entries.

If a matching row is invalid, no CSV annotations are committed. Image viewing
continues, while annotation reads and export return the validation error. Bad
ROI payloads on unmatched dataset-level rows are intentionally ignored.

A CSV is read within fixed bounds, and one past a bound fails the same way: a
row of at most 16 MiB, at most 65,536 ROIs in a row and 65,536 frames for a
ROI, and at most 16,777,216 rows.

## The Import's Report

Every import leaves a report of what it did, at
`GET /api/annotations/import`: how many rows matched a loaded file and how
many did not, and, by CSV row and position in the row, every box that was
corrected (`negative_coordinate`, `non_integer_coordinate`) or kept although
it is unusual (`out_of_range`, `zero_area`, `corners_out_of_order`,
`frames_empty`). The viewer also prints one line for each kind to standard
error. The report holds row numbers and counts, never a path or a value of
the file.

## Exported Row Order

**Export ROIs** writes one row for each file that has a ROI, sorted by path
(the byte order of the path as written), so two exports of the same
annotations are the same file.

## Documents

A session's annotations can also be downloaded as one document of the
annotation model, which holds everything the CSV cannot: every shape, layer
and label, and a description of each annotated file.

- `GET /api/annotations/export.json`: one JSON document.
- `GET /api/annotations/export.jsonl`: the same, a record a line, for large
  sessions (a JSON document is limited to 256 MiB).

A document names each file by its key, its SOP Instance UID and its path
below the deepest directory that holds every loaded file, so it can be
loaded again with `--annotations` after the files have moved or the viewer
runs on another machine. It never holds an absolute path: files loaded
from folders that have nothing in common but the filesystem root, or a
directory above your home directory, are named by their file names.

**What a reload promises.** An annotation is put on a loaded file only
after that file was compared with the key the document names it by: its
SOP Instance UID for a `sop:` key, the BLAKE3 digest of its bytes for a
`b3:` key. The key, the UID and the path only find the files to compare:

- by path, a file is found when its own path ends with the document's
  path, so loading a subfolder of the exported folder, a parent of it, or
  a copy of it elsewhere finds the same files. One path component (the
  file name) is enough to find a file; of several files a path finds, the
  one that passes the comparison takes the annotations. A path that names
  more than 64 loaded files is too vague to find a file by, and so is a
  UID that more than 64 loaded files carry; a file that only such a path
  or UID could find is reported as `file_ambiguous`;
- by content, when the document names a file by a `b3:` key and no key,
  UID or path finds it: among the loaded files of the same byte size,
  dimensions and format, the one whose bytes have that digest. So a copy
  of an annotated file is found wherever it is and whatever it is called;
- a file found that holds other bytes, or carries another UID, is refused
  (`evidence_conflict`), and so is one that can no longer be read to
  compare (`file_unverified`) or has other dimensions (`size_mismatch`).
  Its annotations are counted in the report and left out, as are those of
  a file that is not loaded (`file_unmatched`);
- comparing a `b3:` key hashes the file, which a large file takes a moment
  for; the import's state is `loading` meanwhile. The files hashed are
  those a document's keys, UIDs and paths find and, when it must search by
  content, the loaded files of the same byte size and dimensions as the
  file it looks for, each once. No other file is read;
- a `sop:` key says which UID the file has, so a file named by one is
  matched by its UID, unless the document also records a `b3:` digest for
  it, which is then compared too. A digest of another kind (`sha256:`) is
  not compared.

A document path longer than 4,096 bytes is not valid and fails the import.
A path in an EMBED CSV that is longer than that names no file and is
counted as unmatched.

**Files with the same bytes** share a key, and each keeps the ROIs that
were written for it: the document marks such a record with the path of its
copy (`extensions.embed.row_of`), and a reload shows on each copy what it
showed before. When that copy is not loaded, its ROIs are left out and
counted as unmatched. An annotation that was drawn on the image shows on
every copy, before and after a reload; a copy that is the only one loaded
is found by its content although the document names the other.

**Masked sessions.** A session started with `--mask` writes a document
that names files by the digest of their bytes and by nothing else: no UID
or patient id, real or masked, because the masks are different in every
run; no folder or file name, which often say whose the files are (a file
is `File 1`, as the masked viewer shows it); and no user name. What is
left of a file is its content digest, byte size, SOP class, format, rows,
columns and frame count. Labels on a patient, study, series or folder are
left out. It can be loaded into any session, masked or not:
each file is found by its content, which hashes the loaded files of the
same size and dimensions. The annotated files are read to make the
document.

A masked document does not say which of several byte-identical files a
ROI was on: it has no path to say it with, and such files differ in
nothing else. It says only that the ROI was on one of that many copies.
When the document is loaded, those ROIs are put on the copy whose path
sorts first, the same one every time, and the import's report counts
them as `copy_placement_not_kept`. To keep each copy's own ROIs, export
from a session that is not masked.

A JSON Lines file that was cut off, at any line, is not loaded.

A document is read within bounds: 1,048,576 JSON values in one annotation
(the largest mask and the largest polygon the model allows are within
that), 131,072 of them objects or arrays, 64 levels of nesting, and 4 GiB
for the memory all its records are charged, whatever the size of the file.
A file past one of them fails to load and the import's message says
which. The viewer counts the same way when it writes: a session whose
document would be past the 4 GiB is refused its export, so whatever the
viewer exports it can load again. A rectangle is charged about 1,900 bytes
and an annotated file about 1,300.

One rule about empty documents: a document on a single line that holds no
file, layer, annotation or label is taken for a JSON Lines file that was
cut off after its first line, and is not loaded. The same empty document
written over several lines is loaded, and brings nothing. No document the
viewer writes is empty, because a session always has a layer.

Loading a document is all or nothing: a record that breaks a rule of the
annotation model fails the whole import and the report names its line. One
exception keeps a session reloadable: a box that an EMBED CSV import kept as
written (past the image edge, without area, corners out of order) is taken
back as written, and listed in the report.

Loaded records keep their ids, layers, classes, shapes, frames and
attributes. The session that loads them stamps them as its own import
(`import:native`), so the original author and times are not kept.

Exporting a document needs the key of every file that has ROIs. For a file
that must be hashed first the download waits; when a file can no longer be
read, the document export answers an error naming it, and the CSV export
still works.

For symptom-oriented fixes, see the
[troubleshooting guide](troubleshooting.md#annotation-csv-fails-to-load).

## API Shape

The viewer-internal HTTP API represents annotations as:

```json
{
  "num_roi": 2,
  "roi_coords": [[120, 340, 220, 430], [400, 510, 480, 590]],
  "roi_frames": [[0, 1, 2], [5, 6]]
}
```

`PUT /api/file/{index}/annotations` replaces one file's in-memory annotations and
returns the canonicalized payload. Invalid coordinates or frame mappings return
`400 {"code": "bad_request", "error": "..."}`. See the [internal API reference](api.md) for endpoint
details.

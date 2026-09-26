# dcmview Annotation Reference

`dcmview` can load, edit, and export rectangular ROI annotations in an
EMBED-style CSV format. Annotations are kept in memory only: the input CSV and
source DICOM files are never modified.

`dcmview` is intended for research and development inspection, not clinical
diagnosis. Annotation CSVs can contain local paths and case identifiers; do not
share them publicly unless they are fully de-identified and approved for public
use.

## Loading Annotations

Pass an annotation CSV with `--annotations`:

```bash
dcmview --annotations ./embed_annotations.csv ./study_dir
```

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

Coordinates are image pixel indices. Each box must be inside image bounds, and
`ymax`/`xmax` must be greater than `ymin`/`xmin`.

`ROI_frames` is a JSON array with one frame-index list per ROI:

```json
[[0, 1, 2], [5, 6]]
```

Frame indices are zero-based and must be less than `NumberOfFrames`. Empty
frame lists mean the ROI applies to all frames.

JSON-valued fields must be CSV-quoted.

## Example

```csv
anon_dicom_path,num_ROI,ROI_coords,ROI_frames
/path/to/dbt_case.dcm,2,"[[120,340,220,430],[400,510,480,590]]","[[0,1,2],[5,6]]"
/path/to/ffdm_case.dcm,1,"[[80,150,190,260]]","[]"
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
- A coordinate box outside image bounds.
- A frame index outside the file's frame range.
- A different number of `ROI_coords` and `ROI_frames` entries.

If a matching row is invalid, no CSV annotations are committed. Image viewing
continues, while annotation reads and export return the validation error. Bad
ROI payloads on unmatched dataset-level rows are intentionally ignored.

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

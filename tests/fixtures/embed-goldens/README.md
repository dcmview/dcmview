# EMBED CSV goldens

The bytes dcmview 0.3.x reads and writes for EMBED-style ROI CSVs, frozen
before the annotation store is replaced
(`docs/design/annotation-model.md` section 11 step 1).
`tests/integration/embed_goldens.rs` runs them and describes each case.

- `<case>.input.csv` is passed to `--annotations`.
- `<case>.expected.csv` is what `GET /api/annotations/export.csv` must then
  return, byte for byte, with rows in any order.
- A `rejected-*` input has no expected file: its import fails and export
  answers 500.
- A `startup-*` input has none either: the process exits before it serves.

`{{ROOT}}` stands for the directory holding the matched DICOM files. The test
copies committed fixtures into a temporary directory and substitutes its
canonical absolute path into both files before the viewer starts and before
the comparison, so the files compared hold real paths. `{{LINK}}` stands for
a symbolic link to that directory.

The cases run on Unix only. Including Windows would need LF endings pinned
for this directory in `.gitattributes` and a placeholder for the path
separator.

These files are written by hand and are not produced by
`generate_test_fixtures`. Keep LF line endings and the final newline. Change
an expected file only for a behaviour change the owner has signed off, in the
commit that makes it.

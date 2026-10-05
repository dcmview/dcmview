# EMBED CSV goldens

The bytes dcmview 0.3.x reads and writes for EMBED-style ROI CSVs, frozen
before the annotation store is replaced
(`docs/design/annotation-model.md` section 11 step 1).
`tests/integration/embed_goldens.rs` runs them and describes each case.

- `<case>.input.csv` is passed to `--annotations`.
- `<case>.expected.csv` is what `GET /api/annotations/export.csv` must then
  return, byte for byte, with rows in any order.
- A `rejected-*` input has no expected file: its import fails as a whole and
  export answers 500.

`{{ROOT}}` stands for the directory holding the matched DICOM files. The test
copies committed fixtures into a temporary directory and substitutes its
canonical absolute path into both files before the viewer starts and before
the comparison, so the files compared hold real paths.

These files are written by hand and are not produced by
`generate_test_fixtures`. Keep LF line endings and the final newline. Change
an expected file only for a behaviour change the owner has signed off, in the
commit that makes it.

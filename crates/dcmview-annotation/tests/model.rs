//! Tests of the annotation model through its public API.
//!
//! The wire shapes are pinned by `tests/fixtures/document.json` (one document
//! that uses every member of the format) and `tests/fixtures/operations.json`
//! (one envelope per operation, with the queue keys it touches). Both are
//! written by hand from `docs/design/annotation-model.md`; the frontend's
//! tests may read the same files.

mod model {
    mod bounds;
    mod frames;
    mod generated;
    mod geometry;
    mod keys;
    mod operations;
    mod support;
    mod validation;
    mod wire;
}

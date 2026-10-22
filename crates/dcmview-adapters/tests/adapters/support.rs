//! A file registry for the tests: loaded files an input can name.

use dcmview_adapters::{
    FileResolver, ImportContext, Limits, Matcher, Refusal, Report, Resolution, Resolved,
};
use dcmview_annotation::{Author, FileKey, FileRef, ImageSize, LabelSchema, LayerId, Timestamp};
use std::path::PathBuf;

/// One loaded file.
#[derive(Debug, Clone)]
pub struct Loaded {
    pub path: &'static str,
    pub key: Option<FileKey>,
    pub size: ImageSize,
    pub size_bytes: u64,
    /// What the resolver says of the file instead of matching it.
    pub refusal: Option<Refusal>,
}

pub fn size(rows: u32, columns: u32, frames: u32) -> ImageSize {
    ImageSize {
        columns,
        rows,
        frames,
    }
}

pub fn key(text: &str) -> FileKey {
    FileKey::parse(text).expect("file key")
}

pub fn loaded(path: &'static str, key_text: &str, size: ImageSize) -> Loaded {
    Loaded {
        path,
        key: Some(key(key_text)),
        size,
        size_bytes: 1_000,
        refusal: None,
    }
}

/// Loaded files, each with its position as its slot. A path names every
/// file that has it; a reference is matched by key, then by SOP Instance
/// UID (a `sop:` key with that UID), then by path.
pub struct Pool(pub Vec<Loaded>);

impl Pool {
    fn resolved(&self, slot: usize) -> Resolved {
        let file = &self.0[slot];
        Resolved {
            slot,
            key: file.key.clone(),
            size: file.size,
            size_bytes: file.size_bytes,
        }
    }
}

impl FileResolver for Pool {
    fn resolve(&self, reference: &FileRef) -> Resolution {
        let by_key = |wanted: &str| {
            self.0
                .iter()
                .position(|file| file.key.as_ref().is_some_and(|key| key.as_str() == wanted))
        };
        let found = by_key(reference.key.as_str())
            .map(|slot| (slot, Matcher::Key))
            .or_else(|| {
                let uid = reference.sop_instance_uid.as_deref()?;
                Some((by_key(&format!("sop:{uid}"))?, Matcher::SopInstanceUid))
            })
            .or_else(|| {
                let slot = self.0.iter().position(|file| file.path == reference.path)?;
                Some((slot, Matcher::Path))
            });
        match found {
            None => Resolution::Unmatched,
            Some((slot, by)) => match self.0[slot].refusal {
                Some(refusal) => Resolution::Refused(refusal),
                None => Resolution::Matched(self.resolved(slot), by),
            },
        }
    }

    fn by_path(&self, path: &str, out: &mut Vec<Resolved>) {
        for (slot, file) in self.0.iter().enumerate() {
            if file.path == path {
                out.push(self.resolved(slot));
            }
        }
    }
}

/// What an import is handed in these tests, apart from the resolver.
pub struct Session {
    pub schema: Option<LabelSchema>,
    pub layer: LayerId,
    pub author: Author,
    pub now: Timestamp,
}

impl Session {
    pub fn new(schema: Option<LabelSchema>) -> Self {
        Self {
            schema,
            layer: LayerId::parse("default").expect("layer id"),
            author: Author::parse("import:test").expect("author"),
            now: Timestamp::parse("2026-10-10T08:00:00.000Z").expect("timestamp"),
        }
    }

    pub fn context<'a>(&'a self, resolver: &'a dyn FileResolver) -> ImportContext<'a> {
        ImportContext {
            resolver,
            schema: self.schema.as_ref(),
            take_schema: false,
            layer: &self.layer,
            author: &self.author,
            now: &self.now,
            keep_going: &|| true,
            limits: Limits::DEFAULT,
        }
    }
}

pub fn report_json(report: &Report) -> serde_json::Value {
    serde_json::to_value(report).expect("a report is JSON")
}

/// The model crate's document fixture: every record kind and geometry.
pub fn fixture_document_text() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../dcmview-annotation/tests/fixtures/document.json");
    std::fs::read_to_string(path).expect("the model's document fixture")
}

/// The viewer's EMBED golden inputs, placeholders as committed.
pub fn embed_golden_inputs() -> Vec<(String, String)> {
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/embed-goldens");
    let mut inputs: Vec<(String, String)> = std::fs::read_dir(directory)
        .expect("the EMBED goldens")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".input.csv"))
        .map(|path| {
            (
                path.file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned(),
                std::fs::read_to_string(&path).expect("golden input"),
            )
        })
        .collect();
    inputs.sort();
    assert!(inputs.len() >= 10, "the goldens moved");
    inputs
}

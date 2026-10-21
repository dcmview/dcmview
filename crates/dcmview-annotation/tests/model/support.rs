//! Fixtures and the small helpers the tables share.

use dcmview_annotation::{
    Document, FileKey, ImageSize, Invalid, LabelSchema, OpEnvelope, ViolationCode,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub const DOCUMENT: &str = include_str!("../fixtures/document.json");
pub const OPERATIONS: &str = include_str!("../fixtures/operations.json");

/// The single-frame DICOM file of the fixture: 160 rows by 240 columns.
pub const FILE_A: &str = "sop:1.2.826.0.1.3680043.8.498.1";
/// The three-frame DICOM file of the fixture: 160 rows by 240 columns.
pub const FILE_B: &str = "sop:1.2.826.0.1.3680043.8.498.2";

/// 160 rows by 240 columns, three frames: the fixture's multiframe file.
pub const SIZE: ImageSize = ImageSize {
    columns: 240,
    rows: 160,
    frames: 3,
};

/// A version 4 UUID, which no record or operation id may be.
pub const UUID_V4: &str = "3f2b8c1e-5d4a-4e6f-9a7b-1c2d3e4f5a6b";
/// A version 7 UUID that names nothing in the fixtures.
pub const UUID_V7_OTHER: &str = "0199c0de-0000-7000-8000-00000000ffff";

pub fn document_value() -> Value {
    serde_json::from_str(DOCUMENT).expect("fixture document is JSON")
}

pub fn document() -> Document {
    Document::from_json_str(DOCUMENT).expect("fixture document reads")
}

/// One entry of `operations.json`.
pub struct OperationCase {
    pub name: String,
    pub envelope: Value,
    pub queue_keys: Vec<String>,
}

pub fn operation_cases() -> Vec<OperationCase> {
    let cases: Vec<Value> = serde_json::from_str(OPERATIONS).expect("fixture operations are JSON");
    cases
        .into_iter()
        .map(|case| OperationCase {
            name: case["name"].as_str().expect("case name").to_string(),
            envelope: case["envelope"].clone(),
            queue_keys: case["queue_keys"]
                .as_array()
                .expect("queue keys")
                .iter()
                .map(|key| key.as_str().expect("queue key").to_string())
                .collect(),
        })
        .collect()
}

pub fn operation(name: &str) -> Value {
    operation_cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("no operation fixture named {name}"))
        .envelope
}

pub fn read_envelope(envelope: &Value) -> OpEnvelope {
    OpEnvelope::from_json_str(&envelope.to_string()).expect("envelope reads")
}

/// The files and schema of the fixture document, as a validation context
/// borrows them.
pub struct Fixture {
    pub files: BTreeMap<FileKey, ImageSize>,
    pub schema: LabelSchema,
}

pub fn fixture() -> Fixture {
    let document = document();
    Fixture {
        files: document
            .files
            .iter()
            .map(|file| (file.key.clone(), file.size()))
            .collect(),
        schema: document.schema.expect("fixture has a schema"),
    }
}

/// A change to a JSON value, addressed by JSON Pointer.
pub enum Edit {
    /// Replace the value at the pointer, or add the member it names.
    Set(&'static str, Value),
    /// Remove the member the pointer names.
    Remove(&'static str),
    /// Append to the array at the pointer.
    Push(&'static str, Value),
}

pub fn apply(mut value: Value, edits: &[Edit]) -> Value {
    for edit in edits {
        match edit {
            Edit::Set(pointer, new) => {
                let (parent, member) = split(pointer);
                match value.pointer_mut(parent).expect("parent exists") {
                    Value::Object(map) => {
                        map.insert(member.to_string(), new.clone());
                    }
                    Value::Array(items) => {
                        items[member.parse::<usize>().expect("array index")] = new.clone();
                    }
                    other => panic!("{pointer}: cannot set a member of {other}"),
                }
            }
            Edit::Remove(pointer) => {
                let (parent, member) = split(pointer);
                value
                    .pointer_mut(parent)
                    .and_then(Value::as_object_mut)
                    .expect("parent object exists")
                    .remove(member)
                    .expect("member exists");
            }
            Edit::Push(pointer, new) => value
                .pointer_mut(pointer)
                .and_then(Value::as_array_mut)
                .expect("array exists")
                .push(new.clone()),
        }
    }
    value
}

fn split(pointer: &str) -> (&str, &str) {
    pointer.rsplit_once('/').expect("pointer has a parent")
}

/// What a table row expects of a validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Valid,
    Code(ViolationCode),
}

pub fn assert_outcome(name: &str, outcome: Result<(), Invalid>, expected: Expect) {
    match (outcome, expected) {
        (Ok(()), Expect::Valid) => {}
        (Err(invalid), Expect::Code(code)) => {
            assert!(
                invalid.has(code),
                "{name}: expected {code:?}, got {:?}",
                invalid.violations
            );
        }
        (Ok(()), Expect::Code(code)) => panic!("{name}: expected {code:?}, but it is valid"),
        (Err(invalid), Expect::Valid) => {
            panic!("{name}: expected valid, got {:?}", invalid.violations)
        }
    }
}

/// Fails when any object in `text` names one member twice. `serde_json::Value`
/// cannot show this: it keeps the last of two members with one name.
pub fn assert_no_member_twice(name: &str, text: &str) {
    use serde::de::{DeserializeSeed, Deserializer, Error, MapAccess, SeqAccess, Visitor};

    /// Walks any JSON value; the string is the JSON Pointer of the value.
    struct Walk(String);

    impl<'de> DeserializeSeed<'de> for Walk {
        type Value = ();

        fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
            deserializer.deserialize_any(self)
        }
    }

    impl<'de> Visitor<'de> for Walk {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON value")
        }

        fn visit_bool<E>(self, _: bool) -> Result<(), E> {
            Ok(())
        }

        fn visit_i64<E>(self, _: i64) -> Result<(), E> {
            Ok(())
        }

        fn visit_u64<E>(self, _: u64) -> Result<(), E> {
            Ok(())
        }

        fn visit_f64<E>(self, _: f64) -> Result<(), E> {
            Ok(())
        }

        fn visit_str<E>(self, _: &str) -> Result<(), E> {
            Ok(())
        }

        fn visit_unit<E>(self) -> Result<(), E> {
            Ok(())
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<(), A::Error> {
            let mut index = 0;
            while items
                .next_element_seed(Walk(format!("{}/{index}", self.0)))?
                .is_some()
            {
                index += 1;
            }
            Ok(())
        }

        fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<(), A::Error> {
            let mut seen = std::collections::HashSet::new();
            while let Some(member) = members.next_key::<String>()? {
                let pointer = format!("{}/{member}", self.0);
                if !seen.insert(member) {
                    return Err(A::Error::custom(format!("{pointer} is written twice")));
                }
                members.next_value_seed(Walk(pointer))?;
            }
            Ok(())
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(text);
    if let Err(error) = Walk(String::new()).deserialize(&mut deserializer) {
        panic!("{name}: {error}");
    }
}

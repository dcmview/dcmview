//! What the tag endpoints may read and hold of a DICOM file
//! (`data_set::read_for_tags`, `/tags`, `/tags/select`), counted as
//! `data_sets.rs` counts the catalog read: bytes at the source, heap at the
//! allocator.

use super::data_set_files::{
    element, element_declaring, encapsulated_pixel_data, identity, image, image_module, nested,
    part10, pixel_data, CONTENT_SEQUENCE, DEFLATED_LE, EXPLICIT_LE, JPEG_BASELINE, KIB, MIB,
    PRIVATE_BLOB,
};
use super::data_sets::{
    deflated_values, discover, measured, report, BULK, DECLARED, INFLATER_HEAP, READ_HEAP,
};
use super::heap::CountedRuntime;
use dcmview::annotations::AnnotationStore;
use dcmview::data_set::{
    read_for_tags, TagCut, TagExtent, DATA_SET_INFLATED_BUDGET_BYTES, DATA_SET_VALUE_MAX_BYTES,
};
use dcmview::server::{self, AppState, FileRegistry};
use dcmview::types::FileEntry;
use dicom_core::Tag;
use dicom_dictionary_std::tags;
use serde_json::{json, Value};

/// The heap a request to a tag endpoint may hold beside what its read
/// holds, for a file of megabytes: the request, the response and the tree,
/// never the file.
const TAG_REQUEST_HEAP: u64 = MIB;

struct TagCase {
    name: &'static str,
    file: Vec<u8>,
    /// Top-level nodes the tree must show, by tag, with their value.
    shows: Vec<(&'static str, Value)>,
    /// The last node of the tree.
    last: &'static str,
    /// The most bytes the read may take from the file.
    bytes: u64,
    /// The most heap it may hold.
    heap: u64,
    /// Whether discovery lists the file, so the endpoints can be asked.
    listed: bool,
    /// Why the tree ends before the data set does, if it does.
    cut: Option<TagCut>,
}

fn binary(length: usize) -> Value {
    json!({ "type": "binary", "length": length })
}

fn text(value: &str) -> Value {
    json!({ "type": "string", "value": value })
}

const PIXEL_DATA: &str = "(7FE0,0010)";
const BLOB: &str = "(0009,1010)";
const TRAILER: &str = "(7FE1,0010)";

fn tag_cases() -> Vec<TagCase> {
    let samples = vec![0_u16; BULK];
    let pixel_bytes = 2 * BULK;
    let trailer = element(Tag(0x7FE1, 0x0010), "LO", b"TRAILER ");
    let limit = DATA_SET_VALUE_MAX_BYTES as usize;
    let large_image = |before: &[u8], pixels: Vec<u8>, after: &[u8]| {
        part10(
            EXPLICIT_LE,
            &[
                identity("2.25.4000"),
                before.to_vec(),
                image_module(2, 2),
                pixels,
                after.to_vec(),
            ]
            .concat(),
        )
    };
    // Bytes after the data set are not elements of it, however many there
    // are and whatever the data set ends with.
    let padded = |name, bytes: usize, pixels: bool| TagCase {
        name,
        file: part10(
            EXPLICIT_LE,
            &[
                identity("2.25.4000"),
                image_module(2, 2),
                if pixels {
                    [pixel_data(&[1, 2, 3, 4]), trailer.clone()].concat()
                } else {
                    Vec::new()
                },
                vec![0; bytes],
            ]
            .concat(),
        ),
        shows: Vec::new(),
        last: if pixels { TRAILER } else { "(0028,0103)" },
        bytes: 64 * KIB,
        heap: READ_HEAP,
        listed: false,
        cut: None,
    };
    let case = |name, file, shows, last, listed| TagCase {
        name,
        file,
        shows,
        last,
        bytes: 64 * KIB,
        heap: READ_HEAP,
        listed,
        cut: None,
    };
    // Top-level bytes that are not an element, and not padding either.
    let not_an_element = |name, before: Vec<u8>, after: Vec<u8>, last| TagCase {
        cut: Some(TagCut::NotAnElement),
        ..case(
            name,
            image(EXPLICIT_LE, &before, &after),
            Vec::new(),
            last,
            false,
        )
    };
    vec![
        not_an_element(
            "an element of tag zero in the middle of the data set",
            element(Tag(0, 0), "UL", &[0; 4]),
            Vec::new(),
            "(0008,0060)",
        ),
        not_an_element(
            "a header that cannot be read behind the pixel data",
            Vec::new(),
            vec![0x08, 0x00, 0x10, 0x00, b'L'],
            PIXEL_DATA,
        ),
        not_an_element(
            "zeros and then something else behind the pixel data",
            Vec::new(),
            [vec![0; 16], trailer.clone()].concat(),
            PIXEL_DATA,
        ),
        padded("4 bytes of padding behind the pixel data", 4, true),
        padded("6 bytes of padding behind the pixel data", 6, true),
        padded("8 bytes of padding behind the pixel data", 8, true),
        padded("37 bytes of padding behind the pixel data", 37, true),
        padded("4 bytes of padding and no pixel data", 4, false),
        padded("6 bytes of padding and no pixel data", 6, false),
        padded("8 bytes of padding and no pixel data", 8, false),
        padded("37 bytes of padding and no pixel data", 37, false),
        // Pixel data and what follows it are described from headers.
        case(
            "native pixel data and a trailing element",
            large_image(&[], pixel_data(&samples), &trailer),
            vec![
                (PIXEL_DATA, binary(pixel_bytes)),
                (TRAILER, text("TRAILER")),
            ],
            TRAILER,
            true,
        ),
        case(
            "encapsulated pixel data and a trailing element",
            part10(
                JPEG_BASELINE,
                &[
                    identity("2.25.4000"),
                    image_module(2, 2),
                    encapsulated_pixel_data(&[vec![0; BULK], vec![0; BULK]]),
                    trailer.clone(),
                ]
                .concat(),
            ),
            vec![
                (PIXEL_DATA, binary(pixel_bytes)),
                (TRAILER, text("TRAILER")),
            ],
            TRAILER,
            true,
        ),
        // A well-formed element is listed wherever it stands.
        case(
            "an element of a lower tag behind the pixel data",
            large_image(
                &[],
                pixel_data(&[1, 2, 3, 4]),
                &element(Tag(0x0009, 0x1012), "LO", b"BEHIND"),
            ),
            vec![(PIXEL_DATA, binary(8))],
            PIXEL_DATA,
            true,
        ),
        // Sequences nested past the limit end the tree where they are.
        TagCase {
            cut: Some(TagCut::TooDeep),
            heap: 4 * READ_HEAP,
            ..case(
                "sequences nested without end behind the pixel data",
                large_image(
                    &[],
                    pixel_data(&[1, 2, 3, 4]),
                    &nested(Tag(0x7FE1, 0x1010), 100_000, &[], false),
                ),
                vec![(PIXEL_DATA, binary(8))],
                "(7FE1,1010)",
                true,
            )
        },
        TagCase {
            cut: Some(TagCut::TooDeep),
            heap: 4 * READ_HEAP,
            ..case(
                "sequences nested without end before the pixel data",
                image(
                    EXPLICIT_LE,
                    &nested(CONTENT_SEQUENCE, 100_000, &[], false),
                    &[],
                ),
                Vec::new(),
                "(0040,A730)",
                false,
            )
        },
        // A value over the limit is shown by its length, like a bulk value.
        case(
            "a bulk value and long text",
            large_image(
                &[
                    element(PRIVATE_BLOB, "OB", &vec![0; BULK]),
                    element(Tag(0x0009, 0x1011), "UT", &vec![b'a'; limit + 2]),
                    element(Tag(0x0009, 0x1012), "UT", b"short "),
                ]
                .concat(),
                pixel_data(&[1, 2, 3, 4]),
                &[],
            ),
            vec![
                (BLOB, binary(BULK)),
                ("(0009,1011)", binary(limit + 2)),
                ("(0009,1012)", text("short")),
            ],
            PIXEL_DATA,
            true,
        ),
        TagCase {
            bytes: 2 * limit as u64,
            // The value, its decoded text and the buffer it was read into.
            heap: READ_HEAP + 4 * limit as u64,
            ..case(
                "text at the limit",
                large_image(
                    &element(Tag(0x0009, 0x1011), "UT", &vec![b'a'; limit]),
                    pixel_data(&[1, 2, 3, 4]),
                    &[],
                ),
                vec![("(0009,1011)", text(&format!("{}\u{2026}", "a".repeat(256))))],
                PIXEL_DATA,
                true,
            )
        },
        // A data set that ends inside a value that is not read ends there.
        TagCase {
            cut: Some(TagCut::InsideValue),
            ..case(
                "a value declared past the end",
                image(
                    EXPLICIT_LE,
                    &element_declaring(PRIVATE_BLOB, "OB", DECLARED, &[0; 8]),
                    &[],
                ),
                vec![(BLOB, binary(DECLARED as usize))],
                BLOB,
                false,
            )
        },
        TagCase {
            cut: Some(TagCut::InsideValue),
            bytes: MIB,
            heap: READ_HEAP + INFLATER_HEAP,
            ..case(
                "deflated pixel data past the budget and a trailing element",
                part10(
                    DEFLATED_LE,
                    &[
                        identity("2.25.4000"),
                        image_module(2, 2),
                        element(
                            tags::PIXEL_DATA,
                            "OW",
                            &vec![0; DATA_SET_INFLATED_BUDGET_BYTES as usize + 2],
                        ),
                        trailer.clone(),
                    ]
                    .concat(),
                ),
                vec![(
                    PIXEL_DATA,
                    binary(DATA_SET_INFLATED_BUDGET_BYTES as usize + 2),
                )],
                PIXEL_DATA,
                true,
            )
        },
    ]
}

fn node<'a>(nodes: &'a Value, tag: &str) -> Option<&'a Value> {
    nodes
        .as_array()
        .expect("a list of nodes")
        .iter()
        .find(|node| node["tag"] == tag)
}

/// The tag read never reads a value it shows by its length (pixel data,
/// bulk values, values over the limit), and the tag endpoints hold no more
/// than the tree for a file of any size.
#[test]
fn a_tag_read_holds_no_value_it_shows_by_its_length() {
    let cases = tag_cases();
    for case in &cases {
        let (tree, cost) = measured(
            &case.file,
            |source, length| read_for_tags(source, length, TagExtent::Whole),
            |read| {
                let read = read.unwrap_or_else(|error| panic!("{}: {error:#}", case.name));
                assert_eq!(read.cut, case.cut, "{}: why the tree ends", case.name);
                read.object.tags().collect::<Vec<_>>()
            },
        );
        report(|| {
            format!(
                "tags     {:<52} {:>9} bytes in the file, {:>9} read, {:>9} held",
                case.name,
                case.file.len(),
                cost.bytes,
                cost.heap
            )
        });
        let last = tree.last().expect("a data set with elements");
        if case.name.starts_with("an element of a lower tag") {
            assert!(tree.contains(&Tag(0x0009, 0x1012)), "{}", case.name);
        }
        assert_ne!(
            tree.first(),
            Some(&Tag(0, 0)),
            "{}: bytes after the data set are listed",
            case.name
        );
        assert_eq!(
            format!("({:04X},{:04X})", last.0, last.1),
            case.last,
            "{}: where the tree ends",
            case.name
        );
        assert!(
            cost.bytes <= case.bytes,
            "{}: {} bytes read, at most {}",
            case.name,
            cost.bytes,
            case.bytes
        );
        assert!(
            cost.heap <= case.heap,
            "{}: {} bytes of heap held, at most {}",
            case.name,
            cost.heap,
            case.heap
        );
    }
    // A deflated data set whose values would hold more than its budget
    // has no tree, whichever extent is read; an honest one that looks like
    // it has; and neither read holds more than the budget allows.
    for values in deflated_values() {
        for extent in [TagExtent::BeforePixelData, TagExtent::Whole] {
            let (shown, cost) = measured(
                &values.file,
                |source, length| read_for_tags(source, length, extent),
                |read| read.is_ok(),
            );
            let context = format!("{}, {extent:?}", values.name);
            report(|| format!("tags     {context:<60} {:>9} held", cost.heap));
            assert_eq!(shown, values.shown, "{context}");
            assert!(
                cost.heap <= values.heap,
                "{context}: {} bytes of heap held, at most {}",
                cost.heap,
                values.heap
            );
        }
    }

    // Text of a deflated data set is decoded as the parser decodes it. A
    // character set takes effect only when it is declared as a Code
    // String; text longer than a piece that is not plain ASCII is kept as
    // its first piece, up to the last whole character.
    let text_of = |before: Vec<u8>, tag: Tag| {
        let file = image(DEFLATED_LE, &before, &[]);
        measured(
            &file,
            |source, length| read_for_tags(source, length, TagExtent::Whole),
            |read| {
                let read = read.expect("a tree");
                let value = read.object.get(tag).expect("the element");
                value.to_str().expect("text").to_string()
            },
        )
        .0
    };
    let thai = element(Tag(0x0009, 0x1011), "LO", &[0xa1; 4]);
    let set = |vr| element(tags::SPECIFIC_CHARACTER_SET, vr, b"ISO_IR 166");
    let undeclared = text_of(thai.clone(), Tag(0x0009, 0x1011));
    let as_code_string = text_of([set("CS"), thai.clone()].concat(), Tag(0x0009, 0x1011));
    let as_long_string = text_of([set("LO"), thai].concat(), Tag(0x0009, 0x1011));
    assert_ne!(
        as_code_string, undeclared,
        "a declared character set decodes"
    );
    assert_eq!(
        as_long_string, undeclared,
        "a character set that is not a Code String is not one"
    );
    // One ASCII letter, then two-byte characters: the 2,048th straddles
    // the end of the first piece.
    let straddling = [b"a".to_vec(), "\u{e9}".repeat(6_000).into_bytes()].concat();
    let kept = text_of(
        [
            element(tags::SPECIFIC_CHARACTER_SET, "CS", b"ISO_IR 192"),
            element(
                Tag(0x0009, 0x1011),
                "UT",
                &straddling[..straddling.len() - 1],
            ),
        ]
        .concat(),
        Tag(0x0009, 0x1011),
    );
    assert_eq!(kept, format!("a{}", "\u{e9}".repeat(2_047)));

    let listed: Vec<_> = cases.iter().filter(|case| case.listed).collect();
    let files: Vec<_> = listed
        .iter()
        .map(|case| (case.name, case.file.as_slice()))
        .collect();
    let (entries, _, _dir) = discover(&files);
    let entries: Vec<FileEntry> = listed
        .iter()
        .enumerate()
        .map(|(index, case)| FileEntry {
            index,
            ..entries
                .get(case.name)
                .unwrap_or_else(|| panic!("{} is listed", case.name))
                .clone()
        })
        .collect();
    let runtime = CountedRuntime::new();
    let ask = |path: String, read_heap: u64| {
        let state = AppState::new(
            FileRegistry::from_files(entries.clone()),
            AnnotationStore::empty(),
        );
        let ((status, body), heap) = runtime.peak_during(async {
            let server = axum_test::TestServer::new(server::router(state));
            let response = server.get(&path).await;
            (response.status_code(), response.json::<Value>())
        });
        assert_eq!(status, 200, "{path}: {body}");
        let limit = TAG_REQUEST_HEAP + read_heap;
        assert!(
            heap <= limit,
            "{path}: {heap} bytes of heap held, at most {limit}"
        );
        report(|| format!("request  {path:<52} {heap:>9} held"));
        body
    };
    for (index, case) in listed.iter().enumerate() {
        let tree = ask(format!("/api/file/{index}/tags"), case.heap);
        let nodes = tree.as_array().expect("a list of nodes");
        // A tree that ends before its data set says so in a last leaf.
        let noted = nodes.last().is_some_and(|node| node["tag"] == "Note");
        assert_eq!(noted, case.cut.is_some(), "{}: {tree}", case.name);
        let last = nodes.iter().rev().nth(usize::from(noted));
        assert_eq!(
            last.map(|node| &node["tag"]),
            Some(&Value::from(case.last)),
            "{}: where the served tree ends",
            case.name
        );
        if case.name.starts_with("sequences nested") {
            let ends = ask(
                format!("/api/file/{index}/tags/select?path={}", case.last),
                case.heap,
            );
            for node in [last.expect("a node"), &ends] {
                assert_eq!(node["value"]["type"], "error", "{}: {node}", case.name);
            }
        }
        for (tag, value) in &case.shows {
            let shown = node(&tree, tag).map(|node| &node["value"]);
            assert_eq!(shown, Some(value), "{}: {tag} in the tree", case.name);
            let selected = ask(
                format!("/api/file/{index}/tags/select?path={tag}"),
                case.heap,
            );
            assert_eq!(&selected["value"], value, "{}: {tag} selected", case.name);
        }
    }
}

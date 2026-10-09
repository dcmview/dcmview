//! What reading a DICOM data set may cost, whatever the file declares
//! (`data_set::read_for_catalog`, `data_set::read_for_tags`).
//!
//! Every element of a DICOM file declares its own length, and a deflated
//! data set its own size, so the limits are asserted in things that can be
//! counted, never in time: the bytes a read takes from its source, counted
//! at the source, and the heap it holds, counted at the allocator. The same
//! files then go through discovery, which must list and refuse them as the
//! read does. `data_set_tags.rs` does the same for the tag endpoints.
//!
//! `RASTER_COST_REPORT=1 cargo test --test raster_cost data_set -- --nocapture`
//! prints what each file cost.

use super::data_set_files::{
    element, element_declaring, identity, image, image_module, nested, part10, part10_with_meta,
    pixel_data, sequence, CONTENT_SEQUENCE, DEFLATED_LE, EXPLICIT_LE, KIB, OVERLAY_DATA,
    PRIVATE_BLOB, TRAILING_PADDING,
};
use super::heap;
use super::raster_files::CountedFile;
use dcmview::data_set::{
    read_for_catalog, DATA_SET_CATALOG_MAX_VALUES, DATA_SET_INFLATED_BUDGET_BYTES,
    DATA_SET_INFLATED_ELEMENT_CHARGE_BYTES, DATA_SET_INFLATED_OVERSHOOT_BYTES, DATA_SET_MAX_DEPTH,
    DATA_SET_VALUE_MAX_BYTES,
};
use dcmview::loader::{self, DiscoverOptions, DiscoveryReason, FormatSelection};
use dcmview::types::FileEntry;
use dicom_core::Tag;
use dicom_dictionary_std::tags;
use std::collections::BTreeMap;
use std::io::{BufReader, Seek, SeekFrom};
use tokio::sync::mpsc;

/// What a read may hold beside what it keeps of the file: its buffers and
/// the small object it builds from a file of a few hundred bytes.
pub(super) const READ_HEAP: u64 = 64 * KIB;

/// What inflating a deflated data set holds beside that.
pub(super) const INFLATER_HEAP: u64 = 256 * KIB;

/// A length no file here comes near.
pub(super) const DECLARED: u32 = 2 * 1024 * 1024 * 1024;

/// The bytes of a bulk value: over the longest value a read keeps.
pub(super) const BULK: usize = 4 * DATA_SET_VALUE_MAX_BYTES as usize;

pub(super) fn report(line: impl FnOnce() -> String) {
    if std::env::var_os("RASTER_COST_REPORT").is_some() {
        println!("{}", line());
    }
}

/// What one read took from its source and held.
pub(super) struct Cost {
    pub(super) bytes: u64,
    pub(super) heap: u64,
}

/// A counted source at the magic code, where both reads begin.
fn source(file: &[u8]) -> BufReader<CountedFile> {
    let mut source = BufReader::new(CountedFile::new(file.to_vec()));
    source
        .seek(SeekFrom::Start(128))
        .expect("seek to the magic code");
    source
}

/// Runs `read` over `file` and returns its result with what it cost. The
/// result is dropped inside the measurement, so what it holds is counted.
pub(super) fn measured<T, S>(
    file: &[u8],
    read: impl FnOnce(&mut BufReader<CountedFile>, u64) -> T,
    summary: impl FnOnce(T) -> S,
) -> (S, Cost) {
    let mut source = source(file);
    let (summary, heap) = heap::peak_during(|| summary(read(&mut source, file.len() as u64)));
    let cost = Cost {
        bytes: source.get_ref().bytes_read,
        heap,
    };
    (summary, cost)
}

/// What the catalog read makes of a file, and discovery with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Catalog {
    /// Read, with a pixel element: listed with pixels.
    Image,
    /// Refused: skipped as `dicom_parse_failed`.
    Refused,
    /// Read, but not walked to a pixel element: skipped as
    /// `inspection_failed`.
    Unwalked,
}

struct CatalogCase {
    name: &'static str,
    file: Vec<u8>,
    expected: Catalog,
    /// The most bytes the read may take from the file.
    bytes: u64,
    /// The most heap it may hold.
    heap: u64,
    /// An element the object must hold with its whole value.
    keeps: Option<(Tag, usize)>,
    /// An element the object must not hold.
    omits: Option<Tag>,
}

impl CatalogCase {
    /// A file of a few hundred bytes, read whole at most.
    fn small(name: &'static str, file: Vec<u8>, expected: Catalog) -> Self {
        let bytes = file.len() as u64;
        Self {
            name,
            file,
            expected,
            bytes,
            heap: READ_HEAP,
            keeps: None,
            omits: None,
        }
    }

    fn costing(mut self, bytes: u64, heap: u64) -> Self {
        (self.bytes, self.heap) = (bytes, heap);
        self
    }
}

/// `count` elements of `vr` holding `value`, with a tag each, in private
/// groups.
pub(super) fn elements_of(count: u32, vr: &str, value: &[u8]) -> Vec<u8> {
    (0..count)
        .flat_map(|index| {
            let tag = Tag(
                0x0011 + 2 * (index / 0x1000) as u16,
                0x1000 + (index % 0x1000) as u16,
            );
            element(tag, vr, value)
        })
        .collect()
}

/// `count` empty elements with a tag each, in private groups.
fn empty_elements(count: u32) -> Vec<u8> {
    elements_of(count, "LO", &[])
}

/// What a read of a deflated data set may hold: its budget, what it may
/// hold over it while one value is built, and what any read holds.
pub(super) const DEFLATED_HEAP: u64 =
    DATA_SET_INFLATED_BUDGET_BYTES + DATA_SET_INFLATED_OVERSHOOT_BYTES + READ_HEAP + INFLATER_HEAP;

/// A deflated data set whose values hold more, once read, than their
/// bytes, or look as if they might.
pub(super) struct DeflatedValues {
    pub(super) name: String,
    /// The elements, which go before the image module.
    pub(super) elements: Vec<u8>,
    /// Whether the catalog read accepts the file.
    pub(super) listed: bool,
    /// Whether the tag read has a tree for it.
    pub(super) shown: bool,
}

/// Files that would hold many times the budget if a value were charged its
/// bytes alone, which no read may accept without passing those values
/// over, and honest files that look like them, which every read accepts.
pub(super) fn deflated_values() -> Vec<DeflatedValues> {
    let limit = DATA_SET_VALUE_MAX_BYTES as usize;
    let case = |name: &str, elements: Vec<u8>, listed, shown| DeflatedValues {
        name: format!("deflated, {name}"),
        elements,
        listed,
        shown,
    };
    let declared = |name: &str| element(tags::SPECIFIC_CHARACTER_SET, "CS", name.as_bytes());
    // One-character values, as many as a value of the longest kept length
    // holds, behind kept values that leave little of the budget.
    let one_character_values = [
        elements_of(60, "UT", &vec![b'a'; limit]),
        element(PRIVATE_BLOB, "UC", &b"a\\".repeat(limit / 2)),
    ]
    .concat();
    // Values that each grow when decoded, many to a string.
    let growing = [vec![0xa1; 100], vec![b'\\']].concat().repeat(648);
    let mut cases = vec![
        case(
            "strings split into values",
            elements_of(4_000, "LO", &vec![b'\\'; 4_095]),
            false,
            false,
        ),
        case(
            "strings of nothing but separators",
            elements_of(256, "LO", &vec![b'\\'; 65_534]),
            false,
            false,
        ),
        case(
            "one-character values behind a nearly spent budget",
            one_character_values,
            // More values than the catalog keeps of one string.
            true,
            false,
        ),
        case(
            "strings of values that grow when decoded",
            [declared("ISO_IR 166"), elements_of(1_100, "LO", &growing)].concat(),
            false,
            false,
        ),
        case(
            "lists of tags",
            elements_of(1_500, "AT", &vec![0; 65_532]),
            false,
            false,
        ),
        case(
            "plain text past the budget",
            elements_of(70, "UT", &vec![b'a'; limit]),
            false,
            false,
        ),
        // Honest values of the same kinds.
        case(
            "one long plain string",
            element(PRIVATE_BLOB, "UC", &vec![b'a'; limit]),
            true,
            true,
        ),
        case(
            "long lists of numbers",
            elements_of(80, "DS", &b"123.4567\\".repeat(7_281)),
            true,
            true,
        ),
    ];
    // Text a decoder holds far more for than it has bytes: bytes that are
    // valid in a character set, bytes that are not, and escapes, in every
    // kind of text element, more of it than the budget.
    for (bytes, what) in [(0xa1_u8, "high"), (0xff, "invalid"), (0x1b, "escape")] {
        for character_set in [
            None,
            Some("ISO_IR 192"),
            Some("ISO_IR 166"),
            Some("ISO_IR 13"),
            Some("GB18030"),
            Some("ISO 2022 IR 87"),
        ] {
            let escapes = bytes == 0x1b;
            if escapes != (character_set == Some("ISO 2022 IR 87")) {
                continue;
            }
            let set = character_set.map_or(Vec::new(), declared);
            let name = character_set.unwrap_or("no character set");
            for (vr, length, count) in [
                ("UT", limit, 70),
                ("UC", limit, 70),
                ("LT", 65_534, 1_100),
                ("LO", 65_534, 1_100),
            ] {
                cases.push(case(
                    &format!("{vr} text of {what} bytes in {name}"),
                    [set.clone(), elements_of(count, vr, &vec![bytes; length])].concat(),
                    false,
                    false,
                ));
            }
        }
    }
    cases
}

fn catalog_cases() -> Vec<CatalogCase> {
    use Catalog::{Image, Refused, Unwalked};
    let small = CatalogCase::small;
    let before =
        |name, before: Vec<u8>, expected| small(name, image(EXPLICIT_LE, &before, &[]), expected);
    let deflated = |name, before: Vec<u8>, expected| {
        let file = image(DEFLATED_LE, &before, &[]);
        let bytes = file.len() as u64;
        small(name, file, expected).costing(bytes, READ_HEAP + INFLATER_HEAP)
    };
    let declared = |tag, vr| element_declaring(tag, vr, DECLARED, &[0; 8]);
    let bulk = vec![0_u8; BULK];
    let budget = DATA_SET_INFLATED_BUDGET_BYTES;
    let elements_in_budget = (budget / DATA_SET_INFLATED_ELEMENT_CHARGE_BYTES) as u32;
    // An item and its one element are charged three units.
    let items_in_budget = (elements_in_budget / 3) as usize;
    let item = element(PRIVATE_BLOB, "LO", &[]);
    let tag = Tag(0x0002, 0x0102);
    let limit = DATA_SET_VALUE_MAX_BYTES as usize;

    let mut overlay = small(
        "overlay data",
        part10(
            EXPLICIT_LE,
            &[
                identity("2.25.4000"),
                image_module(2, 2),
                element(OVERLAY_DATA, "OW", &bulk),
                pixel_data(&[1, 2, 3, 4]),
            ]
            .concat(),
        ),
        Image,
    );
    // The value, and the copy the assertion below takes of it.
    overlay.heap = READ_HEAP + 3 * BULK as u64;
    overlay.keeps = Some((OVERLAY_DATA, BULK));

    let mut cases = vec![
        before("an honest image", Vec::new(), Image),
        // A value the file does not hold is never allocated.
        before(
            "a value declared past the end",
            declared(PRIVATE_BLOB, "OB"),
            Refused,
        ),
        before(
            "text declared past the end",
            declared(PRIVATE_BLOB, "UT"),
            Refused,
        ),
        before(
            "a value declared past the end in an item",
            sequence(CONTENT_SEQUENCE, &[declared(PRIVATE_BLOB, "OB")]),
            Refused,
        ),
        small(
            "overlay data declared past the end",
            part10(
                EXPLICIT_LE,
                &[
                    identity("2.25.4000"),
                    image_module(2, 2),
                    declared(OVERLAY_DATA, "OW"),
                    pixel_data(&[1, 2, 3, 4]),
                ]
                .concat(),
            ),
            Refused,
        ),
        small(
            "a file meta value declared past the end",
            part10_with_meta(
                EXPLICIT_LE,
                &declared(tag, "OB"),
                &[
                    identity("2.25.4000"),
                    image_module(2, 2),
                    pixel_data(&[1, 2, 3, 4]),
                ]
                .concat(),
            ),
            Refused,
        ),
        // The pixel element is found by its header alone.
        small(
            "pixel data declared past the end",
            part10(
                EXPLICIT_LE,
                &[
                    identity("2.25.4000"),
                    image_module(2, 2),
                    declared(tags::PIXEL_DATA, "OW"),
                ]
                .concat(),
            ),
            Image,
        ),
        small(
            "a value declared past the end behind the pixel data",
            image(EXPLICIT_LE, &[], &declared(TRAILING_PADDING, "OB")),
            Image,
        ),
        small(
            "padding declared past the end and no pixel data",
            part10(
                EXPLICIT_LE,
                &[
                    identity("2.25.4000"),
                    image_module(2, 2),
                    declared(TRAILING_PADDING, "OB"),
                ]
                .concat(),
            ),
            Unwalked,
        ),
        // A value the catalog has no use for is passed over, not read.
        before(
            "a bulk value before the pixel data",
            element(PRIVATE_BLOB, "OB", &bulk),
            Image,
        )
        .costing(64 * KIB, READ_HEAP),
        before(
            "long text before the pixel data",
            element(PRIVATE_BLOB, "UT", &bulk),
            Image,
        )
        .costing(64 * KIB, READ_HEAP),
        before(
            "a bulk value in an item",
            sequence(CONTENT_SEQUENCE, &[element(PRIVATE_BLOB, "OB", &bulk)]),
            Image,
        )
        .costing(64 * KIB, READ_HEAP),
        overlay,
        // The longest value kept, and the shortest one that is not.
        {
            let mut case = before(
                "a value at the limit",
                element(PRIVATE_BLOB, "OB", &vec![0; limit]),
                Image,
            );
            case.heap = READ_HEAP + 3 * limit as u64;
            case.keeps = Some((PRIVATE_BLOB, limit));
            case
        },
        {
            let mut case = before(
                "a value over the limit",
                element(PRIVATE_BLOB, "OB", &vec![0; limit + 2]),
                Image,
            )
            .costing(64 * KIB, READ_HEAP);
            case.omits = Some(PRIVATE_BLOB);
            case
        },
        // The longest list of values the catalog keeps, and one value more.
        {
            let most = DATA_SET_CATALOG_MAX_VALUES as usize;
            let mut case = before(
                "a string of as many values as are kept",
                element(PRIVATE_BLOB, "UC", &vec![b'\\'; most - 1]),
                Image,
            );
            case.heap = READ_HEAP + 128 * most as u64;
            case.bytes *= 2;
            case.keeps = Some((PRIVATE_BLOB, most));
            case
        },
        {
            let most = DATA_SET_CATALOG_MAX_VALUES as usize;
            let mut case = before(
                "a string of more values than are kept",
                element(PRIVATE_BLOB, "UC", &vec![b'\\'; most + 1]),
                Image,
            );
            case.bytes *= 2;
            case.heap = READ_HEAP + 2 * most as u64;
            case.omits = Some(PRIVATE_BLOB);
            case
        },
        // Nesting has a fixed depth.
        {
            let case = before(
                "sequences nested to the limit",
                nested(CONTENT_SEQUENCE, DATA_SET_MAX_DEPTH, &item, true),
                Image,
            );
            let bytes = case.bytes;
            case.costing(bytes, 4 * READ_HEAP)
        },
        before(
            "sequences nested past the limit",
            nested(CONTENT_SEQUENCE, DATA_SET_MAX_DEPTH + 1, &item, true),
            Refused,
        ),
        before(
            "sequences nested without end",
            nested(CONTENT_SEQUENCE, 100_000, &[], false),
            Refused,
        ),
        // A data set of empty elements holds a multiple of its size, and
        // no more than this one.
        {
            let case = before("elements of no length", empty_elements(20_000), Image);
            let bytes = case.bytes;
            case.costing(bytes, READ_HEAP + 64 * bytes)
        },
        // A deflated data set is read within its budget, in bytes inflated
        // and in what is built from them.
        deflated("a deflated image", Vec::new(), Image),
        // The budget counts the bytes of every value, kept ones included.
        {
            let case = deflated(
                "deflated, kept values within the budget",
                elements_of(30, "OB", &vec![0; limit]),
                Image,
            );
            let bytes = case.bytes;
            case.costing(bytes, budget)
        },
        {
            let case = deflated(
                "deflated, kept values past the budget",
                elements_of(70, "OB", &vec![0; limit]),
                Refused,
            );
            let bytes = case.bytes;
            case.costing(bytes, budget + limit as u64)
        },
        deflated(
            "deflated, a bulk value within the budget",
            element(PRIVATE_BLOB, "OB", &vec![0; budget as usize / 2]),
            Image,
        ),
        deflated(
            "deflated, a bulk value past the budget",
            element(PRIVATE_BLOB, "OB", &vec![0; budget as usize]),
            Refused,
        ),
        {
            let case = deflated(
                "deflated, elements within the budget",
                empty_elements(elements_in_budget / 2),
                Image,
            );
            let bytes = case.bytes;
            case.costing(bytes, budget)
        },
        {
            let case = deflated(
                "deflated, elements past the budget",
                empty_elements(4 * elements_in_budget),
                Refused,
            );
            let bytes = case.bytes;
            // The read ends with the budget, not with the elements.
            case.costing(bytes, budget / 2)
        },
        {
            let case = deflated(
                "deflated, items past the budget",
                sequence(
                    CONTENT_SEQUENCE,
                    &vec![item.clone(); items_in_budget + 1_000],
                ),
                Refused,
            );
            let bytes = case.bytes;
            case.costing(bytes, budget)
        },
    ];
    cases.extend(deflated_values().into_iter().map(|values| {
        let name: &'static str = Box::leak(values.name.into_boxed_str());
        let expected = if values.listed { Image } else { Refused };
        let case = deflated(name, values.elements, expected);
        let bytes = case.bytes;
        case.costing(bytes, DEFLATED_HEAP)
    }));
    cases
}

/// Runs the production loader over `files` and returns the entries it
/// lists, by file name, and how many files it skipped for each reason.
pub(super) fn discover(
    files: &[(&str, &[u8])],
) -> (
    BTreeMap<String, FileEntry>,
    BTreeMap<DiscoveryReason, usize>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("temp dir");
    for (index, (_, bytes)) in files.iter().enumerate() {
        std::fs::write(dir.path().join(format!("{index:03}.dcm")), bytes).expect("write file");
    }
    let paths = [dir.path().to_path_buf()];
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (report, entries) = tokio::runtime::Runtime::new()
        .expect("runtime")
        .block_on(async {
            let scan = loader::discover_progressive(
                &paths,
                DiscoverOptions {
                    recursive: true,
                    filters: Vec::new(),
                    formats: FormatSelection::all(),
                },
                events_tx,
                loader::DiscoveryCancellation::new(),
            );
            let collect = async {
                let mut entries = BTreeMap::new();
                while let Some(event) = events_rx.recv().await {
                    if let loader::DiscoveryEvent::Selected { file, .. } = event {
                        let index: usize = file
                            .path
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .and_then(|stem| stem.parse().ok())
                            .expect("a numbered file");
                        entries.insert(files[index].0.to_string(), *file);
                    }
                }
                entries
            };
            tokio::join!(scan, collect)
        });
    let report = report.expect("discovery completes");
    (entries, report.skipped_by_reason, dir)
}

/// The catalog read never allocates a value the file does not hold, passes
/// over what the catalog has no use for, nests to a fixed depth and reads a
/// deflated data set within its budget; discovery lists and refuses the
/// same files.
#[test]
fn a_catalog_read_holds_what_it_read_and_never_what_a_file_declares() {
    let cases = catalog_cases();
    for case in &cases {
        let (outcome, cost) = measured(&case.file, read_for_catalog, |read| {
            let read = read?;
            if let Some(tag) = case.omits {
                assert!(
                    read.object.get(tag).is_none(),
                    "{}: the value is not kept",
                    case.name
                );
            }
            if let Some((tag, length)) = case.keeps {
                let kept = read
                    .object
                    .get(tag)
                    .map(|element| element.to_bytes().expect("a binary value").len());
                assert_eq!(kept, Some(length), "{}: the value is kept", case.name);
            }
            Some(match read.pixels {
                Ok(Some(pixels)) => {
                    assert_eq!(pixels.tag, tags::PIXEL_DATA, "{}", case.name);
                    Catalog::Image
                }
                Ok(None) => panic!("{}: a file without pixel data", case.name),
                Err(_) => Catalog::Unwalked,
            })
        });
        let outcome = outcome.unwrap_or(Catalog::Refused);
        report(|| {
            format!(
                "catalog  {:<52} {:>9} bytes in the file, {:>9} read, {:>9} held: {outcome:?}",
                case.name,
                case.file.len(),
                cost.bytes,
                cost.heap
            )
        });
        assert_eq!(outcome, case.expected, "{}", case.name);
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

    let files: Vec<_> = cases
        .iter()
        .map(|case| (case.name, case.file.as_slice()))
        .collect();
    let (entries, skipped, _dir) = discover(&files);
    for case in &cases {
        let entry = entries.get(case.name);
        assert_eq!(
            entry.is_some(),
            case.expected == Catalog::Image,
            "{}: listed",
            case.name
        );
        if let Some(entry) = entry {
            assert!(entry.has_pixels, "{}", case.name);
            assert_eq!((entry.rows, entry.columns), (2, 2), "{}", case.name);
            assert_eq!(entry.sop_instance_uid, "2.25.4000", "{}", case.name);
        }
    }
    let count = |expected| {
        cases
            .iter()
            .filter(|case| case.expected == expected)
            .count()
    };
    assert_eq!(
        skipped.get(&DiscoveryReason::DicomParseFailed).copied(),
        Some(count(Catalog::Refused))
    );
    assert_eq!(
        skipped.get(&DiscoveryReason::InspectionFailed).copied(),
        Some(count(Catalog::Unwalked))
    );
}

//! Bounded reads of a DICOM file's data set.
//!
//! A DICOM file is input nobody vouches for, and every element in it
//! declares its own length. This module is the only code that reads a data
//! set for the catalog ([`read_for_catalog`]) and for the tag endpoints
//! ([`read_for_tags`]), and it is the contract for what those reads may
//! cost, whatever the file declares:
//!
//! - **No value is read past the end.** A value is read only when its
//!   declared length fits in the bytes the file has left (for a deflated
//!   data set: in what is left of the inflated budget). The same holds for
//!   every element of the file meta group, checked before the group is
//!   parsed.
//! - **No value over [`DATA_SET_VALUE_MAX_BYTES`] is read**, with one
//!   exception: the catalog reads Overlay Data (60xx,3000) at its declared
//!   length, because the entry keeps the overlay planes. The tag endpoints
//!   read no value they show by its length alone ([`shown_by_length`]).
//! - **What is not read is passed over**, by seeking, so a skipped value
//!   costs neither memory nor a read of its bytes. A deflated data set
//!   cannot be seeked: its skipped values are inflated and discarded.
//! - **A deflated data set is read within
//!   [`DATA_SET_INFLATED_BUDGET_BYTES`]**: every inflated byte counts, kept
//!   or discarded, and so do
//!   [`DATA_SET_INFLATED_ELEMENT_CHARGE_BYTES`] for every element the read
//!   builds, twice that for every item, and
//!   [`DATA_SET_INFLATED_VALUE_CHARGE_BYTES`] for every value of a
//!   multi-valued string after its first, because a few bytes of a deflate
//!   stream can inflate to thousands of elements or of values. Text that
//!   is not plain ASCII is charged [`DATA_SET_INFLATED_TEXT_CHARGE`] times
//!   its length more, and a list of tags its length once more. The charge
//!   is worked out from the value's bytes before anything is built from
//!   them, and a value whose charge the budget does not cover is not read.
//! - **The catalog read keeps no string of more than
//!   [`DATA_SET_CATALOG_MAX_VALUES`] values**, counted in its bytes before
//!   it is split.
//! - **Sequences nest up to [`DATA_SET_MAX_DEPTH`] deep.**
//! - **Pixel data is never read.** The catalog read ends at the header of
//!   the top-level pixel element; the tag read passes over every pixel
//!   element and fragment.
//!
//! What a read holds is therefore bounded by what the file supplies, never
//! by a number the file declares: the values kept (each backed by its bytes
//! in the file) and the in-memory elements and values built from the bytes
//! read. That is not a fixed budget: an object model holds more than the
//! bytes it was built from. Only a read of a deflated data set has one, and
//! holds no more than it and the two buffers a value passes through, each
//! at most [`DATA_SET_VALUE_MAX_BYTES`]. `tests/raster_cost/data_sets.rs` counts the
//! reads, the bytes and the heap at the source and the allocator.
//!
//! The catalog read refuses a file that breaks a limit before its pixel
//! element: discovery lists it as `dicom_parse_failed`. The tag read fails
//! the same way for a value it would keep, and otherwise shows what the
//! limits let it reach: an element it does not read is listed by its
//! declared length; a data set that ends, or runs out of inflated budget,
//! inside a value it is passing over ends there; and so does one whose
//! sequences nest past the limit, at the sequence that does
//! ([`TagDataSet::too_deep`]). Top-level bytes that are not an element end
//! the tag read without an error and are not listed: a header that cannot
//! be read, or the tag (0000,0000), which is what zero padding after a data
//! set parses as. Every well-formed element is listed where it stands.

use anyhow::{anyhow, bail, ensure, Context, Result};
use dicom_core::header::{DataElementHeader, Length};
use dicom_core::value::{DataSetSequence, PrimitiveValue, Value};
use dicom_core::{Tag, VR};
use dicom_dictionary_std::tags;
use dicom_encoding::decode::Decode;
use dicom_encoding::{Codec, TransferSyntax, TransferSyntaxIndex};
use dicom_object::mem::InMemElement;
use dicom_object::{DefaultDicomObject, FileMetaTable, InMemDicomObject};
use dicom_parser::dataset::lazy_read::LazyDataSetReader;
use dicom_parser::dataset::LazyDataToken;
use dicom_parser::stateful::decode::Result as DecodeResult;
use dicom_parser::{DynStatefulDecoder, StatefulDecode};
use dicom_transfer_syntax_registry::TransferSyntaxRegistry;
use std::io::{BufReader, Read, Seek};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// The longest value either read keeps, in bytes. Longer than any value
/// the catalog or the tag tree uses but Overlay Data: a lookup table of
/// 65,536 16-bit entries is an eighth of it.
pub const DATA_SET_VALUE_MAX_BYTES: u32 = 1024 * 1024;

/// What one read of a deflated data set may spend: the bytes it inflates
/// and the charge for what it builds.
pub const DATA_SET_INFLATED_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// What an element built from a deflated data set is charged against the
/// budget, beside its bytes; an item is charged twice this. Each is more
/// than the element or the item holds in memory.
pub const DATA_SET_INFLATED_ELEMENT_CHARGE_BYTES: u64 = 512;

/// What each value of a multi-valued string read from a deflated data set
/// is charged against the budget after the first, beside its bytes: what a
/// value holds in memory with the list it is kept in, while that list
/// grows.
pub const DATA_SET_INFLATED_VALUE_CHARGE_BYTES: u64 = 72;

/// How many times its length a text value read from a deflated data set is
/// charged, beside its bytes, when it is not plain ASCII: a decoder
/// reserves three bytes for each byte of such text.
pub const DATA_SET_INFLATED_TEXT_CHARGE: u64 = 4;

/// The most values a multi-valued string may have for the catalog read to
/// keep it. No catalog field is read from a longer list, and each value of
/// one holds far more than its bytes.
pub const DATA_SET_CATALOG_MAX_VALUES: u64 = 4096;

/// How deep sequences may nest: an element of the data set is at depth 0,
/// an element of an item of a top-level sequence at depth 1.
pub const DATA_SET_MAX_DEPTH: usize = 64;

/// Where the magic code of a Part 10 file is, and where both reads begin.
const MAGIC_CODE_OFFSET: u64 = 128;

/// A value shorter than this is passed over by reading it through the
/// buffer the reads already share; a longer one by seeking.
const SEEK_OVER_MIN_BYTES: u64 = 16 * 1024;

/// Whether the tag endpoints show an element by its length alone: Pixel
/// Data and every bulk binary value representation.
pub fn shown_by_length(tag: Tag, vr: VR) -> bool {
    tag == tags::PIXEL_DATA || matches!(vr, VR::OB | VR::OW | VR::OD | VR::OF | VR::UN | VR::OL)
}

/// The header of a data set's own pixel element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelElementHeader {
    /// Pixel Data, Float Pixel Data or Double Float Pixel Data.
    pub tag: Tag,
    /// The value length of a native element; `None` for encapsulated pixel
    /// data.
    pub length: Option<u32>,
}

/// What the catalog is built from.
pub struct CatalogDataSet {
    /// The data set before Float Pixel Data (7FE0,0008), without the values
    /// the limits leave out, under the file's own meta group.
    pub object: DefaultDicomObject,
    /// A sequence item before the pixel data declares an odd length, which
    /// a conformant data set never does.
    pub odd_item_length: bool,
    /// The top-level pixel element, or why the data set could not be walked
    /// to it.
    pub pixels: Result<Option<PixelElementHeader>>,
}

/// Reads what the catalog needs of a Part 10 file: its meta group, its data
/// set up to, and excluding, Float Pixel Data, and the header of its
/// top-level pixel element.
///
/// `source` is positioned at the magic code (offset 128) and `file_length`
/// is the length of the file it reads. `None` is a file whose meta group or
/// data set before the pixel data does not parse or breaks a limit of this
/// module.
///
/// Only top-level elements count as the pixel element, so one nested in a
/// sequence (an Icon Image Sequence, for example) or pixel-tag bytes inside
/// another value never make a no-pixel object look like an image.
pub fn read_for_catalog<R: Read + Seek>(
    source: &mut BufReader<R>,
    file_length: u64,
) -> Option<CatalogDataSet> {
    let (meta, transfer_syntax, start) = read_meta(source, file_length).ok()?;
    let plan = Plan {
        keep: |top, header| {
            top < tags::FLOAT_PIXEL_DATA
                && (header.len.0 <= DATA_SET_VALUE_MAX_BYTES || is_overlay_data(header.tag))
        },
        stop_at: |header| {
            header.vr != VR::SQ
                && matches!(
                    header.tag,
                    tags::PIXEL_DATA | tags::FLOAT_PIXEL_DATA | tags::DOUBLE_FLOAT_PIXEL_DATA
                )
        },
        list_passed_over: false,
        ends_inside_passed_over: false,
        ends_past_depth: false,
        ends_at_padding: false,
        max_values: Some(DATA_SET_CATALOG_MAX_VALUES),
    };
    let walked = walk_data_set(source, start, file_length, transfer_syntax, &plan).ok()?;
    let reached_pixel_region = walked
        .last_top_level
        .is_some_and(|tag| tag >= tags::FLOAT_PIXEL_DATA);
    let pixels = match walked.end {
        Ok(End::Stopped(header)) => Ok(Some(PixelElementHeader {
            tag: header.tag,
            length: header.len.get(),
        })),
        Ok(_) => Ok(None),
        Err(error) if reached_pixel_region => Err(error),
        Err(_) => return None,
    };
    let mut object = walked.object;
    object.retain(|element| element.header().tag < tags::FLOAT_PIXEL_DATA);
    Some(CatalogDataSet {
        object: object.with_exact_meta(meta),
        odd_item_length: walked.odd_item_length,
        pixels,
    })
}

fn is_overlay_data(tag: Tag) -> bool {
    (0x6000..=0x601e).contains(&tag.0) && tag.0 & 1 == 0 && tag.1 == 0x3000
}

/// How much of a data set a tag read walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagExtent {
    /// The elements before Float Pixel Data (7FE0,0008).
    BeforePixelData,
    /// Every element of the data set.
    Whole,
}

/// What the tag endpoints show a data set from.
pub struct TagDataSet {
    /// Every element of the extent. An element whose value was not read is
    /// empty and keeps its declared length; encapsulated Pixel Data has the
    /// length of its fragments, without the offset table.
    pub object: InMemDicomObject,
    /// The bytes of the top-level encapsulated Pixel Data's fragments,
    /// which an element's length cannot hold past 4 GiB.
    pub pixel_fragment_bytes: Option<u64>,
    /// The top-level element in which sequences nest past
    /// [`DATA_SET_MAX_DEPTH`]. The object ends inside it, at the sequence
    /// that is too deep, which is listed without items.
    pub too_deep: Option<Tag>,
}

/// Reads what the tag endpoints show of a Part 10 file's data set.
///
/// `source` is positioned at the magic code (offset 128) and `file_length`
/// is the length of the file it reads. No value the endpoints show by its
/// length ([`shown_by_length`]) and no value over
/// [`DATA_SET_VALUE_MAX_BYTES`] is read.
pub fn read_for_tags<R: Read + Seek>(
    source: &mut BufReader<R>,
    file_length: u64,
    extent: TagExtent,
) -> Result<TagDataSet> {
    let (_, transfer_syntax, start) = read_meta(source, file_length)?;
    let plan = Plan {
        keep: |_, header| {
            !shown_by_length(header.tag, header.vr) && header.len.0 <= DATA_SET_VALUE_MAX_BYTES
        },
        stop_at: match extent {
            TagExtent::BeforePixelData => |header| header.tag >= tags::FLOAT_PIXEL_DATA,
            TagExtent::Whole => |_| false,
        },
        list_passed_over: true,
        ends_inside_passed_over: true,
        ends_past_depth: true,
        ends_at_padding: true,
        max_values: None,
    };
    let walked = walk_data_set(source, start, file_length, transfer_syntax, &plan)?;
    walked.end?;
    Ok(TagDataSet {
        object: walked.object,
        pixel_fragment_bytes: walked.fragment_bytes,
        too_deep: walked.too_deep,
    })
}

/// Reads the file meta group at the magic code, after checking that every
/// value it declares is in the file: the meta reader sizes each value from
/// its header.
fn read_meta<R: Read + Seek>(
    source: &mut BufReader<R>,
    file_length: u64,
) -> Result<(FileMetaTable, &'static TransferSyntax, u64)> {
    // Positions are counted, not asked of the source: asking is a system
    // call for every file of a scan.
    let start = MAGIC_CODE_OFFSET;
    let mut position = start;
    let mut magic = [0_u8; 4];
    source.read_exact(&mut magic).context("magic code")?;
    position += 4;
    let decoder = dicom_encoding::decode::file_header_decoder();
    let (group, read) = decoder
        .decode_header(source)
        .map_err(|_| anyhow!("file meta group has no length"))?;
    position += read as u64;
    ensure!(
        group.tag == Tag(0x0002, 0x0000) && group.len == Length(4),
        "file meta group has no length"
    );
    let mut group_length = [0_u8; 4];
    source
        .read_exact(&mut group_length)
        .context("file meta group length")?;
    position += 4;
    let group_end = position + u64::from(u32::from_le_bytes(group_length));
    ensure!(
        group_end <= file_length,
        "file meta group runs past the end of the file"
    );
    while position < group_end {
        let (element, read) = decoder
            .decode_header(source)
            .map_err(|_| anyhow!("file meta element header"))?;
        position += read as u64;
        let length = element
            .len
            .get()
            .context("file meta element of undefined length")?;
        ensure!(
            u64::from(length) <= file_length.saturating_sub(position),
            "file meta value runs past the end of the file"
        );
        source
            .seek_relative(i64::from(length))
            .context("file meta value")?;
        position += u64::from(length);
    }
    // The group is a few hundred bytes, so this stays inside the buffer.
    source
        .seek_relative(start as i64 - position as i64)
        .context("return to the file meta group")?;
    let meta = FileMetaTable::from_reader(&mut *source).context("file meta group")?;
    let transfer_syntax = TransferSyntaxRegistry
        .get(meta.transfer_syntax())
        .context("unknown transfer syntax")?;
    // The meta reader has read what the check stepped over: the data set
    // starts where the check ended.
    Ok((meta, transfer_syntax, position))
}

/// What one walk reads and what it leaves out.
struct Plan {
    /// Whether the value of an element is read. The first argument is the
    /// tag of the top-level element it is, or is nested in.
    keep: fn(Tag, &DataElementHeader) -> bool,
    /// The top-level element at whose header the walk ends.
    stop_at: fn(&DataElementHeader) -> bool,
    /// Whether an element whose value was passed over is listed, empty, at
    /// its declared length.
    list_passed_over: bool,
    /// Whether a data set that ends inside a value being passed over ends
    /// there. Otherwise that is an error.
    ends_inside_passed_over: bool,
    /// Whether a sequence nested past the limit ends the walk, listed
    /// without items. Otherwise that is an error.
    ends_past_depth: bool,
    /// Whether the data set ends, without an error, at top-level bytes
    /// that are not an element: a header that cannot be read, or the tag
    /// (0000,0000), which is what zero padding parses as and no data set
    /// holds.
    ends_at_padding: bool,
    /// The most values a string may split into and still be read; a string
    /// of more is passed over.
    max_values: Option<u64>,
}

/// How a walk ended.
enum End {
    /// At the end of the data set.
    Complete,
    /// At the header of the element the plan stops at.
    Stopped(DataElementHeader),
    /// Inside a value that was being passed over.
    CutShort,
}

struct Walked {
    /// The top-level elements built before the walk ended or failed.
    object: InMemDicomObject,
    end: Result<End>,
    /// The tag of the last top-level element whose header was read.
    last_top_level: Option<Tag>,
    odd_item_length: bool,
    fragment_bytes: Option<u64>,
    too_deep: Option<Tag>,
}

/// Walks the data set that follows the file meta group, from the offset
/// `start` of the file. A deflated data set
/// is read through its transfer syntax's adapter, within the inflated
/// budget.
fn walk_data_set<R: Read + Seek>(
    source: &mut BufReader<R>,
    start: u64,
    file_length: u64,
    transfer_syntax: &TransferSyntax,
    plan: &Plan,
) -> Result<Walked> {
    match transfer_syntax.codec() {
        Codec::Dataset(Some(adapter)) => {
            let meter = Arc::new(Meter::default());
            let inflated = Metered {
                inner: adapter.adapt_reader(Box::new(source)),
                meter: Arc::clone(&meter),
            };
            let decoder = DynStatefulDecoder::new_with_ts(inflated, transfer_syntax, 0)
                .map_err(|_| anyhow!("no decoder for the transfer syntax"))?;
            Ok(walk(
                Bounded {
                    inner: decoder,
                    offset: 0,
                    end: u64::MAX,
                    seek: None,
                    meter: Some(meter),
                },
                plan,
            ))
        }
        Codec::Dataset(None) => bail!("unsupported data set encoding"),
        _ => {
            let decoder = DynStatefulDecoder::new_with_ts(source, transfer_syntax, start)
                .map_err(|_| anyhow!("no decoder for the transfer syntax"))?;
            Ok(walk(
                Bounded {
                    inner: decoder,
                    offset: 0,
                    end: file_length,
                    seek: Some(|decoder, position| decoder.seek(position)),
                    meter: None,
                },
                plan,
            ))
        }
    }
}

fn walk<D: StatefulDecode>(source: Bounded<D>, plan: &Plan) -> Walked {
    let meter = source.meter.clone();
    let mut walk = Walk {
        reader: LazyDataSetReader::new(source),
        meter: meter.clone(),
        plan,
        last_top_level: None,
        odd_item_length: false,
        fragment_bytes: None,
        too_deep: None,
    };
    // Elements go straight into the object: collecting them first would hold
    // every one twice.
    let mut object = InMemDicomObject::new_empty();
    let end = walk.top_level(&mut object).and_then(|end| {
        // The inflated budget ends a data set like the end of its file
        // does; only the meter tells them apart.
        let exhausted = meter.is_some_and(|meter| meter.exhausted.load(Ordering::Relaxed));
        ensure!(
            !(exhausted && matches!(end, End::Complete)),
            "the data set inflates to more than {DATA_SET_INFLATED_BUDGET_BYTES} bytes"
        );
        Ok(end)
    });
    Walked {
        object,
        end,
        last_top_level: walk.last_top_level,
        odd_item_length: walk.odd_item_length,
        fragment_bytes: walk.fragment_bytes,
        too_deep: walk.too_deep,
    }
}

/// What reading one element came to.
enum Step {
    Element(InMemElement),
    /// Passed over and not listed.
    PassedOver,
    Stop(DataElementHeader),
    /// The data set ended inside the element, which is listed when the plan
    /// lists what it passes over.
    CutShort(Option<InMemElement>),
    ItemEnd,
}

struct Walk<'p, D: StatefulDecode> {
    reader: LazyDataSetReader<Bounded<D>>,
    /// The budget of an inflating source, charged for what the walk builds.
    meter: Option<Arc<Meter>>,
    plan: &'p Plan,
    last_top_level: Option<Tag>,
    odd_item_length: bool,
    fragment_bytes: Option<u64>,
    too_deep: Option<Tag>,
}

impl<D: StatefulDecode> Walk<'_, D> {
    fn top_level(&mut self, object: &mut InMemDicomObject) -> Result<End> {
        loop {
            match self.step(0, None)? {
                None => return Ok(End::Complete),
                Some(Step::Element(element)) => {
                    object.put(element);
                }
                Some(Step::PassedOver) => {}
                Some(Step::Stop(header)) => return Ok(End::Stopped(header)),
                Some(Step::CutShort(element)) => {
                    if let Some(element) = element {
                        object.put(element);
                    }
                    return Ok(End::CutShort);
                }
                Some(Step::ItemEnd) => bail!("item delimiter outside an item"),
            }
        }
    }

    /// Reads the next element at `depth`, whose top-level element is `top`
    /// (`None` at depth 0). `None` at the end of the data set.
    fn step(&mut self, depth: usize, top: Option<Tag>) -> Result<Option<Step>> {
        let Some(token) = self.reader.advance() else {
            return Ok(None);
        };
        let token = match token {
            Err(_) if depth == 0 && self.plan.ends_at_padding => return Ok(None),
            token => token.map_err(|_| anyhow!("unreadable element header"))?,
        };
        let header = match token {
            LazyDataToken::ElementHeader(header) => header,
            LazyDataToken::SequenceStart { tag, len } => DataElementHeader {
                tag,
                vr: VR::SQ,
                len,
            },
            LazyDataToken::PixelSequenceStart => DataElementHeader {
                tag: tags::PIXEL_DATA,
                vr: VR::OB,
                len: Length::UNDEFINED,
            },
            LazyDataToken::ItemEnd if depth > 0 => return Ok(Some(Step::ItemEnd)),
            _ => bail!("unexpected token in a data set"),
        };
        if depth == 0 {
            if self.plan.ends_at_padding && header.tag == Tag(0, 0) {
                return Ok(None);
            }
            self.last_top_level = Some(header.tag);
            if (self.plan.stop_at)(&header) {
                return Ok(Some(Step::Stop(header)));
            }
        }
        let top = top.unwrap_or(header.tag);
        let (element, whole) = if header.vr == VR::SQ && depth >= DATA_SET_MAX_DEPTH {
            ensure!(
                self.plan.ends_past_depth,
                "sequences nest deeper than {DATA_SET_MAX_DEPTH}"
            );
            self.too_deep = Some(top);
            let empty = Value::Sequence(DataSetSequence::new(Vec::new(), header.len));
            return Ok(Some(Step::CutShort(Some(element_of(
                header, header.len, empty,
            )))));
        } else if header.vr == VR::SQ {
            let (items, whole) = self.items(depth + 1, top)?;
            let sequence = Value::Sequence(DataSetSequence::new(items, header.len));
            (Some(element_of(header, header.len, sequence)), whole)
        } else if header.len.is_undefined() {
            let (bytes, whole) = self.fragments()?;
            if depth == 0 {
                self.fragment_bytes = Some(bytes);
            }
            // The all-ones length means undefined.
            let length = Length(u32::try_from(bytes).unwrap_or(u32::MAX).min(u32::MAX - 1));
            (self.passed_over(header, length), whole)
        } else {
            let mut keep = (self.plan.keep)(top, &header);
            let LazyDataToken::LazyValue { header, decoder } = self
                .reader
                .advance()
                .context("element has no value")?
                .map_err(|_| anyhow!("unreadable element value"))?
            else {
                bail!("unexpected token for a value");
            };
            let length = u64::from(header.len.0);
            if keep {
                ensure!(
                    length <= decoder.remaining(),
                    "a value runs past the end of the data set"
                );
            }
            // What a value holds once read is not what it takes in the
            // file: a string is one string per value, and text can decode
            // to more bytes than it has. The bytes say which, before
            // anything is built from them.
            let splits = splits_into_values(header.vr);
            let text = splits || matches!(header.vr, VR::LT | VR::ST | VR::UT | VR::UR);
            let budgeted = decoder.meter.is_some();
            let capped = self
                .plan
                .max_values
                .is_some_and(|most| splits && length > most);
            let looked_at = if keep && length > 0 && ((budgeted && text) || capped) {
                Some(
                    decoder
                        .look_at(header.len.0)
                        .map_err(|_| anyhow!("unreadable element value"))?
                        .context("a value runs past the end of the data set")?,
                )
            } else {
                None
            };
            if let (Some(most), Some(looked_at)) = (self.plan.max_values, looked_at) {
                keep &= !(splits && looked_at.values > most);
            }
            if keep && budgeted {
                let values = looked_at.filter(|_| splits).map_or(1, |value| value.values);
                let expands = looked_at.is_some_and(|value| !value.plain);
                let charge = (values - 1) * DATA_SET_INFLATED_VALUE_CHARGE_BYTES
                    + if expands {
                        length * DATA_SET_INFLATED_TEXT_CHARGE
                    } else if header.vr == VR::AT {
                        length
                    } else {
                        0
                    };
                ensure!(
                    charge <= decoder.remaining(),
                    "a value would hold more than the inflated budget has left"
                );
                if let Some(meter) = &decoder.meter {
                    meter.spent.fetch_add(charge, Ordering::Relaxed);
                }
            }
            if keep {
                let value = decoder
                    .read_value_preserved(&header)
                    .map_err(|_| anyhow!("unreadable element value"))?;
                (
                    Some(element_of(header, header.len, Value::Primitive(value))),
                    true,
                )
            } else {
                let whole = decoder
                    .pass_over(header.len.0)
                    .map_err(|_| anyhow!("unreadable element value"))?;
                (self.passed_over(header, header.len), whole)
            }
        };
        if element.is_some() {
            self.charge(1);
        }
        if whole {
            return Ok(Some(element.map_or(Step::PassedOver, Step::Element)));
        }
        ensure!(
            self.plan.ends_inside_passed_over,
            "a value runs past the end of the data set"
        );
        Ok(Some(Step::CutShort(element)))
    }

    /// Charges an inflating source's budget for what was built: one unit
    /// for an element, two for an item.
    fn charge(&self, units: u64) {
        if let Some(meter) = &self.meter {
            meter.spent.fetch_add(
                units * DATA_SET_INFLATED_ELEMENT_CHARGE_BYTES,
                Ordering::Relaxed,
            );
        }
    }

    /// The listing of an element whose value was not read.
    fn passed_over(&self, header: DataElementHeader, length: Length) -> Option<InMemElement> {
        self.plan
            .list_passed_over
            .then(|| element_of(header, length, Value::Primitive(PrimitiveValue::Empty)))
    }

    /// Reads the items of a sequence whose elements are at `depth`. False
    /// when the data set ended inside one.
    fn items(&mut self, depth: usize, top: Tag) -> Result<(Vec<InMemDicomObject>, bool)> {
        let mut items = Vec::new();
        loop {
            let token = self
                .reader
                .advance()
                .context("data set ended inside a sequence")?
                .map_err(|_| anyhow!("unreadable item header"))?;
            match token {
                LazyDataToken::ItemStart { len } => {
                    self.odd_item_length |= len.get().is_some_and(|length| length % 2 != 0);
                    self.charge(2);
                    let mut item = InMemDicomObject::new_empty();
                    let whole = loop {
                        match self
                            .step(depth, Some(top))?
                            .context("data set ended inside a sequence")?
                        {
                            Step::Element(element) => {
                                item.put(element);
                            }
                            Step::PassedOver => {}
                            Step::ItemEnd => break true,
                            Step::CutShort(element) => {
                                if let Some(element) = element {
                                    item.put(element);
                                }
                                break false;
                            }
                            Step::Stop(_) => bail!("a walk stops at top-level elements only"),
                        }
                    };
                    items.push(item);
                    if !whole {
                        return Ok((items, false));
                    }
                }
                LazyDataToken::SequenceEnd => return Ok((items, true)),
                _ => bail!("unexpected token in a sequence"),
            }
        }
    }

    /// Passes over the fragments of encapsulated pixel data. Returns their
    /// bytes, without the offset table, and false when the data set ended
    /// inside them.
    fn fragments(&mut self) -> Result<(u64, bool)> {
        let mut bytes = 0_u64;
        let mut items = 0_u64;
        loop {
            match self.reader.advance() {
                Some(Ok(LazyDataToken::ItemStart { .. })) => items += 1,
                Some(Ok(LazyDataToken::ItemEnd)) => {}
                Some(Ok(LazyDataToken::LazyItemValue { len, decoder })) => {
                    // The first item is the offset table.
                    if items > 1 {
                        bytes += u64::from(len);
                    }
                    if !decoder
                        .pass_over(len)
                        .map_err(|_| anyhow!("unreadable pixel data fragment"))?
                    {
                        return Ok((bytes, false));
                    }
                }
                Some(Ok(LazyDataToken::SequenceEnd)) => return Ok((bytes, true)),
                _ => return Ok((bytes, false)),
            }
        }
    }
}

/// Whether the parser reads a value of this representation as one string
/// for each backslash-separated value.
fn splits_into_values(vr: VR) -> bool {
    matches!(
        vr,
        VR::AE
            | VR::AS
            | VR::CS
            | VR::DA
            | VR::DS
            | VR::DT
            | VR::IS
            | VR::LO
            | VR::PN
            | VR::SH
            | VR::TM
            | VR::UC
            | VR::UI
    )
}

fn element_of(
    header: DataElementHeader,
    length: Length,
    value: Value<InMemDicomObject>,
) -> InMemElement {
    InMemElement::new_with_len(header.tag, header.vr, length, value)
}

/// The budget of one read of a deflated data set.
#[derive(Default)]
struct Meter {
    /// The bytes inflated.
    read: AtomicU64,
    /// The bytes inflated and the charges for what was built from them.
    spent: AtomicU64,
    /// A read was asked for after the budget was spent.
    exhausted: AtomicBool,
    /// Bytes that were looked at and are still to be read: an inflating
    /// source cannot go back, so they are handed out again from here.
    looked_at: std::sync::Mutex<(Vec<u8>, usize)>,
}

impl Meter {
    fn left(&self) -> u64 {
        DATA_SET_INFLATED_BUDGET_BYTES.saturating_sub(self.spent.load(Ordering::Relaxed))
    }
}

/// A reader that hands out bytes while its meter's budget lasts and counts
/// them.
struct Metered<R> {
    inner: R,
    meter: Arc<Meter>,
}

impl<R: Read> Read for Metered<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        {
            // Bytes looked at were counted and charged when they were
            // inflated.
            let mut looked_at = self.meter.looked_at.lock().expect("meter lock");
            let (bytes, at) = &mut *looked_at;
            if *at < bytes.len() {
                let count = buffer.len().min(bytes.len() - *at);
                buffer[..count].copy_from_slice(&bytes[*at..*at + count]);
                *at += count;
                if *at == bytes.len() {
                    *looked_at = (Vec::new(), 0);
                }
                self.meter.read.fetch_add(count as u64, Ordering::Relaxed);
                return Ok(count);
            }
        }
        let left = self.meter.left();
        if left == 0 {
            self.meter.exhausted.store(true, Ordering::Relaxed);
            return Ok(0);
        }
        let allowed = buffer
            .len()
            .min(usize::try_from(left).unwrap_or(usize::MAX));
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.meter.read.fetch_add(read as u64, Ordering::Relaxed);
        self.meter.spent.fetch_add(read as u64, Ordering::Relaxed);
        Ok(read)
    }
}

/// What a value's bytes say of what it will hold once it is read.
#[derive(Clone, Copy)]
struct LookedAt {
    /// The values a string splits into: one more than its separators.
    values: u64,
    /// Every byte is ASCII and none is an escape, so the text decodes to as
    /// many bytes as it has.
    plain: bool,
}

/// A stateful decoder that knows where its data set must end and can pass
/// over a value without reading it.
struct Bounded<D: StatefulDecode> {
    inner: D,
    /// The bytes passed over by seeking, which `inner` did not count, less
    /// the bytes looked at and given back, which it counted twice.
    offset: i64,
    /// The length of the file, which a data set read by seeking cannot
    /// reach past. An inflating source ends with its meter's budget.
    end: u64,
    /// Seeks `inner`'s source to a position, when it can seek.
    seek: Option<fn(&mut D, u64) -> DecodeResult<()>>,
    /// The count of an inflating source.
    meter: Option<Arc<Meter>>,
}

impl<D: StatefulDecode> Bounded<D> {
    /// The bytes the data set can still hold from here.
    fn remaining(&self) -> u64 {
        match &self.meter {
            Some(meter) => meter.left(),
            None => self.end.saturating_sub(self.position()),
        }
    }

    /// Looks at the next `length` bytes, which the caller has checked
    /// against [`Self::remaining`], and leaves them to be read: counts the
    /// values a string of them splits into and says whether any byte could
    /// decode to more than itself. `None` when the data set ends first.
    fn look_at(&mut self, length: u32) -> DecodeResult<Option<LookedAt>> {
        let mut bytes = Vec::new();
        self.inner.read_to_vec(length, &mut bytes)?;
        self.offset -= i64::from(length);
        let whole = bytes.len() == length as usize;
        let looked_at = LookedAt {
            values: 1 + bytes.iter().filter(|byte| **byte == b'\\').count() as u64,
            // Bytes of 0x80 and above, and an escape that changes what the
            // bytes behind it mean.
            plain: bytes.iter().all(|byte| *byte < 0x80 && *byte != 0x1b),
        };
        match (&self.meter, self.seek) {
            (Some(meter), _) => *meter.looked_at.lock().expect("meter lock") = (bytes, 0),
            (None, Some(seek)) => {
                let target = self.position();
                seek(&mut self.inner, target)?;
            }
            (None, None) => {}
        }
        Ok(whole.then_some(looked_at))
    }

    /// Passes over the next `length` bytes. False when the data set ends
    /// first, in which case nothing may be read after.
    fn pass_over(&mut self, length: u32) -> DecodeResult<bool> {
        let bytes = u64::from(length);
        if bytes > self.remaining() {
            return Ok(false);
        }
        match self.seek {
            Some(seek) if bytes >= SEEK_OVER_MIN_BYTES => {
                self.offset += bytes as i64;
                let target = self.position();
                seek(&mut self.inner, target)?;
                Ok(true)
            }
            _ => {
                let read = |meter: &Option<Arc<Meter>>| {
                    meter
                        .as_ref()
                        .map(|meter| meter.read.load(Ordering::Relaxed))
                };
                let before = read(&self.meter);
                self.inner.skip_bytes(length)?;
                Ok(match (before, read(&self.meter)) {
                    (Some(before), Some(after)) => after - before == bytes,
                    _ => true,
                })
            }
        }
    }
}

impl<D: StatefulDecode> StatefulDecode for Bounded<D> {
    type Reader = D::Reader;

    fn decode_header(&mut self) -> DecodeResult<DataElementHeader> {
        self.inner.decode_header()
    }

    fn decode_item_header(&mut self) -> DecodeResult<dicom_core::header::SequenceItemHeader> {
        self.inner.decode_item_header()
    }

    fn read_value(&mut self, header: &DataElementHeader) -> DecodeResult<PrimitiveValue> {
        self.inner.read_value(header)
    }

    fn read_value_preserved(&mut self, header: &DataElementHeader) -> DecodeResult<PrimitiveValue> {
        self.inner.read_value_preserved(header)
    }

    fn read_value_bytes(&mut self, header: &DataElementHeader) -> DecodeResult<PrimitiveValue> {
        self.inner.read_value_bytes(header)
    }

    fn read_to_vec(&mut self, length: u32, vec: &mut Vec<u8>) -> DecodeResult<()> {
        self.inner.read_to_vec(length, vec)
    }

    fn read_u32_to_vec(&mut self, length: u32, vec: &mut Vec<u32>) -> DecodeResult<()> {
        self.inner.read_u32_to_vec(length, vec)
    }

    fn read_to<W>(&mut self, length: u32, out: W) -> DecodeResult<()>
    where
        Self: Sized,
        W: std::io::Write,
    {
        self.inner.read_to(length, out)
    }

    fn skip_bytes(&mut self, length: u32) -> DecodeResult<()> {
        self.inner.skip_bytes(length)
    }

    fn seek(&mut self, position: u64) -> DecodeResult<()>
    where
        Self::Reader: Seek,
    {
        self.inner.seek(position)
    }

    fn position(&self) -> u64 {
        self.inner.position().saturating_add_signed(self.offset)
    }
}

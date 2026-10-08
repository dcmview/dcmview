//! Which frames of its file an annotation applies to.

use crate::validate::Invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The frames an annotation applies to (`docs/design/annotation-model.md`
/// 2.3): every frame of the file, or a set of zero-based frame indices.
///
/// On the wire: the string `"all"`, or an object `{ "set": [0, 2] }`, which
/// may also carry `"as_written"` (see [`FrameSet`]).
///
/// `All` is a real value, not a missing list. A set with several frames means
/// the same 2D shape on each of them; it is not a 3D shape. An explicit set
/// that happens to name every frame stays a set: `{ "set": [0] }` on a
/// single-frame file is not `"all"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(from = "FrameScopeWire", into = "FrameScopeWire")]
#[ts(type = "\"all\" | FrameSet")]
pub enum FrameScope {
    All,
    Set(FrameSet),
}

/// An explicit set of frames.
///
/// Invariants, checked by [`FrameScope::validate`] and not by
/// deserialization:
///
/// - `set` is not empty, strictly ascending (so sorted and without repeats),
///   holds at most 65,536 indices ([`crate::limits::MAX_FRAMES_IN_SET`]), and
///   every index is below the file's frame count.
/// - `as_written`, when present, holds at most 65,536 indices and names
///   exactly the frames in `set`: sorted and with repeats removed it equals
///   `set`.
///
/// # The list as written
///
/// Owner decision, EMBED parity amendment of 2026-10-05: "Frame lists are
/// kept as written. A per-ROI frame list that is unsorted or repeats a frame
/// round-trips through EMBED import and export unchanged." `as_written` is
/// that list. It is present only when the list an import read differs from
/// `set` (`[2, 0, 0]` beside `[0, 2]`), so a consumer that reads `set` alone
/// is always right about which frames are meant. An edit of the annotation's
/// frames replaces the whole scope and so drops it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, TS)]
pub struct FrameSet {
    pub set: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub as_written: Option<Vec<u32>>,
}

impl FrameScope {
    /// The scope for exactly these frames, sorted and with repeats removed,
    /// with nothing kept as written.
    pub fn set(mut frames: Vec<u32>) -> FrameScope {
        frames.sort_unstable();
        frames.dedup();
        FrameScope::Set(FrameSet {
            set: frames,
            as_written: None,
        })
    }

    /// The scope for a frame list read from a format that keeps its order
    /// and repeats (EMBED's `ROI_frames`). `set` is the list sorted with
    /// repeats removed; `as_written` is the list itself when that differs
    /// from `set`, and absent when the list was already in that form.
    ///
    /// The list is taken as given: an empty list gives an empty set, which
    /// [`FrameScope::validate`] refuses. Reading an empty list as "every
    /// frame" is the importing adapter's decision, made before it calls this.
    pub fn from_written(frames: Vec<u32>) -> FrameScope {
        let mut set = frames.clone();
        set.sort_unstable();
        set.dedup();
        let as_written = (set != frames).then_some(frames);
        Self::Set(FrameSet { set, as_written })
    }

    /// The frame list to write back to a format that keeps order and
    /// repeats: `as_written` when present, otherwise `set`. `None` for
    /// `All`.
    pub fn written(&self) -> Option<&[u32]> {
        match self {
            Self::All => None,
            Self::Set(frames) => Some(frames.as_written.as_deref().unwrap_or(&frames.set)),
        }
    }

    /// Whether the scope covers `frame`. `All` covers every frame.
    pub fn contains(&self, frame: u32) -> bool {
        match self {
            Self::All => true,
            Self::Set(frames) => frames.set.contains(&frame),
        }
    }

    /// Checks the invariants on [`FrameSet`] for a file of `frame_count`
    /// frames. `All` is valid for any frame count, including 0.
    ///
    /// Bounded work: a list longer than
    /// [`crate::limits::MAX_FRAMES_IN_SET`] is refused on its length alone.
    ///
    /// Violation codes: `frames_empty`, `frames_not_normalized`,
    /// `frame_out_of_range`, `frames_as_written_mismatch`,
    /// `too_many_frames`.
    pub fn validate(&self, frame_count: u32) -> Result<(), Invalid> {
        crate::Check::check(self, frame_count)
    }
}

/// The string `"all"` for every frame of the file, or an explicit set.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
enum FrameScopeWire {
    Keyword(AllFrames),
    Set(FrameSet),
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum AllFrames {
    All,
}

impl From<FrameScopeWire> for FrameScope {
    fn from(wire: FrameScopeWire) -> FrameScope {
        match wire {
            FrameScopeWire::Keyword(AllFrames::All) => FrameScope::All,
            FrameScopeWire::Set(set) => FrameScope::Set(set),
        }
    }
}

impl From<FrameScope> for FrameScopeWire {
    fn from(scope: FrameScope) -> FrameScopeWire {
        match scope {
            FrameScope::All => FrameScopeWire::Keyword(AllFrames::All),
            FrameScope::Set(set) => FrameScopeWire::Set(set),
        }
    }
}

impl JsonSchema for FrameScope {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "FrameScope".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        FrameScopeWire::json_schema(generator)
    }
}

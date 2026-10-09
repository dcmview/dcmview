use thiserror::Error;

/// A reason for a failed decode in dcmview's own words, for a response
/// body: fixed wording, and numbers the viewer computed or the catalog entry
/// holds. Never text of the file, and never the text of a library's error:
/// a decoder's message can quote bytes of the file it refused.
///
/// A decode error shows the outermost `Stated` of its cause chain and
/// nothing else of it ([`PixelError::Decode`]). Put one in the chain with
/// [`Stated::error`], or over a library's error with
/// `anyhow::Context::context`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct Stated(String);

impl Stated {
    pub(crate) fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }

    /// An error that is this reason.
    pub(crate) fn error(reason: impl Into<String>) -> anyhow::Error {
        anyhow::Error::new(Self::new(reason))
    }
}

/// What a decode error says in a response: its context and the reason the
/// viewer stated, if it stated one.
fn decode_text(context: &str, source: &anyhow::Error) -> String {
    match source.downcast_ref::<Stated>() {
        Some(stated) => format!("{context}: {stated}"),
        None => context.to_string(),
    }
}

/// A transfer syntax UID as a response may show it: only when it is written
/// as a UID is, since the text comes from the file.
fn shown_uid(uid: &str) -> String {
    let shaped = !uid.is_empty()
        && uid.len() <= 64
        && uid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.');
    if shaped {
        format!(": {uid}")
    } else {
        String::new()
    }
}

#[derive(Debug, Error)]
pub enum PixelError {
    #[error("file {file_index} has no pixel data")]
    NoPixelData { file_index: usize },
    #[error("frame {frame} is out of range: the file has {frame_count} frame(s)")]
    FrameOutOfRange { frame: u32, frame_count: u32 },
    #[error("unsupported transfer syntax{}", shown_uid(.0))]
    UnsupportedTransferSyntax(String),
    #[error("unsupported pixel layout: {0}")]
    UnsupportedLayout(String),
    #[error("invalid window request: {0}")]
    InvalidWindow(String),
    /// Running decodes hold the decode memory budget and too many requests
    /// already wait for it. Nothing is wrong with the request: it succeeds
    /// once decodes finish.
    #[error("the viewer is busy decoding other frames; try again shortly")]
    DecodeBusy,
    /// The frame's decode needs more memory than its class may ever reserve
    /// (`limit_bytes`: the decode memory budget, or the share of it that
    /// thumbnails may use when `background`). Waiting cannot help; a larger
    /// `--decode-memory` can.
    #[error(
        "decoding this frame needs {needed_bytes} bytes of memory and {} is {limit_bytes} bytes; \
         start dcmview with a larger --decode-memory to decode it",
        if *background { "the share of the decode memory budget that thumbnails may use" } else { "the decode memory budget" }
    )]
    DecodeMemoryExceeded {
        needed_bytes: u64,
        limit_bytes: u64,
        background: bool,
    },
    /// Rendered as its context and the reason the viewer [`Stated`], if
    /// any: this text goes into response bodies and the warning log. The
    /// rest of the cause chain, which may be a library's text, is
    /// [`PixelError::detail`].
    #[error("{}", decode_text(.context, .source))]
    Decode {
        context: &'static str,
        #[source]
        source: anyhow::Error,
    },
}

impl PixelError {
    /// `FrameOutOfRange` unless `frame` is one of the file's frames.
    pub(crate) fn ensure_frame(frame: u32, frame_count: u32) -> Result<(), Self> {
        if frame < frame_count {
            Ok(())
        } else {
            Err(Self::FrameOutOfRange { frame, frame_count })
        }
    }

    pub(crate) fn frame_decode(source: anyhow::Error) -> Self {
        Self::Decode {
            context: "frame decode failed",
            source,
        }
    }

    /// The whole cause chain of a decode error, for the debug log of a
    /// session that may log what a library says of a file. `None` for the
    /// other variants, whose text is complete.
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Decode { context, source } => Some(format!("{context}: {source:#}")),
            _ => None,
        }
    }

    /// An equivalent error for each request that shared one decode: same
    /// variant (and so the same HTTP status), with the source as text under
    /// the reason it stated.
    pub(crate) fn duplicate(&self) -> Self {
        match self {
            Self::NoPixelData { file_index } => Self::NoPixelData {
                file_index: *file_index,
            },
            Self::FrameOutOfRange { frame, frame_count } => Self::FrameOutOfRange {
                frame: *frame,
                frame_count: *frame_count,
            },
            Self::UnsupportedTransferSyntax(uid) => Self::UnsupportedTransferSyntax(uid.clone()),
            Self::UnsupportedLayout(reason) => Self::UnsupportedLayout(reason.clone()),
            Self::InvalidWindow(message) => Self::InvalidWindow(message.clone()),
            Self::DecodeBusy => Self::DecodeBusy,
            Self::DecodeMemoryExceeded {
                needed_bytes,
                limit_bytes,
                background,
            } => Self::DecodeMemoryExceeded {
                needed_bytes: *needed_bytes,
                limit_bytes: *limit_bytes,
                background: *background,
            },
            Self::Decode { context, source } => {
                let text = anyhow::anyhow!("{source:#}");
                Self::Decode {
                    context,
                    source: match source.downcast_ref::<Stated>() {
                        Some(stated) => text.context(stated.clone()),
                        None => text,
                    },
                }
            }
        }
    }

    pub(crate) fn raw_decode(source: anyhow::Error) -> Self {
        Self::Decode {
            context: "raw frame decode failed",
            source,
        }
    }
}

pub type PixelResult<T> = std::result::Result<T, PixelError>;

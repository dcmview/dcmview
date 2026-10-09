use thiserror::Error;

#[derive(Debug, Error)]
pub enum PixelError {
    #[error("file {file_index} has no pixel data")]
    NoPixelData { file_index: usize },
    #[error("frame {frame} is out of range: the file has {frame_count} frame(s)")]
    FrameOutOfRange { frame: u32, frame_count: u32 },
    #[error("unsupported transfer syntax: {0}")]
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
    /// Rendered with the source's whole cause chain: the decoder's own
    /// reason is what the viewer shows.
    #[error("{context}: {source:#}")]
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

    /// An equivalent error for each request that shared one decode: same
    /// variant (and so the same HTTP status), with the source as text.
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
            Self::Decode { context, source } => Self::Decode {
                context,
                source: anyhow::anyhow!("{source:#}"),
            },
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

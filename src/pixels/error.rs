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

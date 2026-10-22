use serde::{Deserialize, Serialize};

/// Every failure the engine can report. Each variant maps to a stable
/// machine-readable `code` so the Mac shell and the CLI can react to it
/// without parsing English.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    UnsupportedInput(String),
    #[error("can't turn {from} into {to}")]
    UnsupportedRoute { from: String, to: String },
    #[error("{0}")]
    NothingToDo(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("PDF error: {0}")]
    Pdf(String),
    #[error("FFmpeg isn't installed. Install it or bundle it next to the veer binary.")]
    FfmpegMissing,
    #[error("FFmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("{0}")]
    Missing(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    TooLarge(String),
    /// A helper program (the Vision helper) failed.
    #[error("{0}")]
    Helper(String),
    #[error("cancelled")]
    Cancelled,
    #[error("timed out after {0} s")]
    Timeout(u64),
    #[error("the conversion worker crashed: {0}")]
    WorkerCrashed(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Error::UnsupportedInput(_) => "unsupported_input",
            Error::UnsupportedRoute { .. } => "unsupported_route",
            Error::NothingToDo(_) => "nothing_to_do",
            Error::Io(_) => "io",
            Error::Image(_) => "image",
            Error::Pdf(_) => "pdf",
            Error::FfmpegMissing => "ffmpeg_missing",
            Error::Ffmpeg(_) => "ffmpeg",
            Error::Missing(_) => "missing_tool",
            Error::Invalid(_) => "invalid",
            Error::TooLarge(_) => "too_large",
            Error::Helper(_) => "helper",
            Error::Cancelled => "cancelled",
            Error::Timeout(_) => "timeout",
            Error::WorkerCrashed(_) => "worker_crashed",
            Error::Internal(_) => "internal",
        }
    }

    pub fn info(&self) -> ErrorInfo {
        ErrorInfo {
            code: self.code().to_string(),
            message: self.to_string(),
        }
    }
}

impl From<lopdf::Error> for Error {
    fn from(e: lopdf::Error) -> Self {
        Error::Pdf(e.to_string())
    }
}

/// Serializable form of [`Error`] used on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

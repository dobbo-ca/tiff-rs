use std::{fmt, io};

/// One variant per Go error of tiff-go. Messages carry numbers and tag ids only, never file
/// content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `*LimitError` (wrapping `ErrLimit`): `name` is the cap that tripped. `got` is 0 when the
    /// part that refused (the Deflate reader) does not say by how much.
    Limit {
        name: &'static str,
        got: u64,
        max: u64,
    },
    /// `ErrCorrupt`: structurally invalid input.
    Corrupt(String),
    /// `ErrUnsupported`: valid TIFF this crate does not decode.
    Unsupported(String),
    /// An error from the caller's [`ReadAt`](crate::ReadAt), as its kind.
    Io(io::ErrorKind),
    /// The allocator refused a size that is within the limits.
    OutOfMemory,
    /// The cancel flag was set (Go's `ctx.Err()`).
    Cancelled,
    /// The `rows` callback returned `ControlFlow::Break` (Go returns the callback's own error).
    Stopped,
}

pub(crate) fn corrupt(s: impl Into<String>) -> Error {
    Error::Corrupt(s.into())
}

pub(crate) fn unsupported(s: impl Into<String>) -> Error {
    Error::Unsupported(s.into())
}

pub(crate) fn limit(name: &'static str, got: u64, max: u64) -> Error {
    Error::Limit { name, got, max }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Limit { name, got, max } => {
                write!(f, "tiff: limit exceeded: {name} {got} > {max}")
            }
            Self::Corrupt(s) => write!(f, "tiff: corrupt: {s}"),
            Self::Unsupported(s) => write!(f, "tiff: unsupported: {s}"),
            Self::Io(k) => write!(f, "tiff: read error: {k}"),
            Self::OutOfMemory => f.write_str("tiff: out of memory"),
            Self::Cancelled => f.write_str("tiff: cancelled"),
            Self::Stopped => f.write_str("tiff: stopped by the caller"),
        }
    }
}

impl std::error::Error for Error {}

/// How the codec readers (`Read` impls) carry an [`Error`] through `io::Error`.
impl From<Error> for io::Error {
    fn from(e: Error) -> Self {
        io::Error::new(io::ErrorKind::InvalidData, e)
    }
}

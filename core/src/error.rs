use std::fmt;
use std::io;

#[derive(Debug)]
pub enum Error {
    /// A change was built against an older version of the buffer.
    StaleVersion {
        base: u64,
        current: u64,
    },
    /// The byte offset is out of bounds or not on a char boundary.
    InvalidPosition(usize),
    /// Edits in one change set overlap each other.
    OverlappingEdits,
    /// A selection must contain at least one range, and the primary index must be in bounds.
    InvalidSelection,
    InvalidPattern(String),
    /// A plugin's buffer, which only its owner may change.
    ReadOnly,
    /// The buffer has no file path to save to.
    NoPath,
    Io(io::Error),
    /// Loading or starting a plugin failed.
    Plugin(String),
    /// config.toml is invalid.
    Config(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::StaleVersion { base, current } => {
                write!(f, "stale version {base} (current is {current})")
            }
            Error::InvalidPosition(pos) => write!(f, "invalid position {pos}"),
            Error::OverlappingEdits => f.write_str("edits overlap"),
            Error::InvalidSelection => f.write_str("invalid selection"),
            Error::InvalidPattern(msg) => write!(f, "invalid pattern: {msg}"),
            Error::ReadOnly => f.write_str("the buffer is read-only"),
            Error::NoPath => f.write_str("buffer has no path"),
            Error::Io(err) => err.fmt(f),
            Error::Plugin(msg) => f.write_str(msg),
            Error::Config(msg) => write!(f, "config.toml: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error::Io(err)
    }
}

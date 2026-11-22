use std::fmt;
use std::io;
use std::path::Path;

/// A user-facing error. An I/O failure keeps its `io::Error`, so a caller can look at
/// the kind and decide whether to retry, skip, warn or fail, and the message is only
/// formatted for the user at the CLI boundary.
#[derive(Debug)]
pub struct Error {
    context: String,
    source: Option<io::Error>,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Error {
            context: msg.into(),
            source: None,
        }
    }

    /// An I/O failure on `path`.
    pub fn io(path: &Path, source: io::Error) -> Self {
        Error {
            context: path.display().to_string(),
            source: Some(source),
        }
    }

    /// An I/O failure with a description of what was being done.
    pub fn io_context(context: impl Into<String>, source: io::Error) -> Self {
        Error {
            context: context.into(),
            source: Some(source),
        }
    }

    /// The kind of the underlying I/O error, if there is one.
    pub fn io_kind(&self) -> Option<io::ErrorKind> {
        self.source.as_ref().map(io::Error::kind)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(source) => write!(f, "{}: {source}", self.context),
            None => f.write_str(&self.context),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e as _)
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Error {
            context: "I/O error".to_string(),
            source: Some(source),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

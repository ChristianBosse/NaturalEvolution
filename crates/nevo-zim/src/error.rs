use std::fmt;

#[derive(Debug)]
pub enum ZimError {
    /// The file couldn't be read (missing, no permission, …).
    Io(std::io::Error),
    /// The file is not a ZIM file, or it is damaged.
    Corrupt(String),
    /// Something in the file is bigger than NEVO accepts. The limits protect small nodes from
    /// files built to exhaust their memory.
    TooLarge { what: &'static str, limit: u64 },
}

impl fmt::Display for ZimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZimError::Io(e) => write!(f, "file error: {e}"),
            ZimError::Corrupt(why) => write!(f, "damaged or invalid ZIM file: {why}"),
            ZimError::TooLarge { what, limit } => write!(f, "{what} is larger than the {limit} byte limit"),
        }
    }
}

impl std::error::Error for ZimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ZimError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ZimError {
    fn from(e: std::io::Error) -> ZimError {
        ZimError::Io(e)
    }
}

/// Converts the `zim` crate's errors without exposing its types in our API.
pub(crate) fn from_zim(e: zim::Error) -> ZimError {
    match e {
        zim::Error::Parsing(inner) => match inner.downcast::<std::io::Error>() {
            // A read past the end inside the file is damage, not an I/O problem.
            Ok(io) if io.kind() == std::io::ErrorKind::UnexpectedEof => ZimError::Corrupt(io.to_string()),
            Ok(io) => ZimError::Io(*io),
            Err(other) => ZimError::Corrupt(other.to_string()),
        },
        other => ZimError::Corrupt(other.to_string()),
    }
}

pub(crate) fn corrupt(why: impl Into<String>) -> ZimError {
    ZimError::Corrupt(why.into())
}

pub type Result<T> = std::result::Result<T, ZimError>;

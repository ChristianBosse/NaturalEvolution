use std::fmt;

use crate::Kind;

/// Why a file was refused at import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejected {
    TooLarge {
        kind: Kind,
        size: u64,
        max: u64,
    },
    /// The image header is missing, cut short or says the image is empty.
    BadImageHeader,
    /// Width × height (or one side) is above the limit: decoding it would exhaust memory.
    ImageTooLarge {
        width: u32,
        height: u32,
    },
    /// The zip structure can't be read.
    BadArchive(String),
    /// The zip unpacks to far more than it weighs, or has too many entries.
    ZipBomb(String),
    /// A zip entry name is absolute or climbs out with `..`.
    UnsafeArchivePath(String),
}

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rejected::TooLarge { kind, size, max } => {
                write!(
                    f,
                    "{} file is {size} bytes, more than the {max} byte limit",
                    kind.mime()
                )
            }
            Rejected::BadImageHeader => write!(f, "image header is missing or damaged"),
            Rejected::ImageTooLarge { width, height } => write!(f, "image is {width}×{height} pixels, too large"),
            Rejected::BadArchive(why) => write!(f, "damaged zip archive: {why}"),
            Rejected::ZipBomb(why) => write!(f, "zip archive refused: {why}"),
            Rejected::UnsafeArchivePath(name) => write!(f, "zip entry has an unsafe path: {name}"),
        }
    }
}

impl std::error::Error for Rejected {}

/// Why text could not be extracted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractError {
    /// There is no text extractor for this type (images, media, ZIM, …).
    Unsupported(Kind),
    /// The file failed a safety check (EPUBs are checked again before reading).
    Rejected(Rejected),
    /// The file is damaged or uses something this crate refuses to read.
    Malformed(String),
    /// A parser panicked. The message is the panic's, if it was a string.
    Panicked(String),
    /// Extraction took longer than [`Limits::timeout`](crate::Limits::timeout).
    TimedOut,
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtractError::Unsupported(kind) => write!(f, "no text extraction for {}", kind.mime()),
            ExtractError::Rejected(r) => write!(f, "{r}"),
            ExtractError::Malformed(why) => write!(f, "can't read file: {why}"),
            ExtractError::Panicked(why) => write!(f, "parser crashed: {why}"),
            ExtractError::TimedOut => write!(f, "extraction took too long"),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<Rejected> for ExtractError {
    fn from(r: Rejected) -> ExtractError {
        ExtractError::Rejected(r)
    }
}

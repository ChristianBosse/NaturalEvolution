use std::fmt;

/// Everything that can make bytes or an object invalid under the format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Malformed or non-canonical CBOR.
    Cbor(&'static str),
    /// The encoded object is larger than [`crate::MAX_OBJECT_BYTES`].
    TooLarge,
    /// The object's `t` field is not the type that was asked for.
    WrongType { expected: &'static str, found: String },
    /// The object's `v` field is a format version this code doesn't know.
    UnsupportedVersion(i128),
    /// A required field is absent.
    Missing(&'static str),
    /// A field is present but its value breaks the spec.
    Invalid { field: &'static str, reason: String },
    /// The signature doesn't match the signer's key.
    BadSignature,
    /// An ID or key in text or binary form is malformed.
    BadId(&'static str),
    /// The operating system's random number source failed.
    Random,
}

impl Error {
    pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> Error {
        Error::Invalid {
            field,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Cbor(why) => write!(f, "invalid CBOR: {why}"),
            Error::TooLarge => write!(f, "object is too large"),
            Error::WrongType { expected, found } => write!(f, "expected a `{expected}` object, found `{found}`"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported format version {v}"),
            Error::Missing(field) => write!(f, "missing field `{field}`"),
            Error::Invalid { field, reason } => write!(f, "invalid `{field}`: {reason}"),
            Error::BadSignature => write!(f, "signature does not verify"),
            Error::BadId(why) => write!(f, "invalid ID or key: {why}"),
            Error::Random => write!(f, "no random numbers available from the operating system"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

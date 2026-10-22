use std::fmt;

use nevo_store::StoreError;

#[derive(Debug)]
pub enum BundleError {
    Store(StoreError),
    Format(nevo_format::Error),
    Io(std::io::Error),
    /// The file doesn't start with a valid, signed `bundle.cbor`, so nothing in it is used.
    NotABundle(String),
    /// What was asked doesn't fit one bundle (too many blobs or log entries). Choose fewer
    /// nodes, topics or a size limit.
    TooLarge(String),
    /// The output already exists, or the folder for a plain export isn't empty.
    Target(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Store(e) => write!(f, "{e}"),
            BundleError::Format(e) => write!(f, "{e}"),
            BundleError::Io(e) => write!(f, "file error: {e}"),
            BundleError::NotABundle(why) => write!(f, "not a usable .nevo bundle: {why}"),
            BundleError::TooLarge(why) => write!(f, "too much for one bundle: {why}"),
            BundleError::Target(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for BundleError {}

impl From<StoreError> for BundleError {
    fn from(e: StoreError) -> BundleError {
        BundleError::Store(e)
    }
}

impl From<nevo_format::Error> for BundleError {
    fn from(e: nevo_format::Error) -> BundleError {
        BundleError::Format(e)
    }
}

impl From<std::io::Error> for BundleError {
    fn from(e: std::io::Error) -> BundleError {
        BundleError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, BundleError>;

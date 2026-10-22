use std::fmt;

use nevo_format::{Id, PublicKey};

#[derive(Debug)]
pub enum StoreError {
    /// The bytes are not a valid object (see [`nevo_format::Error`]).
    Format(nevo_format::Error),
    Db(rusqlite::Error),
    Io(std::io::Error),
    /// A blob's content doesn't hash to the ID it was supposed to have.
    BlobMismatch {
        expected: Id,
        actual: Id,
    },
    /// A log entry doesn't continue the log, or contradicts the node's signed head.
    Log(String),
    /// The node signed two different versions of its log. The evidence is kept.
    Forked {
        node: PublicKey,
        seq: u64,
    },
    /// The object must not leave this node (visibility `node` or `private`, or it points at
    /// such content).
    NotShareable,
    /// The database was written by a newer (or unknown) version of NEVO.
    SchemaVersion(String),
    /// A number is too big to store (above 2^63 - 1).
    OutOfRange(&'static str),
    /// A blob write stopped because free space fell below the reserve (see [`crate::Store::reserve`]),
    /// or the disk ran full. `free` is 0 when the disk itself reported full.
    DiskFull {
        free: u64,
        reserve: u64,
    },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Format(e) => write!(f, "{e}"),
            StoreError::Db(e) => write!(f, "database error: {e}"),
            StoreError::Io(e) => write!(f, "file error: {e}"),
            StoreError::BlobMismatch { expected, actual } => {
                write!(f, "blob content hashes to {actual}, expected {expected}")
            }
            StoreError::Log(why) => write!(f, "log error: {why}"),
            StoreError::Forked { node, seq } => write!(f, "{node} signed two different logs at entry {seq}"),
            StoreError::NotShareable => write!(f, "object is node-only or private and can't be published"),
            StoreError::SchemaVersion(v) => write!(f, "database schema version {v} is not supported"),
            StoreError::OutOfRange(what) => write!(f, "{what} is too large to store"),
            StoreError::DiskFull { free, reserve } => {
                write!(f, "not enough free space: {free} bytes free, {reserve} kept in reserve")
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<nevo_format::Error> for StoreError {
    fn from(e: nevo_format::Error) -> StoreError {
        StoreError::Format(e)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> StoreError {
        StoreError::Db(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> StoreError {
        StoreError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

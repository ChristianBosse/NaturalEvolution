use std::fmt;

use nevo_store::StoreError;

#[derive(Debug)]
pub enum SyncError {
    Store(StoreError),
    /// Bytes that are not valid under the content format.
    Format(nevo_format::Error),
    Io(std::io::Error),
    /// The peer address is not a local network address. NEVO never talks to the internet (D-005).
    NotLocal(String),
    BadUrl(String),
    /// The connection failed or timed out.
    Transport(String),
    /// The peer answered with an error status (403: we are not paired with it).
    Status {
        status: u16,
        message: String,
    },
    /// The peer is not the node we paired with, or its answer isn't signed by that node's key.
    WrongPeer(String),
    /// The peer sent something that breaks the protocol.
    Protocol(String),
    /// The peer speaks none of the protocol versions we know. Lists the versions it offered.
    Version(Vec<u64>),
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncError::Store(e) => write!(f, "{e}"),
            SyncError::Format(e) => write!(f, "{e}"),
            SyncError::Io(e) => write!(f, "file error: {e}"),
            SyncError::NotLocal(addr) => write!(f, "{addr} is not on the local network; NEVO never uses the internet"),
            SyncError::BadUrl(why) => write!(f, "bad peer address: {why}"),
            SyncError::Transport(why) => write!(f, "connection failed: {why}"),
            SyncError::Status { status, message } => write!(f, "peer refused ({status}): {message}"),
            SyncError::WrongPeer(why) => write!(f, "wrong peer: {why}"),
            SyncError::Protocol(why) => write!(f, "protocol error: {why}"),
            SyncError::Version(offered) => write!(f, "peer offers protocol versions {offered:?}, we speak 1"),
        }
    }
}

impl std::error::Error for SyncError {}

impl From<StoreError> for SyncError {
    fn from(e: StoreError) -> SyncError {
        SyncError::Store(e)
    }
}

impl From<nevo_format::Error> for SyncError {
    fn from(e: nevo_format::Error) -> SyncError {
        SyncError::Format(e)
    }
}

impl From<std::io::Error> for SyncError {
    fn from(e: std::io::Error) -> SyncError {
        SyncError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, SyncError>;

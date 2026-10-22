use std::fmt;

#[derive(Debug)]
pub enum SearchError {
    /// An error from Tantivy: a damaged index, a full disk, a locked writer, …
    Index(tantivy::TantivyError),
    Io(std::io::Error),
    /// The settings can't work (for example a writer memory budget below Tantivy's minimum).
    Config(String),
    /// The index folder was written by a different (or unknown) version of nevo-search and
    /// must be rebuilt.
    Version(String),
    /// A document or passage the index can't take (for example an empty key).
    InvalidDocument(String),
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SearchError::Index(e) => write!(f, "search index error: {e}"),
            SearchError::Io(e) => write!(f, "file error: {e}"),
            SearchError::Config(why) => write!(f, "bad search settings: {why}"),
            SearchError::Version(v) => write!(f, "search index format {v} is not supported; rebuild the index"),
            SearchError::InvalidDocument(why) => write!(f, "can't index document: {why}"),
        }
    }
}

impl std::error::Error for SearchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SearchError::Index(e) => Some(e),
            SearchError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<tantivy::TantivyError> for SearchError {
    fn from(e: tantivy::TantivyError) -> SearchError {
        SearchError::Index(e)
    }
}

impl From<tantivy::directory::error::OpenDirectoryError> for SearchError {
    fn from(e: tantivy::directory::error::OpenDirectoryError) -> SearchError {
        SearchError::Index(e.into())
    }
}

impl From<std::io::Error> for SearchError {
    fn from(e: std::io::Error) -> SearchError {
        SearchError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, SearchError>;

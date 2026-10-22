//! `.nevo` bundles and plain-folder export (spec section 5).
//!
//! A bundle carries signed logs, the objects they list and, by choice, blobs from one library to
//! another on a USB stick, for nodes out of network range (D-022) and for NEVO Seed (D-007). It
//! is a plain uncompressed tar:
//!
//! ```text
//! bundle.cbor          signed `t: "bundle"` object: log ranges and blobs inside
//! objects/<head-id>    for each log: its signed head,
//! objects/<entry-id>   its entries in order,
//! objects/<obj-id>     then the objects they list
//! blobs/<blob-id>      blobs, last, so a cut-off copy loses blobs before metadata
//! ```
//!
//! Only content another node may see goes in: objects come from
//! [`nevo_store::Store::shareable_object_bytes`], so node-only and private content never leaves.
//! On import every head, entry, object and blob is checked on its own; the bundle's signature
//! only says who wrote it.
//!
//! [`export_folders`] writes a library as ordinary folders, one per item, so it stays usable
//! without NEVO.

mod error;
mod folders;
mod read;
mod write;

pub use error::{BundleError, Result};
pub use folders::{export_folders, safe_name, FolderOptions, FolderReport};
pub use read::{import_bundle, import_bundle_file, ImportReport};
pub use write::{export_bundle, export_bundle_file, BlobFilter, ExportOptions, ExportReport};

/// Most entries of one log a bundle may carry. The importer holds one log's entries in memory
/// until it reaches the signed head that vouches for them (about 130 bytes each).
pub const MAX_LOG_ENTRIES: u64 = 200_000;

/// Most log entries in one bundle. Bounds the importer's memory for the IDs they list.
pub const MAX_BUNDLE_ENTRIES: u64 = 1_000_000;

/// Problems listed one by one in a report; more are only counted.
const MAX_PROBLEMS: usize = 200;

/// A report's list of problems, capped so a badly damaged bundle can't fill memory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Problems {
    pub listed: Vec<String>,
    /// Problems beyond those listed.
    pub more: usize,
}

impl Problems {
    pub(crate) fn add(&mut self, problem: impl Into<String>) {
        if self.listed.len() < MAX_PROBLEMS {
            self.listed.push(problem.into());
        } else {
            self.more += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.listed.is_empty()
    }

    pub fn len(&self) -> usize {
        self.listed.len() + self.more
    }
}

/// Member names inside a bundle.
pub(crate) mod member {
    use nevo_format::Id;

    pub const BUNDLE: &str = "bundle.cbor";

    pub fn object(id: Id) -> String {
        format!("objects/{id}")
    }

    pub fn blob(id: Id) -> String {
        format!("blobs/{id}")
    }

    #[derive(Debug, PartialEq, Eq)]
    pub enum Member {
        Bundle,
        Object(Id),
        Blob(Id),
        /// A Bao outboard. Version 1 readers verify blobs whole and ignore these.
        Outboard(Id),
        /// Anything else, including names that try to leave the folder (`../x`, `/etc/x`).
        Odd,
    }

    /// Reads a member name. Only the exact names a bundle uses are accepted; an ID must be in its
    /// canonical text form, which contains only `b`, `a`-`z` and `2`-`7`, so no accepted name
    /// can point outside a folder.
    pub fn parse(name: &[u8]) -> Member {
        let Ok(name) = std::str::from_utf8(name) else {
            return Member::Odd;
        };
        if name == BUNDLE {
            return Member::Bundle;
        }
        let id = |text: &str| text.parse::<Id>().ok().filter(|id| id.to_string() == text);
        match name.split_once('/') {
            Some(("objects", rest)) => id(rest).map_or(Member::Odd, Member::Object),
            Some(("blobs", rest)) => match rest.strip_suffix(".obao") {
                Some(rest) => id(rest).map_or(Member::Odd, Member::Outboard),
                None => id(rest).map_or(Member::Odd, Member::Blob),
            },
            _ => Member::Odd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::member::{self, Member};
    use nevo_format::Id;

    #[test]
    fn member_names() {
        let id = Id::of(b"x");
        assert_eq!(member::parse(b"bundle.cbor"), Member::Bundle);
        assert_eq!(member::parse(member::object(id).as_bytes()), Member::Object(id));
        assert_eq!(member::parse(member::blob(id).as_bytes()), Member::Blob(id));
        assert_eq!(
            member::parse(format!("blobs/{id}.obao").as_bytes()),
            Member::Outboard(id)
        );
        let upper = format!("objects/{}", id.to_string().to_uppercase());
        for odd in [
            "../bundle.cbor",
            "/etc/passwd",
            "objects/../../x",
            "objects/",
            "objects/x/../y",
            "./bundle.cbor",
            "blobs\\x",
            upper.as_str(),
            "objects/bundle.cbor",
        ] {
            assert_eq!(member::parse(odd.as_bytes()), Member::Odd, "{odd}");
        }
        assert_eq!(member::parse(&[0xff, 0xfe]), Member::Odd);
    }
}

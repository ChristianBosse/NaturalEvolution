//! Garbage collection: deletes blob files that nothing uses, such as files from uploads whose
//! form was abandoned or failed. A blob is in use while any stored revision lists it (whatever
//! its visibility) or derived data reads or produces it.
//!
//! ## Why a grace period, and what it covers
//!
//! Blobs are stored before the object that uses them: the web server stores each uploaded file,
//! then signs and stores the revision. In between, the blob is unused. So only files older than
//! the grace period are candidates. A put_blob of bytes that are already stored replaces the
//! file, which makes it young again.
//!
//! Each candidate is then checked and deleted while holding the database lock and the blob lock,
//! after checking its age once more:
//!
//! - The database lock means no revision or derived object that uses it can be stored between
//!   the check and the delete. One stored just after the delete simply has a missing blob
//!   ([`Store::missing_blobs`]), as if it had arrived before the blob.
//! - The blob lock means put_blob can't put a fresh copy in place between the age check and
//!   the delete.
//!
//! What is left is a caller that holds a [`crate::BlobInfo`] for longer than the grace period
//! before storing the object that uses it. The default (24 hours) is far longer than an upload
//! takes in practice: 4 GB, the web form's limit, takes 9 hours at 1 Mbit/s. The locks work
//! within one process; two processes that share a store only have the grace period.

use std::fs::{self, DirEntry};
use std::path::Path;
use std::time::{Duration, SystemTime};

use nevo_format::Id;
use rusqlite::Connection;

use crate::error::Result;
use crate::Store;

/// How old an unused blob file must be before garbage collection deletes it.
pub const GC_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// Error messages kept in a report. A failing disk can produce one per file.
const MAX_ERRORS: usize = 20;

/// What a garbage collection did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Blob files looked at.
    pub scanned: u64,
    pub removed: u64,
    pub bytes_freed: u64,
    /// Files or folders that could not be checked or deleted. Collection goes on past them.
    pub failed: u64,
    /// The first few of those errors.
    pub errors: Vec<String>,
}

impl GcReport {
    fn fail(&mut self, path: &Path, e: impl std::fmt::Display) {
        self.failed += 1;
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!("{}: {e}", path.display()));
        }
    }
}

impl Store {
    /// Deletes blob files that no stored object uses and that are older than `grace`
    /// (normally [`GC_GRACE`]). Files in `blobs/` that aren't named like blobs are left alone.
    pub fn collect_garbage(&self, grace: Duration) -> Result<GcReport> {
        let mut report = GcReport::default();
        let blobs = self.root.join("blobs");
        // Folder by folder, so memory use doesn't grow with the number of blobs.
        for entry in fs::read_dir(&blobs)? {
            self.gc_entry(entry, 0, grace, &mut report);
        }
        Ok(report)
    }

    /// `depth` 0 and 1 are the `xx` and `yy` folders, 2 the blob files.
    fn gc_entry(&self, entry: std::io::Result<DirEntry>, depth: u8, grace: Duration, report: &mut GcReport) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return report.fail(&self.root.join("blobs"), e),
        };
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else { return };
        if depth < 2 {
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if !is_dir || name.len() != 2 || !name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
                return;
            }
            match fs::read_dir(&path) {
                Ok(entries) => entries.for_each(|e| self.gc_entry(e, depth + 1, grace, report)),
                Err(e) => report.fail(&path, e),
            }
            return;
        }
        // Only a blob's exact name at its own place: never a stray or misplaced file.
        let Ok(id) = name.parse::<Id>() else { return };
        if id.to_string() != name || self.blob_path(id) != path {
            return;
        }
        report.scanned += 1;
        // Age first: it needs no database query, and most young files are about to be used.
        match is_old(&path, grace) {
            Ok(true) => {}
            Ok(false) => return,
            Err(e) => return report.fail(&path, e),
        }
        let db = self.db();
        let _blobs = self.blob_lock.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        match in_use(&db, id) {
            Ok(false) => {}
            Ok(true) => return,
            Err(e) => return report.fail(&path, e),
        }
        // Checked again under the locks: put_blob may have put a fresh copy in place since.
        let size = match fs::metadata(&path) {
            Ok(meta) if age(&meta).is_some_and(|a| a > grace) => meta.len(),
            Ok(_) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => return report.fail(&path, e),
        };
        match fs::remove_file(&path) {
            Ok(()) => {
                report.removed += 1;
                report.bytes_freed += size;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => report.fail(&path, e),
        }
    }

    /// Deletes a blob's file even if revisions use it: an admin's "purge this file". Those
    /// revisions then list it in [`Store::missing_blobs`]. Returns false if it wasn't stored.
    pub fn remove_blob(&self, id: Id) -> Result<bool> {
        let _blobs = self.blob_lock.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        match fs::remove_file(self.blob_path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
}

/// Time since the file was last written. None if that is in the future: the clock was wrong
/// then or is now (a Pi has no clock battery), so the file counts as young and is kept.
fn age(meta: &fs::Metadata) -> Option<Duration> {
    SystemTime::now().duration_since(meta.modified().ok()?).ok()
}

fn is_old(path: &Path, grace: Duration) -> std::io::Result<bool> {
    Ok(age(&fs::metadata(path)?).is_some_and(|a| a > grace))
}

fn in_use(db: &Connection, id: Id) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM revision_files WHERE blob = ?1)
             OR EXISTS (SELECT 1 FROM derived WHERE src = ?1)
             OR EXISTS (SELECT 1 FROM derived WHERE out = ?1)",
        [id.digest()],
        |r| r.get(0),
    )
}

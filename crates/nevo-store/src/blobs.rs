//! Blob files. A blob is stored at `blobs/<xx>/<yy>/<id>`, where `xx` and `yy` are the first two
//! bytes of its hash in hex. Writes go to `tmp/` first and are moved into place only after the
//! content has been hashed and flushed, so a blob file always matches its name when written.
//! Storage can still decay later; [`Store::verify_blob`] checks for that.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use nevo_format::Id;

use crate::error::{Result, StoreError};
use crate::Store;

const BUFFER: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobInfo {
    pub id: Id,
    pub size: u64,
    /// False if the store already had this blob.
    pub new: bool,
}

/// Leftover temporary files older than this are from interrupted writes (power cut, crash).
/// Younger ones may belong to another process using the same store, so they are left alone.
const STALE_TMP: Duration = Duration::from_secs(60 * 60);

/// Deletes leftovers from writes that were interrupted.
pub(crate) fn clean_tmp(root: &Path) -> Result<()> {
    let tmp = root.join("tmp");
    fs::create_dir_all(&tmp)?;
    for entry in fs::read_dir(&tmp)? {
        let entry = entry?;
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > STALE_TMP);
        if stale && entry.file_name().to_string_lossy().ends_with(".part") {
            // Another process may still hold it open (Windows refuses then); try again next time.
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// Removes the temporary file unless the write succeeded.
struct TmpFile(Option<PathBuf>);

impl TmpFile {
    fn path(&self) -> &Path {
        self.0.as_deref().expect("temporary file already kept")
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TmpFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

impl Store {
    /// Where the blob's file is, whether or not it exists.
    pub fn blob_path(&self, id: Id) -> PathBuf {
        let d = id.digest();
        self.root
            .join("blobs")
            .join(format!("{:02x}", d[0]))
            .join(format!("{:02x}", d[1]))
            .join(id.to_string())
    }

    pub fn has_blob(&self, id: Id) -> bool {
        self.blob_path(id).is_file()
    }

    /// Stores everything `reader` yields and returns its ID.
    pub fn put_blob(&self, reader: impl Read) -> Result<BlobInfo> {
        self.write_blob(reader, None)
    }

    /// Stores a blob that must hash to `expected` (a blob received from another node or a USB
    /// bundle). Nothing is kept if it doesn't match.
    pub fn put_blob_expecting(&self, reader: impl Read, expected: Id) -> Result<BlobInfo> {
        self.write_blob(reader, Some(expected))
    }

    fn write_blob(&self, mut reader: impl Read, expected: Option<Id>) -> Result<BlobInfo> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "{}-{}.part",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut tmp = TmpFile(Some(self.root.join("tmp").join(name)));

        let mut file = File::create(tmp.path())?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; BUFFER];
        let mut size = 0u64;
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            };
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            size += n as u64;
        }
        file.sync_all()?;
        drop(file);

        let id = Id::from_digest(*hasher.finalize().as_bytes());
        if let Some(expected) = expected {
            if expected != id {
                return Err(StoreError::BlobMismatch { expected, actual: id });
            }
        }

        // The fresh copy replaces any existing file, even though the ID is the same: that is how a
        // corrupt copy (see `verify_blob`) gets repaired by fetching the blob again.
        let dest = self.blob_path(id);
        let existed = dest.is_file();
        let dir = dest.parent().expect("blob paths have a parent");
        fs::create_dir_all(dir)?;
        match fs::rename(tmp.path(), &dest) {
            Ok(()) => tmp.disarm(),
            // On Windows a file that is open for reading can't be replaced. The verified copy is
            // dropped then; a caller repairing a blob can try again once readers are done.
            Err(_) if dest.is_file() => return Ok(BlobInfo { id, size, new: false }),
            Err(e) => return Err(e.into()),
        }
        sync_dir(dir)?;
        Ok(BlobInfo {
            id,
            size,
            new: !existed,
        })
    }

    /// Opens a blob for reading.
    pub fn open_blob(&self, id: Id) -> Result<Option<File>> {
        match File::open(self.blob_path(id)) {
            Ok(f) => Ok(Some(f)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Re-hashes a blob's file: `Some(false)` means it is corrupt (bit rot, a bad SD card).
    pub fn verify_blob(&self, id: Id) -> Result<Option<bool>> {
        match self.open_blob(id)? {
            Some(file) => Ok(Some(Id::of_reader(file)? == id)),
            None => Ok(None),
        }
    }

    /// Blobs that stored revisions use but this node doesn't have.
    pub fn missing_blobs(&self) -> Result<Vec<Id>> {
        let db = self.db();
        let mut stmt = db.prepare("SELECT DISTINCT blob FROM revision_files")?;
        let ids = stmt
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids
            .into_iter()
            .filter_map(|d| crate::id_from_db(&d))
            .filter(|&id| !self.has_blob(id))
            .collect())
    }
}

/// Makes a rename durable. Windows has no directory sync; NTFS journals the rename itself.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<()> {
    Ok(())
}

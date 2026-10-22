//! Free space. A full disk breaks SQLite for the whole node (commits and the write-ahead log need
//! room), so blob writes stop while some space, the reserve, is still free. Blobs are where the
//! space goes: one upload can be gigabytes, while objects are a few KB each.

use std::path::Path;
use std::sync::atomic::Ordering;

use crate::error::{Result, StoreError};
use crate::Store;

/// The smallest default reserve.
pub const MIN_RESERVE: u64 = 512 * 1024 * 1024;

/// The default reserve: [`MIN_RESERVE`] or 5% of the disk, whichever is larger. On a 32 GB SD
/// card that is 1.6 GB, which leaves room for the database, logs and system updates.
pub(crate) fn default_reserve(root: &Path) -> u64 {
    let total = fs4::total_space(root).unwrap_or(0);
    MIN_RESERVE.max(total / 20)
}

impl Store {
    /// Bytes available to this process on the filesystem that holds the store.
    pub fn free_space(&self) -> Result<u64> {
        Ok(fs4::available_space(&self.root)?)
    }

    /// Space that blob writes leave free (see [`Store::set_reserve`]).
    pub fn reserve(&self) -> u64 {
        self.reserve.load(Ordering::Relaxed)
    }

    /// Changes the reserve. The default is max(512 MiB, 5% of the disk).
    pub fn set_reserve(&self, bytes: u64) {
        self.reserve.store(bytes, Ordering::Relaxed);
    }

    /// Fails with [`StoreError::DiskFull`] if free space is below the reserve.
    pub(crate) fn check_space(&self) -> Result<()> {
        let (free, reserve) = (self.free_space()?, self.reserve());
        if free < reserve {
            return Err(StoreError::DiskFull { free, reserve });
        }
        Ok(())
    }
}

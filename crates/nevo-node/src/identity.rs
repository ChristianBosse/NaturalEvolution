//! The node's own signing key, kept in `node.key` in the data folder.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use nevo_format::Keypair;

pub const KEY_FILE: &str = "node.key";

/// Loads the node key, or creates one on first start.
pub fn load_or_create(data_dir: &Path) -> io::Result<Keypair> {
    let path = data_dir.join(KEY_FILE);
    match fs::read(&path) {
        Ok(bytes) => {
            let secret: [u8; 32] = bytes.try_into().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} must hold exactly 32 bytes", path.display()),
                )
            })?;
            Ok(Keypair::from_secret(secret))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let key = Keypair::generate().map_err(|e| io::Error::other(e.to_string()))?;
            // Write a temporary file first: a power cut mid-write must not leave an empty key
            // that stops the node from starting.
            let tmp = data_dir.join(format!("{KEY_FILE}.tmp"));
            let _ = fs::remove_file(&tmp);
            let mut file = new_private_file(&tmp)?;
            file.write_all(&key.secret())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, &path)?;
            Ok(key)
        }
        Err(e) => Err(e),
    }
}

/// Creates a file only its owner can read (on Unix; Windows user folders are already private).
fn new_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

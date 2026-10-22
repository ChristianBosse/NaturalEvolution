//! SHA-256 helpers shared by downloads and vaults.
//!
//! SHA-256 (not BLAKE3, which the content format uses) because it is what download sites publish:
//! Kiwix, Hugging Face and most Linux mirrors ship `.sha256` files.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256 as Hasher};

/// A SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sha256(pub [u8; 32]);

impl Sha256 {
    /// Parses 64 hex digits (either case). Surrounding spaces are ignored, and so is anything
    /// after the first space, so the first line of a `sha256sum` file (`<hash>  <name>`) works.
    pub fn from_hex(s: &str) -> Option<Sha256> {
        let s = s.split_whitespace().next()?;
        if s.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, pair) in s.as_bytes().chunks(2).enumerate() {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            out[i] = (hi * 16 + lo) as u8;
        }
        Some(Sha256(out))
    }

    pub fn of(bytes: &[u8]) -> Sha256 {
        Sha256(Hasher::digest(bytes).into())
    }

    /// Hashes a whole file, calling `progress` with the bytes read so far.
    pub fn of_file(path: &Path, progress: &mut dyn FnMut(u64)) -> io::Result<Sha256> {
        let mut file = File::open(path)?;
        let mut hasher = Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut done = 0u64;
        loop {
            let n = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            hasher.update(&buf[..n]);
            done += n as u64;
            progress(done);
        }
        Ok(Sha256(hasher.finalize().into()))
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({self})")
    }
}

/// Stored as hex text in JSON files.
impl serde::Serialize for Sha256 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Sha256 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Sha256, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        Sha256::from_hex(&s)
            .filter(|_| s.len() == 64)
            .ok_or_else(|| serde::de::Error::custom("not a SHA-256 in hex"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let h = Sha256::of(b"abc");
        assert_eq!(
            h.to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(Sha256::from_hex(&h.to_string()), Some(h));
        assert_eq!(
            Sha256::from_hex(&format!("{}  file.zim\n", h.to_string().to_uppercase())),
            Some(h)
        );
        assert_eq!(Sha256::from_hex("abc"), None);
        assert_eq!(Sha256::from_hex(&"g".repeat(64)), None);
    }
}

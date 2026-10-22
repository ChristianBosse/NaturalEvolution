//! Object IDs (BLAKE3 multihashes) and public keys, in binary and text form.

use std::fmt;
use std::io::Read;
use std::str::FromStr;

use crate::base32;
use crate::cbor::Value;
use crate::error::{Error, Result};

/// Multihash prefix: hash function BLAKE3 (0x1e), digest length 32 (0x20).
pub const MULTIHASH_BLAKE3: [u8; 2] = [0x1e, 0x20];

/// The ID of an object or blob: the BLAKE3 hash of its bytes.
///
/// Stored in objects as the 34-byte multihash, written as text as `b` + base32 of those bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id([u8; 32]);

impl Id {
    /// The ID of these bytes.
    pub fn of(bytes: &[u8]) -> Id {
        Id(*blake3::hash(bytes).as_bytes())
    }

    /// The ID of everything a reader yields, for blobs too big to hold in memory.
    pub fn of_reader(mut reader: impl Read) -> std::io::Result<Id> {
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf)? {
                0 => return Ok(Id(*hasher.finalize().as_bytes())),
                n => {
                    hasher.update(&buf[..n]);
                }
            }
        }
    }

    pub fn from_digest(digest: [u8; 32]) -> Id {
        Id(digest)
    }

    pub fn digest(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_multihash(&self) -> [u8; 34] {
        let mut out = [0u8; 34];
        out[..2].copy_from_slice(&MULTIHASH_BLAKE3);
        out[2..].copy_from_slice(&self.0);
        out
    }

    pub fn from_multihash(bytes: &[u8]) -> Result<Id> {
        match bytes {
            [0x1e, 0x20, digest @ ..] if digest.len() == 32 => Ok(Id(digest.try_into().unwrap())),
            [0x1e, ..] => Err(Error::BadId("BLAKE3 multihash must hold 32 bytes")),
            _ => Err(Error::BadId("unknown hash function")),
        }
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", base32::encode(&self.to_multihash()))
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({self})")
    }
}

impl FromStr for Id {
    type Err = Error;

    fn from_str(s: &str) -> Result<Id> {
        let body = s.strip_prefix('b').ok_or(Error::BadId("ID text must start with `b`"))?;
        Id::from_multihash(&base32::decode(body).ok_or(Error::BadId("not base32"))?)
    }
}

impl From<Id> for Value {
    fn from(id: Id) -> Value {
        Value::Bytes(id.to_multihash().to_vec())
    }
}

/// An Ed25519 public key. Written as text as `k` + base32 of its 32 bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey(pub(crate) [u8; 32]);

impl PublicKey {
    pub fn from_bytes(bytes: &[u8]) -> Result<PublicKey> {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| Error::BadId("a public key is 32 bytes"))?;
        // Reject bytes that aren't a valid curve point, so a bad key fails here, not at verify.
        ed25519_dalek::VerifyingKey::from_bytes(&bytes).map_err(|_| Error::BadId("not an Ed25519 public key"))?;
        Ok(PublicKey(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "k{}", base32::encode(&self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl FromStr for PublicKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<PublicKey> {
        let body = s
            .strip_prefix('k')
            .ok_or(Error::BadId("key text must start with `k`"))?;
        PublicKey::from_bytes(&base32::decode(body).ok_or(Error::BadId("not base32"))?)
    }
}

impl From<PublicKey> for Value {
    fn from(key: PublicKey) -> Value {
        Value::Bytes(key.0.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_matches_b3sum() {
        // b3sum of the empty input
        assert_eq!(
            Id::of(b"")
                .digest()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
        assert_eq!(Id::of_reader(&b"hello world"[..]).unwrap(), Id::of(b"hello world"));
    }

    #[test]
    fn id_text_round_trip() {
        let id = Id::of(b"rocket stove");
        let text = id.to_string();
        // The multihash prefix 0x1e 0x20 makes every v1 ID start with "bdyq".
        assert!(text.starts_with("bdyq"));
        assert_eq!(text.len(), 56);
        assert_eq!(text.parse::<Id>().unwrap(), id);
        assert_eq!(text.to_uppercase().replacen('B', "b", 1).parse::<Id>().unwrap(), id);
        assert!("x".parse::<Id>().is_err());
        assert!(
            Id::from_multihash(&[0x12, 0x20]).is_err(),
            "SHA-256 multihash is not accepted in v1"
        );
    }
}

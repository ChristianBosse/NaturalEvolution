//! Ed25519 key pairs and object signatures.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::cbor::{encode_map, Map, Value};
use crate::error::{Error, Result};
use crate::id::PublicKey;

/// Signatures cover this prefix plus the object type, so a signature made for one type can't be
/// replayed as another.
pub const DOMAIN: &[u8] = b"nevo/1/";

/// A secret signing key with its public key.
pub struct Keypair(SigningKey);

impl Keypair {
    /// A new key pair from the operating system's random number source.
    pub fn generate() -> Result<Keypair> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).map_err(|_| Error::Random)?;
        Ok(Keypair::from_secret(secret))
    }

    /// Rebuilds a key pair from its 32-byte secret (from a backup or QR code).
    pub fn from_secret(secret: [u8; 32]) -> Keypair {
        Keypair(SigningKey::from_bytes(&secret))
    }

    /// The 32-byte secret. Whoever has it can sign as this key, so handle it like a password.
    pub fn secret(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    pub fn public(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().to_bytes())
    }

    /// Signs an object map: drops any old `sig`, signs the rest and adds the new `sig`.
    pub fn sign(&self, mut object: Map) -> Result<Map> {
        object.remove("sig");
        let message = signing_message(&object)?;
        let signature = self.0.sign(&message);
        object.insert("sig", signature.to_bytes().to_vec());
        Ok(object)
    }
}

/// Checks that `object`'s `sig` was made by `signer` over the rest of the object.
pub fn verify(object: &Map, signer: &PublicKey) -> Result<()> {
    let sig = match object.get("sig") {
        Some(Value::Bytes(b)) => Signature::from_slice(b).map_err(|_| Error::BadSignature)?,
        Some(_) => return Err(Error::invalid("sig", "must be 64 bytes")),
        None => return Err(Error::Missing("sig")),
    };
    let mut unsigned = object.clone();
    unsigned.remove("sig");
    let message = signing_message(&unsigned)?;
    let key = VerifyingKey::from_bytes(signer.as_bytes()).map_err(|_| Error::BadSignature)?;
    // Strict verification rejects weak keys and malleable signatures.
    key.verify_strict(&message, &sig).map_err(|_| Error::BadSignature)
}

fn signing_message(unsigned: &Map) -> Result<Vec<u8>> {
    let t = match unsigned.get("t") {
        Some(Value::Text(t)) => t,
        _ => return Err(Error::Missing("t")),
    };
    let mut message = Vec::with_capacity(DOMAIN.len() + t.len() + 256);
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(t.as_bytes());
    message.extend_from_slice(&encode_map(unsigned));
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object() -> Map {
        let mut m = Map::new();
        m.insert("t", "att").insert("v", 1u64).insert("claim", "verified");
        m
    }

    #[test]
    fn sign_and_verify() {
        let kp = Keypair::from_secret([7; 32]);
        let signed = kp.sign(object()).unwrap();
        verify(&signed, &kp.public()).unwrap();

        let other = Keypair::from_secret([8; 32]);
        assert_eq!(verify(&signed, &other.public()), Err(Error::BadSignature));

        let mut changed = signed.clone();
        changed.insert("claim", "dangerous");
        assert_eq!(verify(&changed, &kp.public()), Err(Error::BadSignature));

        let mut retyped = signed.clone();
        retyped.insert("t", "rev");
        assert_eq!(verify(&retyped, &kp.public()), Err(Error::BadSignature));
    }

    #[test]
    fn signatures_are_deterministic() {
        let kp = Keypair::from_secret([7; 32]);
        assert_eq!(kp.sign(object()).unwrap(), kp.sign(object()).unwrap());
    }

    #[test]
    fn generated_keys_differ_and_round_trip() {
        let a = Keypair::generate().unwrap();
        let b = Keypair::generate().unwrap();
        assert_ne!(a.public(), b.public());
        assert_eq!(Keypair::from_secret(a.secret()).public(), a.public());
    }
}

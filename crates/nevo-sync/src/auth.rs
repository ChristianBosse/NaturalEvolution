//! Request and response signatures, and the server's challenges.
//!
//! There is no TLS on a LAN without a certificate authority, so signatures do the work:
//!
//! - The client gets a random challenge from `/hello` and signs every request with its node key
//!   over the challenge, a counter that grows with each request, the operation and the body hash.
//!   The server accepts it only from a paired key, once per counter value. No clocks are
//!   involved: a Pi's clock is often wrong (D-020).
//! - The server signs each answer over the request's signature and the answer's body hash, so
//!   the client knows the answer came from the peer it paired with, for this request.
//!
//! Signed messages are CBOR maps signed like objects (`nevo/1/` + type), with types `sync-req`
//! and `sync-res` that no stored object uses, so a signature can't be reused as content.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nevo_format::cbor::{Map, Value};
use nevo_format::{verify, Keypair, PublicKey};

use crate::error::{Result, SyncError};
use crate::PROTOCOL_VERSION;

pub(crate) const CHALLENGE_LEN: usize = 16;

fn request_map(node: PublicKey, challenge: &[u8], count: u64, op: &str, body: &[u8]) -> Map {
    let mut m = Map::new();
    m.insert("t", "sync-req")
        .insert("v", PROTOCOL_VERSION)
        .insert("node", node)
        .insert("ch", challenge.to_vec())
        .insert("n", count)
        .insert("op", op)
        .insert("body", blake3::hash(body).as_bytes().to_vec());
    m
}

fn response_map(node: PublicKey, request_sig: &[u8], body: &[u8]) -> Map {
    let mut m = Map::new();
    m.insert("t", "sync-res")
        .insert("v", PROTOCOL_VERSION)
        .insert("node", node)
        .insert("req", request_sig.to_vec())
        .insert("body", blake3::hash(body).as_bytes().to_vec());
    m
}

fn signature(map: Map, key: &Keypair) -> Result<Vec<u8>> {
    match key.sign(map)?.get("sig") {
        Some(Value::Bytes(sig)) => Ok(sig.clone()),
        _ => Err(SyncError::Protocol("signing produced no signature".into())),
    }
}

fn check(mut map: Map, sig: &[u8], signer: &PublicKey) -> Result<()> {
    map.insert("sig", sig.to_vec());
    Ok(verify(&map, signer)?)
}

/// Signs a request for operation `op` (such as `heads`) with this node's key.
pub fn sign_request(key: &Keypair, challenge: &[u8], count: u64, op: &str, body: &[u8]) -> Result<Vec<u8>> {
    signature(request_map(key.public(), challenge, count, op, body), key)
}

pub fn verify_request(node: PublicKey, challenge: &[u8], count: u64, op: &str, body: &[u8], sig: &[u8]) -> Result<()> {
    check(request_map(node, challenge, count, op, body), sig, &node)
}

/// Signs an answer to the request that carried `request_sig`.
pub fn sign_response(key: &Keypair, request_sig: &[u8], body: &[u8]) -> Result<Vec<u8>> {
    signature(response_map(key.public(), request_sig, body), key)
}

pub fn verify_response(node: PublicKey, request_sig: &[u8], body: &[u8], sig: &[u8]) -> Result<()> {
    check(response_map(node, request_sig, body), sig, &node)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Server-side challenges

/// A challenge unused for this long is forgotten; the client then asks for a new one.
const CHALLENGE_IDLE: Duration = Duration::from_secs(15 * 60);
/// Open challenges kept at most. `/hello` needs no signature, so this bounds what a flood of
/// hellos can cost; the oldest challenges are dropped first.
const MAX_CHALLENGES: usize = 1024;

struct Slot {
    last_used: Instant,
    /// The first peer to use the challenge owns it.
    peer: Option<PublicKey>,
    count: u64,
    /// Issue order, to break ties between equal times.
    order: u64,
}

#[derive(Default)]
pub(crate) struct Challenges {
    slots: HashMap<Vec<u8>, Slot>,
    issued: u64,
}

impl Challenges {
    pub fn issue(&mut self) -> Result<Vec<u8>> {
        let now = Instant::now();
        self.slots
            .retain(|_, s| now.duration_since(s.last_used) < CHALLENGE_IDLE);
        if self.slots.len() >= MAX_CHALLENGES {
            if let Some(oldest) = self
                .slots
                .iter()
                .min_by_key(|(_, s)| (s.last_used, s.order))
                .map(|(k, _)| k.clone())
            {
                self.slots.remove(&oldest);
            }
        }
        let mut challenge = vec![0u8; CHALLENGE_LEN];
        getrandom::fill(&mut challenge).map_err(|_| SyncError::Protocol("no random numbers".into()))?;
        self.issued += 1;
        self.slots.insert(
            challenge.clone(),
            Slot {
                last_used: now,
                peer: None,
                count: 0,
                order: self.issued,
            },
        );
        Ok(challenge)
    }

    /// Accepts `count` under `challenge` for `peer` once. Call only after the signature checked
    /// out, so strangers can't use up a peer's counter values.
    pub fn accept(&mut self, challenge: &[u8], peer: PublicKey, count: u64) -> bool {
        let now = Instant::now();
        let Some(slot) = self.slots.get_mut(challenge) else {
            return false;
        };
        if now.duration_since(slot.last_used) >= CHALLENGE_IDLE
            || slot.peer.is_some_and(|p| p != peer)
            || count <= slot.count
        {
            return false;
        }
        slot.peer = Some(peer);
        slot.count = count;
        slot.last_used = now;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_responses_verify_only_unchanged() {
        let k = Keypair::from_secret([1; 32]);
        let other = Keypair::from_secret([2; 32]);
        let sig = sign_request(&k, b"challenge", 1, "heads", b"body").unwrap();
        verify_request(k.public(), b"challenge", 1, "heads", b"body", &sig).unwrap();
        assert!(verify_request(other.public(), b"challenge", 1, "heads", b"body", &sig).is_err());
        assert!(verify_request(k.public(), b"challenge", 2, "heads", b"body", &sig).is_err());
        assert!(verify_request(k.public(), b"challenge", 1, "blob", b"body", &sig).is_err());
        assert!(verify_request(k.public(), b"challenge", 1, "heads", b"bodx", &sig).is_err());

        let res = sign_response(&other, &sig, b"answer").unwrap();
        verify_response(other.public(), &sig, b"answer", &res).unwrap();
        assert!(verify_response(k.public(), &sig, b"answer", &res).is_err());
        assert!(verify_response(other.public(), &sig, b"answex", &res).is_err());
        // A response signature is not a request signature.
        assert!(verify_request(other.public(), &sig, 1, "heads", b"answer", &res).is_err());
    }

    #[test]
    fn challenges_refuse_replays_and_other_peers() {
        let (a, b) = (
            Keypair::from_secret([1; 32]).public(),
            Keypair::from_secret([2; 32]).public(),
        );
        let mut c = Challenges::default();
        let ch = c.issue().unwrap();
        assert_eq!(ch.len(), CHALLENGE_LEN);
        assert!(!c.accept(b"unknown", a, 1));
        assert!(c.accept(&ch, a, 1));
        assert!(!c.accept(&ch, a, 1), "replay");
        assert!(c.accept(&ch, a, 5));
        assert!(!c.accept(&ch, a, 3), "counter went back");
        assert!(!c.accept(&ch, b, 9), "owned by another peer");
    }

    #[test]
    fn challenges_are_bounded() {
        let mut c = Challenges::default();
        let first = c.issue().unwrap();
        for _ in 0..MAX_CHALLENGES {
            c.issue().unwrap();
        }
        assert!(c.slots.len() <= MAX_CHALLENGES);
        assert!(!c.accept(&first, Keypair::from_secret([1; 32]).public(), 1));
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(hex(&[0, 255, 16]), "00ff10");
        assert_eq!(unhex("00ff10").unwrap(), vec![0, 255, 16]);
        assert!(unhex("0").is_none());
        assert!(unhex("zz").is_none());
    }
}

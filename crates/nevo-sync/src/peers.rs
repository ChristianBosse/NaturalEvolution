//! Paired peers: the nodes an admin approved. Only they may pull from this node.
//!
//! Kept in a plain text file an admin can read and fix by hand, one peer per line:
//!
//! ```text
//! # key                                                  address                      name
//! kabc…xyz http://192.168.1.20:8080/sync School library
//! kdef…uvw - Clinic (no fixed address; found by discovery)
//! ```

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};

use nevo_format::PublicKey;

use crate::error::{Result, SyncError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    pub key: PublicKey,
    /// Where to pull from, such as `http://192.168.1.20:8080/sync`, if it doesn't change.
    pub address: Option<String>,
    /// What the admin calls it.
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerList {
    peers: Vec<Peer>,
}

/// The peer list shared between the sync server and the admin pages that change it.
pub type SharedPeers = Arc<RwLock<PeerList>>;

impl PeerList {
    pub fn new() -> PeerList {
        PeerList::default()
    }

    /// Reads the list from `path`. A missing file is an empty list (nothing paired yet).
    pub fn load(path: impl AsRef<Path>) -> Result<PeerList> {
        match fs::read_to_string(path) {
            Ok(text) => PeerList::parse(&text),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(PeerList::new()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes the list to `path`, replacing it in one step so a power cut can't leave half a file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let tmp = path.with_extension("tmp");
        let mut file = fs::File::create(&tmp)?;
        file.write_all(self.to_text().as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn parse(text: &str) -> Result<PeerList> {
        let mut list = PeerList::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = |why: &str| SyncError::Protocol(format!("peer list line {}: {why}", n + 1));
            let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim_start();
            let (address, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            list.add(Peer {
                key: key.parse().map_err(|_| bad("bad key"))?,
                address: match address {
                    "" | "-" => None,
                    a => Some(a.to_string()),
                },
                name: name.trim().to_string(),
            });
        }
        Ok(list)
    }

    pub fn to_text(&self) -> String {
        let mut text = String::from("# NEVO paired peers: key, address (or -), name\n");
        for p in &self.peers {
            // Names and addresses are single-line text in this file.
            let name = p.name.replace(['\n', '\r'], " ");
            let address = p.address.as_deref().unwrap_or("-").replace(char::is_whitespace, "");
            text.push_str(&format!("{} {} {}\n", p.key, address, name));
        }
        text
    }

    /// Adds a peer, or replaces the entry with the same key.
    pub fn add(&mut self, peer: Peer) {
        match self.peers.iter_mut().find(|p| p.key == peer.key) {
            Some(existing) => *existing = peer,
            None => self.peers.push(peer),
        }
    }

    /// Unpairs a peer. Returns whether it was paired.
    pub fn remove(&mut self, key: PublicKey) -> bool {
        let before = self.peers.len();
        self.peers.retain(|p| p.key != key);
        self.peers.len() != before
    }

    pub fn get(&self, key: PublicKey) -> Option<&Peer> {
        self.peers.iter().find(|p| p.key == key)
    }

    pub fn contains(&self, key: PublicKey) -> bool {
        self.get(key).is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Peer> {
        self.peers.iter()
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Wraps the list for sharing with a [`crate::SyncServer`].
    pub fn shared(self) -> SharedPeers {
        Arc::new(RwLock::new(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nevo_format::Keypair;

    #[test]
    fn save_load_and_edit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.txt");
        assert!(PeerList::load(&path).unwrap().is_empty());

        let (a, b) = (
            Keypair::from_secret([1; 32]).public(),
            Keypair::from_secret([2; 32]).public(),
        );
        let mut list = PeerList::new();
        list.add(Peer {
            key: a,
            address: Some("http://192.168.1.20:8080/sync".into()),
            name: "School library".into(),
        });
        list.add(Peer {
            key: b,
            address: None,
            name: "Clinic\nsecond line".into(),
        });
        list.save(&path).unwrap();

        let loaded = PeerList::load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(a), list.get(a));
        assert_eq!(loaded.get(b).unwrap().name, "Clinic second line");
        assert_eq!(loaded.get(b).unwrap().address, None);

        let mut edited = loaded;
        edited.add(Peer {
            key: a,
            address: None,
            name: "Renamed".into(),
        });
        assert_eq!(edited.len(), 2);
        assert_eq!(edited.get(a).unwrap().name, "Renamed");
        assert!(edited.remove(b));
        assert!(!edited.remove(b));
        assert!(!edited.contains(b));
    }

    #[test]
    fn bad_lines_are_reported() {
        assert!(PeerList::parse("# only a comment\n\n").unwrap().is_empty());
        let err = PeerList::parse("\nnot-a-key - Someone\n").unwrap_err();
        assert!(err.to_string().contains("line 2"));
    }
}

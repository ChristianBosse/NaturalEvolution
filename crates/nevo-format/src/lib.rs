//! NEVO content format, version 1.
//!
//! Implements `docs/spec/content-model.md`: BLAKE3 IDs, Ed25519 signatures, deterministic CBOR,
//! and the object types (revisions, attestations, key records, revocations, derived data and log
//! entries). Everything here is pure computation: storage and sync live in other crates.
//!
//! ```
//! use nevo_format::{open, seal, Keypair, Revision};
//! # use nevo_format::*;
//! # let author = Keypair::from_secret([1; 32]);
//! # let revision = Revision {
//! #     item: None, parents: vec![], author: author.public(), clock: 1,
//! #     time: Time { ms: 0, src: TimeSource::Unknown }, kind: Kind::Guide,
//! #     title: "Boiling water".into(), lang: "en".into(), topics: vec!["water/treatment".parse().unwrap()],
//! #     license: "CC-BY-SA-4.0".into(), vis: Visibility::Public,
//! #     content: vec![ContentFile { name: "guide.md".into(), blob: Id::of(b"..."), size: 3, mime: "text/markdown".into() }],
//! #     source: None, summary: None, extra: Default::default(),
//! # };
//! let sealed = seal(&revision, &author).unwrap();
//! let opened = open::<Revision>(sealed.bytes()).unwrap();
//! assert_eq!(opened.id(), sealed.id());
//! ```

mod base32;
pub mod cbor;
mod error;
mod id;
mod object;
mod sign;
mod topic;

pub use error::{Error, Result};
pub use id::{Id, PublicKey, MULTIHASH_BLAKE3};
pub use object::{
    open, open_any, seal, seal_unsigned, AnyObject, Attestation, Claim, ContentFile, Derived, KeyKind, KeyRecord, Kind,
    LogEntry, LogHead, Object, Revision, Revoke, Sealed, Subject, Time, TimeSource, Visibility, FORMAT_VERSION,
    MAX_OBJECT_BYTES,
};
pub use object::{Bundle, BundleBlob, BundleLog, MAX_BUNDLE_LIST};
pub use sign::{verify, Keypair, DOMAIN};
pub use topic::{Topic, STARTING_TOPICS};

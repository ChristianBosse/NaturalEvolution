//! NEVO sync protocol, version 1 (`docs/spec/sync.md`).
//!
//! Nodes on the same local network pull from each other. Everything that moves can be checked
//! without trusting the sender: objects by their IDs and signatures, log entries by the hash
//! chain up to a signed head, blobs by their hash. Request signatures only decide *who* may ask.
//!
//! - [`router`]: the routes a node mounts under `/sync` to serve paired peers.
//! - [`PeerClient`]: talks to one paired peer over HTTP.
//! - [`pull`]: one pull round from a peer: heads, then entries, objects and blobs.
//! - [`PeerList`]: the peers an admin paired with, kept in a small text file.
//!
//! ```no_run
//! # async fn example(store: std::sync::Arc<nevo_store::Store>, me: std::sync::Arc<nevo_format::Keypair>,
//! #                  peer_key: nevo_format::PublicKey) -> Result<(), nevo_sync::SyncError> {
//! let peer = nevo_sync::PeerClient::connect("http://192.168.1.20:8080/sync", peer_key, me).await?;
//! let report = nevo_sync::pull(&store, &peer, &nevo_sync::PullOptions::new(&store)).await?;
//! println!("{} new objects, {} new blobs", report.objects, report.blobs);
//! # Ok(()) }
//! ```

mod auth;
mod client;
mod engine;
mod error;
mod local;
pub mod msg;
mod peers;
mod server;

pub use auth::{sign_request, sign_response, verify_request, verify_response};
pub use client::PeerClient;
pub use engine::{pull, BlobPolicy, PullOptions, PullReport, SimplePolicy};
pub use error::{Result, SyncError};
pub use local::is_local;
pub use peers::{Peer, PeerList, SharedPeers};
pub use server::{router, ServerConfig, SyncServer};

/// The protocol version this crate speaks. It appears in the `/v1/` paths and in every signature.
pub const PROTOCOL_VERSION: u64 = 1;

/// HTTP headers that carry request and response signatures.
pub mod headers {
    /// The requesting node's public key, as `k…` text.
    pub const NODE: &str = "nevo-node";
    /// The server's challenge from `/hello`, in hex.
    pub const CHALLENGE: &str = "nevo-challenge";
    /// The request counter under that challenge, in decimal. It must grow with every request.
    pub const COUNT: &str = "nevo-count";
    /// The Ed25519 signature, in hex: over the request, or (in a response) over the answer.
    pub const SIG: &str = "nevo-sig";
}

/// Size caps shared by the server and the client.
pub mod limits {
    /// Largest request body. Requests are small lists of IDs.
    pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
    /// Largest response body the client accepts: one blob piece plus framing.
    pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
    /// Most log heads in one `heads` answer.
    pub const MAX_HEADS: u64 = 500;
    /// Most log entries in one `entries` answer (about 90 bytes each).
    pub const MAX_ENTRIES: u64 = 4096;
    /// Most object IDs in one `objects` request.
    pub const MAX_IDS: usize = 256;
    /// Objects stop being added to an `objects` answer once it is this large.
    pub const MAX_OBJECT_BYTES: usize = 8 * 1024 * 1024;
    /// Blobs move in pieces of this size, aligned to it, so that each piece can later be checked
    /// on its own against the blob's BLAKE3 tree (content-model spec, section 2).
    pub const PIECE: u64 = 1024 * 1024;
    /// Most pieces in one `blob` answer.
    pub const MAX_PIECES: u64 = 4;
}

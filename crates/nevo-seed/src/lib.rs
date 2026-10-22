//! NEVO Seed: the companion tool that runs on a computer with internet and prepares USB sticks
//! for offline NEVO nodes (VISION.md, D-007). It is the only part of NEVO that goes online.
//!
//! This crate holds the machinery that doesn't depend on the catalog format:
//!
//! - [`download`]: resumable HTTP(S) downloads with mirrors, retries, a bandwidth limit, and a
//!   size and SHA-256 check before the file gets its final name.
//! - [`stick`]: a real-capacity test that catches fake USB sticks.
//! - [`vault`]: manifests and yearly re-verification of long-term storage drives (D-028).

pub mod download;
pub mod hash;
pub mod stick;
pub mod vault;

pub use hash::Sha256;

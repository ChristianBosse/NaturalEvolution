//! The NEVO node: a web server for the local library.
//!
//! Anyone on the local network opens it in a browser to browse, read and add guides. This is
//! the first slice of Phase 1; search, the built-in AI, user roles and sync come next.

pub mod app;
pub mod housekeeping;
pub mod identity;
mod markdown;
mod pages;

pub use app::{router, AppState};

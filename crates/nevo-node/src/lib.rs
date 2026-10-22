//! The NEVO node: a web server for the local library.
//!
//! Anyone on the local network opens it in a browser to browse and read; people with an account
//! add guides. This is the first slice of Phase 1; search, the built-in AI and sync come next.

mod account_pages;
pub mod accounts;
pub mod app;
pub mod auth;
mod diff;
mod edit;
pub mod housekeeping;
pub mod identity;
mod markdown;
mod pages;
mod setup;

pub use app::{router, AppState};

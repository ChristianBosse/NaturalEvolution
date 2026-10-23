//! The NEVO node: the local library, on its own screen and as a web server.
//!
//! The node's own screen (`screen`, D-062) works with no other device. Anyone on the local
//! network can also open it in a browser to browse and read; people with an account add guides.
//! The built-in AI and sync come next.

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
#[cfg(feature = "screen")]
pub mod screen;
pub mod search;
mod setup;

pub use app::{router, AppState};

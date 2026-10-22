//! NEVO trust: which label each revision gets on this node (D-023 to D-025).
//!
//! | Label | When |
//! |---|---|
//! | Reference | Signed by a key trusted as a **catalog** for the topic |
//! | Verified | Attested `verified` for the topic by a key trusted as a **curator** (or catalog) for it |
//! | Community | Signed by any trusted key, or listed in the own log of a trusted **node**, for the topic |
//! | Unknown | Anything else |
//!
//! ## Where trust comes from
//!
//! Trust is local. The admin's choices are `trust` attestations signed by this node's key and
//! kept in the store (see [`config`]), with a `role` field naming what the key is trusted to do
//! ([`Role`]). This node's own key is trusted as a curator for every known topic. A new node
//! trusts only the catalogs that signed its seed ([`seed_defaults`]), and a paired node is trusted
//! as a node, so its content is Community until the admin promotes it ([`pair_node`]).
//!
//! **Trust is never counted.** A key is trusted only through a chain that starts at this node's
//! admin, so a thousand unknown keys saying the same thing count for nothing.
//!
//! **One hop.** A `trust` attestation by a key the admin trusts directly as a curator or catalog
//! is followed once ([`Delegation::OneHop`], the default; [`Delegation::Off`] turns it off):
//!
//! - the new key is trusted only for topics both attestations cover (a curator trusted for
//!   `medical` can't hand out `water`, and one trusted for `medical/first-aid` hands out at most
//!   that, even if it says `medical`);
//! - its role is at most curator, so only the admin can make a key a catalog (Reference);
//! - its own `trust` attestations are not followed.
//!
//! A withdrawn grant (a `withdraw` attestation by the same author) no longer counts.
//!
//! ## Topics
//!
//! Trust for a topic covers its sub-topics: `medical` covers `medical/first-aid` but not `water`.
//! A top-level topic this format version doesn't know gets no trust, so content in it is Unknown.
//! A revision with several topics gets the weakest of its labels for the topics in context (see
//! [`Trust::assess`]). A revision without topics is treated as `general`.
//!
//! ## What cancels trust
//!
//! - **Revocation:** a key revoked from clock N loses every object and attestation it signed
//!   with clock N or higher. A revocation counts only if [`nevo_format::Revoke::is_authorized`]
//!   accepts it against every key record seen for the key (D-039), so a thief's new record
//!   can't block the owner's recovery key. A revoked author makes the revision Unknown, even if
//!   a curator verified it. Clocks are chosen by the signer, so a thief can backdate: owners
//!   should revoke from the clock after their last object they know is genuine.
//! - **Forks:** a node caught signing two different logs ([`nevo_store::Store::is_forked`]) is
//!   trusted for nothing: its signed content is Unknown and its log no longer vouches for
//!   anything. Content another trusted key signed keeps that key's label, since the forked node
//!   couldn't have changed it. The fork is listed in the reasons either way.
//! - **Withdrawal:** an attestation its author withdrew no longer counts.
//!
//! ## Flags
//!
//! `dangerous`, `outdated` and `hide` attestations about the revision from a curator or catalog
//! trusted for one of its topics come back as [`Flag`]s next to the label. `hide` means the UI
//! hides the revision by default. Flags never change the label.

pub mod config;
mod label;

pub use config::{
    all_topics, grant, pair_node, role_of, seed_defaults, trust_attestation, withdraw, Delegation, Role, TrustConfig,
    ROLE_FIELD,
};
pub use label::{Assessment, Flag, FlagKind, Grant, Label, Reason, Source, Trust};

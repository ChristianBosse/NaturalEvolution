//! The admin's trust choices.
//!
//! They are not a settings file: each choice is a `trust` attestation signed by this node's key
//! and kept in the store. So the configuration is content-addressed, signed and auditable like
//! any other object, and undoing a choice is a `withdraw` attestation, which keeps the history.
//!
//! [`grant`] and [`withdraw`] store these attestations with [`Store::put_object`], not
//! [`Store::publish`]: they stay on this node and never reach another node's log. An admin who
//! wants to share their choices can publish them, but nothing depends on that.

use nevo_format::cbor::{Map, Value};
use nevo_format::{seal, Attestation, Claim, Id, Keypair, PublicKey, Sealed, Subject, Time, Topic, STARTING_TOPICS};
use nevo_store::{Result, Store};

/// The attestation field that names the role a `trust` attestation grants (proposed D-040).
/// Format version 1 keeps unknown fields, so older nodes store and pass it on unchanged.
pub const ROLE_FIELD: &str = "role";

/// What a trusted key is trusted to do, within the topics it is trusted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// A paired node: content it signed, or published in its own log, is Community.
    Node,
    /// A person: content they signed is Community.
    Author,
    /// Like an author, and their `verified` attestations make content Verified. Their
    /// `dangerous`, `outdated` and `hide` flags count, and their `trust` attestations are
    /// followed one hop (see [`Delegation`]).
    Curator,
    /// Like a curator, and content it signed is Reference.
    Catalog,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Node => "node",
            Role::Author => "author",
            Role::Curator => "curator",
            Role::Catalog => "catalog",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Some(match s {
            "node" => Role::Node,
            "author" => Role::Author,
            "curator" => Role::Curator,
            "catalog" => Role::Catalog,
            _ => return None,
        })
    }

    /// Whether this key's `verified`, `dangerous`, `outdated`, `hide` and `trust` attestations count.
    pub fn can_attest(self) -> bool {
        matches!(self, Role::Curator | Role::Catalog)
    }

    /// Whether content listed in this key's own log counts. A person's key has no log, so only
    /// nodes, and curators or catalogs that run one, vouch for what their log lists.
    pub fn vouches_for_log(self) -> bool {
        self != Role::Author
    }

    /// The most a key can get through someone else's `trust` attestation. Only this node's admin
    /// can make a key a catalog, because catalog content is labeled Reference.
    pub(crate) fn delegated(self) -> Role {
        match self {
            Role::Catalog => Role::Curator,
            other => other,
        }
    }
}

/// The role a `trust` attestation grants. Without a `role` field it means curator, which is what
/// the spec describes ("catalogs and curators publish these"). An unknown role (from a later
/// version) grants nothing.
pub fn role_of(attestation: &Attestation) -> Option<Role> {
    match attestation.extra.get(ROLE_FIELD) {
        None => Some(Role::Curator),
        Some(Value::Text(s)) => Role::parse(s),
        Some(_) => None,
    }
}

/// How far `trust` attestations by others are followed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delegation {
    /// Only keys this node's admin trusts directly.
    Off,
    /// Also keys that a curator or catalog the admin trusts says to trust, but never further,
    /// only within the topics both attestations cover, and never as a catalog.
    #[default]
    OneHop,
}

/// What the trust layer needs to know besides the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustConfig {
    /// This node's key. Its `trust` attestations are the admin's choices, and it is itself
    /// trusted as a curator for every known topic.
    pub node: PublicKey,
    pub delegation: Delegation,
}

impl TrustConfig {
    /// The default: follow trust one hop.
    pub fn new(node: PublicKey) -> TrustConfig {
        TrustConfig {
            node,
            delegation: Delegation::default(),
        }
    }
}

/// Every top-level topic this format version knows. Trusting a key for all of them trusts it for
/// everything a version 1 node can trust anyone for.
pub fn all_topics() -> Vec<Topic> {
    STARTING_TOPICS
        .iter()
        .map(|(top, _)| top.parse().expect("starting topics are valid"))
        .collect()
}

/// An unsigned `trust` attestation: `node` trusts `key` as `role` for `topics`.
pub fn trust_attestation(
    node: PublicKey,
    key: PublicKey,
    role: Role,
    topics: Vec<Topic>,
    clock: u64,
    time: Time,
) -> Attestation {
    let mut extra = Map::new();
    extra.insert(ROLE_FIELD, role.as_str());
    Attestation {
        about: Subject::Key(key),
        claim: Claim::Trust,
        topics,
        related: None,
        note: None,
        author: node,
        clock,
        time,
        extra,
    }
}

/// Records that this node trusts `key` as `role` for `topics`. Choices add up: granting a key a
/// second role or more topics keeps the first grant until it is withdrawn.
pub fn grant(
    store: &Store,
    node: &Keypair,
    key: PublicKey,
    role: Role,
    topics: &[Topic],
    time: Time,
) -> Result<Sealed<Attestation>> {
    let clock = store.author_clock(node.public())? + 1;
    let attestation = trust_attestation(node.public(), key, role, topics.to_vec(), clock, time);
    put(store, &attestation, node)
}

/// Takes back an earlier grant (or any other attestation this node signed).
pub fn withdraw(store: &Store, node: &Keypair, attestation: Id, time: Time) -> Result<Sealed<Attestation>> {
    let withdrawal = Attestation {
        about: Subject::Object(attestation),
        claim: Claim::Withdraw,
        topics: Vec::new(),
        related: None,
        note: None,
        author: node.public(),
        clock: store.author_clock(node.public())? + 1,
        time,
        extra: Map::new(),
    };
    put(store, &withdrawal, node)
}

/// The trust a new node starts with (D-024): the catalogs that signed its seed, for every topic.
/// Nothing else is trusted until the admin chooses.
pub fn seed_defaults(
    store: &Store,
    node: &Keypair,
    seed_catalogs: &[PublicKey],
    time: Time,
) -> Result<Vec<Sealed<Attestation>>> {
    seed_catalogs
        .iter()
        .map(|catalog| grant(store, node, *catalog, Role::Catalog, &all_topics(), time.clone()))
        .collect()
}

/// Pairs with another node (D-024): its content arrives as Community, for every topic, until the
/// admin promotes it (for example with a curator grant) or withdraws this grant.
pub fn pair_node(store: &Store, node: &Keypair, peer: PublicKey, time: Time) -> Result<Sealed<Attestation>> {
    grant(store, node, peer, Role::Node, &all_topics(), time)
}

fn put(store: &Store, attestation: &Attestation, node: &Keypair) -> Result<Sealed<Attestation>> {
    let sealed = seal(attestation, node)?;
    store.put_object(sealed.bytes())?;
    Ok(sealed)
}

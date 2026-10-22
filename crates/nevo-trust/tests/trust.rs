use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::{Store, StoreError};
use nevo_trust::*;

fn time() -> Time {
    Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Unknown,
    }
}

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn topics(list: &[&str]) -> Vec<Topic> {
    list.iter().map(|t| t.parse().unwrap()).collect()
}

fn topic(t: &str) -> Topic {
    t.parse().unwrap()
}

struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    key: Keypair,
}

impl Node {
    fn new() -> Node {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        Node {
            _dir: dir,
            store,
            key: key(100),
        }
    }

    fn trust(&self) -> Trust<'_> {
        Trust::new(&self.store, TrustConfig::new(self.key.public()))
    }

    fn grant(&self, who: &Keypair, role: Role, list: &[&str]) -> Sealed<Attestation> {
        grant(&self.store, &self.key, who.public(), role, &topics(list), time()).unwrap()
    }

    fn put<T: Object>(&self, object: &T, signer: &Keypair) -> Sealed<T> {
        let sealed = seal(object, signer).unwrap();
        self.store.put_object(sealed.bytes()).unwrap();
        sealed
    }

    fn guide(&self, author: &Keypair, title: &str, list: &[&str], clock: u64) -> Sealed<Revision> {
        self.put(&revision(author, title, list, clock), author)
    }

    fn attest(&self, author: &Keypair, about: Id, claim: Claim, list: &[&str], clock: u64) -> Sealed<Attestation> {
        self.put(&attestation(author, Subject::Object(about), claim, list, clock), author)
    }

    /// `author` (not this node) says to trust `who` as `role`.
    fn vouch(&self, author: &Keypair, who: &Keypair, role: Role, list: &[&str]) -> Sealed<Attestation> {
        let att = trust_attestation(author.public(), who.public(), role, topics(list), 1, time());
        self.put(&att, author)
    }

    fn record(&self, owner: &Keypair, name: &str, recovery: Option<PublicKey>, clock: u64) {
        let record = KeyRecord {
            key: owner.public(),
            kind: KeyKind::Person,
            name: name.into(),
            recovery,
            clock,
            time: time(),
            extra: Map::new(),
        };
        self.put(&record, owner);
    }

    fn revoke(&self, revoked: &Keypair, by: &Keypair, from: u64) {
        let revoke = Revoke {
            key: revoked.public(),
            from,
            by: by.public(),
            clock: 10,
            time: time(),
            extra: Map::new(),
        };
        self.put(&revoke, by);
    }

    fn label(&self, revision: &Sealed<Revision>) -> Label {
        self.trust().label(revision, None).unwrap()
    }
}

fn revision(author: &Keypair, title: &str, list: &[&str], clock: u64) -> Revision {
    Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock,
        time: time(),
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: topics(list),
        license: "CC-BY-SA-4.0".into(),
        vis: Visibility::Public,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob: Id::of(title.as_bytes()),
            size: 5,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    }
}

fn attestation(author: &Keypair, about: Subject, claim: Claim, list: &[&str], clock: u64) -> Attestation {
    Attestation {
        about,
        claim,
        topics: topics(list),
        related: None,
        note: None,
        author: author.public(),
        clock,
        time: time(),
        extra: Map::new(),
    }
}

/// Makes `node` sign a second, different first log entry, so stores see it as forked.
fn fork(store: &Store, node: &Keypair) {
    store.sign_head(node).unwrap().unwrap();
    let other = seal_unsigned(&LogEntry {
        seq: 1,
        prev: None,
        obj: Id::of(b"something else"),
        extra: Map::new(),
    })
    .unwrap();
    let head = seal(
        &LogHead {
            node: node.public(),
            seq: 1,
            entry: other.id(),
            extra: Map::new(),
        },
        node,
    )
    .unwrap();
    assert!(matches!(store.put_object(head.bytes()), Err(StoreError::Forked { .. })));
    assert!(store.is_forked(node.public()).unwrap());
}

// ---------------------------------------------------------------------------------------------
// Defaults

#[test]
fn a_new_node_trusts_only_its_seed_catalogs() {
    let n = Node::new();
    let (catalog, stranger, peer, person) = (key(1), key(2), key(3), key(4));
    seed_defaults(&n.store, &n.key, &[catalog.public()], time()).unwrap();

    let pack = n.guide(&catalog, "First aid pack", &["medical/first-aid"], 1);
    let a = n.trust().assess(&pack, None).unwrap();
    assert_eq!(a.label, Label::Reference);
    assert!(matches!(&a.reasons[0], Reason::Catalog { by, .. } if by.key == catalog.public() && by.via.is_none()));
    assert!(a.flags.is_empty());

    let other = n.guide(&stranger, "Stranger's guide", &["water"], 1);
    let a = n.trust().assess(&other, None).unwrap();
    assert_eq!(a.label, Label::Unknown);
    assert_eq!(a.reasons, vec![Reason::NoTrustedSource { topic: topic("water") }]);

    // Content from a peer node, not yet paired: Unknown. Once paired: Community.
    let shared = n.guide(&person, "Peer's guide", &["food/cooking"], 1);
    n.store.publish(&peer, shared.bytes()).unwrap();
    assert_eq!(n.label(&shared), Label::Unknown);
    pair_node(&n.store, &n.key, peer.public(), time()).unwrap();
    let a = n.trust().assess(&shared, None).unwrap();
    assert_eq!(a.label, Label::Community);
    assert!(matches!(&a.reasons[0], Reason::TrustedNode { by, .. } if by.key == peer.public()));

    // A paired node's attestations don't count until the admin promotes it.
    n.attest(&peer, shared.id(), Claim::Verified, &["food"], 2);
    assert_eq!(n.label(&shared), Label::Community);
    n.grant(&peer, Role::Curator, &["food"]);
    assert_eq!(n.label(&shared), Label::Verified);

    // This node's own key is trusted, as a curator.
    let own = n.guide(&n.key, "Local notes", &["general"], 50);
    assert_eq!(n.label(&own), Label::Community);
}

#[test]
fn a_thousand_unknown_keys_count_for_nothing() {
    let n = Node::new();
    let author = key(1);
    let guide = n.guide(&author, "Miracle cure", &["medical"], 1);
    for i in 0..1000u32 {
        let mut secret = [7u8; 32];
        secret[..4].copy_from_slice(&i.to_le_bytes());
        let sybil = Keypair::from_secret(secret);
        // Half vouch for the guide, half for its author.
        if i % 2 == 0 {
            n.attest(&sybil, guide.id(), Claim::Verified, &["medical"], 1);
        } else {
            n.vouch(&sybil, &author, Role::Catalog, &["medical"]);
        }
    }
    let mut trust = n.trust();
    let a = trust.assess(&guide, None).unwrap();
    assert_eq!(a.label, Label::Unknown);
    assert!(trust.grants(author.public()).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Topics

#[test]
fn trust_is_scoped_by_topic() {
    let n = Node::new();
    let (doctor, author) = (key(1), key(2));
    n.record(&doctor, "Dr. A", None, 1);
    n.grant(&doctor, Role::Curator, &["medical"]);

    let burns = n.guide(&author, "Burns", &["medical/first-aid"], 1);
    n.attest(&doctor, burns.id(), Claim::Verified, &["medical/first-aid"], 2);
    let a = n.trust().assess(&burns, None).unwrap();
    assert_eq!(a.label, Label::Verified);
    match &a.reasons[0] {
        Reason::Verified { by, topic: t, .. } => {
            assert_eq!(by.name.as_deref(), Some("Dr. A"));
            assert_eq!(by.role, Role::Curator);
            assert_eq!(t, &topic("medical/first-aid"));
        }
        other => panic!("expected a verification, got {other:?}"),
    }

    // Not trusted for water, even when the attestation says water.
    let wells = n.guide(&author, "Wells", &["water/wells"], 1);
    n.attest(&doctor, wells.id(), Claim::Verified, &["water"], 3);
    assert_eq!(n.label(&wells), Label::Unknown);

    // An attestation only verifies the topics it names.
    let mixed = n.guide(&author, "Cholera", &["medical/infections", "water/treatment"], 1);
    n.attest(&doctor, mixed.id(), Claim::Verified, &["medical"], 4);
    let mut trust = n.trust();
    assert_eq!(trust.label(&mixed, None).unwrap(), Label::Unknown, "weakest topic wins");
    assert_eq!(trust.label(&mixed, Some(&topic("medical"))).unwrap(), Label::Verified);
    assert_eq!(
        trust.label(&mixed, Some(&topic("medical/infections/cholera"))).unwrap(),
        Label::Verified
    );
    assert_eq!(trust.label(&mixed, Some(&topic("water"))).unwrap(), Label::Unknown);

    let off_topic = n.guide(&author, "Splints", &["medical/first-aid"], 1);
    n.attest(&doctor, off_topic.id(), Claim::Verified, &["medical/dental"], 5);
    assert_eq!(n.label(&off_topic), Label::Unknown);
}

#[test]
fn unknown_top_level_topics_get_no_trust() {
    let n = Node::new();
    let catalog = key(1);
    seed_defaults(&n.store, &n.key, &[catalog.public()], time()).unwrap();
    n.grant(&catalog, Role::Catalog, &["spacecraft"]);

    let future = n.guide(&catalog, "Orbits", &["spacecraft/orbits"], 1);
    let a = n.trust().assess(&future, None).unwrap();
    assert_eq!(a.label, Label::Unknown);
    assert!(a.reasons.contains(&Reason::UnknownTopic {
        topic: topic("spacecraft/orbits")
    }));

    // An unknown sub-topic of a known topic is treated as its parent.
    let sub = n.guide(&catalog, "Snake bites", &["medical/snake-bites"], 2);
    assert_eq!(n.label(&sub), Label::Reference);

    let mut trust = n.trust();
    let grants = trust.grants(catalog.public()).unwrap();
    assert!(grants.iter().all(|g| g.topics.iter().all(|t| t.is_known())));
}

// ---------------------------------------------------------------------------------------------
// One hop

#[test]
fn trust_by_others_is_followed_one_hop() {
    let n = Node::new();
    let (curator, deputy, third, author) = (key(1), key(2), key(3), key(4));
    n.grant(&curator, Role::Curator, &["medical"]);
    n.vouch(&curator, &deputy, Role::Curator, &["medical", "water"]);
    n.vouch(&deputy, &third, Role::Curator, &["medical"]);

    let guide = n.guide(&author, "Fever", &["medical"], 1);
    n.attest(&deputy, guide.id(), Claim::Verified, &["medical"], 2);
    let a = n.trust().assess(&guide, None).unwrap();
    assert_eq!(a.label, Label::Verified);
    assert!(
        matches!(&a.reasons[0], Reason::Verified { by, .. } if by.key == deputy.public() && by.via == Some(curator.public()))
    );

    // The deputy only gets what the curator could give: medical, not water.
    let water = n.guide(&author, "Filters", &["water"], 1);
    n.attest(&deputy, water.id(), Claim::Verified, &["water"], 3);
    assert_eq!(n.label(&water), Label::Unknown);

    // Two hops: not followed.
    let other = n.guide(&author, "Coughs", &["medical"], 1);
    n.attest(&third, other.id(), Claim::Verified, &["medical"], 2);
    assert_eq!(n.label(&other), Label::Unknown);

    // With delegation off, the deputy counts for nothing.
    let mut strict = Trust::new(
        &n.store,
        TrustConfig {
            node: n.key.public(),
            delegation: Delegation::Off,
        },
    );
    assert_eq!(strict.label(&guide, None).unwrap(), Label::Unknown);
    assert!(strict.grants(deputy.public()).unwrap().is_empty());
}

#[test]
fn one_hop_never_widens_trust() {
    let n = Node::new();
    let (catalog, narrow, made_catalog, sub, writer, friend) = (key(1), key(2), key(3), key(4), key(5), key(6));
    n.grant(&catalog, Role::Catalog, &["medical"]);
    n.grant(&narrow, Role::Curator, &["medical/first-aid"]);
    n.grant(&writer, Role::Author, &["medical"]);

    // A catalog can't make another catalog: the most it hands out is curator.
    n.vouch(&catalog, &made_catalog, Role::Catalog, &["medical"]);
    let pack = n.guide(&made_catalog, "Not official", &["medical"], 1);
    assert_eq!(n.label(&pack), Label::Community);

    // A curator trusted for first aid hands out first aid, even if it says medical.
    n.vouch(&narrow, &sub, Role::Curator, &["medical"]);
    let mut trust = n.trust();
    let grants = trust.grants(sub.public()).unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].topics, topics(&["medical/first-aid"]));
    assert_eq!(grants[0].via, Some(narrow.public()));

    // An author's trust attestations don't count.
    n.vouch(&writer, &friend, Role::Curator, &["medical"]);
    assert!(n.trust().grants(friend.public()).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Revocation

#[test]
fn revocation_cancels_from_its_clock_on() {
    let n = Node::new();
    let (author, doctor, recovery, stranger) = (key(1), key(2), key(3), key(4));
    n.grant(&author, Role::Author, &["general"]);
    n.grant(&doctor, Role::Curator, &["medical"]);

    let before = n.guide(&author, "Before", &["general"], 4);
    let after = n.guide(&author, "After", &["general"], 5);
    assert_eq!(n.label(&after), Label::Community);
    n.revoke(&author, &author, 5);
    assert_eq!(n.label(&before), Label::Community);
    let a = n.trust().assess(&after, None).unwrap();
    assert_eq!(a.label, Label::Unknown);
    assert!(a.reasons.contains(&Reason::Revoked {
        key: author.public(),
        from: 5
    }));

    // Revoked by its recovery key: attestations from the clock on stop counting.
    n.record(&doctor, "Dr. B", Some(recovery.public()), 1);
    let old = n.guide(&key(9), "Old", &["medical"], 1);
    let new = n.guide(&key(9), "New", &["medical"], 1);
    n.attest(&doctor, old.id(), Claim::Verified, &["medical"], 2);
    n.attest(&doctor, new.id(), Claim::Verified, &["medical"], 3);
    n.revoke(&doctor, &recovery, 3);
    assert_eq!(n.label(&old), Label::Verified);
    assert_eq!(n.label(&new), Label::Unknown);

    // A revocation signed by someone else is ignored.
    let fine = n.guide(&doctor, "Doctor's guide", &["medical"], 1);
    let stranger_target = n.guide(&author, "Early", &["general"], 1);
    n.revoke(&author, &stranger, 0);
    assert_eq!(n.label(&stranger_target), Label::Community);
    assert_eq!(n.label(&fine), Label::Community);
}

#[test]
fn a_thief_cannot_block_the_owners_recovery_key() {
    let n = Node::new();
    let (owner, owner_recovery, thief_recovery) = (key(1), key(2), key(3));
    n.grant(&owner, Role::Curator, &["medical"]);
    n.record(&owner, "Dr. Owner", Some(owner_recovery.public()), 1);
    let genuine = n.guide(&owner, "Genuine", &["medical"], 2);

    // The thief, holding the stolen key, publishes a new record naming their own recovery key,
    // then signs content.
    n.record(&owner, "Dr. Thief", Some(thief_recovery.public()), 3);
    let forged = n.guide(&owner, "Forged", &["medical"], 4);

    // The owner revokes with the recovery key from the original record (D-039).
    n.revoke(&owner, &owner_recovery, 3);
    let mut trust = n.trust();
    assert_eq!(trust.label(&genuine, None).unwrap(), Label::Community);
    let a = trust.assess(&forged, None).unwrap();
    assert_eq!(a.label, Label::Unknown);
    assert!(a.reasons.contains(&Reason::Revoked {
        key: owner.public(),
        from: 3
    }));
    let a = trust.assess(&genuine, None).unwrap();
    assert!(
        matches!(&a.reasons[0], Reason::TrustedAuthor { by, .. } if by.name.as_deref() == Some("Dr. Owner")),
        "the name comes from a record signed before the revocation"
    );
}

// ---------------------------------------------------------------------------------------------
// Forks

#[test]
fn a_forked_node_is_trusted_for_nothing() {
    let n = Node::new();
    let (peer, person, catalog) = (key(1), key(2), key(3));
    pair_node(&n.store, &n.key, peer.public(), time()).unwrap();
    n.grant(&catalog, Role::Catalog, &["medical"]);

    let relayed = n.guide(&person, "Relayed", &["food"], 1);
    let own = n.guide(&peer, "Peer's own", &["food"], 1);
    let pack = n.guide(&catalog, "Pack", &["medical"], 1);
    for r in [&relayed, &own, &pack] {
        n.store.publish(&peer, r.bytes()).unwrap();
    }
    assert_eq!(n.label(&relayed), Label::Community);
    assert_eq!(n.label(&own), Label::Community);

    fork(&n.store, &peer);
    let forked = Reason::Forked { node: peer.public() };
    let mut trust = n.trust();
    for r in [&relayed, &own] {
        let a = trust.assess(r, None).unwrap();
        assert_eq!(a.label, Label::Unknown);
        assert!(a.reasons.contains(&forked));
    }
    // The catalog's signature doesn't depend on the forked node.
    let a = trust.assess(&pack, None).unwrap();
    assert_eq!(a.label, Label::Reference);
    assert!(a.reasons.contains(&forked));
    assert!(trust.grants(peer.public()).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Withdrawal

#[test]
fn withdrawn_attestations_and_grants_stop_counting() {
    let n = Node::new();
    let (doctor, author, other) = (key(1), key(2), key(3));
    let granted = n.grant(&doctor, Role::Curator, &["medical"]);
    let guide = n.guide(&author, "Burns", &["medical"], 1);
    let verified = n.attest(&doctor, guide.id(), Claim::Verified, &["medical"], 2);
    assert_eq!(n.label(&guide), Label::Verified);

    // Only the author can withdraw.
    n.attest(&other, verified.id(), Claim::Withdraw, &[], 3);
    assert_eq!(n.label(&guide), Label::Verified);
    n.attest(&doctor, verified.id(), Claim::Withdraw, &[], 3);
    assert_eq!(n.label(&guide), Label::Unknown);

    // The admin withdraws the grant itself.
    let again = n.guide(&doctor, "Doctor's guide", &["medical"], 4);
    assert_eq!(n.label(&again), Label::Community);
    withdraw(&n.store, &n.key, granted.id(), time()).unwrap();
    assert_eq!(n.label(&again), Label::Unknown);
    assert!(n.trust().grants(doctor.public()).unwrap().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Flags

#[test]
fn flags_from_trusted_curators() {
    let n = Node::new();
    let (doctor, engineer, stranger, author) = (key(1), key(2), key(3), key(4));
    n.grant(&doctor, Role::Curator, &["medical"]);
    n.grant(&engineer, Role::Curator, &["energy"]);

    let guide = n.guide(&author, "Home remedy", &["medical/medicines", "food"], 1);
    let mut dangerous = attestation(&doctor, Subject::Object(guide.id()), Claim::Dangerous, &[], 2);
    dangerous.note = Some("Wrong dose".into());
    n.put(&dangerous, &doctor);
    n.attest(&doctor, guide.id(), Claim::Outdated, &["medical"], 3);
    n.attest(&doctor, guide.id(), Claim::Hide, &[], 4);
    // Not counted: an untrusted key, a curator for another topic, and a flag scoped elsewhere.
    n.attest(&stranger, guide.id(), Claim::Dangerous, &[], 1);
    n.attest(&engineer, guide.id(), Claim::Hide, &[], 1);
    n.attest(&doctor, guide.id(), Claim::Dangerous, &["food"], 5);

    // Flags show whatever the context.
    let a = n.trust().assess(&guide, Some(&topic("food"))).unwrap();
    assert_eq!(a.label, Label::Unknown);
    assert_eq!(a.flags.len(), 3, "{:?}", a.flags);
    assert!(a.has(FlagKind::Dangerous) && a.has(FlagKind::Outdated) && a.hidden());
    let flag = a.flags.iter().find(|f| f.kind == FlagKind::Dangerous).unwrap();
    assert_eq!(flag.note.as_deref(), Some("Wrong dose"));
    assert_eq!(flag.by.key, doctor.public());
}

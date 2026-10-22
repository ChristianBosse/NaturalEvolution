//! End-to-end checks of the object rules in docs/spec/content-model.md.

use nevo_format::cbor::{self, Map, Value};
use nevo_format::*;

fn author() -> Keypair {
    Keypair::from_secret(std::array::from_fn(|i| i as u8))
}

fn time() -> Time {
    Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Unknown,
    }
}

fn topic(s: &str) -> Topic {
    s.parse().unwrap()
}

fn first_revision(author: &Keypair) -> Revision {
    Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock: 1,
        time: time(),
        kind: Kind::Guide,
        title: "How to build a rocket stove".into(),
        lang: "en".into(),
        topics: vec![topic("energy/cooking"), topic("shelter/building")],
        license: "CC-BY-SA-4.0".into(),
        vis: Visibility::Public,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob: Id::of(b"guide v1"),
            size: 8_412,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    }
}

/// Re-encodes a sealed object's map after `change`, keeping the old signature.
fn tampered(bytes: &[u8], change: impl FnOnce(&mut Map)) -> Vec<u8> {
    let Value::Map(mut map) = cbor::decode(bytes).unwrap() else {
        panic!("not a map")
    };
    change(&mut map);
    cbor::encode_map(&map)
}

#[test]
fn revision_round_trip() {
    let kp = author();
    let sealed = seal(&first_revision(&kp), &kp).unwrap();
    let opened = open::<Revision>(sealed.bytes()).unwrap();
    assert_eq!(opened, sealed);
    assert_eq!(
        opened.object.item_id(opened.id()),
        sealed.id(),
        "a first revision's ID is the item ID"
    );
    assert_eq!(
        cbor::encode(&cbor::decode(sealed.bytes()).unwrap()),
        sealed.bytes(),
        "bytes are canonical"
    );
    assert!(sealed.id().to_string().starts_with("bdyq"));
}

#[test]
fn edits_and_merges() {
    let kp = author();
    let first = seal(&first_revision(&kp), &kp).unwrap();
    let edit = |clock, parents: Vec<Id>| Revision {
        item: Some(first.id()),
        parents,
        clock,
        title: format!("Rocket stove v{clock}"),
        ..first_revision(&kp)
    };
    let a = seal(&edit(2, vec![first.id()]), &kp).unwrap();
    let b = seal(&edit(2, vec![first.id()]).clone(), &kp).unwrap();
    assert_eq!(a.id(), b.id(), "same content, same ID");
    let merge = seal(&edit(3, vec![a.id(), first.id()]), &kp).unwrap();
    assert_eq!(merge.object.item_id(merge.id()), first.id());

    // A later revision must name its item and parents; a first one must have neither.
    assert!(seal(
        &Revision {
            item: None,
            ..edit(2, vec![first.id()])
        },
        &kp
    )
    .is_err());
    assert!(seal(
        &Revision {
            parents: vec![],
            ..edit(2, vec![])
        },
        &kp
    )
    .is_err());
    assert!(seal(&edit(3, vec![a.id(), a.id()]), &kp).is_err(), "repeated parent");
}

#[test]
fn tampering_is_detected() {
    let kp = author();
    let sealed = seal(&first_revision(&kp), &kp).unwrap();

    let retitled = tampered(sealed.bytes(), |m| {
        m.insert("title", "How to build a petrol stove");
    });
    assert_eq!(open::<Revision>(&retitled).unwrap_err(), Error::BadSignature);

    let other = Keypair::from_secret([9; 32]);
    let reattributed = tampered(sealed.bytes(), |m| {
        m.insert("author", other.public());
    });
    assert_eq!(open::<Revision>(&reattributed).unwrap_err(), Error::BadSignature);

    let unsigned = tampered(sealed.bytes(), |m| {
        m.remove("sig");
    });
    assert_eq!(open::<Revision>(&unsigned).unwrap_err(), Error::Missing("sig"));
}

#[test]
fn only_the_author_can_seal() {
    let kp = author();
    let stranger = Keypair::from_secret([9; 32]);
    assert!(seal(&first_revision(&kp), &stranger).is_err());
}

#[test]
fn validation_rules() {
    let kp = author();
    let bad = |r: Revision| seal(&r, &kp).is_err();
    let base = || first_revision(&kp);
    assert!(
        bad(Revision {
            title: "  ".into(),
            ..base()
        }),
        "blank title"
    );
    assert!(
        bad(Revision {
            lang: "english!".into(),
            ..base()
        }),
        "bad language tag"
    );
    assert!(
        bad(Revision {
            topics: vec![],
            ..base()
        }),
        "no topics"
    );
    assert!(
        bad(Revision {
            content: vec![],
            ..base()
        }),
        "no files"
    );
    assert!(bad(Revision { clock: 0, ..base() }), "clock starts at 1");
    for name in ["../etc/passwd", "a/b.md", "C:evil", "..", ""] {
        let mut r = base();
        r.content[0].name = name.into();
        assert!(bad(r), "unsafe file name {name:?}");
    }
    // Unknown topics are refused at parse time.
    assert!(!"build".parse::<Topic>().unwrap().is_known());
}

#[test]
fn unknown_fields_survive_a_round_trip() {
    let kp = author();
    let mut r = first_revision(&kp);
    r.extra.insert("rating", 5u64);
    let sealed = seal(&r, &kp).unwrap();
    let opened = open::<Revision>(sealed.bytes()).unwrap();
    assert_eq!(opened.object.extra.get("rating"), Some(&Value::Int(5)));
    assert_eq!(seal(&opened.object, &kp).unwrap().id(), sealed.id());
}

#[test]
fn newer_versions_and_types() {
    let kp = author();
    let sealed = seal(&first_revision(&kp), &kp).unwrap();
    let v2 = tampered(sealed.bytes(), |m| {
        m.insert("v", 2u64);
    });
    assert_eq!(open::<Revision>(&v2).unwrap_err(), Error::UnsupportedVersion(2));

    let mut future = Map::new();
    future
        .insert("t", "poll")
        .insert("v", 1u64)
        .insert("question", "Plant beans now?");
    let bytes = cbor::encode_map(&future);
    assert!(matches!(open_any(&bytes).unwrap(), AnyObject::Unknown { t, .. } if t == "poll"));
    assert!(matches!(open::<Revision>(&bytes), Err(Error::WrongType { .. })));
}

#[test]
fn attestations() {
    let curator = Keypair::from_secret([3; 32]);
    let kp = author();
    let rev = seal(&first_revision(&kp), &kp).unwrap();
    let att = |about, claim, topics: Vec<Topic>, related| Attestation {
        about,
        claim,
        topics,
        related,
        note: None,
        author: curator.public(),
        clock: 1,
        time: time(),
        extra: Map::new(),
    };

    let verified = seal(
        &att(Subject::Object(rev.id()), Claim::Verified, vec![topic("energy")], None),
        &curator,
    )
    .unwrap();
    assert!(matches!(open_any(verified.bytes()).unwrap(), AnyObject::Attestation(_)));

    let trust = seal(
        &att(Subject::Key(kp.public()), Claim::Trust, vec![topic("medical")], None),
        &curator,
    )
    .unwrap();
    assert_eq!(trust.object.about, Subject::Key(kp.public()));

    assert!(
        seal(&att(Subject::Key(kp.public()), Claim::Trust, vec![], None), &curator).is_err(),
        "trust needs a topic"
    );
    assert!(seal(
        &att(Subject::Object(rev.id()), Claim::Trust, vec![topic("medical")], None),
        &curator
    )
    .is_err());
    assert!(seal(
        &att(Subject::Key(kp.public()), Claim::Dangerous, vec![], None),
        &curator
    )
    .is_err());
    assert!(
        seal(
            &att(Subject::Object(rev.id()), Claim::TranslationOf, vec![], None),
            &curator
        )
        .is_err(),
        "needs related"
    );
}

#[test]
fn keys_and_revocation() {
    let kp = author();
    let recovery = Keypair::from_secret([5; 32]);
    let record = seal(
        &KeyRecord {
            key: kp.public(),
            kind: KeyKind::Person,
            name: "Maria (clinic volunteer)".into(),
            recovery: Some(recovery.public()),
            clock: 1,
            time: time(),
            extra: Map::new(),
        },
        &kp,
    )
    .unwrap();

    let revoke = |by: &Keypair| Revoke {
        key: kp.public(),
        from: 42,
        by: by.public(),
        clock: 43,
        time: time(),
        extra: Map::new(),
    };
    let by_recovery = seal(&revoke(&recovery), &recovery).unwrap();
    assert!(by_recovery.object.is_authorized(std::slice::from_ref(&record.object)));
    assert!(seal(&revoke(&kp), &kp).unwrap().object.is_authorized(&[]));

    let stranger = Keypair::from_secret([6; 32]);
    let by_stranger = seal(&revoke(&stranger), &stranger).unwrap();
    assert!(
        !by_stranger.object.is_authorized(std::slice::from_ref(&record.object)),
        "signed, but not allowed to revoke"
    );
}

#[test]
fn log_chain() {
    let node = Keypair::from_secret([4; 32]);
    let first = seal_unsigned(&LogEntry {
        seq: 1,
        prev: None,
        obj: Id::of(b"a"),
        extra: Map::new(),
    })
    .unwrap();
    let second = seal_unsigned(&LogEntry {
        seq: 2,
        prev: Some(first.id()),
        obj: Id::of(b"b"),
        extra: Map::new(),
    })
    .unwrap();
    assert!(seal_unsigned(&LogEntry {
        seq: 2,
        prev: None,
        obj: Id::of(b"b"),
        extra: Map::new()
    })
    .is_err());
    assert!(seal_unsigned(&LogEntry {
        seq: 1,
        prev: Some(first.id()),
        obj: Id::of(b"b"),
        extra: Map::new()
    })
    .is_err());

    let head = seal(
        &LogHead {
            node: node.public(),
            seq: 2,
            entry: second.id(),
            extra: Map::new(),
        },
        &node,
    )
    .unwrap();
    assert!(matches!(open_any(head.bytes()).unwrap(), AnyObject::LogHead(_)));

    // Log entries are small because they don't repeat the node key (spec section 7).
    assert!(second.bytes().len() < 100, "{} bytes", second.bytes().len());
}

#[test]
fn derived_data() {
    let seed = Keypair::from_secret([2; 32]);
    let d = Derived {
        from: Id::of(b"pdf"),
        producer: "nevo-ocr/1.0".into(),
        out: Id::of(b"text"),
        author: seed.public(),
        extra: Map::new(),
    };
    assert!(seal(&d, &seed).is_ok());
    assert!(
        seal(
            &Derived {
                producer: "nevo-ocr".into(),
                ..d
            },
            &seed
        )
        .is_err(),
        "needs a version"
    );
}

#[test]
fn rejects_oversized_input() {
    let big = vec![0u8; MAX_OBJECT_BYTES + 1];
    assert_eq!(open_any(&big).unwrap_err(), Error::TooLarge);
}

/// The same revision built by the independent Python encoder in spikes/content-model must give
/// byte-identical results: same size (spec section 7), same ID, same key text.
#[test]
fn matches_python_reference_encoder() {
    let kp = author();
    let sealed = seal(&first_revision(&kp), &kp).unwrap();
    assert_eq!(sealed.bytes().len(), 385);
    assert_eq!(
        sealed.id().to_string(),
        "bdyqnhrr5rhmltokhz4x447725uekzlkmsqacdpw4sofcxyfaq75bhwy"
    );
    assert_eq!(
        kp.public().to_string(),
        "kaoqqpp7tzyil4hlq3umoos6atft6jvrqtosq2xy53sdgiesvgg4a"
    );
}

/// Values added by later spec versions are kept, not rejected (spec section 1, "Forward
/// compatibility"). Counters are capped so they fit a signed 64-bit database integer.
#[test]
fn forward_compatible_values_and_limits() {
    let kp = author();
    let first = seal(&first_revision(&kp), &kp).unwrap();
    let resign = |change: &dyn Fn(&mut Map)| {
        let Value::Map(mut m) = cbor::decode(first.bytes()).unwrap() else {
            unreachable!()
        };
        change(&mut m);
        let signed = kp.sign(m).unwrap();
        open::<Revision>(&cbor::encode_map(&signed))
    };

    let newer = resign(&|m| {
        m.insert("topics", Value::Array(vec!["legal/contracts".into()]));
        let mut time = Map::new();
        time.insert("ms", 0u64).insert("src", "ntp");
        m.insert("time", time);
    })
    .unwrap();
    assert!(!newer.object.topics[0].is_known());
    assert_eq!(newer.object.time.src, TimeSource::Other("ntp".into()));

    assert!(
        resign(&|m| {
            m.insert("vis", "friends");
        })
        .is_err(),
        "a new visibility would need format version 2"
    );
    assert!(resign(&|m| {
        m.insert("clock", 1u64 << 63);
    })
    .is_err());
    assert!(resign(&|m| {
        m.insert("clock", i64::MAX as u64);
    })
    .is_ok());
}

#[test]
fn verified_needs_a_topic() {
    let curator = Keypair::from_secret([3; 32]);
    let a = Attestation {
        about: Subject::Object(Id::of(b"x")),
        claim: Claim::Verified,
        topics: vec![],
        related: None,
        note: None,
        author: curator.public(),
        clock: 1,
        time: time(),
        extra: Map::new(),
    };
    assert!(seal(&a, &curator).is_err());
}

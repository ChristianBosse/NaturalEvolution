//! The bundle object (`t: "bundle"`) of docs/spec/content-model.md section 5.

use nevo_format::cbor::{self, Map, Value};
use nevo_format::*;

fn writer() -> Keypair {
    Keypair::from_secret([7; 32])
}

fn bundle(by: &Keypair) -> Bundle {
    Bundle {
        by: by.public(),
        clock: 3,
        time: Time {
            ms: 1_792_000_000_000,
            src: TimeSource::Unknown,
        },
        logs: vec![BundleLog {
            node: Keypair::from_secret([8; 32]).public(),
            from: 1,
            to: 4,
            head: Id::of(b"head"),
        }],
        blobs: vec![
            BundleBlob {
                blob: Id::of(b"one"),
                size: 3,
            },
            BundleBlob {
                blob: Id::of(b"two"),
                size: 3,
            },
        ],
        note: Some("Medical kit".into()),
        extra: Map::new(),
    }
}

#[test]
fn seals_and_opens() {
    let kp = writer();
    let sealed = seal(&bundle(&kp), &kp).unwrap();
    let opened = open::<Bundle>(sealed.bytes()).unwrap();
    assert_eq!(opened.object, bundle(&kp));
    assert_eq!(opened.id(), Id::of(sealed.bytes()));
}

#[test]
fn only_the_writer_can_sign() {
    let kp = writer();
    assert!(seal(&bundle(&kp), &Keypair::from_secret([9; 32])).is_err());

    // Re-signing someone else's bundle under a different `by` breaks the signature.
    let sealed = seal(&bundle(&kp), &kp).unwrap();
    let Value::Map(mut map) = cbor::decode(sealed.bytes()).unwrap() else {
        panic!("not a map")
    };
    map.insert("by", Keypair::from_secret([9; 32]).public());
    assert_eq!(
        open::<Bundle>(&cbor::encode_map(&map)).unwrap_err(),
        Error::BadSignature
    );
}

#[test]
fn rejects_bad_ranges_and_repeats() {
    let kp = writer();
    let bad = |b: Bundle| seal(&b, &kp).is_err();

    let mut b = bundle(&kp);
    b.logs[0].from = 5;
    assert!(bad(b), "from after to");

    let mut b = bundle(&kp);
    b.logs[0].from = 0;
    assert!(bad(b), "seq starts at 1");

    let mut b = bundle(&kp);
    b.logs.push(b.logs[0].clone());
    assert!(bad(b), "two ranges for one node");

    let mut b = bundle(&kp);
    b.blobs[1].blob = b.blobs[0].blob;
    assert!(bad(b), "repeated blob");

    let mut b = bundle(&kp);
    b.clock = 0;
    assert!(bad(b), "clock starts at 1");
}

#[test]
fn empty_lists_and_unknown_fields_are_kept() {
    let kp = writer();
    let mut b = bundle(&kp);
    b.logs.clear();
    b.blobs.clear();
    b.note = None;
    b.extra.insert("compress", "none");
    let sealed = seal(&b, &kp).unwrap();
    let opened = open::<Bundle>(sealed.bytes()).unwrap();
    assert_eq!(opened.object.extra.get("compress"), Some(&Value::Text("none".into())));
    assert_eq!(opened.bytes(), sealed.bytes());
}

#[test]
fn is_not_a_library_object() {
    let kp = writer();
    let sealed = seal(&bundle(&kp), &kp).unwrap();
    assert!(matches!(open_any(sealed.bytes()).unwrap(), AnyObject::Unknown { t, .. } if t == "bundle"));
    // Bundle signatures can't be passed off as another type's.
    assert!(matches!(open::<Revision>(sealed.bytes()), Err(Error::WrongType { .. })));
}

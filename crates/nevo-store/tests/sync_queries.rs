//! The small queries nevo-sync builds on.

use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::{Store, StoreError};

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn revision(author: &Keypair, title: &str, vis: Visibility, blob: Id) -> Sealed<Revision> {
    let r = Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock: 1,
        time: Time {
            ms: 1_792_000_000_000,
            src: TimeSource::Unknown,
        },
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: vec!["general".parse().unwrap()],
        license: "CC-BY-SA-4.0".into(),
        vis,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob,
            size: 5,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    };
    seal(&r, author).unwrap()
}

#[test]
fn log_nodes_and_signed_heads() {
    let (_d, s) = store();
    let (n1, n2) = (key(1), key(2));
    assert!(s.log_nodes(None, 10).unwrap().is_empty());
    assert!(s.signed_head(n1.public(), u64::MAX).unwrap().is_none());

    s.publish(&n1, revision(&n1, "a", Visibility::Public, Id::of(b"a")).bytes())
        .unwrap();
    let h1 = s.sign_head(&n1).unwrap().unwrap();
    s.publish(&n1, revision(&n1, "b", Visibility::Public, Id::of(b"b")).bytes())
        .unwrap();
    let h2 = s.sign_head(&n1).unwrap().unwrap();
    s.publish(&n2, revision(&n2, "c", Visibility::Public, Id::of(b"c")).bytes())
        .unwrap();
    s.sign_head(&n2).unwrap();

    let mut both = vec![n1.public(), n2.public()];
    both.sort();
    assert_eq!(s.log_nodes(None, 10).unwrap(), both);
    assert_eq!(s.log_nodes(None, 1).unwrap(), vec![both[0]]);
    assert_eq!(s.log_nodes(Some(both[0]), 10).unwrap(), vec![both[1]]);

    assert_eq!(s.signed_head(n1.public(), u64::MAX).unwrap().unwrap().id(), h2.id());
    assert_eq!(s.signed_head(n1.public(), 1).unwrap().unwrap().id(), h1.id());
    assert!(s.signed_head(n1.public(), 0).unwrap().is_none());
}

#[test]
fn missing_logged_objects_and_blob_users() {
    let (_da, a) = store();
    let node = key(3);
    let mut revs: Vec<_> = (0..4u8)
        .map(|i| revision(&node, &format!("g{i}"), Visibility::Public, Id::of(&[i])))
        .collect();
    for r in &revs {
        a.publish(&node, r.bytes()).unwrap();
    }
    a.sign_head(&node).unwrap();

    // B receives only the log: every listed object is missing.
    let (_db, b) = store();
    b.put_object(a.signed_head(node.public(), u64::MAX).unwrap().unwrap().bytes())
        .unwrap();
    let entries: Vec<_> = a
        .log_entries(node.public(), 0, 10)
        .unwrap()
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    b.append_log_entries(node.public(), &entries).unwrap();
    revs.sort_by_key(|r| r.id());
    let ids: Vec<_> = revs.iter().map(|r| r.id()).collect();
    assert_eq!(b.missing_logged_objects(None, 10).unwrap(), ids);
    assert_eq!(b.missing_logged_objects(None, 2).unwrap(), ids[..2]);
    assert_eq!(b.missing_logged_objects(Some(ids[1]), 10).unwrap(), ids[2..]);
    b.put_object(revs[0].bytes()).unwrap();
    assert_eq!(b.missing_logged_objects(None, 10).unwrap(), ids[1..]);

    // Blob users include private revisions; sync decides what to do with them.
    let shared = Id::of(b"shared");
    let public = revision(&node, "pub", Visibility::Public, shared);
    let private = revision(&node, "priv", Visibility::Private, shared);
    a.put_object(public.bytes()).unwrap();
    a.put_object(private.bytes()).unwrap();
    let mut users: Vec<_> = a.blob_users(shared).unwrap().iter().map(|r| r.id()).collect();
    users.sort();
    let mut expected = vec![public.id(), private.id()];
    expected.sort();
    assert_eq!(users, expected);
    assert!(a.blob_users(Id::of(b"nobody")).unwrap().is_empty());
}

#[test]
fn fork_evidence_is_the_contradicting_heads() {
    let (_d, s) = store();
    let node = key(4);
    s.publish(&node, revision(&node, "a", Visibility::Public, Id::of(b"a")).bytes())
        .unwrap();
    let honest = s.sign_head(&node).unwrap().unwrap();
    assert!(s.fork_evidence(node.public()).unwrap().is_empty());

    let other = seal_unsigned(&LogEntry {
        seq: 1,
        prev: None,
        obj: Id::of(b"other"),
        extra: Map::new(),
    })
    .unwrap();
    let forked = seal(
        &LogHead {
            node: node.public(),
            seq: 1,
            entry: other.id(),
            extra: Map::new(),
        },
        &node,
    )
    .unwrap();
    assert!(matches!(s.put_object(forked.bytes()), Err(StoreError::Forked { .. })));
    let mut evidence = s.fork_evidence(node.public()).unwrap();
    evidence.sort();
    let mut expected = vec![honest.bytes().to_vec(), forked.bytes().to_vec()];
    expected.sort();
    assert_eq!(evidence, expected);
}

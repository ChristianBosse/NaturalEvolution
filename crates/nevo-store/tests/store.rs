use std::fs;
use std::io::Write;
use std::time::{Duration, SystemTime};

use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::{Store, StoreError, GC_GRACE, MIN_RESERVE};

fn time() -> Time {
    Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Unknown,
    }
}

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn revision(author: &Keypair, title: &str, topics: &[&str], blob: Id) -> Revision {
    Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock: 1,
        time: time(),
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: topics.iter().map(|t| t.parse().unwrap()).collect(),
        license: "CC-BY-SA-4.0".into(),
        vis: Visibility::Public,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob,
            size: 5,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    }
}

fn edit(author: &Keypair, item: Id, parents: Vec<Id>, clock: u64, title: &str) -> Revision {
    Revision {
        item: Some(item),
        parents,
        clock,
        ..revision(author, title, &["energy/cooking"], Id::of(title.as_bytes()))
    }
}

fn attestation(author: &Keypair, about: Subject, claim: Claim, topics: &[&str]) -> Attestation {
    Attestation {
        about,
        claim,
        topics: topics.iter().map(|t| t.parse().unwrap()).collect(),
        related: None,
        note: None,
        author: author.public(),
        clock: 1,
        time: time(),
        extra: Map::new(),
    }
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn sorted(mut ids: Vec<Id>) -> Vec<Id> {
    ids.sort();
    ids
}

/// Publishes `n` revisions by a fixed author to `node`'s log on `store`.
fn publish_n(store: &Store, node: &Keypair, from: u8, n: u8) -> Vec<Sealed<Revision>> {
    let author = key(1);
    (from..from + n)
        .map(|i| {
            let r = seal(
                &revision(&author, &format!("Guide {i}"), &["general"], Id::of(&[i])),
                &author,
            )
            .unwrap();
            store.publish(node, r.bytes()).unwrap();
            r
        })
        .collect()
}

fn entry_bytes(store: &Store, node: &Keypair) -> Vec<Vec<u8>> {
    store
        .log_entries(node.public(), 0, 1000)
        .unwrap()
        .into_iter()
        .map(|(e, _)| e)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Objects

#[test]
fn objects_round_trip_and_persist() {
    let (dir, store) = store();
    let kp = key(1);
    let rev = seal(
        &revision(&kp, "Boiling water", &["water/treatment"], Id::of(b"guide")),
        &kp,
    )
    .unwrap();

    let put = store.put_object(rev.bytes()).unwrap();
    assert!(put.new);
    assert_eq!(put.id, rev.id());
    assert!(!store.put_object(rev.bytes()).unwrap().new, "storing twice is harmless");
    assert_eq!(store.get::<Revision>(rev.id()).unwrap().unwrap(), rev);
    drop(store);

    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(reopened.object_bytes(rev.id()).unwrap().unwrap(), rev.bytes());
    assert!(reopened.has_object(rev.id()).unwrap());
    assert!(reopened.get::<Attestation>(rev.id()).is_err(), "wrong type");
}

#[test]
fn rejects_invalid_objects() {
    let (_dir, store) = store();
    let kp = key(1);
    let rev = seal(&revision(&kp, "Boiling water", &["water"], Id::of(b"g")), &kp).unwrap();
    // Change one letter of the title: still valid CBOR, but the signature no longer matches.
    let mut tampered = rev.bytes().to_vec();
    let at = tampered.windows(7).position(|w| w == b"Boiling").unwrap();
    tampered[at] = b'C';
    assert!(matches!(
        store.put_object(&tampered),
        Err(StoreError::Format(Error::BadSignature))
    ));
    assert!(matches!(
        store.put_object(b"not cbor"),
        Err(StoreError::Format(Error::Cbor(_)))
    ));
    assert!(!store.has_object(Id::of(&tampered)).unwrap());
}

#[test]
fn branches_and_merges() {
    let (_dir, store) = store();
    let (alice, bob) = (key(1), key(2));
    let first = seal(
        &revision(&alice, "Rocket stove", &["energy/cooking"], Id::of(b"v1")),
        &alice,
    )
    .unwrap();
    let item = first.id();
    store.put_object(first.bytes()).unwrap();
    assert_eq!(store.item_tips(item).unwrap(), vec![first.clone()]);

    // Two people edit the same version offline: two tips.
    let a = seal(&edit(&alice, item, vec![item], 2, "Rocket stove (Alice)"), &alice).unwrap();
    let b = seal(&edit(&bob, item, vec![item], 2, "Rocket stove (Bob)"), &bob).unwrap();
    store.put_object(a.bytes()).unwrap();
    store.put_object(b.bytes()).unwrap();
    let tips = store.item_tips(item).unwrap().iter().map(|r| r.id()).collect();
    assert_eq!(sorted(tips), sorted(vec![a.id(), b.id()]));

    // A merge joins them again.
    let merge = seal(&edit(&alice, item, vec![a.id(), b.id()], 3, "Rocket stove"), &alice).unwrap();
    store.put_object(merge.bytes()).unwrap();
    assert_eq!(store.item_tips(item).unwrap(), vec![merge.clone()]);
    assert_eq!(store.item_revisions(item).unwrap().len(), 4);
}

#[test]
fn topics_follow_the_current_version() {
    let (_dir, store) = store();
    let kp = key(1);
    let first_aid = seal(&revision(&kp, "Burns", &["medical/first-aid"], Id::of(b"1")), &kp).unwrap();
    let dental = seal(&revision(&kp, "Toothache", &["medical/dental"], Id::of(b"2")), &kp).unwrap();
    let water = seal(&revision(&kp, "Wells", &["water/wells"], Id::of(b"3")), &kp).unwrap();
    for r in [&first_aid, &dental, &water] {
        store.put_object(r.bytes()).unwrap();
    }
    let topic = |s: &str| s.parse::<Topic>().unwrap();
    assert_eq!(
        sorted(store.items_in_topic(&topic("medical")).unwrap()),
        sorted(vec![first_aid.id(), dental.id()])
    );
    assert_eq!(
        store.items_in_topic(&topic("medical/first-aid")).unwrap(),
        vec![first_aid.id()]
    );
    assert!(
        store.items_in_topic(&topic("medical/first")).unwrap().is_empty(),
        "a shared prefix is not a parent"
    );

    // The wells guide is moved to `farming`: it leaves `water`.
    let moved = Revision {
        item: Some(water.id()),
        parents: vec![water.id()],
        clock: 2,
        ..revision(&kp, "Wells", &["farming"], Id::of(b"3"))
    };
    store.put_object(seal(&moved, &kp).unwrap().bytes()).unwrap();
    assert!(store.items_in_topic(&topic("water")).unwrap().is_empty());
    assert_eq!(store.items_in_topic(&topic("farming")).unwrap(), vec![water.id()]);
}

#[test]
fn attestations_keys_revocations_and_derived() {
    let (_dir, store) = store();
    let (author, curator, recovery) = (key(1), key(2), key(3));
    let rev = seal(
        &revision(&author, "Burns", &["medical/first-aid"], Id::of(b"g")),
        &author,
    )
    .unwrap();
    store.put_object(rev.bytes()).unwrap();

    let verified = seal(
        &attestation(&curator, Subject::Object(rev.id()), Claim::Verified, &["medical"]),
        &curator,
    )
    .unwrap();
    let trust = seal(
        &attestation(&curator, Subject::Key(author.public()), Claim::Trust, &["medical"]),
        &curator,
    )
    .unwrap();
    store.put_object(verified.bytes()).unwrap();
    store.put_object(trust.bytes()).unwrap();
    assert_eq!(
        store.attestations_about(Subject::Object(rev.id())).unwrap(),
        vec![verified]
    );
    assert_eq!(
        store.attestations_about(Subject::Key(author.public())).unwrap(),
        vec![trust]
    );

    let record = seal(
        &KeyRecord {
            key: author.public(),
            kind: KeyKind::Person,
            name: "Maria".into(),
            recovery: Some(recovery.public()),
            clock: 1,
            time: time(),
            extra: Map::new(),
        },
        &author,
    )
    .unwrap();
    store.put_object(record.bytes()).unwrap();
    // A thief with the main key publishes a newer record naming their own recovery key.
    let thief = key(4);
    let hijack = seal(
        &KeyRecord {
            recovery: Some(thief.public()),
            clock: 50,
            ..record.object.clone()
        },
        &author,
    )
    .unwrap();
    store.put_object(hijack.bytes()).unwrap();
    let records: Vec<KeyRecord> = store
        .key_records(author.public())
        .unwrap()
        .into_iter()
        .map(|r| r.object)
        .collect();
    assert_eq!(records.len(), 2);

    let revoke = seal(
        &Revoke {
            key: author.public(),
            from: 10,
            by: recovery.public(),
            clock: 11,
            time: time(),
            extra: Map::new(),
        },
        &recovery,
    )
    .unwrap();
    store.put_object(revoke.bytes()).unwrap();
    let revocations = store.revocations(author.public()).unwrap();
    assert!(
        revocations[0].object.is_authorized(&records),
        "the owner's original recovery key still works"
    );

    let derived = seal(
        &Derived {
            from: Id::of(b"g"),
            producer: "nevo-text/1".into(),
            out: Id::of(b"text"),
            author: author.public(),
            extra: Map::new(),
        },
        &author,
    )
    .unwrap();
    store.put_object(derived.bytes()).unwrap();
    assert_eq!(store.derived_from(Id::of(b"g")).unwrap(), vec![derived]);
}

// ---------------------------------------------------------------------------------------------
// Blobs

#[test]
fn blobs_are_verified_deduplicated_and_repaired() {
    let (dir, store) = store();
    // Bigger than the write buffer, so the streaming path is exercised.
    let data: Vec<u8> = (0..1_000_003u32).map(|i| (i % 251) as u8).collect();

    let info = store.put_blob(&data[..]).unwrap();
    assert!(info.new);
    assert_eq!(info.id, Id::of(&data));
    assert_eq!(info.size, data.len() as u64);
    assert!(store.has_blob(info.id));
    assert!(!store.put_blob(&data[..]).unwrap().new);
    assert_eq!(fs::read(store.blob_path(info.id)).unwrap(), data);
    let d = info.id.digest();
    assert!(store
        .blob_path(info.id)
        .starts_with(dir.path().join("blobs").join(format!("{:02x}", d[0]))));
    assert_eq!(store.verify_blob(info.id).unwrap(), Some(true));

    // A wrong expected ID keeps nothing, not even a temporary file.
    let wrong = Id::of(b"something else");
    assert!(matches!(
        store.put_blob_expecting(&b"hello"[..], wrong),
        Err(StoreError::BlobMismatch { .. })
    ));
    assert!(!store.has_blob(Id::of(b"hello")));
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    assert!(store.put_blob_expecting(&b"hello"[..], Id::of(b"hello")).unwrap().new);

    // Bit rot is detected, and fetching the blob again repairs it.
    fs::OpenOptions::new()
        .append(true)
        .open(store.blob_path(info.id))
        .unwrap()
        .write_all(b"!")
        .unwrap();
    assert_eq!(store.verify_blob(info.id).unwrap(), Some(false));
    store.put_blob_expecting(&data[..], info.id).unwrap();
    assert_eq!(store.verify_blob(info.id).unwrap(), Some(true));

    assert_eq!(store.verify_blob(Id::of(b"absent")).unwrap(), None);
    assert!(store.open_blob(Id::of(b"absent")).unwrap().is_none());
}

#[test]
fn missing_blobs_and_interrupted_writes() {
    let (dir, store) = store();
    let kp = key(1);
    let rev = seal(&revision(&kp, "Burns", &["medical"], Id::of(b"guide text")), &kp).unwrap();
    store.put_object(rev.bytes()).unwrap();
    assert_eq!(store.missing_blobs().unwrap(), vec![Id::of(b"guide text")]);
    store.put_blob(&b"guide text"[..]).unwrap();
    assert!(store.missing_blobs().unwrap().is_empty());
    drop(store);

    // An old leftover is from a crash; a fresh one may belong to another process.
    let tmp = dir.path().join("tmp");
    fs::write(tmp.join("old.part"), b"half a blob").unwrap();
    let old = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    fs::File::options()
        .write(true)
        .open(tmp.join("old.part"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    fs::write(tmp.join("fresh.part"), b"being written").unwrap();
    let _reopened = Store::open(dir.path()).unwrap();
    assert!(!tmp.join("old.part").exists());
    assert!(tmp.join("fresh.part").exists());
}

// ---------------------------------------------------------------------------------------------
// Housekeeping

fn age(path: &std::path::Path, by: Duration) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - by)
        .unwrap();
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

#[test]
fn garbage_collection_removes_only_old_unused_blobs() {
    let (dir, store) = store();
    let kp = key(1);
    let put = |bytes: &[u8]| store.put_blob(bytes).unwrap().id;
    let (public, node_only, private, src, out) = (put(b"pub"), put(b"node"), put(b"priv"), put(b"src"), put(b"out"));
    let (orphan, young) = (put(b"abandoned upload"), put(b"upload in progress"));

    for (blob, vis, n) in [
        (public, Visibility::Public, 1),
        (node_only, Visibility::Node, 2),
        (private, Visibility::Private, 3),
    ] {
        let rev = Revision {
            vis,
            ..revision(&kp, &format!("Guide {n}"), &["water"], blob)
        };
        store.put_object(seal(&rev, &kp).unwrap().bytes()).unwrap();
    }
    let derived = Derived {
        from: src,
        producer: "nevo-ocr/1".into(),
        out,
        author: kp.public(),
        extra: Map::new(),
    };
    store.put_object(seal(&derived, &kp).unwrap().bytes()).unwrap();

    // Things that aren't blobs, even with a blob's name in the wrong folder, stay.
    let stray = dir.path().join("blobs").join("notes.txt");
    fs::write(&stray, b"keep me").unwrap();
    let misplaced = dir.path().join("blobs").join("00").join(orphan.to_string());
    fs::create_dir_all(misplaced.parent().unwrap()).unwrap();
    fs::write(&misplaced, b"abandoned upload").unwrap();

    for id in [public, node_only, private, src, out, orphan] {
        age(&store.blob_path(id), 2 * DAY);
    }
    age(&store.blob_path(young), DAY / 2);
    age(&stray, 2 * DAY);
    age(&misplaced, 2 * DAY);

    let report = store.collect_garbage(GC_GRACE).unwrap();
    assert_eq!(report.scanned, 7);
    assert_eq!(report.removed, 1);
    assert_eq!(report.bytes_freed, b"abandoned upload".len() as u64);
    assert_eq!(report.failed, 0, "{:?}", report.errors);
    assert!(!store.has_blob(orphan));
    for id in [public, node_only, private, src, out, young] {
        assert!(store.has_blob(id), "{id} is used or young");
    }
    assert!(stray.exists() && misplaced.exists());

    // A fresh copy of the same bytes makes a blob young again.
    age(&store.blob_path(young), 2 * DAY);
    put(b"upload in progress");
    assert_eq!(store.collect_garbage(GC_GRACE).unwrap().removed, 0);
    assert_eq!(store.collect_garbage(Duration::ZERO).unwrap().removed, 1);
    assert!(!store.has_blob(young));
}

#[test]
fn blobs_can_be_purged() {
    let (_dir, store) = store();
    let kp = key(1);
    let blob = store.put_blob(&b"unwanted"[..]).unwrap().id;
    let rev = seal(&revision(&kp, "Unwanted", &["water"], blob), &kp).unwrap();
    store.put_object(rev.bytes()).unwrap();
    assert!(store.remove_blob(blob).unwrap());
    assert!(!store.remove_blob(blob).unwrap());
    assert_eq!(store.missing_blobs().unwrap(), vec![blob]);
}

#[test]
fn blob_writes_keep_a_free_space_reserve() {
    let (dir, store) = store();
    let free = store.free_space().unwrap();
    assert!(free > 0);
    assert!(store.reserve() >= MIN_RESERVE);

    store.set_reserve(free + 1024 * 1024 * 1024);
    match store.put_blob(&b"too much"[..]) {
        Err(StoreError::DiskFull { reserve, .. }) => assert_eq!(reserve, store.reserve()),
        other => panic!("expected DiskFull, got {other:?}"),
    }
    assert!(!store.has_blob(Id::of(b"too much")));
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0, "no leftovers");

    // Objects are still stored: they are tiny, and the reserve is there for them.
    let kp = key(1);
    let rev = seal(&revision(&kp, "Still works", &["water"], Id::of(b"x")), &kp).unwrap();
    store.put_object(rev.bytes()).unwrap();

    store.set_reserve(0);
    store.put_blob(&b"fits"[..]).unwrap();
}

/// A reader that lowers the reserve's headroom while it is being read, as another upload
/// filling the disk would.
struct Filling<'a> {
    store: &'a Store,
    left: u64,
}

impl std::io::Read for Filling<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        // After the first chunk, the disk "fills up".
        self.store.set_reserve(u64::MAX);
        let n = buf.len().min(self.left as usize);
        buf[..n].fill(7);
        self.left -= n as u64;
        Ok(n)
    }
}

#[test]
fn a_long_blob_write_stops_when_the_disk_fills() {
    let (dir, store) = store();
    store.set_reserve(0);
    let reader = Filling {
        store: &store,
        left: 65 * 1024 * 1024,
    };
    assert!(matches!(store.put_blob(reader), Err(StoreError::DiskFull { .. })));
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0, "no leftovers");
}

// ---------------------------------------------------------------------------------------------
// Publishing and privacy

#[test]
fn publish_builds_a_chained_log() {
    let (_dir, store) = store();
    let node = key(9);
    assert_eq!(store.log_position(node.public()).unwrap(), 0);
    assert!(store.sign_head(&node).unwrap().is_none());

    let revs = publish_n(&store, &node, 0, 2);
    assert_eq!(store.log_position(node.public()).unwrap(), 2);
    store.publish(&node, revs[0].bytes()).unwrap();
    assert_eq!(
        store.log_position(node.public()).unwrap(),
        2,
        "publishing twice adds no entry"
    );

    let entries = store.log_entries(node.public(), 0, 10).unwrap();
    assert_eq!(
        entries.iter().map(|(_, obj)| *obj).collect::<Vec<_>>(),
        vec![revs[0].id(), revs[1].id()]
    );
    let second = open::<LogEntry>(&entries[1].0).unwrap();
    assert_eq!(second.object.prev, Some(Id::of(&entries[0].0)));
    assert_eq!(store.log_entries(node.public(), 1, 10).unwrap().len(), 1);

    let head = store.sign_head(&node).unwrap().unwrap();
    assert_eq!(store.log_head(node.public()).unwrap(), Some((2, second.id())));
    assert_eq!(head.object.entry, second.id());
    assert!(!store.is_forked(node.public()).unwrap());
}

#[test]
fn private_content_never_reaches_a_log() {
    let (_dir, store) = store();
    let (node, author) = (key(9), key(1));
    let diary = seal(
        &Revision {
            vis: Visibility::Private,
            ..revision(&author, "Diary", &["general"], Id::of(b"d"))
        },
        &author,
    )
    .unwrap();
    assert!(matches!(
        store.publish(&node, diary.bytes()),
        Err(StoreError::NotShareable)
    ));
    store.put_object(diary.bytes()).unwrap(); // kept locally
    let node_only = seal(
        &Revision {
            vis: Visibility::Node,
            ..revision(&author, "Clinic rota", &["general"], Id::of(b"r"))
        },
        &author,
    )
    .unwrap();
    assert!(matches!(
        store.publish(&node, node_only.bytes()),
        Err(StoreError::NotShareable)
    ));

    // Nothing that points at the diary may be published either.
    let public_edit = seal(
        &edit(&author, diary.id(), vec![diary.id()], 2, "Diary, now public?"),
        &author,
    )
    .unwrap();
    assert!(matches!(
        store.publish(&node, public_edit.bytes()),
        Err(StoreError::NotShareable)
    ));
    let about = seal(
        &attestation(&author, Subject::Object(diary.id()), Claim::Hide, &[]),
        &author,
    )
    .unwrap();
    assert!(matches!(
        store.publish(&node, about.bytes()),
        Err(StoreError::NotShareable)
    ));
    let derived = seal(
        &Derived {
            from: Id::of(b"d"),
            producer: "nevo-text/1".into(),
            out: Id::of(b"t"),
            author: author.public(),
            extra: Map::new(),
        },
        &author,
    )
    .unwrap();
    assert!(matches!(
        store.publish(&node, derived.bytes()),
        Err(StoreError::NotShareable)
    ));
    assert_eq!(store.log_position(node.public()).unwrap(), 0);

    // Only published objects are served to other nodes.
    let public = publish_n(&store, &node, 0, 1);
    assert!(store.shareable_object_bytes(public[0].id()).unwrap().is_some());
    assert!(store.shareable_object_bytes(diary.id()).unwrap().is_none());
    assert!(
        store.object_bytes(diary.id()).unwrap().is_some(),
        "still readable locally"
    );
}

// ---------------------------------------------------------------------------------------------
// Sync between stores

/// Copies node A's log to node B the way sync will: signed heads first, then entries that end
/// at a head, then the objects they list.
#[test]
fn logs_replicate_between_stores() {
    let (_da, a) = store();
    let (_db, b) = store();
    let node = key(9);
    let mut published = publish_n(&a, &node, 0, 3);
    let head3 = a.sign_head(&node).unwrap().unwrap();
    published.extend(publish_n(&a, &node, 3, 2));
    let head5 = a.sign_head(&node).unwrap().unwrap();

    b.put_object(head3.bytes()).unwrap();
    b.put_object(head5.bytes()).unwrap();
    let entries = a.log_entries(node.public(), 0, 100).unwrap();
    let bytes: Vec<Vec<u8>> = entries.iter().map(|(e, _)| e.clone()).collect();
    assert_eq!(b.append_log_entries(node.public(), &bytes[..3]).unwrap(), 3);
    assert_eq!(b.append_log_entries(node.public(), &bytes[3..]).unwrap(), 5);
    for (_, obj) in &entries {
        b.put_object(&a.shareable_object_bytes(*obj).unwrap().unwrap()).unwrap();
    }
    assert_eq!(b.log_position(node.public()).unwrap(), 5);
    assert!(published.iter().all(|r| b.has_object(r.id()).unwrap()));
    assert!(!b.is_forked(node.public()).unwrap());
}

#[test]
fn unsigned_entries_need_a_signed_head() {
    let (_da, a) = store();
    let node = key(9);
    publish_n(&a, &node, 0, 3);
    let head = a.sign_head(&node).unwrap().unwrap();
    let bytes = entry_bytes(&a, &node);

    let (_db, b) = store();
    assert!(
        b.append_log_entries(node.public(), &bytes).is_err(),
        "no head stored yet"
    );
    b.put_object(head.bytes()).unwrap();
    assert!(b.append_log_entries(node.public(), &bytes[1..2]).is_err(), "gap");
    assert!(
        b.append_log_entries(node.public(), &[bytes[0].clone(), bytes[2].clone()])
            .is_err(),
        "reordered"
    );
    assert!(
        b.append_log_entries(node.public(), &bytes[..2]).is_err(),
        "doesn't end at a signed head"
    );
    assert_eq!(b.log_position(node.public()).unwrap(), 0, "a failed batch adds nothing");
    assert_eq!(b.append_log_entries(node.public(), &bytes).unwrap(), 3);
    assert!(
        b.append_log_entries(node.public(), &bytes[..1]).is_err(),
        "no duplicates"
    );

    // A peer invents entry 4 for this node. Without a signed head at 4 it is refused, so the
    // node's real entry 4 can still arrive later.
    let forged = seal_unsigned(&LogEntry {
        seq: 4,
        prev: Some(Id::of(&bytes[2])),
        obj: Id::of(b"fake"),
        extra: Map::new(),
    })
    .unwrap();
    assert!(b.append_log_entries(node.public(), &[forged.bytes().to_vec()]).is_err());
    publish_n(&a, &node, 3, 1);
    let head4 = a.sign_head(&node).unwrap().unwrap();
    b.put_object(head4.bytes()).unwrap();
    assert_eq!(
        b.append_log_entries(node.public(), &entry_bytes(&a, &node)[3..])
            .unwrap(),
        4
    );
}

#[test]
fn forks_are_detected_and_kept_as_evidence() {
    let (_da, a) = store();
    let node = key(9);
    publish_n(&a, &node, 0, 3);
    let bytes = entry_bytes(&a, &node);
    let honest3 = a.sign_head(&node).unwrap().unwrap();

    // The node also signs a different entry 2.
    let other2 = seal_unsigned(&LogEntry {
        seq: 2,
        prev: Some(Id::of(&bytes[0])),
        obj: Id::of(b"other"),
        extra: Map::new(),
    })
    .unwrap();
    let forked2 = seal(
        &LogHead {
            node: node.public(),
            seq: 2,
            entry: other2.id(),
            extra: Map::new(),
        },
        &node,
    )
    .unwrap();
    let honest2 = seal(
        &LogHead {
            node: node.public(),
            seq: 2,
            entry: Id::of(&bytes[1]),
            extra: Map::new(),
        },
        &node,
    )
    .unwrap();

    // Store B has the honest log; the forked head contradicts it.
    let (_db, b) = store();
    b.put_object(honest3.bytes()).unwrap();
    b.append_log_entries(node.public(), &bytes).unwrap();
    assert!(matches!(
        b.put_object(forked2.bytes()),
        Err(StoreError::Forked { seq: 2, .. })
    ));
    assert!(b.is_forked(node.public()).unwrap());
    assert!(b.has_object(forked2.id()).unwrap(), "the signed evidence is kept");

    // Store C sees the forked head first: a later honest head no longer hides it.
    let (_dc, c) = store();
    c.put_object(forked2.bytes()).unwrap();
    c.put_object(honest3.bytes()).unwrap();
    assert!(
        c.append_log_entries(node.public(), &bytes).is_err(),
        "entry 2 contradicts a signed head"
    );
    assert!(matches!(c.put_object(honest2.bytes()), Err(StoreError::Forked { .. })));
    assert!(c.is_forked(node.public()).unwrap());
}

#[test]
fn recent_tips_and_author_clock() {
    let (_dir, store) = store();
    let kp = key(1);
    assert_eq!(store.author_clock(kp.public()).unwrap(), 0);
    let a = seal(&revision(&kp, "A", &["general"], Id::of(b"a")), &kp).unwrap();
    let b = seal(&revision(&kp, "B", &["general"], Id::of(b"b")), &kp).unwrap();
    store.put_object(a.bytes()).unwrap();
    store.put_object(b.bytes()).unwrap();
    let a2 = seal(&edit(&kp, a.id(), vec![a.id()], 7, "A, edited"), &kp).unwrap();
    store.put_object(a2.bytes()).unwrap();

    let recent: Vec<String> = store
        .recent_tips(10)
        .unwrap()
        .into_iter()
        .map(|r| r.object.title)
        .collect();
    assert_eq!(recent, vec!["A, edited", "B"], "newest first, one entry per item");
    assert_eq!(store.recent_tips(1).unwrap().len(), 1);
    assert_eq!(store.author_clock(kp.public()).unwrap(), 7);
}

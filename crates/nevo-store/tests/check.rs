//! `Store::check`: finds damage of every kind, and repair fixes what it safely can.

use std::fs;
use std::path::Path;

use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::{CheckOptions, CheckReport, Problem, Store};
use rusqlite::Connection;

const CHECK: CheckOptions = CheckOptions {
    blobs: true,
    repair: false,
};
const REPAIR: CheckOptions = CheckOptions {
    blobs: true,
    repair: true,
};

fn time() -> Time {
    Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Unknown,
    }
}

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn revision(author: &Keypair, clock: u64, title: &str, blob: Id) -> Revision {
    Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock,
        time: time(),
        kind: Kind::Guide,
        title: title.into(),
        lang: "en".into(),
        topics: vec!["water/treatment".parse().unwrap(), "general".parse().unwrap()],
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

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn sql(dir: &Path, statement: &str) {
    let conn = Connection::open(dir.join("objects.sqlite")).unwrap();
    conn.execute_batch(statement).unwrap();
}

fn assert_clean(report: &CheckReport) {
    assert!(report.is_ok(), "{report}");
    assert_eq!(report.warning_count, 0, "{report}");
}

/// A store holding every kind of object and log state: published revisions with edits and
/// blobs, attestations, keys, revocations, derived data, unknown types, another node's log
/// received through sync, and a fork.
fn full_store() -> (tempfile::TempDir, Store, Sealed<Revision>) {
    let (dir, store) = store();
    let (node, author, curator) = (key(9), key(1), key(2));
    let blob = store.put_blob(&b"hello"[..]).unwrap();
    let rev = seal(&revision(&author, 1, "Boiling water", blob.id), &author).unwrap();
    store.publish(&node, rev.bytes()).unwrap();
    let edit = seal(
        &Revision {
            item: Some(rev.id()),
            parents: vec![rev.id()],
            ..revision(&author, 2, "Boiling water, edited", blob.id)
        },
        &author,
    )
    .unwrap();
    store.publish(&node, edit.bytes()).unwrap();
    store.sign_head(&node).unwrap();

    let att = Attestation {
        about: Subject::Object(rev.id()),
        claim: Claim::Verified,
        topics: vec!["water".parse().unwrap()],
        related: Some(edit.id()),
        note: None,
        author: curator.public(),
        clock: 1,
        time: time(),
        extra: Map::new(),
    };
    store.publish(&node, seal(&att, &curator).unwrap().bytes()).unwrap();
    let trust = Attestation {
        about: Subject::Key(author.public()),
        claim: Claim::Trust,
        clock: 2,
        related: None,
        ..att
    };
    store.put_object(seal(&trust, &curator).unwrap().bytes()).unwrap();
    let record = KeyRecord {
        key: author.public(),
        kind: KeyKind::Person,
        name: "Maria".into(),
        recovery: Some(key(3).public()),
        clock: 3,
        time: time(),
        extra: Map::new(),
    };
    store.publish(&node, seal(&record, &author).unwrap().bytes()).unwrap();
    let revoke = Revoke {
        key: author.public(),
        from: 2,
        by: key(3).public(),
        clock: 1,
        time: time(),
        extra: Map::new(),
    };
    store.put_object(seal(&revoke, &key(3)).unwrap().bytes()).unwrap();
    let derived = Derived {
        from: blob.id,
        producer: "nevo-ocr/1".into(),
        out: Id::of(b"text"),
        author: node.public(),
        extra: Map::new(),
    };
    store.put_object(seal(&derived, &node).unwrap().bytes()).unwrap();
    let mut future = Map::new();
    future.insert("t", "future").insert("v", 1u64).insert("x", "kept as is");
    store.put_object(&cbor::encode_map(&future)).unwrap();
    store.sign_head(&node).unwrap();

    // Another node's log, received through sync, and that node signing a contradicting head.
    let (other_dir, other) = (tempfile::tempdir().unwrap(), key(8));
    let remote = Store::open(other_dir.path()).unwrap();
    for clock in 10..13 {
        let r = seal(
            &revision(&other, clock, &format!("Remote {clock}"), Id::of(b"x")),
            &other,
        )
        .unwrap();
        remote.publish(&other, r.bytes()).unwrap();
        store.put_object(r.bytes()).unwrap();
    }
    let head = remote.sign_head(&other).unwrap().unwrap();
    store.put_object(head.bytes()).unwrap();
    let entries: Vec<Vec<u8>> = remote
        .log_entries(other.public(), 0, 100)
        .unwrap()
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    store.append_log_entries(other.public(), &entries).unwrap();
    let forked = seal(
        &LogHead {
            node: other.public(),
            seq: 2,
            entry: Id::of(b"another entry 2"),
            extra: Map::new(),
        },
        &other,
    )
    .unwrap();
    assert!(store.put_object(forked.bytes()).is_err());
    assert!(store.is_forked(other.public()).unwrap());
    (dir, store, rev)
}

#[test]
fn a_healthy_store_passes_and_rebuilds_exactly() {
    let (dir, store, _) = full_store();
    let report = store.check(CHECK);
    assert_clean(&report);
    assert!(report.objects > 15);
    assert_eq!(report.log_entries, 7, "4 own entries and 3 received");
    assert_eq!(report.log_heads, 3, "the forked head is evidence, not indexed");
    assert_eq!((report.blob_files, report.blobs_hashed), (1, 1));

    // Repairing a healthy store changes nothing and writes no report.
    let report = store.check(REPAIR);
    assert_clean(&report);
    assert!(report.repairs.is_empty());
    assert!(!dir.path().join("quarantine").exists());
}

#[test]
fn wrong_index_tables_are_found_and_rebuilt() {
    let (dir, store, rev) = full_store();
    let in_topic = || {
        let mut items = store.items_in_topic(&"water".parse().unwrap()).unwrap();
        items.sort();
        items
    };
    let topics_before = in_topic();
    assert_eq!(topics_before.len(), 4);
    sql(
        dir.path(),
        "DELETE FROM revision_topics WHERE topic = 'water/treatment';
         DELETE FROM keys;
         INSERT INTO revision_parents (rev, parent) VALUES (x'00', x'01');
         UPDATE log_heads SET entry = x'02' WHERE seq = 2;",
    );
    assert!(in_topic().is_empty());

    let report = store.check(CHECK);
    assert!(!report.is_ok());
    let tables: Vec<&str> = report
        .errors
        .iter()
        .filter_map(|p| match p {
            Problem::Index { table, .. } => Some(*table),
            _ => None,
        })
        .collect();
    assert_eq!(
        tables,
        ["revision_parents", "revision_topics", "keys", "log_heads"],
        "{report}"
    );
    assert!(
        report.errors.iter().any(|p| matches!(p, Problem::Head { .. })),
        "the changed head row no longer matches its object: {report}"
    );

    let report = store.check(REPAIR);
    assert_eq!(report.repairs.len(), 1, "{report}");
    assert_clean(&store.check(CHECK));
    assert_eq!(in_topic(), topics_before);
    assert_eq!(store.key_records(key(1).public()).unwrap().len(), 1);
    assert_eq!(store.item_tips(rev.id()).unwrap().len(), 1);
}

#[test]
fn corrupt_objects_are_quarantined_not_deleted() {
    let (dir, store, rev) = full_store();
    let att = store
        .attestations_about(Subject::Key(key(1).public()))
        .unwrap()
        .remove(0);
    // Bit rot inside the stored bytes of an attestation that isn't in any log.
    let mut rotten = att.bytes().to_vec();
    let at = rotten.windows(5).position(|w| w == b"trust").unwrap();
    rotten[at] = b'T';
    let conn = Connection::open(dir.path().join("objects.sqlite")).unwrap();
    conn.execute(
        "UPDATE objects SET bytes = ?1 WHERE id = ?2",
        rusqlite::params![rotten, att.id().digest()],
    )
    .unwrap();
    // And an object stored under the wrong ID.
    conn.execute(
        "UPDATE objects SET id = ?1 WHERE id = ?2",
        rusqlite::params![Id::of(b"elsewhere").digest(), rev.id().digest()],
    )
    .unwrap();
    drop(conn);

    let report = store.check(CHECK);
    let corrupt = report
        .errors
        .iter()
        .filter(|p| matches!(p, Problem::CorruptObject { .. }))
        .count();
    assert_eq!(corrupt, 2, "{report}");

    let report = store.check(REPAIR);
    let quarantine = report.quarantine_report.clone().expect("a quarantine report");
    let text = fs::read_to_string(&quarantine).unwrap();
    assert!(text.contains(&att.id().to_string()), "{text}");
    let kept = fs::read(dir.path().join("quarantine/objects").join(att.id().to_string())).unwrap();
    assert_eq!(kept, rotten, "the damaged bytes are kept for inspection");
    assert!(!store.has_object(att.id()).unwrap());
    assert!(store
        .attestations_about(Subject::Key(key(1).public()))
        .unwrap()
        .is_empty());

    // The revision's log entry still names it, so a re-sync can bring it back. Storing it again
    // makes the store whole.
    store.put_object(rev.bytes()).unwrap();
    assert_clean(&store.check(CHECK));
}

#[test]
fn corrupt_blobs_are_found_only_when_hashing_and_quarantined() {
    let (dir, store) = store();
    let blob = store.put_blob(&b"the real content"[..]).unwrap();
    fs::write(store.blob_path(blob.id), b"the rotten content").unwrap();
    fs::write(dir.path().join("blobs/stray.txt"), b"?").unwrap();
    fs::write(dir.path().join("tmp/123-0.part"), b"half a blob").unwrap();

    let quick = store.check(CheckOptions::default());
    assert!(quick.is_ok(), "{quick}");
    assert_eq!(quick.blobs_hashed, 0);
    assert_eq!(quick.warning_count, 2, "{quick}");

    let report = store.check(CHECK);
    assert!(matches!(report.errors.as_slice(), [Problem::CorruptBlob { id, .. }] if *id == blob.id));

    let report = store.check(REPAIR);
    assert!(report.quarantine_report.is_some(), "{report}");
    assert!(!store.has_blob(blob.id));
    let moved = dir.path().join("quarantine/blobs").join(blob.id.to_string());
    assert_eq!(fs::read(moved).unwrap(), b"the rotten content");
    assert!(store.check(CHECK).is_ok());

    // Fetching the blob again restores it.
    store.put_blob(&b"the real content"[..]).unwrap();
    assert!(store.check(CHECK).is_ok());
}

#[test]
fn broken_logs_are_reported_and_left_alone() {
    let (dir, store, _) = full_store();
    let conn = Connection::open(dir.path().join("objects.sqlite")).unwrap();
    conn.execute(
        "DELETE FROM log_entries WHERE node = ?1 AND seq = 2",
        [key(9).public().as_bytes()],
    )
    .unwrap();
    drop(conn);
    let report = store.check(REPAIR);
    assert!(
        report
            .errors
            .iter()
            .any(|p| matches!(p, Problem::Log { seq: 3, why, .. } if why.contains("gap"))),
        "{report}"
    );
    assert!(
        report.repairs.is_empty(),
        "logs are not derivable, so repair leaves them"
    );
    assert!(!store.check(CHECK).is_ok());
}

#[test]
fn damaged_database_pages_are_reported() {
    let (dir, store, _) = full_store();
    drop(store);
    // A dead SD card sector inside one of SQLite's own indexes.
    let path = dir.path().join("objects.sqlite");
    let conn = Connection::open(&path).unwrap();
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let (page_size, root): (i64, i64) = conn
        .query_row(
            "SELECT (SELECT page_size FROM pragma_page_size), rootpage FROM sqlite_master WHERE name = 'revisions_item'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    drop(conn);
    let mut bytes = fs::read(&path).unwrap();
    let (page_size, root) = (page_size as usize, root as usize);
    bytes[(root - 1) * page_size..root * page_size].fill(0x5a);
    fs::write(&path, bytes).unwrap();

    let store = Store::open(dir.path()).unwrap();
    let report = store.check(CHECK);
    assert!(
        report.errors.iter().any(|p| matches!(p, Problem::Database(_))),
        "{report}"
    );
    // Only SQLite's index is hit: every object, index table and log is still checked and fine.
    assert_eq!(report.objects, 22, "{report}");
    assert!(
        report
            .errors
            .iter()
            .all(|p| matches!(p, Problem::Database(_) | Problem::Failed { step: "database", .. })),
        "{report}"
    );

    // Repair can't rebuild an unreadable page, but it must fail cleanly and change nothing.
    let report = store.check(REPAIR);
    assert!(!report.is_ok());
    assert!(report.quarantine_report.is_none(), "{report}");
    assert_eq!(store.check(CHECK).objects, 22);
}

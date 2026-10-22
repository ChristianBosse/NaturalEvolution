//! Property tests: random sequences of log operations and random blob writes.
//!
//! Each test runs `PROPTEST_CASES` cases (64 by default here: every case opens several SQLite
//! stores and runs dozens of transactions). Heavier runs:
//! `PROPTEST_CASES=2000 cargo test -p nevo-store --test properties`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::sync::LazyLock;

use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::{Store, StoreError};
use proptest::prelude::*;

fn config(default_cases: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default_cases);
    ProptestConfig::with_cases(cases)
}

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

/// Revisions to publish, sealed once: signing is the slow part of building them.
static REVISIONS: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
    let author = key(1);
    (0..8u8)
        .map(|i| {
            let revision = Revision {
                item: None,
                parents: vec![],
                author: author.public(),
                clock: 1,
                time: Time {
                    ms: 1_792_000_000_000,
                    src: TimeSource::Unknown,
                },
                kind: Kind::Guide,
                title: format!("Guide {i}"),
                lang: "en".into(),
                topics: vec!["general".parse().unwrap()],
                license: "CC-BY-SA-4.0".into(),
                vis: Visibility::Public,
                content: vec![ContentFile {
                    name: "guide.md".into(),
                    blob: Id::of(&[i]),
                    size: 1,
                    mime: "text/markdown".into(),
                }],
                source: None,
                summary: None,
                extra: Map::new(),
            };
            seal(&revision, &author).unwrap().bytes().to_vec()
        })
        .collect()
});

// ---------------------------------------------------------------------------------------------
// Logs: two stores write the same node's log (the honest copy and a fork made with the same key,
// as a restored backup or a cloned SD card would), a third store receives both.

#[derive(Clone, Debug)]
enum Change {
    None,
    Shuffle(u64),
    Reverse,
    Drop(usize),
    Repeat(usize),
    /// Flip bits of one byte of one entry.
    Tamper(usize, usize, u8),
    /// Swap one entry for the entry at the same place in the other store's log.
    Splice(usize),
}

#[derive(Clone, Debug)]
enum Op {
    Publish {
        fork: bool,
        rev: usize,
    },
    SignHead {
        fork: bool,
    },
    DeliverHead {
        pick: usize,
    },
    /// Entries from `start` (the receiver's position when `None`, as sync does) up to `end` (the
    /// highest head the receiver holds when `None`).
    DeliverEntries {
        fork: bool,
        start: Option<usize>,
        end: Option<usize>,
        change: Change,
    },
    /// The receiving store also publishes to its own node's log.
    PublishOwn {
        rev: usize,
    },
    SignOwnHead,
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        6 => Just(Change::None),
        1 => any::<u64>().prop_map(Change::Shuffle),
        1 => Just(Change::Reverse),
        1 => any::<usize>().prop_map(Change::Drop),
        1 => any::<usize>().prop_map(Change::Repeat),
        2 => (any::<usize>(), any::<usize>(), 1..=255u8).prop_map(|(e, b, x)| Change::Tamper(e, b, x)),
        1 => any::<usize>().prop_map(Change::Splice),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let fork = prop::bool::weighted(0.25);
    prop_oneof![
        // The fork mostly publishes other revisions than the honest log, so the two differ.
        5 => fork.prop_flat_map(|fork| {
            let revs = if fork { 2..REVISION_COUNT } else { 0..REVISION_COUNT - 2 };
            revs.prop_map(move |rev| Op::Publish { fork, rev })
        }),
        3 => fork.prop_map(|fork| Op::SignHead { fork }),
        4 => any::<usize>().prop_map(|pick| Op::DeliverHead { pick }),
        6 => (fork, prop::option::weighted(0.3, 0..6usize), prop::option::weighted(0.3, 0..10usize), change())
            .prop_map(|(fork, start, end, change)| Op::DeliverEntries { fork, start, end, change }),
        1 => (0..REVISION_COUNT).prop_map(|rev| Op::PublishOwn { rev }),
        1 => Just(Op::SignOwnHead),
    ]
}

const REVISION_COUNT: usize = 8;

/// What the receiving store should know about the node's log, worked out independently.
#[derive(Clone, Debug, Default, PartialEq)]
struct Model {
    /// Entry IDs, in order: entry `seq` is at index `seq - 1`.
    entries: Vec<Id>,
    /// Signed heads accepted: seq -> entry ID.
    heads: BTreeMap<u64, Id>,
    forked: bool,
}

impl Model {
    /// A head conflicts when the node already signed, or the store already holds, a different
    /// entry at that position.
    fn head_conflicts(&self, head: &LogHead) -> bool {
        let differs = |id: Option<&Id>| id.is_some_and(|id| *id != head.entry);
        differs(self.heads.get(&head.seq)) || differs(self.entries.get(head.seq as usize - 1))
    }

    /// The entry IDs after appending `batch`, or `None` if the store must refuse it.
    fn append(&self, batch: &[Vec<u8>]) -> Option<Vec<Id>> {
        let mut entries = self.entries.clone();
        let mut last_seq = None;
        for bytes in batch {
            let entry = open::<LogEntry>(bytes).ok()?;
            let e = &entry.object;
            if e.seq != entries.len() as u64 + 1 || e.prev != entries.last().copied() {
                return None;
            }
            if self.heads.get(&e.seq).is_some_and(|h| *h != entry.id()) {
                return None;
            }
            entries.push(entry.id());
            last_seq = Some(e.seq);
        }
        match last_seq {
            Some(seq) if self.heads.get(&seq) != entries.last() => None,
            _ => Some(entries),
        }
    }
}

/// Everything a store shows about one node's log.
#[derive(Debug, PartialEq)]
struct LogView {
    entries: Vec<Vec<u8>>,
    position: u64,
    head: Option<(u64, Id)>,
    forked: bool,
}

/// Reads a node's log and checks it is gap-free and hash-chained.
fn view(store: &Store, node: PublicKey) -> LogView {
    let listed = store.log_entries(node, 0, usize::MAX).unwrap();
    let mut prev = None;
    for (i, (bytes, obj)) in listed.iter().enumerate() {
        let entry = open::<LogEntry>(bytes).expect("stored entries are valid");
        assert_eq!(entry.object.seq, i as u64 + 1, "gap-free");
        assert_eq!(entry.object.prev, prev, "hash-chained");
        assert_eq!(entry.object.obj, *obj);
        prev = Some(entry.id());
    }
    let view = LogView {
        entries: listed.into_iter().map(|(bytes, _)| bytes).collect(),
        position: store.log_position(node).unwrap(),
        head: store.log_head(node).unwrap(),
        forked: store.is_forked(node).unwrap(),
    };
    assert_eq!(view.position, view.entries.len() as u64);
    view
}

fn apply_change(batch: &mut Vec<Vec<u8>>, change: &Change, other_log: &[Vec<u8>], start: usize) {
    if batch.is_empty() {
        return;
    }
    let n = batch.len();
    match *change {
        Change::None => {}
        Change::Shuffle(mut seed) => {
            for i in (1..n).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                batch.swap(i, (seed % (i as u64 + 1)) as usize);
            }
        }
        Change::Reverse => batch.reverse(),
        Change::Drop(i) => {
            batch.remove(i % n);
        }
        Change::Repeat(i) => {
            let copy = batch[i % n].clone();
            batch.insert(i % n, copy);
        }
        Change::Tamper(e, b, x) => {
            let entry = &mut batch[e % n];
            let at = b % entry.len();
            entry[at] ^= x;
        }
        Change::Splice(i) => {
            if let Some(other) = other_log.get(start + i % n) {
                batch[i % n] = other.clone();
            }
        }
    }
}

struct World {
    _dirs: Vec<tempfile::TempDir>,
    /// The node's own store, and a second store writing a fork of the same node's log.
    honest: Store,
    fork: Store,
    /// Receives the node's heads and entries from both.
    receiver: Store,
    node: Keypair,
    own: Keypair,
    /// Every head the node signed, from either store.
    heads: Vec<Vec<u8>>,
    model: Model,
}

impl World {
    fn new() -> World {
        let (d1, honest) = store();
        let (d2, fork) = store();
        let (d3, receiver) = store();
        World {
            _dirs: vec![d1, d2, d3],
            honest,
            fork,
            receiver,
            node: key(9),
            own: key(10),
            heads: vec![],
            model: Model::default(),
        }
    }

    fn source(&self, fork: bool) -> &Store {
        if fork {
            &self.fork
        } else {
            &self.honest
        }
    }

    fn log(&self, fork: bool) -> Vec<Vec<u8>> {
        view(self.source(fork), self.node.public()).entries
    }

    fn run(&mut self, op: &Op) {
        let node = self.node.public();
        match op {
            Op::Publish { fork, rev } => {
                let before = self.log(*fork).len();
                let bytes = &REVISIONS[*rev];
                let put = self.source(*fork).publish(&self.node, bytes).unwrap();
                let listed = self
                    .source(*fork)
                    .log_entries(node, 0, usize::MAX)
                    .unwrap()
                    .iter()
                    .filter(|(_, obj)| *obj == put.id)
                    .count();
                assert_eq!(listed, 1, "an object is listed once");
                assert!(self.log(*fork).len() - before <= 1);
            }
            Op::SignHead { fork } => {
                let store = self.source(*fork);
                match store.sign_head(&self.node).unwrap() {
                    Some(head) => {
                        assert_eq!(head.object.seq, store.log_position(node).unwrap());
                        self.heads.push(head.bytes().to_vec());
                    }
                    None => assert_eq!(store.log_position(node).unwrap(), 0),
                }
            }
            Op::DeliverHead { pick } => {
                if self.heads.is_empty() {
                    return;
                }
                let bytes = &self.heads[pick % self.heads.len()];
                let head = open::<LogHead>(bytes).unwrap().object;
                let conflict = self.model.head_conflicts(&head);
                let result = self.receiver.put_object(bytes);
                if conflict {
                    assert!(
                        matches!(result, Err(StoreError::Forked { seq, .. }) if seq == head.seq),
                        "expected a fork at {}: {result:?}",
                        head.seq
                    );
                    self.model.forked = true;
                } else {
                    result.unwrap();
                    self.model.heads.entry(head.seq).or_insert(head.entry);
                }
            }
            Op::DeliverEntries {
                fork,
                start,
                end,
                change,
            } => {
                let log = self.log(*fork);
                let other = self.log(!*fork);
                let highest = self.model.heads.keys().next_back().copied().unwrap_or(0) as usize;
                let start = start.unwrap_or(self.model.entries.len()).min(log.len());
                let end = end.unwrap_or(highest).clamp(start, log.len());
                let mut batch = log[start..end].to_vec();
                apply_change(&mut batch, change, &other, start);

                let before = view(&self.receiver, node);
                let expected = self.model.append(&batch);
                let result = self.receiver.append_log_entries(node, &batch);
                match (expected, result) {
                    (Some(entries), Ok(position)) => {
                        assert_eq!(position, entries.len() as u64);
                        self.model.entries = entries;
                    }
                    (None, Err(_)) => {
                        assert_eq!(view(&self.receiver, node), before, "a failed batch changes nothing");
                    }
                    (expected, result) => panic!("expected {expected:?}, got {result:?}"),
                }
            }
            Op::PublishOwn { rev } => {
                self.receiver.publish(&self.own, &REVISIONS[*rev]).unwrap();
            }
            Op::SignOwnHead => {
                self.receiver.sign_head(&self.own).unwrap();
            }
        }
    }

    fn check(&self) {
        let node = self.node.public();
        // The writers' own logs: chained, and never seen as forked by their own store.
        for store in [&self.honest, &self.fork] {
            assert!(!view(store, node).forked);
        }
        assert!(!view(&self.receiver, self.own.public()).forked);

        let received = view(&self.receiver, node);
        let ids: Vec<Id> = received.entries.iter().map(|e| Id::of(e)).collect();
        assert_eq!(ids, self.model.entries, "entries");
        assert_eq!(
            received.forked, self.model.forked,
            "forks are recorded exactly when they happen"
        );
        assert_eq!(
            received.head,
            self.model.heads.iter().next_back().map(|(s, e)| (*s, *e)),
            "highest signed head"
        );
        assert!(
            received.position <= received.head.map_or(0, |(seq, _)| seq),
            "a foreign log never goes past what its node signed"
        );
        for (seq, entry) in &self.model.heads {
            if let Some(stored) = ids.get(*seq as usize - 1) {
                assert_eq!(stored, entry, "stored entries agree with every accepted head");
            }
        }
    }
}

proptest! {
    #![proptest_config(config(64))]

    #[test]
    fn logs_stay_chained_and_honest(ops in prop::collection::vec(op(), 1..50)) {
        let mut world = World::new();
        for op in &ops {
            world.run(op);
            world.check();
        }
    }
}

/// A node restored from an old backup (or two devices sharing a node key) signs a different log.
/// A store that already holds the first copy records the fork when the second one's head arrives.
#[test]
fn a_fork_is_recorded_once_its_heads_meet() {
    let (_d1, honest) = store();
    let (_d2, restored) = store();
    let (_d3, receiver) = store();
    let node = key(9);
    for rev in &REVISIONS[..3] {
        honest.publish(&node, rev).unwrap();
    }
    let head3 = honest.sign_head(&node).unwrap().unwrap();
    for rev in [&REVISIONS[0], &REVISIONS[4]] {
        restored.publish(&node, rev).unwrap();
    }
    let other2 = restored.sign_head(&node).unwrap().unwrap();

    receiver.put_object(head3.bytes()).unwrap();
    let entries: Vec<Vec<u8>> = honest
        .log_entries(node.public(), 0, 10)
        .unwrap()
        .into_iter()
        .map(|(e, _)| e)
        .collect();
    assert_eq!(receiver.append_log_entries(node.public(), &entries).unwrap(), 3);
    assert!(!receiver.is_forked(node.public()).unwrap());
    assert!(matches!(
        receiver.put_object(other2.bytes()),
        Err(StoreError::Forked { seq: 2, .. })
    ));
    assert!(receiver.is_forked(node.public()).unwrap());
}

/// The same restored node, from its own side: a peer sends back a head the node signed before the
/// backup was restored. That head is stored (nothing contradicts it yet), and from then on every
/// `publish` that reaches its position is refused ("contradicts its signed head"), so the node
/// can never publish again. Recording the fork and carrying on would keep it alive.
#[test]
#[ignore = "known gap: a node restored from a backup can't publish past a head it signed before"]
fn a_restored_node_can_still_publish() {
    let (_d1, honest) = store();
    let (_d2, restored) = store();
    let node = key(9);
    for rev in &REVISIONS[..3] {
        honest.publish(&node, rev).unwrap();
    }
    let head3 = honest.sign_head(&node).unwrap().unwrap();

    restored.publish(&node, &REVISIONS[0]).unwrap();
    restored.put_object(head3.bytes()).unwrap();
    restored.publish(&node, &REVISIONS[5]).unwrap();
    let third = restored.publish(&node, &REVISIONS[6]);
    assert!(third.is_ok(), "{third:?}");
}

// ---------------------------------------------------------------------------------------------
// Blobs

/// The store's write buffer (`blobs.rs`): sizes around it exercise the streaming loop's edges.
const BUFFER: usize = 256 * 1024;

fn blob_size() -> impl Strategy<Value = usize> {
    prop_oneof![
        4 => 0..2000usize,
        1 => prop::sample::select(vec![0, 1, BUFFER - 1, BUFFER, BUFFER + 1, 2 * BUFFER, 2 * BUFFER + 1]),
        1 => 0..3 * BUFFER,
    ]
}

/// Deterministic bytes from a seed: random enough, and cheap to shrink.
fn blob_data(seed: u64, size: usize) -> Vec<u8> {
    let mut state = seed | 1;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// A reader that hands out data in uneven pieces, is interrupted now and then, and can fail
/// partway through.
struct Awkward<'a> {
    data: &'a [u8],
    pos: usize,
    pieces: Vec<usize>,
    calls: usize,
    fail_at: Option<usize>,
}

impl Read for Awkward<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if self.calls % 5 == 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.fail_at.is_some_and(|at| self.pos >= at) {
            return Err(io::Error::other("the USB stick was pulled out"));
        }
        let piece = self.pieces[self.calls % self.pieces.len()].max(1);
        let n = piece.min(buf.len()).min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn awkward<'a>(data: &'a [u8], pieces: &[usize], fail_at: Option<usize>) -> Awkward<'a> {
    Awkward {
        data,
        pos: 0,
        pieces: pieces.to_vec(),
        calls: 0,
        fail_at,
    }
}

fn tmp_is_empty(store: &Store) -> bool {
    fs::read_dir(store.root().join("tmp")).unwrap().next().is_none()
}

proptest! {
    #![proptest_config(config(64))]

    #[test]
    fn blobs_store_exactly_what_was_read(
        seed in any::<u64>(),
        size in blob_size(),
        pieces in prop::collection::vec(prop_oneof![1..16usize, 1..BUFFER + 2], 1..5),
        fail_at in any::<usize>(),
    ) {
        let (_dir, store) = store();
        let data = blob_data(seed, size);
        let id = Id::of(&data);

        // A reader that fails keeps nothing, not even a temporary file.
        if !data.is_empty() {
            let at = fail_at % data.len();
            prop_assert!(store.put_blob(awkward(&data, &pieces, Some(at))).is_err());
            prop_assert!(!store.has_blob(id));
            prop_assert!(!store.has_blob(Id::of(&data[..at])));
            prop_assert!(tmp_is_empty(&store));
        }

        // The wrong expected ID keeps nothing either.
        let wrong = Id::of(&[&data[..], b"!"].concat());
        let result = store.put_blob_expecting(awkward(&data, &pieces, None), wrong);
        let mismatch = matches!(result, Err(StoreError::BlobMismatch { expected, actual }) if expected == wrong && actual == id);
        prop_assert!(mismatch, "{:?}", result);
        prop_assert!(!store.has_blob(id) && !store.has_blob(wrong));
        prop_assert!(tmp_is_empty(&store));

        let info = store.put_blob(awkward(&data, &pieces, None)).unwrap();
        prop_assert_eq!(info.id, id);
        prop_assert_eq!(info.size, data.len() as u64);
        prop_assert!(info.new);
        prop_assert_eq!(fs::read(store.blob_path(id)).unwrap(), data.clone());
        prop_assert_eq!(store.verify_blob(id).unwrap(), Some(true));
        prop_assert!(tmp_is_empty(&store));

        // Writing it again, in other pieces, changes nothing.
        let again = store.put_blob_expecting(&data[..], id).unwrap();
        prop_assert_eq!(again.id, id);
        prop_assert!(!again.new);
        prop_assert_eq!(store.verify_blob(id).unwrap(), Some(true));
        prop_assert!(tmp_is_empty(&store));
    }
}

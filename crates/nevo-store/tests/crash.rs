//! Crash testing: a child process writes to a store nonstop (blobs, publishes, signed heads,
//! logs received through sync) and is killed at a random moment, over and over. After every
//! kill the store must open and pass `Store::check` with zero errors: no corrupt objects, no
//! broken log chains, no half-written blob under a real ID, no fork of the node's own log.
//! Leftover temporary files are the only thing allowed.
//!
//! The child is this test binary itself, run with `NEVO_CRASH_WRITER` set; see
//! [`writer_process`].
//!
//! Limits: SIGKILL ends the process but not the operating system, so data already handed to the
//! kernel survives even if it was never flushed to the disk. Real power loss also drops the
//! page cache, which is stricter. That case rests on SQLite's `synchronous = FULL` and on blob
//! writes syncing the file before the rename and the folder after it; this test can't prove those
//! calls reach the hardware. A fault-injecting filesystem (or a VM with its power cut) would.

use std::fs::File;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nevo_format::cbor::Map;
use nevo_format::*;
use nevo_store::{CheckOptions, Problem, Store};

/// When set, `writer_process` writes to the stores in this folder until it is killed.
const WRITER_ENV: &str = "NEVO_CRASH_WRITER";

#[test]
fn survives_10_kills() {
    crash_loop(10);
}

#[test]
#[ignore = "slow: run with --ignored"]
fn survives_200_kills() {
    crash_loop(200);
}

/// Does nothing in a normal test run. The crash harness runs it as its child process.
#[test]
fn writer_process() {
    if let Ok(dir) = std::env::var(WRITER_ENV) {
        write_forever(Path::new(&dir));
    }
}

fn crash_loop(kills: usize) {
    let dir = tempfile::tempdir().unwrap();
    let exe = std::env::current_exe().unwrap();
    let log = dir.path().join("writer.log");
    let mut rng = Rng::seeded();
    let started = Instant::now();
    let mut mid_write = 0;
    let mut objects = 0;
    for kill in 1..=kills {
        let mut child = Command::new(&exe)
            .args(["writer_process", "--exact", "--nocapture", "--test-threads=1"])
            .env(WRITER_ENV, dir.path())
            .stdout(Stdio::null())
            .stderr(File::create(&log).unwrap())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(20 + rng.below(300)));
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "the writer stopped by itself ({status}) before kill {kill}:\n{}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        // SIGKILL on Unix: no destructors, no flushing, nothing gets to finish.
        child.kill().unwrap();
        child.wait().unwrap();

        for name in ["node", "remote"] {
            let path = dir.path().join(name);
            if !path.join("objects.sqlite").exists() {
                continue; // killed before the store was created
            }
            let store = Store::open(&path).unwrap();
            let report = store.check(CheckOptions {
                blobs: true,
                repair: false,
            });
            assert!(report.is_ok(), "{name} store after kill {kill}:\n{report}");
            for warning in &report.warnings {
                assert!(
                    matches!(warning, Problem::TmpFile(_)),
                    "{name} store after kill {kill}: {warning}"
                );
            }
            for node in [key(9), key(8)] {
                assert!(
                    !store.is_forked(node.public()).unwrap(),
                    "{name} store after kill {kill}: a log forked, so a committed write was lost"
                );
            }
            // Blobs are durable before the revisions that use them are published.
            assert!(store.missing_blobs().unwrap().is_empty(), "{name} after kill {kill}");
            if name == "node" {
                mid_write += usize::from(report.warning_count > 0);
                objects = report.objects;
            }
        }
    }
    eprintln!(
        "{kills} kills in {:.1}s: {objects} objects written; temporary files left behind after {mid_write} kills",
        started.elapsed().as_secs_f64()
    );
    assert!(objects > 0, "the writer never got to write anything");
}

// ---------------------------------------------------------------------------------------------
// The writer

fn key(n: u8) -> Keypair {
    Keypair::from_secret([n; 32])
}

fn time() -> Time {
    Time {
        ms: 1_792_000_000_000,
        src: TimeSource::Unknown,
    }
}

fn revision(author: &Keypair, clock: u64, blob: Id, size: u64) -> Revision {
    Revision {
        item: None,
        parents: vec![],
        author: author.public(),
        clock,
        time: time(),
        kind: Kind::Guide,
        title: format!("Guide {clock}"),
        lang: "en".into(),
        topics: vec!["general".parse().unwrap()],
        license: "CC-BY-SA-4.0".into(),
        vis: Visibility::Public,
        content: vec![ContentFile {
            name: "guide.md".into(),
            blob,
            size,
            mime: "text/markdown".into(),
        }],
        source: None,
        summary: None,
        extra: Map::new(),
    }
}

/// Writes a random mix of everything a node does, as fast as it can, until killed. Each run
/// continues where the last one was cut off: clocks come from the store.
fn write_forever(dir: &Path) -> ! {
    let store = Store::open(dir.join("node")).unwrap();
    let remote = Store::open(dir.join("remote")).unwrap();
    let (node, remote_node, author, curator) = (key(9), key(8), key(1), key(2));
    // The remote node's revisions all use this blob, so both stores have every blob they need.
    let shared = remote.put_blob(&b"remote guide"[..]).unwrap();
    store.put_blob(&b"remote guide"[..]).unwrap();
    let mut rng = Rng::seeded();
    loop {
        match rng.below(10) {
            // A new guide: its blob first, then the revision that uses it.
            0..=3 => {
                let size = if rng.below(4) == 0 {
                    rng.below(600_000)
                } else {
                    rng.below(4000)
                };
                let blob = store.put_blob(&rng.bytes(size as usize)[..]).unwrap();
                let clock = store.author_clock(author.public()).unwrap() + 1;
                let rev = seal(&revision(&author, clock, blob.id, size), &author).unwrap();
                store.publish(&node, rev.bytes()).unwrap();
            }
            4 | 5 => {
                let clock = store.author_clock(curator.public()).unwrap() + 1;
                let att = Attestation {
                    about: Subject::Key(author.public()),
                    claim: Claim::Trust,
                    topics: vec!["general".parse().unwrap()],
                    related: None,
                    note: Some(format!("note {}", rng.below(1_000_000))),
                    author: curator.public(),
                    clock,
                    time: time(),
                    extra: Map::new(),
                };
                store.publish(&node, seal(&att, &curator).unwrap().bytes()).unwrap();
            }
            6 => {
                store.sign_head(&node).unwrap();
            }
            // Sync from another node: it publishes and signs, the node takes its head and entries.
            _ => {
                let clock = remote.author_clock(remote_node.public()).unwrap() + 1;
                let rev = seal(&revision(&remote_node, clock, shared.id, 12), &remote_node).unwrap();
                remote.publish(&remote_node, rev.bytes()).unwrap();
                let head = remote.sign_head(&remote_node).unwrap().unwrap();
                store.put_object(rev.bytes()).unwrap();
                store.put_object(head.bytes()).unwrap();
                let have = store.log_position(remote_node.public()).unwrap();
                let entries: Vec<Vec<u8>> = remote
                    .log_entries(remote_node.public(), have, usize::MAX)
                    .unwrap()
                    .into_iter()
                    .map(|(e, _)| e)
                    .collect();
                store.append_log_entries(remote_node.public(), &entries).unwrap();
            }
        }
    }
}

/// xorshift64*: random enough to vary kill times and sizes, without a dependency.
struct Rng(u64);

impl Rng {
    fn seeded() -> Rng {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
        Rng((nanos ^ u64::from(std::process::id()).rotate_left(32)) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len + 8);
        while out.len() < len {
            out.extend_from_slice(&self.next().to_le_bytes());
        }
        out.truncate(len);
        out
    }
}

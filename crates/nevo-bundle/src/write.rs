//! Writing a bundle. Two passes over the store: the first decides what goes in (the bundle object
//! must come first in the tar and lists every blob), the second streams it out. Only IDs are kept
//! in memory between the passes, and blobs are copied straight from their files.

use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use nevo_format::{
    open_any, seal, AnyObject, Bundle, BundleBlob, BundleLog, Id, Keypair, LogHead, PublicKey, Sealed, Time,
    TimeSource, Topic, Visibility, MAX_BUNDLE_LIST, MAX_OBJECT_BYTES,
};
use nevo_store::Store;

use crate::error::{BundleError, Result};
use crate::member;
use crate::{Problems, MAX_BUNDLE_ENTRIES, MAX_LOG_ENTRIES};

/// Log entries read from the store at a time.
const PAGE: usize = 1000;

/// Which blobs travel. Metadata always moves; blobs move by policy (spec section 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobFilter {
    pub include: bool,
    /// Only blobs of revisions in one of these topics (or their sub-topics). Empty means all.
    pub topics: Vec<Topic>,
    /// Only blobs up to this many bytes.
    pub max_size: Option<u64>,
}

impl BlobFilter {
    pub fn all() -> BlobFilter {
        BlobFilter {
            include: true,
            topics: Vec::new(),
            max_size: None,
        }
    }

    pub fn none() -> BlobFilter {
        BlobFilter {
            include: false,
            ..BlobFilter::all()
        }
    }

    fn wants_topics(&self, topics: &[Topic]) -> bool {
        self.include && (self.topics.is_empty() || topics.iter().any(|t| self.topics.iter().any(|f| f.covers(t))))
    }

    fn wants_size(&self, size: u64) -> bool {
        self.max_size.is_none_or(|max| size <= max)
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Only these nodes' logs. `None` means every log this store holds a signed head for.
    pub nodes: Option<Vec<PublicKey>>,
    /// How much of each log the receiver already has: only later entries are written.
    pub known: BTreeMap<PublicKey, u64>,
    pub blobs: BlobFilter,
    /// What the bundle is for, shown to whoever imports it.
    pub note: Option<String>,
    pub time: Time,
}

impl Default for ExportOptions {
    fn default() -> ExportOptions {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        ExportOptions {
            nodes: None,
            known: BTreeMap::new(),
            blobs: BlobFilter::all(),
            note: None,
            time: Time {
                ms,
                src: TimeSource::Unknown,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// ID of the bundle object.
    pub bundle: Id,
    pub logs: usize,
    pub entries: u64,
    pub objects: usize,
    pub blobs: usize,
    pub blob_bytes: u64,
    /// Objects listed in a log that this store doesn't have or that aren't public. They were
    /// left out.
    pub skipped_objects: usize,
    /// Blobs left out (missing, wrong size) or written but found damaged while copying (the
    /// receiver will reject those).
    pub problems: Problems,
}

/// What pass one decided.
struct Plan {
    logs: Vec<PlannedLog>,
    blobs: Vec<BundleBlob>,
}

struct PlannedLog {
    head: Sealed<LogHead>,
    from: u64,
    /// Objects first listed by this log, in log order.
    objects: Vec<Id>,
}

/// Writes a bundle file: to `<path>.part` first, renamed into place once complete and flushed,
/// so a stick pulled out early never holds a half-written bundle under the real name.
pub fn export_bundle_file(store: &Store, key: &Keypair, options: &ExportOptions, path: &Path) -> Result<ExportReport> {
    if path.exists() {
        return Err(BundleError::Target(format!("{} already exists", path.display())));
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = Path::new(&part);
    let result = (|| {
        let file = File::create(part)?;
        let mut out = BufWriter::new(file);
        let report = export_bundle(store, key, options, &mut out)?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        fs::rename(part, path)?;
        Ok(report)
    })();
    if result.is_err() {
        let _ = fs::remove_file(part);
    }
    result
}

/// Writes a bundle to `out`, signed by `key`.
///
/// If `key`'s own log has entries past its last signed head, a new head is signed first, so
/// those entries can travel too: receivers accept entries only up to a signed head.
pub fn export_bundle(store: &Store, key: &Keypair, options: &ExportOptions, out: impl Write) -> Result<ExportReport> {
    let own = key.public();
    let signed_to = store.held_signed_head(own)?.map_or(0, |h| h.object.seq);
    if store.log_position(own)? > signed_to {
        store.sign_head(key)?;
    }

    let mut report = ExportReport {
        bundle: Id::of(b""),
        logs: 0,
        entries: 0,
        objects: 0,
        blobs: 0,
        blob_bytes: 0,
        skipped_objects: 0,
        problems: Problems::default(),
    };
    let plan = plan(store, options, &mut report)?;

    let bundle = Bundle {
        by: own,
        clock: store.author_clock(own)? + 1,
        time: options.time.clone(),
        logs: plan
            .logs
            .iter()
            .map(|l| BundleLog {
                node: l.head.object.node,
                from: l.from,
                to: l.head.object.seq,
                head: l.head.id(),
            })
            .collect(),
        blobs: plan.blobs.clone(),
        note: options.note.clone(),
        extra: Default::default(),
    };
    let sealed = match seal(&bundle, key) {
        Ok(sealed) => sealed,
        Err(nevo_format::Error::TooLarge) => {
            return Err(BundleError::TooLarge(format!(
                "{} logs and {} blobs don't fit in one {} byte bundle object; choose fewer topics or a size limit",
                bundle.logs.len(),
                bundle.blobs.len(),
                MAX_OBJECT_BYTES
            )))
        }
        Err(e) => return Err(e.into()),
    };
    report.bundle = sealed.id();

    // Pass two: write everything in the order the plan fixed.
    let mtime = u64::try_from(options.time.ms / 1000).unwrap_or(0);
    let mut tar = tar::Builder::new(out);
    append(&mut tar, member::BUNDLE, sealed.bytes(), mtime)?;
    for log in &plan.logs {
        let node = log.head.object.node;
        append(&mut tar, &member::object(log.head.id()), log.head.bytes(), mtime)?;
        let mut seq = log.from - 1;
        while seq < log.head.object.seq {
            let want = usize::try_from(log.head.object.seq - seq)
                .unwrap_or(usize::MAX)
                .min(PAGE);
            let page = store.log_entries(node, seq, want)?;
            if page.is_empty() {
                return Err(BundleError::Store(nevo_store::StoreError::Log(format!(
                    "log of {node} ended at entry {seq} while being written"
                ))));
            }
            for (bytes, _) in &page {
                append(&mut tar, &member::object(Id::of(bytes)), bytes, mtime)?;
            }
            seq += page.len() as u64;
            report.entries += page.len() as u64;
        }
        for &id in &log.objects {
            match shareable(store, id)? {
                Some(bytes) => {
                    append(&mut tar, &member::object(id), &bytes, mtime)?;
                    report.objects += 1;
                }
                None => report.skipped_objects += 1,
            }
        }
        report.logs += 1;
    }
    for blob in &plan.blobs {
        let Some(file) = store.open_blob(blob.blob)? else {
            // Listed but gone since pass one: the receiver will report it missing.
            report
                .problems
                .add(format!("blob {} disappeared while writing", blob.blob));
            continue;
        };
        let mut reader = HashingReader {
            inner: file,
            left: blob.size,
            hasher: blake3::Hasher::new(),
        };
        let header = header(&member::blob(blob.blob), blob.size, mtime)?;
        tar.append(&header, &mut reader)?;
        if Id::from_digest(*reader.hasher.finalize().as_bytes()) != blob.blob {
            report.problems.add(format!(
                "blob {} is damaged in this library; the receiver will reject it",
                blob.blob
            ));
        }
        report.blobs += 1;
        report.blob_bytes += blob.size;
    }
    tar.into_inner()?.flush()?;
    Ok(report)
}

/// Pass one: which log ranges, objects and blobs go in.
fn plan(store: &Store, options: &ExportOptions, report: &mut ExportReport) -> Result<Plan> {
    let nodes = match &options.nodes {
        Some(nodes) => nodes.clone(),
        None => store.nodes_with_entries()?,
    };
    let mut logs = Vec::new();
    let mut total_entries = 0u64;
    let mut seen_objects = HashSet::new();
    // Blobs wanted by revisions, with the size the revision states; and derived results, which
    // travel with their source blob.
    let mut wanted: Vec<(Id, Option<u64>)> = Vec::new();
    let mut derived: Vec<(Id, Id)> = Vec::new();

    for node in nodes {
        let Some(head) = store.held_signed_head(node)? else {
            continue;
        };
        let from = options.known.get(&node).copied().unwrap_or(0).saturating_add(1);
        let to = head.object.seq;
        if from > to {
            continue;
        }
        let count = to - from + 1;
        total_entries += count;
        if count > MAX_LOG_ENTRIES || total_entries > MAX_BUNDLE_ENTRIES {
            return Err(BundleError::TooLarge(format!(
                "a bundle carries at most {MAX_LOG_ENTRIES} entries per log and {MAX_BUNDLE_ENTRIES} in all; \
                 write several bundles, each starting where the last one ended"
            )));
        }

        let mut objects = Vec::new();
        let mut seq = from - 1;
        while seq < to {
            let want = usize::try_from(to - seq).unwrap_or(usize::MAX).min(PAGE);
            let page = store.log_entries(node, seq, want)?;
            if page.is_empty() {
                break;
            }
            seq += page.len() as u64;
            for (_, obj) in page {
                if !seen_objects.insert(obj) {
                    continue;
                }
                let Some(bytes) = shareable(store, obj)? else {
                    report.skipped_objects += 1;
                    continue;
                };
                match open_any(&bytes)? {
                    AnyObject::Revision(r) if options.blobs.wants_topics(&r.object.topics) => {
                        wanted.extend(r.object.content.iter().map(|f| (f.blob, Some(f.size))));
                    }
                    AnyObject::Derived(d) => derived.push((d.object.from, d.object.out)),
                    _ => {}
                }
                objects.push(obj);
            }
        }
        if seq < to {
            // The store vouched for entries up to its signed head, so this means it changed
            // underneath us. Skip the log rather than write a range that can't be imported.
            report
                .problems
                .add(format!("log of {node} is shorter than its signed head"));
            continue;
        }
        logs.push(PlannedLog { head, from, objects });
    }

    let sources: HashSet<Id> = wanted.iter().map(|(id, _)| *id).collect();
    wanted.extend(
        derived
            .into_iter()
            .filter(|(from, _)| sources.contains(from))
            .map(|(_, out)| (out, None)),
    );

    let mut blobs = Vec::new();
    let mut seen_blobs = HashSet::new();
    for (id, stated) in wanted {
        if !seen_blobs.insert(id) {
            continue;
        }
        let size = match fs::metadata(store.blob_path(id)) {
            Ok(m) if m.is_file() => m.len(),
            // Blobs move by policy, so a library often lacks some. Not a problem to report.
            _ => continue,
        };
        if stated.is_some_and(|s| s != size) {
            report
                .problems
                .add(format!("blob {id} has the wrong size in this library; left out"));
            continue;
        }
        if options.blobs.wants_size(size) {
            blobs.push(BundleBlob { blob: id, size });
        }
    }
    if blobs.len() > MAX_BUNDLE_LIST {
        return Err(BundleError::TooLarge(format!(
            "{} blobs (at most {MAX_BUNDLE_LIST}); choose fewer topics or a size limit",
            blobs.len()
        )));
    }
    Ok(Plan { logs, blobs })
}

/// An object's bytes if it may leave this node. The store already refuses node-only and private
/// objects; revisions are checked again here, because a leak can't be undone.
fn shareable(store: &Store, id: Id) -> Result<Option<Vec<u8>>> {
    let Some(bytes) = store.shareable_object_bytes(id)? else {
        return Ok(None);
    };
    match open_any(&bytes)? {
        AnyObject::Revision(r) if r.object.vis != Visibility::Public => Ok(None),
        _ => Ok(Some(bytes)),
    }
}

fn header(path: &str, size: u64, mtime: u64) -> io::Result<tar::Header> {
    let mut h = tar::Header::new_ustar();
    h.set_path(path)?;
    h.set_size(size);
    h.set_mode(0o644);
    h.set_mtime(mtime);
    h.set_entry_type(tar::EntryType::Regular);
    h.set_cksum();
    Ok(h)
}

fn append<W: Write>(tar: &mut tar::Builder<W>, path: &str, bytes: &[u8], mtime: u64) -> io::Result<()> {
    tar.append(&header(path, bytes.len() as u64, mtime)?, bytes)
}

/// Yields exactly `left` bytes of a blob file and hashes them on the way. A tar header states
/// the size up front, so a file that shrank must fail the export rather than misalign the tar.
struct HashingReader<R> {
    inner: R,
    left: u64,
    hasher: blake3::Hasher,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Ok(0);
        }
        let max = usize::try_from(self.left).unwrap_or(usize::MAX).min(buf.len());
        let n = self.inner.read(&mut buf[..max])?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "a blob file shrank while being written",
            ));
        }
        self.hasher.update(&buf[..n]);
        self.left -= n as u64;
        Ok(n)
    }
}

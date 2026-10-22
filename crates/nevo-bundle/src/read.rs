//! Importing a bundle. The tar is read once, in order, without trusting anything in it: each
//! head, entry, object and blob is checked on its own and stored as soon as it is valid, so a
//! damaged or cut-off bundle still yields everything before the damage. Importing again is
//! harmless and fills in what was missing.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek};
use std::path::Path;

use nevo_format::{open, open_any, AnyObject, Bundle, Id, LogEntry, PublicKey, Sealed, MAX_OBJECT_BYTES};
use nevo_store::{Store, StoreError};

use crate::error::{BundleError, Result};
use crate::member::{self, Member};
use crate::{Problems, MAX_BUNDLE_ENTRIES, MAX_LOG_ENTRIES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// ID of the bundle object.
    pub bundle: Id,
    /// Who wrote the bundle. Says nothing about whether its content is trusted.
    pub by: PublicKey,
    pub note: Option<String>,
    /// New signed heads stored.
    pub heads: usize,
    /// Log entries added.
    pub entries: u64,
    /// New objects stored (not counting heads and entries).
    pub objects: usize,
    pub blobs: usize,
    pub blob_bytes: u64,
    /// Objects and blobs this library already had.
    pub already_had: usize,
    pub problems: Problems,
}

impl ImportReport {
    /// Whether everything the bundle lists was read and accepted.
    pub fn complete(&self) -> bool {
        self.problems.is_empty()
    }
}

pub fn import_bundle_file(store: &Store, path: &Path) -> Result<ImportReport> {
    import_bundle(store, BufReader::new(File::open(path)?))
}

/// Imports a bundle into `store`.
///
/// Fails only if the bundle object can't be read or the library can't be written (disk full, a
/// database error). Everything else (bad members, damage, a cut-off file) is skipped, listed
/// in the report's problems, and leaves what was valid in place.
pub fn import_bundle<R: Read + Seek>(store: &Store, reader: R) -> Result<ImportReport> {
    let mut archive = tar::Archive::new(reader);
    // Raw: GNU long names and pax headers are not interpreted, so their size fields can't make
    // the reader allocate. Bundles never use them; they show up as odd members.
    let mut members = archive.entries_with_seek()?.raw(true);

    let first = match members.next() {
        Some(Ok(entry)) => entry,
        Some(Err(e)) => return Err(BundleError::NotABundle(e.to_string())),
        None => return Err(BundleError::NotABundle("the file is empty".into())),
    };
    if member::parse(&first.path_bytes()) != Member::Bundle || !is_file(&first) {
        return Err(BundleError::NotABundle("bundle.cbor must come first".into()));
    }
    let bytes = read_small(first).map_err(|e| BundleError::NotABundle(format!("bundle.cbor: {e}")))?;
    let bundle = open::<Bundle>(&bytes).map_err(|e| BundleError::NotABundle(format!("bundle.cbor: {e}")))?;

    let mut import = Import::new(store, &bundle);
    let mut index = 1usize;
    for next in members {
        index += 1;
        let entry = match next {
            Ok(entry) => entry,
            Err(e) => {
                import
                    .problems
                    .add(format!("the bundle is damaged or cut off at member {index}: {e}"));
                break;
            }
        };
        import.member(entry)?;
    }
    Ok(import.finish(&bundle))
}

/// Progress through one log range of the bundle.
#[derive(Default)]
struct LogState {
    /// The head arrived and was accepted.
    head: Option<Id>,
    /// Entries waiting for the last one, which the head vouches for.
    entries: Vec<Sealed<LogEntry>>,
    done: bool,
    /// Already reported; the rest of this log is ignored.
    failed: bool,
}

struct Import<'a> {
    store: &'a Store,
    bundle: &'a Bundle,
    logs: Vec<LogState>,
    log_by_head: HashMap<Id, usize>,
    /// The log whose entries come next.
    current: Option<usize>,
    /// Objects listed by entries that chain to a signed head. Only these are imported.
    wanted: HashSet<Id>,
    received: HashSet<Id>,
    blob_sizes: HashMap<Id, u64>,
    blobs_seen: HashSet<Id>,
    report: ImportReport,
    problems: Problems,
}

impl<'a> Import<'a> {
    fn new(store: &'a Store, bundle: &'a Sealed<Bundle>) -> Import<'a> {
        let b = &bundle.object;
        let mut problems = Problems::default();
        let mut logs: Vec<LogState> = b.logs.iter().map(|_| LogState::default()).collect();
        let mut total = 0u64;
        for (log, state) in b.logs.iter().zip(&mut logs) {
            let count = log.to - log.from + 1;
            total = total.saturating_add(count);
            if count > MAX_LOG_ENTRIES || total > MAX_BUNDLE_ENTRIES {
                problems.add(format!(
                    "log of {} has more entries than one bundle may carry; skipped",
                    log.node
                ));
                state.failed = true;
            }
        }
        Import {
            store,
            bundle: b,
            logs,
            log_by_head: b.logs.iter().enumerate().map(|(i, l)| (l.head, i)).collect(),
            current: None,
            wanted: HashSet::new(),
            received: HashSet::new(),
            blob_sizes: b.blobs.iter().map(|x| (x.blob, x.size)).collect(),
            blobs_seen: HashSet::new(),
            report: ImportReport {
                bundle: bundle.id(),
                by: b.by,
                note: b.note.clone(),
                heads: 0,
                entries: 0,
                objects: 0,
                blobs: 0,
                blob_bytes: 0,
                already_had: 0,
                problems: Problems::default(),
            },
            problems,
        }
    }

    fn member<R: Read>(&mut self, entry: tar::Entry<'_, R>) -> Result<()> {
        let name = entry.path_bytes().into_owned();
        if entry.header().entry_type() == tar::EntryType::Directory {
            return Ok(());
        }
        let parsed = member::parse(&name);
        if parsed == Member::Odd || !is_file(&entry) {
            self.problems.add(format!("skipped unexpected member {}", shown(&name)));
            return Ok(());
        }
        match parsed {
            Member::Object(id) => match read_small(entry) {
                Ok(bytes) if Id::of(&bytes) == id => self.object(id, &bytes),
                Ok(_) => {
                    self.problems.add(format!("object {id} is damaged; skipped"));
                    Ok(())
                }
                Err(e) => {
                    self.problems.add(format!("object {id}: {e}"));
                    Ok(())
                }
            },
            Member::Blob(id) => self.blob(id, entry),
            // Version 1 verifies blobs whole against their ID (spec section 5).
            Member::Outboard(_) => Ok(()),
            Member::Bundle => {
                self.problems.add("skipped a second bundle.cbor");
                Ok(())
            }
            Member::Odd => unreachable!("handled above"),
        }
    }

    fn object(&mut self, id: Id, bytes: &[u8]) -> Result<()> {
        let object = match open_any(bytes) {
            Ok(object) => object,
            Err(e) => {
                self.problems.add(format!("object {id} is invalid: {e}"));
                return Ok(());
            }
        };
        match object {
            AnyObject::LogHead(head) => {
                let Some(&i) = self.log_by_head.get(&id) else {
                    self.problems
                        .add(format!("head {id} is not listed in bundle.cbor; skipped"));
                    return Ok(());
                };
                self.close_current();
                // Its entries come next, even if they are to be ignored.
                self.current = Some(i);
                let bundle = self.bundle;
                let log = &bundle.logs[i];
                if self.logs[i].failed {
                    return Ok(());
                }
                if head.object.node != log.node || head.object.seq != log.to {
                    return self.fail(i, "its head doesn't match bundle.cbor");
                }
                match self.store.put_object(bytes) {
                    Ok(put) => self.report.heads += usize::from(put.new),
                    Err(e @ (StoreError::Db(_) | StoreError::Io(_) | StoreError::SchemaVersion(_))) => {
                        return Err(e.into())
                    }
                    // Includes a head that contradicts what this library knows (a fork): the
                    // store keeps it as evidence, but its entries can't be trusted.
                    Err(e) => return self.fail(i, &e.to_string()),
                }
                self.logs[i].head = Some(head.object.entry);
            }
            AnyObject::LogEntry(entry) => {
                let Some(i) = self.current else {
                    self.problems
                        .add(format!("entry {id} doesn't follow a log head; skipped"));
                    return Ok(());
                };
                let state = &self.logs[i];
                if state.failed {
                    return Ok(());
                }
                let expected_seq = self.bundle.logs[i].from + state.entries.len() as u64;
                let chains = state
                    .entries
                    .last()
                    .is_none_or(|last| entry.object.prev == Some(last.id()));
                if entry.object.seq != expected_seq || !chains {
                    return self.fail(i, "entries are out of order");
                }
                let last = entry.object.seq == self.bundle.logs[i].to;
                self.logs[i].entries.push(entry);
                if last {
                    self.current = None;
                    self.finish_log(i)?;
                }
            }
            _ => {
                if !self.wanted.contains(&id) {
                    self.problems
                        .add(format!("object {id} is not listed by a log in the bundle; skipped"));
                    return Ok(());
                }
                self.received.insert(id);
                match self.store.put_object(bytes) {
                    Ok(put) if put.new => self.report.objects += 1,
                    Ok(_) => self.report.already_had += 1,
                    Err(e) => return self.store_problem(e, || format!("object {id}")),
                }
            }
        }
        Ok(())
    }

    /// All entries of log `i` arrived: check they chain to its signed head and add the ones
    /// this library doesn't have yet.
    fn finish_log(&mut self, i: usize) -> Result<()> {
        let bundle = self.bundle;
        let log = &bundle.logs[i];
        let entries = std::mem::take(&mut self.logs[i].entries);
        if entries.last().map(|e| e.id()) != self.logs[i].head {
            return self.fail(i, "its entries don't lead to its signed head");
        }
        // The hash chain ties every entry to the signed head, so the objects they list are
        // vouched for by the node even if its log can't be extended here yet.
        self.wanted.extend(entries.iter().map(|e| e.object.obj));
        self.logs[i].done = true;

        let have = self.store.log_position(log.node)?;
        if have >= log.to {
            return Ok(());
        }
        if have + 1 < log.from {
            self.problems.add(format!(
                "log of {} starts at entry {} but this library has only {have}; its objects were \
                 imported, its entries were not (import an earlier bundle first)",
                log.node, log.from
            ));
            return Ok(());
        }
        let new: Vec<Vec<u8>> = entries
            .iter()
            .filter(|e| e.object.seq > have)
            .map(|e| e.bytes().to_vec())
            .collect();
        match self.store.append_log_entries(log.node, &new) {
            Ok(_) => self.report.entries += new.len() as u64,
            Err(e) => return self.store_problem(e, || format!("log of {}", log.node)),
        }
        Ok(())
    }

    fn blob<R: Read>(&mut self, id: Id, entry: tar::Entry<'_, R>) -> Result<()> {
        let Some(&size) = self.blob_sizes.get(&id) else {
            self.problems
                .add(format!("blob {id} is not listed in bundle.cbor; skipped"));
            return Ok(());
        };
        if entry.size() != size {
            self.problems.add(format!("blob {id} has the wrong size; skipped"));
            return Ok(());
        }
        if !self.blobs_seen.insert(id) {
            return Ok(());
        }
        if self.store.has_blob(id) {
            self.report.already_had += 1;
            return Ok(());
        }
        let mut reader = Counting {
            inner: entry,
            read: 0,
            failed: false,
        };
        match self.store.put_blob_expecting(&mut reader, id) {
            Ok(info) => {
                self.report.blobs += usize::from(info.new);
                self.report.blob_bytes += info.size;
            }
            Err(StoreError::BlobMismatch { .. }) if reader.read < size => {
                self.problems.add(format!("blob {id} is cut off; skipped"));
            }
            Err(StoreError::Io(e)) if reader.failed => {
                self.problems
                    .add(format!("blob {id} can't be read from the bundle: {e}"));
            }
            Err(StoreError::BlobMismatch { .. }) => {
                self.problems.add(format!("blob {id} is damaged; rejected"));
            }
            Err(e) => return self.store_problem(e, || format!("blob {id}")),
        }
        Ok(())
    }

    /// A head arriving means the previous log's entries are over.
    fn close_current(&mut self) {
        if let Some(i) = self.current.take() {
            if !self.logs[i].done && !self.logs[i].failed {
                let _ = self.fail(i, "its entries stop before its signed head");
            }
        }
    }

    fn fail(&mut self, i: usize, why: &str) -> Result<()> {
        let state = &mut self.logs[i];
        if !state.failed {
            state.failed = true;
            state.entries = Vec::new();
            self.problems.add(format!(
                "log of {}: {why}; its entries were skipped",
                self.bundle.logs[i].node
            ));
        }
        Ok(())
    }

    /// Database and disk errors stop the import (retrying later resumes it); anything else is
    /// a problem with this one piece of the bundle.
    fn store_problem(&mut self, e: StoreError, what: impl FnOnce() -> String) -> Result<()> {
        match e {
            StoreError::Db(_) | StoreError::Io(_) | StoreError::SchemaVersion(_) => Err(e.into()),
            e => {
                self.problems.add(format!("{}: {e}", what()));
                Ok(())
            }
        }
    }

    fn finish(mut self, bundle: &Sealed<Bundle>) -> ImportReport {
        self.close_current();
        for (log, state) in bundle.object.logs.iter().zip(&self.logs) {
            if !state.done && !state.failed {
                self.problems
                    .add(format!("log of {} is missing from the bundle", log.node));
            }
        }
        let missing_objects = self
            .wanted
            .iter()
            .filter(|id| !self.received.contains(id))
            .filter(|&&id| !self.store.has_object(id).unwrap_or(false))
            .count();
        if missing_objects > 0 {
            self.problems.add(format!(
                "{missing_objects} objects listed by the logs are neither in the bundle nor in this library"
            ));
        }
        for blob in &bundle.object.blobs {
            if !self.blobs_seen.contains(&blob.blob) {
                self.problems
                    .add(format!("blob {} is missing from the bundle", blob.blob));
            }
        }
        self.report.problems = self.problems;
        self.report
    }
}

fn is_file<R: Read>(entry: &tar::Entry<'_, R>) -> bool {
    entry.header().entry_type() == tar::EntryType::Regular
}

/// Reads an object member, refusing anything larger than an object may be.
fn read_small<R: Read>(entry: tar::Entry<'_, R>) -> io::Result<Vec<u8>> {
    let size = entry.size();
    if size > MAX_OBJECT_BYTES as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "larger than any object may be",
        ));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    entry.take(size).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "cut off"));
    }
    Ok(bytes)
}

/// A member name for messages: printable, and short even if the name isn't.
fn shown(name: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(name).chars().take(120).collect();
    format!("{text:?}")
}

/// Counts bytes read and notes read errors, to tell a cut-off blob from a damaged one, and
/// either from a failure to write the library.
struct Counting<R> {
    inner: R,
    read: u64,
    failed: bool,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf).inspect_err(|_| self.failed = true)?;
        self.read += n as u64;
        Ok(n)
    }
}

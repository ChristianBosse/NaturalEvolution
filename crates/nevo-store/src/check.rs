//! The store's "fsck": checks that everything on disk is what it claims to be, and can repair
//! what is safe to repair.
//!
//! What is checked:
//! - SQLite's own `integrity_check` (damaged database pages).
//! - Every object: its bytes decode in canonical form, validate, carry a good signature, and hash
//!   to the ID and type they are stored under.
//! - Index tables: they are rebuilt from the objects into a scratch database and compared row by
//!   row with the real ones.
//! - Logs: every log starts at 1, has no gaps and is hash-chained; every signed head matches its
//!   object and the entries; fork evidence is still there.
//! - Blobs: every file sits under its own ID. Re-hashing them all is optional because it reads
//!   all content.
//! - Leftover temporary files. These are warnings, not errors: an interrupted write leaves them,
//!   and they never sit under a real ID.
//!
//! Repair only does what loses nothing. Index tables are derived from `objects`, so they are
//! rebuilt from it. Corrupt objects and blobs are moved into `quarantine/` with a report, never
//! deleted. Logs can't be derived from anything, so a broken log is reported but left alone.
//!
//! Memory use stays flat however big the store is: objects and log entries are read a batch at a
//! time, and the rebuilt index lives in a scratch database file under `tmp/`, not in memory.

use std::fmt;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use nevo_format::{open, open_any, AnyObject, Id, LogEntry, LogHead, PublicKey};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};

use crate::error::Result;
use crate::{head_conflict, id_from_db, index, object_id, type_name, Store};

/// Tables that `index()` writes, which can all be rebuilt from `objects`. Keep this in sync with
/// `index()`: a table missing here makes the check fail loudly (the scratch database reads the
/// real store read-only), it can't damage anything.
///
/// `log_entries` and `log_forks` are not here: which node an entry belongs to, and which heads
/// were seen in which order, are not recorded in the objects themselves.
const INDEX_TABLES: &[&str] = &[
    "revisions",
    "revision_parents",
    "revision_topics",
    "revision_files",
    "attestations",
    "keys",
    "revokes",
    "derived",
    "log_heads",
];

/// Rows read per query. Objects are fetched one by one within a batch, so even 1 MB objects keep
/// memory small.
const BATCH: i64 = 1024;

/// Problems listed in a report; the counts include the rest.
const MAX_LISTED: usize = 1000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CheckOptions {
    /// Re-hash every blob file. Slow: it reads all content.
    pub blobs: bool,
    /// Rebuild index tables if they are wrong, and move corrupt objects and blobs into
    /// `quarantine/`. Nothing is ever deleted.
    pub repair: bool,
}

/// Something wrong (or, for warnings, merely odd) found by [`Store::check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// SQLite found damaged pages.
    Database(String),
    /// An object row that doesn't hold a valid object under its ID.
    CorruptObject { rowid: i64, id: Option<Id>, why: String },
    /// An index table doesn't match the objects: rows it lacks, and rows it shouldn't have.
    Index {
        table: &'static str,
        missing: u64,
        extra: u64,
    },
    /// A log has a gap, a broken hash chain or a bad entry.
    Log { node: String, seq: u64, why: String },
    /// A signed head doesn't match its object or the log.
    Head { node: String, seq: u64, why: String },
    /// A blob file whose content doesn't hash to its name.
    CorruptBlob { id: Id, actual: Id },
    /// A check step couldn't run.
    Failed { step: &'static str, why: String },
    /// Warning: a leftover from an interrupted write.
    TmpFile(PathBuf),
    /// Warning: a file under `blobs/` that isn't named after an ID or is in the wrong folder.
    UnexpectedFile(PathBuf),
}

impl Problem {
    fn is_warning(&self) -> bool {
        matches!(self, Problem::TmpFile(_) | Problem::UnexpectedFile(_))
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Database(line) => write!(f, "database: {line}"),
            Problem::CorruptObject {
                rowid,
                id: Some(id),
                why,
            } => write!(f, "object {id} (row {rowid}): {why}"),
            Problem::CorruptObject { rowid, id: None, why } => write!(f, "object in row {rowid}: {why}"),
            Problem::Index { table, missing, extra } => {
                write!(f, "index table {table}: {missing} rows missing, {extra} rows extra")
            }
            Problem::Log { node, seq, why } => write!(f, "log of {node}, entry {seq}: {why}"),
            Problem::Head { node, seq, why } => write!(f, "signed head of {node} at {seq}: {why}"),
            Problem::CorruptBlob { id, actual } => write!(f, "blob {id}: content hashes to {actual}"),
            Problem::Failed { step, why } => write!(f, "{step}: check could not run: {why}"),
            Problem::TmpFile(path) => write!(f, "leftover temporary file {}", path.display()),
            Problem::UnexpectedFile(path) => write!(f, "unexpected file {}", path.display()),
        }
    }
}

/// What [`Store::check`] found, and what it repaired.
#[derive(Debug, Clone, Default)]
pub struct CheckReport {
    pub objects: u64,
    pub log_entries: u64,
    pub log_heads: u64,
    pub blob_files: u64,
    pub blobs_hashed: u64,
    /// Errors, at most 1000 of them; `error_count` counts all.
    pub errors: Vec<Problem>,
    pub error_count: u64,
    /// Things that are harmless but worth knowing, such as leftover temporary files.
    pub warnings: Vec<Problem>,
    pub warning_count: u64,
    /// What repair did, one line per action.
    pub repairs: Vec<String>,
    /// The report of everything moved into `quarantine/`, if anything was.
    pub quarantine_report: Option<PathBuf>,
}

impl CheckReport {
    /// True if no errors were found. Warnings don't count.
    pub fn is_ok(&self) -> bool {
        self.error_count == 0
    }

    fn add(&mut self, problem: Problem) {
        let (list, count) = if problem.is_warning() {
            (&mut self.warnings, &mut self.warning_count)
        } else {
            (&mut self.errors, &mut self.error_count)
        };
        *count += 1;
        if list.len() < MAX_LISTED {
            list.push(problem);
        }
    }

    fn repaired(&mut self, line: String) {
        if self.repairs.len() < MAX_LISTED {
            self.repairs.push(line);
        }
    }

    /// Records a step that failed outright.
    fn step(&mut self, step: &'static str, result: Result<()>) {
        if let Err(e) = result {
            self.add(Problem::Failed {
                step,
                why: e.to_string(),
            });
        }
    }
}

impl fmt::Display for CheckReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} objects, {} log entries, {} signed heads, {} blob files ({} re-hashed)",
            self.objects, self.log_entries, self.log_heads, self.blob_files, self.blobs_hashed
        )?;
        for (kind, list, count) in [
            ("error", &self.errors, self.error_count),
            ("warning", &self.warnings, self.warning_count),
        ] {
            for problem in list {
                writeln!(f, "{kind}: {problem}")?;
            }
            if count > list.len() as u64 {
                writeln!(f, "... and {} more {kind}s", count - list.len() as u64)?;
            }
        }
        for line in &self.repairs {
            writeln!(f, "repaired: {line}")?;
        }
        if let Some(path) = &self.quarantine_report {
            writeln!(f, "quarantine report: {}", path.display())?;
        }
        match self.error_count {
            0 => write!(f, "OK"),
            n => write!(f, "{n} errors"),
        }
    }
}

/// What repair needs to know from the check.
#[derive(Default)]
struct Found {
    db_damaged: bool,
    corrupt_objects: u64,
    index_wrong: bool,
}

impl Store {
    /// Checks the whole store and, with `options.repair`, fixes what can be fixed safely.
    ///
    /// The check reads the database through its own read-only connection, inside one read
    /// transaction, so it sees a consistent snapshot and never blocks other readers or writers.
    /// Repair takes the write lock while it works.
    pub fn check(&self, options: CheckOptions) -> CheckReport {
        let mut report = CheckReport::default();
        let mut found = Found::default();
        match Scratch::create(&self.root) {
            Ok(mut scratch) => {
                let conn = scratch.conn.as_mut().expect("scratch connection is open");
                let result = check_database(conn, &mut report, &mut found);
                report.step("database", result);
            }
            Err(e) => report.step("open", Err(e)),
        }

        let mut quarantine = Quarantine::new(&self.root);
        let result = self.check_blobs(options, &mut quarantine, &mut report);
        report.step("blobs", result);
        let result = self.check_tmp(&mut report);
        report.step("tmp", result);

        if options.repair {
            let result = self.repair(&found, &mut quarantine, &mut report);
            report.step("repair", result);
        }
        let result = quarantine.finish().map(|path| report.quarantine_report = path);
        report.step("quarantine", result);
        report
    }

    fn check_blobs(&self, options: CheckOptions, quarantine: &mut Quarantine, report: &mut CheckReport) -> Result<()> {
        let blobs = self.root.join("blobs");
        if !blobs.is_dir() {
            return Ok(());
        }
        // blobs/xx/yy/<id>: read one folder at a time, so memory doesn't grow with the store.
        for a in fs::read_dir(&blobs)? {
            let a = a?.path();
            if !a.is_dir() {
                report.add(Problem::UnexpectedFile(a));
                continue;
            }
            for b in fs::read_dir(&a)? {
                let b = b?.path();
                if !b.is_dir() {
                    report.add(Problem::UnexpectedFile(b));
                    continue;
                }
                for file in fs::read_dir(&b)? {
                    self.check_blob_file(&file?.path(), options, quarantine, report)?;
                }
            }
        }
        Ok(())
    }

    fn check_blob_file(
        &self,
        path: &Path,
        options: CheckOptions,
        quarantine: &mut Quarantine,
        report: &mut CheckReport,
    ) -> Result<()> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Outboards for large blobs (spec section 5) live next to them; they get their own check.
        if name.ends_with(".obao") {
            return Ok(());
        }
        let id = match name.parse::<Id>() {
            Ok(id) if self.blob_path(id) == path && path.is_file() => id,
            _ => {
                report.add(Problem::UnexpectedFile(path.to_path_buf()));
                return Ok(());
            }
        };
        report.blob_files += 1;
        if !options.blobs {
            return Ok(());
        }
        let actual = match File::open(path).and_then(Id::of_reader) {
            Ok(actual) => actual,
            // A file that can't be read is as bad as a corrupt one, but there is nothing to move.
            Err(e) => {
                report.add(Problem::Failed {
                    step: "blobs",
                    why: format!("{}: {e}", path.display()),
                });
                return Ok(());
            }
        };
        report.blobs_hashed += 1;
        if actual != id {
            report.add(Problem::CorruptBlob { id, actual });
            if options.repair {
                let to = quarantine.blob(path, id, actual)?;
                report.repaired(format!("moved corrupt blob {id} to {}", to.display()));
            }
        }
        Ok(())
    }

    fn check_tmp(&self, report: &mut CheckReport) -> Result<()> {
        let tmp = self.root.join("tmp");
        if !tmp.is_dir() {
            return Ok(());
        }
        for entry in fs::read_dir(&tmp)? {
            report.add(Problem::TmpFile(entry?.path()));
        }
        Ok(())
    }

    /// Quarantines corrupt objects and rebuilds the index tables, in one transaction. Runs only
    /// if the check found something, and re-tests every object itself instead of trusting the
    /// check's snapshot, since writes may have happened in between.
    fn repair(&self, found: &Found, quarantine: &mut Quarantine, report: &mut CheckReport) -> Result<()> {
        if !found.db_damaged && found.corrupt_objects == 0 && !found.index_wrong {
            return Ok(());
        }
        let mut db = self.db();
        if found.db_damaged {
            // SQLite's own indexes are derived from its tables, so rebuilding them is safe. That
            // fixes indexes whose entries disagree with their table. It can't fix unreadable
            // pages (REINDEX fails then and changes nothing); those need a backup or a re-sync.
            match db.execute_batch("REINDEX") {
                Ok(()) => report.repaired("rebuilt SQLite's indexes (REINDEX)".into()),
                Err(e) => report.step("repair", Err(e.into())),
            }
        }
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut moved = 0u64;
        if found.corrupt_objects > 0 {
            for_each_object(&tx, "main", |row| {
                if let Err(why) = verify_row(&row) {
                    let to = quarantine.object(&row, &why)?;
                    // The copy is on disk (synced) before the row goes, and the row only goes
                    // when the transaction commits.
                    tx.execute("DELETE FROM objects WHERE rowid = ?1", [row.rowid])?;
                    report.repaired(format!("moved corrupt object in row {} to {}", row.rowid, to.display()));
                    moved += 1;
                }
                Ok(())
            })?;
        }
        if found.index_wrong || moved > 0 {
            for table in INDEX_TABLES {
                tx.execute(&format!("DELETE FROM {table}"), [])?;
            }
            let mut count = 0u64;
            for_each_object(&tx, "main", |row| {
                if let Ok(object) = verify_row(&row) {
                    reindex(&tx, &object)?;
                    count += 1;
                }
                Ok(())
            })?;
            report.repaired(format!("rebuilt the index tables from {count} objects"));
        }
        quarantine.sync()?;
        tx.commit()?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Database checks

/// Integrity, then objects, indexes and logs in one read snapshot of the store.
fn check_database(conn: &mut Connection, report: &mut CheckReport, found: &mut Found) -> Result<()> {
    // The steps go on after a failed one: damage in one table says nothing about the others.
    // The integrity check runs on its own, because a damaged page can end the transaction it
    // runs in.
    let result = check_integrity(conn, report, found);
    if result.is_err() {
        found.db_damaged = true;
    }
    report.step("database", result);
    let tx = conn.transaction()?;
    let result = check_objects(&tx, report, found);
    report.step("objects", result);
    let result = check_logs(&tx, report);
    report.step("logs", result);
    // Ends the read snapshot. Only the scratch database was written.
    tx.commit()?;
    Ok(())
}

fn check_integrity(conn: &Connection, report: &mut CheckReport, found: &mut Found) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA store.integrity_check")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let line: String = row.get(0)?;
        if line != "ok" {
            found.db_damaged = true;
            report.add(Problem::Database(line));
        }
    }
    Ok(())
}

/// Opens every object and rebuilds the index from them into the scratch tables, then compares.
fn check_objects(tx: &Transaction, report: &mut CheckReport, found: &mut Found) -> Result<()> {
    for_each_object(tx, "store", |row| {
        report.objects += 1;
        match verify_row(&row) {
            Ok(object) => reindex(tx, &object)?,
            Err(why) => {
                found.corrupt_objects += 1;
                report.add(Problem::CorruptObject {
                    rowid: row.rowid,
                    id: row.id.as_deref().and_then(id_from_db),
                    why,
                });
            }
        }
        Ok(())
    })?;
    for table in INDEX_TABLES {
        let count = |sql: String| -> Result<u64> { Ok(tx.query_row(&sql, [], |r| r.get::<_, i64>(0))? as u64) };
        // Set differences find changed rows; the row counts also catch duplicated ones.
        let mut missing = count(format!(
            "SELECT COUNT(*) FROM (SELECT * FROM main.{table} EXCEPT SELECT * FROM store.{table})"
        ))?;
        let mut extra = count(format!(
            "SELECT COUNT(*) FROM (SELECT * FROM store.{table} EXCEPT SELECT * FROM main.{table})"
        ))?;
        let rebuilt = count(format!("SELECT COUNT(*) FROM main.{table}"))?;
        let stored = count(format!("SELECT COUNT(*) FROM store.{table}"))?;
        missing = missing.max(rebuilt.saturating_sub(stored));
        extra = extra.max(stored.saturating_sub(rebuilt));
        if missing > 0 || extra > 0 {
            found.index_wrong = true;
            report.add(Problem::Index { table, missing, extra });
        }
    }
    Ok(())
}

/// Indexes an object the way `insert_object` does when it is first stored: a signed head that
/// contradicts an earlier head or an accepted entry is kept as fork evidence but not indexed.
/// Objects are replayed in the order they were stored (rowid order), so "earlier" matches.
fn reindex(tx: &Transaction, object: &AnyObject) -> Result<()> {
    if let AnyObject::LogHead(head) = object {
        if head_conflict(tx, &head.object)?.is_some() {
            return Ok(());
        }
    }
    index(tx, object)
}

fn check_logs(tx: &Transaction, report: &mut CheckReport) -> Result<()> {
    // Walk the nodes one at a time with the primary key, instead of loading the list of nodes.
    let mut node: Vec<u8> = Vec::new();
    while let Some(next) = tx.query_row(
        "SELECT MIN(node) FROM store.log_entries WHERE node > ?1",
        [&node],
        |r| r.get::<_, Option<Vec<u8>>>(0),
    )? {
        check_log(tx, &next, report)?;
        node = next;
    }
    check_heads(tx, report)?;

    let mut stmt = tx.prepare(
        "SELECT node, seq FROM store.log_forks f
         WHERE NOT EXISTS (SELECT 1 FROM store.objects WHERE id = f.first)
            OR NOT EXISTS (SELECT 1 FROM store.objects WHERE id = f.second)",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        report.add(Problem::Log {
            node: node_name(&row.get::<_, Vec<u8>>(0)?),
            seq: row.get::<_, i64>(1)? as u64,
            why: "the evidence of a fork is missing".into(),
        });
    }
    Ok(())
}

/// One node's log: entries 1, 2, 3, … each naming the one before it by hash.
fn check_log(tx: &Transaction, node: &[u8], report: &mut CheckReport) -> Result<()> {
    let name = node_name(node);
    let mut expected = 1u64;
    let mut prev: Option<Id> = None;
    let mut after = 0i64;
    let mut stmt = tx.prepare(
        "SELECT l.seq, l.entry, l.obj, o.bytes FROM store.log_entries l
         LEFT JOIN store.objects o ON o.id = l.entry
         WHERE l.node = ?1 AND l.seq > ?2 ORDER BY l.seq LIMIT ?3",
    )?;
    loop {
        let batch = stmt
            .query_map(params![node, after, BATCH], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if batch.is_empty() {
            return Ok(());
        }
        for (seq, entry, obj, bytes) in batch {
            after = seq;
            report.log_entries += 1;
            let seq = seq as u64;
            let mut problem = |why: String| {
                report.add(Problem::Log {
                    node: name.clone(),
                    seq,
                    why,
                })
            };
            let gap = seq != expected;
            if gap {
                problem(format!("expected entry {expected} here: the log has a gap"));
            }
            match bytes.as_deref().map(open::<LogEntry>) {
                None => problem("the entry object is missing".into()),
                Some(Err(e)) => problem(format!("the entry object is invalid: {e}")),
                Some(Ok(e)) => {
                    if e.object.seq != seq {
                        problem(format!("the entry object says it is entry {}", e.object.seq));
                    }
                    // After a gap the chain is already known to be broken; don't report it twice.
                    if !gap && e.object.prev != prev {
                        problem("doesn't chain to the entry before it".into());
                    }
                    if Some(e.object.obj) != id_from_db(&obj) {
                        problem("the entry object lists a different object than its index row".into());
                    }
                }
            }
            prev = id_from_db(&entry);
            expected = seq + 1;
        }
    }
}

/// Every indexed signed head: it must be a valid head saying what its row says, and agree with
/// the stored entry at its position.
fn check_heads(tx: &Transaction, report: &mut CheckReport) -> Result<()> {
    let mut stmt = tx.prepare(
        "SELECT h.node, h.seq, h.entry, o.bytes, l.entry FROM store.log_heads h
         LEFT JOIN store.objects o ON o.id = h.head
         LEFT JOIN store.log_entries l ON l.node = h.node AND l.seq = h.seq
         WHERE (h.node, h.seq) > (?1, ?2) ORDER BY h.node, h.seq LIMIT ?3",
    )?;
    let (mut node, mut after) = (Vec::<u8>::new(), -1i64);
    loop {
        let batch = stmt
            .query_map(params![node, after, BATCH], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, Option<Vec<u8>>>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if batch.is_empty() {
            return Ok(());
        }
        for (row_node, seq, entry, bytes, stored_entry) in batch {
            report.log_heads += 1;
            let mut problem = |why: String| {
                report.add(Problem::Head {
                    node: node_name(&row_node),
                    seq: seq as u64,
                    why,
                })
            };
            match bytes.as_deref().map(open::<LogHead>) {
                None => problem("the head object is missing".into()),
                Some(Err(e)) => problem(format!("the head object is invalid: {e}")),
                Some(Ok(h)) => {
                    let h = &h.object;
                    if h.node.as_bytes().as_slice() != row_node.as_slice()
                        || h.seq != seq as u64
                        || h.entry.digest().as_slice() != entry.as_slice()
                    {
                        problem("the head object doesn't match its index row".into());
                    }
                }
            }
            if stored_entry.is_some_and(|e| e != entry) {
                problem("contradicts the stored log entry at its position".into());
            }
            (node, after) = (row_node, seq);
        }
    }
}

fn node_name(node: &[u8]) -> String {
    match PublicKey::from_bytes(node) {
        Ok(key) => key.to_string(),
        Err(_) => format!(
            "bad key {}",
            node.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Object rows

/// An `objects` row read without assuming its columns hold the right types, since damage can
/// change those too.
struct Row {
    rowid: i64,
    id: Option<Vec<u8>>,
    t: Option<String>,
    bytes: Option<Vec<u8>>,
}

/// Calls `f` for every object in rowid order (the order they were stored). Rowids are read a
/// batch at a time and each row is fetched alone, so `f` may delete the row it was given.
fn for_each_object(conn: &Connection, schema: &str, mut f: impl FnMut(Row) -> Result<()>) -> Result<()> {
    let mut ids = conn.prepare(&format!(
        "SELECT rowid FROM {schema}.objects WHERE rowid > ?1 ORDER BY rowid LIMIT ?2"
    ))?;
    let mut get = conn.prepare(&format!(
        "SELECT id, type, bytes FROM {schema}.objects WHERE rowid = ?1"
    ))?;
    let mut after = i64::MIN;
    loop {
        let batch = ids
            .query_map([after, BATCH], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let Some(&last) = batch.last() else {
            return Ok(());
        };
        for rowid in batch {
            let row = get
                .query_row([rowid], |r| {
                    Ok(Row {
                        rowid,
                        id: r.get_ref(0)?.as_blob().ok().map(<[u8]>::to_vec),
                        t: r.get_ref(1)?.as_str().ok().map(String::from),
                        bytes: r.get_ref(2)?.as_blob().ok().map(<[u8]>::to_vec),
                    })
                })
                .optional()?;
            if let Some(row) = row {
                f(row)?;
            }
        }
        after = last;
    }
}

/// Re-opens a stored object: canonical encoding, field rules and signature (by `open_any`), and
/// that it is stored under its own ID and type.
fn verify_row(row: &Row) -> std::result::Result<AnyObject, String> {
    let bytes = row.bytes.as_deref().ok_or("the stored bytes are not a blob")?;
    let object = open_any(bytes).map_err(|e| e.to_string())?;
    let id = object_id(&object);
    if row.id.as_deref() != Some(id.digest().as_slice()) {
        return Err(format!("stored under the wrong ID; the bytes hash to {id}"));
    }
    let t = type_name(&object);
    if row.t.as_deref() != Some(t) {
        return Err(format!(
            "stored as type {:?} but is {t:?}",
            row.t.as_deref().unwrap_or("")
        ));
    }
    Ok(object)
}

// ---------------------------------------------------------------------------------------------
// Scratch database

/// A throwaway database under `tmp/` holding the rebuilt index, with the real store attached
/// read-only as `store`. SQLite looks up unqualified table names in `main` first, so the
/// store's own `index()` code writes into the scratch tables here, unchanged, while its reads of
/// tables that aren't rebuilt (`log_entries`) fall through to the real store.
struct Scratch {
    /// Always `Some` until dropped.
    conn: Option<Connection>,
    path: PathBuf,
}

impl Scratch {
    fn create(root: &Path) -> Result<Scratch> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let tmp = root.join("tmp");
        fs::create_dir_all(&tmp)?;
        // The `.part` ending lets `Store::open` clean it up if a crash leaves it behind.
        let path = tmp.join(format!(
            "{}-check-{}.part",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_file(&path);
        let mut scratch = Scratch { conn: None, path };
        let conn = scratch.conn.insert(Connection::open(&scratch.path)?);
        // Nothing here needs to survive a crash.
        conn.query_row("PRAGMA journal_mode = OFF", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = OFF; PRAGMA temp_store = FILE;")?;
        conn.execute(
            "ATTACH DATABASE ?1 AS store",
            [read_only_uri(&root.join("objects.sqlite"))?],
        )?;
        for table in INDEX_TABLES {
            // The real table's own definition, so the copy matches it column for column.
            let sql: String = conn.query_row(
                "SELECT sql FROM store.sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |r| r.get(0),
            )?;
            conn.execute_batch(&sql)?;
        }
        Ok(scratch)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Close first: Windows can't delete an open file.
        drop(self.conn.take());
        let _ = fs::remove_file(&self.path);
    }
}

/// A SQLite URI that opens `path` read-only, so the check can never change the store.
fn read_only_uri(path: &Path) -> Result<String> {
    let path = std::path::absolute(path)?;
    let mut text = path.to_string_lossy().replace('\\', "/");
    // Windows paths (C:/...) need a leading slash too: file:///C:/...
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    let mut uri = String::from("file://");
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~:".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri.push_str("?mode=ro");
    Ok(uri)
}

// ---------------------------------------------------------------------------------------------
// Quarantine

/// Where repair moves what it can't trust: `quarantine/objects/`, `quarantine/blobs/` and a
/// report file per run. Nothing is deleted, so a wrong verdict can always be undone by hand.
struct Quarantine {
    dir: PathBuf,
    report: Option<(PathBuf, File)>,
}

impl Quarantine {
    fn new(root: &Path) -> Quarantine {
        Quarantine {
            dir: root.join("quarantine"),
            report: None,
        }
    }

    fn object(&mut self, row: &Row, why: &str) -> Result<PathBuf> {
        let name = match row.id.as_deref().and_then(id_from_db) {
            Some(id) => id.to_string(),
            None => format!("row-{}", row.rowid),
        };
        let dir = self.dir.join("objects");
        fs::create_dir_all(&dir)?;
        let to = unused_path(&dir, &name);
        let mut file = File::create(&to)?;
        file.write_all(row.bytes.as_deref().unwrap_or_default())?;
        file.sync_all()?;
        sync_dir(&dir)?;
        self.note(&format!(
            "object {name} (row {}, type {:?}): {why} -> {}",
            row.rowid,
            row.t.as_deref().unwrap_or(""),
            to.display()
        ))?;
        Ok(to)
    }

    fn blob(&mut self, path: &Path, id: Id, actual: Id) -> Result<PathBuf> {
        let dir = self.dir.join("blobs");
        fs::create_dir_all(&dir)?;
        let to = unused_path(&dir, &id.to_string());
        fs::rename(path, &to)?;
        sync_dir(&dir)?;
        if let Some(from) = path.parent() {
            sync_dir(from)?;
        }
        self.note(&format!("blob {id}: content hashes to {actual} -> {}", to.display()))?;
        Ok(to)
    }

    fn note(&mut self, line: &str) -> Result<()> {
        if self.report.is_none() {
            fs::create_dir_all(&self.dir)?;
            let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let path = unused_path(&self.dir, &format!("report-{secs}.txt"));
            let mut file = File::create(&path)?;
            writeln!(
                file,
                "NEVO store repair, unix time {secs}. Moved here instead of deleted:"
            )?;
            self.report = Some((path, file));
        }
        let (_, file) = self.report.as_mut().expect("report file was just opened");
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        if let Some((_, file)) = &self.report {
            file.sync_all()?;
            sync_dir(&self.dir)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Option<PathBuf>> {
        self.sync()?;
        Ok(self.report.map(|(path, _)| path))
    }
}

/// `dir/name`, or `dir/name.1`, `dir/name.2`, … if taken: quarantine never overwrites.
fn unused_path(dir: &Path, name: &str) -> PathBuf {
    let mut path = dir.join(name);
    let mut n = 0;
    while path.exists() {
        n += 1;
        path = dir.join(format!("{name}.{n}"));
    }
    path
}

/// Makes renames and new files in `dir` durable. Windows has no directory sync; NTFS journals
/// the change itself.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<()> {
    Ok(())
}

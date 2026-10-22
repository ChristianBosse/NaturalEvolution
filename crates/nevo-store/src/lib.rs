//! NEVO node storage.
//!
//! - **Objects** (revisions, attestations, keys, …) are kept in SQLite exactly as received, plus
//!   index tables for lookups. Every object is validated and its signature checked on the way in.
//! - **Blobs** are files under `blobs/`, verified against their ID while being written.
//! - **Logs** are each node's append-only list of published objects, the basis of sync.
//!
//! Layout of a store folder:
//!
//! ```text
//! objects.sqlite   objects and indexes
//! blobs/xx/yy/<id> blob files
//! tmp/             unfinished blob writes
//! ```
//!
//! ## How logs stay honest
//!
//! Log entries are unsigned, so entries received from another node are only accepted when they
//! chain (by hash) to a head that node signed: every batch must end exactly at a stored signed
//! head. All signed heads are kept, and two that contradict each other are recorded as proof that
//! the node forked its log ([`Store::is_forked`]).
//!
//! ## Housekeeping
//!
//! [`Store::collect_garbage`] deletes blob files that nothing uses (from abandoned uploads).
//! Blob writes stop with [`StoreError::DiskFull`] when free space falls below
//! [`Store::reserve`], so a full disk can't break the database.

mod blobs;
mod error;
mod gc;
mod schema;
mod space;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, MutexGuard, RwLock};

use nevo_format::{
    open, open_any, seal_unsigned, AnyObject, Attestation, Derived, Id, KeyRecord, Keypair, LogEntry, LogHead, Object,
    PublicKey, Revision, Revoke, Sealed, Subject, Topic, Visibility,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

pub use blobs::BlobInfo;
pub use error::{Result, StoreError};
pub use gc::{GcReport, GC_GRACE};
pub use space::MIN_RESERVE;

/// A node's storage folder.
pub struct Store {
    root: PathBuf,
    db: Mutex<Connection>,
    /// Bytes that blob writes leave free on the disk.
    reserve: AtomicU64,
    /// Shared while a blob file is put in place, exclusive while garbage collection deletes one.
    blob_lock: RwLock<()>,
}

/// The result of storing an object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Put {
    pub id: Id,
    /// False if the store already had it.
    pub new: bool,
}

impl Store {
    /// Opens the store in `root`, creating it if needed.
    pub fn open(root: impl AsRef<Path>) -> Result<Store> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join("blobs"))?;
        blobs::clean_tmp(&root)?;
        let mut conn = Connection::open(root.join("objects.sqlite"))?;
        schema::init(&mut conn)?;
        Ok(Store {
            reserve: AtomicU64::new(space::default_reserve(&root)),
            root,
            db: Mutex::new(conn),
            blob_lock: RwLock::new(()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn db(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock can't leave SQLite half-written: open transactions
        // roll back when dropped. So a poisoned lock is safe to keep using.
        self.db.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    // -----------------------------------------------------------------------------------------
    // Objects

    /// Validates, verifies and stores an object. Storing the same bytes twice is harmless.
    ///
    /// A signed log head that contradicts what is already known about that node's log is kept
    /// as evidence, recorded as a fork, and reported as [`StoreError::Forked`].
    pub fn put_object(&self, bytes: &[u8]) -> Result<Put> {
        // No free-space check: objects are a few KB, and the reserve that blob writes leave free
        // (see `space.rs`) is what keeps room for them. Refusing them would also stop a node from
        // recording that its own upload finished, after the files were already stored.
        let object = open_any(bytes)?;
        let mut db = self.db();
        let tx = db.transaction()?;
        let outcome = insert_object(&tx, &object, bytes)?;
        tx.commit()?;
        match outcome {
            Inserted::Stored(put) => Ok(put),
            Inserted::Fork { node, seq } => Err(StoreError::Forked { node, seq }),
        }
    }

    pub fn has_object(&self, id: Id) -> Result<bool> {
        let db = self.db();
        Ok(db
            .query_row("SELECT 1 FROM objects WHERE id = ?1", [id.digest()], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// An object's bytes, exactly as stored, including node-only and private ones. Code that
    /// serves other nodes must use [`Store::shareable_object_bytes`] instead.
    pub fn object_bytes(&self, id: Id) -> Result<Option<Vec<u8>>> {
        let db = self.db();
        Ok(db
            .query_row("SELECT bytes FROM objects WHERE id = ?1", [id.digest()], |r| r.get(0))
            .optional()?)
    }

    /// An object's bytes if it may be sent to other nodes: it was published (it is listed in a
    /// log), or it is a log entry or log head itself. Never returns node-only or private content.
    pub fn shareable_object_bytes(&self, id: Id) -> Result<Option<Vec<u8>>> {
        let db = self.db();
        Ok(db
            .query_row(
                "SELECT bytes FROM objects o WHERE o.id = ?1
                 AND (o.type IN ('log', 'head') OR EXISTS (SELECT 1 FROM log_entries l WHERE l.obj = o.id))",
                [id.digest()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// An object of a known type. Its signature is checked again on the way out.
    pub fn get<T: Object>(&self, id: Id) -> Result<Option<Sealed<T>>> {
        match self.object_bytes(id)? {
            Some(bytes) => Ok(Some(open::<T>(&bytes)?)),
            None => Ok(None),
        }
    }

    fn get_all<T: Object>(&self, sql: &str, param: &[u8]) -> Result<Vec<Sealed<T>>> {
        let ids = {
            let db = self.db();
            let mut stmt = db.prepare(sql)?;
            let rows = stmt.query_map([param], |r| r.get::<_, Vec<u8>>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        ids.iter()
            .filter_map(|d| id_from_db(d))
            .filter_map(|id| self.get::<T>(id).transpose())
            .collect()
    }

    /// Every stored revision of an item, oldest clock first.
    pub fn item_revisions(&self, item: Id) -> Result<Vec<Sealed<Revision>>> {
        self.get_all(
            "SELECT id FROM revisions WHERE item = ?1 ORDER BY clock, id",
            item.digest(),
        )
    }

    /// The item's branch tips: revisions that no other stored revision of the item builds on.
    /// One tip means a single line of edits; several mean unmerged offline edits.
    pub fn item_tips(&self, item: Id) -> Result<Vec<Sealed<Revision>>> {
        self.get_all(
            "SELECT r.id FROM revisions r WHERE r.item = ?1 AND NOT EXISTS (
                 SELECT 1 FROM revision_parents p JOIN revisions c ON c.id = p.rev
                 WHERE p.parent = r.id AND c.item = r.item)
             ORDER BY r.clock DESC, r.id",
            item.digest(),
        )
    }

    /// Items whose current version (a branch tip) is in `topic` or one of its sub-topics.
    pub fn items_in_topic(&self, topic: &Topic) -> Result<Vec<Id>> {
        let db = self.db();
        // `topic || '0'` is the first string after every `topic/...` ('0' follows '/' in ASCII),
        // so the range test finds sub-topics using the topic index.
        let mut stmt = db.prepare(
            "SELECT DISTINCT r.item FROM revision_topics t JOIN revisions r ON r.id = t.rev
             WHERE (t.topic = ?1 OR (t.topic >= ?1 || '/' AND t.topic < ?1 || '0'))
             AND NOT EXISTS (
                 SELECT 1 FROM revision_parents p JOIN revisions c ON c.id = p.rev
                 WHERE p.parent = r.id AND c.item = r.item)",
        )?;
        let rows = stmt.query_map([topic.as_str()], |r| r.get::<_, Vec<u8>>(0))?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .filter_map(|d| id_from_db(d))
            .collect())
    }

    /// The current versions (branch tips) of the most recently stored items, newest first.
    /// Includes node-only and private items; pages decide what each reader may see.
    pub fn recent_tips(&self, limit: usize) -> Result<Vec<Sealed<Revision>>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let ids = {
            let db = self.db();
            let mut stmt = db.prepare(
                "SELECT r.id FROM revisions r JOIN objects o ON o.id = r.id
                 WHERE NOT EXISTS (
                     SELECT 1 FROM revision_parents p JOIN revisions c ON c.id = p.rev
                     WHERE p.parent = r.id AND c.item = r.item)
                 ORDER BY o.rowid DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit], |r| r.get::<_, Vec<u8>>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        ids.iter()
            .filter_map(|d| id_from_db(d))
            .filter_map(|id| self.get::<Revision>(id).transpose())
            .collect()
    }

    /// The highest logical clock among objects signed by `author`. A new object by this author
    /// must use a higher one (spec section 2).
    pub fn author_clock(&self, author: PublicKey) -> Result<u64> {
        let db = self.db();
        let max: Option<i64> = db.query_row(
            "SELECT MAX(c) FROM (
                 SELECT MAX(clock) AS c FROM revisions WHERE author = ?1
                 UNION ALL SELECT MAX(clock) FROM attestations WHERE author = ?1
                 UNION ALL SELECT MAX(clock) FROM keys WHERE key = ?1)",
            [author.as_bytes()],
            |r| r.get(0),
        )?;
        Ok(max.map_or(0, |c| c as u64))
    }

    /// Attestations about an object or a key.
    pub fn attestations_about(&self, subject: Subject) -> Result<Vec<Sealed<Attestation>>> {
        let (about, is_key) = subject_to_db(subject);
        let sql = if is_key {
            "SELECT id FROM attestations WHERE about = ?1 AND about_key = 1 ORDER BY clock, id"
        } else {
            "SELECT id FROM attestations WHERE about = ?1 AND about_key = 0 ORDER BY clock, id"
        };
        self.get_all(sql, &about)
    }

    /// Key records published for a key, oldest first.
    pub fn key_records(&self, key: PublicKey) -> Result<Vec<Sealed<KeyRecord>>> {
        self.get_all("SELECT id FROM keys WHERE key = ?1 ORDER BY clock, id", key.as_bytes())
    }

    /// Revocations naming a key. Whether each one is authorized is for the trust layer to decide
    /// (see [`Revoke::is_authorized`], which takes every record from [`Store::key_records`]).
    pub fn revocations(&self, key: PublicKey) -> Result<Vec<Sealed<Revoke>>> {
        self.get_all("SELECT id FROM revokes WHERE key = ?1 ORDER BY id", key.as_bytes())
    }

    /// Derived data computed from a blob, for every producer.
    pub fn derived_from(&self, blob: Id) -> Result<Vec<Sealed<Derived>>> {
        self.get_all(
            "SELECT id FROM derived WHERE src = ?1 ORDER BY producer, id",
            blob.digest(),
        )
    }

    // -----------------------------------------------------------------------------------------
    // Logs

    /// Stores an object and appends it to `node`'s own log, so other nodes will receive it.
    ///
    /// Refused with [`StoreError::NotShareable`]: node-only and private revisions, and objects
    /// that point at them (an edit of a private revision, an attestation about one, derived data
    /// from a blob only private revisions use). Publishing an object already in the log does
    /// nothing.
    pub fn publish(&self, node: &Keypair, bytes: &[u8]) -> Result<Put> {
        let object = open_any(bytes)?;
        if matches!(object, AnyObject::LogEntry(_) | AnyObject::LogHead(_)) {
            return Err(StoreError::Log(
                "log entries and heads are not published as objects".into(),
            ));
        }
        let node_key = node.public();
        let mut db = self.db();
        let tx = db.transaction()?;
        check_shareable(&tx, &object)?;
        let put = match insert_object(&tx, &object, bytes)? {
            Inserted::Stored(put) => put,
            Inserted::Fork { node, seq } => return Err(StoreError::Forked { node, seq }),
        };
        let already_logged = tx
            .query_row(
                "SELECT 1 FROM log_entries WHERE node = ?1 AND obj = ?2",
                params![node_key.as_bytes(), put.id.digest()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !already_logged {
            let (prev_seq, prev) = last_entry(&tx, &node_key)?.map_or((0, None), |(s, id)| (s, Some(id)));
            let entry = seal_unsigned(&LogEntry {
                seq: prev_seq + 1,
                prev,
                obj: put.id,
                extra: Default::default(),
            })?;
            append_entry(&tx, &node_key, &entry)?;
        }
        tx.commit()?;
        Ok(put)
    }

    /// Signs the current end of `node`'s own log. Returns `None` if the log is empty.
    pub fn sign_head(&self, node: &Keypair) -> Result<Option<Sealed<LogHead>>> {
        let Some((seq, entry)) = last_entry(&self.db(), &node.public())? else {
            return Ok(None);
        };
        let head = nevo_format::seal(
            &LogHead {
                node: node.public(),
                seq,
                entry,
                extra: Default::default(),
            },
            node,
        )?;
        self.put_object(head.bytes())?;
        Ok(Some(head))
    }

    /// How many entries of `node`'s log this store has (they always start at 1, without gaps).
    pub fn log_position(&self, node: PublicKey) -> Result<u64> {
        Ok(last_entry(&self.db(), &node)?.map_or(0, |(seq, _)| seq))
    }

    /// The highest signed head stored for `node`: its sequence number and entry ID. Entries up
    /// to this point can be fetched from any node that has them.
    pub fn log_head(&self, node: PublicKey) -> Result<Option<(u64, Id)>> {
        let db = self.db();
        let row: Option<(i64, Vec<u8>)> = db
            .query_row(
                "SELECT seq, entry FROM log_heads WHERE node = ?1 ORDER BY seq DESC LIMIT 1",
                [node.as_bytes()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.and_then(|(seq, entry)| Some((seq as u64, id_from_db(&entry)?))))
    }

    /// Whether `node` has been caught signing two different versions of its log.
    pub fn is_forked(&self, node: PublicKey) -> Result<bool> {
        let db = self.db();
        Ok(db
            .query_row("SELECT 1 FROM log_forks WHERE node = ?1", [node.as_bytes()], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Up to `limit` entries of `node`'s log after `after`, as encoded log entry objects, together
    /// with the IDs of the objects they list.
    pub fn log_entries(&self, node: PublicKey, after: u64, limit: usize) -> Result<Vec<(Vec<u8>, Id)>> {
        let db = self.db();
        let mut stmt = db.prepare(
            "SELECT o.bytes, l.obj FROM log_entries l JOIN objects o ON o.id = l.entry
             WHERE l.node = ?1 AND l.seq > ?2 ORDER BY l.seq LIMIT ?3",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![node.as_bytes(), to_db(after, "seq")?, limit], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (bytes, obj) = row?;
            out.push((
                bytes,
                id_from_db(&obj).ok_or(StoreError::Log("bad object ID in log".into()))?,
            ));
        }
        Ok(out)
    }

    /// Adds entries of another node's log, received through sync. Store that node's signed head
    /// first (with [`Store::put_object`]): the batch must continue the stored log exactly and
    /// end at a signed head, which vouches for every entry before it through the hash chain.
    /// All entries are added, or none.
    ///
    /// Never use this for this node's own log; [`Store::publish`] writes that.
    pub fn append_log_entries(&self, node: PublicKey, entries: &[Vec<u8>]) -> Result<u64> {
        let mut db = self.db();
        let tx = db.transaction()?;
        let mut last = None;
        for bytes in entries {
            let entry = open::<LogEntry>(bytes)?;
            append_entry(&tx, &node, &entry)?;
            last = Some((entry.object.seq, entry.id()));
        }
        if let Some((seq, id)) = last {
            let signed: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT entry FROM log_heads WHERE node = ?1 AND seq = ?2",
                    params![node.as_bytes(), to_db(seq, "seq")?],
                    |r| r.get(0),
                )
                .optional()?;
            if signed.as_deref() != Some(id.digest().as_slice()) {
                return Err(StoreError::Log(format!(
                    "entries of {node} must end at one of its signed heads"
                )));
            }
        }
        let position = last_entry(&tx, &node)?.map_or(0, |(seq, _)| seq);
        tx.commit()?;
        Ok(position)
    }
}

// ---------------------------------------------------------------------------------------------
// Writing helpers (inside a transaction)

enum Inserted {
    Stored(Put),
    /// A signed head contradicted what is known about that node's log.
    Fork {
        node: PublicKey,
        seq: u64,
    },
}

fn type_name(object: &AnyObject) -> &str {
    match object {
        AnyObject::Revision(_) => Revision::TYPE,
        AnyObject::Attestation(_) => Attestation::TYPE,
        AnyObject::Key(_) => KeyRecord::TYPE,
        AnyObject::Revoke(_) => Revoke::TYPE,
        AnyObject::Derived(_) => Derived::TYPE,
        AnyObject::LogEntry(_) => LogEntry::TYPE,
        AnyObject::LogHead(_) => LogHead::TYPE,
        AnyObject::Unknown { t, .. } => t,
    }
}

fn object_id(object: &AnyObject) -> Id {
    match object {
        AnyObject::Revision(s) => s.id(),
        AnyObject::Attestation(s) => s.id(),
        AnyObject::Key(s) => s.id(),
        AnyObject::Revoke(s) => s.id(),
        AnyObject::Derived(s) => s.id(),
        AnyObject::LogEntry(s) => s.id(),
        AnyObject::LogHead(s) => s.id(),
        AnyObject::Unknown { id, .. } => *id,
    }
}

fn insert_object(tx: &Transaction, object: &AnyObject, bytes: &[u8]) -> Result<Inserted> {
    let id = object_id(object);
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO objects (id, type, bytes) VALUES (?1, ?2, ?3)",
        params![id.digest(), type_name(object), bytes],
    )? == 1;
    if let AnyObject::LogHead(head) = object {
        // Heads are checked even when already stored, so a fork is reported every time.
        if let Some(other) = head_conflict(tx, &head.object)? {
            tx.execute(
                "INSERT OR IGNORE INTO log_forks (node, seq, first, second) VALUES (?1, ?2, ?3, ?4)",
                params![
                    head.object.node.as_bytes(),
                    to_db(head.object.seq, "seq")?,
                    other,
                    id.digest()
                ],
            )?;
            return Ok(Inserted::Fork {
                node: head.object.node,
                seq: head.object.seq,
            });
        }
    }
    if inserted {
        index(tx, object)?;
    }
    Ok(Inserted::Stored(Put { id, new: inserted }))
}

fn index(tx: &Transaction, object: &AnyObject) -> Result<()> {
    match object {
        AnyObject::Revision(s) => {
            let (id, r) = (s.id(), &s.object);
            tx.execute(
                "INSERT INTO revisions (id, item, author, clock, kind, title, lang, vis)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    id.digest(),
                    r.item_id(id).digest(),
                    r.author.as_bytes(),
                    to_db(r.clock, "clock")?,
                    r.kind.as_str(),
                    r.title,
                    r.lang,
                    r.vis.as_str()
                ],
            )?;
            for parent in &r.parents {
                tx.execute(
                    "INSERT INTO revision_parents (rev, parent) VALUES (?1, ?2)",
                    [id.digest(), parent.digest()],
                )?;
            }
            for topic in &r.topics {
                tx.execute(
                    "INSERT INTO revision_topics (rev, topic) VALUES (?1, ?2)",
                    params![id.digest(), topic.as_str()],
                )?;
            }
            for f in &r.content {
                tx.execute(
                    "INSERT INTO revision_files (rev, blob, name, size, mime) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        id.digest(),
                        f.blob.digest(),
                        f.name,
                        to_db(f.size, "file size")?,
                        f.mime
                    ],
                )?;
            }
        }
        AnyObject::Attestation(s) => {
            let a = &s.object;
            let (about, is_key) = subject_to_db(a.about);
            tx.execute(
                "INSERT INTO attestations (id, about, about_key, claim, author, clock, related)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    s.id().digest(),
                    about,
                    is_key,
                    a.claim.as_str(),
                    a.author.as_bytes(),
                    to_db(a.clock, "clock")?,
                    a.related.map(|r| r.digest().to_vec())
                ],
            )?;
        }
        AnyObject::Key(s) => {
            let k = &s.object;
            tx.execute(
                "INSERT INTO keys (id, key, kind, name, recovery, clock) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    s.id().digest(),
                    k.key.as_bytes(),
                    k.kind.as_str(),
                    k.name,
                    k.recovery.map(|r| r.as_bytes().to_vec()),
                    to_db(k.clock, "clock")?
                ],
            )?;
        }
        AnyObject::Revoke(s) => {
            let r = &s.object;
            tx.execute(
                "INSERT INTO revokes (id, key, from_clock, by) VALUES (?1, ?2, ?3, ?4)",
                params![
                    s.id().digest(),
                    r.key.as_bytes(),
                    to_db(r.from, "clock")?,
                    r.by.as_bytes()
                ],
            )?;
        }
        AnyObject::Derived(s) => {
            let d = &s.object;
            tx.execute(
                "INSERT INTO derived (id, src, producer, out, author) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    s.id().digest(),
                    d.from.digest(),
                    d.producer,
                    d.out.digest(),
                    d.author.as_bytes()
                ],
            )?;
        }
        AnyObject::LogHead(s) => {
            let h = &s.object;
            tx.execute(
                "INSERT OR IGNORE INTO log_heads (node, seq, entry, head) VALUES (?1, ?2, ?3, ?4)",
                params![
                    h.node.as_bytes(),
                    to_db(h.seq, "seq")?,
                    h.entry.digest(),
                    s.id().digest()
                ],
            )?;
        }
        // Log entries are indexed when appended to a log, which says which node they belong to.
        // Objects of unknown types are kept but not indexed.
        AnyObject::LogEntry(_) | AnyObject::Unknown { .. } => {}
    }
    Ok(())
}

/// Whether a signed head contradicts another signed head, or an entry already accepted for that
/// node, at the same position. Returns the ID of the contradicting head or entry.
///
/// Accepted entries are trustworthy here: they either came from this node's own `publish` or
/// chain to a head the node signed. So a mismatch means the node signed two different logs.
fn head_conflict(tx: &Transaction, head: &LogHead) -> Result<Option<Vec<u8>>> {
    let (node, seq, entry) = (
        head.node.as_bytes(),
        to_db(head.seq, "seq")?,
        head.entry.digest().as_slice(),
    );
    let other_head: Option<(Vec<u8>, Vec<u8>)> = tx
        .query_row(
            "SELECT entry, head FROM log_heads WHERE node = ?1 AND seq = ?2",
            params![node, seq],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((other_entry, other_head)) = other_head {
        if other_entry != entry {
            return Ok(Some(other_head));
        }
    }
    let stored: Option<Vec<u8>> = tx
        .query_row(
            "SELECT entry FROM log_entries WHERE node = ?1 AND seq = ?2",
            params![node, seq],
            |r| r.get(0),
        )
        .optional()?;
    Ok(stored.filter(|s| s != entry))
}

/// Refuses objects that would put node-only or private content into a log.
fn check_shareable(tx: &Transaction, object: &AnyObject) -> Result<()> {
    let is_private_revision = |id: &Id| -> Result<bool> {
        Ok(tx
            .query_row(
                "SELECT 1 FROM revisions WHERE id = ?1 AND vis <> 'public'",
                [id.digest()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    };
    let refused = match object {
        AnyObject::Revision(s) => {
            let r = &s.object;
            r.vis != Visibility::Public
                || r.item.as_ref().map(is_private_revision).transpose()?.unwrap_or(false)
                || r.parents
                    .iter()
                    .map(is_private_revision)
                    .collect::<Result<Vec<_>>>()?
                    .contains(&true)
        }
        AnyObject::Attestation(s) => match (&s.object.about, &s.object.related) {
            (Subject::Object(id), related) => {
                is_private_revision(id)? || related.as_ref().map(is_private_revision).transpose()?.unwrap_or(false)
            }
            (Subject::Key(_), _) => false,
        },
        AnyObject::Derived(s) => {
            // Refused when only node-only or private revisions use the source blob.
            let uses = |vis_test: &str| -> Result<bool> {
                let sql = format!(
                    "SELECT 1 FROM revision_files f JOIN revisions r ON r.id = f.rev WHERE f.blob = ?1 AND r.vis {vis_test}"
                );
                Ok(tx
                    .query_row(&sql, [s.object.from.digest()], |_| Ok(()))
                    .optional()?
                    .is_some())
            };
            uses("<> 'public'")? && !uses("= 'public'")?
        }
        _ => false,
    };
    if refused {
        return Err(StoreError::NotShareable);
    }
    Ok(())
}

fn last_entry(conn: &Connection, node: &PublicKey) -> Result<Option<(u64, Id)>> {
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT seq, entry FROM log_entries WHERE node = ?1 ORDER BY seq DESC LIMIT 1",
            [node.as_bytes()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(seq, entry)| Some((seq as u64, id_from_db(&entry)?))))
}

/// Appends one entry, checking that it continues the log and agrees with any signed head at
/// its position.
fn append_entry(tx: &Transaction, node: &PublicKey, entry: &Sealed<LogEntry>) -> Result<()> {
    let (last_seq, last_id) = last_entry(tx, node)?.map_or((0, None), |(s, id)| (s, Some(id)));
    let e = &entry.object;
    if e.seq != last_seq + 1 || e.prev != last_id {
        return Err(StoreError::Log(format!(
            "entry {} doesn't continue the log of {node}, which has {last_seq} entries",
            e.seq
        )));
    }
    let signed: Option<Vec<u8>> = tx
        .query_row(
            "SELECT entry FROM log_heads WHERE node = ?1 AND seq = ?2",
            params![node.as_bytes(), to_db(e.seq, "seq")?],
            |r| r.get(0),
        )
        .optional()?;
    if signed.is_some_and(|s| s != entry.id().digest()) {
        return Err(StoreError::Log(format!(
            "entry {} of {node} contradicts its signed head",
            e.seq
        )));
    }
    tx.execute(
        "INSERT OR IGNORE INTO objects (id, type, bytes) VALUES (?1, ?2, ?3)",
        params![entry.id().digest(), LogEntry::TYPE, entry.bytes()],
    )?;
    tx.execute(
        "INSERT INTO log_entries (node, seq, entry, obj) VALUES (?1, ?2, ?3, ?4)",
        params![
            node.as_bytes(),
            to_db(e.seq, "seq")?,
            entry.id().digest(),
            e.obj.digest()
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Conversions

pub(crate) fn id_from_db(digest: &[u8]) -> Option<Id> {
    Some(Id::from_digest(digest.try_into().ok()?))
}

fn subject_to_db(subject: Subject) -> (Vec<u8>, bool) {
    match subject {
        Subject::Object(id) => (id.digest().to_vec(), false),
        Subject::Key(key) => (key.as_bytes().to_vec(), true),
    }
}

/// SQLite integers are signed 64-bit. The format caps counters at 2^63 - 1, so this only fails
/// for values that didn't come from a valid object.
fn to_db(value: u64, what: &'static str) -> Result<i64> {
    i64::try_from(value).map_err(|_| StoreError::OutOfRange(what))
}

//! The SQLite schema. Objects keep their exact bytes; every other table is an index that can be
//! rebuilt from them.

use rusqlite::{Connection, OptionalExtension};

use crate::error::{Result, StoreError};

pub const SCHEMA_VERSION: &str = "1";

const TABLES: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Every object exactly as received. IDs are the 32-byte BLAKE3 digests.
CREATE TABLE IF NOT EXISTS objects (
    id    BLOB PRIMARY KEY,
    type  TEXT NOT NULL,
    bytes BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS revisions (
    id     BLOB PRIMARY KEY,
    item   BLOB NOT NULL,
    author BLOB NOT NULL,
    clock  INTEGER NOT NULL,
    kind   TEXT NOT NULL,
    title  TEXT NOT NULL,
    lang   TEXT NOT NULL,
    vis    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS revisions_item ON revisions (item, clock);

CREATE TABLE IF NOT EXISTS revision_parents (
    rev    BLOB NOT NULL,
    parent BLOB NOT NULL,
    PRIMARY KEY (rev, parent)
);
CREATE INDEX IF NOT EXISTS revision_parents_parent ON revision_parents (parent);

CREATE TABLE IF NOT EXISTS revision_topics (
    rev   BLOB NOT NULL,
    topic TEXT NOT NULL,
    PRIMARY KEY (rev, topic)
);
CREATE INDEX IF NOT EXISTS revision_topics_topic ON revision_topics (topic);

CREATE TABLE IF NOT EXISTS revision_files (
    rev  BLOB NOT NULL,
    blob BLOB NOT NULL,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    mime TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS revision_files_rev ON revision_files (rev);
CREATE INDEX IF NOT EXISTS revision_files_blob ON revision_files (blob);

-- `about` is an object ID digest (about_key = 0) or a public key (about_key = 1).
CREATE TABLE IF NOT EXISTS attestations (
    id        BLOB PRIMARY KEY,
    about     BLOB NOT NULL,
    about_key INTEGER NOT NULL,
    claim     TEXT NOT NULL,
    author    BLOB NOT NULL,
    clock     INTEGER NOT NULL,
    related   BLOB
);
CREATE INDEX IF NOT EXISTS attestations_about ON attestations (about, about_key);

CREATE TABLE IF NOT EXISTS keys (
    id       BLOB PRIMARY KEY,
    key      BLOB NOT NULL,
    kind     TEXT NOT NULL,
    name     TEXT NOT NULL,
    recovery BLOB,
    clock    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS keys_key ON keys (key, clock);

CREATE TABLE IF NOT EXISTS revokes (
    id         BLOB PRIMARY KEY,
    key        BLOB NOT NULL,
    from_clock INTEGER NOT NULL,
    by         BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS revokes_key ON revokes (key);

CREATE TABLE IF NOT EXISTS derived (
    id       BLOB PRIMARY KEY,
    src      BLOB NOT NULL,
    producer TEXT NOT NULL,
    out      BLOB NOT NULL,
    author   BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS derived_src ON derived (src, producer);
-- For garbage collection, which keeps any blob that derived data uses.
CREATE INDEX IF NOT EXISTS derived_out ON derived (out);

-- Each node's append-only log. `entry` is the log entry object's ID, `obj` the logged object.
CREATE TABLE IF NOT EXISTS log_entries (
    node  BLOB NOT NULL,
    seq   INTEGER NOT NULL,
    entry BLOB NOT NULL,
    obj   BLOB NOT NULL,
    PRIMARY KEY (node, seq)
);

-- Every signed head seen. Entries from other nodes are accepted only up to one of these.
CREATE TABLE IF NOT EXISTS log_heads (
    node  BLOB NOT NULL,
    seq   INTEGER NOT NULL,
    entry BLOB NOT NULL,
    head  BLOB NOT NULL,
    PRIMARY KEY (node, seq)
);

-- Proof that a node signed two different logs: two contradicting heads (or a head and an entry
-- that chains to an earlier head). Both objects stay in `objects`.
CREATE TABLE IF NOT EXISTS log_forks (
    node   BLOB NOT NULL,
    seq    INTEGER NOT NULL,
    first  BLOB NOT NULL,
    second BLOB NOT NULL,
    PRIMARY KEY (node, seq, first, second)
);
CREATE INDEX IF NOT EXISTS log_entries_obj ON log_entries (obj);
";

pub fn init(conn: &mut Connection) -> Result<()> {
    // WAL lets readers work while a write is in progress. FULL syncs every commit to disk: on a
    // Pi that loses power, a published entry and its signed head must never roll back, or the
    // node would later sign a different entry at the same position and look like it forked.
    conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
    conn.execute_batch("PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")?;

    let tx = conn.transaction()?;
    tx.execute_batch(TABLES)?;
    let version: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
        .optional()?;
    match version.as_deref() {
        None => {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                [SCHEMA_VERSION],
            )?;
        }
        Some(SCHEMA_VERSION) => {}
        Some(other) => return Err(StoreError::SchemaVersion(other.to_string())),
    }
    tx.commit()?;
    Ok(())
}

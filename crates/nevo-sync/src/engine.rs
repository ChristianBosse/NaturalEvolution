//! Pull rounds: bring this node up to date with one peer.
//!
//! One round is heads, then entries, then missing objects, then blobs by policy. Each step
//! stores what it verified before asking for more, so a round can stop at any point (a limit,
//! a lost link, a power cut) and the next round continues from what was stored:
//!
//! - log entries are stored in batches that end at a signed head, so a log's position only
//!   moves to points its owner vouched for;
//! - objects are stored one by one, and "listed in a log but missing" is a store query;
//! - blob parts are appended to a partial file that survives restarts, and the whole blob is
//!   checked against its ID before it is kept.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use nevo_format::{open, open_any, AnyObject, Id, LogEntry, LogHead, PublicKey, Revision, Sealed, Topic, Visibility};
use nevo_store::{Store, StoreError};

use crate::client::PeerClient;
use crate::error::{Result, SyncError};
use crate::limits::{MAX_HEADS, MAX_IDS, MAX_PIECES, PIECE};
use crate::msg::{BlobRequest, EntriesRequest, HeadsRequest, ObjectsRequest};

/// Entries kept in memory while waiting for a signed head to vouch for them. A peer that never
/// reaches a head can't make us hold more (about 9 MB).
const MAX_UNSIGNED_RUN: usize = 100_000;
/// Pages of heads read at most (500 logs each), so a looping peer can't keep a round going.
const MAX_HEAD_PAGES: usize = 1000;

/// Decides which blobs to fetch. Metadata always moves; files move by policy (D-021).
pub trait BlobPolicy: Send + Sync {
    /// Whether to fetch `blob`, about `size` bytes, used by the public revisions `users`.
    fn wants(&self, blob: Id, size: u64, users: &[Sealed<Revision>]) -> bool;
}

/// A policy by size, topic, author and pinned items. The trust layer fills `authors` with the
/// keys it trusts; until then every author is accepted.
#[derive(Debug, Clone, Default)]
pub struct SimplePolicy {
    /// Largest blob fetched, unless pinned. `None`: any size.
    pub max_size: Option<u64>,
    /// Only blobs of revisions in these topics or below. Empty: every topic.
    pub topics: Vec<Topic>,
    /// Only blobs of revisions by these authors. `None`: any author.
    pub authors: Option<Vec<PublicKey>>,
    /// Items or blobs always fetched, whatever their size, topic or author.
    pub pinned: Vec<Id>,
}

impl BlobPolicy for SimplePolicy {
    fn wants(&self, blob: Id, size: u64, users: &[Sealed<Revision>]) -> bool {
        let pinned =
            self.pinned.contains(&blob) || users.iter().any(|r| self.pinned.contains(&r.object.item_id(r.id())));
        if pinned {
            return true;
        }
        if self.max_size.is_some_and(|max| size > max) {
            return false;
        }
        users.iter().any(|r| {
            let topic_ok = self.topics.is_empty()
                || r.object
                    .topics
                    .iter()
                    .any(|t| self.topics.iter().any(|want| want.covers(t)));
            let author_ok = self.authors.as_ref().is_none_or(|a| a.contains(&r.object.author));
            topic_ok && author_ok
        })
    }
}

/// Limits and policy for one pull round.
#[derive(Clone)]
pub struct PullOptions {
    /// Log entries stored per round at most (checked between signed heads).
    pub max_entries: u64,
    /// Objects stored per round at most.
    pub max_objects: u64,
    /// Blob bytes downloaded per round at most (checked between parts of up to 4 MiB).
    pub max_blob_bytes: u64,
    pub policy: Arc<dyn BlobPolicy>,
    /// Where unfinished blob downloads wait between rounds.
    pub partial_dir: PathBuf,
}

impl PullOptions {
    /// Defaults: generous limits, every blob, partial downloads in `<store>/sync-partial`.
    pub fn new(store: &Store) -> PullOptions {
        PullOptions {
            max_entries: 1_000_000,
            max_objects: 100_000,
            max_blob_bytes: 4 << 30,
            policy: Arc::new(SimplePolicy::default()),
            partial_dir: store.root().join("sync-partial"),
        }
    }
}

/// What a pull round did.
#[derive(Debug, Clone, Default)]
pub struct PullReport {
    /// Logs that got new entries.
    pub logs: usize,
    pub entries: u64,
    pub objects: u64,
    pub blobs: u64,
    /// Blob bytes downloaded, including parts of blobs not finished yet.
    pub blob_bytes: u64,
    /// Nodes seen in this round that signed two different logs. The proof is stored; the
    /// trust layer decides what that means for their content.
    pub forks: Vec<PublicKey>,
    /// What was refused (forged heads or entries, corrupt blobs, …). The round goes on.
    pub problems: Vec<String>,
    /// False when a limit stopped the round early; the next round continues.
    pub complete: bool,
}

/// Pulls from `peer` into `store`: one round. Safe to stop at any point and to run again.
///
/// Errors are about the connection or this node's storage. Bad data from the peer is refused
/// item by item and listed in [`PullReport::problems`].
pub async fn pull(store: &Arc<Store>, peer: &PeerClient, options: &PullOptions) -> Result<PullReport> {
    let mut round = Round {
        store,
        peer,
        options,
        report: PullReport {
            complete: true,
            ..PullReport::default()
        },
    };
    let (heads, forks) = round.heads().await?;
    for head in heads {
        round.log(head).await?;
    }
    // Fork evidence comes after the logs: a contradicting head is then checked against the
    // entries just stored, and can't block the honest log from being accepted first.
    round.fork_evidence(forks)?;
    round.objects().await?;
    round.blobs().await?;
    Ok(round.report)
}

struct Round<'a> {
    store: &'a Arc<Store>,
    peer: &'a PeerClient,
    options: &'a PullOptions,
    report: PullReport,
}

impl Round<'_> {
    fn problem(&mut self, what: String) {
        self.report.problems.push(what);
    }

    fn fork(&mut self, node: PublicKey) {
        if !self.report.forks.contains(&node) {
            self.report.forks.push(node);
        }
    }

    async fn heads(&mut self) -> Result<(Vec<Sealed<LogHead>>, Vec<Vec<u8>>)> {
        let (mut heads, mut forks, mut after) = (Vec::new(), Vec::new(), None);
        for _ in 0..MAX_HEAD_PAGES {
            let page = self
                .peer
                .heads(&HeadsRequest {
                    after,
                    limit: MAX_HEADS,
                })
                .await?;
            for bytes in page.heads {
                match open::<LogHead>(&bytes) {
                    Ok(head) => heads.push(head),
                    Err(e) => self.problem(format!("refused a log head: {e}")),
                }
            }
            forks.extend(page.forks);
            match page.next {
                // Pages must move forward, or a peer could keep us asking forever.
                Some(next) if after.is_none_or(|a| next > a) => after = Some(next),
                _ => break,
            }
        }
        Ok((heads, forks))
    }

    /// Brings one log up to the peer's signed head.
    async fn log(&mut self, head: Sealed<LogHead>) -> Result<()> {
        let node = head.object.node;
        if node == self.peer.me() {
            // This node writes its own log; copies from elsewhere add nothing.
            return Ok(());
        }
        let stored = self.store_head(node, head.bytes());
        if stored {
            self.entries(node, head.object.seq).await?;
        }
        if self.store.is_forked(node)? {
            self.fork(node);
        }
        Ok(())
    }

    /// Stores a signed head. Storing it even when we are ahead checks it against our copy of
    /// the log, which is how forks are found.
    fn store_head(&mut self, node: PublicKey, bytes: &[u8]) -> bool {
        match self.store.put_object(bytes) {
            Ok(_) => true,
            Err(StoreError::Forked { .. }) => {
                self.fork(node);
                false
            }
            Err(e) => {
                self.problem(format!("refused a head of {node}: {e}"));
                false
            }
        }
    }

    async fn entries(&mut self, node: PublicKey, target: u64) -> Result<()> {
        let start = self.store.log_position(node)?;
        let mut next = start;
        // Entries received but not yet vouched for by a signed head.
        let mut pending: Vec<Vec<u8>> = Vec::new();
        while next < target {
            if self.report.entries >= self.options.max_entries {
                self.report.complete = false;
                break;
            }
            let res = self
                .peer
                .entries(&EntriesRequest {
                    node,
                    after: next,
                    upto: target,
                })
                .await?;
            if res.entries.is_empty() {
                self.problem(format!("peer sent no entries of {node} after {next}"));
                break;
            }
            let mut valid = true;
            for bytes in res.entries {
                match open::<LogEntry>(&bytes) {
                    Ok(e) if e.object.seq == next + 1 && e.object.seq <= target => {
                        next += 1;
                        pending.push(bytes);
                    }
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                self.problem(format!("refused entries of {node}: they don't continue the log"));
                break;
            }
            let at_head = match res.head {
                Some(bytes) => match open::<LogHead>(&bytes) {
                    Ok(h) if h.object.node == node && h.object.seq == next => {
                        if !self.store_head(node, &bytes) {
                            break;
                        }
                        true
                    }
                    _ => {
                        self.problem(format!("refused a head of {node} sent with its entries"));
                        break;
                    }
                },
                None => next == target,
            };
            if at_head {
                // The store checks the hash chain from the head back to what we had.
                match self.store.append_log_entries(node, &pending) {
                    Ok(_) => {
                        self.report.entries += pending.len() as u64;
                        pending.clear();
                    }
                    Err(e) => {
                        self.problem(format!("refused entries of {node}: {e}"));
                        break;
                    }
                }
            } else if pending.len() > MAX_UNSIGNED_RUN {
                self.problem(format!("refused entries of {node}: too many without a signed head"));
                break;
            }
        }
        if self.store.log_position(node)? > start {
            self.report.logs += 1;
        }
        Ok(())
    }

    fn fork_evidence(&mut self, heads: Vec<Vec<u8>>) -> Result<()> {
        for bytes in heads {
            match open::<LogHead>(&bytes) {
                Ok(head) => {
                    self.store_head(head.object.node, &bytes);
                    if self.store.is_forked(head.object.node)? {
                        self.fork(head.object.node);
                    }
                }
                Err(e) => self.problem(format!("refused fork evidence: {e}")),
            }
        }
        Ok(())
    }

    async fn objects(&mut self) -> Result<()> {
        let mut after = None;
        loop {
            let left = self.options.max_objects.saturating_sub(self.report.objects);
            if left == 0 {
                self.report.complete = false;
                return Ok(());
            }
            let ids = self.store.missing_logged_objects(after, MAX_IDS.min(left as usize))?;
            if ids.is_empty() {
                return Ok(());
            }
            let res = self.peer.objects(&ObjectsRequest { ids: ids.clone() }).await?;
            let done = (res.done as usize).min(ids.len());
            if done == 0 {
                self.problem("peer looked at none of the objects asked for".into());
                return Ok(());
            }
            let asked: HashSet<Id> = ids[..done].iter().copied().collect();
            for bytes in res.objects {
                self.object(&asked, &bytes)?;
            }
            // Objects the peer doesn't have stay missing; another peer may have them.
            after = Some(ids[done - 1]);
        }
    }

    fn object(&mut self, asked: &HashSet<Id>, bytes: &[u8]) -> Result<()> {
        // An object's ID is the hash of its bytes, so this proves it is the one listed in a log.
        let id = Id::of(bytes);
        if !asked.contains(&id) {
            self.problem(format!("peer sent object {id}, which wasn't asked for"));
            return Ok(());
        }
        match open_any(bytes) {
            Ok(AnyObject::Revision(r)) if r.object.vis != Visibility::Public => {
                self.problem(format!("refused {id}: node-only and private revisions never travel"));
                return Ok(());
            }
            Ok(_) => {}
            Err(e) => {
                self.problem(format!("refused {id}: {e}"));
                return Ok(());
            }
        }
        match self.store.put_object(bytes) {
            Ok(put) => self.report.objects += u64::from(put.new),
            Err(StoreError::Format(e)) => self.problem(format!("refused {id}: {e}")),
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    async fn blobs(&mut self) -> Result<()> {
        for blob in self.store.missing_blobs()? {
            let users: Vec<_> = self
                .store
                .blob_users(blob)?
                .into_iter()
                .filter(|r| r.object.vis == Visibility::Public)
                .collect();
            // The sizes the revisions claim. The peer must agree with one of them, which keeps
            // a peer from making us download more than any revision promised.
            let sizes: Vec<u64> = users
                .iter()
                .flat_map(|r| r.object.content.iter().filter(|f| f.blob == blob).map(|f| f.size))
                .collect();
            let Some(&size) = sizes.iter().max() else {
                continue;
            };
            if !self.options.policy.wants(blob, size, &users) {
                continue;
            }
            if self.report.blob_bytes >= self.options.max_blob_bytes {
                self.report.complete = false;
                return Ok(());
            }
            self.blob(blob, &sizes).await?;
        }
        Ok(())
    }

    async fn blob(&mut self, blob: Id, sizes: &[u64]) -> Result<()> {
        fs::create_dir_all(&self.options.partial_dir)?;
        let partial = self.options.partial_dir.join(blob.to_string());
        let mut file = OpenOptions::new().create(true).append(true).open(&partial)?;
        let len = file.metadata()?.len();
        if len > 0 && sizes.contains(&len) {
            // Downloaded in an earlier round but not kept yet (stopped while hashing).
            drop(file);
            return self.finish_blob(blob, partial).await;
        }
        // Resume at the last whole piece: a write cut short by a power loss is redone.
        let mut have = len - len % PIECE;
        file.set_len(have)?;
        loop {
            let left = self.options.max_blob_bytes.saturating_sub(self.report.blob_bytes);
            if left == 0 {
                self.report.complete = false;
                return Ok(());
            }
            let want = left.div_ceil(PIECE).clamp(1, MAX_PIECES) * PIECE;
            let res = match self
                .peer
                .blob(&BlobRequest {
                    id: blob,
                    from: have,
                    len: want,
                })
                .await
            {
                Ok(res) => res,
                // The peer doesn't have it (or may not share it); another peer may.
                Err(SyncError::Status { status: 404, .. }) => return Ok(()),
                Err(SyncError::Status { status, message }) => {
                    self.problem(format!("peer refused blob {blob} ({status}): {message}"));
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            let got = res.data.len() as u64;
            let end = have.saturating_add(got);
            let whole_pieces = got % PIECE == 0 || end == res.size;
            if !sizes.contains(&res.size) || end > res.size || (got == 0 && have < res.size) || !whole_pieces {
                self.problem(format!(
                    "refused blob {blob}: the peer's answer doesn't fit its revisions"
                ));
                return Ok(());
            }
            file.write_all(&res.data)?;
            have = end;
            self.report.blob_bytes += got;
            if have == res.size {
                break;
            }
        }
        file.sync_all()?;
        drop(file);
        self.finish_blob(blob, partial).await
    }

    /// Hashes the finished download and keeps it only if it matches the blob's ID.
    async fn finish_blob(&mut self, blob: Id, partial: PathBuf) -> Result<()> {
        let store = Arc::clone(self.store);
        let path = partial.clone();
        let result = tokio::task::spawn_blocking(move || store.put_blob_expecting(File::open(&path)?, blob))
            .await
            .map_err(|e| SyncError::Io(std::io::Error::other(e)))?;
        // Kept or not, the download is done with. A corrupt one starts over next time.
        let _ = fs::remove_file(&partial);
        match result {
            Ok(_) => self.report.blobs += 1,
            Err(StoreError::BlobMismatch { actual, .. }) => {
                self.problem(format!("refused blob {blob}: the peer's copy hashes to {actual}"))
            }
            // An earlier round that was cut off finished this blob in the background.
            Err(StoreError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound && self.store.has_blob(blob) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}

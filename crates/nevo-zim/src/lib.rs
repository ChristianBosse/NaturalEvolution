//! Kiwix ZIM packs (offline Wikipedia, WikiMed, …) for NEVO nodes.
//!
//! - [`Pack::open`] memory-maps a `.zim` file and reads its metadata. It takes a few
//!   milliseconds and reads almost nothing, whatever the file's size.
//! - [`Pack::find`] looks up an entry by path, following redirects, and [`Pack::read`] returns
//!   its bytes, for serving articles, images and CSS to a browser.
//! - [`Pack::articles`] walks every article for indexing, one cluster at a time.
//! - [`extract`] turns article HTML into clean text in sections, and [`Document::passages`]
//!   splits that into short passages for the AI.
//!
//! ZIM files come from outside, so a damaged or hostile file gives an error, never a panic or a
//! hang, and every size read from the file is checked against a limit before memory is used.
//!
//! The pure-Rust [`zim`](https://crates.io/crates/zim) crate parses the file structure (D-035).
//! Cluster decompression is done here instead, with size limits (see `cluster.rs`). `zim`
//! 0.5.0 has no cargo features, so the command-line crates its own tools need (clap,
//! indicatif, rayon, pbr, …) are always built with it; they add build time, not code we call.
//!
//! ## Measured on WikiMed (1 GB, 104,216 full articles, no pictures)
//!
//! One desktop core (Ryzen 7 9800X3D) in a container capped at 4 GB, 2026-10-22; run
//! `measure_large_pack` in `tests/real_packs.rs` to repeat it. Expect a Pi to be several
//! times slower.
//!
//! | | |
//! |---|---|
//! | Open the pack | 3 ms |
//! | List articles and redirects (before the first article) | 0.2 s |
//! | Iterate, extract text and passages from every article | 34 s (3,000 articles/s) |
//! | HTML read → clean text | 6.8 GB → 438 MB |
//! | Passages | 577,441 (5.5 per article) |
//! | Aliases (redirect titles) | 267,399 |
//! | Peak heap memory (`RssAnon`) | 14 MB |
//! | Peak resident memory (`VmHWM`, includes the mapped file, which the OS can drop) | 808 MB |
//!
//! Decompression takes about 7 s of that; the rest is HTML tokenizing. Extraction is
//! independent per article, so an indexer can spread it over several cores.

mod articles;
mod cluster;
mod error;
mod text;

use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use zim::{DirectoryEntry, MimeType, Namespace, Target, Zim};

pub use articles::{Article, Articles};
use cluster::Cluster;
use error::{corrupt, from_zim};
pub use error::{Result, ZimError};
pub use text::{extract, Document, Passage, Section, PASSAGE_MAX, PASSAGE_MIN};

/// Largest entry [`Pack::read`] returns. Bigger entries (long videos) need a streaming API,
/// which NEVO doesn't have yet.
pub const MAX_ENTRY_SIZE: usize = cluster::MAX_CLUSTER;

/// Metadata values above this size are ignored. The ZIM spec caps the longest one at 4,000
/// characters.
const MAX_METADATA: usize = 64 << 10;

/// The pack's icon is a small PNG.
const MAX_ILLUSTRATION: usize = 1 << 20;

/// Paths longer than this can't exist in a sane file, so they aren't looked up.
const MAX_PATH: usize = 4096;

/// A Kiwix ZIM file, opened for reading.
///
/// `Pack` is `Send + Sync`: one instance can serve many requests at once.
pub struct Pack {
    zim: Zim,
    metadata: Metadata,
    /// The namespace articles live in: `C` since ZIM 6.1, `A` before.
    content_ns: Namespace,
    /// The last decompressed cluster. Pages and their resources tend to share clusters, and so do
    /// the metadata entries read at open.
    last_cluster: Mutex<Option<CachedCluster>>,
}

/// Cluster index, decompressed data, and whether it uses 8-byte offsets.
type CachedCluster = (u32, Arc<Vec<u8>>, bool);

/// What a pack says about itself (the ZIM `M/` entries). Missing values are `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub long_description: Option<String>,
    /// ISO 639-3 codes, comma-separated when there are several (`eng`, `fra,eng`).
    pub language: Option<String>,
    /// `YYYY-MM-DD`.
    pub date: Option<String>,
    pub creator: Option<String>,
    pub publisher: Option<String>,
    /// Kiwix's identifier for the pack, without date or flavour (`wikipedia_en_medicine`).
    pub name: Option<String>,
    /// `maxi`, `nopic`, `mini`, …
    pub flavour: Option<String>,
    pub tags: Option<String>,
    /// Articles as Kiwix counts them, from the article listing: pages plus the redirects to them
    /// (WikiMed: 371,615 = 104,216 pages + 267,399 redirects). `None` before ZIM 6.1.
    pub article_count: Option<u32>,
    /// Every entry: articles, redirects, images, scripts, metadata.
    pub entry_count: u32,
    /// Whether the file carries Kiwix's Xapian full-text index. NEVO can't read it (D-036), but
    /// it makes the file bigger, which matters when choosing what to download.
    pub has_xapian_index: bool,
}

/// An entry that has content: an article, an image, a stylesheet, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The path to link to. For files older than ZIM 6.1 it starts with the namespace (`A/Foo`),
    /// so that relative links inside their HTML keep working.
    pub path: String,
    pub title: String,
    pub mime: String,
    /// The path that was asked for, when it was a redirect to this entry. A web server should
    /// redirect the browser to [`Entry::path`] so relative links resolve.
    pub redirected_from: Option<String>,
    cluster: u32,
    blob: u32,
}

impl Pack {
    /// Opens a ZIM file. Only the header and the metadata entries are read.
    pub fn open(path: impl AsRef<Path>) -> Result<Pack> {
        let zim = Zim::new(path.as_ref()).map_err(from_zim)?;
        let h = &zim.header;
        let content_ns = if (h.version_major, h.version_minor) >= (6, 1) {
            Namespace::UserContent
        } else {
            Namespace::Articles
        };
        let mut pack = Pack {
            zim,
            metadata: Metadata::default(),
            content_ns,
            last_cluster: Mutex::new(None),
        };
        pack.metadata = pack.read_metadata()?;
        Ok(pack)
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// The pack's 48×48 PNG icon, if it has one.
    pub fn illustration(&self) -> Result<Option<Vec<u8>>> {
        self.metadata_bytes("Illustration_48x48@1", MAX_ILLUSTRATION)
    }

    /// The page to show first.
    pub fn main_page(&self) -> Result<Option<Entry>> {
        // `W/mainPage` is the current way; older files only have the header's index.
        let entry = match self
            .zim
            .get_by_path(Namespace::CategoriesArticle, "mainPage")
            .map_err(from_zim)?
        {
            Some(e) => e,
            None => match self.zim.header.main_page {
                Some(idx) => self.zim.get_by_url_index(idx).map_err(from_zim)?,
                None => return Ok(None),
            },
        };
        let mut found = self.resolve(entry)?;
        if let Some(e) = &mut found {
            e.redirected_from = None;
        }
        Ok(found)
    }

    /// Looks up an entry by path, following redirects.
    ///
    /// Paths are tried in the `C` namespace (ZIM 6.1 and later), then in `A` (older files), then
    /// as `<namespace>/<path>` for older files' resources (`I/logo.png`, `-/style.css`).
    pub fn find(&self, path: &str) -> Result<Option<Entry>> {
        if path.is_empty() || path.len() > MAX_PATH {
            return Ok(None);
        }
        let mut tries = vec![(Namespace::UserContent, path), (Namespace::Articles, path)];
        if let Some((ns, rest)) = split_namespace(path) {
            tries.push((ns, rest));
        }
        for (ns, p) in tries {
            if let Some(e) = self.zim.get_by_path(ns, p).map_err(from_zim)? {
                return self.resolve(e);
            }
        }
        Ok(None)
    }

    /// The entry's bytes (HTML, image, …). Entries over [`MAX_ENTRY_SIZE`] are refused.
    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        let cluster = self.cluster(entry.cluster)?;
        let bytes = cluster.blob(entry.blob)?;
        if bytes.len() > MAX_ENTRY_SIZE {
            return Err(ZimError::TooLarge {
                what: "entry",
                limit: MAX_ENTRY_SIZE as u64,
            });
        }
        Ok(bytes.to_vec())
    }

    /// Every article, for indexing. See [`Articles`].
    pub fn articles(&self) -> Result<Articles<'_>> {
        Articles::new(self)
    }

    /// Follows redirects to an entry with content. Entries without content (deleted or link
    /// targets from old files) count as missing.
    fn resolve(&self, requested: DirectoryEntry) -> Result<Option<Entry>> {
        let redirected_from = matches!(requested.target, Some(Target::Redirect(_))).then(|| self.path_of(&requested));
        let e = self.zim.resolve(requested).map_err(from_zim)?;
        match (&e.target, &e.mime_type) {
            (&Some(Target::Cluster(cluster, blob)), MimeType::Type(mime)) => Ok(Some(Entry {
                path: self.path_of(&e),
                title: title_of(&e),
                mime: mime.clone(),
                redirected_from,
                cluster,
                blob,
            })),
            _ => Ok(None),
        }
    }

    fn path_of(&self, e: &DirectoryEntry) -> String {
        if e.namespace == Namespace::UserContent {
            e.url.clone()
        } else {
            format!("{}/{}", e.namespace.as_byte() as char, e.url)
        }
    }

    /// Reads a cluster, reusing the last one if it's the same.
    fn cluster(&self, idx: u32) -> Result<Cluster<'_>> {
        // A panic while holding the lock leaves at worst a stale but valid cache entry.
        let mut last = self.last_cluster.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached, data, extended)) = &*last {
            if *cached == idx {
                return Ok(Cluster::from_decoded(data.clone(), *extended));
            }
        }
        let cluster = Cluster::read(&self.zim, idx)?;
        if let Some((data, extended)) = cluster.decoded() {
            *last = Some((idx, data, extended));
        }
        Ok(cluster)
    }

    /// Reads a blob through the cache, refusing ones over `limit` bytes.
    fn entry_bytes(&self, e: &DirectoryEntry, limit: usize) -> Result<Option<Vec<u8>>> {
        let Some(Target::Cluster(cluster, blob)) = e.target else {
            return Ok(None);
        };
        let cluster = self.cluster(cluster)?;
        let bytes = cluster.blob(blob)?;
        Ok((bytes.len() <= limit).then(|| bytes.to_vec()))
    }

    fn metadata_bytes(&self, name: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        match self.zim.get_by_path(Namespace::Metadata, name).map_err(from_zim)? {
            Some(e) => self.entry_bytes(&e, limit),
            None => Ok(None),
        }
    }

    fn metadata_text(&self, name: &str) -> Result<Option<String>> {
        let bytes = self.metadata_bytes(name, MAX_METADATA)?;
        Ok(bytes
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .filter(|s| !s.is_empty()))
    }

    fn read_metadata(&self) -> Result<Metadata> {
        let text = |name| self.metadata_text(name);
        Ok(Metadata {
            title: text("Title")?,
            description: text("Description")?,
            long_description: text("LongDescription")?,
            language: text("Language")?,
            date: text("Date")?,
            creator: text("Creator")?,
            publisher: text("Publisher")?,
            name: text("Name")?,
            flavour: text("Flavour")?,
            tags: text("Tags")?,
            article_count: self.article_listing_len()?,
            entry_count: self.zim.header.article_count,
            has_xapian_index: self
                .zim
                .get_by_path(Namespace::FulltextIndex, "fulltext/xapian")
                .map_err(from_zim)?
                .is_some(),
        })
    }

    /// Length of `X/listing/titleOrdered/v1`, which lists every article (ZIM 6.1 and later).
    fn article_listing_len(&self) -> Result<Option<u32>> {
        let Some(e) = self.listing_entry()? else {
            return Ok(None);
        };
        let Some(Target::Cluster(cluster, blob)) = e.target else {
            return Ok(None);
        };
        let cluster = self.cluster(cluster)?;
        let len = cluster.blob(blob)?.len();
        if len % 4 != 0 {
            return Err(corrupt("article listing has a broken length"));
        }
        Ok(Some(
            u32::try_from(len / 4).map_err(|_| corrupt("article listing is too long"))?,
        ))
    }

    fn listing_entry(&self) -> Result<Option<DirectoryEntry>> {
        self.zim
            .get_by_path(Namespace::FulltextIndex, "listing/titleOrdered/v1")
            .map_err(from_zim)
    }
}

impl fmt::Debug for Pack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pack")
            .field("file", &self.zim.file_path)
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

/// Splits `I/logo.png` into the namespace and the rest, for old files' resources.
fn split_namespace(path: &str) -> Option<(Namespace, &str)> {
    let (ns, rest) = path.split_at_checked(1)?;
    let rest = rest.strip_prefix('/')?;
    match ns {
        "-" | "A" | "I" | "J" | "C" => Some((Namespace::from(ns.as_bytes()[0]), rest)),
        _ => None,
    }
}

fn title_of(e: &DirectoryEntry) -> String {
    if e.title.is_empty() {
        e.url.clone()
    } else {
        e.title.clone()
    }
}

fn is_html(e: &DirectoryEntry) -> bool {
    matches!(&e.mime_type, MimeType::Type(m) if m.starts_with("text/html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_is_shareable() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Pack>();
    }

    #[test]
    fn splits_old_namespaces() {
        assert!(matches!(
            split_namespace("I/logo.png"),
            Some((Namespace::ImagesFile, "logo.png"))
        ));
        assert!(matches!(split_namespace("-/s.css"), Some((Namespace::Layout, "s.css"))));
        assert!(split_namespace("M/Title").is_none());
        assert!(split_namespace("Foo").is_none());
        assert!(split_namespace("é/x").is_none());
    }

    #[test]
    fn open_rejects_non_zim_files() {
        let dir = std::env::temp_dir().join(format!("nevo-zim-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in [
            ("empty.zim", vec![]),
            ("short.zim", vec![1, 2, 3]),
            ("zeros.zim", vec![0; 4096]),
            ("text.zim", b"<html>not a zim</html>".repeat(100)),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            assert!(Pack::open(&path).is_err(), "{name}");
        }
        assert!(matches!(Pack::open(dir.join("missing.zim")), Err(ZimError::Io(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

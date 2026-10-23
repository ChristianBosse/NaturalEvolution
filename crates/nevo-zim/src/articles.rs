//! Walking every article of a pack, for indexing.

use zim::{Target, Zim};

use crate::cluster::Cluster;
use crate::error::{corrupt, from_zim, Result, ZimError};
use crate::{is_html, title_of, Pack, MAX_ENTRY_SIZE};

/// A redirect may point at another redirect; a chain this long is a loop.
const MAX_REDIRECT_HOPS: usize = 50;

/// One article, as returned by [`Articles`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    /// The path to link to (see [`crate::Entry::path`]).
    pub path: String,
    pub title: String,
    /// Titles of the redirects that lead here ("Heart attack" for "Myocardial infarction"),
    /// worth indexing as alternative names.
    pub aliases: Vec<String>,
    /// The page's HTML. Invalid UTF-8 is replaced, not refused.
    pub html: String,
}

/// Iterator over a pack's articles, from [`Pack::articles`].
///
/// Articles come in storage order, grouped by cluster, so each cluster is decompressed once and
/// only one is in memory at a time. What's held for the whole walk is 12 bytes per article and
/// 8 per redirect (no strings): about 2 MB for WikiMed, about 200 MB for all of English
/// Wikipedia.
///
/// If a cluster can't be read, one error is returned for it and its articles are skipped; the
/// walk then goes on with the next cluster.
pub struct Articles<'a> {
    pack: &'a Pack,
    items: std::vec::IntoIter<Item>,
    /// (article entry, redirect entry), sorted, for looking up aliases.
    aliases: Vec<(u32, u32)>,
    current: Option<(u32, Cluster<'a>)>,
    failed: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
struct Item {
    cluster: u32,
    blob: u32,
    entry: u32,
}

impl<'a> Articles<'a> {
    pub(crate) fn new(pack: &'a Pack) -> Result<Articles<'a>> {
        let zim = &pack.zim;
        let listed = Listed::read(pack)?;
        let mut items = Vec::new();
        let mut aliases = Vec::new();
        for idx in zim.namespace_range(pack.content_ns).map_err(from_zim)? {
            let e = zim.get_by_url_index(idx).map_err(from_zim)?;
            match e.target {
                Some(Target::Cluster(cluster, blob)) if listed.contains(idx) && is_html(&e) => {
                    items.push(Item {
                        cluster,
                        blob,
                        entry: idx,
                    });
                }
                Some(Target::Redirect(to)) => {
                    if let Some(article) = final_article(zim, &listed, to)? {
                        aliases.push((article, idx));
                    }
                }
                _ => {}
            }
        }
        items.sort_unstable_by_key(|i| (i.cluster, i.blob));
        aliases.sort_unstable();
        Ok(Articles {
            pack,
            items: items.into_iter(),
            aliases,
            current: None,
            failed: None,
        })
    }

    fn article(&self, item: Item, cluster: &Cluster<'_>) -> Result<Article> {
        let zim = &self.pack.zim;
        let bytes = cluster.blob(item.blob)?;
        if bytes.len() > MAX_ENTRY_SIZE {
            return Err(ZimError::TooLarge {
                what: "article",
                limit: MAX_ENTRY_SIZE as u64,
            });
        }
        let e = zim.get_by_url_index(item.entry).map_err(from_zim)?;
        let title = title_of(&e);
        let start = self.aliases.partition_point(|&(a, _)| a < item.entry);
        let mut aliases: Vec<String> = Vec::new();
        for &(_, redirect) in self.aliases[start..].iter().take_while(|&&(a, _)| a == item.entry) {
            let alias = title_of(&zim.get_by_url_index(redirect).map_err(from_zim)?);
            if alias != title && !aliases.contains(&alias) {
                aliases.push(alias);
            }
        }
        Ok(Article {
            path: self.pack.path_of(&e),
            title,
            aliases,
            html: String::from_utf8_lossy(bytes).into_owned(),
        })
    }
}

impl Iterator for Articles<'_> {
    type Item = Result<Article>;

    fn next(&mut self) -> Option<Result<Article>> {
        loop {
            let item = self.items.next()?;
            if self.failed == Some(item.cluster) {
                continue;
            }
            if self.current.as_ref().map(|(idx, _)| *idx) != Some(item.cluster) {
                // Free the previous cluster before decompressing the next one.
                self.current = None;
                match Cluster::read(&self.pack.zim, item.cluster) {
                    Ok(c) => self.current = Some((item.cluster, c)),
                    Err(e) => {
                        self.failed = Some(item.cluster);
                        return Some(Err(e));
                    }
                }
            }
            let (_, cluster) = self.current.as_ref()?;
            return Some(self.article(item, cluster));
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.items.len()))
    }
}

/// Which entries are articles: those in the article listing (ZIM 6.1 and later), or every
/// HTML page in older files.
enum Listed {
    All,
    /// One bit per entry. The entry count is bounded by the file size (8 bytes per entry), so
    /// this is at most 1/64 of the file.
    Bits(Vec<u64>),
}

impl Listed {
    fn read(pack: &Pack) -> Result<Listed> {
        let Some(e) = pack.listing_entry()? else {
            return Ok(Listed::All);
        };
        let Some(Target::Cluster(cluster, blob)) = e.target else {
            return Ok(Listed::All);
        };
        let count = pack.zim.header.article_count;
        let mut bits = vec![0u64; (count as usize).div_ceil(64)];
        let cluster = Cluster::read(&pack.zim, cluster)?;
        let listing = cluster.blob(blob)?;
        if listing.len() % 4 != 0 {
            return Err(corrupt("article listing has a broken length"));
        }
        for raw in listing.as_chunks::<4>().0 {
            let idx = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
            if idx < count {
                bits[idx as usize / 64] |= 1 << (idx % 64);
            }
        }
        Ok(Listed::Bits(bits))
    }

    fn contains(&self, idx: u32) -> bool {
        match self {
            Listed::All => true,
            Listed::Bits(bits) => bits.get(idx as usize / 64).is_some_and(|w| w & (1 << (idx % 64)) != 0),
        }
    }
}

/// Follows a redirect chain to the article it ends at, if it ends at one.
fn final_article(zim: &Zim, listed: &Listed, mut idx: u32) -> Result<Option<u32>> {
    for _ in 0..MAX_REDIRECT_HOPS {
        let e = zim.get_by_url_index(idx).map_err(from_zim)?;
        match e.target {
            Some(Target::Redirect(next)) => idx = next,
            Some(Target::Cluster(..)) if listed.contains(idx) && is_html(&e) => return Ok(Some(idx)),
            _ => return Ok(None),
        }
    }
    // A loop in redirects only loses an alias; it's no reason to stop indexing.
    Ok(None)
}

//! Indexes every article of a Kiwix ZIM with nevo-search, then times a few searches.
//!
//! The HTML-to-text and passage splitting here are crude stand-ins for NEVO's real extractor.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::path::Path;
use std::time::Instant;

use nevo_search::{Config, Document, Filter, Passage, SearchIndex, Source};
use zim::{DirectoryEntry, MimeType, Namespace, Target, Zim};

const QUERIES: &[&str] = &[
    "heart attack",
    "oral rehydration solution",
    "how to stop severe bleeding",
    "hypothermia treatment",
    "dehydration in children",
    "diarhea",
    "snake bite first aid",
    "malaria in pregnancy",
];

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    let (Some(path), Some(dir)) = (args.get(1), args.get(2)) else {
        return Err("usage: nevo-search-bench <file.zim> <index-dir>".into());
    };
    let t0 = Instant::now();
    let zim = Zim::new(path)?;
    let articles: HashSet<u32> = zim.article_list_by_title()?.ok_or("no article list")?.to_vec()?.into_iter().collect();
    let mut by_cluster: BTreeMap<u32, Vec<(u32, String, String)>> = BTreeMap::new();
    let mut aliases: HashMap<u32, Vec<String>> = HashMap::new();
    for idx in zim.namespace_range(Namespace::UserContent)? {
        let e = zim.get_by_url_index(idx)?;
        match e.target {
            Some(Target::Redirect(to)) if articles.contains(&to) => aliases.entry(to).or_default().push(title_of(&e)),
            Some(Target::Cluster(c, blob)) if articles.contains(&idx) && is_html(&e) => {
                by_cluster.entry(c).or_default().push((blob, title_of(&e), e.url.clone()));
            }
            _ => {}
        }
    }
    let mut alias_by_url: HashMap<String, Vec<String>> = HashMap::new();
    for (idx, names) in aliases {
        alias_by_url.insert(zim.get_by_url_index(idx)?.url, names);
    }

    let _ = std::fs::remove_dir_all(dir);
    let index = SearchIndex::open(dir, Config::default())?;
    let (mut docs, mut text_bytes, mut passage_count, mut passage_bytes, mut peak_kb) = (0u64, 0, 0, 0, 0);
    for (cluster, items) in by_cluster {
        let cluster = zim.get_cluster(cluster)?;
        let guard = cluster.read()?;
        for (blob, title, url) in items {
            let html = String::from_utf8_lossy(guard.blob(blob)?).into_owned();
            let passages = passages(&html);
            let body: String = passages.iter().map(|p| p.text.as_str()).collect::<Vec<_>>().join("\n");
            text_bytes += body.len() as u64;
            passage_count += passages.len() as u64;
            passage_bytes += passages.iter().map(|p| p.text.len() as u64).sum::<u64>();
            let doc = Document {
                key: format!("zim:wikimed/{url}"),
                source: Source::Zim,
                title,
                aliases: alias_by_url.remove(&url).unwrap_or_default(),
                lang: "en".into(),
                topics: vec!["medical".into()],
                body,
            };
            index.put(&doc, &passages)?;
            docs += 1;
            if docs % 1000 == 0 {
                peak_kb = peak_kb.max(status_kb("RssAnon:").unwrap_or(0));
            }
        }
    }
    index.finish()?;
    let secs = t0.elapsed().as_secs_f64();
    let mb = |p: &str| dir_size(&Path::new(dir).join(p)).unwrap_or(0) as f64 / 1e6;
    println!("indexed {docs} articles, {passage_count} passages in {secs:.1} s");
    println!(
        "text {:.0} MB (passages {:.0} MB); index: docs {:.0} MB, passages {:.0} MB",
        text_bytes as f64 / 1e6,
        passage_bytes as f64 / 1e6,
        mb("docs"),
        mb("passages")
    );
    println!("peak heap (RssAnon, sampled) {:.0} MB", peak_kb as f64 / 1e3);

    for q in QUERIES {
        let t = Instant::now();
        let hits = index.search(q, &Filter::default(), 10)?;
        let ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let passages = index.search_passages(q, &Filter::default(), 5)?;
        let pms = t.elapsed().as_secs_f64() * 1e3;
        let top: Vec<String> = hits.iter().take(3).map(|h| format!("{}{}", h.title, if h.fuzzy { "~" } else { "" })).collect();
        let snakebite = hits.iter().position(|h| h.title == "Snakebite").map_or(String::new(), |r| format!(" [Snakebite #{}]", r + 1));
        let p = passages.first().map_or(String::new(), |p| format!("{} #{}", p.key, p.number));
        println!("{q:30} {ms:5.1} ms {pms:5.1} ms  {}{snakebite} | {p}", top.join("; "));
    }
    Ok(())
}

/// Sections split at <h2>/<h3>, each cut into passages of 500 to 1500 characters.
fn passages(html: &str) -> Vec<Passage> {
    let mut out = Vec::new();
    let mut heading = String::new();
    let mut rest = html;
    loop {
        let next = [rest.find("<h2"), rest.find("<h3")].into_iter().flatten().min();
        let (section, after) = match next {
            Some(i) => (&rest[..i], Some(&rest[i..])),
            None => (rest, None),
        };
        let mut chunk = String::new();
        for sentence in html_to_text(section).split_inclusive(". ") {
            if chunk.len() + sentence.len() > 1500 && chunk.len() >= 500 {
                out.push(Passage { heading: heading.clone(), text: std::mem::take(&mut chunk).trim().into() });
            }
            chunk.push_str(sentence);
        }
        if !chunk.trim().is_empty() {
            out.push(Passage { heading: heading.clone(), text: chunk.trim().into() });
        }
        let Some(after) = after else { break };
        let end = after.find("</h").unwrap_or(after.len());
        heading = html_to_text(&after[..end]);
        rest = &after[end..];
        if let Some(i) = rest.find('>') {
            rest = &rest[i + 1..];
        }
    }
    out
}

/// Drops tags, scripts and styles; decodes a few entities.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else { break };
        let name: String = rest[1..gt].chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        rest = &rest[gt + 1..];
        if matches!(name.to_ascii_lowercase().as_str(), "script" | "style" | "head") {
            rest = rest.find(&format!("</{name}")).map_or("", |i| &rest[i..]);
        }
        out.push(' ');
    }
    out.push_str(rest);
    let out = out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&#160;", " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title_of(e: &DirectoryEntry) -> String {
    if e.title.is_empty() { e.url.clone() } else { e.title.clone() }
}

fn is_html(e: &DirectoryEntry) -> bool {
    matches!(&e.mime_type, MimeType::Type(m) if m.starts_with("text/html"))
}

fn dir_size(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() { dir_size(&entry.path())? } else { meta.len() };
    }
    Ok(total)
}

fn status_kb(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok()
}

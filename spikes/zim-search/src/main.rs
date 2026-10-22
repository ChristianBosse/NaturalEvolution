//! NEVO ZIM spike: read Kiwix ZIM files from Rust and search them with Tantivy.
//!
//! Throwaway spike code. Answers three questions:
//! 1. Can pure Rust read real Kiwix ZIM files (articles, redirects, metadata)?
//! 2. How long does building a Tantivy index take, and how big is it?
//! 3. How fast is search, including reading snippets back out of the ZIM?
//!
//!   nevo-zim-spike info   <file.zim>
//!   nevo-zim-spike get    <file.zim> <path>
//!   nevo-zim-spike index  <file.zim> <index-dir> [heap-mb] [threads]
//!   nevo-zim-spike search <index-dir> <query...>

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::path::Path;
use std::time::Instant;

use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, Occur, Query, QueryParser, TermQuery};
use tantivy::schema::{
    IndexRecordOption, Schema, TextFieldIndexing, TextOptions, Value, STORED, STRING,
};
use tantivy::snippet::SnippetGenerator;
use tantivy::tokenizer::{
    Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, StopWordFilter, TextAnalyzer,
};
use tantivy::{Index, TantivyDocument, Term};
use zim::{DirectoryEntry, MimeType, Namespace, Target, Zim};

type Res<T> = Result<T, Box<dyn Error>>;

const SOURCE_FILE: &str = "nevo-source.txt";

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match (arg(0), arg(1), arg(2)) {
        (Some("info"), Some(zim), _) => info(zim),
        (Some("get"), Some(zim), Some(path)) => get(zim, path),
        (Some("index"), Some(zim), Some(dir)) => {
            let heap_mb = arg(3).map_or(Ok(50), str::parse)?;
            let threads = arg(4).map_or(Ok(1), str::parse)?;
            index(zim, dir, heap_mb, threads)
        }
        (Some("search"), Some(dir), Some(_)) => search(dir, &args[2..].join(" ")),
        _ => {
            eprintln!("usage: info <zim> | get <zim> <path> | index <zim> <dir> [heap-mb] [threads] | search <dir> <query>");
            std::process::exit(2);
        }
    }
}

fn info(path: &str) -> Res<()> {
    let t = Instant::now();
    let zim = Zim::new(path)?;
    let opened = t.elapsed();
    let h = &zim.header;
    println!("file:        {path}");
    println!("format:      {}.{}", h.version_major, h.version_minor);
    println!("entries:     {}", h.article_count);
    println!("clusters:    {}", h.cluster_count);
    let articles = zim.article_list_by_title()?.map(|l| l.len()).transpose()?;
    println!("articles:    {}", articles.map_or("no v1 listing".into(), |n| n.to_string()));
    for key in ["Title", "Language", "Date", "Creator", "Publisher", "Description"] {
        if let Some(c) = zim.metadata(key)? {
            println!("{:<12} {}", format!("{key}:"), String::from_utf8_lossy(&c.to_vec()?));
        }
    }
    if let Some(main) = zim.main_page()? {
        println!("main page:   {}", main.url);
    }
    println!("has Xapian full-text index: {}", zim.fulltext_index()?.is_some());
    println!("opened in {:.1} ms", opened.as_secs_f64() * 1e3);
    print_peak_memory();
    Ok(())
}

fn get(path: &str, article: &str) -> Res<()> {
    let t = Instant::now();
    let zim = Zim::new(path)?;
    let entry = find_entry(&zim, article)?.ok_or("not found")?;
    let was_redirect = matches!(entry.target, Some(Target::Redirect(_)));
    let entry = zim.resolve(entry)?;
    let html = zim.entry_content(&entry)?.ok_or("entry has no content")?.to_vec()?;
    let text = html_to_text(&html);
    let elapsed = t.elapsed();
    println!("{} ({}){}", title_of(&entry), entry.url, if was_redirect { " [via redirect]" } else { "" });
    println!("{} bytes of HTML, {} chars of text, read in {:.1} ms\n", html.len(), text.len(), elapsed.as_secs_f64() * 1e3);
    println!("{}", text.chars().take(1200).collect::<String>());
    Ok(())
}

fn index(path: &str, dir: &str, heap_mb: usize, threads: usize) -> Res<()> {
    let t0 = Instant::now();
    let zim = Zim::new(path)?;
    let articles: HashSet<u32> = zim
        .article_list_by_title()?
        .ok_or("no article listing (needs ZIM 6.1 or newer)")?
        .to_vec()?
        .into_iter()
        .collect();

    // One pass over the content namespace: articles grouped by cluster (so each cluster is
    // decompressed once) and redirect titles collected as aliases ("Heart attack" finds
    // "Myocardial infarction").
    let mut by_cluster: BTreeMap<u32, Vec<(u32, String, String)>> = BTreeMap::new();
    let mut aliases: HashMap<u32, Vec<String>> = HashMap::new();
    let (mut redirects, mut other) = (0u64, 0u64);
    let content_ns = if (zim.header.version_major, zim.header.version_minor) >= (6, 1) {
        Namespace::UserContent
    } else {
        Namespace::Articles
    };
    for idx in zim.namespace_range(content_ns)? {
        let e = zim.get_by_url_index(idx)?;
        match e.target {
            Some(Target::Redirect(to)) if articles.contains(&to) => {
                aliases.entry(to).or_default().push(title_of(&e));
                redirects += 1;
            }
            Some(Target::Cluster(c, blob)) if articles.contains(&idx) && is_html(&e) => {
                by_cluster.entry(c).or_default().push((blob, title_of(&e), e.url.clone()));
            }
            _ if articles.contains(&idx) => other += 1,
            _ => {}
        }
    }
    let scanned = t0.elapsed();

    // Map url index -> aliases by path, since we key documents by path below.
    let mut alias_by_path: HashMap<String, Vec<String>> = HashMap::new();
    for (idx, names) in aliases {
        alias_by_path.insert(zim.get_by_url_index(idx)?.url, names);
    }

    let (schema, f) = schema();
    std::fs::create_dir_all(dir)?;
    let index = Index::create_in_dir(dir, schema)?;
    register_analyzer(&index);
    let mut writer = index.writer_with_num_threads::<TantivyDocument>(threads, heap_mb * 1_000_000)?;

    let (mut docs, mut text_bytes, mut peak_anon_kb) = (0u64, 0u64, 0u64);
    for (cluster, items) in by_cluster {
        let cluster = zim.get_cluster(cluster)?;
        let guard = cluster.read()?;
        for (blob, title, url) in items {
            let text = html_to_text(guard.blob(blob)?);
            text_bytes += text.len() as u64;
            let mut doc = TantivyDocument::default();
            doc.add_text(f.key, normalize(&title));
            for alias in alias_by_path.get(&url).into_iter().flatten() {
                doc.add_text(f.key, normalize(alias));
                doc.add_text(f.aliases, alias);
            }
            doc.add_text(f.path, &url);
            doc.add_text(f.title, &title);
            doc.add_text(f.body, &text);
            writer.add_document(doc)?;
            docs += 1;
            if docs % 500 == 0 {
                peak_anon_kb = peak_anon_kb.max(status_kb("RssAnon:").unwrap_or(0));
            }
        }
    }
    writer.commit()?;
    writer.wait_merging_threads()?;
    std::fs::write(Path::new(dir).join(SOURCE_FILE), std::fs::canonicalize(path)?.to_string_lossy().as_bytes())?;

    let secs = t0.elapsed().as_secs_f64();
    let zim_mb = std::fs::metadata(path)?.len() as f64 / 1e6;
    let index_mb = dir_size(Path::new(dir))? as f64 / 1e6;
    println!("indexed {docs} articles ({:.0} MB of text) in {secs:.1} s ({:.0} articles/s)", text_bytes as f64 / 1e6, docs as f64 / secs);
    println!("listing has {} entries: {docs} articles, {redirects} redirects kept as aliases, {other} other", articles.len());
    println!("scan of entries took {:.1} s; writer heap {heap_mb} MB, {threads} thread(s)", scanned.as_secs_f64());
    println!("peak heap memory (RssAnon, sampled): {:.0} MB; the rest of RSS is the memory-mapped ZIM, which the OS can drop", peak_anon_kb as f64 / 1e3);
    println!("ZIM {zim_mb:.0} MB -> index {index_mb:.0} MB ({:.0}% of the ZIM)", 100.0 * index_mb / zim_mb);
    print_peak_memory();
    Ok(())
}

fn search(dir: &str, query: &str) -> Res<()> {
    let t0 = Instant::now();
    let index = Index::open_in_dir(dir)?;
    register_analyzer(&index);
    let reader = index.reader()?;
    let searcher = reader.searcher();
    let (_, f) = schema();
    let zim = Zim::new(std::fs::read_to_string(Path::new(dir).join(SOURCE_FILE))?.trim())?;
    let opened = t0.elapsed();

    let t1 = Instant::now();
    let mut parser = QueryParser::for_index(&index, vec![f.title, f.aliases, f.body]);
    parser.set_field_boost(f.title, 3.0);
    parser.set_field_boost(f.aliases, 2.0);
    let (parsed, errors) = parser.parse_query_lenient(query);
    if !errors.is_empty() {
        println!("(query warnings: {errors:?})");
    }
    // A query that is exactly an article title or redirect ("heart attack") goes first.
    let exact = TermQuery::new(Term::from_field_text(f.key, &normalize(query)), IndexRecordOption::Basic);
    let query: Box<dyn Query> = Box::new(BooleanQuery::new(vec![
        (Occur::Should, parsed),
        (Occur::Should, Box::new(BoostQuery::new(Box::new(exact), 20.0))),
    ]));
    let hits = searcher.search(&query, &TopDocs::with_limit(10).order_by_score())?;
    let searched = t1.elapsed();

    // Snippets come from the ZIM itself: the index stores only paths and titles.
    let t2 = Instant::now();
    let snippets = SnippetGenerator::create(&searcher, &*query, f.body)?;
    for (score, addr) in &hits {
        let d: TantivyDocument = searcher.doc(*addr)?;
        let get = |field| d.get_first(field).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let (path, title) = (get(f.path), get(f.title));
        let text = match find_entry(&zim, &path)? {
            Some(e) => match zim.entry_content(&zim.resolve(e)?)? {
                Some(c) => html_to_text(&c.to_vec()?),
                None => String::new(),
            },
            None => String::new(),
        };
        let snippet = snippets.snippet(&text).to_html().replace("<b>", "**").replace("</b>", "**");
        println!("{score:6.2}  {title}  ({path})\n        {}", snippet.replace('\n', " "));
    }
    let snippeted = t2.elapsed();
    println!(
        "\n{} hits; open {:.0} ms, search {:.1} ms, read + snippets {:.0} ms",
        hits.len(),
        opened.as_secs_f64() * 1e3,
        searched.as_secs_f64() * 1e3,
        snippeted.as_secs_f64() * 1e3
    );
    print_peak_memory();
    Ok(())
}

struct Fields {
    key: tantivy::schema::Field,
    path: tantivy::schema::Field,
    title: tantivy::schema::Field,
    aliases: tantivy::schema::Field,
    body: tantivy::schema::Field,
}

const ANALYZER: &str = "en_nevo";

/// English words, lowercased, stop words ("how", "to", "the") dropped, stemmed.
fn register_analyzer(index: &Index) {
    let analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(40))
        .filter(LowerCaser)
        .filter(StopWordFilter::new(Language::English).expect("English stop words"))
        .filter(Stemmer::new(Language::English))
        .build();
    index.tokenizers().register(ANALYZER, analyzer);
}

/// Exact-match key for titles and redirects: lowercase, underscores as spaces, single spaces.
fn normalize(s: &str) -> String {
    s.replace('_', " ").to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Paths and titles are stored; the body is indexed only and read back from the ZIM.
/// `key` holds the normalized title and every redirect, for exact matches.
fn schema() -> (Schema, Fields) {
    let english = |stored: bool| {
        let o = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(ANALYZER)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        );
        if stored { o.set_stored() } else { o }
    };
    let mut b = Schema::builder();
    let f = Fields {
        key: b.add_text_field("key", STRING),
        path: b.add_text_field("path", STRING | STORED),
        title: b.add_text_field("title", english(true)),
        aliases: b.add_text_field("aliases", english(false)),
        body: b.add_text_field("body", english(false)),
    };
    (b.build(), f)
}

fn find_entry(zim: &Zim, path: &str) -> Res<Option<DirectoryEntry>> {
    for ns in [Namespace::UserContent, Namespace::Articles] {
        if let Some(e) = zim.get_by_path(ns, path)? {
            return Ok(Some(e));
        }
    }
    Ok(None)
}

fn title_of(e: &DirectoryEntry) -> String {
    if e.title.is_empty() { e.url.clone() } else { e.title.clone() }
}

fn is_html(e: &DirectoryEntry) -> bool {
    matches!(&e.mime_type, MimeType::Type(m) if m.starts_with("text/html"))
}

/// Crude HTML to text: drops tags, comments, scripts and styles, decodes common entities.
/// Good enough to measure indexing; NEVO will want a proper extractor (headings, lists, tables).
fn html_to_text(html: &[u8]) -> String {
    let s = String::from_utf8_lossy(html);
    let mut out = String::with_capacity(s.len() / 2);
    let mut rest: &str = &s;
    while let Some(lt) = rest.find('<') {
        push_decoded(&mut out, &rest[..lt]);
        rest = &rest[lt..];
        if let Some(body) = rest.strip_prefix("<!--") {
            rest = body.find("-->").map_or("", |i| &body[i + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let name: String = rest[1..gt]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        rest = &rest[gt + 1..];
        if matches!(name.as_str(), "script" | "style" | "head" | "noscript") {
            let close = format!("</{name}");
            rest = rest.find(&close).map_or("", |i| &rest[i..]);
        }
        out.push(' ');
    }
    push_decoded(&mut out, rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_decoded(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..].find(';').filter(|&i| i <= 10).and_then(|i| {
            let ent = &rest[1..=i];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                _ => ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, i + 2))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
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

/// Peak resident memory of this process (Linux only).
fn print_peak_memory() {
    match status_kb("VmHWM:") {
        Some(kb) => println!("peak memory (RSS): {:.0} MB", kb as f64 / 1e3),
        None => println!("peak memory: n/a on this OS"),
    }
}

/// A kB value from /proc/self/status (Linux only).
fn status_kb(key: &str) -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok()
}

//! NEVO search: one keyword index over the local library and Kiwix ZIM packs.
//!
//! The caller extracts plain text (from a guide, a ZIM article, a PDF) and hands it in as a
//! [`Document`] plus its [`Passage`]s. Searching returns keys and the few stored fields a result
//! list needs; the caller reads the text back from where it lives (D-036) and can build a
//! [`snippet`] from it. Passage hits carry their text, so the AI can quote them (D-037).
//!
//! Layout of an index folder:
//!
//! ```text
//! VERSION     format version of this crate's schema
//! docs/       Tantivy index of whole documents, for the result list
//! passages/   Tantivy index of passages, for the AI
//! ```
//!
//! ## Why two Tantivy indexes
//!
//! BM25 scores depend on the average field length and the number of entries in the index.
//! Mixing whole articles and their passages in one index would skew both, so each gets its
//! own. They are committed passages first, so a document that shows up in search always has
//! its passages. If the program dies between the two commits, putting the document again fixes
//! it ([`SearchIndex::put`] replaces by key).
//!
//! ## Languages: one set of fields per language group
//!
//! Tantivy picks the analyzer per field, not per document. So each language group (English,
//! French, Spanish, Portuguese, Arabic, and "other" without stemming) has its own title, alias
//! and body fields, and a document's text goes into the fields of its `lang`. A query is run
//! through every group's analyzer against that group's fields, so the searcher doesn't need to
//! know the query's language, and each language keeps its own term statistics. The cost is
//! more (mostly empty) fields per query, which Tantivy skips cheaply.
//!
//! The alternative, one stemmed field plus one unstemmed field, can't work without knowing the
//! query's language: the query would have to be stemmed one way for every document.
//!
//! Languages outside the groups (Swahili, Hindi, …) are lowercased and accent-folded but not
//! stemmed. Scripts written without spaces (Chinese, Japanese, Thai) need a word segmenter,
//! which this crate doesn't have yet.
//!
//! ## Ranking
//!
//! 1. Stop words are dropped and words stemmed, per language group.
//! 2. A query that is exactly a title or alias ("heart attack" for *Myocardial infarction*) gets
//!    a large constant score, so it comes first.
//! 3. Title matches weigh 3, alias matches 2, body matches 1 (BM25 underneath).
//! 4. Two adjacent query words are also tried joined against titles and aliases, so "snake
//!    bite" finds *Snakebite*.
//! 5. If a query of a few words finds fewer than [`Config::fuzzy_below`] hits, its words of 5 or
//!    more letters are retried against titles and aliases with one typo allowed. Those hits come
//!    after the normal ones and are marked [`Hit::fuzzy`].

mod analysis;
mod error;
mod snippet;

use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::{
    Bm25StatisticsProvider, BooleanQuery, BoostQuery, ConstScoreQuery, FuzzyTermQuery, Occur, Query, QueryParser,
    TermQuery,
};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, SchemaBuilder, TextFieldIndexing, TextOptions, Value, STORED, STRING,
};
use tantivy::{DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy, Score, Searcher, TantivyDocument, Term};

use analysis::{Analyzers, GROUPS};

pub use error::{Result, SearchError};
pub use snippet::snippet;

/// Version of the on-disk schema. An index with another version must be rebuilt.
pub const FORMAT_VERSION: u32 = 1;

const VERSION_FILE: &str = "VERSION";
const DOCS_DIR: &str = "docs";
const PASSAGES_DIR: &str = "passages";

const TITLE_BOOST: Score = 3.0;
const ALIAS_BOOST: Score = 2.0;
/// Passage context is the document title plus the section heading.
const CONTEXT_BOOST: Score = 2.0;
/// A joined-word match ("snake bite" -> "Snakebite") stands for two query words.
const JOINED_BOOST: Score = 2.0;
/// Far above any BM25 score, so exact title and alias matches always come first.
const EXACT_SCORE: Score = 1000.0;

/// Tantivy's minimum writer memory per thread (not exported by Tantivy).
const TANTIVY_MIN_WRITER_MEMORY: usize = 15_000_000;

/// Longer queries are cut: nobody types them, and each word costs a lookup per field.
const MAX_QUERY_CHARS: usize = 500;
/// Joined-word matches are tried for this many leading word pairs.
const MAX_JOINED_PAIRS: usize = 6;
/// Typo matches are tried only for queries of at most this many words…
const FUZZY_MAX_WORDS: usize = 5;
/// …and only for words at least this long: one typo in a short word is another word.
const FUZZY_MIN_CHARS: usize = 5;

/// Where a document comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A library item (a guide someone added).
    Library,
    /// An article in a Kiwix ZIM pack.
    Zim,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Library => "library",
            Source::Zim => "zim",
        }
    }

    pub fn from_name(name: &str) -> Option<Source> {
        match name {
            "library" => Some(Source::Library),
            "zim" => Some(Source::Zim),
            _ => None,
        }
    }
}

/// A document to index. Only `key`, `title`, `source`, `lang` and `topics` are stored.
#[derive(Debug, Clone)]
pub struct Document {
    /// Unique: an item ID, or "zim:<pack>/<path>".
    pub key: String,
    pub source: Source,
    pub title: String,
    /// Other names, such as ZIM redirect titles.
    pub aliases: Vec<String>,
    /// BCP 47 tag ("en", "fr-CA"). ISO 639-3 codes ("eng") are accepted too.
    pub lang: String,
    /// Topic paths, such as "medical/first-aid".
    pub topics: Vec<String>,
    pub body: String,
}

/// A piece of a document for the AI: a section heading plus a few paragraphs, about 500 to
/// 1500 characters.
#[derive(Debug, Clone)]
pub struct Passage {
    pub heading: String,
    pub text: String,
}

/// What to search. Empty means everything.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub source: Option<Source>,
    /// Any of these topics, each including its sub-topics: "medical" matches
    /// "medical/first-aid" but not "medical-x". Empty paths are ignored.
    pub topics: Vec<String>,
    /// Only this language. Region and script are ignored: "fr" and "fr-CA" both match every
    /// French document.
    pub lang: Option<String>,
}

/// A document found by [`SearchIndex::search`].
#[derive(Debug, Clone)]
pub struct Hit {
    pub key: String,
    pub title: String,
    pub source: Source,
    pub lang: String,
    pub topics: Vec<String>,
    /// For ordering only; not comparable between queries.
    pub score: Score,
    /// Found only by allowing a typo.
    pub fuzzy: bool,
}

/// A passage found by [`SearchIndex::search_passages`].
#[derive(Debug, Clone)]
pub struct PassageHit {
    pub key: String,
    /// The passage's position in the list given to [`SearchIndex::put`], from 0.
    pub number: u32,
    pub heading: String,
    pub text: String,
    pub score: Score,
    pub fuzzy: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Total memory for buffering writes, split evenly between the two indexes. Tantivy needs
    /// at least 15 MB per index and thread. Default 50 MB.
    pub writer_memory: usize,
    /// Indexing threads per index. Default 1: a Pi has other work to do.
    pub writer_threads: usize,
    /// Typo matching runs when a query finds fewer hits than this. 0 turns it off. Default 3.
    pub fuzzy_below: usize,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            writer_memory: 50_000_000,
            writer_threads: 1,
            fuzzy_below: 3,
        }
    }
}

impl Config {
    fn check(&self) -> Result<()> {
        if !(1..=8).contains(&self.writer_threads) {
            return Err(SearchError::Config("writer threads must be 1 to 8".into()));
        }
        let min = 2 * self.writer_threads * TANTIVY_MIN_WRITER_MEMORY;
        if self.writer_memory < min {
            return Err(SearchError::Config(format!(
                "writer memory must be at least {} MB for {} thread(s)",
                min / 1_000_000,
                self.writer_threads
            )));
        }
        Ok(())
    }
}

/// The search index: whole documents for the result list, and passages for the AI.
///
/// Safe to share between threads. Writes are buffered until [`SearchIndex::commit`]; searches
/// see the last commit. The writers (and their memory) are created on the first write, so a
/// node that only searches never takes the write lock.
pub struct SearchIndex {
    docs: Part,
    passages: Part,
    d: DocFields,
    p: PassageFields,
    config: Config,
}

impl SearchIndex {
    /// Opens the index in `dir`, creating it if needed.
    pub fn open(dir: impl AsRef<Path>, config: Config) -> Result<SearchIndex> {
        config.check()?;
        let dir = dir.as_ref();
        let version_file = dir.join(VERSION_FILE);
        match fs::read_to_string(&version_file) {
            Ok(v) if v.trim() == FORMAT_VERSION.to_string() => {}
            Ok(v) => return Err(SearchError::Version(v.trim().to_string())),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                if dir.join(DOCS_DIR).exists() || dir.join(PASSAGES_DIR).exists() {
                    return Err(SearchError::Version("unknown".into()));
                }
                // Written first: a folder with index data but no version file is unknown.
                fs::create_dir_all(dir)?;
                fs::write(&version_file, format!("{FORMAT_VERSION}\n"))?;
            }
            Err(e) => return Err(e.into()),
        }
        let open = |name: &str, schema: Schema| -> Result<Index> {
            let path = dir.join(name);
            fs::create_dir_all(&path)?;
            Ok(Index::open_or_create(MmapDirectory::open(path)?, schema)?)
        };
        let (doc_schema, d) = doc_schema();
        let (passage_schema, p) = passage_schema();
        let docs = open(DOCS_DIR, doc_schema)?;
        let passages = open(PASSAGES_DIR, passage_schema)?;
        SearchIndex::build(docs, d, passages, p, config)
    }

    /// An index kept in memory only, for tests and small temporary collections.
    pub fn in_memory(config: Config) -> Result<SearchIndex> {
        config.check()?;
        let (doc_schema, d) = doc_schema();
        let (passage_schema, p) = passage_schema();
        SearchIndex::build(
            Index::create_in_ram(doc_schema),
            d,
            Index::create_in_ram(passage_schema),
            p,
            config,
        )
    }

    fn build(docs: Index, d: DocFields, passages: Index, p: PassageFields, config: Config) -> Result<SearchIndex> {
        let doc_parser = d
            .groups
            .iter()
            .enumerate()
            .flat_map(|(i, g)| [(g.title, TITLE_BOOST, i), (g.aliases, ALIAS_BOOST, i), (g.body, 1.0, i)])
            .collect();
        let doc_names = (d.groups.iter().enumerate())
            .flat_map(|(i, g)| [(g.title, i, TITLE_BOOST), (g.aliases, i, ALIAS_BOOST)])
            .collect();
        let passage_parser = (p.groups.iter())
            .enumerate()
            .flat_map(|(i, g)| [(g.context, CONTEXT_BOOST, i), (g.text, 1.0, i)])
            .collect();
        let passage_names = (p.groups.iter().enumerate())
            .map(|(i, g)| (g.context, i, CONTEXT_BOOST))
            .collect();
        Ok(SearchIndex {
            docs: Part::new(docs, d.common, doc_parser, doc_names)?,
            passages: Part::new(passages, p.common, passage_parser, passage_names)?,
            d,
            p,
            config,
        })
    }

    /// Adds a document and its passages, replacing any document with the same key. Visible to
    /// searches after the next [`SearchIndex::commit`].
    pub fn put(&self, doc: &Document, passages: &[Passage]) -> Result<()> {
        if doc.key.is_empty() {
            return Err(SearchError::InvalidDocument("empty key".into()));
        }
        if u32::try_from(passages.len()).is_err() {
            return Err(SearchError::InvalidDocument("too many passages".into()));
        }
        let group = analysis::group_of(&doc.lang);
        let lang_key = analysis::lang_key(&doc.lang);
        let topic_terms: Vec<String> = {
            let mut t: Vec<String> = doc.topics.iter().flat_map(|t| analysis::topic_terms(t)).collect();
            t.sort();
            t.dedup();
            t
        };
        let exact_keys: Vec<String> = {
            let mut k: Vec<String> = std::iter::once(&doc.title)
                .chain(&doc.aliases)
                .map(|t| analysis::exact_key(t))
                .filter(|k| !k.is_empty())
                .collect();
            k.sort();
            k.dedup();
            k
        };
        let common = |c: &Common, out: &mut TantivyDocument| {
            out.add_text(c.key, &doc.key);
            out.add_text(c.source, doc.source.as_str());
            out.add_text(c.lang_key, &lang_key);
            for t in &topic_terms {
                out.add_text(c.topic_filter, t);
            }
            for k in &exact_keys {
                out.add_text(c.exact, k);
            }
            out.add_text(c.group, GROUPS[group].name);
        };

        // Lock order is always passages, then docs (see `commit`).
        let passage_writer = self.passages.writer(&self.config)?;
        let doc_writer = self.docs.writer(&self.config)?;
        let (passage_writer, doc_writer) = (writer_of(&passage_writer), writer_of(&doc_writer));

        let key_term = |c: &Common| Term::from_field_text(c.key, &doc.key);
        // Tantivy applies a delete only to documents added before it, so delete-then-add in
        // one commit replaces.
        passage_writer.delete_term(key_term(&self.p.common));
        doc_writer.delete_term(key_term(&self.d.common));

        let pg = &self.p.groups[group];
        for (n, passage) in passages.iter().enumerate() {
            let mut out = TantivyDocument::default();
            common(&self.p.common, &mut out);
            out.add_u64(self.p.number, n as u64);
            out.add_text(self.p.heading, &passage.heading);
            out.add_text(self.p.text, &passage.text);
            out.add_text(pg.context, &doc.title);
            out.add_text(pg.context, &passage.heading);
            out.add_text(pg.text, &passage.text);
            passage_writer.add_document(out)?;
        }

        let dg = &self.d.groups[group];
        let mut out = TantivyDocument::default();
        common(&self.d.common, &mut out);
        out.add_text(self.d.title, &doc.title);
        out.add_text(self.d.lang, &doc.lang);
        for t in &doc.topics {
            out.add_text(self.d.topics, t);
        }
        out.add_text(dg.title, &doc.title);
        for alias in &doc.aliases {
            out.add_text(dg.aliases, alias);
        }
        out.add_text(dg.body, &doc.body);
        doc_writer.add_document(out)?;
        Ok(())
    }

    /// Removes a document and its passages. Takes effect at the next commit.
    pub fn delete(&self, key: &str) -> Result<()> {
        let passage_writer = self.passages.writer(&self.config)?;
        let doc_writer = self.docs.writer(&self.config)?;
        writer_of(&passage_writer).delete_term(Term::from_field_text(self.p.common.key, key));
        writer_of(&doc_writer).delete_term(Term::from_field_text(self.d.common.key, key));
        Ok(())
    }

    /// Makes all writes so far durable and visible to searches.
    pub fn commit(&self) -> Result<()> {
        self.passages.commit(false)?;
        self.docs.commit(false)
    }

    /// Commits, waits for background segment merges to finish and frees the writers' memory.
    /// Call it at the end of a bulk import; the next write opens new writers.
    pub fn finish(&self) -> Result<()> {
        self.passages.commit(true)?;
        self.docs.commit(true)
    }

    /// Number of documents visible to searches.
    pub fn num_docs(&self) -> u64 {
        self.docs.reader.searcher().num_docs()
    }

    /// Number of passages visible to searches.
    pub fn num_passages(&self) -> u64 {
        self.passages.reader.searcher().num_docs()
    }

    /// The best `limit` documents for a query. Never fails on odd user input: unknown syntax
    /// is ignored.
    pub fn search(&self, query: &str, filter: &Filter, limit: usize) -> Result<Vec<Hit>> {
        let (searcher, found) = self.docs.run(query, filter, limit, self.config.fuzzy_below)?;
        let d = &self.d;
        let mut hits = found
            .into_iter()
            .map(|f| {
                let doc: TantivyDocument = searcher.doc(f.addr)?;
                let source = Source::from_name(&text(&doc, d.common.source)).unwrap_or(Source::Library);
                Ok(Hit {
                    key: text(&doc, d.common.key),
                    title: text(&doc, d.title),
                    source,
                    lang: text(&doc, d.lang),
                    topics: doc
                        .get_all(d.topics)
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    score: f.score,
                    fuzzy: f.fuzzy,
                })
            })
            .collect::<Result<Vec<Hit>>>()?;
        rank_typos_by_title(query, &mut hits);
        Ok(hits)
    }

    /// The best `limit` passages for a query, for the AI to read and quote.
    pub fn search_passages(&self, query: &str, filter: &Filter, limit: usize) -> Result<Vec<PassageHit>> {
        let (searcher, found) = self.passages.run(query, filter, limit, self.config.fuzzy_below)?;
        let p = &self.p;
        found
            .into_iter()
            .map(|f| {
                let doc: TantivyDocument = searcher.doc(f.addr)?;
                let number = doc.get_first(p.number).and_then(|v| v.as_u64()).unwrap_or(0);
                Ok(PassageHit {
                    key: text(&doc, p.common.key),
                    number: u32::try_from(number).unwrap_or(u32::MAX),
                    heading: text(&doc, p.heading),
                    text: text(&doc, p.text),
                    score: f.score,
                    fuzzy: f.fuzzy,
                })
            })
            .collect()
    }
}

/// Typo matches all score alike (Tantivy gives them a constant score), so order them by how
/// many query words are one typo away from a word of the title: "diarhea" puts *Diarrhea*
/// before *Diarrhée*.
fn rank_typos_by_title(query: &str, hits: &mut [Hit]) {
    let first = hits.iter().position(|h| h.fuzzy).unwrap_or(hits.len());
    let words: Vec<Vec<char>> = (analysis::exact_key(query).split(' '))
        .filter(|w| w.chars().count() >= FUZZY_MIN_CHARS)
        .map(|w| w.chars().collect())
        .collect();
    let close = |title: &str| {
        let title: Vec<Vec<char>> = analysis::exact_key(title)
            .split(' ')
            .map(|w| w.chars().collect())
            .collect();
        words
            .iter()
            .filter(|w| title.iter().any(|t| snippet::within_one_edit(w, t)))
            .count()
    };
    // Stable: equal titles keep the index's order.
    hits[first..].sort_by_cached_key(|h| std::cmp::Reverse(close(&h.title)));
}

fn text(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// Schema

/// Fields both indexes have: the key and what filters and exact matches need.
#[derive(Clone, Copy)]
struct Common {
    key: Field,
    source: Field,
    lang_key: Field,
    /// Each topic and its parents.
    topic_filter: Field,
    /// Exact-match keys of the title and aliases (see [`analysis::exact_key`]).
    exact: Field,
    /// The language group's name, to count documents per group (see [`GroupStats`]).
    group: Field,
}

struct DocFields {
    common: Common,
    title: Field,
    lang: Field,
    topics: Field,
    /// One set per language group, in the order of [`GROUPS`].
    groups: Vec<DocGroupFields>,
}

struct DocGroupFields {
    title: Field,
    aliases: Field,
    body: Field,
}

struct PassageFields {
    common: Common,
    number: Field,
    heading: Field,
    text: Field,
    groups: Vec<PassageGroupFields>,
}

struct PassageGroupFields {
    /// Document title and passage heading.
    context: Field,
    text: Field,
}

fn common_fields(b: &mut SchemaBuilder) -> Common {
    Common {
        key: b.add_text_field("key", STRING | STORED),
        source: b.add_text_field("source", STRING | STORED),
        lang_key: b.add_text_field("lang_key", STRING),
        topic_filter: b.add_text_field("topic_filter", STRING),
        exact: b.add_text_field("exact", STRING),
        group: b.add_text_field("group", STRING),
    }
}

/// Short fields keep positions, so quoted phrases work on titles. Long text skips them: they
/// would make the index about a third bigger, and ranking doesn't use them.
fn analyzed(group: usize, positions: bool) -> TextOptions {
    let record = if positions {
        IndexRecordOption::WithFreqsAndPositions
    } else {
        IndexRecordOption::WithFreqs
    };
    TextOptions::default().set_indexing_options(
        TextFieldIndexing::default()
            .set_tokenizer(&analysis::analyzer_name(group))
            .set_index_option(record),
    )
}

fn doc_schema() -> (Schema, DocFields) {
    let mut b = Schema::builder();
    let common = common_fields(&mut b);
    let title = b.add_text_field("title", STORED);
    let lang = b.add_text_field("lang", STORED);
    let topics = b.add_text_field("topics", STORED);
    let groups = (GROUPS.iter().enumerate())
        .map(|(i, g)| DocGroupFields {
            title: b.add_text_field(&format!("title_{}", g.name), analyzed(i, true)),
            aliases: b.add_text_field(&format!("aliases_{}", g.name), analyzed(i, true)),
            body: b.add_text_field(&format!("body_{}", g.name), analyzed(i, false)),
        })
        .collect();
    let fields = DocFields {
        common,
        title,
        lang,
        topics,
        groups,
    };
    (b.build(), fields)
}

fn passage_schema() -> (Schema, PassageFields) {
    let mut b = Schema::builder();
    let common = common_fields(&mut b);
    let number = b.add_u64_field("number", STORED);
    let heading = b.add_text_field("heading", STORED);
    let text = b.add_text_field("text", STORED);
    let groups = (GROUPS.iter().enumerate())
        .map(|(i, g)| PassageGroupFields {
            context: b.add_text_field(&format!("context_{}", g.name), analyzed(i, true)),
            text: b.add_text_field(&format!("text_{}", g.name), analyzed(i, false)),
        })
        .collect();
    let fields = PassageFields {
        common,
        number,
        heading,
        text,
        groups,
    };
    (b.build(), fields)
}

// ---------------------------------------------------------------------------------------------
// One Tantivy index and how to query it

struct Part {
    index: Index,
    reader: IndexReader,
    writer: Mutex<Option<IndexWriter>>,
    parser: QueryParser,
    common: Common,
    /// Title-like fields for joined-word and typo matches: (field, language group, boost).
    names: Vec<(Field, usize, Score)>,
    /// The language group of each analyzed field.
    field_group: HashMap<Field, usize>,
}

struct Found {
    score: Score,
    addr: DocAddress,
    fuzzy: bool,
}

fn writer_of<'a>(guard: &'a MutexGuard<'_, Option<IndexWriter>>) -> &'a IndexWriter {
    guard.as_ref().expect("Part::writer always fills the slot")
}

impl Part {
    fn new(
        index: Index,
        common: Common,
        parser_fields: Vec<(Field, Score, usize)>,
        names: Vec<(Field, usize, Score)>,
    ) -> Result<Part> {
        analysis::register(index.tokenizers());
        // Reloaded by hand after each commit: the automatic policy needs a file watcher thread.
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        let mut parser = QueryParser::for_index(&index, parser_fields.iter().map(|&(f, _, _)| f).collect());
        for &(field, boost, _) in &parser_fields {
            parser.set_field_boost(field, boost);
        }
        let field_group = parser_fields.iter().map(|&(f, _, g)| (f, g)).collect();
        Ok(Part {
            index,
            reader,
            writer: Mutex::new(None),
            parser,
            common,
            names,
            field_group,
        })
    }

    fn lock(&self) -> MutexGuard<'_, Option<IndexWriter>> {
        // A panic while holding the lock leaves only uncommitted buffered writes behind, which a
        // later commit either keeps or a crash drops. Either way the index stays valid.
        self.writer.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn writer(&self, config: &Config) -> Result<MutexGuard<'_, Option<IndexWriter>>> {
        let mut guard = self.lock();
        if guard.is_none() {
            let memory = config.writer_memory / 2;
            *guard = Some(self.index.writer_with_num_threads(config.writer_threads, memory)?);
        }
        Ok(guard)
    }

    fn commit(&self, close: bool) -> Result<()> {
        let mut guard = self.lock();
        if close {
            if let Some(mut writer) = guard.take() {
                writer.commit()?;
                writer.wait_merging_threads()?;
            }
        } else if let Some(writer) = guard.as_mut() {
            writer.commit()?;
        }
        self.reader.reload()?;
        Ok(())
    }

    fn run(&self, query: &str, filter: &Filter, limit: usize, fuzzy_below: usize) -> Result<(Searcher, Vec<Found>)> {
        let searcher = self.reader.searcher();
        let mut found = Vec::new();
        if limit == 0 {
            return Ok((searcher, found));
        }
        let query = match query.char_indices().nth(MAX_QUERY_CHARS) {
            Some((i, _)) => &query[..i],
            None => query,
        };
        let mut analyzers = Analyzers::new();
        let stats = GroupStats::new(&searcher, &self.field_group, self.common.group)?;
        let main = self.filtered(self.main_query(query, &mut analyzers), filter);
        let top = TopDocs::with_limit(limit).order_by_score();
        for (score, addr) in searcher.search_with_statistics_provider(&*main, &top, &stats)? {
            found.push(Found {
                score,
                addr,
                fuzzy: false,
            });
        }
        if found.len() < limit.min(fuzzy_below) {
            if let Some(fuzzy) = self.fuzzy_query(query, &mut analyzers) {
                let fuzzy = self.filtered(fuzzy, filter);
                let top = TopDocs::with_limit(limit + found.len()).order_by_score();
                let more = searcher.search_with_statistics_provider(&*fuzzy, &top, &stats)?;
                for (score, addr) in more {
                    if found.len() == limit {
                        break;
                    }
                    if !found.iter().any(|f| f.addr == addr) {
                        found.push(Found {
                            score,
                            addr,
                            fuzzy: true,
                        });
                    }
                }
            }
        }
        Ok((searcher, found))
    }

    fn main_query(&self, query: &str, analyzers: &mut Analyzers) -> Box<dyn Query> {
        let (parsed, _unknown_syntax) = self.parser.parse_query_lenient(query);
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = vec![(Occur::Should, parsed)];

        let key = analysis::exact_key(query);
        if !key.is_empty() {
            let exact = TermQuery::new(Term::from_field_text(self.common.exact, &key), IndexRecordOption::Basic);
            clauses.push((
                Occur::Should,
                Box::new(ConstScoreQuery::new(Box::new(exact), EXACT_SCORE)),
            ));
        }

        // "snake bite" -> "snakebite": titles often write as one word what people type as two.
        let words = analysis::words(query);
        for pair in words.windows(2).take(MAX_JOINED_PAIRS) {
            let joined = format!("{}{}", pair[0], pair[1]);
            for &(field, group, boost) in &self.names {
                if let Some(term) = analyzers.term(group, &joined) {
                    let q = TermQuery::new(Term::from_field_text(field, &term), IndexRecordOption::WithFreqs);
                    clauses.push((
                        Occur::Should,
                        Box::new(BoostQuery::new(Box::new(q), boost * JOINED_BOOST)),
                    ));
                }
            }
        }
        Box::new(BooleanQuery::new(clauses))
    }

    /// Words of 5 or more letters against title-like fields with one typo allowed. Only for
    /// short queries: each word costs a scan of every title term dictionary.
    fn fuzzy_query(&self, query: &str, analyzers: &mut Analyzers) -> Option<Box<dyn Query>> {
        let words = analysis::words(query);
        if words.len() > FUZZY_MAX_WORDS {
            return None;
        }
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        for word in words.iter().filter(|w| w.chars().count() >= FUZZY_MIN_CHARS) {
            for &(field, group, boost) in &self.names {
                // Stemmed like the index, so "bleding" (-> "bled") can reach "bleed".
                if let Some(term) = analyzers.term(group, word) {
                    let q = FuzzyTermQuery::new(Term::from_field_text(field, &term), 1, true);
                    clauses.push((Occur::Should, Box::new(BoostQuery::new(Box::new(q), boost))));
                }
            }
        }
        (!clauses.is_empty()).then(|| Box::new(BooleanQuery::new(clauses)) as Box<dyn Query>)
    }

    fn filtered(&self, query: Box<dyn Query>, filter: &Filter) -> Box<dyn Query> {
        let term = |field: Field, value: &str| -> Box<dyn Query> {
            Box::new(TermQuery::new(
                Term::from_field_text(field, value),
                IndexRecordOption::Basic,
            ))
        };
        // Filters must not change the score, only whether a document matches.
        let mut must: Vec<Box<dyn Query>> = Vec::new();
        if let Some(source) = filter.source {
            must.push(term(self.common.source, source.as_str()));
        }
        if let Some(lang) = &filter.lang {
            must.push(term(self.common.lang_key, &analysis::lang_key(lang)));
        }
        let topics: Vec<String> = filter.topics.iter().filter_map(|t| analysis::topic_path(t)).collect();
        if !topics.is_empty() {
            let any = topics
                .iter()
                .map(|t| (Occur::Should, term(self.common.topic_filter, t)))
                .collect();
            must.push(Box::new(BooleanQuery::new(any)));
        }
        if must.is_empty() {
            return query;
        }
        let mut clauses = vec![(Occur::Must, query)];
        for q in must {
            clauses.push((Occur::Must, Box::new(ConstScoreQuery::new(q, 0.0)) as Box<dyn Query>));
        }
        Box::new(BooleanQuery::new(clauses))
    }
}

/// BM25 statistics with field lengths averaged per language group.
///
/// Tantivy averages a field's length over all documents in the index, including those of other
/// languages, which leave the field empty. A French title would then look many times longer
/// than the "average" French title, and French documents would lose to English ones on length
/// alone. Dividing by the number of documents in the field's group fixes that.
struct GroupStats<'a> {
    searcher: &'a Searcher,
    field_group: &'a HashMap<Field, usize>,
    group_docs: Vec<u64>,
    total_docs: u64,
}

impl<'a> GroupStats<'a> {
    fn new(searcher: &'a Searcher, field_group: &'a HashMap<Field, usize>, group: Field) -> Result<GroupStats<'a>> {
        let group_docs = (GROUPS.iter())
            .map(|g| searcher.doc_freq(&Term::from_field_text(group, g.name)))
            .collect::<tantivy::Result<_>>()?;
        Ok(GroupStats {
            searcher,
            field_group,
            group_docs,
            total_docs: Bm25StatisticsProvider::total_num_docs(searcher)?,
        })
    }
}

impl Bm25StatisticsProvider for GroupStats<'_> {
    fn total_num_tokens(&self, field: Field) -> tantivy::Result<u64> {
        let tokens = self.searcher.total_num_tokens(field)?;
        Ok(match self.field_group.get(&field).map(|&g| self.group_docs[g]) {
            Some(docs) if docs > 0 => {
                let scaled = u128::from(tokens) * u128::from(self.total_docs) / u128::from(docs);
                u64::try_from(scaled).unwrap_or(u64::MAX)
            }
            _ => tokens,
        })
    }

    fn total_num_docs(&self) -> tantivy::Result<u64> {
        Ok(self.total_docs)
    }

    fn doc_freq(&self, term: &Term) -> tantivy::Result<u64> {
        self.searcher.doc_freq(term)
    }
}

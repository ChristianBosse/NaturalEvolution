//! Library search: keeps the nevo-search index up to date as items are added and edited, and
//! serves the search page.

use std::collections::HashSet;
use std::io::Read;

use axum::extract::{Query, State};
use axum::response::Html;
use nevo_extract::{Kind, Limits};
use nevo_format::{Id, Revision, Visibility};
use nevo_search::{Document, Filter, Passage, SearchIndex, Source};
use nevo_store::Store;

use crate::app::{blocking, AppError, AppResult, AppState};
use crate::pages;

/// Largest attached file whose text a node extracts itself. Bigger documents are left to Seed,
/// which pre-digests on a fast computer (D-029).
const MAX_EXTRACT: u64 = 32 * 1024 * 1024;
const RESULTS: usize = 30;

fn err(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(e.to_string())
}

/// Indexes an item's current version, or removes the item from the index when no searchable
/// version is left. Private items are never indexed: a search result would reveal them.
pub fn index_item(store: &Store, index: &SearchIndex, item: Id) -> AppResult<()> {
    let key = item.to_string();
    let tips = store.item_tips(item)?;
    let Some(tip) = tips
        .iter()
        .filter(|r| r.object.vis != Visibility::Private)
        .max_by_key(|r| r.object.clock)
    else {
        index.delete(&key).map_err(err)?;
        return index.commit().map_err(err);
    };
    let r = &tip.object;
    let (body, passages) = item_text(store, r);
    let doc = Document {
        key,
        source: Source::Library,
        title: r.title.clone(),
        aliases: Vec::new(),
        lang: r.lang.clone(),
        topics: r.topics.iter().map(|t| t.as_str().to_string()).collect(),
        body,
    };
    index.put(&doc, &passages).map_err(err)?;
    index.commit().map_err(err)
}

/// The text of a version: its summary and the text of every attached document small enough to
/// read here (the guide's Markdown, text files, PDFs, EPUBs, web pages), with passages for the AI.
pub fn item_text(store: &Store, r: &Revision) -> (String, Vec<Passage>) {
    let mut parts = Vec::new();
    let mut passages = Vec::new();
    if let Some(summary) = &r.summary {
        parts.push(summary.clone());
    }
    let limits = Limits::default();
    for f in r.content.iter().filter(|f| f.size <= MAX_EXTRACT) {
        let Ok(Some(file)) = store.open_blob(f.blob) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(MAX_EXTRACT).read_to_end(&mut bytes).is_err() {
            continue;
        }
        // The real type comes from the bytes, never from what the uploader's browser claimed.
        let Ok(checked) = nevo_extract::check(&bytes, Some(&f.name), &limits) else {
            continue;
        };
        if !matches!(
            checked.kind,
            Kind::Markdown | Kind::Text | Kind::Pdf | Kind::Epub | Kind::Html
        ) {
            continue;
        }
        // A file the extractor can't read still leaves the item findable by title and topics.
        if let Ok(extracted) = nevo_extract::extract(bytes, checked.kind, &limits) {
            passages.extend(extracted.passages.into_iter().map(|p| Passage {
                heading: p.heading.unwrap_or_else(|| r.title.clone()),
                text: p.text,
            }));
            parts.push(extracted.text);
        }
    }
    (parts.join("\n\n"), passages)
}

/// Indexes every item: used when the index is new or was rebuilt. Returns how many items.
pub fn reindex_all(store: &Store, index: &SearchIndex) -> AppResult<usize> {
    let mut seen = HashSet::new();
    for tip in store.recent_tips(usize::MAX)? {
        let item = tip.object.item_id(tip.id());
        if seen.insert(item) {
            index_item(store, index, item)?;
        }
    }
    Ok(seen.len())
}

/// Re-indexes an item in the background after it changed, so saving isn't slowed down by
/// text extraction. A failure only makes search stale, so it is logged, not shown.
pub fn update_later(state: &AppState, item: Id) {
    let (store, index) = (state.store.clone(), state.search.clone());
    tokio::task::spawn_blocking(move || {
        if index_item(&store, &index, item).is_err() {
            eprintln!("search: couldn't index {item}; it may be missing from search until the next change");
        }
    });
}

#[derive(serde::Deserialize)]
pub(crate) struct SearchQuery {
    #[serde(default)]
    q: String,
}

pub(crate) async fn page(State(state): State<AppState>, Query(query): Query<SearchQuery>) -> AppResult<Html<String>> {
    let q = query.q.trim().chars().take(200).collect::<String>();
    if q.is_empty() {
        return Ok(Html(pages::search(&q, &[]).into_string()));
    }
    let shown = q.clone();
    let results = blocking(move || {
        let hits = state.search.search(&q, &Filter::default(), RESULTS).map_err(err)?;
        let mut out = Vec::new();
        for hit in hits {
            let Ok(item) = hit.key.parse::<Id>() else { continue };
            // The index may lag behind a change for a moment: check the item is still visible.
            let tips = state.store.item_tips(item)?;
            let Some(tip) = tips
                .iter()
                .filter(|r| r.object.vis != Visibility::Private)
                .max_by_key(|r| r.object.clock)
            else {
                continue;
            };
            let (text, _) = item_text(&state.store, &tip.object);
            let snippet = nevo_search::snippet(&q, &nevo_extract::reading_text(&text), &hit.lang, 240);
            out.push(pages::SearchResult {
                item,
                title: hit.title,
                topics: hit.topics,
                snippet,
            });
        }
        Ok(out)
    })
    .await?;
    Ok(Html(pages::search(&shown, &results).into_string()))
}

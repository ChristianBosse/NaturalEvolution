//! The node's own screen (D-062): a touch-first app drawn straight to the display. It runs in the
//! same program as the web server, so the library, search index and AI are loaded only once.
//!
//! The screen reads the library as an anonymous reader would; signing in on the screen comes
//! later. Library calls run on the screen's thread: they are quick on a small library, and
//! searching large ZIM packs will move to a worker thread.

use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Read;
use std::rc::Rc;
use std::time::Duration;

use nevo_format::{Id, Revision, Sealed, Topic, Visibility, STARTING_TOPICS};
use nevo_search::Filter;
use pulldown_cmark::{Event, Options, Parser, Tag};
use slint::{ComponentHandle, ModelRc, SharedString, StyledText, VecModel};

use crate::app::{trust_view, AppState, MAX_TEXT};
use crate::search::item_text;

slint::include_modules!();

/// Topic tiles per row: four fit an 800-pixel-wide screen with room for a finger.
const COLUMNS: usize = 4;
const RECENT: usize = 30;
const RESULTS: usize = 30;
const SNIPPET: usize = 200;
/// How often Home is redrawn, to show guides added from other devices.
const REFRESH: Duration = Duration::from_secs(10);

#[derive(Clone)]
enum Page {
    Home,
    Topic(Topic),
    Search(String),
    Item(Id),
}

/// Shows the screen until its window is closed.
pub fn run(state: AppState) -> Result<(), String> {
    let ui = Screen::new().map_err(|e| format!("screen: {e}"))?;
    ui.set_topic_rows(rows(STARTING_TOPICS.iter().map(|(t, _)| tile(t, &label(t))).collect()));

    let nav = Rc::new(Nav {
        state,
        ui: ui.as_weak(),
        history: RefCell::new(Vec::new()),
    });
    nav.go(Page::Home);

    let n = nav.clone();
    ui.on_home(move || {
        n.history.borrow_mut().clear();
        n.go(Page::Home);
    });
    let n = nav.clone();
    ui.on_back(move || n.back());
    let n = nav.clone();
    ui.on_search(move |q| n.go(Page::Search(q.trim().chars().take(200).collect())));
    let n = nav.clone();
    ui.on_open_topic(move |t| {
        if let Ok(topic) = t.parse() {
            n.go(Page::Topic(topic));
        }
    });
    let n = nav.clone();
    ui.on_open_item(move |id| {
        if let Ok(item) = id.parse() {
            n.go(Page::Item(item));
        }
    });
    let n = nav.clone();
    ui.on_key(move |k| n.key(&k));

    let refresh = slint::Timer::default();
    let n = nav;
    refresh.start(slint::TimerMode::Repeated, REFRESH, move || {
        if matches!(n.history.borrow().last(), Some(Page::Home)) {
            n.show(&Page::Home);
        }
    });

    ui.run().map_err(|e| format!("screen: {e}"))
}

struct Nav {
    state: AppState,
    ui: slint::Weak<Screen>,
    /// Pages visited since Home, the current one last.
    history: RefCell<Vec<Page>>,
}

impl Nav {
    fn go(&self, page: Page) {
        self.history.borrow_mut().push(page.clone());
        self.show(&page);
    }

    fn back(&self) {
        let page = {
            let mut history = self.history.borrow_mut();
            if history.len() > 1 {
                history.pop();
            }
            history.last().cloned()
        };
        if let Some(page) = page {
            self.show(&page);
        }
    }

    fn show(&self, page: &Page) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let shown = match page {
            Page::Home => self.home(&ui),
            Page::Topic(topic) => self.topic(&ui, topic),
            Page::Search(q) => self.search(&ui, q),
            Page::Item(item) => self.item(&ui, *item),
        };
        if shown.is_err() {
            ui.set_heading("Something went wrong".into());
            ui.set_subtopic_rows(rows(Vec::new()));
            ui.set_cards(model(Vec::new()));
            ui.set_empty_text("This page couldn't be read from the library. Try again, or go back.".into());
            ui.set_page("topic".into());
        }
        ui.set_can_back(self.history.borrow().len() > 1);
    }

    fn home(&self, ui: &Screen) -> Result<(), String> {
        // The name is set in the setup wizard, which may finish while the screen is showing.
        let name = self.state.accounts.node_name().map_err(err)?;
        ui.set_node_name(name.unwrap_or_else(|| "Library".into()).into());
        let mut seen = HashSet::new();
        let cards = self
            .state
            .store
            .recent_tips(100)
            .map_err(err)?
            .iter()
            .filter(|r| visible(&r.object))
            .filter(|r| seen.insert(r.object.item_id(r.id())))
            .take(RECENT)
            .map(|r| {
                let summary = r.object.summary.as_deref().unwrap_or("");
                card(
                    r.object.item_id(r.id()),
                    &r.object,
                    StyledText::from_plain_text(summary),
                )
            })
            .collect();
        ui.set_recent(model(cards));
        ui.set_page("home".into());
        Ok(())
    }

    fn topic(&self, ui: &Screen, topic: &Topic) -> Result<(), String> {
        let store = &self.state.store;
        let mut items = Vec::new();
        for item in store.items_in_topic(topic).map_err(err)? {
            if let Some(tip) = current(&store.item_tips(item).map_err(err)?) {
                items.push((item, tip.clone()));
            }
        }
        items.sort_by_key(|(_, r)| r.object.title.to_lowercase());
        let subtopics = STARTING_TOPICS
            .iter()
            .find(|(top, _)| *top == topic.as_str())
            .map(|(top, subs)| subs.iter().map(|s| tile(&format!("{top}/{s}"), &label(s))).collect())
            .unwrap_or_default();
        let cards = items
            .iter()
            .map(|(item, r)| {
                let summary = r.object.summary.as_deref().unwrap_or("");
                card(*item, &r.object, StyledText::from_plain_text(summary))
            })
            .collect();
        ui.set_heading(topic_label(topic.as_str()).into());
        ui.set_subtopic_rows(rows(subtopics));
        ui.set_cards(model(cards));
        ui.set_empty_text("Nothing in this topic yet.".into());
        ui.set_page("topic".into());
        Ok(())
    }

    fn search(&self, ui: &Screen, q: &str) -> Result<(), String> {
        let mut cards = Vec::new();
        if !q.is_empty() {
            let store = &self.state.store;
            for hit in self.state.search.search(q, &Filter::default(), RESULTS).map_err(err)? {
                let Ok(item) = hit.key.parse::<Id>() else { continue };
                // The index may lag behind a change for a moment: check the item is still visible.
                let Some(tip) = current(&store.item_tips(item).map_err(err)?).cloned() else {
                    continue;
                };
                let (text, _) = item_text(store, &tip.object);
                let snippet = nevo_search::snippet(q, &nevo_extract::reading_text(&text), &hit.lang, SNIPPET);
                cards.push(card(item, &tip.object, styled(&snippet_markdown(&snippet))));
            }
        }
        ui.set_query(q.into());
        ui.set_keyboard_open(false);
        ui.set_heading(if q.is_empty() {
            "Search".into()
        } else {
            format!("Results for “{q}”").into()
        });
        ui.set_subtopic_rows(rows(Vec::new()));
        ui.set_cards(model(cards));
        ui.set_empty_text(
            if q.is_empty() {
                "Type what you are looking for."
            } else {
                "Nothing in the library matches. Try other words, or fewer of them."
            }
            .into(),
        );
        ui.set_page("search".into());
        Ok(())
    }

    fn item(&self, ui: &Screen, item: Id) -> Result<(), String> {
        let store = &self.state.store;
        let tips = store.item_tips(item).map_err(err)?;
        let tip = current(&tips).ok_or("not found")?;
        let r = &tip.object;
        let text = guide_text(&self.state, r);
        // The label is advice: if it can't be worked out, the guide still shows, without it.
        let trust = nevo_trust::Trust::new(store, nevo_trust::TrustConfig::new(self.state.node.public()))
            .assess(tip, None)
            .ok()
            .map(|a| trust_view(&a));
        let (label, reasons, warnings) = match trust {
            Some(t) => (capitalized(t.label), t.reasons, t.warnings),
            None => (String::new(), Vec::new(), Vec::new()),
        };
        ui.set_guide(Guide {
            title: r.title.clone().into(),
            summary: r.summary.clone().unwrap_or_default().into(),
            topic: r
                .topics
                .first()
                .map(|t| topic_label(t.as_str()))
                .unwrap_or_default()
                .into(),
            trust: label.into(),
            reasons: model(reasons.into_iter().map(SharedString::from).collect()),
            warnings: model(warnings.into_iter().map(SharedString::from).collect()),
            blocks: model(text.map(|t| blocks(&t)).unwrap_or_default()),
        });
        ui.set_keyboard_open(false);
        ui.set_page("item".into());
        Ok(())
    }

    /// A key on the on-screen keyboard.
    fn key(&self, key: &str) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let mut q = ui.get_query().to_string();
        match key {
            "back" => {
                q.pop();
            }
            "space" => q.push(' '),
            "accents" => return ui.set_accents(!ui.get_accents()),
            "hide" => return ui.set_keyboard_open(false),
            "search" => return self.go(Page::Search(q.trim().to_string())),
            other => q.push_str(other),
        }
        ui.set_query(q.into());
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Private items are shown only to their author, and the screen has no signed-in author yet.
fn visible(r: &Revision) -> bool {
    r.vis != Visibility::Private
}

fn current(tips: &[Sealed<Revision>]) -> Option<&Sealed<Revision>> {
    tips.iter()
        .filter(|r| visible(&r.object))
        .max_by_key(|r| r.object.clock)
}

/// The guide's text: its first Markdown file, as on the web page.
fn guide_text(state: &AppState, r: &Revision) -> Option<String> {
    let f = r
        .content
        .iter()
        .find(|f| f.mime == "text/markdown" && f.size <= MAX_TEXT as u64)?;
    let file = state.store.open_blob(f.blob).ok()??;
    let mut bytes = Vec::new();
    file.take(MAX_TEXT as u64).read_to_end(&mut bytes).ok()?;
    // Old text files are often not UTF-8; show them anyway rather than fail.
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Splits Markdown into headings and other blocks (paragraphs, lists, quotes), so headings can be
/// drawn large; Slint's styled text draws the rest, with bold, italics and lists.
fn blocks(md: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut heading: Option<String> = None;
    for (event, range) in Parser::new_ext(md, Options::empty()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    start = range.start;
                    if matches!(tag, Tag::Heading { .. }) {
                        heading = Some(String::new());
                    }
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    out.push(match heading.take() {
                        Some(text) => Block {
                            heading: true,
                            text: text.into(),
                            body: StyledText::default(),
                        },
                        None => Block {
                            heading: false,
                            text: SharedString::new(),
                            body: styled(&md[start..range.end]),
                        },
                    });
                }
            }
            Event::Text(t) | Event::Code(t) => {
                if let Some(h) = heading.as_mut() {
                    h.push_str(&t);
                }
            }
            _ => {}
        }
    }
    out
}

/// Markdown that Slint can't read is shown as plain text rather than not at all.
fn styled(md: &str) -> StyledText {
    StyledText::from_markdown(md).unwrap_or_else(|_| StyledText::from_plain_text(md))
}

/// nevo-search snippets are HTML with matches in `<b>`; Slint's styled text takes Markdown.
fn snippet_markdown(html: &str) -> String {
    let text = html
        .replace("<b>", "\u{1}")
        .replace("</b>", "\u{2}")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '\u{1}' | '\u{2}' => out.push_str("**"),
            c if c.is_ascii_punctuation() => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

fn card(item: Id, r: &Revision, detail: StyledText) -> Card {
    Card {
        id: item.to_string().into(),
        title: r.title.clone().into(),
        detail,
        topic: r
            .topics
            .first()
            .map(|t| topic_label(t.as_str()))
            .unwrap_or_default()
            .into(),
    }
}

fn tile(key: &str, label: &str) -> TopicTile {
    TopicTile {
        key: key.into(),
        label: label.into(),
    }
}

/// Lays tiles out in rows of [`COLUMNS`], padding the last row so every tile is the same width.
fn rows(tiles: Vec<TopicTile>) -> ModelRc<TopicRow> {
    let rows = tiles
        .chunks(COLUMNS)
        .map(|chunk| {
            let mut row = chunk.to_vec();
            row.resize(COLUMNS, tile("", ""));
            TopicRow { tiles: model(row) }
        })
        .collect();
    model(rows)
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(items))
}

/// `first-aid` → `First aid`.
fn label(segment: &str) -> String {
    capitalized(&segment.replace('-', " "))
}

/// `medical/first-aid` → `Medical › First aid`.
fn topic_label(topic: &str) -> String {
    topic.split('/').map(label).collect::<Vec<_>>().join(" › ")
}

fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_naturally() {
        assert_eq!(topic_label("medical/first-aid"), "Medical › First aid");
        assert_eq!(label("mental-health"), "Mental health");
    }

    #[test]
    fn snippets_keep_matches_bold_and_escape_markdown() {
        assert_eq!(snippet_markdown("a <b>boil</b> &amp; 1*2"), "a **boil** \\& 1\\*2");
    }

    #[test]
    fn markdown_splits_into_headings_and_blocks() {
        let b = blocks("## Steps\n\n1. One\n2. Two\n\nSome **bold** text.\n");
        assert_eq!(b.len(), 3);
        assert!(b[0].heading && b[0].text == "Steps");
        assert!(!b[1].heading && !b[2].heading);
    }
}

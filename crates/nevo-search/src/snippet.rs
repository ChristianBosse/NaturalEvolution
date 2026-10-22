//! Highlighted excerpts for result lists, built from text the caller read back from its source.

use std::collections::HashMap;

use crate::analysis::{self, Analyzers};

/// Queries are cut like in search; a snippet never needs more.
const MAX_QUERY_CHARS: usize = 500;
/// Enough to find the best window in any real article without scanning forever.
const MAX_MATCHES: usize = 1000;
/// Words this long may match with one typo, as in search.
const FUZZY_MIN_CHARS: usize = 5;

/// An excerpt of `text` of at most `max_chars` characters (plus "…" marks where it was cut),
/// around the part that matches `query` best, with matching words wrapped in `<b>…</b>`.
///
/// The result is safe to put into HTML as is: everything except the `<b>` tags is escaped.
/// Runs of whitespace become one space. `lang` is the text's language (BCP 47), so words match
/// the way search matched them ("bleeding" highlights "bleeds").
pub fn snippet(query: &str, text: &str, lang: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let query = match query.char_indices().nth(MAX_QUERY_CHARS) {
        Some((i, _)) => &query[..i],
        None => query,
    };
    let group = analysis::group_of(lang);
    let mut analyzers = Analyzers::new();

    // Each query term gets a number, so a window can count how many different ones it has.
    let mut wanted: HashMap<String, usize> = HashMap::new();
    for term in analyzers.terms(group, query) {
        let next = wanted.len();
        wanted.entry(term).or_insert(next);
    }
    let words = analysis::words(query);
    for pair in words.windows(2) {
        if let Some(term) = analyzers.term(group, &format!("{}{}", pair[0], pair[1])) {
            let next = wanted.len();
            wanted.entry(term).or_insert(next);
        }
    }
    let fuzzy: Vec<Vec<char>> = (words.iter())
        .filter(|w| w.chars().count() >= FUZZY_MIN_CHARS)
        .map(|w| w.chars().collect())
        .collect();

    let mut matches: Vec<Match> = Vec::new();
    for (from, to, term) in analyzers.spans(group, text) {
        let id = wanted.get(&term).copied().or_else(|| {
            let word: Vec<char> = text[from..to].to_lowercase().chars().collect();
            (word.len() >= FUZZY_MIN_CHARS)
                .then(|| fuzzy.iter().position(|f| within_one_edit(f, &word)))
                .flatten()
                .map(|i| wanted.len() + i)
        });
        if let Some(id) = id {
            matches.push(Match { from, to, id });
            if matches.len() == MAX_MATCHES {
                break;
            }
        }
    }

    let (start, end) = window(text, &matches, max_chars);
    let mut out = Writer::default();
    if start > 0 {
        out.out.push('…');
    }
    let mut pos = start;
    for m in matches.iter().filter(|m| m.from >= start && m.to <= end) {
        out.text(&text[pos..m.from]);
        out.open_bold();
        out.text(&text[m.from..m.to]);
        out.out.push_str("</b>");
        pos = m.to;
    }
    out.text(&text[pos..end]);
    let mut html = out.out.trim_end().to_string();
    if end < text.len() {
        html.push('…');
    }
    html
}

struct Match {
    from: usize,
    to: usize,
    /// Which query term matched.
    id: usize,
}

/// The byte range of the best window: the most different query terms, then the most matches,
/// then the earliest. Starts a little before its first match and ends at a word boundary.
fn window(text: &str, matches: &[Match], max_chars: usize) -> (usize, usize) {
    if text.chars().count() <= max_chars {
        return (0, text.len());
    }
    let lead = max_chars / 5;
    let mut best = (0, text.len().min(byte_after(text, 0, max_chars)));
    let mut best_score = (0, 0);
    for m in matches {
        let mut start = byte_before(text, m.from, lead);
        // Don't start mid-word: skip to the next space if it comes before the match.
        if start > 0 && !text[..start].ends_with(char::is_whitespace) {
            if let Some(i) = text[start..m.from].find(char::is_whitespace) {
                start += i + 1;
            }
        }
        let end = byte_after(text, start, max_chars);
        let inside = matches.iter().filter(|o| o.from >= start && o.to <= end);
        let mut ids: Vec<usize> = inside.clone().map(|o| o.id).collect();
        ids.sort_unstable();
        ids.dedup();
        let score = (ids.len(), inside.count());
        if score > best_score {
            best_score = score;
            best = (start, end);
        }
    }
    let (start, mut end) = best;
    // End at a word boundary unless that would throw away more than half the window.
    if end < text.len() {
        if let Some(i) = text[start..end].rfind(char::is_whitespace) {
            if text[start..start + i].chars().count() > max_chars / 2 {
                end = start + i;
            }
        }
    }
    (start, end)
}

/// The byte offset `n` characters after `from` (or the end of the text).
fn byte_after(text: &str, from: usize, n: usize) -> usize {
    text[from..].char_indices().nth(n).map_or(text.len(), |(i, _)| from + i)
}

/// The byte offset `n` characters before `to` (or 0).
fn byte_before(text: &str, to: usize, n: usize) -> usize {
    if n == 0 {
        return to;
    }
    text[..to].char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i)
}

/// True if `a` becomes `b` with at most one insertion, deletion, substitution or swap of two
/// neighbouring letters (the distance Tantivy's typo matching uses).
pub(crate) fn within_one_edit(a: &[char], b: &[char]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let same = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[same..], &b[same..]);
    match a.len().cmp(&b.len()) {
        std::cmp::Ordering::Less => a == &b[1..],
        std::cmp::Ordering::Greater => &a[1..] == b,
        std::cmp::Ordering::Equal if a.is_empty() => true,
        std::cmp::Ordering::Equal => {
            a[1..] == b[1..] || (a.len() >= 2 && a[0] == b[1] && a[1] == b[0] && a[2..] == b[2..])
        }
    }
}

/// HTML-escaping output with whitespace runs collapsed.
#[derive(Default)]
struct Writer {
    out: String,
    space: bool,
}

impl Writer {
    /// Writes a space left pending by earlier whitespace, unless at the start.
    fn flush_space(&mut self) {
        if self.space && !self.out.is_empty() && !self.out.ends_with('…') {
            self.out.push(' ');
        }
        self.space = false;
    }

    /// Opens a `<b>`, after any pending space so the space stays outside the tag.
    fn open_bold(&mut self) {
        self.flush_space();
        self.out.push_str("<b>");
    }

    fn text(&mut self, s: &str) {
        for c in s.chars() {
            if c.is_whitespace() {
                self.space = true;
                continue;
            }
            self.flush_space();
            match c {
                '&' => self.out.push_str("&amp;"),
                '<' => self.out.push_str("&lt;"),
                '>' => self.out.push_str("&gt;"),
                '"' => self.out.push_str("&quot;"),
                '\'' => self.out.push_str("&#39;"),
                c if c.is_control() => {}
                c => self.out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn one_edit() {
        assert!(within_one_edit(&chars("diarhea"), &chars("diarrhea")));
        assert!(within_one_edit(&chars("diarrhea"), &chars("diarhea")));
        assert!(within_one_edit(&chars("bleding"), &chars("bleeding")));
        assert!(within_one_edit(&chars("hypotermia"), &chars("hypothermia")));
        assert!(within_one_edit(&chars("tetnaus"), &chars("tetanus")));
        assert!(within_one_edit(&chars("malaria"), &chars("malarie")));
        assert!(within_one_edit(&chars("same"), &chars("same")));
        assert!(!within_one_edit(&chars("diarea"), &chars("diarrhea")));
        assert!(!within_one_edit(&chars("abcd"), &chars("badc")));
    }

    #[test]
    fn window_boundaries() {
        assert_eq!(byte_after("héllo", 0, 2), 3);
        assert_eq!(byte_after("héllo", 0, 99), 6);
        assert_eq!(byte_before("héllo", 6, 4), 1);
        assert_eq!(byte_before("héllo", 6, 99), 0);
        assert_eq!(byte_before("héllo", 6, 0), 6);
    }
}

//! Article HTML to clean text, in sections, and passages for the AI.
//!
//! The HTML is tokenized with [`html5gum`]: a WHATWG-conformant tokenizer with no required
//! dependencies, fuzzed upstream, that decodes every named entity and never needs the whole
//! document as a tree. On top of it sits a small element stack, just enough to know which
//! element a piece of text is in, and to skip whole subtrees (reference lists, navboxes, …).
//!
//! What's dropped was chosen from real mwoffliner (Kiwix) pages, which are MediaWiki Parsoid
//! HTML: scripts and styles, hidden elements, citation markers (`sup.reference`), reference
//! lists, navboxes, hatnotes ("See also: …"), maintenance boxes, category links, the Kiwix
//! footer, and sections such as "References" or "External links". Infobox and table rows
//! become `cell | cell` lines, list items become `- item` lines.

use std::convert::Infallible;

use html5gum::emitters::callback::{CallbackEmitter, CallbackEvent};
use html5gum::{Span, Tokenizer};

/// Passages aim for this many characters or more, when the section is long enough.
pub const PASSAGE_MIN: usize = 500;
/// Passages never have more characters than this.
pub const PASSAGE_MAX: usize = 1500;

/// HTML beyond this size is cut off before extraction. The largest Wikipedia pages are a few
/// MB, so this only stops hostile input.
const MAX_HTML: usize = 16 << 20;

/// Elements nested deeper than this are treated as part of their parent. Real pages stay under
/// 50; the limit keeps the stack and end-tag matching small on hostile input.
const MAX_DEPTH: usize = 256;

/// An article's text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    /// From `<title>` or the first `<h1>`.
    pub title: Option<String>,
    /// In page order. The first is the introduction, with an empty heading, if the page has
    /// text before its first heading.
    pub sections: Vec<Section>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    /// 2 for `<h2>` … 6 for `<h6>`; 1 for the introduction.
    pub level: u8,
    /// Whitespace-collapsed paragraphs. List items start with `- `; table rows are
    /// `cell | cell`.
    pub paragraphs: Vec<String>,
}

/// A short piece of an article, sized for an AI prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    /// The section's heading path, such as `Treatment > Antivenom`. Empty for the introduction.
    pub heading: String,
    /// Whole paragraphs joined by newlines; only a paragraph longer than [`PASSAGE_MAX`] is cut,
    /// at a sentence end where possible.
    pub text: String,
}

impl Document {
    /// The text as one string: headings and paragraphs, separated by blank lines.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for s in &self.sections {
            for line in std::iter::once(&s.heading)
                .filter(|h| !h.is_empty())
                .chain(&s.paragraphs)
            {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(line);
            }
        }
        out
    }

    /// Splits each section into passages of about [`PASSAGE_MIN`] to [`PASSAGE_MAX`]
    /// characters, at paragraph boundaries. Short sections give one short passage.
    pub fn passages(&self) -> Vec<Passage> {
        let mut out = Vec::new();
        let mut path: Vec<(u8, &str)> = Vec::new();
        for s in &self.sections {
            while path.last().is_some_and(|&(level, _)| level >= s.level) {
                path.pop();
            }
            if !s.heading.is_empty() {
                path.push((s.level, &s.heading));
            }
            let heading = path.iter().map(|&(_, h)| h).collect::<Vec<_>>().join(" > ");
            let mut chunk = String::new();
            let mut chunk_len = 0;
            for piece in s.paragraphs.iter().flat_map(|p| split_long(p)) {
                let len = piece.chars().count();
                if chunk_len > 0 && chunk_len + 1 + len > PASSAGE_MAX {
                    out.push(Passage {
                        heading: heading.clone(),
                        text: std::mem::take(&mut chunk),
                    });
                    chunk_len = 0;
                }
                if chunk_len > 0 {
                    chunk.push('\n');
                    chunk_len += 1;
                }
                chunk.push_str(piece);
                chunk_len += len;
            }
            if chunk_len > 0 {
                out.push(Passage { heading, text: chunk });
            }
        }
        out
    }
}

/// Cuts a paragraph into pieces of at most [`PASSAGE_MAX`] characters, preferring sentence ends,
/// then spaces, and each at least [`PASSAGE_MIN`] long except the last.
fn split_long(p: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = p;
    while rest.chars().count() > PASSAGE_MAX {
        let at = |n: usize| rest.char_indices().nth(n).map_or(rest.len(), |(i, _)| i);
        let (min, max) = (at(PASSAGE_MIN), at(PASSAGE_MAX));
        let window = &rest[min..max];
        let sentence = [". ", "? ", "! ", "。"]
            .iter()
            .filter_map(|end| window.rfind(end).map(|i| i + end.trim_end().len()))
            .max();
        let cut = min + sentence.or_else(|| window.rfind(' ')).unwrap_or(window.len());
        out.push(rest[..cut].trim_end());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Extracts the readable text of an article's HTML. Never fails: broken HTML gives less text.
pub fn extract(html: &str) -> Document {
    let mut end = html.len().min(MAX_HTML);
    while !html.is_char_boundary(end) {
        end -= 1;
    }
    let html = &html[..end];
    tokenize(&html[..droppable_tail(html)])
}

/// Where the page's closing run of dropped sections starts (References, External links, …),
/// or the end of the page.
///
/// On Wikipedia pages these sections and the navboxes after them are half the HTML, and
/// tokenizing them only to throw them away took half the time. Cutting them first gives the
/// same text, as long as no heading after the cut would have ended the drop; so every `<h1>`
/// and `<h2>` after it must be a dropped section too. (`<h3>` and lower never end one.)
fn droppable_tail(html: &str) -> usize {
    // Real pages end with a handful of such sections; the limit bounds the work on hostile input.
    const MAX_HEADINGS: usize = 32;
    let b = html.as_bytes();
    let mut cut = b.len();
    let mut end = b.len();
    let mut seen = 0;
    // Walk the headings backwards from the end, while they are droppable.
    while let Some(at) = b[..end].iter().rposition(|&c| c == b'<') {
        end = at;
        let level = match b.get(at + 1..at + 4) {
            Some(&[h, level @ (b'1' | b'2'), next]) if h | 0x20 == b'h' && !next.is_ascii_alphanumeric() => level,
            _ => continue,
        };
        // A heading's markup is short; one that doesn't close soon is treated as content.
        let window = &b[at..b.len().min(at + 1024)];
        let close = window
            .windows(3)
            .position(|w| w[0] == b'<' && w[1] == b'/' && w[2] | 0x20 == b'h');
        let droppable = level == b'2' && close.is_some_and(|len| tokenize(&html[at..at + len]).sections.is_empty());
        seen += 1;
        if !droppable || seen > MAX_HEADINGS {
            break;
        }
        cut = at;
    }
    cut
}

fn tokenize(html: &str) -> Document {
    let mut ex = Extractor::default();
    let mut emitter = CallbackEmitter::new(|event: CallbackEvent<'_>, _: Span<()>| -> Option<Infallible> {
        ex.event(event);
        None
    });
    // Without this, the contents of <script> and <style> would be parsed as HTML.
    emitter.naively_switch_states(true);
    // Reading from a string can't fail.
    let Ok(()) = Tokenizer::new_with_emitter(html, emitter).finish();
    ex.finish()
}

/// Elements whose content is never text.
const SKIP_TAGS: &[&str] = &[
    "script", "style", "noscript", "template", "svg", "math", "button", "select", "textarea", "iframe", "object",
    "video", "audio", "canvas", "nav", "footer", "map",
];

/// Elements with no end tag.
const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
];

/// Elements that start a new paragraph.
const BLOCK_TAGS: &[&str] = &[
    "p",
    "div",
    "section",
    "article",
    "main",
    "header",
    "aside",
    "blockquote",
    "pre",
    "figure",
    "figcaption",
    "ul",
    "ol",
    "dl",
    "dt",
    "dd",
    "li",
    "table",
    "caption",
    "tbody",
    "thead",
    "tfoot",
    "tr",
    "center",
    "details",
    "summary",
    "address",
    "hr",
    "body",
];

/// Classes of MediaWiki and mwoffliner noise.
const DROP_CLASSES: &[&str] = &[
    // Citation markers and reference lists.
    "reference",
    "mw-ref",
    "references",
    "reflist",
    "refbegin",
    "mw-references-wrap",
    "mw-references",
    "mw-cite-backlink",
    "Z3988",
    // Navigation and page furniture.
    "navbox",
    "navbox-styles",
    "vertical-navbox",
    "navbar",
    "hatnote",
    "dablink",
    "mw-editsection",
    "mw-jump-link",
    "catlinks",
    "printfooter",
    "authority-control",
    "toc",
    "zim-footer",
    "shortdescription",
    "noprint",
    "noexcerpt",
    // Maintenance boxes ("This article needs more citations"), sister-project and portal boxes.
    "metadata",
    "ambox",
    "mbox-small",
    "side-box",
    "sistersitebox",
    "portalbox",
    // "[citation needed]" and similar inline notes.
    "Inline-Template",
    "Template-Fact",
    // Math markup: its fallback text is LaTeX source.
    "mwe-math-element",
    "mw-empty-elt",
    "infobox-hiddenrow",
    "cs1-hidden-error",
    "cs1-visible-error",
];

const DROP_IDS: &[&str] = &[
    "catlinks",
    "toc",
    "mw-navigation",
    "footer",
    "siteSub",
    "contentSub",
    "jump-to-nav",
];

/// Sections that are lists of sources or links, compared in lowercase. English first, then the
/// other big Wikipedias.
const DROP_SECTIONS: &[&str] = &[
    "references",
    "notes",
    "citations",
    "sources",
    "bibliography",
    "further reading",
    "external links",
    "see also",
    "footnotes",
    "works cited",
    "notes and references",
    "references and notes",
    "general references",
    "cited sources",
    "explanatory notes",
    "références",
    "notes et références",
    "liens externes",
    "bibliographie",
    "voir aussi",
    "einzelnachweise",
    "weblinks",
    "literatur",
    "siehe auch",
    "anmerkungen",
    "referencias",
    "enlaces externos",
    "bibliografía",
    "véase también",
    "notas",
];

/// A tag name, kept inline: names longer than 16 bytes are compared by their first 16.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Name {
    bytes: [u8; 16],
    len: u8,
}

impl Name {
    fn new(raw: &[u8]) -> Name {
        let mut n = Name::default();
        let len = raw.len().min(16);
        n.bytes[..len].copy_from_slice(&raw[..len]);
        n.bytes[..len].make_ascii_lowercase();
        n.len = len as u8;
        n
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    fn is(&self, name: &str) -> bool {
        self.as_bytes() == name.as_bytes()
    }

    fn is_any(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.is(n))
    }

    fn heading_level(&self) -> Option<u8> {
        match self.as_bytes() {
            [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inline,
    Block,
    /// `<h1>`: the page title. `<title>` too.
    Title,
    Heading(u8),
    ListItem,
    Row,
    Cell,
}

#[derive(Clone, Copy)]
struct Frame {
    name: Name,
    kind: Kind,
    /// This element started a dropped subtree.
    skip: bool,
}

/// Where text currently goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Paragraph,
    Heading,
    Title,
}

#[derive(Default)]
struct Extractor {
    stack: Vec<Frame>,
    /// Frames on the stack with `skip` set; text is kept only when this is zero.
    skipping: usize,
    /// Open table cells. Inside one, blocks don't start new paragraphs: a whole row is one line.
    cells: usize,
    /// Cells seen in the current row, for the ` | ` separators.
    row_cells: usize,
    /// The start tag being read, and whether its attributes say to drop it.
    tag: Name,
    attr: Name,
    drop_tag: bool,

    doc: Document,
    para: String,
    /// A space is owed before the next visible character.
    space: bool,
    /// `- ` is owed at the start of the next paragraph.
    bullet: bool,
    heading: Option<(u8, String)>,
    title: Option<String>,
    /// Dropping everything until a heading of this level or higher ("References").
    drop_level: Option<u8>,
}

impl Extractor {
    fn event(&mut self, event: CallbackEvent<'_>) {
        match event {
            CallbackEvent::OpenStartTag { name } => {
                self.tag = Name::new(name);
                self.drop_tag = false;
            }
            CallbackEvent::AttributeName { name } => {
                self.attr = Name::new(name);
                if self.attr.is("hidden") {
                    self.drop_tag = true;
                }
            }
            CallbackEvent::AttributeValue { value } => {
                if drops(self.attr, value) {
                    self.drop_tag = true;
                }
            }
            CallbackEvent::CloseStartTag { self_closing } => self.start_tag(self_closing),
            CallbackEvent::EndTag { name } => self.end_tag(Name::new(name)),
            CallbackEvent::String { value } => self.text(&String::from_utf8_lossy(value)),
            CallbackEvent::Comment { .. } | CallbackEvent::Doctype { .. } | CallbackEvent::Error(_) => {}
        }
    }

    fn start_tag(&mut self, self_closing: bool) {
        let name = self.tag;
        if name.is_any(VOID_TAGS) {
            if name.is("br") || name.is("hr") {
                self.space = true;
            }
            return;
        }
        self.close_implied(name);
        let skip = self.drop_tag || name.is_any(SKIP_TAGS);
        let kind = if name.is("title") || name.is("h1") {
            Kind::Title
        } else if let Some(level) = name.heading_level() {
            Kind::Heading(level)
        } else if name.is("li") || name.is("dt") || name.is("dd") {
            Kind::ListItem
        } else if name.is("tr") {
            Kind::Row
        } else if name.is("td") || name.is("th") {
            Kind::Cell
        } else if name.is_any(BLOCK_TAGS) {
            Kind::Block
        } else {
            Kind::Inline
        };
        // Foreign content (SVG, MathML) really is self-closing; HTML elements aren't, but they
        // are rare enough that treating them as empty does no harm.
        if self_closing || self.stack.len() >= MAX_DEPTH {
            return;
        }
        self.stack.push(Frame { name, kind, skip });
        if skip {
            self.skipping += 1;
            return;
        }
        if self.skipping > 0 {
            return;
        }
        match kind {
            Kind::Title => {
                self.break_paragraph();
                self.title = Some(String::new());
            }
            Kind::Heading(level) => {
                self.break_paragraph();
                if self.drop_level.is_some_and(|l| level <= l) {
                    self.drop_level = None;
                }
                self.heading = Some((level, String::new()));
            }
            Kind::ListItem => {
                if self.cells > 0 {
                    self.separate(", ");
                } else {
                    self.break_paragraph();
                    self.bullet = name.is("li");
                }
            }
            Kind::Row => {
                if self.cells == 0 {
                    self.break_paragraph();
                    self.row_cells = 0;
                }
            }
            Kind::Cell => {
                if self.cells == 0 {
                    if self.row_cells > 0 {
                        self.separate(" | ");
                    }
                    self.row_cells += 1;
                } else {
                    self.space = true;
                }
                self.cells += 1;
            }
            Kind::Block if self.cells > 0 => self.space = true,
            Kind::Block => self.break_paragraph(),
            Kind::Inline => {}
        }
    }

    /// Closes elements that HTML closes implicitly (`<p>` before a block, `<li>` before the
    /// next `<li>`, …). Parsoid output always closes them, but other HTML might not.
    fn close_implied(&mut self, name: Name) {
        let (siblings, boundary): (&[&str], &[&str]) = if name.is("li") {
            (&["li"], &["ul", "ol"])
        } else if name.is("dt") || name.is("dd") {
            (&["dt", "dd"], &["dl"])
        } else if name.is("tr") {
            (&["tr"], &["table"])
        } else if name.is("td") || name.is("th") {
            (&["td", "th"], &["tr", "table"])
        } else if name.is_any(BLOCK_TAGS) || name.heading_level().is_some() {
            if self.stack.last().is_some_and(|f| f.name.is("p")) {
                self.pop();
            }
            return;
        } else {
            return;
        };
        let found = self
            .stack
            .iter()
            .rev()
            .take_while(|f| !f.name.is_any(boundary))
            .position(|f| f.name.is_any(siblings));
        if let Some(depth) = found {
            for _ in 0..=depth {
                self.pop();
            }
        }
    }

    fn end_tag(&mut self, name: Name) {
        // An end tag without a matching open element is ignored, as browsers do.
        if let Some(depth) = self.stack.iter().rev().position(|f| f.name == name) {
            for _ in 0..=depth {
                self.pop();
            }
        }
    }

    fn pop(&mut self) {
        let Some(frame) = self.stack.pop() else { return };
        if frame.skip {
            self.skipping -= 1;
            return;
        }
        if self.skipping > 0 {
            return;
        }
        match frame.kind {
            Kind::Title => {
                let title = self
                    .title
                    .take()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty());
                if self.doc.title.is_none() {
                    self.doc.title = title;
                }
            }
            Kind::Heading(level) => {
                let Some((_, text)) = self.heading.take() else { return };
                let text = text.trim().to_string();
                if self.drop_level.is_some() {
                    return;
                }
                if DROP_SECTIONS.contains(&text.to_lowercase().as_str()) {
                    self.drop_level = Some(level);
                } else {
                    self.doc.sections.push(Section {
                        heading: text,
                        level,
                        paragraphs: Vec::new(),
                    });
                }
                self.space = false;
            }
            Kind::Cell => {
                self.cells -= 1;
                if self.cells > 0 {
                    self.space = true;
                }
            }
            Kind::Row => {
                if self.cells == 0 {
                    self.break_paragraph();
                }
            }
            Kind::ListItem | Kind::Block => {
                if self.cells > 0 {
                    self.space = true;
                } else {
                    self.break_paragraph();
                }
            }
            Kind::Inline => {}
        }
    }

    fn target(&self) -> Target {
        if self.title.is_some() {
            Target::Title
        } else if self.heading.is_some() {
            Target::Heading
        } else {
            Target::Paragraph
        }
    }

    fn text(&mut self, text: &str) {
        if self.skipping > 0 {
            return;
        }
        let target = self.target();
        if target == Target::Paragraph && self.drop_level.is_some() {
            return;
        }
        let bullet = self.bullet && target == Target::Paragraph;
        let mut space = self.space;
        let buf = match target {
            Target::Title => self.title.get_or_insert_with(String::new),
            Target::Heading => &mut self.heading.get_or_insert_with(Default::default).1,
            Target::Paragraph => &mut self.para,
        };
        let mut wrote = false;
        for c in text.chars() {
            // Soft hyphens and zero-width characters are invisible; they'd only break words.
            if matches!(c, '\u{ad}' | '\u{200b}'..='\u{200d}' | '\u{2060}' | '\u{feff}') {
                continue;
            }
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if !wrote && bullet && buf.is_empty() {
                buf.push_str("- ");
                space = false;
            }
            if space && !buf.is_empty() && !buf.ends_with(' ') {
                buf.push(' ');
            }
            space = false;
            buf.push(c);
            wrote = true;
        }
        self.space = space;
        if wrote && bullet {
            self.bullet = false;
        }
    }

    /// Adds a separator inside the current line, if it has text.
    fn separate(&mut self, sep: &str) {
        let trimmed = self.para.trim_end();
        // Nothing to separate yet, or a separator is already there ("cell | " then a list).
        if self.skipping > 0 || trimmed.is_empty() || trimmed.ends_with(['|', ',']) {
            return;
        }
        self.para.truncate(trimmed.len());
        self.para.push_str(sep);
        self.space = false;
    }

    fn break_paragraph(&mut self) {
        self.space = false;
        self.bullet = false;
        let text = std::mem::take(&mut self.para);
        let text = text.trim().trim_end_matches(" |").trim_end_matches(',');
        // A bullet with nothing after it is an empty list item.
        if text.is_empty() || text == "-" || self.drop_level.is_some() {
            return;
        }
        if self.doc.sections.is_empty() {
            self.doc.sections.push(Section {
                heading: String::new(),
                level: 1,
                paragraphs: Vec::new(),
            });
        }
        if let Some(section) = self.doc.sections.last_mut() {
            section.paragraphs.push(text.to_string());
        }
    }

    fn finish(mut self) -> Document {
        while !self.stack.is_empty() {
            self.pop();
        }
        self.break_paragraph();
        self.doc
    }
}

/// Whether an attribute marks its element as noise or hidden.
fn drops(attr: Name, value: &[u8]) -> bool {
    let value = String::from_utf8_lossy(value);
    if attr.is("class") {
        value.split_ascii_whitespace().any(|c| DROP_CLASSES.contains(&c))
    } else if attr.is("id") {
        DROP_IDS.contains(&value.as_ref())
    } else if attr.is("style") {
        let style: String = value
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase();
        style.contains("display:none") || style.contains("visibility:hidden")
    } else if attr.is("aria-hidden") {
        value.eq_ignore_ascii_case("true")
    } else if attr.is("role") {
        // Navboxes and hatnotes carry these roles even when their classes differ.
        value == "navigation" || value == "note"
    } else if attr.is("typeof") {
        value.contains("mw:Extension/references")
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// All paragraphs, one per line.
    fn text(html: &str) -> String {
        extract(html)
            .sections
            .iter()
            .flat_map(|s| s.paragraphs.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn keeps_paragraphs_headings_and_title() {
        let doc = extract(
            "<html><head><title>Snakebite</title></head><body><h1>Snakebite</h1>\
             <p>Intro one.</p><p>Intro two.</p>\
             <section><div class=\"mw-heading mw-heading2\"><h2 id=\"Treatment\">Treatment</h2></div>\
             <p>Wash the wound.</p>\
             <section><h3>Antivenom</h3><p>Antivenom works.</p></section></section></body></html>",
        );
        assert_eq!(doc.title.as_deref(), Some("Snakebite"));
        let summary: Vec<_> = doc
            .sections
            .iter()
            .map(|s| (s.level, s.heading.as_str(), s.paragraphs.len()))
            .collect();
        assert_eq!(summary, [(1, "", 2), (2, "Treatment", 1), (3, "Antivenom", 1)]);
        assert_eq!(doc.sections[1].paragraphs, ["Wash the wound."]);
        assert_eq!(
            doc.text(),
            "Intro one.\n\nIntro two.\n\nTreatment\n\nWash the wound.\n\nAntivenom\n\nAntivenom works."
        );
    }

    #[test]
    fn drops_scripts_styles_and_comments() {
        let html = "<p>A<script>var x = '<p>no</p>';</script>B<style>p { color: red }</style>C<!-- note -->D\
                    <noscript>no</noscript><template><p>no</p></template><svg><text>no</text></svg></p>";
        assert_eq!(text(html), "ABCD");
    }

    #[test]
    fn drops_citation_markers() {
        let html = "<p>Pain is common.<sup class=\"mw-ref reference\" id=\"cite_ref-1\"><a href=\"#cite_note-1\">\
                    <span class=\"mw-reflink-text\"><span class=\"cite-bracket\">[</span>1\
                    <span class=\"cite-bracket\">]</span></span></a></sup> Swelling too.<sup class=\"reference\">[2]</sup>\
                    <span class=\"Inline-Template\">[<i>citation needed</i>]</span></p>";
        assert_eq!(text(html), "Pain is common. Swelling too.");
    }

    #[test]
    fn drops_reference_lists() {
        let html = "<p>Body.</p><div class=\"mw-references-wrap\"><ol class=\"mw-references references\">\
                    <li id=\"cite_note-1\"><span class=\"mw-cite-backlink\">^</span> Smith J. Retrieved 24 August 2009.\
                    </li></ol></div><div class=\"reflist\"><ol><li>Old style. Retrieved 2010.</li></ol></div>\
                    <div typeof=\"mw:Extension/references\"><ol><li>Typed list.</li></ol></div>\
                    <div class=\"refbegin\"><ul><li>Book, ISBN 123</li></ul></div><p>End.</p>";
        assert_eq!(text(html), "Body.\nEnd.");
    }

    #[test]
    fn drops_reference_and_link_sections_by_heading() {
        let doc = extract(
            "<h2>Treatment</h2><p>Keep.</p><h2>References</h2><p>Junk.</p><h3>Journals</h3><p>Junk.</p>\
             <h2>External links</h2><ul><li>WHO database</li></ul><h2>See also</h2><ul><li>Other</li></ul>\
             <h2>History</h2><p>Kept again.</p>",
        );
        let headings: Vec<_> = doc.sections.iter().map(|s| s.heading.as_str()).collect();
        assert_eq!(headings, ["Treatment", "History"]);
        assert_eq!(doc.text(), "Treatment\n\nKeep.\n\nHistory\n\nKept again.");
    }

    #[test]
    fn drops_navboxes_hatnotes_and_page_furniture() {
        let html = "<div role=\"note\" class=\"hatnote navigation-not-searchable\">See also: List</div>\
                    <p>Body.</p>\
                    <div class=\"navbox-styles\"><style>.x{}</style></div>\
                    <div role=\"navigation\" class=\"navbox\"><table><tr><th>Classification</th><td>D</td></tr>\
                    </table></div><div role=\"navigation\" aria-label=\"x\"><ul><li>Other nav</li></ul></div>\
                    <table class=\"box-Refimprove metadata ambox\"><tr><td>This article needs citations.</td></tr>\
                    </table><div class=\"noprint portalbox\">Portal: Medicine</div>\
                    <div id=\"catlinks\" class=\"catlinks\">Categories: <ul><li>Animal bites</li></ul></div>\
                    <div class=\"zim-footer\">This page is issued from Wikipedia.</div>";
        assert_eq!(text(html), "Body.");
    }

    #[test]
    fn drops_edit_links_from_headings() {
        let doc = extract("<h2>Diagnosis<span class=\"mw-editsection\">[<a href=\"#\">edit</a>]</span></h2><p>x</p>");
        assert_eq!(doc.sections[0].heading, "Diagnosis");
    }

    #[test]
    fn drops_hidden_elements() {
        let html = "<p>A<span style=\"display: none\">no</span>B<span style=\"DISPLAY:NONE;color:red\">no</span>\
                    C<span hidden>no</span>D<span aria-hidden=\"true\">no</span>E<span class=\"infobox-hiddenrow\">\
                    no</span>F<span style=\"visibility:hidden\">no</span>G</p>";
        assert_eq!(text(html), "ABCDEFG");
    }

    #[test]
    fn decodes_entities() {
        let html = "<p>Fish &amp; chips &lt;3 &quot;q&quot; &#39;s&#39; caf&eacute; 5&nbsp;mg &#x2014; &#8212; \
                    &mdash; &frac12; &rarr; &unknownentity; AT&T</p>";
        assert_eq!(
            text(html),
            "Fish & chips <3 \"q\" 's' café 5 mg — — — ½ → &unknownentity; AT&T"
        );
        assert_eq!(extract("<h2>A &amp; B</h2><p>x</p>").sections[0].heading, "A & B");
    }

    #[test]
    fn collapses_whitespace_and_invisible_characters() {
        let html = "<p>  one\n\n  two\t<b>three</b>four in\u{ad}visible\u{200b}word </p>";
        assert_eq!(text(html), "one two threefour invisibleword");
    }

    #[test]
    fn lists_become_bullets() {
        let html = "<p>Signs:</p><ul><li>Pain</li><li>Swelling<ul><li>Local</li></ul></li><li></li></ul>\
                    <ol><li>First<li>Second</ol><dl><dt>Term</dt><dd>Meaning</dd></dl>";
        assert_eq!(
            text(html),
            "Signs:\n- Pain\n- Swelling\n- Local\n- First\n- Second\nTerm\nMeaning"
        );
    }

    #[test]
    fn table_rows_become_lines() {
        let html = "<table class=\"infobox\"><tbody>\
                    <tr><th colspan=\"2\">Snakebite</th></tr>\
                    <tr><th>Specialty</th><td>Emergency medicine</td></tr>\
                    <tr><th>Symptoms</th><td><div class=\"plainlist\"><ul><li>Pain</li><li>Swelling</li></ul></div>\
                    </td></tr><tr><th>Deaths</th><td>94,000<br>per year<sup class=\"reference\">[3]</sup></td></tr>\
                    <tr><td></td><td>Only second</td></tr>\
                    </tbody></table><table><tr><td>a<td>b<tr><td>c</table>";
        assert_eq!(
            text(html),
            "Snakebite\nSpecialty | Emergency medicine\nSymptoms | Pain, Swelling\nDeaths | 94,000 per year\n\
             Only second\na | b\nc"
        );
    }

    #[test]
    fn keeps_figure_captions() {
        let html = "<figure><span><img src=\"x.png\" alt=\"\"></span><figcaption>A Russell's viper</figcaption>\
                    </figure><div class=\"thumb\"><div class=\"thumbcaption\">Map of snakes</div></div>";
        assert_eq!(text(html), "A Russell's viper\nMap of snakes");
    }

    #[test]
    fn closes_implied_paragraphs() {
        assert_eq!(text("<p>One<p>Two<div>Three</div>"), "One\nTwo\nThree");
        assert_eq!(text("<p>Unclosed <b>bold"), "Unclosed bold");
        assert_eq!(text("</div></p>Stray end tags</span>"), "Stray end tags");
    }

    #[test]
    fn survives_hostile_html() {
        let deep = "<div>".repeat(100_000) + "deep" + &"</div>".repeat(100_000);
        assert_eq!(text(&deep), "deep");
        let skipped_deep = "<div class=\"navbox\">".to_string() + &"<span>".repeat(10_000) + "x</div><p>after</p>";
        assert!(!text(&skipped_deep).contains('x'));
        for html in [
            "<p>unterminated <!-- comment",
            "<script>never closed <p>text",
            "<p a=\"unterminated attribute>text",
            "<<<>>><</>&&&&#;&#x;&#99999999999;</</p>",
            "<h2>heading never closed<p>text",
            "<title>title never closed",
            "\u{0}\u{fffd}<p>\u{0}</p>",
        ] {
            let _ = extract(html);
        }
        let attrs = format!("<p class=\"{}\">text</p>", "x ".repeat(100_000));
        assert_eq!(text(&attrs), "text");
        let mut noise = String::new();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..200_000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let symbols = [
                '<', '>', '/', 'p', 'd', 'i', 'v', '"', '=', ' ', '&', ';', 'h', '2', '!', '-',
            ];
            noise.push(symbols[seed as usize % symbols.len()]);
        }
        let _ = extract(&noise);
    }

    #[test]
    fn cutting_the_dropped_tail_changes_nothing() {
        let pages = [
            // The usual ending: cut at "References".
            (
                "<h2>Treatment</h2><p>Keep.</p><div class=\"mw-heading\"><h2 id=\"References\">References\
              <span class=\"mw-editsection\">edit</span></h2></div><ol><li>Ref</li></ol>\
              <h2>External links</h2><ul><li>Link</li></ul><div class=\"navbox\">Nav</div>",
                Some("<div class=\"mw-heading\">"),
            ),
            // A real section after the references: no cut.
            ("<h2>References</h2><p>Junk.</p><h2>History</h2><p>Kept.</p>", None),
            ("<h2>References</h2><p>Junk.</p><H2>History</H2><p>Kept.</p>", None),
            // An <h1> after them: no cut, it isn't a dropped section.
            ("<p>Intro.</p><h2>Notes</h2><p>Junk.</p><h1>Title</h1>", None),
            // Subsections don't end a dropped section, so they are cut with it.
            (
                "<p>Intro.</p><h2>See also</h2><h3>More</h3><p>Junk.</p>",
                Some("<p>Intro.</p>"),
            ),
            // A heading that never closes is content.
            ("<p>Intro.</p><h2>References", None),
        ];
        for (html, kept) in pages {
            let cut = droppable_tail(html);
            match kept {
                Some(prefix) => assert!(cut < html.len() && html[..cut].ends_with(prefix), "{html}"),
                None => assert_eq!(cut, html.len(), "{html}"),
            }
            assert_eq!(extract(html), tokenize(html), "{html}");
        }
        // Hostile: a page made of nothing but reference headings stays fast.
        let many = "<h2>References</h2>".repeat(500_000);
        assert!(droppable_tail(&many) > 0);
    }

    fn sentence(i: usize) -> String {
        format!("Sentence number {i} says something useful about snakes. ")
    }

    #[test]
    fn passages_group_paragraphs_up_to_the_limit() {
        let para = |n: usize| (0..n).map(sentence).collect::<String>();
        let html = format!(
            "<p>Short intro.</p><h2>Treatment</h2><p>{}</p><p>{}</p><p>{}</p><h3>Antivenom</h3><p>Use it.</p>\
             <h2>History</h2><p>Old.</p>",
            para(10),
            para(10),
            para(10)
        );
        let passages = extract(&html).passages();
        let summary: Vec<_> = passages
            .iter()
            .map(|p| (p.heading.as_str(), p.text.chars().count()))
            .collect();
        // Each paragraph is ~570 characters, so two fit in a passage and the third starts another.
        assert_eq!(summary.len(), 5, "{summary:?}");
        assert_eq!(summary[0], ("", 12));
        assert_eq!(summary[1].0, "Treatment");
        assert!(summary[1].1 > 1000 && summary[1].1 <= PASSAGE_MAX);
        assert_eq!(passages[1].text.matches('\n').count(), 1);
        assert_eq!(summary[2].0, "Treatment");
        assert_eq!(summary[3], ("Treatment > Antivenom", 7));
        assert_eq!(summary[4], ("History", 4));
    }

    #[test]
    fn long_paragraphs_split_at_sentences() {
        let long: String = (0..100).map(sentence).collect();
        let passages = extract(&format!("<p>{long}</p>")).passages();
        assert!(passages.len() >= 4);
        for (i, p) in passages.iter().enumerate() {
            let n = p.text.chars().count();
            assert!(n <= PASSAGE_MAX, "{n}");
            assert!(p.text.ends_with('.') || i + 1 == passages.len(), "{}", p.text);
        }
        let rejoined: Vec<_> = passages.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(rejoined.join(" "), long.trim_end());

        // No sentence end or space at all: cut at the limit, on a character boundary.
        let word = "é".repeat(4000);
        let passages = extract(&format!("<p>{word}</p>")).passages();
        let lengths: Vec<_> = passages.iter().map(|p| p.text.chars().count()).collect();
        assert_eq!(lengths, [1500, 1500, 1000]);
    }
}

//! Text decoding, the clean text format, passages, and Markdown and plain text readers.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::TextEncoding;

/// Passages are at least this many characters, unless a section is shorter.
pub const MIN_PASSAGE: usize = 500;
/// Passages are at most this many characters.
pub const MAX_PASSAGE: usize = 1500;
/// A passage is closed once it reaches this size, so most land near the middle of the range.
const TARGET_PASSAGE: usize = 1000;

// ---------------------------------------------------------------------------------------------
// Decoding

/// Windows-1252 characters for bytes 0x80–0x9f. The five unassigned bytes stay C1 controls.
const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{81}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2c6}', '\u{2030}',
    '\u{160}', '\u{2039}', '\u{152}', '\u{8d}', '\u{17d}', '\u{8f}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201c}',
    '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}', '\u{9d}',
    '\u{17e}', '\u{178}',
];

fn latin1(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if (0x80..0xa0).contains(&b) {
                CP1252_HIGH[b as usize - 0x80]
            } else {
                b as char
            }
        })
        .collect()
}

fn utf16(bytes: &[u8], little_endian: bool) -> String {
    let units = bytes.chunks_exact(2).map(|p| {
        if little_endian {
            u16::from_le_bytes([p[0], p[1]])
        } else {
            u16::from_be_bytes([p[0], p[1]])
        }
    });
    char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect()
}

/// Decodes a whole text file: UTF-8 or UTF-16 with a byte order mark, UTF-8 if valid, else
/// Windows-1252 (which every byte sequence is).
pub(crate) fn decode(bytes: &[u8]) -> (String, TextEncoding) {
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return (String::from_utf8_lossy(rest).into_owned(), TextEncoding::Utf8);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return (utf16(rest, true), TextEncoding::Utf16Le);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return (utf16(rest, false), TextEncoding::Utf16Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), TextEncoding::Utf8),
        Err(_) => (latin1(bytes), TextEncoding::Latin1),
    }
}

/// Decodes the start of a file if it looks like text, or returns `None` for binary data.
/// The head may end in the middle of a UTF-8 character.
pub(crate) fn decode_head(head: &[u8]) -> Option<(String, TextEncoding)> {
    let (s, encoding) = if head.starts_with(b"\xff\xfe") || head.starts_with(b"\xfe\xff") {
        decode(head)
    } else {
        let body = head.strip_prefix(b"\xef\xbb\xbf").unwrap_or(head);
        if body.contains(&0) {
            return None;
        }
        match std::str::from_utf8(body) {
            Ok(s) => (s.to_string(), TextEncoding::Utf8),
            // Cut short at the end of the head: still UTF-8.
            Err(e) if e.error_len().is_none() => (
                String::from_utf8_lossy(&body[..e.valid_up_to()]).into_owned(),
                TextEncoding::Utf8,
            ),
            Err(_) => (latin1(body), TextEncoding::Latin1),
        }
    };
    // Text has almost no control characters besides tabs, line breaks and form feeds.
    let total = s.chars().count();
    let controls = s
        .chars()
        .filter(|&c| c.is_control() && !matches!(c, '\t' | '\n' | '\r' | '\x0c'))
        .count();
    if total == 0 && !head.is_empty() || controls * 50 > total {
        return None;
    }
    Some((s, encoding))
}

/// Collapses all whitespace runs to one space and trims.
pub(crate) fn squash_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------------------------------------
// The clean text format (see the crate docs)

/// Builds clean text block by block, stopping at a character limit.
pub(crate) struct DocBuilder {
    out: String,
    chars: usize,
    max_chars: usize,
    pub(crate) truncated: bool,
}

impl DocBuilder {
    pub(crate) fn new(max_chars: usize) -> DocBuilder {
        DocBuilder {
            out: String::new(),
            chars: 0,
            max_chars,
            truncated: false,
        }
    }

    pub(crate) fn is_full(&self) -> bool {
        self.truncated
    }

    pub(crate) fn heading(&mut self, level: u8, text: &str) {
        let text = squash_spaces(&text.replace('\x0c', " "));
        if !text.is_empty() {
            let level = level.clamp(1, 6) as usize;
            self.push(&format!("{} {text}", "#".repeat(level)));
        }
    }

    /// Adds a paragraph. Line breaks inside are kept; blank lines and control characters are
    /// dropped so the paragraph stays one block.
    pub(crate) fn para(&mut self, text: &str) {
        let mut block = String::with_capacity(text.len());
        for line in text.lines() {
            let line: String = line.chars().filter(|&c| !c.is_control() || c == '\t').collect();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if !block.is_empty() {
                block.push('\n');
            }
            if line.starts_with(['#', '\\']) {
                block.push('\\');
            }
            block.push_str(line);
        }
        if !block.is_empty() {
            self.push(&block);
        }
    }

    pub(crate) fn page_break(&mut self) {
        self.push("\x0c");
    }

    fn push(&mut self, block: &str) {
        if self.truncated {
            return;
        }
        let n = block.chars().count();
        if self.chars + n > self.max_chars {
            self.truncated = true;
            return;
        }
        if !self.out.is_empty() {
            self.out.push_str("\n\n");
        }
        self.out.push_str(block);
        self.chars += n + 2;
    }

    pub(crate) fn finish(self) -> String {
        // Page breaks at the very end say nothing.
        let mut out = self.out;
        while out.ends_with('\x0c') {
            out.pop();
            while out.ends_with('\n') {
                out.pop();
            }
        }
        out
    }
}

enum Block<'a> {
    Heading(u8, &'a str),
    Page,
    Para(String),
}

fn parse_block(block: &str) -> Block<'_> {
    if block == "\x0c" {
        return Block::Page;
    }
    let hashes = block.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && block[hashes..].starts_with(' ') && !block.contains('\n') {
        return Block::Heading(hashes as u8, block[hashes + 1..].trim());
    }
    let para = block
        .lines()
        .map(|l| l.strip_prefix('\\').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    Block::Para(para)
}

// ---------------------------------------------------------------------------------------------
// Passages

/// A piece of text small enough to give the AI and to rank in search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Passage {
    /// The section it belongs to: the heading path, such as `Water > Boiling`.
    pub heading: Option<String>,
    /// The PDF page it starts on (1-based), for citations.
    pub page: Option<u32>,
    pub text: String,
}

/// Cuts clean text (as in [`Extracted::text`](crate::Extracted::text)) into passages of
/// [`MIN_PASSAGE`] to [`MAX_PASSAGE`] characters. Cuts fall between paragraphs; a passage never
/// spans two sections, so the last one of a short section can be shorter. Paragraphs longer
/// than the maximum are cut after a sentence, or else at a space.
pub fn passages(text: &str) -> Vec<Passage> {
    let paged = text.starts_with('\x0c') || text.contains("\n\x0c\n");
    let mut s = Splitter {
        out: Vec::new(),
        buf: String::new(),
        buf_chars: 0,
        buf_page: None,
        page: if paged { Some(1) } else { None },
        trail: Vec::new(),
        section: 0,
        last_section: None,
    };
    for block in text.split("\n\n") {
        match parse_block(block) {
            Block::Page => s.page = s.page.map(|p| p + 1),
            Block::Heading(level, title) => {
                s.flush();
                s.trail.retain(|(l, _)| *l < level);
                s.trail.push((level, title.to_string()));
                s.section += 1;
            }
            Block::Para(p) => s.add_para(&p),
        }
    }
    s.flush();
    s.out
}

struct Splitter {
    out: Vec<Passage>,
    buf: String,
    buf_chars: usize,
    buf_page: Option<u32>,
    page: Option<u32>,
    trail: Vec<(u8, String)>,
    section: usize,
    last_section: Option<usize>,
}

impl Splitter {
    fn heading(&self) -> Option<String> {
        if self.trail.is_empty() {
            None
        } else {
            Some(
                self.trail
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" > "),
            )
        }
    }

    fn add_para(&mut self, p: &str) {
        if p.chars().count() > MAX_PASSAGE {
            self.flush();
            for piece in split_long(p) {
                self.add_piece(piece);
            }
        } else {
            self.add_piece(p);
        }
    }

    fn add_piece(&mut self, p: &str) {
        let n = p.chars().count();
        if n == 0 {
            return;
        }
        if self.buf_chars > 0 && self.buf_chars + 2 + n > MAX_PASSAGE {
            self.flush();
        }
        if self.buf.is_empty() {
            self.buf_page = self.page;
        } else {
            self.buf.push_str("\n\n");
            self.buf_chars += 2;
        }
        self.buf.push_str(p);
        self.buf_chars += n;
        if self.buf_chars >= TARGET_PASSAGE {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.buf);
        let n = std::mem::replace(&mut self.buf_chars, 0);
        // A short tail joins the previous passage of the same section when it fits.
        if n < MIN_PASSAGE && self.last_section == Some(self.section) {
            if let Some(last) = self.out.last_mut() {
                if last.text.chars().count() + 2 + n <= MAX_PASSAGE {
                    last.text.push_str("\n\n");
                    last.text.push_str(&text);
                    return;
                }
            }
        }
        self.out.push(Passage {
            heading: self.heading(),
            page: self.buf_page,
            text,
        });
        self.last_section = Some(self.section);
    }
}

/// Cuts a long paragraph into pieces of at most [`MAX_PASSAGE`] characters.
fn split_long(p: &str) -> Vec<&str> {
    let mut pieces = Vec::new();
    let mut rest = p.trim();
    while rest.chars().count() > MAX_PASSAGE {
        let byte_at = |n: usize| rest.char_indices().nth(n).map_or(rest.len(), |(i, _)| i);
        let (lo, hi) = (byte_at(MIN_PASSAGE), byte_at(MAX_PASSAGE));
        let window = &rest[lo..hi];
        let sentence = [". ", "? ", "! ", ".\n", "\n", "; "]
            .iter()
            .filter_map(|end| window.rfind(end).map(|i| lo + i + 1))
            .max();
        let cut = sentence
            .or_else(|| window.rfind(char::is_whitespace).map(|i| lo + i))
            .unwrap_or(hi);
        let cut = if cut == 0 { hi } else { cut };
        pieces.push(rest[..cut].trim_end());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

// ---------------------------------------------------------------------------------------------
// Plain text and Markdown

/// Plain text: paragraphs are separated by blank lines (or form feeds); line breaks are kept.
pub(crate) fn plain_to_blocks(s: &str, doc: &mut DocBuilder) {
    let mut para = String::new();
    for line in s.split(['\n', '\x0c']) {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            if !para.is_empty() {
                doc.para(&para);
                para.clear();
            }
            if doc.is_full() {
                return;
            }
        } else {
            if !para.is_empty() {
                para.push('\n');
            }
            para.push_str(line);
        }
    }
    doc.para(&para);
}

fn level(h: HeadingLevel) -> u8 {
    match h {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Markdown, read with pulldown-cmark. Raw HTML inside it is dropped.
pub(crate) fn markdown_to_blocks(s: &str, doc: &mut DocBuilder) {
    let mut buf = String::new();
    let flush = |buf: &mut String, doc: &mut DocBuilder| {
        doc.para(buf);
        buf.clear();
    };
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_FOOTNOTES;
    let mut lists = 0usize;
    for event in Parser::new_ext(s, options) {
        if doc.is_full() {
            return;
        }
        match event {
            Event::Start(Tag::Heading { .. }) => flush(&mut buf, doc),
            Event::End(TagEnd::Heading(h)) => {
                doc.heading(level(h), &buf);
                buf.clear();
            }
            Event::Start(Tag::List(_)) => {
                if lists == 0 {
                    flush(&mut buf, doc);
                }
                lists += 1;
            }
            Event::End(TagEnd::List(_)) => {
                lists = lists.saturating_sub(1);
                if lists == 0 {
                    flush(&mut buf, doc);
                }
            }
            Event::Start(Tag::Item) => {
                if !buf.is_empty() && !buf.ends_with('\n') {
                    buf.push('\n');
                }
                buf.push_str("- ");
            }
            Event::Start(Tag::CodeBlock(_)) | Event::Start(Tag::BlockQuote(_)) | Event::Start(Tag::Table(_)) => {
                flush(&mut buf, doc)
            }
            Event::End(TagEnd::Paragraph) => {
                // Paragraphs inside list items stay with their list.
                if lists == 0 {
                    flush(&mut buf, doc);
                } else {
                    buf.push('\n');
                }
            }
            Event::End(TagEnd::CodeBlock) | Event::End(TagEnd::BlockQuote(_)) | Event::End(TagEnd::Table) => {
                flush(&mut buf, doc)
            }
            Event::End(TagEnd::TableCell) => buf.push_str(" | "),
            Event::End(TagEnd::TableRow) | Event::End(TagEnd::TableHead) => {
                let trimmed = buf.trim_end_matches([' ', '|']).len();
                buf.truncate(trimmed);
                buf.push('\n');
            }
            Event::Text(t) | Event::Code(t) => buf.push_str(&t),
            Event::SoftBreak => buf.push(' '),
            Event::HardBreak | Event::Rule => buf.push('\n'),
            _ => {}
        }
    }
    flush(&mut buf, doc);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin1_and_utf16() {
        assert_eq!(
            decode(b"caf\xe9 \x93ok\x94"),
            ("café “ok”".to_string(), TextEncoding::Latin1)
        );
        assert_eq!(decode(b"caf\xc3\xa9").1, TextEncoding::Utf8);
        assert_eq!(decode(b"\xff\xfeh\0i\0"), ("hi".to_string(), TextEncoding::Utf16Le));
        assert!(decode_head(b"ok\x01\x02\x03\x04\x05").is_none());
        // A head cut inside a two-byte character is still UTF-8.
        assert_eq!(decode_head(b"caf\xc3").unwrap().1, TextEncoding::Utf8);
    }

    #[test]
    fn clean_text_round_trip() {
        let mut d = DocBuilder::new(1000);
        d.heading(2, "  Water\n  safety ");
        d.para("# not a heading\n\n\\ slash\nline two");
        d.page_break();
        d.para("next page");
        let text = d.finish();
        assert_eq!(
            text,
            "## Water safety\n\n\\# not a heading\n\\\\ slash\nline two\n\n\x0c\n\nnext page"
        );
        let p = passages(&text);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].heading.as_deref(), Some("Water safety"));
        assert_eq!(p[0].page, Some(1));
        assert_eq!(p[0].text, "# not a heading\n\\ slash\nline two\n\nnext page");
    }

    #[test]
    fn passage_sizes() {
        let sentence = "Boil water for one minute to kill germs. ";
        let mut d = DocBuilder::new(1_000_000);
        d.heading(1, "Water");
        for i in 0..40 {
            d.para(&sentence.repeat(1 + i % 7));
        }
        d.heading(2, "Long");
        d.para(&sentence.repeat(200));
        d.heading(1, "Short");
        d.para("Tiny section.");
        let p = passages(&d.finish());
        let last = p.last().unwrap();
        assert_eq!(last.heading.as_deref(), Some("Short"));
        assert_eq!(last.text, "Tiny section.");
        for x in &p[..p.len() - 1] {
            let n = x.text.chars().count();
            assert!((MIN_PASSAGE..=MAX_PASSAGE).contains(&n), "passage of {n} chars");
        }
        assert!(p.iter().any(|x| x.heading.as_deref() == Some("Water > Long")));
        // Long paragraphs are cut after a sentence.
        assert!(p
            .iter()
            .filter(|x| x.heading.as_deref() == Some("Water > Long"))
            .all(|x| x.text.ends_with('.')));
    }

    #[test]
    fn truncation() {
        let mut d = DocBuilder::new(10);
        d.para("12345");
        d.para("6789012345");
        assert!(d.truncated);
        assert_eq!(d.finish(), "12345");
    }

    #[test]
    fn markdown() {
        let mut d = DocBuilder::new(10_000);
        markdown_to_blocks("# Title\n\nSome *text*\nwrapped.\n\n- one\n- two\n\n<script>x</script>\n\n## Sub\n\n| a | b |\n|---|---|\n| 1 | 2 |\n", &mut d);
        assert_eq!(
            d.finish(),
            "# Title\n\nSome text wrapped.\n\n- one\n- two\n\n## Sub\n\na | b\n1 | 2"
        );
    }
}

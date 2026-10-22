//! PDF text extraction with pdf-extract (on top of lopdf), page by page.
//!
//! pdf-extract gives positioned characters; this module turns them into lines, paragraphs (from
//! the vertical gaps) and headings (lines in a larger font than the body text). Nothing in a
//! PDF is ever run: scripts, forms and links are ignored.

use std::collections::{HashMap, HashSet};
use std::panic::{self, AssertUnwindSafe};

use pdf_extract::{Dictionary, Document, MediaBox, Object, ObjectId, OutputDev, OutputError, Transform};

use crate::guard::Cancel;
use crate::text::DocBuilder;
use crate::{ExtractError, Limits};

/// Deepest chain of form XObjects drawing each other. Real files use a handful of levels; each
/// level is a recursive call inside pdf-extract.
const MAX_XOBJECT_DEPTH: u32 = 32;
/// Deepest page tree (pdf-extract climbs `Parent` links recursively).
const MAX_PAGE_TREE_DEPTH: usize = 256;

/// PDF names that start scripts or programs when a viewer opens the file.
const ACTIVE_NAMES: &[&[u8]] = &[b"JavaScript", b"JS", b"Launch", b"EmbeddedFile", b"RichMedia", b"XFA"];

/// Best-effort scan for script or launch actions. Names inside compressed object streams are
/// not seen.
pub(crate) fn has_active_content(bytes: &[u8]) -> bool {
    let mut i = 0;
    while let Some(p) = bytes[i..].iter().position(|&b| b == b'/') {
        let start = i + p + 1;
        let end = bytes[start..]
            .iter()
            .position(|&b| !is_regular(b))
            .map_or(bytes.len(), |e| start + e);
        if ACTIVE_NAMES.contains(&&bytes[start..end]) {
            return true;
        }
        i = end.max(start);
    }
    false
}

/// Characters that may appear in a PDF name or operator (not whitespace or a delimiter).
fn is_regular(b: u8) -> bool {
    !matches!(
        b,
        b'\0'
            | b'\t'
            | b'\n'
            | b'\x0c'
            | b'\r'
            | b' '
            | b'('
            | b')'
            | b'<'
            | b'>'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'/'
            | b'%'
    )
}

pub(crate) struct PdfInfo {
    pub(crate) title: Option<String>,
    pub(crate) pages: u32,
    pub(crate) failed_pages: u32,
    pub(crate) truncated: bool,
}

pub(crate) fn to_blocks(
    bytes: &[u8],
    limits: &Limits,
    doc: &mut DocBuilder,
    cancel: &Cancel,
) -> Result<PdfInfo, ExtractError> {
    let malformed = |e: pdf_extract::Error| ExtractError::Malformed(format!("PDF: {e}"));
    let mut pdf = Document::load_mem(bytes).map_err(malformed)?;
    if pdf.is_encrypted() {
        // Many PDFs are "encrypted" with an empty password only to set permissions.
        pdf.decrypt("")
            .map_err(|_| ExtractError::Malformed("PDF needs a password".into()))?;
    }
    cancel.check()?;
    let pages: Vec<u32> = pdf.get_pages().keys().copied().collect();
    if pages.is_empty() {
        return Err(ExtractError::Malformed("PDF has no pages".into()));
    }
    check_page_tree(&pdf)?;
    check_xobjects(&pdf)?;
    cancel.check()?;

    let mut collector = Collector::new(cancel, limits.max_text_chars);
    let mut failed_pages = 0;
    for &page in pages.iter().take(limits.max_pdf_pages as usize) {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            pdf_extract::output_doc_page(&pdf, &mut collector, page)
        }));
        collector.close_line();
        match collector.stop {
            Some(Stop::Cancelled) => return Err(ExtractError::TimedOut),
            Some(Stop::Full) => break,
            None => {}
        }
        // A page that errs or panics loses its text, not the whole document.
        if !matches!(result, Ok(Ok(()))) {
            failed_pages += 1;
        }
    }
    let truncated = collector.stop == Some(Stop::Full) || pages.len() > limits.max_pdf_pages as usize;
    layout(&collector.lines, doc);
    Ok(PdfInfo {
        title: info_title(&pdf),
        pages: pages.len() as u32,
        failed_pages,
        truncated,
    })
}

// ---------------------------------------------------------------------------------------------
// Checks for unbounded recursion in pdf-extract. A stack overflow aborts the process, so these
// must be refused before drawing.

fn refuse(why: &str) -> ExtractError {
    ExtractError::Malformed(format!("PDF refused: {why}"))
}

/// pdf-extract looks up inherited page attributes by climbing `Parent` links recursively.
fn check_page_tree(pdf: &Document) -> Result<(), ExtractError> {
    for &id in pdf.get_pages().values() {
        let mut current = id;
        for depth in 0.. {
            if depth > MAX_PAGE_TREE_DEPTH {
                return Err(refuse("page tree loops or is too deep"));
            }
            match pdf
                .get_dictionary(current)
                .and_then(|d| d.get(b"Parent"))
                .and_then(Object::as_reference)
            {
                Ok(parent) => current = parent,
                Err(_) => break,
            }
        }
    }
    Ok(())
}

fn resolve<'a>(pdf: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => pdf.get_object(*id).ok(),
        other => Some(other),
    }
}

fn resolve_dict<'a>(pdf: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    match resolve(pdf, object)? {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// The `XObject` entries (name → object) of a resource dictionary.
fn xobjects_of<'a>(pdf: &'a Document, resources: &'a Dictionary) -> Vec<(&'a [u8], ObjectId)> {
    let Some(dict) = resources.get(b"XObject").ok().and_then(|x| resolve_dict(pdf, x)) else {
        return Vec::new();
    };
    dict.iter()
        .filter_map(|(name, o)| o.as_reference().ok().map(|id| (name.as_slice(), id)))
        .collect()
}

/// pdf-extract draws an XObject (`/Name Do`) by recursing into it, with no cycle check, and
/// treats every XObject as a form, images included. An XObject without its own resources
/// uses its caller's. This builds the graph of which XObject can draw which, by name, and
/// refuses cycles and very deep chains.
fn check_xobjects(pdf: &Document) -> Result<(), ExtractError> {
    // Every name → XObject binding in every resource dictionary.
    let mut by_name: HashMap<&[u8], HashSet<ObjectId>> = HashMap::new();
    for object in pdf.objects.values() {
        let dict = match object {
            Object::Dictionary(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        let mut bindings = xobjects_of(pdf, dict);
        if let Some(resources) = dict.get(b"Resources").ok().and_then(|r| resolve_dict(pdf, r)) {
            bindings.extend(xobjects_of(pdf, resources));
        }
        for (name, id) in bindings {
            by_name.entry(name).or_default().insert(id);
        }
    }
    let all: HashSet<ObjectId> = by_name.values().flatten().copied().collect();

    let mut edges: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
    for &id in &all {
        let Ok(Object::Stream(stream)) = pdf.get_object(id) else {
            continue;
        };
        // The same bytes pdf-extract would run as content.
        let content = if stream.filters().is_ok() {
            stream.decompressed_content().unwrap_or_else(|_| stream.content.clone())
        } else {
            stream.content.clone()
        };
        let called = drawn_names(&content);
        if called.is_empty() {
            continue;
        }
        let own = stream.dict.get(b"Resources").ok().and_then(|r| resolve_dict(pdf, r));
        let targets: Vec<ObjectId> = match own {
            Some(resources) => xobjects_of(pdf, resources)
                .into_iter()
                .filter(|(n, _)| called.contains(n))
                .map(|(_, t)| t)
                .collect(),
            // Inherited resources: any XObject bound to one of these names anywhere.
            None => called
                .iter()
                .filter_map(|n| by_name.get(n))
                .flatten()
                .copied()
                .collect(),
        };
        edges.insert(id, targets);
    }
    check_graph(&edges)
}

/// Names used with the `Do` operator (`/Name Do`).
fn drawn_names(content: &[u8]) -> HashSet<&[u8]> {
    let mut names = HashSet::new();
    let mut i = 0;
    while let Some(p) = content.get(i..).and_then(|c| c.windows(2).position(|w| w == b"Do")) {
        let at = i + p;
        i = at + 2;
        let after_ok = content.get(at + 2).is_none_or(|&b| !is_regular(b));
        let before = &content[..at];
        let trimmed = before.len() - before.iter().rev().take_while(|b| b.is_ascii_whitespace()).count();
        if !after_ok || trimmed == before.len() {
            continue;
        }
        let start = before[..trimmed]
            .iter()
            .rposition(|&b| !is_regular(b))
            .map_or(0, |s| s + 1);
        if start > 0 && before[start - 1] == b'/' {
            names.insert(&before[start..trimmed]);
        }
    }
    names
}

/// Refuses cycles and chains deeper than [`MAX_XOBJECT_DEPTH`], without recursing itself.
fn check_graph(edges: &HashMap<ObjectId, Vec<ObjectId>>) -> Result<(), ExtractError> {
    // On the stack while being explored, or done with its chain depth.
    enum State {
        Open,
        Done(u32),
    }
    let mut state: HashMap<ObjectId, State> = HashMap::new();
    for &start in edges.keys() {
        if state.contains_key(&start) {
            continue;
        }
        state.insert(start, State::Open);
        let mut stack: Vec<(ObjectId, usize)> = vec![(start, 0)];
        while let Some(top) = stack.last_mut() {
            let (node, i) = *top;
            top.1 += 1;
            let children = edges.get(&node).map_or(&[][..], Vec::as_slice);
            if let Some(&child) = children.get(i) {
                match state.get(&child) {
                    Some(State::Open) => return Err(refuse("drawing loops back on itself")),
                    Some(State::Done(_)) => {}
                    None => {
                        state.insert(child, State::Open);
                        stack.push((child, 0));
                    }
                }
            } else {
                let deepest = children
                    .iter()
                    .map(|c| match state.get(c) {
                        Some(State::Done(d)) => *d,
                        _ => 0,
                    })
                    .max()
                    .unwrap_or(0);
                if deepest + 1 > MAX_XOBJECT_DEPTH {
                    return Err(refuse("drawing nests too deep"));
                }
                state.insert(node, State::Done(deepest + 1));
                stack.pop();
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Title

fn info_title(pdf: &Document) -> Option<String> {
    let info = resolve_dict(pdf, pdf.trailer.get(b"Info").ok()?)?;
    match resolve(pdf, info.get(b"Title").ok()?)? {
        Object::String(bytes, _) => Some(text_string(bytes)),
        _ => None,
    }
}

/// A PDF text string: UTF-16BE with a byte order mark, UTF-8 with one (PDF 2.0), or
/// PDFDocEncoding, read as Latin-1 (they differ only in rare punctuation).
fn text_string(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(b"\xfe\xff") {
        let units = rest.chunks_exact(2).map(|p| u16::from_be_bytes([p[0], p[1]]));
        char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect()
    } else if let Some(rest) = b.strip_prefix(b"\xef\xbb\xbf") {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

// ---------------------------------------------------------------------------------------------
// Collecting lines

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancelled,
    Full,
}

struct Line {
    page: u32,
    text: String,
    /// Largest font size on the line, in points after scaling.
    size: f64,
    /// Vertical distance from the previous line, in units of this line's font size. Infinite
    /// for the first line of a page.
    gap: f64,
}

/// Receives characters from pdf-extract and groups them into lines. Returning an error from a
/// callback stops pdf-extract, which is how the deadline and the text limit are enforced.
struct Collector<'c> {
    cancel: &'c Cancel,
    lines: Vec<Line>,
    line: String,
    line_size: f64,
    line_gap: f64,
    page: u32,
    flip: Transform,
    last_end: f64,
    last_y: f64,
    first_char: bool,
    chars: usize,
    max_chars: usize,
    stop: Option<Stop>,
}

impl<'c> Collector<'c> {
    fn new(cancel: &'c Cancel, max_chars: usize) -> Collector<'c> {
        Collector {
            cancel,
            lines: Vec::new(),
            line: String::new(),
            line_size: 0.0,
            line_gap: f64::INFINITY,
            page: 0,
            flip: Transform::identity(),
            last_end: f64::INFINITY,
            last_y: 0.0,
            first_char: false,
            chars: 0,
            max_chars,
            stop: None,
        }
    }

    fn close_line(&mut self) {
        let text = self.line.trim();
        if !text.is_empty() {
            self.lines.push(Line {
                page: self.page,
                text: text.to_string(),
                size: self.line_size,
                gap: self.line_gap,
            });
        }
        self.line.clear();
        self.line_size = 0.0;
    }

    fn halt(&mut self, why: Stop) -> Result<(), OutputError> {
        self.stop = Some(why);
        Err(OutputError::FormatError(std::fmt::Error))
    }
}

impl OutputDev for Collector<'_> {
    fn begin_page(
        &mut self,
        page: u32,
        media_box: &MediaBox,
        _: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), OutputError> {
        self.close_line();
        self.page = page;
        self.line_gap = f64::INFINITY;
        // PDF y grows upwards; flip it so lines go down the page.
        self.flip = Transform::row_major(1., 0., 0., -1., 0., media_box.ury - media_box.lly);
        self.last_end = f64::INFINITY;
        self.first_char = false;
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), OutputError> {
        self.close_line();
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &Transform,
        width: f64,
        _: f64,
        font_size: f64,
        c: &str,
    ) -> Result<(), OutputError> {
        if self.chars % 256 == 0 && self.cancel.stopped() {
            return self.halt(Stop::Cancelled);
        }
        let position = trm.post_transform(&self.flip);
        let (x, y) = (position.m31, position.m32);
        // The rendered size: the font size through the text matrix, as the side of a square.
        let sx = font_size * (trm.m11 + trm.m21);
        let sy = font_size * (trm.m12 + trm.m22);
        let mut size = (sx * sy).abs().sqrt();
        if !size.is_finite() || size < 0.1 {
            size = 1.0;
        }
        if self.first_char && !self.line.is_empty() {
            let dy = (y - self.last_y).abs();
            if dy > size * 1.5 || (x < self.last_end && dy > size * 0.5) {
                self.close_line();
                self.line_gap = dy / size;
            } else if x > self.last_end + size * 0.1 && !self.line.ends_with(' ') {
                self.line.push(' ');
            }
        }
        let n = c.chars().count();
        if self.chars + n > self.max_chars {
            return self.halt(Stop::Full);
        }
        self.chars += n;
        self.line.push_str(c);
        self.line_size = self.line_size.max(size);
        self.first_char = false;
        self.last_y = y;
        self.last_end = x + width * size;
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), OutputError> {
        self.first_char = true;
        Ok(())
    }

    fn end_word(&mut self) -> Result<(), OutputError> {
        Ok(())
    }

    fn end_line(&mut self) -> Result<(), OutputError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Layout: lines to paragraphs and headings

/// Rounds a size so near-equal sizes count as one.
fn bucket(x: f64) -> i64 {
    (x * 2.0).round() as i64
}

/// The most common value (weighted), by bucket.
fn mode(values: impl Iterator<Item = (f64, usize)>) -> Option<f64> {
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for (v, w) in values {
        *counts.entry(bucket(v)).or_default() += w;
    }
    counts
        .into_iter()
        .max_by_key(|&(b, n)| (n, -b))
        .map(|(b, _)| b as f64 / 2.0)
}

fn layout(lines: &[Line], doc: &mut DocBuilder) {
    // Body text is the size most characters are set in, and normal line spacing is the most
    // common gap between lines: anything clearly wider starts a paragraph.
    let body = mode(lines.iter().map(|l| (l.size, l.text.len()))).unwrap_or(0.0);
    let spacing = mode(lines.iter().filter(|l| l.gap < 3.0).map(|l| (l.gap, 1))).unwrap_or(1.2);
    let para_gap = (spacing * 1.25).max(spacing + 0.25);
    let is_heading = |l: &Line| {
        body > 0.0 && l.size >= body * 1.2 && l.text.chars().count() <= 150 && l.text.chars().any(char::is_alphabetic)
    };
    // The three largest heading sizes become levels 1–3.
    let mut sizes: Vec<i64> = lines.iter().filter(|l| is_heading(l)).map(|l| bucket(l.size)).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    let level = |l: &Line| {
        sizes
            .iter()
            .position(|&s| s == bucket(l.size))
            .map_or(3, |i| (i + 1).min(3)) as u8
    };

    let mut para = String::new();
    let mut heading: Option<(u8, i64, String)> = None;
    let mut page = lines.first().map_or(1, |l| l.page);
    for line in lines {
        if doc.is_full() {
            return;
        }
        if line.page != page {
            flush_heading(&mut heading, doc);
            flush_para(&mut para, doc);
            for _ in page..line.page {
                doc.page_break();
            }
            page = line.page;
        }
        if is_heading(line) {
            flush_para(&mut para, doc);
            let size = bucket(line.size);
            match &mut heading {
                // A heading set over two lines.
                Some((_, s, text)) if *s == size && line.gap < 3.0 => {
                    text.push(' ');
                    text.push_str(&line.text);
                }
                _ => {
                    flush_heading(&mut heading, doc);
                    heading = Some((level(line), size, line.text.clone()));
                }
            }
            continue;
        }
        flush_heading(&mut heading, doc);
        if line.gap > para_gap {
            flush_para(&mut para, doc);
        }
        join_line(&mut para, &line.text);
    }
    flush_heading(&mut heading, doc);
    flush_para(&mut para, doc);
}

fn flush_heading(heading: &mut Option<(u8, i64, String)>, doc: &mut DocBuilder) {
    if let Some((level, _, text)) = heading.take() {
        doc.heading(level, &text);
    }
}

fn flush_para(para: &mut String, doc: &mut DocBuilder) {
    doc.para(para);
    para.clear();
}

/// Joins a layout line to its paragraph. A line ending in a hyphen inside a word is joined
/// without a space but keeps the hyphen: browsers and word processors (most uploads) only
/// break lines at real hyphens ("self-" + "contained"); typeset books that split words
/// ("exam-" + "ple") are the rarer case.
fn join_line(para: &mut String, line: &str) {
    let in_word = para.ends_with('-')
        && para.chars().rev().nth(1).is_some_and(char::is_alphabetic)
        && line.chars().next().is_some_and(char::is_lowercase);
    if !para.is_empty() && !in_word {
        para.push(' ');
    }
    para.push_str(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_content() {
        assert!(has_active_content(b"<< /S /JavaScript /JS (app.alert(1)) >>"));
        assert!(has_active_content(b"<</OpenAction<</S/Launch/F(cmd.exe)>>>>"));
        assert!(!has_active_content(b"<< /Type /Page /JSONData 1 >>"));
    }

    #[test]
    fn do_names() {
        let names = drawn_names(b"q /Fm0 Do Q\n/Im1\nDo /X Doc BT (Do) Tj ET /Last Do");
        let mut v: Vec<&[u8]> = names.into_iter().collect();
        v.sort();
        assert_eq!(v, vec![&b"Fm0"[..], b"Im1", b"Last"]);
    }

    #[test]
    fn graph() {
        let id = |n| (n, 0u16);
        let mut edges = HashMap::new();
        edges.insert(id(1), vec![id(2)]);
        edges.insert(id(2), vec![id(3)]);
        assert!(check_graph(&edges).is_ok());
        edges.insert(id(3), vec![id(1)]);
        assert!(check_graph(&edges).is_err());
        let chain: HashMap<_, _> = (0..100).map(|i| (id(i), vec![id(i + 1)])).collect();
        assert!(check_graph(&chain).is_err());
    }

    #[test]
    fn hyphens() {
        let mut p = String::new();
        join_line(&mut p, "a self-");
        join_line(&mut p, "contained box, well-");
        join_line(&mut p, "Known text");
        assert_eq!(p, "a self-contained box, well- Known text");
    }
}

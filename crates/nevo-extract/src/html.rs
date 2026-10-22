//! A small, forgiving HTML and XHTML reader: enough to pull text, headings and the title out of
//! web pages and EPUB chapters. It never builds a tree, so memory stays proportional to the
//! text, and broken markup only costs some text, never a panic.

use std::borrow::Cow;

use crate::text::{squash_spaces, DocBuilder};

/// One piece of markup.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token<'a> {
    Text(&'a str),
    /// A start tag: lowercased name, raw attributes, and whether it ends in `/>`.
    Start(String, Vec<(String, String)>, bool),
    End(String),
    /// Text of `<script>`, `<style>`, `<title>` and the like, up to their end tag.
    Raw(&'a str),
}

/// Elements whose content is not markup.
const RAW_TEXT: &[&str] = &["script", "style", "title", "textarea", "xmp", "noembed", "noframes"];

pub(crate) struct Tokenizer<'a> {
    s: &'a str,
    pos: usize,
    raw_end: Option<String>,
}

impl<'a> Tokenizer<'a> {
    pub(crate) fn new(s: &'a str) -> Tokenizer<'a> {
        Tokenizer {
            s,
            pos: 0,
            raw_end: None,
        }
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let s = self.s;
        loop {
            if self.pos >= s.len() {
                return None;
            }
            let rest = &s[self.pos..];
            if let Some(name) = self.raw_end.take() {
                let end = find_ci(rest, &format!("</{name}")).unwrap_or(rest.len());
                self.pos += end;
                return Some(Token::Raw(&rest[..end]));
            }
            if !rest.starts_with('<') {
                let end = rest.find('<').unwrap_or(rest.len());
                self.pos += end;
                return Some(Token::Text(&rest[..end]));
            }
            if let Some(comment) = rest.strip_prefix("<!--") {
                self.pos += comment.find("-->").map_or(rest.len(), |i| i + 7);
                continue;
            }
            if let Some(cdata) = rest.strip_prefix("<![CDATA[") {
                let end = cdata.find("]]>").unwrap_or(cdata.len());
                self.pos += 9 + end + if end < cdata.len() { 3 } else { 0 };
                return Some(Token::Raw(&cdata[..end]));
            }
            if rest.starts_with("<!") || rest.starts_with("<?") {
                self.pos += rest.find('>').map_or(rest.len(), |i| i + 1);
                continue;
            }
            if let Some(after) = rest.strip_prefix("</") {
                let name_len = after.find(|c: char| !is_name_char(c)).unwrap_or(after.len());
                if name_len == 0 {
                    // `</ ` or `</>`: not a tag, drop the bracket.
                    self.pos += 2;
                    continue;
                }
                let name = after[..name_len].to_ascii_lowercase();
                self.pos += 2 + after.find('>').map_or(after.len(), |i| i + 1);
                return Some(Token::End(name));
            }
            let after = &rest[1..];
            if !after.starts_with(|c: char| c.is_ascii_alphabetic()) {
                // A lone `<` in text.
                self.pos += 1;
                return Some(Token::Text("<"));
            }
            let name_len = after.find(|c: char| !is_name_char(c)).unwrap_or(after.len());
            let name = after[..name_len].to_ascii_lowercase();
            let (attrs, self_closing, used) = attributes(&after[name_len..]);
            self.pos += 1 + name_len + used;
            if !self_closing && RAW_TEXT.contains(&local_name(&name)) {
                self.raw_end = Some(name.clone());
            }
            return Some(Token::Start(name, attrs, self_closing));
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.')
}

/// `epub:switch` → `switch`, `xhtml:p` → `p`.
pub(crate) fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Parses attributes up to the closing `>`. Returns them, whether the tag ended in `/>`, and
/// the bytes used. Quoted values may contain `>`.
fn attributes(s: &str) -> (Vec<(String, String)>, bool, usize) {
    let b = s.as_bytes();
    let mut attrs = Vec::new();
    let mut i = 0;
    let mut self_closing = false;
    while i < b.len() {
        match b[i] {
            b'>' => return (attrs, self_closing, i + 1),
            b'/' => {
                self_closing = true;
                i += 1;
            }
            c if c.is_ascii_whitespace() => i += 1,
            _ => {
                self_closing = false;
                let start = i;
                while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/') {
                    i += 1;
                }
                let name = s[start..i].to_ascii_lowercase();
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                let mut value = String::new();
                if i < b.len() && b[i] == b'=' {
                    i += 1;
                    while i < b.len() && b[i].is_ascii_whitespace() {
                        i += 1;
                    }
                    if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                        let q = b[i];
                        let end = b[i + 1..].iter().position(|&c| c == q).map_or(b.len(), |p| i + 1 + p);
                        value = decode_entities(&s[i + 1..end]).into_owned();
                        i = (end + 1).min(b.len());
                    } else {
                        let start = i;
                        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'>' {
                            i += 1;
                        }
                        value = decode_entities(&s[start..i]).into_owned();
                    }
                }
                attrs.push((name, value));
            }
        }
    }
    (attrs, self_closing, b.len())
}

pub(crate) fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(n, _)| local_name(n) == name)
        .map(|(_, v)| v.as_str())
}

/// Entity names for U+00A0 to U+00FF, in order.
const LATIN1_ENTITIES: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf", "laquo", "not", "shy",
    "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para", "middot", "cedil", "sup1", "ordm",
    "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave", "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig",
    "Ccedil", "Egrave", "Eacute", "Ecirc", "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve",
    "Oacute", "Ocirc", "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN",
    "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave", "eacute", "ecirc",
    "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde", "ograve", "oacute", "ocirc", "otilde", "ouml",
    "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml", "yacute", "thorn", "yuml",
];

/// Other common entities. Rare ones stay as written, which is still readable.
const OTHER_ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("ndash", '–'),
    ("mdash", '—'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("sbquo", '‚'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("bdquo", '„'),
    ("hellip", '…'),
    ("bull", '•'),
    ("euro", '€'),
    ("trade", '™'),
    ("dagger", '†'),
    ("Dagger", '‡'),
    ("permil", '‰'),
    ("lsaquo", '‹'),
    ("rsaquo", '›'),
    ("OElig", 'Œ'),
    ("oelig", 'œ'),
    ("Scaron", 'Š'),
    ("scaron", 'š'),
    ("Yuml", 'Ÿ'),
    ("fnof", 'ƒ'),
    ("circ", 'ˆ'),
    ("tilde", '˜'),
    ("ensp", '\u{2002}'),
    ("emsp", '\u{2003}'),
    ("thinsp", '\u{2009}'),
    ("zwnj", '\u{200c}'),
    ("zwj", '\u{200d}'),
    ("minus", '−'),
    ("larr", '←'),
    ("rarr", '→'),
    ("uarr", '↑'),
    ("darr", '↓'),
    ("harr", '↔'),
    ("le", '≤'),
    ("ge", '≥'),
    ("ne", '≠'),
    ("asymp", '≈'),
    ("infin", '∞'),
    ("prime", '′'),
    ("Prime", '″'),
    ("alpha", 'α'),
    ("beta", 'β'),
    ("gamma", 'γ'),
    ("delta", 'δ'),
    ("mu", 'μ'),
    ("pi", 'π'),
    ("sigma", 'σ'),
    ("omega", 'ω'),
    ("Delta", 'Δ'),
    ("Omega", 'Ω'),
];

fn entity(name: &str) -> Option<char> {
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        // NUL, surrogates and out-of-range numbers become the replacement character.
        return Some(char::from_u32(code).filter(|&c| c != '\0').unwrap_or('\u{fffd}'));
    }
    if let Some(i) = LATIN1_ENTITIES.iter().position(|&e| e == name) {
        return char::from_u32(0xa0 + i as u32);
    }
    OTHER_ENTITIES.iter().find(|(e, _)| *e == name).map(|&(_, c)| c)
}

/// Replaces `&name;`, `&#123;` and `&#x7b;`. Unknown or unterminated entities stay as written.
pub(crate) fn decode_entities(s: &str) -> Cow<'_, str> {
    if !s.contains('&') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '#'))
            .map(|e| e + 1);
        match end {
            Some(e) if e <= 33 && rest[e..].starts_with(';') => match entity(&rest[1..e]) {
                Some(c) => {
                    out.push(c);
                    rest = &rest[e + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            },
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Elements that start a new paragraph.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "header",
    "hgroup",
    "hr",
    "html",
    "legend",
    "main",
    "menu",
    "ol",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "tbody",
    "tfoot",
    "thead",
    "ul",
];

/// Elements whose whole content is not reading text.
const SKIPPED: &[&str] = &[
    "head", "script", "style", "noscript", "template", "svg", "math", "nav", "iframe", "object", "select", "button",
];

/// Reads an HTML or XHTML document into `doc`. Returns the `<title>`, if any.
pub(crate) fn to_blocks(html: &str, doc: &mut DocBuilder) -> Option<String> {
    let mut title = None;
    let mut para = Para::default();
    let mut heading: Option<(u8, String)> = None;
    // Depth inside skipped elements, and the name that opened the outermost one.
    let mut skip: Vec<String> = Vec::new();
    let mut in_title = false;
    let mut pre = 0usize;

    for token in Tokenizer::new(html) {
        if doc.is_full() {
            break;
        }
        match token {
            Token::Start(name, attrs, self_closing) => {
                let name = local_name(&name).to_string();
                if name == "title" && !self_closing {
                    in_title = true;
                    continue;
                }
                let hidden = attr(&attrs, "hidden").is_some() || attr(&attrs, "aria-hidden") == Some("true");
                if !skip.is_empty() || hidden || SKIPPED.contains(&name.as_str()) {
                    if !self_closing && !is_void(&name) {
                        skip.push(name);
                    }
                    continue;
                }
                if let Some(level) = heading_level(&name) {
                    para.flush(doc);
                    heading = Some((level, String::new()));
                } else if BLOCKS.contains(&name.as_str()) {
                    para.flush(doc);
                    if name == "pre" {
                        pre += 1;
                    }
                } else {
                    match name.as_str() {
                        "br" => para.line_break(),
                        "li" | "tr" => {
                            para.line_break();
                            if name == "li" {
                                para.push_str("- ");
                            }
                        }
                        "td" | "th" => para.cell(),
                        _ => {}
                    }
                }
            }
            Token::End(name) => {
                let name = local_name(&name).to_string();
                if !skip.is_empty() {
                    // Close the innermost matching element; tolerate missing end tags.
                    if let Some(i) = skip.iter().rposition(|n| *n == name) {
                        skip.truncate(i);
                    }
                    continue;
                }
                if let Some(level) = heading_level(&name) {
                    if let Some((l, text)) = heading.take() {
                        doc.heading(l.min(level), &text);
                    }
                } else if BLOCKS.contains(&name.as_str()) {
                    para.flush(doc);
                    if name == "pre" {
                        pre = pre.saturating_sub(1);
                    }
                }
            }
            Token::Text(t) => {
                if !skip.is_empty() {
                    continue;
                }
                let t = decode_entities(t);
                match heading.as_mut() {
                    Some((_, h)) => h.push_str(&t),
                    None if pre > 0 => para.push_pre(&t),
                    None => para.push_text(&t),
                }
            }
            Token::Raw(t) => {
                if in_title {
                    in_title = false;
                    if title.is_none() {
                        title = Some(squash_spaces(&decode_entities(t)));
                    }
                } else if skip.is_empty() {
                    // Script-like elements are always skipped, so this is CDATA (ordinary text
                    // in XHTML) or the content of a `<textarea>`.
                    para.push_text(t);
                }
            }
        }
    }
    if let Some((l, text)) = heading {
        doc.heading(l, &text);
    }
    para.flush(doc);
    title.filter(|t| !t.is_empty())
}

fn heading_level(name: &str) -> Option<u8> {
    match name {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

/// Elements that never have content, so they never close a skip.
fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

/// The paragraph being built, with HTML whitespace rules: runs of spaces collapse to one.
#[derive(Default)]
struct Para {
    s: String,
    space: bool,
}

impl Para {
    fn push_text(&mut self, t: &str) {
        for c in t.chars() {
            if c.is_whitespace() && c != '\u{a0}' {
                self.space = true;
            } else {
                if self.space && !self.s.is_empty() && !self.s.ends_with('\n') {
                    self.s.push(' ');
                }
                self.space = false;
                self.s.push(if c == '\u{a0}' { ' ' } else { c });
            }
        }
    }

    fn push_pre(&mut self, t: &str) {
        if self.space && !self.s.is_empty() {
            self.s.push(' ');
        }
        self.space = false;
        self.s.push_str(t);
    }

    fn push_str(&mut self, t: &str) {
        self.s.push_str(t);
        self.space = false;
    }

    fn line_break(&mut self) {
        let trimmed = self.s.trim_end_matches(' ').len();
        self.s.truncate(trimmed);
        if !self.s.is_empty() && !self.s.ends_with('\n') {
            self.s.push('\n');
        }
        self.space = false;
    }

    fn cell(&mut self) {
        if !self.s.is_empty() && !self.s.ends_with('\n') {
            self.s.push_str(" | ");
        }
        self.space = false;
    }

    fn flush(&mut self, doc: &mut DocBuilder) {
        doc.para(&self.s);
        self.s.clear();
        self.space = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(html: &str) -> (Option<String>, String) {
        let mut d = DocBuilder::new(100_000);
        let title = to_blocks(html, &mut d);
        (title, d.finish())
    }

    #[test]
    fn strips_tags() {
        let (title, t) = text(
            "<!DOCTYPE html><html><head><title>Water &amp; you</title><style>p{color:red}</style>\
             <script>if (a<b) document.write('<p>no</p>')</script></head><body>\
             <nav><a href=/>Home</a></nav><h1>Boiling <em>water</em></h1>\
             <p>Bring&nbsp;water to a <b>rolling</b>\n   boil.<br>Wait one&nbsp;minute.</p>\
             <!-- comment <p>hidden</p> --><ul><li>Pot<li>Fire</ul>\
             <table><tr><th>Height<th>Time<tr><td>0 m<td>1 min</table>\
             <p hidden>secret</p><p>caf&eacute; &#233; &#xE9; &bogus; AT&T</p></body></html>",
        );
        assert_eq!(title.as_deref(), Some("Water & you"));
        assert_eq!(
            t,
            "# Boiling water\n\nBring water to a rolling boil.\nWait one minute.\n\n- Pot\n- Fire\n\n\
             Height | Time\n0 m | 1 min\n\ncafé é é &bogus; AT&T"
        );
    }

    #[test]
    fn broken_markup_does_not_panic() {
        for s in [
            "<",
            "<a",
            "<a href=\"",
            "</",
            "<!--",
            "<![CDATA[",
            "<p>x</q></p",
            "&#xFFFFFFFF;",
            "&#0;",
            "<é>",
            "<script>",
        ] {
            let _ = text(s);
        }
        assert_eq!(text("a < b and c > d").1, "a < b and c > d");
        assert_eq!(text("<p>&#xD800;</p>").1, "\u{fffd}");
    }

    #[test]
    fn xhtml() {
        let (_, t) = text(
            "<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><body>\
             <h2 class=\"c\">One</h2><p>A<br/>B</p><p><![CDATA[x < y]]></p><div/><p>C</p></body></html>",
        );
        assert_eq!(t, "## One\n\nA\nB\n\nx < y\n\nC");
    }
}

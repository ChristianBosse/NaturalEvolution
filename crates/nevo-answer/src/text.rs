//! Small text tools shared by the parser and the checks.
//!
//! Everything works on characters, never on byte offsets picked by hand, so no input can make a
//! slice land inside a character.

/// Lowercases, drops accents and markdown emphasis, and turns typographic apostrophes into `'`.
///
/// Accents go because models and sources disagree on them ("cuillère", "cuillere"), and English,
/// French and Spanish words stay distinct without them.
pub(crate) fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '*' | '_' | '`' => {}
            '\u{2018}' | '\u{2019}' | '\u{02BC}' => out.push('\''),
            '\u{0153}' | '\u{0152}' => out.push_str("oe"),
            _ => {
                for lower in c.to_lowercase() {
                    out.push(strip_accent(lower));
                }
            }
        }
    }
    out
}

fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        _ => c,
    }
}

/// French elided words and what they stand for ("n'utilisez" is "ne utilisez").
const ELISIONS: &[(&str, &str)] = &[
    ("n", "ne"),
    ("l", "le"),
    ("d", "de"),
    ("j", "je"),
    ("s", "se"),
    ("c", "ce"),
    ("m", "me"),
    ("t", "te"),
    ("qu", "que"),
    ("jusqu", "jusque"),
    ("lorsqu", "lorsque"),
];

/// A comma in the output of [`words`]. Commas separate list items, which the negation check needs.
pub(crate) const COMMA: &str = ",";

/// Splits folded text into words and commas.
///
/// "don't" gives "do", "not" so negations look the same however they were typed.
pub(crate) fn words(folded: &str) -> Vec<String> {
    let chars: Vec<char> = folded.chars().collect();
    let mut out = Vec::new();
    let mut word = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_alphanumeric() {
            word.push(c);
        } else if c == '\'' {
            let next = chars.get(i + 1).copied();
            let after = chars.get(i + 2).copied();
            if word.ends_with('n') && next == Some('t') && !after.is_some_and(char::is_alphanumeric) {
                word.pop();
                push_word(&mut out, &mut word);
                out.push("not".to_string());
                i += 1;
            } else if let Some((_, full)) = ELISIONS.iter().find(|(short, _)| *short == word) {
                word.clear();
                out.push(full.to_string());
            } else {
                push_word(&mut out, &mut word);
            }
        } else {
            push_word(&mut out, &mut word);
            if c == ',' {
                out.push(COMMA.to_string());
            }
        }
        i += 1;
    }
    push_word(&mut out, &mut word);
    out
}

fn push_word(out: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        out.push(std::mem::take(word));
    }
}

/// A rough English stem, so "cutting", "cuts" and "cut" match. Non-ASCII words are kept as they are.
pub(crate) fn stem(word: &str) -> String {
    if word.len() <= 3 || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return word.to_string();
    }
    // Only "-ing" and "-ed" double a final consonant ("cutting"), so only they undo it.
    let mut verb_ending = false;
    let mut s = if let Some(base) = word.strip_suffix("ies").or_else(|| word.strip_suffix("ied")) {
        format!("{base}y")
    } else if word.len() > 5 && word.ends_with("ing") {
        verb_ending = true;
        word[..word.len() - 3].to_string()
    } else if word.len() > 4 && word.ends_with("ed") {
        verb_ending = true;
        word[..word.len() - 2].to_string()
    } else if ["ches", "shes", "sses", "xes", "zes"]
        .iter()
        .any(|end| word.ends_with(end))
    {
        word[..word.len() - 2].to_string()
    } else if word.ends_with('s') && !["ss", "us", "is"].iter().any(|end| word.ends_with(end)) {
        word[..word.len() - 1].to_string()
    } else {
        word.to_string()
    };
    let b = s.as_bytes();
    if verb_ending && b.len() >= 3 && b[b.len() - 1] == b[b.len() - 2] && !b"aeiouls".contains(&b[b.len() - 1]) {
        s.pop();
    }
    if s.len() > 3 && s.ends_with('e') {
        s.pop();
    }
    if s.len() > 3 && s.ends_with('y') {
        s.pop();
        s.push('i');
    }
    s
}

/// Splits text into sentence-like pieces: at line breaks, `;`, `:`, and at `.`, `!` or `?` when
/// they end a sentence (so "0.5" stays whole).
pub(crate) fn sentences(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        let next = iter.peek().map(|&(_, n)| n);
        let ends = match c {
            '\n' | ';' | ':' => true,
            '.' | '!' | '?' => next.is_none_or(|n| n.is_whitespace() || n == '[' || n == '*' || n == ')'),
            _ => false,
        };
        if ends {
            push_piece(&mut out, &text[start..i]);
            start = i + c.len_utf8();
        }
    }
    push_piece(&mut out, &text[start..]);
    out
}

fn push_piece<'a>(out: &mut Vec<&'a str>, piece: &'a str) {
    let piece = piece.trim();
    if piece.chars().any(char::is_alphanumeric) {
        out.push(piece);
    }
}

/// Finds citations like `[S1]` or `[S1, S2]`. Returns each one's byte range and the IDs inside.
pub(crate) fn citations(text: &str) -> Vec<(std::ops::Range<usize>, Vec<String>)> {
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(open) = text[search..].find('[').map(|o| o + search) {
        // Only look a short way for the closing bracket, so many unclosed '[' stay linear.
        let close = text[open + 1..]
            .char_indices()
            .take(MAX_CITATION_CHARS + 1)
            .find(|&(_, c)| c == ']')
            .map(|(i, _)| open + 1 + i);
        match close.and_then(|close| Some((close, citation_ids(&text[open + 1..close])?))) {
            Some((close, ids)) => {
                out.push((open..close + 1, ids));
                search = close + 1;
            }
            None => search = open + 1,
        }
    }
    out
}

/// The longest text between brackets that can still be a citation list.
const MAX_CITATION_CHARS: usize = 60;

fn citation_ids(inner: &str) -> Option<Vec<String>> {
    let mut ids = Vec::new();
    for part in inner.split([',', ';', ' ']).filter(|p| !p.is_empty()) {
        let digits = part.strip_prefix(['S', 's'])?;
        if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let number = digits.trim_start_matches('0');
        ids.push(format!("S{}", if number.is_empty() { "0" } else { number }));
    }
    (!ids.is_empty()).then_some(ids)
}

/// The text with citations replaced by spaces.
pub(crate) fn strip_citations(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (range, _) in citations(text) {
        out.push_str(&text[last..range.start]);
        out.push(' ');
        last = range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Normalizes a source ID the way citations are normalized ("s01" and "S1" are the same).
pub(crate) fn normalize_id(id: &str) -> String {
    citation_ids(id.trim())
        .and_then(|ids| ids.into_iter().next())
        .unwrap_or_else(|| id.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_and_splits_words() {
        assert_eq!(words(&fold("Don't **cut** it")), ["do", "not", "cut", "it"]);
        assert_eq!(
            words(&fold("N’utilisez pas, jamais")),
            ["ne", "utilisez", "pas", ",", "jamais"]
        );
        assert_eq!(words(&fold("Cuillère à café")), ["cuillere", "a", "cafe"]);
        assert_eq!(words(&fold("the person's leg")), ["the", "person", "s", "leg"]);
    }

    #[test]
    fn stems() {
        for (a, b) in [
            ("cutting", "cut"),
            ("rubbing", "rub"),
            ("applying", "apply"),
            ("applied", "apply"),
            ("tasting", "taste"),
            ("removed", "remove"),
            ("tourniquets", "tourniquet"),
            ("legs", "leg"),
            ("sucking", "suck"),
        ] {
            assert_eq!(stem(a), stem(b), "{a} / {b}");
        }
        assert_ne!(stem("scented"), stem("unscented"));
    }

    #[test]
    fn splits_sentences_but_not_decimals() {
        assert_eq!(
            sentences("Mix 0.5 l. Stir!\nYou need: salt"),
            ["Mix 0.5 l", "Stir", "You need", "salt"]
        );
    }

    #[test]
    fn finds_citations() {
        let found = citations("a [S1] b [S2, s3] c [1] [Sx] [S4");
        let ids: Vec<_> = found.into_iter().map(|(_, ids)| ids).collect();
        assert_eq!(ids, [vec!["S1"], vec!["S2", "S3"]]);
        assert_eq!(strip_citations("cool it [S1]."), "cool it  .");
        assert_eq!(normalize_id("s01"), "S1");
    }
}

//! Turning text into search terms: one analyzer per language group, plus the plain
//! normalization used for exact title matches and topic filters.

use tantivy::tokenizer::{
    AsciiFoldingFilter, Language, LowerCaser, RemoveLongFilter, SimpleTokenizer, Stemmer, StopWordFilter, TextAnalyzer,
    TokenStream, TokenizerManager,
};

/// Longer tokens are dropped: they are almost always junk (URLs, base64, chemical names) and
/// would bloat the term dictionary.
const MAX_TOKEN_BYTES: usize = 40;

/// Tantivy's English list (Lucene's 33 words) plus the words people put around questions:
/// "how do I stop bleeding" should search for "stop bleeding". "who" stays out: it is also
/// the World Health Organization.
const ENGLISH_STOP_WORDS: &[&str] = &[
    // Lucene
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "if", "in", "into", "is", "it", "no", "not", "of",
    "on", "or", "such", "that", "the", "their", "then", "there", "these", "they", "this", "to", "was", "will", "with",
    // Questions
    "how", "what", "when", "where", "which", "why", "do", "does", "did", "can", "could", "should", "would", "i", "me",
    "my", "we", "our", "you", "your", "about", "have", "has", "had", "been", "were", "so", "very",
];

/// A set of languages that share one analyzer, and so one set of index fields.
pub(crate) struct Group {
    /// Used in field and analyzer names.
    pub name: &'static str,
    /// Primary language subtags that use this group. The first is the BCP 47 code; the others
    /// are ISO 639-2/3 codes, which Kiwix uses in ZIM metadata ("eng").
    pub codes: &'static [&'static str],
    /// Stemmer and stop words. `None` means words are only lowercased and folded.
    pub language: Option<Language>,
}

/// Adding a group changes the schema, so it needs a new [`crate::FORMAT_VERSION`].
pub(crate) const GROUPS: &[Group] = &[
    Group {
        name: "en",
        codes: &["en", "eng"],
        language: Some(Language::English),
    },
    Group {
        name: "fr",
        codes: &["fr", "fra", "fre"],
        language: Some(Language::French),
    },
    Group {
        name: "es",
        codes: &["es", "spa"],
        language: Some(Language::Spanish),
    },
    Group {
        name: "pt",
        codes: &["pt", "por"],
        language: Some(Language::Portuguese),
    },
    Group {
        name: "ar",
        codes: &["ar", "ara"],
        language: Some(Language::Arabic),
    },
    Group {
        name: "other",
        codes: &[],
        language: None,
    },
];

/// The group for languages without their own analyzer. Always last.
pub(crate) const OTHER: usize = GROUPS.len() - 1;

fn primary_subtag(lang: &str) -> String {
    lang.trim().split(['-', '_']).next().unwrap_or("").to_ascii_lowercase()
}

/// The analyzer group for a BCP 47 language tag.
pub(crate) fn group_of(lang: &str) -> usize {
    let primary = primary_subtag(lang);
    GROUPS
        .iter()
        .position(|g| g.codes.contains(&primary.as_str()))
        .unwrap_or(OTHER)
}

/// The language as the filter sees it: the primary subtag, with the 3-letter codes of known
/// groups mapped to their 2-letter code. "fr-CA", "fra" and "fr" all become "fr".
pub(crate) fn lang_key(lang: &str) -> String {
    let primary = primary_subtag(lang);
    match GROUPS.iter().find(|g| g.codes.contains(&primary.as_str())) {
        Some(g) => g.codes[0].to_string(),
        None => primary,
    }
}

pub(crate) fn analyzer_name(group: usize) -> String {
    format!("nevo_{}", GROUPS[group].name)
}

/// Words split on anything that isn't a letter or digit, lowercased, stop words dropped,
/// stemmed, then accents folded. Folding comes after stemming because the Snowball stemmers
/// expect accented input; folding lets "diarrhee" find "diarrhée".
fn analyzer(group: &Group) -> TextAnalyzer {
    let mut b = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(MAX_TOKEN_BYTES))
        .filter(LowerCaser)
        .dynamic();
    if let Some(lang) = group.language {
        let stop = match lang {
            Language::English => Some(StopWordFilter::remove(ENGLISH_STOP_WORDS.iter().map(|w| w.to_string()))),
            _ => StopWordFilter::new(lang),
        };
        if let Some(stop) = stop {
            b = b.filter_dynamic(stop);
        }
        b = b.filter_dynamic(Stemmer::new(lang));
    }
    b.filter_dynamic(AsciiFoldingFilter).build()
}

pub(crate) fn register(tokenizers: &TokenizerManager) {
    for (i, group) in GROUPS.iter().enumerate() {
        tokenizers.register(&analyzer_name(i), analyzer(group));
    }
}

/// Analyzers for analyzing queries and snippets outside the index, built on first use (a
/// Tantivy analyzer needs `&mut` to run, so they can't be shared between threads).
pub(crate) struct Analyzers(Vec<Option<TextAnalyzer>>);

impl Analyzers {
    pub fn new() -> Analyzers {
        Analyzers(GROUPS.iter().map(|_| None).collect())
    }

    fn get(&mut self, group: usize) -> &mut TextAnalyzer {
        self.0[group].get_or_insert_with(|| analyzer(&GROUPS[group]))
    }

    /// Terms of `text` in `group`, with their byte ranges in `text`.
    pub fn spans(&mut self, group: usize, text: &str) -> Vec<(usize, usize, String)> {
        let mut out = Vec::new();
        let mut stream = self.get(group).token_stream(text);
        while stream.advance() {
            let t = stream.token();
            out.push((t.offset_from, t.offset_to, t.text.clone()));
        }
        out
    }

    /// Every term `text` produces in `group`.
    pub fn terms(&mut self, group: usize, text: &str) -> Vec<String> {
        self.spans(group, text).into_iter().map(|(_, _, t)| t).collect()
    }

    /// The term a single word produces in `group`, or `None` for a stop word.
    pub fn term(&mut self, group: usize, word: &str) -> Option<String> {
        self.terms(group, word).into_iter().next()
    }
}

/// Plain words: lowercased, nothing removed or stemmed.
pub(crate) fn words(text: &str) -> Vec<String> {
    let mut analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(RemoveLongFilter::limit(MAX_TOKEN_BYTES))
        .filter(LowerCaser)
        .build();
    let mut out = Vec::new();
    let mut stream = analyzer.token_stream(text);
    while stream.advance() {
        out.push(stream.token().text.clone());
    }
    out
}

/// The key for exact title matches: words lowercased, accents folded, joined by single
/// spaces. "Heart_attack", "heart attack?" and "HEART  ATTACK" all give "heart attack".
pub(crate) fn exact_key(text: &str) -> String {
    let mut analyzer = TextAnalyzer::builder(SimpleTokenizer::default())
        .filter(LowerCaser)
        .filter(AsciiFoldingFilter)
        .build();
    let mut out = String::new();
    let mut stream = analyzer.token_stream(text);
    while stream.advance() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&stream.token().text);
    }
    out
}

/// A topic path cleaned up for filtering: lowercased, without empty parts or outer slashes.
/// `None` if nothing is left.
pub(crate) fn topic_path(topic: &str) -> Option<String> {
    let parts: Vec<String> = topic
        .split('/')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The filter terms for a topic: the topic and each of its parents, so that filtering on
/// "medical" finds "medical/first-aid". Whole path segments only: "medical-x" is not under
/// "medical".
pub(crate) fn topic_terms(topic: &str) -> Vec<String> {
    let Some(path) = topic_path(topic) else {
        return Vec::new();
    };
    let mut out: Vec<String> = path.match_indices('/').map(|(i, _)| path[..i].to_string()).collect();
    out.push(path);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags() {
        assert_eq!(group_of("en"), 0);
        assert_eq!(group_of("EN-gb"), 0);
        assert_eq!(group_of("fra"), 1);
        assert_eq!(group_of("es_MX"), 2);
        assert_eq!(group_of("sw"), OTHER);
        assert_eq!(group_of(""), OTHER);
        assert_eq!(lang_key("fr-CA"), "fr");
        assert_eq!(lang_key("eng"), "en");
        assert_eq!(lang_key("SW"), "sw");
    }

    #[test]
    fn analyzers_stem_and_drop_stop_words() {
        let mut a = Analyzers::new();
        assert_eq!(a.terms(0, "How to stop the bleeding"), ["stop", "bleed"]);
        assert_eq!(a.terms(1, "Les morsures de serpents"), a.terms(1, "morsure serpent"));
        assert_eq!(a.terms(1, "Diarrhées"), a.terms(1, "diarrhée"));
        // Stemming comes before folding, so a missing accent can change the stem. The typo
        // fallback usually catches it.
        assert_ne!(a.term(1, "diarrhée"), a.term(1, "diarrhee"));
        assert_eq!(a.terms(2, "hemorragias"), a.terms(2, "hemorragia"));
        assert_eq!(a.terms(OTHER, "Kuhara kwa Watoto"), ["kuhara", "kwa", "watoto"]);
    }

    #[test]
    fn exact_keys() {
        assert_eq!(exact_key("Heart_attack"), "heart attack");
        assert_eq!(exact_key("  HEART   attack? "), "heart attack");
        assert_eq!(exact_key("Diarrhée"), "diarrhee");
        assert_eq!(exact_key("?!"), "");
    }

    #[test]
    fn topics() {
        assert_eq!(topic_terms("medical/first-aid"), ["medical", "medical/first-aid"]);
        assert_eq!(topic_terms("/Medical//first-aid/"), ["medical", "medical/first-aid"]);
        assert_eq!(topic_terms("medical-x"), ["medical-x"]);
        assert!(topic_terms("//").is_empty());
    }
}

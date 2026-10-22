//! The negation check: a "do not" in a source must not get lost in the answer.
//!
//! Small models drop negations ("Do not … try to suck out the venom" becomes "Try to suck out the
//! venom"). This finds every forbidden action in the cited sources, then every answer clause that
//! mentions all the words of one of those actions. If such a clause has no negation or warning word
//! of its own, the answer states the forbidden action as advice.
//!
//! Matching is by word stems, so it only works when the answer is in the source's language. A
//! translated answer is not matched at all, which can miss a lost negation but never blocks a good
//! answer.

use crate::text::{fold, sentences, stem, strip_citations, words, COMMA};
use crate::Source;

/// A forbidden action found in a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prohibition {
    pub source: String,
    /// The action's words as written in the source, for messages.
    pub phrase: String,
    stems: Vec<String>,
    /// A lone verb whose object was a pronoun ("do not remove it"). Without its object it only
    /// counts when the answer clause starts with it, as an instruction does ("Remove the …").
    lone_verb: bool,
}

/// Object pronouns that mark the word before them as a verb.
const PRONOUNS: &[&str] = &["it", "them"];

/// Words that start a forbidden action in a source.
#[rustfmt::skip]
const CUES: &[&str] = &[
    "not", "no", "never", "avoid", "avoiding", "without", "cannot", "ne", "jamais", "sans", "nunca", "jamas", "sin",
    "evite", "evita", "evitar", "evitez", "eviter",
];

/// Extra words that mark an answer clause as negative or as a warning ("can be fatal", "can cause
/// infection"). A clause that names a forbidden action together with its danger keeps the warning.
#[rustfmt::skip]
const ANSWER_CUES: &[&str] = &[
    "pas", "nor", "ni", "refrain", "dangerous", "danger", "unsafe", "harmful", "harm", "worse", "worsen", "worsens",
    "fatal", "deadly", "death", "die", "dies", "infection", "infections", "damage", "injury", "injuries", "risk",
    "risks", "poison", "poisonous", "toxic",
    "peligroso", "peligrosa", "peligrosos", "riesgo", "dangereux", "dangereuse", "risque", "nocif", "nocivo",
];

/// Two-word cues: "instead of X", "rather than X".
const ANSWER_CUE_PAIRS: &[(&str, &str)] = &[
    ("instead", "of"),
    ("rather", "than"),
    ("en", "lugar"),
    ("en", "vez"),
    ("au", "lieu"),
];

/// Words that split an answer sentence into clauses with their own polarity.
const CONTRASTS: &[&str] = &["but", "however", "pero", "mais", "cependant"];

/// Words that separate list items after a cue: "do not cut, suck or apply ice".
const ITEM_SEPARATORS: &[&str] = &["or", "nor", "ni", "o", "u", "ou"];

/// Words that open a condition. A negation inside a condition ("if it does not smell") is not a
/// prohibition.
const CONDITIONS: &[&str] = &["if", "unless", "when", "whenever", "si", "cuando", "lorsque", "quand"];

/// Words that carry no action of their own. Generic verbs ("use", "apply", "try") are here too, so
/// "do not use ice" forbids "ice", which also catches "apply ice".
#[rustfmt::skip]
const STOPWORDS: &[&str] = &[
    // English
    "a", "an", "the", "to", "of", "in", "on", "at", "for", "with", "by", "from", "into", "onto", "over", "under",
    "up", "out", "off", "and", "but", "then", "than", "that", "this", "these", "those", "it", "its", "they", "them",
    "their", "there", "is", "are", "was", "were", "be", "been", "being", "do", "does", "did", "have", "has", "had",
    "can", "could", "should", "would", "will", "shall", "may", "might", "must", "try", "attempt", "use", "using",
    "put", "putting", "apply", "applying", "make", "get", "you", "your", "he", "she", "his", "her", "him", "we",
    "our", "us", "i", "me", "my", "person", "people", "someone", "anyone", "any", "some", "all", "each", "very",
    "more", "most", "less", "much", "many", "as", "so", "such", "just", "only", "also", "even", "about", "too",
    "again", "first", "after", "before", "while", "until", "what", "which", "who", "how", "why", "once",
    // French
    "le", "la", "les", "un", "une", "des", "de", "du", "au", "aux", "et", "en", "dans", "sur", "sous", "par",
    "pour", "avec", "ce", "cet", "cette", "ces", "il", "elle", "ils", "elles", "on", "vous", "nous", "je", "se", "y",
    "est", "sont", "pas", "plus", "utiliser", "utilisez", "mettre", "mettez", "appliquer", "appliquez", "essayer",
    "essayez", "faire", "faites", "que", "qui",
    // Spanish
    "el", "los", "las", "unos", "unas", "del", "al", "con", "por", "para", "sobre", "bajo", "su", "sus", "lo",
    "es", "son", "ser", "estar", "usar", "use", "usa", "aplicar", "aplique", "aplica", "poner", "ponga",
    "intentar", "intente", "haga",
];

/// The forbidden actions in a source.
pub(crate) fn prohibitions(source: &Source) -> Vec<Prohibition> {
    let mut out = Vec::new();
    for sentence in sentences(&source.text) {
        let words = words(&fold(&strip_citations(sentence)));
        let in_condition = condition_mask(&words);
        let mut active = false;
        let mut item: Vec<&str> = Vec::new();
        for (i, word) in words.iter().enumerate() {
            let w = word.as_str();
            if in_condition[i] {
                close_item(&mut out, source, &mut item);
                active = false;
            } else if is_source_cue(&words, i) {
                close_item(&mut out, source, &mut item);
                active = true;
            } else if active && (w == COMMA || ITEM_SEPARATORS.contains(&w)) {
                close_item(&mut out, source, &mut item);
            } else if active {
                item.push(w);
            }
        }
        close_item(&mut out, source, &mut item);
    }
    out
}

fn is_source_cue(words: &[String], i: usize) -> bool {
    let w = words[i].as_str();
    // "pas" only starts a prohibition on its own; after "ne" it just closes "ne … pas".
    CUES.contains(&w) || (w == "pas" && !words[i.saturating_sub(4)..i].iter().any(|p| p == "ne"))
}

fn close_item(out: &mut Vec<Prohibition>, source: &Source, item: &mut Vec<&str>) {
    let content: Vec<&str> = item
        .iter()
        .copied()
        .filter(|w| !STOPWORDS.contains(w) && !CUES.contains(w) && !is_number(w))
        .collect();
    if !content.is_empty() {
        let lone_verb = content.len() == 1
            && item
                .iter()
                .position(|w| *w == content[0])
                .and_then(|i| item.get(i + 1))
                .is_some_and(|next| PRONOUNS.contains(next));
        let prohibition = Prohibition {
            source: source.id.clone(),
            phrase: item.join(" "),
            stems: content.iter().map(|w| stem(w)).collect(),
            lone_verb,
        };
        if !out.contains(&prohibition) {
            out.push(prohibition);
        }
    }
    item.clear();
}

/// Marks the words inside a condition. A sentence that starts with one ("If it does not, repeat")
/// has it up to the first comma; a condition later in the sentence runs to its end.
fn condition_mask(words: &[String]) -> Vec<bool> {
    let mut mask = vec![false; words.len()];
    let mut i = 0;
    while i < words.len() {
        if CONDITIONS.contains(&words[i].as_str()) {
            let end = if i == 0 {
                words.iter().position(|w| w == COMMA).unwrap_or(words.len())
            } else {
                words.len()
            };
            for flag in &mut mask[i..end] {
                *flag = true;
            }
            i = end.max(i + 1);
        } else {
            i += 1;
        }
    }
    mask
}

/// An answer sentence that states a forbidden action without negating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Violation {
    pub prohibition: Prohibition,
    pub sentence: String,
}

/// Every place where the answer states one of the prohibitions as advice.
pub(crate) fn violations(prohibitions: &[Prohibition], answer: &str) -> Vec<Violation> {
    let mut out = Vec::new();
    for sentence in negation_free_lines(answer).flat_map(sentences) {
        let words = words(&fold(&strip_citations(sentence)));
        for clause in clauses(&words) {
            if has_answer_cue(clause) {
                continue;
            }
            let stems: Vec<String> = clause.iter().map(|w| stem(w)).collect();
            let lead = clause
                .iter()
                .find(|w| w.as_str() != COMMA && !STOPWORDS.contains(&w.as_str()) && !is_number(w))
                .map(|w| stem(w));
            for p in prohibitions {
                let matched = if p.lone_verb {
                    lead.as_ref() == p.stems.first()
                } else {
                    p.stems.iter().all(|s| stems.contains(s))
                };
                if matched {
                    let found = Violation {
                        prohibition: p.clone(),
                        sentence: sentence.to_string(),
                    };
                    if !out.contains(&found) {
                        out.push(found);
                    }
                }
            }
        }
    }
    out
}

/// The answer's lines, minus the items of a negative list ("Do not:" followed by bullets), whose
/// negation is in the heading.
fn negation_free_lines(answer: &str) -> impl Iterator<Item = &str> {
    let mut under_negative_heading = false;
    answer.lines().filter(move |line| {
        let bare = line.trim_matches(|c: char| c.is_whitespace() || matches!(c, '*' | '_' | '#'));
        let is_item = bare.starts_with(['-', '*', '•']) || bare.starts_with(|c: char| c.is_ascii_digit());
        if under_negative_heading && is_item {
            return false;
        }
        let heading = words(&fold(bare));
        under_negative_heading = bare.ends_with(':') && heading.len() <= 6 && has_answer_cue(&heading);
        true
    })
}

fn is_number(word: &str) -> bool {
    word.bytes().all(|b| b.is_ascii_digit())
}

fn clauses(words: &[String]) -> impl Iterator<Item = &[String]> {
    words
        .split(|w| CONTRASTS.contains(&w.as_str()))
        .filter(|c| !c.is_empty())
}

fn has_answer_cue(clause: &[String]) -> bool {
    clause
        .iter()
        .any(|w| CUES.contains(&w.as_str()) || ANSWER_CUES.contains(&w.as_str()))
        || clause
            .windows(2)
            .any(|pair| ANSWER_CUE_PAIRS.iter().any(|(a, b)| pair[0] == *a && pair[1] == *b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Trust;

    fn source(text: &str) -> Source {
        Source::new("S1", Trust::Reference, "t", text)
    }

    fn phrases(text: &str) -> Vec<String> {
        prohibitions(&source(text)).into_iter().map(|p| p.phrase).collect()
    }

    #[test]
    fn splits_a_negated_list() {
        assert_eq!(
            phrases("Do not cut the wound, try to suck out the venom, apply ice or use a tourniquet. Keep calm."),
            [
                "cut the wound",
                "try to suck out the venom",
                "apply ice",
                "use a tourniquet"
            ]
        );
        assert_eq!(
            phrases("Do not rub the arms and legs, and do not put the person in a hot bath."),
            ["rub the arms and legs", "put the person in a hot bath"]
        );
    }

    #[test]
    fn ignores_negations_inside_conditions() {
        assert!(phrases("If it does not, repeat the dose and wait another 15 minutes.").is_empty());
        assert!(phrases("Get help if the child cannot drink, or shows no improvement.").is_empty());
        assert_eq!(phrases("Discard any jar without tasting it."), ["tasting it"]);
    }

    #[test]
    fn reads_french_and_spanish() {
        assert_eq!(phrases("N'utilisez pas de glace."), ["utilisez pas de glace"]);
        assert_eq!(phrases("Nunca beba agua de mar."), ["beba agua de mar"]);
    }

    #[test]
    fn flags_a_lost_negation() {
        let p = prohibitions(&source(
            "Do not cut the wound, try to suck out the venom, apply ice or use a tourniquet.",
        ));
        let bad = |answer: &str| !violations(&p, answer).is_empty();
        assert!(bad("⚠️ Do not cut the wound [S1]. Try to suck out the venom [S1]."));
        assert!(bad("4. Apply **ice** (if available) to reduce swelling [S1]."));
        assert!(bad("You need: A tourniquet (if instructed)."));
        assert!(bad("Do not cut the wound, but apply ice."));
        assert!(!bad("Do not cut the wound or try to suck out the venom [S1]."));
        assert!(!bad("No, you should not cut the wound or suck out the venom."));
        assert!(!bad("Sucking out the venom can worsen the injury."));
        assert!(!bad("Keep the person still [S1]."));
        assert!(!bad("**Do not:**\n- Apply ice.\n- Use a tourniquet.\n\nKeep calm."));
        assert!(bad("**Do not:**\n- Cut the wound.\n\nApply ice."));
    }

    #[test]
    fn a_lone_verb_must_lead_the_clause() {
        let p = prohibitions(&source("Do not remove it; leave that to medical staff."));
        let bad = |answer: &str| !violations(&p, answer).is_empty();
        assert!(bad("- Remove the tourniquet unless told to."));
        assert!(bad("3. Then remove the tourniquet."));
        assert!(!bad("Leave the tourniquet in place for medical staff to remove."));
    }
}

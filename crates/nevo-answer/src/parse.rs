//! Reading the parts of a model answer that the prompt asks for.

use crate::text::{citations, fold, strip_citations, words};

/// The parts of a model answer, as far as they can be found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    /// Cited source IDs ("S1"), in order of first use, without repeats.
    pub citations: Vec<String>,
    /// Whether and where the answer says "I don't know. The library doesn't cover this."
    pub refusal: Option<Refusal>,
    /// Warning lines (with ⚠️ or starting with "Warning"), without the marker.
    pub warnings: Vec<String>,
    /// Numbered steps ("1. …", "2) …", "[3] …"), without the number.
    pub steps: Vec<String>,
    /// The "You need:" list, if there is one.
    pub you_need: Option<String>,
}

/// Where the refusal sentence sits in the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The answer is the refusal and nothing else.
    Only,
    /// The answer opens with the refusal and then answers anyway. The two contradict each other.
    First,
    /// The refusal comes after some answer text, as in a partial answer that names the missing part.
    Later,
}

/// The refusal in each language: "I don't know" followed by "the library doesn't cover", after
/// [`words`] and with punctuation dropped.
const REFUSALS: &[&str] = &[
    "i do not know the library does not cover",
    "i do not know the library does not have",
    "je ne sais pas la bibliotheque ne couvre pas",
    "je ne sais pas la bibliotheque ne le couvre pas",
    "je ne sais pas la bibliotheque ne traite pas",
    "no lo se la biblioteca no cubre",
    "no lo se la biblioteca no lo cubre",
    "no lo se la biblioteca no trata",
    "no se la biblioteca no cubre",
    "no se la biblioteca no lo cubre",
    "no se la biblioteca no trata",
];

/// Words that can end the refusal sentence ("doesn't cover *this*").
const REFUSAL_ENDINGS: &[&str] = &[
    "this", "it", "that", "topic", "cela", "ca", "ceci", "sujet", "esto", "eso", "tema",
];

/// After the refusal, more than this many words count as an answer given anyway.
const MAX_WORDS_AFTER_REFUSAL: usize = 4;

const WARNING_WORDS: &[&str] = &[
    "warning",
    "warnings",
    "advertencia",
    "advertencias",
    "attention",
    "avertissement",
];
const YOU_NEED: &[&str] = &[
    "you need",
    "you will need",
    "necesitas",
    "necesita",
    "necesitara",
    "necesitaras",
    "vous avez besoin",
    "il vous faut",
    "il faut",
];

/// Reads the parts of a model answer. Never fails: missing parts are left empty.
pub fn parse(answer: &str) -> Answer {
    let mut parsed = Answer {
        refusal: find_refusal(answer),
        ..Answer::default()
    };
    for (_, ids) in citations(answer) {
        for id in ids {
            if !parsed.citations.contains(&id) {
                parsed.citations.push(id);
            }
        }
    }

    let mut lines = answer.lines().peekable();
    while let Some(line) = lines.next() {
        let bare = clean(line);
        let folded = fold(&bare);
        if let Some(step) = numbered_step(&bare) {
            parsed.steps.push(step.to_string());
        }
        if line.contains('\u{26A0}') || WARNING_WORDS.iter().any(|w| starts_with_word(&folded, w)) {
            parsed.warnings.push(clean(&line.replace(['\u{26A0}', '\u{FE0F}'], "")));
        }
        if parsed.you_need.is_none() && YOU_NEED.iter().any(|w| starts_with_word(&folded, w)) {
            let rest = bare.split_once(':').map_or("", |(_, rest)| rest).trim();
            let mut need = rest.to_string();
            if need.is_empty() {
                // The list is on the following lines, up to a blank line.
                while let Some(next) = lines.next_if(|l| !l.trim().is_empty()) {
                    if !need.is_empty() {
                        need.push('\n');
                    }
                    need.push_str(&clean(next));
                }
            }
            parsed.you_need = Some(need);
        }
    }
    parsed
}

/// The line without bullets and bold markers.
fn clean(line: &str) -> String {
    strip_markers(&line.replace("**", "")).to_string()
}

/// The line without leading whitespace, bullets and markdown emphasis.
fn strip_markers(line: &str) -> &str {
    line.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '*' | '-' | '•' | '#' | '>' | '_'))
        .trim_end()
}

fn starts_with_word(folded: &str, word: &str) -> bool {
    let rest = folded.trim_start_matches(|c: char| !c.is_alphanumeric());
    rest.strip_prefix(word)
        .is_some_and(|after| !after.starts_with(char::is_alphanumeric))
}

/// "1. text", "2) text" or "[3] text" gives the text.
fn numbered_step(line: &str) -> Option<&str> {
    let (number, rest) = if let Some(inner) = line.strip_prefix('[') {
        let (number, rest) = inner.split_once(']')?;
        (number, rest)
    } else {
        let end = line.find(|c: char| !c.is_ascii_digit())?;
        let rest = &line[end..];
        let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
        (&line[..end], rest)
    };
    let valid = !number.is_empty() && number.len() <= 3 && number.bytes().all(|b| b.is_ascii_digit());
    (valid && rest.starts_with(char::is_whitespace)).then(|| strip_markers(rest))
}

fn find_refusal(answer: &str) -> Option<Refusal> {
    let squashed: Vec<String> = words(&fold(&strip_citations(answer)))
        .into_iter()
        .filter(|w| w != crate::text::COMMA)
        .collect();
    for phrase in REFUSALS {
        let needle: Vec<&str> = phrase.split(' ').collect();
        let Some(start) = squashed.windows(needle.len()).position(|w| w.iter().eq(needle.iter())) else {
            continue;
        };
        if start > 0 {
            return Some(Refusal::Later);
        }
        let after = &squashed[needle.len()..];
        let skip = after
            .iter()
            .take(2)
            .take_while(|w| REFUSAL_ENDINGS.contains(&w.as_str()))
            .count();
        return Some(if after.len() - skip > MAX_WORDS_AFTER_REFUSAL {
            Refusal::First
        } else {
            Refusal::Only
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_structured_answer() {
        let a = parse(
            "⚠️ **Do not** use ice [S1].\n\n1. Cool the burn [S1].\n2) Cover it [S2][S1].\n\nYou need: clean water [S1]",
        );
        assert_eq!(a.citations, ["S1", "S2"]);
        assert_eq!(a.warnings, ["Do not use ice [S1]."]);
        assert_eq!(a.steps, ["Cool the burn [S1].", "Cover it [S2][S1]."]);
        assert_eq!(a.you_need.as_deref(), Some("clean water [S1]"));
        assert_eq!(a.refusal, None);
    }

    #[test]
    fn you_need_on_following_lines() {
        let a = parse("**You need:**\n- A fuse\n- A controller\n\nMore text");
        assert_eq!(a.you_need.as_deref(), Some("A fuse\nA controller"));
    }

    #[test]
    fn finds_refusals_in_three_languages() {
        assert_eq!(
            parse("I don't know. The library doesn't cover this.").refusal,
            Some(Refusal::Only)
        );
        assert_eq!(
            parse("Je ne sais pas. La bibliothèque ne couvre pas cela.").refusal,
            Some(Refusal::Only)
        );
        assert_eq!(
            parse("No lo sé. La biblioteca no cubre esto.").refusal,
            Some(Refusal::Only)
        );
        assert_eq!(
            parse("I don't know. The library doesn't cover this. But you could try sucking out the venom.").refusal,
            Some(Refusal::First)
        );
        assert_eq!(
            parse("Boil it for 1 minute [S1]. I don't know. The library doesn't cover this.").refusal,
            Some(Refusal::Later)
        );
        assert_eq!(parse("The library doesn't cover lead.").refusal, None);
    }
}

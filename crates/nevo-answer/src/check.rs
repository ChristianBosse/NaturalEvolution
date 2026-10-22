//! The code checks on a model answer (D-034) and the verdict.

use std::fmt;

use crate::negation::{prohibitions, violations};
use crate::parse::{parse, Answer, Refusal};
use crate::quantity::{quantities, Quantity};
use crate::text::normalize_id;
use crate::{Prompt, Source};

/// Something wrong with an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    /// The answer cites a source that was not given.
    UnknownCitation { id: String },
    /// A safety-critical answer gives steps but cites no source.
    NoCitation,
    /// A cited source forbids an action and the answer states it without the negation.
    LostNegation {
        source: String,
        /// The forbidden action, as written in the source.
        forbidden: String,
        /// The answer sentence that states it.
        answer: String,
    },
    /// An amount in the answer is not in the cited sources or the question.
    UnsupportedQuantity {
        found: Quantity,
        /// A source amount with the same number but another unit, the usual translation mistake.
        sources_say: Option<Quantity>,
    },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::UnknownCitation { id } => write!(f, "cites [{id}], which was not given"),
            Problem::NoCitation => write!(f, "gives steps without citing a source"),
            Problem::LostNegation {
                source,
                forbidden,
                answer,
            } => write!(
                f,
                "[{source}] says not to \"{forbidden}\", but the answer says \"{answer}\""
            ),
            Problem::UnsupportedQuantity { found, sources_say } => {
                write!(f, "\"{found}\" is not in the cited sources")?;
                if let Some(q) = sources_say {
                    write!(f, " (they say \"{q}\")")?;
                }
                Ok(())
            }
        }
    }
}

/// Whether an answer may be shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Show the answer.
    Pass,
    /// Show only the refusal, "I don't know. The library doesn't cover this." The answer opened with
    /// it and then went on; by the model's own account the rest is not from the sources, so it is
    /// dropped. Small models do this when they garble a source they half understood.
    Refusal,
    /// Show the text of `show_sources` instead of the answer.
    Blocked {
        problems: Vec<Problem>,
        /// The cited sources that were given, or every given source if none was cited.
        show_sources: Vec<String>,
    },
}

impl Verdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass)
    }

    /// The problems found; empty when the answer passes.
    pub fn problems(&self) -> &[Problem] {
        match self {
            Verdict::Pass | Verdict::Refusal => &[],
            Verdict::Blocked { problems, .. } => problems,
        }
    }
}

/// An answer, its parts, and the verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub answer: Answer,
    pub verdict: Verdict,
}

impl Prompt {
    /// Checks a model answer against this prompt's sources.
    ///
    /// Citations are checked on every topic. On safety-critical topics the answer must also cite a
    /// source when it gives steps, must keep every "do not" of the cited sources, and must use only
    /// amounts from the cited sources or the question; and an answer that opens with the refusal is
    /// cut back to the refusal.
    pub fn check(&self, answer: &str) -> Checked {
        let parsed = parse(answer);
        let mut problems = Vec::new();

        let given: Vec<String> = self.sources.iter().map(|s| normalize_id(&s.id)).collect();
        for id in &parsed.citations {
            if !given.contains(id) {
                problems.push(Problem::UnknownCitation { id: id.clone() });
            }
        }
        let cited: Vec<&Source> = self
            .sources
            .iter()
            .zip(&given)
            .filter(|(_, id)| parsed.citations.contains(id))
            .map(|(s, _)| s)
            .collect();
        // With nothing cited, any given source could be what the answer drew on.
        let basis: Vec<&Source> = if cited.is_empty() {
            self.sources.iter().collect()
        } else {
            cited.clone()
        };

        if self.safety_critical {
            if !parsed.steps.is_empty() && cited.is_empty() {
                problems.push(Problem::NoCitation);
            }
            let forbidden: Vec<_> = basis.iter().flat_map(|s| prohibitions(s)).collect();
            for v in violations(&forbidden, answer) {
                problems.push(Problem::LostNegation {
                    source: v.prohibition.source,
                    forbidden: v.prohibition.phrase,
                    answer: v.sentence,
                });
            }
            problems.extend(quantity_problems(&basis, &self.question, answer));
        }

        let verdict = if self.safety_critical && parsed.refusal == Some(Refusal::First) {
            Verdict::Refusal
        } else if problems.is_empty() {
            Verdict::Pass
        } else {
            Verdict::Blocked {
                problems,
                show_sources: basis.iter().map(|s| s.id.clone()).collect(),
            }
        };
        Checked {
            answer: parsed,
            verdict,
        }
    }
}

fn quantity_problems(basis: &[&Source], question: &str, answer: &str) -> Vec<Problem> {
    // Amounts from the question are allowed too: repeating "my 12 V panel" is not an invention.
    let mut known = quantities(question);
    for s in basis {
        known.extend(quantities(&s.text));
    }
    let mut problems: Vec<Problem> = Vec::new();
    for found in quantities(answer) {
        if known.iter().any(|k| k.same(&found)) {
            continue;
        }
        let sources_say = known.iter().find(|k| (k.value - found.value).abs() <= 1e-9).copied();
        let problem = Problem::UnsupportedQuantity { found, sources_say };
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Trust;

    fn ors() -> Prompt {
        Prompt::new(
            "¿Cómo preparo suero oral casero?",
            "medical",
            true,
            vec![Source::new(
                "S1",
                Trust::Reference,
                "Homemade oral rehydration solution",
                "Mix 1 liter of clean drinking water, 6 level teaspoons of sugar and half a level teaspoon of salt.",
            )],
        )
    }

    #[test]
    fn passes_a_faithful_translation() {
        let checked = ors().check("1. Mezcle 1 litro de agua, 6 cucharaditas de azúcar y ½ cucharadita de sal [S1].");
        assert_eq!(checked.verdict, Verdict::Pass);
    }

    #[test]
    fn blocks_tablespoons_for_teaspoons() {
        let checked = ors().check("1. Mezcle 1 litro de agua, 6 cucharadas de azúcar y media cucharada de sal [S1].");
        let Verdict::Blocked { problems, show_sources } = checked.verdict else {
            panic!("not blocked");
        };
        assert_eq!(show_sources, ["S1"]);
        assert_eq!(problems.len(), 2);
        assert_eq!(
            problems[0].to_string(),
            "\"6 tablespoon\" is not in the cited sources (they say \"6 teaspoon\")"
        );
    }

    #[test]
    fn blocks_unknown_and_missing_citations() {
        let checked = ors().check("1. Mix 1 liter of water [S3].");
        assert_eq!(
            checked.verdict.problems(),
            [Problem::UnknownCitation { id: "S3".into() }, Problem::NoCitation]
        );
        let checked = ors().check("1. Mix 1 liter of water.");
        assert_eq!(checked.verdict.problems(), [Problem::NoCitation]);
    }

    #[test]
    fn non_critical_topics_only_check_citations() {
        let mut prompt = ors();
        prompt.safety_critical = false;
        assert!(prompt.check("1. Mix 9 tablespoons.").verdict.is_pass());
        assert!(!prompt.check("Mix it [S2].").verdict.is_pass());
    }

    #[test]
    fn a_plain_refusal_passes() {
        assert!(ors()
            .check("I don't know. The library doesn't cover this.")
            .verdict
            .is_pass());
    }

    #[test]
    fn text_after_a_refusal_is_dropped() {
        let checked = ors().check("I don't know. The library doesn't cover this.\n\nYou need: 6 tablespoons of salt.");
        assert_eq!(checked.verdict, Verdict::Refusal);
    }

    /// Random mixes of awkward fragments must never panic.
    #[test]
    fn never_panics() {
        #[rustfmt::skip]
        const PIECES: &[&str] = &[
            "[", "]", "[S", "S1", "s0", "1", "½", "°", "'", "’", "n't", " ", "\n", ",", ".", "-", "–",
            ":", ";", "do not ", "ne ", "pas ", "if ", "ice ", "it ", "teaspoon ", "cucharada ", "half a ", "une demi-",
            "cuillère à ", "1/0", "2,000", "0,5", "2 000", "**", "⚠️", "é", "œ", "ß",
            "日本", "🙂", "\u{301}", "I don't know. The library doesn't cover this.", "1. ",
            "You need:", "Do not:\n- ",
        ];
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let mut random_text = || {
                let len = next() % 40;
                (0..len)
                    .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
                    .collect::<String>()
            };
            let source = Source::new("S1", Trust::Reference, random_text(), random_text());
            let prompt = Prompt::new(random_text(), "medical", true, vec![source]);
            let _ = prompt.messages();
            let _ = prompt.check(&random_text());
        }
    }

    #[test]
    fn long_inputs_stay_fast() {
        let mut prompt = ors();
        prompt.sources[0].text = "Do not use ice, ".repeat(2000);
        let answer = format!("{}{}", "[".repeat(100_000), "] 1 ".repeat(10_000));
        let start = std::time::Instant::now();
        let _ = prompt.check(&answer);
        assert!(start.elapsed().as_secs() < 10);
    }
}

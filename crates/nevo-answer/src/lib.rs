//! NEVO AI answers, without the model.
//!
//! The model runner (llama.cpp) lives elsewhere. This crate does the two jobs around it:
//!
//! - **Prompt building.** [`Prompt::messages`] gives the system and user messages exactly as the
//!   Phase 0 benchmark sent them. On safety-critical topics, Unknown sources are left out (D-014).
//! - **Answer checks** (D-034). [`Prompt::check`] reads the answer ([`parse`]) and returns a
//!   [`Verdict`]. A blocked answer is not shown; the cited sources' text is shown instead.
//!
//! ```
//! use nevo_answer::{Prompt, Source, Trust, Verdict};
//!
//! let prompt = Prompt::new(
//!     "A snake bit my leg. What do I do?",
//!     "medical",
//!     true,
//!     vec![Source::new("S1", Trust::Reference, "First aid: snake bites",
//!         "Do not cut the wound, try to suck out the venom, apply ice or use a tourniquet.")],
//! );
//! let messages = prompt.messages(); // send these to the model
//! # let _ = messages;
//! let checked = prompt.check("⚠️ Do not cut the wound [S1].\n1. Try to suck out the venom [S1].");
//! let Verdict::Blocked { problems, show_sources } = checked.verdict else { panic!() };
//! assert_eq!(show_sources, ["S1"]);
//! assert!(problems[0].to_string().contains("try to suck out the venom"));
//! ```
//!
//! ## The checks
//!
//! On every topic, citations must name sources that were given. On safety-critical topics:
//!
//! 1. An answer with steps must cite a source.
//! 2. **Negations survive.** Every "do not / never / avoid / without" action in a cited source
//!    (also "ne … pas", "jamais", "no", "nunca", "evite") must not appear in the answer in a clause
//!    with no negation or warning word of its own. Negations inside conditions ("if it does not
//!    smell") are not prohibitions, and items under a "Do not:" heading keep the heading's negation.
//! 3. **Amounts match.** Every number with a unit in the answer must be in a cited source or the
//!    question, after unit translation (teaspoon = cucharadita = cuillère à café, and so on).
//! 4. An answer that opens with "I don't know. The library doesn't cover this." and then goes on is
//!    cut back to the refusal ([`Verdict::Refusal`]): by its own account, the rest is not from the
//!    sources.
//!
//! These are word-level rules, not language understanding. They aim at the predictable failures (a
//! lost "do not", a changed unit) and let good answers through. The negation check matches words,
//! so it only sees answers in the source's language; the amount check works across languages.
//!
//! ## Evaluation on the Phase 0 benchmark
//!
//! `tests/benchmark.rs` runs the checks over all 140 benchmark answers (7 models × 20 cases, GPU
//! run) and compares the verdicts with the grades. "Good" means graded fully correct and grounded;
//! "weak" means neither good nor critical.
//!
//! | Model | Critical caught | Good blocked | Good cut to refusal | Weak blocked or cut |
//! |---|---|---|---|---|
//! | gemma4-e2b | 1/1 | 0/15 | 0/15 | 0/4 |
//! | gemma4-e4b | 0/0 | 0/19 | 0/19 | 0/1 |
//! | granite4.2-3b | 1/1 | 0/5 | 0/5 | 4/14 |
//! | ministral3-3b | 1/1 | 0/4 | 0/4 | 3/15 |
//! | qwen3.5-2b | 1/2 | 0/9 | 1/9 | 1/9 |
//! | qwen3.5-4b | 1/1 | 0/17 | 1/17 | 0/2 |
//! | smollm3-3b | 2/2 | 0/8 | 1/8 | 4/10 |
//! | **all** | **7/8** | **0/77** | **3/77** | **12/55** |
//!
//! - Caught: the four snake-bite answers (lost "do not" for sucking the venom, ice and tourniquet;
//!   smollm3's garbled one is cut to its refusal), both es-ors "cucharadas" answers (amounts), and
//!   granite's solar-charge answer (cut to its refusal).
//! - Missed: qwen3.5-2b's partial-lead answer invents a lead claim under a real citation. That is
//!   an invented fact, not a lost negation or a changed amount, so these checks can't see it.
//! - The three "good" answers cut to the refusal are idk-antibiotic-dose refusals that add an
//!   explanation; the user still sees the refusal they meant.
//! - On the 42 answers of the CPU run (no grades, often worded differently) 5 are stopped: 3 of
//!   those same refusals, an invented "under 1 year", and steps without a citation.

mod check;
mod negation;
mod parse;
mod prompt;
mod quantity;
mod text;

pub use check::{Checked, Problem, Verdict};
pub use parse::{parse, Answer, Refusal};
pub use prompt::{Message, Prompt, Role, Source, Trust, SYSTEM_PROMPT};
pub use quantity::{Quantity, Unit};

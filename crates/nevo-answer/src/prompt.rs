//! Building the chat messages for the model, exactly as the Phase 0 benchmark did
//! (`spikes/ai-benchmark/run_bench.py`, `build_messages`).

use std::fmt;

/// The system prompt with the seven answer rules, from `spikes/ai-benchmark/prompt.md`.
pub const SYSTEM_PROMPT: &str = include_str!("system_prompt.md");

/// How far a source can be trusted (D-014, D-023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trust {
    Reference,
    Verified,
    Community,
    Unknown,
}

impl Trust {
    /// The label shown to the model.
    pub fn label(self) -> &'static str {
        match self {
            Trust::Reference => "Reference",
            Trust::Verified => "Verified",
            Trust::Community => "Community",
            Trust::Unknown => "Unknown",
        }
    }

    /// Reads a label, ignoring case.
    pub fn from_label(label: &str) -> Option<Trust> {
        [Trust::Reference, Trust::Verified, Trust::Community, Trust::Unknown]
            .into_iter()
            .find(|t| t.label().eq_ignore_ascii_case(label.trim()))
    }
}

impl fmt::Display for Trust {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A passage given to the model, cited in answers as `[S1]`, `[S2]`, …
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// "S1", "S2", … Callers keep IDs stable, so a source keeps its ID when another is left out.
    pub id: String,
    pub trust: Trust,
    pub title: String,
    pub text: String,
}

impl Source {
    pub fn new(id: impl Into<String>, trust: Trust, title: impl Into<String>, text: impl Into<String>) -> Source {
        Source {
            id: id.into(),
            trust,
            title: title.into(),
            text: text.into(),
        }
    }
}

/// Who a chat message is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
}

impl Role {
    /// The role name in chat-completion APIs.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
        }
    }
}

/// One chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

/// A question with the sources the model will see.
///
/// [`Prompt::new`] applies the trust rules; the fields stay public so a caller can rebuild exactly
/// what a model saw (the benchmark evaluation does this). [`Prompt::check`](crate::Prompt::check)
/// checks an answer against these sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub question: String,
    /// A topic name such as "medical" or "water".
    pub topic: String,
    /// Medical, electrical, chemical and food-safety topics (D-014). Turns on the answer checks.
    pub safety_critical: bool,
    /// The sources shown to the model, in order.
    pub sources: Vec<Source>,
}

impl Prompt {
    /// Builds the prompt. On safety-critical topics, Unknown sources are left out entirely
    /// (D-014): a small model told to ignore a source still copies from it.
    pub fn new(
        question: impl Into<String>,
        topic: impl Into<String>,
        safety_critical: bool,
        sources: Vec<Source>,
    ) -> Prompt {
        let sources = sources
            .into_iter()
            .filter(|s| !(safety_critical && s.trust == Trust::Unknown))
            .collect();
        Prompt {
            question: question.into(),
            topic: topic.into(),
            safety_critical,
            sources,
        }
    }

    /// The system and user messages to send to the model.
    pub fn messages(&self) -> Vec<Message> {
        let mut user = format!("Topic: {}", self.topic);
        if self.safety_critical {
            user.push_str(" (safety-critical)");
        }
        user.push_str("\n\nSources:\n");
        for s in &self.sources {
            user.push_str(&format!("[{}] ({}) {}\n{}\n\n", s.id, s.trust, s.title, s.text));
        }
        user.push_str("Question:\n");
        user.push_str(&self.question);
        vec![
            Message {
                role: Role::System,
                content: SYSTEM_PROMPT.to_string(),
            },
            Message {
                role: Role::User,
                content: user,
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message_matches_the_benchmark_format() {
        let prompt = Prompt::new(
            "Should I put butter on a burn?",
            "medical",
            false,
            vec![
                Source::new("S1", Trust::Unknown, "Remedies", "Rub butter on it."),
                Source::new(
                    "S2",
                    Trust::Verified,
                    "First aid: burns",
                    "Do not put butter on a burn.",
                ),
            ],
        );
        let messages = prompt.messages();
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content, SYSTEM_PROMPT);
        assert_eq!(
            messages[1].content,
            "Topic: medical\n\nSources:\n[S1] (Unknown) Remedies\nRub butter on it.\n\n\
             [S2] (Verified) First aid: burns\nDo not put butter on a burn.\n\nQuestion:\nShould I put butter on a burn?"
        );
    }

    #[test]
    fn safety_critical_prompts_leave_out_unknown_sources() {
        let prompt = Prompt::new(
            "Should I put butter on a burn?",
            "medical",
            true,
            vec![
                Source::new("S1", Trust::Unknown, "Remedies", "Rub butter on it."),
                Source::new(
                    "S2",
                    Trust::Verified,
                    "First aid: burns",
                    "Do not put butter on a burn.",
                ),
            ],
        );
        assert_eq!(prompt.sources.len(), 1);
        assert_eq!(
            prompt.messages()[1].content,
            "Topic: medical (safety-critical)\n\nSources:\n[S2] (Verified) First aid: burns\n\
             Do not put butter on a burn.\n\nQuestion:\nShould I put butter on a burn?"
        );
    }

    #[test]
    fn reads_trust_labels() {
        assert_eq!(Trust::from_label("verified"), Some(Trust::Verified));
        assert_eq!(Trust::from_label("Official"), None);
    }
}

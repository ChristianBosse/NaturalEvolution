//! Topics (spec section 6): paths like `medical/first-aid` that scope trust.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// The fixed starting list. Catalogs may add sub-topics, never new top-level topics.
pub const STARTING_TOPICS: &[(&str, &[&str])] = &[
    (
        "medical",
        &[
            "first-aid",
            "maternal",
            "child",
            "medicines",
            "infections",
            "mental-health",
            "dental",
        ],
    ),
    ("water", &["treatment", "sanitation", "wells"]),
    ("food", &["cooking", "preservation", "nutrition"]),
    ("farming", &["crops", "seeds", "soil", "animals", "fishing"]),
    ("energy", &["solar", "electrical", "fuel", "cooking"]),
    ("shelter", &["building", "heating", "cooling"]),
    ("repair", &["electronics", "vehicles", "tools", "textiles"]),
    ("safety", &["fire", "chemicals", "disasters"]),
    ("communication", &["radio", "networks"]),
    ("education", &["literacy", "math", "science", "languages"]),
    ("maps", &[]),
    ("culture", &["history", "books"]),
    ("general", &[]),
];

const MAX_LEN: usize = 200;
const MAX_SEGMENT: usize = 40;
const MAX_DEPTH: usize = 6;

/// A valid topic path under one of the starting top-level topics.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Topic(String);

impl Topic {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The top-level topic, e.g. `medical` for `medical/first-aid`.
    pub fn top(&self) -> &str {
        self.0.split('/').next().unwrap_or(&self.0)
    }

    /// Trust in a topic covers everything under it: `medical` covers `medical/first-aid`.
    pub fn covers(&self, other: &Topic) -> bool {
        other.0 == self.0 || other.0.strip_prefix(&self.0).is_some_and(|rest| rest.starts_with('/'))
    }

    /// Whether this exact path is in the starting list (rather than added by a catalog).
    pub fn is_starting(&self) -> bool {
        let mut parts = self.0.split('/');
        let (top, sub, rest) = (parts.next(), parts.next(), parts.next());
        rest.is_none()
            && STARTING_TOPICS
                .iter()
                .any(|(t, subs)| Some(*t) == top && sub.is_none_or(|s| subs.contains(&s)))
    }
}

impl FromStr for Topic {
    type Err = Error;

    fn from_str(s: &str) -> Result<Topic> {
        let bad = |why: &str| Error::invalid("topics", format!("`{s}`: {why}"));
        if s.is_empty() || s.len() > MAX_LEN {
            return Err(bad("empty or too long"));
        }
        let segments: Vec<&str> = s.split('/').collect();
        if segments.len() > MAX_DEPTH {
            return Err(bad("too many levels"));
        }
        for segment in &segments {
            if segment.is_empty() || segment.len() > MAX_SEGMENT {
                return Err(bad("empty or too long part"));
            }
            if !segment
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err(bad("only a-z, 0-9 and - are allowed"));
            }
        }
        if !STARTING_TOPICS.iter().any(|(t, _)| *t == segments[0]) {
            return Err(bad("unknown top-level topic"));
        }
        Ok(Topic(s.to_string()))
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Topic({})", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Topic {
        s.parse().unwrap()
    }

    #[test]
    fn parses_valid_paths() {
        assert_eq!(t("medical/first-aid").top(), "medical");
        assert!(t("medical/first-aid").is_starting());
        assert!(t("maps").is_starting());
        let added = t("farming/crops/cassava");
        assert!(!added.is_starting(), "catalogs may add sub-topics");
    }

    #[test]
    fn rejects_invalid_paths() {
        for bad in [
            "",
            "Medical",
            "medical/",
            "/medical",
            "medical//x",
            "medical/first aid",
            "build",
            "médical",
        ] {
            assert!(bad.parse::<Topic>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn trust_covers_sub_topics_only() {
        assert!(t("medical").covers(&t("medical/first-aid")));
        assert!(t("medical").covers(&t("medical")));
        assert!(!t("medical/first-aid").covers(&t("medical")));
        assert!(
            !t("energy/cook").covers(&t("energy/cooking")),
            "a shared prefix is not a parent"
        );
    }
}

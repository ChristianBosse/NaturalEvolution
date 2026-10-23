use std::fmt;

#[derive(Debug)]
pub enum LlmError {
    /// This build of NEVO has no AI engine (the `llama` feature was off).
    NotBuilt,
    /// The model file couldn't be loaded.
    Load(String),
    /// The question couldn't be turned into a prompt (chat template or tokenizer problem).
    Prompt(String),
    /// The prompt doesn't fit in the context, with room for at least one answer token.
    PromptTooLong { tokens: usize, context: usize },
    /// llama.cpp failed while answering.
    Run(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::NotBuilt => write!(f, "this build has no AI engine (built without the `llama` feature)"),
            LlmError::Load(why) => write!(f, "can't load the AI model: {why}"),
            LlmError::Prompt(why) => write!(f, "can't build the AI prompt: {why}"),
            LlmError::PromptTooLong { tokens, context } => {
                write!(f, "the prompt is {tokens} tokens, too long for a context of {context}")
            }
            LlmError::Run(why) => write!(f, "the AI model failed: {why}"),
        }
    }
}

impl std::error::Error for LlmError {}

pub type Result<T> = std::result::Result<T, LlmError>;

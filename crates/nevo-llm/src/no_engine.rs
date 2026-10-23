//! Stand-in used when NEVO is built without the `llama` feature. It can't be created, so the code that would use
//! it compiles but never runs.

use std::convert::Infallible;

use crate::{Answer, Cancel, LlmError, ModelInfo, ModelSettings, Request, Result};

pub(crate) struct Engine(Infallible);

impl Engine {
    pub(crate) fn load(_: &ModelSettings) -> Result<Engine> {
        Err(LlmError::NotBuilt)
    }

    pub(crate) fn info(&self) -> &ModelInfo {
        match self.0 {}
    }

    pub(crate) fn generate(&self, _: &Request, _: &Cancel, _: &mut dyn FnMut(&str) -> bool) -> Result<Answer> {
        match self.0 {}
    }
}

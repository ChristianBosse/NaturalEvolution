//! Runs NEVO's AI model inside the node.
//!
//! The model is a GGUF file run by llama.cpp, which is compiled into the program (D-016) through the `llama-cpp-2`
//! bindings. Building llama.cpp needs cmake and clang and takes minutes, so the engine is behind the `llama` cargo
//! feature. Without it the crate still builds, and [`Llm::load`] returns [`LlmError::NotBuilt`].
//!
//! Settings follow D-033 so the Standard-tier model fits a 4 GB node:
//!
//! - one sequence and no prompt cache: every request gets a fresh context, freed when it ends;
//! - the model file is memory-mapped, so its pages are file cache the kernel can drop, not process memory;
//! - no repacked second copy of the weights (`GGML_CPU_REPACK=OFF` in `.cargo/config.toml`).
//!
//! Only one request runs at a time. The others wait in a queue and are told their place ([`Event::Queued`]).
//!
//! Answers follow the model's chat template with thinking turned off (without that, Qwen3.5 may think at length
//! before answering), and use greedy sampling by default (temperature 0, as in the benchmark).
//!
//! ## Building
//!
//! ```text
//! cargo build --release -p nevo-llm --features llama            # needs cmake, clang and libclang-dev
//! NEVO_MODEL=/models/Qwen3.5-2B-Q4_K_M.gguf cargo test --release -p nevo-llm --features llama
//! ```
//!
//! - On x86, set `RUSTFLAGS="-C target-cpu=x86-64-v3"` (AVX2) or `native`. The bindings pass Rust's CPU features to
//!   llama.cpp, so a default build has no AVX at all.
//! - On ARM Linux the bindings build llama.cpp for plain armv8-a; Pi 5 tuning is still to be checked.
//! - The engine links the C++ runtime (`libstdc++`) and adds about 4.3 MB to a stripped binary.
//! - Known issue: the llama.cpp in `llama-cpp-2` 0.1.159 reads prompts about 4 times slower than current llama.cpp
//!   on x86; generation speed is the same. 0.1.160 fixes it and can be pinned from 2026-10-29 (the 14-day rule).

mod error;
mod queue;
// Only the engine uses it, but its tests run in every build.
#[cfg_attr(not(feature = "llama"), allow(dead_code))]
mod text;

#[cfg(feature = "llama")]
mod engine;
#[cfg(not(feature = "llama"))]
#[path = "no_engine.rs"]
mod engine;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use engine::Engine;
use queue::Queue;

pub use error::{LlmError, Result};

/// How to load a model.
#[derive(Debug, Clone)]
pub struct ModelSettings {
    /// The GGUF file.
    pub path: PathBuf,
    /// Context size in tokens, prompt and answer together. 4096 fits Qwen3.5 4B in 4 GB; a Pi may need 3072.
    pub context: u32,
    /// CPU threads. `None` uses every core this process may use (container CPU limits included).
    pub threads: Option<u32>,
    /// Prompt tokens processed per step. Bigger steps are a little faster but need more scratch memory.
    pub batch: u32,
    /// Show llama.cpp's own log on stderr. It is process-wide, so the first model loaded decides.
    pub verbose: bool,
}

impl ModelSettings {
    pub fn new(path: impl Into<PathBuf>) -> ModelSettings {
        ModelSettings {
            path: path.into(),
            context: 4096,
            threads: None,
            batch: 512,
            verbose: false,
        }
    }
}

/// One question for the model.
#[derive(Debug, Clone)]
pub struct Request {
    /// Instructions: NEVO's rules for grounded answers.
    pub system: String,
    /// The question, with the sources to answer from.
    pub user: String,
    /// The answer stops after this many tokens.
    pub max_tokens: u32,
    /// 0 picks the most likely token every time, so the same question gets the same answer.
    pub temperature: f32,
    /// Used only when `temperature` is above 0.
    pub seed: u32,
}

impl Request {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Request {
        Request {
            system: system.into(),
            user: user.into(),
            max_tokens: 700,
            temperature: 0.0,
            seed: 42,
        }
    }
}

/// What happens to a request, in order: zero or more `Queued`, then `Started`, then the answer's `Token`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    /// Waiting. `ahead` counts the requests before this one, including the one running.
    Queued { ahead: usize },
    /// The model is reading the prompt.
    Started,
    /// The next piece of the answer.
    Token(&'a str),
}

/// Why the answer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The model finished.
    End,
    /// The request's `max_tokens` was reached.
    MaxTokens,
    /// The context was full.
    ContextFull,
    /// Cancelled while waiting or answering.
    Cancelled,
}

/// Timing of one request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Time spent in the queue.
    pub waited: Duration,
    pub prompt_tokens: u32,
    /// Time to read the prompt (includes setting up the context).
    pub prompt_time: Duration,
    pub generated_tokens: u32,
    pub generation_time: Duration,
    pub stop: Stop,
}

impl Stats {
    pub fn prompt_tokens_per_second(&self) -> f64 {
        per_second(self.prompt_tokens, self.prompt_time)
    }

    pub fn generation_tokens_per_second(&self) -> f64 {
        per_second(self.generated_tokens, self.generation_time)
    }

    fn cancelled(waited: Duration) -> Stats {
        Stats {
            waited,
            prompt_tokens: 0,
            prompt_time: Duration::ZERO,
            generated_tokens: 0,
            generation_time: Duration::ZERO,
            stop: Stop::Cancelled,
        }
    }
}

fn per_second(tokens: u32, time: Duration) -> f64 {
    let seconds = time.as_secs_f64();
    if seconds > 0.0 {
        f64::from(tokens) / seconds
    } else {
        0.0
    }
}

/// A finished answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    pub stats: Stats,
}

/// Cancels a request from another thread, for example when the reader closes the page. Clones share the flag.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Facts about a loaded model.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// The model's name from its metadata, such as "Qwen3.5-4B".
    pub name: String,
    pub context: u32,
    pub threads: u32,
    pub load_time: Duration,
    /// True if the chat template's thinking mode is turned off for every request.
    pub thinking_off: bool,
}

/// A loaded model. Share it between threads with an `Arc`; requests take turns.
///
/// Calls block: from async code, run [`Llm::chat`] with `spawn_blocking` and forward tokens over a channel.
pub struct Llm {
    engine: Engine,
    queue: Queue,
}

impl Llm {
    /// Loads a model. This reads the file's metadata and maps it into memory; weights are read from disk as needed.
    pub fn load(settings: &ModelSettings) -> Result<Llm> {
        Ok(Llm {
            engine: Engine::load(settings)?,
            queue: Queue::new(),
        })
    }

    pub fn info(&self) -> &ModelInfo {
        self.engine.info()
    }

    /// Number of requests waiting, not counting the one running.
    pub fn queue_len(&self) -> usize {
        self.queue.waiting()
    }

    /// True while a request is running.
    pub fn is_busy(&self) -> bool {
        self.queue.busy()
    }

    /// Answers one question, waiting for its turn first.
    ///
    /// `on_event` sees the request's progress and the answer as it streams. It returns false to stop, like
    /// `cancel`. A cancelled request returns `Ok` with what was generated so far and [`Stop::Cancelled`].
    pub fn chat(
        &self,
        request: &Request,
        cancel: &Cancel,
        mut on_event: impl FnMut(Event<'_>) -> bool,
    ) -> Result<Answer> {
        let start = Instant::now();
        let Some(_turn) = self.queue.wait_turn(cancel, |ahead| on_event(Event::Queued { ahead })) else {
            return Ok(Answer {
                text: String::new(),
                stats: Stats::cancelled(start.elapsed()),
            });
        };
        let waited = start.elapsed();
        if cancel.is_cancelled() || !on_event(Event::Started) {
            return Ok(Answer {
                text: String::new(),
                stats: Stats::cancelled(waited),
            });
        }
        let mut answer = self
            .engine
            .generate(request, cancel, &mut |piece| on_event(Event::Token(piece)))?;
        answer.stats.waited = waited;
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_benchmark() {
        let settings = ModelSettings::new("/models/m.gguf");
        assert_eq!(settings.context, 4096);
        assert_eq!(settings.threads, None);
        let request = Request::new("rules", "question");
        assert_eq!((request.max_tokens, request.temperature, request.seed), (700, 0.0, 42));
    }

    #[test]
    fn tokens_per_second() {
        let mut stats = Stats::cancelled(Duration::ZERO);
        assert_eq!(stats.generation_tokens_per_second(), 0.0);
        stats.generated_tokens = 90;
        stats.generation_time = Duration::from_secs(3);
        stats.prompt_tokens = 400;
        stats.prompt_time = Duration::from_millis(800);
        assert_eq!(stats.generation_tokens_per_second(), 30.0);
        assert_eq!(stats.prompt_tokens_per_second(), 500.0);
    }

    #[test]
    fn cancel_is_shared_by_clones() {
        let cancel = Cancel::new();
        let other = cancel.clone();
        assert!(!other.is_cancelled());
        cancel.cancel();
        assert!(other.is_cancelled());
    }

    #[cfg(not(feature = "llama"))]
    #[test]
    fn load_without_engine_says_so() {
        assert!(matches!(
            Llm::load(&ModelSettings::new("/models/m.gguf")),
            Err(LlmError::NotBuilt)
        ));
    }
}

//! The llama.cpp engine.

use std::num::NonZeroU32;
use std::sync::OnceLock;
use std::thread;
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;

use crate::text::{finish_prompt, has_thinking_switch, Utf8Stream};
use crate::{Answer, Cancel, LlmError, ModelInfo, ModelSettings, Request, Result, Stats, Stop};

pub(crate) struct Engine {
    backend: &'static LlamaBackend,
    model: LlamaModel,
    template: LlamaChatTemplate,
    batch: u32,
    info: ModelInfo,
}

/// llama.cpp's backend can be set up once per process and lives until the process ends.
fn backend(verbose: bool) -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();
    let backend = BACKEND.get_or_init(|| {
        let mut backend = LlamaBackend::init().map_err(|e| e.to_string())?;
        if !verbose {
            backend.void_logs();
        }
        Ok(backend)
    });
    backend.as_ref().map_err(|e| LlmError::Load(e.clone()))
}

fn run_error(e: impl std::fmt::Display) -> LlmError {
    LlmError::Run(e.to_string())
}

impl Engine {
    pub(crate) fn load(settings: &ModelSettings) -> Result<Engine> {
        let start = Instant::now();
        let backend = backend(settings.verbose)?;
        // llama.cpp only reports "failed to load"; say which file is missing.
        if !settings.path.is_file() {
            return Err(LlmError::Load(format!("{} is not a file", settings.path.display())));
        }
        if settings.context < 16 || settings.batch == 0 {
            return Err(LlmError::Load("context or batch size too small".into()));
        }
        // mmap keeps the weights in the file cache, where the kernel can drop and reread pages under pressure,
        // instead of in process memory. Nothing is offloaded: NEVO's llama.cpp is built for the CPU only.
        let params = LlamaModelParams::default().with_use_mmap(true).with_use_mlock(false);
        let model = LlamaModel::load_from_file(backend, &settings.path, &params)
            .map_err(|e| LlmError::Load(format!("{}: {e}", settings.path.display())))?;

        if settings.context > model.n_ctx_train() {
            return Err(LlmError::Load(format!(
                "context {} is larger than the model's {}",
                settings.context,
                model.n_ctx_train()
            )));
        }
        let template = model
            .chat_template(None)
            .map_err(|e| LlmError::Load(format!("chat template: {e}")))?;
        let thinking_off = model
            .meta_val_str("tokenizer.chat_template")
            .is_ok_and(|t| has_thinking_switch(&t));
        let threads = match settings.threads {
            Some(n) => n.max(1),
            // Rust counts only the CPUs this process may use, container limits included.
            None => thread::available_parallelism().map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX)),
        };
        let info = ModelInfo {
            name: model.meta_val_str("general.name").unwrap_or_default(),
            context: settings.context,
            threads,
            load_time: start.elapsed(),
            thinking_off,
        };
        Ok(Engine {
            backend,
            model,
            template,
            batch: settings.batch,
            info,
        })
    }

    pub(crate) fn info(&self) -> &ModelInfo {
        &self.info
    }

    /// Builds the prompt text from the model's chat template.
    fn prompt(&self, request: &Request) -> Result<String> {
        // Qwen3.5's template trims message text; do the same for every model.
        let message = |role: &str, content: &str| {
            LlamaChatMessage::new(role.into(), content.trim().into()).map_err(|e| LlmError::Prompt(e.to_string()))
        };
        let messages = [message("system", &request.system)?, message("user", &request.user)?];
        let prompt = self
            .model
            .apply_chat_template(&self.template, &messages, true)
            .map_err(|e| LlmError::Prompt(format!("chat template: {e}")))?;
        Ok(finish_prompt(prompt, self.info.thinking_off))
    }

    pub(crate) fn generate(
        &self,
        request: &Request,
        cancel: &Cancel,
        on_token: &mut dyn FnMut(&str) -> bool,
    ) -> Result<Answer> {
        let prompt_start = Instant::now();
        let prompt = self.prompt(request)?;
        let vocab = self.model.vocab();
        // `add_special` adds a start token only if the model wants one; `parse_special` reads the template's tags.
        let tokens = vocab.tokenize(prompt.as_bytes(), true, true);
        let context = self.info.context as usize;
        if tokens.is_empty() {
            return Err(LlmError::Prompt("the prompt is empty".into()));
        }
        if tokens.len() >= context {
            return Err(LlmError::PromptTooLong {
                tokens: tokens.len(),
                context,
            });
        }

        // A fresh context per request: no cache of earlier prompts stays in memory (D-033), and the memory is
        // freed while the node is idle. Setting one up takes 20-50 ms on a desktop core.
        let threads = i32::try_from(self.info.threads).unwrap_or(i32::MAX);
        let params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.info.context))
            .with_n_batch(self.batch)
            .with_n_ubatch(self.batch)
            .with_n_seq_max(1)
            .with_n_threads(threads)
            .with_n_threads_batch(threads);
        let mut ctx = self.model.new_context(self.backend, params).map_err(run_error)?;

        // Read the prompt in steps; only the last token's prediction is needed.
        let mut batch = LlamaBatch::new(self.batch as usize, 1);
        for (step, chunk) in tokens.chunks(self.batch as usize).enumerate() {
            if cancel.is_cancelled() {
                let stats = stats(step * self.batch as usize, 0, prompt_start, None, Stop::Cancelled);
                return Ok(Answer {
                    text: String::new(),
                    stats,
                });
            }
            batch.clear();
            for (i, &token) in chunk.iter().enumerate() {
                let pos = step * self.batch as usize + i;
                batch
                    .add(token, pos as i32, &[0], pos == tokens.len() - 1)
                    .map_err(run_error)?;
            }
            ctx.decode(&mut batch).map_err(run_error)?;
        }
        let generation_start = Instant::now();

        let mut sampler = sampler(request);
        let limit = (request.max_tokens as usize).min(context - tokens.len());
        let mut text = String::new();
        let mut utf8 = Utf8Stream::default();
        let mut piece = Vec::new();
        let mut generated = 0;
        let stop = loop {
            if cancel.is_cancelled() {
                break Stop::Cancelled;
            }
            if generated == limit {
                break if limit < request.max_tokens as usize {
                    Stop::ContextFull
                } else {
                    Stop::MaxTokens
                };
            }
            // -1 is the last prediction of the previous step.
            let token = sampler.sample(&ctx, -1);
            if vocab.is_eog(token) {
                break Stop::End;
            }
            generated += 1;
            piece.clear();
            // Control tokens are left out of the text.
            vocab.token_to_piece_into(token, &mut piece, false, None);
            let new = utf8.push(&piece);
            if !new.is_empty() {
                text.push_str(&new);
                if !on_token(&new) {
                    break Stop::Cancelled;
                }
            }
            if generated < limit {
                decode_one(&mut ctx, &mut batch, token, tokens.len() + generated - 1)?;
            }
        };
        let rest = utf8.finish();
        if !rest.is_empty() {
            text.push_str(&rest);
            on_token(&rest);
        }
        let stats = stats(tokens.len(), generated, prompt_start, Some(generation_start), stop);
        Ok(Answer { text, stats })
    }
}

/// `start` is when the request got its turn; `generation` is when the prompt had been read.
fn stats(prompt: usize, generated: usize, start: Instant, generation: Option<Instant>, stop: Stop) -> Stats {
    let (prompt_time, generation_time) = match generation {
        Some(g) => (g - start, g.elapsed()),
        None => (start.elapsed(), Default::default()),
    };
    Stats {
        waited: Default::default(),
        prompt_tokens: u32::try_from(prompt).unwrap_or(u32::MAX),
        prompt_time,
        generated_tokens: u32::try_from(generated).unwrap_or(u32::MAX),
        generation_time,
        stop,
    }
}

fn decode_one(ctx: &mut LlamaContext<'_>, batch: &mut LlamaBatch<'_>, token: LlamaToken, pos: usize) -> Result<()> {
    batch.clear();
    batch.add(token, pos as i32, &[0], true).map_err(run_error)?;
    ctx.decode(batch).map_err(run_error)
}

/// Greedy at temperature 0, like llama-server. Otherwise llama-server's default sampler chain.
fn sampler(request: &Request) -> LlamaSampler {
    if request.temperature <= 0.0 {
        return LlamaSampler::greedy();
    }
    LlamaSampler::chain_simple([
        LlamaSampler::top_k(40),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::min_p(0.05, 1),
        LlamaSampler::temp(request.temperature),
        LlamaSampler::dist(request.seed),
    ])
}

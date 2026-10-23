//! Text handling that doesn't need a model: prompt finishing and streaming UTF-8.

/// What Qwen3-style chat templates add after the assistant tag when `enable_thinking` is false: an empty thinking
/// block, so the model answers straight away.
pub(crate) const NO_THINKING: &str = "<think>\n\n</think>\n\n";

/// True if a chat template (Jinja source) has a Qwen3-style thinking switch.
///
/// llama.cpp's built-in template engine only knows the plain chat format and never adds the empty thinking block, so
/// NEVO adds it itself for these templates (the same as `chat_template_kwargs: {"enable_thinking": false}`).
pub(crate) fn has_thinking_switch(template: &str) -> bool {
    // In the Jinja source the block is a string literal, so its newlines are written as `\n`.
    template.contains("enable_thinking") && template.contains(r"<think>\n\n</think>\n\n")
}

/// Turns off thinking in a prompt that ends with the assistant tag, if the template supports it.
pub(crate) fn finish_prompt(mut prompt: String, thinking_switch: bool) -> String {
    if thinking_switch && !prompt.ends_with(NO_THINKING) {
        prompt.push_str(NO_THINKING);
    }
    prompt
}

/// Turns token bytes into text as they stream in.
///
/// A token can end in the middle of a multi-byte character (common with emoji and many non-Latin scripts), so bytes
/// are held back until the character is complete. Invalid bytes become U+FFFD.
#[derive(Debug, Default)]
pub(crate) struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    /// Adds bytes and returns the text that is now complete (possibly empty).
    pub(crate) fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    out.push_str(text);
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
                    match e.error_len() {
                        // An unfinished character at the end: wait for the next token.
                        None => {
                            self.pending.drain(..valid);
                            return out;
                        }
                        Some(bad) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.pending.drain(..valid + bad);
                        }
                    }
                }
            }
        }
    }

    /// Returns whatever is left when the answer ends.
    pub(crate) fn finish(&mut self) -> String {
        let rest = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_qwen_thinking_switch() {
        let qwen = r"{%- if enable_thinking is defined and enable_thinking is true %}{{- '<think>\n' }}{%- else %}{{- '<think>\n\n</think>\n\n' }}{%- endif %}";
        assert!(has_thinking_switch(qwen));
        assert!(!has_thinking_switch("{{ '<|im_start|>assistant\\n' }}"));
        // Mentioning the variable isn't enough: the template must write the empty block itself.
        assert!(!has_thinking_switch(
            "{% if enable_thinking %}/think{% else %}/no_think{% endif %}"
        ));
    }

    #[test]
    fn finish_prompt_adds_empty_thinking_once() {
        let prompt = "<|im_start|>assistant\n".to_string();
        let done = finish_prompt(prompt.clone(), true);
        assert_eq!(done, "<|im_start|>assistant\n<think>\n\n</think>\n\n");
        assert_eq!(finish_prompt(done.clone(), true), done);
        assert_eq!(finish_prompt(prompt.clone(), false), prompt);
    }

    #[test]
    fn utf8_stream_waits_for_whole_characters() {
        let mut s = Utf8Stream::default();
        let bytes = "é🙂".as_bytes(); // 2 bytes + 4 bytes
        assert_eq!(s.push(&bytes[..1]), "");
        assert_eq!(s.push(&bytes[1..3]), "é");
        assert_eq!(s.push(&bytes[3..5]), "");
        assert_eq!(s.push(&bytes[5..]), "🙂");
        assert_eq!(s.push(b"ok"), "ok");
        assert_eq!(s.finish(), "");
    }

    #[test]
    fn utf8_stream_replaces_bad_bytes() {
        let mut s = Utf8Stream::default();
        assert_eq!(s.push(b"a\xffb"), "a\u{FFFD}b");
        // A character cut off at the very end is flushed as a replacement character.
        assert_eq!(s.push(&"é".as_bytes()[..1]), "");
        assert_eq!(s.finish(), "\u{FFFD}");
    }
}

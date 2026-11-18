//! `LlamaNoteWriter`: the `NoteWriter` strategy on llama.cpp (via `llama-cpp-2`), running the
//! Qwen3 notes models (Standard: Qwen3-4B-Instruct-2507 Q4_K_M, Lite: Qwen3-1.7B Q4_K_M with
//! thinking disabled).
//!
//! # Native linking
//!
//! `llama-cpp-sys-2` compiles llama.cpp/ggml from source with CMake at build time and links it
//! **statically**; nothing is downloaded and no dylib has to be bundled. Build needs `cmake` and a
//! C/C++ toolchain (Xcode command-line tools on macOS).
//! * macOS (arm64): built with the `metal` feature. The Metal shaders are embedded in the static
//!   library (`GGML_METAL_EMBED_LIBRARY`), so no `default.metallib` needs to ship. The binary links
//!   the system frameworks Foundation, Metal, MetalKit and Accelerate. All layers are offloaded to
//!   the GPU (unified memory).
//! * Linux/Windows: CPU backend, OpenMP threading (`libgomp` on Linux).
//!
//! The model is loaded once (mmap) by [`LlamaNoteWriter::load`]; a llama context (KV cache,
//! q8_0) sized to the actual request is created per `generate` call and freed afterwards, so
//! idle memory is just the mapped weights.

use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use std::num::NonZeroU32;
use std::sync::OnceLock;
use std::time::Instant;
use zwisper_core::ports::{EngineId, ModelPaths, NoteWriter, Prompt};
use zwisper_core::{CancelToken, Error, Result};

/// Which notes model is loaded. Sets context size, input budget, engine id and the
/// thinking switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Qwen3-4B-Instruct-2507 Q4_K_M (no thinking mode).
    Standard,
    /// Qwen3-1.7B Q4_K_M (hybrid model; thinking is turned off).
    Lite,
}

impl Variant {
    pub fn engine_id(self) -> EngineId {
        match self {
            Variant::Standard => "qwen3-4b-instruct-2507-q4km",
            Variant::Lite => "qwen3-1.7b-q4km",
        }
    }
    /// Maximum context window we ever allocate.
    pub fn context_tokens(self) -> u32 {
        match self {
            Variant::Standard => 32_768,
            Variant::Lite => 24_576,
        }
    }
    /// Budget for the transcript part of a prompt (leaves room for instructions + output).
    pub fn input_budget_tokens(self) -> usize {
        match self {
            Variant::Standard => 24_000,
            Variant::Lite => 16_000,
        }
    }
    /// Whether the model must be told not to think (Qwen3 hybrid models).
    pub fn disable_thinking(self) -> bool {
        matches!(self, Variant::Lite)
    }
}

fn backend() -> Result<&'static LlamaBackend> {
    static BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default());
            LlamaBackend::init().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| Error::Engine(format!("llama.cpp backend: {e}")))
}

fn engine<E: std::fmt::Display>(what: &str) -> impl FnOnce(E) -> Error + '_ {
    move |e| Error::Engine(format!("llama.cpp: {what}: {e}"))
}

fn n_threads() -> i32 {
    std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4) as i32
}

/// Qwen3 chat (ChatML) prompt for one system + one user turn, ending with an open
/// assistant turn. With `no_think`, uses Qwen3's documented switches: "/no_think" in the user
/// turn and an empty, already-closed think block (what the official template emits for
/// `enable_thinking=False`).
pub fn chatml(system: &str, user: &str, no_think: bool) -> String {
    let mut s = String::with_capacity(system.len() + user.len() + 128);
    if !system.trim().is_empty() {
        s.push_str("<|im_start|>system\n");
        s.push_str(system.trim());
        s.push_str("<|im_end|>\n");
    }
    s.push_str("<|im_start|>user\n");
    s.push_str(user.trim());
    if no_think {
        s.push_str(" /no_think");
    }
    s.push_str("<|im_end|>\n<|im_start|>assistant\n");
    if no_think {
        s.push_str("<think>\n\n</think>\n\n");
    }
    s
}

/// Turns the raw generated text into the user-visible text: drops a leading
/// `<think>…</think>` block and leading whitespace. While generation is still running
/// (`done == false`), text that might still turn into a think block is held back, so the
/// visible text only ever grows by appending (what is streamed is never retracted).
pub fn visible_text(raw: &str, done: bool) -> &str {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let t = raw.trim_start();
    if let Some(rest) = t.strip_prefix(OPEN) {
        return match rest.find(CLOSE) {
            Some(i) => rest[i + CLOSE.len()..].trim_start(),
            // Unterminated think block: nothing visible (yet).
            None => "",
        };
    }
    if !done && OPEN.starts_with(t) {
        return ""; // "", "<", "<th", ... could still become "<think>"
    }
    t
}

/// Incremental UTF-8 decoder: bytes of one character can be split across tokens.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8], out: &mut String) {
        self.pending.extend_from_slice(bytes);
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    return;
                }
                Err(e) => {
                    let ok = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..ok]).expect("valid prefix"));
                    match e.error_len() {
                        // Incomplete sequence at the end: wait for more bytes.
                        None => {
                            self.pending.drain(..ok);
                            return;
                        }
                        // Invalid bytes: replace and continue.
                        Some(n) => {
                            out.push(char::REPLACEMENT_CHARACTER);
                            self.pending.drain(..ok + n);
                        }
                    }
                }
            }
        }
    }
    fn finish(&mut self, out: &mut String) {
        if !self.pending.is_empty() {
            out.push_str(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
    }
}

/// Statistics of the most recent `generate` call.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GenerationStats {
    pub prompt_tokens: usize,
    pub generated_tokens: usize,
    pub prompt_secs: f64,
    pub generate_secs: f64,
    pub context_tokens: u32,
}

impl GenerationStats {
    pub fn prompt_tokens_per_sec(&self) -> f64 {
        self.prompt_tokens as f64 / self.prompt_secs.max(1e-9)
    }
    pub fn tokens_per_sec(&self) -> f64 {
        self.generated_tokens as f64 / self.generate_secs.max(1e-9)
    }
}

/// Qwen3 notes model on llama.cpp.
pub struct LlamaNoteWriter {
    model: LlamaModel,
    variant: Variant,
    last_stats: GenerationStats,
}

impl LlamaNoteWriter {
    /// Loads the GGUF at `paths["gguf"]`.
    pub fn load(paths: &ModelPaths, variant: Variant) -> Result<Self> {
        let path = paths.get("gguf")?;
        if !path.exists() {
            return Err(Error::NotFound(format!("model file 'gguf' at {}", path.display())));
        }
        let backend = backend()?;
        // Default params offload all layers to the GPU where one exists (Metal) and mmap the file.
        let params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(backend, path, &params).map_err(engine("load model"))?;
        Ok(LlamaNoteWriter { model, variant, last_stats: GenerationStats::default() })
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    /// Timing of the last `generate` call (for logs / benchmarks).
    pub fn last_stats(&self) -> GenerationStats {
        self.last_stats
    }

    fn tokenize(&self, text: &str, special: bool) -> Vec<LlamaToken> {
        self.model
            .str_to_token(text, if special { AddBos::Never } else { AddBos::Never })
            .unwrap_or_default()
    }

    fn sampler(&self, temperature: f32) -> LlamaSampler {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(42);
        let penalties = LlamaSampler::penalties(self.model.n_vocab(), 256, 1.05, 0.0, 0.0);
        if temperature <= 0.0 {
            return LlamaSampler::chain_simple([penalties, LlamaSampler::greedy()]);
        }
        // Qwen3 recommended non-thinking settings: top_p 0.8, top_k 20, min_p 0.
        LlamaSampler::chain_simple([
            penalties,
            LlamaSampler::top_k(20),
            LlamaSampler::top_p(0.8, 1),
            LlamaSampler::temp(temperature),
            LlamaSampler::dist(seed),
        ])
    }
}

impl NoteWriter for LlamaNoteWriter {
    fn id(&self) -> EngineId {
        self.variant.engine_id()
    }

    fn count_tokens(&mut self, text: &str) -> usize {
        self.tokenize(text, false).len()
    }

    fn input_budget_tokens(&self) -> usize {
        self.variant.input_budget_tokens()
    }

    fn generate(
        &mut self,
        prompt: &Prompt,
        on_delta: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<String> {
        cancel.check()?;
        let backend = backend()?;
        let text = chatml(&prompt.system, &prompt.user, self.variant.disable_thinking());
        // The template's special tokens (<|im_start|> …) must be parsed as such.
        let tokens = self.model.str_to_token(&text, AddBos::Never).map_err(engine("tokenize"))?;
        let max_ctx = self.variant.context_tokens().min(self.model.n_ctx_train().max(512));
        if tokens.len() + 16 >= max_ctx as usize {
            return Err(Error::Engine(format!(
                "prompt is {} tokens, context is {max_ctx}",
                tokens.len()
            )));
        }
        let max_new = (prompt.max_tokens.max(1) as usize).min(max_ctx as usize - tokens.len() - 8);
        // Allocate only the context this request needs (KV memory scales with it).
        let n_ctx = ((tokens.len() + max_new + 8).div_ceil(256) * 256).min(max_ctx as usize) as u32;
        let n_batch = n_ctx.min(2048);
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(n_ctx))
            .with_n_batch(n_batch)
            .with_n_ubatch(n_batch.min(512))
            .with_n_threads(n_threads())
            .with_n_threads_batch(n_threads())
            .with_flash_attention_policy(llama_cpp_2::context::params::LLAMA_FLASH_ATTN_TYPE_AUTO)
            .with_type_k(KvCacheType::Q8_0)
            .with_type_v(KvCacheType::Q8_0);
        let mut ctx = self.model.new_context(backend, ctx_params).map_err(engine("create context"))?;

        // Prompt (prefill) in batches; logits only for the very last token.
        let t0 = Instant::now();
        let mut batch = LlamaBatch::new(n_batch as usize, 1);
        let last = tokens.len() - 1;
        for (chunk_idx, chunk) in tokens.chunks(n_batch as usize).enumerate() {
            cancel.check()?;
            batch.clear();
            for (j, tok) in chunk.iter().enumerate() {
                let pos = chunk_idx * n_batch as usize + j;
                batch.add(*tok, pos as i32, &[0], pos == last).map_err(engine("batch"))?;
            }
            ctx.decode(&mut batch).map_err(engine("decode prompt"))?;
        }
        let prompt_secs = t0.elapsed().as_secs_f64();

        let mut sampler = self.sampler(prompt.temperature);
        sampler.accept_many(tokens.iter());
        let vocab = self.model.vocab();
        let mut raw = String::new();
        let mut utf8 = Utf8Stream::default();
        let mut emitted = 0usize;
        let mut piece = Vec::with_capacity(32);
        let mut pos = tokens.len() as i32;
        let mut generated = 0usize;
        let t1 = Instant::now();
        while generated < max_new {
            cancel.check()?;
            let tok = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(tok);
            generated += 1;
            if vocab.is_eog(tok) {
                break;
            }
            piece.clear();
            // special = false: control tokens render as nothing.
            vocab.token_to_piece_into(tok, &mut piece, false, None);
            utf8.push(&piece, &mut raw);
            let vis = visible_text(&raw, false);
            if vis.len() > emitted {
                on_delta(&vis[emitted..]);
                emitted = vis.len();
            }
            batch.clear();
            batch.add(tok, pos, &[0], true).map_err(engine("batch"))?;
            pos += 1;
            ctx.decode(&mut batch).map_err(engine("decode"))?;
        }
        utf8.finish(&mut raw);
        let vis = visible_text(&raw, true);
        // Text held back while it could have become "<think>" (or a trailing partial char).
        if vis.len() > emitted && vis.starts_with(&visible_text(&raw, false)[..emitted]) {
            on_delta(&vis[emitted..]);
        }
        self.last_stats = GenerationStats {
            prompt_tokens: tokens.len(),
            generated_tokens: generated,
            prompt_secs,
            generate_secs: t1.elapsed().as_secs_f64(),
            context_tokens: n_ctx,
        };
        tracing::debug!(stats = ?self.last_stats, "notes generated");
        Ok(vis.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatml_layout() {
        assert_eq!(
            chatml("sys", "hi", false),
            "<|im_start|>system\nsys<|im_end|>\n<|im_start|>user\nhi<|im_end|>\n<|im_start|>assistant\n"
        );
        let p = chatml("", "hi", true);
        assert_eq!(p, "<|im_start|>user\nhi /no_think<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n");
    }

    #[test]
    fn visible_text_strips_think() {
        assert_eq!(visible_text("<think>\nhmm\n</think>\n\n## A", true), "## A");
        assert_eq!(visible_text("<think>\nhmm", false), "");
        assert_eq!(visible_text("<thi", false), "");
        assert_eq!(visible_text("<thi", true), "<thi");
        assert_eq!(visible_text("\n\n- [ ] milk", false), "- [ ] milk");
        assert_eq!(visible_text("<b>x</b>", false), "<b>x</b>");
    }

    #[test]
    fn visible_text_only_grows() {
        let raw = "\n<think>\nplan\n</think>\n\n## Summary\nDone. <b>ok</b>";
        let mut prev = String::new();
        for end in 0..=raw.len() {
            if !raw.is_char_boundary(end) {
                continue;
            }
            let v = visible_text(&raw[..end], false);
            assert!(v.starts_with(prev.as_str()), "{prev:?} -> {v:?}");
            prev = v.to_string();
        }
        assert_eq!(visible_text(raw, true), prev);
    }

    #[test]
    fn utf8_split_across_tokens() {
        let bytes = "héllo 🎉".as_bytes();
        let mut s = Utf8Stream::default();
        let mut out = String::new();
        for b in bytes {
            s.push(std::slice::from_ref(b), &mut out);
        }
        s.finish(&mut out);
        assert_eq!(out, "héllo 🎉");
        let mut out = String::new();
        s.push(&[b'a', 0xff, b'b'], &mut out);
        assert_eq!(out, "a\u{fffd}b");
    }

    #[test]
    fn variants() {
        assert_eq!(Variant::Standard.engine_id(), "qwen3-4b-instruct-2507-q4km");
        assert_eq!(Variant::Lite.engine_id(), "qwen3-1.7b-q4km");
        assert!(Variant::Lite.disable_thinking() && !Variant::Standard.disable_thinking());
        assert_eq!(Variant::Lite.context_tokens(), 24_576);
    }
}

//! `Lazy<T>`: a decorator that loads an engine on first use and drops it on
//! `unload()` or after an idle timeout. Keeps memory policy out of adapters.

use crate::cancel::CancelToken;
use crate::domain::{Segment, SpeakerTurn};
use crate::error::Result;
use crate::ports::*;
use std::time::{Duration, Instant};

pub type Loader<T> = Box<dyn FnMut() -> Result<T> + Send>;

pub struct Lazy<T> {
    id: EngineId,
    loader: Loader<T>,
    value: Option<T>,
    last_used: Instant,
    idle_timeout: Option<Duration>,
}

impl<T> Lazy<T> {
    pub fn new(id: EngineId, loader: impl FnMut() -> Result<T> + Send + 'static) -> Self {
        Lazy {
            id,
            loader: Box::new(loader),
            value: None,
            last_used: Instant::now(),
            idle_timeout: None,
        }
    }

    /// Drop the engine if it has not been used for `timeout` (checked on `tick`).
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = Some(timeout);
        self
    }

    pub fn is_loaded(&self) -> bool {
        self.value.is_some()
    }

    pub fn get(&mut self) -> Result<&mut T> {
        self.last_used = Instant::now();
        if self.value.is_none() {
            tracing::info!(engine = self.id, "loading engine");
            self.value = Some((self.loader)()?);
        }
        Ok(self.value.as_mut().expect("just loaded"))
    }

    /// Ensure the engine is loaded now (e.g. "keep dictation model warm").
    pub fn preload(&mut self) -> Result<()> {
        self.get().map(|_| ())
    }

    pub fn drop_value(&mut self) {
        if self.value.take().is_some() {
            tracing::info!(engine = self.id, "unloaded engine");
        }
    }

    /// Unload if idle longer than the timeout. Call periodically.
    pub fn tick(&mut self) {
        if let Some(t) = self.idle_timeout {
            if self.value.is_some() && self.last_used.elapsed() >= t {
                self.drop_value();
            }
        }
    }
}

impl<T: Transcriber> Transcriber for Lazy<T> {
    fn id(&self) -> EngineId {
        self.id
    }
    fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>> {
        self.get()?.transcribe(audio)
    }
    fn preload(&mut self) -> Result<()> {
        self.get()?.preload()
    }
    fn unload(&mut self) {
        self.drop_value()
    }
    fn maintain(&mut self) {
        self.tick()
    }
}

impl<T: Diarizer> Diarizer for Lazy<T> {
    fn id(&self) -> EngineId {
        self.id
    }
    fn diarize(&mut self, audio: &[f32]) -> Result<Vec<SpeakerTurn>> {
        self.get()?.diarize(audio)
    }
    fn unload(&mut self) {
        self.drop_value()
    }
    fn maintain(&mut self) {
        self.tick()
    }
}

impl<T: NoteWriter> NoteWriter for Lazy<T> {
    fn id(&self) -> EngineId {
        self.id
    }
    fn count_tokens(&mut self, text: &str) -> usize {
        match self.get() {
            Ok(v) => v.count_tokens(text),
            Err(_) => text.len() / 4,
        }
    }
    fn input_budget_tokens(&self) -> usize {
        self.value
            .as_ref()
            .map(|v| v.input_budget_tokens())
            .unwrap_or(16_000)
    }
    fn generate(
        &mut self,
        prompt: &Prompt,
        on_delta: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<String> {
        self.get()?.generate(prompt, on_delta, cancel)
    }
    fn unload(&mut self) {
        self.drop_value()
    }
    fn maintain(&mut self) {
        self.tick()
    }
}

/// `Fallback<A, B>`: try A, and on error try B. Used for text injection
/// (paste first, keystroke typing second).
pub struct Fallback<A, B> {
    pub primary: A,
    pub secondary: B,
}

impl<A: TextInjector, B: TextInjector> TextInjector for Fallback<A, B> {
    fn id(&self) -> EngineId {
        self.primary.id()
    }
    fn inject(&self, text: &str) -> Result<()> {
        match self.primary.inject(text) {
            Ok(()) => Ok(()),
            Err(e) => {
                tracing::warn!(error = %e, "primary injector failed; falling back");
                self.secondary.inject(text)
            }
        }
    }
}

//! Hotkey → record → transcribe → paste.

use super::SharedCtx;
use crate::bus::{DictationPhase, Event};
use crate::capture::{Capture, FrameProcessor};
use crate::domain::*;
use crate::dsp;
use crate::error::{Error, Result};
use crate::ports::KeyState;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Holding the key longer than this = push-to-talk; release stops recording.
const HOLD_THRESHOLD: Duration = Duration::from_millis(350);
/// Utterances shorter than this (after trimming) are ignored.
const MIN_SPEECH_MS: u64 = 250;
const SILENCE_RMS: f32 = 0.006;

struct Recorder {
    buf: Vec<f32>,
    events: crate::bus::EventBus,
    last_level: Instant,
}

impl FrameProcessor for Recorder {
    fn process(&mut self, frames: &[f32]) {
        self.buf.extend_from_slice(frames);
        if self.last_level.elapsed() >= Duration::from_millis(33) {
            self.last_level = Instant::now();
            self.events.publish(Event::DictationLevel(dsp::meter_level(frames)));
        }
    }
}

enum State {
    Idle,
    Listening { capture: Capture<Recorder>, pressed_at: Instant, started: Instant },
    Transcribing,
}

pub struct DictationService {
    ctx: SharedCtx,
    state: Arc<Mutex<State>>,
}

impl DictationService {
    pub fn new(ctx: SharedCtx) -> Self {
        DictationService { ctx, state: Arc::new(Mutex::new(State::Idle)) }
    }

    pub fn is_idle(&self) -> bool {
        matches!(*self.state.lock(), State::Idle)
    }

    /// Single shortcut: tap toggles, hold = push-to-talk.
    pub fn on_hotkey(&self, key: KeyState) {
        let action = {
            let st = self.state.lock();
            match (&*st, key) {
                (State::Idle, KeyState::Pressed) => Some(true),
                (State::Listening { .. }, KeyState::Pressed) => Some(false),
                (State::Listening { pressed_at, .. }, KeyState::Released)
                    if pressed_at.elapsed() >= HOLD_THRESHOLD =>
                {
                    Some(false)
                }
                _ => None,
            }
        };
        match action {
            Some(true) => self.start(),
            Some(false) => self.stop(),
            None => {}
        }
    }

    pub fn start(&self) {
        let mut st = self.state.lock();
        if !matches!(*st, State::Idle) {
            return;
        }
        let settings = self.ctx.ports.settings.load();
        let res = (|| -> Result<Capture<Recorder>> {
            let src = self.ctx.ports.audio.microphone(settings.microphone.as_deref())?;
            Capture::start(
                src,
                Recorder { buf: Vec::with_capacity(16_000 * 30), events: self.ctx.events.clone(), last_level: Instant::now() },
            )
        })();
        match res {
            Ok(capture) => {
                *st = State::Listening { capture, pressed_at: Instant::now(), started: Instant::now() };
                self.ctx.events.publish(Event::Dictation(DictationPhase::Listening));
                // Warm the model while the user speaks.
                let ctx = self.ctx.clone();
                std::thread::spawn(move || {
                    let t = ctx.engines.transcriber.current();
                    let mut t = t.lock();
                    let _ = t.preload();
                });
            }
            Err(e) => self.fail(e),
        }
    }

    pub fn cancel(&self) {
        let mut st = self.state.lock();
        if let State::Listening { .. } = &*st {
            if let State::Listening { capture, .. } = std::mem::replace(&mut *st, State::Idle) {
                capture.stop();
            }
            self.ctx.events.publish(Event::Dictation(DictationPhase::Idle));
        }
    }

    pub fn stop(&self) {
        let (capture, started) = {
            let mut st = self.state.lock();
            match std::mem::replace(&mut *st, State::Transcribing) {
                State::Listening { capture, started, .. } => (capture, started),
                other => {
                    *st = other;
                    return;
                }
            }
        };
        self.ctx.events.publish(Event::Dictation(DictationPhase::Transcribing));
        let ctx = self.ctx.clone();
        let state = self.state.clone();
        std::thread::spawn(move || {
            let rec = capture.stop();
            let result = transcribe_and_inject(&ctx, &rec.buf, started.elapsed());
            *state.lock() = State::Idle;
            match result {
                Ok(Some(text)) => {
                    ctx.events.publish(Event::Dictated(text));
                    ctx.events.publish(Event::Dictation(DictationPhase::Idle));
                }
                Ok(None) => ctx.events.publish(Event::Dictation(DictationPhase::Idle)),
                Err(e) => ctx.events.publish(Event::Dictation(DictationPhase::Error(e.to_string()))),
            }
        });
    }

    fn fail(&self, e: Error) {
        tracing::warn!(error = %e, "dictation failed to start");
        self.ctx.events.publish(Event::Dictation(DictationPhase::Error(e.to_string())));
    }
}

fn transcribe_and_inject(ctx: &SharedCtx, audio: &[f32], elapsed: Duration) -> Result<Option<String>> {
    let (speech, _) = dsp::trim_silence(audio, SILENCE_RMS);
    if samples_to_ms(speech.len()) < MIN_SPEECH_MS {
        return Ok(None);
    }
    let segments = {
        let t = ctx.engines.transcriber.current();
        let mut t = t.lock();
        t.transcribe(speech)?
    };
    let text = clean_join(&segments);
    if text.is_empty() {
        return Ok(None);
    }
    ctx.ports.injector.inject(&text)?;
    if ctx.ports.settings.load().keep_dictation_history {
        let d = Dictation { at: ctx.ports.clock.now(), text: text.clone(), duration_ms: elapsed.as_millis() as u64 };
        if let Err(e) = ctx.ports.repo.append_dictation(&d) {
            tracing::warn!(error = %e, "could not save dictation history");
        }
    }
    Ok(Some(text))
}

/// Join segment texts, dropping non-speech markers.
pub fn clean_join(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|s| s.text.trim())
        .filter(|t| !t.is_empty() && !(t.starts_with('[') && t.ends_with(']')))
        .collect::<Vec<_>>()
        .join(" ")
}

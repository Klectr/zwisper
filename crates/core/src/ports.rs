//! Ports: the traits every strategy (adapter) implements.
//!
//! Rules:
//! * Synchronous, `Send`, object-safe. Threading is the caller's concern.
//! * Inputs/outputs are `crate::domain` types only.
//! * Errors are `crate::Error` only.

use crate::domain::*;
use crate::error::Result;
use crate::cancel::CancelToken;
use std::path::PathBuf;

/// Identifies a concrete strategy in logs and session metadata.
pub type EngineId = &'static str;

// ───────────────────────────── audio ─────────────────────────────

/// Called with 16 kHz mono f32 frames. May be invoked on a real-time thread:
/// implementations must not block.
pub type FrameSink = Box<dyn FnMut(&[f32]) + Send>;

/// A live audio stream (microphone or system output).
pub trait AudioSource: Send {
    fn kind(&self) -> TrackKind;
    /// Start delivering frames to `sink`. Adapters convert to 16 kHz mono.
    fn start(&mut self, sink: FrameSink) -> Result<()>;
    fn stop(&mut self);
}

/// Creates audio sources. One strategy per OS (cpal + CoreAudio tap on macOS, ...).
pub trait AudioSourceFactory: Send + Sync {
    fn microphone(&self, device: Option<&str>) -> Result<Box<dyn AudioSource>>;
    /// `Ok(None)` when the platform cannot capture system audio.
    fn system_output(&self) -> Result<Option<Box<dyn AudioSource>>>;
    fn list_microphones(&self) -> Vec<String>;
}

/// Splits a continuous stream into speech chunks.
pub trait VoiceActivity: Send {
    fn id(&self) -> EngineId;
    /// Feed frames; returns any chunks that just completed.
    fn accept(&mut self, frames: &[f32]) -> Result<Vec<SpeechChunk>>;
    /// End of stream: return whatever speech is still buffered.
    fn flush(&mut self) -> Result<Vec<SpeechChunk>>;
    fn reset(&mut self);
}

/// Persists a track while recording.
pub trait TrackWriter: Send {
    fn write(&mut self, frames: &[f32]) -> Result<()>;
    fn finish(self: Box<Self>) -> Result<()>;
}

/// Stores and reads back recorded meeting audio.
pub trait AudioRecorder: Send + Sync {
    fn create(&self, session: SessionId, track: TrackKind) -> Result<Box<dyn TrackWriter>>;
    fn read(&self, session: SessionId, track: TrackKind) -> Result<Vec<f32>>;
}

// ───────────────────────────── AI engines ─────────────────────────────

/// Speech → text with word timestamps (relative to the start of `audio`).
pub trait Transcriber: Send {
    fn id(&self) -> EngineId;
    fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>>;
    /// Load now so the next call is fast (no-op if already loaded).
    fn preload(&mut self) -> Result<()> {
        Ok(())
    }
    /// Release memory. Called between pipeline stages.
    fn unload(&mut self) {}
    /// Periodic housekeeping (e.g. idle unload). Called about once a second.
    fn maintain(&mut self) {}
}

/// Audio → who spoke when.
pub trait Diarizer: Send {
    fn id(&self) -> EngineId;
    fn diarize(&mut self, audio: &[f32]) -> Result<Vec<SpeakerTurn>>;
    fn unload(&mut self) {}
    fn maintain(&mut self) {}
}

/// A fully rendered prompt for the notes LLM.
#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// Prompt → streamed Markdown.
pub trait NoteWriter: Send {
    fn id(&self) -> EngineId;
    /// Rough token count, used to decide whether to chunk long meetings.
    fn count_tokens(&mut self, text: &str) -> usize {
        text.len() / 4
    }
    /// Context budget for the transcript part of a prompt.
    fn input_budget_tokens(&self) -> usize {
        16_000
    }
    fn generate(
        &mut self,
        prompt: &Prompt,
        on_delta: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<String>;
    /// Load the model now, so the caller can report that step separately.
    fn preload(&mut self) -> Result<()> {
        Ok(())
    }
    fn unload(&mut self) {}
    fn maintain(&mut self) {}
}

/// Input for building a notes prompt.
#[derive(Debug, Clone)]
pub struct NoteInput {
    pub title: String,
    pub date: String,
    /// `[mm:ss] Name: text` lines.
    pub transcript: String,
    pub my_notes: String,
    pub speakers: Vec<String>,
}

/// The curated template set: prompts + output validation.
pub trait TemplateCatalog: Send + Sync {
    fn prompt(&self, template: Template, input: &NoteInput) -> Prompt;
    /// Prompt that condenses one chunk of a very long meeting.
    fn chunk_prompt(&self, input: &NoteInput, part: usize, of: usize) -> Prompt;
    /// `Err(reason)` when the output is missing required structure.
    fn validate(&self, template: Template, markdown: &str) -> std::result::Result<(), String>;
}

/// Words + turns → attributed paragraphs.
pub trait SpeakerAligner: Send + Sync {
    /// `segments` are raw transcript segments of one track (track-relative times),
    /// `turns` the diarizer output for the same track. Returns paragraphs with
    /// `SpeakerId(turn.speaker + first_id)`.
    fn align(
        &self,
        track: TrackKind,
        segments: &[TranscriptSegment],
        turns: &[SpeakerTurn],
        first_id: u32,
    ) -> Vec<TranscriptSegment>;
}

/// Removes mic text that is really remote audio leaking from speakers.
pub trait EchoFilter: Send + Sync {
    fn filter(
        &self,
        mic: Vec<TranscriptSegment>,
        system: &[TranscriptSegment],
    ) -> Vec<TranscriptSegment>;
}

// ───────────────────────────── OS integration ─────────────────────────────

/// Puts text into whatever app has focus.
pub trait TextInjector: Send + Sync {
    fn id(&self) -> EngineId;
    fn inject(&self, text: &str) -> Result<()>;
    /// Put `text` on the clipboard without pasting. The fallback when `inject`
    /// fails (e.g. Accessibility was revoked), so a dictation is never lost.
    fn copy(&self, _text: &str) -> Result<()> {
        Err(crate::Error::InvalidState("this injector can't use the clipboard".into()))
    }
}

/// Quiets other apps' audio while dictating.
///
/// Best effort and non-blocking: calls return immediately, failures are
/// logged by the adapter and never interrupt dictation. `restore` after
/// `duck` must always bring the sound back, and is a no-op otherwise.
pub trait AudioDucker: Send + Sync {
    fn duck(&self, how: Ducking);
    fn restore(&self);
}

/// For platforms (and demos) without ducking.
pub struct NoDucking;
impl AudioDucker for NoDucking {
    fn duck(&self, _how: Ducking) {}
    fn restore(&self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyAction {
    Dictate,
    Meeting,
    /// Cancel the current dictation. Only bound while dictating (it's Esc).
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    Pressed,
    Released,
}

pub type HotkeyCallback = Box<dyn Fn(HotkeyAction, KeyState) + Send + Sync>;

/// Global keyboard shortcuts.
pub trait HotkeyService: Send {
    /// Replace all bindings. `chord` is e.g. "Alt+Space".
    fn bind(&mut self, bindings: &[(HotkeyAction, String)], callback: HotkeyCallback) -> Result<()>;
    fn unbind_all(&mut self);
    /// Add (`Some(chord)`) or remove (`None`) one extra binding for `action`
    /// without touching the others — e.g. Esc only while dictating, so it
    /// isn't taken from other apps the rest of the time. Uses the callback
    /// from `bind`.
    fn set_temporary(&mut self, _action: HotkeyAction, _chord: Option<&str>) -> Result<()> {
        Ok(())
    }
}

pub trait Permissions: Send + Sync {
    fn state(&self, kind: PermissionKind) -> PermissionState;
    /// Trigger the OS prompt (if any). Non-blocking.
    fn request(&self, kind: PermissionKind);
    /// Open the relevant pane in system settings.
    fn open_settings(&self, kind: PermissionKind);
}

pub trait Clock: Send + Sync {
    fn now(&self) -> chrono::DateTime<chrono::Utc>;
}

// ───────────────────────────── storage ─────────────────────────────

pub trait SessionRepository: Send + Sync {
    fn list(&self) -> Result<Vec<SessionSummary>>;
    fn load(&self, id: SessionId) -> Result<Session>;
    fn save(&self, session: &Session) -> Result<()>;
    fn delete(&self, id: SessionId) -> Result<()>;
    fn folder(&self, id: SessionId) -> Option<PathBuf>;
    fn append_dictation(&self, d: &Dictation) -> Result<()>;
    fn dictations(&self, limit: usize) -> Result<Vec<Dictation>>;
    fn clear_dictations(&self) -> Result<()>;
}

pub trait SettingsStore: Send + Sync {
    fn load(&self) -> Settings;
    fn save(&self, s: &Settings) -> Result<()>;
}

/// Files of an installed pack, by logical name (e.g. "encoder", "gguf").
#[derive(Debug, Clone, Default)]
pub struct ModelPaths {
    pub files: std::collections::BTreeMap<String, PathBuf>,
}

impl ModelPaths {
    pub fn get(&self, name: &str) -> Result<&PathBuf> {
        self.files
            .get(name)
            .ok_or_else(|| crate::Error::NotFound(format!("model file '{name}'")))
    }
}

pub type ProgressFn<'a> = &'a mut dyn FnMut(u64, u64);

/// The curated model manifest + downloads.
pub trait ModelStore: Send + Sync {
    fn status(&self, pack: Pack, tier: Tier) -> PackStatus;
    fn paths(&self, pack: Pack, tier: Tier) -> Result<ModelPaths>;
    fn download(&self, pack: Pack, tier: Tier, progress: ProgressFn, cancel: &CancelToken) -> Result<()>;
    fn remove(&self, pack: Pack, tier: Tier) -> Result<()>;
    /// Credits for About → Acknowledgements.
    fn acknowledgements(&self) -> Vec<String>;
}

// ───────────────────────────── composition ─────────────────────────────

/// Builds concrete engines. Implemented by the composition root (`app`), so core
/// can (re)create engines — e.g. after a tier switch — without naming adapters.
pub trait EngineFactory: Send + Sync {
    fn transcriber(&self, tier: Tier) -> Result<Box<dyn Transcriber>>;
    fn vad(&self) -> Result<Box<dyn VoiceActivity>>;
    fn diarizer(&self, tier: Tier) -> Result<Box<dyn Diarizer>>;
    fn note_writer(&self, tier: Tier) -> Result<Box<dyn NoteWriter>>;
}

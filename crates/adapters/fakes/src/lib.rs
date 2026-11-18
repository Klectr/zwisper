//! Test doubles for every port. Deterministic, dependency-free and fast.

use chrono::{DateTime, TimeZone, Utc};
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use zwisper_core::domain::*;
use zwisper_core::dsp;
use zwisper_core::ports::*;
use zwisper_core::{CancelToken, Error, Result};

// ───────────────────────────── audio ─────────────────────────────

/// Plays a fixed buffer as 10 ms frames, then goes quiet.
pub struct VecSource {
    kind: TrackKind,
    samples: Arc<Vec<f32>>,
    realtime: bool,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl VecSource {
    pub fn new(kind: TrackKind, samples: Vec<f32>, realtime: bool) -> Self {
        VecSource { kind, samples: Arc::new(samples), realtime, stop: Arc::default(), thread: None }
    }
}

impl AudioSource for VecSource {
    fn kind(&self) -> TrackKind {
        self.kind
    }
    fn start(&mut self, mut sink: FrameSink) -> Result<()> {
        let samples = self.samples.clone();
        let stop = self.stop.clone();
        let realtime = self.realtime;
        self.thread = Some(std::thread::spawn(move || {
            for f in samples.chunks(160) {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                sink(f);
                if realtime {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }));
        Ok(())
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Audio factory serving canned buffers. Waits for playback to finish before
/// `stop()` returns, so tests are deterministic.
#[derive(Default, Clone)]
pub struct FakeAudio {
    pub mic: Arc<Mutex<Vec<f32>>>,
    pub system: Arc<Mutex<Option<Vec<f32>>>>,
    pub fail_mic: bool,
}

impl FakeAudio {
    pub fn new(mic: Vec<f32>, system: Option<Vec<f32>>) -> Self {
        FakeAudio { mic: Arc::new(Mutex::new(mic)), system: Arc::new(Mutex::new(system)), fail_mic: false }
    }
}

/// Wraps a source and only stops it after all samples were delivered.
struct DrainingSource(VecSource, Arc<AtomicBool>);

impl AudioSource for DrainingSource {
    fn kind(&self) -> TrackKind {
        self.0.kind()
    }
    fn start(&mut self, mut sink: FrameSink) -> Result<()> {
        let done = self.1.clone();
        let total = self.0.samples.len();
        let mut seen = 0usize;
        self.0.start(Box::new(move |f| {
            sink(f);
            seen += f.len();
            if seen >= total {
                done.store(true, Ordering::SeqCst);
            }
        }))
    }
    fn stop(&mut self) {
        if !self.0.samples.is_empty() {
            while !self.1.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.0.stop()
    }
}

impl AudioSourceFactory for FakeAudio {
    fn microphone(&self, _device: Option<&str>) -> Result<Box<dyn AudioSource>> {
        if self.fail_mic {
            return Err(Error::Permission(PermissionKind::Microphone));
        }
        Ok(Box::new(DrainingSource(
            VecSource::new(TrackKind::Mic, self.mic.lock().clone(), false),
            Arc::default(),
        )))
    }
    fn system_output(&self) -> Result<Option<Box<dyn AudioSource>>> {
        Ok(self.system.lock().clone().map(|s| {
            Box::new(DrainingSource(VecSource::new(TrackKind::System, s, false), Arc::default()))
                as Box<dyn AudioSource>
        }))
    }
    fn list_microphones(&self) -> Vec<String> {
        vec!["Fake Mic".into()]
    }
}

/// Build test audio: `(amplitude, ms)` pieces of constant "speech" or silence.
pub fn tone(pieces: &[(f32, u64)]) -> Vec<f32> {
    let mut v = Vec::new();
    for &(amp, ms) in pieces {
        let n = ms_to_samples(ms);
        // Alternate sign so the mean is 0 but RMS = amp.
        v.extend((0..n).map(|i| if i % 2 == 0 { amp } else { -amp }));
    }
    v
}

#[derive(Default)]
pub struct MemRecorder {
    tracks: Mutex<HashMap<(SessionId, TrackKind), Arc<Mutex<Vec<f32>>>>>,
}

struct MemWriter(Arc<Mutex<Vec<f32>>>);
impl TrackWriter for MemWriter {
    fn write(&mut self, frames: &[f32]) -> Result<()> {
        self.0.lock().extend_from_slice(frames);
        Ok(())
    }
    fn finish(self: Box<Self>) -> Result<()> {
        Ok(())
    }
}

impl AudioRecorder for MemRecorder {
    fn create(&self, session: SessionId, track: TrackKind) -> Result<Box<dyn TrackWriter>> {
        let buf = Arc::new(Mutex::new(Vec::new()));
        self.tracks.lock().insert((session, track), buf.clone());
        Ok(Box::new(MemWriter(buf)))
    }
    fn read(&self, session: SessionId, track: TrackKind) -> Result<Vec<f32>> {
        self.tracks
            .lock()
            .get(&(session, track))
            .map(|b| b.lock().clone())
            .ok_or_else(|| Error::NotFound("track".into()))
    }
}

// ───────────────────────────── engines ─────────────────────────────

/// Turns audio into text deterministically: each loud 100 ms block becomes a
/// word chosen by its amplitude (`vocab`), with exact word timestamps.
pub struct ScriptedTranscriber {
    pub vocab: Vec<(f32, String)>,
    pub calls: Arc<AtomicUsize>,
    pub loaded: Arc<AtomicBool>,
}

impl ScriptedTranscriber {
    pub fn new(vocab: &[(f32, &str)]) -> Self {
        ScriptedTranscriber {
            vocab: vocab.iter().map(|(a, w)| (*a, w.to_string())).collect(),
            calls: Arc::default(),
            loaded: Arc::default(),
        }
    }
}

impl Default for ScriptedTranscriber {
    fn default() -> Self {
        Self::new(&[(0.1, "hello"), (0.3, "world")])
    }
}

impl Transcriber for ScriptedTranscriber {
    fn id(&self) -> EngineId {
        "scripted-transcriber"
    }
    fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>> {
        self.loaded.store(true, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let block = ms_to_samples(100);
        let mut words = Vec::new();
        for (i, b) in audio.chunks(block).enumerate() {
            let r = dsp::rms(b);
            if r < 0.02 {
                continue;
            }
            let word = self
                .vocab
                .iter()
                .min_by(|x, y| (x.0 - r).abs().total_cmp(&(y.0 - r).abs()))
                .map(|x| x.1.clone())
                .unwrap_or_else(|| "speech".into());
            // Collapse repeated identical words from consecutive blocks.
            let start = i as u64 * 100;
            match words.last_mut() {
                Some(Word { text, end_ms, .. }) if *text == word && *end_ms == start => *end_ms = start + 100,
                _ => words.push(Word { text: word, start_ms: start, end_ms: start + 100 }),
            }
        }
        if words.is_empty() {
            return Ok(vec![]);
        }
        Ok(vec![Segment {
            start_ms: words[0].start_ms,
            end_ms: words.last().unwrap().end_ms,
            text: words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "),
            words,
        }])
    }
    fn preload(&mut self) -> Result<()> {
        self.loaded.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn unload(&mut self) {
        self.loaded.store(false, Ordering::SeqCst);
    }
}

/// Labels speakers by amplitude band: each distinct vocab amplitude is a voice.
pub struct ScriptedDiarizer {
    pub voices: Vec<f32>,
    pub fail: bool,
}

impl Default for ScriptedDiarizer {
    fn default() -> Self {
        ScriptedDiarizer { voices: vec![0.1, 0.3], fail: false }
    }
}

impl Diarizer for ScriptedDiarizer {
    fn id(&self) -> EngineId {
        "scripted-diarizer"
    }
    fn diarize(&mut self, audio: &[f32]) -> Result<Vec<SpeakerTurn>> {
        if self.fail {
            return Err(Error::Engine("diarizer exploded".into()));
        }
        let block = ms_to_samples(100);
        let mut turns: Vec<SpeakerTurn> = Vec::new();
        for (i, b) in audio.chunks(block).enumerate() {
            let r = dsp::rms(b);
            if r < 0.02 {
                continue;
            }
            let sp = self
                .voices
                .iter()
                .enumerate()
                .min_by(|x, y| (x.1 - r).abs().total_cmp(&(y.1 - r).abs()))
                .map(|x| x.0 as u32)
                .unwrap_or(0)
                + 10; // arbitrary labels, as real diarizers produce
            let start = i as u64 * 100;
            match turns.last_mut() {
                Some(t) if t.speaker == sp && t.end_ms == start => t.end_ms = start + 100,
                _ => turns.push(SpeakerTurn { start_ms: start, end_ms: start + 100, speaker: sp }),
            }
        }
        Ok(turns)
    }
}

/// Returns canned Markdown, streamed word by word.
pub struct CannedNoteWriter {
    pub reply: String,
    pub prompts: Arc<Mutex<Vec<Prompt>>>,
    pub budget: usize,
    pub id: EngineId,
}

impl CannedNoteWriter {
    pub fn new(reply: &str) -> Self {
        CannedNoteWriter { reply: reply.into(), prompts: Arc::default(), budget: 16_000, id: "canned-notes" }
    }
}

impl Default for CannedNoteWriter {
    fn default() -> Self {
        Self::new("## Summary\nWe met.\n\n## Decisions\n- Ship it\n\n## Action items\n- [ ] You: send notes\n")
    }
}

impl NoteWriter for CannedNoteWriter {
    fn id(&self) -> EngineId {
        self.id
    }
    fn input_budget_tokens(&self) -> usize {
        self.budget
    }
    fn count_tokens(&mut self, text: &str) -> usize {
        text.split_whitespace().count()
    }
    fn generate(&mut self, prompt: &Prompt, on_delta: &mut dyn FnMut(&str), cancel: &CancelToken) -> Result<String> {
        cancel.check()?;
        self.prompts.lock().push(prompt.clone());
        let mut out = String::new();
        for (i, piece) in self.reply.split_inclusive(' ').enumerate() {
            if i % 8 == 0 {
                cancel.check()?;
            }
            on_delta(piece);
            out.push_str(piece);
        }
        Ok(out)
    }
}

// ───────────────────────────── OS ─────────────────────────────

#[derive(Default, Clone)]
pub struct RecordingInjector {
    pub injected: Arc<Mutex<Vec<String>>>,
    pub copied: Arc<Mutex<Vec<String>>>,
    pub fail: bool,
}

impl TextInjector for RecordingInjector {
    fn id(&self) -> EngineId {
        "recording-injector"
    }
    fn inject(&self, text: &str) -> Result<()> {
        if self.fail {
            return Err(Error::Permission(PermissionKind::Accessibility));
        }
        self.injected.lock().push(text.to_string());
        Ok(())
    }
    fn copy(&self, text: &str) -> Result<()> {
        self.copied.lock().push(text.to_string());
        Ok(())
    }
}

pub struct AllGranted;
impl Permissions for AllGranted {
    fn state(&self, _: PermissionKind) -> PermissionState {
        PermissionState::Granted
    }
    fn request(&self, _: PermissionKind) {}
    fn open_settings(&self, _: PermissionKind) {}
}

pub struct FixedClock(pub DateTime<Utc>);
impl Default for FixedClock {
    fn default() -> Self {
        FixedClock(Utc.with_ymd_and_hms(2026, 11, 17, 15, 30, 0).unwrap())
    }
}
impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// Manual hotkeys: tests call `fire`.
#[derive(Default)]
pub struct ManualHotkey {
    cb: Arc<Mutex<Option<HotkeyCallback>>>,
}
impl ManualHotkey {
    pub fn fire(&self, a: HotkeyAction, k: KeyState) {
        if let Some(cb) = self.cb.lock().as_ref() {
            cb(a, k)
        }
    }
}
impl HotkeyService for ManualHotkey {
    fn bind(&mut self, _: &[(HotkeyAction, String)], callback: HotkeyCallback) -> Result<()> {
        *self.cb.lock() = Some(callback);
        Ok(())
    }
    fn unbind_all(&mut self) {
        *self.cb.lock() = None;
    }
}

// ───────────────────────────── storage ─────────────────────────────

#[derive(Default)]
pub struct InMemoryRepo {
    sessions: Mutex<BTreeMap<SessionId, Session>>,
    dictations: Mutex<Vec<Dictation>>,
}

impl SessionRepository for InMemoryRepo {
    fn list(&self) -> Result<Vec<SessionSummary>> {
        Ok(self.sessions.lock().values().map(SessionSummary::from).collect())
    }
    fn load(&self, id: SessionId) -> Result<Session> {
        self.sessions.lock().get(&id).cloned().ok_or_else(|| Error::NotFound(id.to_string()))
    }
    fn save(&self, s: &Session) -> Result<()> {
        self.sessions.lock().insert(s.id, s.clone());
        Ok(())
    }
    fn delete(&self, id: SessionId) -> Result<()> {
        self.sessions.lock().remove(&id);
        Ok(())
    }
    fn folder(&self, _: SessionId) -> Option<PathBuf> {
        None
    }
    fn append_dictation(&self, d: &Dictation) -> Result<()> {
        self.dictations.lock().push(d.clone());
        Ok(())
    }
    fn dictations(&self, limit: usize) -> Result<Vec<Dictation>> {
        Ok(self.dictations.lock().iter().rev().take(limit).cloned().collect())
    }
    fn clear_dictations(&self) -> Result<()> {
        self.dictations.lock().clear();
        Ok(())
    }
}

#[derive(Default)]
pub struct MemSettings(pub Mutex<Settings>);
impl SettingsStore for MemSettings {
    fn load(&self) -> Settings {
        self.0.lock().clone()
    }
    fn save(&self, s: &Settings) -> Result<()> {
        *self.0.lock() = s.clone();
        Ok(())
    }
}

/// Every pack is installed; downloads complete instantly.
#[derive(Default)]
pub struct FakeModels {
    pub missing: Mutex<Vec<Pack>>,
}
impl ModelStore for FakeModels {
    fn status(&self, pack: Pack, _: Tier) -> PackStatus {
        if self.missing.lock().contains(&pack) {
            PackStatus::NotDownloaded { bytes: 1000 }
        } else {
            PackStatus::Ready { bytes: 1000 }
        }
    }
    fn paths(&self, pack: Pack, _: Tier) -> Result<ModelPaths> {
        if self.missing.lock().contains(&pack) {
            return Err(Error::ModelMissing(pack));
        }
        Ok(ModelPaths::default())
    }
    fn download(&self, pack: Pack, _: Tier, progress: ProgressFn, cancel: &CancelToken) -> Result<()> {
        for i in 0..=10 {
            cancel.check()?;
            progress(i * 100, 1000);
        }
        self.missing.lock().retain(|p| *p != pack);
        Ok(())
    }
    fn remove(&self, pack: Pack, _: Tier) -> Result<()> {
        self.missing.lock().push(pack);
        Ok(())
    }
    fn acknowledgements(&self) -> Vec<String> {
        vec!["Fake models".into()]
    }
}

/// Simple template catalog for core tests (the real one lives in adapters/templates).
pub struct SimpleTemplates;
impl TemplateCatalog for SimpleTemplates {
    fn prompt(&self, t: Template, i: &NoteInput) -> Prompt {
        Prompt {
            system: format!("Write {}", t.label()),
            user: format!("{}\nMY NOTES:\n{}\nTRANSCRIPT:\n{}", i.title, i.my_notes, i.transcript),
            max_tokens: 512,
            temperature: 0.7,
        }
    }
    fn chunk_prompt(&self, i: &NoteInput, part: usize, of: usize) -> Prompt {
        Prompt { system: format!("Condense part {part}/{of}"), user: i.transcript.clone(), max_tokens: 256, temperature: 0.3 }
    }
    fn validate(&self, _: Template, md: &str) -> std::result::Result<(), String> {
        if md.trim().is_empty() {
            Err("empty".into())
        } else {
            Ok(())
        }
    }
}

/// Engine factory producing the scripted engines. Records which tiers were requested.
#[derive(Default)]
pub struct FakeEngines {
    pub tiers: Mutex<Vec<Tier>>,
    pub diarizer_fails: bool,
    pub notes_reply: Option<String>,
    pub notes_budget: Option<usize>,
    pub notes_prompts: Arc<Mutex<Vec<Prompt>>>,
}

impl EngineFactory for FakeEngines {
    fn transcriber(&self, tier: Tier) -> Result<Box<dyn Transcriber>> {
        self.tiers.lock().push(tier);
        Ok(Box::new(ScriptedTranscriber::default()))
    }
    fn vad(&self) -> Result<Box<dyn VoiceActivity>> {
        Ok(Box::new(dsp::EnergyVad::new(0.02)))
    }
    fn diarizer(&self, _: Tier) -> Result<Box<dyn Diarizer>> {
        Ok(Box::new(ScriptedDiarizer { fail: self.diarizer_fails, ..Default::default() }))
    }
    fn note_writer(&self, tier: Tier) -> Result<Box<dyn NoteWriter>> {
        let mut w = match &self.notes_reply {
            Some(r) => CannedNoteWriter::new(r),
            None => CannedNoteWriter::default(),
        };
        w.prompts = self.notes_prompts.clone();
        if let Some(b) = self.notes_budget {
            w.budget = b;
        }
        w.id = match tier {
            Tier::Standard => "canned-standard",
            Tier::Lite => "canned-lite",
        };
        Ok(Box::new(w))
    }
}

/// A complete set of fake ports. Tweak fields before calling `ports()`.
pub struct FakeWorld {
    pub audio: FakeAudio,
    pub recorder: Arc<MemRecorder>,
    pub injector: RecordingInjector,
    pub repo: Arc<InMemoryRepo>,
    pub settings: Arc<MemSettings>,
    pub models: Arc<FakeModels>,
    pub engines: Arc<FakeEngines>,
}

impl Default for FakeWorld {
    fn default() -> Self {
        FakeWorld {
            audio: FakeAudio::default(),
            recorder: Arc::default(),
            injector: RecordingInjector::default(),
            repo: Arc::default(),
            settings: Arc::default(),
            models: Arc::default(),
            engines: Arc::default(),
        }
    }
}

impl FakeWorld {
    pub fn ports(&self) -> zwisper_core::Ports {
        zwisper_core::Ports {
            audio: Arc::new(self.audio.clone()),
            recorder: self.recorder.clone(),
            injector: Arc::new(self.injector.clone()),
            repo: self.repo.clone(),
            settings: self.settings.clone(),
            models: self.models.clone(),
            templates: Arc::new(SimpleTemplates),
            aligner: Arc::new(zwisper_core::align::MaxOverlapAligner),
            echo: Arc::new(zwisper_core::echo::TextSimilarityEcho::default()),
            clock: Arc::new(FixedClock::default()),
            engines: self.engines.clone(),
            system_memory_bytes: 16 * 1024 * 1024 * 1024,
        }
    }
}

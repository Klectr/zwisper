//! Plain data types shared by every layer. No behaviour that depends on an engine,
//! an OS API or the UI lives here.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// All audio inside Zwisper is 16 kHz mono f32. Adapters convert at the edge.
pub const SAMPLE_RATE: u32 = 16_000;

/// Milliseconds since the start of a recording.
pub type Ms = u64;

pub fn samples_to_ms(n: usize) -> Ms {
    (n as u64 * 1000) / SAMPLE_RATE as u64
}

pub fn ms_to_samples(ms: Ms) -> usize {
    (ms * SAMPLE_RATE as u64 / 1000) as usize
}

/// Which physical stream a piece of audio/text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum TrackKind {
    /// The user's microphone. Always attributed to "You" in remote meetings.
    Mic,
    /// Everything the computer plays (the other meeting participants).
    System,
}

impl TrackKind {
    pub fn file_stem(self) -> &'static str {
        match self {
            TrackKind::Mic => "mic",
            TrackKind::System => "system",
        }
    }
}

/// A word with timings relative to the start of its track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start_ms: Ms,
    pub end_ms: Ms,
}

/// Raw transcriber output: one recognised stretch of speech.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
}

impl Segment {
    pub fn offset(mut self, by: Ms) -> Self {
        self.start_ms += by;
        self.end_ms += by;
        for w in &mut self.words {
            w.start_ms += by;
            w.end_ms += by;
        }
        self
    }
}

/// Diarizer output: "speaker `speaker` talked from start to end".
/// `speaker` is the diarizer's own cluster label, not yet a `SpeakerId`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpeakerTurn {
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub speaker: u32,
}

/// A chunk of contiguous speech found by the voice-activity detector.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechChunk {
    pub start_ms: Ms,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SpeakerId(pub u32);

impl SpeakerId {
    /// The local user. Always id 0.
    pub const YOU: SpeakerId = SpeakerId(0);
    /// Placeholder for remote speech before diarization has run.
    pub const UNKNOWN: SpeakerId = SpeakerId(u32::MAX);
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: SpeakerId,
    pub name: String,
    /// Index into the UI's fixed speaker palette.
    pub color_idx: u8,
    pub is_you: bool,
}

/// A paragraph of the final transcript, attributed to a speaker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: u64,
    pub track: TrackKind,
    pub speaker_id: SpeakerId,
    pub start_ms: Ms,
    pub end_ms: Ms,
    pub text: String,
    #[serde(default)]
    pub words: Vec<Word>,
    #[serde(default)]
    pub edited: bool,
}

/// The fixed, curated set of note templates. Deliberately an enum, not a string:
/// there is no user-defined template in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Template {
    Summary,
    ActionItems,
    TodoList,
    OneOnOne,
    FollowUpEmail,
}

impl Template {
    pub const ALL: [Template; 5] = [
        Template::Summary,
        Template::ActionItems,
        Template::TodoList,
        Template::OneOnOne,
        Template::FollowUpEmail,
    ];
    pub const DEFAULT: Template = Template::Summary;

    pub fn label(self) -> &'static str {
        match self {
            Template::Summary => "Summary",
            Template::ActionItems => "Action items",
            Template::TodoList => "To-do list",
            Template::OneOnOne => "1:1",
            Template::FollowUpEmail => "Follow-up email",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteResult {
    pub markdown: String,
    pub generated_at: DateTime<Utc>,
    /// Which engine produced it (e.g. "qwen3-4b-instruct-2507-q4km").
    pub engine: String,
    #[serde(default)]
    pub edited: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub ulid::Ulid);

impl SessionId {
    pub fn new() -> Self {
        SessionId(ulid::Ulid::new())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Recording,
    Paused,
    Processing,
    Done,
    /// Post-processing failed; transcript is still usable.
    Failed,
}

/// A meeting. `session.json` on disk is the source of truth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub id: SessionId,
    pub title: String,
    pub created: DateTime<Utc>,
    pub duration_ms: Ms,
    pub state: SessionState,
    pub tracks: Vec<TrackKind>,
    pub speakers: Vec<Speaker>,
    pub segments: Vec<TranscriptSegment>,
    #[serde(default)]
    pub my_notes: String,
    #[serde(default)]
    pub notes: BTreeMap<Template, NoteResult>,
    #[serde(default)]
    pub active_template: Option<Template>,
}

impl Session {
    pub const VERSION: u32 = 1;

    pub fn new(title: impl Into<String>, created: DateTime<Utc>) -> Self {
        Session {
            version: Self::VERSION,
            id: SessionId::new(),
            title: title.into(),
            created,
            duration_ms: 0,
            state: SessionState::Recording,
            tracks: Vec::new(),
            speakers: vec![Speaker {
                id: SpeakerId::YOU,
                name: "You".into(),
                color_idx: 0,
                is_you: true,
            }],
            segments: Vec::new(),
            my_notes: String::new(),
            notes: BTreeMap::new(),
            active_template: None,
        }
    }

    pub fn speaker(&self, id: SpeakerId) -> Option<&Speaker> {
        self.speakers.iter().find(|s| s.id == id)
    }

    pub fn speaker_name(&self, id: SpeakerId) -> &str {
        if id == SpeakerId::UNKNOWN {
            return "Speaker ?";
        }
        self.speaker(id).map(|s| s.name.as_str()).unwrap_or("Unknown")
    }

    pub fn next_segment_id(&self) -> u64 {
        self.segments.iter().map(|s| s.id + 1).max().unwrap_or(0)
    }

    /// Segments sorted by time for display.
    pub fn sort_segments(&mut self) {
        self.segments
            .sort_by(|a, b| a.start_ms.cmp(&b.start_ms).then(a.track.cmp(&b.track)));
    }

    /// One-line preview for library cards.
    pub fn preview(&self) -> String {
        if let Some(n) = self.notes.get(&Template::Summary) {
            if let Some(line) = n
                .markdown
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
            {
                return line.trim_start_matches(['-', '*', ' ']).to_string();
            }
        }
        self.segments
            .first()
            .map(|s| s.text.clone())
            .unwrap_or_default()
    }

    /// Render the transcript as `[mm:ss] Name: text` lines. This is both the
    /// human-readable `transcript.md` body and the LLM input format.
    pub fn transcript_lines(&self) -> String {
        let mut out = String::new();
        for s in &self.segments {
            out.push_str(&format!(
                "[{}] {}: {}\n",
                fmt_clock(s.start_ms),
                self.speaker_name(s.speaker_id),
                s.text.trim()
            ));
        }
        out
    }
}

/// Summary row for the library list, cheap to produce.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub title: String,
    pub created: DateTime<Utc>,
    pub duration_ms: Ms,
    pub state: SessionState,
    pub preview: String,
}

impl From<&Session> for SessionSummary {
    fn from(s: &Session) -> Self {
        SessionSummary {
            id: s.id,
            title: s.title.clone(),
            created: s.created,
            duration_ms: s.duration_ms,
            state: s.state,
            preview: s.preview(),
        }
    }
}

/// One dictation, for the optional history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dictation {
    pub at: DateTime<Utc>,
    pub text: String,
    pub duration_ms: Ms,
}

/// Hardware tier. The only model choice the product exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tier {
    Standard,
    Lite,
}

impl Tier {
    /// 16 GB or more → Standard.
    pub fn for_memory_bytes(bytes: u64) -> Tier {
        if bytes >= 15 * 1024 * 1024 * 1024 {
            Tier::Standard
        } else {
            Tier::Lite
        }
    }
}

/// Downloadable model packs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Pack {
    Speech,
    Speakers,
    Notes,
}

impl Pack {
    pub const ALL: [Pack; 3] = [Pack::Speech, Pack::Speakers, Pack::Notes];
    pub fn label(self) -> &'static str {
        match self {
            Pack::Speech => "Speech",
            Pack::Speakers => "Speakers",
            Pack::Notes => "Notes",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackStatus {
    NotDownloaded { bytes: u64 },
    Downloading { done: u64, total: u64 },
    Ready { bytes: u64 },
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PermissionKind {
    Microphone,
    /// Needed to paste into other apps.
    Accessibility,
    /// Needed to capture meeting audio.
    SystemAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionState {
    Granted,
    Denied,
    NotDetermined,
}

/// What happens to other apps' sound while you dictate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ducking {
    /// Leave it alone.
    Off,
    /// Turn the output volume down.
    Lower,
    /// Quieter and low-passed ("from the next room"): audible, not intelligible.
    #[default]
    Muffle,
}

/// User preferences. Stored as `config.toml` by an adapter; no secrets exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dictate_shortcut: String,
    pub meeting_shortcut: String,
    /// `None` = system default.
    pub microphone: Option<String>,
    pub record_system_audio: bool,
    pub tier: Option<Tier>,
    pub keep_dictation_history: bool,
    /// Quiet other apps while dictating (skipped during meetings).
    pub dictation_ducking: Ducking,
    pub launch_at_login: bool,
    pub library_dir: Option<std::path::PathBuf>,
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dictate_shortcut: "Alt+Space".into(),
            meeting_shortcut: "Alt+Shift+M".into(),
            microphone: None,
            record_system_audio: true,
            tier: None,
            keep_dictation_history: true,
            dictation_ducking: Ducking::default(),
            launch_at_login: false,
            library_dir: None,
            onboarded: false,
        }
    }
}

pub fn fmt_clock(ms: Ms) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

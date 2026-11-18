//! Commands in, events out. The UI (and hotkeys, tray, CLI…) only ever talk to
//! the core through these two enums.

use crate::domain::*;
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Command {
    // Dictation
    DictationHotkey(crate::ports::KeyState),
    StartDictation,
    StopDictation,
    CancelDictation,
    // Meetings
    StartMeeting { title: Option<String> },
    PauseMeeting,
    ResumeMeeting,
    EndMeeting,
    DiscardMeeting,
    // Editing
    SetTitle { id: SessionId, title: String },
    SetMyNotes { id: SessionId, text: String },
    RenameSpeaker { id: SessionId, speaker: SpeakerId, name: String },
    ReassignSegment { id: SessionId, segment: u64, speaker: SpeakerId },
    EditSegment { id: SessionId, segment: u64, text: String },
    EditNote { id: SessionId, template: Template, markdown: String },
    SelectTemplate { id: SessionId, template: Template },
    GenerateNotes { id: SessionId, template: Template, force: bool },
    CancelNotes,
    DeleteSession { id: SessionId },
    ClearDictations,
    // Models & settings
    DownloadPack(Pack),
    CancelDownload(Pack),
    RemovePack(Pack),
    SetTier(Tier),
    UpdateSettings(Settings),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DictationPhase {
    Idle,
    Listening,
    Transcribing,
    Error(String),
}

#[derive(Debug, Clone)]
pub enum Event {
    Dictation(DictationPhase),
    DictationLevel(f32),
    Dictated(String),

    MeetingStarted(SessionId),
    MeetingState { id: SessionId, state: SessionState },
    MeetingLevel { track: TrackKind, level: f32 },
    MeetingElapsed { id: SessionId, ms: Ms },
    LiveSegment { id: SessionId, segment: TranscriptSegment },
    Pipeline { id: SessionId, step: String, done: bool },

    SessionUpdated(Box<Session>),
    SessionDeleted(SessionId),
    LibraryChanged,

    NotesStarted { id: SessionId, template: Template },
    NotesDelta { id: SessionId, template: Template, delta: String },
    NotesDone { id: SessionId, template: Template },

    Pack { pack: Pack, status: PackStatus },
    TierChanged(Tier),
    SettingsChanged(Settings),

    /// Something the user should see (non-fatal).
    Notice(String),
    Error(String),
}

/// Fan-out event bus. Every subscriber gets every event.
#[derive(Clone, Default)]
pub struct EventBus {
    subs: Arc<Mutex<Vec<Sender<Event>>>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = unbounded();
        self.subs.lock().push(tx);
        rx
    }

    pub fn publish(&self, e: Event) {
        let mut subs = self.subs.lock();
        subs.retain(|s| s.send(e.clone()).is_ok());
    }
}

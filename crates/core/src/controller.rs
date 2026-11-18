//! `Core`: the single entry point front-ends use. Spawns a controller thread
//! that executes `Command`s; front-ends subscribe to `Event`s and may call the
//! read-only query methods directly.

use crate::bus::{Command, Event, EventBus};
use crate::cancel::CancelToken;
use crate::domain::*;
use crate::engines::Engines;
use crate::error::Result;
use crate::services::{dictation::DictationService, library, meeting::MeetingService, models, models::ModelService, notes, Ctx, Ports, SharedCtx};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Core {
    ctx: SharedCtx,
    tx: Sender<Command>,
    meeting: Arc<MeetingService>,
    dictation: Arc<DictationService>,
}

impl Core {
    pub fn start(ports: Ports) -> Result<Core> {
        let settings = ports.settings.load();
        let tier = settings
            .tier
            .unwrap_or_else(|| Tier::for_memory_bytes(ports.system_memory_bytes));
        let engines = Engines::build(ports.engines.clone(), tier)?;
        let ctx: SharedCtx = Arc::new(Ctx { ports, engines, events: EventBus::new() });
        let (tx, rx) = unbounded();
        let core = Core {
            meeting: Arc::new(MeetingService::new(ctx.clone())),
            dictation: Arc::new(DictationService::new(ctx.clone())),
            ctx,
            tx,
        };
        let worker = core.clone();
        std::thread::Builder::new()
            .name("zwisper-controller".into())
            .spawn(move || worker.run(rx))
            .map_err(|e| crate::Error::Engine(e.to_string()))?;
        Ok(core)
    }

    // ───── front-end API ─────

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        self.ctx.events.subscribe()
    }

    pub fn events(&self) -> &EventBus {
        &self.ctx.events
    }

    pub fn library(&self) -> Vec<SessionSummary> {
        let mut v = self.ctx.ports.repo.list().unwrap_or_default();
        v.sort_by(|a, b| b.created.cmp(&a.created));
        v
    }

    pub fn session(&self, id: SessionId) -> Option<Session> {
        self.ctx.ports.repo.load(id).ok()
    }

    pub fn dictations(&self, limit: usize) -> Vec<Dictation> {
        self.ctx.ports.repo.dictations(limit).unwrap_or_default()
    }

    pub fn settings(&self) -> Settings {
        self.ctx.ports.settings.load()
    }

    pub fn tier(&self) -> Tier {
        models::current_tier(&self.ctx)
    }

    pub fn pack_status(&self, pack: Pack) -> PackStatus {
        self.ctx.ports.models.status(pack, self.tier())
    }

    pub fn acknowledgements(&self) -> Vec<String> {
        self.ctx.ports.models.acknowledgements()
    }

    pub fn active_meeting(&self) -> Option<SessionId> {
        self.meeting.active_id()
    }

    pub fn session_folder(&self, id: SessionId) -> Option<std::path::PathBuf> {
        self.ctx.ports.repo.folder(id)
    }

    // ───── controller loop ─────

    fn run(self, rx: Receiver<Command>) {
        let notes_cancel: Arc<Mutex<Option<CancelToken>>> = Arc::default();
        let models = ModelService::default();
        loop {
            let cmd = match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(c) => c,
                Err(RecvTimeoutError::Timeout) => {
                    self.ctx.engines.maintain();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            tracing::debug!(?cmd, "command");
            let ctx = &self.ctx;
            let report = |r: Result<()>| {
                if let Err(e) = r {
                    ctx.events.publish(Event::Error(e.to_string()));
                }
            };
            match cmd {
                Command::DictationHotkey(k) => self.dictation.on_hotkey(k),
                Command::StartDictation => self.dictation.start(),
                Command::StopDictation => self.dictation.stop(),
                Command::CancelDictation => self.dictation.cancel(),

                Command::StartMeeting { title } => report(self.meeting.start(title).map(|_| ())),
                Command::PauseMeeting => self.meeting.pause(true),
                Command::ResumeMeeting => self.meeting.pause(false),
                Command::EndMeeting => {
                    self.meeting.end();
                }
                Command::DiscardMeeting => self.meeting.discard(),

                Command::SetTitle { id, title } => report(library::set_title(ctx, id, title)),
                Command::SetMyNotes { id, text } => {
                    // Live meeting notes are kept in the in-memory session too.
                    report(library::set_my_notes(ctx, id, text))
                }
                Command::RenameSpeaker { id, speaker, name } => report(library::rename_speaker(ctx, id, speaker, name)),
                Command::ReassignSegment { id, segment, speaker } => {
                    report(library::reassign_segment(ctx, id, segment, speaker))
                }
                Command::EditSegment { id, segment, text } => report(library::edit_segment(ctx, id, segment, text)),
                Command::EditNote { id, template, markdown } => report(library::edit_note(ctx, id, template, markdown)),
                Command::SelectTemplate { id, template } => {
                    self.spawn_notes(&notes_cancel, id, template, false)
                }
                Command::GenerateNotes { id, template, force } => {
                    self.spawn_notes(&notes_cancel, id, template, force)
                }
                Command::CancelNotes => {
                    if let Some(c) = notes_cancel.lock().as_ref() {
                        c.cancel();
                    }
                }
                Command::DeleteSession { id } => report(library::delete(ctx, id)),
                Command::ClearDictations => report(ctx.ports.repo.clear_dictations()),

                Command::DownloadPack(p) => models.download(ctx, p),
                Command::CancelDownload(p) => models.cancel(p),
                Command::RemovePack(p) => models.remove(ctx, p),
                Command::SetTier(t) => models.set_tier(ctx, t),
                Command::UpdateSettings(s) => {
                    report(ctx.ports.settings.save(&s));
                    ctx.events.publish(Event::SettingsChanged(s));
                }
                Command::Shutdown => {
                    self.dictation.cancel();
                    self.meeting.end();
                    break;
                }
            }
        }
    }

    fn spawn_notes(&self, slot: &Arc<Mutex<Option<CancelToken>>>, id: SessionId, template: Template, force: bool) {
        let cancel = CancelToken::new();
        if let Some(old) = slot.lock().replace(cancel.clone()) {
            old.cancel();
        }
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            if let Err(e) = notes::generate_for(&ctx, id, template, force, &cancel) {
                if e != crate::Error::Cancelled {
                    ctx.events.publish(Event::Error(format!("Notes: {e}")));
                }
                ctx.events.publish(Event::NotesDone { id, template });
            }
        });
    }
}

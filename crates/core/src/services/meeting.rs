//! Meeting recording: two tracks → recorder + VAD → live transcript.

use super::pipeline;
use super::SharedCtx;
use crate::bus::Event;
use crate::capture::{Capture, FrameProcessor};
use crate::domain::*;
use crate::dsp;
use crate::error::{Error, Result};
use crate::ports::{TrackWriter, VoiceActivity};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct TrackProcessor {
    ctx: SharedCtx,
    track: TrackKind,
    /// Mic speech is "You" in remote meetings; unknown in in-person ones.
    speaker: SpeakerId,
    session: Arc<Mutex<Session>>,
    writer: Option<Box<dyn TrackWriter>>,
    vad: Box<dyn VoiceActivity>,
    paused: Arc<AtomicBool>,
    samples: usize,
    last_level: Instant,
    last_elapsed_s: u64,
}

impl TrackProcessor {
    fn transcribe_chunk(&mut self, chunk: SpeechChunk) {
        let result = {
            let t = self.ctx.engines.transcriber.current();
            let mut t = t.lock();
            t.transcribe(&chunk.samples)
        };
        let segments = match result {
            Ok(s) => s,
            Err(e) => {
                self.ctx.events.publish(Event::Error(format!("Transcription failed: {e}")));
                return;
            }
        };
        let id = self.session.lock().id;
        for seg in segments {
            let seg = seg.offset(chunk.start_ms);
            let text = seg.text.trim().to_string();
            if text.is_empty() || (text.starts_with('[') && text.ends_with(']')) {
                continue;
            }
            let ts = {
                let mut s = self.session.lock();
                let ts = TranscriptSegment {
                    id: s.next_segment_id(),
                    track: self.track,
                    speaker_id: self.speaker,
                    start_ms: seg.start_ms,
                    end_ms: seg.end_ms,
                    text,
                    words: seg.words,
                    edited: false,
                };
                s.segments.push(ts.clone());
                ts
            };
            self.ctx.events.publish(Event::LiveSegment { id, segment: ts });
        }
    }
}

impl FrameProcessor for TrackProcessor {
    fn process(&mut self, frames: &[f32]) {
        if self.paused.load(Ordering::SeqCst) {
            return;
        }
        self.samples += frames.len();
        if let Some(w) = self.writer.as_mut() {
            if let Err(e) = w.write(frames) {
                self.ctx.events.publish(Event::Error(format!("Recording to disk failed: {e}")));
                self.writer = None;
            }
        }
        if self.last_level.elapsed() >= Duration::from_millis(50) {
            self.last_level = Instant::now();
            self.ctx.events.publish(Event::MeetingLevel { track: self.track, level: dsp::meter_level(frames) });
        }
        if self.track == TrackKind::Mic {
            let s = samples_to_ms(self.samples) / 1000;
            if s != self.last_elapsed_s {
                self.last_elapsed_s = s;
                let id = self.session.lock().id;
                self.ctx.events.publish(Event::MeetingElapsed { id, ms: samples_to_ms(self.samples) });
            }
        }
        match self.vad.accept(frames) {
            Ok(chunks) => {
                for c in chunks {
                    self.transcribe_chunk(c);
                }
            }
            Err(e) => tracing::warn!(error = %e, "vad failed"),
        }
    }

    fn finish(&mut self) {
        if let Ok(chunks) = self.vad.flush() {
            for c in chunks {
                self.transcribe_chunk(c);
            }
        }
        if let Some(w) = self.writer.take() {
            if let Err(e) = w.finish() {
                self.ctx.events.publish(Event::Error(format!("Could not finalize audio: {e}")));
            }
        }
    }
}

struct Active {
    session: Arc<Mutex<Session>>,
    captures: Vec<Capture<TrackProcessor>>,
    paused: Arc<AtomicBool>,
    in_person: bool,
}

pub struct MeetingService {
    ctx: SharedCtx,
    active: Mutex<Option<Active>>,
    pipeline_running: Arc<AtomicBool>,
}

impl MeetingService {
    pub fn new(ctx: SharedCtx) -> Self {
        MeetingService { ctx, active: Mutex::new(None), pipeline_running: Arc::new(AtomicBool::new(false)) }
    }

    pub fn active_id(&self) -> Option<SessionId> {
        self.active.lock().as_ref().map(|a| a.session.lock().id)
    }

    pub fn start(&self, title: Option<String>) -> Result<SessionId> {
        let mut active = self.active.lock();
        if active.is_some() {
            return Err(Error::InvalidState("a meeting is already recording".into()));
        }
        let ports = &self.ctx.ports;
        let settings = ports.settings.load();
        let now = ports.clock.now();
        let title = title.unwrap_or_else(|| {
            format!("Meeting {}", now.with_timezone(&chrono::Local).format("%b %-d, %-I:%M %p"))
        });
        let mut session = Session::new(title, now);

        let mic = ports.audio.microphone(settings.microphone.as_deref())?;
        let system = if settings.record_system_audio {
            match ports.audio.system_output() {
                Ok(s) => s,
                Err(e) => {
                    self.ctx.events.publish(Event::Notice(format!(
                        "Recording your microphone only — system audio unavailable ({e})"
                    )));
                    None
                }
            }
        } else {
            None
        };
        let in_person = system.is_none();
        session.tracks.push(TrackKind::Mic);
        if system.is_some() {
            session.tracks.push(TrackKind::System);
        }
        ports.repo.save(&session)?;
        let id = session.id;
        let session = Arc::new(Mutex::new(session));
        let paused = Arc::new(AtomicBool::new(false));

        let mut captures = Vec::new();
        let mut sources = vec![(TrackKind::Mic, mic)];
        if let Some(s) = system {
            sources.push((TrackKind::System, s));
        }
        for (track, src) in sources {
            let speaker = if track == TrackKind::Mic && !in_person { SpeakerId::YOU } else { SpeakerId::UNKNOWN };
            let proc = TrackProcessor {
                ctx: self.ctx.clone(),
                track,
                speaker,
                session: session.clone(),
                writer: Some(ports.recorder.create(id, track)?),
                vad: self.ctx.engines.new_vad()?,
                paused: paused.clone(),
                samples: 0,
                last_level: Instant::now(),
                last_elapsed_s: 0,
            };
            match Capture::start(src, proc) {
                Ok(c) => captures.push(c),
                Err(e) => {
                    for c in captures {
                        c.stop();
                    }
                    let _ = ports.repo.delete(id);
                    return Err(e);
                }
            }
        }
        // Load the speech model now rather than on the first chunk.
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let t = ctx.engines.transcriber.current();
            let _ = t.lock().preload();
        });

        *active = Some(Active { session, captures, paused, in_person });
        self.ctx.events.publish(Event::MeetingStarted(id));
        self.ctx.events.publish(Event::MeetingState { id, state: SessionState::Recording });
        self.ctx.events.publish(Event::LibraryChanged);
        Ok(id)
    }

    pub fn pause(&self, pause: bool) {
        if let Some(a) = self.active.lock().as_ref() {
            a.paused.store(pause, Ordering::SeqCst);
            let mut s = a.session.lock();
            s.state = if pause { SessionState::Paused } else { SessionState::Recording };
            self.ctx.events.publish(Event::MeetingState { id: s.id, state: s.state });
        }
    }

    fn stop_capture(&self) -> Option<(Session, bool)> {
        let a = self.active.lock().take()?;
        let mut duration = 0;
        for c in a.captures {
            let p = c.stop();
            duration = duration.max(samples_to_ms(p.samples));
        }
        let mut s = a.session.lock().clone();
        // The repo copy is the source of truth for fields the user edits while
        // recording (title, my notes).
        if let Ok(stored) = self.ctx.ports.repo.load(s.id) {
            s.title = stored.title;
            s.my_notes = stored.my_notes;
        }
        s.duration_ms = duration;
        s.sort_segments();
        Some((s, a.in_person))
    }

    /// Stop recording and run post-processing in the background.
    pub fn end(&self) -> Option<SessionId> {
        let (mut session, in_person) = self.stop_capture()?;
        session.state = SessionState::Processing;
        let id = session.id;
        if let Err(e) = self.ctx.ports.repo.save(&session) {
            self.ctx.events.publish(Event::Error(format!("Could not save meeting: {e}")));
        }
        self.ctx.events.publish(Event::MeetingState { id, state: SessionState::Processing });
        self.ctx.events.publish(Event::SessionUpdated(Box::new(session.clone())));
        let ctx = self.ctx.clone();
        let running = self.pipeline_running.clone();
        running.store(true, Ordering::SeqCst);
        std::thread::Builder::new()
            .name("post-process".into())
            .spawn(move || {
                pipeline::run_default(&ctx, session, in_person);
                running.store(false, Ordering::SeqCst);
            })
            .ok();
        Some(id)
    }

    /// Stop recording and delete everything.
    pub fn discard(&self) {
        if let Some((s, _)) = self.stop_capture() {
            let _ = self.ctx.ports.repo.delete(s.id);
            self.ctx.events.publish(Event::SessionDeleted(s.id));
            self.ctx.events.publish(Event::LibraryChanged);
        }
    }

    pub fn is_processing(&self) -> bool {
        self.pipeline_running.load(Ordering::SeqCst)
    }
}

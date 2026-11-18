//! Post-meeting processing as an ordered list of swappable steps.

use super::{notes, SharedCtx};
use crate::bus::Event;
use crate::cancel::CancelToken;
use crate::domain::*;
use crate::error::{Error, Result};

pub struct PipelineCtx<'a> {
    pub ctx: &'a SharedCtx,
    pub session: Session,
    pub in_person: bool,
    pub cancel: CancelToken,
}

pub trait PipelineStep: Send + Sync {
    fn name(&self) -> &'static str;
    /// Optional steps may fail without failing the meeting.
    fn optional(&self) -> bool {
        true
    }
    fn run(&self, p: &mut PipelineCtx) -> Result<()>;
}

/// Free the speech model before heavier models load.
pub struct UnloadSpeech;
impl PipelineStep for UnloadSpeech {
    fn name(&self) -> &'static str {
        "Finishing transcript"
    }
    fn run(&self, p: &mut PipelineCtx) -> Result<()> {
        p.ctx.engines.transcriber.current().lock().unload();
        Ok(())
    }
}

/// Diarize the remote track (or the mic in person) and align words to speakers.
pub struct DetectSpeakers;
impl PipelineStep for DetectSpeakers {
    fn name(&self) -> &'static str {
        "Detecting speakers"
    }
    fn run(&self, p: &mut PipelineCtx) -> Result<()> {
        let track = if p.in_person { TrackKind::Mic } else { TrackKind::System };
        let has_track_text = p.session.segments.iter().any(|s| s.track == track);
        let result = if has_track_text {
            p.ctx
                .ports
                .recorder
                .read(p.session.id, track)
                .and_then(|audio| {
                    let d = p.ctx.engines.diarizer.current();
                    let mut d = d.lock();
                    let r = d.diarize(&audio);
                    d.unload();
                    r
                })
        } else {
            Ok(Vec::new())
        };
        let turns = match result {
            Ok(t) => t,
            Err(e) => {
                // Without diarization everyone remote becomes "Speaker 1".
                assign_single_speaker(&mut p.session, track);
                return Err(e);
            }
        };
        let segs: Vec<TranscriptSegment> = p.session.segments.iter().filter(|s| s.track == track).cloned().collect();
        let aligned = if turns.is_empty() {
            assign_single_speaker(&mut p.session, track);
            return Ok(());
        } else {
            p.ctx.ports.aligner.align(track, &segs, &turns, 1)
        };
        p.session.segments.retain(|s| s.track != track);
        let mut next = p.session.next_segment_id();
        for mut s in aligned {
            s.id = next;
            next += 1;
            p.session.segments.push(s);
        }
        p.session.sort_segments();
        ensure_speakers(&mut p.session);
        Ok(())
    }
}

fn assign_single_speaker(session: &mut Session, track: TrackKind) {
    for s in session.segments.iter_mut().filter(|s| s.track == track) {
        if s.speaker_id == SpeakerId::UNKNOWN {
            s.speaker_id = SpeakerId(1);
        }
    }
    ensure_speakers(session);
}

/// Create a `Speaker` entry for every id referenced by a segment.
pub fn ensure_speakers(session: &mut Session) {
    let mut ids: Vec<SpeakerId> = session.segments.iter().map(|s| s.speaker_id).collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        if id == SpeakerId::UNKNOWN || session.speaker(id).is_some() {
            continue;
        }
        session.speakers.push(Speaker {
            id,
            name: format!("Speaker {}", id.0),
            color_idx: (id.0 % 6) as u8,
            is_you: false,
        });
    }
}

/// Drop mic paragraphs that are echoes of remote audio.
pub struct RemoveEcho;
impl PipelineStep for RemoveEcho {
    fn name(&self) -> &'static str {
        "Cleaning up"
    }
    fn run(&self, p: &mut PipelineCtx) -> Result<()> {
        if p.in_person {
            return Ok(());
        }
        let (mic, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut p.session.segments).into_iter().partition(|s| s.track == TrackKind::Mic);
        let kept = p.ctx.ports.echo.filter(mic, &rest);
        p.session.segments = rest;
        p.session.segments.extend(kept);
        p.session.sort_segments();
        Ok(())
    }
}

/// Write the default note.
pub struct WriteNotes;
impl PipelineStep for WriteNotes {
    fn name(&self) -> &'static str {
        "Writing notes"
    }
    fn run(&self, p: &mut PipelineCtx) -> Result<()> {
        if p.session.segments.is_empty() && p.session.my_notes.trim().is_empty() {
            return Ok(());
        }
        // Persist first so the note writer works from the final transcript.
        p.ctx.ports.repo.save(&p.session)?;
        p.ctx.events.publish(Event::SessionUpdated(Box::new(p.session.clone())));
        notes::generate_into(p.ctx, &mut p.session, Template::DEFAULT, &p.cancel)
    }
}

pub fn default_steps() -> Vec<Box<dyn PipelineStep>> {
    vec![Box::new(UnloadSpeech), Box::new(DetectSpeakers), Box::new(RemoveEcho), Box::new(WriteNotes)]
}

pub fn run(ctx: &SharedCtx, session: Session, in_person: bool, steps: &[Box<dyn PipelineStep>]) -> Session {
    let id = session.id;
    let mut p = PipelineCtx { ctx, session, in_person, cancel: CancelToken::new() };
    let mut failed = false;
    for step in steps {
        ctx.events.publish(Event::Pipeline { id, step: step.name().into(), done: false });
        if let Err(e) = step.run(&mut p) {
            match e {
                Error::ModelMissing(pack) => ctx.events.publish(Event::Notice(format!(
                    "{} skipped — download the {} model in Settings.",
                    step.name(),
                    pack.label()
                ))),
                e => {
                    tracing::warn!(step = step.name(), error = %e, "pipeline step failed");
                    ctx.events.publish(Event::Error(format!("{}: {e}", step.name())));
                    if !step.optional() {
                        failed = true;
                        break;
                    }
                }
            }
        }
    }
    p.session.state = if failed { SessionState::Failed } else { SessionState::Done };
    if let Err(e) = ctx.ports.repo.save(&p.session) {
        ctx.events.publish(Event::Error(format!("Could not save meeting: {e}")));
    }
    ctx.events.publish(Event::Pipeline { id, step: String::new(), done: true });
    ctx.events.publish(Event::MeetingState { id, state: p.session.state });
    ctx.events.publish(Event::SessionUpdated(Box::new(p.session.clone())));
    if let Some(template) = p.session.active_template {
        ctx.events.publish(Event::NotesDone { id, template });
    }
    ctx.events.publish(Event::LibraryChanged);
    p.session
}

pub fn run_default(ctx: &SharedCtx, session: Session, in_person: bool) -> Session {
    run(ctx, session, in_person, &default_steps())
}

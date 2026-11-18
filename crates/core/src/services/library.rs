//! Editing stored sessions.

use super::{pipeline::ensure_speakers, SharedCtx};
use crate::bus::Event;
use crate::domain::*;
use crate::error::{Error, Result};

fn edit(ctx: &SharedCtx, id: SessionId, f: impl FnOnce(&mut Session) -> Result<()>) -> Result<()> {
    let mut s = ctx.ports.repo.load(id)?;
    f(&mut s)?;
    ctx.ports.repo.save(&s)?;
    ctx.events.publish(Event::SessionUpdated(Box::new(s)));
    Ok(())
}

pub fn set_title(ctx: &SharedCtx, id: SessionId, title: String) -> Result<()> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(Error::InvalidState("title cannot be empty".into()));
    }
    edit(ctx, id, |s| {
        s.title = title;
        Ok(())
    })?;
    ctx.events.publish(Event::LibraryChanged);
    Ok(())
}

pub fn set_my_notes(ctx: &SharedCtx, id: SessionId, text: String) -> Result<()> {
    edit(ctx, id, |s| {
        s.my_notes = text;
        Ok(())
    })
}

/// Rename a speaker everywhere. Renaming to an existing name merges the two.
pub fn rename_speaker(ctx: &SharedCtx, id: SessionId, speaker: SpeakerId, name: String) -> Result<()> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(Error::InvalidState("name cannot be empty".into()));
    }
    edit(ctx, id, |s| {
        let existing = s
            .speakers
            .iter()
            .find(|sp| sp.id != speaker && sp.name.eq_ignore_ascii_case(&name))
            .map(|sp| sp.id);
        if let Some(target) = existing {
            for seg in s.segments.iter_mut().filter(|g| g.speaker_id == speaker) {
                seg.speaker_id = target;
            }
            if !s.speakers.iter().any(|sp| sp.id == speaker && sp.is_you) {
                s.speakers.retain(|sp| sp.id != speaker);
            }
        } else {
            let sp = s
                .speakers
                .iter_mut()
                .find(|sp| sp.id == speaker)
                .ok_or_else(|| Error::NotFound("speaker".into()))?;
            sp.name = name;
        }
        Ok(())
    })
}

pub fn reassign_segment(ctx: &SharedCtx, id: SessionId, segment: u64, speaker: SpeakerId) -> Result<()> {
    edit(ctx, id, |s| {
        let seg = s
            .segments
            .iter_mut()
            .find(|g| g.id == segment)
            .ok_or_else(|| Error::NotFound("segment".into()))?;
        seg.speaker_id = speaker;
        ensure_speakers(s);
        Ok(())
    })
}

pub fn edit_segment(ctx: &SharedCtx, id: SessionId, segment: u64, text: String) -> Result<()> {
    edit(ctx, id, |s| {
        let seg = s
            .segments
            .iter_mut()
            .find(|g| g.id == segment)
            .ok_or_else(|| Error::NotFound("segment".into()))?;
        seg.text = text;
        seg.edited = true;
        Ok(())
    })
}

pub fn edit_note(ctx: &SharedCtx, id: SessionId, template: Template, markdown: String) -> Result<()> {
    edit(ctx, id, |s| {
        let n = s
            .notes
            .get_mut(&template)
            .ok_or_else(|| Error::NotFound("note".into()))?;
        n.markdown = markdown;
        n.edited = true;
        Ok(())
    })
}

pub fn delete(ctx: &SharedCtx, id: SessionId) -> Result<()> {
    ctx.ports.repo.delete(id)?;
    ctx.events.publish(Event::SessionDeleted(id));
    ctx.events.publish(Event::LibraryChanged);
    Ok(())
}

//! Transcript → notes with the curated templates.

use super::SharedCtx;
use crate::bus::Event;
use crate::cancel::CancelToken;
use crate::domain::*;
use crate::error::{Error, Result};
use crate::ports::NoteInput;

pub fn note_input(session: &Session) -> NoteInput {
    let mut speakers: Vec<String> = session
        .segments
        .iter()
        .map(|s| session.speaker_name(s.speaker_id).to_string())
        .collect();
    speakers.sort();
    speakers.dedup();
    NoteInput {
        title: session.title.clone(),
        date: session.created.with_timezone(&chrono::Local).format("%A, %B %-d, %Y").to_string(),
        transcript: session.transcript_lines(),
        my_notes: session.my_notes.clone(),
        speakers,
    }
}

/// Split transcript lines into parts of at most `budget` tokens.
fn split_lines(lines: &str, budget: usize, count: &mut dyn FnMut(&str) -> usize) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut used = 0;
    for line in lines.lines() {
        let n = count(line) + 1;
        if used + n > budget && !parts.last().unwrap().is_empty() {
            parts.push(String::new());
            used = 0;
        }
        let last = parts.last_mut().unwrap();
        last.push_str(line);
        last.push('\n');
        used += n;
    }
    parts
}

/// Generate `template` for `session`, streaming deltas as events, and store the result in the session.
pub fn generate_into(ctx: &SharedCtx, session: &mut Session, template: Template, cancel: &CancelToken) -> Result<()> {
    let id = session.id;
    let templates = &ctx.ports.templates;
    let writer = ctx.engines.notes.current();
    let mut w = writer.lock();
    let mut input = note_input(session);

    ctx.events.publish(Event::NotesStarted { id, template });

    // Very long meetings: condense in parts first (map), then write the note (reduce).
    let budget = w.input_budget_tokens();
    let tokens = w.count_tokens(&input.transcript);
    if tokens > budget {
        let parts = {
            let w2 = &mut *w;
            split_lines(&input.transcript, budget, &mut |l| w2.count_tokens(l))
        };
        let of = parts.len();
        let mut condensed = String::new();
        for (i, part) in parts.into_iter().enumerate() {
            cancel.check()?;
            ctx.events.publish(Event::Pipeline { id, step: format!("Reading part {} of {}", i + 1, of), done: false });
            let part_input = NoteInput { transcript: part, ..input.clone() };
            let prompt = templates.chunk_prompt(&part_input, i + 1, of);
            let out = w.generate(&prompt, &mut |_| {}, cancel)?;
            condensed.push_str(&format!("Part {} of {}:\n{}\n\n", i + 1, of, out.trim()));
        }
        input.transcript = condensed;
    }

    let mut prompt = templates.prompt(template, &input);
    let mut attempt = 0;
    let markdown = loop {
        attempt += 1;
        let mut on_delta = |d: &str| {
            ctx.events.publish(Event::NotesDelta { id, template, delta: d.to_string() });
        };
        let out = w.generate(&prompt, &mut on_delta, cancel)?;
        let out = strip_think(&out);
        match templates.validate(template, &out) {
            Ok(()) => break out,
            Err(reason) if attempt < 2 => {
                tracing::info!(%reason, "note failed validation; retrying");
                prompt.temperature = 0.2;
                ctx.events.publish(Event::NotesStarted { id, template });
            }
            Err(reason) => {
                tracing::warn!(%reason, "note failed validation twice; keeping output");
                break out;
            }
        }
    };
    let engine = w.id().to_string();
    w.unload();
    drop(w);

    session.notes.insert(
        template,
        NoteResult { markdown, generated_at: ctx.ports.clock.now(), engine, edited: false },
    );
    session.active_template = Some(template);
    // `NotesDone` is published by the caller once the result is persisted.
    Ok(())
}

/// Remove any `<think>…</think>` block a reasoning model might emit.
pub fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out.trim().to_string()
}

/// Generate notes for a stored session (from the UI).
pub fn generate_for(ctx: &SharedCtx, id: SessionId, template: Template, force: bool, cancel: &CancelToken) -> Result<()> {
    let mut session = ctx.ports.repo.load(id)?;
    if !force && session.notes.contains_key(&template) {
        session.active_template = Some(template);
        ctx.ports.repo.save(&session)?;
        ctx.events.publish(Event::SessionUpdated(Box::new(session)));
        return Ok(());
    }
    if session.segments.is_empty() && session.my_notes.trim().is_empty() {
        return Err(Error::InvalidState("nothing to write notes from yet".into()));
    }
    let res = generate_into(ctx, &mut session, template, cancel);
    // Reload in case the user edited while we generated; only replace the note.
    let mut latest = ctx.ports.repo.load(id).unwrap_or_else(|_| session.clone());
    if let Some(n) = session.notes.get(&template) {
        latest.notes.insert(template, n.clone());
        latest.active_template = Some(template);
    }
    ctx.ports.repo.save(&latest)?;
    ctx.events.publish(Event::SessionUpdated(Box::new(latest)));
    if res.is_ok() {
        ctx.events.publish(Event::NotesDone { id, template });
    }
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn strips_think() {
        assert_eq!(super::strip_think("<think>hmm</think>\n## Summary\nok"), "## Summary\nok");
    }
}

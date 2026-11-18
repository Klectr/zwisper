//! `BuiltinTemplates`: the five curated note templates.
//!
//! Prompts are written for small instruct models: one task, an explicit output
//! skeleton, and hard rules against inventing facts. Every template has a
//! validator so the notes service can retry once on malformed output.

use zwisper_core::domain::Template;
use zwisper_core::ports::{NoteInput, Prompt, TemplateCatalog};

pub struct BuiltinTemplates;

const SYSTEM: &str = "You are an expert meeting note-taker. You write clear, compact notes in Markdown. \
Rules:\n\
- Use ONLY information from the transcript and the user's notes. Never invent names, dates, numbers or decisions.\n\
- The user's own notes are authoritative facts; prefer them when they conflict with the transcript.\n\
- \"You\" in the transcript is the user who recorded the meeting.\n\
- Be concise. No preamble, no closing remarks, no code fences.\n\
- If a section has nothing to report, write \"- None\".";

struct Spec {
    instructions: &'static str,
    skeleton: &'static str,
    required: &'static [&'static str],
    max_tokens: u32,
}

fn spec(t: Template) -> Spec {
    match t {
        Template::Summary => Spec {
            instructions: "Write meeting notes with a short summary, the decisions made, and the action items.",
            skeleton: "## Summary\n<2-4 sentences>\n\n## Decisions\n- <decision>\n\n## Action items\n- [ ] <Owner>: <task> (<due date if stated>)",
            required: &["## Summary", "## Decisions", "## Action items"],
            max_tokens: 900,
        },
        Template::ActionItems => Spec {
            instructions: "List every action item, task, follow-up or commitment from the meeting. One line each, with the owner first.",
            skeleton: "## Action items\n- [ ] <Owner>: <task> (<due date if stated>)",
            required: &["## Action items", "- [ ]"],
            max_tokens: 600,
        },
        Template::TodoList => Spec {
            instructions: "Write the user's personal to-do list: only things \"You\" committed to or needs to do. Make each item a concrete next step.",
            skeleton: "## To-do\n- [ ] <task>",
            required: &["## To-do", "- [ ]"],
            max_tokens: 500,
        },
        Template::OneOnOne => Spec {
            instructions: "Write notes for a 1:1 meeting between the user and one other person.",
            skeleton: "## Updates\n- <update>\n\n## Wins\n- <win>\n\n## Blockers\n- <blocker>\n\n## Feedback\n- <feedback>\n\n## Follow-ups\n- [ ] <Owner>: <task>",
            required: &["## Updates", "## Blockers", "## Follow-ups"],
            max_tokens: 900,
        },
        Template::FollowUpEmail => Spec {
            instructions: "Write a short follow-up email from the user to the other attendees: thank them, recap key points and decisions, and list next steps with owners. Plain, friendly, professional.",
            skeleton: "Subject: <subject>\n\nHi <names or team>,\n\n<recap paragraph>\n\nNext steps:\n- <Owner>: <task>\n\nThanks,\n<leave the sign-off name blank>",
            required: &["Subject:", "Next steps"],
            max_tokens: 700,
        },
    }
}

fn context(i: &NoteInput) -> String {
    let notes = if i.my_notes.trim().is_empty() { "(none)" } else { i.my_notes.trim() };
    format!(
        "Meeting: {}\nDate: {}\nParticipants: {}\n\n<my_notes>\n{}\n</my_notes>\n\n<transcript>\n{}</transcript>",
        i.title,
        i.date,
        if i.speakers.is_empty() { "unknown".to_string() } else { i.speakers.join(", ") },
        notes,
        i.transcript
    )
}

impl TemplateCatalog for BuiltinTemplates {
    fn prompt(&self, t: Template, i: &NoteInput) -> Prompt {
        let s = spec(t);
        Prompt {
            system: SYSTEM.to_string(),
            user: format!(
                "{}\n\n{}\n\nOutput exactly this structure:\n{}\n",
                context(i),
                s.instructions,
                s.skeleton
            ),
            max_tokens: s.max_tokens,
            temperature: 0.7,
        }
    }

    fn chunk_prompt(&self, i: &NoteInput, part: usize, of: usize) -> Prompt {
        Prompt {
            system: SYSTEM.to_string(),
            user: format!(
                "{}\n\nThis is part {part} of {of} of a long meeting. Condense it into bullet points that keep every \
                 decision, action item (with owner), number, date and open question. Keep speaker names. \
                 Output only the bullets.",
                context(i)
            ),
            max_tokens: 700,
            temperature: 0.3,
        }
    }

    fn validate(&self, t: Template, md: &str) -> Result<(), String> {
        if md.trim().is_empty() {
            return Err("empty output".into());
        }
        if md.contains("<transcript>") || md.contains("<my_notes>") {
            return Err("echoed the prompt".into());
        }
        let s = spec(t);
        let missing: Vec<&str> = s.required.iter().copied().filter(|r| !md.contains(r)).collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(format!("missing {}", missing.join(", ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zwisper_core::testkit::template_contract;

    #[test]
    fn contract() {
        template_contract(
            &BuiltinTemplates,
            &[
                (Template::Summary, "## Summary\nShip.\n\n## Decisions\n- Ship Thursday\n\n## Action items\n- [ ] Priya: draft"),
                (Template::ActionItems, "## Action items\n- [ ] Priya: draft"),
                (Template::TodoList, "## To-do\n- [ ] Confirm QA"),
                (Template::OneOnOne, "## Updates\n- a\n\n## Wins\n- b\n\n## Blockers\n- None\n\n## Feedback\n- c\n\n## Follow-ups\n- [ ] You: x"),
                (Template::FollowUpEmail, "Subject: Recap\n\nHi team,\n\nThanks.\n\nNext steps:\n- Priya: draft\n\nThanks,"),
            ],
        );
    }

    #[test]
    fn rejects_missing_sections() {
        assert!(BuiltinTemplates.validate(Template::Summary, "## Summary\nok").is_err());
    }
}

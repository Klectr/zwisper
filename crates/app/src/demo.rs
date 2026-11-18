//! Demo composition: real storage and templates, scripted engines and fake
//! audio. Runs anywhere (including Linux CI) — used for UI development,
//! screenshots and as living documentation of the composition root.

use chrono::{Duration, Utc};
use std::sync::Arc;
use zwisper_core::domain::*;
use zwisper_core::ports::*;
use zwisper_core::Ports;
use zwisper_fakes::*;

pub fn ports(root: &std::path::Path) -> Ports {
    let store = Arc::new(zwisper_store_fs::FsStore::new(root.join("Library")).expect("library dir"));
    let settings = Arc::new(zwisper_store_fs::TomlSettings::new(root.join("config.toml")));
    let mut s = settings.load();
    s.library_dir = Some(root.join("Library"));
    let _ = settings.save(&s);
    let audio = FakeAudio::new(
        tone(&[(0.0, 300), (0.1, 700), (0.0, 200), (0.3, 600), (0.0, 300)]),
        Some(tone(&[(0.0, 1500), (0.1, 1200), (0.0, 600), (0.3, 1500), (0.0, 500)])),
    );
    Ports {
        audio: Arc::new(audio),
        recorder: store.clone(),
        injector: Arc::new(RecordingInjector::default()),
        ducker: Arc::new(zwisper_core::ports::NoDucking),
        repo: store,
        settings,
        models: Arc::new(FakeModels::default()),
        templates: Arc::new(zwisper_templates::BuiltinTemplates),
        aligner: Arc::new(zwisper_core::align::MaxOverlapAligner),
        echo: Arc::new(zwisper_core::echo::TextSimilarityEcho::default()),
        clock: Arc::new(SystemClockFake),
        engines: Arc::new(FakeEngines::default()),
        system_memory_bytes: 16 << 30,
    }
}

struct SystemClockFake;
impl Clock for SystemClockFake {
    fn now(&self) -> chrono::DateTime<Utc> {
        Utc::now()
    }
}

/// Seed a realistic finished meeting and a few dictations (idempotent).
pub fn seed(repo: &dyn SessionRepository) -> Option<SessionId> {
    if let Some(s) = repo.list().ok()?.into_iter().find(|s| s.title == "Weekly product sync") {
        return Some(s.id);
    }
    let mut s = Session::new("Weekly product sync", Utc::now() - Duration::minutes(42));
    s.state = SessionState::Done;
    s.duration_ms = 31 * 60 * 1000;
    s.tracks = vec![TrackKind::Mic, TrackKind::System];
    s.speakers.push(Speaker { id: SpeakerId(1), name: "Priya".into(), color_idx: 1, is_you: false });
    s.speakers.push(Speaker { id: SpeakerId(2), name: "Jordan".into(), color_idx: 2, is_you: false });
    let lines: [(u32, u64, &str); 8] = [
        (0, 2, "Okay, let's start with the launch checklist. Where are we on the release notes?"),
        (1, 6, "Release notes are drafted. I still need a screenshot for the new settings page."),
        (2, 11, "I can get you that by end of day. The build is stable on my machine."),
        (0, 15, "Good. Do we want to ship Thursday or hold until next week?"),
        (1, 19, "Thursday works if QA signs off tomorrow."),
        (2, 23, "QA is on track. One open bug in the export flow, but it has a workaround."),
        (0, 28, "Then we ship Thursday. Priya, please send the announcement draft to me on Wednesday."),
        (1, 34, "Will do. I'll loop in marketing as well."),
    ];
    for (i, (sp, sec, text)) in lines.iter().enumerate() {
        s.segments.push(TranscriptSegment {
            id: i as u64,
            track: if *sp == 0 { TrackKind::Mic } else { TrackKind::System },
            speaker_id: SpeakerId(*sp),
            start_ms: sec * 1000,
            end_ms: sec * 1000 + 3500,
            text: text.to_string(),
            words: vec![],
            edited: false,
        });
    }
    s.my_notes = "Ask Jordan about the export format.".into();
    s.notes.insert(
        Template::Summary,
        NoteResult {
            markdown: "## Summary\nThe team reviewed launch readiness and agreed to ship on Thursday, pending QA sign-off tomorrow.\n\n## Decisions\n- Ship the release on Thursday.\n- Hold the export bug fix for the next patch.\n\n## Action items\n- [ ] Priya: send the announcement draft on Wednesday\n- [ ] Jordan: share the settings page screenshot today\n- [x] You: confirm QA sign-off tomorrow".into(),
            generated_at: Utc::now(),
            engine: "qwen3-4b-instruct-2507-q4km".into(),
            edited: false,
        },
    );
    s.active_template = Some(Template::Summary);
    repo.save(&s).ok()?;

    let mut s2 = Session::new("1:1 with Sam", Utc::now() - Duration::days(1) - Duration::hours(3));
    s2.state = SessionState::Done;
    s2.duration_ms = 24 * 60 * 1000;
    s2.notes.insert(
        Template::Summary,
        NoteResult {
            markdown: "## Summary\nSam is ramping up on the sync engine and wants more code-review feedback.\n\n## Decisions\n- None\n\n## Action items\n- [ ] You: pair with Sam on Friday".into(),
            generated_at: Utc::now(),
            engine: "qwen3-4b-instruct-2507-q4km".into(),
            edited: false,
        },
    );
    repo.save(&s2).ok()?;
    for (mins, text) in [
        (5, "Sounds good — let's sync after lunch and lock the plan."),
        (50, "Can you send me the latest numbers before the board prep?"),
        (130, "Remind me to renew the domain before Friday."),
    ] {
        let _ = repo.append_dictation(&Dictation { at: Utc::now() - Duration::minutes(mins), text: text.into(), duration_ms: 3000 });
    }
    Some(s.id)
}

pub struct DemoPlatform;
impl zwisper_ui::UiPlatform for DemoPlatform {
    fn permissions(&self) -> Arc<dyn Permissions> {
        Arc::new(AllGranted)
    }
    // Cosmetic only, so the demo pill looks like the real one.
    #[cfg(target_os = "macos")]
    fn make_frameless(&self, window: &gpui::Window) {
        crate::mac::make_frameless(window)
    }
}

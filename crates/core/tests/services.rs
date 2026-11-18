//! Service tests: the whole core running on fakes. No models, no OS.

use crossbeam_channel::Receiver;
use std::time::{Duration, Instant};
use zwisper_core::domain::*;
use zwisper_core::ports::{KeyState, SettingsStore};
use zwisper_core::{Command, Core, DictationPhase, Event};
use zwisper_fakes::*;

fn wait_for(rx: &Receiver<Event>, timeout_s: u64, mut pred: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(timeout_s);
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        if let Ok(e) = rx.recv_timeout(Duration::from_millis(50)) {
            let hit = pred(&e);
            seen.push(e);
            if hit {
                return seen;
            }
        }
    }
    panic!("timed out; events seen: {seen:#?}");
}

fn start(world: &FakeWorld) -> (Core, Receiver<Event>) {
    let core = Core::start(world.ports()).unwrap();
    let rx = core.subscribe();
    (core, rx)
}

#[test]
fn dictation_toggle_pastes_text_and_keeps_history() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.0, 300), (0.1, 500), (0.0, 200), (0.3, 500), (0.0, 300)]);
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    core.send(Command::StopDictation);
    let ev = wait_for(&rx, 5, |e| matches!(e, Event::Dictated(_)));
    assert!(ev.iter().any(|e| matches!(e, Event::Dictation(DictationPhase::Transcribing))));
    assert_eq!(world.injector.injected.lock().as_slice(), ["hello world"]);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Idle)));
    assert_eq!(core.dictations(10)[0].text, "hello world");
}

#[test]
fn dictation_push_to_talk_stops_on_release() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.1, 600)]);
    let (core, rx) = start(&world);
    core.send(Command::DictationHotkey(KeyState::Pressed));
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    std::thread::sleep(Duration::from_millis(450));
    core.send(Command::DictationHotkey(KeyState::Released));
    wait_for(&rx, 5, |e| matches!(e, Event::Dictated(t) if t == "hello"));
}

#[test]
fn dictation_quick_tap_keeps_listening_until_second_press() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.3, 600)]);
    let (core, rx) = start(&world);
    core.send(Command::DictationHotkey(KeyState::Pressed));
    core.send(Command::DictationHotkey(KeyState::Released));
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    std::thread::sleep(Duration::from_millis(200));
    assert!(world.injector.injected.lock().is_empty());
    core.send(Command::DictationHotkey(KeyState::Pressed));
    wait_for(&rx, 5, |e| matches!(e, Event::Dictated(t) if t == "world"));
}

#[test]
fn dictation_silence_injects_nothing() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.0, 1000)]);
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    core.send(Command::StopDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Idle)));
    assert!(world.injector.injected.lock().is_empty());
}

#[test]
fn dictation_copies_to_clipboard_when_paste_is_not_allowed() {
    let mut world = FakeWorld::default();
    world.injector.fail = true;
    *world.audio.mic.lock() = tone(&[(0.0, 300), (0.1, 500), (0.0, 200), (0.3, 500), (0.0, 300)]);
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    core.send(Command::StopDictation);
    wait_for(&rx, 5, |e| {
        matches!(e, Event::Dictation(DictationPhase::Copied { needs: Some(PermissionKind::Accessibility) }))
    });
    assert!(world.injector.injected.lock().is_empty());
    assert_eq!(world.injector.copied.lock().as_slice(), ["hello world"]);
    assert_eq!(core.dictations(10)[0].text, "hello world");
}

#[test]
fn dictation_muffles_other_audio_until_you_stop() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.1, 600)]);
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(world.ducker.calls.lock().as_slice(), ["duck:Muffle"]);
    core.send(Command::StopDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Transcribing)));
    assert_eq!(world.ducker.calls.lock().as_slice(), ["duck:Muffle", "restore"]);
}

#[test]
fn dictation_restores_audio_on_cancel() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.1, 600)]);
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    core.send(Command::CancelDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Idle)));
    assert_eq!(world.ducker.calls.lock().as_slice(), ["duck:Muffle", "restore"]);
}

#[test]
fn dictation_leaves_audio_alone_when_ducking_is_off() {
    let world = FakeWorld::default();
    let mut s = world.settings.load();
    s.dictation_ducking = Ducking::Off;
    world.settings.save(&s).unwrap();
    *world.audio.mic.lock() = tone(&[(0.1, 600)]);
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    core.send(Command::StopDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Transcribing)));
    assert!(world.ducker.calls.lock().is_empty());
}

#[test]
fn dictation_does_not_duck_during_a_meeting() {
    let world = remote_meeting_world();
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Listening)));
    core.send(Command::CancelDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Idle)));
    assert!(world.ducker.calls.lock().is_empty());
}

#[test]
fn dictation_reports_missing_mic_permission() {
    let mut world = FakeWorld::default();
    world.audio.fail_mic = true;
    let (core, rx) = start(&world);
    core.send(Command::StartDictation);
    wait_for(&rx, 5, |e| matches!(e, Event::Dictation(DictationPhase::Error(m)) if m.contains("Microphone")));
}

/// Remote meeting: you speak at 0–1 s; remote voice A at 2–3 s, B at 4–5 s, A again at 6–7 s.
fn remote_meeting_world() -> FakeWorld {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.1, 1000), (0.0, 7000)]);
    *world.audio.system.lock() = Some(tone(&[
        (0.0, 2000),
        (0.1, 1000),
        (0.0, 1000),
        (0.3, 1000),
        (0.0, 1000),
        (0.1, 1000),
        (0.0, 1000),
    ]));
    world
}

#[test]
fn meeting_end_to_end_labels_speakers_and_writes_notes() {
    let world = remote_meeting_world();
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: Some("Weekly sync".into()) });
    let ev = wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    let id = match ev.last().unwrap() {
        Event::MeetingStarted(id) => *id,
        _ => unreachable!(),
    };
    core.send(Command::SetMyNotes { id, text: "ask about export".into() });
    std::thread::sleep(Duration::from_millis(100));
    core.send(Command::EndMeeting);
    let ev = wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    assert!(ev.iter().any(|e| matches!(e, Event::NotesDelta { .. })));
    assert!(ev.iter().any(|e| matches!(e, Event::Pipeline { step, .. } if step == "Detecting speakers")));

    let s = core.session(id).unwrap();
    assert_eq!(s.state, SessionState::Done);
    assert_eq!(s.title, "Weekly sync");
    assert_eq!(s.my_notes, "ask about export");
    assert_eq!(s.duration_ms, 8000);
    let lines: Vec<(String, String)> =
        s.segments.iter().map(|g| (s.speaker_name(g.speaker_id).to_string(), g.text.clone())).collect();
    assert_eq!(
        lines,
        vec![
            ("You".into(), "hello".into()),
            ("Speaker 1".into(), "hello".into()),
            ("Speaker 2".into(), "world".into()),
            ("Speaker 1".into(), "hello".into()),
        ]
    );
    let note = &s.notes[&Template::Summary];
    assert!(note.markdown.contains("## Action items"));
    assert_eq!(note.engine, "canned-standard");
    // The note writer saw the labelled transcript and the user's notes.
    let prompts = world.engines.notes_prompts.lock();
    assert!(prompts[0].user.contains("Speaker 2: world"));
    assert!(prompts[0].user.contains("ask about export"));
    // Library preview comes from the summary.
    assert_eq!(core.library()[0].preview, "We met.");
}

#[test]
fn meeting_without_system_audio_diarizes_the_mic() {
    let world = FakeWorld::default();
    *world.audio.mic.lock() = tone(&[(0.1, 1000), (0.0, 1000), (0.3, 1000), (0.0, 1000)]);
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::EndMeeting);
    let ev = wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    let id = ev.iter().find_map(|e| if let Event::MeetingState { id, .. } = e { Some(*id) } else { None }).unwrap();
    let s = core.session(id).unwrap();
    assert!(s.title.starts_with("Meeting "));
    let names: Vec<&str> = s.segments.iter().map(|g| s.speaker_name(g.speaker_id)).collect();
    assert_eq!(names, vec!["Speaker 1", "Speaker 2"]);
}

#[test]
fn diarizer_failure_still_produces_notes() {
    let mut world = remote_meeting_world();
    world.engines = std::sync::Arc::new(FakeEngines { diarizer_fails: true, ..Default::default() });
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::EndMeeting);
    let ev = wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    assert!(ev.iter().any(|e| matches!(e, Event::Error(m) if m.contains("diarizer exploded"))));
    let id = core.library()[0].id;
    let s = core.session(id).unwrap();
    assert!(s.segments.iter().filter(|g| g.track == TrackKind::System).all(|g| s.speaker_name(g.speaker_id) == "Speaker 1"));
    assert!(s.notes.contains_key(&Template::Summary));
}

#[test]
fn missing_notes_model_skips_notes_with_a_notice() {
    let world = remote_meeting_world();
    world.models.missing.lock().push(Pack::Notes);
    // A factory that respects the model store, like the real one.
    struct Strict(std::sync::Arc<FakeModels>, FakeEngines);
    impl zwisper_core::ports::EngineFactory for Strict {
        fn transcriber(&self, t: Tier) -> zwisper_core::Result<Box<dyn zwisper_core::ports::Transcriber>> { self.1.transcriber(t) }
        fn vad(&self) -> zwisper_core::Result<Box<dyn zwisper_core::ports::VoiceActivity>> { self.1.vad() }
        fn diarizer(&self, t: Tier) -> zwisper_core::Result<Box<dyn zwisper_core::ports::Diarizer>> { self.1.diarizer(t) }
        fn note_writer(&self, t: Tier) -> zwisper_core::Result<Box<dyn zwisper_core::ports::NoteWriter>> {
            let models = self.0.clone();
            let inner = self.1.note_writer(t)?;
            let mut inner = Some(inner);
            Ok(Box::new(zwisper_core::lazy::Lazy::new("strict-notes", move || {
                use zwisper_core::ports::ModelStore;
                models.paths(Pack::Notes, t)?;
                Ok(LazyBox(inner.take().unwrap()))
            })))
        }
    }
    struct LazyBox(Box<dyn zwisper_core::ports::NoteWriter>);
    impl zwisper_core::ports::NoteWriter for LazyBox {
        fn id(&self) -> &'static str { self.0.id() }
        fn generate(&mut self, p: &zwisper_core::ports::Prompt, d: &mut dyn FnMut(&str), c: &zwisper_core::CancelToken) -> zwisper_core::Result<String> { self.0.generate(p, d, c) }
    }
    let mut ports = world.ports();
    ports.engines = std::sync::Arc::new(Strict(world.models.clone(), FakeEngines::default()));
    let core = Core::start(ports).unwrap();
    let rx = core.subscribe();
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::EndMeeting);
    let ev = wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    assert!(ev.iter().any(|e| matches!(e, Event::Notice(m) if m.contains("Notes model"))), "{ev:#?}");
    let s = core.session(core.library()[0].id).unwrap();
    assert!(s.notes.is_empty());
    assert_eq!(s.speakers.len(), 3);
}

#[test]
fn rename_speaker_and_merge() {
    let world = remote_meeting_world();
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::EndMeeting);
    wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    let id = core.library()[0].id;

    core.send(Command::RenameSpeaker { id, speaker: SpeakerId(1), name: "Priya".into() });
    wait_for(&rx, 5, |e| matches!(e, Event::SessionUpdated(s) if s.speaker_name(SpeakerId(1)) == "Priya"));
    // Renaming Speaker 2 to "priya" merges them.
    core.send(Command::RenameSpeaker { id, speaker: SpeakerId(2), name: "priya".into() });
    wait_for(&rx, 5, |e| matches!(e, Event::SessionUpdated(s) if s.speaker(SpeakerId(2)).is_none()));
    let s = core.session(id).unwrap();
    assert!(s.segments.iter().filter(|g| g.track == TrackKind::System).all(|g| g.speaker_id == SpeakerId(1)));
}

#[test]
fn switching_tier_hot_swaps_the_note_writer() {
    let world = remote_meeting_world();
    let (core, rx) = start(&world);
    assert_eq!(core.tier(), Tier::Standard);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::EndMeeting);
    wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    let id = core.library()[0].id;

    core.send(Command::SetTier(Tier::Lite));
    wait_for(&rx, 5, |e| matches!(e, Event::TierChanged(Tier::Lite)));
    core.send(Command::GenerateNotes { id, template: Template::ActionItems, force: false });
    wait_for(&rx, 5, |e| matches!(e, Event::NotesDone { template: Template::ActionItems, .. }));
    let s = core.session(id).unwrap();
    assert_eq!(s.notes[&Template::ActionItems].engine, "canned-lite");
    assert_eq!(s.notes[&Template::Summary].engine, "canned-standard");
    assert_eq!(s.active_template, Some(Template::ActionItems));
    assert_eq!(*world.engines.tiers.lock(), vec![Tier::Standard, Tier::Lite]);
    assert_eq!(world.settings.0.lock().tier, Some(Tier::Lite));
}

#[test]
fn long_meetings_are_condensed_in_parts() {
    let mut world = remote_meeting_world();
    world.engines = std::sync::Arc::new(FakeEngines { notes_budget: Some(6), ..Default::default() });
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    core.send(Command::EndMeeting);
    let ev = wait_for(&rx, 10, |e| matches!(e, Event::MeetingState { state: SessionState::Done, .. }));
    assert!(ev.iter().any(|e| matches!(e, Event::Pipeline { step, .. } if step.starts_with("Reading part 1 of"))));
    let prompts = world.engines.notes_prompts.lock();
    assert!(prompts.len() >= 3, "expected chunk prompts + final, got {}", prompts.len());
    assert!(prompts.last().unwrap().user.contains("Part 1 of"));
}

#[test]
fn discard_deletes_the_meeting() {
    let world = remote_meeting_world();
    let (core, rx) = start(&world);
    core.send(Command::StartMeeting { title: None });
    wait_for(&rx, 5, |e| matches!(e, Event::MeetingStarted(_)));
    assert_eq!(core.library().len(), 1);
    core.send(Command::DiscardMeeting);
    wait_for(&rx, 5, |e| matches!(e, Event::SessionDeleted(_)));
    assert!(core.library().is_empty());
}

#[test]
fn pack_download_reports_progress() {
    let world = FakeWorld::default();
    world.models.missing.lock().push(Pack::Speakers);
    let (core, rx) = start(&world);
    assert!(matches!(core.pack_status(Pack::Speakers), PackStatus::NotDownloaded { .. }));
    core.send(Command::DownloadPack(Pack::Speakers));
    wait_for(&rx, 5, |e| matches!(e, Event::Pack { pack: Pack::Speakers, status: PackStatus::Ready { .. } }));
}

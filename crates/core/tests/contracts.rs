//! The fakes must pass the same contracts as the real adapters.

use zwisper_core::testkit::*;
use zwisper_fakes::*;

#[test]
fn scripted_transcriber_contract() {
    let speech = tone(&[(0.0, 200), (0.1, 500), (0.0, 100), (0.3, 500)]);
    transcriber_contract(&mut ScriptedTranscriber::default(), &speech, Some("hello world"), 0.0);
}

#[test]
fn scripted_diarizer_contract() {
    let audio = tone(&[(0.1, 1000), (0.0, 500), (0.3, 1000)]);
    diarizer_contract(&mut ScriptedDiarizer::default(), &audio, 2);
}

#[test]
fn canned_note_writer_contract() {
    note_writer_contract(&mut CannedNoteWriter::default());
}

#[test]
fn in_memory_repo_contract() {
    repository_contract(&InMemoryRepo::default());
}

//! Reusable contract test suites. Every adapter for a port must pass its
//! port's contract; so do the fakes. Call these from each adapter's tests.

use crate::cancel::CancelToken;
use crate::domain::*;
use crate::ports::*;

/// Word error rate between a reference and a hypothesis (case/punctuation-insensitive).
pub fn wer(reference: &str, hypothesis: &str) -> f64 {
    let norm = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(String::from)
            .collect()
    };
    let r = norm(reference);
    let h = norm(hypothesis);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for i in 1..=r.len() {
        let mut cur = vec![i; h.len() + 1];
        for j in 1..=h.len() {
            let sub = prev[j - 1] + usize::from(r[i - 1] != h[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        prev = cur;
    }
    prev[h.len()] as f64 / r.len() as f64
}

/// Transcriber contract.
/// * silence → no non-empty text
/// * segments ordered, words inside their segment, timestamps monotonic
/// * if a reference is given, WER ≤ `max_wer`
pub fn transcriber_contract(t: &mut dyn Transcriber, speech: &[f32], reference: Option<&str>, max_wer: f64) {
    let silence = vec![0.0f32; SAMPLE_RATE as usize * 2];
    let out = t.transcribe(&silence).expect("transcribe silence");
    let text: String = out.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ");
    let text = text.trim_matches(|c: char| !c.is_alphanumeric());
    assert!(text.is_empty(), "{}: silence produced text {text:?}", t.id());

    let out = t.transcribe(speech).expect("transcribe speech");
    assert!(!out.is_empty(), "{}: speech produced no segments", t.id());
    let dur = samples_to_ms(speech.len()) + 500;
    let mut last_start = 0;
    for s in &out {
        assert!(s.start_ms <= s.end_ms, "{}: segment ends before it starts", t.id());
        assert!(s.start_ms >= last_start, "{}: segments out of order", t.id());
        assert!(s.end_ms <= dur, "{}: segment beyond audio end", t.id());
        last_start = s.start_ms;
        let mut wl = 0;
        for w in &s.words {
            assert!(w.start_ms >= wl, "{}: word timestamps go backwards", t.id());
            assert!(w.start_ms + 50 >= s.start_ms && w.end_ms <= s.end_ms + 50, "{}: word outside segment", t.id());
            wl = w.start_ms;
        }
    }
    if let Some(r) = reference {
        let hyp: String = out.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
        let e = wer(r, &hyp);
        assert!(e <= max_wer, "{}: WER {e:.3} > {max_wer} (hyp: {hyp:?})", t.id());
    }
}

/// Diarizer contract: turns ordered by start, non-negative, within audio,
/// and at least `min_speakers` distinct labels on the fixture.
pub fn diarizer_contract(d: &mut dyn Diarizer, audio: &[f32], min_speakers: usize) {
    let turns = d.diarize(audio).expect("diarize");
    let dur = samples_to_ms(audio.len()) + 500;
    let mut last = 0;
    for t in &turns {
        assert!(t.start_ms < t.end_ms, "{}: empty turn", d.id());
        assert!(t.start_ms >= last, "{}: turns not sorted", d.id());
        assert!(t.end_ms <= dur, "{}: turn beyond audio", d.id());
        last = t.start_ms;
    }
    let mut labels: Vec<u32> = turns.iter().map(|t| t.speaker).collect();
    labels.sort();
    labels.dedup();
    assert!(labels.len() >= min_speakers, "{}: found {} speakers, want ≥ {}", d.id(), labels.len(), min_speakers);
}

/// NoteWriter contract: streams deltas whose concatenation equals the result,
/// honours cancellation, and produces non-empty output.
pub fn note_writer_contract(w: &mut dyn NoteWriter) {
    let prompt = Prompt {
        system: "You are a concise assistant. Reply in Markdown.".into(),
        user: "Write a to-do list with exactly two items: buy milk, call Sam. Use '- [ ]' checkboxes.".into(),
        max_tokens: 64,
        temperature: 0.2,
    };
    let mut streamed = String::new();
    let out = w
        .generate(&prompt, &mut |d| streamed.push_str(d), &CancelToken::new())
        .expect("generate");
    assert!(!out.trim().is_empty(), "{}: empty output", w.id());
    assert_eq!(streamed.trim(), out.trim(), "{}: deltas don't add up to the result", w.id());

    let cancel = CancelToken::new();
    cancel.cancel();
    let r = w.generate(&prompt, &mut |_| {}, &cancel);
    assert!(matches!(r, Err(crate::Error::Cancelled)), "{}: ignored cancellation", w.id());
}

/// SessionRepository contract.
pub fn repository_contract(repo: &dyn SessionRepository) {
    let now = chrono::Utc::now();
    let mut s = Session::new("Contract test", now);
    s.segments.push(TranscriptSegment {
        id: 0,
        track: TrackKind::Mic,
        speaker_id: SpeakerId::YOU,
        start_ms: 0,
        end_ms: 1000,
        text: "hello".into(),
        words: vec![],
        edited: false,
    });
    repo.save(&s).unwrap();
    assert_eq!(repo.load(s.id).unwrap(), s);
    assert!(repo.list().unwrap().iter().any(|x| x.id == s.id));
    s.title = "Renamed".into();
    repo.save(&s).unwrap();
    assert_eq!(repo.load(s.id).unwrap().title, "Renamed");
    repo.delete(s.id).unwrap();
    assert!(repo.load(s.id).is_err());

    let d = Dictation { at: now, text: "dictated".into(), duration_ms: 900 };
    repo.append_dictation(&d).unwrap();
    assert!(repo.dictations(10).unwrap().iter().any(|x| x.text == "dictated"));
    repo.clear_dictations().unwrap();
    assert!(repo.dictations(10).unwrap().is_empty());
}

/// TemplateCatalog contract: every template renders a prompt mentioning the
/// transcript and user notes, and validates its own canonical example.
pub fn template_contract(c: &dyn TemplateCatalog, examples: &[(Template, &str)]) {
    let input = NoteInput {
        title: "Weekly sync".into(),
        date: "Monday".into(),
        transcript: "[0:01] You: ship thursday\n".into(),
        my_notes: "ask about export".into(),
        speakers: vec!["You".into()],
    };
    for t in Template::ALL {
        let p = c.prompt(t, &input);
        assert!(p.user.contains("ship thursday"), "{t:?}: transcript missing from prompt");
        assert!(p.user.contains("ask about export"), "{t:?}: my notes missing from prompt");
        assert!(p.max_tokens > 0);
    }
    for (t, ex) in examples {
        assert_eq!(c.validate(*t, ex), Ok(()), "{t:?}: canonical example failed validation");
        assert!(c.validate(*t, "").is_err(), "{t:?}: empty output passed validation");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wer_basics() {
        assert_eq!(super::wer("Hello world", "hello, world!"), 0.0);
        assert_eq!(super::wer("a b c d", "a x c d"), 0.25);
    }
}

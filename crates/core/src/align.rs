//! `MaxOverlapAligner`: assigns every word to the diarizer turn it overlaps most,
//! then regroups consecutive same-speaker words into paragraphs.

use crate::domain::*;
use crate::ports::SpeakerAligner;

#[derive(Debug, Default, Clone)]
pub struct MaxOverlapAligner;

fn overlap(a0: Ms, a1: Ms, b0: Ms, b1: Ms) -> Ms {
    a1.min(b1).saturating_sub(a0.max(b0))
}

fn pick_turn(turns: &[SpeakerTurn], start: Ms, end: Ms) -> Option<u32> {
    if turns.is_empty() {
        return None;
    }
    let best = turns
        .iter()
        .map(|t| (overlap(start, end.max(start + 1), t.start_ms, t.end_ms), t))
        .max_by_key(|(o, _)| *o)
        .unwrap();
    if best.0 > 0 {
        return Some(best.1.speaker);
    }
    // No overlap (e.g. word in a gap): nearest turn by distance.
    let mid = (start + end) / 2;
    turns
        .iter()
        .min_by_key(|t| {
            if mid < t.start_ms {
                t.start_ms - mid
            } else {
                mid.saturating_sub(t.end_ms)
            }
        })
        .map(|t| t.speaker)
}

fn join_words(words: &[Word]) -> String {
    let mut s = String::new();
    for w in words {
        let t = w.text.trim();
        if t.is_empty() {
            continue;
        }
        let no_space = t.starts_with(['.', ',', '!', '?', ';', ':', '\''])
            || s.is_empty();
        if !no_space {
            s.push(' ');
        }
        s.push_str(t);
    }
    s
}

impl SpeakerAligner for MaxOverlapAligner {
    fn align(
        &self,
        track: TrackKind,
        segments: &[TranscriptSegment],
        turns: &[SpeakerTurn],
        first_id: u32,
    ) -> Vec<TranscriptSegment> {
        // Diarizer labels are arbitrary; renumber by first appearance.
        let mut order: Vec<u32> = Vec::new();
        let mut sorted_turns = turns.to_vec();
        sorted_turns.sort_by_key(|t| t.start_ms);
        for t in &sorted_turns {
            if !order.contains(&t.speaker) {
                order.push(t.speaker);
            }
        }
        let map_id = |label: u32| {
            SpeakerId(first_id + order.iter().position(|&l| l == label).unwrap_or(0) as u32)
        };

        let mut out: Vec<TranscriptSegment> = Vec::new();
        for seg in segments.iter().filter(|s| s.track == track) {
            if seg.words.is_empty() {
                // No word timings: attribute the whole segment.
                let sp = pick_turn(&sorted_turns, seg.start_ms, seg.end_ms)
                    .map(map_id)
                    .unwrap_or(SpeakerId(first_id));
                out.push(TranscriptSegment {
                    speaker_id: sp,
                    ..seg.clone()
                });
                continue;
            }
            // Split the segment's words into runs by speaker.
            let mut run: Vec<Word> = Vec::new();
            let mut run_sp: Option<SpeakerId> = None;
            let flush = |run: &mut Vec<Word>, sp: Option<SpeakerId>, out: &mut Vec<TranscriptSegment>| {
                if run.is_empty() {
                    return;
                }
                out.push(TranscriptSegment {
                    id: 0,
                    track,
                    speaker_id: sp.unwrap_or(SpeakerId(first_id)),
                    start_ms: run.first().unwrap().start_ms,
                    end_ms: run.last().unwrap().end_ms,
                    text: join_words(run),
                    words: std::mem::take(run),
                    edited: false,
                });
            };
            for w in &seg.words {
                let sp = pick_turn(&sorted_turns, w.start_ms, w.end_ms).map(map_id);
                if run_sp.is_some() && sp != run_sp {
                    flush(&mut run, run_sp, &mut out);
                }
                run_sp = sp.or(run_sp);
                run.push(w.clone());
            }
            flush(&mut run, run_sp, &mut out);
        }

        // Merge adjacent paragraphs of the same speaker separated by < 1.5 s.
        let mut merged: Vec<TranscriptSegment> = Vec::new();
        for s in out {
            if let Some(last) = merged.last_mut() {
                if last.speaker_id == s.speaker_id && s.start_ms.saturating_sub(last.end_ms) < 1500 {
                    last.end_ms = s.end_ms;
                    last.text = format!("{} {}", last.text.trim_end(), s.text.trim_start());
                    last.words.extend(s.words);
                    continue;
                }
            }
            merged.push(s);
        }
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(t: &str, a: Ms, b: Ms) -> Word {
        Word { text: t.into(), start_ms: a, end_ms: b }
    }

    #[test]
    fn splits_segment_at_speaker_change() {
        let seg = TranscriptSegment {
            id: 0,
            track: TrackKind::System,
            speaker_id: SpeakerId::UNKNOWN,
            start_ms: 0,
            end_ms: 4000,
            text: "hi there. yes hello".into(),
            words: vec![w("hi", 0, 400), w("there.", 400, 900), w("yes", 2100, 2400), w("hello", 2400, 3000)],
            edited: false,
        };
        let turns = [
            SpeakerTurn { start_ms: 0, end_ms: 1000, speaker: 7 },
            SpeakerTurn { start_ms: 2000, end_ms: 3200, speaker: 3 },
        ];
        let out = MaxOverlapAligner.align(TrackKind::System, &[seg], &turns, 1);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].speaker_id, SpeakerId(1));
        assert_eq!(out[0].text, "hi there.");
        assert_eq!(out[1].speaker_id, SpeakerId(2));
        assert_eq!(out[1].text, "yes hello");
    }

    #[test]
    fn word_in_gap_goes_to_nearest_turn() {
        let seg = TranscriptSegment {
            id: 0, track: TrackKind::System, speaker_id: SpeakerId::UNKNOWN,
            start_ms: 0, end_ms: 5000, text: String::new(),
            words: vec![w("a", 100, 200), w("b", 4900, 5000)], edited: false,
        };
        let turns = [
            SpeakerTurn { start_ms: 0, end_ms: 300, speaker: 0 },
            SpeakerTurn { start_ms: 4000, end_ms: 4500, speaker: 1 },
        ];
        let out = MaxOverlapAligner.align(TrackKind::System, &[seg], &turns, 1);
        assert_eq!(out.iter().map(|s| s.speaker_id.0).collect::<Vec<_>>(), vec![1, 2]);
    }
}

//! `TextSimilarityEcho`: drop mic paragraphs that duplicate overlapping system
//! audio (remote voices picked up by the mic when not using headphones).

use crate::domain::TranscriptSegment;
use crate::ports::EchoFilter;

#[derive(Debug, Clone)]
pub struct TextSimilarityEcho {
    pub threshold: f64,
    /// Time slack when looking for overlapping system speech.
    pub slack_ms: u64,
}

impl Default for TextSimilarityEcho {
    fn default() -> Self {
        TextSimilarityEcho { threshold: 0.8, slack_ms: 500 }
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

impl EchoFilter for TextSimilarityEcho {
    fn filter(&self, mic: Vec<TranscriptSegment>, system: &[TranscriptSegment]) -> Vec<TranscriptSegment> {
        mic.into_iter()
            .filter(|m| {
                let overlapping: Vec<&str> = system
                    .iter()
                    .filter(|s| {
                        s.start_ms <= m.end_ms + self.slack_ms && m.start_ms <= s.end_ms + self.slack_ms
                    })
                    .map(|s| s.text.as_str())
                    .collect();
                if overlapping.is_empty() {
                    return true;
                }
                let mic_n = normalize(&m.text);
                if mic_n.is_empty() {
                    return false;
                }
                let sys_n = normalize(&overlapping.join(" "));
                // Mic text contained in system text, or near-identical.
                let sim = if sys_n.contains(&mic_n) {
                    1.0
                } else {
                    strsim::normalized_levenshtein(&mic_n, &sys_n)
                };
                sim < self.threshold
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::*;

    fn seg(track: TrackKind, a: Ms, b: Ms, t: &str) -> TranscriptSegment {
        TranscriptSegment { id: 0, track, speaker_id: SpeakerId::YOU, start_ms: a, end_ms: b, text: t.into(), words: vec![], edited: false }
    }

    #[test]
    fn drops_echo_keeps_real_speech() {
        let sys = vec![seg(TrackKind::System, 0, 3000, "We should ship on Thursday.")];
        let mic = vec![
            seg(TrackKind::Mic, 100, 3100, "we should ship on thursday"),
            seg(TrackKind::Mic, 5000, 7000, "Sounds good to me."),
        ];
        let out = TextSimilarityEcho::default().filter(mic, &sys);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].text, "Sounds good to me.");
    }
}

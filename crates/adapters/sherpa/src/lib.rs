//! Audio engines on [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (one ONNX Runtime
//! for every audio model):
//!
//! * [`ParakeetTranscriber`] — `Transcriber`, NeMo Parakeet TDT 0.6B v2 (int8), word timestamps.
//! * [`SileroVad`] — `VoiceActivity`, Silero VAD v5.
//! * [`SherpaDiarizer`] — `Diarizer`, pyannote segmentation-3.0 + WeSpeaker ResNet34 + clustering.
//!
//! All constructors take the pack's [`ModelPaths`] (logical names from the model manifest:
//! "encoder", "decoder", "joiner", "tokens", "vad", "segmentation", "embedding").
//!
//! # Native linking
//!
//! The `sherpa-onnx-sys` build script links the **prebuilt static** sherpa-onnx + ONNX Runtime
//! archives by default (cargo feature `static`, which we use). Nothing has to be bundled
//! next to the binary: on macOS arm64 the final executable only additionally links the system
//! `libc++` and the Foundation/CoreML frameworks it pulls in. The archive it needs is
//! `sherpa-onnx-v<version>-osx-arm64-static-lib.tar.bz2` (Linux:
//! `...-linux-x64-static-lib.tar.bz2`), fetched at build time from GitHub releases.
//!
//! Build-time environment variables understood by `sherpa-onnx-sys`:
//! * `SHERPA_ONNX_ARCHIVE_DIR=/dir` — directory holding the release archive already downloaded
//!   (use this in CI / behind proxies; its built-in downloader uses rustls with the Mozilla roots
//!   and fails behind TLS-intercepting proxies).
//! * `SHERPA_ONNX_LIB_DIR=/dir` — point at an already-extracted `lib/` directory instead.
//!
//! If we ever switch to the `shared` feature, `libsherpa-onnx-c-api.dylib` and
//! `libonnxruntime.<ver>.dylib` must be bundled in `Contents/Frameworks` (rpath
//! `@loader_path`/`@executable_path/../Frameworks`). Static linking avoids that.

use sherpa_onnx::{
    FastClusteringConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeakerDiarization,
    OfflineSpeakerDiarizationConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
};
use zwisper_core::domain::{samples_to_ms, Ms, Segment, SpeakerTurn, SpeechChunk, Word, SAMPLE_RATE};
use zwisper_core::ports::{Diarizer, EngineId, ModelPaths, Transcriber, VoiceActivity};
use zwisper_core::{Error, Result};

/// Threads for ONNX Runtime: min(4, cores).
fn num_threads() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get().min(4))
        .unwrap_or(2) as i32
}

fn path_str(paths: &ModelPaths, name: &str) -> Result<String> {
    let p = paths.get(name)?;
    if !p.exists() {
        return Err(Error::NotFound(format!("model file '{name}' at {}", p.display())));
    }
    Ok(p.to_string_lossy().into_owned())
}

// ───────────────────────────── transcriber ─────────────────────────────

/// Splits a transcript into separate segments where words are this far apart.
const SEGMENT_GAP_MS: Ms = 1_000;
/// Duration assumed for the last token when the model reports none.
const FALLBACK_TOKEN_MS: Ms = 80;

/// NeMo Parakeet TDT 0.6B v2 (int8) via sherpa-onnx's offline transducer recognizer.
pub struct ParakeetTranscriber {
    recognizer: OfflineRecognizer,
}

impl ParakeetTranscriber {
    pub const ID: EngineId = "parakeet-tdt-0.6b-v2";

    pub fn load(paths: &ModelPaths) -> Result<Self> {
        let mut config = OfflineRecognizerConfig::default();
        config.model_config.transducer.encoder = Some(path_str(paths, "encoder")?);
        config.model_config.transducer.decoder = Some(path_str(paths, "decoder")?);
        config.model_config.transducer.joiner = Some(path_str(paths, "joiner")?);
        config.model_config.tokens = Some(path_str(paths, "tokens")?);
        config.model_config.model_type = Some("nemo_transducer".into());
        config.model_config.num_threads = num_threads();
        config.model_config.provider = Some("cpu".into());
        config.decoding_method = Some("greedy_search".into());
        let recognizer = OfflineRecognizer::create(&config)
            .ok_or_else(|| Error::Engine("sherpa-onnx: failed to create Parakeet recognizer".into()))?;
        Ok(ParakeetTranscriber { recognizer })
    }
}

impl Transcriber for ParakeetTranscriber {
    fn id(&self) -> EngineId {
        Self::ID
    }

    fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>> {
        // Digital silence: skip the model entirely.
        if audio.iter().all(|x| x.abs() < 1e-4) {
            return Ok(Vec::new());
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, audio);
        self.recognizer.decode(&stream);
        let r = stream
            .get_result()
            .ok_or_else(|| Error::Engine("sherpa-onnx: no recognition result".into()))?;
        let audio_ms = samples_to_ms(audio.len());
        let tokens: Vec<Token> = match &r.timestamps {
            Some(ts) if ts.len() == r.tokens.len() => r
                .tokens
                .iter()
                .enumerate()
                .map(|(i, t)| Token {
                    text: t.clone(),
                    start_s: ts[i],
                    dur_s: r.durations.as_ref().and_then(|d| d.get(i).copied()),
                })
                .collect(),
            _ => {
                // No timestamps: one segment spanning the input, no words.
                let text = r.text.trim().to_string();
                if text.is_empty() {
                    return Ok(Vec::new());
                }
                return Ok(vec![Segment { start_ms: 0, end_ms: audio_ms, text, words: vec![] }]);
            }
        };
        let words = merge_tokens(&tokens, audio_ms);
        Ok(split_segments(words, SEGMENT_GAP_MS))
    }
}

/// One BPE token with its timing in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub text: String,
    pub start_s: f32,
    pub dur_s: Option<f32>,
}

fn is_word_start(t: &str) -> bool {
    t.starts_with('▁') || t.starts_with(' ')
}

fn secs_to_ms(s: f32) -> Ms {
    (s.max(0.0) * 1000.0).round() as Ms
}

/// Merge BPE tokens into words: a token beginning with '▁' (or a space) starts a new word;
/// other tokens (sub-words, punctuation) attach to the current word.
/// Word end = end of its last token (start + duration when known), never past the next
/// word's start; without durations it is the next word's start (or an estimate for the last).
pub fn merge_tokens(tokens: &[Token], audio_ms: Ms) -> Vec<Word> {
    struct Acc {
        text: String,
        start: Ms,
        last_tok_end: Option<Ms>,
        last_tok_start: Ms,
    }
    let mut accs: Vec<Acc> = Vec::new();
    for t in tokens {
        let start = secs_to_ms(t.start_s);
        let end = t.dur_s.filter(|d| *d > 0.0).map(|d| start + secs_to_ms(d));
        let piece = t.text.trim_start_matches(['▁', ' ']);
        match accs.last_mut() {
            Some(a) if !is_word_start(&t.text) => {
                a.text.push_str(piece);
                a.last_tok_end = end.or(a.last_tok_end);
                a.last_tok_start = start.max(a.last_tok_start);
            }
            _ => accs.push(Acc { text: piece.to_string(), start, last_tok_end: end, last_tok_start: start }),
        }
    }
    let mut words: Vec<Word> = Vec::with_capacity(accs.len());
    let mut prev_start = 0;
    for i in 0..accs.len() {
        let a = &accs[i];
        let start = a.start.max(prev_start).min(audio_ms);
        let next_start = accs.get(i + 1).map(|n| n.start);
        let own_end = a.last_tok_end.unwrap_or(a.last_tok_start + FALLBACK_TOKEN_MS);
        let end = match next_start {
            Some(n) if a.last_tok_end.is_some() => own_end.min(n),
            Some(n) => n,
            None => own_end,
        };
        let end = end.min(audio_ms).max(start);
        prev_start = start;
        if a.text.trim().is_empty() {
            continue;
        }
        words.push(Word { text: a.text.trim().to_string(), start_ms: start, end_ms: end });
    }
    words
}

/// Group words into segments, starting a new one at gaps > `gap_ms`.
pub fn split_segments(words: Vec<Word>, gap_ms: Ms) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut cur: Vec<Word> = Vec::new();
    let finish = |cur: &mut Vec<Word>, out: &mut Vec<Segment>| {
        if cur.is_empty() {
            return;
        }
        let words = std::mem::take(cur);
        let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        out.push(Segment {
            start_ms: words[0].start_ms,
            end_ms: words.iter().map(|w| w.end_ms).max().unwrap_or(words[0].end_ms),
            text,
            words,
        });
    };
    for w in words {
        if let Some(last) = cur.last() {
            if w.start_ms > last.end_ms + gap_ms {
                finish(&mut cur, &mut out);
            }
        }
        cur.push(w);
    }
    finish(&mut cur, &mut out);
    out
}

// ───────────────────────────── VAD ─────────────────────────────

/// Audio kept before each detected speech start: Silero reports onsets ~150 ms late,
/// which would clip the first phoneme.
const PREROLL_SAMPLES: usize = SAMPLE_RATE as usize / 4;
/// How much recent input is retained for the pre-roll: must exceed the longest chunk
/// (max_speech 20 s) plus the silence that ends it.
const HISTORY_SAMPLES: usize = SAMPLE_RATE as usize * 25;

/// Silero VAD v5 via sherpa-onnx.
pub struct SileroVad {
    vad: VoiceActivityDetector,
    /// Most recent input; `history[0]` is absolute sample `history_start`.
    history: Vec<f32>,
    history_start: usize,
    /// Absolute sample index just past the last emitted chunk.
    last_end: usize,
}

impl SileroVad {
    pub const ID: EngineId = "silero-vad-v5";

    pub fn load(paths: &ModelPaths) -> Result<Self> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(path_str(paths, "vad")?),
                threshold: 0.5,
                min_silence_duration: 0.5,
                min_speech_duration: 0.25,
                window_size: 512,
                max_speech_duration: 20.0,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            debug: false,
            ..Default::default()
        };
        let vad = VoiceActivityDetector::create(&config, 60.0)
            .ok_or_else(|| Error::Engine("sherpa-onnx: failed to create Silero VAD".into()))?;
        Ok(SileroVad { vad, history: Vec::new(), history_start: 0, last_end: 0 })
    }

    fn drain(&mut self) -> Vec<SpeechChunk> {
        let mut out = Vec::new();
        while let Some(seg) = self.vad.front() {
            let start = seg.start().max(0) as usize;
            // Pre-roll from our own history, never overlapping the previous chunk.
            let pre_from = start
                .saturating_sub(PREROLL_SAMPLES)
                .max(self.last_end)
                .max(self.history_start)
                .min(start);
            let pre_to = start.min(self.history_start + self.history.len());
            let mut samples = Vec::with_capacity(seg.n().max(0) as usize + PREROLL_SAMPLES);
            if pre_from < pre_to {
                samples.extend_from_slice(&self.history[pre_from - self.history_start..pre_to - self.history_start]);
            }
            let pre_from = pre_to - samples.len();
            samples.extend_from_slice(seg.samples());
            self.last_end = start + seg.samples().len();
            out.push(SpeechChunk { start_ms: samples_to_ms(pre_from), samples });
            self.vad.pop();
        }
        out
    }

    fn remember(&mut self, frames: &[f32]) {
        self.history.extend_from_slice(frames);
        if self.history.len() > 2 * HISTORY_SAMPLES {
            let drop = self.history.len() - HISTORY_SAMPLES;
            self.history.drain(..drop);
            self.history_start += drop;
        }
    }
}

impl VoiceActivity for SileroVad {
    fn id(&self) -> EngineId {
        Self::ID
    }
    fn accept(&mut self, frames: &[f32]) -> Result<Vec<SpeechChunk>> {
        self.remember(frames);
        self.vad.accept_waveform(frames);
        Ok(self.drain())
    }
    fn flush(&mut self) -> Result<Vec<SpeechChunk>> {
        self.vad.flush();
        Ok(self.drain())
    }
    fn reset(&mut self) {
        self.vad.reset();
        self.history.clear();
        self.history_start = 0;
        self.last_end = 0;
    }
}

// ───────────────────────────── diarizer ─────────────────────────────

/// Default cosine-distance threshold for agglomerative clustering (lower = more speakers).
pub const DEFAULT_CLUSTER_THRESHOLD: f32 = 0.5;

/// pyannote segmentation-3.0 + WeSpeaker ResNet34 embeddings + fast clustering,
/// speaker count found automatically from a distance threshold.
pub struct SherpaDiarizer {
    sd: OfflineSpeakerDiarization,
}

impl SherpaDiarizer {
    pub const ID: EngineId = "pyannote-seg3+wespeaker-r34";

    pub fn load(paths: &ModelPaths) -> Result<Self> {
        Self::with_threshold(paths, DEFAULT_CLUSTER_THRESHOLD)
    }

    /// Like [`load`](Self::load) with a custom clustering threshold (for tuning).
    pub fn with_threshold(paths: &ModelPaths, threshold: f32) -> Result<Self> {
        let mut config = OfflineSpeakerDiarizationConfig::default();
        config.segmentation.pyannote.model = Some(path_str(paths, "segmentation")?);
        config.segmentation.num_threads = num_threads();
        config.segmentation.provider = Some("cpu".into());
        config.embedding.model = Some(path_str(paths, "embedding")?);
        config.embedding.num_threads = num_threads();
        config.embedding.provider = Some("cpu".into());
        config.clustering = FastClusteringConfig { num_clusters: -1, threshold, compute_confidence: false };
        let sd = OfflineSpeakerDiarization::create(&config)
            .ok_or_else(|| Error::Engine("sherpa-onnx: failed to create speaker diarization".into()))?;
        if sd.sample_rate() != SAMPLE_RATE as i32 {
            return Err(Error::Engine(format!("diarizer expects {} Hz audio", sd.sample_rate())));
        }
        Ok(SherpaDiarizer { sd })
    }
}

impl Diarizer for SherpaDiarizer {
    fn id(&self) -> EngineId {
        Self::ID
    }
    fn diarize(&mut self, audio: &[f32]) -> Result<Vec<SpeakerTurn>> {
        // pyannote needs at least one 10 s window worth of context to be meaningful,
        // but handles shorter input; skip only empty input.
        if audio.is_empty() {
            return Ok(Vec::new());
        }
        let r = self
            .sd
            .process(audio)
            .ok_or_else(|| Error::Engine("sherpa-onnx: diarization failed".into()))?;
        let dur = samples_to_ms(audio.len());
        let mut turns: Vec<SpeakerTurn> = r
            .sort_by_start_time()
            .into_iter()
            .filter(|s| s.speaker >= 0)
            .map(|s| SpeakerTurn {
                start_ms: secs_to_ms(s.start).min(dur),
                end_ms: secs_to_ms(s.end).min(dur),
                speaker: s.speaker as u32,
            })
            .filter(|t| t.start_ms < t.end_ms)
            .collect();
        turns.sort_by_key(|t| (t.start_ms, t.end_ms));
        Ok(turns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(t: &str, s: f32, d: Option<f32>) -> Token {
        Token { text: t.into(), start_s: s, dur_s: d }
    }

    #[test]
    fn merges_bpe_tokens_into_words() {
        let toks = [
            tok("▁Hel", 0.10, Some(0.08)),
            tok("lo", 0.18, Some(0.08)),
            tok(",", 0.26, Some(0.04)),
            tok("▁world", 0.40, Some(0.24)),
            tok(".", 0.64, Some(0.04)),
        ];
        let w = merge_tokens(&toks, 10_000);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], Word { text: "Hello,".into(), start_ms: 100, end_ms: 300 });
        assert_eq!(w[1], Word { text: "world.".into(), start_ms: 400, end_ms: 680 });
    }

    #[test]
    fn merges_without_durations_and_space_prefix() {
        let toks = [tok(" I", 0.0, None), tok(" am", 0.3, None), tok(" here", 0.6, None)];
        let w = merge_tokens(&toks, 700);
        let t: Vec<_> = w.iter().map(|w| (w.text.as_str(), w.start_ms, w.end_ms)).collect();
        assert_eq!(t, vec![("I", 0, 300), ("am", 300, 600), ("here", 600, 680)]);
        // Clamped to audio end.
        let w = merge_tokens(&toks, 650);
        assert_eq!(w[2].end_ms, 650);
    }

    #[test]
    fn leading_subword_without_marker_starts_a_word() {
        let w = merge_tokens(&[tok("ok", 0.0, Some(0.1)), tok("ay", 0.1, Some(0.1))], 1000);
        assert_eq!(w, vec![Word { text: "okay".into(), start_ms: 0, end_ms: 200 }]);
    }

    #[test]
    fn splits_segments_at_long_gaps() {
        let w = |t: &str, s, e| Word { text: t.into(), start_ms: s, end_ms: e };
        let segs = split_segments(vec![w("a", 0, 200), w("b", 300, 500), w("c", 2000, 2300)], 1000);
        assert_eq!(segs.len(), 2);
        assert_eq!((segs[0].start_ms, segs[0].end_ms, segs[0].text.as_str()), (0, 500, "a b"));
        assert_eq!((segs[1].start_ms, segs[1].end_ms, segs[1].text.as_str()), (2000, 2300, "c"));
        assert!(split_segments(vec![], 1000).is_empty());
    }
}

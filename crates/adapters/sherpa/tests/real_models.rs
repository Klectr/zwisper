//! Real-model tests. They download the Speech and Speakers packs into
//! `/home/user/zwisper-models` (override with `ZWISPER_MODELS`) on first run.
//! Run with `cargo test -p zwisper-sherpa -- --ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::time::Instant;
use zwisper_core::domain::{samples_to_ms, Pack, Tier, SAMPLE_RATE};
use zwisper_core::dsp::Resampler;
use zwisper_core::ports::{Diarizer, ModelPaths, ModelStore, Transcriber, VoiceActivity};
use zwisper_core::testkit::{diarizer_contract, transcriber_contract, wer};
use zwisper_core::CancelToken;
use zwisper_sherpa::{ParakeetTranscriber, SherpaDiarizer, SileroVad};

/// Reference transcript of `parakeet-0.wav` (test_wavs/0.wav of the sherpa-onnx Parakeet package).
const REFERENCE_0: &str =
    "Well, I don't wish to see it any more, observed Phoebe, turning away her eyes. It is certainly very like the old portrait.";

fn models_root() -> PathBuf {
    std::env::var_os("ZWISPER_MODELS").map(PathBuf::from).unwrap_or_else(|| "/home/user/zwisper-models".into())
}

fn pack(p: Pack) -> ModelPaths {
    let store = zwisper_models_http::HttpModelStore::new(models_root());
    if store.paths(p, Tier::Standard).is_err() {
        store
            .download(p, Tier::Standard, &mut |_, _| {}, &CancelToken::new())
            .expect("download pack");
    }
    store.paths(p, Tier::Standard).expect("pack paths")
}

fn fixture(name: &str) -> Vec<f32> {
    read_wav(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name))
}

/// Reads any PCM/float WAV as 16 kHz mono f32.
fn read_wav(path: &Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).expect("open wav");
    let spec = r.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => r.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            r.samples::<i32>().map(|s| s.unwrap() as f32 / scale).collect()
        }
    };
    let mono = zwisper_core::dsp::downmix(&interleaved, spec.channels as usize);
    Resampler::new(spec.sample_rate).process(&mono)
}

fn rss_mb() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let l = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    l.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kb| kb / 1024)
}

#[test]
#[ignore = "downloads ~660 MB of models"]
fn parakeet_contract_and_speed() {
    let paths = pack(Pack::Speech);
    let t0 = Instant::now();
    let mut t = ParakeetTranscriber::load(&paths).expect("load");
    println!("load: {:.2?}, RSS after load: {:?} MB", t0.elapsed(), rss_mb());

    let speech = fixture("parakeet-0.wav");
    transcriber_contract(&mut t, &speech, Some(REFERENCE_0), 0.1);

    // Speed (warm).
    let secs = speech.len() as f64 / SAMPLE_RATE as f64;
    let t1 = Instant::now();
    let segs = t.transcribe(&speech).unwrap();
    let el = t1.elapsed().as_secs_f64();
    println!("audio {secs:.2}s, decode {el:.3}s, RTF {:.3} ({:.1}x realtime), RSS {:?} MB", el / secs, secs / el, rss_mb());
    for s in &segs {
        println!("[{}–{} ms] {}", s.start_ms, s.end_ms, s.text);
        for w in &s.words {
            println!("    {:>5}–{:>5} {}", w.start_ms, w.end_ms, w.text);
        }
    }
    let hyp: String = segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ");
    println!("WER {:.3}", wer(REFERENCE_0, &hyp));

    // Longer input: the fixture with a 3 s pause in the middle → split into two segments,
    // second segment's words offset by the pause.
    let mut long = speech.clone();
    long.extend(std::iter::repeat(0.0).take(3 * SAMPLE_RATE as usize));
    long.extend_from_slice(&speech);
    let segs = t.transcribe(&long).unwrap();
    for s in &segs {
        println!("[{}–{} ms] {}", s.start_ms, s.end_ms, s.text);
    }
    assert!(segs.len() >= 2, "expected a split at the pause");
    let second = samples_to_ms(speech.len()) + 3000;
    assert!(segs.last().unwrap().start_ms + 500 >= second, "second segment starts too early");
}

#[test]
#[ignore = "downloads ~660 MB of models"]
fn silero_vad_splits_at_pauses() {
    let paths = pack(Pack::Speech);
    let mut vad = SileroVad::load(&paths).expect("load vad");
    let speech = fixture("parakeet-0.wav");
    let gap = 2 * SAMPLE_RATE as usize;
    let mut audio = vec![0.0f32; SAMPLE_RATE as usize]; // 1 s lead-in
    audio.extend_from_slice(&speech);
    audio.extend(std::iter::repeat(0.0).take(gap));
    audio.extend_from_slice(&speech);
    audio.extend(std::iter::repeat(0.0).take(SAMPLE_RATE as usize / 4)); // short tail: needs flush

    for round in 0..2 {
        let mut chunks = Vec::new();
        for frames in audio.chunks(1600) {
            chunks.extend(vad.accept(frames).unwrap());
        }
        chunks.extend(vad.flush().unwrap());
        for c in &chunks {
            println!("round {round}: chunk at {} ms, {} ms long", c.start_ms, samples_to_ms(c.samples.len()));
        }
        assert!(chunks.len() >= 2, "expected ≥ 2 chunks, got {}", chunks.len());
        // First chunk begins near 1 s; a chunk begins near the second copy.
        let second_at = samples_to_ms(SAMPLE_RATE as usize + speech.len() + gap);
        assert!((1000..=1500).contains(&chunks[0].start_ms), "first chunk at {}", chunks[0].start_ms);
        assert!(
            chunks.iter().any(|c| c.start_ms.abs_diff(second_at) < 600),
            "no chunk near {second_at} ms"
        );
        let mut last = 0;
        for c in &chunks {
            assert!(c.start_ms >= last);
            last = c.start_ms + samples_to_ms(c.samples.len());
            assert!(last <= samples_to_ms(audio.len()) + 100);
        }
        vad.reset(); // second round must start from 0 again
    }
    // Silence → nothing.
    assert!(vad.accept(&vec![0.0; 3 * SAMPLE_RATE as usize]).unwrap().is_empty());
    assert!(vad.flush().unwrap().is_empty());
}

#[test]
#[ignore = "downloads ~33 MB of models"]
fn diarizer_contract_two_speakers() {
    let paths = pack(Pack::Speakers);
    let audio = fixture("two-speakers-en.wav");
    let mut d = SherpaDiarizer::load(&paths).expect("load");
    diarizer_contract(&mut d, &audio, 2);
    let t0 = Instant::now();
    let turns = d.diarize(&audio).unwrap();
    let secs = audio.len() as f64 / SAMPLE_RATE as f64;
    println!("diarized {secs:.1}s in {:.2?} (RTF {:.3}), RSS {:?} MB", t0.elapsed(), t0.elapsed().as_secs_f64() / secs, rss_mb());
    for t in &turns {
        println!("  {:>6}–{:>6} ms  speaker {}", t.start_ms, t.end_ms, t.speaker);
    }
    let mut labels: Vec<u32> = turns.iter().map(|t| t.speaker).collect();
    labels.sort();
    labels.dedup();
    assert_eq!(labels.len(), 2, "two-speaker fixture should yield exactly 2 speakers");
}

/// Prints the speaker count for a range of thresholds (tuning aid).
#[test]
#[ignore = "tuning aid"]
fn diarizer_threshold_sweep() {
    let paths = pack(Pack::Speakers);
    let audio = fixture("two-speakers-en.wav");
    for th in [0.3f32, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9] {
        let mut d = SherpaDiarizer::with_threshold(&paths, th).unwrap();
        let turns = d.diarize(&audio).unwrap();
        let mut l: Vec<u32> = turns.iter().map(|t| t.speaker).collect();
        l.sort();
        l.dedup();
        println!("threshold {th}: {} speakers, {} turns", l.len(), turns.len());
    }
}

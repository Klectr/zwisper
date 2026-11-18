//! Small, dependency-free audio helpers shared by adapters and services.

use crate::domain::{ms_to_samples, samples_to_ms, SpeechChunk, SAMPLE_RATE};
use crate::error::Result;
use crate::ports::{EngineId, VoiceActivity};

/// Average interleaved channels into mono.
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Streaming resampler to 16 kHz. Box-filter decimation + linear interpolation:
/// cheap, allocation-light, and good enough for speech models.
#[derive(Debug, Clone)]
pub struct Resampler {
    ratio: f64, // input samples per output sample
    pos: f64,
    prev: f32,
    pending: Vec<f32>,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Self {
        Resampler {
            ratio: input_rate as f64 / SAMPLE_RATE as f64,
            pos: 0.0,
            prev: 0.0,
            pending: Vec::new(),
        }
    }

    pub fn is_passthrough(&self) -> bool {
        (self.ratio - 1.0).abs() < f64::EPSILON
    }

    pub fn process(&mut self, mono: &[f32]) -> Vec<f32> {
        if self.is_passthrough() {
            return mono.to_vec();
        }
        // Low-pass by averaging a window the size of the decimation factor.
        let win = self.ratio.floor().max(1.0) as usize;
        self.pending.extend_from_slice(mono);
        let mut out = Vec::with_capacity((mono.len() as f64 / self.ratio) as usize + 2);
        while self.pos + 1.0 < self.pending.len() as f64 {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let lo = i.saturating_sub(win / 2);
            let hi = (i + win / 2 + 1).min(self.pending.len());
            let avg = |a: usize, b: usize, buf: &[f32]| {
                let s = &buf[a..b];
                s.iter().sum::<f32>() / s.len().max(1) as f32
            };
            let a = avg(lo, hi, &self.pending);
            let b_idx = (i + 1).min(self.pending.len() - 1);
            let b = avg(b_idx.saturating_sub(win / 2), (b_idx + win / 2 + 1).min(self.pending.len()), &self.pending);
            out.push(a + (b - a) * frac);
            self.pos += self.ratio;
        }
        // Keep a tail for the next call.
        let consumed = (self.pos as usize).saturating_sub(win);
        if consumed > 0 {
            self.prev = self.pending[consumed - 1];
            self.pending.drain(..consumed);
            self.pos -= consumed as f64;
        }
        out
    }
}

pub fn rms(frames: &[f32]) -> f32 {
    if frames.is_empty() {
        return 0.0;
    }
    (frames.iter().map(|x| x * x).sum::<f32>() / frames.len() as f32).sqrt()
}

/// Level in 0..=1 for meters (≈ -60 dBFS → 0, 0 dBFS → 1).
pub fn meter_level(frames: &[f32]) -> f32 {
    let r = rms(frames).max(1e-6);
    let db = 20.0 * r.log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Trim leading/trailing silence (with 200 ms padding). Returns the slice range start offset in ms.
pub fn trim_silence(samples: &[f32], threshold: f32) -> (&[f32], u64) {
    let frame = ms_to_samples(20);
    let pad = ms_to_samples(200);
    let loud = |c: &[f32]| rms(c) > threshold;
    let first = samples.chunks(frame).position(loud);
    let last = samples.chunks(frame).rposition(loud);
    match (first, last) {
        (Some(f), Some(l)) => {
            let start = (f * frame).saturating_sub(pad);
            let end = ((l + 1) * frame + pad).min(samples.len());
            (&samples[start..end], samples_to_ms(start))
        }
        _ => (&samples[0..0], 0),
    }
}

/// A simple energy-based voice-activity detector. Used in tests and as a
/// fallback strategy when the Silero model is unavailable.
pub struct EnergyVad {
    threshold: f32,
    min_silence_ms: u64,
    max_chunk_ms: u64,
    frame: usize,
    buf: Vec<f32>,
    buf_start_ms: u64,
    pos_ms: u64,
    in_speech: bool,
    silence_ms: u64,
    carry: Vec<f32>,
}

impl EnergyVad {
    pub fn new(threshold: f32) -> Self {
        EnergyVad {
            threshold,
            min_silence_ms: 500,
            max_chunk_ms: 20_000,
            frame: ms_to_samples(20),
            buf: Vec::new(),
            buf_start_ms: 0,
            pos_ms: 0,
            in_speech: false,
            silence_ms: 0,
            carry: Vec::new(),
        }
    }

    fn emit(&mut self) -> Option<SpeechChunk> {
        self.in_speech = false;
        self.silence_ms = 0;
        if self.buf.is_empty() {
            return None;
        }
        Some(SpeechChunk {
            start_ms: self.buf_start_ms,
            samples: std::mem::take(&mut self.buf),
        })
    }
}

impl VoiceActivity for EnergyVad {
    fn id(&self) -> EngineId {
        "energy-vad"
    }

    fn accept(&mut self, frames: &[f32]) -> Result<Vec<SpeechChunk>> {
        let mut out = Vec::new();
        self.carry.extend_from_slice(frames);
        let frame = self.frame;
        let n = self.carry.len() / frame;
        let data: Vec<f32> = self.carry.drain(..n * frame).collect();
        for f in data.chunks_exact(frame) {
            let loud = rms(f) > self.threshold;
            if loud {
                if !self.in_speech {
                    self.in_speech = true;
                    self.buf_start_ms = self.pos_ms;
                }
                self.silence_ms = 0;
                self.buf.extend_from_slice(f);
            } else if self.in_speech {
                self.buf.extend_from_slice(f);
                self.silence_ms += 20;
                if self.silence_ms >= self.min_silence_ms {
                    out.extend(self.emit());
                }
            }
            self.pos_ms += 20;
            if self.in_speech && samples_to_ms(self.buf.len()) >= self.max_chunk_ms {
                out.extend(self.emit());
            }
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<SpeechChunk>> {
        Ok(self.emit().into_iter().collect())
    }

    fn reset(&mut self) {
        *self = EnergyVad::new(self.threshold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_48k_to_16k_length() {
        let mut r = Resampler::new(48_000);
        let input = vec![0.5f32; 48_000];
        let mut out = r.process(&input[..24_000]);
        out.extend(r.process(&input[24_000..]));
        assert!((out.len() as i64 - 16_000).abs() < 10, "{}", out.len());
        assert!(out.iter().all(|x| (x - 0.5).abs() < 1e-3));
    }

    #[test]
    fn trim_finds_speech() {
        let mut s = vec![0.0f32; 16_000];
        s.extend(vec![0.3f32; 16_000]);
        s.extend(vec![0.0f32; 16_000]);
        let (t, off) = trim_silence(&s, 0.01);
        assert!(off >= 750 && off <= 800, "{off}");
        assert!(t.len() > 16_000 && t.len() < 16_000 + 2 * ms_to_samples(250));
    }

    #[test]
    fn energy_vad_splits_on_pause() {
        let mut v = EnergyVad::new(0.01);
        let mut s = vec![0.2f32; 16_000];
        s.extend(vec![0.0f32; 16_000]);
        s.extend(vec![0.2f32; 8_000]);
        let mut chunks = v.accept(&s).unwrap();
        chunks.extend(v.flush().unwrap());
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].start_ms, 0);
        assert_eq!(chunks[1].start_ms, 2000);
    }
}

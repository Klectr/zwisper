//! Pure, platform-independent sample conversion used by both the cpal
//! microphone and the Core Audio tap. Compiled (and unit-tested) everywhere.

use zwisper_core::dsp::{downmix, Resampler};

// Linear-PCM format flags from <CoreAudioTypes/CoreAudioBaseTypes.h>. Defined
// here (rather than imported) so this module builds and tests on Linux.
pub const FLAG_IS_FLOAT: u32 = 1 << 0;
pub const FLAG_IS_BIG_ENDIAN: u32 = 1 << 1;
pub const FLAG_IS_SIGNED_INTEGER: u32 = 1 << 2;
pub const FLAG_IS_NON_INTERLEAVED: u32 = 1 << 5;
/// `kAudioFormatLinearPCM` ('lpcm').
pub const FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");

/// The subset of an `AudioStreamBasicDescription` needed to decode samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PcmFormat {
    pub sample_rate: f64,
    pub format_id: u32,
    pub flags: u32,
    pub bits_per_channel: u32,
    pub channels_per_frame: u32,
}

/// How a buffer's bytes are laid out per sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    F32,
    F64,
    I16,
    I24In32,
    I32,
}

impl PcmFormat {
    /// The sample encoding, or `None` if we can't decode this format (not
    /// linear PCM, big-endian, unsigned, or an unusual bit depth).
    pub fn sample_kind(&self) -> Option<SampleKind> {
        if self.format_id != FORMAT_LINEAR_PCM || self.flags & FLAG_IS_BIG_ENDIAN != 0 {
            return None;
        }
        if self.flags & FLAG_IS_FLOAT != 0 {
            return match self.bits_per_channel {
                32 => Some(SampleKind::F32),
                64 => Some(SampleKind::F64),
                _ => None,
            };
        }
        if self.flags & FLAG_IS_SIGNED_INTEGER != 0 {
            return match self.bits_per_channel {
                16 => Some(SampleKind::I16),
                24 => Some(SampleKind::I24In32), // VERIFY ON MAC: assumes 24-in-32, low-aligned
                32 => Some(SampleKind::I32),
                _ => None,
            };
        }
        None
    }

    pub fn is_interleaved(&self) -> bool {
        self.flags & FLAG_IS_NON_INTERLEAVED == 0
    }
}

impl SampleKind {
    pub fn bytes(self) -> usize {
        match self {
            SampleKind::I16 => 2,
            SampleKind::F32 | SampleKind::I24In32 | SampleKind::I32 => 4,
            SampleKind::F64 => 8,
        }
    }
}

/// Decode little-endian native PCM bytes into f32 samples, appending to `out`.
/// Trailing bytes that don't form a whole sample are ignored.
pub fn decode_into(bytes: &[u8], kind: SampleKind, out: &mut Vec<f32>) {
    let n = kind.bytes();
    out.reserve(bytes.len() / n);
    for c in bytes.chunks_exact(n) {
        let v = match kind {
            SampleKind::F32 => f32::from_le_bytes([c[0], c[1], c[2], c[3]]),
            SampleKind::F64 => {
                f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]) as f32
            }
            SampleKind::I16 => i16::from_le_bytes([c[0], c[1]]) as f32 / 32_768.0,
            SampleKind::I24In32 => {
                // Sign-extend the low 24 bits.
                let raw = i32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                ((raw << 8) >> 8) as f32 / 8_388_608.0
            }
            SampleKind::I32 => i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0,
        };
        out.push(v);
    }
}

/// Average several equally long mono buffers (one per channel of a
/// non-interleaved stream) into `out`. Shorter buffers are treated as silence.
pub fn mix_planar(channels: &[&[f32]], out: &mut Vec<f32>) {
    out.clear();
    let len = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    if channels.is_empty() {
        return;
    }
    out.resize(len, 0.0);
    for ch in channels {
        for (o, s) in out.iter_mut().zip(ch.iter()) {
            *o += *s;
        }
    }
    let k = 1.0 / channels.len() as f32;
    for o in out.iter_mut() {
        *o *= k;
    }
}

/// Interleaved device audio → 16 kHz mono. Owns the resampler state so it
/// must persist across callbacks of one stream.
///
/// Note: `zwisper_core::dsp::{downmix, Resampler}` return fresh `Vec`s, so this
/// allocates a little per callback. That is acceptable on macOS (the system
/// allocator does not take a global lock on the fast path) but is flagged in
/// the report; a ring buffer handoff would remove it.
#[derive(Debug, Clone)]
pub struct MonoConverter {
    channels: usize,
    resampler: Resampler,
}

impl MonoConverter {
    pub fn new(input_rate: u32, channels: usize) -> Self {
        MonoConverter {
            channels: channels.max(1),
            resampler: Resampler::new(input_rate),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Convert interleaved f32 frames. Returns 16 kHz mono (possibly empty).
    pub fn process_interleaved(&mut self, interleaved: &[f32]) -> Vec<f32> {
        let mono = downmix(interleaved, self.channels);
        self.process_mono(&mono)
    }

    /// Convert already-mono frames at the input rate.
    pub fn process_mono(&mut self, mono: &[f32]) -> Vec<f32> {
        self.resampler.process(mono)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(flags: u32, bits: u32) -> PcmFormat {
        PcmFormat {
            sample_rate: 48_000.0,
            format_id: FORMAT_LINEAR_PCM,
            flags,
            bits_per_channel: bits,
            channels_per_frame: 2,
        }
    }

    #[test]
    fn sample_kinds() {
        assert_eq!(fmt(FLAG_IS_FLOAT, 32).sample_kind(), Some(SampleKind::F32));
        assert_eq!(fmt(FLAG_IS_FLOAT | FLAG_IS_NON_INTERLEAVED, 32).sample_kind(), Some(SampleKind::F32));
        assert!(!fmt(FLAG_IS_FLOAT | FLAG_IS_NON_INTERLEAVED, 32).is_interleaved());
        assert_eq!(fmt(FLAG_IS_SIGNED_INTEGER, 16).sample_kind(), Some(SampleKind::I16));
        assert_eq!(fmt(FLAG_IS_FLOAT | FLAG_IS_BIG_ENDIAN, 32).sample_kind(), None);
        assert_eq!(fmt(0, 16).sample_kind(), None); // unsigned
        let mut f = fmt(FLAG_IS_FLOAT, 32);
        f.format_id = u32::from_be_bytes(*b"aac ");
        assert_eq!(f.sample_kind(), None);
    }

    #[test]
    fn decode_formats() {
        let mut out = Vec::new();
        let bytes: Vec<u8> = [0.5f32, -0.25].iter().flat_map(|v| v.to_le_bytes()).collect();
        decode_into(&bytes, SampleKind::F32, &mut out);
        assert_eq!(out, vec![0.5, -0.25]);

        out.clear();
        let bytes: Vec<u8> = [16_384i16, -32_768].iter().flat_map(|v| v.to_le_bytes()).collect();
        decode_into(&bytes, SampleKind::I16, &mut out);
        assert_eq!(out, vec![0.5, -1.0]);

        out.clear();
        let bytes: Vec<u8> = [i32::MIN, 1 << 30].iter().flat_map(|v| v.to_le_bytes()).collect();
        decode_into(&bytes, SampleKind::I32, &mut out);
        assert_eq!(out, vec![-1.0, 0.5]);

        out.clear();
        // -4194304 (= -0.5 in 24-bit) stored low-aligned in 32 bits, upper byte garbage-free.
        let v: i32 = -4_194_304 & 0x00ff_ffff;
        decode_into(&v.to_le_bytes(), SampleKind::I24In32, &mut out);
        assert_eq!(out, vec![-0.5]);

        out.clear();
        decode_into(&[1, 2, 3], SampleKind::I16, &mut out); // partial sample ignored
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn planar_mix() {
        let l = [1.0f32, 1.0, 1.0];
        let r = [0.0f32, -1.0];
        let mut out = Vec::new();
        mix_planar(&[&l, &r], &mut out);
        assert_eq!(out, vec![0.5, 0.0, 0.5]);
        mix_planar(&[], &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn converter_48k_stereo_to_16k_mono() {
        let mut c = MonoConverter::new(48_000, 2);
        let input: Vec<f32> = (0..48_000).flat_map(|_| [0.2f32, 0.4]).collect();
        let mut out = Vec::new();
        for chunk in input.chunks(2 * 512) {
            out.extend(c.process_interleaved(chunk));
        }
        assert!((out.len() as i64 - 16_000).abs() < 10, "{}", out.len());
        assert!(out.iter().all(|x| (x - 0.3).abs() < 1e-3));
    }

    #[test]
    fn converter_passthrough_16k_mono() {
        let mut c = MonoConverter::new(16_000, 1);
        assert_eq!(c.process_interleaved(&[0.1, 0.2]), vec![0.1, 0.2]);
    }
}

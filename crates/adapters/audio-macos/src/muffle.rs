//! The "from the next room" effect used while dictating: you can tell sound
//! is playing, but can't follow the words.
//!
//! Three layers, all scaled by `amount` (0 = untouched, 1 = fully muffled):
//! 1. **Soft saturation** (before the filter): evens out loud and quiet
//!    syllables and adds a warm thump; its harmonics are filtered away.
//! 2. **Steep low-pass** (36 dB/octave, gliding 18 kHz → 300 Hz): removes the
//!    consonants and most vowel formants that carry intelligibility.
//! 3. **Room smear**: a short, dark reverb (Freeverb-style combs + allpasses)
//!    blurs syllables together, flattening the rhythm the brain decodes
//!    speech from even without high frequencies.
//!
//! Pure DSP, no OS calls, no allocation in `process` (buffers are sized in
//! `new`) — safe on a real-time audio thread. `amount` glides so switching
//! on/off sounds like a door closing rather than a hard cut.

/// Low-pass corner when fully muffled.
pub const CUTOFF_MUFFLED_HZ: f32 = 300.0;
/// Corner when open: above hearing for practical purposes.
pub const CUTOFF_OPEN_HZ: f32 = 18_000.0;
/// Output gain when fully muffled.
pub const GAIN_MUFFLED: f32 = 0.6;
/// Reverb share of the output when fully muffled.
pub const ROOM_WET: f32 = 0.7;
/// Saturation drive when fully muffled (1 = none).
pub const DRIVE: f32 = 3.0;
/// Glide time constant; ~3τ ≈ 150 ms to settle.
pub const GLIDE_SECONDS: f32 = 0.05;

/// Low-pass stages (12 dB/octave each).
const STAGES: usize = 3;
/// Recompute filter coefficients every this many samples while gliding.
const COEFF_BLOCK: usize = 32;

/// RBJ low-pass biquad, transposed direct form II.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn set_lowpass(&mut self, sample_rate: f32, cutoff: f32) {
        let q = std::f32::consts::FRAC_1_SQRT_2;
        let w0 = 2.0 * std::f32::consts::PI * cutoff / sample_rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 - cos) / 2.0 / a0;
        self.b1 = (1.0 - cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Feedback comb with a one-pole low-pass in the loop (darker tail).
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    const FEEDBACK: f32 = 0.84;
    const DAMP: f32 = 0.4;

    fn new(len: usize) -> Self {
        Comb { buf: vec![0.0; len.max(1)], pos: 0, store: 0.0 }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.buf[self.pos];
        self.store = y * (1.0 - Self::DAMP) + self.store * Self::DAMP;
        self.buf[self.pos] = x + self.store * Self::FEEDBACK;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// Schroeder allpass (diffuses without colouring).
struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    const GAIN: f32 = 0.5;

    fn new(len: usize) -> Self {
        Allpass { buf: vec![0.0; len.max(1)], pos: 0 }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.pos];
        let y = delayed - x;
        self.buf[self.pos] = x + delayed * Self::GAIN;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

/// A small room for one channel.
struct Room {
    combs: [Comb; 4],
    allpasses: [Allpass; 2],
}

impl Room {
    /// Freeverb's tunings (in samples at 44.1 kHz), scaled to the rate;
    /// `spread` offsets the right channel so the stereo image stays wide.
    fn new(sample_rate: f32, spread: usize) -> Self {
        let n = |samples_44k: usize| ((samples_44k + spread) as f32 * sample_rate / 44_100.0) as usize;
        Room {
            combs: [Comb::new(n(1116)), Comb::new(n(1277)), Comb::new(n(1422)), Comb::new(n(1617))],
            allpasses: [Allpass::new(n(556)), Allpass::new(n(341))],
        }
    }

    /// Input gain keeping the wet level roughly equal to the dry level.
    const INPUT_GAIN: f32 = 0.3;

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let x = x * Self::INPUT_GAIN;
        let mut y: f32 = self.combs.iter_mut().map(|c| c.tick(x)).sum();
        for a in &mut self.allpasses {
            y = a.tick(y);
        }
        y
    }
}

/// Stereo muffler.
pub struct Muffler {
    sample_rate: f32,
    amount: f32,
    glide: f32,
    filters: [[Biquad; STAGES]; 2],
    rooms: [Room; 2],
    since_coeffs: usize,
}

impl Muffler {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8_000.0);
        let mut m = Muffler {
            sample_rate,
            amount: 0.0,
            glide: 1.0 - (-1.0 / (GLIDE_SECONDS * sample_rate)).exp(),
            filters: [[Biquad::default(); STAGES]; 2],
            rooms: [Room::new(sample_rate, 0), Room::new(sample_rate, 23)],
            since_coeffs: 0,
        };
        m.update_coeffs();
        m
    }

    /// Current amount, 0 (open) … 1 (muffled).
    pub fn amount(&self) -> f32 {
        self.amount
    }

    fn cutoff(&self) -> f32 {
        // Log interpolation sounds even across the glide.
        let c = CUTOFF_OPEN_HZ * (CUTOFF_MUFFLED_HZ / CUTOFF_OPEN_HZ).powf(self.amount);
        c.min(self.sample_rate * 0.45)
    }

    fn update_coeffs(&mut self) {
        let c = self.cutoff();
        for ch in &mut self.filters {
            for f in ch {
                f.set_lowpass(self.sample_rate, c);
            }
        }
        self.since_coeffs = 0;
    }

    #[inline]
    fn channel(filters: &mut [Biquad; STAGES], room: &mut Room, x: f32, amount: f32) -> f32 {
        // 1. Saturate (unity for small signals, squashes peaks); blended so
        //    amount 0 is bit-for-bit transparent.
        let drive = 1.0 + (DRIVE - 1.0) * amount;
        let x = x + ((x * drive).tanh() / drive - x) * amount;
        // 2. Low-pass.
        let mut y = x;
        for f in filters.iter_mut() {
            y = f.tick(y);
        }
        // 3. Room. Always run the reverb so its tail is ready when the door closes.
        let wet = room.tick(y) * ROOM_WET * amount;
        let dry = y * (1.0 - ROOM_WET * amount);
        // 4. Gain.
        (dry + wet) * (1.0 - (1.0 - GAIN_MUFFLED) * amount)
    }

    /// Process `left`/`right` in place, gliding towards `target` (0…1).
    /// Extra samples in the longer slice are left untouched.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], target: f32) {
        let target = target.clamp(0.0, 1.0);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if (self.amount - target).abs() > 1e-4 {
                self.amount += (target - self.amount) * self.glide;
                self.since_coeffs += 1;
                if self.since_coeffs >= COEFF_BLOCK {
                    self.update_coeffs();
                }
            } else if self.amount != target {
                self.amount = target;
                self.update_coeffs();
            }
            let a = self.amount;
            let [fl, fr] = &mut self.filters;
            let [rl, rr] = &mut self.rooms;
            *l = Self::channel(fl, rl, *l, a);
            *r = Self::channel(fr, rr, *r, a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Run `input` through a muffler that has already settled at `amount`.
    fn settled(input: &[f32], amount: f32) -> Vec<f32> {
        let mut m = Muffler::new(SR);
        let (mut wl, mut wr) = (vec![0.0; SR as usize], vec![0.0; SR as usize]);
        m.process(&mut wl, &mut wr, amount);
        let (mut l, mut r) = (input.to_vec(), input.to_vec());
        m.process(&mut l, &mut r, amount);
        l
    }

    fn gain_at(freq: f32, amount: f32) -> f32 {
        let input = sine(freq, 0.3, SR as usize);
        let out = settled(&input, amount);
        // Skip the first half second so the reverb has filled.
        let h = input.len() / 2;
        rms(&out[h..]) / rms(&input[h..])
    }

    #[test]
    fn open_is_transparent() {
        // Level, not samples: the open 18 kHz filter shifts phase inaudibly.
        for f in [100.0, 1_000.0, 5_000.0] {
            let g = gain_at(f, 0.0);
            assert!((g - 1.0).abs() < 0.02, "{f} Hz gain {g}");
        }
    }

    #[test]
    fn muffled_keeps_rumble_but_kills_speech_frequencies() {
        let rumble = gain_at(120.0, 1.0);
        let formants = gain_at(1_000.0, 1.0);
        let consonants = gain_at(3_000.0, 1.0);
        assert!(rumble > 0.15, "120 Hz gain {rumble}");
        assert!(formants < 0.01, "1 kHz gain {formants}");
        assert!(consonants < 0.001, "3 kHz gain {consonants}");
    }

    /// Envelope depth of a 4 Hz syllable-rate amplitude modulation.
    fn syllable_depth(x: &[f32]) -> f32 {
        let win = (SR * 0.02) as usize;
        let env: Vec<f32> = x.chunks(win).map(rms).collect();
        let tail = &env[env.len() / 2..];
        let (lo, hi) = tail.iter().fold((f32::MAX, 0.0f32), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        (hi - lo) / (hi + lo)
    }

    #[test]
    fn smears_the_syllable_rhythm() {
        // 150 Hz "voice" pulsing at 4 Hz, like syllables.
        let input: Vec<f32> = (0..(SR as usize * 3))
            .map(|i| {
                let t = i as f32 / SR;
                let env = 0.5 + 0.5 * (2.0 * std::f32::consts::PI * 4.0 * t).sin();
                0.3 * env * (2.0 * std::f32::consts::PI * 150.0 * t).sin()
            })
            .collect();
        let before = syllable_depth(&input);
        let after = syllable_depth(&settled(&input, 1.0));
        assert!(after < 0.7 * before, "rhythm depth {before:.2} -> {after:.2}");
    }

    #[test]
    fn glides_instead_of_jumping() {
        let mut m = Muffler::new(SR);
        let (mut l, mut r) = (vec![0.0; 480], vec![0.0; 480]); // 10 ms
        m.process(&mut l, &mut r, 1.0);
        assert!(m.amount() > 0.05 && m.amount() < 0.5, "amount after 10 ms: {}", m.amount());
        let (mut l, mut r) = (vec![0.0; 24_000], vec![0.0; 24_000]); // 500 ms
        m.process(&mut l, &mut r, 1.0);
        assert_eq!(m.amount(), 1.0);
    }

    #[test]
    fn stays_finite_and_bounded() {
        for rate in [16_000.0, 44_100.0, 96_000.0] {
            let mut m = Muffler::new(rate);
            let mut l: Vec<f32> = (0..rate as usize).map(|i| if i % 97 == 0 { 1.0 } else { -0.8 }).collect();
            let mut r = l.clone();
            m.process(&mut l, &mut r, 1.0);
            assert!(l.iter().all(|v| v.is_finite() && v.abs() < 2.0), "rate {rate}");
        }
    }
}

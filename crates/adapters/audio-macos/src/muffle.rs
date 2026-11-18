//! The "from the next room" effect used while dictating: a steep low-pass
//! (speech intelligibility lives in the 1–4 kHz consonants, so cutting those
//! leaves rhythm and bass but no words) plus a gain drop.
//!
//! Pure DSP, no OS calls, no allocation in `process` — safe on a real-time
//! audio thread. `amount` glides between 0 (untouched) and 1 (fully muffled)
//! so turning it on/off sounds like a door closing rather than a hard cut.

/// Low-pass corner when fully muffled.
pub const CUTOFF_MUFFLED_HZ: f32 = 450.0;
/// Corner when open: above hearing for practical purposes.
pub const CUTOFF_OPEN_HZ: f32 = 18_000.0;
/// Output gain when fully muffled (≈ −8 dB, on top of what the filter removes).
pub const GAIN_MUFFLED: f32 = 0.4;
/// Glide time constant; ~3τ ≈ 150 ms to settle.
pub const GLIDE_SECONDS: f32 = 0.05;
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

/// Stereo muffler: two cascaded biquads per channel (24 dB/octave).
pub struct Muffler {
    sample_rate: f32,
    amount: f32,
    glide: f32,
    filters: [[Biquad; 2]; 2],
    since_coeffs: usize,
}

impl Muffler {
    pub fn new(sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(8_000.0);
        let mut m = Muffler {
            sample_rate,
            amount: 0.0,
            glide: 1.0 - (-1.0 / (GLIDE_SECONDS * sample_rate)).exp(),
            filters: [[Biquad::default(); 2]; 2],
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

    /// Filter `left`/`right` in place, gliding towards `target` (0…1).
    /// The slices should be the same length; extra samples in the longer one
    /// are left untouched.
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
            let gain = 1.0 - (1.0 - GAIN_MUFFLED) * self.amount;
            let [[l0, l1], [r0, r1]] = &mut self.filters;
            *l = l1.tick(l0.tick(*l)) * gain;
            *r = r1.tick(r0.tick(*r)) * gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / SR).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Gain of the muffler at `freq` once settled at `amount`.
    fn gain_at(freq: f32, amount: f32) -> f32 {
        let mut m = Muffler::new(SR);
        // Settle the glide and the filter state.
        let mut warm_l = sine(freq, SR as usize);
        let mut warm_r = warm_l.clone();
        m.process(&mut warm_l, &mut warm_r, amount);
        let input = sine(freq, 24_000);
        let (mut l, mut r) = (input.clone(), input.clone());
        m.process(&mut l, &mut r, amount);
        rms(&l) / rms(&input)
    }

    #[test]
    fn open_is_transparent_for_speech() {
        assert!(gain_at(200.0, 0.0) > 0.98);
        assert!(gain_at(3_000.0, 0.0) > 0.98);
    }

    #[test]
    fn muffled_keeps_bass_but_kills_consonants() {
        let bass = gain_at(150.0, 1.0);
        let consonants = gain_at(3_000.0, 1.0);
        assert!(bass > 0.3, "bass gain {bass}");
        assert!(consonants < 0.01, "3 kHz gain {consonants}");
    }

    #[test]
    fn glides_instead_of_jumping() {
        let mut m = Muffler::new(SR);
        let mut l = vec![0.0; 480]; // 10 ms
        let mut r = vec![0.0; 480];
        m.process(&mut l, &mut r, 1.0);
        assert!(m.amount() > 0.05 && m.amount() < 0.5, "amount after 10 ms: {}", m.amount());
        let mut l = vec![0.0; 24_000]; // 500 ms
        let mut r = vec![0.0; 24_000];
        m.process(&mut l, &mut r, 1.0);
        assert_eq!(m.amount(), 1.0);
    }

    #[test]
    fn stays_finite_at_low_sample_rates() {
        let mut m = Muffler::new(16_000.0);
        let mut l = sine(1_000.0, 16_000);
        let mut r = l.clone();
        m.process(&mut l, &mut r, 0.0);
        assert!(l.iter().all(|v| v.is_finite()));
    }
}

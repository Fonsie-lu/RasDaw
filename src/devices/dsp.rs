//! Small DSP building blocks shared by the built-in devices.

use std::f32::consts::PI;

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db * 0.05)
}

pub fn gain_to_db(g: f32) -> f32 {
    20.0 * g.max(1e-9).log10()
}

/// One-pole smoothing coefficient for a time constant in milliseconds.
pub fn coef_ms(ms: f32, sr: f32) -> f32 {
    (-1.0 / (ms.max(0.01) * 0.001 * sr)).exp()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coeffs {
    pub const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// RBJ cookbook filters. `f` is clamped below Nyquist.
    fn rbj(kind: Kind, f: f32, q: f32, gain_db: f32, sr: f32) -> Self {
        let f = f.clamp(1.0, sr * 0.49);
        let w = 2.0 * PI * f / sr;
        let (sw, cw) = w.sin_cos();
        let alpha = sw / (2.0 * q.max(0.05));
        let a = 10f32.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Lowpass => ((1.0 - cw) / 2.0, 1.0 - cw, (1.0 - cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            Kind::Highpass => ((1.0 + cw) / 2.0, -(1.0 + cw), (1.0 + cw) / 2.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            Kind::Peak => (1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a),
            Kind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - s),
                    (a + 1.0) + (a - 1.0) * cw + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - s,
                )
            }
            Kind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - s),
                    (a + 1.0) - (a - 1.0) * cw + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - s,
                )
            }
        };
        Coeffs { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    pub fn lowpass(f: f32, q: f32, sr: f32) -> Self {
        Self::rbj(Kind::Lowpass, f, q, 0.0, sr)
    }
    pub fn highpass(f: f32, q: f32, sr: f32) -> Self {
        Self::rbj(Kind::Highpass, f, q, 0.0, sr)
    }
    pub fn peak(f: f32, q: f32, gain_db: f32, sr: f32) -> Self {
        Self::rbj(Kind::Peak, f, q, gain_db, sr)
    }
    pub fn low_shelf(f: f32, q: f32, gain_db: f32, sr: f32) -> Self {
        Self::rbj(Kind::LowShelf, f, q, gain_db, sr)
    }
    pub fn high_shelf(f: f32, q: f32, gain_db: f32, sr: f32) -> Self {
        Self::rbj(Kind::HighShelf, f, q, gain_db, sr)
    }

    /// Magnitude response at frequency `f`.
    pub fn magnitude(&self, f: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * f / sr;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-20)).sqrt()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Lowpass,
    Highpass,
    Peak,
    LowShelf,
    HighShelf,
}

/// Q values of the two sections of a 4th-order Butterworth filter.
pub const BUTTERWORTH4_Q: [f32; 2] = [0.541_196, 1.306_563];

/// Transposed direct form II biquad.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub c: Coeffs,
    z1: f32,
    z2: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self { c: Coeffs::IDENTITY, z1: 0.0, z2: 0.0 }
    }
}

impl Biquad {
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let c = &self.c;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Fixed-size delay line (allocated once, on the main thread).
pub struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    pub fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(4)], pos: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn push(&mut self, x: f32) {
        self.pos = (self.pos + 1) % self.buf.len();
        self.buf[self.pos] = x;
    }

    /// Sample written `d` pushes ago (0 = newest), linearly interpolated.
    pub fn read(&self, d: f32) -> f32 {
        let len = self.buf.len();
        let d = d.clamp(0.0, (len - 2) as f32);
        let i = d.floor();
        let frac = d - i;
        let a = self.buf[(self.pos + len - i as usize) % len];
        let b = self.buf[(self.pos + len - i as usize - 1) % len];
        a + (b - a) * frac
    }

    pub fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_have_the_expected_gain() {
        let sr = 48_000.0;
        let lp = Coeffs::lowpass(1000.0, std::f32::consts::FRAC_1_SQRT_2, sr);
        assert!((lp.magnitude(10.0, sr) - 1.0).abs() < 0.01);
        assert!((gain_to_db(lp.magnitude(1000.0, sr)) + 3.01).abs() < 0.1);
        let pk = Coeffs::peak(1000.0, 1.0, 6.0, sr);
        assert!((gain_to_db(pk.magnitude(1000.0, sr)) - 6.0).abs() < 0.05);
        let ls = Coeffs::low_shelf(100.0, std::f32::consts::FRAC_1_SQRT_2, -12.0, sr);
        assert!((gain_to_db(ls.magnitude(10.0, sr)) + 12.0).abs() < 0.2);
        assert!(gain_to_db(ls.magnitude(5000.0, sr)).abs() < 0.1);
    }

    #[test]
    fn delay_line_reads_back() {
        let mut d = DelayLine::new(8);
        for i in 0..5 {
            d.push(i as f32);
        }
        assert_eq!(d.read(0.0), 4.0);
        assert_eq!(d.read(2.0), 2.0);
        assert_eq!(d.read(1.5), 2.5);
    }
}

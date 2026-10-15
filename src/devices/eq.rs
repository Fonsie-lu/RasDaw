//! 5-band EQ (low shelf, 3 peaks, high shelf) with fixed protection filters:
//! - 4th-order Butterworth high-pass at 20 Hz and low-pass at 16 kHz,
//! - everything below 100 Hz is summed to mono (the side signal is high-passed at 100 Hz with a
//!   Linkwitz-Riley 4th-order filter).

use super::dsp::{Biquad, Coeffs, BUTTERWORTH4_Q};
use super::{Ctx, Dsp, ParamDef, Unit};

pub const LOW_CUT_HZ: f32 = 20.0;
pub const HIGH_CUT_HZ: f32 = 16_000.0;
pub const MONO_BELOW_HZ: f32 = 100.0;

pub const PARAMS: &[ParamDef] = &[
    ParamDef::new("low_freq", "Low Freq", 20.0, 500.0, 80.0, Unit::Hz),
    ParamDef::new("low_gain", "Low Gain", -18.0, 18.0, 0.0, Unit::Db),
    ParamDef::new("low_q", "Low Q", 0.3, 4.0, 0.71, Unit::Q),
    ParamDef::new("lm_freq", "Lo-Mid Freq", 80.0, 2000.0, 250.0, Unit::Hz),
    ParamDef::new("lm_gain", "Lo-Mid Gain", -18.0, 18.0, 0.0, Unit::Db),
    ParamDef::new("lm_q", "Lo-Mid Q", 0.3, 10.0, 1.0, Unit::Q),
    ParamDef::new("mid_freq", "Mid Freq", 300.0, 6000.0, 1000.0, Unit::Hz),
    ParamDef::new("mid_gain", "Mid Gain", -18.0, 18.0, 0.0, Unit::Db),
    ParamDef::new("mid_q", "Mid Q", 0.3, 10.0, 1.0, Unit::Q),
    ParamDef::new("hm_freq", "Hi-Mid Freq", 1000.0, 12000.0, 3500.0, Unit::Hz),
    ParamDef::new("hm_gain", "Hi-Mid Gain", -18.0, 18.0, 0.0, Unit::Db),
    ParamDef::new("hm_q", "Hi-Mid Q", 0.3, 10.0, 1.0, Unit::Q),
    ParamDef::new("high_freq", "High Freq", 2000.0, 16000.0, 8000.0, Unit::Hz),
    ParamDef::new("high_gain", "High Gain", -18.0, 18.0, 0.0, Unit::Db),
    ParamDef::new("high_q", "High Q", 0.3, 4.0, 0.71, Unit::Q),
    ParamDef::new("output", "Output", -18.0, 18.0, 0.0, Unit::Db),
];

/// Coefficients of the 5 bands for the given parameter values.
pub fn band_coeffs(p: &[f32], sr: f32) -> [Coeffs; 5] {
    [
        Coeffs::low_shelf(p[0], p[2], p[1], sr),
        Coeffs::peak(p[3], p[5], p[4], sr),
        Coeffs::peak(p[6], p[8], p[7], sr),
        Coeffs::peak(p[9], p[11], p[10], sr),
        Coeffs::high_shelf(p[12], p[14], p[13], sr),
    ]
}

fn fixed_coeffs(sr: f32) -> [Coeffs; 4] {
    [
        Coeffs::highpass(LOW_CUT_HZ, BUTTERWORTH4_Q[0], sr),
        Coeffs::highpass(LOW_CUT_HZ, BUTTERWORTH4_Q[1], sr),
        Coeffs::lowpass(HIGH_CUT_HZ, BUTTERWORTH4_Q[0], sr),
        Coeffs::lowpass(HIGH_CUT_HZ, BUTTERWORTH4_Q[1], sr),
    ]
}

/// Total response in dB at `f` for the mid (mono) signal, for the rack display.
pub fn response_db(p: &[f32], f: f32, sr: f32) -> f32 {
    let mut g = super::dsp::db_to_gain(p[15]);
    for c in band_coeffs(p, sr).iter().chain(fixed_coeffs(sr).iter()) {
        g *= c.magnitude(f, sr);
    }
    super::dsp::gain_to_db(g)
}

pub struct Eq5 {
    /// Per channel: 5 bands, then 2 high-pass and 2 low-pass sections.
    chain: [[Biquad; 9]; 2],
    side_hp: [Biquad; 2],
    last: [f32; 16],
    sr: f32,
}

impl Default for Eq5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Eq5 {
    pub fn new() -> Self {
        Self {
            chain: [[Biquad::default(); 9]; 2],
            side_hp: [Biquad::default(); 2],
            last: [f32::NAN; 16],
            sr: 0.0,
        }
    }

    fn update(&mut self, p: &[f32], sr: f32) {
        if p == self.last && sr == self.sr {
            return;
        }
        let bands = band_coeffs(p, sr);
        let fixed = fixed_coeffs(sr);
        for ch in &mut self.chain {
            for (b, c) in ch.iter_mut().zip(bands.iter().chain(fixed.iter())) {
                b.c = *c;
            }
        }
        // Linkwitz-Riley 4th order = two 2nd-order Butterworth sections at the same Q.
        for b in &mut self.side_hp {
            b.c = Coeffs::highpass(MONO_BELOW_HZ, std::f32::consts::FRAC_1_SQRT_2, sr);
        }
        self.last.copy_from_slice(p);
        self.sr = sr;
    }
}

impl Dsp for Eq5 {
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32 {
        self.update(p, ctx.sample_rate);
        let out = super::dsp::db_to_gain(p[15]);
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            // Mono below 100 Hz: keep the mid signal whole, high-pass only the side.
            let mid = 0.5 * (*a + *b);
            let mut side = 0.5 * (*a - *b);
            for f in &mut self.side_hp {
                side = f.process(side);
            }
            let mut x = [mid + side, mid - side];
            for (ch, v) in x.iter_mut().enumerate() {
                for f in &mut self.chain[ch] {
                    *v = f.process(*v);
                }
            }
            *a = x[0] * out;
            *b = x[1] * out;
        }
        0.0
    }
}

//! Compressor and limiter.

use super::dsp::{coef_ms, db_to_gain, gain_to_db, DelayLine};
use super::{Ctx, Dsp, ParamDef, Unit};

pub const COMPRESSOR_PARAMS: &[ParamDef] = &[
    ParamDef::new("threshold", "Threshold", -60.0, 0.0, -18.0, Unit::Db),
    ParamDef::new("ratio", "Ratio", 1.0, 20.0, 4.0, Unit::Ratio),
    ParamDef::new("attack", "Attack", 0.1, 100.0, 10.0, Unit::Ms),
    ParamDef::new("release", "Release", 5.0, 1000.0, 120.0, Unit::Ms),
    ParamDef::new("knee", "Knee", 0.0, 12.0, 6.0, Unit::Db),
    ParamDef::new("makeup", "Makeup", 0.0, 24.0, 0.0, Unit::Db),
    ParamDef::new("mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
];

pub const LIMITER_PARAMS: &[ParamDef] = &[
    ParamDef::new("gain", "Gain", 0.0, 24.0, 0.0, Unit::Db),
    ParamDef::new("ceiling", "Ceiling", -12.0, 0.0, -0.3, Unit::Db),
    ParamDef::new("release", "Release", 1.0, 1000.0, 80.0, Unit::Ms),
];

/// Feed-forward, stereo-linked peak compressor with a soft knee (gain computer in dB).
pub struct Compressor {
    env_db: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        Self { env_db: 0.0 }
    }
}

/// Gain change in dB (≤ 0) for an input level in dB.
fn gain_computer(level: f32, threshold: f32, ratio: f32, knee: f32) -> f32 {
    let over = level - threshold;
    let slope = 1.0 / ratio - 1.0;
    if 2.0 * over < -knee {
        0.0
    } else if knee > 0.0 && 2.0 * over.abs() <= knee {
        slope * (over + knee / 2.0).powi(2) / (2.0 * knee)
    } else {
        slope * over
    }
}

impl Dsp for Compressor {
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32 {
        let (threshold, ratio, knee, mix) = (p[0], p[1].max(1.0), p[4], p[6]);
        let att = coef_ms(p[2], ctx.sample_rate);
        let rel = coef_ms(p[3], ctx.sample_rate);
        let makeup = db_to_gain(p[5]);
        let mut max_gr = 0.0f32;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let level = gain_to_db(a.abs().max(b.abs()));
            let target = gain_computer(level, threshold, ratio, knee);
            let c = if target < self.env_db { att } else { rel };
            self.env_db = target + c * (self.env_db - target);
            max_gr = max_gr.max(-self.env_db);
            let g = db_to_gain(self.env_db) * makeup;
            *a = *a * (1.0 - mix) + *a * g * mix;
            *b = *b * (1.0 - mix) + *b * g * mix;
        }
        max_gr
    }
}

/// Look-ahead peak limiter: the gain follows the minimum required gain over the look-ahead
/// window, so it is already down when a peak leaves the delay line. A final clip at the
/// ceiling catches what the smoothing misses.
pub struct Limiter {
    delay: [DelayLine; 2],
    /// Required gains of the last `lookahead + 1` samples (ring).
    required: Vec<f32>,
    pos: usize,
    lookahead: usize,
    gain: f32,
}

const LOOKAHEAD_MS: f32 = 1.5;

impl Limiter {
    pub fn new(sr: f32) -> Self {
        let lookahead = ((LOOKAHEAD_MS * 0.001 * sr) as usize).max(1);
        Self {
            delay: [DelayLine::new(lookahead + 4), DelayLine::new(lookahead + 4)],
            required: vec![1.0; lookahead + 1],
            pos: 0,
            lookahead,
            gain: 1.0,
        }
    }
}

impl Dsp for Limiter {
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32 {
        let input = db_to_gain(p[0]);
        let ceiling = db_to_gain(p[1]);
        let rel = coef_ms(p[2], ctx.sample_rate);
        // Reaches the target within the look-ahead window (about 5 time constants).
        let att = (-5.0 / self.lookahead as f32).exp();
        let mut min_gain = 1.0f32;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = (*a * input, *b * input);
            let peak = x.abs().max(y.abs());
            self.pos = (self.pos + 1) % self.required.len();
            self.required[self.pos] = if peak > ceiling { ceiling / peak } else { 1.0 };
            let target = self.required.iter().copied().fold(1.0f32, f32::min);
            let c = if target < self.gain { att } else { rel };
            self.gain = target + c * (self.gain - target);
            min_gain = min_gain.min(self.gain);
            self.delay[0].push(x);
            self.delay[1].push(y);
            let d = self.lookahead as f32;
            *a = (self.delay[0].read(d) * self.gain).clamp(-ceiling, ceiling);
            *b = (self.delay[1].read(d) * self.gain).clamp(-ceiling, ceiling);
        }
        -gain_to_db(min_gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knee_is_continuous() {
        let f = |l| gain_computer(l, -20.0, 4.0, 6.0);
        assert_eq!(f(-30.0), 0.0);
        assert!((f(-23.0) - 0.0).abs() < 1e-4);
        assert!((f(-17.0) - (-0.75 * 3.0)).abs() < 1e-4);
        assert!((f(0.0) - (-15.0)).abs() < 1e-4);
    }
}

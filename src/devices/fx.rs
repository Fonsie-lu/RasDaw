//! Delay, reverb and distortion.

use super::dsp::{coef_ms, db_to_gain, Biquad, Coeffs, DelayLine};
use super::{Ctx, Dsp, ParamDef, Unit};

// ---------------------------------------------------------------- delay

const DIVISIONS: &[&str] = &["1/32", "1/16", "1/16.", "1/8", "1/8.", "1/4", "1/4.", "1/2", "1/1"];
const DIVISION_BEATS: [f32; 9] = [0.125, 0.25, 0.375, 0.5, 0.75, 1.0, 1.5, 2.0, 4.0];
const OFF_ON: &[&str] = &["Off", "On"];
const MAX_DELAY_S: f32 = 2.5;

pub const DELAY_PARAMS: &[ParamDef] = &[
    ParamDef::new("time", "Time", 1.0, 2000.0, 375.0, Unit::Ms),
    ParamDef::choice("sync", "Sync", OFF_ON, 1),
    ParamDef::choice("division", "Division", DIVISIONS, 4),
    ParamDef::new("feedback", "Feedback", 0.0, 0.95, 0.4, Unit::Percent),
    ParamDef::choice("pingpong", "Ping-Pong", OFF_ON, 0),
    ParamDef::new("low_cut", "Low Cut", 20.0, 2000.0, 100.0, Unit::Hz),
    ParamDef::new("high_cut", "High Cut", 1000.0, 20000.0, 8000.0, Unit::Hz),
    ParamDef::new("mix", "Mix", 0.0, 1.0, 0.3, Unit::Percent),
];

pub struct Delay {
    lines: [DelayLine; 2],
    /// Feedback-path filters per channel: low cut, high cut.
    filters: [[Biquad; 2]; 2],
    /// Smoothed delay time in samples (None until the first block).
    current: Option<f32>,
    last_cuts: (f32, f32),
}

impl Delay {
    pub fn new(sr: f32) -> Self {
        let len = (MAX_DELAY_S * sr) as usize + 8;
        Self {
            lines: [DelayLine::new(len), DelayLine::new(len)],
            filters: [[Biquad::default(); 2]; 2],
            current: None,
            last_cuts: (0.0, 0.0),
        }
    }
}

impl Dsp for Delay {
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32 {
        let sr = ctx.sample_rate;
        let seconds = if p[1] >= 0.5 {
            DIVISION_BEATS[(p[2] as usize).min(DIVISION_BEATS.len() - 1)] * 60.0 / ctx.bpm.max(1.0)
        } else {
            p[0] * 0.001
        };
        let max = (self.lines[0].capacity() - 4) as f32;
        let target = (seconds * sr).clamp(1.0, max);
        let mut d = self.current.unwrap_or(target);
        let (feedback, pingpong, mix) = (p[3], p[4] >= 0.5, p[7]);
        if self.last_cuts != (p[5], p[6]) {
            for ch in &mut self.filters {
                ch[0].c = Coeffs::highpass(p[5], std::f32::consts::FRAC_1_SQRT_2, sr);
                ch[1].c = Coeffs::lowpass(p[6], std::f32::consts::FRAC_1_SQRT_2, sr);
            }
            self.last_cuts = (p[5], p[6]);
        }
        let glide = 1.0 - coef_ms(60.0, sr);
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            d += (target - d) * glide;
            let mut wet = [self.lines[0].read(d), self.lines[1].read(d)];
            for (ch, w) in wet.iter_mut().enumerate() {
                let x = self.filters[ch][0].process(*w);
                *w = self.filters[ch][1].process(x);
            }
            let (dry_l, dry_r) = (*a, *b);
            if pingpong {
                // Input enters left; the echoes bounce between the channels.
                self.lines[0].push(0.5 * (dry_l + dry_r) + wet[1] * feedback);
                self.lines[1].push(wet[0] * feedback);
            } else {
                self.lines[0].push(dry_l + wet[0] * feedback);
                self.lines[1].push(dry_r + wet[1] * feedback);
            }
            *a = dry_l * (1.0 - mix) + wet[0] * mix;
            *b = dry_r * (1.0 - mix) + wet[1] * mix;
        }
        self.current = Some(d);
        0.0
    }
}

// ---------------------------------------------------------------- reverb (Freeverb topology)

pub const REVERB_PARAMS: &[ParamDef] = &[
    ParamDef::new("size", "Size", 0.0, 1.0, 0.6, Unit::Percent),
    ParamDef::new("damping", "Damping", 0.0, 1.0, 0.5, Unit::Percent),
    ParamDef::new("width", "Width", 0.0, 1.0, 1.0, Unit::Percent),
    ParamDef::new("predelay", "Pre-Delay", 0.1, 200.0, 10.0, Unit::Ms),
    ParamDef::new("low_cut", "Low Cut", 20.0, 1000.0, 150.0, Unit::Hz),
    ParamDef::new("mix", "Mix", 0.0, 1.0, 0.25, Unit::Percent),
];

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0, store: 0.0 }
    }

    #[inline]
    fn process(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.pos] = x + self.store * feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    pos: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0 }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let b = self.buf[self.pos];
        self.buf[self.pos] = x + b * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        b - x
    }
}

pub struct Reverb {
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    predelay: DelayLine,
    low_cut: Biquad,
    last_cut: f32,
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        let scale = sr / 44_100.0;
        let len = |n: usize, ch: usize| ((n + ch * STEREO_SPREAD) as f32 * scale) as usize;
        Self {
            combs: [0, 1].map(|ch| COMBS.iter().map(|&n| Comb::new(len(n, ch))).collect()),
            allpasses: [0, 1].map(|ch| ALLPASSES.iter().map(|&n| Allpass::new(len(n, ch))).collect()),
            predelay: DelayLine::new((0.21 * sr) as usize),
            low_cut: Biquad::default(),
            last_cut: 0.0,
        }
    }
}

impl Dsp for Reverb {
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32 {
        let feedback = p[0] * 0.28 + 0.7;
        let damp = p[1] * 0.4;
        let width = p[2];
        let pre = p[3] * 0.001 * ctx.sample_rate;
        let mix = p[5];
        if self.last_cut != p[4] {
            self.low_cut.c = Coeffs::highpass(p[4], std::f32::consts::FRAC_1_SQRT_2, ctx.sample_rate);
            self.last_cut = p[4];
        }
        let wet1 = width * 0.5 + 0.5;
        let wet2 = (1.0 - width) * 0.5;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            self.predelay.push((*a + *b) * 0.015);
            let x = self.low_cut.process(self.predelay.read(pre));
            let mut out = [0.0f32; 2];
            for ((o, combs), allpasses) in out.iter_mut().zip(&mut self.combs).zip(&mut self.allpasses) {
                let mut s = 0.0;
                for c in combs.iter_mut() {
                    s += c.process(x, feedback, damp);
                }
                for ap in allpasses.iter_mut() {
                    s = ap.process(s);
                }
                *o = s;
            }
            let (wl, wr) = (out[0] * wet1 + out[1] * wet2, out[1] * wet1 + out[0] * wet2);
            // Freeverb's wet level is about 3x the dry level at the same setting.
            *a = *a * (1.0 - mix) + wl * mix * 3.0;
            *b = *b * (1.0 - mix) + wr * mix * 3.0;
        }
        0.0
    }
}

// ---------------------------------------------------------------- distortion

const SHAPES: &[&str] = &["Soft", "Hard", "Fold", "Tube"];

pub const DISTORTION_PARAMS: &[ParamDef] = &[
    ParamDef::new("drive", "Drive", 0.0, 36.0, 12.0, Unit::Db),
    ParamDef::choice("shape", "Shape", SHAPES, 0),
    ParamDef::new("tone", "Tone", 500.0, 20000.0, 8000.0, Unit::Hz),
    ParamDef::new("output", "Output", -24.0, 6.0, -6.0, Unit::Db),
    ParamDef::new("mix", "Mix", 0.0, 1.0, 1.0, Unit::Percent),
];

pub struct Distortion {
    tone: [Biquad; 2],
    dc: [(f32, f32); 2],
    last_tone: f32,
}

impl Default for Distortion {
    fn default() -> Self {
        Self::new()
    }
}

impl Distortion {
    pub fn new() -> Self {
        Self {
            tone: [Biquad::default(); 2],
            dc: [(0.0, 0.0); 2],
            last_tone: 0.0,
        }
    }
}

fn shape(kind: usize, x: f32) -> f32 {
    match kind {
        0 => x.tanh(),
        1 => x.clamp(-1.0, 1.0),
        // Triangle fold: identity inside ±1, reflected outside.
        2 => 4.0 * ((x * 0.25 + 0.25).rem_euclid(1.0) - 0.5).abs() - 1.0,
        // Asymmetric tube-like curve; the DC it adds is removed afterwards.
        _ => {
            if x >= 0.0 {
                (x * 1.2).tanh()
            } else {
                (x * 0.8).tanh() * 0.9
            }
        }
    }
}

impl Dsp for Distortion {
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32 {
        let drive = db_to_gain(p[0]);
        let kind = p[1] as usize;
        let out = db_to_gain(p[3]);
        let mix = p[4];
        if self.last_tone != p[2] {
            for t in &mut self.tone {
                t.c = Coeffs::lowpass(p[2], std::f32::consts::FRAC_1_SQRT_2, ctx.sample_rate);
            }
            self.last_tone = p[2];
        }
        let dc_coef = 1.0 - (2.0 * std::f32::consts::PI * 10.0 / ctx.sample_rate);
        for (ch, buf) in [l, r].into_iter().enumerate() {
            for s in buf.iter_mut() {
                let y = self.tone[ch].process(shape(kind, *s * drive));
                // DC blocker
                let (x1, y1) = &mut self.dc[ch];
                let z = y - *x1 + dc_coef * *y1;
                *x1 = y;
                *y1 = z;
                *s = *s * (1.0 - mix) + z * out * mix;
            }
        }
        0.0
    }
}

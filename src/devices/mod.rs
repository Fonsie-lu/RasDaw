//! Devices bundled with RasDaw. They are configured only in the device rack (no editor
//! window) and saved in the project as a parameter map.
//!
//! Threading: parameter values are atomics written by the UI and read by the audio thread once
//! per block. The DSP state (`Box<dyn Dsp>`) is touched only by the audio thread; it and all its
//! buffers are allocated in [`BuiltinDevice::new`] on the main thread.

pub mod dsp;
mod dynamics;
pub mod eq;
mod fx;
pub mod notes;

use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub const MAX_PARAMS: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    Db,
    Hz,
    Ms,
    /// 0..1 shown as percent.
    Percent,
    Ratio,
    Q,
    /// Stepped: the value is the index into the labels.
    Choice(&'static [&'static str]),
}

#[derive(Clone, Copy, Debug)]
pub struct ParamDef {
    /// Stable key in project files. Never rename.
    pub key: &'static str,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Logarithmic knob travel (frequencies, times, ratios).
    pub log: bool,
    pub unit: Unit,
}

impl ParamDef {
    pub const fn new(key: &'static str, name: &'static str, min: f32, max: f32, default: f32, unit: Unit) -> Self {
        let log = matches!(unit, Unit::Hz | Unit::Ms | Unit::Ratio | Unit::Q);
        Self { key, name, min, max, default, log, unit }
    }

    pub const fn choice(key: &'static str, name: &'static str, labels: &'static [&'static str], default: usize) -> Self {
        Self {
            key,
            name,
            min: 0.0,
            max: (labels.len() - 1) as f32,
            default: default as f32,
            log: false,
            unit: Unit::Choice(labels),
        }
    }

    /// Linear knob travel even for time units (for ranges starting at 0).
    pub const fn linear(mut self) -> Self {
        self.log = false;
        self
    }

    pub fn clamp(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        if matches!(self.unit, Unit::Choice(_)) {
            v.round()
        } else {
            v
        }
    }

    pub fn to_norm(&self, v: f32) -> f32 {
        let v = self.clamp(v);
        if self.log {
            (v / self.min).ln() / (self.max / self.min).ln()
        } else {
            (v - self.min) / (self.max - self.min)
        }
    }

    pub fn from_norm(&self, n: f32) -> f32 {
        let n = n.clamp(0.0, 1.0);
        let v = if self.log {
            self.min * (self.max / self.min).powf(n)
        } else {
            self.min + n * (self.max - self.min)
        };
        self.clamp(v)
    }

    pub fn format(&self, v: f32) -> String {
        match self.unit {
            Unit::Db => format!("{v:+.1} dB"),
            Unit::Hz if v >= 1000.0 => format!("{:.2} kHz", v / 1000.0),
            Unit::Hz => format!("{v:.0} Hz"),
            Unit::Ms if v >= 1000.0 => format!("{:.2} s", v / 1000.0),
            Unit::Ms if v < 10.0 => format!("{v:.1} ms"),
            Unit::Ms => format!("{v:.0} ms"),
            Unit::Percent => format!("{:.0} %", v * 100.0),
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Q => format!("Q {v:.2}"),
            Unit::Choice(labels) => labels.get(v.round() as usize).copied().unwrap_or("?").to_owned(),
        }
    }
}

/// Per-block context for the DSP.
pub struct Ctx {
    pub sample_rate: f32,
    pub bpm: f32,
}

pub trait Dsp: Send {
    /// Processes stereo audio in place. `p` holds the current parameter values in the order
    /// of [`Kind::params`]. Returns the largest gain reduction in dB (dynamics), else 0.
    fn process(&mut self, p: &[f32], l: &mut [f32], r: &mut [f32], ctx: &Ctx) -> f32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Eq5,
    Compressor,
    Limiter,
    Delay,
    Reverb,
    Distortion,
    Chord,
    Arp,
    NoteRepeat,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Eq5,
        Kind::Compressor,
        Kind::Limiter,
        Kind::Distortion,
        Kind::Delay,
        Kind::Reverb,
        Kind::Chord,
        Kind::Arp,
        Kind::NoteRepeat,
    ];

    /// Note-altering device (rack section NOTE FX) rather than an audio effect.
    pub fn is_note(self) -> bool {
        matches!(self, Kind::Chord | Kind::Arp | Kind::NoteRepeat)
    }

    pub fn section(self) -> crate::model::Section {
        if self.is_note() {
            crate::model::Section::NoteFx
        } else {
            crate::model::Section::Effect
        }
    }

    /// Plugin id stored in `DeviceSpec::plugin_id`. Never change.
    pub fn id(self) -> &'static str {
        match self {
            Kind::Eq5 => "rasdaw.eq5",
            Kind::Compressor => "rasdaw.compressor",
            Kind::Limiter => "rasdaw.limiter",
            Kind::Delay => "rasdaw.delay",
            Kind::Reverb => "rasdaw.reverb",
            Kind::Distortion => "rasdaw.distortion",
            Kind::Chord => "rasdaw.chord",
            Kind::Arp => "rasdaw.arp",
            Kind::NoteRepeat => "rasdaw.note-repeat",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Eq5 => "EQ-5",
            Kind::Compressor => "Compressor",
            Kind::Limiter => "Limiter",
            Kind::Delay => "Delay",
            Kind::Reverb => "Reverb",
            Kind::Distortion => "Distortion",
            Kind::Chord => "Chord",
            Kind::Arp => "Arp",
            Kind::NoteRepeat => "Note Repeat",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Kind::Eq5 => "5-band EQ; cuts below 20 Hz and above 16 kHz, mono below 100 Hz",
            Kind::Compressor => "Feed-forward compressor with soft knee",
            Kind::Limiter => "Look-ahead brickwall limiter",
            Kind::Delay => "Stereo / ping-pong delay with tempo sync",
            Kind::Reverb => "Algorithmic room reverb",
            Kind::Distortion => "Waveshaping distortion",
            Kind::Chord => "Plays a chord for every note, with inversion, octave and strum",
            Kind::Arp => "Arpeggiator synced to the song tempo",
            Kind::NoteRepeat => "Retriggers held notes on the tempo grid",
        }
    }

    pub fn from_id(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.id() == id)
    }

    pub fn params(self) -> &'static [ParamDef] {
        match self {
            Kind::Eq5 => eq::PARAMS,
            Kind::Compressor => dynamics::COMPRESSOR_PARAMS,
            Kind::Limiter => dynamics::LIMITER_PARAMS,
            Kind::Delay => fx::DELAY_PARAMS,
            Kind::Reverb => fx::REVERB_PARAMS,
            Kind::Distortion => fx::DISTORTION_PARAMS,
            Kind::Chord => notes::CHORD_PARAMS,
            Kind::Arp => notes::ARP_PARAMS,
            Kind::NoteRepeat => notes::REPEAT_PARAMS,
        }
    }

    /// Fixed number of knob rows in the rack (the EQ shows one band per column).
    pub fn rows(self) -> Option<usize> {
        match self {
            Kind::Eq5 => Some(3),
            _ => None,
        }
    }

    /// Whether parameter `index` has an effect with the current `values` (the rack dims the
    /// others, e.g. the delay time while tempo sync is on).
    pub fn param_active(self, index: usize, values: &[f32]) -> bool {
        match (self, index) {
            (Kind::Delay, 0) => values[1] < 0.5,
            (Kind::Delay, 2) => values[1] >= 0.5,
            _ => true,
        }
    }

    /// Whether the rack shows a gain reduction meter.
    pub fn has_gr_meter(self) -> bool {
        matches!(self, Kind::Compressor | Kind::Limiter)
    }

    fn make_processor(self, sr: f32) -> Processor {
        match self {
            Kind::Eq5 => Processor::Audio(Box::new(eq::Eq5::new())),
            Kind::Compressor => Processor::Audio(Box::new(dynamics::Compressor::new())),
            Kind::Limiter => Processor::Audio(Box::new(dynamics::Limiter::new(sr))),
            Kind::Delay => Processor::Audio(Box::new(fx::Delay::new(sr))),
            Kind::Reverb => Processor::Audio(Box::new(fx::Reverb::new(sr))),
            Kind::Distortion => Processor::Audio(Box::new(fx::Distortion::new())),
            Kind::Chord => Processor::Notes(Box::<notes::Chord>::default()),
            Kind::Arp => Processor::Notes(Box::<notes::Arp>::default()),
            Kind::NoteRepeat => Processor::Notes(Box::<notes::NoteRepeat>::default()),
        }
    }
}

enum Processor {
    Audio(Box<dyn Dsp>),
    Notes(Box<dyn notes::NoteDsp>),
}

pub struct BuiltinDevice {
    pub uid: u64,
    pub kind: Kind,
    pub sample_rate: f32,
    values: Vec<AtomicU32>,
    /// Largest gain reduction of the last block, in dB (f32 bits).
    gain_reduction: AtomicU32,
    dsp: UnsafeCell<Processor>,
}

// `dsp` is only used by the audio thread (see the module docs).
unsafe impl Sync for BuiltinDevice {}

impl BuiltinDevice {
    pub fn new(kind: Kind, sample_rate: f32) -> Arc<Self> {
        let params = kind.params();
        assert!(params.len() <= MAX_PARAMS);
        Arc::new(Self {
            uid: crate::clap_host::next_uid(),
            kind,
            sample_rate,
            values: params.iter().map(|p| AtomicU32::new(p.default.to_bits())).collect(),
            gain_reduction: AtomicU32::new(0),
            dsp: UnsafeCell::new(kind.make_processor(sample_rate)),
        })
    }

    pub fn params(&self) -> &'static [ParamDef] {
        self.kind.params()
    }

    pub fn value(&self, index: usize) -> f32 {
        self.values
            .get(index)
            .map_or(0.0, |v| f32::from_bits(v.load(Ordering::Relaxed)))
    }

    pub fn values(&self) -> Vec<f32> {
        (0..self.values.len()).map(|i| self.value(i)).collect()
    }

    pub fn set_value(&self, index: usize, value: f32) {
        if let (Some(slot), Some(def)) = (self.values.get(index), self.params().get(index)) {
            slot.store(def.clamp(value).to_bits(), Ordering::Relaxed);
        }
    }

    pub fn gain_reduction_db(&self) -> f32 {
        f32::from_bits(self.gain_reduction.load(Ordering::Relaxed))
    }

    /// Parameter values keyed by `ParamDef::key`, for the project file.
    pub fn to_map(&self) -> BTreeMap<String, f32> {
        self.params()
            .iter()
            .enumerate()
            .map(|(i, p)| (p.key.to_owned(), self.value(i)))
            .collect()
    }

    /// Applies a saved map; unknown keys are ignored, missing ones keep their value.
    pub fn load_map(&self, map: &BTreeMap<String, f32>) {
        for (i, p) in self.params().iter().enumerate() {
            if let Some(&v) = map.get(p.key) {
                self.set_value(i, v);
            }
        }
    }

    fn param_snapshot(&self) -> [f32; MAX_PARAMS] {
        let mut p = [0f32; MAX_PARAMS];
        for (i, v) in self.values.iter().enumerate() {
            p[i] = f32::from_bits(v.load(Ordering::Relaxed));
        }
        p
    }

    /// Processes `n` frames from `input` into `output`. Returns false for note devices, which
    /// leave the audio alone. Audio thread only.
    pub fn process(&self, input: [&[f32]; 2], output: [&mut [f32]; 2], n: usize, bpm: f64) -> bool {
        let Processor::Audio(dsp) = (unsafe { &mut *self.dsp.get() }) else {
            return false;
        };
        let p = self.param_snapshot();
        let [l, r] = output;
        l[..n].copy_from_slice(&input[0][..n]);
        r[..n].copy_from_slice(&input[1][..n]);
        let ctx = Ctx {
            sample_rate: self.sample_rate,
            bpm: bpm as f32,
        };
        let gr = dsp.process(&p[..self.values.len()], &mut l[..n], &mut r[..n], &ctx);
        self.gain_reduction.store(gr.to_bits(), Ordering::Relaxed);
        true
    }

    /// Note devices: writes the device's output for the `input` events into `out` (cleared
    /// first, sorted on return). Audio devices copy the input. Audio thread only.
    pub fn process_notes(&self, input: &[crate::engine::NoteEvent], out: &mut Vec<crate::engine::NoteEvent>, ctx: &notes::NoteCtx) {
        out.clear();
        match unsafe { &mut *self.dsp.get() } {
            Processor::Notes(dsp) => {
                let p = self.param_snapshot();
                dsp.process(&p[..self.values.len()], input, out, ctx);
                crate::engine::sequencer::sort_events(out);
            }
            Processor::Audio(_) => out.extend_from_slice(&input[..input.len().min(out.capacity())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: Kind, setup: impl Fn(&BuiltinDevice), input: impl Fn(usize) -> (f32, f32), frames: usize) -> (Vec<f32>, Vec<f32>) {
        let dev = BuiltinDevice::new(kind, 48_000.0);
        setup(&dev);
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        let block = 256;
        let mut i = 0;
        while i < frames {
            let n = block.min(frames - i);
            let il: Vec<f32> = (i..i + n).map(|k| input(k).0).collect();
            let ir: Vec<f32> = (i..i + n).map(|k| input(k).1).collect();
            let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
            dev.process([&il, &ir], [&mut l, &mut r], n, 120.0);
            ol.extend(l);
            or.extend(r);
            i += n;
        }
        (ol, or)
    }

    fn sine(f: f32, amp: f32) -> impl Fn(usize) -> f32 {
        move |k| amp * (2.0 * std::f32::consts::PI * f * k as f32 / 48_000.0).sin()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn params_map_round_trip_and_norm() {
        for kind in Kind::ALL {
            let dev = BuiltinDevice::new(kind, 48_000.0);
            for (i, p) in dev.params().iter().enumerate() {
                let n = p.to_norm(p.default);
                assert!((p.from_norm(n) - p.default).abs() <= (p.max - p.min) * 1e-4, "{} {}", kind.name(), p.key);
                dev.set_value(i, p.from_norm(0.3));
            }
            let map = dev.to_map();
            let other = BuiltinDevice::new(kind, 48_000.0);
            other.load_map(&map);
            assert_eq!(dev.values(), other.values());
        }
    }

    #[test]
    fn every_device_stays_finite_and_makes_sound() {
        let noise = |k: usize| {
            let x = ((k as u32).wrapping_mul(2654435761) >> 8) as f32 / (1u32 << 24) as f32 - 0.5;
            (x, -x * 0.5)
        };
        for kind in Kind::ALL.into_iter().filter(|k| !k.is_note()) {
            let (l, r) = run(kind, |_| {}, noise, 48_000);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{}", kind.name());
            assert!(peak(&l[24_000..]) > 0.01, "{} is silent", kind.name());
            assert!(peak(&l) < 4.0, "{} blew up", kind.name());
        }
    }

    #[test]
    fn limiter_holds_the_ceiling() {
        // +12 dB input gain into -1 dB ceiling.
        let (l, _) = run(
            Kind::Limiter,
            |d| {
                d.set_value(0, 12.0);
                d.set_value(1, -1.0);
            },
            |k| (sine(220.0, 0.9)(k), sine(220.0, 0.9)(k)),
            48_000,
        );
        assert!(peak(&l) <= dsp::db_to_gain(-1.0) + 1e-4, "peak {}", peak(&l));
        assert!(peak(&l[24_000..]) > dsp::db_to_gain(-2.0));
    }

    #[test]
    fn compressor_reduces_loud_signals() {
        let (l, _) = run(
            Kind::Compressor,
            |d| {
                d.set_value(0, -30.0); // threshold
                d.set_value(1, 8.0); // ratio
            },
            |k| (sine(440.0, 0.9)(k), sine(440.0, 0.9)(k)),
            48_000,
        );
        assert!(peak(&l[24_000..]) < 0.5, "peak {}", peak(&l[24_000..]));
    }

    #[test]
    fn eq_cuts_extremes_and_monos_the_bass() {
        let mono_side = |f: f32| {
            // Pure side signal (L = -R): below 100 Hz it must vanish.
            let (l, r) = run(Kind::Eq5, |_| {}, |k| (sine(f, 0.5)(k), -sine(f, 0.5)(k)), 48_000);
            peak(&l[24_000..]).max(peak(&r[24_000..]))
        };
        assert!(mono_side(40.0) < 0.05, "side at 40 Hz: {}", mono_side(40.0));
        assert!(mono_side(1000.0) > 0.45);
        let level = |f: f32| {
            let (l, _) = run(Kind::Eq5, |_| {}, |k| (sine(f, 0.5)(k), sine(f, 0.5)(k)), 48_000);
            peak(&l[24_000..])
        };
        assert!(level(10.0) < 0.1, "10 Hz: {}", level(10.0));
        assert!(level(19_000.0) < 0.1, "19 kHz: {}", level(19_000.0));
        assert!((level(1000.0) - 0.5).abs() < 0.03, "1 kHz: {}", level(1000.0));
    }

    #[test]
    fn delay_repeats_after_the_set_time() {
        let (l, _) = run(
            Kind::Delay,
            |d| {
                d.set_value(1, 0.0); // sync off
                d.set_value(0, 100.0); // 100 ms
                d.set_value(7, 1.0); // mix fully wet
            },
            |k| if k == 0 { (1.0, 1.0) } else { (0.0, 0.0) },
            9_600,
        );
        let at = l.iter().enumerate().skip(10).max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        assert!((at as i32 - 4800).abs() < 8, "echo at {at}");
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    /// Rough CPU cost per device (run with --nocapture; release-like dev profile).
    #[test]
    fn report_cpu_cost() {
        let n = 48_000;
        let input: Vec<f32> = (0..n).map(|k| (k as f32 * 0.01).sin() * 0.5).collect();
        for kind in Kind::ALL.into_iter().filter(|k| !k.is_note()) {
            let dev = BuiltinDevice::new(kind, 48_000.0);
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            let start = std::time::Instant::now();
            for chunk in input.chunks(256) {
                let m = chunk.len();
                dev.process([chunk, chunk], [&mut l[..m], &mut r[..m]], m, 120.0);
            }
            let pct = start.elapsed().as_secs_f64() * 100.0;
            eprintln!("{:<11} {:>6.2} % of one core at 48 kHz", kind.name(), pct);
        }
    }
}

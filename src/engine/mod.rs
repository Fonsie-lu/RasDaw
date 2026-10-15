//! Real-time engine. The UI owns an [`EngineHandle`]; the audio thread owns the [`Engine`].
//!
//! - The UI publishes immutable [`Snapshot`]s (tracks, note sources, device chains). The engine
//!   swaps them in at the start of a block and sends the old one back, so nothing is ever freed
//!   on the audio thread. The last reference to a removed plugin instance therefore dies on the
//!   UI thread, after the engine called `stop_processing` on it.
//! - Transport and note preview go through a command queue.
//! - Playhead position and meters come back through atomics in [`Shared`].

pub mod sequencer;

pub use sequencer::NoteEvent;

use crate::clap_host::PluginInstance;
use crate::model::{PlayMode, Project, BEATS_PER_BAR};
use clap_sys::events::*;
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use sequencer::{Source, Span, Voices};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub const MAX_TRACKS: usize = 128;

pub struct DeviceSnap {
    pub instance: Arc<PluginInstance>,
    pub bypass: bool,
}

pub struct TrackSnap {
    pub id: u64,
    pub source: Source,
    /// Changes whenever `source` changes; the engine then releases the track's sounding notes.
    pub source_key: u64,
    /// Per-channel gain including volume, pan, mute and solo.
    pub gain: [f32; 2],
    pub devices: Vec<DeviceSnap>,
}

pub struct Snapshot {
    pub tracks: Vec<TrackSnap>,
    pub bpm: f64,
    pub loop_enabled: bool,
    pub loop_end: f64,
}

impl Snapshot {
    pub fn empty() -> Self {
        Self {
            tracks: Vec::new(),
            bpm: 120.0,
            loop_enabled: false,
            loop_end: 0.0,
        }
    }

    /// Builds the engine view of a project. `devices` returns the live chain of a track.
    pub fn build(project: &Project, mut devices: impl FnMut(u64) -> Vec<DeviceSnap>) -> Self {
        let any_solo = project.tracks.iter().any(|t| t.solo);
        let tracks = project
            .tracks
            .iter()
            .take(MAX_TRACKS)
            .map(|t| {
                let mode = t.play_mode();
                let source = match mode {
                    PlayMode::Arrangement => Source::Arrangement(sequencer::arrangement_notes(
                        t.arrangement
                            .notes
                            .iter()
                            .map(|n| (n.start, n.len, n.key, n.velocity)),
                    )),
                    PlayMode::Scene(i) => {
                        let clip = t.scenes[i].as_ref().expect("play_mode checked the clip");
                        Source::Scene {
                            notes: sequencer::scene_notes(
                                clip.notes.iter().map(|n| (n.start, n.len, n.key, n.velocity)),
                                clip.length,
                            ),
                            length: clip.length,
                        }
                    }
                    PlayMode::Silent => Source::Silent,
                };
                let mut h = std::collections::hash_map::DefaultHasher::new();
                format!("{mode:?}").hash(&mut h);
                if let Source::Scene { length, .. } = &source {
                    length.to_bits().hash(&mut h);
                }
                if let Source::Arrangement(n) | Source::Scene { notes: n, .. } = &source {
                    for n in n {
                        (n.start.to_bits(), n.end.to_bits(), n.key, n.velocity.to_bits()).hash(&mut h);
                    }
                }
                let audible = !t.mute && (!any_solo || t.solo);
                let level = if audible { volume_gain(t.volume) } else { 0.0 };
                let angle = (t.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
                TrackSnap {
                    id: t.id,
                    source,
                    source_key: h.finish(),
                    gain: [
                        level * angle.cos() * std::f32::consts::SQRT_2,
                        level * angle.sin() * std::f32::consts::SQRT_2,
                    ],
                    devices: devices(t.id),
                }
            })
            .collect();
        Self {
            tracks,
            bpm: project.bpm,
            loop_enabled: project.loop_enabled,
            loop_end: project.length_beats(),
        }
    }
}

/// Fader position 0..1 to linear gain; 0.8 is unity, 1.0 is about +3.9 dB.
pub fn volume_gain(volume: f32) -> f32 {
    let v = volume.clamp(0.0, 1.0) / 0.8;
    v * v
}

pub enum Command {
    Play,
    /// Stops; a second stop while stopped rewinds to the start.
    Stop,
    Seek(f64),
    /// Live note from the UI (piano roll keyboard).
    Note { track: u64, key: u8, velocity: f32, on: bool },
}

pub struct Shared {
    playing: AtomicBool,
    position: AtomicU64,
    peaks: Vec<[AtomicU32; 2]>,
    master: [AtomicU32; 2],
    load: AtomicU32,
}

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            position: AtomicU64::new(0f64.to_bits()),
            peaks: (0..MAX_TRACKS).map(|_| [AtomicU32::new(0), AtomicU32::new(0)]).collect(),
            master: [AtomicU32::new(0), AtomicU32::new(0)],
            load: AtomicU32::new(0),
        }
    }

    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }

    /// Playhead in beats.
    pub fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }

    /// Peak since the last call, per channel, for the track at `index` in the snapshot.
    pub fn take_track_peak(&self, index: usize) -> [f32; 2] {
        match self.peaks.get(index) {
            Some(p) => [take_peak(&p[0]), take_peak(&p[1])],
            None => [0.0; 2],
        }
    }

    pub fn take_master_peak(&self) -> [f32; 2] {
        [take_peak(&self.master[0]), take_peak(&self.master[1])]
    }

    /// Share of the buffer period spent rendering (0..1).
    pub fn load(&self) -> f32 {
        f32::from_bits(self.load.load(Ordering::Relaxed))
    }
}

fn take_peak(a: &AtomicU32) -> f32 {
    f32::from_bits(a.swap(0, Ordering::Relaxed))
}

/// Positive floats order like their bit patterns, so `fetch_max` on the bits works.
fn store_peak(a: &AtomicU32, v: f32) {
    if v.is_finite() {
        a.fetch_max(v.abs().to_bits(), Ordering::Relaxed);
    }
}

pub struct EngineHandle {
    commands: rtrb::Producer<Command>,
    snapshots: rtrb::Producer<Arc<Snapshot>>,
    garbage: rtrb::Consumer<Arc<Snapshot>>,
    pub shared: Arc<Shared>,
    pub sample_rate: f64,
    pub max_block: u32,
}

impl EngineHandle {
    pub fn send(&mut self, cmd: Command) {
        let _ = self.commands.push(cmd);
    }

    pub fn publish(&mut self, snapshot: Snapshot) {
        if self.snapshots.push(Arc::new(snapshot)).is_err() {
            eprintln!("engine: snapshot queue full, dropping update");
        }
    }

    /// Drops snapshots the engine is done with. Call regularly from the UI thread.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

struct TrackRt {
    id: u64,
    source_key: u64,
    voices: Voices,
    live: [bool; 128],
    release: bool,
    pending: [(u8, f32, bool); 32],
    pending_len: usize,
}

impl TrackRt {
    fn new(id: u64) -> Self {
        Self {
            id,
            source_key: 0,
            voices: Voices::default(),
            live: [false; 128],
            release: false,
            pending: [(0, 0.0, false); 32],
            pending_len: 0,
        }
    }
}

struct Bufs {
    /// Chain input and output; swapped after every device.
    a: [Vec<f32>; 2],
    b: [Vec<f32>; 2],
    master: [Vec<f32>; 2],
}

pub struct Engine {
    commands: rtrb::Consumer<Command>,
    snapshots: rtrb::Consumer<Arc<Snapshot>>,
    garbage: rtrb::Producer<Arc<Snapshot>>,
    /// Retired snapshots that did not fit into the garbage queue yet.
    held: Vec<Arc<Snapshot>>,
    current: Arc<Snapshot>,
    rt: Vec<TrackRt>,
    rt_scratch: Vec<TrackRt>,
    events: Vec<NoteEvent>,
    bufs: Bufs,
    shared: Arc<Shared>,
    sample_rate: f64,
    max_block: usize,
    playing: bool,
    position: f64,
    release_all: bool,
}

pub fn new(sample_rate: f64, max_block: u32) -> (Engine, EngineHandle) {
    let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(1024);
    let (snap_tx, snap_rx) = rtrb::RingBuffer::new(64);
    let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(256);
    let shared = Arc::new(Shared::new());
    let n = max_block as usize;
    let buf = || [vec![0.0f32; n], vec![0.0f32; n]];
    let engine = Engine {
        commands: cmd_rx,
        snapshots: snap_rx,
        garbage: garbage_tx,
        held: Vec::with_capacity(64),
        current: Arc::new(Snapshot::empty()),
        rt: Vec::with_capacity(MAX_TRACKS),
        rt_scratch: Vec::with_capacity(MAX_TRACKS),
        events: Vec::with_capacity(4096),
        bufs: Bufs {
            a: buf(),
            b: buf(),
            master: buf(),
        },
        shared: shared.clone(),
        sample_rate,
        max_block: n,
        playing: false,
        position: 0.0,
        release_all: false,
    };
    let handle = EngineHandle {
        commands: cmd_tx,
        snapshots: snap_tx,
        garbage: garbage_rx,
        shared,
        sample_rate,
        max_block,
    };
    (engine, handle)
}

impl Engine {
    /// Renders `out.len() / 2` interleaved stereo frames.
    pub fn render_interleaved(&mut self, out: &mut [f32]) {
        let started = Instant::now();
        let frames = out.len() / 2;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.max_block);
            self.render_block(n);
            let [l, r] = &self.bufs.master;
            for i in 0..n {
                out[(done + i) * 2] = l[i];
                out[(done + i) * 2 + 1] = r[i];
            }
            done += n;
        }
        let period = frames as f64 / self.sample_rate;
        if period > 0.0 {
            let load = (started.elapsed().as_secs_f64() / period) as f32;
            let old = self.shared.load();
            self.shared
                .load
                .store((old * 0.9 + load * 0.1).to_bits(), Ordering::Relaxed);
        }
    }

    /// Renders one block of at most `max_block` frames into `bufs.master`.
    pub fn render_block(&mut self, n: usize) {
        self.take_snapshots();
        self.handle_commands();

        let snap = self.current.clone();
        let fpb = self.sample_rate * 60.0 / snap.bpm.max(1.0);
        let block_beats = n as f64 / fpb;

        // Transport: one span, or two when the loop end falls inside this block.
        let mut spans: [Option<Span>; 2] = [None, None];
        let mut wrap_at: Option<u32> = None;
        let mut next_position = self.position;
        if self.playing {
            let p = self.position;
            let span = |b0: f64, b1: f64, frame0: f64| Span {
                b0,
                b1,
                frame0,
                frames_per_beat: fpb,
                frames: n as u32,
            };
            if snap.loop_enabled && snap.loop_end > 0.0 && p + block_beats >= snap.loop_end {
                // Exact (fractional) frame of the loop end; beat 0 starts there.
                let first = ((snap.loop_end - p).max(0.0) * fpb).min(n as f64);
                if snap.loop_end > p {
                    spans[0] = Some(span(p, snap.loop_end, 0.0));
                }
                let rest = (p + block_beats - snap.loop_end).max(0.0);
                wrap_at = Some(first.round() as u32);
                if rest > 0.0 {
                    spans[1] = Some(span(0.0, rest, first));
                }
                next_position = rest;
            } else {
                spans[0] = Some(span(p, p + block_beats, 0.0));
                next_position = p + block_beats;
            }
        }
        let transport = self.transport_event(&snap);

        for b in &mut self.bufs.master {
            b[..n].fill(0.0);
        }
        let release_all = std::mem::take(&mut self.release_all);
        for (i, track) in snap.tracks.iter().enumerate() {
            let rt = &mut self.rt[i];
            let events = &mut self.events;
            events.clear();
            if release_all || rt.release {
                rt.release = false;
                rt.voices.release_all(0, events);
            }
            for &(key, velocity, on) in &rt.pending[..rt.pending_len] {
                let k = key as usize & 127;
                if on && rt.live[k] {
                    events.push(NoteEvent { time: 0, on: false, key, velocity: 0.0 });
                }
                rt.live[k] = on;
                events.push(NoteEvent { time: 0, on, key, velocity });
            }
            rt.pending_len = 0;
            if let Some(s) = spans[0] {
                sequencer::render(&track.source, &mut rt.voices, s, events);
            }
            if let Some(t) = wrap_at {
                rt.voices.release_all(t.min(n as u32 - 1), events);
            }
            if let Some(s) = spans[1] {
                sequencer::render(&track.source, &mut rt.voices, s, events);
            }
            sequencer::sort_events(events);

            // Device chain: notes go to every device, audio flows left to right.
            let bufs = &mut self.bufs;
            for b in &mut bufs.a {
                b[..n].fill(0.0);
            }
            for dev in &track.devices {
                if dev.bypass {
                    continue;
                }
                let [il, ir] = &bufs.a;
                let [ol, or] = &mut bufs.b;
                if dev.instance.process([il, ir], [ol, or], n, events, &transport) {
                    std::mem::swap(&mut bufs.a, &mut bufs.b);
                }
            }
            let [l, r] = &bufs.a;
            let [ml, mr] = &mut bufs.master;
            let (mut pl, mut pr) = (0.0f32, 0.0f32);
            for j in 0..n {
                let a = l[j] * track.gain[0];
                let b = r[j] * track.gain[1];
                ml[j] += a;
                mr[j] += b;
                pl = pl.max(a.abs());
                pr = pr.max(b.abs());
            }
            if let Some(p) = self.shared.peaks.get(i) {
                store_peak(&p[0], pl);
                store_peak(&p[1], pr);
            }
        }
        let [ml, mr] = &mut self.bufs.master;
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for j in 0..n {
            // Guard the speakers against broken plugins.
            ml[j] = if ml[j].is_finite() { ml[j].clamp(-4.0, 4.0) } else { 0.0 };
            mr[j] = if mr[j].is_finite() { mr[j].clamp(-4.0, 4.0) } else { 0.0 };
            pl = pl.max(ml[j].abs());
            pr = pr.max(mr[j].abs());
        }
        store_peak(&self.shared.master[0], pl);
        store_peak(&self.shared.master[1], pr);

        self.position = next_position;
        self.shared
            .position
            .store(self.position.to_bits(), Ordering::Relaxed);
        self.shared.playing.store(self.playing, Ordering::Relaxed);
    }

    fn transport_event(&self, snap: &Snapshot) -> clap_event_transport {
        let beats = self.position;
        let seconds = beats * 60.0 / snap.bpm.max(1.0);
        let bar = (beats / BEATS_PER_BAR).floor();
        let mut flags = CLAP_TRANSPORT_HAS_TEMPO
            | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
            | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
            | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
        if self.playing {
            flags |= CLAP_TRANSPORT_IS_PLAYING;
        }
        if snap.loop_enabled {
            flags |= CLAP_TRANSPORT_IS_LOOP_ACTIVE;
        }
        let bt = |b: f64| (b * CLAP_BEATTIME_FACTOR as f64).round() as i64;
        let st = |s: f64| (s * CLAP_SECTIME_FACTOR as f64).round() as i64;
        clap_event_transport {
            header: clap_event_header {
                size: std::mem::size_of::<clap_event_transport>() as u32,
                time: 0,
                space_id: CLAP_CORE_EVENT_SPACE_ID,
                type_: CLAP_EVENT_TRANSPORT,
                flags: 0,
            },
            flags,
            song_pos_beats: bt(beats),
            song_pos_seconds: st(seconds),
            tempo: snap.bpm,
            tempo_inc: 0.0,
            loop_start_beats: 0,
            loop_end_beats: bt(snap.loop_end),
            loop_start_seconds: 0,
            loop_end_seconds: st(snap.loop_end * 60.0 / snap.bpm.max(1.0)),
            bar_start: bt(bar * BEATS_PER_BAR),
            bar_number: bar as i32,
            tsig_num: 4,
            tsig_denom: 4,
        }
    }

    fn take_snapshots(&mut self) {
        // Retry handing back snapshots that did not fit earlier.
        while let Some(s) = self.held.pop() {
            if let Err(rtrb::PushError::Full(s)) = self.garbage.push(s) {
                self.held.push(s);
                break;
            }
        }
        while let Ok(new) = self.snapshots.pop() {
            let old = std::mem::replace(&mut self.current, new);
            // Instances that left every chain stop processing here, on the audio thread.
            for t in &old.tracks {
                for d in &t.devices {
                    let still_used = self
                        .current
                        .tracks
                        .iter()
                        .any(|nt| nt.devices.iter().any(|nd| nd.instance.uid == d.instance.uid));
                    if !still_used {
                        d.instance.stop_processing();
                    }
                }
            }
            if let Err(rtrb::PushError::Full(old)) = self.garbage.push(old) {
                if self.held.len() < self.held.capacity() {
                    self.held.push(old);
                } else {
                    // Should never happen: the UI collects every frame. Leaking beats freeing
                    // on the audio thread.
                    std::mem::forget(old);
                }
            }
            self.remap_tracks();
        }
    }

    /// Keeps per-track runtime state (sounding notes) attached to the right track ids.
    fn remap_tracks(&mut self) {
        self.rt_scratch.clear();
        for t in &self.current.tracks {
            let rt = match self.rt.iter().position(|r| r.id == t.id) {
                Some(i) => {
                    let mut rt = self.rt.swap_remove(i);
                    if rt.source_key != t.source_key {
                        rt.release = true;
                    }
                    rt
                }
                None => TrackRt::new(t.id),
            };
            let mut rt = rt;
            rt.source_key = t.source_key;
            self.rt_scratch.push(rt);
        }
        std::mem::swap(&mut self.rt, &mut self.rt_scratch);
        self.rt_scratch.clear();
    }

    fn handle_commands(&mut self) {
        while let Ok(cmd) = self.commands.pop() {
            match cmd {
                Command::Play => self.playing = true,
                Command::Stop => {
                    if !self.playing {
                        self.position = 0.0;
                    }
                    self.playing = false;
                    self.release_all = true;
                }
                Command::Seek(beat) => {
                    self.position = beat.max(0.0);
                    self.release_all = true;
                }
                Command::Note { track, key, velocity, on } => {
                    if let Some(rt) = self.rt.iter_mut().find(|r| r.id == track) {
                        if rt.pending_len < rt.pending.len() {
                            rt.pending[rt.pending_len] = (key, velocity, on);
                            rt.pending_len += 1;
                        }
                    }
                }
            }
        }
    }
}

impl Drop for Engine {
    /// The audio thread ends: stop every instance there, before the UI deactivates them.
    fn drop(&mut self) {
        for t in &self.current.tracks {
            for d in &t.devices {
                d.instance.stop_processing();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_loops_and_reports_position() {
        let mut p = Project::demo();
        p.bars = 1; // 4-beat loop
        let (mut engine, mut handle) = new(48_000.0, 512);
        handle.publish(Snapshot::build(&p, |_| Vec::new()));
        handle.send(Command::Play);
        // 120 bpm: 24000 samples per beat; render 5 beats.
        let mut out = vec![0.0; 2 * 1000];
        for _ in 0..120 {
            engine.render_interleaved(&mut out);
        }
        let pos = handle.shared.position();
        assert!((pos - 1.0).abs() < 1e-6, "position {pos}");
        assert!(handle.shared.playing());
        handle.send(Command::Stop);
        handle.send(Command::Stop);
        engine.render_interleaved(&mut out);
        assert!(!handle.shared.playing());
        assert!(handle.shared.position() == 0.0);
        handle.collect_garbage();
    }
}

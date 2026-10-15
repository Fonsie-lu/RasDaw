//! Real-time engine. The UI owns an [`EngineHandle`]; the audio thread owns the [`Engine`].
//!
//! - The UI publishes immutable [`Snapshot`]s (tracks, note sources, device chains). The engine
//!   swaps them in at the start of a block and sends the old one back, so nothing is ever freed
//!   on the audio thread. The last reference to a removed plugin instance therefore dies on the
//!   UI thread, after the engine called `stop_processing` on it.
//! - Transport, scene launches and note preview go through a command queue.
//! - Scenes run on their own launcher clock, independent of the arranger transport. A launch
//!   waits for the next point of the launch grid (`Command::SetLaunchQuantize`), then the
//!   scene loops until it is stopped or replaced.
//! - Live notes of the record-armed track come back as [`RecEvent`]s with their position in
//!   the playing scene, so the UI can write them into the clip.
//! - Playhead position and meters come back through atomics in [`Shared`].

pub mod sequencer;

pub use sequencer::NoteEvent;

use crate::clap_host::PluginInstance;
use crate::devices::notes::NoteCtx;
use crate::devices::BuiltinDevice;
use crate::midi::MidiMsg;
use crate::model::{Project, BEATS_PER_BAR, SCENES};
use clap_sys::events::*;
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use sequencer::{Source, Span, Voices};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

pub const MAX_TRACKS: usize = 128;

/// A device in a track chain: a hosted CLAP plugin or a built-in RasDaw device.
#[derive(Clone)]
pub enum DeviceRef {
    Clap(Arc<PluginInstance>),
    Builtin(Arc<BuiltinDevice>),
}

impl DeviceRef {
    pub fn uid(&self) -> u64 {
        match self {
            DeviceRef::Clap(i) => i.uid,
            DeviceRef::Builtin(d) => d.uid,
        }
    }

    /// Audio thread only. Returns false if nothing was written to `output`.
    #[allow(clippy::too_many_arguments)]
    fn process(
        &self,
        input: [&[f32]; 2],
        output: [&mut [f32]; 2],
        n: usize,
        notes: &[NoteEvent],
        midi: &[MidiMsg],
        transport: &clap_event_transport,
    ) -> bool {
        match self {
            DeviceRef::Clap(i) => i.process(input, output, n, notes, midi, transport),
            DeviceRef::Builtin(d) => d.process(input, output, n, transport.tempo),
        }
    }

    /// Audio thread only.
    fn stop_processing(&self) {
        if let DeviceRef::Clap(i) = self {
            i.stop_processing();
        }
    }
}

pub struct DeviceSnap {
    pub device: DeviceRef,
    pub bypass: bool,
    /// Note-altering device: the devices after it get its output notes instead of the input.
    pub note_fx: bool,
}

pub struct TrackSnap {
    pub id: u64,
    /// Timeline blocks as notes (`Source::Arrangement`), or `Silent` without blocks.
    pub blocks: Source,
    pub scenes: [Source; SCENES],
    /// Content hashes; when what a track plays changes, its sounding notes are released.
    pub blocks_key: u64,
    pub scene_keys: [u64; SCENES],
    /// Per-channel gain including volume, pan, mute and solo.
    pub gain: [f32; 2],
    pub devices: Vec<DeviceSnap>,
}

pub struct Snapshot {
    pub tracks: Vec<TrackSnap>,
    /// Effects on the sum of all tracks, and the master volume/pan gain.
    pub master: Vec<DeviceSnap>,
    pub master_gain: [f32; 2],
    pub bpm: f64,
    pub loop_enabled: bool,
    pub loop_end: f64,
}

impl Snapshot {
    pub fn empty() -> Self {
        Self {
            tracks: Vec::new(),
            master: Vec::new(),
            master_gain: [1.0; 2],
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
                let blocks = if t.has_arrangement() {
                    Source::Arrangement(sequencer::arrangement_notes(block_notes(t).into_iter()))
                } else {
                    Source::Silent
                };
                let scenes: [Source; SCENES] = std::array::from_fn(|s| match &t.scenes[s] {
                    Some(clip) => Source::Scene {
                        notes: sequencer::scene_notes(
                            clip.notes.iter().map(|n| (n.start, n.len, n.key, n.velocity)),
                            clip.length,
                        ),
                        length: clip.length,
                    },
                    None => Source::Silent,
                });
                let blocks_key = source_key(&blocks);
                let scene_keys = std::array::from_fn(|s| source_key(&scenes[s]));
                let audible = !t.mute && (!any_solo || t.solo);
                let level = if audible { volume_gain(t.volume) } else { 0.0 };
                TrackSnap {
                    id: t.id,
                    blocks,
                    scenes,
                    blocks_key,
                    scene_keys,
                    gain: pan_gain(level, t.pan),
                    devices: devices(t.id),
                }
            })
            .collect();
        let m = &project.master;
        Self {
            tracks,
            master: devices(crate::model::MASTER_ID),
            master_gain: pan_gain(volume_gain(m.volume), m.pan),
            bpm: project.bpm,
            loop_enabled: project.loop_enabled,
            loop_end: project.length_beats(),
        }
    }
}

fn source_key(source: &Source) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match source {
        Source::Silent => 0u8.hash(&mut h),
        Source::Arrangement(notes) => {
            1u8.hash(&mut h);
            hash_notes(notes, &mut h);
        }
        Source::Scene { notes, length } => {
            2u8.hash(&mut h);
            length.to_bits().hash(&mut h);
            hash_notes(notes, &mut h);
        }
    }
    h.finish()
}

fn hash_notes(notes: &[sequencer::SeqNote], h: &mut impl Hasher) {
    for n in notes {
        (n.start.to_bits(), n.end.to_bits(), n.key, n.velocity.to_bits()).hash(h);
    }
}

/// Expands a track's scene blocks into timeline notes: each block loops its scene clip and
/// cuts notes at the block end.
pub fn block_notes(track: &crate::model::Track) -> Vec<(f64, f64, u8, f32)> {
    let mut out = Vec::new();
    for b in &track.blocks {
        let Some(clip) = track.scenes.get(b.scene).and_then(Option::as_ref) else {
            continue;
        };
        if clip.length <= 0.0 {
            continue;
        }
        let mut t = 0.0;
        while t < b.length {
            for n in &clip.notes {
                if n.start >= clip.length || t + n.start >= b.length {
                    continue;
                }
                let start = b.start + t + n.start;
                let end = (start + n.len).min(b.end());
                out.push((start, end - start, n.key, n.velocity));
            }
            t += clip.length;
        }
    }
    out
}

/// Identity of "track plays scene `s`, started at `start`" for release detection.
fn scene_mode(track: &TrackSnap, s: usize, start: f64) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (s, start.to_bits(), track.scene_keys[s]).hash(&mut h);
    h.finish() | 1
}

/// Equal-power pan law, unity in the centre.
fn pan_gain(level: f32, pan: f32) -> [f32; 2] {
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    [
        level * angle.cos() * std::f32::consts::SQRT_2,
        level * angle.sin() * std::f32::consts::SQRT_2,
    ]
}

/// Every device of a snapshot: the track chains and the master chain.
fn all_devices(snap: &Snapshot) -> impl Iterator<Item = &DeviceSnap> {
    snap.tracks.iter().flat_map(|t| t.devices.iter()).chain(snap.master.iter())
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
    /// Starts a track's scene at the next launch grid point, or stops it now (`None`).
    LaunchScene { track: u64, scene: Option<usize> },
    /// Stops every scene.
    StopScenes,
    /// Launch grid in beats (0 = launch at once).
    SetLaunchQuantize(f64),
}

/// A live note of the record-armed track, at `beat` beats after its scene started.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecEvent {
    pub track: u64,
    pub scene: usize,
    pub key: u8,
    pub velocity: f32,
    pub on: bool,
    pub beat: f64,
}

/// A live event waiting for the next block of one track.
#[derive(Clone, Copy)]
enum Live {
    Note { key: u8, velocity: f32, on: bool },
    /// Any other MIDI channel message (CC, pitch bend, pressure, program change).
    Midi(MidiMsg),
}

pub struct Shared {
    playing: AtomicBool,
    position: AtomicU64,
    peaks: Vec<[AtomicU32; 2]>,
    master: [AtomicU32; 2],
    load: AtomicU32,
    /// Track id that receives MIDI keyboard input (0 = none).
    midi_target: AtomicU64,
    midi_activity: AtomicU32,
    /// Keys held live (MIDI keyboard or preview) on the MIDI target track, as a bit set.
    live_keys: [AtomicU64; 2],
    /// Per snapshot track index: playing scene + 1 (low byte) and queued scene + 1.
    scene_state: Vec<AtomicU32>,
    /// Per snapshot track index: beats since the playing scene started.
    scene_beat: Vec<AtomicU64>,
    /// Track whose live notes are reported as `RecEvent`s (0 = none).
    record_track: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            position: AtomicU64::new(0f64.to_bits()),
            peaks: (0..MAX_TRACKS).map(|_| [AtomicU32::new(0), AtomicU32::new(0)]).collect(),
            master: [AtomicU32::new(0), AtomicU32::new(0)],
            load: AtomicU32::new(0),
            midi_target: AtomicU64::new(0),
            midi_activity: AtomicU32::new(0),
            live_keys: [AtomicU64::new(0), AtomicU64::new(0)],
            scene_state: (0..MAX_TRACKS).map(|_| AtomicU32::new(0)).collect(),
            scene_beat: (0..MAX_TRACKS).map(|_| AtomicU64::new(0)).collect(),
            record_track: AtomicU64::new(0),
        }
    }

    /// Playing scene of the track at snapshot index `index`.
    pub fn scene_playing(&self, index: usize) -> Option<usize> {
        let v = self.scene_state.get(index)?.load(Ordering::Relaxed) & 0xff;
        (v > 0).then(|| v as usize - 1)
    }

    /// Scene waiting for the launch grid on the track at `index`.
    pub fn scene_queued(&self, index: usize) -> Option<usize> {
        let v = (self.scene_state.get(index)?.load(Ordering::Relaxed) >> 8) & 0xff;
        (v > 0).then(|| v as usize - 1)
    }

    /// Beats since the playing scene of the track at `index` started (not wrapped).
    pub fn scene_beat(&self, index: usize) -> f64 {
        self.scene_beat
            .get(index)
            .map_or(0.0, |b| f64::from_bits(b.load(Ordering::Relaxed)))
    }

    /// Reports the live notes of this track as `RecEvent`s (0 = none).
    pub fn set_record_track(&self, track: u64) {
        self.record_track.store(track, Ordering::Relaxed);
    }

    /// Routes MIDI keyboard input to the track with this id (0 = nowhere).
    pub fn set_midi_target(&self, track: u64) {
        self.midi_target.store(track, Ordering::Relaxed);
    }

    /// Counts up with every incoming MIDI message (drives the activity LED).
    pub fn midi_activity_count(&self) -> u32 {
        self.midi_activity.load(Ordering::Relaxed)
    }

    pub(crate) fn midi_activity(&self) {
        self.midi_activity.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether `key` is held live on the MIDI target track.
    pub fn live_key(&self, key: u8) -> bool {
        let k = key as usize & 127;
        self.live_keys[k / 64].load(Ordering::Relaxed) & (1 << (k % 64)) != 0
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
    midi: Option<rtrb::Producer<MidiMsg>>,
    recorded: rtrb::Consumer<RecEvent>,
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

    /// The producer end of the engine's MIDI input queue (for the MIDI thread). Only once.
    pub fn take_midi_input(&mut self) -> Option<rtrb::Producer<MidiMsg>> {
        self.midi.take()
    }

    /// Live notes of the record-armed track since the last call.
    pub fn take_recorded(&mut self, out: &mut Vec<RecEvent>) {
        while let Ok(e) = self.recorded.pop() {
            out.push(e);
        }
    }

    /// Drops snapshots the engine is done with. Call regularly from the UI thread.
    pub fn collect_garbage(&mut self) {
        while self.garbage.pop().is_ok() {}
    }
}

struct TrackRt {
    id: u64,
    /// Identity of what was rendered last block; a change releases the sounding notes.
    last_mode: u64,
    /// Launcher: playing scene and the launcher beat it started at, and a queued launch.
    scene: Option<usize>,
    scene_start: f64,
    queued: Option<(usize, f64)>,
    voices: Voices,
    live: [bool; 128],
    release: bool,
    pending: [Live; 64],
    pending_len: usize,
}

impl TrackRt {
    fn new(id: u64) -> Self {
        Self {
            id,
            last_mode: 0,
            scene: None,
            scene_start: 0.0,
            queued: None,
            voices: Voices::default(),
            live: [false; 128],
            release: false,
            pending: [Live::Midi([0; 3]); 64],
            pending_len: 0,
        }
    }

    fn push(&mut self, live: Live) {
        if self.pending_len < self.pending.len() {
            self.pending[self.pending_len] = live;
            self.pending_len += 1;
        }
    }
}

fn push_live(rt: &mut [TrackRt], track: u64, live: Live) {
    if let Some(rt) = rt.iter_mut().find(|r| r.id == track) {
        rt.push(live);
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
    midi: rtrb::Consumer<MidiMsg>,
    /// Track that received MIDI input in the last block, and the track each held key went to
    /// (so note-offs reach the right track after the selection changed).
    midi_target: u64,
    midi_owner: [u64; 128],
    midi_events: Vec<MidiMsg>,
    /// Output buffer of built-in note devices (swapped with `events`).
    note_scratch: Vec<NoteEvent>,
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
    /// Launcher clock (beats); runs while any scene plays or waits.
    launch_pos: f64,
    launch_q: f64,
    recorded: rtrb::Producer<RecEvent>,
}

pub fn new(sample_rate: f64, max_block: u32) -> (Engine, EngineHandle) {
    let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(1024);
    let (snap_tx, snap_rx) = rtrb::RingBuffer::new(64);
    let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(256);
    let (midi_tx, midi_rx) = rtrb::RingBuffer::new(1024);
    let (rec_tx, rec_rx) = rtrb::RingBuffer::new(1024);
    let shared = Arc::new(Shared::new());
    let n = max_block as usize;
    let buf = || [vec![0.0f32; n], vec![0.0f32; n]];
    let engine = Engine {
        commands: cmd_rx,
        midi: midi_rx,
        midi_target: 0,
        midi_owner: [0; 128],
        midi_events: Vec::with_capacity(256),
        note_scratch: Vec::with_capacity(4096),
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
        launch_pos: 0.0,
        launch_q: 0.25,
        recorded: rec_tx,
    };
    let handle = EngineHandle {
        commands: cmd_tx,
        snapshots: snap_tx,
        garbage: garbage_rx,
        midi: Some(midi_tx),
        recorded: rec_rx,
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
        self.handle_midi();

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
        let launch_running = self.rt.iter().any(|r| r.scene.is_some() || r.queued.is_some());
        let launch_end = self.launch_pos + block_beats;
        let record_track = self.shared.record_track.load(Ordering::Relaxed);
        for (i, track) in snap.tracks.iter().enumerate() {
            let rt = &mut self.rt[i];
            let events = &mut self.events;
            events.clear();
            let midi_events = &mut self.midi_events;
            midi_events.clear();
            // Timeline blocks win while the arranger plays; otherwise the launcher plays.
            let arranger = self.playing && !matches!(track.blocks, Source::Silent);
            let mode = match (arranger, rt.scene) {
                (true, _) => track.blocks_key.wrapping_add(1),
                (false, Some(s)) => scene_mode(track, s, rt.scene_start),
                (false, None) => 0,
            };
            if mode != rt.last_mode || (release_all && arranger) || rt.release {
                rt.release = false;
                rt.last_mode = mode;
                rt.voices.release_all(0, events);
            }
            for live in &rt.pending[..rt.pending_len] {
                match *live {
                    Live::Note { key, velocity, on } => {
                        let k = key as usize & 127;
                        if on && rt.live[k] {
                            events.push(NoteEvent { time: 0, on: false, key, velocity: 0.0 });
                        }
                        rt.live[k] = on;
                        events.push(NoteEvent { time: 0, on, key, velocity });
                        if let (true, Some(scene)) = (track.id == record_track, rt.scene) {
                            let _ = self.recorded.push(RecEvent {
                                track: track.id,
                                scene,
                                key,
                                velocity,
                                on,
                                beat: self.launch_pos - rt.scene_start,
                            });
                        }
                    }
                    Live::Midi(m) => midi_events.push(m),
                }
            }
            rt.pending_len = 0;
            if track.id == self.midi_target {
                let mut bits = [0u64; 2];
                for (k, &on) in rt.live.iter().enumerate() {
                    if on {
                        bits[k / 64] |= 1 << (k % 64);
                    }
                }
                self.shared.live_keys[0].store(bits[0], Ordering::Relaxed);
                self.shared.live_keys[1].store(bits[1], Ordering::Relaxed);
            }
            if arranger {
                if let Some(s) = spans[0] {
                    sequencer::render(&track.blocks, &mut rt.voices, s, events);
                }
                if let Some(t) = wrap_at {
                    rt.voices.release_all(t.min(n as u32 - 1), events);
                }
                if let Some(s) = spans[1] {
                    sequencer::render(&track.blocks, &mut rt.voices, s, events);
                }
            }
            if launch_running {
                // A queued launch that is due splits the block at its grid point.
                let due = rt.queued.filter(|q| q.1 < launch_end);
                let split = due.map_or(n as f64, |(_, q)| ((q - self.launch_pos).max(0.0) * fpb).min(n as f64));
                let span = |b0: f64, beats: f64, frame0: f64| Span {
                    b0,
                    b1: b0 + beats,
                    frame0,
                    frames_per_beat: fpb,
                    frames: n as u32,
                };
                if let (false, Some(s)) = (arranger, rt.scene) {
                    if split > 0.0 {
                        let b0 = self.launch_pos - rt.scene_start;
                        sequencer::render(&track.scenes[s], &mut rt.voices, span(b0, split / fpb, 0.0), events);
                    }
                }
                if let Some((s, q)) = due {
                    rt.scene = Some(s);
                    rt.scene_start = q;
                    rt.queued = None;
                    if !arranger {
                        rt.voices.release_all((split as u32).min(n as u32 - 1), events);
                        rt.last_mode = scene_mode(track, s, q);
                        let rest = block_beats - split / fpb;
                        if rest > 0.0 {
                            let b0 = self.launch_pos + split / fpb - q;
                            sequencer::render(&track.scenes[s], &mut rt.voices, span(b0, rest, split), events);
                        }
                    }
                }
            }
            if let Some(st) = self.shared.scene_state.get(i) {
                let pack = |v: Option<usize>| v.map_or(0, |s| s as u32 + 1);
                st.store(pack(rt.scene) | pack(rt.queued.map(|q| q.0)) << 8, Ordering::Relaxed);
                self.shared.scene_beat[i].store((self.launch_pos - rt.scene_start).to_bits(), Ordering::Relaxed);
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
                if dev.device.process([il, ir], [ol, or], n, events, midi_events, &transport) {
                    std::mem::swap(&mut bufs.a, &mut bufs.b);
                }
                if dev.note_fx {
                    match &dev.device {
                        DeviceRef::Clap(inst) => inst.take_output_notes(events, midi_events),
                        DeviceRef::Builtin(d) => {
                            let ctx = NoteCtx {
                                sample_rate: self.sample_rate as f32,
                                bpm: snap.bpm,
                                frames: n as u32,
                                song_beat: if arranger {
                                    Some(self.position)
                                } else {
                                    rt.scene.map(|_| self.launch_pos - rt.scene_start)
                                },
                            };
                            d.process_notes(events, &mut self.note_scratch, &ctx);
                            std::mem::swap(events, &mut self.note_scratch);
                        }
                    }
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
        // Master chain: effects on the sum, then master volume and pan.
        let bufs = &mut self.bufs;
        for c in 0..2 {
            bufs.a[c][..n].copy_from_slice(&bufs.master[c][..n]);
        }
        for dev in &snap.master {
            if dev.bypass {
                continue;
            }
            let [il, ir] = &bufs.a;
            let [ol, or] = &mut bufs.b;
            if dev.device.process([il, ir], [ol, or], n, &[], &[], &transport) {
                std::mem::swap(&mut bufs.a, &mut bufs.b);
            }
        }
        for c in 0..2 {
            let g = snap.master_gain[c];
            for (m, a) in bufs.master[c][..n].iter_mut().zip(&bufs.a[c][..n]) {
                *m = a * g;
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

        // The launcher clock runs while any scene plays or waits; idle, it rests at 0.
        if self.rt.iter().any(|r| r.scene.is_some() || r.queued.is_some()) {
            self.launch_pos = launch_end;
        } else {
            self.launch_pos = 0.0;
        }
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
            for d in all_devices(&old) {
                let still_used = all_devices(&self.current).any(|nd| nd.device.uid() == d.device.uid());
                if !still_used {
                    d.device.stop_processing();
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
                Some(i) => self.rt.swap_remove(i),
                None => TrackRt::new(t.id),
            };
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
                    push_live(&mut self.rt, track, Live::Note { key, velocity, on });
                }
                Command::LaunchScene { track, scene } => {
                    let running = self.rt.iter().any(|r| r.scene.is_some() || r.queued.is_some());
                    if !running {
                        // The launcher clock starts with this launch.
                        self.launch_pos = 0.0;
                    }
                    let at = if running && self.launch_q > 0.0 {
                        let q = (self.launch_pos / self.launch_q - 1e-9).ceil() * self.launch_q;
                        q.max(self.launch_pos)
                    } else {
                        self.launch_pos
                    };
                    if let Some(rt) = self.rt.iter_mut().find(|r| r.id == track) {
                        match scene {
                            Some(s) => rt.queued = Some((s.min(SCENES - 1), at)),
                            None => {
                                rt.scene = None;
                                rt.queued = None;
                            }
                        }
                    }
                }
                Command::StopScenes => {
                    for rt in &mut self.rt {
                        rt.scene = None;
                        rt.queued = None;
                    }
                }
                Command::SetLaunchQuantize(q) => self.launch_q = q.max(0.0),
            }
        }
    }

    /// Routes MIDI keyboard input to the target track. When the target changes, the old
    /// track gets its held notes, sustain and pitch bend released.
    fn handle_midi(&mut self) {
        let target = self.shared.midi_target.load(Ordering::Relaxed);
        if target != self.midi_target {
            let old = self.midi_target;
            if let Some(rt) = self.rt.iter_mut().find(|r| r.id == old) {
                for key in 0..128u8 {
                    if rt.live[key as usize] {
                        rt.push(Live::Note { key, velocity: 0.0, on: false });
                    }
                }
                rt.push(Live::Midi([0xb0, 64, 0]));
                rt.push(Live::Midi([0xe0, 0, 64]));
            }
            if !self.rt.iter().any(|r| r.id == target) {
                self.shared.live_keys[0].store(0, Ordering::Relaxed);
                self.shared.live_keys[1].store(0, Ordering::Relaxed);
            }
            self.midi_target = target;
        }
        while let Ok(m) = self.midi.pop() {
            let key = (m[1] & 0x7f) as usize;
            match m[0] & 0xf0 {
                0x90 if m[2] > 0 => {
                    self.midi_owner[key] = target;
                    let velocity = m[2] as f32 / 127.0;
                    push_live(&mut self.rt, target, Live::Note { key: key as u8, velocity, on: true });
                }
                0x80 | 0x90 => {
                    let owner = std::mem::take(&mut self.midi_owner[key]);
                    let to = if owner != 0 { owner } else { target };
                    push_live(&mut self.rt, to, Live::Note { key: key as u8, velocity: 0.0, on: false });
                }
                _ => push_live(&mut self.rt, target, Live::Midi(m)),
            }
        }
    }
}

impl Drop for Engine {
    /// The audio thread ends: stop every instance there, before the UI deactivates them.
    fn drop(&mut self) {
        for d in all_devices(&self.current) {
            d.device.stop_processing();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loop_their_scene_clip() {
        use crate::model::{Block, Clip, Note, Track};
        let mut t = Track::new(1, "T".into(), 0);
        let mut c = Clip::new(2.0);
        c.notes.push(Note { start: 0.0, len: 1.0, key: 60, velocity: 1.0 });
        c.notes.push(Note { start: 1.5, len: 1.0, key: 62, velocity: 1.0 });
        t.scenes[1] = Some(c);
        t.blocks.push(Block { start: 8.0, length: 3.0, scene: 1 });
        t.blocks.push(Block { start: 20.0, length: 4.0, scene: 0 }); // empty slot: ignored
        let n: Vec<(f64, f64, u8)> = block_notes(&t).into_iter().map(|n| (n.0, n.1, n.2)).collect();
        // Loop 1: 8.0 (C), 9.5 (D, 1 beat); loop 2: 10.0 (C, cut at the block end 11.0).
        assert_eq!(n, vec![(8.0, 1.0, 60), (9.5, 1.0, 62), (10.0, 1.0, 60)]);
    }

    fn launcher_project() -> (Project, u64) {
        let p = Project::demo();
        let id = p.tracks[0].id;
        (p, id)
    }

    /// Renders `beats` beats at 120 bpm / 48 kHz in 480-frame blocks.
    fn run_beats(engine: &mut Engine, beats: f64) {
        let mut out = vec![0.0; 2 * 480];
        let blocks = (beats * 24_000.0 / 480.0).round() as usize;
        for _ in 0..blocks {
            engine.render_interleaved(&mut out);
        }
    }

    #[test]
    fn scenes_play_and_loop_without_the_arranger() {
        let (p, lead) = launcher_project();
        let (mut engine, mut handle) = new(48_000.0, 512);
        handle.publish(Snapshot::build(&p, |_| Vec::new()));
        handle.send(Command::LaunchScene { track: lead, scene: Some(0) });
        run_beats(&mut engine, 6.0);
        assert!(!handle.shared.playing(), "the arranger must stay stopped");
        assert_eq!(handle.shared.scene_playing(0), Some(0));
        // Beats since launch keep counting; the 4-beat clip loops on them.
        assert!((handle.shared.scene_beat(0) - 6.0).abs() < 0.03, "{}", handle.shared.scene_beat(0));
        handle.send(Command::LaunchScene { track: lead, scene: None });
        run_beats(&mut engine, 0.1);
        assert_eq!(handle.shared.scene_playing(0), None);
        handle.collect_garbage();
    }

    #[test]
    fn a_launch_waits_for_the_grid() {
        let (p, lead) = launcher_project();
        let (mut engine, mut handle) = new(48_000.0, 512);
        handle.publish(Snapshot::build(&p, |_| Vec::new()));
        handle.send(Command::SetLaunchQuantize(1.0));
        handle.send(Command::LaunchScene { track: lead, scene: Some(0) });
        run_beats(&mut engine, 0.3);
        // Running clock: scene 2 waits for beat 1.
        handle.send(Command::LaunchScene { track: lead, scene: Some(1) });
        run_beats(&mut engine, 0.4);
        assert_eq!(handle.shared.scene_playing(0), Some(0));
        assert_eq!(handle.shared.scene_queued(0), Some(1));
        run_beats(&mut engine, 0.6);
        assert_eq!(handle.shared.scene_playing(0), Some(1));
        assert_eq!(handle.shared.scene_queued(0), None);
        // It started exactly on beat 1 of the launcher clock.
        let b = handle.shared.scene_beat(0);
        assert!(b > 0.0 && b < 0.4, "{b}");
        handle.collect_garbage();
    }

    #[test]
    fn live_notes_of_the_armed_track_are_reported() {
        let (p, lead) = launcher_project();
        let (mut engine, mut handle) = new(48_000.0, 512);
        handle.publish(Snapshot::build(&p, |_| Vec::new()));
        handle.send(Command::LaunchScene { track: lead, scene: Some(2) });
        handle.shared.set_record_track(lead);
        run_beats(&mut engine, 1.0);
        handle.send(Command::Note { track: lead, key: 64, velocity: 0.5, on: true });
        run_beats(&mut engine, 0.5);
        handle.send(Command::Note { track: lead, key: 64, velocity: 0.0, on: false });
        run_beats(&mut engine, 0.1);
        let mut rec = Vec::new();
        handle.take_recorded(&mut rec);
        assert_eq!(rec.len(), 2);
        assert!(rec[0].on && !rec[1].on && rec[0].scene == 2 && rec[0].key == 64);
        assert!((rec[0].beat - 1.0).abs() < 0.03 && (rec[1].beat - 1.5).abs() < 0.03, "{rec:?}");
        handle.collect_garbage();
    }

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

//! Project data as edited by the UI. The audio thread never sees these types directly; it gets
//! an immutable [`crate::engine::Snapshot`] built from them.

use serde::{Deserialize, Serialize};

/// Number of scene slots per track (columns of the scene launcher).
pub const SCENES: usize = 4;
pub const BEATS_PER_BAR: f64 = 4.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Start and length in beats, relative to the clip start.
    pub start: f64,
    pub len: f64,
    pub key: u8,
    pub velocity: f32,
}

impl Note {
    pub fn end(&self) -> f64 {
        self.start + self.len
    }
}

pub const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Time signatures offered in the UI.
pub const TIME_SIGNATURES: [(u8, u8); 7] = [(2, 4), (3, 4), (4, 4), (5, 4), (6, 8), (7, 8), (12, 8)];

fn default_time_sig() -> (u8, u8) {
    (4, 4)
}

/// A scene clip: loops while it plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Loop length in beats (quarter notes).
    pub length: f64,
    /// Kept sorted by start time (see [`Clip::sort`]).
    pub notes: Vec<Note>,
    /// Root key, 0 = C .. 11 = B.
    #[serde(default)]
    pub root: u8,
    /// Numerator and denominator.
    #[serde(default = "default_time_sig")]
    pub time_sig: (u8, u8),
}

impl Default for Clip {
    fn default() -> Self {
        Self::new(BEATS_PER_BAR)
    }
}

impl Clip {
    pub fn new(length: f64) -> Self {
        Self {
            length,
            notes: Vec::new(),
            root: 0,
            time_sig: default_time_sig(),
        }
    }

    /// Length of one bar in beats (quarter notes) for this clip's time signature.
    pub fn bar_beats(&self) -> f64 {
        let (num, den) = self.time_sig;
        num.max(1) as f64 * 4.0 / den.max(1) as f64
    }

    /// "C · 4/4"
    pub fn key_sig_label(&self) -> String {
        format!("{} · {}/{}", KEY_NAMES[self.root as usize % 12], self.time_sig.0, self.time_sig.1)
    }

    pub fn sort(&mut self) {
        self.notes
            .sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    }

    pub fn key_range(&self) -> Option<(u8, u8)> {
        let lo = self.notes.iter().map(|n| n.key).min()?;
        let hi = self.notes.iter().map(|n| n.key).max()?;
        Some((lo, hi))
    }
}

/// One plugin in a track's device chain, as stored in the project file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceSpec {
    /// CLAP plugin id, e.g. `ch.fonsie.alpha-juno`.
    pub plugin_id: String,
    pub name: String,
    #[serde(default)]
    pub bypass: bool,
    /// Plugin state from the CLAP state extension. Filled in when the project is saved.
    #[serde(default)]
    pub state: Option<Vec<u8>>,
    /// Parameter values of a built-in device (`plugin_id` starts with `rasdaw.`), by key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<std::collections::BTreeMap<String, f32>>,
    /// Rack section. Missing in older projects; then derived from the plugin's features.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<Section>,
}

/// Which part of the device rack a device sits in. The chain runs in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// One note-altering device (CLAP `note-effect`).
    NoteFx,
    /// One instrument.
    Instrument,
    /// Any number of audio effects.
    Effect,
}

impl Section {
    pub fn single(self) -> bool {
        self != Section::Effect
    }
}

/// A block on a track's timeline: plays the track's clip in scene slot `scene`, looping, from
/// `start` for `length` beats.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub start: f64,
    pub length: f64,
    pub scene: usize,
}

impl Block {
    pub fn end(&self) -> f64 {
        self.start + self.length
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub color: usize,
    /// Scene blocks on the timeline. If there are any, the track plays them and its launcher
    /// scenes are muted.
    #[serde(default)]
    pub blocks: Vec<Block>,
    pub scenes: [Option<Clip>; SCENES],
    /// Scene playing in the launcher, mirrored from the engine every frame (not saved: a
    /// project always opens with the launcher stopped).
    #[serde(skip)]
    pub active_scene: Option<usize>,
    /// Scene waiting for the launch grid, mirrored from the engine.
    #[serde(skip)]
    pub queued_scene: Option<usize>,
    pub volume: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub devices: Vec<DeviceSpec>,
}

/// What a track plays right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    Arrangement,
    Scene(usize),
    Silent,
}

impl Track {
    pub fn new(id: u64, name: String, color: usize) -> Self {
        Self {
            id,
            name,
            color,
            blocks: Vec::new(),
            scenes: Default::default(),
            active_scene: None,
            queued_scene: None,
            volume: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            devices: Vec::new(),
        }
    }

    pub fn has_arrangement(&self) -> bool {
        !self.blocks.is_empty()
    }

    /// Index of the block playing at beat `pos`.
    pub fn block_at(&self, pos: f64) -> Option<usize> {
        self.blocks.iter().position(|b| pos >= b.start && pos < b.end())
    }

    pub fn sort_blocks(&mut self) {
        self.blocks.sort_by(|a, b| a.start.total_cmp(&b.start));
    }

    /// The core routing rule: while the arranger plays, a track with blocks plays its blocks
    /// (its scene is muted); otherwise its launched scene loops. Scenes play independently of
    /// the arranger.
    pub fn play_mode(&self, arranger_playing: bool) -> PlayMode {
        if arranger_playing && self.has_arrangement() {
            return PlayMode::Arrangement;
        }
        match self.active_scene {
            Some(i) if self.scenes[i].is_some() => PlayMode::Scene(i),
            _ => PlayMode::Silent,
        }
    }
}

/// Rack key and device owner id of the master track (no real track has this id).
pub const MASTER_ID: u64 = u64::MAX;

/// The master track: the sum of all tracks runs through its effects. It has no scenes and no
/// timeline blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Master {
    pub volume: f32,
    pub pan: f32,
    #[serde(default)]
    pub devices: Vec<DeviceSpec>,
}

impl Default for Master {
    fn default() -> Self {
        Self {
            volume: 0.8,
            pan: 0.0,
            devices: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub bpm: f64,
    /// Length of the timeline in bars.
    pub bars: u32,
    pub loop_enabled: bool,
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub master: Master,
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            bars: 32,
            loop_enabled: true,
            tracks: Vec::new(),
            master: Master::default(),
            next_id: 1,
        }
    }
}

impl Project {
    pub fn add_track(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let n = self.tracks.len();
        self.tracks
            .push(Track::new(id, format!("Track {}", n + 1), n));
        n
    }

    pub fn length_beats(&self) -> f64 {
        self.bars as f64 * BEATS_PER_BAR
    }

    /// A small starting project: four tracks, two of them with looping scene clips.
    pub fn demo() -> Self {
        let mut p = Project::default();
        for _ in 0..4 {
            p.add_track();
        }
        let names = ["Lead", "Bass", "Pad", "Keys"];
        for (t, name) in p.tracks.iter_mut().zip(names) {
            t.name = name.to_owned();
        }
        let n = |start: f64, len: f64, key: u8| Note {
            start,
            len,
            key,
            velocity: 0.8,
        };
        // Lead: C minor arpeggio, 1 bar
        let mut arp = Clip::new(4.0);
        for (i, k) in [60u8, 63, 67, 72, 67, 63, 60, 55].iter().enumerate() {
            arp.notes.push(n(i as f64 * 0.5, 0.45, *k));
        }
        p.tracks[0].scenes[0] = Some(arp);
        let mut arp2 = Clip::new(8.0);
        for (i, k) in [60u8, 65, 68, 72, 70, 67, 63, 62].iter().enumerate() {
            arp2.notes.push(n(i as f64, 0.9, *k));
        }
        arp2.root = 5;
        p.tracks[0].scenes[1] = Some(arp2);
        // Bass: 2 bars
        let mut bass = Clip::new(8.0);
        for (i, k) in [36u8, 36, 39, 36, 34, 34, 31, 34].iter().enumerate() {
            bass.notes.push(n(i as f64, 0.7, *k));
        }
        p.tracks[1].scenes[0] = Some(bass);
        // Pad chord, 4 bars
        let mut pad = Clip::new(16.0);
        for k in [48u8, 55, 60, 63] {
            pad.notes.push(n(0.0, 7.5, k));
        }
        for k in [44u8, 51, 56, 60] {
            pad.notes.push(n(8.0, 7.5, k));
        }
        pad.sort();
        p.tracks[2].scenes[0] = Some(pad);
        p
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("project serialises")
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_blocks_mute_the_scene_while_the_arranger_plays() {
        let mut p = Project::demo();
        p.tracks[0].active_scene = Some(0);
        assert_eq!(p.tracks[0].play_mode(true), PlayMode::Scene(0));
        p.tracks[0].blocks.push(Block { start: 0.0, length: 8.0, scene: 1 });
        assert_eq!(p.tracks[0].play_mode(true), PlayMode::Arrangement);
        // With the arranger stopped the launched scene plays.
        assert_eq!(p.tracks[0].play_mode(false), PlayMode::Scene(0));
        // Missing scene clips play nothing.
        assert_eq!(p.tracks[3].play_mode(false), PlayMode::Silent);
        p.tracks[3].active_scene = Some(2);
        assert_eq!(p.tracks[3].play_mode(false), PlayMode::Silent);
    }

    #[test]
    fn project_json_round_trip() {
        let mut p = Project::demo();
        p.tracks[1].devices.push(DeviceSpec {
            plugin_id: "x.y".into(),
            name: "Y".into(),
            bypass: true,
            state: Some(vec![1, 2, 3]),
            params: None,
            section: None,
        });
        p.tracks[1].devices.push(DeviceSpec {
            plugin_id: "rasdaw.delay".into(),
            name: "Delay".into(),
            bypass: false,
            state: None,
            params: Some([("mix".to_owned(), 0.5)].into_iter().collect()),
            section: Some(Section::Effect),
        });
        p.tracks[1].blocks.push(Block { start: 4.0, length: 8.0, scene: 0 });
        p.tracks[1].scenes[0].as_mut().unwrap().time_sig = (6, 8);
        let back = Project::from_json(&p.to_json()).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn old_projects_still_load() {
        // Pre-blocks format: `arrangement` field, clips without key/time signature.
        let json = r#"{"bpm":120.0,"bars":8,"loop_enabled":true,"next_id":2,"tracks":[{"id":1,"name":"T","color":0,
            "arrangement":{"length":0.0,"notes":[]},"scenes":[{"length":4.0,"notes":[]},null,null,null],
            "active_scene":0,"volume":0.8,"pan":0.0,"mute":false,"solo":false,"devices":[]}]}"#;
        let p = Project::from_json(json).unwrap();
        assert!(p.tracks[0].blocks.is_empty());
        let clip = p.tracks[0].scenes[0].as_ref().unwrap();
        assert_eq!((clip.root, clip.time_sig), (0, (4, 4)));
        assert_eq!(clip.bar_beats(), 4.0);
    }
}

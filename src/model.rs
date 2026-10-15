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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Loop length in beats. Only used for scene clips; the arrangement uses the project length.
    pub length: f64,
    /// Kept sorted by start time (see [`Clip::sort`]).
    pub notes: Vec<Note>,
}

impl Clip {
    pub fn new(length: f64) -> Self {
        Self {
            length,
            notes: Vec::new(),
        }
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: u64,
    pub name: String,
    pub color: usize,
    /// Notes on the timeline. If this has any notes, the track plays the arrangement and its
    /// scenes are muted.
    pub arrangement: Clip,
    pub scenes: [Option<Clip>; SCENES],
    /// Scene that loops while the arrangement is empty.
    pub active_scene: Option<usize>,
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
            arrangement: Clip::new(0.0),
            scenes: Default::default(),
            active_scene: None,
            volume: 0.8,
            pan: 0.0,
            mute: false,
            solo: false,
            devices: Vec::new(),
        }
    }

    pub fn has_arrangement(&self) -> bool {
        !self.arrangement.notes.is_empty()
    }

    /// The core routing rule: notes on the timeline win, otherwise the active scene loops.
    pub fn play_mode(&self) -> PlayMode {
        if self.has_arrangement() {
            return PlayMode::Arrangement;
        }
        match self.active_scene {
            Some(i) if self.scenes[i].as_ref().is_some_and(|c| !c.notes.is_empty()) => {
                PlayMode::Scene(i)
            }
            _ => PlayMode::Silent,
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
    pub next_id: u64,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            bpm: 120.0,
            bars: 32,
            loop_enabled: true,
            tracks: Vec::new(),
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
        p.tracks[0].active_scene = Some(0);
        p.tracks[1].active_scene = Some(0);
        p.tracks[2].active_scene = Some(0);
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
    fn arrangement_notes_mute_the_scene() {
        let mut p = Project::demo();
        assert_eq!(p.tracks[0].play_mode(), PlayMode::Scene(0));
        p.tracks[0].arrangement.notes.push(Note {
            start: 0.0,
            len: 1.0,
            key: 60,
            velocity: 1.0,
        });
        assert_eq!(p.tracks[0].play_mode(), PlayMode::Arrangement);
        // Empty or missing scene clips play nothing.
        assert_eq!(p.tracks[3].play_mode(), PlayMode::Silent);
        p.tracks[3].active_scene = Some(2);
        assert_eq!(p.tracks[3].play_mode(), PlayMode::Silent);
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
        });
        p.tracks[1].devices.push(DeviceSpec {
            plugin_id: "rasdaw.delay".into(),
            name: "Delay".into(),
            bypass: false,
            state: None,
            params: Some([("mix".to_owned(), 0.5)].into_iter().collect()),
        });
        let back = Project::from_json(&p.to_json()).unwrap();
        assert_eq!(p, back);
    }
}

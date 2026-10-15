//! The main window.
//!
//! Layout (Bitwig-like):
//! - top: transport bar
//! - top left: scene launcher, 4 scene slots per track
//! - top right: track headers and the arranger timeline, rows aligned with the scene rows. The
//!   timeline holds scene blocks; double-clicking a block, a track header or a scene slot
//!   replaces the timeline with the piano roll, ESC returns
//! - the selected track receives the MIDI keyboard (see `midi.rs`)
//! - bottom: device rack of the selected track (fixed height): note FX, instrument, effects
//! - Ctrl+S / Ctrl+O open the save / open dialog; Ctrl +/-/0 scale the whole UI

mod arranger;
mod builtin_card;
mod dialogs;
mod launcher;
mod piano_roll;
mod rack;
mod transport;
pub mod widgets;

use crate::audio::{self, DriverInfo};
use crate::clap_host::gui::GuiManager;
use crate::clap_host::{PluginDesc, PluginInstance, Registry};
use crate::devices::{BuiltinDevice, Kind};
use crate::engine::{Command, DeviceRef, DeviceSnap, EngineHandle, Snapshot};
use crate::midi::MidiInput;
use crate::model::{Clip, DeviceSpec, Project, Section, BEATS_PER_BAR, MASTER_ID};
use crate::settings::Settings;
use crate::theme::current as th;
use eframe::egui::{self, Key};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Height of a track row; the scene launcher and the arranger share it so rows line up.
pub const ROW_H: f32 = 74.0;
/// Height of the master row below the tracks (visibly smaller than a track).
pub const MASTER_H: f32 = 40.0;
/// Section bars above the launcher / arranger.
pub const BAR_H: f32 = 24.0;
/// Scene header buttons and the timeline ruler.
pub const HEADER_H: f32 = 28.0;
pub const SLOT_W: f32 = 112.0;
pub const LAUNCHER_W: f32 = SLOT_W * crate::model::SCENES as f32 + 8.0;
pub const TRACK_HEAD_W: f32 = 168.0;
/// Fixed height of the device rack.
pub const RACK_H: f32 = 282.0;

/// What the piano roll edits: a scene clip of a track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditTarget {
    pub track: u64,
    pub scene: usize,
}

impl EditTarget {
    pub fn track_id(self) -> u64 {
        self.track
    }
}

/// What the Delete key removes (besides piano roll notes).
#[derive(Clone, Debug, PartialEq)]
pub enum Selection {
    /// A device in the rack (`track` may be `MASTER_ID`); `uid` as in `App::device_uid`.
    Device { track: u64, uid: u64 },
    /// A scene clip in the launcher.
    Scene { track: u64, scene: usize },
    /// A timeline block, identified by value (indices change when blocks are sorted).
    Block { track: u64, block: crate::model::Block },
}

/// MIDI recording into a scene clip (record button in the slot's title bar).
pub struct Recording {
    pub track: u64,
    pub scene: usize,
    /// The clip was empty: it grows while recording and is trimmed to whole bars at the end.
    pub grow: bool,
    /// The scene has started playing (a launch may wait for the grid first).
    pub started: bool,
    /// Held keys: start beat and velocity.
    held: [Option<(f64, f32)>; 128],
    max_end: f64,
}

/// Where a recorded note goes: `b0`/`b1` are beats since the scene started. A growing clip
/// keeps them as they are; an existing loop wraps the start into the loop (overdub).
pub fn recorded_note_span(grow: bool, clip_len: f64, b0: f64, b1: f64) -> (f64, f64) {
    let min = 1.0 / 64.0;
    if grow {
        (b0.max(0.0), (b1 - b0).max(min))
    } else {
        let l = clip_len.max(1e-6);
        (b0.rem_euclid(l), (b1 - b0).clamp(min, l))
    }
}

#[cfg(test)]
mod tests {
    use super::recorded_note_span;

    #[test]
    fn recorded_notes_grow_or_wrap_into_the_loop() {
        assert_eq!(recorded_note_span(true, 256.0, 9.5, 10.0), (9.5, 0.5));
        // Overdub into a 4-beat loop: beat 9.5 is beat 1.5 of the third pass.
        assert_eq!(recorded_note_span(false, 4.0, 9.5, 10.0), (1.5, 0.5));
        // Very short and over-long notes are clamped.
        assert_eq!(recorded_note_span(false, 4.0, 1.0, 1.0), (1.0, 1.0 / 64.0));
        assert_eq!(recorded_note_span(false, 4.0, 0.0, 9.0), (0.0, 4.0));
    }
}

/// Save or open dialog (Ctrl+S / Ctrl+O).
pub struct FileDialog {
    pub save: bool,
    pub path: String,
}

/// A device card being dragged to a new position in the effect section.
#[derive(Clone, Copy)]
pub struct DeviceDrag {
    pub track: u64,
    pub from: usize,
    /// Pointer offset inside the title bar when the drag started.
    pub grab: egui::Vec2,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BlockDragMode {
    Move,
    Resize,
}

/// A timeline block being moved or resized.
#[derive(Clone, Copy)]
pub struct BlockDrag {
    pub track: u64,
    pub index: usize,
    pub mode: BlockDragMode,
    pub orig: (f64, f64),
}

#[derive(Clone, Copy)]
pub enum NoteDragMode {
    Move,
    Resize,
}

#[derive(Clone, Copy)]
pub struct NoteDrag {
    pub index: usize,
    pub mode: NoteDragMode,
    pub orig: (f64, f64, u8),
}

pub struct PianoRollView {
    /// Highest visible key (fractional for smooth scrolling).
    pub key_top: f32,
    pub key_h: f32,
    /// Pixels per beat and the beat at the left edge.
    pub zoom: f32,
    pub scroll: f64,
    pub snap: f64,
    pub new_len: f64,
    pub drag: Option<NoteDrag>,
    pub selected: Option<usize>,
    pub preview_key: Option<u8>,
}

impl Default for PianoRollView {
    fn default() -> Self {
        Self {
            key_top: 84.0,
            key_h: 14.0,
            zoom: 96.0,
            scroll: 0.0,
            snap: 0.25,
            new_len: 0.5,
            drag: None,
            selected: None,
            preview_key: None,
        }
    }
}

pub struct View {
    pub selected_track: usize,
    pub editor: Option<EditTarget>,
    /// Vertical scroll of the track rows (shared by launcher and arranger).
    pub scroll_y: f32,
    /// Timeline: pixels per beat and the beat at the left edge.
    pub tl_zoom: f32,
    pub tl_scroll: f64,
    pub pr: PianoRollView,
    pub browser_open: bool,
    /// Which rack section the browser adds to.
    pub browser_section: Section,
    pub browser_filter: String,
    pub file_dialog: Option<FileDialog>,
    pub device_drag: Option<DeviceDrag>,
    /// A scene slot dragged from the launcher onto the timeline: (track id, scene).
    pub scene_drag: Option<(u64, usize)>,
    pub block_drag: Option<BlockDrag>,
    pub selection: Option<Selection>,
    pub recording: Option<Recording>,
    /// The master row is selected: the rack shows the master chain.
    pub master_selected: bool,
    pub rename: Option<(u64, String)>,
    /// Parameter page per plugin instance in the rack.
    pub param_page: HashMap<u64, usize>,
    /// Smoothed meter levels per track index, and the master.
    pub meters: Vec<[f32; 2]>,
    pub master_meter: [f32; 2],
    /// MIDI activity counter seen last frame, and when it last changed (LED).
    pub midi_seen: u32,
    pub midi_flash: Option<Instant>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            selected_track: 0,
            editor: None,
            scroll_y: 0.0,
            tl_zoom: 22.0,
            tl_scroll: 0.0,
            pr: PianoRollView::default(),
            browser_open: false,
            browser_section: Section::Effect,
            browser_filter: String::new(),
            file_dialog: None,
            device_drag: None,
            scene_drag: None,
            block_drag: None,
            selection: None,
            recording: None,
            master_selected: false,
            rename: None,
            param_page: HashMap::new(),
            meters: Vec::new(),
            master_meter: [0.0; 2],
            midi_seen: 0,
            midi_flash: None,
        }
    }
}

pub struct App {
    // Field order is drop order: editors close first, then the audio thread stops (which
    // stops processing on every instance), then the instances are destroyed here.
    gui: GuiManager,
    stream: Option<audio::Stream>,
    pub midi: MidiInput,
    pub project: Project,
    /// Live devices per track id, parallel to `Track::devices` (None = failed load).
    pub rack: HashMap<u64, Vec<Option<DeviceRef>>>,
    pub engine: EngineHandle,
    pub registry: Registry,
    pub driver: DriverInfo,
    pub path: PathBuf,
    pub view: View,
    pub settings: Settings,
    /// The engine snapshot is outdated.
    pub dirty: bool,
    status: Option<(String, Instant)>,
}

impl App {
    pub fn new(
        engine: EngineHandle,
        stream: audio::Stream,
        driver: DriverInfo,
        registry: Registry,
        path: PathBuf,
    ) -> Self {
        let mut engine = engine;
        let midi = match engine.take_midi_input() {
            Some(queue) => MidiInput::start(queue, engine.shared.clone()),
            None => unreachable!("the MIDI queue is taken once, here"),
        };
        let mut app = Self {
            gui: GuiManager::default(),
            stream: Some(stream),
            midi,
            project: Project::demo(),
            rack: HashMap::new(),
            engine,
            registry,
            driver,
            path,
            view: View::default(),
            settings: Settings::load(),
            dirty: true,
            status: None,
        };
        app.engine.send(Command::SetLaunchQuantize(app.settings.launch_quantize));
        if app.path.exists() {
            let path = app.path.clone();
            app.load_project(&path);
        }
        app.sync_rack();
        app.apply_start_view();
        app
    }

    /// `DAW_VIEW=piano:<track>[:<scene>]`, `rack:<track>`, `browser`, `save`,
    /// `launch:<scene>` or `record:<track>:<scene>` sets up that state at start (for
    /// screenshots and debugging).
    fn apply_start_view(&mut self) {
        let Ok(spec) = std::env::var("DAW_VIEW") else {
            return;
        };
        let parts: Vec<&str> = spec.split(':').collect();
        match parts.as_slice() {
            ["browser"] => self.view.browser_open = true,
            ["piano", track, rest @ ..] => {
                let Some(t) = track.parse::<usize>().ok().and_then(|i| self.project.tracks.get(i)) else {
                    return;
                };
                self.view.selected_track = track.parse().unwrap_or(0);
                let scene = rest.first().and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
                if scene < crate::model::SCENES && t.scenes[scene].is_some() {
                    self.view.editor = Some(EditTarget { track: t.id, scene });
                }
            }
            ["rack", track] => self.view.selected_track = track.parse().unwrap_or(0),
            ["master"] => self.view.master_selected = true,
            ["launch", scene] => self.launch_scene(scene.parse().unwrap_or(0)),
            ["record", track, scene] => {
                let (t, s) = (track.parse().unwrap_or(0), scene.parse().unwrap_or(0));
                if t < self.project.tracks.len() && s < crate::model::SCENES {
                    self.toggle_record(t, s);
                }
            }
            ["save"] => self.open_file_dialog(true),
            _ => {}
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        eprintln!("{msg}");
        self.status = Some((msg, Instant::now()));
    }

    pub fn status(&self) -> Option<&str> {
        self.status
            .as_ref()
            .filter(|(_, t)| t.elapsed().as_secs() < 6)
            .map(|(s, _)| s.as_str())
    }

    pub fn track_index(&self, id: u64) -> Option<usize> {
        self.project.tracks.iter().position(|t| t.id == id)
    }

    pub fn clip_mut(&mut self, target: EditTarget) -> Option<&mut Clip> {
        let i = self.track_index(target.track)?;
        self.project.tracks[i].scenes.get_mut(target.scene)?.as_mut()
    }

    pub fn clip(&self, target: EditTarget) -> Option<&Clip> {
        let i = self.track_index(target.track)?;
        self.project.tracks[i].scenes.get(target.scene)?.as_ref()
    }

    /// Opens the piano roll on a track's scene clip, creating a one-bar clip if the slot is
    /// empty.
    pub fn edit_scene(&mut self, track: usize, scene: usize) {
        let t = &mut self.project.tracks[track];
        if t.scenes[scene].is_none() {
            t.scenes[scene] = Some(Clip::new(BEATS_PER_BAR));
            if t.active_scene.is_none() {
                t.active_scene = Some(scene);
            }
            self.dirty = true;
        }
        self.view.selected_track = track;
        self.view.master_selected = false;
        self.view.editor = Some(EditTarget { track: t.id, scene });
        self.view.pr.selected = None;
        self.view.pr.scroll = 0.0;
    }

    // ------------------------------------------------------------- devices

    pub fn instantiate(&self, desc: &PluginDesc) -> Result<Arc<PluginInstance>, String> {
        PluginInstance::new(desc, self.engine.sample_rate, self.engine.max_block)
    }

    /// Rack section a device belongs in, from its plugin features.
    pub fn classify(&self, spec: &DeviceSpec) -> Section {
        if let Some(s) = spec.section {
            return s;
        }
        if let Some(kind) = Kind::from_id(&spec.plugin_id) {
            return kind.section();
        }
        match self.registry.find(&spec.plugin_id) {
            Some(d) if d.is_instrument() => Section::Instrument,
            Some(d) if d.is_note_effect() => Section::NoteFx,
            _ => Section::Effect,
        }
    }

    /// Device list of a track or of the master (`MASTER_ID`).
    pub fn chain_specs(&self, owner: u64) -> &[DeviceSpec] {
        if owner == MASTER_ID {
            return &self.project.master.devices;
        }
        self.project.tracks.iter().find(|t| t.id == owner).map_or(&[], |t| &t.devices)
    }

    pub fn chain_specs_mut(&mut self, owner: u64) -> Option<&mut Vec<DeviceSpec>> {
        if owner == MASTER_ID {
            return Some(&mut self.project.master.devices);
        }
        self.project.tracks.iter_mut().find(|t| t.id == owner).map(|t| &mut t.devices)
    }

    /// Ids of every device chain: the tracks, then the master.
    fn owners(&self) -> Vec<u64> {
        self.project.tracks.iter().map(|t| t.id).chain([MASTER_ID]).collect()
    }

    /// Index range of a section in a device list (which is kept sorted by section).
    pub fn section_range(&self, owner: u64, section: Section) -> std::ops::Range<usize> {
        let devs = self.chain_specs(owner);
        let start = devs.iter().take_while(|d| d.section.unwrap_or(Section::Effect) < section).count();
        let len = devs[start..].iter().take_while(|d| d.section.unwrap_or(Section::Effect) == section).count();
        start..start + len
    }

    /// Gives every device a section and sorts the chain into note FX, instrument, effects.
    /// A second device in a single-slot section moves to the effects; the master holds only
    /// effects.
    fn normalize_sections(&mut self) {
        for owner in self.owners() {
            if self.rack.contains_key(&owner) {
                continue;
            }
            let mut seen = Vec::new();
            let sections: Vec<Section> = self
                .chain_specs(owner)
                .iter()
                .map(|d| {
                    let s = if owner == MASTER_ID { Section::Effect } else { self.classify(d) };
                    if s.single() && seen.contains(&s) {
                        Section::Effect
                    } else {
                        seen.push(s);
                        s
                    }
                })
                .collect();
            let devs = self.chain_specs_mut(owner).unwrap();
            for (d, s) in devs.iter_mut().zip(sections) {
                d.section = Some(s);
            }
            // Stable sort keeps the effect order; the rack is rebuilt afterwards.
            devs.sort_by_key(|d| d.section);
        }
    }

    /// Makes sure every device chain has a rack entry matching its device list (after a load).
    fn sync_rack(&mut self) {
        self.normalize_sections();
        let mut errors = Vec::new();
        for owner in self.owners() {
            if self.rack.contains_key(&owner) {
                continue;
            }
            let mut chain = Vec::new();
            for d in self.chain_specs(owner) {
                if let Some(kind) = Kind::from_id(&d.plugin_id) {
                    let dev = BuiltinDevice::new(kind, self.engine.sample_rate as f32);
                    if let Some(map) = &d.params {
                        dev.load_map(map);
                    }
                    chain.push(Some(DeviceRef::Builtin(dev)));
                    continue;
                }
                let inst = match self.registry.find(&d.plugin_id) {
                    Some(desc) => match self.instantiate(desc) {
                        Ok(inst) => {
                            if let Some(state) = &d.state {
                                if !inst.load_state(state) {
                                    errors.push(format!("{}: state not restored", d.name));
                                }
                            }
                            Some(DeviceRef::Clap(inst))
                        }
                        Err(e) => {
                            errors.push(e);
                            None
                        }
                    },
                    None => {
                        errors.push(format!("plugin not installed: {} ({})", d.name, d.plugin_id));
                        None
                    }
                };
                chain.push(inst);
            }
            self.rack.insert(owner, chain);
        }
        if !errors.is_empty() {
            self.set_status(errors.join("; "));
        }
        self.dirty = true;
    }

    /// Puts a device into a section: single-slot sections replace their device, effects
    /// append.
    fn insert_device(&mut self, owner: u64, section: Section, mut spec: DeviceSpec, dev: DeviceRef) {
        spec.section = Some(section);
        let range = self.section_range(owner, section);
        if section.single() && !range.is_empty() {
            self.remove_device(owner, range.start);
        }
        let at = self.section_range(owner, section).end;
        let Some(devs) = self.chain_specs_mut(owner) else {
            return;
        };
        devs.insert(at, spec);
        self.rack.entry(owner).or_default().insert(at, Some(dev));
        self.dirty = true;
    }

    pub fn add_device(&mut self, owner: u64, desc: &PluginDesc, section: Section) {
        match PluginInstance::new(desc, self.engine.sample_rate, self.engine.max_block) {
            Ok(inst) => {
                let spec = DeviceSpec {
                    plugin_id: desc.id.clone(),
                    name: desc.name.clone(),
                    bypass: false,
                    state: None,
                    params: None,
                    section: None,
                };
                self.insert_device(owner, section, spec, DeviceRef::Clap(inst));
                self.set_status(format!("added {}", desc.name));
            }
            Err(e) => self.set_status(e),
        }
    }

    pub fn add_builtin(&mut self, owner: u64, kind: Kind) {
        if owner == MASTER_ID && kind.is_note() {
            return;
        }
        let dev = BuiltinDevice::new(kind, self.engine.sample_rate as f32);
        let spec = DeviceSpec {
            plugin_id: kind.id().to_owned(),
            name: kind.name().to_owned(),
            bypass: false,
            state: None,
            params: Some(dev.to_map()),
            section: None,
        };
        self.insert_device(owner, kind.section(), spec, DeviceRef::Builtin(dev));
    }

    pub fn remove_device(&mut self, owner: u64, index: usize) {
        if let Some(Some(dev)) = self.rack.get(&owner).and_then(|c| c.get(index)) {
            self.gui.close(dev.uid());
        }
        if let Some(devs) = self.chain_specs_mut(owner) {
            if index < devs.len() {
                devs.remove(index);
            }
        }
        if let Some(chain) = self.rack.get_mut(&owner) {
            // The engine still holds it until it swaps in the next snapshot; the last
            // reference comes back through the garbage queue and dies on this thread.
            if index < chain.len() {
                chain.remove(index);
            }
        }
        self.dirty = true;
    }

    pub fn move_device(&mut self, owner: u64, from: usize, to: usize) {
        let Some(devs) = self.chain_specs_mut(owner) else {
            return;
        };
        if from >= devs.len() || to >= devs.len() {
            return;
        }
        let d = devs.remove(from);
        devs.insert(to, d);
        if let Some(chain) = self.rack.get_mut(&owner) {
            let i = chain.remove(from);
            chain.insert(to, i);
        }
        self.dirty = true;
    }

    pub fn toggle_editor(&mut self, inst: &Arc<PluginInstance>, track_name: &str) {
        let title = format!("{} — {}", inst.desc.name, track_name);
        if let Err(e) = self.gui.toggle(inst, &title) {
            self.set_status(format!("{}: {e}", inst.desc.name));
        }
    }

    pub fn editor_open(&self, uid: u64) -> bool {
        self.gui.is_open(uid)
    }

    pub fn delete_track(&mut self, index: usize) {
        let t = self.project.tracks.remove(index);
        if let Some(chain) = self.rack.remove(&t.id) {
            for dev in chain.into_iter().flatten() {
                self.gui.close(dev.uid());
            }
        }
        if self.view.editor.is_some_and(|e| e.track_id() == t.id) {
            self.view.editor = None;
        }
        self.view.selected_track = self
            .view
            .selected_track
            .min(self.project.tracks.len().saturating_sub(1));
        self.dirty = true;
    }

    pub fn add_track(&mut self) {
        let i = self.project.add_track();
        self.rack.insert(self.project.tracks[i].id, Vec::new());
        self.view.selected_track = i;
        self.view.master_selected = false;
        self.dirty = true;
    }

    // ------------------------------------------------------------- project files

    pub fn open_file_dialog(&mut self, save: bool) {
        self.view.file_dialog = Some(FileDialog {
            save,
            path: self.path.display().to_string(),
        });
    }

    pub fn save_project(&mut self, path: &std::path::Path) {
        self.path = path.to_owned();
        let chains = self
            .project
            .tracks
            .iter_mut()
            .map(|t| (t.id, &mut t.devices))
            .chain([(MASTER_ID, &mut self.project.master.devices)]);
        for (owner, devices) in chains {
            if let Some(chain) = self.rack.get(&owner) {
                for (spec, dev) in devices.iter_mut().zip(chain) {
                    match dev {
                        Some(DeviceRef::Clap(inst)) => spec.state = inst.save_state(),
                        Some(DeviceRef::Builtin(dev)) => spec.params = Some(dev.to_map()),
                        None => {}
                    }
                }
            }
        }
        match std::fs::write(&self.path, self.project.to_json()) {
            Ok(()) => self.set_status(format!("saved {}", self.path.display())),
            Err(e) => self.set_status(format!("save failed: {e}")),
        }
    }

    pub fn load_project(&mut self, path: &std::path::Path) {
        let project = match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|s| Project::from_json(&s).map_err(|e| e.to_string()))
        {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("load failed: {e}"));
                return;
            }
        };
        self.path = path.to_owned();
        self.gui.close_all();
        self.engine.send(Command::Stop);
        self.project = project;
        // Old instances stay alive in the engine's snapshot until it swaps; see remove_device.
        self.rack.clear();
        self.view = View::default();
        self.sync_rack();
        if self.status().is_none() {
            self.set_status(format!("loaded {}", self.path.display()));
        }
    }

    // ------------------------------------------------------------- engine sync

    fn publish(&mut self) {
        let rack = &self.rack;
        let project = &self.project;
        let snap = Snapshot::build(&self.project, |id| {
            let specs: &[DeviceSpec] = if id == MASTER_ID {
                &project.master.devices
            } else {
                project.tracks.iter().find(|t| t.id == id).map_or(&[], |t| &t.devices)
            };
            rack.get(&id)
                .map(|chain| {
                    chain
                        .iter()
                        .zip(specs)
                        .filter_map(|(dev, spec)| {
                            dev.as_ref().map(|d| DeviceSnap {
                                device: d.clone(),
                                bypass: spec.bypass,
                                note_fx: spec.section == Some(Section::NoteFx),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        self.engine.publish(snap);
        self.dirty = false;
    }

    fn service_plugins(&mut self) {
        for chain in self.rack.values() {
            for dev in chain.iter().flatten() {
                if let DeviceRef::Clap(inst) = dev {
                    inst.idle();
                }
            }
        }
        self.gui.poll();
    }

    fn update_meters(&mut self) {
        let n = self.project.tracks.len();
        self.view.meters.resize(n, [0.0; 2]);
        let decay = 0.82;
        for i in 0..n {
            let p = self.engine.shared.take_track_peak(i);
            for (m, p) in self.view.meters[i].iter_mut().zip(p) {
                *m = p.max(*m * decay);
            }
        }
        let p = self.engine.shared.take_master_peak();
        for (m, p) in self.view.master_meter.iter_mut().zip(p) {
            *m = p.max(*m * decay);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        let (esc, space, save, open, del, home) = ctx.input(|i| {
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Space),
                i.modifiers.command && i.key_pressed(Key::S),
                i.modifiers.command && i.key_pressed(Key::O),
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                i.key_pressed(Key::Home),
            )
        });
        if save {
            self.open_file_dialog(true);
        }
        if open {
            self.open_file_dialog(false);
        }
        // The dialog handles its own keys.
        if self.view.file_dialog.is_some() {
            return;
        }
        if esc && !typing {
            if self.view.browser_open {
                self.view.browser_open = false;
            } else {
                self.view.editor = None;
                self.view.pr.drag = None;
                self.view.pr.selected = None;
            }
        }
        if space && !typing {
            self.toggle_play();
        }
        if home && !typing {
            self.engine.send(Command::Seek(0.0));
        }
        if del && !typing {
            if let (Some(target), Some(sel)) = (self.view.editor, self.view.pr.selected) {
                if let Some(clip) = self.clip_mut(target) {
                    if sel < clip.notes.len() {
                        clip.notes.remove(sel);
                        self.dirty = true;
                    }
                }
                self.view.pr.selected = None;
            } else if let Some(sel) = self.view.selection.take() {
                self.delete_selection(sel);
            }
        }
    }

    /// Uid of device `di` of a device chain, as used for selection.
    pub fn device_uid(&self, owner: u64, di: usize) -> u64 {
        match self.rack.get(&owner).and_then(|c| c.get(di)) {
            Some(Some(dev)) => dev.uid(),
            _ => rack::missing_uid(&self.chain_specs(owner)[di], di),
        }
    }

    /// Deletes a scene clip: stops it, ends a recording into it, closes its piano roll.
    pub fn delete_scene(&mut self, track: u64, scene: usize) {
        let Some(ti) = self.track_index(track) else {
            return;
        };
        if self.view.recording.as_ref().is_some_and(|r| r.track == track && r.scene == scene) {
            self.finish_recording();
        }
        let t = &self.project.tracks[ti];
        if t.active_scene == Some(scene) || t.queued_scene == Some(scene) {
            self.launch(track, None);
        }
        self.project.tracks[ti].scenes[scene] = None;
        if self.view.editor == Some(EditTarget { track, scene }) {
            self.view.editor = None;
        }
        self.dirty = true;
    }

    fn delete_selection(&mut self, sel: Selection) {
        match sel {
            Selection::Device { track, uid } => {
                if let Some(di) = (0..self.chain_specs(track).len()).find(|&di| self.device_uid(track, di) == uid) {
                    self.remove_device(track, di);
                }
            }
            Selection::Scene { track, scene } => self.delete_scene(track, scene),
            Selection::Block { track, block } => {
                let Some(ti) = self.track_index(track) else {
                    return;
                };
                let blocks = &mut self.project.tracks[ti].blocks;
                if let Some(i) = blocks.iter().position(|b| *b == block) {
                    blocks.remove(i);
                    self.dirty = true;
                }
            }
        }
    }

    pub fn toggle_play(&mut self) {
        if self.engine.shared.playing() {
            self.engine.send(Command::Stop);
        } else {
            self.engine.send(Command::Play);
        }
    }

    /// Starts every track's clip in scene column `scene` (tracks without a clip there stop).
    /// Scenes play on their own; the arranger transport is not touched.
    pub fn launch_scene(&mut self, scene: usize) {
        let launches: Vec<(u64, Option<usize>)> = self
            .project
            .tracks
            .iter()
            .map(|t| (t.id, t.scenes[scene].as_ref().map(|_| scene)))
            .collect();
        for (track, scene) in launches {
            self.launch(track, scene);
        }
    }

    /// Launches (or with `None` stops) a track's scene. Publishes a pending snapshot first,
    /// so the engine already knows the track and clip.
    pub fn launch(&mut self, track: u64, scene: Option<usize>) {
        if self.dirty {
            self.publish();
        }
        self.engine.send(Command::LaunchScene { track, scene });
    }

    // ------------------------------------------------------------- recording

    /// Record button of a scene slot: arms recording (creating the clip if needed and
    /// launching the scene), or stops it.
    pub fn toggle_record(&mut self, ti: usize, scene: usize) {
        let id = self.project.tracks[ti].id;
        if let Some(rec) = &self.view.recording {
            let same = rec.track == id && rec.scene == scene;
            self.finish_recording();
            if same {
                return;
            }
        }
        let t = &mut self.project.tracks[ti];
        let clip = t.scenes[scene].get_or_insert_with(|| Clip::new(BEATS_PER_BAR));
        let grow = clip.notes.is_empty();
        if grow {
            // Long enough not to loop while recording; trimmed in `finish_recording`.
            clip.length = 256.0;
        }
        let playing = t.active_scene == Some(scene) || t.queued_scene == Some(scene);
        self.view.recording = Some(Recording {
            track: id,
            scene,
            grow,
            started: false,
            held: [None; 128],
            max_end: 0.0,
        });
        self.view.selected_track = ti;
        self.view.master_selected = false;
        self.engine.shared.set_record_track(id);
        self.dirty = true;
        if !playing || grow {
            // (Re)start, so a growing clip begins at its start.
            self.launch(id, Some(scene));
        }
    }

    /// Ends recording: open notes end now, a grown clip is trimmed to whole bars.
    pub fn finish_recording(&mut self) {
        let Some(mut rec) = self.view.recording.take() else {
            return;
        };
        self.engine.shared.set_record_track(0);
        let Some(ti) = self.track_index(rec.track) else {
            return;
        };
        let now = self.engine.shared.scene_beat(ti);
        for key in 0..128u8 {
            if let Some((b0, v)) = rec.held[key as usize].take() {
                self.add_recorded_note(&mut rec, ti, b0, now, key, v);
            }
        }
        if let Some(clip) = self.project.tracks[ti].scenes[rec.scene].as_mut() {
            if rec.grow {
                let bar = clip.bar_beats();
                clip.length = ((rec.max_end / bar).ceil() * bar).max(bar);
            }
            clip.sort();
        }
        self.dirty = true;
    }

    fn add_recorded_note(&mut self, rec: &mut Recording, ti: usize, b0: f64, b1: f64, key: u8, velocity: f32) {
        let Some(clip) = self.project.tracks[ti].scenes[rec.scene].as_mut() else {
            return;
        };
        let (start, len) = recorded_note_span(rec.grow, clip.length, b0, b1);
        if rec.grow {
            rec.max_end = rec.max_end.max(start + len);
        }
        clip.notes.push(crate::model::Note { start, len, key, velocity });
        clip.sort();
        self.dirty = true;
    }

    /// Writes the notes the engine reported for the armed track into the clip.
    fn process_recorded(&mut self) {
        let mut events = Vec::new();
        self.engine.take_recorded(&mut events);
        let Some(mut rec) = self.view.recording.take() else {
            return;
        };
        let Some(ti) = self.track_index(rec.track) else {
            self.engine.shared.set_record_track(0);
            return;
        };
        let t = &self.project.tracks[ti];
        if t.active_scene == Some(rec.scene) {
            rec.started = true;
        }
        let (track, scene) = (rec.track, rec.scene);
        for e in events.iter().filter(|e| e.track == track && e.scene == scene) {
            let k = e.key as usize & 127;
            if e.on {
                rec.held[k] = Some((e.beat, e.velocity));
            } else if let Some((b0, v)) = rec.held[k].take() {
                self.add_recorded_note(&mut rec, ti, b0, e.beat, e.key, v);
            }
        }
        let t = &self.project.tracks[ti];
        let stopped = rec.started && t.active_scene != Some(rec.scene) && t.queued_scene != Some(rec.scene);
        self.view.recording = Some(rec);
        if stopped {
            // The scene was stopped or replaced: recording ends with it.
            self.finish_recording();
        }
    }

    /// Bars.beats.sixteenths display of a beat position.
    pub fn format_position(beats: f64) -> String {
        let bar = (beats / BEATS_PER_BAR).floor();
        let beat = (beats - bar * BEATS_PER_BAR).floor();
        let six = ((beats - beats.floor()) * 4.0).floor();
        format!("{:>3}.{}.{}", bar as i64 + 1, beat as i64 + 1, six as i64 + 1)
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.engine.collect_garbage();
        self.service_plugins();
        self.update_meters();
        // Launcher state lives in the engine; mirror it for drawing.
        for (i, t) in self.project.tracks.iter_mut().enumerate() {
            t.active_scene = self.engine.shared.scene_playing(i);
            t.queued_scene = self.engine.shared.scene_queued(i);
        }
        self.process_recorded();
        // The selected track receives the MIDI keyboard.
        let target = self.project.tracks.get(self.view.selected_track).map_or(0, |t| t.id);
        self.engine.shared.set_midi_target(target);
        let seen = self.engine.shared.midi_activity_count();
        if seen != self.view.midi_seen {
            self.view.midi_seen = seen;
            self.view.midi_flash = Some(Instant::now());
        }
        self.shortcuts(ctx);
        // Ctrl +/-/0 (handled by egui) or the scale menu change the zoom; remember it.
        let zoom = ctx.zoom_factor();
        if (zoom - self.settings.ui_scale).abs() > 1e-3 {
            self.settings.ui_scale = zoom;
            self.settings.save();
        }

        let t = th();
        egui::TopBottomPanel::top("transport")
            .exact_height(44.0)
            .frame(egui::Frame::new().fill(t.bg_dark))
            .show(ctx, |ui| self.transport_ui(ui));
        egui::TopBottomPanel::bottom("rack")
            .resizable(false)
            .exact_height(RACK_H)
            .frame(egui::Frame::new().fill(t.bg_dark))
            .show(ctx, |ui| self.rack_ui(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg))
            .show(ctx, |ui| self.main_ui(ui));
        if self.view.browser_open {
            self.browser_ui(ctx);
        }
        if self.view.file_dialog.is_some() {
            self.file_dialog_ui(ctx);
        }

        if self.dirty {
            self.publish();
        }
        // Playhead, meters and plugin callbacks need a steady frame rate.
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.gui.close_all();
        // Stopping the audio thread stops processing on every instance before they are
        // deactivated and destroyed (when `rack` and the engine handle drop).
        self.stream.take();
        self.engine.collect_garbage();
    }
}

impl App {
    fn main_ui(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        let launcher = egui::Rect::from_min_max(full.min, egui::pos2(full.left() + LAUNCHER_W, full.bottom()));
        let right = egui::Rect::from_min_max(egui::pos2(launcher.right() + 6.0, full.top()), full.max);
        self.launcher_ui(ui, launcher);
        match self.view.editor {
            Some(target) if self.track_index(target.track_id()).is_some() => {
                self.piano_roll_ui(ui, right, target)
            }
            _ => {
                self.view.editor = None;
                self.arranger_ui(ui, right);
            }
        }
        // A scene slot dragged towards the timeline: label under the pointer. The arranger
        // has already handled a drop this frame.
        if let Some((_, scene)) = self.view.scene_drag {
            let (down, pointer) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
            if let (true, Some(pp)) = (down, pointer) {
                let t = th();
                let fp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("scene_ghost")));
                let r = egui::Rect::from_min_size(pp + egui::vec2(12.0, 8.0), egui::vec2(84.0, 20.0));
                widgets::section_bar(&fp, r, &format!("SCENE {}", scene + 1), t.accent);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            } else {
                self.view.scene_drag = None;
            }
        }
    }
}

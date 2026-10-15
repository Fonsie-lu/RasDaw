//! The main window.
//!
//! Layout (Bitwig-like):
//! - top: transport bar
//! - top left: scene launcher, 4 scene slots per track
//! - top right: track headers and the arranger timeline, rows aligned with the scene rows;
//!   double-clicking a lane (or a scene slot) replaces the timeline with the piano roll, ESC
//!   returns
//! - bottom: device rack of the selected track, left to right in signal order

mod arranger;
mod launcher;
mod piano_roll;
mod rack;
mod transport;
pub mod widgets;

use crate::audio::{self, DriverInfo};
use crate::clap_host::gui::GuiManager;
use crate::clap_host::{PluginDesc, PluginInstance, Registry};
use crate::engine::{Command, DeviceSnap, EngineHandle, Snapshot};
use crate::model::{Clip, DeviceSpec, Project, BEATS_PER_BAR};
use crate::theme::current as th;
use eframe::egui::{self, Key};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Height of a track row; the scene launcher and the arranger share it so rows line up.
pub const ROW_H: f32 = 64.0;
/// Section bars above the launcher / arranger.
pub const BAR_H: f32 = 24.0;
/// Scene header buttons and the timeline ruler.
pub const HEADER_H: f32 = 28.0;
pub const SLOT_W: f32 = 96.0;
pub const LAUNCHER_W: f32 = SLOT_W * crate::model::SCENES as f32 + 8.0;
pub const TRACK_HEAD_W: f32 = 168.0;

/// What the piano roll edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    Arrangement(u64),
    Scene(u64, usize),
}

impl EditTarget {
    pub fn track_id(self) -> u64 {
        match self {
            EditTarget::Arrangement(id) | EditTarget::Scene(id, _) => id,
        }
    }
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
    pub browser_filter: String,
    pub rename: Option<(u64, String)>,
    /// Parameter page per plugin instance in the rack.
    pub param_page: HashMap<u64, usize>,
    /// Smoothed meter levels per track index, and the master.
    pub meters: Vec<[f32; 2]>,
    pub master_meter: [f32; 2],
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
            browser_filter: String::new(),
            rename: None,
            param_page: HashMap::new(),
            meters: Vec::new(),
            master_meter: [0.0; 2],
        }
    }
}

pub struct App {
    // Field order is drop order: editors close first, then the audio thread stops (which
    // stops processing on every instance), then the instances are destroyed here.
    gui: GuiManager,
    stream: Option<audio::Stream>,
    pub project: Project,
    /// Live plugin instances per track id, parallel to `Track::devices` (None = failed load).
    pub rack: HashMap<u64, Vec<Option<Arc<PluginInstance>>>>,
    pub engine: EngineHandle,
    pub registry: Registry,
    pub driver: DriverInfo,
    pub path: PathBuf,
    pub path_text: String,
    pub view: View,
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
        let mut app = Self {
            gui: GuiManager::default(),
            stream: Some(stream),
            project: Project::demo(),
            rack: HashMap::new(),
            engine,
            registry,
            driver,
            path_text: path.display().to_string(),
            path,
            view: View::default(),
            dirty: true,
            status: None,
        };
        if app.path.exists() {
            app.load_project();
        }
        app.sync_rack();
        app.apply_start_view();
        app
    }

    /// `DAW_VIEW=piano:<track>[:<scene>]` or `DAW_VIEW=browser` opens that view at start
    /// (for screenshots and debugging).
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
                self.view.editor = Some(match rest.first().and_then(|s| s.parse::<usize>().ok()) {
                    Some(s) if s < crate::model::SCENES && t.scenes[s].is_some() => EditTarget::Scene(t.id, s),
                    _ => EditTarget::Arrangement(t.id),
                });
            }
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
        let i = self.track_index(target.track_id())?;
        let t = &mut self.project.tracks[i];
        match target {
            EditTarget::Arrangement(_) => Some(&mut t.arrangement),
            EditTarget::Scene(_, s) => t.scenes[s].as_mut(),
        }
    }

    pub fn clip(&self, target: EditTarget) -> Option<&Clip> {
        let t = &self.project.tracks[self.track_index(target.track_id())?];
        match target {
            EditTarget::Arrangement(_) => Some(&t.arrangement),
            EditTarget::Scene(_, s) => t.scenes[s].as_ref(),
        }
    }

    // ------------------------------------------------------------- devices

    pub fn instantiate(&self, desc: &PluginDesc) -> Result<Arc<PluginInstance>, String> {
        PluginInstance::new(desc, self.engine.sample_rate, self.engine.max_block)
    }

    /// Makes sure every track has a rack entry matching its device list (after a load).
    fn sync_rack(&mut self) {
        let mut errors = Vec::new();
        for t in &self.project.tracks {
            if self.rack.contains_key(&t.id) {
                continue;
            }
            let mut chain = Vec::new();
            for d in &t.devices {
                let inst = match self.registry.find(&d.plugin_id) {
                    Some(desc) => match self.instantiate(desc) {
                        Ok(inst) => {
                            if let Some(state) = &d.state {
                                if !inst.load_state(state) {
                                    errors.push(format!("{}: state not restored", d.name));
                                }
                            }
                            Some(inst)
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
            self.rack.insert(t.id, chain);
        }
        if !errors.is_empty() {
            self.set_status(errors.join("; "));
        }
        self.dirty = true;
    }

    pub fn add_device(&mut self, track: usize, desc: &PluginDesc) {
        let Some(t) = self.project.tracks.get_mut(track) else {
            return;
        };
        let id = t.id;
        match PluginInstance::new(desc, self.engine.sample_rate, self.engine.max_block) {
            Ok(inst) => {
                t.devices.push(DeviceSpec {
                    plugin_id: desc.id.clone(),
                    name: desc.name.clone(),
                    bypass: false,
                    state: None,
                });
                self.rack.entry(id).or_default().push(Some(inst));
                self.dirty = true;
                self.set_status(format!("added {}", desc.name));
            }
            Err(e) => self.set_status(e),
        }
    }

    pub fn remove_device(&mut self, track: usize, index: usize) {
        let id = self.project.tracks[track].id;
        if let Some(Some(inst)) = self.rack.get(&id).and_then(|c| c.get(index)) {
            self.gui.close(inst.uid);
        }
        self.project.tracks[track].devices.remove(index);
        if let Some(chain) = self.rack.get_mut(&id) {
            // The engine still holds it until it swaps in the next snapshot; the last
            // reference comes back through the garbage queue and dies on this thread.
            chain.remove(index);
        }
        self.dirty = true;
    }

    pub fn move_device(&mut self, track: usize, from: usize, to: usize) {
        let id = self.project.tracks[track].id;
        let devs = &mut self.project.tracks[track].devices;
        if from >= devs.len() || to >= devs.len() {
            return;
        }
        let d = devs.remove(from);
        devs.insert(to, d);
        if let Some(chain) = self.rack.get_mut(&id) {
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
            for inst in chain.into_iter().flatten() {
                self.gui.close(inst.uid);
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
        self.dirty = true;
    }

    // ------------------------------------------------------------- project files

    pub fn save_project(&mut self) {
        self.path = PathBuf::from(self.path_text.trim());
        for t in &mut self.project.tracks {
            if let Some(chain) = self.rack.get(&t.id) {
                for (spec, inst) in t.devices.iter_mut().zip(chain) {
                    if let Some(inst) = inst {
                        spec.state = inst.save_state();
                    }
                }
            }
        }
        match std::fs::write(&self.path, self.project.to_json()) {
            Ok(()) => self.set_status(format!("saved {}", self.path.display())),
            Err(e) => self.set_status(format!("save failed: {e}")),
        }
    }

    pub fn load_project(&mut self) {
        self.path = PathBuf::from(self.path_text.trim());
        let project = match std::fs::read_to_string(&self.path)
            .map_err(|e| e.to_string())
            .and_then(|s| Project::from_json(&s).map_err(|e| e.to_string()))
        {
            Ok(p) => p,
            Err(e) => {
                self.set_status(format!("load failed: {e}"));
                return;
            }
        };
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
        let tracks = &self.project.tracks;
        let snap = Snapshot::build(&self.project, |id| {
            let specs = &tracks.iter().find(|t| t.id == id).unwrap().devices;
            rack.get(&id)
                .map(|chain| {
                    chain
                        .iter()
                        .zip(specs)
                        .filter_map(|(inst, spec)| {
                            inst.as_ref().map(|i| DeviceSnap {
                                instance: i.clone(),
                                bypass: spec.bypass,
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
            for inst in chain.iter().flatten() {
                inst.idle();
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
        let (esc, space, save, del, home) = ctx.input(|i| {
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Space),
                i.modifiers.command && i.key_pressed(Key::S),
                i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace),
                i.key_pressed(Key::Home),
            )
        });
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
        if save {
            self.save_project();
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
    pub fn launch_scene(&mut self, scene: usize) {
        for t in &mut self.project.tracks {
            t.active_scene = t.scenes[scene].as_ref().map(|_| scene);
        }
        self.dirty = true;
        if !self.engine.shared.playing() {
            self.engine.send(Command::Play);
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
        self.shortcuts(ctx);

        let t = th();
        egui::TopBottomPanel::top("transport")
            .exact_height(44.0)
            .frame(egui::Frame::new().fill(t.bg_dark))
            .show(ctx, |ui| self.transport_ui(ui));
        egui::TopBottomPanel::bottom("rack")
            .resizable(true)
            .default_height(300.0)
            .min_height(150.0)
            .frame(egui::Frame::new().fill(t.bg_dark))
            .show(ctx, |ui| self.rack_ui(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.bg))
            .show(ctx, |ui| self.main_ui(ui));
        if self.view.browser_open {
            self.browser_ui(ctx);
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
    }
}

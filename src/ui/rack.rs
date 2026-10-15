//! Device rack (bottom, fixed height): the selected track's channel strip, then three
//! sections in signal order:
//! - NOTE FX: one note-altering device,
//! - INSTRUMENT: one instrument,
//! - EFFECTS: any number of audio effects (CLAP or built-in).
//!
//! Cards have only a title bar: the cross at its left deletes the device, a click toggles it
//! on/off, dragging (effects) reorders, right-click opens the editor and removal menu. A click
//! anywhere on a card selects it; Del deletes the selected device. Built-in device bodies are drawn by
//! `builtin_card.rs`.

use super::widgets::{self, button, caption, fader, knob, meter, section_bar, with_alpha};
use super::{App, DeviceDrag};
use crate::clap_host::PluginInstance;
use crate::devices::Kind;
use crate::engine::{volume_gain, DeviceRef};
use crate::model::{DeviceSpec, Section, MASTER_ID};
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Id, LayerId, Order, Rect, Sense, Stroke};
use std::sync::Arc;

const STRIP_W: f32 = 168.0;
pub(super) const CARD_W: f32 = 304.0;
const EMPTY_W: f32 = 128.0;
const PLUS_W: f32 = 84.0;
const GAP: f32 = 10.0;
const GROUP_GAP: f32 = 22.0;
const LABEL_H: f32 = 16.0;
pub(super) const TITLE_H: f32 = 22.0;
/// Top of the card body, relative to the card.
pub(super) const BODY_TOP: f32 = 4.0 + TITLE_H + 6.0;
const COLS: usize = 4;
/// Height a knob cell needs: knob, name and value lines.
pub(super) const CELL_H: f32 = 64.0;

const SECTIONS: [(Section, &str); 3] = [
    (Section::NoteFx, "NOTE FX"),
    (Section::Instrument, "INSTRUMENT"),
    (Section::Effect, "EFFECTS"),
];

pub(super) enum RackAction {
    Bypass(usize),
    Editor(usize),
    Remove(usize),
    Page(u64, usize),
    Browse(Section),
    StartDrag(usize, egui::Vec2),
    Select(u64),
}

fn card_width(dev: Option<&DeviceRef>, h: f32) -> f32 {
    match dev {
        Some(DeviceRef::Builtin(d)) => super::builtin_card::width(d.kind, h),
        _ => CARD_W,
    }
}

impl App {
    pub(super) fn rack_ui(&mut self, ui: &mut egui::Ui) {
        let t = th();
        let full = ui.max_rect();
        let p = ui.painter().clone();
        p.rect_filled(full, widgets::r(0.0), t.bg_dark);
        p.line_segment([full.left_top(), full.right_top()], Stroke::new(1.0_f32, t.bevel));
        // Claim the whole panel so it keeps its fixed size.
        ui.allocate_rect(full, Sense::hover());
        let master = self.view.master_selected || self.project.tracks.is_empty();
        if !master {
            self.view.selected_track = self.view.selected_track.min(self.project.tracks.len() - 1);
        }
        let area = full.shrink2(vec2(8.0, 8.0));
        let strip = Rect::from_min_size(area.min, vec2(STRIP_W, area.height()));
        // `track_id` is the device chain owner: a track id or MASTER_ID.
        let track_id = if master {
            self.master_strip(ui, strip);
            MASTER_ID
        } else {
            let ti = self.view.selected_track;
            self.channel_strip(ui, strip, ti);
            self.project.tracks[ti].id
        };

        let devices = Rect::from_min_max(pos2(strip.right() + 12.0, area.top()), area.max);
        let mut actions = Vec::new();
        let mut effect_cards: Vec<(usize, Rect)> = Vec::new();
        let chain = self.rack.get(&track_id).cloned().unwrap_or_default();
        let specs = self.chain_specs(track_id).to_vec();
        // The master holds only effects.
        let sections: Vec<(Section, &str)> = SECTIONS.iter().copied().filter(|(s, _)| !master || *s == Section::Effect).collect();
        let ranges: Vec<_> = sections.iter().map(|(s, _)| self.section_range(track_id, *s)).collect();
        let effects = self.section_range(track_id, Section::Effect);

        ui.scope_builder(egui::UiBuilder::new().max_rect(devices), |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("rack_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let h = devices.height() - 12.0;
                        let card_h = h - LABEL_H;
                        for (gi, ((section, label), range)) in sections.iter().zip(&ranges).enumerate() {
                            let widths: Vec<f32> = range.clone().map(|di| card_width(chain[di].as_ref(), card_h)).collect();
                            let extra = if section.single() {
                                if range.is_empty() { EMPTY_W } else { 0.0 }
                            } else {
                                PLUS_W + if range.is_empty() { 0.0 } else { GAP }
                            };
                            let group_w = widths.iter().sum::<f32>() + GAP * widths.len().saturating_sub(1) as f32 + extra;
                            let (group, _) = ui.allocate_exact_size(vec2(group_w, h), Sense::hover());
                            caption(ui.painter(), group.left_top() + vec2(2.0, 0.0), Align2::LEFT_TOP, label, 10.0, t.text_faint);
                            let mut x = group.left();
                            let top = group.top() + LABEL_H;
                            for (di, w) in range.clone().zip(widths) {
                                let rect = Rect::from_min_size(pos2(x, top), vec2(w, card_h));
                                self.device_card(ui, rect, di, &specs[di], chain[di].as_ref(), &mut actions);
                                if *section == Section::Effect {
                                    effect_cards.push((di, rect));
                                }
                                x += w + GAP;
                            }
                            if extra > 0.0 {
                                let w = if section.single() { EMPTY_W } else { PLUS_W };
                                let rect = Rect::from_min_size(pos2(group.right() - w, top), vec2(w, card_h));
                                let text = match section {
                                    Section::NoteFx => "NOTE FX",
                                    Section::Instrument => "INSTRUMENT",
                                    Section::Effect => "EFFECT",
                                };
                                if add_card(ui, rect, ("add", gi), text) {
                                    actions.push(RackAction::Browse(*section));
                                }
                            }
                            if gi + 1 < sections.len() {
                                let (gap, _) = ui.allocate_exact_size(vec2(GROUP_GAP, h), Sense::hover());
                                caption(ui.painter(), gap.center() + vec2(0.0, LABEL_H * 0.5), Align2::CENTER_CENTER, "›", 18.0, t.text_faint);
                            }
                        }
                    });
                    self.device_drop(ui, track_id, &effects, &effect_cards, &specs);
                });
        });

        for a in actions {
            let track_name = if master { "Master".to_owned() } else { self.project.tracks[self.view.selected_track].name.clone() };
            match a {
                RackAction::Bypass(i) => {
                    if let Some(d) = self.chain_specs_mut(track_id).and_then(|d| d.get_mut(i)) {
                        d.bypass = !d.bypass;
                    }
                    self.dirty = true;
                }
                RackAction::Editor(i) => {
                    if let Some(Some(DeviceRef::Clap(inst))) = self.rack.get(&track_id).and_then(|c| c.get(i)).cloned() {
                        self.toggle_editor(&inst, &track_name);
                    }
                }
                RackAction::Remove(i) => self.remove_device(track_id, i),
                RackAction::Page(uid, page) => {
                    self.view.param_page.insert(uid, page);
                }
                RackAction::Browse(section) => {
                    self.view.browser_section = section;
                    self.view.browser_open = true;
                }
                RackAction::StartDrag(from, grab) => {
                    self.view.device_drag = Some(DeviceDrag { track: track_id, from, grab });
                }
                RackAction::Select(uid) => {
                    self.view.selection = Some(super::Selection::Device { track: track_id, uid });
                }
            }
        }
    }

    /// While an effect card is dragged: ghost title under the pointer and an insertion
    /// marker; on release, move the device there.
    fn device_drop(&mut self, ui: &mut egui::Ui, owner: u64, effects: &std::ops::Range<usize>, cards: &[(usize, Rect)], specs: &[DeviceSpec]) {
        let Some(drag) = self.view.device_drag else {
            return;
        };
        if drag.track != owner || !effects.contains(&drag.from) {
            self.view.device_drag = None;
            return;
        }
        let t = th();
        let (down, pointer) = ui.input(|i| (i.pointer.primary_down(), i.pointer.latest_pos()));
        let Some(pp) = pointer else {
            return;
        };
        // Insertion slot: number of cards whose centre is left of the pointer.
        let slot = cards.iter().filter(|(_, r)| r.center().x < pp.x).count();
        let marker_x = match cards.get(slot) {
            Some((_, r)) => r.left() - GAP * 0.5,
            None => cards.last().map_or(pp.x, |(_, r)| r.right() + GAP * 0.5),
        };
        if let Some((_, r)) = cards.first() {
            let p = ui.painter();
            p.line_segment([pos2(marker_x, r.top()), pos2(marker_x, r.bottom())], Stroke::new(3.0_f32, t.accent));
        }
        let fp = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new("device_ghost")));
        let ghost = Rect::from_min_size(pp - drag.grab, vec2(CARD_W.min(220.0), TITLE_H));
        fp.rect_filled(ghost.translate(vec2(0.0, 3.0)), widgets::r(3.0), egui::Color32::from_black_alpha(120));
        section_bar(&fp, ghost, &specs[drag.from].name.to_uppercase(), with_alpha(t.device_bars[1], 0.85));
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        if !down {
            let from_local = drag.from - effects.start;
            let to_local = if slot > from_local { slot - 1 } else { slot };
            if to_local != from_local {
                self.move_device(owner, drag.from, effects.start + to_local);
            }
            self.view.device_drag = None;
        }
    }

    /// Device chain shown in the rack: the selected track's, or the master's.
    pub(super) fn rack_owner(&self) -> u64 {
        match self.project.tracks.get(self.view.selected_track) {
            Some(t) if !self.view.master_selected => t.id,
            _ => MASTER_ID,
        }
    }

    /// Master strip: volume and pan of the sum, master meter.
    fn master_strip(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let t = th();
        let p = ui.painter().clone();
        p.rect_filled(rect, widgets::r(4.0), t.bg_panel);
        let bar = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, 20.0));
        section_bar(&p, bar, "MASTER", t.text);
        let m = &self.project.master;
        let (vol0, pan0) = (m.volume, m.pan);
        let body_top = bar.bottom() + 8.0;
        let fr = Rect::from_min_max(pos2(rect.left() + 14.0, body_top), pos2(rect.left() + 54.0, rect.bottom() - 22.0));
        let (_, new_vol) = fader(ui, fr, "master_vol", vol0, 0.8, t.text);
        let vol = new_vol.unwrap_or(vol0);
        let db = 20.0 * volume_gain(vol).max(1e-6).log10();
        let db_text = if db < -60.0 { "-∞ dB".to_owned() } else { format!("{db:+.1} dB") };
        caption(&p, pos2(fr.center().x + 4.0, rect.bottom() - 10.0), Align2::CENTER_CENTER, &db_text, 9.5, t.text_dim);
        let kc = pos2(rect.left() + 100.0, body_top + 26.0);
        let (_, new_pan) = knob(ui, kc, 16.0, "master_pan", (pan0 + 1.0) * 0.5, 0.5, t.text);
        let pan = new_pan.map(|v| v * 2.0 - 1.0).unwrap_or(pan0);
        caption(&p, kc + vec2(0.0, 26.0), Align2::CENTER_CENTER, "PAN", 9.5, t.text_dim);
        let pan_text = match (pan * 50.0).round() as i32 {
            0 => "C".to_owned(),
            v if v < 0 => format!("L{}", -v),
            v => format!("R{v}"),
        };
        caption(&p, kc + vec2(0.0, 38.0), Align2::CENTER_CENTER, &pan_text, 9.5, t.text);
        meter(&p, Rect::from_min_max(pos2(rect.right() - 20.0, body_top), pos2(rect.right() - 9.0, rect.bottom() - 10.0)), self.view.master_meter);
        if new_vol.is_some() || new_pan.is_some() {
            self.project.master.volume = vol;
            self.project.master.pan = pan;
            self.dirty = true;
        }
    }

    fn channel_strip(&mut self, ui: &mut egui::Ui, rect: Rect, ti: usize) {
        let t = th();
        let p = ui.painter().clone();
        let track = &self.project.tracks[ti];
        let color = t.track(track.color);
        p.rect_filled(rect, widgets::r(4.0), t.bg_panel);
        let bar = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, 20.0));
        section_bar(&p, bar, &widgets::fit_text(ui, &track.name.to_uppercase(), 13.0, bar.width() - 10.0), color);

        let body_top = bar.bottom() + 8.0;
        let fr = Rect::from_min_max(pos2(rect.left() + 14.0, body_top), pos2(rect.left() + 54.0, rect.bottom() - 22.0));
        let (_, new_vol) = fader(ui, fr, ("vol", track.id), track.volume, 0.8, color);
        let vol = new_vol.unwrap_or(track.volume);
        let db = 20.0 * volume_gain(vol).max(1e-6).log10();
        let db_text = if db < -60.0 { "-∞ dB".to_owned() } else { format!("{db:+.1} dB") };
        caption(&p, pos2(fr.center().x + 4.0, rect.bottom() - 10.0), Align2::CENTER_CENTER, &db_text, 9.5, t.text_dim);

        let kc = pos2(rect.left() + 100.0, body_top + 26.0);
        let pan01 = (track.pan + 1.0) * 0.5;
        let (_, new_pan) = knob(ui, kc, 16.0, ("pan", track.id), pan01, 0.5, color);
        let pan = new_pan.map(|v| v * 2.0 - 1.0).unwrap_or(track.pan);
        caption(&p, kc + vec2(0.0, 26.0), Align2::CENTER_CENTER, "PAN", 9.5, t.text_dim);
        let pan_text = match (pan * 50.0).round() as i32 {
            0 => "C".to_owned(),
            v if v < 0 => format!("L{}", -v),
            v => format!("R{v}"),
        };
        caption(&p, kc + vec2(0.0, 38.0), Align2::CENTER_CENTER, &pan_text, 9.5, t.text);

        let (mute, solo, id) = (track.mute, track.solo, track.id);
        let mb = Rect::from_min_size(pos2(kc.x - 26.0, kc.y + 52.0), vec2(24.0, 18.0));
        let toggle_mute = button(ui, mb, ("rack_mute", id), "M", mute.then_some(t.mute)).clicked();
        let sb = mb.translate(vec2(28.0, 0.0));
        let toggle_solo = button(ui, sb, ("rack_solo", id), "S", solo.then_some(t.solo)).clicked();

        let level = self.view.meters.get(ti).copied().unwrap_or([0.0; 2]);
        meter(&p, Rect::from_min_max(pos2(rect.right() - 20.0, body_top), pos2(rect.right() - 9.0, rect.bottom() - 10.0)), level);

        let track = &mut self.project.tracks[ti];
        if new_vol.is_some() || new_pan.is_some() || toggle_mute || toggle_solo {
            track.volume = vol;
            track.pan = pan;
            track.mute ^= toggle_mute;
            track.solo ^= toggle_solo;
            self.dirty = true;
        }
    }


    /// Card frame shared by all devices: body and title bar. Click the title to toggle
    /// on/off, drag it to reorder (effects), right-click for the editor and removal.
    /// `glyph_w` reserves room at the right end of the title for extra controls.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn device_frame(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        di: usize,
        uid: u64,
        spec: &DeviceSpec,
        color: egui::Color32,
        has_editor: bool,
        glyph_w: f32,
        actions: &mut Vec<RackAction>,
    ) -> Rect {
        let t = th();
        let p = ui.painter().clone();
        let dragging = self.view.device_drag.is_some_and(|d| d.from == di);
        p.rect_filled(rect.translate(vec2(0.0, 3.0)), widgets::r(4.0), egui::Color32::from_black_alpha(120));
        p.rect_filled(rect, widgets::r(4.0), if dragging { t.bg_dark } else { t.bg_panel });
        p.line_segment([rect.left_top() + vec2(4.0, 0.0), rect.right_top() - vec2(4.0, 0.0)], Stroke::new(1.0_f32, t.bevel));
        // Any press inside the card selects it (Del removes the selection).
        let pressed_here = ui.input(|i| i.pointer.any_pressed() && i.pointer.press_origin().is_some_and(|o| rect.contains(o)));
        if pressed_here {
            actions.push(RackAction::Select(uid));
        }
        if self.view.selection == Some(super::Selection::Device { track: self.rack_owner(), uid }) {
            widgets::outline(&p, rect.expand(2.0), t.accent, 2.0);
        }
        let bar = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, TITLE_H));
        // Delete cross at the left end of the title bar.
        let cross = Rect::from_min_size(bar.min, vec2(24.0, TITLE_H));
        let title = Rect::from_min_max(pos2(cross.right(), bar.top()), pos2(bar.right() - glyph_w, bar.bottom()));
        let effect = spec.section == Some(Section::Effect);
        let sense = if effect { Sense::click_and_drag() } else { Sense::click() };
        let resp = ui.interact(title, Id::new(("dev_title", uid)), sense);
        let bar_col = if spec.bypass { t.text_faint } else { color };
        let bar_col = if resp.hovered() { widgets::with_alpha(bar_col, 0.85) } else { bar_col };
        p.rect_filled(bar, widgets::r(3.0), bar_col);
        let name = widgets::fit_text(ui, &spec.name.to_uppercase(), 13.0, title.width() - 12.0);
        caption(&p, title.center(), Align2::CENTER_CENTER, &name, 13.0, t.bar_text);
        if glyph(ui, cross, ("del", uid), "✕", false).on_hover_text("Delete device (Del)").clicked() {
            actions.push(RackAction::Remove(di));
        }
        if resp.clicked() {
            actions.push(RackAction::Bypass(di));
        }
        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(title.min);
            actions.push(RackAction::StartDrag(di, origin - bar.min));
        }
        let hint = if spec.bypass { "off — click to turn on" } else { "on — click to turn off" };
        let resp = resp.on_hover_text(format!("{}: {hint}{}", spec.name, if effect { ", drag to move" } else { "" }));
        resp.context_menu(|ui| {
            if has_editor && ui.button("Open editor").clicked() {
                actions.push(RackAction::Editor(di));
                ui.close_menu();
            }
            if ui.button(if spec.bypass { "Turn on" } else { "Turn off" }).clicked() {
                actions.push(RackAction::Bypass(di));
                ui.close_menu();
            }
            if ui.button("Remove").clicked() {
                actions.push(RackAction::Remove(di));
                ui.close_menu();
            }
        });
        Rect::from_min_max(pos2(rect.left(), rect.top() + BODY_TOP), rect.max)
    }

    fn device_card(&self, ui: &mut egui::Ui, rect: Rect, di: usize, spec: &DeviceSpec, dev: Option<&DeviceRef>, actions: &mut Vec<RackAction>) {
        let t = th();
        match dev {
            Some(DeviceRef::Builtin(d)) => self.builtin_card(ui, rect, di, spec, d, actions),
            Some(DeviceRef::Clap(inst)) => self.clap_card(ui, rect, di, spec, inst, actions),
            None => {
                let body = self.device_frame(ui, rect, di, missing_uid(spec, di), spec, t.record, false, 0.0, actions);
                caption(ui.painter(), body.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, "NOT INSTALLED", 12.0, t.record);
                caption(ui.painter(), body.center() + vec2(0.0, 10.0), Align2::CENTER_CENTER, &spec.plugin_id, 10.0, t.text_dim);
            }
        }
    }

    fn clap_card(&self, ui: &mut egui::Ui, rect: Rect, di: usize, spec: &DeviceSpec, inst: &Arc<PluginInstance>, actions: &mut Vec<RackAction>) {
        let t = th();
        let kind = if inst.desc.is_instrument() {
            0
        } else if inst.desc.is_effect() {
            1
        } else {
            2
        };
        let accent = t.device_bars[kind];
        let uid = inst.uid;
        let params = inst.params();
        let body_h = rect.height() - BODY_TOP - 6.0;
        let rows = ((body_h / CELL_H).floor() as usize).clamp(1, 4);
        let page_size = COLS * rows;
        let pages = params.len().div_ceil(page_size).max(1);
        let page = self.view.param_page.get(&uid).copied().unwrap_or(0).min(pages - 1);

        // Title glyphs (right to left): editor window, page arrows.
        let glyph_w = if inst.has_gui() { 26.0 } else { 0.0 } + if pages > 1 { 70.0 } else { 0.0 };
        let body = self.device_frame(ui, rect, di, uid, spec, accent, inst.has_gui(), glyph_w, actions);
        let bar = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, TITLE_H));
        let mut gx = bar.right();
        if inst.has_gui() {
            let r = Rect::from_min_max(pos2(gx - 26.0, bar.top()), pos2(gx, bar.bottom()));
            let open = self.editor_open(uid);
            if glyph(ui, r, ("ed", uid), "", open).on_hover_text("Plugin editor").clicked() {
                actions.push(RackAction::Editor(di));
            }
            gx -= 26.0;
        }
        if pages > 1 {
            let r = Rect::from_min_max(pos2(gx - 18.0, bar.top()), pos2(gx, bar.bottom()));
            if glyph(ui, r, ("pg+", uid), "›", false).clicked() && page + 1 < pages {
                actions.push(RackAction::Page(uid, page + 1));
            }
            caption(ui.painter(), pos2(gx - 35.0, bar.center().y), Align2::CENTER_CENTER, &format!("{}/{}", page + 1, pages), 10.0, t.bar_text);
            let r = Rect::from_min_max(pos2(gx - 70.0, bar.top()), pos2(gx - 52.0, bar.bottom()));
            if glyph(ui, r, ("pg-", uid), "‹", false).clicked() && page > 0 {
                actions.push(RackAction::Page(uid, page - 1));
            }
        }

        let p = ui.painter().clone();
        let cell_w = (rect.width() - 12.0) / COLS as f32;
        let cell_h = (body.height() - 6.0) / rows as f32;
        if params.is_empty() {
            caption(&p, body.center(), Align2::CENTER_CENTER, "no parameters", 11.0, t.text_faint);
        }
        for (slot, info) in params.iter().skip(page * page_size).take(page_size).enumerate() {
            let col = slot % COLS;
            let row = slot / COLS;
            let cx = rect.left() + 6.0 + cell_w * (col as f32 + 0.5);
            let radius = 14.0;
            let cy = body.top() + cell_h * row as f32 + (cell_h - CELL_H).max(0.0) * 0.5 + radius + 4.0;
            let range = (info.max - info.min).max(1e-9);
            let value = inst.param_value(info.id).unwrap_or(info.default);
            let norm = ((value - info.min) / range) as f32;
            let def = ((info.default - info.min) / range) as f32;
            let (resp, new) = knob(ui, pos2(cx, cy), radius, ("param", uid, info.id), norm, def, accent);
            if let Some(n) = new {
                let mut v = info.min + n as f64 * range;
                if info.is_stepped() {
                    v = v.round();
                }
                inst.set_param(info.id, v);
            }
            let shown = new.map(|n| info.min + n as f64 * range).unwrap_or(value);
            let text = inst.param_text(info.id, shown).unwrap_or_else(|| format!("{shown:.2}"));
            let name = widgets::fit_text(ui, &info.name, 9.5, cell_w - 6.0);
            caption(&p, pos2(cx, cy + radius + 9.0), Align2::CENTER_CENTER, &name, 9.5, t.text);
            let vt = widgets::fit_text(ui, &text, 9.0, cell_w - 6.0);
            caption(&p, pos2(cx, cy + radius + 21.0), Align2::CENTER_CENTER, &vt, 9.0, t.text_dim);
            resp.on_hover_text(format!("{}{}: {text}", if info.module.is_empty() { String::new() } else { format!("{}/", info.module) }, info.name));
        }
    }

    /// Device browser for one rack section. Clicking an entry adds it to the selected track.
    pub(super) fn browser_ui(&mut self, ctx: &egui::Context) {
        let t = th();
        let section = self.view.browser_section;
        let mut open = true;
        let mut chosen: Option<Result<Kind, crate::clap_host::PluginDesc>> = None;
        let title = match section {
            Section::NoteFx => "ADD NOTE FX",
            Section::Instrument => "ADD INSTRUMENT",
            Section::Effect => "ADD EFFECT",
        };
        egui::Window::new(title)
            .id(Id::new("device_browser"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([360.0, 420.0])
            .anchor(egui::Align2::LEFT_BOTTOM, [STRIP_W + 30.0, -(super::RACK_H + 10.0)])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search");
                    ui.text_edit_singleline(&mut self.view.browser_filter).request_focus();
                });
                ui.separator();
                let filter = self.view.browser_filter.to_lowercase();
                let matches = |name: &str, vendor: &str| filter.is_empty() || name.to_lowercase().contains(&filter) || vendor.to_lowercase().contains(&filter);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let mut any = false;
                    if section != Section::Instrument {
                        let builtins: Vec<Kind> = Kind::ALL
                            .into_iter()
                            .filter(|k| k.section() == section && matches(k.name(), "RasDaw"))
                            .collect();
                        if !builtins.is_empty() {
                            any = true;
                            ui.colored_label(t.device_bars[3], "RASDAW DEVICES");
                            for k in builtins {
                                if ui.selectable_label(false, k.name()).on_hover_text(k.description()).clicked() {
                                    chosen = Some(Ok(k));
                                }
                            }
                            ui.add_space(6.0);
                        }
                    }
                    let plugins: Vec<_> = self
                        .registry
                        .plugins
                        .iter()
                        .filter(|d| match section {
                            Section::NoteFx => d.is_note_effect(),
                            Section::Instrument => d.is_instrument(),
                            Section::Effect => !d.is_instrument() && !d.is_note_effect(),
                        })
                        .filter(|d| matches(&d.name, &d.vendor))
                        .collect();
                    if !plugins.is_empty() {
                        any = true;
                        ui.colored_label(t.device_bars[if section == Section::Instrument { 0 } else { 1 }], "CLAP PLUGINS");
                        for d in plugins {
                            let label = format!("{}   ·  {} {}", d.name, d.vendor, d.version);
                            if ui.selectable_label(false, label).on_hover_text(d.library.path.display().to_string()).clicked() {
                                chosen = Some(Err(d.clone()));
                            }
                        }
                    }
                    if !any {
                        ui.label(match section {
                            Section::NoteFx => "Nothing found.",
                            Section::Instrument => "No instrument plugins installed.",
                            Section::Effect => "Nothing found.",
                        });
                    }
                    ui.separator();
                    ui.collapsing("Search paths", |ui| {
                        for dir in crate::clap_host::Registry::search_dirs() {
                            ui.label(dir.display().to_string());
                        }
                    });
                    if !self.registry.errors.is_empty() {
                        ui.collapsing(format!("{} failed to load", self.registry.errors.len()), |ui| {
                            for (path, e) in &self.registry.errors {
                                ui.label(format!("{}: {e}", path.display()));
                            }
                        });
                    }
                });
            });
        if let Some(choice) = chosen {
            let owner = self.rack_owner();
            match choice {
                Ok(kind) => self.add_builtin(owner, kind),
                Err(desc) => self.add_device(owner, &desc, section),
            }
            open = false;
        }
        if !open {
            self.view.browser_open = false;
            self.view.browser_filter.clear();
        }
    }
}

/// Stand-in uid of a device whose plugin is not installed.
pub(super) fn missing_uid(spec: &DeviceSpec, di: usize) -> u64 {
    Id::new(("missing", &spec.plugin_id, di)).value()
}

/// Small clickable glyph inside a title bar.
fn glyph(ui: &mut egui::Ui, rect: Rect, id: impl std::hash::Hash, text: &str, lit: bool) -> egui::Response {
    let t = th();
    let resp = ui.interact(rect, Id::new(("glyph", id)), Sense::click());
    if resp.hovered() || lit {
        ui.painter().rect_filled(rect.shrink(2.0), widgets::r(2.0), egui::Color32::from_black_alpha(if lit { 90 } else { 50 }));
    }
    if text.is_empty() {
        // Window icon (two overlapping frames); the panel font has no such glyph.
        let c = rect.center();
        let st = Stroke::new(1.5_f32, t.bar_text);
        let back = Rect::from_center_size(c + vec2(2.0, -2.0), vec2(10.0, 8.0));
        let front = Rect::from_center_size(c + vec2(-2.0, 2.0), vec2(10.0, 8.0));
        ui.painter().rect_stroke(back, widgets::r(1.0), st, egui::StrokeKind::Middle);
        ui.painter().rect_filled(front, widgets::r(1.0), egui::Color32::from_black_alpha(0));
        ui.painter().rect_stroke(front, widgets::r(1.0), st, egui::StrokeKind::Middle);
    } else {
        caption(ui.painter(), rect.center(), Align2::CENTER_CENTER, text, 14.0, t.bar_text);
    }
    resp
}

/// Dashed "+" card for an empty slot or the end of the effect chain.
fn add_card(ui: &mut egui::Ui, rect: Rect, id: impl std::hash::Hash, label: &str) -> bool {
    let t = th();
    let resp = ui.interact(rect, Id::new(id), Sense::click());
    let p = ui.painter();
    let col = if resp.hovered() { t.accent } else { t.text_faint };
    p.rect_filled(rect, widgets::r(4.0), if resp.hovered() { t.bg_card } else { t.bg_panel });
    p.rect_stroke(rect, widgets::r(4.0), Stroke::new(1.0_f32, with_alpha(col, 0.6)), egui::StrokeKind::Inside);
    caption(p, rect.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, "+", 30.0, col);
    caption(p, rect.center() + vec2(0.0, 20.0), Align2::CENTER_CENTER, label, 10.0, col);
    resp.clicked()
}

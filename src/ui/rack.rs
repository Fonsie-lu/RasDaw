//! Device rack (bottom): the selected track's channel strip, then its CLAP devices left to
//! right in signal order, then the "+" card that opens the plugin browser.

use super::widgets::{self, button, caption, fader, knob, meter, section_bar, with_alpha};
use super::App;
use crate::clap_host::PluginInstance;
use crate::engine::volume_gain;
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Id, Rect, Sense, Stroke};
use std::sync::Arc;

const STRIP_W: f32 = 168.0;
const CARD_W: f32 = 304.0;
const PLUS_W: f32 = 110.0;
const COLS: usize = 4;
/// Height a knob cell needs: knob, name and value lines.
const CELL_H: f32 = 64.0;

enum RackAction {
    Bypass(usize),
    Editor(usize),
    Move(usize, usize),
    Remove(usize),
    Page(u64, usize),
}

impl App {
    pub(super) fn rack_ui(&mut self, ui: &mut egui::Ui) {
        let t = th();
        let full = ui.max_rect();
        let p = ui.painter().clone();
        p.rect_filled(full, widgets::r(0.0), t.bg_dark);
        p.line_segment([full.left_top(), full.right_top()], Stroke::new(1.0_f32, t.bevel));
        if self.project.tracks.is_empty() {
            caption(&p, full.center(), Align2::CENTER_CENTER, "No tracks. Add one with + TRACK.", 13.0, t.text_dim);
            return;
        }
        let ti = self.view.selected_track.min(self.project.tracks.len() - 1);
        self.view.selected_track = ti;

        let area = full.shrink2(vec2(8.0, 8.0));
        let strip = Rect::from_min_size(area.min, vec2(STRIP_W, area.height()));
        self.channel_strip(ui, strip, ti);

        let devices = Rect::from_min_max(pos2(strip.right() + 10.0, area.top()), area.max);
        let mut actions = Vec::new();
        ui.scope_builder(egui::UiBuilder::new().max_rect(devices), |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("rack_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let h = devices.height() - 14.0;
                        let track = &self.project.tracks[ti];
                        let chain = self.rack.get(&track.id).cloned().unwrap_or_default();
                        for (di, (spec, inst)) in track.devices.iter().zip(&chain).enumerate() {
                            let (rect, _) = ui.allocate_exact_size(vec2(CARD_W, h), Sense::hover());
                            let n = track.devices.len();
                            self.device_card(ui, rect, di, n, spec, inst.as_ref(), &mut actions);
                            let (gap, _) = ui.allocate_exact_size(vec2(18.0, h), Sense::hover());
                            caption(ui.painter(), gap.center(), Align2::CENTER_CENTER, "›", 18.0, t.text_faint);
                        }
                        let (rect, _) = ui.allocate_exact_size(vec2(PLUS_W, h), Sense::hover());
                        let resp = ui.interact(rect, Id::new("add_device"), Sense::click());
                        let pp = ui.painter();
                        let col = if resp.hovered() { t.accent } else { t.text_faint };
                        pp.rect_filled(rect, widgets::r(4.0), if resp.hovered() { t.bg_card } else { t.bg_panel });
                        pp.rect_stroke(rect, widgets::r(4.0), Stroke::new(1.0_f32, with_alpha(col, 0.6)), egui::StrokeKind::Inside);
                        caption(pp, rect.center() - vec2(0.0, 10.0), Align2::CENTER_CENTER, "+", 34.0, col);
                        caption(pp, rect.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, "ADD DEVICE", 11.0, col);
                        if resp.clicked() {
                            self.view.browser_open = !self.view.browser_open;
                        }
                    });
                });
        });

        // Claim the whole panel, or egui shrinks the resizable panel to the used area.
        ui.allocate_rect(full, Sense::hover());

        for a in actions {
            let track_name = self.project.tracks[ti].name.clone();
            let id = self.project.tracks[ti].id;
            match a {
                RackAction::Bypass(i) => {
                    let d = &mut self.project.tracks[ti].devices[i];
                    d.bypass = !d.bypass;
                    self.dirty = true;
                }
                RackAction::Editor(i) => {
                    if let Some(Some(inst)) = self.rack.get(&id).and_then(|c| c.get(i)).cloned() {
                        self.toggle_editor(&inst, &track_name);
                    }
                }
                RackAction::Move(from, to) => self.move_device(ti, from, to),
                RackAction::Remove(i) => self.remove_device(ti, i),
                RackAction::Page(uid, page) => {
                    self.view.param_page.insert(uid, page);
                }
            }
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

    #[allow(clippy::too_many_arguments)]
    fn device_card(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        di: usize,
        count: usize,
        spec: &crate::model::DeviceSpec,
        inst: Option<&Arc<PluginInstance>>,
        actions: &mut Vec<RackAction>,
    ) {
        let t = th();
        let p = ui.painter().clone();
        p.rect_filled(rect.translate(vec2(0.0, 3.0)), widgets::r(4.0), egui::Color32::from_black_alpha(120));
        p.rect_filled(rect, widgets::r(4.0), t.bg_panel);
        p.line_segment([rect.left_top() + vec2(4.0, 0.0), rect.right_top() - vec2(4.0, 0.0)], Stroke::new(1.0_f32, t.bevel));
        let bar = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, 20.0));
        let Some(inst) = inst else {
            section_bar(&p, bar, &format!("MISSING · {}", spec.name.to_uppercase()), t.record);
            caption(&p, rect.center(), Align2::CENTER_CENTER, &spec.plugin_id, 11.0, t.text_dim);
            let b = Rect::from_min_size(pos2(rect.right() - 30.0, bar.bottom() + 6.0), vec2(24.0, 18.0));
            if button(ui, b, ("rm", di, &spec.plugin_id), "✕", None).clicked() {
                actions.push(RackAction::Remove(di));
            }
            return;
        };
        let kind = if inst.desc.is_instrument() {
            0
        } else if inst.desc.is_effect() {
            1
        } else {
            2
        };
        let bar_col = if spec.bypass { t.text_faint } else { t.device_bars[kind] };
        section_bar(&p, bar, &widgets::fit_text(ui, &spec.name.to_uppercase(), 13.0, bar.width() - 10.0), bar_col);

        // Button row
        let by = bar.bottom() + 6.0;
        let uid = inst.uid;
        let mut x = rect.left() + 6.0;
        let b = Rect::from_min_size(pos2(x, by), vec2(36.0, 18.0));
        if button(ui, b, ("on", uid), "ON", (!spec.bypass).then_some(t.play))
            .on_hover_text("Bypass on/off")
            .clicked()
        {
            actions.push(RackAction::Bypass(di));
        }
        x += 40.0;
        if inst.has_gui() {
            let b = Rect::from_min_size(pos2(x, by), vec2(44.0, 18.0));
            let open = self.editor_open(uid);
            if button(ui, b, ("edit", uid), "EDIT", open.then_some(t.accent))
                .on_hover_text("Open the plugin's own editor")
                .clicked()
            {
                actions.push(RackAction::Editor(di));
            }
        }
        let mut rx = rect.right() - 6.0;
        let b = Rect::from_min_size(pos2(rx - 24.0, by), vec2(24.0, 18.0));
        if button(ui, b, ("rm", uid), "✕", None).on_hover_text("Remove device").clicked() {
            actions.push(RackAction::Remove(di));
        }
        rx -= 32.0;
        if di + 1 < count {
            let b = Rect::from_min_size(pos2(rx - 24.0, by), vec2(24.0, 18.0));
            if button(ui, b, ("right", uid), "▶", None).on_hover_text("Move right").clicked() {
                actions.push(RackAction::Move(di, di + 1));
            }
        }
        rx -= 28.0;
        if di > 0 {
            let b = Rect::from_min_size(pos2(rx - 24.0, by), vec2(24.0, 18.0));
            if button(ui, b, ("left", uid), "◀", None).on_hover_text("Move left").clicked() {
                actions.push(RackAction::Move(di, di - 1));
            }
        }

        // Parameter knobs, one page at a time; as many rows as the rack height allows.
        let params = inst.params();
        let grid_top = by + 26.0;
        let grid_bottom = rect.bottom() - 22.0;
        let rows = (((grid_bottom - grid_top) / CELL_H).floor() as usize).clamp(1, 4);
        let page_size = COLS * rows;
        let pages = params.len().div_ceil(page_size).max(1);
        let page = self.view.param_page.get(&uid).copied().unwrap_or(0).min(pages - 1);
        let cell_w = (rect.width() - 12.0) / COLS as f32;
        let cell_h = (grid_bottom - grid_top) / rows as f32;
        let accent = t.device_bars[kind];
        if params.is_empty() {
            caption(&p, pos2(rect.center().x, (grid_top + grid_bottom) * 0.5), Align2::CENTER_CENTER, "no parameters", 11.0, t.text_faint);
        }
        for (slot, info) in params.iter().skip(page * page_size).take(page_size).enumerate() {
            let col = slot % COLS;
            let row = slot / COLS;
            let cx = rect.left() + 6.0 + cell_w * (col as f32 + 0.5);
            let radius = 14.0;
            let cy = grid_top + cell_h * row as f32 + (cell_h - CELL_H) * 0.5 + radius + 4.0;
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

        // Footer: vendor and pages
        let fy = rect.bottom() - 11.0;
        caption(&p, pos2(rect.left() + 8.0, fy), Align2::LEFT_CENTER, &widgets::fit_text(ui, &inst.desc.vendor, 9.0, 140.0), 9.0, t.text_faint);
        if pages > 1 {
            let b = Rect::from_min_size(pos2(rect.right() - 92.0, fy - 8.0), vec2(20.0, 16.0));
            if button(ui, b, ("pg-", uid), "◀", None).clicked() && page > 0 {
                actions.push(RackAction::Page(uid, page - 1));
            }
            caption(&p, pos2(rect.right() - 50.0, fy), Align2::CENTER_CENTER, &format!("{}/{}", page + 1, pages), 10.0, t.text_dim);
            let b = Rect::from_min_size(pos2(rect.right() - 28.0, fy - 8.0), vec2(20.0, 16.0));
            if button(ui, b, ("pg+", uid), "▶", None).clicked() && page + 1 < pages {
                actions.push(RackAction::Page(uid, page + 1));
            }
        }
    }

    /// Plugin browser window. Clicking a plugin adds it to the selected track.
    pub(super) fn browser_ui(&mut self, ctx: &egui::Context) {
        let t = th();
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("ADD DEVICE")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([360.0, 420.0])
            .anchor(egui::Align2::LEFT_BOTTOM, [STRIP_W + 30.0, -270.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search");
                    ui.text_edit_singleline(&mut self.view.browser_filter).request_focus();
                });
                ui.separator();
                let filter = self.view.browser_filter.to_lowercase();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    type Filter = fn(&crate::clap_host::PluginDesc) -> bool;
                    let groups: [(&str, Filter, usize); 3] = [
                        ("INSTRUMENTS", |d| d.is_instrument(), 0),
                        ("EFFECTS", |d| !d.is_instrument() && d.is_effect(), 1),
                        ("OTHER", |d| !d.is_instrument() && !d.is_effect(), 2),
                    ];
                    for (title, pred, kind) in groups {
                        let list: Vec<_> = self
                            .registry
                            .plugins
                            .iter()
                            .filter(|d| pred(d))
                            .filter(|d| filter.is_empty() || d.name.to_lowercase().contains(&filter) || d.vendor.to_lowercase().contains(&filter))
                            .collect();
                        if list.is_empty() {
                            continue;
                        }
                        ui.colored_label(t.device_bars[kind], title);
                        for d in list {
                            let label = format!("{}   ·  {} {}", d.name, d.vendor, d.version);
                            if ui.selectable_label(false, label).on_hover_text(d.library.path.display().to_string()).clicked() {
                                chosen = Some(d.clone());
                            }
                        }
                        ui.add_space(6.0);
                    }
                    if self.registry.plugins.is_empty() {
                        ui.label("No CLAP plugins found.");
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
        if let Some(desc) = chosen {
            let ti = self.view.selected_track;
            self.add_device(ti, &desc);
            open = false;
        }
        if !open {
            self.view.browser_open = false;
            self.view.browser_filter.clear();
        }
    }
}

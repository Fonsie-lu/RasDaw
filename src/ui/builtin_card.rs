//! Rack cards of the built-in devices: all parameters at once (no pages, no editor window),
//! plus an EQ response graph with draggable band handles, or a gain reduction meter.
//! Note FX use dropdowns for choices with 3-15 entries and flip switches for on/off; all
//! other parameters are knobs.

use super::rack::{RackAction, BODY_TOP, CELL_H};
use super::widgets::{self, caption, knob, with_alpha};
use super::App;
use crate::devices::{eq, BuiltinDevice, Kind, Unit};
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Id, Pos2, Rect, Sense, Shape, Stroke};
use std::sync::Arc;

const CELL_W: f32 = 70.0;
const EQ_GRAPH_W: f32 = 230.0;
const GR_W: f32 = 26.0;
/// Space below the knob grid.
const BOTTOM: f32 = 6.0;

fn layout(kind: Kind, h: f32) -> (usize, usize) {
    let n = kind.params().len();
    let rows = kind
        .rows()
        .unwrap_or_else(|| (((h - BODY_TOP - BOTTOM) / CELL_H).floor() as usize).clamp(1, 4))
        .min(n);
    (rows, n.div_ceil(rows))
}

fn extra_width(kind: Kind) -> f32 {
    match kind {
        Kind::Eq5 => EQ_GRAPH_W + 8.0,
        k if k.has_gr_meter() => GR_W + 6.0,
        _ => 0.0,
    }
}

/// Card width for a built-in device at rack height `h`.
pub fn width(kind: Kind, h: f32) -> f32 {
    let (_, cols) = layout(kind, h);
    (12.0 + cols as f32 * CELL_W + extra_width(kind)).max(200.0)
}

impl App {
    pub(super) fn builtin_card(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        di: usize,
        spec: &crate::model::DeviceSpec,
        dev: &Arc<BuiltinDevice>,
        actions: &mut Vec<RackAction>,
    ) {
        let t = th();
        let accent = t.device_bars[3];
        let body = self.device_frame(ui, rect, di, dev.uid, spec, accent, false, 0.0, actions);
        let p = ui.painter().clone();
        let grid_top = body.top();
        let grid_bottom = rect.bottom() - BOTTOM;
        let (rows, _) = layout(dev.kind, rect.height());
        let cell_h = (grid_bottom - grid_top) / rows as f32;
        let mut x0 = rect.left() + 6.0;

        if dev.kind == Kind::Eq5 {
            let graph = Rect::from_min_max(pos2(x0, grid_top), pos2(x0 + EQ_GRAPH_W, grid_bottom));
            eq_graph(ui, graph, dev);
            x0 = graph.right() + 8.0;
        }

        let values = dev.values();
        for (i, def) in dev.params().iter().enumerate() {
            let active = dev.kind.param_active(i, &values);
            let knob_accent = if active { accent } else { t.text_faint };
            let (col, row) = (i / rows, i % rows);
            let cx = x0 + CELL_W * (col as f32 + 0.5);
            let radius = 14.0;
            let cy = grid_top + cell_h * row as f32 + (cell_h - CELL_H).max(0.0) * 0.5 + radius + 4.0;
            let value = dev.value(i);
            let name_color = if active { t.text } else { t.text_faint };
            // Note FX: finite choices get a dropdown (3-15 entries) or a flip switch (2).
            let choices = match def.unit {
                Unit::Choice(labels) if dev.kind.is_note() => Some(labels),
                _ => None,
            };
            if let Some(labels) = choices.filter(|l| (3..=15).contains(&l.len())) {
                let mut idx = value.round() as usize;
                let r = Rect::from_center_size(pos2(cx, cy), vec2(CELL_W - 6.0, 22.0));
                // A child Ui: `ui.put` would move the rack's horizontal layout cursor.
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
                egui::ComboBox::from_id_salt(("builtin_choice", dev.uid, i))
                    .width(CELL_W - 6.0)
                    .selected_text(labels.get(idx).copied().unwrap_or("?"))
                    .show_ui(&mut child, |ui| {
                        for (k, label) in labels.iter().enumerate() {
                            ui.selectable_value(&mut idx, k, *label);
                        }
                    });
                if idx as f32 != value.round() {
                    dev.set_value(i, idx as f32);
                }
                caption(&p, pos2(cx, cy + radius + 9.0), Align2::CENTER_CENTER, &widgets::fit_text(ui, def.name, 9.5, CELL_W - 4.0), 9.5, name_color);
                continue;
            }
            if choices.is_some_and(|l| l.len() == 2) {
                let on = value >= 0.5;
                let r = Rect::from_center_size(pos2(cx, cy), vec2(38.0, 20.0));
                if widgets::flip_switch(ui, r, ("builtin_switch", dev.uid, i), on, knob_accent).clicked() {
                    dev.set_value(i, if on { 0.0 } else { 1.0 });
                }
                caption(&p, pos2(cx, cy + radius + 9.0), Align2::CENTER_CENTER, &widgets::fit_text(ui, def.name, 9.5, CELL_W - 4.0), 9.5, name_color);
                if cell_h >= 56.0 {
                    caption(&p, pos2(cx, cy + radius + 21.0), Align2::CENTER_CENTER, &def.format(value), 9.0, t.text_dim);
                }
                continue;
            }
            let (resp, new) = knob(ui, pos2(cx, cy), radius, ("builtin", dev.uid, i), def.to_norm(value), def.to_norm(def.default), knob_accent);
            let shown = match new {
                Some(n) => {
                    let v = def.from_norm(n);
                    dev.set_value(i, v);
                    v
                }
                None => value,
            };
            let text = def.format(shown);
            caption(&p, pos2(cx, cy + radius + 9.0), Align2::CENTER_CENTER, &widgets::fit_text(ui, def.name, 9.5, CELL_W - 4.0), 9.5, name_color);
            if cell_h >= 56.0 {
                caption(&p, pos2(cx, cy + radius + 21.0), Align2::CENTER_CENTER, &widgets::fit_text(ui, &text, 9.0, CELL_W - 4.0), 9.0, t.text_dim);
            }
            resp.on_hover_text(format!("{}: {text}", def.name));
        }

        if dev.kind.has_gr_meter() {
            let m = Rect::from_min_max(pos2(rect.right() - 6.0 - GR_W, grid_top), pos2(rect.right() - 6.0, grid_bottom - 12.0));
            gr_meter(&p, m, dev.gain_reduction_db());
        }

    }
}

/// Gain reduction bar, growing down from the top (0..24 dB).
fn gr_meter(p: &egui::Painter, rect: Rect, gr_db: f32) {
    let t = th();
    let bar = Rect::from_min_max(rect.min + vec2(7.0, 0.0), pos2(rect.right() - 7.0, rect.bottom()));
    p.rect_filled(bar, widgets::r(1.0), t.slot);
    let f = (gr_db / 24.0).clamp(0.0, 1.0);
    if f > 0.0 {
        p.rect_filled(Rect::from_min_max(bar.min, pos2(bar.right(), bar.top() + f * bar.height())), widgets::r(1.0), t.mute);
    }
    for db in [6.0, 12.0, 18.0] {
        let y = bar.top() + db / 24.0 * bar.height();
        p.line_segment([pos2(bar.left() - 3.0, y), pos2(bar.left(), y)], Stroke::new(1.0_f32, t.text_faint));
    }
    caption(p, pos2(rect.center().x, rect.bottom() + 6.0), Align2::CENTER_CENTER, &format!("{gr_db:.1}"), 8.5, t.text_dim);
    caption(p, pos2(rect.center().x, rect.top() - 6.0), Align2::CENTER_CENTER, "GR", 8.5, t.text_faint);
}

const F_MIN: f32 = 10.0;
const F_MAX: f32 = 22_000.0;
const DB_RANGE: f32 = 24.0;

/// EQ response: fixed cuts and the mono-bass region are shaded; the bands have handles
/// (drag: frequency and gain).
fn eq_graph(ui: &mut egui::Ui, rect: Rect, dev: &BuiltinDevice) {
    let t = th();
    let p = ui.painter_at(rect);
    let sr = dev.sample_rate;
    let x_of = |f: f32| rect.left() + (f / F_MIN).ln() / (F_MAX / F_MIN).ln() * rect.width();
    let f_of = |x: f32| F_MIN * (F_MAX / F_MIN).powf(((x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    let y_of = |db: f32| rect.center().y - db.clamp(-DB_RANGE, DB_RANGE) / DB_RANGE * rect.height() * 0.5;
    let db_of = |y: f32| (rect.center().y - y) / (rect.height() * 0.5) * DB_RANGE;

    p.rect_filled(rect, widgets::r(3.0), t.bg_dark);
    // Fixed processing regions
    let cut = with_alpha(t.record, 0.10);
    p.rect_filled(Rect::from_min_max(rect.min, pos2(x_of(eq::LOW_CUT_HZ), rect.bottom())), widgets::r(0.0), cut);
    p.rect_filled(Rect::from_min_max(pos2(x_of(eq::HIGH_CUT_HZ), rect.top()), rect.max), widgets::r(0.0), cut);
    let mono = Rect::from_min_max(pos2(x_of(eq::LOW_CUT_HZ), rect.top()), pos2(x_of(eq::MONO_BELOW_HZ), rect.bottom()));
    p.rect_filled(mono, widgets::r(0.0), with_alpha(t.solo, 0.08));
    caption(&p, pos2(mono.center().x, rect.top() + 7.0), Align2::CENTER_CENTER, "MONO", 8.0, with_alpha(t.solo, 0.8));
    // Grid
    for f in [100.0, 1000.0, 10_000.0] {
        let x = x_of(f);
        p.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0_f32, t.grid_bar));
        let label = if f >= 1000.0 { format!("{}k", f / 1000.0) } else { format!("{f}") };
        caption(&p, pos2(x + 2.0, rect.bottom() - 2.0), Align2::LEFT_BOTTOM, &label, 8.0, t.text_faint);
    }
    for db in [-12.0, 12.0] {
        p.line_segment([pos2(rect.left(), y_of(db)), pos2(rect.right(), y_of(db))], Stroke::new(1.0_f32, t.grid_beat));
    }
    p.line_segment([pos2(rect.left(), y_of(0.0)), pos2(rect.right(), y_of(0.0))], Stroke::new(1.0_f32, t.grid_bar));

    // Response curve
    let values = dev.values();
    let n = 160;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = rect.left() + rect.width() * i as f32 / n as f32;
            pos2(x, y_of(eq::response_db(&values, f_of(x), sr)))
        })
        .collect();
    let accent = t.device_bars[3];
    let zero = y_of(0.0);
    for w in pts.windows(2) {
        let quad = vec![w[0], w[1], pos2(w[1].x, zero), pos2(w[0].x, zero)];
        p.add(Shape::convex_polygon(quad, with_alpha(accent, 0.15), Stroke::NONE));
    }
    p.add(Shape::line(pts, Stroke::new(2.0_f32, accent)));

    // Band handles: (freq index, gain index)
    for band in 0..5 {
        let (fi, gi) = (band * 3, band * 3 + 1);
        let (fdef, gdef) = (dev.params()[fi], dev.params()[gi]);
        let c = pos2(x_of(values[fi]), y_of(values[gi]));
        let id = Id::new(("eq_band", dev.uid, band));
        let resp = ui.interact(Rect::from_center_size(c, vec2(16.0, 16.0)), id, Sense::click_and_drag());
        if resp.drag_started() {
            ui.data_mut(|d| d.insert_temp(id, (values[fi], values[gi])));
        }
        if resp.dragged() {
            if let (Some((f0, g0)), Some(start)) = (ui.data(|d| d.get_temp::<(f32, f32)>(id)), ui.input(|i| i.pointer.press_origin())) {
                let cur = resp.interact_pointer_pos().unwrap_or(start);
                let f = f_of(x_of(f0) + cur.x - start.x);
                let g = db_of(y_of(g0) + cur.y - start.y);
                dev.set_value(fi, fdef.clamp(f));
                dev.set_value(gi, gdef.clamp(g));
            }
        }
        if resp.double_clicked() {
            dev.set_value(gi, gdef.default);
        }
        let active = resp.hovered() || resp.dragged();
        p.circle_filled(c, if active { 6.5 } else { 5.0 }, t.tracks[band]);
        p.circle_stroke(c, if active { 6.5 } else { 5.0 }, Stroke::new(1.0_f32, t.bar_text));
        caption(&p, c, Align2::CENTER_CENTER, &format!("{}", band + 1), 7.5, t.bar_text);
        resp.on_hover_text(format!("Band {}: {} · {}", band + 1, fdef.format(values[fi]), gdef.format(values[gi])));
    }
}

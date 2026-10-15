//! Track headers and the arranger timeline (top right). Double-click a lane to edit its notes.

use super::widgets::{self, button, caption, meter, panel, section_bar, with_alpha};
use super::{App, EditTarget, BAR_H, HEADER_H, ROW_H, TRACK_HEAD_W};
use crate::engine::Command;
use crate::model::{Clip, PlayMode, BEATS_PER_BAR};
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Color32, Id, Painter, Rect, Sense, Stroke};

/// Draws a clip's notes squeezed into `rect`, showing the beat range `[b0, b1)`.
pub fn mini_notes(p: &Painter, rect: Rect, clip: &Clip, b0: f64, b1: f64, color: Color32) {
    let Some((lo, hi)) = clip.key_range() else {
        return;
    };
    if b1 <= b0 {
        return;
    }
    let (lo, hi) = (lo.saturating_sub(2) as f32, hi.saturating_add(2) as f32);
    let range = (hi - lo).max(8.0);
    let row = (rect.height() / range).clamp(1.5, 6.0);
    let scale = rect.width() as f64 / (b1 - b0);
    for n in &clip.notes {
        if n.end() <= b0 || n.start >= b1 {
            continue;
        }
        let x0 = rect.left() + ((n.start.max(b0) - b0) * scale) as f32;
        let x1 = rect.left() + ((n.end().min(b1) - b0) * scale) as f32;
        let y = rect.bottom() - (n.key as f32 - lo) / range * rect.height();
        let r = Rect::from_min_max(pos2(x0, y - row * 0.5), pos2((x1 - 1.0).max(x0 + 1.0), y + row * 0.5));
        p.rect_filled(r.intersect(rect), widgets::r(0.0), color);
    }
}

impl App {
    pub(super) fn arranger_ui(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let t = th();
        let p = ui.painter_at(rect);
        panel(&p, rect);
        let head_x1 = rect.left() + TRACK_HEAD_W;
        section_bar(
            &p,
            Rect::from_min_max(rect.min + vec2(4.0, 4.0), pos2(head_x1 - 2.0, rect.top() + BAR_H)),
            "TRACKS",
            t.tracks[1],
        );
        section_bar(
            &p,
            Rect::from_min_max(pos2(head_x1 + 2.0, rect.top() + 4.0), pos2(rect.right() - 4.0, rect.top() + BAR_H)),
            "ARRANGER",
            t.tracks[2],
        );
        let header_top = rect.top() + BAR_H + 4.0;
        let tl = Rect::from_min_max(pos2(head_x1 + 4.0, header_top), pos2(rect.right() - 4.0, rect.bottom()));
        let ruler = Rect::from_min_max(tl.min, pos2(tl.right(), header_top + HEADER_H));
        let rows = Rect::from_min_max(pos2(rect.left(), ruler.bottom()), rect.max);
        let lanes = Rect::from_min_max(pos2(tl.left(), rows.top()), pos2(tl.right(), rows.bottom()));

        // "+ TRACK" in the header row of the track column
        let b = Rect::from_min_size(pos2(rect.left() + 8.0, header_top + 4.0), vec2(72.0, HEADER_H - 10.0));
        if button(ui, b, "add_track", "+ TRACK", None).clicked() {
            self.add_track();
        }
        let b = Rect::from_min_size(pos2(rect.left() + 86.0, header_top + 4.0), vec2(74.0, HEADER_H - 10.0));
        if button(ui, b, "stop_scenes", "■ SCENES", None)
            .on_hover_text("Stop every scene")
            .clicked()
        {
            for tr in &mut self.project.tracks {
                tr.active_scene = None;
            }
            self.dirty = true;
        }

        self.scroll_rows(ui, rows);
        self.timeline_zoom_scroll(ui, tl);
        let zoom = self.view.tl_zoom as f64;
        let b0 = self.view.tl_scroll;
        let b1 = b0 + tl.width() as f64 / zoom;
        let x_of = |beat: f64| tl.left() + ((beat - b0) * zoom) as f32;
        let length = self.project.length_beats();
        let pos = self.engine.shared.position();
        let playing = self.engine.shared.playing();

        // Ruler
        let rp = ui.painter_at(ruler);
        rp.rect_filled(ruler, widgets::r(0.0), t.bg_dark);
        if self.project.loop_enabled {
            let lr = Rect::from_min_max(pos2(x_of(0.0), ruler.bottom() - 5.0), pos2(x_of(length), ruler.bottom()));
            rp.rect_filled(lr, widgets::r(1.0), with_alpha(t.mute, 0.7));
        }
        let bar_step = bar_label_step(zoom);
        let first_bar = (b0 / BEATS_PER_BAR).floor().max(0.0) as i64;
        let mut bar = first_bar;
        while (bar as f64) * BEATS_PER_BAR < b1 {
            let x = x_of(bar as f64 * BEATS_PER_BAR);
            if bar % bar_step == 0 {
                rp.line_segment([pos2(x, ruler.top() + 12.0), pos2(x, ruler.bottom())], Stroke::new(1.0_f32, t.text_faint));
                caption(&rp, pos2(x + 3.0, ruler.top() + 3.0), Align2::LEFT_TOP, &format!("{}", bar + 1), 10.0, t.text_dim);
            }
            bar += 1;
        }
        let rresp = ui.interact(ruler, Id::new("ruler"), Sense::click_and_drag());
        if rresp.clicked() || rresp.dragged() {
            if let Some(pp) = rresp.interact_pointer_pos() {
                let beat = (b0 + ((pp.x - tl.left()) as f64 / zoom)).max(0.0).round();
                self.engine.send(Command::Seek(beat));
            }
        }

        // Lanes background and grid
        let lp = ui.painter_at(lanes);
        lp.rect_filled(lanes, widgets::r(0.0), t.bg);
        let beat_lines = zoom >= 10.0;
        let mut beat = b0.floor().max(0.0);
        while beat < b1 {
            let is_bar = beat % BEATS_PER_BAR == 0.0;
            if is_bar || beat_lines {
                let x = x_of(beat);
                lp.line_segment(
                    [pos2(x, lanes.top()), pos2(x, lanes.bottom())],
                    Stroke::new(1.0_f32, if is_bar { t.grid_bar } else { t.grid_beat }),
                );
            }
            beat += 1.0;
        }
        if length < b1 {
            let r = Rect::from_min_max(pos2(x_of(length).max(lanes.left()), lanes.top()), lanes.max);
            lp.rect_filled(r, widgets::r(0.0), with_alpha(t.bg_dark, 0.7));
        }

        let hp = ui.painter_at(rows);
        let mut action: Option<(usize, LaneAction)> = None;
        for (i, track) in self.project.tracks.iter().enumerate() {
            let y = rows.top() + i as f32 * ROW_H - self.view.scroll_y;
            if y + ROW_H < rows.top() || y > rows.bottom() {
                continue;
            }
            let color = t.track(track.color);
            let selected = i == self.view.selected_track;
            let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), ROW_H));
            let head = Rect::from_min_max(row.min, pos2(head_x1, row.bottom()));
            let lane = Rect::from_min_max(pos2(tl.left(), y), pos2(tl.right(), row.bottom()));

            // Header
            hp.rect_filled(head.shrink2(vec2(4.0, 2.0)), widgets::r(3.0), if selected { t.bg_highlight } else { t.bg_card });
            hp.rect_filled(
                Rect::from_min_size(head.min + vec2(4.0, 2.0), vec2(5.0, ROW_H - 4.0)),
                widgets::r(2.0),
                color,
            );
            let hresp = ui.interact(head.intersect(rows), Id::new(("track_head", track.id)), Sense::click());
            let renaming = self.view.rename.as_ref().is_some_and(|(id, _)| *id == track.id);
            if !renaming {
                let name = widgets::fit_text(ui, &track.name, 13.0, TRACK_HEAD_W - 70.0);
                caption(&hp, head.min + vec2(16.0, 9.0), Align2::LEFT_TOP, &name, 13.0, t.text);
            }
            let mode = match track.play_mode() {
                PlayMode::Arrangement => "TIMELINE".to_owned(),
                PlayMode::Scene(s) => format!("SCENE {}", s + 1),
                PlayMode::Silent => "—".to_owned(),
            };
            caption(&hp, head.min + vec2(16.0, 27.0), Align2::LEFT_TOP, &mode, 9.5, t.text_dim);
            let mb = Rect::from_min_size(head.min + vec2(16.0, ROW_H - 22.0), vec2(22.0, 15.0));
            if button(ui, mb, ("mute", track.id), "M", track.mute.then_some(t.mute)).clicked() {
                action = Some((i, LaneAction::Mute));
            }
            let sb = mb.translate(vec2(26.0, 0.0));
            if button(ui, sb, ("solo", track.id), "S", track.solo.then_some(t.solo)).clicked() {
                action = Some((i, LaneAction::Solo));
            }
            let level = self.view.meters.get(i).copied().unwrap_or([0.0; 2]);
            meter(&hp, Rect::from_min_size(pos2(head_x1 - 20.0, y + 8.0), vec2(9.0, ROW_H - 16.0)), level);
            if hresp.double_clicked() {
                action = Some((i, LaneAction::Edit));
            } else if hresp.clicked() {
                action = Some((i, LaneAction::Select));
            }
            hresp.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    action = Some((i, LaneAction::Rename));
                    ui.close_menu();
                }
                if ui.button("Next colour").clicked() {
                    action = Some((i, LaneAction::Colour));
                    ui.close_menu();
                }
                if ui.button("Delete track").clicked() {
                    action = Some((i, LaneAction::Delete));
                    ui.close_menu();
                }
            });

            // Lane
            let lane_vis = lane.intersect(rows);
            if selected {
                lp.rect_filled(lane_vis, widgets::r(0.0), with_alpha(t.bg_highlight, 0.5));
            }
            lp.line_segment([pos2(lanes.left(), lane.bottom()), pos2(lanes.right(), lane.bottom())], Stroke::new(1.0_f32, t.border));
            let inner = lane.shrink2(vec2(0.0, 6.0));
            if track.has_arrangement() {
                // One region across the timeline, like a long clip.
                let region = Rect::from_min_max(pos2(x_of(0.0), inner.top()), pos2(x_of(length), inner.bottom()));
                lp.rect_filled(region, widgets::r(3.0), with_alpha(color, 0.16));
                lp.rect_filled(
                    Rect::from_min_size(region.min, vec2(region.width(), 3.0)),
                    widgets::r(1.0),
                    color,
                );
                mini_notes(&lp, inner, &track.arrangement, b0, b1, color);
            } else if let PlayMode::Scene(s) = track.play_mode() {
                // Ghost of the looping scene clip.
                let clip = track.scenes[s].as_ref().unwrap();
                let len = clip.length.max(0.25);
                let mut k = (b0 / len).floor().max(0.0);
                while k * len < b1 && k * len < length.max(b1) {
                    let r = Rect::from_min_max(pos2(x_of(k * len), inner.top()), pos2(x_of((k + 1.0) * len), inner.bottom()));
                    lp.line_segment([r.left_top(), r.left_bottom()], Stroke::new(1.0_f32, with_alpha(color, 0.25)));
                    mini_notes(&lp, r, clip, 0.0, len, with_alpha(color, 0.28));
                    k += 1.0;
                }
                let label = format!("▶ SCENE {} LOOP", s + 1);
                let pill = Rect::from_min_size(pos2(lanes.left() + 4.0, inner.top()), vec2(104.0, 15.0));
                lp.rect_filled(pill, widgets::r(3.0), with_alpha(t.bg_dark, 0.85));
                caption(&lp, pill.left_center() + vec2(5.0, 0.0), Align2::LEFT_CENTER, &label, 10.0, color);
            }
            let lresp = ui.interact(lane_vis, Id::new(("lane", track.id)), Sense::click());
            if lresp.double_clicked() {
                action = Some((i, LaneAction::Edit));
            } else if lresp.clicked() {
                action = Some((i, LaneAction::Select));
            }
            lresp.context_menu(|ui| {
                if ui.button("Edit notes").clicked() {
                    action = Some((i, LaneAction::Edit));
                    ui.close_menu();
                }
                if let Some(s) = track.active_scene.filter(|&s| track.scenes[s].is_some()) {
                    if ui.button(format!("Fill timeline with scene {}", s + 1)).clicked() {
                        action = Some((i, LaneAction::FillFromScene(s)));
                        ui.close_menu();
                    }
                }
                if track.has_arrangement() && ui.button("Clear timeline notes").clicked() {
                    action = Some((i, LaneAction::Clear));
                    ui.close_menu();
                }
            });
        }

        // Rename field
        if let Some((id, mut text)) = self.view.rename.take() {
            if let Some(i) = self.track_index(id) {
                let y = rows.top() + i as f32 * ROW_H - self.view.scroll_y;
                let r = Rect::from_min_size(pos2(rect.left() + 14.0, y + 6.0), vec2(TRACK_HEAD_W - 44.0, 20.0));
                let resp = ui.put(r, egui::TextEdit::singleline(&mut text));
                resp.request_focus();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if enter || resp.lost_focus() {
                    if !text.trim().is_empty() {
                        self.project.tracks[i].name = text.trim().to_owned();
                    }
                } else if !esc {
                    self.view.rename = Some((id, text));
                }
            }
        }

        // Playhead
        let x = x_of(pos);
        if x >= tl.left() && x <= tl.right() {
            let col = if playing { t.playhead } else { with_alpha(t.playhead, 0.5) };
            ui.painter_at(tl).line_segment([pos2(x, ruler.top()), pos2(x, tl.bottom())], Stroke::new(1.5_f32, col));
        }

        if let Some((i, a)) = action {
            self.view.selected_track = i;
            let track = &mut self.project.tracks[i];
            match a {
                LaneAction::Select => {}
                LaneAction::Mute => track.mute = !track.mute,
                LaneAction::Solo => track.solo = !track.solo,
                LaneAction::Rename => self.view.rename = Some((track.id, track.name.clone())),
                LaneAction::Colour => track.color = (track.color + 1) % t.tracks.len(),
                LaneAction::Edit => {
                    self.view.editor = Some(EditTarget::Arrangement(track.id));
                    self.view.pr.selected = None;
                    self.view.pr.scroll = 0.0;
                }
                LaneAction::Clear => track.arrangement.notes.clear(),
                LaneAction::FillFromScene(s) => {
                    let clip = track.scenes[s].clone().unwrap();
                    let mut k = 0.0;
                    track.arrangement.notes.clear();
                    while clip.length > 0.0 && k + clip.length <= length + 1e-9 {
                        for n in &clip.notes {
                            let mut n = n.clone();
                            n.start += k;
                            track.arrangement.notes.push(n);
                        }
                        k += clip.length;
                    }
                    track.arrangement.sort();
                }
                LaneAction::Delete => {
                    self.delete_track(i);
                }
            }
            self.dirty = true;
        }
    }

    /// Shift+wheel scrolls the timeline, Ctrl+wheel zooms around the pointer.
    fn timeline_zoom_scroll(&mut self, ui: &egui::Ui, tl: Rect) {
        if !ui.rect_contains_pointer(tl) {
            return;
        }
        let (delta, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        let v = &mut self.view;
        if zoom != 1.0 {
            if let Some(pp) = pointer {
                let at = v.tl_scroll + ((pp.x - tl.left()) / v.tl_zoom) as f64;
                v.tl_zoom = (v.tl_zoom * zoom).clamp(2.0, 400.0);
                v.tl_scroll = at - ((pp.x - tl.left()) / v.tl_zoom) as f64;
            }
        }
        if delta.x != 0.0 {
            v.tl_scroll -= (delta.x / v.tl_zoom) as f64;
        }
        v.tl_scroll = v.tl_scroll.max(0.0);
    }
}

enum LaneAction {
    Select,
    Mute,
    Solo,
    Rename,
    Colour,
    Edit,
    Clear,
    FillFromScene(usize),
    Delete,
}

/// Bar number spacing that keeps labels apart at the given zoom.
fn bar_label_step(zoom: f64) -> i64 {
    let bar_px = zoom * BEATS_PER_BAR;
    [1, 2, 4, 8, 16, 32, 64]
        .into_iter()
        .find(|&s| bar_px * s as f64 >= 36.0)
        .unwrap_or(64)
}

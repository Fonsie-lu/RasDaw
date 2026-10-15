//! Scene launcher (top left): 4 scene slots per track, rows aligned with the arranger.
//! A slot loops while its track has no notes on the timeline; otherwise it is muted.

use super::widgets::{self, button, caption, panel, section_bar, with_alpha};
use super::{App, BAR_H, HEADER_H, MASTER_H, ROW_H, SLOT_W};

/// Height of a slot's title bar.
const HEAD_H: f32 = 16.0;
use crate::model::{PlayMode, KEY_NAMES, SCENES, TIME_SIGNATURES};
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Id, Pos2, Rect, Sense, Stroke};

impl App {
    pub(super) fn launcher_ui(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let t = th();
        let p = ui.painter_at(rect);
        panel(&p, rect);
        section_bar(
            &p,
            Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, BAR_H - 4.0)),
            "SCENES",
            t.accent,
        );
        let header_top = rect.top() + BAR_H + 4.0;
        for s in 0..SCENES {
            let b = Rect::from_min_size(
                pos2(rect.left() + 6.0 + s as f32 * SLOT_W, header_top + 3.0),
                vec2(SLOT_W - 8.0, HEADER_H - 8.0),
            );
            if button(ui, b, ("scene", s), &format!("▶  SCENE {}", s + 1), None)
                .on_hover_text("Launch this scene on every track")
                .clicked()
            {
                self.launch_scene(s);
            }
        }

        let rows = Rect::from_min_max(pos2(rect.left(), header_top + HEADER_H), rect.max);
        self.scroll_rows(ui, rows);
        let p = ui.painter_at(rows);
        let arranger = self.engine.shared.playing();
        let now = ui.input(|i| i.time);

        let mut action: Option<(usize, usize, SlotAction)> = None;
        for (i, track) in self.project.tracks.iter().enumerate() {
            let y = rows.top() + i as f32 * ROW_H - self.view.scroll_y;
            if y + ROW_H < rows.top() || y > rows.bottom() {
                continue;
            }
            if i == self.view.selected_track && !self.view.master_selected {
                p.rect_filled(
                    Rect::from_min_size(pos2(rows.left(), y), vec2(rows.width(), ROW_H)),
                    widgets::r(0.0),
                    t.bg_highlight,
                );
            }
            p.line_segment(
                [pos2(rows.left(), y + ROW_H), pos2(rows.right(), y + ROW_H)],
                Stroke::new(1.0_f32, t.border),
            );
            let color = t.track(track.color);
            let arr_mute = track.has_arrangement();
            for s in 0..SCENES {
                let slot = Rect::from_min_size(
                    pos2(rows.left() + 6.0 + s as f32 * SLOT_W, y + 4.0),
                    vec2(SLOT_W - 8.0, ROW_H - 8.0),
                );
                let clip = track.scenes[s].as_ref();
                // Clips can be dragged onto the timeline to make a block.
                let sense = if clip.is_some() { Sense::click_and_drag() } else { Sense::click() };
                let resp = ui.interact(slot.intersect(rows), Id::new(("slot", track.id, s)), sense);
                if resp.drag_started() {
                    self.view.scene_drag = Some((track.id, s));
                }
                let active = track.active_scene == Some(s);
                let queued = track.queued_scene == Some(s);
                let sounding = active && track.play_mode(arranger) == PlayMode::Scene(s);
                let beat = self.engine.shared.scene_beat(i);
                let recording = self.view.recording.as_ref().is_some_and(|r| r.track == track.id && r.scene == s);
                // Blink: playing slots flash on every beat, queued ones blink fast.
                let pulse = if sounding {
                    let f = 1.0 - beat.rem_euclid(1.0) as f32;
                    f * f
                } else if queued {
                    if (now * 6.0) as i64 % 2 == 0 { 1.0 } else { 0.0 }
                } else {
                    0.0
                };
                let head = Rect::from_min_size(slot.min, vec2(slot.width(), HEAD_H));
                let rec_rect = Rect::from_center_size(head.right_center() - vec2(HEAD_H * 0.5, 0.0), vec2(HEAD_H, HEAD_H));
                match clip {
                    None => {
                        p.rect_filled(slot, widgets::r(3.0), t.bg_dark);
                        if resp.hovered() {
                            caption(&p, slot.center() + vec2(0.0, 4.0), Align2::CENTER_CENTER, "+", 16.0, t.text_faint);
                        }
                        // Bitwig-style stop square
                        let sq = Rect::from_center_size(head.left_center() + vec2(10.0, 0.0), vec2(6.0, 6.0));
                        p.rect_stroke(sq, widgets::r(0.0), Stroke::new(1.0_f32, t.text_faint), egui::StrokeKind::Inside);
                    }
                    Some(clip) => {
                        let alpha = if arr_mute && arranger { 0.25 } else if active || queued { 1.0 } else { 0.55 };
                        p.rect_filled(slot, widgets::r(3.0), with_alpha(color, 0.22 * alpha + 0.06));
                        let head_col = widgets::mix_colors(with_alpha(color, alpha), t.text, pulse * 0.45);
                        p.rect_filled(head, widgets::r(3.0), head_col);
                        let label = if recording && self.view.recording.as_ref().is_some_and(|r| r.grow) {
                            "REC".to_owned()
                        } else {
                            let bars = clip.length / clip.bar_beats();
                            format!("{} BAR{}", fmt_bars(bars), if bars > 1.0 { "S" } else { "" })
                        };
                        caption(&p, head.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, &label, 9.5, t.bar_text);
                        let sig = format!("{} {}/{}", KEY_NAMES[clip.root as usize % 12], clip.time_sig.0, clip.time_sig.1);
                        caption(&p, rec_rect.left_center() - vec2(3.0, 0.0), Align2::RIGHT_CENTER, &sig, 9.0, t.bar_text);
                        // Play triangle in the head
                        let c = head.left_center() + vec2(9.0, 0.0);
                        if active || queued {
                            play_icon(&p, c, if sounding || pulse > 0.5 { t.bar_text } else { with_alpha(t.bar_text, 0.5) });
                        }
                        let body = Rect::from_min_max(pos2(slot.left() + 4.0, head.bottom() + 3.0), slot.max - vec2(4.0, 6.0));
                        super::arranger::mini_notes(&p, body, clip, 0.0, clip.length, with_alpha(color, 0.35 + 0.65 * alpha));
                        if sounding && clip.length > 0.0 {
                            let f = (beat.rem_euclid(clip.length) / clip.length) as f32;
                            let bar = Rect::from_min_size(pos2(slot.left(), slot.bottom() - 3.0), vec2(slot.width() * f, 3.0));
                            p.rect_filled(bar, widgets::r(1.0), t.text);
                        }
                        if arr_mute && arranger && active {
                            caption(&p, body.center(), Align2::CENTER_CENTER, "MUTED · ARR", 10.0, t.mute);
                        }
                    }
                }
                // Record button on the right of the title bar
                let rresp = ui.interact(rec_rect.intersect(rows), Id::new(("rec", track.id, s)), Sense::click());
                let rec_pulse = 0.55 + 0.45 * ((now * 4.0).sin() as f32 * 0.5 + 0.5);
                let dot = if recording {
                    with_alpha(t.record, rec_pulse)
                } else if rresp.hovered() {
                    t.record
                } else if clip.is_some() {
                    with_alpha(t.bar_text, 0.55)
                } else {
                    t.text_faint
                };
                p.circle_filled(rec_rect.center(), 4.5, dot);
                if recording {
                    p.circle_stroke(rec_rect.center(), 6.5, Stroke::new(1.0_f32, t.record));
                    widgets::outline(&p, slot, with_alpha(t.record, rec_pulse), 2.0);
                }
                if rresp.on_hover_text(if recording { "Stop recording" } else { "Record MIDI into this scene" }).clicked() {
                    action = Some((i, s, SlotAction::Record));
                }
                if self.view.selection == Some(super::Selection::Scene { track: track.id, scene: s }) && clip.is_some() {
                    widgets::outline(&p, slot.expand(1.0), t.accent, 2.0);
                }
                if resp.hovered() {
                    widgets::outline(&p, slot, with_alpha(t.text, 0.4), 1.0);
                }
                if resp.double_clicked() {
                    action = Some((i, s, SlotAction::Edit));
                } else if resp.clicked() {
                    action = Some((i, s, SlotAction::Toggle));
                }
                resp.context_menu(|ui| {
                    if let Some(clip) = clip {
                        ui.menu_button("Length", |ui| {
                            for bars in [1.0, 2.0, 4.0, 8.0, 16.0] {
                                if ui.button(format!("{bars} bar(s)")).clicked() {
                                    action = Some((i, s, SlotAction::Length(bars * clip.bar_beats())));
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button("Root key", |ui| {
                            for (k, name) in KEY_NAMES.iter().enumerate() {
                                if ui.selectable_label(clip.root as usize == k, *name).clicked() {
                                    action = Some((i, s, SlotAction::Root(k as u8)));
                                    ui.close_menu();
                                }
                            }
                        });
                        ui.menu_button("Time signature", |ui| {
                            for ts in TIME_SIGNATURES {
                                if ui.selectable_label(clip.time_sig == ts, format!("{}/{}", ts.0, ts.1)).clicked() {
                                    action = Some((i, s, SlotAction::TimeSig(ts)));
                                    ui.close_menu();
                                }
                            }
                        });
                        if s + 1 < SCENES && ui.button("Duplicate to next slot").clicked() {
                            action = Some((i, s, SlotAction::Duplicate));
                            ui.close_menu();
                        }
                        if ui.button("Delete clip").clicked() {
                            action = Some((i, s, SlotAction::Delete));
                            ui.close_menu();
                        }
                    } else if ui.button("Create clip").clicked() {
                        action = Some((i, s, SlotAction::Edit));
                        ui.close_menu();
                    }
                });
            }
        }

        // Master row: no scene slots.
        let my = rows.top() + self.project.tracks.len() as f32 * ROW_H - self.view.scroll_y;
        let master_row = Rect::from_min_size(pos2(rows.left(), my), vec2(rows.width(), MASTER_H));
        if master_row.intersects(rows) {
            if self.view.master_selected {
                p.rect_filled(master_row, widgets::r(0.0), t.bg_highlight);
            }
            let strip = master_row.shrink2(vec2(6.0, 5.0));
            p.rect_filled(strip, widgets::r(3.0), t.bg_dark);
            caption(&p, strip.center(), Align2::CENTER_CENTER, "MASTER · NO SCENES", 9.5, t.text_faint);
            if ui.interact(master_row.intersect(rows), Id::new("master_launcher"), Sense::click()).clicked() {
                self.view.master_selected = true;
            }
        }

        if let Some((i, s, a)) = action {
            self.view.selected_track = i;
            self.view.master_selected = false;
            if matches!(a, SlotAction::Toggle | SlotAction::Edit) && self.project.tracks[i].scenes[s].is_some() {
                // Del deletes the selected scene clip.
                self.view.selection = Some(super::Selection::Scene { track: self.project.tracks[i].id, scene: s });
            }
            let track = &mut self.project.tracks[i];
            match a {
                SlotAction::Toggle => {
                    // Click a playing (or queued) clip to stop it; an empty slot stops the
                    // track, as in Bitwig. Scenes run without the arranger.
                    let stop = track.scenes[s].is_none() || track.active_scene == Some(s) || track.queued_scene == Some(s);
                    let id = track.id;
                    self.launch(id, (!stop).then_some(s));
                }
                SlotAction::Record => self.toggle_record(i, s),
                SlotAction::Edit => self.edit_scene(i, s),
                SlotAction::Length(len) => {
                    if let Some(c) = &mut track.scenes[s] {
                        c.length = len;
                    }
                }
                SlotAction::Root(k) => {
                    if let Some(c) = &mut track.scenes[s] {
                        c.root = k;
                    }
                }
                SlotAction::TimeSig(ts) => {
                    if let Some(c) = &mut track.scenes[s] {
                        // Keep the number of bars.
                        let bars = (c.length / c.bar_beats()).max(1.0).round();
                        c.time_sig = ts;
                        c.length = bars * c.bar_beats();
                    }
                }
                SlotAction::Duplicate => {
                    track.scenes[s + 1] = track.scenes[s].clone();
                }
                SlotAction::Delete => {
                    let id = track.id;
                    self.delete_scene(id, s);
                }
            }
            self.dirty = true;
        }
    }

    /// Mouse wheel over the track rows scrolls launcher and arranger together.
    pub(super) fn scroll_rows(&mut self, ui: &egui::Ui, rows: Rect) {
        let hovered = ui.rect_contains_pointer(rows);
        let (dy, shift, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.modifiers.shift, i.zoom_delta()));
        if hovered && dy != 0.0 && !shift && zoom == 1.0 {
            self.view.scroll_y -= dy;
        }
        let content = self.project.tracks.len() as f32 * ROW_H + MASTER_H + ROW_H * 0.5;
        self.view.scroll_y = self.view.scroll_y.clamp(0.0, (content - rows.height()).max(0.0));
    }
}

enum SlotAction {
    Toggle,
    Record,
    Edit,
    Length(f64),
    Root(u8),
    TimeSig((u8, u8)),
    Duplicate,
    Delete,
}

fn fmt_bars(bars: f64) -> String {
    if bars.fract() == 0.0 {
        format!("{}", bars as i64)
    } else {
        format!("{bars:.2}")
    }
}

fn play_icon(p: &egui::Painter, c: Pos2, color: egui::Color32) {
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-3.0, -4.0), c + vec2(4.0, 0.0), c + vec2(-3.0, 4.0)],
        color,
        Stroke::NONE,
    ));
}

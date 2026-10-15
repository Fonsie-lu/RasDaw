//! Scene launcher (top left): 4 scene slots per track, rows aligned with the arranger.
//! A slot loops while its track has no notes on the timeline; otherwise it is muted.

use super::widgets::{self, button, caption, panel, section_bar, with_alpha};
use super::{App, EditTarget, BAR_H, HEADER_H, ROW_H, SLOT_W};
use crate::model::{Clip, PlayMode, BEATS_PER_BAR, SCENES};
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
        let playing = self.engine.shared.playing();
        let pos = self.engine.shared.position();

        let mut action: Option<(usize, usize, SlotAction)> = None;
        for (i, track) in self.project.tracks.iter().enumerate() {
            let y = rows.top() + i as f32 * ROW_H - self.view.scroll_y;
            if y + ROW_H < rows.top() || y > rows.bottom() {
                continue;
            }
            if i == self.view.selected_track {
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
                let resp = ui.interact(slot.intersect(rows), Id::new(("slot", track.id, s)), Sense::click());
                let clip = track.scenes[s].as_ref();
                let active = track.active_scene == Some(s);
                let sounding = active && playing && track.play_mode() == PlayMode::Scene(s);
                match clip {
                    None => {
                        p.rect_filled(slot, widgets::r(3.0), t.bg_dark);
                        if resp.hovered() {
                            caption(&p, slot.center(), Align2::CENTER_CENTER, "+", 16.0, t.text_faint);
                        }
                        // Bitwig-style stop square
                        let sq = Rect::from_center_size(slot.left_center() + vec2(10.0, 0.0), vec2(6.0, 6.0));
                        p.rect_stroke(sq, widgets::r(0.0), Stroke::new(1.0_f32, t.text_faint), egui::StrokeKind::Inside);
                    }
                    Some(clip) => {
                        let alpha = if arr_mute { 0.25 } else if active { 1.0 } else { 0.55 };
                        p.rect_filled(slot, widgets::r(3.0), with_alpha(color, 0.22 * alpha + 0.06));
                        let head = Rect::from_min_size(slot.min, vec2(slot.width(), 14.0));
                        p.rect_filled(head, widgets::r(3.0), with_alpha(color, alpha));
                        let label = format!("{} BAR{}", fmt_bars(clip.length), if clip.length > BEATS_PER_BAR { "S" } else { "" });
                        caption(&p, head.left_center() + vec2(16.0, 0.0), Align2::LEFT_CENTER, &label, 9.5, t.bar_text);
                        // Play triangle / stop square in the head
                        let c = head.left_center() + vec2(8.0, 0.0);
                        if active {
                            play_icon(&p, c, if sounding { t.bar_text } else { with_alpha(t.bar_text, 0.6) });
                        }
                        let body = Rect::from_min_max(pos2(slot.left() + 4.0, head.bottom() + 3.0), slot.max - vec2(4.0, 6.0));
                        super::arranger::mini_notes(&p, body, clip, 0.0, clip.length, with_alpha(color, 0.35 + 0.65 * alpha));
                        if sounding && clip.length > 0.0 {
                            let f = (pos.rem_euclid(clip.length) / clip.length) as f32;
                            let bar = Rect::from_min_size(pos2(slot.left(), slot.bottom() - 3.0), vec2(slot.width() * f, 3.0));
                            p.rect_filled(bar, widgets::r(1.0), t.text);
                        }
                        if arr_mute && active {
                            caption(&p, body.center(), Align2::CENTER_CENTER, "MUTED · ARR", 10.0, t.mute);
                        }
                    }
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
                    if clip.is_some() {
                        for bars in [1.0, 2.0, 4.0, 8.0] {
                            if ui.button(format!("Length: {bars} bar(s)")).clicked() {
                                action = Some((i, s, SlotAction::Length(bars * BEATS_PER_BAR)));
                                ui.close_menu();
                            }
                        }
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

        if let Some((i, s, a)) = action {
            self.view.selected_track = i;
            let track = &mut self.project.tracks[i];
            match a {
                SlotAction::Toggle => {
                    track.active_scene = match (&track.scenes[s], track.active_scene) {
                        (Some(_), Some(cur)) if cur == s => None,
                        (Some(_), _) => Some(s),
                        // An empty slot stops the track, as in Bitwig.
                        (None, _) => None,
                    };
                    // Launching a clip starts the transport.
                    if track.active_scene.is_some() && !playing {
                        self.engine.send(crate::engine::Command::Play);
                    }
                }
                SlotAction::Edit => {
                    if track.scenes[s].is_none() {
                        track.scenes[s] = Some(Clip::new(BEATS_PER_BAR));
                        track.active_scene = Some(s);
                    }
                    self.view.editor = Some(EditTarget::Scene(track.id, s));
                    self.view.pr.selected = None;
                    self.view.pr.scroll = 0.0;
                }
                SlotAction::Length(len) => {
                    if let Some(c) = &mut track.scenes[s] {
                        c.length = len;
                    }
                }
                SlotAction::Duplicate => {
                    track.scenes[s + 1] = track.scenes[s].clone();
                }
                SlotAction::Delete => {
                    track.scenes[s] = None;
                    if track.active_scene == Some(s) {
                        track.active_scene = None;
                    }
                    if self.view.editor == Some(EditTarget::Scene(track.id, s)) {
                        self.view.editor = None;
                    }
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
        let content = (self.project.tracks.len() as f32 + 1.0) * ROW_H;
        self.view.scroll_y = self.view.scroll_y.clamp(0.0, (content - rows.height()).max(0.0));
    }
}

enum SlotAction {
    Toggle,
    Edit,
    Length(f64),
    Duplicate,
    Delete,
}

fn fmt_bars(beats: f64) -> String {
    let bars = beats / BEATS_PER_BAR;
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

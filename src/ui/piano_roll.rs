//! Piano roll for a track's timeline notes or one of its scene clips. ESC goes back.
//!
//! Mouse: click empty space adds a note, drag on empty space draws one, drag a note to move
//! it, drag its right edge to resize, right-click deletes. Del removes the selected note.
//! Wheel scrolls keys, Shift+wheel scrolls time, Ctrl+wheel zooms. The keyboard previews notes.

use super::widgets::{self, button, caption, panel, section_bar_left, with_alpha};
use super::{App, EditTarget, NoteDrag, NoteDragMode, BAR_H, HEADER_H};
use crate::engine::Command;
use crate::model::{Clip, Note, PlayMode, BEATS_PER_BAR};
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, CursorIcon, Id, Pos2, Rect, Sense, Stroke};

const KEY_W: f32 = 54.0;
const EDGE_PX: f32 = 7.0;

fn is_black(key: u8) -> bool {
    matches!(key % 12, 1 | 3 | 6 | 8 | 10)
}

fn key_name(key: u8) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES[key as usize % 12], key as i32 / 12 - 1)
}

/// Sorts the clip and returns the new index of the note that was at `index`.
fn sort_keep(clip: &mut Clip, index: usize) -> Option<usize> {
    let n = clip.notes.get(index)?.clone();
    clip.sort();
    clip.notes.iter().position(|x| *x == n)
}

impl App {
    pub(super) fn piano_roll_ui(&mut self, ui: &mut egui::Ui, rect: Rect, target: EditTarget) {
        let t = th();
        let ti = self.track_index(target.track_id()).unwrap();
        let track = &self.project.tracks[ti];
        let color = t.track(track.color);
        let track_name = track.name.clone();
        let track_id = track.id;
        let clip_len = match target {
            EditTarget::Arrangement(_) => self.project.length_beats(),
            EditTarget::Scene(_, s) => track.scenes[s].as_ref().map_or(BEATS_PER_BAR, |c| c.length),
        };
        // Where the playhead sits inside this clip, if it is the one playing.
        let pos = self.engine.shared.position();
        let playhead = match (target, track.play_mode()) {
            (EditTarget::Arrangement(_), PlayMode::Arrangement) => Some(pos),
            (EditTarget::Scene(_, s), PlayMode::Scene(a)) if s == a => Some(pos.rem_euclid(clip_len)),
            _ => None,
        };

        let p = ui.painter_at(rect);
        panel(&p, rect);
        let what = match target {
            EditTarget::Arrangement(_) => "TIMELINE".to_owned(),
            EditTarget::Scene(_, s) => format!("SCENE {}", s + 1),
        };
        let bar = Rect::from_min_max(rect.min + vec2(4.0, 4.0), pos2(rect.right() - 4.0, rect.top() + BAR_H));
        section_bar_left(&p, bar, &format!("PIANO ROLL · {} · {}", track_name.to_uppercase(), what), color);

        // Controls on the right of the bar (right to left)
        let mut x = bar.right() - 4.0;
        let by = bar.top() + 2.0;
        let bh = bar.height() - 4.0;
        let b = Rect::from_min_max(pos2(x - 70.0, by), pos2(x, by + bh));
        if button(ui, b, "pr_close", "ESC ✕", None).on_hover_text("Back to the tracks (Esc)").clicked() {
            self.view.editor = None;
            return;
        }
        x -= 80.0;
        for (label, snap) in [("1/32", 0.125), ("1/16", 0.25), ("1/8", 0.5), ("1/4", 1.0)] {
            let b = Rect::from_min_max(pos2(x - 40.0, by), pos2(x, by + bh));
            let lit = (self.view.pr.snap == snap).then_some(t.text);
            if button(ui, b, ("snap", label), label, lit).on_hover_text("Grid").clicked() {
                self.view.pr.snap = snap;
            }
            x -= 44.0;
        }
        if let EditTarget::Scene(_, s) = target {
            x -= 8.0;
            let b = Rect::from_min_max(pos2(x - 24.0, by), pos2(x, by + bh));
            if button(ui, b, "len+", "+", None).on_hover_text("Clip one bar longer").clicked() {
                if let Some(c) = &mut self.project.tracks[ti].scenes[s] {
                    c.length = (c.length + BEATS_PER_BAR).min(64.0 * BEATS_PER_BAR);
                    self.dirty = true;
                }
            }
            x -= 28.0;
            let b = Rect::from_min_max(pos2(x - 24.0, by), pos2(x, by + bh));
            if button(ui, b, "len-", "−", None).on_hover_text("Clip one bar shorter").clicked() {
                if let Some(c) = &mut self.project.tracks[ti].scenes[s] {
                    c.length = (c.length - BEATS_PER_BAR).max(BEATS_PER_BAR);
                    self.dirty = true;
                }
            }
            x -= 28.0;
            caption(&p, pos2(x, bar.center().y), Align2::RIGHT_CENTER, &format!("LENGTH {} BARS", clip_len / BEATS_PER_BAR), 11.0, t.bar_text);
        }

        let header_top = rect.top() + BAR_H + 4.0;
        let keys = Rect::from_min_max(pos2(rect.left() + 4.0, header_top + HEADER_H), pos2(rect.left() + 4.0 + KEY_W, rect.bottom() - 4.0));
        let grid = Rect::from_min_max(pos2(keys.right(), keys.top()), pos2(rect.right() - 4.0, keys.bottom()));
        let ruler = Rect::from_min_max(pos2(grid.left(), header_top), pos2(grid.right(), grid.top()));

        self.piano_roll_scroll(ui, keys.union(grid), grid);
        let v = &self.view.pr;
        let (zoom, scroll, key_top, kh, snap) = (v.zoom as f64, v.scroll, v.key_top, v.key_h, v.snap);
        let x_of = |b: f64| grid.left() + ((b - scroll) * zoom) as f32;
        let beat_at = |x: f32| scroll + ((x - grid.left()) as f64 / zoom);
        let y_of = |k: u8| grid.top() + (key_top - k as f32) * kh;
        let key_at = |y: f32| (key_top - (y - grid.top()) / kh).floor().clamp(0.0, 127.0) as u8;
        let b1 = beat_at(grid.right());

        // Rows
        let gp = ui.painter_at(grid);
        let kp = ui.painter_at(keys);
        let lowest = key_at(grid.bottom());
        let highest = key_at(grid.top());
        for k in lowest..=highest {
            let y = y_of(k);
            let row = Rect::from_min_max(pos2(grid.left(), y), pos2(grid.right(), y + kh));
            gp.rect_filled(row, widgets::r(0.0), if is_black(k) { t.row_black } else { t.row_white });
            if k % 12 == 0 {
                gp.line_segment([row.left_bottom(), row.right_bottom()], Stroke::new(1.0_f32, t.grid_bar));
            }
            // Keyboard
            let key_rect = Rect::from_min_max(pos2(keys.left(), y), pos2(keys.right(), y + kh));
            let midi_target = self.view.selected_track == ti;
            let pressed = self.view.pr.preview_key == Some(k) || (midi_target && self.engine.shared.live_key(k));
            let fill = if pressed { color } else if is_black(k) { t.key_black } else { t.key_white };
            kp.rect_filled(key_rect.shrink2(vec2(0.0, 0.5)), widgets::r(1.0), fill);
            if k % 12 == 0 {
                caption(&kp, key_rect.right_center() - vec2(4.0, 0.0), Align2::RIGHT_CENTER, &key_name(k), 9.0, t.bar_text);
            }
        }
        // Verticals: grid steps, beats, bars
        let step = if zoom * snap >= 8.0 { snap } else if zoom >= 8.0 { 1.0 } else { BEATS_PER_BAR };
        let mut b = (scroll / step).floor() * step;
        while b < b1 {
            let x = x_of(b);
            let col = if b % BEATS_PER_BAR == 0.0 {
                t.grid_bar
            } else if b.fract() == 0.0 {
                with_alpha(t.grid_bar, 0.55)
            } else {
                t.grid_beat
            };
            gp.line_segment([pos2(x, grid.top()), pos2(x, grid.bottom())], Stroke::new(1.0_f32, col));
            b += step;
        }
        if clip_len < b1 {
            let r = Rect::from_min_max(pos2(x_of(clip_len).max(grid.left()), grid.top()), grid.max);
            gp.rect_filled(r, widgets::r(0.0), with_alpha(t.bg_dark, 0.65));
            gp.line_segment([r.left_top(), r.left_bottom()], Stroke::new(2.0_f32, t.mute));
        }

        // Ruler
        let rp = ui.painter_at(ruler);
        rp.rect_filled(ruler, widgets::r(0.0), t.bg_dark);
        let mut bar_i = (scroll / BEATS_PER_BAR).floor().max(0.0);
        let label_every = if zoom * BEATS_PER_BAR >= 40.0 { 1.0 } else { 4.0 };
        while bar_i * BEATS_PER_BAR < b1 {
            let x = x_of(bar_i * BEATS_PER_BAR);
            if bar_i % label_every == 0.0 {
                rp.line_segment([pos2(x, ruler.top() + 12.0), pos2(x, ruler.bottom())], Stroke::new(1.0_f32, t.text_faint));
                caption(&rp, pos2(x + 3.0, ruler.top() + 3.0), Align2::LEFT_TOP, &format!("{}", bar_i as i64 + 1), 10.0, t.text_dim);
            }
            bar_i += 1.0;
        }

        // Notes
        // A copy, so the edits below can borrow `self` mutably.
        let Some(clip) = self.clip(target).cloned() else {
            self.view.editor = None;
            return;
        };
        let clip = &clip;
        let note_rect = |n: &Note| -> Rect {
            Rect::from_min_max(pos2(x_of(n.start), y_of(n.key) + 1.0), pos2(x_of(n.end()) - 1.0, y_of(n.key) + kh - 1.0))
        };
        for (i, n) in clip.notes.iter().enumerate() {
            let r = note_rect(n);
            if !r.intersects(grid) {
                continue;
            }
            let outside = n.start >= clip_len;
            let alpha = if outside { 0.3 } else { 0.55 + 0.45 * n.velocity };
            gp.rect_filled(r, widgets::r(2.0), with_alpha(color, alpha));
            gp.rect_filled(
                Rect::from_min_max(r.min, pos2(r.left() + 3.0, r.bottom())),
                widgets::r(1.0),
                with_alpha(t.bar_text, 0.35),
            );
            if self.view.pr.selected == Some(i) {
                widgets::outline(&gp, r.expand(1.0), t.text, 1.5);
            }
            if r.width() > 34.0 && kh >= 12.0 {
                caption(&gp, r.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, &key_name(n.key), 8.5, t.bar_text);
            }
        }
        if let Some(ph) = playhead {
            let x = x_of(ph);
            if x >= grid.left() && x <= grid.right() {
                let col = if self.engine.shared.playing() { t.playhead } else { with_alpha(t.playhead, 0.5) };
                ui.painter_at(ruler.union(grid))
                    .line_segment([pos2(x, ruler.top()), pos2(x, grid.bottom())], Stroke::new(1.5_f32, col));
            }
        }

        // ---- interaction: grid
        let hit = |clip: &Clip, pos: Pos2| -> Option<(usize, bool)> {
            clip.notes.iter().enumerate().rev().find_map(|(i, n)| {
                let r = note_rect(n);
                r.contains(pos).then(|| (i, pos.x >= r.right() - EDGE_PX.min(r.width() * 0.4)))
            })
        };
        let resp = ui.interact(grid, Id::new(("pr_grid", track_id)), Sense::click_and_drag());
        if let Some(hp) = resp.hover_pos() {
            match hit(clip, hp) {
                Some((_, true)) => ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal),
                Some((_, false)) => ui.ctx().set_cursor_icon(CursorIcon::Grab),
                None => {}
            }
        }
        let snap_floor = |b: f64| ((b / snap).floor() * snap).max(0.0);
        let snap_round = |b: f64| ((b / snap).round() * snap).max(0.0);
        let origin = ui.input(|i| i.pointer.press_origin());
        let clip_hit_origin = origin.and_then(|o| hit(clip, o));
        let new_len = self.view.pr.new_len;
        let current = resp.interact_pointer_pos();

        if resp.drag_started() {
            if let Some(o) = origin {
                match clip_hit_origin {
                    Some((i, edge)) => {
                        let n = &clip.notes[i];
                        self.view.pr.drag = Some(NoteDrag {
                            index: i,
                            mode: if edge { NoteDragMode::Resize } else { NoteDragMode::Move },
                            orig: (n.start, n.len, n.key),
                        });
                        self.view.pr.selected = Some(i);
                    }
                    None => {
                        // Draw a new note by dragging.
                        let start = snap_floor(beat_at(o.x));
                        let key = key_at(o.y);
                        let clip = self.clip_mut(target).unwrap();
                        clip.notes.push(Note { start, len: snap, key, velocity: 0.8 });
                        let i = clip.notes.len() - 1;
                        self.view.pr.drag = Some(NoteDrag { index: i, mode: NoteDragMode::Resize, orig: (start, snap, key) });
                        self.view.pr.selected = Some(i);
                        self.dirty = true;
                    }
                }
            }
        }
        if let (true, Some(drag), Some(o), Some(cur)) = (resp.dragged(), self.view.pr.drag, origin, current) {
            let db = (cur.x - o.x) as f64 / zoom;
            let dk = -((cur.y - o.y) / kh).round() as i32;
            let (s0, l0, k0) = drag.orig;
            if let Some(n) = self.clip_mut(target).and_then(|c| c.notes.get_mut(drag.index)) {
                match drag.mode {
                    NoteDragMode::Move => {
                        n.start = snap_round(s0 + db);
                        n.key = (k0 as i32 + dk).clamp(0, 127) as u8;
                    }
                    NoteDragMode::Resize => {
                        n.len = (snap_round(s0 + l0 + db) - s0).max(snap);
                    }
                }
                self.dirty = true;
            }
        }
        if resp.drag_stopped() {
            if let Some(drag) = self.view.pr.drag.take() {
                if let Some(clip) = self.clip_mut(target) {
                    let len = clip.notes.get(drag.index).map(|n| n.len);
                    self.view.pr.selected = sort_keep(clip, drag.index);
                    if let Some(len) = len {
                        self.view.pr.new_len = len;
                    }
                }
                self.dirty = true;
            }
        }
        if resp.clicked() {
            if let Some(o) = origin {
                match clip_hit_origin {
                    Some((i, _)) => self.view.pr.selected = Some(i),
                    None => {
                        let clip = self.clip_mut(target).unwrap();
                        clip.notes.push(Note { start: snap_floor(beat_at(o.x)), len: new_len, key: key_at(o.y), velocity: 0.8 });
                        let i = clip.notes.len() - 1;
                        self.view.pr.selected = sort_keep(clip, i);
                        self.dirty = true;
                    }
                }
            }
        }
        if resp.secondary_clicked() {
            if let Some((i, _)) = current.and_then(|c| hit(clip, c)) {
                self.clip_mut(target).unwrap().notes.remove(i);
                self.view.pr.selected = None;
                self.dirty = true;
            }
        }

        // ---- interaction: keyboard preview
        let kresp = ui.interact(keys, Id::new(("pr_keys", track_id)), Sense::click_and_drag());
        let down_key = if kresp.is_pointer_button_down_on() {
            ui.input(|i| i.pointer.interact_pos()).filter(|p| keys.contains(*p)).map(|p| key_at(p.y))
        } else {
            None
        };
        if down_key != self.view.pr.preview_key {
            if let Some(k) = self.view.pr.preview_key {
                self.engine.send(Command::Note { track: track_id, key: k, velocity: 0.0, on: false });
            }
            if let Some(k) = down_key {
                self.engine.send(Command::Note { track: track_id, key: k, velocity: 0.8, on: true });
            }
            self.view.pr.preview_key = down_key;
        }
    }

    fn piano_roll_scroll(&mut self, ui: &egui::Ui, area: Rect, grid: Rect) {
        let v = &mut self.view.pr;
        if ui.rect_contains_pointer(area) {
            let (delta, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
            if zoom != 1.0 {
                if let Some(pp) = pointer {
                    let at = v.scroll + ((pp.x - grid.left()) / v.zoom) as f64;
                    v.zoom = (v.zoom * zoom).clamp(8.0, 800.0);
                    v.scroll = at - ((pp.x - grid.left()) / v.zoom) as f64;
                }
            } else {
                v.key_top += delta.y / v.key_h;
                v.scroll -= (delta.x / v.zoom) as f64;
            }
        }
        let visible = grid.height() / v.key_h;
        v.key_top = v.key_top.clamp(visible.min(127.0), 127.0);
        v.scroll = v.scroll.max(0.0);
    }
}

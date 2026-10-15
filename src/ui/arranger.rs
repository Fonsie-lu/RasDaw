//! Track headers and the arranger timeline (top right). The timeline holds scene blocks:
//! each block loops one of the track's scene clips and shows only its root key and time
//! signature. Drag a scene slot onto a lane (or double-click the lane) to add a block; drag a
//! block to move it, its right edge to resize; double-click it to edit the scene's notes. The
//! cross at a block's left deletes it; a click selects it and Del deletes the selection.

use super::widgets::{self, button, caption, meter, panel, section_bar, with_alpha};
use super::{App, BlockDrag, BlockDragMode, BAR_H, HEADER_H, MASTER_H, ROW_H, TRACK_HEAD_W};
use crate::engine::Command;
use crate::model::{Block, Clip, PlayMode, BEATS_PER_BAR, SCENES};
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
            self.engine.send(Command::StopScenes);
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
            let midi_target = i == self.view.selected_track;
            let selected = midi_target && !self.view.master_selected;
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
            let mode = match track.play_mode(playing) {
                PlayMode::Arrangement => "TIMELINE".to_owned(),
                PlayMode::Scene(s) => format!("SCENE {}", s + 1),
                PlayMode::Silent => "—".to_owned(),
            };
            caption(&hp, head.min + vec2(16.0, 27.0), Align2::LEFT_TOP, &mode, 9.5, t.text_dim);
            if midi_target {
                // The selected track receives the MIDI keyboard.
                let badge = Rect::from_min_size(pos2(head_x1 - 64.0, y + 26.0), vec2(36.0, 13.0));
                hp.rect_filled(badge, widgets::r(2.0), with_alpha(t.play, 0.85));
                caption(&hp, badge.center(), Align2::CENTER_CENTER, "MIDI", 9.0, t.bar_text);
            }
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
                // The track's active scene clip, else its first clip, else a new one.
                let scene = track
                    .active_scene
                    .or_else(|| (0..SCENES).find(|&s| track.scenes[s].is_some()))
                    .unwrap_or(0);
                action = Some((i, LaneAction::EditScene(scene)));
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

            // Lane: scene blocks. A block shows only its scene's root key and time signature.
            let lane_vis = lane.intersect(rows);
            if selected {
                lp.rect_filled(lane_vis, widgets::r(0.0), with_alpha(t.bg_highlight, 0.5));
            }
            lp.line_segment([pos2(lanes.left(), lane.bottom()), pos2(lanes.right(), lane.bottom())], Stroke::new(1.0_f32, t.border));
            let inner = lane.shrink2(vec2(0.0, 6.0));
            let lresp = ui.interact(lane_vis, Id::new(("lane", track.id)), Sense::click());
            if !track.has_arrangement() {
                if let PlayMode::Scene(s) = track.play_mode(playing) {
                    let label = format!("▶ SCENE {} LOOP", s + 1);
                    let pill = Rect::from_min_size(pos2(lanes.left() + 4.0, inner.center().y - 8.0), vec2(104.0, 16.0));
                    lp.rect_filled(pill, widgets::r(3.0), with_alpha(t.bg_dark, 0.85));
                    caption(&lp, pill.left_center() + vec2(5.0, 0.0), Align2::LEFT_CENTER, &label, 10.0, with_alpha(color, 0.8));
                }
            }
            for (bi, b) in track.blocks.iter().enumerate() {
                let r = Rect::from_min_max(pos2(x_of(b.start), inner.top()), pos2(x_of(b.end()) - 1.0, inner.bottom()));
                if !r.intersects(lanes) {
                    continue;
                }
                let clip = track.scenes.get(b.scene).and_then(Option::as_ref);
                let sounding = playing && pos >= b.start && pos < b.end();
                let fill = match clip {
                    Some(_) => with_alpha(color, if sounding { 0.42 } else { 0.28 }),
                    None => with_alpha(t.record, 0.18),
                };
                lp.rect_filled(r, widgets::r(3.0), fill);
                lp.rect_filled(Rect::from_min_size(r.min, vec2(r.width(), 3.0)), widgets::r(1.0), color);
                if let Some(clip) = clip {
                    // Loop points of the clip inside the block
                    let mut k = clip.length;
                    while clip.length > 0.0 && k < b.length - 1e-9 {
                        let x = x_of(b.start + k);
                        lp.line_segment([pos2(x, r.top() + 6.0), pos2(x, r.bottom() - 4.0)], Stroke::new(1.0_f32, with_alpha(color, 0.35)));
                        k += clip.length;
                    }
                }
                let text = clip.map_or_else(|| "EMPTY".to_owned(), |c| c.key_sig_label());
                // Delete cross at the left, then the scene number badge.
                let cross = Rect::from_min_size(pos2(r.left() + 3.0, r.center().y - 8.0), vec2(16.0, 16.0));
                let show_cross = r.width() > 44.0;
                let badge_x = if show_cross { cross.right() + 3.0 } else { r.left() + 4.0 };
                let badge = Rect::from_min_size(pos2(badge_x, r.center().y - 7.0), vec2(14.0, 14.0));
                let selected_block = self.view.selection
                    == Some(super::Selection::Block { track: track.id, block: b.clone() });
                if selected_block {
                    widgets::outline(&lp, r.expand(1.0), t.text, 2.0);
                }
                if r.width() > 24.0 {
                    lp.rect_filled(badge, widgets::r(2.0), color);
                    caption(&lp, badge.center(), Align2::CENTER_CENTER, &format!("{}", b.scene + 1), 9.0, t.bar_text);
                }
                if r.width() > 60.0 + if show_cross { 19.0 } else { 0.0 } {
                    let txt = widgets::fit_text(ui, &text, 12.0, r.right() - badge.right() - 8.0);
                    caption(&lp, pos2(badge.right() + 5.0, r.center().y), Align2::LEFT_CENTER, &txt, 12.0, t.text);
                }
                let id = Id::new(("block", track.id, bi));
                let bresp = ui.interact(r.intersect(lanes).intersect(rows), id, Sense::click_and_drag());
                let near_edge = |x: f32| x >= r.right() - 7.0;
                if let Some(hp) = bresp.hover_pos() {
                    ui.ctx().set_cursor_icon(if near_edge(hp.x) { egui::CursorIcon::ResizeHorizontal } else { egui::CursorIcon::Grab });
                }
                if show_cross {
                    let cresp = ui.interact(cross.intersect(rows), Id::new(("block_del", track.id, bi)), Sense::click());
                    if cresp.hovered() {
                        lp.rect_filled(cross, widgets::r(2.0), egui::Color32::from_black_alpha(70));
                    }
                    caption(&lp, cross.center(), Align2::CENTER_CENTER, "✕", 11.0, if cresp.hovered() { t.text } else { with_alpha(t.text, 0.65) });
                    if cresp.on_hover_text("Delete block (Del)").clicked() {
                        action = Some((i, LaneAction::DeleteBlock(bi)));
                    }
                }
                if bresp.drag_started() {
                    action = Some((i, LaneAction::SelectBlock(bi)));
                    let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or_default();
                    self.view.block_drag = Some(BlockDrag {
                        track: track.id,
                        index: bi,
                        mode: if near_edge(origin.x) { BlockDragMode::Resize } else { BlockDragMode::Move },
                        orig: (b.start, b.length),
                    });
                }
                if bresp.double_clicked() {
                    action = Some((i, LaneAction::EditScene(b.scene)));
                } else if bresp.clicked() {
                    action = Some((i, LaneAction::SelectBlock(bi)));
                }
                bresp.context_menu(|ui| {
                    for s in 0..SCENES {
                        if track.scenes[s].is_some() && ui.selectable_label(b.scene == s, format!("Scene {}", s + 1)).clicked() {
                            action = Some((i, LaneAction::SetBlockScene(bi, s)));
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("Edit notes").clicked() {
                        action = Some((i, LaneAction::EditScene(b.scene)));
                        ui.close_menu();
                    }
                    if ui.button("Delete block").clicked() {
                        action = Some((i, LaneAction::DeleteBlock(bi)));
                        ui.close_menu();
                    }
                });
            }
            // Drop target for a scene dragged from the launcher
            if let (Some((_, s)), Some(hp)) = (self.view.scene_drag, ui.ctx().pointer_hover_pos()) {
                if lane_vis.contains(hp) {
                    if let Some(clip) = track.scenes[s].as_ref() {
                        let start = (b0 + ((hp.x - tl.left()) as f64 / zoom)).max(0.0).floor();
                        let len = default_block_len(clip.length);
                        let r = Rect::from_min_max(pos2(x_of(start), inner.top()), pos2(x_of(start + len), inner.bottom()));
                        widgets::outline(&lp, r, color, 2.0);
                        if ui.input(|i| i.pointer.any_released()) {
                            action = Some((i, LaneAction::AddBlock(start, s)));
                        }
                    }
                }
            }
            let click_beat = lresp
                .interact_pointer_pos()
                .map(|pp| (b0 + ((pp.x - tl.left()) as f64 / zoom)).max(0.0).floor());
            if lresp.double_clicked() {
                // New block of the active scene (or the first clip) where the user clicked.
                let scene = track
                    .active_scene
                    .filter(|&s| track.scenes[s].is_some())
                    .or_else(|| (0..SCENES).find(|&s| track.scenes[s].is_some()));
                match (scene, click_beat) {
                    (Some(s), Some(beat)) => action = Some((i, LaneAction::AddBlock(beat, s))),
                    _ => action = Some((i, LaneAction::EditScene(0))),
                }
            } else if lresp.clicked() {
                action = Some((i, LaneAction::Select));
            }
            lresp.context_menu(|ui| {
                let beat = click_beat.unwrap_or(0.0);
                for s in 0..SCENES {
                    if track.scenes[s].is_some() && ui.button(format!("Add block: scene {}", s + 1)).clicked() {
                        action = Some((i, LaneAction::AddBlock(beat, s)));
                        ui.close_menu();
                    }
                }
                if track.has_arrangement() && ui.button("Clear all blocks").clicked() {
                    action = Some((i, LaneAction::ClearBlocks));
                    ui.close_menu();
                }
            });
        }

        self.drag_block(ui, zoom);

        self.master_row(ui, rect, rows, lanes, head_x1);

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
            self.view.master_selected = false;
            let track = &mut self.project.tracks[i];
            match a {
                LaneAction::Select => {}
                LaneAction::Mute => track.mute = !track.mute,
                LaneAction::Solo => track.solo = !track.solo,
                LaneAction::Rename => self.view.rename = Some((track.id, track.name.clone())),
                LaneAction::Colour => track.color = (track.color + 1) % t.tracks.len(),
                LaneAction::EditScene(scene) => {
                    self.edit_scene(i, scene);
                }
                LaneAction::AddBlock(start, scene) => {
                    let len = track.scenes[scene].as_ref().map_or(BEATS_PER_BAR, |c| default_block_len(c.length));
                    track.blocks.push(Block { start, length: len, scene });
                    track.sort_blocks();
                }
                LaneAction::SetBlockScene(bi, scene) => {
                    if let Some(b) = track.blocks.get_mut(bi) {
                        b.scene = scene;
                    }
                }
                LaneAction::DeleteBlock(bi) => {
                    if bi < track.blocks.len() {
                        track.blocks.remove(bi);
                    }
                }
                LaneAction::SelectBlock(bi) => {
                    if let Some(b) = track.blocks.get(bi) {
                        self.view.selection = Some(super::Selection::Block { track: track.id, block: b.clone() });
                    }
                }
                LaneAction::ClearBlocks => track.blocks.clear(),
                LaneAction::Delete => {
                    self.delete_track(i);
                }
            }
            self.dirty = true;
        }
    }

    /// Master row below the tracks: header with meter; its lane takes no blocks.
    fn master_row(&mut self, ui: &mut egui::Ui, rect: Rect, rows: Rect, lanes: Rect, head_x1: f32) {
        let t = th();
        let y = rows.top() + self.project.tracks.len() as f32 * ROW_H - self.view.scroll_y;
        let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), MASTER_H));
        if !row.intersects(rows) {
            return;
        }
        let hp = ui.painter_at(rows);
        let head = Rect::from_min_max(row.min, pos2(head_x1, row.bottom()));
        let selected = self.view.master_selected;
        hp.rect_filled(head.shrink2(vec2(4.0, 2.0)), widgets::r(3.0), if selected { t.bg_highlight } else { t.bg_card });
        hp.rect_filled(Rect::from_min_size(head.min + vec2(4.0, 2.0), vec2(5.0, MASTER_H - 4.0)), widgets::r(2.0), t.text);
        caption(&hp, head.left_center() + vec2(16.0, -6.0), Align2::LEFT_CENTER, "MASTER", 12.0, t.text);
        caption(&hp, head.left_center() + vec2(16.0, 9.0), Align2::LEFT_CENTER, "EFFECTS ONLY", 8.5, t.text_dim);
        meter(&hp, Rect::from_min_size(pos2(head_x1 - 20.0, y + 6.0), vec2(9.0, MASTER_H - 12.0)), self.view.master_meter);
        let lane = Rect::from_min_max(pos2(lanes.left(), y), pos2(lanes.right(), row.bottom())).intersect(rows);
        let lp = ui.painter_at(lanes);
        lp.rect_filled(lane, widgets::r(0.0), with_alpha(t.bg_dark, if selected { 0.6 } else { 0.85 }));
        lp.line_segment([lane.left_top(), lane.right_top()], Stroke::new(1.0_f32, t.bevel));
        let resp = ui.interact(head.intersect(rows), Id::new("master_head"), Sense::click());
        let lresp = ui.interact(lane, Id::new("master_lane"), Sense::click());
        if resp.clicked() || lresp.clicked() {
            self.view.master_selected = true;
        }
    }

    /// Moves or resizes the block being dragged (1-beat grid), and keeps blocks sorted.
    fn drag_block(&mut self, ui: &egui::Ui, zoom: f64) {
        let Some(d) = self.view.block_drag else {
            return;
        };
        let (down, origin, cur) = ui.input(|i| (i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.latest_pos()));
        let Some(ti) = self.track_index(d.track) else {
            self.view.block_drag = None;
            return;
        };
        if let (Some(o), Some(c)) = (origin, cur) {
            let db = ((c.x - o.x) as f64 / zoom).round();
            if let Some(b) = self.project.tracks[ti].blocks.get_mut(d.index) {
                let (s0, l0) = d.orig;
                let new = match d.mode {
                    BlockDragMode::Move => ((s0 + db).max(0.0), l0),
                    BlockDragMode::Resize => (s0, (l0 + db).max(1.0)),
                };
                if (b.start, b.length) != new {
                    (b.start, b.length) = new;
                    self.dirty = true;
                }
            }
        }
        if !down {
            if let Some(b) = self.project.tracks[ti].blocks.get(d.index) {
                self.view.selection = Some(super::Selection::Block { track: d.track, block: b.clone() });
            }
            self.project.tracks[ti].sort_blocks();
            self.view.block_drag = None;
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
    EditScene(usize),
    AddBlock(f64, usize),
    SetBlockScene(usize, usize),
    DeleteBlock(usize),
    SelectBlock(usize),
    ClearBlocks,
    Delete,
}

/// Length of a new block: whole loops of the clip, at least one 4/4 bar.
fn default_block_len(clip_len: f64) -> f64 {
    if clip_len <= 0.0 {
        return BEATS_PER_BAR;
    }
    clip_len * (BEATS_PER_BAR / clip_len).ceil().max(1.0)
}

/// Bar number spacing that keeps labels apart at the given zoom.
fn bar_label_step(zoom: f64) -> i64 {
    let bar_px = zoom * BEATS_PER_BAR;
    [1, 2, 4, 8, 16, 32, 64]
        .into_iter()
        .find(|&s| bar_px * s as f64 >= 36.0)
        .unwrap_or(64)
}

//! Top bar: play/stop, position display, tempo, timeline length, loop, file and engine status.

use super::widgets::{self, button, caption, font, meter, mono};
use super::App;
use crate::engine::Command;
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Rect, Stroke};

impl App {
    pub(super) fn transport_ui(&mut self, ui: &mut egui::Ui) {
        let t = th();
        let full = ui.max_rect();
        let p = ui.painter().clone();
        p.line_segment([full.left_bottom(), full.right_bottom()], Stroke::new(1.0_f32, t.bevel));
        let cy = full.center().y;
        let mut x = full.left() + 12.0;

        let playing = self.engine.shared.playing();
        let b = Rect::from_min_size(pos2(x, cy - 12.0), vec2(44.0, 24.0));
        if button(ui, b, "play", "▶", playing.then_some(t.play)).clicked() {
            self.engine.send(Command::Play);
        }
        x += 50.0;
        let b = Rect::from_min_size(pos2(x, cy - 12.0), vec2(44.0, 24.0));
        if button(ui, b, "stop", "■", None)
            .on_hover_text("Stop (again: back to start)")
            .clicked()
        {
            self.engine.send(Command::Stop);
        }
        x += 56.0;

        // OLED-style position display
        let pos = self.engine.shared.position();
        let disp = Rect::from_min_size(pos2(x, cy - 15.0), vec2(118.0, 30.0));
        p.rect_filled(disp, widgets::r(3.0), t.bg_dark);
        p.rect_stroke(disp, widgets::r(3.0), Stroke::new(1.0_f32, t.bevel), egui::StrokeKind::Inside);
        p.text(disp.center(), Align2::CENTER_CENTER, App::format_position(pos), mono(17.0), t.solo);
        x += 132.0;

        let mut bpm = self.project.bpm;
        let mut bars = self.project.bars;
        let mut loop_on = self.project.loop_enabled;
        caption(&p, pos2(x, cy), Align2::LEFT_CENTER, "BPM", 10.0, t.text_dim);
        ui.put(
            Rect::from_min_size(pos2(x + 28.0, cy - 11.0), vec2(62.0, 22.0)),
            egui::DragValue::new(&mut bpm).range(20.0..=300.0).speed(0.2).fixed_decimals(1),
        );
        x += 104.0;
        caption(&p, pos2(x, cy), Align2::LEFT_CENTER, "BARS", 10.0, t.text_dim);
        ui.put(
            Rect::from_min_size(pos2(x + 34.0, cy - 11.0), vec2(48.0, 22.0)),
            egui::DragValue::new(&mut bars).range(1..=512).speed(0.2),
        );
        x += 96.0;
        let b = Rect::from_min_size(pos2(x, cy - 10.0), vec2(52.0, 20.0));
        if button(ui, b, "loop", "LOOP", loop_on.then_some(t.mute))
            .on_hover_text("Loop the timeline over its whole length")
            .clicked()
        {
            loop_on = !loop_on;
        }
        if bpm != self.project.bpm || bars != self.project.bars || loop_on != self.project.loop_enabled {
            self.project.bpm = bpm;
            self.project.bars = bars;
            self.project.loop_enabled = loop_on;
            self.dirty = true;
        }
        x += 62.0;
        self.midi_button(ui, &p, x, cy);
        x += 76.0;

        // Right side: master meter, engine info, file controls (laid out right to left).
        let mut rx = full.right() - 12.0;
        let m = Rect::from_min_max(pos2(rx - 14.0, cy - 15.0), pos2(rx, cy + 15.0));
        meter(&p, m, self.view.master_meter);
        rx -= 22.0;
        let info = format!(
            "{} · {} Hz · {} · DSP {:>3.0}%",
            self.driver.name,
            self.driver.sample_rate,
            self.driver.period,
            self.engine.shared.load() * 100.0
        );
        let info_w = ui.fonts(|f| f.layout_no_wrap(info.clone(), font(10.0), t.text).size().x);
        caption(&p, pos2(rx, cy), Align2::RIGHT_CENTER, &info, 10.0, t.text_faint);
        rx -= info_w + 16.0;
        let b = Rect::from_min_size(pos2(rx - 50.0, cy - 10.0), vec2(50.0, 20.0));
        if button(ui, b, "open", "OPEN", None).on_hover_text("Load the project from the path").clicked() {
            self.load_project();
        }
        rx -= 56.0;
        let b = Rect::from_min_size(pos2(rx - 50.0, cy - 10.0), vec2(50.0, 20.0));
        if button(ui, b, "save", "SAVE", None).on_hover_text("Save the project (Ctrl+S)").clicked() {
            self.save_project();
        }
        rx -= 58.0;
        let path_w = (rx - x - 8.0).clamp(80.0, 320.0);
        ui.put(
            Rect::from_min_size(pos2(rx - path_w, cy - 11.0), vec2(path_w, 22.0)),
            egui::TextEdit::singleline(&mut self.path_text).font(font(11.0)),
        );
        if let Some(status) = self.status() {
            let s = status.to_owned();
            let left = x;
            let avail = rx - path_w - left - 12.0;
            if avail > 60.0 {
                let txt = widgets::fit_text(ui, &s, 10.5, avail);
                caption(&p, pos2(left, cy), Align2::LEFT_CENTER, &txt, 10.5, t.mute);
            }
        }
    }
}

impl App {
    /// MIDI button with activity LED; opens the input port list.
    fn midi_button(&mut self, ui: &mut egui::Ui, p: &egui::Painter, x: f32, cy: f32) {
        let t = th();
        let b = Rect::from_min_size(pos2(x, cy - 10.0), vec2(50.0, 20.0));
        let resp = button(ui, b, "midi", "MIDI", None).on_hover_text("MIDI keyboard inputs");
        let lit = self
            .view
            .midi_flash
            .is_some_and(|t| t.elapsed().as_millis() < 120);
        let led = pos2(b.right() + 10.0, cy);
        if lit {
            p.circle_filled(led, 7.0, widgets::with_alpha(t.play, 0.2));
        }
        p.circle_filled(led, 4.0, if lit { t.play } else { t.bg_highlight });
        p.circle_stroke(led, 4.0, Stroke::new(1.0_f32, egui::Color32::from_black_alpha(160)));

        let id = ui.make_persistent_id("midi_popup");
        if resp.clicked() {
            ui.memory_mut(|m| m.toggle_popup(id));
        }
        egui::popup_below_widget(ui, id, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
            ui.set_min_width(300.0);
            ui.label(egui::RichText::new("MIDI INPUTS → SELECTED TRACK").color(t.text_dim));
            ui.separator();
            if let Some(e) = self.midi.error() {
                ui.colored_label(t.record, e);
                return;
            }
            let ports = self.midi.ports();
            if ports.is_empty() {
                ui.label("No MIDI sources found. Plug in a keyboard; it connects automatically.");
            }
            for port in ports {
                let mut on = port.connected;
                let label = if port.hardware { port.name.clone() } else { format!("{} (software)", port.name) };
                if ui.checkbox(&mut on, label).changed() {
                    self.midi.set_connected(port.client, port.port, on);
                }
            }
        });
    }
}

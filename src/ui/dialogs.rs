//! Save / open dialog (Ctrl+S / Ctrl+O): a modal with the project path.

use super::widgets::{self, button, caption};
use super::App;
use crate::theme::current as th;
use eframe::egui::{self, pos2, vec2, Align2, Id, Key, Rect};
use std::path::PathBuf;

impl App {
    pub(super) fn file_dialog_ui(&mut self, ctx: &egui::Context) {
        let t = th();
        let Some(dialog) = self.view.file_dialog.as_mut() else {
            return;
        };
        let save = dialog.save;
        let mut confirm = false;
        let mut cancel = false;
        egui::Modal::new(Id::new("file_dialog"))
            .frame(egui::Frame::new().fill(t.bg_card).inner_margin(16.0).corner_radius(widgets::r(6.0)).stroke(egui::Stroke::new(1.0_f32, t.bevel)))
            .show(ctx, |ui| {
                ui.set_width(520.0);
                let (bar, _) = ui.allocate_exact_size(vec2(520.0, 22.0), egui::Sense::hover());
                widgets::section_bar(ui.painter(), bar, if save { "SAVE PROJECT" } else { "OPEN PROJECT" }, t.accent);
                ui.add_space(12.0);
                caption(ui.painter(), ui.cursor().min, Align2::LEFT_TOP, "FILE", 10.0, t.text_dim);
                ui.add_space(16.0);
                let resp = ui.add(egui::TextEdit::singleline(&mut dialog.path).desired_width(520.0).font(widgets::font(13.0)));
                resp.request_focus();
                ui.add_space(14.0);
                let (row, _) = ui.allocate_exact_size(vec2(520.0, 24.0), egui::Sense::hover());
                let ok = Rect::from_min_size(pos2(row.right() - 90.0, row.top()), vec2(90.0, 24.0));
                let no = Rect::from_min_size(pos2(row.right() - 190.0, row.top()), vec2(90.0, 24.0));
                if button(ui, ok, "dlg_ok", if save { "SAVE" } else { "OPEN" }, Some(t.accent)).clicked() {
                    confirm = true;
                }
                if button(ui, no, "dlg_cancel", "CANCEL", None).clicked() {
                    cancel = true;
                }
                caption(ui.painter(), row.left_center(), Align2::LEFT_CENTER, "Enter to confirm · Esc to cancel", 10.0, t.text_faint);
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    confirm = true;
                }
                if ui.input(|i| i.key_pressed(Key::Escape)) {
                    cancel = true;
                }
            });
        if confirm {
            let path = PathBuf::from(self.view.file_dialog.take().unwrap().path.trim());
            if save {
                self.save_project(&path);
            } else {
                self.load_project(&path);
            }
        } else if cancel {
            self.view.file_dialog = None;
        }
    }
}

//! Panel-style drawing helpers modelled on the Alpha Juno editor: coloured section bars,
//! raised buttons with a drop shadow, fader caps in slots, ribbed knobs, LEDs and meters.

use crate::theme::{current as th, Theme};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Painter, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, Ui,
};

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("panel".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("mono".into()))
}

pub fn r(px: f32) -> CornerRadius {
    CornerRadius::same(px as u8)
}

pub fn with_alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Linear mix of two colours (alpha included).
pub fn mix_colors(a: Color32, b: Color32, t: f32) -> Color32 {
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let l = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgba_unmultiplied(l(0), l(1), l(2), l(3))
}

/// Panel body with the bevel line along its top edge.
pub fn panel(p: &Painter, rect: Rect) {
    let t = th();
    p.rect_filled(rect, r(0.0), t.bg_panel);
    p.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0_f32, t.bevel),
    );
}

/// Rounded coloured bar with a centred dark title, like the DCO/VCF bars on the Juno panel.
pub fn section_bar(p: &Painter, rect: Rect, title: &str, color: Color32) {
    p.rect_filled(rect, r(3.0), color);
    p.text(rect.center(), Align2::CENTER_CENTER, title, font(13.0), th().bar_text);
}

/// Same bar, title left-aligned (used where the right side holds controls).
pub fn section_bar_left(p: &Painter, rect: Rect, title: &str, color: Color32) {
    p.rect_filled(rect, r(3.0), color);
    p.text(
        rect.left_center() + vec2(8.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        font(13.0),
        th().bar_text,
    );
}

/// Raised panel button. `lit` fills it with `color` (toggle state).
pub fn button(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, label: &str, lit: Option<Color32>) -> Response {
    let resp = ui.interact(rect, Id::new(("btn", id)), Sense::click());
    paint_button(ui.painter(), rect, label, lit, resp.hovered(), resp.is_pointer_button_down_on());
    resp
}

pub fn paint_button(p: &Painter, rect: Rect, label: &str, lit: Option<Color32>, hovered: bool, down: bool) {
    let t = th();
    let face = match lit {
        Some(c) => c,
        None if down => t.button_down,
        None if hovered => t.button_hover,
        None => t.button,
    };
    let rect = if down { rect.translate(vec2(0.0, 1.0)) } else { rect };
    p.rect_filled(rect.translate(vec2(0.0, 2.0)), r(2.0), Color32::from_black_alpha(150));
    p.rect_filled(rect, r(2.0), face);
    p.line_segment(
        [rect.left_top() + vec2(2.0, 1.0), rect.right_top() + vec2(-2.0, 1.0)],
        Stroke::new(1.0_f32, Color32::from_white_alpha(28)),
    );
    let text = if lit.is_some() { t.bar_text } else { t.text };
    let size = if rect.height() < 18.0 { 10.0 } else { 11.5 };
    p.text(rect.center(), Align2::CENTER_CENTER, label, font(size), text);
}

/// On/off flip switch: a pill with a sliding knob, filled with `accent` when on.
pub fn flip_switch(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, on: bool, accent: Color32) -> Response {
    let t = th();
    let resp = ui.interact(rect, Id::new(("switch", id)), Sense::click());
    let p = ui.painter();
    let radius = rect.height() * 0.5;
    let track = if on { accent } else { t.slot };
    p.rect_filled(rect.translate(vec2(0.0, 1.5)), r(radius), Color32::from_black_alpha(120));
    p.rect_filled(rect, r(radius), track);
    p.rect_stroke(rect, r(radius), Stroke::new(1.0_f32, t.bevel), StrokeKind::Inside);
    let x = if on { rect.right() - radius } else { rect.left() + radius };
    let c = pos2(x, rect.center().y);
    p.circle_filled(c, radius - 2.5, if resp.hovered() { t.text } else { t.knob_cap });
    resp
}

/// Stereo peak meter, bottom-up, with the usual green/yellow/red zones.
pub fn meter(p: &Painter, rect: Rect, level: [f32; 2]) {
    let t = th();
    p.rect_filled(rect, r(1.0), t.slot);
    let w = (rect.width() - 1.0) * 0.5;
    for (ch, &v) in level.iter().enumerate() {
        let x0 = rect.left() + ch as f32 * (w + 1.0);
        let db = 20.0 * v.max(1e-6).log10();
        let frac = ((db + 60.0) / 66.0).clamp(0.0, 1.0);
        if frac <= 0.0 {
            continue;
        }
        let top = rect.bottom() - frac * rect.height();
        let col = if db > 0.0 {
            t.meter_high
        } else if db > -9.0 {
            t.meter_mid
        } else {
            t.meter_low
        };
        p.rect_filled(
            Rect::from_min_max(pos2(x0, top), pos2(x0 + w, rect.bottom())),
            r(0.0),
            col,
        );
    }
}

/// Fader cap in a slot, as on the Juno panel. Returns the new value if it was moved.
pub fn fader(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash, value: f32, default: f32, accent: Color32) -> (Response, Option<f32>) {
    let t = th();
    let id = Id::new(("fader", id));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let travel = rect.height() - 30.0;
    let mut new = None;
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, value));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let start: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(value);
        let v = (start - resp.drag_delta().y / travel * if fine { 0.15 } else { 1.0 }).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(id, v));
        new = Some(v);
    }
    if resp.double_clicked() {
        new = Some(default);
    }
    if resp.hovered() && !resp.dragged() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            new = Some((value + scroll * 0.0015).clamp(0.0, 1.0));
        }
    }
    let p = ui.painter();
    let x = rect.center().x;
    let (y0, y1) = (rect.top() + 15.0, rect.bottom() - 15.0);
    // Scale ticks
    for i in 0..=10 {
        let y = y1 - i as f32 / 10.0 * (y1 - y0);
        let w = if i % 5 == 0 { 7.0 } else { 4.0 };
        p.line_segment([pos2(x - 9.0 - w, y), pos2(x - 9.0, y)], Stroke::new(1.0_f32, t.text_faint));
    }
    p.rect_filled(
        Rect::from_center_size(pos2(x, (y0 + y1) * 0.5), vec2(5.0, y1 - y0 + 14.0)),
        r(2.0),
        t.slot,
    );
    let v = new.unwrap_or(value);
    cap(p, t, pos2(x, y1 - v * (y1 - y0)), if resp.dragged() { t.text } else { accent });
    (resp, new)
}

fn cap(p: &Painter, t: &Theme, c: Pos2, line: Color32) {
    let body = Rect::from_center_size(c, vec2(20.0, 28.0));
    p.rect_filled(body.translate(vec2(1.5, 3.0)), r(2.0), Color32::from_black_alpha(140));
    p.rect_filled(body, r(2.0), t.cap);
    p.rect_filled(Rect::from_center_size(c - vec2(0.0, 7.5), vec2(18.0, 9.0)), r(1.0), t.cap_top);
    p.rect_filled(Rect::from_center_size(c + vec2(0.0, 7.5), vec2(18.0, 9.0)), r(1.0), t.cap_bottom);
    p.line_segment([c - vec2(9.0, 0.0), c + vec2(9.0, 0.0)], Stroke::new(2.2_f32, line));
}

/// Rotary knob for a 0..1 value with a value arc in `accent`. Vertical drag (Shift = fine),
/// wheel, double-click resets. Returns the new value if it changed.
pub fn knob(
    ui: &mut Ui,
    center: Pos2,
    radius: f32,
    id: impl std::hash::Hash,
    value: f32,
    default: f32,
    accent: Color32,
) -> (Response, Option<f32>) {
    let t = th();
    let id = Id::new(("knob", id));
    let rect = Rect::from_center_size(center, vec2(radius * 2.4, radius * 2.4));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut new = None;
    if resp.drag_started() {
        ui.data_mut(|d| d.insert_temp(id, value));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let start: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(value);
        let v = (start - resp.drag_delta().y / 160.0 * if fine { 0.15 } else { 1.0 }).clamp(0.0, 1.0);
        ui.data_mut(|d| d.insert_temp(id, v));
        new = Some(v);
    }
    if resp.double_clicked() {
        new = Some(default);
    }
    if resp.hovered() && !resp.dragged() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            new = Some((value + scroll * 0.001).clamp(0.0, 1.0));
        }
    }
    let v = new.unwrap_or(value);
    let p = ui.painter();
    let a0 = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let arc = |from: f32, to: f32, rad: f32| -> Vec<Pos2> {
        let steps = 32;
        (0..=steps)
            .map(|i| {
                let a = from + (to - from) * i as f32 / steps as f32;
                center + vec2(a.cos(), a.sin()) * rad
            })
            .collect()
    };
    p.add(Shape::line(arc(a0, a0 + sweep, radius + 3.0), Stroke::new(3.0_f32, t.knob_ring)));
    if v > 0.001 {
        p.add(Shape::line(arc(a0, a0 + sweep * v, radius + 3.0), Stroke::new(3.0_f32, accent)));
    }
    p.circle_filled(center + vec2(1.0, 2.0), radius - 1.0, Color32::from_black_alpha(140));
    p.circle_filled(center, radius - 1.0, t.cap_top);
    p.circle_filled(center, radius - 4.0, t.knob_cap);
    let a = a0 + sweep * v;
    p.line_segment(
        [center + vec2(a.cos(), a.sin()) * (radius * 0.25), center + vec2(a.cos(), a.sin()) * (radius - 4.5)],
        Stroke::new(2.5_f32, t.knob_pointer),
    );
    (resp, new)
}

/// Text clipped to `max_w` with an ellipsis.
pub fn fit_text(ui: &Ui, text: &str, size: f32, max_w: f32) -> String {
    let width = |s: &str| {
        ui.fonts(|f| f.layout_no_wrap(s.to_owned(), font(size), Color32::WHITE).size().x)
    };
    if width(text) <= max_w {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while !s.is_empty() && width(&format!("{s}…")) > max_w {
        s.pop();
    }
    format!("{s}…")
}

/// Global egui styling: Tokyo Night colours for the stock widgets (drag values, text edits,
/// menus, scroll bars), and the panel fonts.
pub fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "panel".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/DejaVuSansCondensed-Bold.ttf"
        ))),
    );
    fonts.font_data.insert(
        "mono".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
            "../../assets/fonts/DejaVuSansMono-Bold.ttf"
        ))),
    );
    fonts
        .families
        .insert(FontFamily::Name("panel".into()), vec!["panel".to_owned()]);
    fonts
        .families
        .insert(FontFamily::Name("mono".into()), vec!["mono".to_owned()]);
    // Stock widgets use the condensed panel font too, with egui's font as fallback for symbols.
    fonts
        .families
        .get_mut(&FontFamily::Proportional)
        .unwrap()
        .insert(0, "panel".to_owned());
    ctx.set_fonts(fonts);

    let t = th();
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(t.text);
    v.panel_fill = t.bg;
    v.window_fill = t.bg_card;
    v.window_stroke = Stroke::new(1.0_f32, t.bevel);
    v.extreme_bg_color = t.bg_dark;
    v.faint_bg_color = t.bg_panel;
    v.selection.bg_fill = t.selection;
    v.selection.stroke = Stroke::new(1.0_f32, t.text);
    v.hyperlink_color = t.accent;
    v.widgets.noninteractive.bg_fill = t.bg_panel;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.bevel);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text_dim);
    v.widgets.inactive.bg_fill = t.button;
    v.widgets.inactive.weak_bg_fill = t.button;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t.text);
    v.widgets.hovered.bg_fill = t.button_hover;
    v.widgets.hovered.weak_bg_fill = t.button_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, t.accent);
    v.widgets.active.bg_fill = t.selection;
    v.widgets.active.weak_bg_fill = t.selection;
    v.widgets.open.bg_fill = t.bg_highlight;
    v.window_corner_radius = r(4.0);
    v.menu_corner_radius = r(3.0);
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.interaction.tooltip_delay = 0.4;
    });
}

/// Small dimmed caption.
pub fn caption(p: &Painter, pos: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
    p.text(pos, align, text, font(size), color);
}

/// Outline used for focus/selection.
pub fn outline(p: &Painter, rect: Rect, color: Color32, width: f32) {
    p.rect_stroke(rect, r(2.0), Stroke::new(width, color), StrokeKind::Inside);
}

//! Tokyo Night colour scheme. Every colour the UI paints comes from here; never hard-code
//! colours in the UI modules. More schemes can be added later as further `Theme` constants.

use eframe::egui::Color32;

pub struct Theme {
    /// Main surfaces: window background, panel body, darker wells and raised cards.
    pub bg: Color32,
    pub bg_dark: Color32,
    pub bg_panel: Color32,
    pub bg_card: Color32,
    pub bg_highlight: Color32,
    /// Lines between rows and panels, and the bevel along panel edges.
    pub border: Color32,
    pub bevel: Color32,
    /// Grid lines in the timeline and piano roll.
    pub grid_beat: Color32,
    pub grid_bar: Color32,

    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    /// Dark text on coloured bars.
    pub bar_text: Color32,

    pub accent: Color32,
    pub playhead: Color32,
    pub play: Color32,
    pub record: Color32,
    pub mute: Color32,
    pub solo: Color32,
    pub selection: Color32,

    /// Fader slot, cap faces and knob parts, as on the Alpha Juno panel.
    pub slot: Color32,
    pub cap: Color32,
    pub cap_top: Color32,
    pub cap_bottom: Color32,
    pub knob_ring: Color32,
    pub knob_cap: Color32,
    pub knob_pointer: Color32,

    pub button: Color32,
    pub button_hover: Color32,
    pub button_down: Color32,

    pub meter_low: Color32,
    pub meter_mid: Color32,
    pub meter_high: Color32,

    /// White and black keys of the piano roll keyboard, and the row tints behind them.
    pub key_white: Color32,
    pub key_black: Color32,
    pub row_white: Color32,
    pub row_black: Color32,

    /// Track colours, indexed by `Track::color`.
    pub tracks: [Color32; 8],
    /// Section bar colours of the device rack (instrument, effect, unknown, built-in).
    pub device_bars: [Color32; 4],
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Tokyo Night (night variant).
pub const TOKYO_NIGHT: Theme = Theme {
    bg: c(0x1a1b26),
    bg_dark: c(0x16161e),
    bg_panel: c(0x1f2335),
    bg_card: c(0x24283b),
    bg_highlight: c(0x292e42),
    border: c(0x15161e),
    bevel: c(0x3b4261),
    grid_beat: c(0x232433),
    grid_bar: c(0x343a55),

    text: c(0xc0caf5),
    text_dim: c(0x737aa2),
    text_faint: c(0x565f89),
    bar_text: c(0x16161e),

    accent: c(0x7aa2f7),
    playhead: c(0xff9e64),
    play: c(0x9ece6a),
    record: c(0xf7768e),
    mute: c(0xe0af68),
    solo: c(0x7dcfff),
    selection: c(0x3d59a1),

    slot: c(0x16161e),
    cap: c(0x24283b),
    cap_top: c(0x414868),
    cap_bottom: c(0x1f2335),
    knob_ring: c(0x3b4261),
    knob_cap: c(0xa9b1d6),
    knob_pointer: c(0x16161e),

    button: c(0x3b4261),
    button_hover: c(0x414868),
    button_down: c(0x24283b),

    meter_low: c(0x9ece6a),
    meter_mid: c(0xe0af68),
    meter_high: c(0xf7768e),

    key_white: c(0xa9b1d6),
    key_black: c(0x16161e),
    row_white: c(0x1f2335),
    row_black: c(0x1a1b26),

    tracks: [
        c(0x7aa2f7),
        c(0xbb9af7),
        c(0x7dcfff),
        c(0x9ece6a),
        c(0xff9e64),
        c(0xe0af68),
        c(0x1abc9c),
        c(0xf7768e),
    ],
    device_bars: [c(0xbb9af7), c(0x7dcfff), c(0x737aa2), c(0x1abc9c)],
};

pub fn current() -> &'static Theme {
    &TOKYO_NIGHT
}

impl Theme {
    pub fn track(&self, index: usize) -> Color32 {
        self.tracks[index % self.tracks.len()]
    }
}

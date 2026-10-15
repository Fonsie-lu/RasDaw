//! Per-user settings, stored as JSON in `$XDG_CONFIG_HOME/rasdaw/settings.json`
//! (default `~/.config/rasdaw/settings.json`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn one() -> f32 {
    1.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// GUI scale (egui zoom factor), 1.0 = 100 %.
    #[serde(default = "one")]
    pub ui_scale: f32,
    /// Scene launch grid in beats (0 = launch at once).
    #[serde(default = "sixteenth")]
    pub launch_quantize: f64,
}

fn sixteenth() -> f64 {
    0.25
}

impl Default for Settings {
    fn default() -> Self {
        Self { ui_scale: 1.0, launch_quantize: sixteenth() }
    }
}

/// Launch grid choices: label and length in beats.
pub const LAUNCH_QUANTIZE: [(&str, f64); 9] = [
    ("Off", 0.0),
    ("1/32", 0.125),
    ("1/16", 0.25),
    ("1/8", 0.5),
    ("1/4", 1.0),
    ("1/2", 2.0),
    ("1 Bar", 4.0),
    ("2 Bars", 8.0),
    ("4 Bars", 16.0),
];

/// Scale steps offered in the UI.
pub const UI_SCALES: [f32; 9] = [0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5];

impl Settings {
    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("rasdaw").join("settings.json"))
    }

    pub fn load() -> Self {
        let mut s: Settings = Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        s.ui_scale = s.ui_scale.clamp(0.5, 3.0);
        s
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

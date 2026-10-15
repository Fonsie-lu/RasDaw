//! RasDaw by Fonsie Audio: Bitwig-inspired DAW framework with CLAP plugin hosting.
//!
//! Usage: `rasdaw [project.json]` (default `~/rasdaw-project.json`).

use rasdaw::{audio, clap_host, engine, ui};
use std::path::PathBuf;

fn main() -> eframe::Result {
    clap_host::mark_main_thread();

    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("rasdaw-project.json")
        });

    let registry = clap_host::Registry::scan();
    eprintln!(
        "found {} CLAP plugin(s), {} file(s) failed",
        registry.plugins.len(),
        registry.errors.len()
    );

    let (backend, driver) = audio::open();
    eprintln!("audio: {} @ {} Hz, period {}", driver.name, driver.sample_rate, driver.period);
    let (engine, handle) = engine::new(driver.sample_rate as f64, audio::MAX_BLOCK);
    let stream = audio::start(backend, engine);

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("RasDaw")
            .with_app_id("rasdaw")
            .with_inner_size([1600.0, 980.0])
            .with_min_inner_size([1000.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RasDaw",
        options,
        Box::new(move |cc| {
            ui::widgets::setup_style(&cc.egui_ctx);
            Ok(Box::new(ui::App::new(handle, stream, driver, registry, path)))
        }),
    )
}

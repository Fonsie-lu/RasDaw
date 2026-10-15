//! Opens a CLAP plugin's editor in an embedded X11 window, services it for a few seconds and
//! closes it again. No audio. Usage: cargo run --example editor_smoke -- [clap id] [seconds]
use rasdaw::clap_host::{self, gui::GuiManager, PluginInstance, Registry};
use std::time::{Duration, Instant};

fn main() {
    clap_host::mark_main_thread();
    let mut args = std::env::args().skip(1);
    let id = args.next().unwrap_or_else(|| "ch.fonsie.alpha-juno".into());
    let secs: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(4.0);
    let reg = Registry::scan();
    let desc = reg.find(&id).unwrap_or_else(|| panic!("{id} not installed"));
    let inst = PluginInstance::new(desc, 48_000.0, 512).expect("instantiate");
    let mut gui = GuiManager::default();
    gui.open(&inst, &format!("{} editor test", desc.name)).expect("open editor");
    println!("editor open: {}", gui.is_open(inst.uid));
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < secs && gui.is_open(inst.uid) {
        inst.idle();
        gui.poll();
        std::thread::sleep(Duration::from_millis(16));
    }
    gui.close(inst.uid);
    println!("closed cleanly");
}

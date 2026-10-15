//! Hosts a real CLAP instrument from the standard search path through the engine and checks
//! that it makes sound. Skips (passes) when no instrument is installed.
//! Set DAW_TEST_PLUGIN=<clap id> to pick one; the default prefers Alpha Juno.

use rasdaw::clap_host::{self, PluginInstance, Registry};
use rasdaw::engine::{self, Command, DeviceSnap, Snapshot};
use rasdaw::model::Project;

#[test]
fn instrument_plays_scene_notes() {
    clap_host::mark_main_thread();
    let reg = Registry::scan();
    let wanted = std::env::var("DAW_TEST_PLUGIN").unwrap_or_else(|_| "ch.fonsie.alpha-juno".into());
    let Some(desc) = reg
        .find(&wanted)
        .or_else(|| reg.plugins.iter().find(|d| d.is_instrument()))
        .cloned()
    else {
        eprintln!("no CLAP instrument installed; skipping");
        return;
    };
    eprintln!("testing with {} ({})", desc.name, desc.id);

    let inst = PluginInstance::new(&desc, 48_000.0, 512).expect("instantiate");
    let params = inst.params();
    eprintln!("{} parameters", params.len());
    if let Some(p) = params.first() {
        let v = inst.param_value(p.id).expect("get_value");
        eprintln!("first param {} = {v} ({:?})", p.name, inst.param_text(p.id, v));
    }
    let state = inst.save_state();
    if let Some(s) = &state {
        assert!(inst.load_state(s), "state round trip");
    }

    // Demo project: track 0 loops an arpeggio in scene 1.
    let project = Project::demo();
    let lead = project.tracks[0].id;
    let (mut engine, mut handle) = engine::new(48_000.0, 512);
    let dev = inst.clone();
    handle.publish(Snapshot::build(&project, |id| {
        if id == lead {
            vec![DeviceSnap { instance: dev.clone(), bypass: false }]
        } else {
            Vec::new()
        }
    }));
    handle.send(Command::Play);

    // Render 1 s on a thread marked as the audio thread.
    let peak = std::thread::spawn(move || {
        clap_host::mark_audio_thread();
        let mut out = vec![0.0f32; 2 * 480];
        let mut peak = 0.0f32;
        for _ in 0..100 {
            engine.render_interleaved(&mut out);
            peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
        }
        drop(engine); // stops processing on this thread
        peak
    })
    .join()
    .unwrap();
    handle.collect_garbage();
    eprintln!("peak {peak}");
    assert!(peak > 1e-3, "instrument stayed silent (peak {peak})");
    assert!(peak.is_finite());
    drop(inst);
}

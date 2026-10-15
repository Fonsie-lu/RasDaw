//! Hosts a real CLAP instrument from the standard search path through the engine and checks
//! that it makes sound. Skips (passes) when no instrument is installed.
//! Set DAW_TEST_PLUGIN=<clap id> to pick one; the default prefers Alpha Juno.

use rasdaw::clap_host::{self, PluginInstance, Registry};
use rasdaw::engine::{self, Command, DeviceRef, DeviceSnap, Snapshot};
use rasdaw::devices::{BuiltinDevice, Kind};
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
    // Built-in limiter after the instrument: +12 dB into a -12 dB ceiling.
    let limiter = BuiltinDevice::new(Kind::Limiter, 48_000.0);
    limiter.set_value(0, 12.0);
    limiter.set_value(1, -12.0);
    let ceiling = 10f32.powf(-12.0 / 20.0);
    handle.publish(Snapshot::build(&project, |id| {
        if id == lead {
            vec![
                DeviceSnap { device: DeviceRef::Clap(dev.clone()), bypass: false, note_fx: false },
                DeviceSnap { device: DeviceRef::Builtin(limiter.clone()), bypass: false, note_fx: false },
            ]
        } else {
            Vec::new()
        }
    }));
    // Scenes play on their own launcher clock; no arranger transport needed.
    handle.send(Command::LaunchScene { track: lead, scene: Some(0) });

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
    // Track volume 0.8 is unity and pan centre is unity, so the limiter ceiling holds at the output.
    assert!(peak <= ceiling + 1e-3, "limiter in the chain did not hold {ceiling} (peak {peak})");
    assert!(peak.is_finite());
    drop(inst);
}

/// Renders 0.5 s of the demo lead track through `chain` and returns the left channel.
fn render_lead(chain: Vec<DeviceSnap>) -> Vec<f32> {
    let project = Project::demo();
    let lead = project.tracks[0].id;
    let (mut engine, mut handle) = engine::new(48_000.0, 512);
    let mut chain = Some(chain);
    handle.publish(Snapshot::build(&project, |id| if id == lead { chain.take().unwrap_or_default() } else { Vec::new() }));
    handle.send(Command::LaunchScene { track: lead, scene: Some(0) });
    let out = std::thread::spawn(move || {
        clap_host::mark_audio_thread();
        let mut buf = vec![0.0f32; 2 * 480];
        let mut left = Vec::new();
        for _ in 0..50 {
            engine.render_interleaved(&mut buf);
            left.extend(buf.iter().step_by(2));
        }
        drop(engine);
        left
    })
    .join()
    .unwrap();
    handle.collect_garbage();
    out
}

#[test]
fn builtin_note_fx_changes_what_the_instrument_plays() {
    clap_host::mark_main_thread();
    let reg = Registry::scan();
    let Some(desc) = reg.find("ch.fonsie.alpha-juno").cloned() else {
        eprintln!("Alpha Juno not installed; skipping");
        return;
    };
    let synth = |note_fx: Option<Kind>| {
        let inst = PluginInstance::new(&desc, 48_000.0, 512).expect("instantiate");
        let mut chain = Vec::new();
        if let Some(kind) = note_fx {
            let dev = BuiltinDevice::new(kind, 48_000.0);
            if kind == Kind::Chord {
                dev.set_value(0, 10.0); // octave chord: adds the note an octave up
            }
            chain.push(DeviceSnap { device: DeviceRef::Builtin(dev), bypass: false, note_fx: true });
        }
        chain.push(DeviceSnap { device: DeviceRef::Clap(inst), bypass: false, note_fx: false });
        render_lead(chain)
    };
    let plain = synth(None);
    let chord = synth(Some(Kind::Chord));
    let energy = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
    let diff: f32 = plain.iter().zip(&chord).map(|(a, b)| (a - b) * (a - b)).sum();
    eprintln!("energy plain {} chord {} diff {}", energy(&plain), energy(&chord), diff);
    assert!(energy(&plain) > 1.0 && energy(&chord) > 1.0);
    assert!(diff > 0.1 * energy(&plain), "the chord device had no effect");
}

#[test]
fn master_effects_process_the_sum() {
    clap_host::mark_main_thread();
    let reg = Registry::scan();
    let Some(desc) = reg.find("ch.fonsie.alpha-juno").cloned() else {
        eprintln!("Alpha Juno not installed; skipping");
        return;
    };
    let project = Project::demo();
    let lead = project.tracks[0].id;
    let inst = PluginInstance::new(&desc, 48_000.0, 512).expect("instantiate");
    // Limiter on the master: +12 dB into a -6 dB ceiling. Alpha Juno alone peaks near
    // 0.26, so a peak right at the ceiling (0.50) proves the master chain ran.
    let limiter = BuiltinDevice::new(Kind::Limiter, 48_000.0);
    limiter.set_value(0, 12.0);
    limiter.set_value(1, -6.0);
    let ceiling = 10f32.powf(-6.0 / 20.0);
    let (mut engine, mut handle) = engine::new(48_000.0, 512);
    let lim = limiter.clone();
    handle.publish(Snapshot::build(&project, |id| {
        if id == lead {
            vec![DeviceSnap { device: DeviceRef::Clap(inst.clone()), bypass: false, note_fx: false }]
        } else if id == rasdaw::model::MASTER_ID {
            vec![DeviceSnap { device: DeviceRef::Builtin(lim.clone()), bypass: false, note_fx: false }]
        } else {
            Vec::new()
        }
    }));
    handle.send(Command::LaunchScene { track: lead, scene: Some(0) });
    let peak = std::thread::spawn(move || {
        clap_host::mark_audio_thread();
        let mut out = vec![0.0f32; 2 * 480];
        let mut peak = 0.0f32;
        for _ in 0..100 {
            engine.render_interleaved(&mut out);
            peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
        }
        drop(engine);
        peak
    })
    .join()
    .unwrap();
    handle.collect_garbage();
    eprintln!("master peak {peak}");
    assert!(peak <= ceiling + 1e-3, "master limiter did not hold {ceiling} (peak {peak})");
    assert!(peak > 0.9 * ceiling, "master chain did not process the sum (peak {peak})");
}

//! End-to-end MIDI input: a second ALSA sequencer client plays notes into RasDaw's input port,
//! and the engine must route them to the MIDI target track. Skips when there is no sequencer.

use rasdaw::engine::{self, Snapshot};
use rasdaw::midi::MidiInput;
use rasdaw::model::Project;
use std::ffi::{c_char, c_int, c_uint, c_void};
use std::time::{Duration, Instant};

#[repr(C)]
#[derive(Clone, Copy)]
struct SeqEvent {
    type_: u8,
    flags: u8,
    tag: u8,
    queue: u8,
    time: [u32; 2],
    source: [u8; 2],
    dest: [u8; 2],
    data: [u8; 12],
}

/// Minimal sequencer client with one output port.
struct Sender {
    _lib: libloading::Library,
    seq: *mut c_void,
    port: c_int,
    client: c_int,
    output: unsafe extern "C" fn(*mut c_void, *mut SeqEvent) -> c_int,
}

impl Sender {
    fn open() -> Option<Self> {
        unsafe {
            let lib = libloading::Library::new("libasound.so.2").ok()?;
            let open: unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int =
                *lib.get(b"snd_seq_open\0").ok()?;
            let set_name: unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int =
                *lib.get(b"snd_seq_set_client_name\0").ok()?;
            let create: unsafe extern "C" fn(*mut c_void, *const c_char, c_uint, c_uint) -> c_int =
                *lib.get(b"snd_seq_create_simple_port\0").ok()?;
            let client_id: unsafe extern "C" fn(*mut c_void) -> c_int = *lib.get(b"snd_seq_client_id\0").ok()?;
            let output = *lib.get(b"snd_seq_event_output_direct\0").ok()?;
            let mut seq = std::ptr::null_mut();
            if open(&mut seq, c"default".as_ptr(), 1, 0) < 0 {
                return None;
            }
            set_name(seq, c"RasDaw test keyboard".as_ptr());
            let port = create(seq, c"Keys".as_ptr(), 1 | (1 << 5), (1 << 1) | (1 << 20));
            let client = client_id(seq);
            Some(Self { _lib: lib, seq, port, client, output })
        }
    }

    fn note(&self, on: bool, key: u8, velocity: u8) {
        let mut data = [0u8; 12];
        data[1] = key;
        data[2] = velocity;
        let mut ev = SeqEvent {
            type_: if on { 6 } else { 7 },
            flags: 0,
            tag: 0,
            queue: 253, // direct
            time: [0; 2],
            source: [self.client as u8, self.port as u8],
            dest: [254, 253], // all subscribers
            data,
        };
        unsafe { (self.output)(self.seq, &mut ev) };
    }
}

fn wait_until(mut f: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(3) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn keyboard_notes_reach_the_target_track() {
    let Some(sender) = Sender::open() else {
        eprintln!("no ALSA sequencer; skipping");
        return;
    };
    let project = Project::demo();
    let target = project.tracks[1].id;
    let (mut engine, mut handle) = engine::new(48_000.0, 256);
    handle.publish(Snapshot::build(&project, |_| Vec::new()));
    let shared = handle.shared.clone();
    shared.set_midi_target(target);
    let midi = MidiInput::start(handle.take_midi_input().unwrap(), shared.clone());
    assert!(midi.error().is_none(), "{:?}", midi.error());

    // The test client is software, so it is listed but not auto-connected.
    assert!(wait_until(|| midi.ports().iter().any(|p| p.client == sender.client)), "port not listed");
    midi.set_connected(sender.client, sender.port, true);
    assert!(wait_until(|| midi.ports().iter().any(|p| p.client == sender.client && p.connected)));

    let mut out = vec![0.0f32; 512];
    let mut render_until = |want: bool| {
        wait_until(|| {
            engine.render_interleaved(&mut out);
            shared.live_key(64) == want
        })
    };
    let activity = shared.midi_activity_count();
    sender.note(true, 64, 100);
    assert!(render_until(true), "note-on did not arrive");
    assert!(shared.midi_activity_count() > activity);
    sender.note(false, 64, 0);
    assert!(render_until(false), "note-off did not arrive");
    drop(midi);
    handle.collect_garbage();
}

//! MIDI keyboard input through the ALSA sequencer, loaded at runtime with `dlopen`
//! (`libasound.so.2`), so no ALSA headers are needed.
//!
//! A background thread owns the sequencer handle. It:
//! - creates one input port ("RasDaw:Input"),
//! - connects every hardware MIDI output port, also ones plugged in later (it listens to the
//!   system announce port),
//! - turns incoming events into raw MIDI bytes and pushes them to the engine's MIDI queue.
//!
//! The engine routes the messages to the MIDI target track (the selected track in the UI).
//! The UI lists the ports and can connect or disconnect each one via [`MidiInput::set_connected`].

use crate::engine::Shared;
use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One raw MIDI channel message: status, data 1, data 2.
pub type MidiMsg = [u8; 3];

#[derive(Clone, Debug, PartialEq)]
pub struct PortInfo {
    pub client: i32,
    pub port: i32,
    /// "Client name: port name".
    pub name: String,
    pub hardware: bool,
    pub connected: bool,
}

#[derive(Default)]
pub struct MidiState {
    pub ports: Vec<PortInfo>,
    pub error: Option<String>,
}

enum Ctl {
    Connect(i32, i32, bool),
}

/// UI-side handle of the MIDI thread.
pub struct MidiInput {
    state: Arc<Mutex<MidiState>>,
    ctl: Option<mpsc::Sender<Ctl>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MidiInput {
    /// Starts the MIDI thread. If the sequencer is not available, the returned handle only
    /// carries the error.
    pub fn start(out: rtrb::Producer<MidiMsg>, shared: Arc<Shared>) -> Self {
        let state = Arc::new(Mutex::new(MidiState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (ctl_tx, ctl_rx) = mpsc::channel();
        let seq = match Seq::open("RasDaw") {
            Ok(seq) => seq,
            Err(e) => {
                eprintln!("midi: {e}");
                state.lock().unwrap().error = Some(e);
                return Self {
                    state,
                    ctl: None,
                    stop,
                    thread: None,
                };
            }
        };
        let thread = {
            let state = state.clone();
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("midi".into())
                .spawn(move || run(seq, out, shared, state, ctl_rx, stop))
                .expect("spawn midi thread")
        };
        Self {
            state,
            ctl: Some(ctl_tx),
            stop,
            thread: Some(thread),
        }
    }

    pub fn ports(&self) -> Vec<PortInfo> {
        self.state.lock().unwrap().ports.clone()
    }

    pub fn error(&self) -> Option<String> {
        self.state.lock().unwrap().error.clone()
    }

    pub fn set_connected(&self, client: i32, port: i32, on: bool) {
        if let Some(ctl) = &self.ctl {
            let _ = ctl.send(Ctl::Connect(client, port, on));
        }
    }
}

impl Drop for MidiInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------- thread

fn run(
    seq: Seq,
    mut out: rtrb::Producer<MidiMsg>,
    shared: Arc<Shared>,
    state: Arc<Mutex<MidiState>>,
    ctl: mpsc::Receiver<Ctl>,
    stop: Arc<AtomicBool>,
) {
    // Ports the user disconnected by hand; hot-plug auto-connect leaves them alone.
    let mut manual_off: Vec<(i32, i32)> = Vec::new();
    let mut connected: Vec<(i32, i32)> = Vec::new();
    let rescan = |seq: &Seq, connected: &mut Vec<(i32, i32)>, manual_off: &[(i32, i32)]| {
        let ports = seq.list_inputs();
        connected.retain(|c| ports.iter().any(|p| (p.client, p.port) == *c));
        for p in &ports {
            let key = (p.client, p.port);
            if p.hardware && !connected.contains(&key) && !manual_off.contains(&key) && seq.connect(p.client, p.port, true) {
                connected.push(key);
            }
        }
        let list = ports
            .into_iter()
            .map(|mut p| {
                p.connected = connected.contains(&(p.client, p.port));
                p
            })
            .collect();
        state.lock().unwrap().ports = list;
    };
    rescan(&seq, &mut connected, &manual_off);

    while !stop.load(Ordering::Acquire) {
        let mut topology_changed = false;
        if seq.wait(100) {
            while let Some(ev) = seq.next_event() {
                match ev {
                    SeqInput::Midi(msg) => {
                        let _ = out.push(msg);
                        shared.midi_activity();
                    }
                    SeqInput::Topology => topology_changed = true,
                }
            }
        }
        while let Ok(Ctl::Connect(client, port, on)) = ctl.try_recv() {
            let key = (client, port);
            if seq.connect(client, port, on) || !on {
                connected.retain(|c| *c != key);
                manual_off.retain(|c| *c != key);
                if on {
                    connected.push(key);
                } else {
                    manual_off.push(key);
                }
            }
            topology_changed = true;
        }
        if topology_changed {
            rescan(&seq, &mut connected, &manual_off);
        }
    }
}

// ---------------------------------------------------------------- ALSA sequencer FFI

const SND_SEQ_OPEN_INPUT: c_int = 2;
const SND_SEQ_NONBLOCK: c_int = 1;
const CAP_READ: c_uint = 1 << 0;
const CAP_WRITE: c_uint = 1 << 1;
const CAP_SUBS_READ: c_uint = 1 << 5;
const CAP_SUBS_WRITE: c_uint = 1 << 6;
const CAP_NO_EXPORT: c_uint = 1 << 7;
const TYPE_MIDI_GENERIC: c_uint = 1 << 1;
const TYPE_HARDWARE: c_uint = 1 << 16;
const TYPE_APPLICATION: c_uint = 1 << 20;
const CLIENT_SYSTEM: c_int = 0;
const PORT_SYSTEM_ANNOUNCE: c_int = 1;
const POLLIN: i16 = 0x1;

const EV_NOTEON: u8 = 6;
const EV_NOTEOFF: u8 = 7;
const EV_KEYPRESS: u8 = 8;
const EV_CONTROLLER: u8 = 10;
const EV_PGMCHANGE: u8 = 11;
const EV_CHANPRESS: u8 = 12;
const EV_PITCHBEND: u8 = 13;
const EV_CLIENT_START: u8 = 60;
const EV_CLIENT_EXIT: u8 = 61;
const EV_PORT_START: u8 = 63;
const EV_PORT_EXIT: u8 = 64;

/// `snd_seq_event_t` (28 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SeqEvent {
    pub type_: u8,
    pub flags: u8,
    pub tag: u8,
    pub queue: u8,
    pub time: [u32; 2],
    pub source: [u8; 2],
    pub dest: [u8; 2],
    pub data: [u8; 12],
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

extern "C" {
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: i32) -> i32;
}

enum SeqInput {
    Midi(MidiMsg),
    Topology,
}

/// Converts a sequencer event into a MIDI channel message.
pub(crate) fn event_to_midi(ev: &SeqEvent) -> Option<MidiMsg> {
    let ch = ev.data[0] & 0x0f;
    let note = ev.data[1] & 0x7f;
    let vel = ev.data[2] & 0x7f;
    let param = u32::from_ne_bytes(ev.data[4..8].try_into().unwrap());
    let value = i32::from_ne_bytes(ev.data[8..12].try_into().unwrap());
    let d = |v: i32| v.clamp(0, 127) as u8;
    Some(match ev.type_ {
        EV_NOTEON => [0x90 | ch, note, vel],
        EV_NOTEOFF => [0x80 | ch, note, vel],
        EV_KEYPRESS => [0xa0 | ch, note, vel],
        EV_CONTROLLER => [0xb0 | ch, (param & 0x7f) as u8, d(value)],
        EV_PGMCHANGE => [0xc0 | ch, d(value), 0],
        EV_CHANPRESS => [0xd0 | ch, d(value), 0],
        EV_PITCHBEND => {
            let v = (value + 8192).clamp(0, 16383) as u16;
            [0xe0 | ch, (v & 0x7f) as u8, (v >> 7) as u8]
        }
        _ => return None,
    })
}

type Open = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetName = unsafe extern "C" fn(*mut c_void, *const c_char) -> c_int;
type ClientId = unsafe extern "C" fn(*mut c_void) -> c_int;
type CreatePort = unsafe extern "C" fn(*mut c_void, *const c_char, c_uint, c_uint) -> c_int;
type ConnectFrom = unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int) -> c_int;
type EventInput = unsafe extern "C" fn(*mut c_void, *mut *mut SeqEvent) -> c_int;
type PollCount = unsafe extern "C" fn(*mut c_void, i16) -> c_int;
type PollDescs = unsafe extern "C" fn(*mut c_void, *mut PollFd, c_uint, i16) -> c_int;
type Close = unsafe extern "C" fn(*mut c_void) -> c_int;
type InfoMalloc = unsafe extern "C" fn(*mut *mut c_void) -> c_int;
type InfoFree = unsafe extern "C" fn(*mut c_void);
type InfoSetInt = unsafe extern "C" fn(*mut c_void, c_int);
type InfoGetInt = unsafe extern "C" fn(*const c_void) -> c_int;
type InfoGetUint = unsafe extern "C" fn(*const c_void) -> c_uint;
type InfoGetName = unsafe extern "C" fn(*const c_void) -> *const c_char;
type QueryNext = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

struct Fns {
    event_input: EventInput,
    poll_count: PollCount,
    poll_descs: PollDescs,
    connect_from: ConnectFrom,
    disconnect_from: ConnectFrom,
    close: Close,
    client_info_malloc: InfoMalloc,
    client_info_free: InfoFree,
    client_info_set_client: InfoSetInt,
    client_info_get_client: InfoGetInt,
    client_info_get_name: InfoGetName,
    query_next_client: QueryNext,
    port_info_malloc: InfoMalloc,
    port_info_free: InfoFree,
    port_info_set_client: InfoSetInt,
    port_info_set_port: InfoSetInt,
    port_info_get_port: InfoGetInt,
    port_info_get_name: InfoGetName,
    port_info_get_capability: InfoGetUint,
    port_info_get_type: InfoGetUint,
    query_next_port: QueryNext,
}

struct Seq {
    _lib: libloading::Library,
    f: Fns,
    handle: *mut c_void,
    me: c_int,
    port: c_int,
}

// Only the MIDI thread uses the handle after `open`.
unsafe impl Send for Seq {}

impl Seq {
    fn open(name: &str) -> Result<Self, String> {
        unsafe {
            let lib = libloading::Library::new("libasound.so.2").map_err(|e| e.to_string())?;
            macro_rules! sym {
                ($name:literal) => {
                    *lib.get($name).map_err(|e| format!("{e}"))?
                };
            }
            let open: Open = sym!(b"snd_seq_open\0");
            let set_name: SetName = sym!(b"snd_seq_set_client_name\0");
            let client_id: ClientId = sym!(b"snd_seq_client_id\0");
            let create_port: CreatePort = sym!(b"snd_seq_create_simple_port\0");
            let f = Fns {
                event_input: sym!(b"snd_seq_event_input\0"),
                poll_count: sym!(b"snd_seq_poll_descriptors_count\0"),
                poll_descs: sym!(b"snd_seq_poll_descriptors\0"),
                connect_from: sym!(b"snd_seq_connect_from\0"),
                disconnect_from: sym!(b"snd_seq_disconnect_from\0"),
                close: sym!(b"snd_seq_close\0"),
                client_info_malloc: sym!(b"snd_seq_client_info_malloc\0"),
                client_info_free: sym!(b"snd_seq_client_info_free\0"),
                client_info_set_client: sym!(b"snd_seq_client_info_set_client\0"),
                client_info_get_client: sym!(b"snd_seq_client_info_get_client\0"),
                client_info_get_name: sym!(b"snd_seq_client_info_get_name\0"),
                query_next_client: sym!(b"snd_seq_query_next_client\0"),
                port_info_malloc: sym!(b"snd_seq_port_info_malloc\0"),
                port_info_free: sym!(b"snd_seq_port_info_free\0"),
                port_info_set_client: sym!(b"snd_seq_port_info_set_client\0"),
                port_info_set_port: sym!(b"snd_seq_port_info_set_port\0"),
                port_info_get_port: sym!(b"snd_seq_port_info_get_port\0"),
                port_info_get_name: sym!(b"snd_seq_port_info_get_name\0"),
                port_info_get_capability: sym!(b"snd_seq_port_info_get_capability\0"),
                port_info_get_type: sym!(b"snd_seq_port_info_get_type\0"),
                query_next_port: sym!(b"snd_seq_query_next_port\0"),
            };
            let mut handle = std::ptr::null_mut();
            let e = open(&mut handle, c"default".as_ptr(), SND_SEQ_OPEN_INPUT, SND_SEQ_NONBLOCK);
            if e < 0 || handle.is_null() {
                return Err(format!("ALSA sequencer not available ({e})"));
            }
            let cname = CString::new(name).unwrap_or_default();
            set_name(handle, cname.as_ptr());
            let port = create_port(
                handle,
                c"Input".as_ptr(),
                CAP_WRITE | CAP_SUBS_WRITE,
                TYPE_MIDI_GENERIC | TYPE_APPLICATION,
            );
            if port < 0 {
                (f.close)(handle);
                return Err(format!("could not create the MIDI input port ({port})"));
            }
            let me = client_id(handle);
            // Hot-plug notifications.
            (f.connect_from)(handle, port, CLIENT_SYSTEM, PORT_SYSTEM_ANNOUNCE);
            Ok(Self {
                _lib: lib,
                f,
                handle,
                me,
                port,
            })
        }
    }

    /// Blocks up to `timeout_ms` until events arrive.
    fn wait(&self, timeout_ms: i32) -> bool {
        unsafe {
            let n = (self.f.poll_count)(self.handle, POLLIN).clamp(0, 8) as usize;
            let mut fds: Vec<PollFd> = (0..n).map(|_| PollFd { fd: -1, events: 0, revents: 0 }).collect();
            let n = (self.f.poll_descs)(self.handle, fds.as_mut_ptr(), n as c_uint, POLLIN).max(0) as usize;
            poll(fds.as_mut_ptr(), n as _, timeout_ms) > 0
        }
    }

    fn next_event(&self) -> Option<SeqInput> {
        loop {
            let mut ev: *mut SeqEvent = std::ptr::null_mut();
            let r = unsafe { (self.f.event_input)(self.handle, &mut ev) };
            if r < 0 || ev.is_null() {
                // -EAGAIN: nothing left. Other errors (e.g. -ENOSPC on overrun) also end the
                // batch; the next poll picks up again.
                return None;
            }
            let ev = unsafe { *ev };
            match ev.type_ {
                EV_CLIENT_START | EV_CLIENT_EXIT | EV_PORT_START | EV_PORT_EXIT => {
                    return Some(SeqInput::Topology)
                }
                _ => {
                    if let Some(msg) = event_to_midi(&ev) {
                        return Some(SeqInput::Midi(msg));
                    }
                }
            }
        }
    }

    fn connect(&self, client: i32, port: i32, on: bool) -> bool {
        let f = if on { self.f.connect_from } else { self.f.disconnect_from };
        unsafe { f(self.handle, self.port, client, port) >= 0 }
    }

    /// Every port another client offers for reading (MIDI sources).
    fn list_inputs(&self) -> Vec<PortInfo> {
        let mut out = Vec::new();
        unsafe {
            let f = &self.f;
            let (mut ci, mut pi) = (std::ptr::null_mut(), std::ptr::null_mut());
            if (f.client_info_malloc)(&mut ci) < 0 || (f.port_info_malloc)(&mut pi) < 0 {
                return out;
            }
            (f.client_info_set_client)(ci, -1);
            while (f.query_next_client)(self.handle, ci) >= 0 {
                let client = (f.client_info_get_client)(ci);
                if client == CLIENT_SYSTEM || client == self.me {
                    continue;
                }
                let cname = crate::clap_host::cstr_to_string((f.client_info_get_name)(ci));
                (f.port_info_set_client)(pi, client);
                (f.port_info_set_port)(pi, -1);
                while (f.query_next_port)(self.handle, pi) >= 0 {
                    let caps = (f.port_info_get_capability)(pi);
                    let ty = (f.port_info_get_type)(pi);
                    if caps & (CAP_READ | CAP_SUBS_READ) != (CAP_READ | CAP_SUBS_READ) || caps & CAP_NO_EXPORT != 0 {
                        continue;
                    }
                    let pname = crate::clap_host::cstr_to_string((f.port_info_get_name)(pi));
                    let through = cname.contains("Midi Through");
                    out.push(PortInfo {
                        client,
                        port: (f.port_info_get_port)(pi),
                        name: format!("{cname}: {pname}"),
                        hardware: ty & TYPE_HARDWARE != 0 && !through,
                        connected: false,
                    });
                }
            }
            (f.client_info_free)(ci);
            (f.port_info_free)(pi);
        }
        out
    }
}

impl Drop for Seq {
    fn drop(&mut self) {
        unsafe {
            (self.f.close)(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(type_: u8, data: [u8; 12]) -> SeqEvent {
        SeqEvent { type_, flags: 0, tag: 0, queue: 0, time: [0; 2], source: [0; 2], dest: [0; 2], data }
    }

    #[test]
    fn converts_sequencer_events() {
        assert_eq!(event_to_midi(&ev(EV_NOTEON, [2, 60, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0])), Some([0x92, 60, 100]));
        let mut d = [0u8; 12];
        d[0] = 1;
        d[4..8].copy_from_slice(&64u32.to_ne_bytes());
        d[8..12].copy_from_slice(&127i32.to_ne_bytes());
        assert_eq!(event_to_midi(&ev(EV_CONTROLLER, d)), Some([0xb1, 64, 127]));
        let mut d = [0u8; 12];
        d[8..12].copy_from_slice(&(-8192i32).to_ne_bytes());
        assert_eq!(event_to_midi(&ev(EV_PITCHBEND, d)), Some([0xe0, 0, 0]));
        d[8..12].copy_from_slice(&0i32.to_ne_bytes());
        assert_eq!(event_to_midi(&ev(EV_PITCHBEND, d)), Some([0xe0, 0, 64]));
        assert_eq!(event_to_midi(&ev(EV_PORT_START, [0; 12])), None);
        assert_eq!(std::mem::size_of::<SeqEvent>(), 28);
    }
}

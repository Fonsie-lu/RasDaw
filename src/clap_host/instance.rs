//! One live plugin instance: lifecycle, audio processing, parameters, state and GUI calls.

use super::events::{InputEvents, OutputEvents};
use super::host::HostData;
use super::scan::PluginDesc;
use super::chars_to_string;
use crate::engine::NoteEvent;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::clap_event_transport;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::gui::*;
use clap_sys::ext::params::*;
use clap_sys::ext::posix_fd_support::*;
use clap_sys::ext::state::*;
use clap_sys::ext::timer_support::*;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{clap_process, CLAP_PROCESS_ERROR};
use clap_sys::stream::{clap_istream, clap_ostream};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct ParamInfo {
    pub id: clap_id,
    pub name: String,
    pub module: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub flags: u32,
}

impl ParamInfo {
    pub fn is_stepped(&self) -> bool {
        self.flags & CLAP_PARAM_IS_STEPPED != 0
    }
}

struct Ext {
    params: *const clap_plugin_params,
    gui: *const clap_plugin_gui,
    state: *const clap_plugin_state,
    audio_ports: *const clap_plugin_audio_ports,
    timer: *const clap_plugin_timer_support,
    fd: *const clap_plugin_posix_fd_support,
}

struct Port {
    bufs: Vec<Vec<f32>>,
    ptrs: Vec<*mut f32>,
}

impl Port {
    fn new(channels: u32, frames: usize) -> Self {
        let mut bufs: Vec<Vec<f32>> = (0..channels.max(1)).map(|_| vec![0.0; frames]).collect();
        let ptrs = bufs.iter_mut().map(|b| b.as_mut_ptr()).collect();
        Self { bufs, ptrs }
    }
}

/// Everything `process` touches. Only the audio thread uses it once the instance is shared.
struct AudioSide {
    inputs: Vec<Port>,
    outputs: Vec<Port>,
    in_bufs: Vec<clap_audio_buffer>,
    out_bufs: Vec<clap_audio_buffer>,
    events: InputEvents,
    out_events: OutputEvents,
    param_rx: rtrb::Consumer<(clap_id, f64)>,
    steady_time: i64,
    max_frames: usize,
}


pub struct PluginInstance {
    pub uid: u64,
    pub desc: PluginDesc,
    plugin: *const clap_plugin,
    host: Box<HostData>,
    ext: Ext,
    audio: UnsafeCell<Box<AudioSide>>,
    processing: AtomicBool,
    active: AtomicBool,
    gui_created: AtomicBool,
    param_tx: Mutex<rtrb::Producer<(clap_id, f64)>>,
    param_out_rx: Mutex<rtrb::Consumer<(clap_id, f64)>>,
    params: Mutex<Vec<ParamInfo>>,
}

// See the threading contract in `clap_host/mod.rs`.
unsafe impl Send for PluginInstance {}
unsafe impl Sync for PluginInstance {}

impl PluginInstance {
    /// Creates, initialises and activates a plugin. Main thread only.
    pub fn new(desc: &PluginDesc, sample_rate: f64, max_frames: u32) -> Result<Arc<Self>, String> {
        let host = HostData::new(desc.name.clone());
        let id = CString::new(desc.id.as_str()).map_err(|e| e.to_string())?;
        let plugin = unsafe {
            let f = &*desc.library.factory;
            let create = f.create_plugin.ok_or("factory has no create_plugin")?;
            create(desc.library.factory, &host.clap, id.as_ptr())
        };
        if plugin.is_null() {
            return Err(format!("{}: create_plugin failed", desc.name));
        }
        let (param_tx, param_rx) = rtrb::RingBuffer::new(1024);
        let (out_tx, out_rx) = rtrb::RingBuffer::new(1024);
        let mut inst = Self {
            uid: super::next_uid(),
            desc: desc.clone(),
            plugin,
            host,
            ext: Ext {
                params: std::ptr::null(),
                gui: std::ptr::null(),
                state: std::ptr::null(),
                audio_ports: std::ptr::null(),
                timer: std::ptr::null(),
                fd: std::ptr::null(),
            },
            audio: UnsafeCell::new(Box::new(AudioSide {
                inputs: Vec::new(),
                outputs: Vec::new(),
                in_bufs: Vec::new(),
                out_bufs: Vec::new(),
                events: InputEvents::with_capacity(4096),
                out_events: OutputEvents::new(out_tx),
                param_rx,
                steady_time: 0,
                max_frames: max_frames as usize,
            })),
            processing: AtomicBool::new(false),
            active: AtomicBool::new(false),
            gui_created: AtomicBool::new(false),
            param_tx: Mutex::new(param_tx),
            param_out_rx: Mutex::new(out_rx),
            params: Mutex::new(Vec::new()),
        };
        unsafe {
            let init = (*plugin).init.ok_or("plugin has no init")?;
            if !init(plugin) {
                // `Drop` destroys it; nothing else was set up yet.
                return Err(format!("{}: init failed", desc.name));
            }
            inst.ext = Ext {
                params: inst.extension(CLAP_EXT_PARAMS),
                gui: inst.extension(CLAP_EXT_GUI),
                state: inst.extension(CLAP_EXT_STATE),
                audio_ports: inst.extension(CLAP_EXT_AUDIO_PORTS),
                timer: inst.extension(CLAP_EXT_TIMER_SUPPORT),
                fd: inst.extension(CLAP_EXT_POSIX_FD_SUPPORT),
            };
            inst.activate(sample_rate, max_frames)?;
        }
        inst.rescan_params();
        Ok(Arc::new(inst))
    }

    unsafe fn extension<T>(&self, id: &CStr) -> *const T {
        match (*self.plugin).get_extension {
            Some(get) => get(self.plugin, id.as_ptr()) as *const T,
            None => std::ptr::null(),
        }
    }

    fn port_channels(&self, is_input: bool) -> Vec<u32> {
        let mut out = Vec::new();
        if self.ext.audio_ports.is_null() {
            return out;
        }
        unsafe {
            let ap = &*self.ext.audio_ports;
            let (Some(count), Some(get)) = (ap.count, ap.get) else {
                return out;
            };
            for i in 0..count(self.plugin, is_input) {
                let mut info: clap_audio_port_info = std::mem::zeroed();
                if get(self.plugin, i, is_input, &mut info) {
                    out.push(info.channel_count);
                }
            }
        }
        out
    }

    /// Allocates the audio buffers for the plugin's ports and activates it.
    unsafe fn activate(&mut self, sample_rate: f64, max_frames: u32) -> Result<(), String> {
        let ins = self.port_channels(true);
        let outs = self.port_channels(false);
        let audio = self.audio.get_mut();
        let frames = max_frames as usize;
        audio.inputs = ins.iter().map(|&c| Port::new(c, frames)).collect();
        audio.outputs = outs.iter().map(|&c| Port::new(c, frames)).collect();
        let make = |ports: &mut Vec<Port>, counts: &[u32]| -> Vec<clap_audio_buffer> {
            ports
                .iter_mut()
                .zip(counts)
                .map(|(p, &c)| clap_audio_buffer {
                    data32: p.ptrs.as_mut_ptr(),
                    data64: std::ptr::null_mut(),
                    channel_count: c,
                    latency: 0,
                    constant_mask: 0,
                })
                .collect()
        };
        audio.in_bufs = make(&mut audio.inputs, &ins);
        audio.out_bufs = make(&mut audio.outputs, &outs);
        let activate = (*self.plugin).activate.ok_or("plugin has no activate")?;
        if !activate(self.plugin, sample_rate, 1, max_frames) {
            return Err(format!("{}: activate failed", self.desc.name));
        }
        self.active.store(true, Ordering::Release);
        Ok(())
    }

    // ---------------------------------------------------------------- audio thread

    pub fn is_processing(&self) -> bool {
        self.processing.load(Ordering::Acquire)
    }

    /// Audio thread only.
    pub fn start_processing(&self) -> bool {
        if self.is_processing() {
            return true;
        }
        let ok = unsafe {
            match (*self.plugin).start_processing {
                Some(f) => f(self.plugin),
                None => true,
            }
        };
        self.processing.store(ok, Ordering::Release);
        ok
    }

    /// Audio thread only.
    pub fn stop_processing(&self) {
        if self.processing.swap(false, Ordering::AcqRel) {
            unsafe {
                if let Some(f) = (*self.plugin).stop_processing {
                    f(self.plugin);
                }
            }
        }
    }

    /// Runs one block. `input` and `output` are stereo; `notes` must be sorted by time. `midi`
    /// holds raw channel messages (CC, pitch bend, pressure) delivered at the block start.
    /// Returns false if the plugin could not process (the caller then passes audio through).
    /// Audio thread only.
    pub fn process(
        &self,
        input: [&[f32]; 2],
        output: [&mut [f32]; 2],
        frames: usize,
        notes: &[NoteEvent],
        midi: &[[u8; 3]],
        transport: &clap_event_transport,
    ) -> bool {
        if !self.start_processing() {
            return false;
        }
        let audio = unsafe { &mut **self.audio.get() };
        let frames = frames.min(audio.max_frames);
        if frames == 0 {
            return true;
        }
        audio.events.clear();
        while let Ok((id, value)) = audio.param_rx.pop() {
            audio.events.param(0, id, value);
        }
        for m in midi {
            audio.events.midi(0, *m);
        }
        for n in notes {
            audio
                .events
                .note(n.time.min(frames as u32 - 1), n.on, n.key, n.velocity as f64);
        }
        for (pi, port) in audio.inputs.iter_mut().enumerate() {
            let channels = port.bufs.len();
            for (ci, buf) in port.bufs.iter_mut().enumerate() {
                if pi == 0 {
                    let src = if channels == 1 {
                        // Mono input: downmix.
                        for (i, s) in buf[..frames].iter_mut().enumerate() {
                            *s = 0.5 * (input[0][i] + input[1][i]);
                        }
                        continue;
                    } else {
                        input[ci.min(1)]
                    };
                    buf[..frames].copy_from_slice(&src[..frames]);
                } else {
                    buf[..frames].fill(0.0);
                }
            }
        }
        let process = clap_process {
            steady_time: audio.steady_time,
            frames_count: frames as u32,
            transport,
            audio_inputs: audio.in_bufs.as_ptr(),
            audio_outputs: audio.out_bufs.as_mut_ptr(),
            audio_inputs_count: audio.in_bufs.len() as u32,
            audio_outputs_count: audio.out_bufs.len() as u32,
            in_events: audio.events.as_ptr(),
            out_events: audio.out_events.as_ptr(),
        };
        audio.steady_time += frames as i64;
        let status = unsafe {
            match (*self.plugin).process {
                Some(f) => f(self.plugin, &process),
                None => CLAP_PROCESS_ERROR,
            }
        };
        let [out_l, out_r] = output;
        match audio.outputs.first() {
            Some(port) if status != CLAP_PROCESS_ERROR => {
                let l = &port.bufs[0];
                let r = port.bufs.get(1).unwrap_or(l);
                out_l[..frames].copy_from_slice(&l[..frames]);
                out_r[..frames].copy_from_slice(&r[..frames]);
                true
            }
            _ => false,
        }
    }

    // ---------------------------------------------------------------- main thread

    /// Services host callbacks: `on_main_thread`, timers, file descriptors and param rescans.
    pub fn idle(&self) {
        let h = &self.host;
        unsafe {
            if h.callback_requested.swap(false, Ordering::AcqRel) {
                if let Some(f) = (*self.plugin).on_main_thread {
                    f(self.plugin);
                }
            }
            if !self.ext.timer.is_null() {
                let now = Instant::now();
                let due: Vec<clap_id> = {
                    let mut timers = h.timers.lock().unwrap();
                    timers
                        .iter_mut()
                        .filter(|t| t.next <= now)
                        .map(|t| {
                            t.next = now + t.period;
                            t.id
                        })
                        .collect()
                };
                if let Some(on_timer) = (*self.ext.timer).on_timer {
                    for id in due {
                        on_timer(self.plugin, id);
                    }
                }
            }
            if !self.ext.fd.is_null() {
                let fds: Vec<(i32, u32)> = h.fds.lock().unwrap().clone();
                if !fds.is_empty() {
                    if let Some(on_fd) = (*self.ext.fd).on_fd {
                        for (fd, flags) in poll_fds(&fds) {
                            on_fd(self.plugin, fd, flags);
                        }
                    }
                }
            }
        }
        if h.params_rescan.swap(0, Ordering::AcqRel) != 0 {
            self.rescan_params();
        }
        if h.restart_requested.swap(false, Ordering::AcqRel) {
            eprintln!("[{}] requested a restart; not supported yet", self.desc.name);
        }
        // Values the plugin reported are re-read through `param_value`, so just drain them.
        if let Ok(mut rx) = self.param_out_rx.try_lock() {
            while rx.pop().is_ok() {}
        }
    }

    pub fn rescan_params(&self) {
        let mut list = Vec::new();
        if !self.ext.params.is_null() {
            unsafe {
                let p = &*self.ext.params;
                if let (Some(count), Some(get_info)) = (p.count, p.get_info) {
                    for i in 0..count(self.plugin) {
                        let mut info: clap_param_info = std::mem::zeroed();
                        if get_info(self.plugin, i, &mut info)
                            && info.flags & CLAP_PARAM_IS_HIDDEN == 0
                        {
                            list.push(ParamInfo {
                                id: info.id,
                                name: chars_to_string(&info.name),
                                module: chars_to_string(&info.module),
                                min: info.min_value,
                                max: info.max_value,
                                default: info.default_value,
                                flags: info.flags,
                            });
                        }
                    }
                }
            }
        }
        *self.params.lock().unwrap() = list;
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        self.params.lock().unwrap().clone()
    }

    pub fn param_value(&self, id: clap_id) -> Option<f64> {
        if self.ext.params.is_null() {
            return None;
        }
        unsafe {
            let get = (*self.ext.params).get_value?;
            let mut v = 0.0;
            get(self.plugin, id, &mut v).then_some(v)
        }
    }

    pub fn param_text(&self, id: clap_id, value: f64) -> Option<String> {
        if self.ext.params.is_null() {
            return None;
        }
        unsafe {
            let to_text = (*self.ext.params).value_to_text?;
            let mut buf = [0 as c_char; 128];
            to_text(self.plugin, id, value, buf.as_mut_ptr(), buf.len() as u32)
                .then(|| chars_to_string(&buf))
        }
    }

    /// Queues a parameter change; the engine delivers it with the next `process` call.
    pub fn set_param(&self, id: clap_id, value: f64) {
        let _ = self.param_tx.lock().unwrap().push((id, value));
    }

    pub fn save_state(&self) -> Option<Vec<u8>> {
        if self.ext.state.is_null() {
            return None;
        }
        unsafe {
            let save = (*self.ext.state).save?;
            let mut buf: Vec<u8> = Vec::new();
            let stream = clap_ostream {
                ctx: &mut buf as *mut Vec<u8> as *mut c_void,
                write: Some(ostream_write),
            };
            save(self.plugin, &stream).then_some(buf)
        }
    }

    pub fn load_state(&self, data: &[u8]) -> bool {
        if self.ext.state.is_null() {
            return false;
        }
        unsafe {
            let Some(load) = (*self.ext.state).load else {
                return false;
            };
            let mut reader = (data, 0usize);
            let stream = clap_istream {
                ctx: &mut reader as *mut (&[u8], usize) as *mut c_void,
                read: Some(istream_read),
            };
            let ok = load(self.plugin, &stream);
            if ok {
                self.rescan_params();
            }
            ok
        }
    }

    // ---- GUI (main thread). Used by `gui::GuiManager`.

    pub fn has_gui(&self) -> bool {
        !self.ext.gui.is_null()
    }

    fn gui(&self) -> Option<&clap_plugin_gui> {
        unsafe { self.ext.gui.as_ref() }
    }

    pub fn gui_is_api_supported(&self, floating: bool) -> bool {
        self.gui()
            .and_then(|g| g.is_api_supported)
            .is_some_and(|f| unsafe { f(self.plugin, CLAP_WINDOW_API_X11.as_ptr(), floating) })
    }

    pub fn gui_create(&self, floating: bool) -> bool {
        let ok = self
            .gui()
            .and_then(|g| g.create)
            .is_some_and(|f| unsafe { f(self.plugin, CLAP_WINDOW_API_X11.as_ptr(), floating) });
        self.gui_created.store(ok, Ordering::Release);
        ok
    }

    pub fn gui_destroy(&self) {
        if self.gui_created.swap(false, Ordering::AcqRel) {
            if let Some(f) = self.gui().and_then(|g| g.destroy) {
                unsafe { f(self.plugin) }
            }
        }
    }

    pub fn gui_set_scale(&self, scale: f64) -> bool {
        self.gui()
            .and_then(|g| g.set_scale)
            .is_some_and(|f| unsafe { f(self.plugin, scale) })
    }

    pub fn gui_get_size(&self) -> Option<(u32, u32)> {
        let f = self.gui()?.get_size?;
        let (mut w, mut h) = (0, 0);
        unsafe { f(self.plugin, &mut w, &mut h) }.then_some((w, h))
    }

    pub fn gui_can_resize(&self) -> bool {
        self.gui()
            .and_then(|g| g.can_resize)
            .is_some_and(|f| unsafe { f(self.plugin) })
    }

    pub fn gui_adjust_size(&self, w: u32, h: u32) -> (u32, u32) {
        let (mut w2, mut h2) = (w, h);
        match self.gui().and_then(|g| g.adjust_size) {
            Some(f) if unsafe { f(self.plugin, &mut w2, &mut h2) } => (w2, h2),
            _ => (w, h),
        }
    }

    pub fn gui_set_size(&self, w: u32, h: u32) -> bool {
        self.gui()
            .and_then(|g| g.set_size)
            .is_some_and(|f| unsafe { f(self.plugin, w, h) })
    }

    pub fn gui_set_parent(&self, xid: u32) -> bool {
        let window = clap_window {
            api: CLAP_WINDOW_API_X11.as_ptr(),
            specific: clap_window_handle { x11: xid as _ },
        };
        self.gui()
            .and_then(|g| g.set_parent)
            .is_some_and(|f| unsafe { f(self.plugin, &window) })
    }

    pub fn gui_suggest_title(&self, title: &str) {
        if let Some(f) = self.gui().and_then(|g| g.suggest_title) {
            let t = CString::new(title).unwrap_or_default();
            unsafe { f(self.plugin, t.as_ptr()) }
        }
    }

    pub fn gui_show(&self) -> bool {
        self.gui()
            .and_then(|g| g.show)
            .is_some_and(|f| unsafe { f(self.plugin) })
    }

    pub fn gui_hide(&self) -> bool {
        self.gui()
            .and_then(|g| g.hide)
            .is_some_and(|f| unsafe { f(self.plugin) })
    }

    pub fn take_gui_resize_request(&self) -> Option<(u32, u32)> {
        self.host.gui_resize.lock().unwrap().take()
    }

    /// `Some(was_destroyed)` once the plugin reported that its floating window was closed.
    pub fn take_gui_closed(&self) -> Option<bool> {
        self.host.gui_closed.lock().unwrap().take()
    }

}

impl Drop for PluginInstance {
    fn drop(&mut self) {
        unsafe {
            self.gui_destroy();
            // Normally the engine already stopped processing on the audio thread.
            self.stop_processing();
            if self.active.swap(false, Ordering::AcqRel) {
                if let Some(f) = (*self.plugin).deactivate {
                    f(self.plugin);
                }
            }
            if let Some(f) = (*self.plugin).destroy {
                f(self.plugin);
            }
        }
    }
}

unsafe extern "C" fn ostream_write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    let buf = &mut *((*stream).ctx as *mut Vec<u8>);
    buf.extend_from_slice(std::slice::from_raw_parts(buffer as *const u8, size as usize));
    size as i64
}

unsafe extern "C" fn istream_read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    let (data, pos) = &mut *((*stream).ctx as *mut (&[u8], usize));
    let n = (data.len() - *pos).min(size as usize);
    std::ptr::copy_nonoverlapping(data.as_ptr().add(*pos), buffer as *mut u8, n);
    *pos += n;
    n as i64
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

/// Non-blocking poll of the plugin's registered file descriptors.
fn poll_fds(fds: &[(i32, u32)]) -> Vec<(i32, u32)> {
    const POLLIN: i16 = 0x1;
    const POLLOUT: i16 = 0x4;
    const POLLERR: i16 = 0x8;
    let mut pfds: Vec<PollFd> = fds
        .iter()
        .map(|&(fd, flags)| PollFd {
            fd,
            events: if flags & CLAP_POSIX_FD_READ != 0 { POLLIN } else { 0 }
                | if flags & CLAP_POSIX_FD_WRITE != 0 { POLLOUT } else { 0 },
            revents: 0,
        })
        .collect();
    let n = unsafe { poll(pfds.as_mut_ptr(), pfds.len() as _, 0) };
    if n <= 0 {
        return Vec::new();
    }
    pfds.iter()
        .filter(|p| p.revents != 0)
        .map(|p| {
            let mut flags = 0;
            if p.revents & POLLIN != 0 {
                flags |= CLAP_POSIX_FD_READ;
            }
            if p.revents & POLLOUT != 0 {
                flags |= CLAP_POSIX_FD_WRITE;
            }
            if p.revents & POLLERR != 0 {
                flags |= CLAP_POSIX_FD_ERROR;
            }
            (p.fd, flags)
        })
        .collect()
}

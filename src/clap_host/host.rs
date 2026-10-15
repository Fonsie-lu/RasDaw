//! The `clap_host` struct handed to each plugin instance, and the host extensions.
//!
//! Plugins may call these callbacks from any thread, so they only set flags or push into
//! mutex-protected lists. The main thread acts on them in `PluginInstance::idle`.

use clap_sys::ext::audio_ports::{clap_host_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::gui::{clap_host_gui, CLAP_EXT_GUI};
use clap_sys::ext::latency::{clap_host_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::log::*;
use clap_sys::ext::note_ports::{
    clap_host_note_ports, clap_note_dialect, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI,
};
use clap_sys::ext::params::{
    clap_host_params, clap_param_clear_flags, clap_param_rescan_flags, CLAP_EXT_PARAMS,
};
use clap_sys::ext::posix_fd_support::{
    clap_host_posix_fd_support, clap_posix_fd_flags, CLAP_EXT_POSIX_FD_SUPPORT,
};
use clap_sys::ext::state::{clap_host_state, CLAP_EXT_STATE};
use clap_sys::ext::thread_check::{clap_host_thread_check, CLAP_EXT_THREAD_CHECK};
use clap_sys::ext::timer_support::{clap_host_timer_support, CLAP_EXT_TIMER_SUPPORT};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::version::CLAP_VERSION;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Timer {
    pub id: clap_id,
    pub period: Duration,
    pub next: Instant,
}

/// Per-instance host state. Boxed so the `clap_host` inside keeps a stable address.
pub struct HostData {
    pub clap: clap_host,
    pub plugin_name: String,
    pub callback_requested: AtomicBool,
    pub restart_requested: AtomicBool,
    pub params_rescan: AtomicU32,
    pub state_dirty: AtomicBool,
    pub gui_resize: Mutex<Option<(u32, u32)>>,
    pub gui_closed: Mutex<Option<bool>>,
    pub timers: Mutex<Vec<Timer>>,
    next_timer_id: AtomicU32,
    pub fds: Mutex<Vec<(i32, clap_posix_fd_flags)>>,
}

const HOST_NAME: &CStr = c"RasDaw";
const HOST_VENDOR: &CStr = c"Fonsie Audio";
const HOST_URL: &CStr = c"";
const HOST_VERSION: &CStr = c"0.1.0";

impl HostData {
    pub fn new(plugin_name: String) -> Box<Self> {
        let mut data = Box::new(Self {
            clap: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: HOST_NAME.as_ptr(),
                vendor: HOST_VENDOR.as_ptr(),
                url: HOST_URL.as_ptr(),
                version: HOST_VERSION.as_ptr(),
                get_extension: Some(get_extension),
                request_restart: Some(request_restart),
                request_process: Some(request_process),
                request_callback: Some(request_callback),
            },
            plugin_name,
            callback_requested: AtomicBool::new(false),
            restart_requested: AtomicBool::new(false),
            params_rescan: AtomicU32::new(0),
            state_dirty: AtomicBool::new(false),
            gui_resize: Mutex::new(None),
            gui_closed: Mutex::new(None),
            timers: Mutex::new(Vec::new()),
            next_timer_id: AtomicU32::new(1),
            fds: Mutex::new(Vec::new()),
        });
        let ptr: *mut HostData = &mut *data;
        data.clap.host_data = ptr as *mut c_void;
        data
    }
}

unsafe fn data<'a>(host: *const clap_host) -> Option<&'a HostData> {
    if host.is_null() || (*host).host_data.is_null() {
        return None;
    }
    Some(&*((*host).host_data as *const HostData))
}

unsafe extern "C" fn get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_LOG {
        &HOST_LOG as *const _ as *const c_void
    } else if id == CLAP_EXT_THREAD_CHECK {
        &HOST_THREAD_CHECK as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI {
        &HOST_GUI as *const _ as *const c_void
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        &HOST_TIMER as *const _ as *const c_void
    } else if id == CLAP_EXT_POSIX_FD_SUPPORT {
        &HOST_FD as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &HOST_LATENCY as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &HOST_AUDIO_PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_NOTE_PORTS {
        &HOST_NOTE_PORTS as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn request_restart(host: *const clap_host) {
    if let Some(d) = data(host) {
        d.restart_requested.store(true, Ordering::Release);
    }
}

/// The engine processes every instance continuously, so there is nothing to wake up.
unsafe extern "C" fn request_process(_host: *const clap_host) {}

unsafe extern "C" fn request_callback(host: *const clap_host) {
    if let Some(d) = data(host) {
        d.callback_requested.store(true, Ordering::Release);
    }
}

// ---- log ----

static HOST_LOG: clap_host_log = clap_host_log { log: Some(log) };

unsafe extern "C" fn log(host: *const clap_host, severity: clap_log_severity, msg: *const c_char) {
    let name = data(host).map(|d| d.plugin_name.as_str()).unwrap_or("?");
    let level = match severity {
        CLAP_LOG_DEBUG => "debug",
        CLAP_LOG_INFO => "info",
        CLAP_LOG_WARNING => "warn",
        CLAP_LOG_ERROR => "error",
        CLAP_LOG_FATAL => "fatal",
        CLAP_LOG_HOST_MISBEHAVING => "host misbehaving",
        CLAP_LOG_PLUGIN_MISBEHAVING => "plugin misbehaving",
        _ => "log",
    };
    // Logging from the audio thread allocates; plugins only do that for errors.
    eprintln!("[{name}] {level}: {}", super::cstr_to_string(msg));
}

// ---- thread check ----

static HOST_THREAD_CHECK: clap_host_thread_check = clap_host_thread_check {
    is_main_thread: Some(is_main_thread),
    is_audio_thread: Some(is_audio_thread),
};

unsafe extern "C" fn is_main_thread(_host: *const clap_host) -> bool {
    super::is_main_thread()
}

unsafe extern "C" fn is_audio_thread(_host: *const clap_host) -> bool {
    super::is_audio_thread()
}

// ---- params ----

static HOST_PARAMS: clap_host_params = clap_host_params {
    rescan: Some(params_rescan),
    clear: Some(params_clear),
    request_flush: Some(params_request_flush),
};

unsafe extern "C" fn params_rescan(host: *const clap_host, flags: clap_param_rescan_flags) {
    if let Some(d) = data(host) {
        d.params_rescan.fetch_or(flags | 0x8000_0000, Ordering::AcqRel);
    }
}

unsafe extern "C" fn params_clear(_host: *const clap_host, _id: clap_id, _flags: clap_param_clear_flags) {}

/// Parameter changes are flushed by the next `process` call, which always comes soon.
unsafe extern "C" fn params_request_flush(_host: *const clap_host) {}

// ---- gui ----

static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(gui_resize_hints_changed),
    request_resize: Some(gui_request_resize),
    request_show: Some(gui_request_show),
    request_hide: Some(gui_request_hide),
    closed: Some(gui_closed),
};

unsafe extern "C" fn gui_resize_hints_changed(_host: *const clap_host) {}

unsafe extern "C" fn gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    match data(host) {
        Some(d) => {
            *d.gui_resize.lock().unwrap() = Some((width, height));
            true
        }
        None => false,
    }
}

unsafe extern "C" fn gui_request_show(_host: *const clap_host) -> bool {
    false
}

unsafe extern "C" fn gui_request_hide(_host: *const clap_host) -> bool {
    false
}

unsafe extern "C" fn gui_closed(host: *const clap_host, was_destroyed: bool) {
    if let Some(d) = data(host) {
        *d.gui_closed.lock().unwrap() = Some(was_destroyed);
    }
}

// ---- timer support ----

static HOST_TIMER: clap_host_timer_support = clap_host_timer_support {
    register_timer: Some(register_timer),
    unregister_timer: Some(unregister_timer),
};

unsafe extern "C" fn register_timer(host: *const clap_host, period_ms: u32, id: *mut clap_id) -> bool {
    let Some(d) = data(host) else {
        return false;
    };
    let new_id = d.next_timer_id.fetch_add(1, Ordering::Relaxed);
    let period = Duration::from_millis(period_ms.max(15) as u64);
    d.timers.lock().unwrap().push(Timer {
        id: new_id,
        period,
        next: Instant::now() + period,
    });
    if !id.is_null() {
        *id = new_id;
    }
    true
}

unsafe extern "C" fn unregister_timer(host: *const clap_host, id: clap_id) -> bool {
    let Some(d) = data(host) else {
        return false;
    };
    let mut timers = d.timers.lock().unwrap();
    let before = timers.len();
    timers.retain(|t| t.id != id);
    timers.len() != before
}

// ---- posix fd support ----

static HOST_FD: clap_host_posix_fd_support = clap_host_posix_fd_support {
    register_fd: Some(register_fd),
    modify_fd: Some(modify_fd),
    unregister_fd: Some(unregister_fd),
};

unsafe extern "C" fn register_fd(host: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
    let Some(d) = data(host) else {
        return false;
    };
    let mut fds = d.fds.lock().unwrap();
    fds.retain(|(f, _)| *f != fd);
    fds.push((fd, flags));
    true
}

unsafe extern "C" fn modify_fd(host: *const clap_host, fd: i32, flags: clap_posix_fd_flags) -> bool {
    let Some(d) = data(host) else {
        return false;
    };
    let mut fds = d.fds.lock().unwrap();
    match fds.iter_mut().find(|(f, _)| *f == fd) {
        Some(e) => {
            e.1 = flags;
            true
        }
        None => false,
    }
}

unsafe extern "C" fn unregister_fd(host: *const clap_host, fd: i32) -> bool {
    let Some(d) = data(host) else {
        return false;
    };
    let mut fds = d.fds.lock().unwrap();
    let before = fds.len();
    fds.retain(|(f, _)| *f != fd);
    fds.len() != before
}

// ---- state, latency, ports ----

static HOST_STATE: clap_host_state = clap_host_state {
    mark_dirty: Some(state_mark_dirty),
};

unsafe extern "C" fn state_mark_dirty(host: *const clap_host) {
    if let Some(d) = data(host) {
        d.state_dirty.store(true, Ordering::Release);
    }
}

static HOST_LATENCY: clap_host_latency = clap_host_latency {
    changed: Some(latency_changed),
};

/// No latency compensation yet.
unsafe extern "C" fn latency_changed(_host: *const clap_host) {}

static HOST_AUDIO_PORTS: clap_host_audio_ports = clap_host_audio_ports {
    is_rescan_flag_supported: Some(audio_ports_flag_supported),
    rescan: Some(audio_ports_rescan),
};

unsafe extern "C" fn audio_ports_flag_supported(_host: *const clap_host, _flag: u32) -> bool {
    false
}

unsafe extern "C" fn audio_ports_rescan(_host: *const clap_host, _flags: u32) {}

static HOST_NOTE_PORTS: clap_host_note_ports = clap_host_note_ports {
    supported_dialects: Some(note_dialects),
    rescan: Some(note_ports_rescan),
};

unsafe extern "C" fn note_dialects(_host: *const clap_host) -> clap_note_dialect {
    CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI
}

unsafe extern "C" fn note_ports_rescan(_host: *const clap_host, _flags: u32) {}

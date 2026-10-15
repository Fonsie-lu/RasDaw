//! Audio output. The system libraries are loaded at runtime with `dlopen`, so building needs no
//! audio headers. Tried in order:
//! 1. PulseAudio simple API (`libpulse-simple.so.0`), which PipeWire serves via pipewire-pulse.
//! 2. ALSA (`libasound.so.2`).
//! 3. A null driver that renders at real-time pace without output.
//!
//! Environment: `DAW_AUDIO=pulse|alsa|null` forces a driver, `DAW_ALSA_DEVICE` picks the ALSA
//! device (default `default`).

use crate::engine::Engine;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 48_000;
/// Largest block the engine processes at once; plugins are activated with this.
pub const MAX_BLOCK: u32 = 1024;

pub enum Backend {
    Pulse(Pulse),
    Alsa(Alsa),
    Null { period: usize },
}

pub struct DriverInfo {
    pub name: String,
    pub sample_rate: u32,
    pub period: usize,
}

/// Opens the output device. Returns the backend and what it negotiated.
pub fn open() -> (Backend, DriverInfo) {
    let forced = std::env::var("DAW_AUDIO").unwrap_or_default();
    let mut errors = Vec::new();
    if forced.is_empty() || forced == "pulse" {
        match Pulse::open(SAMPLE_RATE) {
            Ok(pulse) => {
                let info = DriverInfo {
                    name: "PulseAudio".into(),
                    sample_rate: SAMPLE_RATE,
                    period: pulse.period,
                };
                return (Backend::Pulse(pulse), info);
            }
            Err(e) => errors.push(e),
        }
    }
    if forced.is_empty() || forced == "alsa" {
        let device = std::env::var("DAW_ALSA_DEVICE").unwrap_or_else(|_| "default".into());
        match Alsa::open(&device, SAMPLE_RATE) {
            Ok(alsa) => {
                let info = DriverInfo {
                    name: format!("ALSA {device}"),
                    sample_rate: SAMPLE_RATE,
                    period: alsa.period,
                };
                return (Backend::Alsa(alsa), info);
            }
            Err(e) => errors.push(e),
        }
    }
    for e in &errors {
        eprintln!("audio: {e}");
    }
    null(if forced == "null" { "null (DAW_AUDIO=null)".into() } else { "null (no audio device)".into() })
}

fn null(name: String) -> (Backend, DriverInfo) {
    let period = 512;
    (
        Backend::Null { period },
        DriverInfo {
            name,
            sample_rate: SAMPLE_RATE,
            period,
        },
    )
}

/// The running audio thread. Dropping it stops the thread (and drops the engine there).
pub struct Stream {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start(backend: Backend, mut engine: Engine) -> Stream {
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let thread = std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            crate::clap_host::mark_audio_thread();
            match backend {
                Backend::Pulse(pulse) => pulse.run(&mut engine, &stop2),
                Backend::Alsa(alsa) => alsa.run(&mut engine, &stop2),
                Backend::Null { period } => run_null(period, &mut engine, &stop2),
            }
            drop(engine);
        })
        .expect("spawn audio thread");
    Stream {
        stop,
        thread: Some(thread),
    }
}

fn run_null(period: usize, engine: &mut Engine, stop: &AtomicBool) {
    let mut buf = vec![0.0f32; period * 2];
    let start = Instant::now();
    let mut frames: u64 = 0;
    while !stop.load(Ordering::Acquire) {
        engine.render_interleaved(&mut buf);
        frames += period as u64;
        let due = Duration::from_secs_f64(frames as f64 / SAMPLE_RATE as f64);
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
}

// ---------------------------------------------------------------- ALSA via dlopen

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;

type PcmOpen = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type PcmSetParams =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type PcmGetParams = unsafe extern "C" fn(*mut c_void, *mut c_ulong, *mut c_ulong) -> c_int;
type PcmWritei = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type PcmRecover = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type PcmClose = unsafe extern "C" fn(*mut c_void) -> c_int;
type StrError = unsafe extern "C" fn(c_int) -> *const c_char;

pub struct Alsa {
    _lib: libloading::Library,
    pcm: *mut c_void,
    float: bool,
    period: usize,
    writei: PcmWritei,
    recover: PcmRecover,
    close: PcmClose,
}

// The PCM handle is only used by the audio thread after `open`.
unsafe impl Send for Alsa {}

impl Alsa {
    fn open(device: &str, rate: u32) -> Result<Self, String> {
        unsafe {
            let lib = libloading::Library::new("libasound.so.2").map_err(|e| e.to_string())?;
            macro_rules! sym {
                ($name:literal, $ty:ty) => {
                    *lib.get::<$ty>($name).map_err(|e| e.to_string())?
                };
            }
            let pcm_open = sym!(b"snd_pcm_open\0", PcmOpen);
            let set_params = sym!(b"snd_pcm_set_params\0", PcmSetParams);
            let get_params = sym!(b"snd_pcm_get_params\0", PcmGetParams);
            let writei = sym!(b"snd_pcm_writei\0", PcmWritei);
            let recover = sym!(b"snd_pcm_recover\0", PcmRecover);
            let close = sym!(b"snd_pcm_close\0", PcmClose);
            let strerror = sym!(b"snd_strerror\0", StrError);
            let msg = |e: c_int| crate::clap_host::cstr_to_string(strerror(e));

            let name = CString::new(device).map_err(|e| e.to_string())?;
            let mut pcm: *mut c_void = std::ptr::null_mut();
            let e = pcm_open(&mut pcm, name.as_ptr(), SND_PCM_STREAM_PLAYBACK, 0);
            if e < 0 {
                return Err(format!("snd_pcm_open({device}): {}", msg(e)));
            }
            // 10 ms requested latency; ALSA resamples if the device runs at another rate.
            let mut float = true;
            let mut e = set_params(pcm, SND_PCM_FORMAT_FLOAT_LE, SND_PCM_ACCESS_RW_INTERLEAVED, 2, rate, 1, 10_000);
            if e < 0 {
                float = false;
                e = set_params(pcm, SND_PCM_FORMAT_S16_LE, SND_PCM_ACCESS_RW_INTERLEAVED, 2, rate, 1, 10_000);
            }
            if e < 0 {
                close(pcm);
                return Err(format!("snd_pcm_set_params: {}", msg(e)));
            }
            let (mut buffer, mut period) = (0 as c_ulong, 0 as c_ulong);
            get_params(pcm, &mut buffer, &mut period);
            let period = (period as usize).clamp(32, MAX_BLOCK as usize);
            Ok(Self {
                _lib: lib,
                pcm,
                float,
                period,
                writei,
                recover,
                close,
            })
        }
    }

    fn run(self, engine: &mut Engine, stop: &AtomicBool) {
        let mut buf = vec![0.0f32; self.period * 2];
        let mut buf16 = vec![0i16; self.period * 2];
        while !stop.load(Ordering::Acquire) {
            engine.render_interleaved(&mut buf);
            let ptr = if self.float {
                buf.as_ptr() as *const c_void
            } else {
                for (d, s) in buf16.iter_mut().zip(&buf) {
                    *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                }
                buf16.as_ptr() as *const c_void
            };
            let mut written = 0usize;
            while written < self.period {
                let frame_bytes = if self.float { 8 } else { 4 };
                let r = unsafe {
                    (self.writei)(
                        self.pcm,
                        (ptr as *const u8).add(written * frame_bytes) as *const c_void,
                        (self.period - written) as c_ulong,
                    )
                };
                if r < 0 {
                    // Underrun or suspend: recover and retry this period.
                    if unsafe { (self.recover)(self.pcm, r as c_int, 1) } < 0 {
                        eprintln!("audio: ALSA write failed ({r}); stopping output");
                        return;
                    }
                } else {
                    written += r as usize;
                }
            }
        }
    }
}

impl Drop for Alsa {
    fn drop(&mut self) {
        unsafe {
            (self.close)(self.pcm);
        }
    }
}

// ---------------------------------------------------------------- PulseAudio simple API via dlopen

const PA_STREAM_PLAYBACK: c_int = 1;
const PA_SAMPLE_FLOAT32LE: c_int = 5;

#[repr(C)]
struct PaSampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct PaBufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

type PaSimpleNew = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    c_int,
    *const c_char,
    *const c_char,
    *const PaSampleSpec,
    *const c_void,
    *const PaBufferAttr,
    *mut c_int,
) -> *mut c_void;
type PaSimpleWrite = unsafe extern "C" fn(*mut c_void, *const c_void, usize, *mut c_int) -> c_int;
type PaSimpleFree = unsafe extern "C" fn(*mut c_void);

pub struct Pulse {
    _lib: libloading::Library,
    conn: *mut c_void,
    period: usize,
    write: PaSimpleWrite,
    free: PaSimpleFree,
}

// The connection is only used by the audio thread after `open`.
unsafe impl Send for Pulse {}

impl Pulse {
    fn open(rate: u32) -> Result<Self, String> {
        unsafe {
            let lib = libloading::Library::new("libpulse-simple.so.0").map_err(|e| e.to_string())?;
            let new = *lib.get::<PaSimpleNew>(b"pa_simple_new\0").map_err(|e| e.to_string())?;
            let write = *lib.get::<PaSimpleWrite>(b"pa_simple_write\0").map_err(|e| e.to_string())?;
            let free = *lib.get::<PaSimpleFree>(b"pa_simple_free\0").map_err(|e| e.to_string())?;
            let period = 256usize;
            let spec = PaSampleSpec {
                format: PA_SAMPLE_FLOAT32LE,
                rate,
                channels: 2,
            };
            let bytes = (period * 2 * 4) as u32;
            // Target about 4 periods (~21 ms) of buffering.
            let attr = PaBufferAttr {
                maxlength: u32::MAX,
                tlength: bytes * 4,
                prebuf: u32::MAX,
                minreq: bytes,
                fragsize: u32::MAX,
            };
            let mut err: c_int = 0;
            let conn = new(
                std::ptr::null(),
                c"RasDaw".as_ptr(),
                PA_STREAM_PLAYBACK,
                std::ptr::null(),
                c"Master".as_ptr(),
                &spec,
                std::ptr::null(),
                &attr,
                &mut err,
            );
            if conn.is_null() {
                return Err(format!("pa_simple_new failed (error {err})"));
            }
            Ok(Self {
                _lib: lib,
                conn,
                period,
                write,
                free,
            })
        }
    }

    fn run(self, engine: &mut Engine, stop: &AtomicBool) {
        let mut buf = vec![0.0f32; self.period * 2];
        while !stop.load(Ordering::Acquire) {
            engine.render_interleaved(&mut buf);
            let mut err: c_int = 0;
            let r = unsafe { (self.write)(self.conn, buf.as_ptr() as *const c_void, buf.len() * 4, &mut err) };
            if r < 0 {
                eprintln!("audio: pa_simple_write failed (error {err}); stopping output");
                return;
            }
        }
    }
}

impl Drop for Pulse {
    fn drop(&mut self) {
        unsafe { (self.free)(self.conn) }
    }
}

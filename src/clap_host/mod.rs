//! CLAP plugin hosting: scanning, loading, instances, host callbacks and GUI embedding.
//!
//! Threading contract (see the CLAP spec, `[main-thread]` / `[audio-thread]` markers):
//! - Everything on [`PluginInstance`] except [`PluginInstance::process`],
//!   [`PluginInstance::start_processing`] and [`PluginInstance::stop_processing`] runs on the
//!   UI (main) thread.
//! - Those three run on the engine's audio thread only.
//! - The last `Arc<PluginInstance>` is always dropped on the main thread, because the engine
//!   hands every retired snapshot back to the UI (see `engine`).

mod events;
pub mod gui;
mod host;
mod instance;
mod scan;

pub use instance::PluginInstance;
pub use scan::{PluginDesc, Registry};

use std::cell::Cell;

thread_local! {
    static THREAD_ROLE: Cell<u8> = const { Cell::new(0) };
}

/// Marks the current thread as the main (UI) thread for the thread-check extension.
pub fn mark_main_thread() {
    THREAD_ROLE.with(|r| r.set(1));
}

/// Marks the current thread as the audio thread for the thread-check extension.
pub fn mark_audio_thread() {
    THREAD_ROLE.with(|r| r.set(2));
}

pub(crate) fn is_main_thread() -> bool {
    THREAD_ROLE.with(|r| r.get() == 1)
}

pub(crate) fn is_audio_thread() -> bool {
    THREAD_ROLE.with(|r| r.get() == 2)
}

/// Reads a NUL-terminated C string from a fixed-size or borrowed buffer.
pub(crate) unsafe fn cstr_to_string(ptr: *const std::ffi::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

pub(crate) fn chars_to_string(buf: &[std::ffi::c_char]) -> String {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

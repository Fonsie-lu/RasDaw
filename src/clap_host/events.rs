//! Preallocated CLAP input and output event lists for the audio thread.

use clap_sys::events::*;
use clap_sys::id::clap_id;
use std::ffi::c_void;

/// Storage large enough for any event we send.
#[repr(C)]
#[derive(Clone, Copy)]
pub union EventSlot {
    pub header: clap_event_header,
    pub note: clap_event_note,
    pub param: clap_event_param_value,
}

/// Input events. `list` points back at `self`, so the struct must not move while the plugin
/// holds the pointer (it lives in a boxed `AudioSide` and is only used during `process`).
pub struct InputEvents {
    pub slots: Vec<EventSlot>,
    pub list: clap_input_events,
}

impl InputEvents {
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            slots: Vec::with_capacity(cap),
            list: clap_input_events {
                ctx: std::ptr::null_mut(),
                size: Some(in_size),
                get: Some(in_get),
            },
        }
    }

    pub fn clear(&mut self) {
        self.slots.clear();
    }

    fn push(&mut self, slot: EventSlot) {
        // Never grow on the audio thread; excess events are dropped.
        if self.slots.len() < self.slots.capacity() {
            self.slots.push(slot);
        }
    }

    pub fn note(&mut self, time: u32, on: bool, key: u8, velocity: f64) {
        let type_ = if on {
            CLAP_EVENT_NOTE_ON
        } else {
            CLAP_EVENT_NOTE_OFF
        };
        self.push(EventSlot {
            note: clap_event_note {
                header: header::<clap_event_note>(time, type_),
                note_id: -1,
                port_index: 0,
                channel: 0,
                key: key as i16,
                velocity,
            },
        });
    }

    pub fn param(&mut self, time: u32, id: clap_id, value: f64) {
        self.push(EventSlot {
            param: clap_event_param_value {
                header: header::<clap_event_param_value>(time, CLAP_EVENT_PARAM_VALUE),
                param_id: id,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            },
        });
    }

    pub fn as_ptr(&mut self) -> *const clap_input_events {
        self.list.ctx = self as *mut Self as *mut c_void;
        &self.list
    }
}

fn header<T>(time: u32, type_: u16) -> clap_event_header {
    clap_event_header {
        size: std::mem::size_of::<T>() as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_,
        flags: 0,
    }
}

unsafe extern "C" fn in_size(list: *const clap_input_events) -> u32 {
    let this = &*((*list).ctx as *const InputEvents);
    this.slots.len() as u32
}

unsafe extern "C" fn in_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let this = &*((*list).ctx as *const InputEvents);
    match this.slots.get(index as usize) {
        Some(slot) => &slot.header,
        None => std::ptr::null(),
    }
}

/// Output events. Only parameter values are kept (they come from the plugin's own GUI or
/// internal modulation); everything else is accepted and dropped.
pub struct OutputEvents {
    pub params: rtrb::Producer<(clap_id, f64)>,
    pub list: clap_output_events,
}

impl OutputEvents {
    pub fn new(params: rtrb::Producer<(clap_id, f64)>) -> Self {
        Self {
            params,
            list: clap_output_events {
                ctx: std::ptr::null_mut(),
                try_push: Some(out_try_push),
            },
        }
    }

    pub fn as_ptr(&mut self) -> *const clap_output_events {
        self.list.ctx = self as *mut Self as *mut c_void;
        &self.list
    }
}

unsafe extern "C" fn out_try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    let this = &mut *((*list).ctx as *mut OutputEvents);
    if event.is_null() {
        return false;
    }
    let h = &*event;
    if h.space_id == CLAP_CORE_EVENT_SPACE_ID && h.type_ == CLAP_EVENT_PARAM_VALUE {
        let e = &*(event as *const clap_event_param_value);
        let _ = this.params.push((e.param_id, e.value));
    }
    true
}

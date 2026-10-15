# RasDaw (Fonsie Audio): project notes for Claude

RasDaw by Fonsie Audio: a Bitwig-inspired DAW framework in Rust that hosts **CLAP** plugins.
It bundles six basic effect devices (EQ-5, Compressor, Limiter, Distortion, Delay, Reverb) that
live inside the DAW, not as separate plugins. It ships no instruments. The GUI style follows `~/tools/juno-clone` (coloured section bars, raised
buttons, fader caps, ribbed knobs) with a Tokyo Night colour scheme.

## Commands

```sh
cargo run --release                         # app (binary `rasdaw`); project file defaults to ~/rasdaw-project.json
cargo run --release -- path/to/project.json
./install.sh                                # release build, copied to ~/.local/bin/rasdaw (--no-build, --uninstall, BIN_DIR=...)
cargo test                                  # unit tests + real-plugin integration test
cargo run --example editor_smoke -- ch.fonsie.alpha-juno 4   # open/close one plugin editor
```

- Environment:
  - `DAW_AUDIO=pulse|alsa|null` forces a driver.
  - `DAW_ALSA_DEVICE` picks the ALSA device.
  - `DAW_VIEW=piano:<track>[:<scene>]` or `DAW_VIEW=browser` opens that view at start
    (for screenshots).
  - `DAW_TEST_PLUGIN=<clap id>` picks the instrument for `tests/clap_host.rs`.
- Screenshots: run the app with `env -u WAYLAND_DISPLAY` (forces X11), then
  `import -window "RasDaw" out.png`. Plugin editor windows are titled `<plugin> — <track>`.
- The machine has no ALSA/JACK headers. All audio libraries are `dlopen`ed at runtime.
  The ALSA `default` device fails here because the PipeWire ALSA config is not linked into
  `/etc/alsa/conf.d`, so the PulseAudio backend (served by pipewire-pulse) is the one that works.
- Installed test plugins (all nih-plug, in `~/.clap`): Alpha Juno, Drift, Double Slit
  (instruments), Lemma (reverb).

## Layout

| File | Role |
|------|------|
| `src/main.rs` | Startup: plugin scan, audio driver, engine, eframe window |
| `src/model.rs` | Project data (`Project`, `Track`, `Clip`, `Note`, `DeviceSpec`), serde JSON, `Track::play_mode()` |
| `src/engine/mod.rs` | Real-time engine: `Snapshot` (built from the project), `EngineHandle` (UI side), `Engine` (audio side), transport, device chains, meters |
| `src/engine/sequencer.rs` | Pure note scheduling: `Source` (arrangement / looping scene / silent), `Voices`, `render()` |
| `src/audio.rs` | Output drivers via dlopen: PulseAudio simple API, ALSA, null |
| `src/devices/mod.rs` | Built-in devices: `Kind` (ids, names, param tables), `ParamDef` (ranges, log travel, formatting), `BuiltinDevice` (atomic params + audio-thread DSP), `Dsp` trait, tests and a CPU report |
| `src/devices/dsp.rs` | Biquad (RBJ cookbook) with `magnitude()`, `DelayLine`, dB helpers |
| `src/devices/eq.rs` | EQ-5 plus fixed 20 Hz HP / 16 kHz LP (Butterworth 4th order) and mono below 100 Hz (LR4 high-pass on the side signal); `response_db()` for the graph |
| `src/devices/dynamics.rs` | Compressor (soft knee, stereo linked) and look-ahead limiter (1.5 ms) |
| `src/devices/fx.rs` | Delay (sync, ping-pong, filtered feedback), Reverb (Freeverb topology + pre-delay + low cut), Distortion (soft/hard/fold/tube) |
| `src/midi.rs` | MIDI keyboard input: ALSA sequencer via dlopen, own thread, hot-plug auto-connect of hardware ports, `MidiInput` (UI handle: port list, connect/disconnect) |
| `src/clap_host/scan.rs` | Search path, `PluginLibrary` (dlopen + `clap_entry`), `PluginDesc`, `Registry` |
| `src/clap_host/instance.rs` | `PluginInstance`: create/init/activate, `process`, params, state, GUI calls, `idle()` |
| `src/clap_host/host.rs` | `clap_host` and host extensions (log, thread-check, params, gui, timer, posix-fd, state, latency, audio/note ports) |
| `src/clap_host/events.rs` | Preallocated input/output event lists |
| `src/clap_host/gui.rs` | `GuiManager`: plugin editors embedded in x11rb top-level windows |
| `src/theme.rs` | `Theme` tokens, `TOKYO_NIGHT`. Never hard-code colours elsewhere |
| `src/ui/mod.rs` | `App` (eframe), layout constants, device add/remove/move, save/load, snapshot publishing, shortcuts |
| `src/ui/launcher.rs` | Scene launcher (top left), shared row scrolling |
| `src/ui/arranger.rs` | Track headers + timeline (top right), `mini_notes()` |
| `src/ui/piano_roll.rs` | Piano roll for timeline notes or a scene clip |
| `src/ui/rack.rs` | Channel strip, CLAP device cards (paged knobs), shared card header, device browser |
| `src/ui/builtin_card.rs` | Built-in device cards: all knobs at once, EQ graph with draggable band handles, gain reduction meter |
| `src/ui/widgets.rs` | Juno-style drawing helpers, fonts, egui visuals |
| `tests/clap_host.rs` | Hosts a real instrument through the engine and checks for sound |
| `tests/midi.rs` | A second sequencer client plays notes into RasDaw's port; checks they reach the target track |

## Identity

- Name **RasDaw**, manufacturer **Fonsie Audio**. Crate and binary: `rasdaw`.
- Reported to plugins as `clap_host.name = "RasDaw"`, `vendor = "Fonsie Audio"`
  (`src/clap_host/host.rs`). Window title and app id: `RasDaw` / `rasdaw`.

## Core rules

- **Scene vs. timeline:** a track whose timeline (arrangement clip) has any notes plays the
  timeline and its scenes are muted. Otherwise its active scene clip loops. Scene loops are
  aligned to beat 0 of the timeline (`position mod clip length`). See `Track::play_mode()`.
- Scenes only sound while the transport runs. Launching a scene or a slot starts it.
- Rows of the launcher and the arranger share `ROW_H` and `View::scroll_y`, so they line up.
- Double-click a lane, a track header or a scene slot to open the piano roll; ESC returns.
- **MIDI keyboard:**
  - The selected track receives MIDI input (green MIDI badge on its header).
  - Note on/off become CLAP note events. Other channel messages (CC, pitch bend, pressure,
    program change) go to the plugins as raw `CLAP_EVENT_MIDI`, all at the start of the
    next block (jitter up to one period).
  - A note-off goes to the track that got the note-on, even if the selection changed in
    between. On a target change the old track also gets sustain off and pitch bend centred.
  - Hardware ports (sequencer port type `HARDWARE`, not "Midi Through") connect
    automatically, also when plugged in later. Software ports are listed in the MIDI menu
    and connected by hand. Manual choices last for the session only.
  - Keys held on the target track light up on the piano roll keyboard.

## Built-in devices

- `DeviceSpec::plugin_id` is `rasdaw.<name>` (`Kind::id`); never change these ids or any
  `ParamDef::key`, they are stored in projects. Values are saved in `DeviceSpec::params`
  (a key → value map), not in `state`.
- The chain type is `engine::DeviceRef` (`Clap` or `Builtin`); the rack's
  `HashMap<u64, Vec<Option<DeviceRef>>>` stays parallel to `Track::devices`.
- They have no editor window; everything is set in the rack card.
- Parameters are atomics (UI writes, audio thread reads once per block). The DSP state and all
  buffers are allocated in `BuiltinDevice::new` on the main thread.
- To add a device: a `Kind` variant (id, name, description, params, `make_dsp`), a `Dsp`
  impl, and optionally `rows()`, `param_active()` or a display in `builtin_card.rs`.
- The audio thread sets FTZ/DAZ (`audio::enable_flush_to_zero`) against denormal slowdowns.

## Real-time and threading rules

- The audio thread never allocates or frees:
  - The UI publishes immutable `Arc<Snapshot>`s through an rtrb queue.
  - The engine sends every retired snapshot back through a garbage queue and the UI drops it
    in `collect_garbage()`.
  - Event and audio buffers are preallocated; pushes past capacity are dropped.
- Plugin lifecycle:
  - Create, init and activate happen on the main thread (`PluginInstance::new`).
  - `start_processing` runs lazily on the audio thread at the first `process`.
  - `stop_processing` runs on the audio thread when an instance leaves every chain in a new
    snapshot, or when the `Engine` drops.
  - The last `Arc<PluginInstance>` drops on the main thread, so `Drop` (gui destroy,
    deactivate, destroy) runs there.
- Field order in `App` is drop order: editors close, the audio thread stops, then instances die.
- Parameter changes from the UI go through a per-instance SPSC queue and are delivered as
  `CLAP_EVENT_PARAM_VALUE` at the start of the next `process` call. Values shown in the rack
  come from `params.get_value`.
- Host callbacks may come from any thread and only set flags. `PluginInstance::idle()` acts
  on them every UI frame (on_main_thread, timers, posix fds, param rescans).
- Thread roles for the thread-check extension come from `mark_main_thread` /
  `mark_audio_thread`.

## Not done yet

- Plugin latency compensation, `request_restart` (logged and ignored), audio-ports rescan.
- MIDI recording into clips, MIDI output, MIDI learn, automation, undo.
- Note velocity editing, multi-note selection, copy/paste.
- Clips on the timeline are one region per track (the whole timeline); there is no clip
  splitting.
- VST3 or other plugin formats.
- More themes (add a `Theme` constant; all colours already come from `theme.rs`).

# RasDaw (Fonsie Audio): project notes for Claude

RasDaw by Fonsie Audio: a Bitwig-inspired DAW framework in Rust that hosts **CLAP** plugins.
It ships no instruments or effects of its own. The GUI style follows `~/tools/juno-clone` (coloured section bars, raised
buttons, fader caps, ribbed knobs) with a Tokyo Night colour scheme.

## Commands

```sh
cargo run --release                         # app (binary `rasdaw`); project file defaults to ~/rasdaw-project.json
cargo run --release -- path/to/project.json
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
| `src/ui/rack.rs` | Channel strip, device cards with parameter knobs, plugin browser |
| `src/ui/widgets.rs` | Juno-style drawing helpers, fonts, egui visuals |
| `tests/clap_host.rs` | Hosts a real instrument through the engine and checks for sound |

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
- MIDI input from hardware, recording, automation, undo.
- Note velocity editing, multi-note selection, copy/paste.
- Clips on the timeline are one region per track (the whole timeline); there is no clip
  splitting.
- VST3 or other plugin formats.
- More themes (add a `Theme` constant; all colours already come from `theme.rs`).

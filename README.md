# RasDaw

RasDaw by Fonsie Audio: a Bitwig-inspired DAW framework written in Rust that hosts CLAP plugins.
It bundles a set of basic effect devices; instruments come from CLAP plugins. Tokyo Night colours, with the panel style of the Alpha Juno clone.

## Layout

- **Top left: scenes.** Each track has 4 scene slots holding looping clips. Click a slot to
  launch or stop it. Click a scene header to launch the whole column.
- **Top right: tracks and timeline.** The rows line up with the scene rows.
  - If a track has notes on the timeline, the timeline plays and its scenes are muted.
  - Otherwise the active scene loops.
- **Piano roll.** Double-click a lane, a track header or a scene slot to edit its notes. Press
  Esc to go back.
- **Bottom: device rack.** Shows the selected track's channel strip and its CLAP devices, left
  to right in signal order. Use **+ ADD DEVICE** to browse the installed plugins.

## Built-in devices

Add them with **+ ADD DEVICE** (section "RASDAW DEVICES"). They are set up entirely in the
device rack; there is no separate plugin window.

| Device | What it does |
|--------|--------------|
| EQ-5 | Low shelf, 3 peaks, high shelf. Always cuts below 20 Hz and above 16 kHz and sums everything below 100 Hz to mono. Drag the numbered handles in the graph. |
| Compressor | Threshold, ratio, attack, release, soft knee, makeup, mix; gain reduction meter |
| Limiter | Look-ahead brickwall limiter: gain, ceiling, release; gain reduction meter |
| Distortion | Soft, hard, fold and tube shapes with drive, tone, output and mix |
| Delay | Free time or tempo-synced divisions, feedback, ping-pong, low/high cut in the feedback |
| Reverb | Size, damping, width, pre-delay, low cut, mix |

## MIDI keyboard

- Hardware MIDI keyboards connect automatically through the ALSA sequencer, also when you
  plug them in later.
- The selected track receives the keyboard; its header shows a green **MIDI** badge.
- The **MIDI** button in the transport bar lists all MIDI sources and lets you connect or
  disconnect each one. The LED next to it flashes on incoming MIDI.

## Running

```sh
cargo run --release [project.json]
./install.sh              # build; install to ~/.local/bin/rasdaw plus app launcher entry and icon
./install.sh --uninstall  # remove all of it again
```

- Plugins are searched in `$CLAP_PATH`, `~/.clap`, `/usr/lib/clap` and `/usr/local/lib/clap`.
- Audio goes to PulseAudio/PipeWire, then ALSA, then a silent null driver. Force one with
  `DAW_AUDIO=pulse|alsa|null`.

## Shortcuts

| Key | Action |
|-----|--------|
| Space | Play / stop |
| Home | Back to the start |
| Esc | Close the piano roll or the browser |
| Del | Delete the selected note |
| Ctrl+S | Save the project |

In the piano roll:

- Click to add a note, or drag on empty space to draw one.
- Drag a note to move it, or drag its right edge to resize it.
- Right-click a note to delete it.
- Click the keyboard to preview notes.
- Mouse wheel scrolls; Shift+wheel scrolls time; Ctrl+wheel zooms.

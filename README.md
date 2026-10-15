# RasDaw

RasDaw by Fonsie Audio: a Bitwig-inspired DAW framework written in Rust that hosts CLAP plugins.
It bundles a set of basic effect devices; instruments come from CLAP plugins. Tokyo Night colours, with the panel style of the Alpha Juno clone.

## Layout

- **Top left: scenes.** Each track has 4 scene slots holding looping clips. Each clip has a
  root key and a time signature (right-click the slot, or set them in the piano roll).
  - Click a slot to launch or stop it; click a scene header to launch the whole column;
    **■ SCENES** stops all. Scenes play on their own, without the arranger, and loop until
    stopped. Playing slots blink on the beat, waiting slots blink fast.
  - Launches start on the next step of the **SYNC** grid in the top bar (default 1/16).
  - The ● on the right of a slot's title bar records MIDI into that scene: play your keyboard
    and the notes land in the piano roll. An empty slot grows while you record and is
    trimmed to whole bars; an existing clip records in a loop (overdub). Click ● again or
    stop the scene to finish.
- **Top right: tracks and timeline.** The rows line up with the scene rows. The timeline holds
  only scene blocks; a block loops one of the track's scene clips and shows its root key and
  time signature.
  - Drag a scene slot onto its track's lane, or double-click the lane, to add a block.
  - Drag a block to move it, drag its right edge to resize it, right-click it to change its
    scene. The ✕ at its left deletes it.
  - While the arranger plays, a track with blocks plays them and its scene is muted.
- **Master track:** the smaller MASTER row below the tracks. It takes no scenes and no
  blocks; click it to add effects to the sum of all tracks and set the master volume and pan.
- **Piano roll.** Double-click a block, a track header or a scene slot to edit the scene's
  notes. Press Esc to go back.
- **Bottom: device rack** (fixed height). The selected track's channel strip, then three
  sections in signal order: **NOTE FX** (one note-altering plugin), **INSTRUMENT** (one) and
  **EFFECTS** (any number). Click **+** in a section to add a device.
  - The ✕ at the left of a device's title bar deletes it.
  - Click a device's title bar to turn it on or off.
  - Drag an effect's title bar to move it left or right.
  - Right-click the title bar to open the plugin's own editor or remove the device. CLAP
    plugins also have an editor icon and page arrows in the title bar.
- **GUI scale:** the percentage button in the top bar, or Ctrl + / Ctrl - / Ctrl 0. RasDaw
  remembers it in `~/.config/rasdaw/settings.json`.

## Built-in devices

Add them with the **+** in the NOTE FX or EFFECTS section (list "RASDAW DEVICES"). They are set up entirely in the
device rack; there is no separate plugin window.

| Device | What it does |
|--------|--------------|
Note FX devices use dropdowns for choices and flip switches for on/off settings.

| Chord (note FX) | Plays a chord for every note: major, minor, sus2, sus4, 7, maj7, min7, dim, aug, power, octave; inversion, octave shift, strum |
| Arp (note FX) | Arpeggiator on the song tempo: up, down, up-down, down-up, random, as played; rate 1/4 – 1/32 incl. triplets, gate, 1–4 octaves, latch |
| Note Repeat (note FX) | Retriggers held notes on the tempo grid: rate, gate, velocity decay |
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
| Esc | Close the piano roll, the browser or a dialog |
| Del | Delete the selected note (piano roll), else the selected device, timeline block or scene clip |
| Ctrl+S | Save dialog |
| Ctrl+O | Open dialog |
| Ctrl + / Ctrl - / Ctrl 0 | GUI scale up / down / reset |

In the piano roll:

- Click to add a note, or drag on empty space to draw one.
- Drag a note to move it, or drag its right edge to resize it.
- Right-click a note to delete it.
- Click the keyboard to preview notes.
- Mouse wheel scrolls; Shift+wheel scrolls time; Ctrl+wheel zooms.

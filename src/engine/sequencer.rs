//! Turns note lists into sample-accurate note on/off events, one block at a time. Pure logic,
//! no allocation: the caller owns the output buffer and the per-track [`Voices`].

/// A note as the sequencer sees it, in source-local beats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeqNote {
    pub start: f64,
    pub end: f64,
    pub key: u8,
    pub velocity: f32,
}

/// What a track plays. Notes are sorted by start.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Source {
    /// Timeline notes at absolute beat positions.
    Arrangement(Vec<SeqNote>),
    /// A clip that loops every `length` beats, aligned to beat 0 of the timeline.
    Scene { notes: Vec<SeqNote>, length: f64 },
    #[default]
    Silent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    /// Sample offset inside the block.
    pub time: u32,
    pub on: bool,
    pub key: u8,
    pub velocity: f32,
}

/// Sounding notes of one track, with the source-local beat at which each one ends.
#[derive(Clone)]
pub struct Voices {
    end: [Option<f64>; 128],
}

impl Default for Voices {
    fn default() -> Self {
        Self { end: [None; 128] }
    }
}

impl Voices {
    #[cfg(test)]
    pub fn any(&self) -> bool {
        self.end.iter().any(Option::is_some)
    }

    /// Note-offs for everything that is sounding, at `time`.
    pub fn release_all(&mut self, time: u32, out: &mut Vec<NoteEvent>) {
        for (key, end) in self.end.iter_mut().enumerate() {
            if end.take().is_some() {
                push(out, NoteEvent { time, on: false, key: key as u8, velocity: 0.0 });
            }
        }
    }
}

fn push(out: &mut Vec<NoteEvent>, e: NoteEvent) {
    // Never grow on the audio thread.
    if out.len() < out.capacity() {
        out.push(e);
    }
}

/// Maps a beat range of one block onto sample offsets.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    /// Global (timeline) beats covered, `b0 < b1`.
    pub b0: f64,
    pub b1: f64,
    /// Sample offset of `b0` inside the block and samples per beat.
    pub frame0: f64,
    pub frames_per_beat: f64,
    /// Block length, used to clamp offsets.
    pub frames: u32,
}

impl Span {
    fn time_at(&self, beat_from_b0: f64) -> u32 {
        let t = self.frame0 + beat_from_b0 * self.frames_per_beat;
        (t.max(0.0).round() as u32).min(self.frames.saturating_sub(1))
    }
}

/// Emits the events for `span` (a part of the block without a transport jump inside it).
pub fn render(source: &Source, voices: &mut Voices, span: Span, out: &mut Vec<NoteEvent>) {
    match source {
        Source::Silent => voices.release_all(span.time_at(0.0), out),
        Source::Arrangement(notes) => local_range(notes, voices, span.b0, span.b1, 0.0, span, out),
        Source::Scene { notes, length } => {
            let len = *length;
            if len <= 0.0 {
                return;
            }
            let l0 = span.b0.rem_euclid(len);
            let l1 = l0 + (span.b1 - span.b0);
            if l1 <= len {
                local_range(notes, voices, l0, l1, 0.0, span, out);
            } else {
                // The clip wraps inside this span.
                local_range(notes, voices, l0, len, 0.0, span, out);
                local_range(notes, voices, 0.0, l1 - len, len - l0, span, out);
            }
        }
    }
}

/// Handles the local beat range `[s, e)`, which starts `offset` beats after `span.b0`.
fn local_range(
    notes: &[SeqNote],
    voices: &mut Voices,
    s: f64,
    e: f64,
    offset: f64,
    span: Span,
    out: &mut Vec<NoteEvent>,
) {
    let time = |beat: f64| span.time_at(offset + beat - s);
    let first = notes.partition_point(|n| n.start < s);
    for n in &notes[first..] {
        if n.start >= e {
            break;
        }
        release_until(voices, s, n.start, true, &time, out);
        let k = n.key as usize & 127;
        if voices.end[k].is_some() {
            // Retrigger: end the previous note on this key first.
            push(out, NoteEvent { time: time(n.start), on: false, key: n.key, velocity: 0.0 });
        }
        push(out, NoteEvent { time: time(n.start), on: true, key: n.key, velocity: n.velocity });
        voices.end[k] = Some(n.end);
    }
    release_until(voices, s, e, false, &time, out);
}

/// Releases notes whose end lies in `[s, limit)` (or `[s, limit]` if `inclusive`).
fn release_until(
    voices: &mut Voices,
    s: f64,
    limit: f64,
    inclusive: bool,
    time: &impl Fn(f64) -> u32,
    out: &mut Vec<NoteEvent>,
) {
    for (key, slot) in voices.end.iter_mut().enumerate() {
        if let Some(end) = *slot {
            let hit = end >= s && if inclusive { end <= limit } else { end < limit };
            if hit {
                push(out, NoteEvent { time: time(end), on: false, key: key as u8, velocity: 0.0 });
                *slot = None;
            }
        }
    }
}

/// Builds the sequencer view of a clip. Scene note ends that pass the loop end wrap around.
pub fn scene_notes(notes: impl Iterator<Item = (f64, f64, u8, f32)>, length: f64) -> Vec<SeqNote> {
    let mut v: Vec<SeqNote> = notes
        .filter(|n| n.0 < length && n.0 >= 0.0)
        .map(|(start, len, key, velocity)| {
            // A note may not outlast the loop, or it would overlap its own next start.
            let len = len.min(length - 1e-6).max(1e-6);
            let mut end = start + len;
            if end >= length {
                end -= length;
            }
            SeqNote { start, end, key, velocity }
        })
        .collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    v
}

pub fn arrangement_notes(notes: impl Iterator<Item = (f64, f64, u8, f32)>) -> Vec<SeqNote> {
    let mut v: Vec<SeqNote> = notes
        .map(|(start, len, key, velocity)| SeqNote { start, end: start + len.max(1e-6), key, velocity })
        .collect();
    v.sort_by(|a, b| a.start.total_cmp(&b.start));
    v
}

/// Sorts a block's events by time, note-offs before note-ons at the same time.
pub fn sort_events(events: &mut [NoteEvent]) {
    events.sort_unstable_by_key(|e| (e.time, e.on, e.key));
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPB: f64 = 64.0; // samples per beat; keeps test positions exact

    fn run(source: &Source, beats: f64, block_beats: f64) -> Vec<(f64, bool, u8)> {
        let mut voices = Voices::default();
        let mut all = Vec::new();
        let mut b = 0.0;
        while b < beats - 1e-9 {
            let mut out = Vec::with_capacity(256);
            let frames = (block_beats * FPB) as u32;
            render(
                source,
                &mut voices,
                Span { b0: b, b1: b + block_beats, frame0: 0.0, frames_per_beat: FPB, frames },
                &mut out,
            );
            sort_events(&mut out);
            all.extend(out.iter().map(|e| (b + e.time as f64 / FPB, e.on, e.key)));
            b += block_beats;
        }
        all
    }

    fn n(start: f64, len: f64, key: u8) -> (f64, f64, u8, f32) {
        (start, len, key, 1.0)
    }

    #[test]
    fn arrangement_notes_on_and_off_across_blocks() {
        let src = Source::Arrangement(arrangement_notes([n(0.5, 1.0, 60), n(1.0, 0.25, 64)].into_iter()));
        let ev = run(&src, 4.0, 0.375);
        assert_eq!(ev, vec![(0.5, true, 60), (1.0, true, 64), (1.25, false, 64), (1.5, false, 60)]);
    }

    #[test]
    fn scene_loops_and_wrapped_note_ends() {
        // 2-beat loop; the second note ends past the loop end and must stop at 0.5 next loop.
        let src = Source::Scene { notes: scene_notes([n(0.0, 0.5, 60), n(1.5, 1.0, 62)].into_iter(), 2.0), length: 2.0 };
        let ev = run(&src, 4.0, 0.25);
        assert_eq!(
            ev,
            vec![
                (0.0, true, 60),
                (0.5, false, 60),
                (1.5, true, 62),
                (2.0, true, 60),
                (2.5, false, 60),
                (2.5, false, 62),
                (3.5, true, 62)
            ]
        );
    }

    #[test]
    fn scene_wrap_inside_one_block() {
        let src = Source::Scene { notes: scene_notes([n(0.0, 0.25, 48)].into_iter(), 1.0), length: 1.0 };
        // Block of 0.75 beats starting at 0.5 contains the loop point at 1.0.
        let mut voices = Voices::default();
        let mut out = Vec::with_capacity(16);
        render(&src, &mut voices, Span { b0: 0.5, b1: 1.25, frame0: 0.0, frames_per_beat: FPB, frames: 48 }, &mut out);
        sort_events(&mut out);
        assert_eq!(out.iter().map(|e| (e.time, e.on)).collect::<Vec<_>>(), vec![(32, true)]);
    }

    #[test]
    fn retrigger_same_key() {
        let src = Source::Arrangement(arrangement_notes([n(0.0, 2.0, 60), n(1.0, 0.5, 60)].into_iter()));
        let ev = run(&src, 3.0, 0.5);
        assert_eq!(ev, vec![(0.0, true, 60), (1.0, false, 60), (1.0, true, 60), (1.5, false, 60)]);
    }

    #[test]
    fn silent_releases_everything() {
        let mut voices = Voices::default();
        let mut out = Vec::with_capacity(16);
        let src = Source::Arrangement(arrangement_notes([n(0.0, 4.0, 60)].into_iter()));
        let span = Span { b0: 0.0, b1: 0.5, frame0: 0.0, frames_per_beat: FPB, frames: 32 };
        render(&src, &mut voices, span, &mut out);
        assert!(voices.any());
        out.clear();
        render(&Source::Silent, &mut voices, span, &mut out);
        assert_eq!(out.len(), 1);
        assert!(!out[0].on && !voices.any());
    }
}

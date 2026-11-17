//! Where the speech is in a recording, from its loudness (an energy voice
//! detector). Two uses: cut the silence before and after the words, so
//! Whisper encodes only speech and an empty hold is dropped without a pass;
//! and find the pauses inside an utterance, where [`super::stream`] can
//! transcribe what came before while the person is still talking.
//!
//! A frame counts as speech when it is well above the recording's own noise
//! floor (its quietest tenth) or within 20 dB of its loudest frame, and
//! above an absolute floor. Speech is padded
//! on both sides, so soft first and last sounds survive.

use super::wav;

/// 20 ms frames at 16 kHz.
pub const FRAME: usize = wav::RATE as usize / 50;
/// Kept before the first speech frame and after the last.
const PAD_BEFORE: usize = 15; // 300 ms
const PAD_AFTER: usize = 20; // 400 ms
/// About -50 dBFS, the level mod.rs calls a silent room: below this
/// nothing is speech, however quiet the room.
const ABS_FLOOR: f32 = 0.003;
/// Never more than this far below the loudest frame (20 dB), so a clip that
/// is speech from end to end keeps its soft first and last words.
const UNDER_PEAK: f32 = 0.1;
/// Speech is at least this many times the noise floor's level (about 12 dB).
const OVER_FLOOR: f32 = 4.0;
/// A lone loud frame (a click, a bump) is not speech: it takes this many
/// speech frames in a row.
const MIN_RUN: usize = 3;

fn frame_levels(samples: &[f32]) -> Vec<f32> {
    samples.chunks(FRAME).map(wav::rms).collect()
}

/// The level above which a frame is speech, for this recording.
fn threshold(levels: &[f32]) -> f32 {
    if levels.is_empty() {
        return ABS_FLOOR;
    }
    let mut sorted = levels.to_vec();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let peak = sorted[sorted.len() - 1];
    (floor * OVER_FLOOR).min(peak * UNDER_PEAK).max(ABS_FLOOR)
}

/// Which frames are speech, with runs shorter than [`MIN_RUN`] cleared.
fn speech_frames(levels: &[f32], thr: f32) -> Vec<bool> {
    let mut on: Vec<bool> = levels.iter().map(|l| *l > thr).collect();
    let mut i = 0;
    while i < on.len() {
        if !on[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < on.len() && on[i] {
            i += 1;
        }
        if i - start < MIN_RUN {
            on[start..i].fill(false);
        }
    }
    on
}

/// The sample range holding the speech in `samples` (16 kHz), padded; None
/// when there is no speech at all.
pub fn speech(samples: &[f32]) -> Option<std::ops::Range<usize>> {
    let levels = frame_levels(samples);
    let on = speech_frames(&levels, threshold(&levels));
    let first = on.iter().position(|f| *f)?;
    let last = on.iter().rposition(|f| *f)?;
    let start = first.saturating_sub(PAD_BEFORE) * FRAME;
    let end = ((last + 1 + PAD_AFTER) * FRAME).min(samples.len());
    Some(start..end)
}

/// The middle of the last pause of at least `min_pause` frames that ends
/// before the last `min_pause` frames and starts after `min_speech` frames
/// of speech: a place to cut an utterance in two. None when there is none.
pub fn last_pause(samples: &[f32], min_speech: usize, min_pause: usize) -> Option<usize> {
    let levels = frame_levels(samples);
    let on = speech_frames(&levels, threshold(&levels));
    let mut best = None;
    let mut spoken = 0;
    let mut i = 0;
    while i < on.len() {
        if on[i] {
            spoken += 1;
            i += 1;
            continue;
        }
        let start = i;
        while i < on.len() && !on[i] {
            i += 1;
        }
        // A pause still running at the end may be the end of the utterance:
        // leave it for the release.
        let closed = i < on.len();
        if closed && i - start >= min_pause && spoken >= min_speech {
            best = Some((start + i) / 2 * FRAME);
        }
    }
    best
}

/// The gate's noise floor never goes below this (about -70 dBFS): a
/// digitally silent input would otherwise make any click speech.
const GATE_FLOOR_MIN: f32 = 0.000_3;
/// A frame is loud at this many times the floor (about 10 dB), and never
/// below [`ABS_FLOOR`].
const GATE_OVER: f32 = 3.2;
/// How fast the floor rises toward a louder room, per frame: quickly while
/// nobody speaks (a fan coming on is the floor within a few seconds), very
/// slowly while someone does (steady music or noise becomes the floor in
/// about ten seconds, so it does not count as speech for ever; real speech
/// dips between syllables, and a quieter frame pulls the floor down at once).
const RISE_QUIET: f32 = 0.02;
const RISE_SPEAKING: f32 = 0.000_8;
/// Speech goes on through this many quiet frames (200 ms): the gaps between
/// words.
const HANG: u32 = 10;

/// Speech or not, a frame at a time, as the sound arrives: the listening
/// for the name (`voice::wake`), where there is no whole recording to look
/// at. An energy gate against a noise floor that follows the room, with
/// [`MIN_RUN`] loud frames to start and [`HANG`] quiet ones to stop. It
/// keeps three numbers and allocates nothing.
#[derive(Clone, Debug)]
pub struct Gate {
    floor: f32,
    loud_run: u32,
    quiet_run: u32,
    speaking: bool,
}

impl Default for Gate {
    fn default() -> Self {
        Gate {
            floor: ABS_FLOOR / GATE_OVER,
            loud_run: 0,
            quiet_run: u32::MAX / 2,
            speaking: false,
        }
    }
}

impl Gate {
    /// Takes one 20 ms frame's loudness ([`wav::rms`]) and says whether it
    /// is loud enough to be speech.
    pub fn push(&mut self, rms: f32) -> bool {
        let threshold = (self.floor * GATE_OVER).max(ABS_FLOOR);
        let loud = rms > threshold;
        if rms < self.floor {
            self.floor = rms.max(GATE_FLOOR_MIN);
        } else {
            let rise = if self.speaking {
                RISE_SPEAKING
            } else {
                RISE_QUIET
            };
            self.floor += (rms - self.floor) * rise;
        }
        if loud {
            self.loud_run += 1;
            self.quiet_run = 0;
            if self.loud_run >= MIN_RUN as u32 {
                self.speaking = true;
            }
        } else {
            self.loud_run = 0;
            self.quiet_run = self.quiet_run.saturating_add(1);
            if self.quiet_run >= HANG {
                self.speaking = false;
            }
        }
        loud
    }

    /// Someone is speaking: [`MIN_RUN`] loud frames, and less than [`HANG`]
    /// quiet ones since.
    pub fn speaking(&self) -> bool {
        self.speaking
    }

    /// Quiet frames in a row up to now.
    pub fn quiet(&self) -> u32 {
        self.quiet_run
    }

    /// Loud frames in a row up to now.
    pub fn loud(&self) -> u32 {
        self.loud_run
    }

    /// Forgets the speech, keeps the floor: after a pause in listening.
    pub fn settle(&mut self) {
        self.loud_run = 0;
        self.quiet_run = u32::MAX / 2;
        self.speaking = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_hears_speech_and_learns_the_room() {
        let mut g = Gate::default();
        // A quiet room: nothing.
        for _ in 0..100 {
            assert!(!g.push(0.0008));
        }
        assert!(!g.speaking());
        // A click is not speech.
        g.push(0.2);
        g.push(0.0008);
        assert!(!g.speaking());
        // Speech: three loud frames start it.
        for _ in 0..3 {
            g.push(0.05);
        }
        assert!(g.speaking());
        // The gaps between words do not end it; a fifth of a second does.
        for _ in 0..(HANG - 1) {
            g.push(0.0008);
        }
        assert!(g.speaking());
        g.push(0.0008);
        assert!(!g.speaking());
        assert_eq!(g.quiet(), HANG);
        // A fan comes on and stays: after a few seconds it is the room.
        let mut loud_at_end = true;
        for _ in 0..600 {
            loud_at_end = g.push(0.02);
        }
        assert!(!loud_at_end && !g.speaking(), "{g:?}");
        // Speech over the fan still counts.
        for _ in 0..3 {
            g.push(0.2);
        }
        assert!(g.speaking());
        // A digital silence does not make a hiss speech.
        let mut s = Gate::default();
        for _ in 0..50 {
            s.push(0.0);
        }
        assert!(!s.push(0.002));
    }

    /// `parts` of (seconds, loud): a tone for speech, faint noise for silence.
    fn clip(parts: &[(f32, bool)]) -> Vec<f32> {
        let mut out = Vec::new();
        for (secs, loud) in parts {
            let n = (secs * wav::RATE as f32) as usize;
            for i in 0..n {
                let t = i as f32 / wav::RATE as f32;
                let noise = ((i * 7919) % 101) as f32 / 101.0 - 0.5;
                let v = if *loud {
                    0.3 * (t * 2.0 * std::f32::consts::PI * 220.0).sin()
                } else {
                    0.001 * noise
                };
                out.push(v);
            }
        }
        out
    }

    #[test]
    fn silence_around_the_words_is_cut() {
        let s = clip(&[(1.0, false), (2.0, true), (1.5, false)]);
        let r = speech(&s).unwrap();
        let rate = wav::RATE as usize;
        assert!(r.start > rate / 2 && r.start <= rate, "{r:?}");
        assert!(r.end >= 3 * rate && r.end < 3 * rate + rate / 2, "{r:?}");
        assert_eq!(speech(&clip(&[(2.0, false)])), None, "nothing said");
        assert_eq!(speech(&[]), None);
    }

    #[test]
    fn a_pause_inside_the_utterance_is_a_place_to_cut() {
        let s = clip(&[(2.5, true), (0.8, false), (1.0, true)]);
        let at = last_pause(&s, 50, 25).unwrap();
        let rate = wav::RATE as f32;
        assert!((at as f32 / rate - 2.9).abs() < 0.1, "{at}");
        // Too little speech before it, or a pause too short.
        assert_eq!(last_pause(&s, 200, 25), None);
        assert_eq!(last_pause(&s, 50, 60), None);
        // A pause still going on at the end is not cut.
        assert_eq!(
            last_pause(&clip(&[(2.5, true), (1.0, false)]), 50, 25),
            None
        );
    }
}

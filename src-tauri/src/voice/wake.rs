//! Listening for the name (lanes/wake.md). Off until the person turns it on
//! in Settings or the tray. Then the microphone stays open: "Aevaleron, what
//! is on my screen" opens the bar with "What is on my screen" and sends it,
//! with no key to press and nothing to click.
//!
//! What it costs while it waits, and what it keeps:
//! - The microphone's callback writes into a two-second ring in memory
//!   ([`capture::Ring`]) and allocates nothing. This thread wakes about 33
//!   times a second, reads what is new and runs an energy gate over it
//!   ([`vad::Gate`]). No model runs on silence or on a steady noise.
//! - Nothing is written to disk and nothing is sent anywhere while it waits.
//!   Sound before speech is kept for 0.3 s and then overwritten. Turning the
//!   mode off closes the microphone and drops the ring.
//!
//! When someone starts speaking, the start of what they say (up to the first
//! pause after it, at most 1.8 s) is transcribed on this PC and checked for
//! the name ([`keyword::verdict`]). Not the name: the sound is dropped. The
//! name: the dictation pill shows at once and the rest of the utterance is
//! kept, the name and a pause of up to 2.5 s included, until the person has
//! been quiet for [`END_SILENCE`] frames (0.9 s) or 30 s have passed. Then the
//! whole of it is transcribed with the person's engine, the name comes off,
//! and the line goes to the assistant through [`submit`].
//!
//! Hold-to-talk and the bar's mic come first: while either records or is
//! being written down, what is heard here is ignored. Several utterances in
//! a row without the name (a meeting, a video) make it rest for 30 s and
//! then longer, so the checks never take the processor; and there is never
//! more than one check at a time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{json, Value};

use crate::shell::{async_runtime, AppHandle};

use super::capture::{self, Listen};
use super::keyword::{self, Verdict};
use super::settings::{self, VoiceSettings};
use super::{engine, model, pill, stream, vad, wav, whisper, Origin};

// ------------------------------------------------------------- the timing

/// Every count below is in 20 ms frames.
pub const FRAME_MS: u64 = 20;
/// Kept from before the speech started, so a soft first sound survives.
const PRE: usize = 15;
/// The name is at least this much speech before a pause can end it: 0.45 s.
const CHECK_VOICED: u32 = 22;
/// A pause this long after the name ends the part that is checked: 160 ms.
/// A comma after a name is about that or longer.
const CHECK_PAUSE: u32 = 8;
/// Without a pause, the first 1.8 s are checked.
const CHECK_LEN: usize = 90;
/// After only a "hey", a second look at up to 2.8 s.
const RECHECK_LEN: usize = 140;
/// More speech, after a first look, before the second.
const RECHECK_MORE: u32 = 10;
/// Less speech than this (0.24 s) is a cough or a click, never checked.
const SHORT: u32 = 12;
/// An utterance that ends before it was checked is over after 0.7 s.
const ENDED: u32 = 35;
/// After an utterance without the name, 0.6 s of quiet before listening
/// for the next: the same sentence going on is not a new start.
const REARM: u32 = 30;
/// The input ends after this much quiet: 0.9 s. Pauses inside a sentence
/// (a breath, a comma, finding the word) are mostly under 0.7 s, so this
/// rarely cuts anyone off, and the line goes a moment after they stop. The
/// bar's dictation pieces cut at 0.5 s; this waits longer because cutting
/// here sends.
pub const END_SILENCE: u32 = 45;
/// After the bare name, how long to wait for the rest: 2.5 s.
const BARE_WAIT: u64 = 125;
/// The longest input: 30 s.
const MAX_FRAMES: usize = 1500;
/// Quiet kept after the last word of what is sent: 0.2 s.
const TAIL: u32 = 10;
/// Quiet kept after the part that is checked: 60 ms.
const CHECK_TAIL: u32 = 3;
/// This many utterances without the name inside [`FAIL_WINDOW`] make it rest.
const FAILS: usize = 4;
const FAIL_WINDOW: u64 = 3000;
/// The first rest is 30 s; each one straight after doubles, up to 2 min.
const REST: u64 = 1500;
const REST_MAX: u64 = 6000;
/// Five minutes without an utterance that was not the name.
const CALM: u64 = 15_000;

// -------------------------------------------------------- the state machine

/// Where the listening is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Quiet: the gate runs, nothing else.
    Waiting,
    /// Someone started speaking; not checked yet.
    Hearing,
    /// The start is being checked for the name.
    Checking,
    /// Not the name: waiting for that utterance to end.
    Ignoring,
    /// The name was heard: keeping the rest until they stop.
    Capturing,
    /// The line is being written down and sent.
    Busy,
    /// Too many utterances without the name: no checks for a while.
    Resting,
}

/// What the listener does next.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Check these samples (the start of the utterance) for the name.
    Check(Vec<f32>),
    /// The name: show the pill. `after_ms` is the time since the name ended.
    Heard { after_ms: u64 },
    /// They stopped: write these samples down and send the line.
    Finish(Vec<f32>),
    /// Only the name, then nothing: hide the pill.
    GaveUp,
    /// Resting for `ms`.
    Rest { ms: u64 },
}

/// The listening as a state machine over 20 ms frames, with no threads, no
/// clock and no model: [`Machine::push`] takes each frame, the verdict of a
/// check comes back through [`Machine::checked`]. The unit tests drive it
/// with made-up frames.
pub struct Machine {
    /// Samples in a frame.
    frame: usize,
    gate: vad::Gate,
    phase: Phase,
    /// Frames seen.
    now: u64,
    /// The pre-roll while waiting; the utterance after.
    buf: Vec<f32>,
    /// Loud frames in this utterance.
    voiced: u32,
    voiced_at_cut: u32,
    checks: u8,
    /// The frame the speech before the last check ended at.
    cut_frame: u64,
    /// Loud frames in a row since the check.
    after: u32,
    /// They said something after the name.
    spoke_after: bool,
    fails: Vec<u64>,
    last_fail: Option<u64>,
    rest_until: u64,
    rest_len: u64,
    level: f32,
}

impl Machine {
    pub fn new(frame: usize) -> Machine {
        let frame = frame.max(1);
        Machine {
            frame,
            gate: vad::Gate::default(),
            phase: Phase::Waiting,
            now: 0,
            buf: Vec::with_capacity(2 * PRE * frame),
            voiced: 0,
            voiced_at_cut: 0,
            checks: 0,
            cut_frame: 0,
            after: 0,
            spoke_after: false,
            fails: Vec::with_capacity(FAILS),
            last_fail: None,
            rest_until: 0,
            rest_len: REST,
            level: 0.0,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The last frame's loudness, for the pill's meter.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// One frame of `frame` samples.
    pub fn push(&mut self, frame: &[f32]) -> Option<Action> {
        self.now += 1;
        let rms = wav::rms(frame);
        self.level = wav::level(rms);
        let loud = self.gate.push(rms);
        match self.phase {
            Phase::Busy => None,
            Phase::Resting => {
                self.keep_pre(frame);
                if self.now >= self.rest_until {
                    self.phase = Phase::Ignoring;
                }
                None
            }
            Phase::Ignoring => {
                self.keep_pre(frame);
                if self.gate.quiet() >= REARM {
                    self.phase = Phase::Waiting;
                }
                None
            }
            Phase::Waiting => {
                self.keep_pre(frame);
                if self.gate.speaking() {
                    self.begin();
                }
                None
            }
            Phase::Hearing => {
                self.buf.extend_from_slice(frame);
                if loud {
                    self.voiced += 1;
                }
                self.hearing()
            }
            Phase::Checking => {
                self.grow(frame, loud);
                None
            }
            Phase::Capturing => {
                self.grow(frame, loud);
                self.capturing()
            }
        }
    }

    /// What the check of the start found.
    pub fn checked(&mut self, v: &Verdict) -> Option<Action> {
        if self.phase != Phase::Checking {
            return None;
        }
        match v {
            Verdict::Yes { rest } => {
                if !rest.is_empty() {
                    self.spoke_after = true;
                }
                self.phase = Phase::Capturing;
                self.rest_len = REST;
                self.fails.clear();
                let after_ms = (self.now - self.cut_frame) * FRAME_MS;
                Some(Action::Heard { after_ms })
            }
            Verdict::Unsure if self.checks < 2 => {
                self.phase = Phase::Hearing;
                None
            }
            _ => self.refuse(),
        }
    }

    /// The line was sent (or given up on): listen again once it is quiet.
    pub fn done(&mut self) {
        if self.phase == Phase::Busy {
            self.phase = Phase::Ignoring;
        }
    }

    /// Hold-to-talk or the bar's mic took over: whatever was heard goes.
    pub fn reset(&mut self) {
        self.drop_utterance();
        self.gate.settle();
        if self.phase != Phase::Busy && self.phase != Phase::Resting {
            self.phase = Phase::Waiting;
        }
    }

    fn keep_pre(&mut self, frame: &[f32]) {
        self.buf.extend_from_slice(frame);
        let keep = PRE * self.frame;
        if self.buf.len() >= 2 * keep {
            let from = self.buf.len() - keep;
            self.buf.drain(..from);
        }
    }

    fn begin(&mut self) {
        let keep = (PRE + self.gate.loud() as usize) * self.frame;
        if self.buf.len() > keep {
            let from = self.buf.len() - keep;
            self.buf.drain(..from);
        }
        self.phase = Phase::Hearing;
        self.voiced = self.gate.loud();
        self.voiced_at_cut = 0;
        self.checks = 0;
        self.after = 0;
        self.spoke_after = false;
    }

    fn frames(&self) -> usize {
        self.buf.len() / self.frame
    }

    fn hearing(&mut self) -> Option<Action> {
        let quiet = self.gate.quiet();
        let frames = self.frames();
        if self.checks == 0 {
            if self.voiced >= CHECK_VOICED && quiet >= CHECK_PAUSE {
                return self.check(quiet);
            }
            if frames >= PRE + CHECK_LEN {
                return self.check(0);
            }
            if quiet >= ENDED {
                if self.voiced >= SHORT {
                    return self.check(quiet);
                }
                self.drop_utterance();
                self.phase = Phase::Waiting;
            }
            return None;
        }
        let more = self.voiced - self.voiced_at_cut >= RECHECK_MORE;
        if (more && quiet >= CHECK_PAUSE) || frames >= PRE + RECHECK_LEN {
            return self.check(quiet);
        }
        if quiet >= ENDED {
            // Only "hey", then quiet.
            return self.refuse();
        }
        None
    }

    fn check(&mut self, quiet: u32) -> Option<Action> {
        let drop = quiet.saturating_sub(CHECK_TAIL) as usize * self.frame;
        let upto = self.buf.len().saturating_sub(drop);
        self.cut_frame = self.now - u64::from(quiet);
        self.checks += 1;
        self.voiced_at_cut = self.voiced;
        self.after = 0;
        self.phase = Phase::Checking;
        Some(Action::Check(self.buf[..upto].to_vec()))
    }

    fn grow(&mut self, frame: &[f32], loud: bool) {
        if self.buf.len() < (PRE + MAX_FRAMES) * self.frame {
            self.buf.extend_from_slice(frame);
        }
        if loud {
            self.voiced += 1;
            self.after += 1;
            if self.after >= 3 {
                self.spoke_after = true;
            }
        } else {
            self.after = 0;
        }
    }

    fn capturing(&mut self) -> Option<Action> {
        let quiet = self.gate.quiet();
        if self.frames() >= PRE + MAX_FRAMES {
            return Some(self.finish(0));
        }
        if self.spoke_after && quiet >= END_SILENCE {
            return Some(self.finish(quiet));
        }
        let since = self.now - self.cut_frame;
        if !self.spoke_after && since >= BARE_WAIT && quiet >= END_SILENCE {
            self.drop_utterance();
            self.phase = Phase::Waiting;
            return Some(Action::GaveUp);
        }
        None
    }

    fn finish(&mut self, quiet: u32) -> Action {
        let drop = quiet.saturating_sub(TAIL) as usize * self.frame;
        let upto = self.buf.len().saturating_sub(drop);
        let out = self.buf[..upto].to_vec();
        self.drop_utterance();
        self.phase = Phase::Busy;
        Action::Finish(out)
    }

    fn refuse(&mut self) -> Option<Action> {
        self.drop_utterance();
        let now = self.now;
        self.fails.retain(|t| now - t < FAIL_WINDOW);
        let calm = match self.last_fail {
            Some(t) => now - t >= CALM,
            None => true,
        };
        if calm {
            // Five quiet minutes: the next rest is a short one again.
            self.rest_len = REST;
        }
        self.last_fail = Some(now);
        if self.fails.len() == FAILS {
            self.fails.remove(0);
        }
        self.fails.push(now);
        if self.fails.len() >= FAILS {
            self.fails.clear();
            let len = self.rest_len;
            self.rest_until = now + len;
            self.rest_len = (len * 2).min(REST_MAX);
            self.phase = Phase::Resting;
            return Some(Action::Rest { ms: len * FRAME_MS });
        }
        self.phase = if self.gate.quiet() >= REARM {
            Phase::Waiting
        } else {
            Phase::Ignoring
        };
        None
    }

    /// Lets the utterance go, and the room a long one took.
    fn drop_utterance(&mut self) {
        self.buf.clear();
        let small = 2 * PRE * self.frame;
        if self.buf.capacity() > 4 * small {
            self.buf.shrink_to(small);
        }
        self.voiced = 0;
        self.voiced_at_cut = 0;
        self.checks = 0;
        self.after = 0;
        self.spoke_after = false;
    }
}

// ------------------------------------------------------- what is shown

/// Where listening stands, for Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Off,
    /// The microphone is open and it listens.
    Listening,
    /// On, but not listening yet, and why (the model is downloading).
    Waiting(String),
    /// On, but it cannot listen, and why (no microphone).
    Error(String),
}

static STATUS: Mutex<Status> = parking_lot::const_mutex(Status::Off);
static MIC_OPEN: AtomicBool = AtomicBool::new(false);
/// A check or a line is being worked on.
static WORKING: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();
/// Which listening thread may say how it stands: a thread being stopped
/// does not overwrite the one that replaced it.
static RUN_ID: AtomicU64 = AtomicU64::new(0);

/// Whether the microphone is open for listening: the tray shows its mark.
pub fn listening() -> bool {
    MIC_OPEN.load(Ordering::SeqCst)
}

/// Whether a check or a line is being worked on now.
pub fn working() -> bool {
    WORKING.load(Ordering::SeqCst)
}

/// What Settings shows under the switch.
pub fn status_json() -> Value {
    match &*STATUS.lock() {
        Status::Off => json!({ "state": "off" }),
        Status::Listening => json!({ "state": "listening" }),
        Status::Waiting(m) => json!({ "state": "waiting", "message": m }),
        Status::Error(m) => json!({ "state": "error", "message": m }),
    }
}

fn set_status_for(id: u64, status: Status, mic: bool) {
    if RUN_ID.load(Ordering::SeqCst) == id {
        set_status(status, mic);
    }
}

fn set_status(status: Status, mic: bool) {
    *STATUS.lock() = status;
    let was = MIC_OPEN.swap(mic, Ordering::SeqCst);
    if was != mic {
        if let Some(app) = APP.get() {
            crate::tray::refresh(app);
        }
    }
}

/// How listening went: counts and timings, never words (but in the smoke
/// run, which reads the last check's text).
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub checks: u64,
    pub heard: u64,
    pub refused: u64,
    pub rests: u64,
    pub sent: u64,
    /// The last check: its time and the model file.
    pub check_ms: u64,
    pub check_model: String,
    /// From the end of the name to the pill being asked to show.
    pub heard_ms: u64,
    /// Writing down the whole input.
    pub transcribe_ms: u64,
    /// From the end of the speech to the line being sent, the 0.9 s of
    /// quiet that ends it included.
    pub sent_ms: u64,
    /// Time spent in the listening loop itself, and its passes.
    pub work_us: u64,
    pub loops: u64,
    /// The listening thread's processor time.
    pub cpu_ms: u64,
    pub last_text: String,
}

static STATS: Mutex<Option<Stats>> = parking_lot::const_mutex(None);

fn stats_mut<R>(f: impl FnOnce(&mut Stats) -> R) -> R {
    let mut s = STATS.lock();
    f(s.get_or_insert_with(Stats::default))
}

pub fn stats() -> Stats {
    STATS.lock().clone().unwrap_or_default()
}

pub fn reset_stats() {
    *STATS.lock() = None;
}

// ----------------------------------------------------- the smoke run's hooks

type Queue = std::sync::Arc<Mutex<std::collections::VecDeque<f32>>>;

/// Set by the smoke probe: listening reads this queue instead of the
/// microphone, and lines go to the sink instead of the assistant.
static TEST_QUEUE: Mutex<Option<Queue>> = parking_lot::const_mutex(None);
static SINK: Mutex<Option<Sender<String>>> = parking_lot::const_mutex(None);

/// Listening reads from a queue the probe fills ([`play`]); None gives the
/// microphone back. Takes effect at the next start.
pub fn use_test_source(on: bool) {
    *TEST_QUEUE.lock() = on.then(Queue::default);
}

/// "Speaks" 16 kHz samples into the test source, in real time.
pub fn play(samples: &[f32]) {
    if let Some(q) = TEST_QUEUE.lock().as_ref() {
        q.lock().extend(samples.iter().copied());
    }
}

/// Lines go to `sink` instead of the assistant (the smoke probe).
pub fn set_sink(sink: Option<Sender<String>>) {
    *SINK.lock() = sink;
}

// ------------------------------------------------------------- running it

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    word: String,
    device: String,
    language: String,
    engine: String,
}

impl Key {
    fn of(s: &VoiceSettings) -> Key {
        Key {
            word: s.wake_word.clone(),
            device: s.device.clone(),
            language: s.language.clone(),
            engine: s.engine.clone(),
        }
    }
}

enum Msg {
    Stop,
    Verdict {
        verdict: Verdict,
        ms: u64,
        model: String,
        text: String,
    },
    Done,
}

struct Running {
    key: Key,
    tx: Sender<Msg>,
}

static RUNNING: Mutex<Option<Running>> = parking_lot::const_mutex(None);

/// Called once from voice's setup.
pub fn init(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// Starts, restarts or stops listening to match the settings `s`.
pub fn apply(s: &VoiceSettings) {
    let Some(app) = APP.get() else { return };
    let mut slot = RUNNING.lock();
    if !s.wake {
        if let Some(r) = slot.take() {
            let _ = r.tx.send(Msg::Stop);
        }
        drop(slot);
        RUN_ID.fetch_add(1, Ordering::SeqCst);
        set_status(Status::Off, false);
        return;
    }
    let key = Key::of(s);
    if slot.as_ref().is_some_and(|r| r.key == key) {
        return;
    }
    if let Some(r) = slot.take() {
        let _ = r.tx.send(Msg::Stop);
    }
    let (tx, rx) = mpsc::channel();
    let id = RUN_ID.fetch_add(1, Ordering::SeqCst) + 1;
    let (a, k, t) = (app.clone(), key.clone(), tx.clone());
    let spawned = std::thread::Builder::new()
        .name("voice-wake".into())
        .spawn(move || {
            let out = crate::crash::guard("voice-wake", || run(id, &a, &k, &rx, &t));
            if out.is_none() {
                set_status_for(
                    id,
                    Status::Error("Listening stopped unexpectedly. Turn it off and on.".into()),
                    false,
                );
            }
        });
    if spawned.is_ok() {
        *slot = Some(Running { key, tx });
        pill::prepare(app);
    }
}

/// The tray's switch.
pub fn toggle(app: &AppHandle) {
    let mut s = settings::get();
    s.wake = !s.wake;
    settings::save(&s);
    super::refresh(&s);
    crate::tray::refresh(app);
}

/// The model the check runs on, and whether it is the dictation's own
/// (kept loaded, one session) or one of its own. Parakeet 110M first when it
/// is on disk and the language is English (a check is about a tenth of a
/// Whisper pass), then Whisper tiny, else the dictation's model. Nothing is
/// downloaded for it.
pub fn checker_model(s: &VoiceSettings) -> Option<(PathBuf, bool)> {
    if !whisper::cpu_ok() {
        return None;
    }
    let dictation = engine::usable(s);
    let mut own: Vec<&'static model::Spec> = Vec::new();
    if s.language == "en" {
        own.push(model::parakeet());
    }
    own.push(model::spec_for("tiny", &s.language));
    for spec in own {
        if let Some(p) = model::usable(spec) {
            let shared = dictation.as_ref() == Some(&p);
            return Some((p, shared));
        }
    }
    dictation.map(|p| (p, true))
}

/// Why listening cannot start yet, for Settings.
fn no_model(s: &VoiceSettings) -> String {
    if !whisper::cpu_ok() {
        return "This PC's processor can't run the voice model that listens for the name.".into();
    }
    if engine::spec(s).is_none() {
        return "Listening for the name needs a voice model on this PC. Pick one under Transcription.".into();
    }
    "Listening starts once the voice model has downloaded.".into()
}

enum End {
    Stop,
    Lost(String),
}

/// Waits `d` for a message; true when it says stop.
fn wait_stop(rx: &Receiver<Msg>, d: Duration) -> bool {
    let until = Instant::now() + d;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        match rx.recv_timeout(left) {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => return false,
        }
    }
}

fn run(id: u64, app: &AppHandle, key: &Key, rx: &Receiver<Msg>, tx: &Sender<Msg>) {
    let mut retry = Duration::from_secs(2);
    loop {
        let s = settings::get();
        if checker_model(&s).is_none() {
            set_status_for(id, Status::Waiting(no_model(&s)), false);
            if wait_stop(rx, Duration::from_secs(5)) {
                break;
            }
            continue;
        }
        let queue = TEST_QUEUE.lock().clone();
        let source = match queue {
            Some(q) => Ok(capture::listen_queue(q)),
            None => capture::listen(&key.device),
        };
        let src = match source {
            Ok(l) => l,
            Err(e) => {
                set_status_for(id, Status::Error(e), false);
                if wait_stop(rx, retry) {
                    break;
                }
                retry = (retry * 2).min(Duration::from_secs(30));
                continue;
            }
        };
        retry = Duration::from_secs(2);
        set_status_for(id, Status::Listening, true);
        let checker = Checker::start(tx.clone(), key);
        let end = listen_on(app, key, &src, rx, tx, &checker);
        drop(checker);
        drop(src);
        match end {
            End::Stop => break,
            End::Lost(why) => {
                set_status_for(id, Status::Error(why), false);
                if wait_stop(rx, retry) {
                    break;
                }
            }
        }
    }
    set_status_for(id, Status::Off, false);
}

/// The listening itself, until it is stopped or the microphone goes.
fn listen_on(
    app: &AppHandle,
    key: &Key,
    src: &Listen,
    rx: &Receiver<Msg>,
    tx: &Sender<Msg>,
    checker: &Checker,
) -> End {
    let rate = src.rate;
    let frame = (rate / 50) as usize;
    let mut m = Machine::new(frame);
    let mut scratch: Vec<f32> = Vec::with_capacity(rate as usize);
    let mut cursor = src.ring.lock().written();
    let mut paused = false;
    let mut beats = src.beats();
    let mut beat_at = Instant::now();
    let mut level_at = Instant::now();
    let mut cpu_at = Instant::now();
    let cpu0 = thread_cpu_ms();
    let mut pill_gen = super::generation();
    loop {
        let msg = rx.recv_timeout(Duration::from_millis(30));
        let began = Instant::now();
        match msg {
            Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => {
                if m.phase() == Phase::Capturing {
                    pill::hide(app, Duration::ZERO, pill_gen);
                }
                return End::Stop;
            }
            Ok(Msg::Verdict {
                verdict,
                ms,
                model,
                text,
            }) => {
                stats_mut(|st| {
                    st.check_ms = ms;
                    st.check_model = model;
                    st.last_text = text;
                    match verdict {
                        Verdict::Yes { .. } => st.heard += 1,
                        Verdict::No => st.refused += 1,
                        Verdict::Unsure => {}
                    }
                });
                let next = m.checked(&verdict);
                act(app, key, rate, next, checker, tx, &mut pill_gen);
            }
            Ok(Msg::Done) => m.done(),
            Err(RecvTimeoutError::Timeout) => {}
        }
        if src.broken() {
            return End::Lost("The microphone went away. Trying it again.".into());
        }
        let b = src.beats();
        if b != beats {
            beats = b;
            beat_at = Instant::now();
        } else if beat_at.elapsed() > Duration::from_secs(3) {
            // A stream that stopped without an error: after the PC slept,
            // or a device that was switched.
            return End::Lost("The microphone stopped sending sound. Trying it again.".into());
        }
        src.ring.lock().read(&mut cursor, frame, &mut scratch);
        if super::busy() {
            // Hold-to-talk or the bar's mic: theirs, not this.
            if !paused {
                paused = true;
                m.reset();
            }
        } else {
            if paused {
                paused = false;
                m.reset();
            }
            for f in scratch.chunks_exact(frame) {
                let next = m.push(f);
                act(app, key, rate, next, checker, tx, &mut pill_gen);
            }
            if m.phase() == Phase::Capturing && level_at.elapsed() >= Duration::from_millis(50) {
                level_at = Instant::now();
                let v = json!({ "origin": Origin::Dictation.name(), "level": m.level() });
                crate::windows::emit(app, pill::LABEL, "voice:level", v);
            }
        }
        let spent = began.elapsed().as_micros() as u64;
        let cpu = (cpu_at.elapsed() >= Duration::from_secs(1)).then(|| {
            cpu_at = Instant::now();
            thread_cpu_ms().saturating_sub(cpu0)
        });
        stats_mut(|st| {
            st.work_us += spent;
            st.loops += 1;
            if let Some(c) = cpu {
                st.cpu_ms = c;
            }
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn act(
    app: &AppHandle,
    key: &Key,
    rate: u32,
    action: Option<Action>,
    checker: &Checker,
    tx: &Sender<Msg>,
    pill_gen: &mut u64,
) {
    let Some(action) = action else { return };
    match action {
        Action::Check(samples) => {
            stats_mut(|st| st.checks += 1);
            checker.send(wav::resample(&samples, rate, wav::RATE));
        }
        Action::Heard { after_ms } => {
            *pill_gen = super::claim_pill();
            let shown = Instant::now();
            let v = super::state(Origin::Dictation, "listening", json!({ "wake": true }));
            pill::show(app, v);
            let ms = after_ms + shown.elapsed().as_millis() as u64;
            stats_mut(|st| st.heard_ms = ms);
        }
        Action::Finish(samples) => {
            let v = super::state(Origin::Dictation, "working", json!({ "wake": true }));
            pill::update(app, v);
            let quiet = Duration::from_millis(u64::from(END_SILENCE) * FRAME_MS);
            let spoke_end = Instant::now()
                .checked_sub(quiet)
                .unwrap_or_else(Instant::now);
            let audio = wav::resample(&samples, rate, wav::RATE);
            let (a, k, t, g) = (app.clone(), key.clone(), tx.clone(), *pill_gen);
            WORKING.store(true, Ordering::SeqCst);
            async_runtime::spawn(async move {
                line(&a, &k, audio, spoke_end, g).await;
                WORKING.store(false, Ordering::SeqCst);
                let _ = t.send(Msg::Done);
            });
        }
        Action::GaveUp => pill::hide(app, Duration::ZERO, *pill_gen),
        Action::Rest { .. } => stats_mut(|st| st.rests += 1),
    }
}

/// Writes the input down with the person's engine (the name given to
/// Whisper as its prompt, so it is written the same way each time), takes
/// the name off and sends what is left.
async fn line(app: &AppHandle, key: &Key, audio: Vec<f32>, spoke_end: Instant, gen: u64) {
    let s = settings::get();
    let began = Instant::now();
    let heard = write_down(&s, audio, &key.word).await;
    let took = began.elapsed().as_millis() as u64;
    let text = match heard {
        Ok(t) => t,
        Err(e) => {
            let v = super::state(Origin::Dictation, "error", json!({ "message": e }));
            pill::update(app, v);
            pill::hide(app, Duration::from_millis(2600), gen);
            return;
        }
    };
    let raw = super::cleanup::strip_noise(&text);
    let rest = keyword::strip(&raw, &key.word);
    if rest.is_empty() {
        pill::hide(app, Duration::ZERO, gen);
        stats_mut(|st| st.transcribe_ms = took);
        return;
    }
    let line = super::cleanup::tidy(&rest);
    pill::hide(app, Duration::from_millis(300), gen);
    submit(app, &line);
    let sent = spoke_end.elapsed().as_millis() as u64;
    stats_mut(|st| {
        st.transcribe_ms = took;
        st.sent_ms = sent;
        st.sent += 1;
    });
}

async fn write_down(s: &VoiceSettings, audio: Vec<f32>, word: &str) -> Result<String, String> {
    if engine::spec(s).is_none() {
        // Gemini, as the person chose for their words.
        let done = stream::Done::default();
        let heard = engine::transcribe(s, audio, done, &|_, _| {}).await?;
        return Ok(heard.text);
    }
    let path = engine::usable(s).or_else(|| checker_model(s).map(|c| c.0));
    let Some(path) = path else {
        return Err("The voice model is not on this PC yet.".into());
    };
    let (language, prompt) = (s.language.clone(), word.to_string());
    let run = async_runtime::spawn_blocking(move || match engine::piece(&path, &audio) {
        Some(p) => whisper::transcribe_after(&path, p, &language, &prompt),
        None => Ok(String::new()),
    });
    run.await
        .map_err(|_| "Transcription stopped unexpectedly.".to_string())?
}

/// The one place a heard line goes to the assistant: as if it had been
/// typed in the bar and Enter pressed (`windows::submit_text`). The bar
/// opens and decides it the way it decides typing: a clear task starts,
/// a question is answered.
pub fn submit(app: &AppHandle, line: &str) {
    if let Some(sink) = SINK.lock().as_ref() {
        let _ = sink.send(line.to_string());
        return;
    }
    crate::windows::submit_text(app, line);
}

// ------------------------------------------------------------- the check

/// One check at a time, on a thread of its own, with the model loaded ahead.
struct Checker {
    jobs: Option<Sender<Vec<f32>>>,
}

impl Checker {
    fn start(tx: Sender<Msg>, key: &Key) -> Checker {
        let (jobs, rx) = mpsc::channel::<Vec<f32>>();
        let key = key.clone();
        let _ = std::thread::Builder::new()
            .name("voice-wake-check".into())
            .spawn(move || {
                let _ = crate::crash::guard("voice-wake-check", || check_loop(&rx, &tx, &key));
            });
        Checker { jobs: Some(jobs) }
    }

    fn send(&self, samples: Vec<f32>) {
        if let Some(j) = &self.jobs {
            let _ = j.send(samples);
        }
    }
}

impl Drop for Checker {
    // The sender going ends the thread and lets its model go.
    fn drop(&mut self) {
        self.jobs.take();
    }
}

/// A model of the checker's own, kept while listening.
struct Own {
    path: PathBuf,
    engine: whisper::Engine,
}

fn load_own(own: &mut Option<Own>, path: &PathBuf) {
    if own.as_ref().is_some_and(|o| &o.path == path) {
        return;
    }
    *own = None;
    if let Ok(engine) = whisper::cold(path) {
        *own = Some(Own {
            path: path.clone(),
            engine,
        });
    }
}

fn check_loop(rx: &Receiver<Vec<f32>>, tx: &Sender<Msg>, key: &Key) {
    let mut own: Option<Own> = None;
    // Loaded ahead, so the first check is as quick as any.
    let s = settings::get();
    match checker_model(&s) {
        Some((path, false)) => load_own(&mut own, &path),
        Some((path, true)) => {
            whisper::preload(&path);
        }
        None => {}
    }
    for samples in rx.iter() {
        WORKING.store(true, Ordering::SeqCst);
        let began = Instant::now();
        let s = settings::get();
        let (text, model) = match checker_model(&s) {
            Some((path, shared)) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let text = if shared {
                    own = None;
                    whisper::transcribe_after(&path, &samples, &key.language, &key.word)
                } else {
                    load_own(&mut own, &path);
                    match &own {
                        Some(o) => whisper::run(&o.engine, &samples, &key.language, &key.word),
                        None => Err("The model did not load.".into()),
                    }
                };
                (text, name)
            }
            None => (Err(String::new()), String::new()),
        };
        let verdict = match &text {
            Ok(t) => keyword::verdict(&super::cleanup::strip_noise(t), &key.word),
            Err(_) => Verdict::No,
        };
        let shown = if crate::selftest::smoke_enabled() {
            text.unwrap_or_default()
        } else {
            String::new()
        };
        WORKING.store(false, Ordering::SeqCst);
        let msg = Msg::Verdict {
            verdict,
            ms: began.elapsed().as_millis() as u64,
            model,
            text: shown,
        };
        if tx.send(msg).is_err() {
            break;
        }
    }
}

/// This thread's processor time so far, in ms.
fn thread_cpu_ms() -> u64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
    // SAFETY: the pseudo handle of the calling thread and four out values.
    let ok = unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) };
    if ok == 0 {
        return 0;
    }
    let t = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    (t(k) + t(u)) / 10_000
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 20 ms frames at 16 kHz.
    const F: usize = 320;

    fn loud() -> Vec<f32> {
        (0..F)
            .map(|i| 0.2 * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / 16_000.0).sin())
            .collect()
    }

    fn quiet() -> Vec<f32> {
        (0..F)
            .map(|i| 0.000_5 * (((i * 7919) % 101) as f32 / 101.0 - 0.5))
            .collect()
    }

    /// Pushes `n` frames, and the actions they gave.
    fn run(m: &mut Machine, frame: &[f32], n: usize) -> Vec<Action> {
        (0..n).filter_map(|_| m.push(frame)).collect()
    }

    fn settled() -> Machine {
        let mut m = Machine::new(F);
        assert!(run(&mut m, &quiet(), 50).is_empty());
        assert_eq!(m.phase(), Phase::Waiting);
        m
    }

    fn yes(rest: &str) -> Verdict {
        Verdict::Yes { rest: rest.into() }
    }

    #[test]
    fn the_name_and_a_sentence_in_one_breath() {
        let mut m = settled();
        assert!(run(&mut m, &loud(), 40).is_empty(), "the name, still going");
        assert_eq!(m.phase(), Phase::Hearing);
        // A comma's pause after the name: the start is checked.
        let a = run(&mut m, &quiet(), CHECK_PAUSE as usize);
        let Some(Action::Check(start)) = a.first() else {
            panic!("{a:?}")
        };
        // The 0.3 s before the speech, the speech, a little of the pause.
        let frames = start.len() / F;
        assert!((40..=40 + PRE + 3).contains(&frames), "{frames}");
        assert_eq!(m.phase(), Phase::Checking);
        // They go on talking while it is checked.
        assert!(run(&mut m, &loud(), 50).is_empty());
        let heard = m.checked(&yes("what is on my screen"));
        assert!(matches!(heard, Some(Action::Heard { .. })), "{heard:?}");
        // They stop: 0.9 s later it is finished, with everything in it.
        let a = run(&mut m, &quiet(), END_SILENCE as usize);
        let Some(Action::Finish(all)) = a.last() else {
            panic!("{a:?}")
        };
        assert!(
            all.len() / F >= 40 + CHECK_PAUSE as usize + 50,
            "{}",
            all.len() / F
        );
        assert_eq!(m.phase(), Phase::Busy);
        // While it is sent, nothing is heard; then it listens again.
        assert!(run(&mut m, &loud(), 40).is_empty());
        m.done();
        assert!(run(&mut m, &quiet(), REARM as usize).is_empty());
        assert_eq!(m.phase(), Phase::Waiting);
    }

    #[test]
    fn the_name_then_a_pause_then_the_question() {
        let mut m = settled();
        run(&mut m, &loud(), 40);
        let a = run(&mut m, &quiet(), CHECK_PAUSE as usize);
        assert!(matches!(a.first(), Some(Action::Check(_))));
        let heard = m.checked(&yes(""));
        let Some(Action::Heard { after_ms }) = heard else {
            panic!("{heard:?}")
        };
        assert_eq!(after_ms, u64::from(CHECK_PAUSE) * FRAME_MS);
        // A 1.6 s pause is not the end: the question has not come yet.
        assert!(run(&mut m, &quiet(), 80).is_empty());
        assert!(run(&mut m, &loud(), 60).is_empty());
        let a = run(&mut m, &quiet(), END_SILENCE as usize);
        assert!(matches!(a.last(), Some(Action::Finish(_))), "{a:?}");
    }

    #[test]
    fn the_bare_name_alone_gives_up_quietly() {
        let mut m = settled();
        run(&mut m, &loud(), 40);
        run(&mut m, &quiet(), CHECK_PAUSE as usize);
        m.checked(&yes(""));
        let a = run(&mut m, &quiet(), BARE_WAIT as usize + 5);
        assert_eq!(a, vec![Action::GaveUp]);
        assert_eq!(m.phase(), Phase::Waiting);
    }

    #[test]
    fn not_the_name_drops_the_sound_and_waits_for_the_end() {
        let mut m = settled();
        run(&mut m, &loud(), 40);
        run(&mut m, &quiet(), CHECK_PAUSE as usize);
        run(&mut m, &loud(), 20);
        assert_eq!(m.checked(&Verdict::No), None);
        assert_eq!(m.phase(), Phase::Ignoring);
        // The same sentence going on is not checked again.
        assert!(run(&mut m, &loud(), 100).is_empty());
        assert!(run(&mut m, &quiet(), REARM as usize).is_empty());
        assert_eq!(m.phase(), Phase::Waiting);
        // A verdict that comes late changes nothing.
        assert_eq!(m.checked(&yes("x")), None);
    }

    #[test]
    fn a_cough_is_never_checked() {
        let mut m = settled();
        let mut a = run(&mut m, &loud(), 6);
        a.extend(run(&mut m, &quiet(), 60));
        assert!(a.is_empty(), "{a:?}");
        assert_eq!(m.phase(), Phase::Waiting);
    }

    #[test]
    fn a_hey_alone_gets_a_second_look() {
        let mut m = settled();
        run(&mut m, &loud(), 25);
        let a = run(&mut m, &quiet(), CHECK_PAUSE as usize);
        let Some(Action::Check(first)) = a.first() else {
            panic!("{a:?}")
        };
        let first = first.len();
        assert_eq!(m.checked(&Verdict::Unsure), None);
        assert_eq!(m.phase(), Phase::Hearing);
        run(&mut m, &loud(), 40);
        let a = run(&mut m, &quiet(), CHECK_PAUSE as usize);
        let Some(Action::Check(second)) = a.first() else {
            panic!("{a:?}")
        };
        assert!(second.len() > first + 30 * F);
        // Twice unsure is no.
        m.checked(&Verdict::Unsure);
        assert_ne!(m.phase(), Phase::Checking);
        assert_ne!(m.phase(), Phase::Hearing);
    }

    #[test]
    fn long_speech_is_checked_on_its_start() {
        let mut m = settled();
        let a = run(&mut m, &loud(), 200);
        assert_eq!(a.len(), 1);
        let Some(Action::Check(start)) = a.first() else {
            panic!("{a:?}")
        };
        assert!(start.len() / F <= PRE + CHECK_LEN);
    }

    #[test]
    fn many_starts_without_the_name_make_it_rest() {
        let mut m = settled();
        let mut rested = None;
        for _ in 0..FAILS {
            run(&mut m, &loud(), 40);
            run(&mut m, &quiet(), CHECK_PAUSE as usize);
            rested = m.checked(&Verdict::No);
            run(&mut m, &quiet(), REARM as usize);
        }
        assert_eq!(
            rested,
            Some(Action::Rest {
                ms: REST * FRAME_MS
            })
        );
        assert_eq!(m.phase(), Phase::Resting);
        // While it rests, speech is not checked.
        assert!(run(&mut m, &loud(), 40).is_empty());
        run(&mut m, &quiet(), REST as usize);
        assert_eq!(m.phase(), Phase::Waiting);
        // Straight after, the next rest is twice as long.
        for _ in 0..FAILS {
            run(&mut m, &loud(), 40);
            run(&mut m, &quiet(), CHECK_PAUSE as usize);
            rested = m.checked(&Verdict::No);
            run(&mut m, &quiet(), REARM as usize);
        }
        assert_eq!(
            rested,
            Some(Action::Rest {
                ms: 2 * REST * FRAME_MS
            })
        );
    }

    #[test]
    fn hold_to_talk_takes_over() {
        let mut m = settled();
        run(&mut m, &loud(), 40);
        run(&mut m, &quiet(), CHECK_PAUSE as usize);
        m.checked(&yes("what"));
        run(&mut m, &loud(), 20);
        assert_eq!(m.phase(), Phase::Capturing);
        m.reset();
        assert_eq!(m.phase(), Phase::Waiting);
        // Nothing is finished from what was dropped.
        assert!(run(&mut m, &quiet(), 100).is_empty());
    }

    #[test]
    fn an_input_stops_at_thirty_seconds() {
        let mut m = settled();
        run(&mut m, &loud(), 40);
        run(&mut m, &quiet(), CHECK_PAUSE as usize);
        m.checked(&yes("read this"));
        // Speech, with the dips between words that tell it from a steady
        // tone (which becomes the room's floor and ends the input).
        let mut a = Vec::new();
        for _ in 0..MAX_FRAMES / 28 + 2 {
            a.extend(run(&mut m, &loud(), 25));
            a.extend(run(&mut m, &quiet(), 3));
            if !a.is_empty() {
                break;
            }
        }
        let Some(Action::Finish(all)) = a.last() else {
            panic!("{a:?}")
        };
        assert_eq!(all.len() / F, PRE + MAX_FRAMES);
        // The room it took goes with it.
        assert!(m.buf.capacity() <= 4 * 2 * PRE * F);
    }
}

//! The smoke runner, and the probe API each lane uses to prove its behaviour
//! inside the real exe (BACKLOG K-30).
//!
//! `AEVAL_SMOKE=1` boots the app as usual; then [`scaffold`] runs on a thread
//! of its own. It runs each lane's probe in turn, prints one line per check,
//! and exits 0 when every `must` held or 1 when one did not:
//!
//! ```text
//! [SMOKE] version=1.1.1
//! [SMOKE] scaffold.bridge=ok
//! [SMOKE] outputMulti=FAIL:2 outputs, want 3
//! [SMOKE] probe.windows=ok
//! [SMOKE] failed: outputMulti
//! ```
//!
//! The rust workflow's Smoke step (`tools/smoke-gate.mjs`) turns red on any
//! `=FAIL`, on a missing `[SMOKE] done`, and on any label listed in
//! `tools/smoke-labels/*.txt` that never printed.
//!
//! # Writing a probe
//!
//! A lane owns `probes/<lane>.rs`, whose `pub fn run(cx: &mut Cx)` prints that
//! lane's labels, and `tools/smoke-labels/<lane>.txt`, which lists them, one
//! per line. List a label in the same commit that makes it print; never
//! remove one to get green.
//!
//! - `cx.must(label, ok, why)` is an invariant: `label=ok`, or
//!   `label=FAIL:<why>`, which fails the run.
//! - `cx.log(label, value)` is an observation: `label=<value>`.
//! - `cx.probe(window, key)` asks that window's native page a named question
//!   (`ui::Page::probe`) on the UI thread and returns the answer as JSON; an
//!   error when the window or its page does not exist. Every page answers
//!   `text` (all the text it shows, joined with `\n`) and `visible`; the bar
//!   also `input`, `focused-input`, `intent`, `locked`, `mode`, `notice`,
//!   `status`, `menu` and `examples`; an answer window `messages`,
//!   `streaming`, `title`, `trail`, `card`, `scrollTop` and `atBottom`.
//! - `cx.input(Input::...)` sends real mouse and keyboard input through
//!   `SendInput` at physical pixels: move, click, double and right click,
//!   drag, wheel, key chords and text. `input::cursor()` reads the pointer.
//! - `cx.wait_for(timeout, || cond)` polls every 50 ms until `cond` holds.
//! - `cx.block_on(fut)` runs async code (the llm, desktop and agent APIs);
//!   `cx.on_main(|app| ..)` runs a closure on the main thread and returns
//!   its value.
//! - `cx.notepad()` is a real Notepad holding [`notepad::TEXT`], for probes
//!   that need an app to read, find in, type into or focus. It opens on first
//!   use (on CI, or with `AEVAL_DESKTOP_NOTEPAD=1`; None otherwise), always
//!   before the `v2desktop` lanes, and closes once every lane has run. Leave
//!   it open, and put back any text you change.
//!
//! A probe runs on the smoke thread, never the main thread, so it may block.
//! A panic ends only that probe: it prints `probe.<lane>=FAIL:<message>` and
//! the next lane runs. Each lane also rolls up into one of Electron's block
//! labels ([`probes::BLOCKS`]). The app is the real one with a scratch
//! `AEVAL_USERDATA`, shared by every lane in turn: leave it as you found it
//! (bar collapsed, extra windows closed, settings you changed put back).
//!
//! ```ignore
//! use std::time::Duration;
//!
//! use serde_json::json;
//!
//! use crate::selftest::{Cx, Input};
//!
//! pub fn run(cx: &mut Cx) {
//!     cx.log("model", crate::store::get().get_model());
//!     let shown = cx.probe("output-1", "visible");
//!     cx.must("outputShown", shown == Ok(json!(true)), format!("{shown:?}"));
//!     // Three notches down over the answer, then wait for it to scroll.
//!     let sent = cx.input(Input::Wheel { at: (400, 300), notches: -3 });
//!     let moved = cx.wait_for(Duration::from_secs(2), || {
//!         let top = cx.probe("output-1", "scrollTop");
//!         top.is_ok_and(|v| v.as_f64() > Some(0.0))
//!     });
//!     cx.must("transcriptWheel", sent.is_ok() && moved, format!("{sent:?}"));
//! }
//! ```

pub mod input;
pub mod notepad;
pub mod probes;
mod report;

use std::any::Any;
use std::fmt::Display;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::shell::AppHandle;

pub use input::Input;
pub use notepad::Notepad;
pub use report::Report;

/// `AEVAL_SMOKE` is set: lib.rs starts [`scaffold`].
pub fn smoke_enabled() -> bool {
    std::env::var_os("AEVAL_SMOKE").is_some()
}

/// The smoke run, on its own thread: every lane's probe, then the verdict.
/// Exits 0 when every must held, else 1; a panic outside the probes prints
/// `[SMOKE] error=<message>` and exits 1.
pub fn scaffold(app: AppHandle) {
    let rep = Report::new("SMOKE");
    let run = panic::catch_unwind(AssertUnwindSafe(|| {
        let mut cx = Cx::new(app.clone(), rep.clone());
        probes::run_all(&mut cx);
    }));
    let code = match run {
        Ok(()) => rep.finish(),
        Err(e) => rep.error(&panic_text(&*e)),
    };
    app.exit(code);
}

/// The message a caught panic carried.
pub fn panic_text(e: &(dyn Any + Send)) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic with no message".to_string()
    }
}

/// What a probe gets: the app, the report, and the means to drive the app as
/// a person would.
pub struct Cx {
    pub app: AppHandle,
    pub rep: Report,
    notepad: Option<Notepad>,
    notepad_tried: bool,
}

// The lanes' probes are the callers; the harness's own probe uses part.
#[allow(dead_code)]
impl Cx {
    pub fn new(app: AppHandle, rep: Report) -> Cx {
        Cx {
            app,
            rep,
            notepad: None,
            notepad_tried: false,
        }
    }

    /// `[SMOKE] <label>=ok`, or `=FAIL:<why>`, which fails the run.
    pub fn must(&self, label: &str, ok: bool, why: impl Display) {
        self.rep.must(label, ok, why);
    }

    /// `[SMOKE] <label>=<value>`.
    pub fn log(&self, label: &str, value: impl Display) {
        self.rep.log(label, value);
    }

    /// The native page of window `label` answers `key` (`ui::Page::probe`),
    /// on the UI thread. An error when there is no such page, or the UI
    /// thread did not answer within 10 s.
    pub fn probe(&self, label: &str, key: &str) -> Result<Value, String> {
        let (l, k) = (label.to_string(), key.to_string());
        let got = self.on_main(move |_| {
            crate::ui::page(&l)?;
            Some(crate::ui::probe(&l, &k))
        });
        match got {
            Some(Some(v)) => Ok(v),
            Some(None) => Err(format!("no page {label}")),
            None => Err(format!("the UI thread did not answer for {label}")),
        }
    }

    /// Real mouse or keyboard input through `SendInput`, at physical pixels.
    pub fn input(&self, gesture: Input<'_>) -> Result<(), String> {
        input::send(gesture)
    }

    /// Polls `cond` every 50 ms: true as soon as it holds, false once
    /// `timeout` has passed without it.
    pub fn wait_for(&self, timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if cond() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Runs async code to completion on the app's runtime.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        crate::shell::async_runtime::block_on(fut)
    }

    /// Runs `f` on the main thread and returns its value; None when the
    /// event loop did not run it within 10 s.
    pub fn on_main<R, F>(&self, f: F) -> Option<R>
    where
        R: Send + 'static,
        F: FnOnce(&AppHandle) -> R + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let app = self.app.clone();
        self.app
            .run_on_main_thread(move || {
                let _ = tx.send(f(&app));
            })
            .ok()?;
        rx.recv_timeout(Duration::from_secs(10)).ok()
    }

    pub fn sleep(&self, ms: u64) {
        thread::sleep(Duration::from_millis(ms));
    }

    /// The smoke's own Notepad, opened on first use and printing
    /// `v2NotepadOpen`; None off CI (unless `AEVAL_DESKTOP_NOTEPAD` asks for
    /// it) or when it did not open, which is not tried twice.
    pub fn notepad(&mut self) -> Option<&Notepad> {
        if self.notepad.is_none() && !self.notepad_tried && probes::notepad_enabled() {
            self.notepad_tried = true;
            let opened = self.block_on(notepad::open());
            let why = opened.as_ref().err().cloned().unwrap_or_default();
            self.must("v2NotepadOpen", opened.is_ok(), why);
            self.notepad = opened.ok();
        }
        self.notepad.as_ref()
    }

    /// Closes the Notepad, if one opened: `v2NotepadClosed=true|false`.
    fn close_notepad(&mut self) {
        if let Some(np) = self.notepad.take() {
            let closed = self.block_on(np.close());
            self.log("v2NotepadClosed", closed);
        }
    }
}

/// The middle of the bar's field in physical pixels (the page says where it
/// is); the middle of the bar's rect if the page cannot say.
pub fn bar_field_px(cx: &Cx) -> (i32, i32) {
    if let Ok(v) = cx.probe(crate::windows::OVERLAY, "field-at") {
        if let (Some(x), Some(y)) = (v[0].as_i64(), v[1].as_i64()) {
            return (x as i32, y as i32);
        }
    }
    let r = crate::windows::screen::dip_to_px(crate::windows::bar_rect(&cx.app));
    let x = (r.x + r.width / 2.0).round() as i32;
    let y = (r.y + r.height / 2.0).round() as i32;
    (x, y)
}

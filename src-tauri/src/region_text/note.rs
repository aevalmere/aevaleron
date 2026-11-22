//! The confirmation for "Copy text from screen": a small capsule beside the
//! rectangle that says what happened ("Copied 42 words" and the first few of
//! them) and goes away by itself. It never takes the keyboard focus (the
//! person is still in their app), lets clicks through, and stays out of
//! screenshots.
//!
//! The window is built hidden when the picker opens, so the note shows the
//! moment the text is on the clipboard, and is kept hidden afterwards. The
//! page says when its listener is in place (`region:note-ready`); what was
//! to be shown waits for that.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::capture::{self, screen, PxRect};
use crate::desktop::win;
use crate::desktop::DesktopError;
use crate::shell::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

use super::assemble;

pub const LABEL: &str = "region-note";
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 92.0;
/// Between the rectangle and the note's window; the capsule sits a little
/// inside its window.
const GAP: f64 = 4.0;
/// The capsule's fade in the page (region-note.css: 150 ms of opacity, 240 ms
/// of travel) and the frame after it.
const FADE: Duration = Duration::from_millis(260);
/// How long each kind of note stays: a glance for the count, longer for
/// something that has to be read.
const COPIED_MS: u64 = 2200;
const NONE_MS: u64 = 3200;
const ERROR_MS: u64 = 6000;

/// What the note says. `ms` is how long it stays; 0 stays until the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub kind: &'static str,
    pub title: String,
    pub lead: String,
    pub ms: u64,
}

impl Note {
    fn new(kind: &'static str, title: &str, lead: &str, ms: u64) -> Note {
        Note {
            kind,
            title: title.to_string(),
            lead: lead.to_string(),
            ms,
        }
    }

    /// The text is on the clipboard: how much, and how it starts.
    pub fn copied(text: &str) -> Note {
        let count = assemble::count_label(text);
        Note::new("copied", &count, &assemble::lead(text), COPIED_MS)
    }

    /// Nothing readable in the rectangle.
    pub fn none() -> Note {
        let lead = "The clipboard is unchanged.";
        Note::new("none", "No text found here", lead, NONE_MS)
    }

    /// Reading takes long enough to say so (a cold helper, a slow app).
    pub fn reading() -> Note {
        Note::new("reading", "Reading the text…", "", 0)
    }

    /// The clipboard would not take the text.
    pub fn not_copied() -> Note {
        let lead = "Another app is using the clipboard. Try again.";
        Note::new("error", "The text could not be copied", lead, ERROR_MS)
    }

    /// Reading failed: what happened and what to do.
    pub fn failed(e: &DesktopError) -> Note {
        if e.code == "NO_OCR" {
            let title = "Windows cannot read text in pictures yet";
            let lead = "Add a language with text recognition in Windows Settings, then try again.";
            return Note::new("error", title, lead, ERROR_MS);
        }
        if e.code == crate::privacy::CODE {
            let title = "This app is on your private apps list";
            let lead = "Nothing was copied. Settings, Privacy has the list.";
            return Note::new("error", title, lead, ERROR_MS);
        }
        let lead = "Nothing was copied. Try again.";
        Note::new("error", "The text could not be read", lead, ERROR_MS)
    }

    /// One sentence for a screen reader.
    pub fn spoken(&self) -> String {
        match self.kind {
            "copied" => format!("{}.", self.title),
            _ => format!("{}. {}", self.title, self.lead),
        }
    }

    fn payload(&self) -> Value {
        json!({
            "kind": self.kind,
            "title": self.title,
            "lead": self.lead,
            "ms": self.ms,
        })
    }
}

struct State {
    /// The page has its listener.
    ready: bool,
    /// What to show once it has.
    waiting: Option<Value>,
    /// Counts notes, so an older note's timer never hides a newer one.
    seq: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    ready: false,
    waiting: None,
    seq: 0,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn create(app: &AppHandle) -> crate::shell::Result<crate::shell::WebviewWindow> {
    state().ready = false;
    let win = WebviewWindowBuilder::new(
        app,
        LABEL,
        WebviewUrl::App("renderer/region-note.html".into()),
    )
    .title("Aevaleron copy text")
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .resizable(false)
    .focusable(false)
    .focused(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .content_protected(true)
    .visible(false)
    .build()?;
    let raw = capture::hwnd_of(&win);
    win::tool_window(raw, true);
    win::set_mouse_through(raw, true);
    capture::exclude_window(raw);
    Ok(win)
}

/// Builds the window, hidden, if it is not there. Never on the event loop
/// thread: WebView2 creation waits on it.
pub fn prepare(app: &AppHandle) {
    if app.get_webview_window(LABEL).is_some() {
        return;
    }
    if let Err(e) = create(app) {
        eprintln!("the copy-text note did not open: {e}");
    }
}

/// Where the note goes, in physical pixels: centred under the rectangle,
/// above it when there is no room below, and inside its bottom edge when
/// the rectangle fills the screen. Always inside the work area.
pub fn place_near(area: PxRect, work: PxRect, scale: f64) -> PxRect {
    let w = (WIDTH * scale).round() as i32;
    let h = (HEIGHT * scale).round() as i32;
    let gap = (GAP * scale).round() as i32;
    let centred = area.x + area.width / 2 - w / 2;
    let x = centred.min(work.x + work.width - w).max(work.x);
    let (top, bottom) = (work.y, work.y + work.height);
    let below = area.y + area.height + gap;
    let above = area.y - gap - h;
    let y = if below + h <= bottom {
        below
    } else if above >= top {
        above
    } else {
        (bottom - h - gap).max(top)
    };
    PxRect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Sends what waits, once the page can hear it.
fn flush(app: &AppHandle) {
    let held = {
        let mut s = state();
        if s.ready {
            s.waiting.take()
        } else {
            None
        }
    };
    if let Some(payload) = held {
        crate::windows::emit(app, LABEL, "region:note", payload);
    }
}

/// `region:note-ready`: the page has its listener.
pub fn page_ready(app: &AppHandle) {
    state().ready = true;
    flush(app);
}

/// Shows `note` beside `area` on `display`, without taking the focus, and
/// takes it down again after its time.
pub fn show(app: &AppHandle, area: PxRect, display: &screen::Display, note: &Note) {
    prepare(app);
    let seq = {
        let mut s = state();
        s.seq += 1;
        s.waiting = Some(note.payload());
        s.seq
    };
    let at = place_near(area, display.work_phys, display.scale_factor);
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(w) = a.get_webview_window(LABEL) {
            win::place(&w, at);
            win::show_inactive(capture::hwnd_of(&w));
        }
    });
    flush(app);
    if note.ms > 0 {
        leave(app, seq, Duration::from_millis(note.ms));
    }
}

/// Hides the window once note `seq` has had its time and the page has faded
/// it: a window hidden with the capsule still drawn would show that frame
/// for a moment under the next note.
fn leave(app: &AppHandle, seq: u64, after: Duration) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(after + FADE);
        let a = app.clone();
        let _ = app.run_on_main_thread(move || {
            if state().seq != seq {
                return;
            }
            if let Some(w) = a.get_webview_window(LABEL) {
                win::hide(capture::hwnd_of(&w));
            }
        });
    });
}

/// Whether the note shows now (the smoke run asks).
pub fn visible(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(x: i32, y: i32, width: i32, height: i32) -> PxRect {
        PxRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_note_sits_under_the_rectangle() {
        let work = px(0, 0, 1920, 1040);
        let r = place_near(px(300, 200, 200, 100), work, 1.0);
        assert_eq!(r, px(210, 304, 380, 92));
        // At 150 % everything is half as large again.
        let r = place_near(px(300, 200, 200, 100), work, 1.5);
        assert_eq!(r, px(115, 306, 570, 138));
    }

    #[test]
    fn it_goes_above_when_there_is_no_room_below() {
        let work = px(0, 0, 1920, 1040);
        let r = place_near(px(300, 800, 200, 200), work, 1.0);
        assert_eq!((r.x, r.y), (210, 704));
        // The rectangle is the whole screen: inside its bottom edge.
        let r = place_near(px(0, 0, 1920, 1080), work, 1.0);
        assert_eq!((r.x, r.y), (770, 944));
    }

    #[test]
    fn it_stays_inside_the_work_area() {
        let work = px(-1280, 0, 1280, 984);
        // Against the left edge of a display left of the primary.
        let r = place_near(px(-1280, 100, 40, 40), work, 1.0);
        assert_eq!((r.x, r.y), (-1280, 144));
        // Against the right edge.
        let r = place_near(px(-60, 100, 60, 40), work, 1.0);
        assert_eq!(r.x + r.width, 0);
        // A work area smaller than the note still starts inside it.
        let r = place_near(px(10, 10, 20, 20), px(0, 0, 200, 60), 1.0);
        assert_eq!((r.x, r.y), (0, 0));
    }

    #[test]
    fn the_words_say_what_happened_and_what_to_do() {
        let n = Note::copied("Invoice 2291 for March\nPlease settle by Friday");
        assert_eq!((n.kind, n.title.as_str()), ("copied", "Copied 8 words"));
        assert_eq!(n.lead, "Invoice 2291 for March Please settle…");
        assert_eq!(n.spoken(), "Copied 8 words.");
        let n = Note::none();
        assert_eq!(
            n.spoken(),
            "No text found here. The clipboard is unchanged."
        );
        let no_ocr = DesktopError::new("NO_OCR", "no Windows OCR language is installed");
        assert!(Note::failed(&no_ocr).lead.contains("Windows Settings"));
        let down = DesktopError::new("HELPER_DOWN", "the helper is not running");
        assert_eq!(Note::failed(&down).lead, "Nothing was copied. Try again.");
        // Only the reading note has no end of its own.
        assert_eq!(Note::reading().ms, 0);
        for n in [Note::none(), Note::not_copied(), Note::failed(&down)] {
            assert!(n.ms > COPIED_MS, "{n:?}");
        }
    }
}

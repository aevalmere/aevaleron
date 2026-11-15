//! Copy text from screen: the person drags a rectangle over anything (a
//! picture, a video frame, a dialog whose text cannot be selected) and the
//! text inside it is on the clipboard.
//!
//! Exact text is preferred. When the rectangle lies over one window and UI
//! Automation gives text for the lines or elements inside it, that text is
//! used, in reading order. Otherwise the rectangle is read with Windows'
//! own OCR, the engine Find uses. Both run in the desktop helper on this PC.
//!
//! Nothing is sent to a model and nothing is written to disk: the text goes
//! from the helper to the clipboard and is forgotten. This module calls no
//! model, question, Find or task code, and a test below holds it to that.
//!
//! - `assemble.rs`: the pieces read, to the text copied (pure, unit-tested).
//! - `note.rs`: the confirmation beside the rectangle.

mod assemble;
pub mod note;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_clipboard_manager::ClipboardExt;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
};

use crate::capture::screen::{self, PxRect};
use crate::desktop::DesktopError;
use crate::region;
use crate::state::{shared, Mode};

use assemble::{Piece, Source};
use note::Note;

/// The most text UI Automation is asked for.
const EXACT_MAX: u64 = 20_000;
/// A read that takes longer than this says it is reading.
const SLOW: Duration = Duration::from_millis(700);
/// After the picker's window is gone, before the screen is asked what is
/// under the rectangle.
const SETTLE: Duration = Duration::from_millis(60);

/// One copy at a time: a request while a pick or a read is under way is
/// dropped.
static BUSY: AtomicBool = AtomicBool::new(false);

struct Busy;

impl Busy {
    fn take() -> Option<Busy> {
        (!BUSY.swap(true, Ordering::SeqCst)).then_some(Busy)
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

/// What a copy did. Never the text itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// `copied`, `none` or `error`.
    pub kind: &'static str,
    /// `exact` (UI Automation) or `ocr`; empty when reading failed.
    pub source: &'static str,
    /// The note's first line, `Copied 42 words`.
    pub title: String,
    pub read_ms: u64,
}

static LAST: Mutex<Option<Outcome>> = Mutex::new(None);

/// What the last copy did (the smoke run asks).
pub fn last_outcome() -> Option<Outcome> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Forgets the last outcome, so the next one is known to be new.
pub fn clear_outcome() {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The action behind the hotkey and the tray item: the picker opens on the
/// screen the person is on, and what they drag over is copied. Escape, or a
/// drag too small to mean anything, changes nothing.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run(&app).await;
    });
}

async fn run(app: &AppHandle) {
    // As the Ask pick: never over the HUD, never two pickers at once.
    if region::is_active(app) || shared(app).lock().mode == Mode::Hud {
        return;
    }
    let Some(_busy) = Busy::take() else {
        return;
    };
    // The private-apps gate, when that lane lands, is called here, before
    // the picker opens: this path never passes through `region::pick`.
    let display = screen::active_display(&screen::all_displays());
    let front = foreground();
    // Built now, hidden, so the note shows the moment the text is copied.
    note::prepare(app);
    let picked = region::select_text(app, Some(display.clone())).await;
    give_back(front);
    let Some(rect) = picked else {
        return;
    };
    tokio::time::sleep(SETTLE).await;
    copy_rect(app, display.rect_to_phys(rect), &display).await;
}

/// The step after the picker: reads the text inside `px` (physical pixels on
/// `display`), puts it on the clipboard and says so beside the rectangle.
/// When nothing readable is there, or reading fails, the clipboard is left
/// as it was and the note says that instead.
pub async fn copy_rect(app: &AppHandle, px: PxRect, display: &screen::Display) -> Outcome {
    let started = Instant::now();
    let reading = read(px);
    tokio::pin!(reading);
    let got = tokio::select! {
        got = &mut reading => got,
        _ = tokio::time::sleep(SLOW) => {
            note::show(app, px, display, &Note::reading());
            reading.await
        }
    };
    let read_ms = started.elapsed().as_millis() as u64;
    let (say, kind, source) = match got {
        Ok((text, source)) if !text.is_empty() => {
            // Windows' clipboard text has CRLF line ends.
            let crlf = text.replace('\n', "\r\n");
            match app.clipboard().write_text(crlf) {
                Ok(()) => (Note::copied(&text), "copied", source.name()),
                Err(e) => {
                    eprintln!("copy text: the clipboard refused: {e}");
                    (Note::not_copied(), "error", source.name())
                }
            }
        }
        Ok((_, source)) => (Note::none(), "none", source.name()),
        Err(e) => {
            eprintln!("copy text: {}: {}", e.code, e.message);
            (Note::failed(&e), "error", "")
        }
    };
    note::show(app, px, display, &say);
    crate::windows::announce(app, &say.spoken(), "polite");
    let outcome = Outcome {
        kind,
        source,
        title: say.title,
        read_ms,
    };
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome.clone());
    outcome
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Exact => "exact",
            Source::Ocr => "ocr",
        }
    }
}

/// The text inside `px` and where it came from; empty when nothing readable
/// is there.
async fn read(px: PxRect) -> Result<(String, Source), DesktopError> {
    let desktop = crate::desktop::client();
    let rect = json!({ "x": px.x, "y": px.y, "width": px.width, "height": px.height });
    let (x, y) = (f64::from(px.x), f64::from(px.y));
    let clip = Piece::new("", x, y, f64::from(px.width), f64::from(px.height));
    if let Some(window) = window_under(px).await? {
        // A refusal here (one of our own windows, an elevated app, a tree
        // too slow to walk) leaves the rectangle to OCR.
        let exact = desktop.get_text(Some(&window), &rect, EXACT_MAX, false);
        if let Ok(got) = exact.await {
            let text = assemble::text_in(pieces(&got["spans"]), &clip, Source::Exact);
            if !text.is_empty() {
                return Ok((text, Source::Exact));
            }
        }
    }
    let got = desktop.ocr(&rect, None, "auto").await?;
    let text = assemble::text_of(pieces(&got["spans"]), Source::Ocr);
    Ok((text, Source::Ocr))
}

/// The helper's spans (`{text, rect {x, y, width, height}}`) as pieces. A
/// span without a rectangle cannot be placed and is left out.
fn pieces(spans: &Value) -> Vec<Piece> {
    let Some(list) = spans.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|s| {
            let n = |k: &str| s["rect"][k].as_f64();
            let text = s["text"].as_str()?;
            Some(Piece::new(
                text,
                n("x")?,
                n("y")?,
                n("width")?,
                n("height")?,
            ))
        })
        .collect()
}

/// The middle of the rectangle and a point just inside each corner.
fn sample_points(px: PxRect) -> [(i32, i32); 5] {
    let inset = |len: i32| (len / 8).clamp(1, 12);
    let (dx, dy) = (inset(px.width), inset(px.height));
    let (left, top) = (px.x + dx, px.y + dy);
    let right = px.x + px.width - 1 - dx;
    let bottom = px.y + px.height - 1 - dy;
    [
        (px.x + px.width / 2, px.y + px.height / 2),
        (left, top),
        (right, top),
        (left, bottom),
        (right, bottom),
    ]
}

/// The one window under the whole rectangle, as the helper names it. None
/// when the rectangle lies over more than one: then no single app's text is
/// what the person sees there, and the pixels are read instead.
async fn window_under(px: PxRect) -> Result<Option<Value>, DesktopError> {
    let desktop = crate::desktop::client();
    let mut found: Option<String> = None;
    for (x, y) in sample_points(px) {
        let point = json!({ "point": { "x": x, "y": y } });
        let at = desktop.window_states(point).await?;
        let Some(handle) = at["atPoint"].as_str().filter(|h| !h.is_empty()) else {
            return Ok(None);
        };
        match &found {
            None => found = Some(handle.to_string()),
            Some(first) if first.as_str() != handle => return Ok(None),
            Some(_) => {}
        }
    }
    Ok(found.map(Value::String))
}

fn foreground() -> isize {
    // SAFETY: a query.
    unsafe { GetForegroundWindow() as isize }
}

/// Gives the keyboard back to the window that had it before the picker took
/// it, when Windows left it with one of ours or with nothing. A person who
/// clicked into another app meanwhile keeps that app.
fn give_back(to: isize) {
    // SAFETY: queries on handles that may be stale, and a request Windows
    // may refuse.
    unsafe {
        let now = GetForegroundWindow();
        if to == 0 || now as isize == to || IsWindow(to as HWND) == 0 {
            return;
        }
        let mut pid = 0u32;
        if !now.is_null() {
            GetWindowThreadProcessId(now, &mut pid);
        }
        if now.is_null() || pid == std::process::id() {
            SetForegroundWindow(to as HWND);
        }
    }
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
    fn the_sample_points_are_inside_the_rectangle() {
        for r in [px(100, 50, 400, 200), px(-900, 20, 13, 12), px(0, 0, 1, 1)] {
            for (x, y) in sample_points(r) {
                assert!(r.contains(x, y) || r.width < 3, "{r:?} {x},{y}");
            }
        }
        let p = sample_points(px(100, 50, 400, 200));
        assert_eq!(p[0], (300, 150));
        assert_eq!((p[1], p[4]), ((112, 62), (487, 237)));
    }

    #[test]
    fn spans_become_pieces() {
        let spans = json!([
            { "text": "total", "rect": { "x": 20, "y": 140, "width": 50, "height": 13 }, "source": "ocr" },
            { "text": "no place", "rect": null },
            { "rect": { "x": 1, "y": 2, "width": 3, "height": 4 } },
        ]);
        assert_eq!(
            pieces(&spans),
            vec![Piece::new("total", 20.0, 140.0, 50.0, 13.0)]
        );
        assert!(pieces(&Value::Null).is_empty());
    }

    #[test]
    fn one_copy_at_a_time() {
        let first = Busy::take();
        assert!(first.is_some());
        assert!(Busy::take().is_none());
        drop(first);
        assert!(Busy::take().is_some());
    }

    /// The promise at the top of this file: these sources name no module
    /// that talks to a model, keeps history or settings, or writes files.
    #[test]
    fn nothing_here_reaches_a_model_or_the_disk() {
        let sources = [
            ("mod.rs", include_str!("mod.rs")),
            ("assemble.rs", include_str!("assemble.rs")),
            ("note.rs", include_str!("note.rs")),
        ];
        let modules = [
            "llm", "host", "ask", "agent", "agents", "find", "history", "store", "skills",
        ];
        let mut banned: Vec<String> = modules.iter().map(|m| format!("crate::{m}")).collect();
        banned.push(["std", "fs"].join("::"));
        banned.push(["std", "io"].join("::"));
        banned.push(["crash", "write"].join("::"));
        banned.push(["services", "()"].concat());
        for (file, text) in sources {
            for path in &banned {
                let named = text.match_indices(path.as_str()).any(|(i, _)| {
                    let next = text[i + path.len()..].chars().next();
                    !next.is_some_and(|c| c.is_alphanumeric() || c == '_')
                });
                assert!(!named, "{file} names {path}");
            }
        }
    }
}

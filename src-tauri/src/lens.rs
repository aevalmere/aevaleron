//! The lens (lib/lens.js, PARITY LENS-01/02, GLASS-09): a sheet over the
//! display the agent controls that draws the control frame and its chip
//! while a task runs. One window per display that has a frame, sized to the
//! display, click-through, never focusable, and excluded from capture, so a
//! presenter's audience never sees it.

use std::collections::HashSet;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::capture::{self, screen};
use crate::desktop::win;
use crate::shell::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use crate::windows::flags;

#[derive(Default)]
struct State {
    frame: Option<(u32, String)>,
    hidden: bool,
    ready: HashSet<String>,
    /// Windows on their way out; each still holds its label until it is gone.
    closing: HashSet<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

pub fn label_for(display_id: u32) -> String {
    format!("lens-{display_id}")
}

/// What one display's lens is told to draw (`lens:draw`).
pub fn draw_payload(frame: Option<&(u32, String)>, display: &screen::Display) -> Value {
    json!({
        "frame": match frame {
            Some((id, chip)) if *id == display.id => json!({ "chip": chip }),
            _ => Value::Null,
        },
    })
}

fn create(app: &AppHandle, d: &screen::Display) -> crate::shell::Result<WebviewWindow> {
    let label = label_for(d.id);
    let win = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("renderer/lens.html".into()))
        .title("Aevaleron")
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
        .on_page_load(|w, p| {
            // A reload starts over. `lens:ready` ends the wait, so a frame
            // set while the page loaded is drawn once it can hear it.
            if matches!(p.event(), crate::shell::webview::PageLoadEvent::Started) {
                with(|s| s.ready.remove(w.label()));
            }
        })
        .build()?;
    let raw = capture::hwnd_of(&win);
    win::tool_window(raw, true);
    win::set_mouse_through(raw, true);
    win::place(&win, d.phys);
    capture::exclude_window(raw);
    win.on_window_event(move |e| {
        if matches!(e, crate::shell::WindowEvent::Destroyed) {
            with(|s| s.closing.remove(&label));
        }
    });
    Ok(win)
}

/// `lens:ready`: the page has its listeners, so what waited can be drawn.
pub fn page_ready(app: &AppHandle, label: &str) {
    with(|s| s.ready.insert(label.to_string()));
    redraw(app);
}

pub fn is_ready(label: &str) -> bool {
    with(|s| s.ready.contains(label))
}

/// Waits up to a second for a window on its way out to be gone. Only ever on
/// a `draw` thread.
fn wait_out_close(app: &AppHandle, label: &str) {
    for _ in 0..50 {
        if !with(|s| s.closing.contains(label)) || app.get_webview_window(label).is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    with(|s| s.closing.remove(label));
}

/// `draw` on its own thread. Windows are never built on the event loop
/// thread (WebView2 creation waits on it), and callers include sync IPC
/// handlers and page-load callbacks that run there.
pub fn redraw(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || draw(&app));
}

static DRAW: Mutex<()> = Mutex::new(());

/// Brings every display's lens in line with the frame.
pub fn draw(app: &AppHandle) {
    let _one = DRAW.lock().unwrap_or_else(|e| e.into_inner());
    let (frame, hidden, ready) = with(|s| (s.frame.clone(), s.hidden, s.ready.clone()));
    for d in screen::all_displays() {
        let label = label_for(d.id);
        let drawn = frame.as_ref().is_some_and(|(id, _)| *id == d.id) && !hidden;
        if drawn {
            wait_out_close(app, &label);
        }
        let existing = app.get_webview_window(&label);
        if !drawn {
            if let Some(w) = existing {
                // Dropped, not hidden: a hidden page keeps the last frame it
                // painted and shows it for a moment when the window comes
                // back (ARCHITECTURE.md 10.2). The next draw waits for the
                // label (`wait_out_close`) and builds a new one.
                with(|s| {
                    s.ready.remove(&label);
                    s.closing.insert(label.clone());
                });
                let _ = w.destroy();
            }
            continue;
        }
        let win = match existing {
            Some(w) => {
                win::place(&w, d.phys);
                w
            }
            None => match create(app, &d) {
                Ok(w) => w,
                Err(_) => continue,
            },
        };
        if ready.contains(&label) {
            let payload = draw_payload(frame.as_ref(), &d);
            let _ = app.emit_to(label.as_str(), "lens:draw", payload);
        }
        let raw = capture::hwnd_of(&win);
        if !win.is_visible().unwrap_or(false) {
            win::show_inactive(raw);
        }
        win::raise(raw);
    }
}

/// The agent's control frame on one display, or `None` to take it down.
pub fn set_frame(app: &AppHandle, display: Option<&screen::Display>, chip: &str) {
    with(|s| s.frame = display.map(|d| (d.id, chip.to_string())));
    redraw(app);
}

// lens.js export; Called by the smoke and shot harnesses (agent e), not merged yet.
#[allow(dead_code)]
pub fn set_hidden(app: &AppHandle, on: bool) {
    with(|s| s.hidden = on);
    redraw(app);
}

fn all_windows(app: &AppHandle) -> Vec<WebviewWindow> {
    app.webview_windows()
        .into_iter()
        .filter(|(l, _)| l.starts_with("lens-"))
        .map(|(_, w)| w)
        .collect()
}

/// The lens windows on screen. A hidden one must not count as something the
/// backdrop has to keep fresh (B-030).
pub fn windows(app: &AppHandle) -> Vec<WebviewWindow> {
    all_windows(app)
        .into_iter()
        .filter(|w| flags::is_visible(capture::hwnd_of(w)))
        .collect()
}

pub fn destroy(app: &AppHandle) {
    let list = all_windows(app);
    with(|s| {
        s.ready.clear();
        s.closing.extend(list.iter().map(|w| w.label().to_string()));
    });
    for w in list {
        let _ = w.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d() -> screen::Display {
        screen::Display::from_phys(
            r"\\.\DISPLAY2",
            false,
            96,
            screen::PxRect {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
            screen::PxRect {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080,
            },
        )
    }

    #[test]
    fn frame_goes_to_its_own_display_only() {
        let disp = d();
        let p = draw_payload(Some(&(disp.id, "chip".into())), &disp);
        assert_eq!(p["frame"]["chip"], "chip");
        let p = draw_payload(Some(&(disp.id + 1, "chip".into())), &disp);
        assert_eq!(p["frame"], Value::Null);
        assert_eq!(draw_payload(None, &disp)["frame"], Value::Null);
    }
}

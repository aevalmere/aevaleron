//! The pages, native (lane native-ui). Each window that held an HTML page
//! holds a Slint component now, driven by a Rust page module that does
//! what the page's script did: it hears the same channels (`on_event`,
//! with the same JSON payloads) and calls the same `ipc` channels
//! (`bridge`), so the core did not change shape.
//!
//! Everything here runs on the UI thread. A page lives in `PAGES` under its
//! window's label from `build` until its window is destroyed.

pub mod bridge;
pub mod intent;
pub mod markdown;

mod bar;
mod cards;
mod history;
mod output;
mod overlays;
mod settings;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use serde_json::Value;
use slint::ComponentHandle;

use crate::shell::platform::{self, KeyPress, Spec};
use crate::shell::{self, webview, BuildSpec, Window};
use crate::state::Rect;

slint::include_modules!();

/// A page that only draws its component and hears nothing yet: the stubs
/// of pages still to be ported, and the blank fallback.
/// `simple_page!(Name, Component)` makes `Name::new(label, spec)`.
#[macro_export]
macro_rules! simple_page {
    ($name:ident, $component:ty) => {
        pub struct $name {
            hwnd: isize,
            ui: $component,
        }

        impl $name {
            pub fn new(
                _label: &str,
                spec: $crate::shell::platform::Spec,
            ) -> Result<std::rc::Rc<Self>, String> {
                let (ui, hwnd) = $crate::ui::create(spec, <$component>::new)?;
                Ok(std::rc::Rc::new($name { hwnd, ui }))
            }
        }

        impl $crate::ui::Page for $name {
            fn hwnd(&self) -> isize {
                self.hwnd
            }

            fn on_event(&self, _channel: &str, _payload: &serde_json::Value) {}

            fn theme(&self, dark: bool, text_scale: f32) {
                $crate::ui::apply_theme(&self.ui, dark, text_scale);
            }
        }
    };
}

/// What a page module does for its window.
pub trait Page {
    /// The window this page draws into.
    fn hwnd(&self) -> isize;
    /// An event Rust sent this window (`emit_to(label, channel, payload)`).
    fn on_event(&self, channel: &str, payload: &Value);
    /// The light or dark theme, and the text size, changed.
    fn theme(&self, dark: bool, text_scale: f32);
    /// What the smoke run asks about the page (`selftest::Cx::probe`):
    /// a named fact, as JSON; Null for one the page does not know.
    fn probe(&self, _key: &str) -> Value {
        Value::Null
    }
    /// The window is going: stop timers, drop what holds the component.
    fn close(&self) {}
}

thread_local! {
    static PAGES: RefCell<HashMap<String, Rc<dyn Page>>> = RefCell::new(HashMap::new());
    static DARK: Cell<bool> = Cell::new(shell::sys::dark_mode());
    static SCALES: RefCell<HashMap<String, f32>> = RefCell::new(HashMap::new());
}

/// The page under `label`, if it exists.
pub fn page(label: &str) -> Option<Rc<dyn Page>> {
    PAGES.with(|p| p.borrow().get(label).cloned())
}

pub fn dark() -> bool {
    DARK.with(Cell::get)
}

/// The text size a page was last told (`ui:text-scale`).
pub fn text_scale(label: &str) -> f32 {
    SCALES.with(|s| s.borrow().get(label).copied().unwrap_or(1.0))
}

/// Applies the theme and the text size to any component (the `Theme`
/// global lives per component).
pub fn apply_theme<T: ComponentHandle>(c: &T, dark: bool, scale: f32)
where
    for<'a> Theme<'a>: slint::Global<'a, T>,
{
    let t = c.global::<Theme>();
    t.set_dark(dark);
    t.set_text_scale(scale);
    t.set_reduced_motion(crate::windows::motion::reduced());
}

/// The shell's event sink: one event for one window, on the UI thread.
pub fn deliver(label: &str, channel: &str, payload: &Value) {
    let Some(p) = page(label) else {
        return;
    };
    if channel == "ui:text-scale" {
        let s = payload["scale"].as_f64().unwrap_or(1.0) as f32;
        SCALES.with(|m| m.borrow_mut().insert(label.to_string(), s));
        p.theme(dark(), s);
    }
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        p.on_event(channel, payload)
    }));
    if caught.is_err() {
        crate::crash::write("page-event", &format!("{label} {channel}"));
    }
}

/// Windows switched between light and dark: every page follows.
pub fn on_theme_change(now: bool) {
    let _ = platform::post(Box::new(move || {
        DARK.with(|d| d.set(now));
        let all: Vec<(String, Rc<dyn Page>)> = PAGES.with(|p| {
            p.borrow()
                .iter()
                .map(|(l, p)| (l.clone(), p.clone()))
                .collect()
        });
        for (label, p) in all {
            p.theme(now, text_scale(&label));
        }
    }));
}

/// A window went: its page goes with it (the shell's window events).
pub fn on_window_event(w: &Window, e: &shell::WindowEvent) {
    if !matches!(e, shell::WindowEvent::Destroyed) {
        return;
    }
    let label = w.label().to_string();
    let gone = PAGES.with(|p| {
        let mut p = p.borrow_mut();
        match p.get(&label) {
            Some(page) if page.hwnd() == w.hwnd() => p.remove(&label),
            _ => None,
        }
    });
    if let Some(page) = gone {
        page.close();
        SCALES.with(|s| s.borrow_mut().remove(&label));
    }
}

/// Which page module draws a page path, and whether its window is
/// see-through (per-pixel alpha, clicks pass where nothing is drawn) or an
/// opaque sheet with DWM's rounded corners.
fn kind_of(page: &str) -> (&'static str, bool) {
    let name = page.rsplit('/').next().unwrap_or(page);
    match name {
        "index.html" => ("bar", true),
        "output.html" => ("output", false),
        "settings.html" => ("settings", false),
        "history.html" => ("history", false),
        "agents.html" => ("agents", false),
        "region-note.html" => ("note", false),
        "voice-pill.html" => ("voice-pill", true),
        "region.html" => ("region", true),
        "lens.html" => ("lens", true),
        "buddy.html" => ("buddy", true),
        _ => ("other", false),
    }
}

/// Builds the page's component inside a window made from `spec`.
fn make(kind: &str, label: &str, spec: Spec) -> Result<Rc<dyn Page>, String> {
    let page: Rc<dyn Page> = match kind {
        "bar" => bar::Bar::new(label, spec)?,
        "output" => output::OutputPage::new(label, spec)?,
        "settings" => settings::SettingsPage::new(label, spec)?,
        "history" => history::HistoryPage::new(label, spec)?,
        "agents" => cards::AgentsPage::new(label, spec)?,
        "note" => cards::NotePage::new(label, spec)?,
        "voice-pill" => overlays::VoicePillPage::new(label, spec)?,
        "region" => overlays::RegionPage::new(label, spec)?,
        "lens" => overlays::LensPage::new(label, spec)?,
        "buddy" => overlays::BuddyPage::new(label, spec)?,
        _ => overlays::BlankPage::new(label, spec)?,
    };
    Ok(page)
}

/// Makes a component in a window built from `spec`: the page modules'
/// constructor. Returns the component and its window's HWND.
pub fn create<T: ComponentHandle>(
    spec: Spec,
    new: impl FnOnce() -> Result<T, slint::PlatformError>,
) -> Result<(T, isize), String> {
    let (made, hwnd) = platform::create_with(spec, new);
    let c = made.map_err(|e| e.to_string())?;
    if hwnd == 0 {
        return Err("the window was not made".into());
    }
    c.show().map_err(|e| e.to_string())?;
    Ok((c, hwnd))
}

/// A key hook for a window: the page's own keys first, then the keys every
/// page has (Ctrl+= / Ctrl+- / Ctrl+0 for the text size, glass.js).
pub fn set_keys(hwnd: isize, label: &str, own: impl Fn(&KeyPress) -> bool + 'static) {
    let label = label.to_string();
    platform::set_key_hook(
        hwnd,
        Rc::new(move |k: &KeyPress| own(k) || common_key(&label, k)),
    );
}

fn common_key(label: &str, k: &KeyPress) -> bool {
    if !k.ctrl || k.alt || k.meta {
        return false;
    }
    // VK_OEM_PLUS, VK_ADD; VK_OEM_MINUS, VK_SUBTRACT; '0', VK_NUMPAD0.
    let dir = match k.vk {
        0xBB | 0x6B => 1,
        0xBD | 0x6D => -1,
        0x30 | 0x60 => 0,
        _ => return false,
    };
    let cur = f64::from(text_scale(label)) / crate::windows::text_scale::os_factor().max(0.01);
    let next = if dir == 0 {
        1.0
    } else {
        ((cur + f64::from(dir) * 0.1).clamp(1.0, 2.0) * 10.0).round() / 10.0
    };
    bridge::send(
        label,
        "settings:set-text-scale",
        serde_json::json!({ "scale": next }),
    );
    true
}

/// DIPs to the physical rect a window is made at.
fn px_rect(b: &BuildSpec) -> (i32, i32, i32, i32) {
    let (w, h) = b.size.unwrap_or((400.0, 300.0));
    let (x, y) = b.position.unwrap_or_else(|| {
        let wa = crate::windows::screen::work_area();
        (wa.x + (wa.width - w) / 2.0, wa.y + (wa.height - h) / 2.0)
    });
    let r = crate::windows::screen::dip_to_px(Rect::new(x, y, w, h));
    (
        r.x.round() as i32,
        r.y.round() as i32,
        r.width.round().max(1.0) as i32,
        r.height.round().max(1.0) as i32,
    )
}

/// `WebviewWindowBuilder::build`: the window and its page, on the UI
/// thread. From another thread it waits for the UI thread, as Tauri did.
pub fn build(spec: BuildSpec) -> shell::Result<Window> {
    if platform::is_ui_thread() {
        return build_here(spec);
    }
    let (tx, rx) = mpsc::channel();
    platform::post(Box::new(move || {
        let _ = tx.send(build_here(spec));
    }))?;
    rx.recv_timeout(Duration::from_secs(20))
        .map_err(|_| shell::Error("the UI thread did not build the window".into()))?
}

fn build_here(mut b: BuildSpec) -> shell::Result<Window> {
    if let Some(old) = page(&b.label) {
        if is_window(old.hwnd()) {
            return Err(shell::Error(format!(
                "a window labelled {} exists",
                b.label
            )));
        }
    }
    let (kind, layered) = kind_of(&b.page);
    let spec = Spec {
        label: b.label.clone(),
        layered,
        rect: px_rect(&b),
        maximizable: b.maximizable && b.resizable && !layered,
        no_activate: !b.focusable,
        title: b.title.clone(),
    };
    let page = make(kind, &b.label, spec).map_err(shell::Error)?;
    let hwnd = page.hwnd();
    PAGES.with(|p| p.borrow_mut().insert(b.label.clone(), page.clone()));
    page.theme(dark(), text_scale(&b.label));
    if b.protect {
        crate::windows::flags::protect(hwnd);
    }
    let w = Window::from_parts(&b.label, hwnd);
    if b.visible {
        if b.focused && b.focusable {
            let _ = w.show();
            platform::focus_window(hwnd);
        } else {
            crate::windows::flags::show_inactive(hwnd);
        }
    }
    // The page has "loaded": its hooks hear it after this call returns,
    // and it says hello as glass.js did, which sends its first state.
    let hook = b.on_page_load.take();
    let loaded = w.clone();
    let _ = platform::post(Box::new(move || {
        if let Some(f) = &hook {
            f(
                loaded.clone(),
                webview::PageLoadPayload {
                    event: webview::PageLoadEvent::Started,
                },
            );
            f(
                loaded.clone(),
                webview::PageLoadPayload {
                    event: webview::PageLoadEvent::Finished,
                },
            );
        }
        shell::page_loaded(&loaded);
        bridge::send(loaded.label(), "glass:hello", Value::Null);
    }));
    Ok(w)
}

fn is_window(hwnd: isize) -> bool {
    // SAFETY: IsWindow accepts any value.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(hwnd as _) != 0 }
}

/// For the smoke run: a page's answer to `key`, asked on the UI thread.
pub fn probe(label: &str, key: &str) -> Value {
    page(label).map(|p| p.probe(key)).unwrap_or(Value::Null)
}

/// The labels of every live page.
pub fn labels() -> Vec<String> {
    PAGES.with(|p| p.borrow().keys().cloned().collect())
}

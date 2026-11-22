//! The see-through overlays: the dictation pill, the area picker, the lens
//! and the buddy (pointer and tour card); and the blank page for any page
//! path with no native page. Each does what its script did
//! (renderer/voice-pill.js, region.js, lens.js, buddy.js): the same events
//! in, the same `ipc` channels out. Their windows are layered, so pixels
//! with alpha 0 let clicks through; Rust's own modules (voice/pill.rs,
//! region.rs, lens.rs, buddy.rs) place, show and hide them.
//!
//! Probe keys (`Page::probe`), for the smoke run:
//! - every page: "text" (all the words shown, one string), "visible" (the
//!   window is on screen and something is drawn in it).
//! - voice pill: "state" (the dictation state it shows).
//! - area picker: "rect" ({x, y, width, height} in window px, or null before
//!   any drag or arrow key), "hint" (its one line), "label" (what a screen
//!   reader would call the page), "dragging".
//! - lens: "marks" (how many marks are drawn: 1 with the frame, else 0),
//!   "frame" (bool), "chip" (the chip's words, "" when none).
//! - buddy: "target" ({x, y}, the tip, or null), "card" ({shown, kind,
//!   message, hint, buttons: [label], x, y, w, h}), "pointer" (the arrow
//!   shows), "message", "kind", "ignoring" (the page lets the mouse through).

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, Timer, TimerMode, VecModel,
};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect, IsWindowVisible};

use super::{
    bridge, BlankWindow, BuddyAction, BuddyWindow, LensWindow, RegionWindow, Theme, VoicePillWindow,
};
use crate::shell::platform::{KeyPress, Spec};

crate::simple_page!(BlankPage, BlankWindow);

// ------------------------------------------------------------- helpers

const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;

/// Whether the window is on screen.
pub(super) fn shown(hwnd: isize) -> bool {
    // SAFETY: a query; any value is accepted.
    unsafe { IsWindowVisible(hwnd as _) != 0 }
}

/// The pointer in a window's logical px, and whether it is over the window.
/// For the windows that let the mouse through: they hear no moves of their
/// own, so they look where the pointer is (the scripts had Rust forward the
/// moves to them).
pub(super) fn pointer_in(hwnd: isize, scale: f32) -> Option<(f32, f32, bool)> {
    let mut p = POINT { x: 0, y: 0 };
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    // SAFETY: out-parameters; a stale HWND makes the call fail.
    let ok = unsafe { GetCursorPos(&mut p) != 0 && GetWindowRect(hwnd as _, &mut r) != 0 };
    if !ok {
        return None;
    }
    let inside = (r.left..r.right).contains(&p.x) && (r.top..r.bottom).contains(&p.y);
    let s = scale.max(0.01);
    let x = (p.x - r.left) as f32 / s;
    let y = (p.y - r.top) as f32 / s;
    Some((x, y, inside))
}

/// What a page's live region said (`AevalA11y.say`): a screen reader hears
/// it through the bar.
pub(super) fn announce(label: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let p = json!({ "text": text, "priority": "polite" });
    bridge::send(label, "a11y:announce", p);
}

/// Sends `channel` once the window is registered and its page is in the
/// registry: what the scripts did last, once their listeners were up.
pub(super) fn say_ready(label: &str, channel: &'static str) {
    let label = label.to_string();
    Timer::single_shot(Duration::ZERO, move || {
        bridge::send(&label, channel, Value::Null);
    });
}

/// The window's size in logical px (`window.innerWidth`, `innerHeight`).
pub(super) fn view<T: ComponentHandle>(c: &T) -> (f32, f32) {
    let w = c.window();
    let s = w.size().to_logical(w.scale_factor());
    (s.width, s.height)
}

/// Windows' animations are off: nothing glides or fades.
pub(super) fn reduced_motion<T: ComponentHandle>(c: &T) -> bool
where
    for<'a> Theme<'a>: slint::Global<'a, T>,
{
    c.global::<Theme>().get_reduced_motion()
}

/// A JSON array's items; none for anything else.
pub(super) fn items(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}

fn num(p: &Value, key: &str) -> f32 {
    p[key].as_f64().filter(|v| v.is_finite()).unwrap_or(0.0) as f32
}

// ------------------------------------------------------------ the pill

const WAVE: usize = 13;

/// The dictation pill (renderer/voice-pill.js): the state Rust sends, and
/// the level as a waveform that scrolls right to left.
pub struct VoicePillPage {
    hwnd: isize,
    label: String,
    ui: VoicePillWindow,
    levels: RefCell<[f32; WAVE]>,
    wave: Rc<VecModel<f32>>,
}

impl VoicePillPage {
    pub fn new(label: &str, spec: Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, VoicePillWindow::new)?;
        let wave = Rc::new(VecModel::from(vec![0.0f32; WAVE]));
        ui.set_levels(ModelRc::from(wave.clone()));
        Ok(Rc::new(VoicePillPage {
            hwnd,
            label: label.to_string(),
            ui,
            levels: RefCell::new([0.0; WAVE]),
            wave,
        }))
    }

    fn draw_levels(&self) {
        self.wave.set_vec(self.levels.borrow().to_vec());
    }

    fn set_say(&self, text: &str) {
        self.ui.set_say(text.into());
    }
}

fn pill_words(state: &str) -> &'static str {
    match state {
        "listening" => "Listening",
        "handsfree" => "Hands-free \u{b7} chord to finish",
        "working" | "cleaning" => "Writing",
        "done" => "Done",
        _ => "",
    }
}

impl super::Page for VoicePillPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        if p["origin"].as_str() != Some("dictation") {
            return;
        }
        match channel {
            "voice:level" => {
                let level = num(p, "level").clamp(0.0, 1.0);
                {
                    let mut l = self.levels.borrow_mut();
                    l.rotate_left(1);
                    l[WAVE - 1] = level;
                }
                self.draw_levels();
            }
            "voice:progress" => {
                if self.ui.get_state().as_str() == "preparing" {
                    self.set_say(&format!("Getting ready {}%", num(p, "progress")));
                }
            }
            "voice:state" => {
                let state = p["state"].as_str().unwrap_or("idle");
                self.ui.set_state(state.into());
                if state == "listening" {
                    *self.levels.borrow_mut() = [0.0; WAVE];
                    self.draw_levels();
                }
                let say = match state {
                    "preparing" => format!("Getting ready {}%", num(p, "progress")),
                    "error" => p["message"]
                        .as_str()
                        .filter(|m| !m.is_empty())
                        .unwrap_or("That did not work.")
                        .to_string(),
                    _ => pill_words(state).to_string(),
                };
                let before = self.ui.get_say();
                self.set_say(&say);
                // The pill's words were a polite live region.
                if before.as_str() != say {
                    announce(&self.label, &say);
                }
            }
            _ => {}
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        match key {
            "state" => json!(self.ui.get_state().as_str()),
            "text" => json!(self.ui.get_say().as_str()),
            "visible" => json!(shown(self.hwnd) && self.ui.get_state().as_str() != "idle"),
            _ => Value::Null,
        }
    }
}

// ------------------------------------------------------ the area picker

/// Under this on either axis is a click that missed (region.rs MIN_EDGE):
/// the size turns red, and Rust discards it.
const MIN_EDGE: f32 = 12.0;
/// A screen reader hears the size at most this often.
const SAY_MS: u64 = 500;
const PICK_LABEL: &str = "Choose an area of the screen";
const PICK_TEXT_LABEL: &str = "Choose an area of the screen to copy its text";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Sel {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[derive(Default)]
struct Pick {
    /// Where the mouse went down, while it is down.
    start: Option<(f32, f32)>,
    sel: Option<Sel>,
    /// The box came from the keyboard.
    keyed: bool,
    /// The screen reader was told how to drive the box.
    told: bool,
    said: String,
    said_at: Option<Instant>,
    /// "Copy text from screen" rather than the Ask capture.
    for_text: bool,
}

/// The area picker (renderer/region.js): drag a rectangle, or put a box in
/// the middle with Enter or an arrow and drive it from the keyboard. Reports
/// `region:done` with the box in window px, or `region:cancel`.
pub struct RegionPage {
    me: Weak<RegionPage>,
    hwnd: isize,
    label: String,
    ui: RegionWindow,
    pick: RefCell<Pick>,
    say_timer: Timer,
}

impl RegionPage {
    pub fn new(label: &str, spec: Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, RegionWindow::new)?;
        let page = Rc::new_cyclic(|me| RegionPage {
            me: me.clone(),
            hwnd,
            label: label.to_string(),
            ui,
            pick: RefCell::new(Pick::default()),
            say_timer: Timer::default(),
        });
        let w = page.me.clone();
        page.ui.on_press(move |x, y| {
            if let Some(p) = w.upgrade() {
                p.press(x, y);
            }
        });
        let w = page.me.clone();
        page.ui.on_drag(move |x, y| {
            if let Some(p) = w.upgrade() {
                p.drag(x, y);
            }
        });
        let w = page.me.clone();
        page.ui.on_release(move |x, y| {
            if let Some(p) = w.upgrade() {
                p.release(x, y);
            }
        });
        let w = page.me.clone();
        page.ui.on_cancel(move || {
            if let Some(p) = w.upgrade() {
                p.cancel();
            }
        });
        let w = page.me.clone();
        super::set_keys(hwnd, label, move |k| w.upgrade().is_some_and(|p| p.key(k)));
        Ok(page)
    }

    fn cancel(&self) {
        bridge::send(&self.label, "region:cancel", Value::Null);
    }

    fn done(&self, s: Sel) {
        let rect = json!({ "x": s.x, "y": s.y, "width": s.w, "height": s.h });
        bridge::send(&self.label, "region:done", rect);
    }

    fn press(&self, x: f32, y: f32) {
        {
            let mut p = self.pick.borrow_mut();
            p.start = Some((x, y));
            p.sel = Some(Sel {
                x: x.round(),
                y: y.round(),
                w: 0.0,
                h: 0.0,
            });
        }
        self.ui.set_dragging(true);
        self.draw();
    }

    /// The box from where the mouse went down to `(x, y)`, inside the window.
    fn from_pointer(&self, x: f32, y: f32) {
        let (vw, vh) = view(&self.ui);
        let mut p = self.pick.borrow_mut();
        let Some((sx, sy)) = p.start else { return };
        let x1 = sx.min(x).max(0.0);
        let y1 = sy.min(y).max(0.0);
        let x2 = sx.max(x).min(vw);
        let y2 = sy.max(y).min(vh);
        p.sel = Some(Sel {
            x: x1.round(),
            y: y1.round(),
            w: (x2 - x1).max(0.0).round(),
            h: (y2 - y1).max(0.0).round(),
        });
    }

    fn drag(&self, x: f32, y: f32) {
        if self.pick.borrow().start.is_none() {
            return;
        }
        self.from_pointer(x, y);
        self.draw();
    }

    fn release(&self, x: f32, y: f32) {
        if self.pick.borrow().start.is_none() {
            return;
        }
        self.from_pointer(x, y);
        let sel = {
            let mut p = self.pick.borrow_mut();
            p.start = None;
            p.sel
        };
        if let Some(s) = sel {
            self.done(s);
        }
    }

    fn draw(&self) {
        let Some(s) = self.pick.borrow().sel else {
            return;
        };
        self.ui.set_has_sel(true);
        self.ui.set_sel_x(s.x);
        self.ui.set_sel_y(s.y);
        self.ui.set_sel_w(s.w);
        self.ui.set_sel_h(s.h);
        let ok = s.w >= MIN_EDGE && s.h >= MIN_EDGE;
        let (w, h) = (s.w as i32, s.h as i32);
        let text = if ok {
            format!("{w} \u{d7} {h}")
        } else {
            format!("{w} \u{d7} {h}, too small")
        };
        self.ui.set_size_text(text.into());
        self.ui.set_too_small(!ok);
        self.announce_size();
    }

    fn size_text(&self) -> String {
        let s = self.pick.borrow().sel.unwrap_or_default();
        format!("Area {} by {}.", s.w as i32, s.h as i32)
    }

    fn say(&self, text: String) {
        announce(&self.label, &text);
        let mut p = self.pick.borrow_mut();
        p.said = text;
        p.said_at = Some(Instant::now());
    }

    /// The size as it changes, at most every 500 ms; the last one always
    /// lands.
    fn announce_size(&self) {
        let text = self.size_text();
        let (told, same, at) = {
            let p = self.pick.borrow();
            (p.told, p.said == text, p.said_at)
        };
        if !told || same || self.say_timer.running() {
            return;
        }
        let gone = at.map_or(SAY_MS, |t| t.elapsed().as_millis() as u64);
        if gone >= SAY_MS {
            self.say(text);
            return;
        }
        let w = self.me.clone();
        let later = move || {
            if let Some(p) = w.upgrade() {
                let now = p.size_text();
                let same = p.pick.borrow().said == now;
                if !same {
                    p.say(now);
                }
            }
        };
        let wait = Duration::from_millis(SAY_MS - gone);
        self.say_timer.start(TimerMode::SingleShot, wait, later);
    }

    /// How to drive the box, once, on the first key.
    fn announce_keys(&self) {
        let for_text = {
            let mut p = self.pick.borrow_mut();
            if p.told {
                return;
            }
            p.told = true;
            p.for_text
        };
        let size = self.size_text();
        let enter = if for_text {
            "Enter copies the text"
        } else {
            "Enter asks"
        };
        self.say(format!("{size} Arrows move it, Alt+arrows resize, {enter}, Esc cancels."));
        // What it said includes the size, so the size is not said again
        // until it changes.
        self.pick.borrow_mut().said = size;
    }

    /// The first arrow or Enter puts a box in the middle, half the screen.
    fn key_box(&self) {
        let (vw, vh) = view(&self.ui);
        let mut p = self.pick.borrow_mut();
        if p.keyed {
            return;
        }
        p.keyed = true;
        let w = (vw / 2.0).round();
        let h = (vh / 2.0).round();
        p.sel = Some(Sel {
            x: ((vw - w) / 2.0).round(),
            y: ((vh - h) / 2.0).round(),
            w,
            h,
        });
    }

    fn key(&self, k: &KeyPress) -> bool {
        if k.vk == VK_ESCAPE {
            self.cancel();
            return true;
        }
        if self.pick.borrow().start.is_some() {
            return false;
        }
        if k.vk == VK_RETURN {
            let keyed = self.pick.borrow().keyed;
            if !keyed {
                self.key_box();
                self.announce_keys();
                self.draw();
                return true;
            }
            let sel = self.pick.borrow().sel;
            if let Some(s) = sel {
                self.done(s);
            }
            return true;
        }
        let (dx, dy) = match k.vk {
            VK_LEFT => (-1.0, 0.0),
            VK_RIGHT => (1.0, 0.0),
            VK_UP => (0.0, -1.0),
            VK_DOWN => (0.0, 1.0),
            _ => return false,
        };
        self.key_box();
        self.announce_keys();
        let step = if k.shift { 64.0 } else { 16.0 };
        let (vw, vh) = view(&self.ui);
        {
            let mut p = self.pick.borrow_mut();
            if let Some(s) = p.sel.as_mut() {
                if k.alt {
                    s.w = (s.w + dx * step).min(vw - s.x).max(MIN_EDGE);
                    s.h = (s.h + dy * step).min(vh - s.y).max(MIN_EDGE);
                } else {
                    s.x = (s.x + dx * step).min(vw - s.w).max(0.0);
                    s.y = (s.y + dy * step).min(vh - s.h).max(0.0);
                }
            }
        }
        self.draw();
        true
    }

    fn page_label(&self) -> &'static str {
        if self.pick.borrow().for_text {
            PICK_TEXT_LABEL
        } else {
            PICK_LABEL
        }
    }
}

impl super::Page for RegionPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        // What the pick is for, "text" or "area" (region.rs; the script
        // read `window.aevalRegionFor`).
        if channel == "region:purpose" {
            let for_text = p["purpose"].as_str() == Some("text");
            self.pick.borrow_mut().for_text = for_text;
            let hint = if for_text {
                "Drag over the text to copy it"
            } else {
                "Drag to select an area"
            };
            self.ui.set_hint_text(hint.into());
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        match key {
            "rect" => match self.pick.borrow().sel {
                Some(s) => json!({ "x": s.x, "y": s.y, "width": s.w, "height": s.h }),
                None => Value::Null,
            },
            "hint" => json!(self.ui.get_hint_text().as_str()),
            "label" => json!(self.page_label()),
            "dragging" => json!(self.pick.borrow().start.is_some()),
            "text" => {
                let mut t = self.ui.get_hint_text().to_string();
                if self.pick.borrow().sel.is_some() {
                    t.push('\n');
                    t.push_str(self.ui.get_size_text().as_str());
                }
                json!(t)
            }
            "visible" => json!(shown(self.hwnd)),
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.say_timer.stop();
    }
}

// ------------------------------------------------------------- the lens

/// The lens (renderer/lens.js): the agent's control frame and its chip on
/// the display the agent is using.
pub struct LensPage {
    hwnd: isize,
    ui: LensWindow,
}

impl LensPage {
    pub fn new(label: &str, spec: Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, LensWindow::new)?;
        // B-034: Rust holds the first draw until the page says it can hear.
        say_ready(label, "lens:ready");
        Ok(Rc::new(LensPage { hwnd, ui }))
    }
}

impl super::Page for LensPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        if channel != "lens:draw" {
            return;
        }
        let frame = &p["frame"];
        let on = frame.is_object();
        let chip = if on {
            frame["chip"].as_str().unwrap_or("")
        } else {
            ""
        };
        self.ui.set_frame_on(on);
        self.ui.set_chip(chip.into());
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        let on = self.ui.get_frame_on();
        match key {
            "frame" => json!(on),
            "marks" => json!(u32::from(on)),
            "chip" | "text" => json!(self.ui.get_chip().as_str()),
            "visible" => json!(shown(self.hwnd) && on),
            _ => Value::Null,
        }
    }
}

// ------------------------------------------------------------ the buddy

/// The arrow's width, the gap to its card and the margin the card keeps from
/// the window's edges (cursor-buddy.css).
const ARROW_W: f32 = 26.0;
const GAP: f32 = 8.0;
const EDGE: f32 = 12.0;
/// Each frame the pointer goes this much of the way that is left.
const EASE: f32 = 0.16;
const FRAME_MS: u64 = 16;
/// How often the page looks where the pointer is.
const POLL_MS: u64 = 50;

/// What a card's button does.
enum Act {
    Skip,
    Card(String),
}

#[derive(Clone, Copy)]
struct Drag {
    from: (f32, f32),
    base: (f32, f32),
}

/// The agent's cursor glides on the glide curve over a fixed time, so "the
/// action fires on arrival" is a promise Rust can keep by waiting that long.
#[derive(Clone, Copy)]
struct Glide {
    from: (f32, f32),
    start: Instant,
    ms: f32,
}

struct Pointer {
    /// The tip's place, in window px.
    target: Option<(f32, f32)>,
    cur: (f32, f32),
    shown: bool,
    message: String,
    kind: String,
    hint: String,
    /// Where the person dragged the card, from its own place. A new point
    /// resets it, so the next one is not left off screen.
    drag_by: (f32, f32),
    dragging: Option<Drag>,
    /// Point mode moves on with a click on the card; tour steps move on
    /// with the app action they describe.
    advance_on_click: bool,
    glide: Option<Glide>,
    /// A tour card: no arrow, the card sits where it was put.
    card_mode: bool,
    card_on: bool,
    card_at: (f32, f32),
    acts: Vec<Act>,
    labels: Vec<String>,
    /// The button the keyboard is on, or -1.
    focus: i32,
    /// The page lets the mouse through (what it last told Rust).
    ignoring: bool,
    /// While a drag holds the mouse the page says it is still there.
    last_beat: Option<Instant>,
}

impl Pointer {
    fn fresh() -> Pointer {
        Pointer {
            target: None,
            cur: (0.0, 0.0),
            shown: false,
            message: String::new(),
            kind: "step".into(),
            hint: String::new(),
            drag_by: (0.0, 0.0),
            dragging: None,
            advance_on_click: false,
            glide: None,
            card_mode: false,
            card_on: false,
            card_at: (0.0, 0.0),
            acts: Vec::new(),
            labels: Vec::new(),
            focus: -1,
            ignoring: true,
            last_beat: None,
        }
    }
}

/// cubic-bezier(0.32, 0.72, 0, 1), DESIGN.md's glide.
fn glide_curve(x: f32) -> f32 {
    const AX: f32 = 1.96;
    const BX: f32 = -1.92;
    const CX: f32 = 0.96;
    const AY: f32 = 0.16;
    const BY: f32 = -1.32;
    const CY: f32 = 2.16;
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let mut t = x;
    for _ in 0..6 {
        let dx = ((AX * t + BX) * t + CX) * t - x;
        let d = (3.0 * AX * t + 2.0 * BX) * t + CX;
        if dx.abs() < 1e-4 || d.abs() < 1e-6 {
            break;
        }
        t -= dx / d;
    }
    ((AY * t + BY) * t + CY) * t
}

/// The buddy (renderer/buddy.js): the pointer that glides to its target and
/// the card beside it, or a tour card on its own.
pub struct BuddyPage {
    me: Weak<BuddyPage>,
    hwnd: isize,
    label: String,
    ui: BuddyWindow,
    st: RefCell<Pointer>,
    tick: Timer,
    poll: Timer,
    /// The arrow as drawn for the dark or the light tone.
    arrow: RefCell<Option<(bool, Image)>>,
}

impl BuddyPage {
    pub fn new(label: &str, spec: Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, BuddyWindow::new)?;
        let page = Rc::new_cyclic(|me| BuddyPage {
            me: me.clone(),
            hwnd,
            label: label.to_string(),
            ui,
            st: RefCell::new(Pointer::fresh()),
            tick: Timer::default(),
            poll: Timer::default(),
            arrow: RefCell::new(None),
        });
        let w = page.me.clone();
        page.ui.on_dismiss(move || {
            if let Some(p) = w.upgrade() {
                bridge::send(&p.label, "buddy:dismiss", Value::Null);
            }
        });
        let w = page.me.clone();
        page.ui.on_action(move |i| {
            if let Some(p) = w.upgrade() {
                p.act(i);
            }
        });
        let w = page.me.clone();
        page.ui.on_card_down(move |x, y| {
            if let Some(p) = w.upgrade() {
                p.card_down(x, y);
            }
        });
        let w = page.me.clone();
        page.ui.on_card_move(move |x, y| {
            if let Some(p) = w.upgrade() {
                p.card_move(x, y);
            }
        });
        let w = page.me.clone();
        page.ui.on_card_up(move |x, y| {
            if let Some(p) = w.upgrade() {
                p.card_up(x, y);
            }
        });
        let w = page.me.clone();
        super::set_keys(hwnd, label, move |k| w.upgrade().is_some_and(|p| p.key(k)));
        // B-034: the first show waits for this.
        say_ready(label, "buddy:ready");
        Ok(page)
    }

    fn show(&self, p: &Value) {
        let (x, y) = (num(p, "x"), num(p, "y"));
        let msg = p["message"].as_str().unwrap_or("").to_string();
        let kind = p["kind"]
            .as_str()
            .filter(|k| !k.is_empty())
            .unwrap_or("step")
            .to_string();
        let index = p["stepIndex"].as_i64().unwrap_or(-1);
        let count = p["stepCount"].as_i64().unwrap_or(0);
        let is_point = kind == "point";
        let card_mode = kind == "card";
        let last = is_point && index + 1 >= count;
        // The last point already carries an x; a hint saying "click to
        // close" under it said the same thing twice.
        let hint = if is_point && !last {
            "Click for the next one"
        } else {
            ""
        };
        let (pip, pips) = if is_point {
            (if count > 1 { index } else { -1 }, count)
        } else if (kind == "step" || card_mode) && count > 1 {
            (index, count)
        } else {
            (-1, 0)
        };

        // The tour's buttons: Skip on a pointer step, the card's own choices
        // on a card. The agent's cursor has none; it narrates.
        let own = items(&p["buttons"]);
        let mut acts = Vec::new();
        let mut shown_acts: Vec<(String, bool)> = Vec::new();
        if card_mode || !own.is_empty() {
            for b in own {
                acts.push(Act::Card(b["id"].as_str().unwrap_or("").to_string()));
                let label = b["label"].as_str().unwrap_or("").to_string();
                shown_acts.push((label, b["primary"].as_bool().unwrap_or(false)));
            }
        } else if p["skippable"].as_bool().unwrap_or(false) {
            acts.push(Act::Skip);
            shown_acts.push(("Skip".to_string(), false));
        }
        let focus_to = shown_acts
            .iter()
            .position(|(_, primary)| *primary)
            .unwrap_or(0) as i32;
        let wants_focus = card_mode && !acts.is_empty();
        let glide_ms = num(p, "glideMs");

        let new_message = {
            let mut st = self.st.borrow_mut();
            st.card_mode = card_mode;
            st.advance_on_click = is_point;
            st.glide = if kind == "agent" && glide_ms > 0.0 && st.shown {
                Some(Glide {
                    from: st.cur,
                    start: Instant::now(),
                    ms: glide_ms,
                })
            } else {
                None
            };
            let new_message = msg != st.message || is_point;
            if new_message {
                st.message = msg.clone();
                st.drag_by = (0.0, 0.0);
            }
            st.kind = kind.clone();
            st.hint = hint.to_string();
            st.labels = shown_acts.iter().map(|(l, _)| l.clone()).collect();
            st.acts = acts;
            st.focus = -1;
            st.target = Some((x, y));
            if !st.shown {
                // The first appearance lands in place; every later one glides.
                st.cur = (x, y);
                st.shown = true;
            }
            st.card_on = true;
            new_message
        };

        let model: Vec<BuddyAction> = shown_acts
            .into_iter()
            .map(|(label, primary)| BuddyAction {
                label: label.into(),
                primary,
            })
            .collect();
        self.ui.set_actions(ModelRc::new(VecModel::from(model)));
        self.ui.set_focus_index(-1);
        self.ui.set_kind(kind.into());
        self.ui.set_hint(hint.into());
        self.ui.set_pip_index(pip as i32);
        self.ui.set_pip_count(pips.max(0) as i32);
        if new_message {
            self.ui.set_message(msg.into());
        }
        self.ui.set_away(false);
        self.ui.set_done(false);
        self.ui.set_arrow_shown(!card_mode);
        self.ui.set_card_shown(true);

        if wants_focus {
            // A card is a dialog: once Rust has made the window focusable,
            // the keyboard is on its main choice.
            let w = self.me.clone();
            bridge::call(&self.label, "buddy:focus", Value::Null, move |_| {
                if let Some(p) = w.upgrade() {
                    p.focus_action(focus_to);
                }
            });
        }
        self.position_card();
        self.schedule();
        self.watch();
    }

    fn focus_action(&self, i: i32) {
        self.st.borrow_mut().focus = i;
        self.ui.set_focus_index(i);
    }

    fn act(&self, i: i32) {
        let channel_payload = {
            let st = self.st.borrow();
            match usize::try_from(i).ok().and_then(|i| st.acts.get(i)) {
                Some(Act::Skip) => Some(("buddy:skip", Value::Null)),
                Some(Act::Card(id)) => Some(("buddy:card", json!({ "id": id }))),
                None => None,
            }
        };
        if let Some((channel, payload)) = channel_payload {
            bridge::send(&self.label, channel, payload);
        }
    }

    /// Beside the arrow, flipped to the other side when there is no room;
    /// a card sits where it was put.
    fn position_card(&self) {
        let (w, h) = (self.ui.get_card_w(), self.ui.get_card_h());
        let (vw, vh) = view(&self.ui);
        let (cur, left, top) = {
            let st = self.st.borrow();
            let Some(t) = st.target else { return };
            let (left, top) = if st.card_mode {
                (t.0, t.1)
            } else {
                // Down and to the right of the tip, so the arrow reads as
                // what joins the card to the thing it points at.
                let mut left = st.cur.0 + ARROW_W + GAP - 4.0;
                if left + w > vw - EDGE {
                    left = st.cur.0 - GAP - w;
                }
                (left, st.cur.1 + 14.0)
            };
            let (max_x, max_y) = ((vw - w - EDGE).max(EDGE), (vh - h - EDGE).max(EDGE));
            let left = (left + st.drag_by.0).min(max_x).max(EDGE);
            let top = (top + st.drag_by.1).min(max_y).max(EDGE);
            (st.cur, left.round(), top.round())
        };
        self.st.borrow_mut().card_at = (left, top);
        self.ui.set_tip_x(cur.0);
        self.ui.set_tip_y(cur.1);
        self.ui.set_card_x(left);
        self.ui.set_card_y(top);
    }

    /// One frame of the glide; whether the pointer still moves.
    fn step(&self) -> bool {
        let reduced = reduced_motion(&self.ui);
        let moving = {
            let mut st = self.st.borrow_mut();
            let Some(t) = st.target else { return false };
            if let Some(g) = st.glide {
                let p = if reduced {
                    1.0
                } else {
                    let ms = g.start.elapsed().as_secs_f32() * 1000.0;
                    glide_curve((ms / g.ms.max(1.0)).min(1.0))
                };
                st.cur = (
                    g.from.0 + (t.0 - g.from.0) * p,
                    g.from.1 + (t.1 - g.from.1) * p,
                );
                if p >= 1.0 {
                    st.glide = None;
                }
            } else {
                let ease = if reduced { 1.0 } else { EASE };
                st.cur.0 += (t.0 - st.cur.0) * ease;
                st.cur.1 += (t.1 - st.cur.1) * ease;
            }
            if (t.0 - st.cur.0).abs() < 0.5 && (t.1 - st.cur.1).abs() < 0.5 {
                st.cur = t;
            }
            st.cur != t
        };
        self.position_card();
        moving
    }

    fn schedule(&self) {
        if self.tick.running() || !self.step() {
            return;
        }
        let w = self.me.clone();
        let frame = move || {
            if let Some(p) = w.upgrade() {
                if !p.step() {
                    p.tick.stop();
                }
            }
        };
        let every = Duration::from_millis(FRAME_MS);
        self.tick.start(TimerMode::Repeated, every, frame);
    }

    /// Looks where the pointer is while the card shows: over it, the page
    /// takes the mouse so its buttons work; anywhere else clicks go through.
    fn watch(&self) {
        if self.poll.running() {
            return;
        }
        let w = self.me.clone();
        let look = move || {
            if let Some(p) = w.upgrade() {
                p.sense();
            }
        };
        let every = Duration::from_millis(POLL_MS);
        self.poll.start(TimerMode::Repeated, every, look);
    }

    fn sense(&self) {
        let at = pointer_in(self.hwnd, self.ui.window().scale_factor());
        match at {
            Some((x, y, true)) => self.pointer_at(Some((x, y))),
            _ => self.pointer_at(None),
        }
    }

    /// The pointer is at `at` in this window, or off it: over the card the
    /// page takes the mouse. A drag keeps it for its whole length, or the
    /// card would drop the moment the pointer slipped off it.
    fn pointer_at(&self, at: Option<(f32, f32)>) {
        let (card_on, (cx, cy)) = {
            let st = self.st.borrow();
            if st.dragging.is_some() {
                return;
            }
            (st.card_on, st.card_at)
        };
        let (w, h) = (self.ui.get_card_w(), self.ui.get_card_h());
        let over = at.is_some_and(|(x, y)| {
            card_on && (cx..cx + w).contains(&x) && (cy..cy + h).contains(&y)
        });
        self.set_ignore(!over);
    }

    fn callout(&self) -> Value {
        let (x, y) = self.st.borrow().card_at;
        json!({ "x": x, "y": y, "w": self.ui.get_card_w(), "h": self.ui.get_card_h() })
    }

    fn set_ignore(&self, ignore: bool) {
        {
            let mut st = self.st.borrow_mut();
            if st.ignoring == ignore {
                return;
            }
            st.ignoring = ignore;
        }
        let payload = if ignore {
            json!({ "ignore": true })
        } else {
            json!({ "ignore": false, "callout": self.callout() })
        };
        bridge::send(&self.label, "buddy:set-ignore", payload);
    }

    fn card_down(&self, x: f32, y: f32) {
        {
            let mut st = self.st.borrow_mut();
            let base = st.drag_by;
            st.dragging = Some(Drag { from: (x, y), base });
        }
        self.ui.set_dragging(true);
    }

    fn card_move(&self, x: f32, y: f32) {
        let beat = {
            let mut st = self.st.borrow_mut();
            let Some(d) = st.dragging else { return };
            st.drag_by = (d.base.0 + (x - d.from.0), d.base.1 + (y - d.from.1));
            let due = !matches!(st.last_beat, Some(t) if t.elapsed() <= Duration::from_secs(1));
            if due {
                st.last_beat = Some(Instant::now());
            }
            due
        };
        self.position_card();
        // Rust takes the mouse back from a page that goes quiet.
        if beat {
            let payload = json!({ "ignore": false, "callout": self.callout() });
            bridge::send(&self.label, "buddy:set-ignore", payload);
        }
    }

    /// A press that never moved is navigation, not a drag: it moves on.
    fn card_up(&self, x: f32, y: f32) {
        let (moved, advance) = {
            let mut st = self.st.borrow_mut();
            let Some(d) = st.dragging.take() else { return };
            let moved = (x - d.from.0).abs() > 2.0 || (y - d.from.1).abs() > 2.0;
            (moved, st.advance_on_click)
        };
        self.ui.set_dragging(false);
        if !moved && advance {
            bridge::send(&self.label, "buddy:advance", Value::Null);
        }
    }

    /// A focused card's keys: Tab and the arrows move between its buttons,
    /// Enter and Space press the one the keyboard is on.
    fn key(&self, k: &KeyPress) -> bool {
        let (n, at) = {
            let st = self.st.borrow();
            (st.acts.len() as i32, st.focus)
        };
        if n == 0 {
            return false;
        }
        match k.vk {
            VK_TAB | VK_LEFT | VK_RIGHT => {
                let back = k.vk == VK_LEFT || (k.vk == VK_TAB && k.shift);
                let dir = if back { -1 } else { 1 };
                let next = if at < 0 { 0 } else { (at + dir).rem_euclid(n) };
                self.focus_action(next);
                true
            }
            VK_RETURN | VK_SPACE if at >= 0 => {
                self.act(at);
                true
            }
            _ => false,
        }
    }

    fn hide(&self) {
        // Fade only; the place is kept so a new show glides from it.
        {
            let mut st = self.st.borrow_mut();
            st.card_on = false;
            st.dragging = None;
            // Rust hands the mouse back at the same moment.
            st.ignoring = true;
        }
        self.poll.stop();
        self.ui.set_dragging(false);
        self.ui.set_arrow_shown(false);
        self.ui.set_card_shown(false);
    }

    /// The window is kept between sessions, so each one starts from a clean
    /// page. Nothing fades: Rust hides the window once this empty page is on
    /// screen (`buddy:cleared`).
    fn reset(&self) {
        self.tick.stop();
        self.poll.stop();
        *self.st.borrow_mut() = Pointer::fresh();
        self.ui.set_away(true);
        self.ui.set_arrow_shown(false);
        self.ui.set_card_shown(false);
        self.ui.set_done(false);
        self.ui.set_dragging(false);
        self.ui.set_message("".into());
        self.ui.set_hint("".into());
        self.ui.set_actions(ModelRc::default());
        self.ui.set_pip_index(-1);
        self.ui.set_pip_count(0);
        self.ui.set_focus_index(-1);
        self.ui.window().request_redraw();
        let label = self.label.clone();
        // A frame or two for the empty page to reach the screen.
        Timer::single_shot(Duration::from_millis(50), move || {
            bridge::send(&label, "buddy:cleared", Value::Null);
        });
    }
}

impl super::Page for BuddyPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        match channel {
            "buddy:show" => self.show(p),
            "buddy:hide" => self.hide(),
            "buddy:done" => {
                self.st.borrow_mut().card_on = false;
                self.ui.set_done(true);
            }
            "buddy:reset" => self.reset(),
            // The pointer Rust forwards while the window lets it through
            // (window px), or that it left the window.
            "buddy:pointer" => {
                let at = p["x"].as_f64().zip(p["y"].as_f64());
                self.pointer_at(at.map(|(x, y)| (x as f32, y as f32)));
            }
            _ => {}
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
        let cached = matches!(&*self.arrow.borrow(), Some((d, _)) if *d == dark);
        if !cached {
            let img = arrow_image(dark);
            self.ui.set_arrow(img.clone());
            *self.arrow.borrow_mut() = Some((dark, img));
        }
    }

    fn probe(&self, key: &str) -> Value {
        let st = self.st.borrow();
        let pointer = self.ui.get_arrow_shown() && !self.ui.get_done();
        match key {
            "target" => match st.target {
                Some((x, y)) => json!({ "x": x, "y": y }),
                None => Value::Null,
            },
            "card" => json!({
                "shown": st.card_on,
                "kind": st.kind,
                "message": st.message,
                "hint": st.hint,
                "buttons": st.labels,
                "x": st.card_at.0,
                "y": st.card_at.1,
                "w": self.ui.get_card_w(),
                "h": self.ui.get_card_h(),
            }),
            "pointer" => json!(pointer),
            "message" => json!(st.message),
            "kind" => json!(st.kind),
            "ignoring" => json!(st.ignoring),
            "text" => {
                let mut words = vec![st.message.clone()];
                if !st.hint.is_empty() {
                    words.push(st.hint.clone());
                }
                words.extend(st.labels.iter().cloned());
                json!(words.join("\n"))
            }
            "visible" => json!(shown(self.hwnd) && (pointer || st.card_on)),
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.tick.stop();
        self.poll.stop();
    }
}

// ------------------------------------------------------- the arrow image

/// One piece of the arrow's outline after the start point.
#[derive(Clone, Copy)]
enum Seg {
    Line(f32, f32),
    /// Control point, end point.
    Quad(f32, f32, f32, f32),
}

/// buddy.html's path, in its own 26 x 30 viewBox: a soft, rounded pointer
/// with its apex at (3.38, 0.29).
const ARROW_START: (f32, f32) = (2.6, 1.6);
const ARROW_PATH: [Seg; 12] = [
    Seg::Line(2.6, 20.4),
    Seg::Quad(2.6, 22.6, 4.3, 21.2),
    Seg::Line(8.6, 17.5),
    Seg::Line(11.5, 24.6),
    Seg::Quad(12.2, 26.3, 13.9, 25.6),
    Seg::Line(15.6, 24.9),
    Seg::Quad(17.3, 24.2, 16.6, 22.5),
    Seg::Line(13.9, 15.9),
    Seg::Line(19.4, 15.6),
    Seg::Quad(21.6, 15.5, 20.1, 13.9),
    Seg::Line(4.8, 0.9),
    Seg::Quad(2.6, -0.6, 2.6, 1.6),
];
/// Image pixels per logical px: enough for a crisp arrow at 250 %.
const ARROW_RES: u32 = 4;
/// The image is 32 x 36 px with the path drawn 2 px in, so the stroke and
/// the shadow fit (overlay-buddy.slint places it by the apex).
const ARROW_SIZE: (u32, u32) = (32, 36);
const ARROW_PAD: f32 = 2.0;
/// Half the stroke (1.6 wide, round joins).
const ARROW_HALF_STROKE: f32 = 0.8;

fn arrow_outline() -> Vec<(f32, f32)> {
    let mut pts = vec![ARROW_START];
    let mut at = ARROW_START;
    for seg in ARROW_PATH {
        match seg {
            Seg::Line(x, y) => {
                at = (x, y);
                pts.push(at);
            }
            Seg::Quad(cx, cy, x, y) => {
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    let u = 1.0 - t;
                    pts.push((
                        u * u * at.0 + 2.0 * u * t * cx + t * t * x,
                        u * u * at.1 + 2.0 * u * t * cy + t * t * y,
                    ));
                }
                at = (x, y);
            }
        }
    }
    pts
}

/// Distance from `(x, y)` to the closed outline, negative inside it.
fn signed_distance(pts: &[(f32, f32)], x: f32, y: f32) -> f32 {
    let mut best = f32::MAX;
    let mut inside = false;
    for (i, &(ax, ay)) in pts.iter().enumerate() {
        let (bx, by) = pts[(i + 1) % pts.len()];
        let (ex, ey) = (bx - ax, by - ay);
        let len2 = ex * ex + ey * ey;
        let t = if len2 > 0.0 {
            (((x - ax) * ex + (y - ay) * ey) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (dx, dy) = (x - (ax + ex * t), y - (ay + ey * t));
        best = best.min(dx * dx + dy * dy);
        if (ay > y) != (by > y) && x < ax + (y - ay) / (by - ay) * ex {
            inside = !inside;
        }
    }
    let d = best.sqrt();
    if inside { -d } else { d }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The arrow (cursor-buddy.css): the accent fill, a 1.6 stroke in the
/// label colour, and the soft offset shadow Windows draws under its own
/// cursor (depth, not glow). Drawn here once per tone, since the software
/// renderer draws images on every build and paths only with a feature.
fn arrow_image(dark: bool) -> Image {
    let pts = arrow_outline();
    let (w, h) = (ARROW_SIZE.0 * ARROW_RES, ARROW_SIZE.1 * ARROW_RES);
    let px = 1.0 / ARROW_RES as f32;
    let stroke: (f32, f32, f32) = if dark {
        (247.0, 243.0, 234.0)
    } else {
        (22.0, 32.0, 45.0)
    };
    let fill: (f32, f32, f32) = (94.0, 134.0, 180.0);
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let pixels = buf.make_mut_slice();
    for j in 0..h {
        for i in 0..w {
            let x = (i as f32 + 0.5) * px - ARROW_PAD;
            let y = (j as f32 + 0.5) * px - ARROW_PAD;
            let d = signed_distance(&pts, x, y);
            let shape = ((ARROW_HALF_STROKE - d) / px + 0.5).clamp(0.0, 1.0);
            let inner = ((-d - ARROW_HALF_STROKE) / px + 0.5).clamp(0.0, 1.0);
            // drop-shadow(0 2px 4px rgb(0 0 0 / 0.5)).
            let sd = signed_distance(&pts, x, y - 2.0) - ARROW_HALF_STROKE;
            let shadow = 0.5 * (1.0 - smoothstep(-3.0, 3.0, sd));
            let a = shape + shadow * (1.0 - shape);
            let mix = |s: f32, f: f32| (s + (f - s) * inner) * shape;
            let to_u8 = |v: f32| v.round().clamp(0.0, 255.0) as u8;
            pixels[(j * w + i) as usize] = Rgba8Pixel {
                r: to_u8(mix(stroke.0, fill.0)),
                g: to_u8(mix(stroke.1, fill.1)),
                b: to_u8(mix(stroke.2, fill.2)),
                a: to_u8(a * 255.0),
            };
        }
    }
    Image::from_rgba8_premultiplied(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glide_curve_starts_and_ends_in_place() {
        assert!(glide_curve(0.0).abs() < 1e-6);
        assert!((glide_curve(1.0) - 1.0).abs() < 1e-6);
        let mid = glide_curve(0.5);
        assert!(mid > 0.5 && mid < 1.0, "{mid}");
    }

    #[test]
    fn the_arrow_outline_is_inside_out_where_it_should_be() {
        let pts = arrow_outline();
        // Inside the shaft, and off to the right of the tip.
        assert!(signed_distance(&pts, 5.0, 10.0) < 0.0);
        assert!(signed_distance(&pts, 20.0, 3.0) > 0.0);
    }
}

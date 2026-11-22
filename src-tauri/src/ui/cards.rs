//! The cards: the agents notification at the top of the screen and the
//! copy-text note. Each does what its script did (renderer/agents.js,
//! region-note.js): the same events in, the same `ipc` channels out. Their
//! windows are opaque: the window is the card.
//!
//! Probe keys (`Page::probe`), for the smoke run:
//! - both: "text" (all the words shown, one string), "visible" (the window
//!   is on screen with a card in it).
//! - agents: "sessions" ([{name, state, time}], the rows under the head,
//!   subagents included; `time` is the row's own words, "working 12m"),
//!   "kind", "title", "open" (the whole message shows), "hover".
//! - note: "kind" ("copied", "none", "error", "reading" or "idle" once it
//!   has faded), "title", "lead".

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};

use super::overlays::{announce, items, pointer_in, reduced_motion, say_ready, shown};
use super::{bridge, AgentRow, AgentsWindow, NoteWindow};
use crate::shell::platform::Spec;

// ---------------------------------------------------------- the agents

/// A pointer crossing the card on its way somewhere else does not open it.
const OPEN_AFTER_MS: u64 = 150;
/// The words lift away over this, then the window may go.
const LIFT_MS: u64 = 220;
/// How often the card looks where the pointer is.
const POLL_MS: u64 = 50;

#[derive(Default)]
struct Card {
    /// A card is up (not lifting away).
    up: bool,
    /// What the card on screen is, so a newer one of the same session with
    /// new times takes its place without closing.
    key: String,
    /// The session each row goes to; None for the "more" line.
    sessions: Vec<Option<String>>,
    /// The rows as drawn, for the probe: name, state, time.
    rows: Vec<(String, String, String)>,
    expand: bool,
    /// The window lets the mouse through (what Rust was last told).
    through: bool,
    hovering: bool,
    /// Counts hides, so a late one never takes a newer card down.
    hide_seq: u64,
}

/// The agents notification (renderer/agents.js). Rust sends `agents:card`
/// with everything to draw; the page draws it, tells Rust how tall the
/// window must be (`agents:size`, which places and shows it at the top of
/// the screen), and says when the pointer is on it (`agents:pass`,
/// `agents:hover`). `agents:hide` lifts the words away, and the page says
/// `agents:hidden` once they are gone.
pub struct AgentsPage {
    me: Weak<AgentsPage>,
    hwnd: isize,
    label: String,
    ui: AgentsWindow,
    st: RefCell<Card>,
    poll: Timer,
    open_timer: Timer,
    hide_timer: Timer,
}

fn text_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("").to_string()
}

impl AgentsPage {
    pub fn new(label: &str, spec: Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, AgentsWindow::new)?;
        let page = Rc::new_cyclic(|me| AgentsPage {
            me: me.clone(),
            hwnd,
            label: label.to_string(),
            ui,
            st: RefCell::new(Card {
                through: true,
                ..Card::default()
            }),
            poll: Timer::default(),
            open_timer: Timer::default(),
            hide_timer: Timer::default(),
        });
        let w = page.me.clone();
        page.ui.on_open_head(move || {
            if let Some(p) = w.upgrade() {
                p.open_session(None);
            }
        });
        let w = page.me.clone();
        page.ui.on_open_row(move |i| {
            if let Some(p) = w.upgrade() {
                let session = usize::try_from(i)
                    .ok()
                    .and_then(|i| p.st.borrow().sessions.get(i).cloned().flatten());
                if let Some(s) = session {
                    p.open_session(Some(s));
                }
            }
        });
        let w = page.me.clone();
        page.ui.on_dismiss(move || {
            if let Some(p) = w.upgrade() {
                if p.st.borrow().up {
                    bridge::send(&p.label, "agents:dismiss", Value::Null);
                }
            }
        });
        // Rust made the window let the mouse through; the page says so too,
        // then that it can hear the card.
        bridge::send(label, "agents:pass", json!({ "ignore": true }));
        say_ready(label, "agents:ready");
        Ok(page)
    }

    /// A click: the head goes to its session's window, a row to its own.
    fn open_session(&self, session: Option<String>) {
        if !self.st.borrow().up {
            return;
        }
        let p = match session {
            Some(s) => json!({ "session": s }),
            None => json!({}),
        };
        bridge::send(&self.label, "agents:open", p);
    }

    fn draw(&self, p: &Value) {
        let list = items(&p["rows"]);
        let deep = |i: usize| list.get(i).and_then(|r| r["depth"].as_i64()).unwrap_or(0) > 0;
        let mut rows = Vec::new();
        let mut sessions = Vec::new();
        let mut seen = Vec::new();
        for (i, r) in list.iter().enumerate() {
            let child = deep(i);
            let next = deep(i + 1);
            let state = text_of(r, "state");
            rows.push(AgentRow {
                name: text_of(r, "name").into(),
                place: text_of(r, "place").into(),
                text: text_of(r, "text").into(),
                state: state.clone().into(),
                child,
                last: child && !next,
                parent: !child && next,
                more: false,
            });
            sessions.push(r["session"].as_str().map(str::to_string));
            seen.push((text_of(r, "name"), state, text_of(r, "text")));
        }
        let more = p["more"].as_u64().unwrap_or(0);
        if more > 0 {
            rows.push(AgentRow {
                text: format!("{more} more in the tray menu").into(),
                more: true,
                ..AgentRow::default()
            });
            sessions.push(None);
        }
        self.ui.set_kind(text_of(p, "kind").into());
        self.ui.set_head_title(text_of(p, "title").into());
        self.ui.set_place(text_of(p, "place").into());
        self.ui.set_detail(text_of(p, "detail").into());
        self.ui.set_alt_tab(p["altTab"].as_bool().unwrap_or(false));
        // The head's own children hang from under its dot.
        self.ui.set_stem(deep(0));
        self.ui.set_rows(ModelRc::new(VecModel::from(rows)));
        let mut st = self.st.borrow_mut();
        st.sessions = sessions;
        st.rows = seen;
        st.expand = p["expand"].as_bool().unwrap_or(false);
    }

    /// The window takes the card's height, at the top of the screen.
    fn size(&self) {
        let height = self.ui.get_content_height();
        bridge::send(&self.label, "agents:size", json!({ "height": height }));
    }

    fn open(&self) {
        let up = self.st.borrow().up;
        if !up || !self.ui.get_can_open() || self.ui.get_open() {
            return;
        }
        self.ui.set_open(true);
        self.size();
    }

    fn close(&self) {
        self.open_timer.stop();
        if self.ui.get_open() {
            self.ui.set_open(false);
            self.size();
        }
    }

    fn card(&self, p: &Value) {
        if p.is_null() {
            return;
        }
        let key = [
            text_of(p, "session"),
            text_of(p, "kind"),
            text_of(p, "title"),
            text_of(p, "detail"),
        ]
        .join("\n");
        let (arriving, same) = {
            let mut st = self.st.borrow_mut();
            st.hide_seq += 1;
            (!st.up, st.key == key)
        };
        self.hide_timer.stop();
        if arriving || !same {
            self.open_timer.stop();
            self.ui.set_open(false);
        }
        self.draw(p);
        {
            let mut st = self.st.borrow_mut();
            st.key = key;
            st.up = true;
        }
        self.ui.set_leaving(false);
        self.size();
        if arriving || !same {
            // The card was a polite live region.
            let detail = text_of(p, "detail");
            let mut say = text_of(p, "title");
            if !detail.is_empty() {
                say = format!("{say}. {detail}");
            }
            announce(&self.label, &say);
        }
        self.watch();
    }

    fn hide(&self) {
        self.close();
        self.poll.stop();
        let seq = {
            let mut st = self.st.borrow_mut();
            st.up = false;
            st.key.clear();
            st.hovering = false;
            st.hide_seq += 1;
            st.hide_seq
        };
        self.ui.set_hover(false);
        self.ui.set_leaving(true);
        // The window hides once the words are gone and that frame is on
        // screen, so it never comes back showing the old card.
        let lift = if reduced_motion(&self.ui) { 0 } else { LIFT_MS };
        let w = self.me.clone();
        let gone = move || {
            if let Some(p) = w.upgrade() {
                let still = {
                    let st = p.st.borrow();
                    st.hide_seq == seq && !st.up
                };
                if still {
                    bridge::send(&p.label, "agents:hidden", Value::Null);
                }
            }
        };
        let wait = Duration::from_millis(lift + 60);
        self.hide_timer.start(TimerMode::SingleShot, wait, gone);
    }

    /// Looks where the pointer is while the card is up: on the card, the
    /// window takes the mouse so a click lands, the countdown waits and,
    /// once the pointer settles, the whole message opens.
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

    fn set_through(&self, through: bool) {
        {
            let mut st = self.st.borrow_mut();
            if st.through == through {
                return;
            }
            st.through = through;
        }
        bridge::send(&self.label, "agents:pass", json!({ "ignore": through }));
    }

    fn sense(&self) {
        let at = pointer_in(self.hwnd, self.ui.window().scale_factor());
        let on = shown(self.hwnd) && matches!(at, Some((_, _, true)));
        self.set_through(!on);
        let (was, expand) = {
            let st = self.st.borrow();
            (st.hovering, st.expand)
        };
        if on == was {
            return;
        }
        self.st.borrow_mut().hovering = on;
        self.ui.set_hover(on);
        bridge::send(&self.label, "agents:hover", json!({ "on": on }));
        if !on {
            self.close();
            return;
        }
        if expand && self.ui.get_can_open() {
            let w = self.me.clone();
            let open = move || {
                if let Some(p) = w.upgrade() {
                    p.open();
                }
            };
            let wait = Duration::from_millis(OPEN_AFTER_MS);
            self.open_timer.start(TimerMode::SingleShot, wait, open);
        }
    }
}

impl super::Page for AgentsPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        match channel {
            "agents:card" => self.card(p),
            "agents:hide" => self.hide(),
            _ => {}
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
        // A new text size is a new height.
        if self.st.borrow().up {
            self.size();
        }
    }

    fn probe(&self, key: &str) -> Value {
        let st = self.st.borrow();
        match key {
            "sessions" => {
                let mut list = Vec::new();
                for (name, state, time) in &st.rows {
                    list.push(json!({ "name": name, "state": state, "time": time }));
                }
                Value::Array(list)
            }
            "kind" => json!(self.ui.get_kind().as_str()),
            "title" => json!(self.ui.get_head_title().as_str()),
            "open" => json!(self.ui.get_open()),
            "hover" => json!(st.hovering),
            "text" => {
                let mut words = vec![self.ui.get_head_title().to_string()];
                for part in [self.ui.get_place(), self.ui.get_detail()] {
                    if !part.is_empty() {
                        words.push(part.to_string());
                    }
                }
                for (name, _, time) in &st.rows {
                    words.push(format!("{name} {time}"));
                }
                json!(words.join("\n"))
            }
            "visible" => json!(shown(self.hwnd) && st.up),
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.poll.stop();
        self.open_timer.stop();
        self.hide_timer.stop();
    }
}

// ------------------------------------------------------------ the note

/// The copy-text note (renderer/region-note.js): shows what Rust sends and
/// fades it after its time. Rust hides the window once the fade is over.
pub struct NotePage {
    hwnd: isize,
    ui: NoteWindow,
    timer: Timer,
}

impl NotePage {
    pub fn new(label: &str, spec: Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, NoteWindow::new)?;
        // The listener is in place: Rust may send what it holds.
        say_ready(label, "region:note-ready");
        Ok(Rc::new(NotePage {
            hwnd,
            ui,
            timer: Timer::default(),
        }))
    }
}

impl super::Page for NotePage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        if channel != "region:note" || p.is_null() {
            return;
        }
        self.timer.stop();
        self.ui.set_note_title(text_of(p, "title").into());
        self.ui.set_lead(text_of(p, "lead").into());
        let kind = p["kind"]
            .as_str()
            .filter(|k| !k.is_empty())
            .unwrap_or("idle");
        self.ui.set_kind(kind.into());
        // The words stay while the card fades, so it does not empty first.
        let ms = p["ms"].as_u64().unwrap_or(0);
        if ms > 0 {
            let ui = self.ui.as_weak();
            let fade = move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_kind("idle".into());
                }
            };
            let wait = Duration::from_millis(ms);
            self.timer.start(TimerMode::SingleShot, wait, fade);
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        match key {
            "kind" => json!(self.ui.get_kind().as_str()),
            "title" => json!(self.ui.get_note_title().as_str()),
            "lead" => json!(self.ui.get_lead().as_str()),
            "text" => {
                let (title, lead) = (self.ui.get_note_title(), self.ui.get_lead());
                if lead.is_empty() {
                    json!(title.as_str())
                } else {
                    json!(format!("{title}\n{lead}"))
                }
            }
            "visible" => json!(shown(self.hwnd) && self.ui.get_kind().as_str() != "idle"),
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.timer.stop();
    }
}

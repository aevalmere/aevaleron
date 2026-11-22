//! The bar (renderer/index.html with overlay.js, bar.js and voice.js),
//! native. One window holds the orb, the open bar with what hangs under it,
//! and the layout editor; Rust's `overlay:state` says which.
//!
//! What the scripts did, here:
//! - `overlay:state` is merged into the page's copy and drawn: the orb's
//!   state and step, the region chip, the notice (urgent ones first), the
//!   target chip, and the open bar's height (`overlay:resize`).
//! - The single field: the guess (`intent::score`) as the person types, a
//!   prefix (? or >), Ctrl+1 / Ctrl+2 or the token's menu (Alt+Down) to
//!   choose, Alt+Enter for the other one, Enter answering a waiting task
//!   (Reply, Start, Continue), @ for one question's model, history recall
//!   on Up and Down, Esc clearing then closing, and the bar's chords
//!   (Ctrl+N, Ctrl+W, Ctrl+Shift+W, Ctrl+Shift+C, Ctrl+., Ctrl+Shift+R,
//!   Ctrl+R, Ctrl+,, Ctrl+H, Ctrl+M), F6 to the answer, Ctrl+Tab through
//!   the targets.
//! - Keys typed while the orb shows are held for 600 ms and typed into the
//!   field when the bar opens; `bar:ready` goes once the field has focus.
//! - The mic: click, or hold and let go; Esc stops without keeping it.
//! - The tour: `buddy:watch` and `buddy:resolve` for `#pill`, `#panel`,
//!   `#bar`, `#intent` and `#query-input`.
//!
//! Probe keys (`Page::probe`): `text` (the token's word, the field, the
//! notice and the status, one per line), `input` (the field), `focused-input`
//! (bool), `visible` (bool), `mode`, `intent` ("ask" or "do"), `locked`
//! (bool), `notice`, `status`, `menu` (`{open, inside}`), `examples` (the
//! chips shown), `mic`, `target`, `model`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::intent::{self, Intent, Score};
use super::{bridge, BarWindow, ModelRow};
use crate::shell::platform::{self, KeyPress};

const PLACEHOLDER: &str = "Ask or do";
const EARLY_MS: u128 = 600;
const HOLD_MS: u128 = 350;

const VK_BACK: u16 = 0x08;
const VK_TAB: u16 = 0x09;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_F6: u16 = 0x75;
const VK_OEM_COMMA: u16 = 0xBC;
const VK_OEM_PERIOD: u16 = 0xBE;

/// One question's model, chosen with @.
#[derive(Clone, Debug, PartialEq)]
struct Pin {
    provider: String,
    provider_name: String,
    model: String,
    name: String,
}

#[derive(Clone, Debug)]
struct Provider {
    id: String,
    name: String,
    models: Vec<(String, String)>,
}

struct St {
    state: Value,
    lock: Option<Intent>,
    guess: Score,
    history: Vec<String>,
    recall: i32,
    draft: String,
    announced: String,
    target_label: String,
    cycling: bool,
    tab_to_notice: bool,
    /// Tab moved to the notice's button: Enter or Space presses it.
    notice_focused: bool,
    notice_section: String,
    notice_was: String,
    pin: Option<Pin>,
    models: Option<Vec<Provider>>,
    models_asked: bool,
    model_open: bool,
    model_dismissed: bool,
    /// The pickable rows behind the model list, by its row index (None for
    /// a provider's heading).
    model_rows: Vec<Option<Pin>>,
    model_mark: i32,
    menu_open: bool,
    menu_mark: i32,
    hint: String,
    note: String,
    status_was: String,
    examples: Vec<String>,
    sent_local: bool,
    dirty_sent: bool,
    ready_pending: bool,
    ready_at: Instant,
    early: Vec<(char, Instant)>,
    watch: Option<String>,
    waiting_was: String,
    mic: String,
    mic_pressed: Option<Instant>,
    mic_note: String,
    last_height: f32,
}

impl Default for St {
    fn default() -> Self {
        St {
            state: json!({ "mode": "pill", "hasKey": false, "captureOk": true }),
            lock: None,
            guess: intent::classify(""),
            history: Vec::new(),
            recall: -1,
            draft: String::new(),
            announced: String::new(),
            target_label: String::new(),
            cycling: false,
            tab_to_notice: false,
            notice_focused: false,
            notice_section: String::new(),
            notice_was: String::new(),
            pin: None,
            models: None,
            models_asked: false,
            model_open: false,
            model_dismissed: false,
            model_rows: Vec::new(),
            model_mark: -1,
            menu_open: false,
            menu_mark: 0,
            hint: String::new(),
            note: String::new(),
            status_was: String::new(),
            examples: Vec::new(),
            sent_local: false,
            dirty_sent: false,
            ready_pending: false,
            ready_at: Instant::now(),
            early: Vec::new(),
            watch: None,
            waiting_was: String::new(),
            mic: "idle".into(),
            mic_pressed: None,
            mic_note: String::new(),
            last_height: -1.0,
        }
    }
}

pub struct Bar {
    hwnd: isize,
    label: String,
    ui: BarWindow,
    me: Weak<Bar>,
    st: RefCell<St>,
    closed: Cell<bool>,
    announce_timer: Timer,
    soon_timer: Timer,
    note_timer: Timer,
    dirty_timer: Timer,
    tip_timer: Timer,
    resize_timer: Timer,
    fold_timer: Timer,
    ready_timer: Timer,
    snap_timer: Timer,
    mic_note_timer: Timer,
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Null) | None => String::new(),
        Some(o) => o.to_string(),
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(t) => !t.is_empty(),
        _ => true,
    }
}

fn intent_of(name: &str) -> Option<Intent> {
    match name {
        "ask" => Some(Intent::Ask),
        "do" | "agent" => Some(Intent::Do),
        _ => None,
    }
}

/// models:list's answer as provider rows (bar.js providerModels).
fn provider_models(r: &Value) -> Vec<Provider> {
    let mut out = Vec::new();
    for p in r["providers"].as_array().into_iter().flatten() {
        let mut seen = std::collections::HashSet::new();
        let mut models = Vec::new();
        for g in p["groups"].as_array().into_iter().flatten() {
            for m in g["models"].as_array().into_iter().flatten() {
                let id = s(m, "id");
                if id.is_empty() || !seen.insert(id.clone()) {
                    continue;
                }
                let name = Some(s(m, "name")).filter(|n| !n.is_empty());
                models.push((id.clone(), name.unwrap_or(id)));
            }
        }
        if !models.is_empty() {
            out.push(Provider {
                id: s(p, "id"),
                name: s(p, "name"),
                models,
            });
        }
    }
    out
}

/// The shortened title the target chip shows.
fn short_title(title: &str) -> String {
    if title.chars().count() > 28 {
        let cut: String = title.chars().take(27).collect();
        format!("{}\u{2026}", cut.trim_end())
    } else {
        title.to_string()
    }
}

/// The notice under the bar: (text, button, section, urgent), or None.
fn notice_for(st: &Value, now: Intent) -> Option<(String, String, String, bool)> {
    let failed = s(st, "hotkeyFailed");
    if !failed.is_empty() {
        return Some((
            format!(
                "Another app already uses {failed}, so it can't open Aevaleron. Choose another shortcut."
            ),
            "Change shortcut".into(),
            "pg-shortcuts".into(),
            true,
        ));
    }
    let private = s(st, "privateNotice");
    if !private.is_empty() {
        return Some((private, "Private apps".into(), "sec-private".into(), false));
    }
    if !truthy(&st["hasKey"]) {
        let text = if now == Intent::Do {
            "Tasks need an AI provider."
        } else {
            "Answers need an AI provider."
        };
        return Some((
            text.into(),
            "Connect a provider".into(),
            "pg-providers".into(),
            false,
        ));
    }
    if truthy(&st["offline"]) {
        return Some((
            "No connection. Answers and tasks need the internet.".into(),
            String::new(),
            String::new(),
            false,
        ));
    }
    if st.get("captureOk") == Some(&Value::Bool(false)) && now == Intent::Ask {
        return Some((
            "Screen capture failed. Your question will be sent without a screenshot.".into(),
            String::new(),
            String::new(),
            true,
        ));
    }
    None
}

/// A run waiting on the person: "asking", "plan", "paused" or "".
fn agent_waiting(st: &Value) -> &'static str {
    let a = &st["agent"];
    if !truthy(&a["running"]) {
        return "";
    }
    match a["state"].as_str().unwrap_or("") {
        "asking" => "asking",
        "plan" => "plan",
        "paused" => "paused",
        _ => "",
    }
}

fn wait_name(w: &str) -> &'static str {
    match w {
        "asking" => "Reply",
        "plan" => "Start",
        _ => "Continue",
    }
}

fn waiting_placeholder(w: &str) -> Option<&'static str> {
    match w {
        "asking" => Some("Type your answer for the task"),
        "plan" => Some("Enter starts the task"),
        "paused" => Some("Enter continues the task"),
        _ => None,
    }
}

fn mic_label(state: &str) -> &'static str {
    match state {
        "listening" => "Stop listening and use what I said",
        "working" => "Writing down what you said",
        "preparing" => "Getting voice input ready",
        _ => "Speak instead of typing",
    }
}

/// `text` with a prefix (? or >) taken off the front: the text left.
fn strip_prefix(text: &str) -> String {
    let mut rest = text.chars();
    rest.next();
    let rest = rest.as_str();
    let mut more = rest.chars();
    match more.next() {
        Some(c) if c.is_whitespace() => more.as_str().to_string(),
        _ => rest.to_string(),
    }
}

/// The words heard, put after the field's text with a space between.
fn joined(field: &str, words: &str) -> String {
    if field.is_empty() || field.ends_with(char::is_whitespace) {
        format!("{field}{words}")
    } else {
        format!("{field} {words}")
    }
}

impl Bar {
    pub fn new(label: &str, spec: platform::Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, BarWindow::new)?;
        let page = Rc::new_cyclic(|me| Bar {
            hwnd,
            label: label.to_string(),
            ui,
            me: me.clone(),
            st: RefCell::new(St::default()),
            closed: Cell::new(false),
            announce_timer: Timer::default(),
            soon_timer: Timer::default(),
            note_timer: Timer::default(),
            dirty_timer: Timer::default(),
            tip_timer: Timer::default(),
            resize_timer: Timer::default(),
            fold_timer: Timer::default(),
            ready_timer: Timer::default(),
            snap_timer: Timer::default(),
            mic_note_timer: Timer::default(),
        });
        page.wire();
        let me = page.me.clone();
        super::set_keys(hwnd, label, move |k| {
            me.upgrade().is_some_and(|p| p.on_key(k))
        });
        page.render_token();
        Ok(page)
    }

    /// Runs `f` on this page later, if it is still there.
    fn with(&self, f: impl Fn(&Bar) + 'static) -> impl Fn() + 'static {
        let me = self.me.clone();
        move || {
            if let Some(p) = me.upgrade() {
                if !p.closed.get() {
                    f(&p);
                }
            }
        }
    }

    fn wire(&self) {
        let ui = &self.ui;
        let hwnd = self.hwnd;
        ui.on_pill_clicked(self.with(|p| {
            p.clicked("#pill");
            p.invoke("overlay:trigger", Value::Null);
        }));
        ui.on_token_clicked(self.with(|p| {
            p.clicked("#intent");
            p.hide_tip();
            if p.st.borrow().menu_open {
                p.close_menu(true);
            } else {
                p.open_menu();
            }
        }));
        let me = self.me.clone();
        ui.on_token_hover(move |on| {
            if let Some(p) = me.upgrade() {
                if on {
                    p.show_tip(400);
                } else {
                    p.hide_tip();
                }
            }
        });
        let me = self.me.clone();
        ui.on_intent_picked(move |i| {
            if let Some(p) = me.upgrade() {
                p.set_lock(Some(if i == 1 { Intent::Do } else { Intent::Ask }), true);
                p.close_menu(true);
            }
        });
        let me = self.me.clone();
        ui.on_model_picked(move |i| {
            if let Some(p) = me.upgrade() {
                let row = p.st.borrow().model_rows.get(i as usize).cloned().flatten();
                if let Some(row) = row {
                    p.pick_model(row);
                }
            }
        });
        ui.on_model_chip_clicked(self.with(|p| {
            p.clear_pin(true);
            p.ui.invoke_focus_input();
        }));
        ui.on_target_new(self.with(|p| {
            p.new_conversation();
            p.ui.invoke_focus_input();
        }));
        ui.on_mic_down(self.with(|p| {
            p.clicked("#mic-btn");
            let listening = p.st.borrow().mic == "listening";
            p.st.borrow_mut().mic_pressed = if listening {
                None
            } else {
                Some(Instant::now())
            };
            if listening {
                p.mic_stop();
            } else {
                p.mic_start();
            }
        }));
        ui.on_mic_up(self.with(|p| {
            let held = p.st.borrow_mut().mic_pressed.take();
            if held.is_some_and(|t| t.elapsed().as_millis() >= HOLD_MS) {
                p.mic_stop();
            }
        }));
        ui.on_region_clicked(self.with(|p| {
            p.clicked("#region-btn");
            p.invoke("region:pick", Value::Null);
        }));
        ui.on_region_clear(self.with(|p| {
            p.clicked("#region-clear");
            p.invoke("region:clear", Value::Null);
        }));
        ui.on_settings_clicked(self.with(|p| {
            p.clicked("#settings-btn");
            p.invoke("overlay:open-settings", Value::Null);
        }));
        ui.on_close_clicked(self.with(|p| {
            p.clicked("#close-btn");
            p.invoke("overlay:minimize", Value::Null);
        }));
        let me = self.me.clone();
        ui.on_example_clicked(move |i| {
            if let Some(p) = me.upgrade() {
                p.clicked("#examples");
                let ex = p.st.borrow().examples.get(i as usize).cloned();
                if let Some(ex) = ex {
                    p.set_field(&ex);
                    p.set_lock(None, false);
                    p.ui.invoke_focus_input();
                    p.on_input();
                }
            }
        });
        ui.on_notice_clicked(self.with(|p| {
            p.clicked("#notice-settings");
            p.open_notice();
        }));
        ui.on_hud_done(self.with(|p| p.invoke("hud:exit", Value::Null)));
        ui.on_hud_drag(move || platform::begin_drag(hwnd));
        let me = self.me.clone();
        ui.on_edited(move |_| {
            if let Some(p) = me.upgrade() {
                p.on_input();
            }
        });
        ui.on_field_context(self.with(|p| {
            p.send("ui:context-menu", json!({ "kind": "edit" }));
        }));
        ui.on_clicked_elsewhere(self.with(|p| p.clicked("")));
    }

    // ------------------------------------------------------------ calls

    fn send(&self, channel: &str, payload: Value) {
        bridge::send(&self.label, channel, payload);
    }

    fn invoke(&self, channel: &str, payload: Value) {
        self.send(channel, payload);
    }

    /// Calls `channel`; `then` gets the answer (Null on failure) on the UI
    /// thread while the page lives.
    fn call(
        &self,
        channel: &str,
        payload: Value,
        then: impl FnOnce(&Bar, Result<Value, String>) + 'static,
    ) {
        let me = self.me.clone();
        bridge::call(&self.label, channel, payload, move |r| {
            if let Some(p) = me.upgrade() {
                if !p.closed.get() {
                    then(&p, r);
                }
            }
        });
    }

    fn announce(&self, text: &str, assertive: bool) {
        if text.is_empty() {
            return;
        }
        let priority = if assertive { "assertive" } else { "polite" };
        self.send(
            "a11y:announce",
            json!({ "text": text, "priority": priority }),
        );
    }

    fn announce_soon(&self, text: &str) {
        let t = text.to_string();
        self.soon_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(500),
            self.with(move |p| p.announce(&t, false)),
        );
    }

    // ------------------------------------------------------------ field

    fn text(&self) -> String {
        self.ui.get_text().to_string()
    }

    fn set_field(&self, text: &str) {
        self.ui.set_text(text.into());
        self.ui.invoke_set_caret(text.len() as i32);
    }

    fn mode(&self) -> String {
        s(&self.st.borrow().state, "mode")
    }

    fn bar_intent(&self) -> Intent {
        let st = self.st.borrow();
        st.lock.unwrap_or(st.guess.intent)
    }

    /// While a task waits, what Enter does for it ("" for nothing).
    fn reply_state(&self) -> &'static str {
        let st = self.st.borrow();
        if st.lock.is_some() {
            return "";
        }
        let w = agent_waiting(&st.state);
        if w == "asking" {
            return w;
        }
        if (w == "plan" || w == "paused") && self.text().trim().is_empty() {
            return w;
        }
        ""
    }

    fn sent_before(&self) -> bool {
        let st = self.st.borrow();
        st.sent_local || truthy(&st.state["hasSentFirstRequest"])
    }

    fn mark_sent(&self) {
        if self.sent_before() {
            return;
        }
        let mut st = self.st.borrow_mut();
        st.sent_local = true;
        st.examples.clear();
        drop(st);
        self.render_examples();
    }

    /// The caret sits at the very start of the field.
    fn caret_at_start(&self) -> bool {
        self.ui.get_caret_x() < 2.0 && self.ui.get_caret_y() < self.ui.get_line_height() * 0.5
    }

    /// Up recalls only from the first row and Down only from the last.
    fn caret_at_edge(&self, dir: i32) -> bool {
        let lh = self.ui.get_line_height().max(1.0);
        let h = self.ui.get_field_height();
        if !self.text().contains('\n') && h < lh * 1.5 {
            return true;
        }
        let y = self.ui.get_caret_y();
        if dir < 0 {
            y < lh * 0.5
        } else {
            y + lh * 1.5 > h
        }
    }

    fn on_input(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.hint.clear();
            st.tab_to_notice = false;
            st.notice_focused = false;
        }
        let text = self.text();
        {
            let mut st = self.st.borrow_mut();
            let recall = st.recall;
            if recall >= 0 && st.history.get(recall as usize) != Some(&text) {
                st.recall = -1;
            }
        }
        self.take_prefix();
        self.classify_now();
        self.render_examples();
        self.sync_model_menu();
    }

    fn classify_now(&self) {
        let g = intent::score(&self.text());
        self.st.borrow_mut().guess = g;
        self.render_token();
    }

    /// A prefix typed at the start becomes the lock and leaves the text.
    fn take_prefix(&self) {
        if self.st.borrow().lock.is_some() {
            return;
        }
        let v = self.text();
        let (lock, _) = intent::parse(&v);
        let Some(lock) = lock else {
            return;
        };
        let rest = strip_prefix(&v);
        self.ui.set_text(rest.as_str().into());
        self.ui.invoke_set_caret(0);
        self.set_lock(Some(lock), true);
    }

    fn set_lock(&self, next: Option<Intent>, say: bool) {
        self.st.borrow_mut().lock = next;
        self.render_token();
        if say {
            let t = match next {
                Some(i) => format!("{}, chosen", i.name()),
                None => "Guessing again".to_string(),
            };
            self.announce(&t, false);
        }
    }

    fn clear_field(&self) {
        self.ui.set_text(SharedString::new());
        self.st.borrow_mut().pin = None;
        self.set_lock(None, false);
        self.render_examples();
    }

    // ------------------------------------------------------------ render

    fn render_token(&self) {
        let waiting = self.reply_state();
        let now = if waiting.is_empty() {
            self.bar_intent()
        } else {
            Intent::Do
        };
        let name = if waiting.is_empty() {
            now.name()
        } else {
            wait_name(waiting)
        };
        let (locked, unsure) = {
            let st = self.st.borrow();
            let locked = st.lock.is_some() || !waiting.is_empty();
            let unsure = !locked && st.guess.offer_do && !self.text().trim().is_empty();
            (locked, unsure)
        };
        let ui = &self.ui;
        ui.set_intent(now.key().into());
        ui.set_token_name(name.into());
        ui.set_token_rest(!(locked || now != Intent::Ask));
        ui.set_token_locked(locked);
        ui.set_token_unsure(unsure);
        // Said once the typing settles, and only when it changed.
        self.announce_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(700),
            self.with(|p| {
                let w = p.reply_state();
                let say = if w.is_empty() {
                    let st = p.st.borrow();
                    let guessed = if st.lock.is_some() { "" } else { ", guessed" };
                    format!("{}{guessed}", st.lock.unwrap_or(st.guess.intent).name())
                } else {
                    wait_name(w).to_string()
                };
                let changed = say != p.st.borrow().announced;
                if changed && !p.text().trim().is_empty() {
                    p.st.borrow_mut().announced = say.clone();
                    p.announce(&say, false);
                }
            }),
        );
        self.render_status();
        self.render_notice();
        self.render_target();
        self.render_pin();
        self.send_dirty();
        self.request_resize();
    }

    fn render_status(&self) {
        let text = {
            let st = self.st.borrow();
            if st.hint.is_empty() {
                st.note.clone()
            } else {
                st.hint.clone()
            }
        };
        let changed = text != self.st.borrow().status_was;
        if changed && !text.is_empty() {
            self.announce_soon(&text);
        }
        self.st.borrow_mut().status_was = text.clone();
        self.ui.set_status_text(text.into());
        self.request_resize();
    }

    fn render_notice(&self) {
        let found = notice_for(&self.st.borrow().state, self.bar_intent());
        let (text, button, section, urgent) = found.unwrap_or_default();
        let was = std::mem::replace(&mut self.st.borrow_mut().notice_was, text.clone());
        self.st.borrow_mut().notice_section = section;
        if text.is_empty() {
            self.st.borrow_mut().notice_focused = false;
        }
        self.ui.set_notice_text(text.as_str().into());
        self.ui.set_notice_button(button.into());
        if !text.is_empty() && text != was {
            self.announce(&text, urgent);
        }
    }

    fn render_target(&self) {
        let (title, label, visible) = {
            let st = self.st.borrow();
            let t = &st.state["target"];
            let visible = st.state["visibleOutputs"].as_f64().unwrap_or(0.0);
            (s(t, "title"), s(t, "label"), visible)
        };
        let show = !title.is_empty() && visible >= 2.0 && self.reply_state().is_empty();
        let label = if show {
            if label.is_empty() {
                title.clone()
            } else {
                label
            }
        } else {
            String::new()
        };
        if show {
            self.ui
                .set_target_text(format!("Continues: {}", short_title(&title)).into());
            let cycled = {
                let st = self.st.borrow();
                st.cycling && label != st.target_label
            };
            if cycled {
                self.st.borrow_mut().cycling = false;
                self.announce(&format!("Continues \"{title}\""), false);
            }
        } else {
            self.ui.set_target_text(SharedString::new());
        }
        self.st.borrow_mut().target_label = label;
        self.request_resize();
    }

    fn render_pin(&self) {
        let name = self.st.borrow().pin.as_ref().map(|p| p.name.clone());
        self.ui.set_model_name(name.unwrap_or_default().into());
    }

    fn render_examples(&self) {
        let show = !self.sent_before() && self.text().is_empty();
        let list: Vec<SharedString> = if show {
            self.st
                .borrow()
                .examples
                .iter()
                .map(SharedString::from)
                .collect()
        } else {
            Vec::new()
        };
        self.ui.set_examples(ModelRc::new(VecModel::from(list)));
        self.request_resize();
    }

    fn render_region(&self) {
        let r = self.st.borrow().state["region"].clone();
        let text = if truthy(&r) {
            format!(
                "Asking about a {} \u{00d7} {} area",
                r["width"], r["height"]
            )
        } else {
            String::new()
        };
        self.ui.set_region_active(!text.is_empty());
        self.ui.set_region_text(text.into());
    }

    fn render_placeholder(&self) {
        let w = agent_waiting(&self.st.borrow().state);
        let note = self.st.borrow().mic_note.clone();
        let text = if !note.is_empty() && self.text().is_empty() {
            note
        } else {
            waiting_placeholder(w).unwrap_or(PLACEHOLDER).to_string()
        };
        self.ui.set_placeholder(text.into());
    }

    /// The open bar's height to Rust, once per change (overlay:resize).
    fn request_resize(&self) {
        if self.mode() != "panel" {
            return;
        }
        self.resize_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(0),
            self.with(|p| {
                if p.mode() != "panel" {
                    return;
                }
                let h = p.ui.get_content_height().ceil();
                if (h - p.st.borrow().last_height).abs() < 0.5 || h < 1.0 {
                    return;
                }
                p.st.borrow_mut().last_height = h;
                p.invoke("overlay:resize", json!({ "height": h }));
            }),
        );
    }

    fn send_dirty(&self) {
        self.dirty_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(100),
            self.with(|p| {
                let dirty = !p.text().is_empty();
                if dirty == p.st.borrow().dirty_sent {
                    return;
                }
                p.st.borrow_mut().dirty_sent = dirty;
                p.invoke("bar:dirty", json!({ "dirty": dirty }));
            }),
        );
    }

    fn show_note(&self, text: &str) {
        self.st.borrow_mut().note = text.to_string();
        let ms = if text.chars().count() > 60 {
            7000
        } else {
            4000
        };
        self.note_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(ms),
            self.with(|p| {
                p.st.borrow_mut().note.clear();
                p.render_status();
            }),
        );
        self.render_status();
    }

    fn set_hint(&self, text: &str) {
        self.st.borrow_mut().hint = text.to_string();
        self.render_status();
    }

    // --------------------------------------------------------- the state

    fn on_state(&self, next: &Value) {
        let (prev_mode, prev_button) = {
            let mut st = self.st.borrow_mut();
            let prev_mode = s(&st.state, "mode");
            let prev_button = truthy(&st.state["button"]);
            if let (Some(cur), Some(add)) = (st.state.as_object_mut(), next.as_object()) {
                for (k, v) in add {
                    cur.insert(k.clone(), v.clone());
                }
            }
            (prev_mode, prev_button)
        };
        self.render(&prev_mode, prev_button);
    }

    fn render(&self, prev_mode: &str, prev_button: bool) {
        let st = self.st.borrow().state.clone();
        let mode = s(&st, "mode");
        let button = truthy(&st["button"]);
        let ui = &self.ui;
        let ps = Some(s(&st, "pillState")).filter(|p| !p.is_empty());
        let ps = ps.unwrap_or_else(|| "idle".into());
        let agent = &st["agent"];
        let paused = agent["state"].as_str() == Some("paused");
        ui.set_pill_state(ps.as_str().into());
        ui.set_pill_alert(truthy(&st["pillError"]) || truthy(&st["offline"]));
        ui.set_pill_orbit(ps == "busy" || (ps == "agent" && truthy(agent) && !paused));
        let label = if truthy(&agent["running"]) {
            if paused {
                "||".to_string()
            } else {
                agent["step"].as_u64().unwrap_or(1).max(1).to_string()
            }
        } else {
            String::new()
        };
        ui.set_pill_label(label.into());
        self.render_region();
        self.sync_from_state();
        ui.set_mode(mode.as_str().into());
        ui.set_button(button);
        let closing = mode == "pill" && (prev_mode != "pill" || (prev_button && !button));
        if closing {
            self.say_folded();
        }
        if mode != "panel" {
            // The bar closing ends an utterance it started (voice.js).
            self.mic_cancel(false);
        }
        if mode == "panel" {
            self.render_notice();
            if prev_mode != "panel" {
                {
                    let mut s = self.st.borrow_mut();
                    s.ready_pending = true;
                    s.ready_at = Instant::now();
                    s.last_height = -1.0;
                }
                ui.invoke_focus_input();
                ui.invoke_select_all_input();
                self.type_early_keys();
                self.bar_opened();
                self.wait_ready();
            }
            self.request_resize();
        }
    }

    /// The window paints without the bar, then Rust may touch it.
    fn say_folded(&self) {
        self.fold_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(34),
            self.with(|p| {
                if p.mode() == "pill" {
                    p.invoke("bar:folded", Value::Null);
                }
            }),
        );
    }

    fn focused(&self) -> bool {
        // SAFETY: GetForegroundWindow takes no arguments.
        let fg = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        fg as isize == self.hwnd && self.ui.get_input_focused()
    }

    /// bar:ready once the field has focus after an open (U25): at once
    /// when the window has it, else as soon as it comes.
    fn wait_ready(&self) {
        if self.focused() {
            self.send_ready("state");
            return;
        }
        self.ready_timer.start(
            TimerMode::Repeated,
            Duration::from_millis(4),
            self.with(|p| {
                let pending = p.st.borrow().ready_pending;
                let stale = p.st.borrow().ready_at.elapsed() > Duration::from_secs(5);
                if !pending || stale || p.mode() != "panel" {
                    p.ready_timer.stop();
                } else if p.focused() {
                    p.ready_timer.stop();
                    p.send_ready("focus");
                }
            }),
        );
    }

    fn send_ready(&self, via: &str) {
        let at = {
            let mut st = self.st.borrow_mut();
            if !st.ready_pending {
                return;
            }
            st.ready_pending = false;
            st.ready_at
        };
        let page_ms = at.elapsed().as_millis() as u64;
        self.invoke("bar:ready", json!({ "pageMs": page_ms, "via": via }));
    }

    fn type_early_keys(&self) {
        let keys = std::mem::take(&mut self.st.borrow_mut().early);
        let text: String = keys
            .into_iter()
            .filter(|(_, at)| at.elapsed().as_millis() < EARLY_MS)
            .map(|(c, _)| c)
            .collect();
        if !text.is_empty() {
            self.set_field(&text);
            self.on_input();
        }
    }

    fn bar_opened(&self) {
        {
            let mut st = self.st.borrow_mut();
            st.recall = -1;
            // A key added since the last time shows up in the @ list.
            st.models = None;
            st.menu_open = false;
            st.model_open = false;
            st.notice_focused = false;
        }
        self.ui.set_menu_open(false);
        self.ui.set_model_open(false);
        self.call("bar:history", Value::Null, |p, r| {
            if let Ok(Value::Array(h)) = r {
                p.st.borrow_mut().history = h
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        });
        self.hide_tip();
        self.render_token();
        self.render_examples();
    }

    fn sync_from_state(&self) {
        let w = agent_waiting(&self.st.borrow().state);
        let was = std::mem::replace(&mut self.st.borrow_mut().waiting_was, w.to_string());
        self.render_placeholder();
        if w == "asking" && was != "asking" && self.mode() == "panel" && self.focused() {
            self.announce(
                "The task has a question. Type your answer here and press Enter.",
                true,
            );
        }
        self.render_examples();
        if w != was {
            self.render_token();
        } else {
            self.render_target();
        }
    }

    // ---------------------------------------------------------- the menus

    fn open_menu(&self) {
        if !self.reply_state().is_empty() {
            return;
        }
        let mark = if self.bar_intent() == Intent::Do {
            1
        } else {
            0
        };
        {
            let mut st = self.st.borrow_mut();
            st.menu_open = true;
            st.menu_mark = mark;
        }
        self.ui.set_menu_mark(mark);
        self.ui.set_menu_open(true);
        self.request_resize();
    }

    fn close_menu(&self, refocus: bool) {
        if !self.st.borrow().menu_open {
            return;
        }
        self.st.borrow_mut().menu_open = false;
        self.ui.set_menu_open(false);
        if refocus {
            self.ui.invoke_focus_input();
        }
        self.request_resize();
    }

    fn show_tip(&self, ms: u64) {
        self.tip_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(ms),
            self.with(|p| {
                if !p.st.borrow().menu_open && !p.ui.get_tip_shown() {
                    p.ui.set_tip_shown(true);
                    p.request_resize();
                }
            }),
        );
    }

    fn hide_tip(&self) {
        self.tip_timer.stop();
        if self.ui.get_tip_shown() {
            self.ui.set_tip_shown(false);
            self.request_resize();
        }
    }

    fn ask_for_models(&self) {
        if std::mem::replace(&mut self.st.borrow_mut().models_asked, true) {
            return;
        }
        self.call("models:list", json!({}), |p, r| {
            let models = provider_models(&r.unwrap_or(Value::Null));
            {
                let mut st = p.st.borrow_mut();
                st.models_asked = false;
                st.models = Some(models);
            }
            p.sync_model_menu();
        });
    }

    fn close_model_menu(&self) {
        if !self.st.borrow().model_open {
            return;
        }
        self.st.borrow_mut().model_open = false;
        self.ui.set_model_open(false);
        self.request_resize();
    }

    fn sync_model_menu(&self) {
        let v = self.text();
        let typed = v.starts_with('@');
        if !typed {
            self.st.borrow_mut().model_dismissed = false;
        }
        let blocked = {
            let st = self.st.borrow();
            st.model_dismissed || st.pin.is_some()
        };
        if !typed || blocked || !self.reply_state().is_empty() {
            self.close_model_menu();
            return;
        }
        let models = self.st.borrow().models.clone();
        match models {
            None => self.ask_for_models(),
            Some(m) if m.is_empty() => self.close_model_menu(),
            Some(m) => self.render_model_menu(&m, v[1..].trim()),
        }
    }

    fn render_model_menu(&self, providers: &[Provider], filter: &str) {
        let needle = filter.to_lowercase();
        let mut rows: Vec<Option<Pin>> = Vec::new();
        let mut view: Vec<ModelRow> = Vec::new();
        for p in providers {
            let hits: Vec<&(String, String)> = p
                .models
                .iter()
                .filter(|(id, name)| format!("{name} {id}").to_lowercase().contains(&needle))
                .collect();
            if hits.is_empty() {
                continue;
            }
            rows.push(None);
            view.push(ModelRow {
                header: true,
                text: p.name.as_str().into(),
                marked: false,
            });
            for (id, name) in hits {
                rows.push(Some(Pin {
                    provider: p.id.clone(),
                    provider_name: p.name.clone(),
                    model: id.clone(),
                    name: name.clone(),
                }));
                view.push(ModelRow {
                    header: false,
                    text: name.as_str().into(),
                    marked: false,
                });
            }
        }
        let first = rows
            .iter()
            .position(Option::is_some)
            .map_or(-1, |i| i as i32);
        let empty = if first < 0 {
            let t = format!("No model called \"{filter}\". Esc to keep typing.");
            self.announce_soon(&t);
            t
        } else {
            String::new()
        };
        {
            let mut st = self.st.borrow_mut();
            st.model_rows = rows;
            st.model_open = true;
        }
        self.ui.set_model_empty(empty.into());
        self.ui.set_model_rows(ModelRc::new(VecModel::from(view)));
        self.ui.set_model_open(true);
        self.mark_model(first);
        self.request_resize();
    }

    fn mark_model(&self, at: i32) {
        self.st.borrow_mut().model_mark = at;
        let rows = self.st.borrow().model_rows.clone();
        let view: Vec<ModelRow> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| ModelRow {
                header: r.is_none(),
                text: match r {
                    Some(p) => p.name.as_str().into(),
                    None => SharedString::new(),
                },
                marked: i as i32 == at,
            })
            .collect();
        // Headings keep their provider names: copy them from the old view.
        let old = self.ui.get_model_rows();
        let view: Vec<ModelRow> = view
            .into_iter()
            .enumerate()
            .map(|(i, mut r)| {
                if r.header {
                    if let Some(o) = slint::Model::row_data(&old, i) {
                        r.text = o.text;
                    }
                }
                r
            })
            .collect();
        self.ui.set_model_rows(ModelRc::new(VecModel::from(view)));
    }

    /// The next pickable row from the marked one, `dir` steps round.
    fn step_model(&self, dir: i32) {
        let (rows, at) = {
            let st = self.st.borrow();
            (st.model_rows.clone(), st.model_mark)
        };
        let picks: Vec<i32> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.is_some())
            .map(|(i, _)| i as i32)
            .collect();
        if picks.is_empty() {
            return;
        }
        let cur = picks.iter().position(|&i| i == at).map_or(-1, |i| i as i32);
        let n = picks.len() as i32;
        let next = if cur < 0 {
            if dir > 0 {
                0
            } else {
                n - 1
            }
        } else {
            (cur + dir + n) % n
        };
        self.mark_model(picks[next as usize]);
    }

    fn pick_model(&self, row: Pin) {
        let name = row.name.clone();
        self.st.borrow_mut().pin = Some(row);
        self.ui.set_text(SharedString::new());
        self.close_model_menu();
        self.classify_now();
        self.render_examples();
        self.announce(&format!("{name} for this question"), false);
        self.ui.invoke_focus_input();
    }

    fn clear_pin(&self, say: bool) {
        if self.st.borrow_mut().pin.take().is_none() {
            return;
        }
        self.render_pin();
        self.request_resize();
        if say {
            self.announce("Model choice removed", false);
        }
    }

    /// True when the model list took the key.
    fn model_key(&self, k: &KeyPress) -> bool {
        if !self.st.borrow().model_open || k.ctrl || k.alt || k.meta {
            return false;
        }
        match k.vk {
            VK_DOWN => self.step_model(1),
            VK_UP => self.step_model(-1),
            VK_RETURN => self.pick_marked(),
            VK_TAB if !k.shift => self.pick_marked(),
            VK_ESCAPE => {
                self.st.borrow_mut().model_dismissed = true;
                self.close_model_menu();
            }
            _ => return false,
        }
        true
    }

    fn pick_marked(&self) {
        let row = {
            let st = self.st.borrow();
            let at = st.model_mark.max(0) as usize;
            st.model_rows
                .get(at)
                .cloned()
                .flatten()
                .or_else(|| st.model_rows.iter().flatten().next().cloned())
        };
        if let Some(row) = row {
            self.pick_model(row);
        }
    }

    fn menu_key(&self, k: &KeyPress) -> bool {
        if !self.st.borrow().menu_open {
            return false;
        }
        match k.vk {
            VK_DOWN | VK_UP => {
                let m = 1 - self.st.borrow().menu_mark;
                self.st.borrow_mut().menu_mark = m;
                self.ui.set_menu_mark(m);
            }
            VK_RETURN | VK_SPACE => {
                let m = self.st.borrow().menu_mark;
                self.set_lock(Some(if m == 1 { Intent::Do } else { Intent::Ask }), true);
                self.close_menu(true);
            }
            VK_ESCAPE => self.close_menu(true),
            VK_TAB => {
                self.close_menu(false);
                return false;
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------ sending

    fn push_history(&self, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        self.call("bar:history-push", json!({ "text": t }), |p, r| {
            if let Ok(Value::Array(h)) = r {
                p.st.borrow_mut().history = h
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
            }
        });
        self.st.borrow_mut().recall = -1;
    }

    fn pin_payload(&self, mut v: Value) -> Value {
        if let Some(p) = &self.st.borrow().pin {
            v["provider"] = json!(p.provider);
            v["model"] = json!(p.model);
        }
        v
    }

    fn send_as(&self, what: Intent, offer_do: bool) {
        let text = self.text();
        let body = text.trim().to_string();
        self.st.borrow_mut().hint.clear();
        if !body.is_empty() {
            self.mark_sent();
        }
        let lock = self.st.borrow().lock;
        let recall_as = match lock {
            Some(l) => format!("{}{body}", l.prefix()),
            None => body.clone(),
        };
        if what == Intent::Do {
            if body.is_empty() {
                self.set_hint("Say what to do, for example \"rename these files by date\".");
                return;
            }
            self.push_history(&format!(">{body}"));
            let payload = self.pin_payload(json!({ "task": body }));
            self.call("agent:start", payload, |p, r| {
                let Ok(r) = r else {
                    p.set_hint("Couldn't start the task. Press Enter to try again.");
                    return;
                };
                match r["status"].as_str().unwrap_or("") {
                    // Waiting on consent keeps the words: declining must not
                    // cost them.
                    "started" | "offline" => p.clear_field(),
                    "busy" => {
                        p.st.borrow_mut().hint = "A task is already running. Esc stops it.".into()
                    }
                    "unsupported" => p.st.borrow_mut().hint = "Doing tasks needs Windows.".into(),
                    "needs-key" => p.st.borrow_mut().hint.clear(),
                    _ => {}
                }
                p.render_status();
                p.render_notice();
                if r["status"] == "needs-key" {
                    p.needs_key(Intent::Do);
                }
            });
            return;
        }
        self.push_history(&recall_as);
        let payload = self.pin_payload(json!({ "question": body, "offerDo": offer_do }));
        // Cleared at once, like a messaging app; with no key the text stays.
        if truthy(&self.st.borrow().state["hasKey"]) {
            self.clear_field();
        }
        self.call("query:submit", payload, move |p, r| {
            let unsent = |p: &Bar| {
                if p.text().is_empty() {
                    p.set_field(&text);
                }
                p.set_hint("Couldn't send that. Press Enter to try again.");
            };
            match r {
                Ok(r) => match r["status"].as_str().unwrap_or("") {
                    "walkthrough" => p.clear_field(),
                    "needs-key" => p.needs_key(Intent::Ask),
                    "error" => unsent(p),
                    _ => {}
                },
                Err(_) => unsent(p),
            }
        });
    }

    /// Enter with no provider connected (U23).
    fn needs_key(&self, what: Intent) {
        self.render_notice();
        if self.ui.get_notice_text().is_empty() {
            return;
        }
        self.ui.set_notice_nudge(true);
        let off = self.with(|p| p.ui.set_notice_nudge(false));
        Timer::single_shot(Duration::from_millis(400), off);
        let who = if what == Intent::Do {
            "Tasks"
        } else {
            "Answers"
        };
        self.announce(
            &format!("{who} need an AI provider. Press Tab to connect one."),
            true,
        );
        self.st.borrow_mut().tab_to_notice = true;
    }

    fn open_notice(&self) {
        let section = self.st.borrow().notice_section.clone();
        let payload = if section.is_empty() {
            Value::Null
        } else {
            json!({ "section": section })
        };
        self.invoke("overlay:open-settings", payload);
    }

    fn on_enter(&self, alt: bool) {
        let waiting = self.reply_state();
        let typed = self.text().trim().to_string();
        if waiting == "asking" {
            if typed.is_empty() {
                return;
            }
            self.push_history(&typed);
            self.invoke("agent:continue", json!({ "text": typed }));
            self.clear_field();
            return;
        }
        if waiting == "plan" || waiting == "paused" {
            self.invoke("agent:continue", json!({}));
            return;
        }
        let now = self.bar_intent();
        if alt {
            self.send_as(now.other(), false);
            return;
        }
        if now == Intent::Do {
            self.send_as(Intent::Do, false);
            return;
        }
        let offer = {
            let st = self.st.borrow();
            st.lock.is_none() && st.guess.offer_do
        };
        self.send_as(Intent::Ask, offer);
    }

    /// Up older, Down back toward what was being typed.
    fn recall_key(&self, dir: i32) -> bool {
        let text = self.text();
        let next_text = {
            let mut st = self.st.borrow_mut();
            let n = st.history.len() as i32;
            if n == 0 {
                return false;
            }
            if st.recall < 0 && (dir > 0 || !text.is_empty()) {
                return false;
            }
            if st.recall < 0 {
                st.draft = text;
                st.recall = n;
            }
            let next = st.recall + dir;
            if next >= n {
                st.recall = -1;
                st.draft.clone()
            } else if next >= 0 {
                st.recall = next;
                st.history[next as usize].clone()
            } else {
                return true;
            }
        };
        self.set_field(&next_text);
        self.set_lock(None, false);
        self.take_prefix();
        self.classify_now();
        let len = self.text().len() as i32;
        self.ui.invoke_set_caret(len);
        self.render_examples();
        true
    }

    // ------------------------------------------------------------- chords

    fn target_label(&self) -> String {
        s(&self.st.borrow().state["target"], "label")
    }

    fn new_conversation(&self) {
        self.invoke("chat:new", Value::Null);
        self.announce("The next question starts a new conversation.", false);
    }

    fn cycle_target(&self, dir: i32) {
        let visible = self.st.borrow().state["visibleOutputs"]
            .as_f64()
            .unwrap_or(0.0);
        if visible < 2.0 {
            return;
        }
        self.st.borrow_mut().cycling = true;
        self.call("output:cycle-target", json!({ "dir": dir }), |p, r| {
            if r.is_err() {
                p.st.borrow_mut().cycling = false;
            }
        });
    }

    fn close_target(&self, all: bool) {
        let label = self.target_label();
        if label.is_empty() {
            return;
        }
        if all {
            self.invoke("output:close-all", Value::Null);
        } else {
            self.invoke("output:close", json!({ "label": label }));
        }
    }

    fn copy_last_answer(&self) {
        let label = self.target_label();
        if label.is_empty() {
            return;
        }
        self.call("output:last-answer", json!({ "label": label }), |p, r| {
            let r = r.unwrap_or(Value::Null);
            let text = match &r {
                Value::String(t) => t.clone(),
                _ => s(&r, "text"),
            };
            if !text.is_empty() && crate::shell::sys::clipboard_write(&text) {
                p.announce("Copied the last answer.", false);
            }
        });
    }

    fn stop_running(&self) {
        let running = truthy(&self.st.borrow().state["agent"]["running"]);
        self.invoke(
            if running {
                "agent:stop"
            } else {
                "query:cancel"
            },
            Value::Null,
        );
    }

    /// The bar's chords (bar.js BAR_CHORDS); true when one ran.
    fn chord(&self, k: &KeyPress) -> bool {
        if !k.ctrl || k.meta {
            return false;
        }
        let key = match k.vk {
            VK_OEM_PERIOD => '.',
            VK_OEM_COMMA => ',',
            0x41..=0x5A => char::from(k.vk as u8).to_ascii_lowercase(),
            _ => return false,
        };
        match (key, k.alt, k.shift) {
            ('n', false, false) => self.new_conversation(),
            ('w', false, false) => self.close_target(false),
            ('w', false, true) => self.close_target(true),
            ('c', false, true) => self.copy_last_answer(),
            ('.', false, _) => self.stop_running(),
            ('r', false, true) => {
                let label = self.target_label();
                let p = if label.is_empty() {
                    json!({})
                } else {
                    json!({ "label": label })
                };
                self.invoke("query:retry", p);
            }
            ('r', false, false) => self.invoke("region:pick", Value::Null),
            (',', false, false) => self.invoke("overlay:open-settings", Value::Null),
            ('h', false, false) => self.invoke("overlay:open-history", Value::Null),
            ('m', false, false) => self.invoke("overlay:minimize", Value::Null),
            _ => return false,
        }
        true
    }

    // --------------------------------------------------------------- keys

    fn on_key(&self, k: &KeyPress) -> bool {
        let mode = self.mode();
        if mode == "hud" {
            let d = match k.vk {
                VK_LEFT => Some((-1, 0)),
                VK_RIGHT => Some((1, 0)),
                VK_UP => Some((0, -1)),
                VK_DOWN => Some((0, 1)),
                _ => None,
            };
            if let Some((dx, dy)) = d {
                self.invoke("hud:nudge", json!({ "dx": dx, "dy": dy }));
                return true;
            }
            if k.vk == VK_RETURN {
                self.invoke("hud:exit", Value::Null);
                return true;
            }
            return false;
        }
        if mode == "pill" {
            if k.ctrl || k.alt || k.meta {
                return false;
            }
            if let Some(c) = k.ch {
                let c = if k.shift { c.to_ascii_uppercase() } else { c };
                self.st.borrow_mut().early.push((c, Instant::now()));
                return true;
            }
            return false;
        }
        if mode != "panel" {
            return false;
        }
        // Esc first stops listening, keeping nothing (voice.js).
        if k.vk == VK_ESCAPE {
            let mic = self.st.borrow().mic.clone();
            if mic != "idle" && mic != "error" {
                self.mic_cancel(true);
                return true;
            }
        }
        if self.model_key(k) || self.menu_key(k) {
            return true;
        }
        if self.st.borrow().notice_focused {
            if matches!(k.vk, VK_RETURN | VK_SPACE) && !k.ctrl && !k.alt {
                self.st.borrow_mut().notice_focused = false;
                self.open_notice();
                return true;
            }
            self.st.borrow_mut().notice_focused = false;
            self.ui.set_notice_nudge(false);
        }
        // Ctrl+1 asks, Ctrl+2 does, by the physical key.
        if k.ctrl && !k.alt && !k.shift && !k.meta {
            let lock = match k.vk {
                0x31 | 0x61 => Some(Intent::Ask),
                0x32 | 0x62 => Some(Intent::Do),
                _ => None,
            };
            if let Some(lock) = lock {
                self.set_lock(Some(lock), true);
                self.ui.invoke_focus_input();
                return true;
            }
        }
        let in_field = self.ui.get_input_focused();
        if !in_field && !k.ctrl && !k.alt && !k.meta && k.ch.is_some() {
            // A printable key anywhere in the open bar types into the field.
            self.ui.invoke_focus_input();
            return false;
        }
        if k.alt && k.vk == VK_DOWN {
            self.open_menu();
            return true;
        }
        if k.vk == VK_F6 {
            self.invoke("ui:focus-answer", Value::Null);
            return true;
        }
        if k.vk == VK_BACK && in_field && !k.ctrl && self.caret_at_start() {
            if self.st.borrow().pin.is_some() {
                self.clear_pin(true);
                return true;
            }
            if self.st.borrow().lock.is_some() {
                self.set_lock(None, true);
                return true;
            }
        }
        if k.vk == VK_TAB && k.ctrl && !k.alt {
            self.cycle_target(if k.shift { -1 } else { 1 });
            return true;
        }
        if k.vk == VK_TAB && !k.shift && !k.alt && !k.ctrl {
            let to_notice =
                self.st.borrow().tab_to_notice && !self.ui.get_notice_button().is_empty();
            if to_notice {
                let mut st = self.st.borrow_mut();
                st.tab_to_notice = false;
                st.notice_focused = true;
                drop(st);
                self.ui.set_notice_nudge(true);
                return true;
            }
        }
        if matches!(k.vk, VK_UP | VK_DOWN) && !k.alt && !k.shift && !k.ctrl && in_field {
            let dir = if k.vk == VK_UP { -1 } else { 1 };
            return self.caret_at_edge(dir) && self.recall_key(dir);
        }
        if k.vk == VK_ESCAPE {
            let busy = {
                let st = self.st.borrow();
                st.lock.is_some() || st.pin.is_some()
            };
            if !self.text().is_empty() || busy {
                self.clear_field();
                self.announce("Cleared", false);
            } else {
                self.invoke("overlay:collapse", Value::Null);
            }
            return true;
        }
        if k.vk == VK_RETURN {
            // Shift+Enter starts a new line.
            if k.shift && !k.ctrl && !k.alt {
                return false;
            }
            self.on_enter(k.alt);
            return true;
        }
        self.chord(k)
    }

    // ---------------------------------------------------------------- mic

    fn set_mic(&self, state: &str, label: Option<&str>) {
        self.st.borrow_mut().mic = state.to_string();
        self.ui.set_mic_state(state.into());
        self.ui
            .set_mic_tip(label.unwrap_or(mic_label(state)).into());
        if state != "listening" {
            self.ui.set_mic_level(0.0);
        }
    }

    fn mic_note(&self, text: &str) {
        self.st.borrow_mut().mic_note = text.to_string();
        self.render_placeholder();
        self.mic_note_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(4000),
            self.with(|p| {
                p.st.borrow_mut().mic_note.clear();
                p.render_placeholder();
            }),
        );
    }

    fn mic_error(&self, why: &str) {
        self.set_mic("error", None);
        self.mic_note(why);
        self.announce(why, true);
    }

    fn mic_start(&self) {
        let st = self.st.borrow().mic.clone();
        if matches!(st.as_str(), "listening" | "working" | "preparing") {
            return;
        }
        self.mic_note_timer.stop();
        self.st.borrow_mut().mic_note.clear();
        self.render_placeholder();
        self.set_mic("listening", None);
        self.call("voice:start", Value::Null, |p, r| match r {
            Ok(r) => {
                if r["ok"] == Value::Bool(false) && p.st.borrow().mic == "listening" {
                    p.mic_error(&s(&r, "error"));
                }
            }
            Err(_) => p.mic_error("Voice input couldn't start. Try again."),
        });
    }

    fn mic_stop(&self) {
        if self.st.borrow().mic != "listening" {
            return;
        }
        self.set_mic("working", None);
        self.call("voice:stop", Value::Null, |p, r| {
            if r.is_err() {
                p.set_mic("idle", None);
            }
        });
    }

    fn mic_cancel(&self, say: bool) {
        let st = self.st.borrow().mic.clone();
        if st == "idle" || st == "error" {
            return;
        }
        self.set_mic("idle", None);
        self.invoke("voice:cancel", Value::Null);
        if say {
            self.announce("Stopped listening.", false);
        }
    }

    fn on_voice_state(&self, p: &Value) {
        let cur = self.st.borrow().mic.clone();
        match p["state"].as_str().unwrap_or("") {
            "listening" => {
                self.set_mic("listening", None);
                self.announce("Listening. Press the mic or Esc to stop.", false);
            }
            "working" | "cleaning" => {
                if cur != "idle" {
                    self.set_mic("working", None);
                }
            }
            "preparing" => {
                if cur != "idle" {
                    self.set_mic("preparing", None);
                    self.announce("Downloading the voice model. This happens once.", false);
                }
            }
            "done" => {
                if cur == "idle" {
                    return;
                }
                self.set_mic("idle", None);
                let text = s(p, "text");
                if !text.is_empty() {
                    let field = self.text();
                    self.ui.invoke_focus_input();
                    self.set_field(&joined(&field, &text));
                    self.on_input();
                    self.announce(&text, false);
                }
            }
            "error" => self.mic_error(&s(p, "message")),
            _ => self.set_mic("idle", None),
        }
    }

    // --------------------------------------------------------------- tour

    /// A click on `id`; with the tour watching something else, the tour
    /// goes (it never traps the person).
    fn clicked(&self, id: &str) {
        let watch = self.st.borrow().watch.clone();
        let Some(watch) = watch else {
            return;
        };
        let hit = !id.is_empty() && watch.split(',').any(|w| w.trim() == id);
        if !hit {
            self.invoke("buddy:dismiss", Value::Null);
        }
    }

    /// Where `#id` is in this window (DIPs), if it is showing.
    fn rect_of(&self, id: &str) -> Option<(f32, f32, f32, f32)> {
        let ui = &self.ui;
        let mode = self.mode();
        let panel = mode == "panel";
        match id {
            "#pill" if mode == "pill" && ui.get_button() => {
                let w = ui.get_pill_w();
                Some((ui.get_pill_x(), ui.get_pill_y(), w, w))
            }
            "#panel" | "#bar" if panel => Some((
                ui.get_bar_x(),
                ui.get_bar_y(),
                ui.get_bar_w(),
                ui.get_bar_h(),
            )),
            "#intent" if panel => Some((
                ui.get_token_x(),
                ui.get_token_y(),
                ui.get_token_w(),
                ui.get_token_h(),
            )),
            "#query-input" if panel => Some((
                ui.get_input_x(),
                ui.get_input_y(),
                ui.get_input_w(),
                ui.get_field_height(),
            )),
            _ => None,
        }
    }

    fn resolve(&self, selector: &str) {
        let found = selector.split(',').find_map(|id| self.rect_of(id.trim()));
        let Some((x, y, w, h)) = found else {
            self.invoke("buddy:resolve-result", json!({ "found": false }));
            return;
        };
        let win = crate::shell::Window::from_parts(&self.label, self.hwnd);
        let scale = win.scale_factor().unwrap_or(1.0);
        let (ox, oy) = win
            .outer_position()
            .map(|p| (f64::from(p.x) / scale, f64::from(p.y) / scale))
            .unwrap_or((0.0, 0.0));
        self.invoke(
            "buddy:resolve-result",
            json!({
                "found": true,
                "cx": ox + f64::from(x + w / 2.0),
                "cy": oy + f64::from(y + h / 2.0),
                "w": w,
                "h": h,
            }),
        );
    }
}

impl super::Page for Bar {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        match channel {
            "overlay:state" => self.on_state(p),
            "hud:snapped" => {
                self.ui.set_hud_snapped(true);
                self.snap_timer.start(
                    TimerMode::SingleShot,
                    Duration::from_millis(350),
                    self.with(|p| p.ui.set_hud_snapped(false)),
                );
            }
            "bar:clear" => {
                self.clear_field();
                self.ui.invoke_focus_input();
            }
            "bar:note" => self.show_note(&s(p, "text")),
            "bar:context" => {
                if !self.sent_before() {
                    let ex = intent::examples(&s(p, "process"), &s(p, "title"));
                    self.st.borrow_mut().examples = ex.iter().map(|e| e.to_string()).collect();
                    self.render_examples();
                }
            }
            "bar:open" => self.set_lock(intent_of(&s(p, "lock")), false),
            "bar:prefill" => {
                self.set_field(&s(p, "text"));
                let lock = match s(p, "mode").as_str() {
                    "agent" => Some(Intent::Do),
                    "ask" => None,
                    _ => self.st.borrow().lock,
                };
                self.ui.invoke_focus_input();
                {
                    let mut st = self.st.borrow_mut();
                    st.lock = lock;
                    st.guess = intent::score(&self.text());
                }
                self.render_token();
                self.render_examples();
            }
            "bar:submit" => {
                self.set_field(&s(p, "text"));
                self.st.borrow_mut().recall = -1;
                self.set_lock(None, false);
                self.take_prefix();
                self.classify_now();
                self.render_examples();
                self.on_enter(false);
            }
            "agent:begin" => {
                if self.text().trim() == s(p, "task").trim() {
                    self.clear_field();
                }
            }
            "buddy:watch" => {
                let sel = s(p, "selector");
                self.st.borrow_mut().watch = Some(sel).filter(|x| !x.is_empty());
            }
            "buddy:resolve" => self.resolve(&s(p, "selector")),
            "voice:level" => {
                if s(p, "origin") == "bar" && self.st.borrow().mic == "listening" {
                    let l = p["level"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
                    self.ui.set_mic_level(l as f32);
                }
            }
            "voice:progress" => {
                if s(p, "origin") == "bar" && self.st.borrow().mic == "preparing" {
                    let label = format!("Getting voice input ready, {}%", s(p, "progress"));
                    self.set_mic("preparing", Some(&label));
                }
            }
            "voice:state" if s(p, "origin") == "bar" => self.on_voice_state(p),
            _ => {}
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        super::apply_theme(&self.ui, dark, text_scale);
        self.request_resize();
    }

    fn probe(&self, key: &str) -> Value {
        let ui = &self.ui;
        match key {
            "input" => json!(self.text()),
            "focused-input" => json!(self.focused()),
            "visible" => {
                // SAFETY: IsWindowVisible accepts any value.
                let v = unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(self.hwnd as _)
                };
                json!(v != 0)
            }
            "mode" => json!(self.mode()),
            "intent" => json!(ui.get_intent().to_string()),
            "locked" => json!(ui.get_token_locked()),
            "notice" => json!(ui.get_notice_text().to_string()),
            "status" => json!(ui.get_status_text().to_string()),
            "menu" => json!({ "open": self.st.borrow().menu_open, "inside": true }),
            "examples" => {
                let list: Vec<String> = slint::Model::iter(&ui.get_examples())
                    .map(|e| e.to_string())
                    .collect();
                json!(list)
            }
            "mic" => json!(self.st.borrow().mic),
            "target" => json!(ui.get_target_text().to_string()),
            "model" => json!(ui.get_model_name().to_string()),
            "text" => {
                let mut parts = Vec::new();
                if !ui.get_token_rest() {
                    parts.push(ui.get_token_name().to_string());
                }
                for t in [ui.get_text(), ui.get_notice_text(), ui.get_status_text()] {
                    if !t.is_empty() {
                        parts.push(t.to_string());
                    }
                }
                json!(parts.join("\n"))
            }
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.closed.set(true);
        for t in [
            &self.announce_timer,
            &self.soon_timer,
            &self.note_timer,
            &self.dirty_timer,
            &self.tip_timer,
            &self.resize_timer,
            &self.fold_timer,
            &self.ready_timer,
            &self.snap_timer,
            &self.mic_note_timer,
        ] {
            t.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_leave_the_text() {
        assert_eq!(strip_prefix("?what is this"), "what is this");
        assert_eq!(strip_prefix("> rename files"), "rename files");
        assert_eq!(strip_prefix("?"), "");
    }

    #[test]
    fn heard_words_join_with_one_space() {
        assert_eq!(joined("", "hello"), "hello");
        assert_eq!(joined("open", "mail"), "open mail");
        assert_eq!(joined("open ", "mail"), "open mail");
    }

    #[test]
    fn titles_shorten_with_an_ellipsis() {
        assert_eq!(short_title("short"), "short");
        let long = "a".repeat(40);
        assert_eq!(short_title(&long).chars().count(), 28);
    }

    #[test]
    fn notices_come_in_order() {
        let st = json!({ "hasKey": false, "hotkeyFailed": "Ctrl+Space" });
        let n = notice_for(&st, Intent::Ask).unwrap();
        assert!(n.0.contains("Ctrl+Space") && n.3);
        let st = json!({ "hasKey": false });
        assert_eq!(
            notice_for(&st, Intent::Do).unwrap().0,
            "Tasks need an AI provider."
        );
        let st = json!({ "hasKey": true, "captureOk": false });
        assert!(notice_for(&st, Intent::Ask).is_some());
        assert!(notice_for(&st, Intent::Do).is_none());
        assert!(notice_for(&json!({ "hasKey": true }), Intent::Ask).is_none());
    }

    #[test]
    fn waiting_states_name_enter() {
        let st = json!({ "agent": { "running": true, "state": "asking" } });
        assert_eq!(agent_waiting(&st), "asking");
        assert_eq!(wait_name("plan"), "Start");
        assert_eq!(
            agent_waiting(&json!({ "agent": { "running": false, "state": "plan" } })),
            ""
        );
    }

    #[test]
    fn models_are_listed_once_per_provider() {
        let r = json!({ "providers": [
            { "id": "a", "name": "A", "groups": [
                { "models": [{ "id": "m1", "name": "One" }, { "id": "m1" }] },
                { "models": [{ "id": "m2" }] }
            ]},
            { "id": "b", "name": "B", "groups": [] }
        ]});
        let p = provider_models(&r);
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0].models,
            vec![("m1".into(), "One".into()), ("m2".into(), "m2".into())]
        );
    }
}

//! Settings (settings/settings.html), native. What settings.js and the
//! three pages it carried (voice-settings.js, agents-settings.js,
//! private-apps-settings.js) did, with the same channels and payloads:
//! `providers` (keys, Connect OpenRouter, the clipboard key, order,
//! failover, the models and their pickers), `prefs` (answer style, task
//! permissions, history, shortcuts, text size, startup, the round button,
//! updates, bar position, the tour), `voice` (dictation, the wake word,
//! transcription, language, microphone, cleanup), the coding agents and
//! the private apps list. The glass slider is gone with the glass.
//!
//! The page is SettingsWindow (ui/settings.slint) and its state the
//! SettingsState global; Rust owns every value and the controls call back.
//!
//! Keys (the hook sees them before Slint): while a shortcut is recorded,
//! every key goes to it; while a picker is open, arrows, Home, End, Enter,
//! Esc and type-ahead work its list; F1 goes to Shortcuts; Alt+Up and
//! Alt+Down move the open provider; PageUp and PageDown scroll; Esc closes
//! the picker, the drag, the remove question, the delete-all question, the
//! open row, then the window.
//!
//! Probe keys (`Page::probe`):
//! - `"text"`: what the page says (summaries, provider rows, statuses,
//!   shortcut rows), one item per line (string);
//! - `"section"`: the page in view, `pg-providers` to `pg-general` (string);
//! - `"open"`: the open row (`sec-...` or `prov-<id>`), empty for none;
//! - `"visible"`: whether the window is on screen (bool);
//! - `"providers"`: the provider ids shown, in order (array of strings);
//! - `"shortcuts"`: action -> the accelerator shown (object of strings;
//!   "Not set" for none, "Press keys" while it is being recorded).

mod prefs;
mod providers;
mod voice;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::{bridge, SetProvRow, SettingsState, SettingsWindow};
use crate::shell::platform::{self, KeyPress};
use crate::shell::{async_runtime, WindowEvent};

/// The event the window's focus sends this page (see `new`).
const FOCUSED: &str = "window:focused";

/// A change that did not save.
const UNSAVED: &str = "Couldn't save that change. Try again.";

/// What a call came back with.
pub(super) enum Reply {
    Got(Value),
    Failed,
    TimedOut,
}

impl Reply {
    /// The answer, unless the call was refused, late, or answered nothing.
    fn value(&self) -> Option<&Value> {
        match self {
            Reply::Got(v) if !v.is_null() => Some(v),
            _ => None,
        }
    }

    /// Refused or late (settings.js `failed`).
    fn failed(&self) -> bool {
        !matches!(self, Reply::Got(_))
    }
}

/// An invoke that never fails: the reply, Failed when the channel is
/// refused, or TimedOut after `ms` (0: no limit).
pub(super) async fn ask(label: String, channel: &'static str, payload: Value, ms: u64) -> Reply {
    let fut = bridge::invoke(&label, channel, payload);
    if ms == 0 {
        return match fut.await {
            Ok(v) => Reply::Got(v),
            Err(_) => Reply::Failed,
        };
    }
    let job =
        async_runtime::spawn(
            async move { tokio::time::timeout(Duration::from_millis(ms), fut).await },
        );
    match job.await {
        Ok(Ok(Ok(v))) => Reply::Got(v),
        Ok(Err(_)) => Reply::TimedOut,
        _ => Reply::Failed,
    }
}

pub(super) fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

pub(super) fn flag(v: &Value, key: &str) -> bool {
    v[key].as_bool().unwrap_or(false)
}

pub(super) fn num(v: &Value, key: &str) -> f64 {
    v[key].as_f64().unwrap_or(0.0)
}

/// `{ok: false}`, the way a channel says it did not do it.
pub(super) fn said_no(v: Option<&Value>) -> bool {
    v.is_some_and(|v| v["ok"] == json!(false))
}

pub(super) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

fn local(ms: f64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms as i64).single()
}

/// "just now", "5 min ago", "at 14:05", "on 12 Oct".
pub(super) fn ago(ms: f64) -> String {
    if ms <= 0.0 {
        return String::new();
    }
    let min = ((now_ms() - ms) / 60_000.0).round();
    if min < 1.0 {
        return "just now".into();
    }
    if min < 60.0 {
        return format!("{} min ago", min as i64);
    }
    let Some(d) = local(ms) else {
        return String::new();
    };
    let today = chrono::Local::now().date_naive();
    if d.date_naive() == today {
        return format!("at {}", d.format("%H:%M"));
    }
    format!("on {}", d.format("%-d %b"))
}

pub(super) fn clock(ms: f64) -> String {
    local(ms).map_or_else(String::new, |d| d.format("%H:%M").to_string())
}

/// Settings' own state; the parts each section keeps are in its module.
#[derive(Default)]
struct State {
    /// The last `settings:get`, with what changed since merged in.
    data: Value,
    prov: providers::Providers,
    voice: voice::Voice,
    /// The global shortcut being recorded.
    capturing: Option<String>,
    /// The shortcut the "Free now" chords are offered for.
    free_for: String,
    /// The cursor buddy's walkthrough selector, while a step watches.
    watching: bool,
    /// Type-ahead in an open picker.
    typed: String,
    typed_at: Option<Instant>,
}

pub struct SettingsPage {
    hwnd: isize,
    label: String,
    ui: SettingsWindow,
    me: Weak<SettingsPage>,
    st: RefCell<State>,
    prov_model: Rc<VecModel<SetProvRow>>,
    scale: Cell<f32>,
    tick: Timer,
    chord_timer: Timer,
    wake_timer: Timer,
    text_timer: Timer,
    statuses: RefCell<HashMap<&'static str, Timer>>,
}

impl SettingsPage {
    pub fn new(label: &str, spec: platform::Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, SettingsWindow::new)?;
        let prov_model = Rc::new(VecModel::<SetProvRow>::default());
        ui.global::<SettingsState>()
            .set_providers(ModelRc::from(prov_model.clone()));
        let page = Rc::new_cyclic(|me| SettingsPage {
            hwnd,
            label: label.to_string(),
            ui,
            me: me.clone(),
            st: RefCell::new(State::default()),
            prov_model,
            scale: Cell::new(1.0),
            tick: Timer::default(),
            chord_timer: Timer::default(),
            wake_timer: Timer::default(),
            text_timer: Timer::default(),
            statuses: RefCell::new(HashMap::new()),
        });
        page.wire();
        let me = page.me.clone();
        super::set_keys(hwnd, label, move |k| {
            me.upgrade().is_some_and(|p| p.on_key(k))
        });
        // The window coming back re-reads what may have changed meanwhile
        // (a key copied, the app in front, the agents found).
        let focus_label = label.to_string();
        crate::shell::Window::from_parts(label, hwnd).on_window_event(move |e| {
            if matches!(e, WindowEvent::Focused(true)) {
                super::deliver(&focus_label, FOCUSED, &Value::Null);
            }
        });
        // "Checked 3 min ago" has to keep being true while the window stays
        // open, and so does a provider's "Checked just now".
        let me = page.me.clone();
        page.tick
            .start(TimerMode::Repeated, Duration::from_secs(30), move || {
                if let Some(p) = me.upgrade() {
                    p.render_update();
                    p.render_providers();
                }
            });
        page.load();
        page.voice_load();
        page.watch_load();
        page.privacy_load();
        Ok(page)
    }

    fn s(&self) -> SettingsState<'_> {
        self.ui.global::<SettingsState>()
    }

    fn wire(&self) {
        let hwnd = self.hwnd;
        self.ui.on_drag(move || platform::begin_drag(hwnd));
        self.ui.on_maximize(move || platform::toggle_maximize(hwnd));
        let me = self.me.clone();
        self.ui.on_close_clicked(move || {
            if let Some(p) = me.upgrade() {
                p.send("settings:close", Value::Null);
            }
        });
        let me = self.me.clone();
        self.ui.on_quit_clicked(move || {
            if let Some(p) = me.upgrade() {
                p.send("app:quit", Value::Null);
            }
        });
        let s = self.s();
        let me = self.me.clone();
        s.on_act(move |what, arg| {
            if let Some(p) = me.upgrade() {
                p.touched();
                p.on_act(&what, &arg);
            }
        });
        let me = self.me.clone();
        s.on_toggle(move |what, on| {
            if let Some(p) = me.upgrade() {
                p.touched();
                p.on_toggle(&what, on);
            }
        });
        let me = self.me.clone();
        s.on_slide(move |what, v, done| {
            if let Some(p) = me.upgrade() {
                p.on_slide(&what, v, done);
            }
        });
        let me = self.me.clone();
        s.on_pick(move |cid, id| {
            if let Some(p) = me.upgrade() {
                p.touched();
                p.on_pick(&cid, &id);
            }
        });
        let me = self.me.clone();
        s.on_text_accept(move |what, t| {
            if let Some(p) = me.upgrade() {
                p.touched();
                match what.as_str() {
                    "wake-word" => p.save_wake_word(&t),
                    "private-name" => p.private_add_typed(&t),
                    _ => {}
                }
            }
        });
        let me = self.me.clone();
        s.on_prov_act(move |id, act, field| {
            if let Some(p) = me.upgrade() {
                p.touched();
                p.prov_act(&id, &act, &field);
            }
        });
        let me = self.me.clone();
        s.on_prov_edited(move |id, t| {
            if let Some(p) = me.upgrade() {
                p.prov_edited(&id, &t);
            }
        });
        let me = self.me.clone();
        s.on_prov_move(move |id, to| {
            if let Some(p) = me.upgrade() {
                p.prov_dropped(&id, to);
            }
        });
    }

    // --------------------------------------------------------- helpers

    fn send(&self, channel: &str, payload: Value) {
        bridge::send(&self.label, channel, payload);
    }

    /// Calls `channel` and hands the reply to `then` with the page, if it
    /// is still there.
    fn call(
        &self,
        channel: &'static str,
        payload: Value,
        ms: u64,
        then: impl FnOnce(&SettingsPage, Reply) + 'static,
    ) {
        let fut = ask(self.label.clone(), channel, payload, ms);
        let me = self.me.clone();
        let _ = slint::spawn_local(async move {
            let r = fut.await;
            if let Some(p) = me.upgrade() {
                then(&p, r);
            }
        });
    }

    /// Said aloud (design-ux U7): the screen reader hears it even though
    /// this window's own text is not reachable to it.
    fn announce(&self, text: &str, priority: &str) {
        if text.is_empty() {
            return;
        }
        self.send(
            "a11y:announce",
            json!({ "text": text, "priority": priority }),
        );
    }

    /// The line under the providers speaks every change there.
    fn say(&self, text: &str) {
        self.s().set_prov_live(text.into());
        self.announce(text, "polite");
    }

    /// A status line: shown, and cleared after four seconds unless sticky.
    fn status(&self, which: &'static str, text: &str, sticky: bool) {
        self.set_status_text(which, text);
        let mut timers = self.statuses.borrow_mut();
        let t = timers.entry(which).or_default();
        t.stop();
        if !text.is_empty() && !sticky {
            let me = self.me.clone();
            t.start(TimerMode::SingleShot, Duration::from_secs(4), move || {
                if let Some(p) = me.upgrade() {
                    p.set_status_text(which, "");
                }
            });
        }
    }

    fn set_status_text(&self, which: &str, text: &str) {
        let s = self.s();
        let t: SharedString = text.into();
        match which {
            "skills" => s.set_skills_status(t),
            "agent" => s.set_agent_status(t),
            "history" => s.set_history_status(t),
            "private" => s.set_private_status(t),
            "shortcut" => s.set_shortcut_error(t),
            _ => {}
        }
    }

    /// A change that did not save: the control is back as stored (the
    /// caller does that) and this says so, on the page and aloud.
    fn unsaved(&self, which: Option<&'static str>) {
        if let Some(w) = which {
            self.status(w, UNSAVED, false);
        }
        self.announce(UNSAVED, "assertive");
    }

    /// A click while the cursor buddy watches for one elsewhere: the step
    /// is put away, as a click off its target did on the web page.
    fn touched(&self) {
        if self.st.borrow().watching {
            self.send("buddy:dismiss", Value::Null);
        }
    }

    fn close_combos(&self) {
        let s = self.s();
        s.set_combo_open(SharedString::new());
        s.set_combo_mark(-1);
    }

    fn open_row(&self, id: &str) {
        self.close_combos();
        self.s().set_open_row(id.into());
    }

    // --------------------------------------------------------- routing

    fn on_act(&self, what: &str, arg: &str) {
        match what {
            "combo" => self.combo_toggle(arg),
            "open-url" => {
                if !arg.is_empty() {
                    self.send("open-external", json!({ "url": arg }));
                }
            }
            "models-mode" => self.set_models_mode(arg),
            "models-for" => self.set_models_provider(arg),
            "skill" => self.save_skill(arg),
            "open-skills" => self.open_skills(),
            "permission" => self.set_agent(json!({ "permission": arg }), None),
            "agent-revoke" => self.revoke_agent(),
            "voice-hotkey" => self.voice_hotkey_button(),
            "voice-engine" => self.voice_engine(arg),
            "voice-download" => self.voice_download(),
            "card-time" => {
                let n: i64 = arg.parse().unwrap_or(1);
                self.watch_save(json!({ "cardTime": n }));
            }
            "watch-test" => self.watch_test(),
            "open-history" => self.send("overlay:open-history", Value::Null),
            "clear-history" => self.history_confirm_open(),
            "history-confirm" => self.history_confirm(arg == "yes"),
            "private-remove" => self.private_remove(arg),
            "private-front" => self.private_add_front(),
            "shortcut" => self.shortcut_button(arg),
            "free" => self.use_free(arg),
            "reset-shortcuts" => self.reset_shortcuts(),
            "update-restart" => self.send("update:restart", Value::Null),
            "update-check" => self.update_check(),
            "edit-position" => self.send("hud:enter", Value::Null),
            "replay-tour" => self.send("tour:replay", Value::Null),
            _ => {}
        }
    }

    fn on_toggle(&self, what: &str, on: bool) {
        match what {
            "failover-across" => self.set_failover(json!({ "acrossProviders": on })),
            "failover-ask" => self.set_failover(json!({ "askBeforePaid": on })),
            "brain-enabled" => self.set_brain_enabled(on),
            "show-paid" => self.set_show_paid(on),
            "screenshots" => self.set_agent(json!({ "screenshots": on }), None),
            "returnCursor" => self.set_agent(json!({ "returnCursor": on }), None),
            "dictation" | "wake" | "cleanup" => {
                self.voice_toggle(what, on);
            }
            "save-history" => self.set_save_history(on),
            "launch-at-login" => self.set_launch_at_login(on),
            "show-button" => self.set_show_button(on),
            "watch" => self.watch_enable(on),
            _ => {
                if let Some(id) = what.strip_prefix("agent:") {
                    self.watch_agent(id, on);
                } else {
                    // The rest are the coding agents' preferences, by name.
                    self.watch_pref(what, on);
                }
            }
        }
    }

    fn on_slide(&self, what: &str, v: f32, done: bool) {
        match what {
            "steps" => {
                let steps = v.round();
                self.s().set_steps(steps);
                if done {
                    self.set_agent(json!({ "maxSteps": steps as i64 }), None);
                }
            }
            "text-scale" => self.slide_text_scale(v.round()),
            _ => {}
        }
    }

    fn on_pick(&self, cid: &str, id: &str) {
        match cid {
            "model" | "brain" | "agent" => self.combo_choose(cid, id),
            "language" => {
                self.close_combos();
                self.voice_save(json!({ "language": id }));
            }
            "mic" => {
                self.close_combos();
                self.voice_save(json!({ "device": id }));
            }
            _ => {}
        }
    }

    // ------------------------------------------------------- pickers

    /// The open picker's rows: (is a heading, id, name).
    fn combo_rows(&self, cid: &str) -> Vec<(bool, String, String)> {
        match cid {
            "model" | "brain" | "agent" => self.model_rows(cid),
            "language" | "mic" => self.voice_rows(cid),
            _ => Vec::new(),
        }
    }

    fn combo_toggle(&self, cid: &str) {
        let s = self.s();
        if s.get_combo_open().as_str() == cid {
            self.close_combos();
            return;
        }
        let rows = self.combo_rows(cid);
        let checked = self.combo_checked(cid);
        let at = rows
            .iter()
            .position(|(h, id, _)| !*h && *id == checked)
            .or_else(|| rows.iter().position(|(h, _, _)| !*h));
        s.set_combo_open(cid.into());
        self.combo_mark(&rows, at.map_or(-1, |i| i as i32));
    }

    fn combo_mark(&self, rows: &[(bool, String, String)], at: i32) {
        let s = self.s();
        s.set_combo_mark(at);
        let k = self.scale.get();
        let mut y = 4.0;
        for (h, _, _) in rows.iter().take(at.max(0) as usize) {
            y += if *h { 28.0 * k } else { 44.0 * k };
        }
        s.set_combo_scroll(y + 22.0 * k);
    }

    /// Arrows, Home, End, Enter and type-ahead in the open picker.
    fn combo_key(&self, k: &KeyPress) -> bool {
        let cid = self.s().get_combo_open().to_string();
        let rows = self.combo_rows(&cid);
        let opts: Vec<usize> = (0..rows.len()).filter(|&i| !rows[i].0).collect();
        if opts.is_empty() {
            return false;
        }
        let mark = self.s().get_combo_mark();
        let pos = opts.iter().position(|&i| i as i32 == mark);
        let go = |i: usize| self.combo_mark(&rows, opts[i] as i32);
        match k.vk {
            0x28 => go(pos.map_or(0, |p| (p + 1) % opts.len())),
            0x26 => go(pos.map_or(opts.len() - 1, |p| (p + opts.len() - 1) % opts.len())),
            0x24 => go(0),
            0x23 => go(opts.len() - 1),
            0x0D | 0x20 => {
                if let Some(p) = pos {
                    let id = rows[opts[p]].1.clone();
                    self.on_pick(&cid, &id);
                }
            }
            0x1B => self.close_combos(),
            0x09 => {
                self.close_combos();
                return false;
            }
            _ => {
                let Some(ch) = k.ch.filter(|_| !k.ctrl && !k.alt && !k.meta) else {
                    return false;
                };
                // The first option whose name starts with what was typed.
                let typed = {
                    let mut st = self.st.borrow_mut();
                    let fresh = st
                        .typed_at
                        .is_none_or(|t| t.elapsed() > Duration::from_millis(700));
                    if fresh {
                        st.typed.clear();
                    }
                    st.typed.push(ch);
                    st.typed_at = Some(Instant::now());
                    st.typed.clone()
                };
                let hit = opts
                    .iter()
                    .position(|&i| rows[i].2.to_lowercase().starts_with(&typed));
                if let Some(h) = hit {
                    go(h);
                }
            }
        }
        true
    }

    // ------------------------------------------------------------ keys

    fn on_key(&self, k: &KeyPress) -> bool {
        if self.st.borrow().capturing.is_some() {
            self.capture_key(k);
            return true;
        }
        if self.st.borrow().voice.chord.is_some() {
            self.chord_key(k);
            return true;
        }
        if !self.s().get_combo_open().is_empty() && self.combo_key(k) {
            return true;
        }
        let plain = !k.ctrl && !k.alt && !k.meta;
        match k.vk {
            0x70 if plain => {
                self.ui.invoke_jump("pg-shortcuts".into());
                true
            }
            0x1B => {
                self.escape();
                true
            }
            0x26 | 0x28 if k.alt && !k.ctrl => self.alt_move(k.vk == 0x26),
            0x21 if plain => {
                self.ui.invoke_scroll_by(-400.0);
                true
            }
            0x22 if plain => {
                self.ui.invoke_scroll_by(400.0);
                true
            }
            _ => false,
        }
    }

    fn escape(&self) {
        let s = self.s();
        if !s.get_combo_open().is_empty() {
            self.close_combos();
            return;
        }
        if !s.get_drag_id().is_empty() {
            s.set_drag_id(SharedString::new());
            s.set_drag_to(-1);
            return;
        }
        if self.cancel_remove() {
            return;
        }
        if !s.get_history_confirm().is_empty() {
            s.set_history_confirm(SharedString::new());
            return;
        }
        if !s.get_open_row().is_empty() {
            s.set_open_row(SharedString::new());
            return;
        }
        self.send("settings:close", Value::Null);
    }

    // --------------------------------------------------------- section

    /// A section someone else asked for: a page, a row, a provider
    /// (`prov-<id>`, from an answer's Fix key), or the names older callers
    /// use.
    fn go_to(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        if matches!(id, "providers" | "pg-account" | "sec-key") {
            self.go_to_providers(id == "sec-key");
            return;
        }
        if id.starts_with("pg-") {
            self.ui.invoke_jump(id.into());
            return;
        }
        if let Some(pid) = id.strip_prefix("prov-") {
            self.go_to_provider(pid);
            return;
        }
        let page = match id {
            "sec-model" | "sec-brain" | "sec-agent-model" => "pg-models",
            "sec-skill" => "pg-answers",
            "sec-agent" => "pg-agent",
            "sec-voice-dictation"
            | "sec-voice-wake"
            | "sec-voice-engine"
            | "sec-voice-language"
            | "sec-voice-mic"
            | "sec-voice-cleanup"
            | "sec-voice-privacy"
            | "sec-voice-credits" => "pg-voice",
            "sec-watch-agents" | "sec-watch-awake" | "sec-watch-cards" => "pg-watch",
            "sec-recipients" | "sec-history" | "sec-private" | "sec-share" => "pg-privacy",
            "sec-shortcuts" => "pg-shortcuts",
            "sec-text-size" | "sec-startup" | "sec-button" | "sec-about" | "sec-layout"
            | "sec-tour" => "pg-general",
            _ => return,
        };
        let opens = !matches!(
            id,
            "sec-voice-privacy"
                | "sec-voice-credits"
                | "sec-recipients"
                | "sec-share"
                | "sec-layout"
                | "sec-tour"
        );
        if opens {
            if matches!(id, "sec-model" | "sec-brain" | "sec-agent-model") {
                // The pickers live under "Choose for each job".
                self.set_models_mode("custom");
            }
            self.open_row(id);
        }
        self.ui.invoke_jump(page.into());
    }

    // ----------------------------------------------------------- probe

    fn texts(&self) -> String {
        let s = self.s();
        let mut out: Vec<String> = Vec::new();
        for i in 0..self.prov_model.row_count() {
            if let Some(r) = self.prov_model.row_data(i) {
                out.push(format!("{} {} {}", r.name, r.word, r.sub));
                if !r.msg.is_empty() {
                    out.push(r.msg.to_string());
                }
            }
        }
        for t in [
            s.get_prov_live(),
            s.get_key_storage(),
            s.get_model_summary(),
            s.get_brain_summary(),
            s.get_agent_model_summary(),
            s.get_skill_summary(),
            s.get_skills_status(),
            s.get_agent_summary(),
            s.get_agent_status(),
            s.get_dictation_summary(),
            s.get_wake_summary(),
            s.get_wake_status(),
            s.get_engine_summary(),
            s.get_engine_status(),
            s.get_language_summary(),
            s.get_mic_summary(),
            s.get_cleanup_summary(),
            s.get_watch_summary(),
            s.get_watch_status(),
            s.get_awake_summary(),
            s.get_cards_summary(),
            s.get_recipients(),
            s.get_history_summary(),
            s.get_history_status(),
            s.get_private_summary(),
            s.get_private_status(),
            s.get_share_summary(),
            s.get_share_line(),
            s.get_shortcut_summary(),
            s.get_shortcut_error(),
            s.get_startup_summary(),
            s.get_button_summary(),
            s.get_about_summary(),
            s.get_update_line(),
            s.get_layout_summary(),
        ] {
            if !t.is_empty() {
                out.push(t.to_string());
            }
        }
        let shortcuts = s.get_shortcuts();
        for i in 0..shortcuts.row_count() {
            if let Some(r) = shortcuts.row_data(i) {
                out.push(format!("{} {}", r.label, r.current));
                if !r.warn.is_empty() {
                    out.push(r.warn.to_string());
                }
            }
        }
        let apps = s.get_private_apps();
        for i in 0..apps.row_count() {
            if let Some(a) = apps.row_data(i) {
                out.push(format!("{} {}", a.name, a.exe));
            }
        }
        out.join("\n")
    }
}

impl super::Page for SettingsPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, payload: &Value) {
        match channel {
            "settings:section" => self.go_to(&text(payload, "id")),
            "provider:status" => self.on_provider_status(payload),
            "update:state" => {
                self.st.borrow_mut().data["update"] = payload.clone();
                self.render_update();
            }
            "ui:text-scale" => self.on_text_scale(payload),
            "voice:progress" => self.on_voice_progress(payload),
            "agents:state" => {
                self.watch_take(payload);
            }
            "buddy:watch" => {
                self.st.borrow_mut().watching = !payload["selector"].is_null();
            }
            // No walkthrough step points into Settings any more; a step
            // that asks is told its target is not here.
            "buddy:resolve" => self.send("buddy:resolve-result", json!({ "found": false })),
            FOCUSED => {
                self.check_clipboard_key();
                if self.st.borrow().voice.chord.is_none() {
                    self.voice_load();
                }
                self.watch_load();
                self.privacy_load();
            }
            _ => {}
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        self.scale.set(text_scale);
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        match key {
            "text" => json!(self.texts()),
            "section" => json!(self.ui.get_current_page().to_string()),
            "open" => json!(self.s().get_open_row().to_string()),
            "visible" => json!(super::history::is_visible(self.hwnd)),
            "providers" => {
                let st = self.st.borrow();
                json!(st
                    .prov
                    .list
                    .iter()
                    .map(|p| p.id.clone())
                    .collect::<Vec<_>>())
            }
            "shortcuts" => {
                let s = self.s().get_shortcuts();
                let mut m = serde_json::Map::new();
                for i in 0..s.row_count() {
                    if let Some(r) = s.row_data(i) {
                        m.insert(r.action.to_string(), json!(r.current.to_string()));
                    }
                }
                Value::Object(m)
            }
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.tick.stop();
        self.chord_timer.stop();
        self.wake_timer.stop();
        self.text_timer.stop();
        for t in self.statuses.borrow().values() {
            t.stop();
        }
    }
}

//! History (history/history.html), native. What history.js did: the list
//! from `history:list {query}`, grouped by day; open (`history:open`),
//! delete with six seconds of Undo (`history:remove` only after), delete
//! all with a confirmation (`history:clear`), the row menu (Ask again,
//! Copy question, Delete), turning saving back on, and `history:changed`.
//!
//! Keys: Up and Down move between rows (Up from the first goes back to
//! the search), Enter opens, Delete deletes, the menu key or Shift+F10
//! opens the row menu, Esc closes the menu, then the confirmation, then
//! clears the search, then closes the window.
//!
//! Probe keys (`Page::probe`):
//! - `"rows"`: how many conversations the list shows (number);
//! - `"query"`: the search as typed (string);
//! - `"text"`: what the page says: the titles shown, the empty line and the
//!   footer, one per line (string);
//! - `"visible"`: whether the window is on screen (bool);
//! - `"section"`: `"list"`, `"empty"` or `"confirm"` (string).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use chrono::TimeZone;
use serde_json::{json, Value};
use slint::{Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

use super::{bridge, HistRow, HistoryWindow};
use crate::shell::platform::{self, KeyPress};

const MIN: f64 = 60_000.0;

/// The answer's opening as words: markdown marks (`**`, `#`, list
/// numbers) are not shown in the list.
fn plain_reply(raw: &str) -> String {
    let words: Vec<String> = super::markdown::lines(raw)
        .iter()
        .map(super::markdown::plain)
        .filter(|t| !t.trim().is_empty())
        .collect();
    words.join(" ")
}
const HOUR: f64 = 60.0 * MIN;
const DAY: f64 = 24.0 * HOUR;

#[derive(Default)]
struct State {
    query: String,
    rows: Vec<Value>,
    enabled: bool,
    /// A delete that Undo can still take back.
    pending: Option<(String, String)>,
    note: String,
    confirming: bool,
    menu_for: Option<String>,
    menu_mark: i32,
    /// Index into the shown rows (headings included) of the row the
    /// keyboard is on, or -1 for the search.
    mark: i32,
    /// Conversation ids of the shown rows, None for a heading.
    shown: Vec<Option<String>>,
}

pub struct HistoryPage {
    hwnd: isize,
    label: String,
    ui: HistoryWindow,
    me: Weak<HistoryPage>,
    st: RefCell<State>,
    model: Rc<VecModel<HistRow>>,
    scale: Cell<f32>,
    search_timer: Timer,
    undo_timer: Timer,
    note_timer: Timer,
}

impl HistoryPage {
    pub fn new(label: &str, spec: platform::Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, HistoryWindow::new)?;
        let model = Rc::new(VecModel::<HistRow>::default());
        ui.set_rows(ModelRc::from(model.clone()));
        let page = Rc::new_cyclic(|me| HistoryPage {
            hwnd,
            label: label.to_string(),
            ui,
            me: me.clone(),
            st: RefCell::new(State {
                enabled: true,
                mark: -1,
                ..State::default()
            }),
            model,
            scale: Cell::new(1.0),
            search_timer: Timer::default(),
            undo_timer: Timer::default(),
            note_timer: Timer::default(),
        });
        page.wire();
        let me = page.me.clone();
        super::set_keys(hwnd, label, move |k| {
            me.upgrade().is_some_and(|p| p.on_key(k))
        });
        page.refresh();
        page.ui.invoke_focus_search();
        Ok(page)
    }

    fn wire(&self) {
        let ui = &self.ui;
        let hwnd = self.hwnd;
        ui.on_drag(move || platform::begin_drag(hwnd));
        ui.on_maximize(move || platform::toggle_maximize(hwnd));
        let me = self.me.clone();
        ui.on_close_clicked(move || {
            if let Some(p) = me.upgrade() {
                p.close_window();
            }
        });
        let me = self.me.clone();
        ui.on_search_edited(move |_| {
            if let Some(p) = me.upgrade() {
                p.search_changed();
            }
        });
        let me = self.me.clone();
        ui.on_search_accepted(move || {
            if let Some(p) = me.upgrade() {
                p.open_first();
            }
        });
        let me = self.me.clone();
        ui.on_open_row(move |id| {
            if let Some(p) = me.upgrade() {
                p.open_row(&id);
            }
        });
        let me = self.me.clone();
        ui.on_delete_row(move |id| {
            if let Some(p) = me.upgrade() {
                p.remove(&id);
            }
        });
        let me = self.me.clone();
        ui.on_menu_at(move |id, x, y| {
            if let Some(p) = me.upgrade() {
                p.open_menu(&id, x, y);
            }
        });
        let me = self.me.clone();
        ui.on_menu_pick(move |cmd| {
            if let Some(p) = me.upgrade() {
                p.menu_pick(&cmd);
            }
        });
        let me = self.me.clone();
        ui.on_menu_dismiss(move || {
            if let Some(p) = me.upgrade() {
                p.close_menu();
            }
        });
        let me = self.me.clone();
        ui.on_undo(move || {
            if let Some(p) = me.upgrade() {
                p.undo();
            }
        });
        let me = self.me.clone();
        ui.on_clear_all(move || {
            if let Some(p) = me.upgrade() {
                p.open_confirm();
            }
        });
        let me = self.me.clone();
        ui.on_confirm(move |yes| {
            if let Some(p) = me.upgrade() {
                if yes {
                    p.clear_all();
                } else {
                    p.close_confirm();
                }
            }
        });
        let me = self.me.clone();
        ui.on_turn_on(move || {
            if let Some(p) = me.upgrade() {
                p.turn_on();
            }
        });
    }

    // ------------------------------------------------------------ list

    fn refresh(&self) {
        let query = self.st.borrow().query.clone();
        let me = self.me.clone();
        bridge::call(
            &self.label,
            "history:list",
            json!({ "query": query }),
            move |r| {
                let Some(p) = me.upgrade() else {
                    return;
                };
                let got = r.unwrap_or(Value::Null);
                {
                    let mut st = p.st.borrow_mut();
                    st.rows = got["rows"].as_array().cloned().unwrap_or_default();
                    if let Some(on) = got["enabled"].as_bool() {
                        st.enabled = on;
                    }
                }
                p.render();
            },
        );
    }

    fn visible_rows(st: &State) -> Vec<Value> {
        let gone = st.pending.as_ref().map(|(id, _)| id.as_str());
        st.rows
            .iter()
            .filter(|r| Some(id_of(r).as_str()) != gone)
            .cloned()
            .collect()
    }

    fn render(&self) {
        let now = now_ms();
        let mut st = self.st.borrow_mut();
        let shown = Self::visible_rows(&st);
        let keep = usize::try_from(st.mark)
            .ok()
            .and_then(|i| st.shown.get(i).cloned().flatten());
        let mut rows = Vec::with_capacity(shown.len() + 8);
        let mut ids = Vec::with_capacity(shown.len() + 8);
        let mut group = String::new();
        for e in &shown {
            let at = e["at"].as_f64().unwrap_or(0.0);
            let g = day_group(at, now);
            if rows.is_empty() || g != group {
                group = g.clone();
                rows.push(HistRow {
                    header: true,
                    title: g.into(),
                    ..HistRow::default()
                });
                ids.push(None);
            }
            let asked = e["asked"].as_u64().unwrap_or(1).max(1);
            let meta = if asked > 1 {
                format!("{} · {asked} questions", when(at, now))
            } else {
                when(at, now)
            };
            rows.push(HistRow {
                header: false,
                id: id_of(e).into(),
                title: str_of(e, "title").into(),
                reply: plain_reply(&str_of(e, "reply")).into(),
                meta: meta.into(),
            });
            ids.push(Some(id_of(e)));
        }
        st.mark = keep
            .and_then(|k| ids.iter().position(|i| i.as_deref() == Some(k.as_str())))
            .map_or(-1, |i| i as i32);
        st.shown = ids;
        let n = shown.len();
        self.model.set_vec(rows);
        self.ui.set_mark(st.mark);
        self.ui.set_list_shown(n > 0);
        let empty = if n > 0 {
            String::new()
        } else if !st.query.is_empty() {
            format!("Nothing matches \"{}\".", st.query)
        } else if !st.enabled {
            "History is off, so nothing is being saved. Turn it on in Settings, Privacy.".into()
        } else {
            "Conversations you have will be listed here. Nothing is saved until you ask something."
                .into()
        };
        self.ui.set_empty_text(empty.into());
        self.ui
            .set_empty_turn_on(n == 0 && st.query.is_empty() && !st.enabled);
        self.ui.set_off_note(!st.enabled && n > 0);
        drop(st);
        self.render_foot();
    }

    fn render_foot(&self) {
        let st = self.st.borrow();
        let n = Self::visible_rows(&st).len();
        let count = if n > 0 && st.query.is_empty() {
            format!(
                "{n} {}",
                if n == 1 {
                    "conversation"
                } else {
                    "conversations"
                }
            )
        } else {
            String::new()
        };
        self.ui.set_count_text(count.into());
        self.ui.set_note_text(st.note.as_str().into());
        self.ui.set_undo_shown(st.pending.is_some());
        // While a delete can still be undone, Undo stands alone: Delete all
        // beside it would be one slip away.
        self.ui
            .set_clear_shown(st.pending.is_none() && n > 0 && st.query.is_empty());
        let confirm = if st.confirming {
            if n == 1 {
                "Delete the 1 conversation?".to_string()
            } else {
                format!("Delete all {n} conversations?")
            }
        } else {
            String::new()
        };
        self.ui.set_confirm_text(confirm.into());
    }

    // ------------------------------------------------------------ note

    fn say(&self, text: &str, sticky: bool) {
        self.note_timer.stop();
        self.st.borrow_mut().note = text.to_string();
        self.render_foot();
        if text.is_empty() {
            return;
        }
        bridge::send(
            &self.label,
            "a11y:announce",
            json!({ "text": text, "priority": "polite" }),
        );
        if !sticky {
            let me = self.me.clone();
            self.note_timer
                .start(TimerMode::SingleShot, Duration::from_secs(4), move || {
                    if let Some(p) = me.upgrade() {
                        p.st.borrow_mut().note.clear();
                        p.render_foot();
                    }
                });
        }
    }

    // ---------------------------------------------------------- delete

    fn commit_pending(&self) {
        let taken = self.st.borrow_mut().pending.take();
        self.undo_timer.stop();
        if let Some((id, _)) = taken {
            bridge::send(&self.label, "history:remove", json!({ "id": id }));
        }
    }

    fn remove(&self, id: &str) {
        let title = {
            let st = self.st.borrow();
            match st.rows.iter().find(|r| id_of(r) == id) {
                Some(r) => str_of(r, "title"),
                None => return,
            }
        };
        // The row the keyboard goes to next: the one after, else before.
        let next = {
            let st = self.st.borrow();
            let order: Vec<String> = Self::visible_rows(&st).iter().map(id_of).collect();
            let at = order.iter().position(|x| x == id).unwrap_or(0);
            order
                .get(at + 1)
                .or_else(|| at.checked_sub(1).and_then(|i| order.get(i)))
                .cloned()
        };
        self.commit_pending();
        self.close_confirm();
        {
            let mut st = self.st.borrow_mut();
            st.pending = Some((id.to_string(), title.clone()));
            st.note = format!("Deleted \"{title}\".");
            st.mark = -1;
        }
        self.note_timer.stop();
        let me = self.me.clone();
        let gone = id.to_string();
        self.undo_timer
            .start(TimerMode::SingleShot, Duration::from_secs(6), move || {
                if let Some(p) = me.upgrade() {
                    p.commit_pending();
                    {
                        let mut st = p.st.borrow_mut();
                        st.rows.retain(|r| id_of(r) != gone);
                        st.note.clear();
                    }
                    p.render();
                }
            });
        self.render();
        if let Some(next) = next {
            self.mark_id(&next);
        }
    }

    fn undo(&self) {
        let back = self.st.borrow_mut().pending.take();
        self.undo_timer.stop();
        let Some((id, _)) = back else {
            return;
        };
        self.st.borrow_mut().note.clear();
        self.render();
        self.mark_id(&id);
        self.say("Restored.", false);
    }

    // --------------------------------------------------------- confirm

    fn open_confirm(&self) {
        self.st.borrow_mut().confirming = true;
        self.render_foot();
    }

    fn close_confirm(&self) {
        self.st.borrow_mut().confirming = false;
        self.render_foot();
    }

    fn clear_all(&self) {
        let n = self.st.borrow().rows.len();
        {
            let mut st = self.st.borrow_mut();
            st.pending = None;
            st.confirming = false;
        }
        self.undo_timer.stop();
        let me = self.me.clone();
        bridge::call(&self.label, "history:clear", Value::Null, move |r| {
            let Some(p) = me.upgrade() else {
                return;
            };
            let ok = matches!(&r, Ok(v) if v["ok"] != json!(false));
            if !ok {
                p.render_foot();
                p.say("Couldn't delete the history.", false);
                return;
            }
            p.st.borrow_mut().rows.clear();
            p.render();
            p.say(
                &if n == 1 {
                    "Deleted 1 conversation.".to_string()
                } else {
                    format!("Deleted {n} conversations.")
                },
                false,
            );
            p.ui.invoke_focus_search();
            p.refresh();
        });
    }

    // ---------------------------------------------------------- search

    fn search_changed(&self) {
        // "Delete all N" was counted for the list before this search.
        self.close_confirm();
        // The filter runs in Rust over every message body; typing fast
        // should not queue one pass per keystroke.
        let me = self.me.clone();
        self.search_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(120),
            move || {
                if let Some(p) = me.upgrade() {
                    let q = p.ui.get_query().trim().to_string();
                    {
                        let mut st = p.st.borrow_mut();
                        st.query = q;
                        st.mark = -1;
                    }
                    p.refresh();
                }
            },
        );
    }

    fn reset_search(&self) {
        self.ui.invoke_set_search(SharedString::new());
        {
            let mut st = self.st.borrow_mut();
            st.query.clear();
            st.mark = -1;
        }
        self.refresh();
        self.ui.invoke_focus_search();
    }

    // ------------------------------------------------------------ rows

    fn open_row(&self, id: &str) {
        self.commit_pending();
        let me = self.me.clone();
        bridge::call(&self.label, "history:open", json!({ "id": id }), move |r| {
            let failed = match &r {
                Err(_) => true,
                Ok(v) => v["ok"] == json!(false),
            };
            if failed {
                if let Some(p) = me.upgrade() {
                    p.say(
                        "Couldn't open that conversation. It may have been deleted.",
                        false,
                    );
                    p.refresh();
                }
            }
        });
    }

    fn open_first(&self) {
        let first = self.st.borrow().shown.iter().flatten().next().cloned();
        if let Some(id) = first {
            self.open_row(&id);
        }
    }

    fn mark_at(&self, i: i32) {
        self.st.borrow_mut().mark = i;
        self.ui.set_mark(i);
        let Ok(at) = usize::try_from(i) else {
            return;
        };
        let st = self.st.borrow();
        let s = self.scale.get();
        let (head, row) = (36.0 * s, 68.0 * s);
        let y: f32 = st
            .shown
            .iter()
            .take(at)
            .map(|r| if r.is_some() { row } else { head })
            .sum();
        drop(st);
        self.ui.invoke_show_row(y, row);
    }

    fn mark_id(&self, id: &str) {
        let at = self
            .st
            .borrow()
            .shown
            .iter()
            .position(|r| r.as_deref() == Some(id));
        if let Some(i) = at {
            self.mark_at(i as i32);
        }
    }

    /// The next conversation row from `from` in direction `dir`.
    fn step(&self, from: i32, dir: i32) -> Option<i32> {
        let st = self.st.borrow();
        let n = st.shown.len() as i32;
        let mut i = from + dir;
        while (0..n).contains(&i) {
            if st.shown[i as usize].is_some() {
                return Some(i);
            }
            i += dir;
        }
        None
    }

    fn marked_id(&self) -> Option<String> {
        let st = self.st.borrow();
        usize::try_from(st.mark)
            .ok()
            .and_then(|i| st.shown.get(i).cloned().flatten())
    }

    // ------------------------------------------------------------ menu

    fn open_menu(&self, id: &str, x: f32, y: f32) {
        {
            let mut st = self.st.borrow_mut();
            st.menu_for = Some(id.to_string());
            st.menu_mark = 0;
        }
        self.ui.set_menu_x(x);
        self.ui.set_menu_y(y);
        self.ui.set_menu_mark(0);
        self.ui.set_menu_open(true);
    }

    fn close_menu(&self) {
        self.st.borrow_mut().menu_for = None;
        self.ui.set_menu_open(false);
    }

    fn question_of(&self, id: &str) -> String {
        let st = self.st.borrow();
        st.rows
            .iter()
            .find(|r| id_of(r) == id)
            .map(|r| {
                let q = str_of(r, "question");
                if q.is_empty() {
                    str_of(r, "title")
                } else {
                    q
                }
            })
            .unwrap_or_default()
    }

    fn menu_pick(&self, cmd: &str) {
        let id = self.st.borrow().menu_for.clone();
        self.close_menu();
        let Some(id) = id else {
            return;
        };
        match cmd {
            "ask" => {
                let me = self.me.clone();
                bridge::call(
                    &self.label,
                    "bar:prefill-open",
                    json!({ "text": self.question_of(&id) }),
                    move |r| {
                        if refused(&r) {
                            if let Some(p) = me.upgrade() {
                                p.say("Couldn't open the bar.", false);
                            }
                        }
                    },
                );
            }
            "copy" => {
                let me = self.me.clone();
                bridge::call(
                    &self.label,
                    "clipboard:write",
                    json!({ "text": self.question_of(&id) }),
                    move |r| {
                        if let Some(p) = me.upgrade() {
                            let text = if refused(&r) {
                                "Couldn't copy the question."
                            } else {
                                "Question copied."
                            };
                            p.say(text, false);
                        }
                    },
                );
            }
            "delete" => self.remove(&id),
            _ => {}
        }
    }

    // ------------------------------------------------------------ rest

    fn turn_on(&self) {
        let me = self.me.clone();
        bridge::call(
            &self.label,
            "settings:set-save-history",
            json!({ "on": true }),
            move |r| {
                if matches!(&r, Ok(v) if v["saveHistory"] == json!(false)) {
                    return;
                }
                if let Some(p) = me.upgrade() {
                    p.st.borrow_mut().enabled = true;
                    p.refresh();
                    p.say("History is on.", false);
                }
            },
        );
    }

    fn close_window(&self) {
        self.commit_pending();
        bridge::send(&self.label, "history:close", Value::Null);
    }

    fn on_key(&self, k: &KeyPress) -> bool {
        const ESC: u16 = 0x1B;
        const ENTER: u16 = 0x0D;
        const TAB: u16 = 0x09;
        const UP: u16 = 0x26;
        const DOWN: u16 = 0x28;
        const HOME: u16 = 0x24;
        const END: u16 = 0x23;
        const DELETE: u16 = 0x2E;
        const APPS: u16 = 0x5D;
        const F10: u16 = 0x79;
        if k.ctrl || k.alt || k.meta {
            return false;
        }
        if self.ui.get_menu_open() {
            let mark = self.st.borrow().menu_mark;
            let next = match k.vk {
                UP => (mark + 2) % 3,
                DOWN => (mark + 1) % 3,
                HOME => 0,
                END => 2,
                ENTER => {
                    let cmd = ["ask", "copy", "delete"][mark.clamp(0, 2) as usize];
                    self.menu_pick(cmd);
                    return true;
                }
                ESC | TAB => {
                    self.close_menu();
                    return true;
                }
                _ => return true,
            };
            self.st.borrow_mut().menu_mark = next;
            self.ui.set_menu_mark(next);
            return true;
        }
        let mark = self.st.borrow().mark;
        match k.vk {
            ESC => {
                if self.st.borrow().confirming {
                    self.close_confirm();
                } else if !self.ui.get_query().is_empty() {
                    self.reset_search();
                } else {
                    self.close_window();
                }
                true
            }
            DOWN => {
                if let Some(i) = self.step(mark, 1) {
                    self.mark_at(i);
                }
                true
            }
            UP => {
                match self.step(mark, -1) {
                    Some(i) if mark >= 0 => self.mark_at(i),
                    _ => {
                        self.mark_at(-1);
                        self.ui.invoke_focus_search();
                    }
                }
                true
            }
            HOME | END if mark >= 0 => {
                let n = self.st.borrow().shown.len() as i32;
                let to = if k.vk == HOME {
                    self.step(-1, 1)
                } else {
                    self.step(n, -1)
                };
                if let Some(i) = to {
                    self.mark_at(i);
                }
                true
            }
            ENTER if mark >= 0 => {
                if let Some(id) = self.marked_id() {
                    self.open_row(&id);
                }
                true
            }
            DELETE if mark >= 0 => {
                if let Some(id) = self.marked_id() {
                    self.remove(&id);
                }
                true
            }
            APPS => self.menu_from_keys(),
            F10 if k.shift => self.menu_from_keys(),
            _ => false,
        }
    }

    /// The menu key: the row menu for the marked row, beside it.
    fn menu_from_keys(&self) -> bool {
        let Some(id) = self.marked_id() else {
            return false;
        };
        let mark = self.st.borrow().mark;
        let s = self.scale.get();
        let y: f32 = {
            let st = self.st.borrow();
            st.shown
                .iter()
                .take(mark.max(0) as usize)
                .map(|r| if r.is_some() { 68.0 * s } else { 36.0 * s })
                .sum()
        };
        // Under the title and the search, less what is scrolled away.
        let top = 48.0 + 48.0 * s + y + 34.0 * s;
        let list_top = 48.0 + 48.0 * s;
        let clamped = top.clamp(list_top, list_top + self.ui.get_list_height());
        self.open_menu(&id, 40.0, clamped);
        true
    }

    fn texts(&self) -> String {
        let mut out = Vec::new();
        for i in 0..self.model.row_count() {
            if let Some(r) = self.model.row_data(i) {
                out.push(r.title.to_string());
            }
        }
        out.push(self.ui.get_empty_text().to_string());
        out.push(self.ui.get_count_text().to_string());
        out.push(self.ui.get_note_text().to_string());
        out.push(self.ui.get_confirm_text().to_string());
        out.retain(|s| !s.is_empty());
        out.join("\n")
    }
}

impl super::Page for HistoryPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, _payload: &Value) {
        // Rust pushes a refresh when a conversation is saved while this
        // window is open, so the list is never stale behind a live chat.
        if channel == "history:changed" {
            self.refresh();
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        self.scale.set(text_scale);
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        match key {
            "rows" => json!(Self::visible_rows(&self.st.borrow()).len()),
            "query" => json!(self.ui.get_query().to_string()),
            "text" => json!(self.texts()),
            "visible" => json!(is_visible(self.hwnd)),
            "section" => {
                let s = if self.st.borrow().confirming {
                    "confirm"
                } else if self.ui.get_list_shown() {
                    "list"
                } else {
                    "empty"
                };
                json!(s)
            }
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.search_timer.stop();
        self.note_timer.stop();
        self.undo_timer.stop();
        // The window can go before the six seconds are up; the delete still
        // holds. The window is gone, so it goes to the store directly.
        if let Some((id, _)) = self.st.borrow_mut().pending.take() {
            crate::history::get().remove(&id);
        }
    }
}

fn refused(r: &Result<Value, String>) -> bool {
    match r {
        Err(_) => true,
        Ok(v) => v["ok"] == json!(false),
    }
}

fn id_of(v: &Value) -> String {
    match &v["id"] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn str_of(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_string()
}

pub(super) fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

pub(super) fn is_visible(hwnd: isize) -> bool {
    // SAFETY: IsWindowVisible accepts any value.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd as _) != 0 }
}

fn local(ms: f64) -> Option<chrono::DateTime<chrono::Local>> {
    chrono::Local.timestamp_millis_opt(ms as i64).single()
}

/// Midnight today, local time, in ms.
fn start_of_today(now: f64) -> f64 {
    let Some(d) = local(now) else {
        return now;
    };
    d.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|m| chrono::Local.from_local_datetime(&m).earliest())
        .map_or(now, |m| m.timestamp_millis() as f64)
}

/// Rows are grouped by calendar day: Today, Yesterday, the weekday for the
/// rest of the week, then Earlier.
fn day_group(at: f64, now: f64) -> String {
    let t0 = start_of_today(now);
    if at >= t0 {
        return "Today".into();
    }
    if at >= t0 - DAY {
        return "Yesterday".into();
    }
    if at >= t0 - 6.0 * DAY {
        return local(at).map_or_else(|| "Earlier".into(), |d| d.format("%A").to_string());
    }
    "Earlier".into()
}

/// The label on a row never contradicts its group: relative today, the
/// time under a day, the date under Earlier.
fn when(at: f64, now: f64) -> String {
    let t0 = start_of_today(now);
    if at >= t0 {
        let delta = now - at;
        if delta < MIN {
            return "Just now".into();
        }
        if delta < HOUR {
            return format!("{} min ago", (delta / MIN) as i64);
        }
        return format!("{} h ago", (delta / HOUR) as i64);
    }
    let Some(d) = local(at) else {
        return String::new();
    };
    if at >= t0 - 6.0 * DAY {
        return d.format("%H:%M").to_string();
    }
    d.format("%-d %b").to_string()
}

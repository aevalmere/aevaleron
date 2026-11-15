//! The find-act lane's smoke labels (`tools/smoke-labels/find-act.txt`):
//! Enter on a Find hit, in the smoke's Notepad. A word in the document is
//! selected and read back through UI Automation; the menu bar's View is
//! pressed and its menu seen open; and a menu command that is not on screen
//! (Edit, Select All) is found by name and run, which the selection shows.
//!
//! The Notepad is the classic one on the runner and the Windows 11 one on a
//! newer image. The first two checks work in both. The Windows 11 one shows
//! its menus to nobody until they are opened, so there the third prints
//! `skipped` with the reason in place of a verdict.

use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
    WM_CANCELMODE,
};

use crate::desktop::{self, protocol};
use crate::find::act;
use crate::host;
use crate::selftest::{Cx, Input};
use crate::windows::{self, flags, OVERLAY};

const WAIT: Duration = Duration::from_secs(8);
/// A page call that folds the bar, brings a window forward and acts in it.
const LONG: Duration = Duration::from_secs(15);

/// A word in the fixture's second line, once.
const WORD: &str = "settle";
/// The menu both Notepads have on their menu bar.
const MENU: &str = "View";
/// A command inside a menu, so not on screen: its effect is the selection.
const COMMAND: &str = "select all";

const MODE_JS: &str = "document.body.dataset.mode || ''";

const ENTER_JS: &str = r#"
(() => {
  const i = document.getElementById('query-input');
  i.focus();
  const e = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
  i.dispatchEvent(e);
  return e.defaultPrevented;
})()
"#;

const CLEAR_JS: &str = r#"
(() => {
  const i = document.getElementById('query-input');
  i.value = '';
  i.dispatchEvent(new Event('input', { bubbles: true }));
  return i.value;
})()
"#;

pub fn run(cx: &mut Cx) {
    let Some(np) = cx.notepad().cloned() else {
        cx.log("findAct", "skipped (no Notepad)");
        return;
    };
    let handle = json!(np.handle);
    let pid = np.win["pid"].as_u64().unwrap_or(0) as u32;
    let class = np.win["className"].as_str().unwrap_or_default();
    cx.log("findActNotepad", format!("{class} pid={pid}"));

    select_word(cx, &handle);
    press_menu(cx, &handle, pid);
    menu_command(cx, &handle);

    // As the lanes after expect it: nothing selected, the bar folded and
    // empty, Find closed.
    front(cx, &handle);
    let _ = cx.input(Input::Key("Ctrl+Home"));
    let _ = cx.eval(OVERLAY, CLEAR_JS);
    fold(cx);
    let text = cx.block_on(desktop::client().doc_text(Some(&handle), 4000, 700));
    let kept = text.as_ref().is_ok_and(|t| {
        let doc = t["docs"][0]["text"].as_str().unwrap_or_default();
        doc.contains("Invoice 2291 for March") && doc.contains("total due 42.00")
    });
    cx.log("findActTextKept", kept);
}

// ----------------------------------------------------------------- helpers

fn rows() -> Value {
    host::services().find.rows()
}

fn mode(cx: &Cx) -> String {
    let m = cx.eval(OVERLAY, MODE_JS).unwrap_or(Value::Null);
    m.as_str().unwrap_or_default().to_string()
}

fn fold(cx: &Cx) {
    cx.on_main(windows::collapse_panel);
    cx.wait_for(WAIT, || mode(cx) == "pill");
    cx.sleep(400);
}

fn front(cx: &Cx, handle: &Value) {
    let _ = cx.block_on(desktop::client().focus_window(handle));
    cx.sleep(300);
}

/// Notepad in front, then the bar opened in Find with `query` in it, as
/// Ctrl+F with words selected opens it. The list once it has what `want`
/// asks for, or as it stands when the wait ran out.
fn find(cx: &Cx, handle: &Value, query: &str, want: impl Fn(&Value) -> bool) -> Value {
    fold(cx);
    front(cx, handle);
    let q = query.to_string();
    cx.on_main(move |app| windows::open_bar(app, Some("find"), Some(q)));
    cx.wait_for(WAIT, || {
        let r = rows();
        r["query"] == query && r["current"].as_i64() >= Some(0) && want(&r)
    });
    // The pages get the list a moment after the engine has it.
    cx.sleep(400);
    rows()
}

fn row_at(list: &Value, at: usize) -> Value {
    list["rows"].get(at).cloned().unwrap_or(Value::Null)
}

fn current_row(list: &Value) -> Value {
    let at = list["current"].as_u64().unwrap_or(u64::MAX);
    row_at(list, usize::try_from(at).unwrap_or(usize::MAX))
}

/// The first row `want` picks.
fn position(list: &Value, want: impl Fn(&Value) -> bool) -> Option<usize> {
    list["rows"].as_array()?.iter().position(want)
}

fn says(row: &Value, key: &str, text: &str) -> bool {
    row[key]
        .as_str()
        .is_some_and(|m| m.eq_ignore_ascii_case(text))
}

/// Makes row `at` the selected one, as the arrows do, and gives the list.
fn pick(cx: &Cx, at: usize) -> Value {
    let find = &host::services().find;
    if find.rows()["current"].as_u64() != Some(at as u64) {
        cx.block_on(find.step(&json!({ "index": at })));
        cx.sleep(300);
    }
    find.rows()
}

/// `find:act` from the bar's page on the selected row of `list`, the call
/// its Enter makes; `gen` stands in for the list's own when given.
fn act_from_page(cx: &Cx, list: &Value, gen: Option<i64>) -> Result<Value, String> {
    let row = current_row(list);
    let gen = gen.map_or(list["gen"].clone(), |g| json!(g));
    let p = json!({ "gen": gen, "index": list["current"], "match": row["match"] });
    let js = format!("window.aeval.invoke('find:act', {p})");
    cx.eval_for(OVERLAY, &js, LONG)
}

fn done(reply: &Result<Value, String>) -> bool {
    reply.as_ref().is_ok_and(|v| v["status"] == "done")
}

/// What is selected in Notepad's document, read through UI Automation.
fn selected(cx: &Cx, handle: &Value) -> String {
    let client = desktop::client();
    let found = cx.block_on(client.find_text(Some(handle), "Invoice", 1));
    let doc = found
        .ok()
        .and_then(|f| f["docs"][0]["ref"].as_str().map(str::to_string));
    let Some(doc) = doc else {
        return "(no document)".to_string();
    };
    match cx.block_on(client.selection(&doc)) {
        Ok(v) => v["text"].as_str().unwrap_or_default().to_string(),
        Err(e) => format!("({}: {})", e.code, e.message),
    }
}

fn overlay_in_front() -> bool {
    // SAFETY: a query with no arguments.
    let fg = unsafe { GetForegroundWindow() } as isize;
    flags::hwnd_for(OVERLAY) == Some(fg)
}

/// A classic menu of process `pid` on screen. `#32768` is the class of
/// every Windows menu window.
fn popup_of(pid: u32) -> bool {
    let class: Vec<u16> = "#32768\0".encode_utf16().collect();
    let mut after: HWND = null_mut();
    loop {
        // SAFETY: `class` is NUL-terminated and alive; the handles come from
        // Windows and are only passed back to it.
        unsafe {
            after = FindWindowExW(null_mut(), after, class.as_ptr(), null());
            if after.is_null() {
                return false;
            }
            let mut owner = 0;
            GetWindowThreadProcessId(after, &mut owner);
            if owner == pid && IsWindowVisible(after) != 0 {
                return true;
            }
        }
    }
}

/// The window's text as UI Automation reads it, lowercased.
fn window_text(cx: &Cx, handle: &Value) -> String {
    let read = cx.block_on(desktop::client().get_text(Some(handle), &Value::Null, 20000, false));
    let text = read
        .ok()
        .and_then(|r| r["text"].as_str().map(str::to_lowercase));
    text.unwrap_or_default()
}

// ------------------------------------------------------------------ checks

/// A word in the document: the row says Select, a list that is not the one
/// on show is left alone, and Enter in the bar folds the bar, gives Notepad
/// the foreground and selects the word there.
fn select_word(cx: &Cx, handle: &Value) {
    front(cx, handle);
    let _ = cx.input(Input::Key("Ctrl+Home"));
    let before = selected(cx, handle);
    let list = find(cx, handle, WORD, |r| {
        position(r, |x| says(x, "match", WORD)).is_some()
    });
    let row = current_row(&list);
    cx.log("findActRow", &row);
    let word = says(&row, "match", WORD) && row["act"] == "Select";
    cx.must("findActWord", word, list.to_string());

    // Enter that names another list: nothing is done, and the bar stays.
    let refused = act_from_page(cx, &list, Some(1));
    let said = refused.as_ref().is_ok_and(|v| v["status"] == "changed");
    let still = selected(cx, handle);
    let open = mode(cx);
    cx.must(
        "findActStale",
        said && still == before && open == "panel",
        format!("{refused:?} selected={still:?} bar={open}"),
    );

    // The real key when the bar has the keyboard; a key that went to
    // Notepad would type into the file. Otherwise the page's own handler.
    let started = Instant::now();
    let real = overlay_in_front();
    let pressed = if real {
        format!("{:?}", cx.input(Input::Key("Enter")))
    } else {
        format!("{:?}", cx.eval(OVERLAY, ENTER_JS))
    };
    let is_word = |cx: &Cx| selected(cx, handle).eq_ignore_ascii_case(WORD);
    let mut landed = cx.wait_for(Duration::from_secs(5), || is_word(cx));
    let mut how = if real { "key" } else { "page" };
    if !landed && real && mode(cx) == "panel" {
        // The key did not reach the field: the same press, made in the page.
        how = "page after key";
        let _ = cx.eval(OVERLAY, ENTER_JS);
        landed = cx.wait_for(WAIT, || is_word(cx));
    }
    let ms = started.elapsed().as_millis();
    let got = selected(cx, handle);
    cx.log("findActEnter", format!("{how} {pressed} ms={ms}"));
    cx.must(
        "findActSelect",
        landed,
        format!("selected {got:?}, before {before:?}"),
    );

    let folded = cx.wait_for(WAIT, || mode(cx) == "pill");
    let fg = cx.block_on(desktop::client().foreground_window());
    let ours = fg
        .as_ref()
        .is_ok_and(|w| w["handle"] == *handle && w["isForeground"] == true);
    let closed = !host::services().find.is_open();
    let title = fg.as_ref().map(|w| w["title"].clone());
    cx.must(
        "findActFolds",
        folded && ours && closed,
        format!("folded={folded} findClosed={closed} foreground={title:?}"),
    );
}

/// The menu bar's View: on screen, the row says Press, and acting on it
/// opens the menu. Then the menu is closed again.
fn press_menu(cx: &Cx, handle: &Value, pid: u32) {
    let before = window_text(cx, handle);
    let list = find(cx, handle, MENU, |r| {
        position(r, |x| says(x, "match", MENU)).is_some()
    });
    let on_bar = |x: &Value| x["kind"] != "menu" && says(x, "match", MENU) && x["act"] == "Press";
    let Some(at) = position(&list, on_bar) else {
        cx.must("findActPress", false, format!("no row to press: {list}"));
        return;
    };
    let list = pick(cx, at);
    cx.log("findActPressRow", current_row(&list));
    let started = Instant::now();
    let reply = act_from_page(cx, &list, None);
    // A classic menu is a window of its own; a Windows 11 one adds its
    // commands to what the window reads as.
    let new_words = |cx: &Cx| {
        let now = window_text(cx, handle);
        ["zoom", "status bar"]
            .iter()
            .any(|w| now.contains(w) && !before.contains(w))
    };
    let mut seen = "";
    let opened = cx.wait_for(Duration::from_secs(5), || {
        if popup_of(pid) {
            seen = "popup";
        } else if new_words(cx) {
            seen = "text";
        }
        !seen.is_empty()
    });
    let ms = started.elapsed().as_millis();
    cx.must(
        "findActPress",
        done(&reply) && opened,
        format!("{reply:?} opened={opened} seen={seen} ms={ms}"),
    );

    // Escape leaves the menu, and once more leaves the menu bar.
    let _ = cx.input(Input::Key("Escape"));
    let _ = cx.input(Input::Key("Escape"));
    let mut closed = cx.wait_for(Duration::from_secs(2), || !popup_of(pid));
    if !closed {
        if let Some(h) = protocol::handle_of(handle) {
            // SAFETY: a message posted to a window Windows knows or not.
            unsafe { PostMessageW(h as usize as HWND, WM_CANCELMODE, 0, 0) };
        }
        closed = cx.wait_for(Duration::from_secs(2), || !popup_of(pid));
    }
    cx.log("findActMenuClosed", closed);
}

/// A command inside a menu: Find lists it after what is on screen, marked
/// as a menu command, and Enter runs it without the menu being opened by
/// hand. Select All shows in the selection.
fn menu_command(cx: &Cx, handle: &Value) {
    let (budget, max, depth) = (act::MENU_BUDGET_MS, act::MENU_MAX, act::MENU_DEPTH);
    let started = Instant::now();
    let menu = desktop::client().menu_items(Some(handle), budget, max as u64, depth as u64);
    let read = cx.block_on(menu);
    let took = started.elapsed().as_millis();
    let (commands, limit) = match &read {
        Ok(r) => act::menu_commands(r, handle, depth, max),
        Err(_) => (Vec::new(), None),
    };
    let source = read.as_ref().map(|r| r["source"].clone());
    let n = commands.len();
    cx.log(
        "findActMenuRead",
        format!("source={source:?} commands={n} limit={limit:?} ms={took}"),
    );
    if commands.is_empty() {
        let why = "skipped: this Notepad lists no menu commands until a menu is opened";
        cx.log("findActMenuCommand", why);
        return;
    }

    front(cx, handle);
    let _ = cx.input(Input::Key("Ctrl+Home"));
    let before = selected(cx, handle);
    let in_menu = |x: &Value| x["kind"] == "menu" && says(x, "match", COMMAND);
    let list = find(cx, handle, COMMAND, |r| position(r, in_menu).is_some());
    let Some(at) = position(&list, in_menu) else {
        let why = format!("the menu was read but the command is not listed: {list}");
        cx.must("findActMenuCommand", false, why);
        return;
    };
    // After what is on screen: no row that is on screen comes later.
    let all = list["rows"].as_array().map_or(0, Vec::len);
    let last_on_screen = (0..all).rev().find(|i| row_at(&list, *i)["kind"] != "menu");
    let ordered = last_on_screen < Some(at);
    let list = pick(cx, at);
    let row = current_row(&list);
    cx.log("findActMenuRow", &row);
    let started = Instant::now();
    let reply = act_from_page(cx, &list, None);
    let whole = |s: &str| s.contains("Invoice 2291") && s.contains("42.00");
    let ran = cx.wait_for(Duration::from_secs(5), || whole(&selected(cx, handle)));
    let ms = started.elapsed().as_millis();
    let got = selected(cx, handle);
    let ok = done(&reply) && ran && ordered && row["act"] == "Press";
    cx.must(
        "findActMenuCommand",
        ok,
        format!("{reply:?} ordered={ordered} selected={got:?} before={before:?} ms={ms}"),
    );
}

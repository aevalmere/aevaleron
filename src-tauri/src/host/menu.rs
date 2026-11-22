//! Right-click menus inside the windows (main.js contextMenu, SPEC-v2 4.5):
//! an answer (Copy, Do it) and a trail row (Copy step, Retry from here). The
//! pages ask for the edit menu on a text field and Copy / Select all on a
//! selection (main.js attachContextMenu, WIN-14).
//!
//! A plain Win32 popup menu, tracked on the UI thread with TPM_RETURNCMD:
//! the click comes back as the call's value, so these menus need no share of
//! the tray's menu events. The edit commands are the keys a text field
//! already answers (Ctrl+X, C, V, A), sent to the page that has the keyboard.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow,
    TrackPopupMenu, MF_SEPARATOR, MF_STRING, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_NULL,
};

use super::agent::{start_run, StartReq};
use crate::shell::{AppHandle, Manager};

const PREFIX: &str = "ctx:";

/// Command ids of the edit roles; the menu's own items are 1..=n.
const ROLE_BASE: usize = 100;

#[derive(Clone, Debug)]
enum Action {
    Copy(String),
    DoIt { task: String, context: String },
}

/// The actions of the menu shown last, by item id. Locked only to swap a
/// map in or take one out, never while the menu is up.
fn pending() -> &'static Mutex<HashMap<String, Action>> {
    static P: OnceLock<Mutex<HashMap<String, Action>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The items for one right-click, as main.js built them.
fn items_for(p: &Value) -> Vec<(&'static str, Action)> {
    let mut items = Vec::new();
    let text = text_of(&p["text"]);
    match p["kind"].as_str() {
        Some("answer") => {
            let question = text_of(&p["question"]);
            items.push(("Copy", Action::Copy(text.clone())));
            items.push((
                "Do it",
                Action::DoIt {
                    task: if question.is_empty() {
                        "Do what this answer describes".into()
                    } else {
                        question
                    },
                    context: format!("Earlier answer:\n{text}"),
                },
            ));
        }
        Some("row") => {
            items.push(("Copy step", Action::Copy(text.clone())));
            let task = text_of(&p["task"]);
            if crate::store::truthy(p.get("ended")) && !task.is_empty() {
                items.push((
                    "Retry from here",
                    Action::DoIt {
                        task,
                        context: format!(
                            "An earlier attempt got as far as: {text}. Continue from that step."
                        ),
                    },
                ));
            }
        }
        _ => {}
    }
    items
}

/// The labels one right-click would show (smoke `contextMenus`).
pub fn item_labels(p: &Value) -> Vec<&'static str> {
    items_for(p).into_iter().map(|(label, _)| label).collect()
}

/// main.js attachContextMenu, by Electron role (`-` is a separator): a
/// text field gets the edit commands, a selection Copy and Select all.
pub fn edit_template(kind: &str) -> &'static [&'static str] {
    match kind {
        "edit" => &["cut", "copy", "paste", "-", "selectAll"],
        "selection" => &["copy", "selectAll"],
        _ => &[],
    }
}

/// A role's menu text and the key that does it with Ctrl.
fn role_entry(role: &str) -> Option<(&'static str, u16)> {
    match role {
        "cut" => Some(("Cut", 0x58)),
        "copy" => Some(("Copy", 0x43)),
        "paste" => Some(("Paste", 0x56)),
        "selectAll" => Some(("Select all", 0x41)),
        _ => None,
    }
}

/// Makes one menu's items with `make`, then swaps their actions in as the
/// pending ones, so the lock is never held while items are made.
fn stage<T>(
    items: Vec<(&'static str, Action)>,
    mut make: impl FnMut(&str, &'static str) -> Result<T, String>,
) -> Vec<T> {
    let mut built = Vec::new();
    let mut actions = HashMap::new();
    for (i, (text, action)) in items.into_iter().enumerate() {
        let id = format!("{PREFIX}{i}");
        match make(&id, text) {
            Ok(item) => {
                actions.insert(id, action);
                built.push(item);
            }
            Err(e) => crate::crash::write("context-menu", &e),
        }
    }
    *pending().lock() = actions;
    built
}

fn run(action: Action) {
    let work = async move {
        match action {
            Action::Copy(t) => {
                crate::shell::sys::clipboard_write(&t);
            }
            Action::DoIt { task, context } => {
                let req = StartReq {
                    task,
                    context: Some(context),
                    ..StartReq::default()
                };
                start_run(&AppHandle::get(), req).await;
            }
        }
    };
    crate::shell::async_runtime::spawn(crate::crash::guard_async("context-menu.run", work));
}

/// One line of the popup: a command id and its text, or a separator.
enum Line {
    Item(usize, &'static str),
    Separator,
}

/// Shows `lines` at the pointer, owned by `hwnd`, and returns the chosen
/// command id (0 when the menu was dismissed). UI thread only.
fn track(hwnd: isize, lines: &[Line]) -> usize {
    let owner = hwnd as HWND;
    // SAFETY: a new menu, filled with NUL-terminated strings that outlive
    // each call, tracked against a live window of ours on its own thread,
    // and destroyed before returning.
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        for line in lines {
            match line {
                Line::Item(id, text) => {
                    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                    AppendMenuW(menu, MF_STRING, *id, wide.as_ptr());
                }
                Line::Separator => {
                    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
                }
            }
        }
        let mut at = POINT { x: 0, y: 0 };
        GetCursorPos(&mut at);
        // Without the window in front the menu does not close on a click
        // outside it (TrackPopupMenu's remarks).
        SetForegroundWindow(owner);
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON;
        let chosen = TrackPopupMenu(menu, flags, at.x, at.y, 0, owner, std::ptr::null());
        PostMessageW(owner, WM_NULL, 0, 0);
        DestroyMenu(menu);
        chosen.max(0) as usize
    }
}

/// What a chosen command id does: one of the menu's own items (by its
/// pending id) or an edit role's chord.
fn chosen(cmd: usize, roles: &[&str]) {
    if cmd == 0 {
        return;
    }
    if cmd >= ROLE_BASE {
        let key = roles.get(cmd - ROLE_BASE).copied().and_then(role_entry);
        if let Some((_, vk)) = key {
            if let Err(e) = crate::voice::insert::ctrl_chord(vk) {
                crate::crash::write("context-menu", &e);
            }
        }
        return;
    }
    let id = format!("{PREFIX}{}", cmd - 1);
    let action = std::mem::take(&mut *pending().lock()).remove(&id);
    if let Some(a) = action {
        run(a);
    }
}

/// `ui:context-menu`: pops the menu at the pointer in the calling window.
pub fn popup(app: &AppHandle, label: &str, p: &Value) -> Value {
    let done = json!({ "ok": true });
    let roles = edit_template(p["kind"].as_str().unwrap_or(""));
    let items = items_for(p);
    if roles.is_empty() && items.is_empty() {
        return done;
    }
    let Some(win) = app.get_webview_window(label) else {
        return done;
    };
    let lines: Vec<Line> = if roles.is_empty() {
        stage(items, |id, text| {
            let n: usize = id[PREFIX.len()..].parse().map_err(|_| id.to_string())?;
            Ok(Line::Item(n + 1, text))
        })
    } else {
        roles
            .iter()
            .enumerate()
            .map(|(i, r)| match role_entry(r) {
                Some((text, _)) => Line::Item(ROLE_BASE + i, text),
                None => Line::Separator,
            })
            .collect()
    };
    let hwnd = win.hwnd();
    // Queued, so the menu never opens inside the call that asked for it.
    let _ = crate::shell::platform::post(Box::new(move || {
        crate::crash::guard("context-menu.popup", || chosen(track(hwnd, &lines), roles));
    }));
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_match_main_js() {
        let answer =
            items_for(&json!({ "kind": "answer", "text": "line one\nline two", "question": "q" }));
        let labels: Vec<&str> = answer.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, ["Copy", "Do it"]);
        match &answer[1].1 {
            Action::DoIt { task, .. } => assert_eq!(task, "q"),
            other => panic!("{other:?}"),
        }
        let row = items_for(&json!({ "kind": "row", "text": "Typed", "ended": true, "task": "t" }));
        assert_eq!(row.len(), 2);
        let open =
            items_for(&json!({ "kind": "row", "text": "Typed", "ended": false, "task": "t" }));
        assert_eq!(open.len(), 1);
        assert!(items_for(&json!({ "kind": "other" })).is_empty());
    }

    #[test]
    fn edit_and_selection_templates_match_main_js() {
        assert_eq!(
            edit_template("edit"),
            ["cut", "copy", "paste", "-", "selectAll"]
        );
        assert_eq!(edit_template("selection"), ["copy", "selectAll"]);
        assert!(edit_template("answer").is_empty());
        assert!(edit_template("").is_empty());
        for role in edit_template("edit") {
            assert_eq!(role_entry(role).is_none(), *role == "-", "{role}");
        }
    }

    #[test]
    fn pending_lock_not_held_while_building() {
        let row = json!({ "kind": "row", "text": "t", "ended": true, "task": "x" });
        let built = stage(items_for(&row), |id, text| {
            let free = pending().try_lock().is_some();
            assert!(free, "pending held while making {id}");
            Ok::<_, String>(format!("{id} {text}"))
        });
        let want = ["ctx:0 Copy step", "ctx:1 Retry from here"];
        assert_eq!(built, want);
        let action = std::mem::take(&mut *pending().lock()).remove("ctx:0");
        assert!(matches!(action, Some(Action::Copy(t)) if t == "t"));
        assert!(pending().lock().is_empty());
    }
}

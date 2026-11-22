//! The polish lane's labels (`tools/smoke-labels/polish.txt`): a menu in
//! the bar is never cut by the bar's window. Rust sizes the window to the
//! bar's menus, so only the real window can show whether the cap
//! (`INPUT_MAX_HEIGHT`, scaled with the text) and the resize during the
//! opening glide leave room for the whole menu.
//!
//! - `barMenuInside`: the token's menu, opened (Alt+Down in the field, as a
//!   person opens it) the moment the bar starts to open, while it glides,
//!   and again under a four-line question, ends inside the window both
//!   times. The bar page's `menu` probe answers `{ open, inside }`, or a
//!   bare `true` for a menu that is open in a window of its own.

use std::time::Duration;

use serde_json::Value;

use crate::selftest::{Cx, Input};
use crate::windows::{self, OVERLAY};

const WAIT: Duration = Duration::from_secs(5);

pub fn run(cx: &mut Cx) {
    bar_menu_inside(cx);
}

fn page(cx: &Cx, key: &str) -> Value {
    cx.probe(OVERLAY, key).unwrap_or(Value::Null)
}

fn mode(cx: &Cx) -> String {
    page(cx, "mode").as_str().unwrap_or_default().to_string()
}

/// (open, inside) from the bar's `menu` probe.
fn menu(cx: &Cx) -> (bool, bool) {
    match page(cx, "menu") {
        Value::Bool(open) => (open, open),
        Value::Object(m) => {
            let open = m.get("open") == Some(&Value::Bool(true));
            (open, open && m.get("inside") == Some(&Value::Bool(true)))
        }
        _ => (false, false),
    }
}

/// "ok", or what the menu probe said.
fn inside_after_resize(cx: &Cx) -> String {
    // The glide (260 ms), the deferred resize and the menu's drop-in.
    let ok = cx.wait_for(Duration::from_millis(1500), || menu(cx) == (true, true));
    if ok {
        "ok".into()
    } else {
        page(cx, "menu").to_string()
    }
}

/// The field focused, then Alt+Down: the token's menu.
fn open_menu(cx: &Cx) -> bool {
    let focused = || page(cx, "focused-input") == Value::Bool(true);
    let _ = cx.wait_for(Duration::from_secs(2), focused);
    let _ = cx.input(Input::Key("Alt+Down"));
    cx.wait_for(Duration::from_secs(1), || menu(cx).0)
}

fn close_menu(cx: &Cx) {
    if menu(cx).0 {
        let _ = cx.input(Input::Key("Escape"));
        let _ = cx.wait_for(Duration::from_secs(1), || !menu(cx).0);
    }
}

/// Empties the field with real keys, then types `value`.
fn type_text(cx: &Cx, value: &str) {
    let _ = cx.input(Input::Key("Ctrl+A"));
    let _ = cx.input(Input::Key("Delete"));
    if !value.is_empty() {
        let _ = cx.input(Input::Text(value));
    }
}

fn bar_menu_inside(cx: &Cx) {
    cx.on_main(windows::collapse_panel);
    let _ = cx.wait_for(WAIT, || mode(cx) == "pill");
    cx.sleep(400);

    // Opened while the bar is still gliding open, by the hotkey so the bar
    // has the keyboard.
    let hotkey = crate::store::get().shortcut("captureQuery");
    if cx.input(Input::Key(&hotkey)).is_err() {
        cx.on_main(|app| windows::open_bar(app, None, None));
    }
    let _ = cx.wait_for(WAIT, || mode(cx) == "panel");
    let opened = open_menu(cx);
    let gliding = inside_after_resize(cx);
    close_menu(cx);

    // Under a question of four lines: Shift+Enter breaks a line.
    type_text(cx, "one");
    for word in ["two", "three", "four"] {
        let _ = cx.input(Input::Key("Shift+Enter"));
        let _ = cx.input(Input::Text(word));
    }
    cx.sleep(300);
    open_menu(cx);
    let tall = inside_after_resize(cx);
    close_menu(cx);
    type_text(cx, "");

    let ok = opened && gliding == "ok" && tall == "ok";
    cx.must(
        "barMenuInside",
        ok,
        format!("opened={opened} gliding={gliding} tall={tall}"),
    );
    cx.on_main(windows::collapse_panel);
    let _ = cx.wait_for(WAIT, || mode(cx) == "pill");
}

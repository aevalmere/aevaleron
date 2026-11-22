//! Find's menu commands (host.cs "menus"): a window's menu read without
//! opening anything, and one command run the way its menu would run it.

use std::time::Instant;

use serde_json::{json, Map, Value};
use windows::Win32::UI::Accessibility::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMenu, GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW, GetSubMenu, IsMenu,
    IsWindowVisible, PostMessageW, SendMessageTimeoutW, HMENU, MF_BYPOSITION, MF_CHECKED,
    MF_DISABLED, MF_GRAYED, MF_SEPARATOR, SMTO_ABORTIFHUNG, WM_COMMAND, WM_INITMENU,
    WM_INITMENUPOPUP,
};

use super::input::{pattern, run_bounded};
use super::uia::{self, El};
use super::wins::{
    enum_windows, hw, is_excluded_window, pid_of, refuse_elevated, refuse_secure_desktop,
    target_window,
};
use super::{arg, check_cancel, err, handle_str, int_arg, sleep_ms, str_arg, to_int, to_text};
use super::{DResult, DesktopError};

fn menu_text(menu: HMENU, pos: i32) -> String {
    let mut buf = [0u16; 260];
    let n = unsafe {
        GetMenuStringW(
            menu,
            pos as u32,
            buf.as_mut_ptr(),
            buf.len() as i32,
            MF_BYPOSITION,
        )
    };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        String::new()
    }
}

struct Bounds {
    depth: usize,
    max: usize,
    budget: u128,
    t0: Instant,
    limit: Option<&'static str>,
}

impl Bounds {
    /// Whether the read stops here, setting `limit` to why.
    fn stop(&mut self, items: usize) -> bool {
        if matches!(self.limit, Some("time") | Some("count")) {
            return true;
        }
        if self.t0.elapsed().as_millis() > self.budget {
            self.limit = Some("time");
            return true;
        }
        if items >= self.max {
            self.limit = Some("count");
            return true;
        }
        false
    }
}

/// One level of a classic menu and, within the bounds, the levels under
/// it. A separator or a picture-only item has no name and is left out.
fn walk_menu(menu: HMENU, path: &[String], pos: &[i32], b: &mut Bounds, items: &mut Vec<Value>) {
    let count = unsafe { GetMenuItemCount(menu) }.min(200);
    for k in 0..count {
        if b.stop(items.len()) {
            return;
        }
        let sub = unsafe { GetSubMenu(menu, k) };
        let mut state = unsafe { GetMenuState(menu, k as u32, MF_BYPOSITION) };
        if state == u32::MAX {
            continue;
        }
        // With a submenu the high byte counts its items; the flags are the
        // low byte alone.
        if !sub.is_null() {
            state &= 0xFF;
        }
        if state & MF_SEPARATOR != 0 {
            continue;
        }
        let name = menu_text(menu, k);
        if name.trim().is_empty() {
            continue;
        }
        let mut p = path.to_vec();
        p.push(name);
        let mut at = pos.to_vec();
        at.push(k);
        let mut d = Map::new();
        d.insert("path".into(), json!(p));
        d.insert("pos".into(), json!(at));
        d.insert("sub".into(), json!(!sub.is_null()));
        d.insert(
            "enabled".into(),
            json!(state & (MF_GRAYED | MF_DISABLED) == 0),
        );
        if state & MF_CHECKED != 0 {
            d.insert("checked".into(), json!(true));
        }
        items.push(Value::Object(d));
        if sub.is_null() {
            continue;
        }
        if p.len() >= b.depth {
            if b.limit.is_none() {
                b.limit = Some("depth");
            }
            continue;
        }
        walk_menu(sub, &p, &at, b, items);
    }
}

/// The window's menu bars, looked for a few levels down and never inside
/// the title bar (whose only menu is the system menu).
fn menu_bars(h: isize, t0: Instant, budget: u128) -> Vec<El> {
    let mut bars = Vec::new();
    let Ok(top) = uia::element_from_handle(h) else {
        return bars;
    };
    let mut level = vec![top];
    let mut opened = 0;
    for _ in 0..4 {
        if level.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for el in &level {
            if opened >= 60 || t0.elapsed().as_millis() > budget {
                return bars;
            }
            opened += 1;
            let Ok(kids) = uia::menu_kids(&el.0, false) else {
                continue;
            };
            for k in kids {
                let ct = uia::menu_info(&k.0).control_type;
                if ct == uia::MENU_BAR {
                    bars.push(k);
                } else if matches!(
                    ct,
                    uia::PANE | uia::GROUP | uia::CUSTOM | uia::TOOLBAR | uia::WINDOW
                ) {
                    next.push(k);
                }
            }
        }
        if !bars.is_empty() {
            return bars;
        }
        level = next;
    }
    bars
}

fn walk_uia_menu(el: &El, path: &[String], b: &mut Bounds, items: &mut Vec<Value>) {
    let Ok(kids) = uia::menu_kids(&el.0, false) else {
        return;
    };
    for k in kids {
        if b.stop(items.len()) {
            return;
        }
        let m = uia::menu_info(&k.0);
        // A submenu is often a Menu element between an item and its
        // children.
        if m.control_type == uia::MENU {
            walk_uia_menu(&k, path, b, items);
            continue;
        }
        if m.control_type != uia::MENU_ITEM || m.name.trim().is_empty() {
            continue;
        }
        let mut p = path.to_vec();
        p.push(m.name.clone());
        let at = items.len();
        items.push(json!({ "path": p, "enabled": m.enabled }));
        if p.len() < b.depth {
            walk_uia_menu(&k, &p, b, items);
        } else if m.sub && b.limit.is_none() {
            b.limit = Some("depth");
        }
        items[at]["sub"] = json!(m.sub || items.len() > at + 1);
    }
}

/// Find's menu commands: the names in a window's menu, read without
/// opening anything. A classic menu is read from the window's own menu
/// handle, every level of it. Any other menu bar is read through UI
/// Automation, which holds only what the application has built so far.
/// Bounded by depth, count and time; `limit` says which bound cut it short.
pub(crate) fn menu_items(a: &Value) -> DResult<Value> {
    let h = target_window(a)?;
    let mut b = Bounds {
        depth: int_arg(a, "depth", 4)?.clamp(1, 8) as usize,
        max: int_arg(a, "max", 400)?.clamp(1, 2000) as usize,
        budget: int_arg(a, "budgetMs", 300)?.clamp(50, 2000) as u128,
        t0: Instant::now(),
        limit: None,
    };
    let mut items = Vec::new();
    let mut source: Option<&str> = None;
    let menu = unsafe { GetMenu(hw(h)) };
    if !menu.is_null() && unsafe { IsMenu(menu) } != 0 {
        walk_menu(menu, &[], &[], &mut b, &mut items);
        if !items.is_empty() {
            source = Some("win32");
        }
    }
    if items.is_empty() {
        let bars = menu_bars(h, b.t0, b.budget);
        for bar in &bars {
            walk_uia_menu(bar, &[], &mut b, &mut items);
        }
        if !bars.is_empty() {
            source = Some("uia");
        }
    }
    Ok(json!({
        "windowHandle": handle_str(h),
        "source": source,
        "items": items,
        "truncated": b.limit.is_some(),
        "limit": b.limit,
        "readMs": b.t0.elapsed().as_millis() as u64,
    }))
}

fn gone() -> DesktopError {
    err("NOT_FOUND", "that menu command is no longer there")
}

fn send_timeout(h: isize, msg: u32, w: usize, l: isize) {
    let mut ignored = 0usize;
    unsafe {
        SendMessageTimeoutW(hw(h), msg, w, l, SMTO_ABORTIFHUNG, 500, &mut ignored);
    }
}

/// A classic menu command, run the way its menu would run it: the
/// application is asked to bring each menu on the way up to date
/// (WM_INITMENUPOPUP, as opening it would), the item is checked to be the
/// one that was listed and to be available, and its command is posted. No
/// menu is drawn.
fn win32_menu_invoke(h: isize, path: &[String], pos: &[Value]) -> DResult<Value> {
    let menu = unsafe { GetMenu(hw(h)) };
    if menu.is_null() || unsafe { IsMenu(menu) } == 0 {
        return Err(err("NOT_FOUND", "that window no longer has a menu"));
    }
    if pos.is_empty() || pos.len() != path.len() {
        return Err(err(
            "BAD_ARGS",
            "menuInvoke needs pos and path of the same length",
        ));
    }
    let pos: Vec<i32> = pos.iter().map(to_int).collect::<DResult<_>>()?;
    let mut cur = menu;
    let steps = pos.iter().zip(path).take(pos.len() - 1);
    for (k, (&at, name)) in steps.enumerate() {
        let sub = unsafe { GetSubMenu(cur, at) };
        if sub.is_null() || menu_text(cur, at) != *name {
            return Err(gone());
        }
        check_cancel()?;
        if k == 0 {
            send_timeout(h, WM_INITMENU, menu as usize, 0);
        }
        send_timeout(h, WM_INITMENUPOPUP, sub as usize, (at & 0xFFFF) as isize);
        cur = sub;
    }
    let last = pos[pos.len() - 1];
    let count = unsafe { GetMenuItemCount(cur) };
    if last < 0 || last >= count || menu_text(cur, last) != path[path.len() - 1] {
        return Err(gone());
    }
    if !unsafe { GetSubMenu(cur, last) }.is_null() {
        return Err(err(
            "NO_PATTERN",
            "that menu item opens a submenu; it is not a command",
        ));
    }
    let state = unsafe { GetMenuState(cur, last as u32, MF_BYPOSITION) };
    if state == u32::MAX {
        return Err(gone());
    }
    if state & (MF_GRAYED | MF_DISABLED) != 0 {
        return Err(err(
            "DISABLED",
            "that menu command is not available right now",
        ));
    }
    let id = unsafe { GetMenuItemID(cur, last) };
    if id == u32::MAX || id == 0 {
        return Err(err("NO_PATTERN", "that menu item has no command"));
    }
    check_cancel()?;
    let sent = unsafe { PostMessageW(hw(h), WM_COMMAND, (id & 0xFFFF) as usize, 0) } != 0;
    Ok(json!({ "ok": sent, "via": "command", "id": (id & 0xFFFF) as i32 }))
}

/// A menu that has just opened: under its item, or in a popup window of
/// the same application.
fn opened_menu_child(parent: &El, h: isize, name: &str) -> Option<El> {
    if let Some(c) = uia::menu_child(&parent.0, name, true) {
        return Some(c);
    }
    let pid = pid_of(h);
    let mut popups = Vec::new();
    enum_windows(|w| {
        if w != h && unsafe { IsWindowVisible(hw(w)) } != 0 && pid_of(w) == pid && popups.len() < 12
        {
            popups.push(w);
        }
        true
    });
    for w in popups {
        if is_excluded_window(w) {
            continue;
        }
        let Ok(el) = uia::element_from_handle(w) else {
            continue;
        };
        if let Some(c) = uia::menu_child(&el.0, name, true) {
            return Some(c);
        }
    }
    None
}

fn expand(el: &El) -> DResult<()> {
    let p: IUIAutomationExpandCollapsePattern = pattern(&el.0, UIA_ExpandCollapsePatternId)
        .ok_or_else(|| err("NO_PATTERN", "that menu cannot be opened"))?;
    unsafe {
        if p.CurrentExpandCollapseState()? != ExpandCollapseState_Expanded {
            p.Expand()?;
        }
    }
    Ok(())
}

fn has<T: windows::core::Interface>(el: &El, id: UIA_PATTERN_ID) -> bool {
    pattern::<T>(&el.0, id).is_some()
}

/// Opens one menu on the path: expanded if it can be, else invoked.
fn open_step(open: &El, name: &str) -> DResult<()> {
    let el = open.clone();
    if has::<IUIAutomationExpandCollapsePattern>(open, UIA_ExpandCollapsePatternId) {
        run_bounded(2000, move || expand(&el))?;
    } else if has::<IUIAutomationInvokePattern>(open, UIA_InvokePatternId) {
        run_bounded(2000, move || {
            // The whole El moves in, not its non-Send field.
            let el = el;
            let p: IUIAutomationInvokePattern = pattern(&el.0, UIA_InvokePatternId)
                .ok_or_else(|| err("NO_PATTERN", "invoke went away"))?;
            unsafe { p.Invoke()? };
            Ok(())
        })?;
    } else {
        return Err(err("NO_PATTERN", format!("\"{name}\" cannot be opened")));
    }
    Ok(())
}

fn run_leaf(leaf: &El) -> DResult<Value> {
    let enabled = unsafe { leaf.0.CurrentIsEnabled() }
        .map(|b| b.as_bool())
        .unwrap_or(true);
    if !enabled {
        return Err(err(
            "DISABLED",
            "that menu command is not available right now",
        ));
    }
    let el = leaf.clone();
    let (via, done) = if has::<IUIAutomationInvokePattern>(leaf, UIA_InvokePatternId) {
        let done = run_bounded(2000, move || {
            // The whole El moves in, not its non-Send field.
            let el = el;
            let p: IUIAutomationInvokePattern = pattern(&el.0, UIA_InvokePatternId)
                .ok_or_else(|| err("NO_PATTERN", "invoke went away"))?;
            unsafe { p.Invoke()? };
            Ok(())
        })?;
        ("invoke", done)
    } else if has::<IUIAutomationTogglePattern>(leaf, UIA_TogglePatternId) {
        let done = run_bounded(2000, move || {
            // The whole El moves in, not its non-Send field.
            let el = el;
            let p: IUIAutomationTogglePattern = pattern(&el.0, UIA_TogglePatternId)
                .ok_or_else(|| err("NO_PATTERN", "toggle went away"))?;
            unsafe { p.Toggle()? };
            Ok(())
        })?;
        ("toggle", done)
    } else if has::<IUIAutomationSelectionItemPattern>(leaf, UIA_SelectionItemPatternId) {
        let done = run_bounded(2000, move || {
            // The whole El moves in, not its non-Send field.
            let el = el;
            let p: IUIAutomationSelectionItemPattern =
                pattern(&el.0, UIA_SelectionItemPatternId)
                    .ok_or_else(|| err("NO_PATTERN", "select went away"))?;
            unsafe { p.Select()? };
            Ok(())
        })?;
        ("select", done)
    } else if has::<IUIAutomationExpandCollapsePattern>(leaf, UIA_ExpandCollapsePatternId) {
        let done = run_bounded(2000, move || {
            // The whole El moves in, not its non-Send field.
            let el = el;
            let p: IUIAutomationExpandCollapsePattern = pattern(&el.0, UIA_ExpandCollapsePatternId)
                .ok_or_else(|| err("NO_PATTERN", "expand went away"))?;
            unsafe { p.Expand()? };
            Ok(())
        })?;
        ("expand", done)
    } else {
        return Err(err(
            "NO_PATTERN",
            "that menu command cannot be run through UI Automation",
        ));
    };
    Ok(json!({ "ok": true, "via": via, "pending": done.is_none() }))
}

/// Any other menu command: each parent is opened in turn through UI
/// Automation, then the command itself is invoked. A step that fails
/// closes what was opened.
fn uia_menu_invoke(h: isize, path: &[String]) -> DResult<Value> {
    if path.is_empty() {
        return Err(err("BAD_ARGS", "menuInvoke needs a path"));
    }
    let t0 = Instant::now();
    let first = menu_bars(h, t0, 1500)
        .iter()
        .find_map(|bar| uia::menu_child(&bar.0, &path[0], false))
        .ok_or_else(|| err("NOT_FOUND", "that menu is no longer there"))?;
    let run = || -> DResult<Value> {
        let mut cur = first.clone();
        for pair in path.windows(2) {
            open_step(&cur, &pair[0])?;
            let mut next = None;
            for _ in 0..16 {
                check_cancel()?;
                sleep_ms(50);
                next = opened_menu_child(&cur, h, &pair[1]);
                if next.is_some() {
                    break;
                }
            }
            cur = next.ok_or_else(|| {
                err(
                    "NOT_FOUND",
                    format!("\"{}\" is not in that menu any more", pair[1]),
                )
            })?;
        }
        run_leaf(&cur)
    };
    let out = run();
    if out.is_err() && path.len() > 1 {
        let el = first.clone();
        let _ = run_bounded(1000, move || {
            // The whole El moves in, not its non-Send field.
            let el = el;
            let p: IUIAutomationExpandCollapsePattern = pattern(&el.0, UIA_ExpandCollapsePatternId)
                .ok_or_else(|| err("NO_PATTERN", "no collapse"))?;
            unsafe { p.Collapse()? };
            Ok(())
        });
    }
    out
}

pub(crate) fn menu_invoke(a: &Value) -> DResult<Value> {
    let h = target_window(a)?;
    refuse_elevated(h)?;
    refuse_secure_desktop()?;
    let path: Vec<String> = arg(a, "path")
        .and_then(Value::as_array)
        .map(|v| v.iter().map(to_text).collect())
        .unwrap_or_default();
    if str_arg(a, "source", "win32") == "uia" {
        return uia_menu_invoke(h, &path);
    }
    let pos = arg(a, "pos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    win32_menu_invoke(h, &path, &pos)
}

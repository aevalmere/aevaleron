//! SendInput and the pattern-first actions (host.cs "input"): click, hover,
//! drag, type, key, scroll, scrollTo, invoke, setValue, restoreCursor.

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;
use serde_json::{json, Value};
use windows::core::{Interface, BSTR};
use windows::Win32::UI::Accessibility::*;
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    MapVirtualKeyW, SendInput, VkKeyScanW, INPUT, INPUT_0, KEYBDINPUT, MOUSEINPUT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetSystemMetrics, SetCursorPos};

use super::uia::{self, El, Info, RefEntry};
use super::wins::{
    focus_window, foreground_root, is_excluded_window, is_window, pid_of, refuse_elevated,
    refuse_excluded, refuse_own_point, refuse_secure_desktop, root, title, window_at,
};
use super::{
    arg, bool_arg, check_cancel, current_req, err, handle_str, int_arg, parse_handle, parse_ref,
    require_int, sleep_ms, spawn_op, str_arg, to_text, u16len, DResult,
};

const INPUT_MOUSE: u32 = 0;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_EXTENDEDKEY: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 2;
const KEYEVENTF_UNICODE: u32 = 4;
pub(crate) const MOUSEEVENTF_MOVE: u32 = 1;
const MOUSEEVENTF_WHEEL: u32 = 0x0800;
const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
/// Marks input as ours ("AEVB"), next to the injected flag Windows sets.
const EXTRA: usize = 0x4145_5642;

// ------------------------------------------------------------ primitives

fn is_release(i: &INPUT) -> bool {
    unsafe {
        if i.r#type == INPUT_KEYBOARD {
            return i.Anonymous.ki.dwFlags & KEYEVENTF_KEYUP != 0;
        }
        if i.r#type != INPUT_MOUSE {
            return false;
        }
        let ups = 0x4 | 0x10 | 0x40 | 0x100;
        let presses = 0x2 | 0x8 | 0x20 | 0x80 | MOUSEEVENTF_WHEEL | MOUSEEVENTF_HWHEEL;
        let f = i.Anonymous.mi.dwFlags;
        f & ups != 0 && f & presses == 0
    }
}

/// A cancelled request sends nothing more, except releases: a key or button
/// it already pressed is always let go.
pub(crate) fn send_inputs(inputs: &[INPUT]) -> DResult<u32> {
    let releases = !inputs.is_empty() && inputs.iter().all(is_release);
    if !releases {
        check_cancel()?;
    }
    let size = std::mem::size_of::<INPUT>() as i32;
    Ok(unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), size) })
}

fn mouse_input(dx: i32, dy: i32, flags: u32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as _,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: EXTRA,
            },
        },
    }
}

pub(crate) fn send_mouse_rel(dx: i32, dy: i32, flags: u32, data: i32) -> DResult<()> {
    send_inputs(&[mouse_input(dx, dy, flags, data)]).map(|_| ())
}

/// MOUSEEVENTF_ABSOLUTE | VIRTUALDESK takes 0..65535 across the whole
/// virtual screen. Rounding can land a pixel off on wide desktops, so the
/// position is checked and corrected.
fn send_mouse_abs(x: i32, y: i32) -> DResult<()> {
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(76),
            GetSystemMetrics(77),
            GetSystemMetrics(78).max(2),
            GetSystemMetrics(79).max(2),
        )
    };
    let nx = (f64::from(x - vx) * 65535.0 / f64::from(vw - 1)).round() as i32;
    let ny = (f64::from(y - vy) * 65535.0 / f64::from(vh - 1)).round() as i32;
    let flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    send_inputs(&[mouse_input(nx, ny, flags, 0)]).map(|_| ())
}

fn cursor() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

pub(crate) fn move_to(x: i32, y: i32) -> DResult<()> {
    check_cancel()?;
    save_cursor();
    send_mouse_abs(x, y)?;
    if let Some(p) = cursor() {
        if p != (x, y) {
            check_cancel()?;
            unsafe {
                SetCursorPos(x, y);
            }
        }
    }
    Ok(())
}

static SAVED_CURSOR: Mutex<Option<(i32, i32)>> = Mutex::new(None);

fn save_cursor() {
    let mut g = SAVED_CURSOR.lock();
    if g.is_none() {
        *g = cursor();
    }
}

pub(crate) fn forget_cursor() {
    *SAVED_CURSOR.lock() = None;
}

pub(crate) fn restore_cursor() -> Value {
    let p = SAVED_CURSOR.lock().take();
    match p {
        Some((x, y)) => {
            unsafe {
                SetCursorPos(x, y);
            }
            json!({ "ok": true, "point": [x, y] })
        }
        None => json!({ "ok": false }),
    }
}

const EXTENDED_KEYS: [u16; 16] = [
    0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2D, 0x2E, 0x5B, 0x5C, 0x5D, 0x6F, 0x90, 0x2C,
];

pub(crate) fn key_input(vk: u16, up: bool) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(u32::from(vk), 0) } as u16;
    let mut flags = if up { KEYEVENTF_KEYUP } else { 0 };
    if EXTENDED_KEYS.contains(&vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: EXTRA,
            },
        },
    }
}

fn unicode_input(c: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: c,
                dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: EXTRA,
            },
        },
    }
}

fn key_names() -> &'static HashMap<String, u16> {
    static NAMES: OnceLock<HashMap<String, u16>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut m: HashMap<String, u16> = [
            ("ctrl", 0x11),
            ("control", 0x11),
            ("shift", 0x10),
            ("alt", 0x12),
            ("option", 0x12),
            ("win", 0x5B),
            ("meta", 0x5B),
            ("super", 0x5B),
            ("cmd", 0x5B),
            ("windows", 0x5B),
            ("enter", 0x0D),
            ("return", 0x0D),
            ("tab", 0x09),
            ("esc", 0x1B),
            ("escape", 0x1B),
            ("space", 0x20),
            ("backspace", 0x08),
            ("back", 0x08),
            ("delete", 0x2E),
            ("del", 0x2E),
            ("insert", 0x2D),
            ("ins", 0x2D),
            ("home", 0x24),
            ("end", 0x23),
            ("pageup", 0x21),
            ("pgup", 0x21),
            ("page_up", 0x21),
            ("prior", 0x21),
            ("pagedown", 0x22),
            ("pgdn", 0x22),
            ("page_down", 0x22),
            ("next", 0x22),
            ("left", 0x25),
            ("up", 0x26),
            ("right", 0x27),
            ("down", 0x28),
            ("arrowleft", 0x25),
            ("arrowup", 0x26),
            ("arrowright", 0x27),
            ("arrowdown", 0x28),
            ("capslock", 0x14),
            ("printscreen", 0x2C),
            ("print", 0x2C),
            ("menu", 0x5D),
            ("apps", 0x5D),
            ("numlock", 0x90),
            ("scrolllock", 0x91),
            ("pause", 0x13),
            ("plus", 0xBB),
            ("equal", 0xBB),
            ("minus", 0xBD),
            ("comma", 0xBC),
            ("period", 0xBE),
            ("add", 0x6B),
            ("subtract", 0x6D),
            ("multiply", 0x6A),
            ("divide", 0x6F),
            ("decimal", 0x6E),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        for i in 1..=24u16 {
            m.insert(format!("f{i}"), 0x70 + i - 1);
        }
        for i in 0..=9u16 {
            m.insert(format!("numpad{i}"), 0x60 + i);
        }
        m
    })
}

const MODIFIERS: [u16; 4] = [0x11, 0x10, 0x12, 0x5B];

/// "ctrl+shift+s" -> [ctrl, shift, s]. A single character that is not a
/// letter or digit goes through VkKeyScan, which also says whether the
/// current layout needs shift for it.
pub(crate) fn parse_chord(chord: &str) -> DResult<Vec<u16>> {
    let mut keys: Vec<u16> = Vec::new();
    let parts: Vec<&str> = chord.split('+').collect();
    for (n, part) in parts.iter().enumerate() {
        let mut p = *part;
        if p.is_empty() && n == parts.len() - 1 && n > 0 {
            p = "+";
        }
        if p.is_empty() {
            continue;
        }
        if let Some(&vk) = key_names().get(&p.to_lowercase()) {
            keys.push(vk);
            continue;
        }
        let units: Vec<u16> = p.encode_utf16().collect();
        if units.len() == 1 {
            let c = units[0];
            if (u16::from(b'a')..=u16::from(b'z')).contains(&c) {
                keys.push(c - u16::from(b'a') + 0x41);
                continue;
            }
            if (u16::from(b'A')..=u16::from(b'Z')).contains(&c) {
                keys.push(c - u16::from(b'A') + 0x41);
                continue;
            }
            if (u16::from(b'0')..=u16::from(b'9')).contains(&c) {
                keys.push(c);
                continue;
            }
            let scan = unsafe { VkKeyScanW(c) };
            if scan != -1 {
                if scan & 0x100 != 0 && !keys.contains(&0x10) {
                    keys.insert(0, 0x10);
                }
                keys.push((scan & 0xFF) as u16);
                continue;
            }
        }
        return Err(err("BAD_ARGS", format!("unknown key '{p}' in '{chord}'")));
    }
    if keys.is_empty() {
        return Err(err("BAD_ARGS", "empty key chord"));
    }
    Ok(keys)
}

pub(crate) fn send_chord(keys: &[u16]) -> DResult<()> {
    let mut list: Vec<INPUT> = keys.iter().map(|&k| key_input(k, false)).collect();
    list.extend(keys.iter().rev().map(|&k| key_input(k, true)));
    send_inputs(&list).map(|_| ())
}

fn hold_modifiers(mods: Option<&[u16]>, up: bool) -> DResult<()> {
    let Some(mods) = mods.filter(|m| !m.is_empty()) else {
        return Ok(());
    };
    let list: Vec<INPUT> = if up {
        mods.iter().rev().map(|&k| key_input(k, true)).collect()
    } else {
        mods.iter().map(|&k| key_input(k, false)).collect()
    };
    send_inputs(&list).map(|_| ())
}

fn parse_mods(a: &Value) -> DResult<Option<Vec<u16>>> {
    let m = str_arg(a, "modifiers", "");
    if m.is_empty() {
        return Ok(None);
    }
    let keys = parse_chord(&m)?;
    if keys.iter().any(|k| !MODIFIERS.contains(k)) {
        return Err(err(
            "BAD_ARGS",
            "modifiers may only hold ctrl, shift, alt, win",
        ));
    }
    Ok(Some(keys))
}

// ------------------------------------------------------------ guards

/// Keyboard input goes wherever focus is, so it is only sent while the
/// intended window is in front. With a window that window is brought forward
/// first; without one, the foreground window is the target unless it is
/// Aevaleron.
fn keyboard_target(target: isize) -> DResult<isize> {
    refuse_secure_desktop()?;
    if target != 0 {
        if !is_window(target) {
            return Err(err("NOT_FOUND", "that window no longer exists"));
        }
        let target = root(target);
        refuse_excluded(target)?;
        refuse_elevated(target)?;
        if foreground_root() != target {
            if focus_window(target)?.is_none() {
                return Err(err(
                    "NOT_FOREGROUND",
                    format!("\"{}\" could not be brought to the front", title(target)),
                ));
            }
            sleep_ms(60);
        }
        return Ok(target);
    }
    let fg = foreground_root();
    if fg == 0 {
        return Err(err("NOT_FOUND", "no foreground window"));
    }
    if is_excluded_window(fg) {
        return Err(err(
            "BLOCKED_OWN_WINDOW",
            "the foreground window is Aevaleron's own; focus the target first",
        ));
    }
    refuse_elevated(fg)?;
    Ok(fg)
}

fn guard_still(target: isize, what: &str, done: usize) -> DResult<()> {
    if foreground_root() != target {
        return Err(err(
            "FOCUS_LOST",
            format!(
                "the target window lost focus after {done} {what}; stopped so nothing lands in the wrong app"
            ),
        ));
    }
    Ok(())
}

/// What is actually under (x, y). With an expected window, anything else on
/// top (a toast, another app) is reported instead of clicked.
fn check_point(x: i32, y: i32, expect: Option<&RefEntry>, force: bool) -> DResult<isize> {
    refuse_own_point(x, y)?;
    refuse_secure_desktop()?;
    let hit_top = root(window_at(x, y));
    if let Some(e) = expect {
        let hp = pid_of(hit_top);
        if !force && hit_top != e.top && hp != e.pid as u32 && hp != pid_of(e.top) {
            return Err(err(
                "OCCLUDED",
                format!(
                    "another window (\"{}\") covers that element; pass force to click anyway",
                    title(hit_top)
                ),
            ));
        }
    }
    refuse_elevated(hit_top)?;
    Ok(hit_top)
}

/// A target is {ref} or {x, y}.
fn target_point(a: &Value) -> DResult<([i32; 2], Option<RefEntry>, Option<Info>)> {
    if arg(a, "ref").is_some() {
        let e = uia::get_ref(arg(a, "ref"))?;
        refuse_elevated(e.top)?;
        let (p, info) = uia::point_for(&e)?;
        return Ok((p, Some(e), Some(info)));
    }
    Ok(([require_int(a, "x")?, require_int(a, "y")?], None, None))
}

/// Invoke on a Win32 button that opens a modal dialog does not return until
/// the dialog closes. The call keeps running on its own thread and the
/// reply says it is still pending instead of timing out.
pub(crate) fn run_bounded<T: Send + 'static>(
    ms: u64,
    act: impl FnOnce() -> DResult<T> + Send + 'static,
) -> DResult<Option<T>> {
    check_cancel()?;
    let rx = spawn_op(current_req(), move || {
        check_cancel()?;
        act()
    });
    match rx.recv_timeout(std::time::Duration::from_millis(ms)) {
        Ok(Some(r)) => r.map(Some),
        Ok(None) => Err(err("INTERNAL", "the action panicked")),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(err("INTERNAL", "the action ended without an answer"))
        }
    }
}

/// A pattern of the element, or None when it has none.
pub(crate) fn pattern<T: Interface>(el: &IUIAutomationElement, id: UIA_PATTERN_ID) -> Option<T> {
    unsafe { el.GetCurrentPattern(id).ok()?.cast::<T>().ok() }
}

fn no_pattern(what: &str) -> super::DesktopError {
    err(
        "NO_PATTERN",
        format!("{what} is not supported by that element"),
    )
}

// ------------------------------------------------------------ ops

pub(crate) fn click(a: &Value) -> DResult<Value> {
    let button = str_arg(a, "button", "left");
    let count = int_arg(a, "count", 1)?.clamp(1, 3);
    let mods = parse_mods(a)?;
    let force = bool_arg(a, "force", false);
    let (p, e, info) = target_point(a)?;
    let mut res = json!({ "point": p });
    // Pattern first: a plain left click on something invokable does not
    // need the person's cursor at all.
    if let (Some(e), Some(info)) = (&e, &info) {
        if info.invoke
            && button == "left"
            && count == 1
            && mods.is_none()
            && !bool_arg(a, "physical", false)
        {
            if !info.enabled {
                return Err(err("DISABLED", "that element is disabled"));
            }
            let el = e.el.clone();
            let done = run_bounded(2000, move || {
                // The whole El moves in, not its non-Send field.
                let el = el;
                let p: IUIAutomationInvokePattern =
                    pattern(&el.0, UIA_InvokePatternId).ok_or_else(|| no_pattern("invoke"))?;
                unsafe { p.Invoke() }.map_err(Into::into)
            })?;
            res["ok"] = json!(true);
            res["via"] = json!("invoke");
            res["pending"] = json!(done.is_none());
            return Ok(res);
        }
    }
    let (down, up) = match button.as_str() {
        "left" => (0x2, 0x4),
        "right" => (0x8, 0x10),
        "middle" => (0x20, 0x40),
        _ => return Err(err("BAD_ARGS", "button must be left, right or middle")),
    };
    let hit_top = check_point(p[0], p[1], e.as_ref(), force)?;
    // A click at a point that names its window lands in that window (or
    // another of the same application) or not at all.
    let in_window = parse_handle(a.get("inWindow"))?;
    if e.is_none() && in_window != 0 && !force {
        let want = root(in_window);
        if hit_top != want && pid_of(hit_top) != pid_of(want) {
            return Err(err(
                "OCCLUDED",
                format!("another window (\"{}\") covers that spot", title(hit_top)),
            ));
        }
    }
    move_to(p[0], p[1])?;
    sleep_ms(20);
    hold_modifiers(mods.as_deref(), false)?;
    let clicked = (|| {
        for n in 0..count {
            // The button alone: an absolute move here would undo move_to's
            // correction by the rounding error.
            send_mouse_rel(0, 0, down, 0)?;
            send_mouse_rel(0, 0, up, 0)?;
            if n + 1 < count {
                sleep_ms(40);
            }
        }
        Ok::<_, super::DesktopError>(())
    })();
    let released = hold_modifiers(mods.as_deref(), true);
    clicked?;
    released?;
    res["ok"] = json!(true);
    res["via"] = json!("sendinput");
    res["windowHandle"] = json!(handle_str(hit_top));
    Ok(res)
}

pub(crate) fn hover(a: &Value) -> DResult<Value> {
    let (p, e, _) = target_point(a)?;
    check_point(p[0], p[1], e.as_ref(), bool_arg(a, "force", false))?;
    move_to(p[0], p[1])?;
    Ok(json!({ "ok": true, "point": p }))
}

fn point_arg(a: &Value, k: &str) -> DResult<[i32; 2]> {
    let Some(d) = a.get(k).filter(|d| d.is_object()) else {
        return Err(err("BAD_ARGS", format!("missing {k} {{x, y}}")));
    };
    Ok([require_int(d, "x")?, require_int(d, "y")?])
}

pub(crate) fn drag(a: &Value) -> DResult<Value> {
    let from = point_arg(a, "from")?;
    let to = point_arg(a, "to")?;
    check_point(from[0], from[1], None, false)?;
    check_point(to[0], to[1], None, false)?;
    let steps = int_arg(a, "steps", 12)?.clamp(2, 60);
    move_to(from[0], from[1])?;
    sleep_ms(30);
    send_mouse_rel(0, 0, 0x2, 0)?;
    // A drag stopped half way lets go where the pointer is, not at the
    // target.
    let moved = (|| {
        for n in 1..=steps {
            let x = from[0] + (to[0] - from[0]) * n / steps;
            let y = from[1] + (to[1] - from[1]) * n / steps;
            move_to(x, y)?;
            sleep_ms(12);
        }
        Ok::<_, super::DesktopError>(())
    })();
    let released = send_mouse_rel(0, 0, 0x4, 0);
    moved?;
    released?;
    Ok(json!({ "ok": true, "from": from, "to": to }))
}

pub(crate) fn type_cmd(a: &Value) -> DResult<Value> {
    let Some(text) = arg(a, "text").map(to_text) else {
        return Err(err("BAD_ARGS", "missing text"));
    };
    let mut win = parse_handle(a.get("windowHandle"))?;
    let mut e = None;
    if arg(a, "ref").is_some() {
        let r = uia::get_ref(arg(a, "ref"))?;
        win = r.top;
        e = Some(r);
    }
    let target = keyboard_target(win)?;
    let mut focused_via = "window";
    if let Some(e) = &e {
        focused_via = uia::focus_element(&e.el)?;
        sleep_ms(40);
    }
    type_into(target, &text)?;
    Ok(json!({
        "ok": true,
        "typed": u16len(&text),
        "focus": focused_via,
        "windowHandle": handle_str(target),
    }))
}

fn type_into(target: isize, text: &str) -> DResult<()> {
    const CHUNK: usize = 24;
    let units: Vec<u16> = text.replace("\r\n", "\n").encode_utf16().collect();
    let mut start = 0;
    while start < units.len() {
        check_cancel()?;
        guard_still(target, "characters", start)?;
        let mut end = units.len().min(start + CHUNK);
        // Never split a surrogate pair across chunks.
        if end < units.len() && (0xD800..0xDC00).contains(&units[end - 1]) {
            end += 1;
        }
        let mut list = Vec::with_capacity((end - start) * 2);
        for &c in &units[start..end] {
            if c == u16::from(b'\n') {
                list.push(key_input(0x0D, false));
                list.push(key_input(0x0D, true));
            } else if c == u16::from(b'\t') {
                list.push(key_input(0x09, false));
                list.push(key_input(0x09, true));
            } else {
                list.push(unicode_input(c, false));
                list.push(unicode_input(c, true));
            }
        }
        send_inputs(&list)?;
        start = end;
        sleep_ms(8);
    }
    Ok(())
}

pub(crate) fn key_cmd(a: &Value) -> DResult<Value> {
    let keys = str_arg(a, "keys", "").trim().to_string();
    if keys.is_empty() {
        return Err(err("BAD_ARGS", "missing keys"));
    }
    let repeat = int_arg(a, "repeat", 1)?.clamp(1, 100);
    let chords = keys
        .split(' ')
        .filter(|c| !c.is_empty())
        .map(parse_chord)
        .collect::<DResult<Vec<_>>>()?;
    let target = keyboard_target(parse_handle(a.get("windowHandle"))?)?;
    let mut done = 0;
    for _ in 0..repeat {
        for c in &chords {
            check_cancel()?;
            guard_still(target, "keys", done)?;
            send_chord(c)?;
            done += 1;
            sleep_ms(15);
        }
    }
    Ok(json!({ "ok": true, "sent": done, "windowHandle": handle_str(target) }))
}

pub(crate) fn scroll_cmd(a: &Value) -> DResult<Value> {
    let dir = str_arg(a, "direction", "down").to_lowercase();
    let amount = int_arg(a, "amount", 3)?.clamp(1, 10);
    let dy = match dir.as_str() {
        "down" => amount,
        "up" => -amount,
        _ => 0,
    };
    let dx = match dir.as_str() {
        "right" => amount,
        "left" => -amount,
        _ => 0,
    };
    if dx == 0 && dy == 0 {
        return Err(err("BAD_ARGS", "direction must be up, down, left or right"));
    }
    let (p, e, info) = target_point(a)?;
    if let (Some(e), Some(info)) = (&e, &info) {
        if info.scroll && !bool_arg(a, "physical", false) {
            let sp: IUIAutomationScrollPattern =
                pattern(&e.el.0, UIA_ScrollPatternId).ok_or_else(|| no_pattern("scroll"))?;
            for _ in 0..dy.abs() {
                check_cancel()?;
                let v = if dy > 0 {
                    ScrollAmount_SmallIncrement
                } else {
                    ScrollAmount_SmallDecrement
                };
                unsafe { sp.Scroll(ScrollAmount_NoAmount, v)? };
            }
            for _ in 0..dx.abs() {
                check_cancel()?;
                let h = if dx > 0 {
                    ScrollAmount_SmallIncrement
                } else {
                    ScrollAmount_SmallDecrement
                };
                unsafe { sp.Scroll(h, ScrollAmount_NoAmount)? };
            }
            return Ok(json!({ "ok": true, "via": "pattern" }));
        }
    }
    check_point(p[0], p[1], e.as_ref(), bool_arg(a, "force", false))?;
    move_to(p[0], p[1])?;
    sleep_ms(15);
    // Positive dy scrolls down (the wheel turning towards you).
    for _ in 0..dy.abs() {
        send_mouse_rel(0, 0, MOUSEEVENTF_WHEEL, if dy > 0 { -120 } else { 120 })?;
        sleep_ms(10);
    }
    for _ in 0..dx.abs() {
        send_mouse_rel(0, 0, MOUSEEVENTF_HWHEEL, if dx > 0 { 120 } else { -120 })?;
        sleep_ms(10);
    }
    Ok(json!({ "ok": true, "via": "sendinput", "point": p }))
}

pub(crate) fn scroll_to(a: &Value) -> DResult<Value> {
    let e = uia::get_ref(arg(a, "ref"))?;
    refuse_elevated(e.top)?;
    let Some(p) = pattern::<IUIAutomationScrollItemPattern>(&e.el.0, UIA_ScrollItemPatternId)
    else {
        return Err(err(
            "NO_PATTERN",
            "that element cannot scroll itself into view (no ScrollItemPattern)",
        ));
    };
    check_cancel()?;
    unsafe { p.ScrollIntoView()? };
    sleep_ms(60);
    let info = uia::read(&e.el.0, false)?;
    let r = parse_ref(arg(a, "ref"))?;
    Ok(json!({ "ok": true, "node": uia::node_dict(&info, r, 0, 0, 300) }))
}

pub(crate) fn invoke_cmd(a: &Value) -> DResult<Value> {
    let e = uia::get_ref(arg(a, "ref"))?;
    refuse_elevated(e.top)?;
    let action = str_arg(a, "action", "invoke");
    let el = e.el.clone();
    let known = [
        "invoke",
        "toggle",
        "select",
        "expand",
        "collapse",
        "focus",
        "focusNear",
    ];
    if !known.contains(&action.as_str()) {
        return Err(err("BAD_ARGS", format!("unknown action: {action}")));
    }
    let what = action.clone();
    let done = run_bounded(2000, move || act(&el, &what)).map_err(|e| {
        // A pattern the element lacks, or one it refuses, is NO_PATTERN.
        if e.code == "UIA_ERROR" || e.code == "DISABLED" {
            err(
                "NO_PATTERN",
                format!("{action} is not supported by that element ({})", e.message),
            )
        } else {
            e
        }
    })?;
    let mut res = json!({ "ok": true, "via": action, "pending": done.is_none() });
    if let Some(Some(focus)) = done {
        res["focus"] = json!(focus);
    }
    Ok(res)
}

/// One invoke action; `focusNear` answers where focus went.
fn act(el: &El, action: &str) -> DResult<Option<&'static str>> {
    let missing = || no_pattern(action);
    unsafe {
        match action {
            "invoke" => {
                let p: IUIAutomationInvokePattern =
                    pattern(&el.0, UIA_InvokePatternId).ok_or_else(missing)?;
                p.Invoke()?;
            }
            "toggle" => {
                let p: IUIAutomationTogglePattern =
                    pattern(&el.0, UIA_TogglePatternId).ok_or_else(missing)?;
                p.Toggle()?;
            }
            "select" => {
                let p: IUIAutomationSelectionItemPattern =
                    pattern(&el.0, UIA_SelectionItemPatternId).ok_or_else(missing)?;
                p.Select()?;
            }
            "expand" | "collapse" => {
                let p: IUIAutomationExpandCollapsePattern =
                    pattern(&el.0, UIA_ExpandCollapsePatternId).ok_or_else(missing)?;
                if action == "expand" {
                    p.Expand()?;
                } else {
                    p.Collapse()?;
                }
            }
            "focus" => el.0.SetFocus()?,
            // Find's "act" on a text field: the field if it takes the
            // keyboard, else the nearest thing in or around it that does.
            _ => return Ok(Some(uia::focus_element(el)?)),
        }
    }
    Ok(None)
}

/// form_input: a string, a number, a boolean for a checkbox, or an option's
/// text for a combo box, without moving the cursor when a pattern can do it.
pub(crate) fn set_value_cmd(a: &Value) -> DResult<Value> {
    let e = uia::get_ref(arg(a, "ref"))?;
    refuse_elevated(e.top)?;
    let Some(value) = arg(a, "value") else {
        return Err(err("BAD_ARGS", "missing value"));
    };
    let sval = to_text(value);
    let i = uia::read(&e.el.0, false)?;
    if !i.enabled {
        return Err(err("DISABLED", "that element is disabled"));
    }
    let el = &e.el.0;
    check_cancel()?;
    let via;
    unsafe {
        if let (Some(want), true) = (value.as_bool(), i.toggle || i.select) {
            if i.toggle {
                let tp: IUIAutomationTogglePattern =
                    pattern(el, UIA_TogglePatternId).ok_or_else(|| no_pattern("toggle"))?;
                for _ in 0..3 {
                    if (tp.CurrentToggleState()? == ToggleState_On) == want {
                        break;
                    }
                    check_cancel()?;
                    tp.Toggle()?;
                }
                via = "toggle";
            } else {
                let sp: IUIAutomationSelectionItemPattern =
                    pattern(el, UIA_SelectionItemPatternId).ok_or_else(|| no_pattern("select"))?;
                if want {
                    sp.Select()?;
                } else {
                    sp.RemoveFromSelection()?;
                }
                via = "select";
            }
        } else if i.value_p && !i.value_ro {
            let vp: IUIAutomationValuePattern =
                pattern(el, UIA_ValuePatternId).ok_or_else(|| no_pattern("value"))?;
            vp.SetValue(&BSTR::from(sval.as_str()))?;
            via = "value";
        } else if i.range && !value.is_boolean() {
            let Ok(v) = sval.trim().parse::<f64>() else {
                return Err(err("BAD_ARGS", "that element takes a number"));
            };
            let rp: IUIAutomationRangeValuePattern =
                pattern(el, UIA_RangeValuePatternId).ok_or_else(|| no_pattern("rangevalue"))?;
            rp.SetValue(v)?;
            via = "rangevalue";
        } else if i.control_type == uia::CT_COMBOBOX || i.control_type == uia::CT_LIST {
            select_option(&e.el, &i, &sval)?;
            via = "selectionitem";
        } else if i.focused || uia::focusable_near(&e.el).is_some() {
            // No pattern: focus it (or the focusable part of it), select
            // everything, type over it.
            let target = keyboard_target(e.top)?;
            uia::focus_element(&e.el)?;
            sleep_ms(40);
            guard_still(target, "keys", 0)?;
            send_chord(&parse_chord("ctrl+a")?)?;
            sleep_ms(20);
            if sval.is_empty() {
                send_chord(&parse_chord("delete")?)?;
            } else {
                type_into(target, &sval)?;
            }
            via = "sendinput";
        } else {
            return Err(err(
                "NO_PATTERN",
                "that element takes no value: no Value, Toggle, RangeValue or SelectionItem pattern and no keyboard focus",
            ));
        }
    }
    let mut res = json!({ "via": via, "ok": true });
    if let Ok(info) = uia::read(el, false) {
        res["node"] = uia::node_dict(&info, parse_ref(arg(a, "ref"))?, 0, 0, 4000);
    }
    Ok(res)
}

fn select_option(el: &El, i: &Info, text: &str) -> DResult<()> {
    let mut ec: Option<IUIAutomationExpandCollapsePattern> = None;
    if i.expand {
        if let Some(p) =
            pattern::<IUIAutomationExpandCollapsePattern>(&el.0, UIA_ExpandCollapsePatternId)
        {
            if unsafe { p.Expand() }.is_ok() {
                sleep_ms(120);
                ec = Some(p);
            }
        }
    }
    let picked = (|| {
        let items = uia::selectable_descendants(el)?;
        let want = text.trim().to_lowercase();
        let mut found: Option<El> = None;
        for (it, name) in items {
            let n = name.to_lowercase();
            if n.trim() == want {
                found = Some(it);
                break;
            }
            if found.is_none() && n.contains(&text.to_lowercase()) {
                found = Some(it);
            }
        }
        let Some(m) = found else {
            return Err(err("NOT_FOUND", format!("no option named \"{text}\"")));
        };
        check_cancel()?;
        let sp: IUIAutomationSelectionItemPattern =
            pattern(&m.0, UIA_SelectionItemPatternId).ok_or_else(|| no_pattern("select"))?;
        unsafe { sp.Select()? };
        Ok(())
    })();
    if let Some(p) = ec {
        let _ = unsafe { p.Collapse() };
    }
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_parse_like_host_cs() {
        assert_eq!(parse_chord("ctrl+shift+s").unwrap(), vec![0x11, 0x10, 0x53]);
        assert_eq!(parse_chord("ctrl++").unwrap()[0], 0x11);
        assert_eq!(parse_chord("F5").unwrap(), vec![0x74]);
        assert_eq!(parse_chord("7").unwrap(), vec![0x37]);
        assert_eq!(parse_chord("bogus").unwrap_err().code, "BAD_ARGS");
        assert_eq!(parse_chord("").unwrap_err().code, "BAD_ARGS");
    }

    #[test]
    fn releases_pass_a_cancelled_request() {
        assert!(is_release(&key_input(0x41, true)));
        assert!(!is_release(&key_input(0x41, false)));
        assert!(is_release(&mouse_input(0, 0, 0x4, 0)));
        assert!(!is_release(&mouse_input(0, 0, 0x2, 0)));
        assert!(!is_release(&mouse_input(0, 0, MOUSEEVENTF_WHEEL, 120)));
    }

    #[test]
    fn modifiers_reject_plain_keys() {
        let e = parse_mods(&json!({ "modifiers": "ctrl+a" })).unwrap_err();
        assert_eq!(e.code, "BAD_ARGS");
        assert_eq!(parse_mods(&json!({})).unwrap(), None);
    }
}

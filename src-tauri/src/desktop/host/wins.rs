//! Top-level windows, monitors, focus and the own-window and elevation
//! refusals (host.cs "windows").

use std::collections::HashMap;

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::DwmGetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForMonitor;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumChildWindows, EnumWindows, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostMessageW,
    SetForegroundWindow, ShowWindow, WindowFromPoint,
};

use super::input;
use super::{
    bool_arg, check_cancel, err, handle_str, own, parse_handle, rect_json, require_int, sleep_ms,
    str_arg, DResult, Rect,
};

const GA_ROOT: u32 = 2;
const GA_ROOTOWNER: u32 = 3;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const WS_EX_TOOLWINDOW: i32 = 0x80;
const WS_EX_TRANSPARENT: i32 = 0x20;
const WS_EX_TOPMOST: i32 = 0x8;
const WS_EX_NOACTIVATE: i32 = 0x0800_0000;
const WM_CLOSE: u32 = 0x10;

pub(crate) fn hw(h: isize) -> HWND {
    h as HWND
}

pub(crate) fn root(h: isize) -> isize {
    unsafe { GetAncestor(hw(h), GA_ROOT) as isize }
}

pub(crate) fn foreground_root() -> isize {
    unsafe { root(GetForegroundWindow() as isize) }
}

pub(crate) fn is_window(h: isize) -> bool {
    h != 0 && unsafe { IsWindow(hw(h)) != 0 }
}

pub(crate) fn pid_of(h: isize) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hw(h), &mut pid);
    }
    pid
}

pub(crate) fn window_at(x: i32, y: i32) -> isize {
    unsafe { WindowFromPoint(POINT { x, y }) as isize }
}

pub(crate) fn ex_style(h: isize) -> i32 {
    unsafe { GetWindowLongW(hw(h), GWL_EXSTYLE) }
}

/// Every top-level window, in z-order. `f` returns false to stop.
pub(crate) fn enum_windows(mut f: impl FnMut(isize) -> bool) {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> i32 {
        let f = &mut *(l as *mut &mut dyn FnMut(isize) -> bool);
        i32::from(f(h as isize))
    }
    let mut d: &mut dyn FnMut(isize) -> bool = &mut f;
    unsafe {
        EnumWindows(Some(cb), &mut d as *mut _ as LPARAM);
    }
}

fn enum_children(parent: isize, mut f: impl FnMut(isize) -> bool) {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> i32 {
        let f = &mut *(l as *mut &mut dyn FnMut(isize) -> bool);
        i32::from(f(h as isize))
    }
    let mut d: &mut dyn FnMut(isize) -> bool = &mut f;
    unsafe {
        EnumChildWindows(hw(parent), Some(cb), &mut d as *mut _ as LPARAM);
    }
}

pub(crate) fn title(h: isize) -> String {
    unsafe {
        let n = GetWindowTextLengthW(hw(h));
        if n <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; n as usize + 2];
        let got = GetWindowTextW(hw(h), buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..got.max(0) as usize])
    }
}

fn title_len(h: isize) -> i32 {
    unsafe { GetWindowTextLengthW(hw(h)) }
}

pub(crate) fn class_of(h: isize) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hw(h), buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// DWMWA_EXTENDED_FRAME_BOUNDS leaves out the invisible resize border.
pub(crate) fn window_rect(h: isize) -> Option<Rect> {
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        let size = std::mem::size_of::<RECT>() as u32;
        let ptr: *mut RECT = &mut r;
        if DwmGetWindowAttribute(hw(h), 9, ptr.cast(), size) != 0
            && GetWindowRect(hw(h), &mut r) == 0
        {
            return None;
        }
    }
    Some([r.left, r.top, r.right - r.left, r.bottom - r.top])
}

fn is_cloaked(h: isize) -> bool {
    let mut c: i32 = 0;
    let ptr: *mut i32 = &mut c;
    unsafe { DwmGetWindowAttribute(hw(h), 14, ptr.cast(), 4) == 0 && c != 0 }
}

/// Mandatory integrity level RID of a process (0x2000 medium, 0x3000 high),
/// or -1 when the token cannot be opened, which for a process we can see at
/// all usually means it is above us.
pub(crate) fn integrity(pid: u32) -> i32 {
    unsafe {
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc.is_null() {
            return -1;
        }
        let mut token = std::ptr::null_mut();
        let opened = OpenProcessToken(proc, TOKEN_QUERY, &mut token) != 0;
        let mut out = -1;
        if opened {
            let mut len = 0u32;
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                std::ptr::null_mut(),
                0,
                &mut len,
            );
            if len > 0 {
                // u64 words keep the buffer aligned for the label's pointer.
                let mut buf = vec![0u64; (len as usize).div_ceil(8)];
                let ptr = buf.as_mut_ptr().cast();
                if GetTokenInformation(token, TokenIntegrityLevel, ptr, len, &mut len) != 0 {
                    let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
                    let sid = label.Label.Sid;
                    let count = *GetSidSubAuthorityCount(sid);
                    if count > 0 {
                        out = *GetSidSubAuthority(sid, u32::from(count) - 1) as i32;
                    }
                }
            }
            CloseHandle(token);
        }
        CloseHandle(proc);
        out
    }
}

pub(crate) fn self_integrity() -> i32 {
    static ME: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *ME.get_or_init(|| integrity(std::process::id()))
}

fn is_above(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let me = self_integrity();
    integrity(pid) > me && me >= 0
}

/// UIPI silently drops synthetic input and UIA calls from a normal process
/// into a higher-integrity one. Saying so beats a click that reports
/// success and did nothing.
pub(crate) fn refuse_elevated(top: isize) -> DResult<()> {
    if top == 0 {
        return Ok(());
    }
    if is_above(pid_of(top)) {
        return Err(err(
            "ELEVATED_TARGET",
            format!(
                "\"{}\" runs as administrator; Windows blocks input to it from a normal app",
                title(top)
            ),
        ));
    }
    Ok(())
}

/// Process names, the way Process.ProcessName gives them (no ".exe").
#[derive(Default)]
pub(crate) struct Names {
    cache: HashMap<u32, String>,
    snapshot: Option<HashMap<u32, String>>,
}

impl Names {
    pub fn get(&mut self, pid: u32) -> String {
        if let Some(n) = self.cache.get(&pid) {
            return n.clone();
        }
        let n = image_name(pid).unwrap_or_else(|| {
            self.snapshot
                .get_or_insert_with(snapshot_names)
                .get(&pid)
                .cloned()
                .unwrap_or_default()
        });
        self.cache.insert(pid, n.clone());
        n
    }
}

fn strip_exe(s: &str) -> String {
    let base = s.rsplit(['\\', '/']).next().unwrap_or(s);
    match base.len().checked_sub(4) {
        Some(cut) if base[cut..].eq_ignore_ascii_case(".exe") => base[..cut].to_string(),
        _ => base.to_string(),
    }
}

fn image_name(pid: u32) -> Option<String> {
    unsafe {
        let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if proc.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(proc, 0, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(proc);
        ok.then(|| strip_exe(&String::from_utf16_lossy(&buf[..len as usize])))
    }
}

fn snapshot_names() -> HashMap<u32, String> {
    use windows::Win32::Foundation::CloseHandle as Close;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut out = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                let exe = String::from_utf16_lossy(&e.szExeFile[..len]);
                out.insert(e.th32ProcessID, strip_exe(&exe));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = Close(snap);
    }
    out
}

pub(crate) fn process_name(pid: u32) -> String {
    Names::default().get(pid)
}

/// UWP apps sit inside ApplicationFrameHost; the process that matters is
/// the one owning the CoreWindow child.
fn app_pid(h: isize, cls: &str) -> u32 {
    let pid = pid_of(h);
    if cls != "ApplicationFrameWindow" {
        return pid;
    }
    let mut found = 0;
    enum_children(h, |c| {
        if class_of(c) == "Windows.UI.Core.CoreWindow" {
            found = pid_of(c);
            return false;
        }
        true
    });
    if found != 0 {
        found
    } else {
        pid
    }
}

fn monitor_info(mon: HMONITOR) -> Option<MONITORINFOEXW> {
    unsafe {
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let ptr: *mut MONITORINFOEXW = &mut mi;
        (GetMonitorInfoW(mon, ptr.cast::<MONITORINFO>()) != 0).then_some(mi)
    }
}

fn device_name(mi: &MONITORINFOEXW) -> String {
    let len = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(32);
    String::from_utf16_lossy(&mi.szDevice[..len])
}

fn monitor_device(h: isize) -> Value {
    let mon = unsafe { MonitorFromWindow(hw(h), 2) };
    match monitor_info(mon) {
        Some(mi) => json!(device_name(&mi)),
        None => Value::Null,
    }
}

pub(crate) fn win_info(h: isize, names: Option<&mut Names>, fg: isize) -> Value {
    let cls = class_of(h);
    let pid = app_pid(h, &cls);
    let process = match names {
        Some(n) => n.get(pid),
        None => process_name(pid),
    };
    let ex = ex_style(h);
    json!({
        "handle": handle_str(h),
        "title": title(h),
        "process": process,
        "pid": pid,
        "rect": rect_json(window_rect(h)),
        "display": monitor_device(h),
        "isForeground": h == fg,
        "isMinimized": unsafe { IsIconic(hw(h)) != 0 },
        // Not counted as hiding what is behind it: a window the mouse
        // passes through, one that never takes focus, and one kept on top.
        // An overlay is one of those, and may be see-through and as large
        // as the screen.
        "through": ex & (WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOPMOST) != 0,
        "elevated": is_above(pid_of(h)),
        "className": cls,
    })
}

fn listable(h: isize, include_minimized: bool) -> bool {
    unsafe {
        if IsWindowVisible(hw(h)) == 0 || is_cloaked(h) || is_excluded_window(h) {
            return false;
        }
        let iconic = IsIconic(hw(h)) != 0;
        if !include_minimized && iconic {
            return false;
        }
        if ex_style(h) & WS_EX_TOOLWINDOW != 0 {
            return false;
        }
        if title_len(h) == 0 {
            return false;
        }
        match window_rect(h) {
            Some(r) => (r[2] > 0 && r[3] > 0) || iconic,
            None => false,
        }
    }
}

pub(crate) fn list_windows(a: &Value) -> DResult<Value> {
    let include_min = bool_arg(a, "includeMinimized", false);
    let mut names = Names::default();
    let fg = foreground_root();
    let mut list = Vec::new();
    enum_windows(|h| {
        if listable(h, include_min) {
            list.push(win_info(h, Some(&mut names), fg));
        }
        true
    });
    Ok(Value::Array(list))
}

/// The app the person was in: the foreground window, unless that is
/// Aevaleron itself (it usually is, at the moment someone asks), in which
/// case the topmost ordinary window under it.
pub(crate) fn user_foreground() -> isize {
    let fg = foreground_root();
    if fg != 0 && !is_excluded_window(fg) && unsafe { IsWindowVisible(hw(fg)) != 0 } {
        return fg;
    }
    let mut pick = 0;
    enum_windows(|h| {
        if !listable(h, false) {
            return true;
        }
        pick = h;
        false
    });
    pick
}

pub(crate) fn foreground_window() -> DResult<Value> {
    let h = user_foreground();
    if h == 0 {
        return Ok(Value::Null);
    }
    Ok(win_info(h, None, foreground_root()))
}

/// Read-only, for the self-check's window gates: the style bits, topmost
/// flag and z-order of the given windows, the first window titled `title`,
/// and the top-level window under `point`. It acts on nothing, so our own
/// windows are allowed.
pub(crate) fn window_states(a: &Value) -> DResult<Value> {
    let mut order = Vec::new();
    enum_windows(|h| {
        order.push(h);
        true
    });
    let z = |h: isize| order.iter().position(|&o| o == h).map_or(-1, |i| i as i64);
    let mut list = Vec::new();
    if let Some(hs) = a.get("handles").and_then(Value::as_array) {
        for v in hs {
            let h = parse_handle(Some(v))?;
            let ex = ex_style(h);
            list.push(json!({
                "handle": handle_str(h),
                "exists": is_window(h),
                "style": unsafe { GetWindowLongW(hw(h), GWL_STYLE) },
                "exStyle": ex,
                "topmost": ex & WS_EX_TOPMOST != 0,
                "visible": unsafe { IsWindowVisible(hw(h)) != 0 },
                "z": z(h),
            }));
        }
    }
    let mut res = json!({ "windows": list });
    let want = str_arg(a, "title", "");
    if !want.is_empty() {
        for &h in &order {
            let n = title_len(h);
            if n <= 0 || n > 200 || title(h) != want {
                continue;
            }
            res["titled"] = json!({
                "handle": handle_str(h),
                "z": z(h),
                "topmost": ex_style(h) & WS_EX_TOPMOST != 0,
            });
            break;
        }
    }
    if let Some(pt) = a.get("point").filter(|p| p.is_object()) {
        let (x, y) = (require_int(pt, "x")?, require_int(pt, "y")?);
        // With move, the real pointer goes there first, so the page under
        // it gets its mousemove and sets its own click-through state.
        if bool_arg(a, "move", false) {
            input::move_to(x, y)?;
            sleep_ms(300);
        }
        res["atPoint"] = json!(handle_str(root(window_at(x, y))));
    }
    Ok(res)
}

/// The window to read: the one named, else the person's foreground app.
pub(crate) fn target_window(a: &Value) -> DResult<isize> {
    let h = parse_handle(a.get("windowHandle"))?;
    let hwnd = if h == 0 { user_foreground() } else { h };
    if hwnd == 0 {
        return Err(err("NOT_FOUND", "no window to read"));
    }
    if !is_window(hwnd) {
        return Err(err(
            "NOT_FOUND",
            format!("window {} no longer exists", handle_str(hwnd)),
        ));
    }
    let hwnd = root(hwnd);
    refuse_excluded(hwnd)?;
    Ok(hwnd)
}

pub(crate) fn monitors() -> Value {
    unsafe extern "system" fn cb(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
        let list = &mut *(data as *mut Vec<Value>);
        let Some(mi) = monitor_info(mon) else {
            return 1;
        };
        let (mut dx, mut dy) = (96u32, 96u32);
        if GetDpiForMonitor(mon, 0, &mut dx, &mut dy) != 0 {
            dx = 96;
        }
        let m = mi.monitorInfo.rcMonitor;
        let w = mi.monitorInfo.rcWork;
        list.push(json!({
            "rect": [m.left, m.top, m.right - m.left, m.bottom - m.top],
            "work": [w.left, w.top, w.right - w.left, w.bottom - w.top],
            "dpi": dx,
            "scale": f64::from(dx) / 96.0,
            "primary": mi.monitorInfo.dwFlags & 1 != 0,
            "device": device_name(&mi),
        }));
        1
    }
    let mut list: Vec<Value> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            std::ptr::null_mut(),
            std::ptr::null(),
            Some(cb),
            &mut list as *mut _ as LPARAM,
        );
    }
    Value::Array(list)
}

fn wait_foreground(h: isize) -> bool {
    for _ in 0..10 {
        if foreground_root() == h {
            return true;
        }
        sleep_ms(20);
    }
    false
}

/// Brings `h` forward, trying each way Windows allows; answers how, or
/// None when nothing worked.
pub(crate) fn focus_window(h: isize) -> DResult<Option<&'static str>> {
    check_cancel()?;
    unsafe {
        if IsIconic(hw(h)) != 0 {
            ShowWindow(hw(h), 9);
            sleep_ms(150);
        }
        if foreground_root() == h {
            return Ok(Some("already"));
        }
        check_cancel()?;
        if SetForegroundWindow(hw(h)) != 0 && wait_foreground(h) {
            return Ok(Some("direct"));
        }
        // Windows lets the process that sent the last input take the
        // foreground. A zero-length relative mouse move is input that moves
        // nothing.
        input::send_mouse_rel(0, 0, input::MOUSEEVENTF_MOVE, 0)?;
        if SetForegroundWindow(hw(h)) != 0 && wait_foreground(h) {
            return Ok(Some("input"));
        }
        check_cancel()?;
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        let me = GetCurrentThreadId();
        let attached =
            fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, 1) != 0;
        BringWindowToTop(hw(h));
        SetForegroundWindow(hw(h));
        if attached {
            AttachThreadInput(me, fg_thread, 0);
        }
        if wait_foreground(h) {
            return Ok(Some("attach"));
        }
        // Last resort: the foreground lock lets a window come forward while
        // Alt is held. The Alt-down lands in the old window and the Alt-up in
        // the new one, so neither sees a lone Alt tap and no menu bar opens.
        input::send_inputs(&[input::key_input(0x12, false)])?;
        SetForegroundWindow(hw(h));
        let _ = input::send_inputs(&[input::key_input(0x12, true)]);
        Ok(wait_foreground(h).then_some("alt"))
    }
}

pub(crate) fn focus_cmd(a: &Value) -> DResult<Value> {
    let h = parse_handle(a.get("handle"))?;
    if !is_window(h) {
        return Err(err("NOT_FOUND", "no such window"));
    }
    let h = root(h);
    refuse_excluded(h)?;
    refuse_elevated(h)?;
    refuse_secure_desktop()?;
    let how = focus_window(h)?;
    Ok(json!({ "ok": how.is_some(), "method": how }))
}

pub(crate) fn close_cmd(a: &Value) -> DResult<Value> {
    let h = parse_handle(a.get("handle"))?;
    if !is_window(h) {
        return Err(err("NOT_FOUND", "no such window"));
    }
    refuse_excluded(h)?;
    check_cancel()?;
    let ok = unsafe { PostMessageW(hw(h), WM_CLOSE, 0, 0) != 0 };
    Ok(json!({ "ok": ok }))
}

// ------------------------------------------------------------ refusals

pub(crate) fn is_excluded_pid(pid: u32) -> bool {
    own().pids.contains(&pid)
}

pub(crate) fn is_excluded_window(h: isize) -> bool {
    if h == 0 {
        return false;
    }
    let mut top = root(h);
    let owner = unsafe { GetAncestor(hw(h), GA_ROOTOWNER) as isize };
    if top == 0 {
        top = h;
    }
    let o = own();
    if o.wins
        .iter()
        .any(|w| w.handle == h || w.handle == top || w.handle == owner)
    {
        return true;
    }
    if o.pids.contains(&pid_of(top)) {
        return true;
    }
    owner != 0 && o.pids.contains(&pid_of(owner))
}

pub(crate) fn refuse_excluded(h: isize) -> DResult<()> {
    if is_excluded_window(h) {
        return Err(err(
            "BLOCKED_OWN_WINDOW",
            "refusing to read or act on Aevaleron's own window",
        ));
    }
    Ok(())
}

/// Any input whose point lands inside one of our visible windows is
/// refused, even when that window is click-through and the click would pass
/// under it: the person sees our surface there, so an agent clicking
/// "through" it is clicking something they cannot see. A window registered
/// with block:false (a full-screen lens) is exempt.
pub(crate) fn refuse_own_point(x: i32, y: i32) -> DResult<()> {
    let blocked = || {
        Err(err(
            "BLOCKED_OWN_WINDOW",
            "that point is on Aevaleron's own window",
        ))
    };
    let o = own();
    for w in o.wins.iter().filter(|w| w.block) {
        let h = w.handle;
        unsafe {
            if !is_window(h) || IsWindowVisible(hw(h)) == 0 || IsIconic(hw(h)) != 0 {
                continue;
            }
        }
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if unsafe { GetWindowRect(hw(h), &mut r) } == 0 {
            continue;
        }
        if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
            return blocked();
        }
    }
    let hit = window_at(x, y);
    if hit != 0 && is_excluded_window(hit) {
        let top = root(hit);
        let exempt = o.wins.iter().any(|w| w.handle == top && !w.block);
        if !exempt {
            return blocked();
        }
    }
    Ok(())
}

/// A locked session (LockApp in front) or a UAC prompt (the input desktop
/// is not ours) takes no input from us, and must not: typing at a lock
/// screen is typing into a password box.
pub(crate) fn refuse_secure_desktop() -> DResult<()> {
    let fg = unsafe { GetForegroundWindow() as isize };
    if fg != 0 && process_name(pid_of(fg)).eq_ignore_ascii_case("LockApp") {
        return Err(err(
            "SECURE_DESKTOP",
            "the session is locked; nothing can be clicked or typed until it is unlocked",
        ));
    }
    unsafe {
        let desk = OpenInputDesktop(0, 0, 0x0100);
        if desk.is_null() {
            return Err(err(
                "SECURE_DESKTOP",
                "a secure desktop (sign-in or UAC prompt) has the input; it cannot be driven",
            ));
        }
        CloseDesktop(desk);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_lose_the_extension() {
        assert_eq!(strip_exe(r"C:\Windows\notepad.exe"), "notepad");
        assert_eq!(strip_exe("Code.EXE"), "Code");
        assert_eq!(strip_exe("System"), "System");
    }

    #[test]
    fn our_own_process_is_named_and_measured() {
        assert!(!process_name(std::process::id()).is_empty());
        assert!(self_integrity() >= -1);
        assert!(is_excluded_pid(std::process::id()));
    }

    #[test]
    fn monitors_have_rects() {
        if let Some(m) = monitors().as_array().and_then(|a| a.first()) {
            assert_eq!(m["rect"].as_array().map(Vec::len), Some(4));
        }
    }
}

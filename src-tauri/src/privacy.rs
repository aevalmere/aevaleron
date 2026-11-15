//! Private apps: a list of programs Aevaleron never reads
//! (.claude/rust-rewrite/lanes/private-apps.md).
//!
//! The list holds exe names (`1password.exe`), kept under `privateApps` in
//! aevaleron.json. While the window in front belongs to one of them nothing
//! is read from the screen: no screenshot, no text through UI Automation,
//! no text recognition, and so nothing from it reaches a model. A capture
//! of the whole screen is refused too; one window is never blacked out.
//!
//! Every reader asks [`check`] before it reads:
//!
//! - `capture::grab`, the one source of screen pixels (Ask's screenshot, a
//!   picked area, the lens's loupe, the glass backdrop);
//! - `Desktop::call`, the one door to the desktop helper ([`check_op`]
//!   before an op is sent, [`check_reply`] on what a read op answers), which
//!   covers Ask's window text, Find, and every read and action of the agent;
//! - Ask, Find, the area picker and the lens themselves, so each can say why
//!   it did nothing;
//! - the agents lane's chat watcher, which reads other apps' windows without
//!   the helper.
//!
//! "The window in front" is the foreground window, or, while one of our own
//! windows has the foreground (the bar usually does when someone asks), the
//! first ordinary window under it: the desktop helper's own rule (host.cs
//! `UserForeground`), so both sides mean the same window.

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::agent::seams::DeskError;
use crate::desktop::DesktopError;
use crate::state::{shared, Mode};
use crate::windows;

/// The settings key. Missing or not a list: [`DEFAULTS`]. A list, even an
/// empty one, is the person's own.
pub const KEY: &str = "privateApps";

/// The error code a refused read carries through the desktop seams.
pub const CODE: &str = "PRIVATE_APP";

/// What the list starts as: password managers and Windows Security, by the
/// exe names their Windows apps run as. LastPass is left out: its desktop
/// app's process name could not be confirmed.
pub const DEFAULTS: [&str; 7] = [
    "1password.exe",
    "bitwarden.exe",
    "dashlane.exe",
    "keepass.exe",
    "keepassxc.exe",
    "protonpass.exe",
    "sechealthui.exe",
];

/// The names people know the default apps by.
const NAMES: [(&str, &str); 7] = [
    ("1password.exe", "1Password"),
    ("bitwarden.exe", "Bitwarden"),
    ("dashlane.exe", "Dashlane"),
    ("keepass.exe", "KeePass"),
    ("keepassxc.exe", "KeePassXC"),
    ("protonpass.exe", "Proton Pass"),
    ("sechealthui.exe", "Windows Security"),
];

const BAD_NAME: &str =
    "That is not a program name. Type the name of the app's .exe file, for example notepad.exe.";
const NO_FRONT: &str =
    "No app has been in front since Aevaleron started. Open the bar over the app, then try again.";

// ------------------------------------------------------------------ names

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// A name as the list keeps it: the file name of whatever was given (a
/// name, with or without `.exe`, or a whole path), in lower case, ending
/// in `.exe`. None when no file name is left.
pub fn normalize(input: &str) -> Option<String> {
    let given = input.trim().trim_matches(['"', '\'']);
    let file = file_name(given).trim().trim_end_matches('.');
    let mut name = file.to_lowercase();
    let bad = |c: char| c.is_control() || "<>:\"|?*".contains(c);
    if name.is_empty() || name == ".exe" || name.chars().count() > 255 || name.chars().any(bad) {
        return None;
    }
    if !name.ends_with(".exe") {
        name.push_str(".exe");
    }
    Some(name)
}

/// Whether `process` (an exe name in any case, with or without `.exe`, or
/// a whole path) is on `list`.
pub fn is_listed(list: &[String], process: &str) -> bool {
    normalize(process).is_some_and(|p| list.contains(&p))
}

/// The name a sentence uses for an exe: the product's for the apps we
/// know, else the file's own name without `.exe`, starting with a capital.
pub fn display_name(exe: &str) -> String {
    let key = normalize(exe).unwrap_or_default();
    if let Some((_, known)) = NAMES.iter().find(|(e, _)| *e == key) {
        return (*known).to_string();
    }
    let file = file_name(exe.trim()).trim();
    let at = file.len().saturating_sub(4);
    let has_exe = file
        .get(at..)
        .is_some_and(|t| t.eq_ignore_ascii_case(".exe"));
    let stem = if has_exe { &file[..at] } else { file };
    let mut letters = stem.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

// ------------------------------------------------------------------- list

fn list_from(stored: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = stored else {
        return DEFAULTS.iter().map(|s| s.to_string()).collect();
    };
    let mut out: Vec<String> = Vec::new();
    for name in items.iter().filter_map(Value::as_str) {
        if let Some(exe) = normalize(name).filter(|e| !out.contains(e)) {
            out.push(exe);
        }
    }
    out
}

/// The list as stored. Empty before the settings file is open (a unit
/// test), so nothing there is checked against the real desktop.
pub fn list() -> Vec<String> {
    match crate::store::try_get() {
        Some(store) => list_from(store.raw(KEY).as_ref()),
        None => Vec::new(),
    }
}

/// Stores the list and has the glass look again at once: it is solid while
/// a listed app is in front.
fn save(list: &[String]) {
    if let Some(store) = crate::store::try_get() {
        store.set_raw(KEY, json!(list));
    }
    crate::backdrop::broadcast_tier();
}

/// The desktop helper hands out refs to elements it has read. One made
/// before an app was listed must not outlive the listing, so the helper
/// starts afresh on its next call and every old ref is stale.
fn forget_refs() {
    let helper = crate::desktop::client();
    if helper.is_ready() {
        helper.stop();
    }
}

/// Adds a program to the list: its exe name as kept, and whether it was
/// new. Err says why `name` is not a program name.
pub fn add(name: &str) -> Result<(String, bool), String> {
    let exe = normalize(name).ok_or_else(|| BAD_NAME.to_string())?;
    let mut all = list();
    if all.contains(&exe) {
        return Ok((exe, false));
    }
    all.push(exe.clone());
    save(&all);
    forget_refs();
    Ok((exe, true))
}

/// Takes a program off the list; false when it was not on it.
pub fn remove(name: &str) -> bool {
    let Some(exe) = normalize(name) else {
        return false;
    };
    let mut all = list();
    let before = all.len();
    all.retain(|e| *e != exe);
    if all.len() == before {
        return false;
    }
    save(&all);
    true
}

// ------------------------------------------------------------------- gate

/// What is about to be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Read {
    /// The screen's pixels, or whichever window is in front.
    Screen,
    /// One window, by handle.
    Window(isize),
    /// Whatever is at a point, in physical pixels.
    Point(i32, i32),
}

/// A read that was not made, because the app is on the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    /// The app's name, for a sentence ("1Password").
    pub app: String,
}

impl Refused {
    /// The one line every surface says.
    pub fn message(&self) -> String {
        format!(
            "{} is on your private apps list, so the screen was not read.",
            self.app
        )
    }
}

impl From<Refused> for DesktopError {
    fn from(r: Refused) -> DesktopError {
        DesktopError::new(CODE, r.message())
    }
}

impl From<Refused> for DeskError {
    fn from(r: Refused) -> DeskError {
        DeskError::new(CODE, r.message())
    }
}

fn verdict(list: &[String], exe: &str) -> Result<(), Refused> {
    if exe.is_empty() || !is_listed(list, exe) {
        return Ok(());
    }
    Err(Refused {
        app: display_name(exe),
    })
}

/// The gate. Every reader calls it before it reads the screen, a window or
/// a point, and reads nothing when it refuses.
pub fn check(what: Read) -> Result<(), Refused> {
    let list = list();
    if list.is_empty() {
        return Ok(());
    }
    verdict(&list, &win::exe_of(win::window_of(what)))
}

/// What a desktop helper op would read or act on, from its arguments. An
/// op that names only a ref is not listed: a ref is made by a read that was
/// checked, and none outlives a change to the list ([`forget_refs`]).
pub fn op_reads(op: &str, args: &Value) -> Vec<Read> {
    let by_ref = args.get("ref").is_some_and(|r| !r.is_null());
    let window = args
        .get("windowHandle")
        .and_then(crate::desktop::protocol::handle_of)
        .filter(|h| *h != 0)
        .map(|h| Read::Window(h as isize));
    let point = |v: &Value| {
        let x = v.get("x").and_then(Value::as_f64)?;
        let y = v.get("y").and_then(Value::as_f64)?;
        Some(Read::Point(x.round() as i32, y.round() as i32))
    };
    match op {
        // Text and tree reads, and the keyboard, which goes to one window.
        "readTree" | "getText" | "findText" | "docText" | "focused" | "type" | "key" => {
            if by_ref {
                Vec::new()
            } else {
                vec![window.unwrap_or(Read::Screen)]
            }
        }
        // Pixels come from the screen as composed, whatever window was
        // asked for.
        "capture" | "ocr" => {
            let mut reads = vec![Read::Screen];
            reads.extend(window);
            reads
        }
        "elementAt" | "click" | "hover" | "scroll" => {
            if by_ref {
                Vec::new()
            } else {
                point(args).into_iter().collect()
            }
        }
        "drag" => [args.get("from"), args.get("to")]
            .into_iter()
            .flatten()
            .filter_map(point)
            .collect(),
        _ => Vec::new(),
    }
}

/// The ops that read "the window in front" when given no window.
fn reads_front(op: &str) -> bool {
    matches!(
        op,
        "readTree" | "getText" | "findText" | "docText" | "capture" | "ocr"
    )
}

/// The gate for a desktop helper op, asked just before it is sent. An op
/// about to read the window in front is pinned to the window that was
/// checked, so the helper cannot pick another one a moment later.
pub fn check_op(op: &str, args: &mut Value) -> Result<(), Refused> {
    let list = list();
    if list.is_empty() {
        return Ok(());
    }
    for read in op_reads(op, args) {
        let hwnd = win::window_of(read);
        verdict(&list, &win::exe_of(hwnd))?;
        if read != Read::Screen || hwnd == 0 || !reads_front(op) {
            continue;
        }
        if let Some(a) = args.as_object_mut() {
            let given = a.get("windowHandle").is_some_and(|h| !h.is_null());
            if !given {
                a.insert("windowHandle".into(), json!(hwnd as u64));
            }
        }
    }
    Ok(())
}

/// The gate again on what a read op answered: the helper says which window
/// it read, and an answer from a listed app goes no further.
pub fn check_reply(op: &str, reply: &Value) -> Result<(), Refused> {
    let reads = matches!(
        op,
        "readTree" | "getText" | "findText" | "docText" | "focused" | "elementAt" | "resolve"
    );
    if !reads {
        return Ok(());
    }
    let handle = reply
        .get("windowHandle")
        .and_then(crate::desktop::protocol::handle_of)
        .filter(|h| *h != 0);
    match handle {
        Some(h) => check(Read::Window(h as isize)),
        None => Ok(()),
    }
}

// ---------------------------------------------------------------- the bar

/// The notice the bar was last given, to tell when it has changed.
static TOLD: Mutex<String> = parking_lot::const_mutex(String::new());

/// The line for the app in front now, or empty when it is not on the list.
fn line_now() -> String {
    let refused = check(Read::Screen).err();
    refused.map(|r| r.message()).unwrap_or_default()
}

/// `overlay:state.privateNotice`: the bar's one line while the app in front
/// is on the list, else empty.
pub fn notice() -> String {
    let now = line_now();
    *TOLD.lock() = now.clone();
    now
}

/// The bar took the keyboard: the app under it may have changed since the
/// bar was told, so it is told again when the notice differs.
pub fn bar_focused(app: &AppHandle) {
    let now = line_now();
    let changed = *TOLD.lock() != now;
    if changed {
        windows::send_overlay_state(app);
    }
}

/// A read was just refused: the bar's notice is brought up to date, and
/// with `open` a folded bar opens first, so the person sees why nothing
/// happened.
pub fn tell(app: &AppHandle, open: bool) {
    let a = app.clone();
    let _ = app.run_on_main_thread(move || {
        if open && shared(&a).lock().mode == Mode::Pill {
            windows::open_bar(&a, None, None);
        }
        windows::send_overlay_state(&a);
    });
}

// --------------------------------------------------------------- settings

/// The app that was in front when the bar last opened, as (exe, name).
fn remembered() -> Option<(String, String)> {
    let hwnd = windows::flags::remembered_foreground();
    let exe = win::exe_of(win::window_of(Read::Window(hwnd)));
    normalize(&exe).map(|kept| (kept, display_name(&exe)))
}

/// What Settings' Private apps section shows: the list, and the app that
/// was in front when the bar last opened.
fn snapshot() -> Value {
    let listed = list();
    let apps: Vec<Value> = listed
        .iter()
        .map(|exe| json!({ "exe": exe, "name": display_name(exe) }))
        .collect();
    let front = remembered().map(|(exe, name)| {
        let on = listed.contains(&exe);
        json!({ "exe": exe, "name": name, "listed": on })
    });
    json!({ "apps": apps, "front": front })
}

fn with_error(why: &str) -> Value {
    let mut v = snapshot();
    v["error"] = json!(why);
    v
}

/// The `privacy:*` channels, all Settings'. Each answers with the whole
/// section's state. `None` for any other channel.
pub fn route(app: &AppHandle, channel: &str, p: &Value) -> Option<Result<Value, String>> {
    let out = match channel {
        "privacy:get" => snapshot(),
        // `{name}` adds a program by name; `{front: true}` adds the app
        // that was in front when the bar last opened.
        "privacy:add" => {
            let front = p.get("front").and_then(Value::as_bool).unwrap_or(false);
            let name = if front {
                remembered().map(|(exe, _)| exe)
            } else {
                p.get("name").and_then(Value::as_str).map(str::to_string)
            };
            match name.as_deref().map(add) {
                Some(Ok((exe, new))) => {
                    windows::send_overlay_state(app);
                    let mut v = snapshot();
                    v["added"] = json!({ "exe": exe, "name": display_name(&exe), "new": new });
                    v
                }
                Some(Err(why)) => with_error(&why),
                None if front => with_error(NO_FRONT),
                None => with_error(BAD_NAME),
            }
        }
        "privacy:remove" => {
            let exe = p.get("exe").and_then(Value::as_str).unwrap_or_default();
            let removed = remove(exe);
            if removed {
                windows::send_overlay_state(app);
            }
            let mut v = snapshot();
            v["removed"] = json!(removed);
            v
        }
        _ => return None,
    };
    Some(Ok(out))
}

// ------------------------------------------------------------------ Win32

/// Which window a read means and which program owns it. Plain Win32
/// queries, declared as `agents/native.rs` declares them.
mod win {
    use std::ffi::c_void;

    use super::Read;

    /// POINT. Only Windows reads the fields.
    #[allow(dead_code)]
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn QueryFullProcessImageNameW(
            process: isize,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> isize;
        fn IsWindowVisible(hwnd: isize) -> i32;
        fn IsIconic(hwnd: isize) -> i32;
        fn GetAncestor(hwnd: isize, flags: u32) -> isize;
        fn GetWindowThreadProcessId(hwnd: isize, pid: *mut u32) -> u32;
        fn EnumWindows(
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            param: isize,
        ) -> i32;
        fn EnumChildWindows(
            parent: isize,
            callback: unsafe extern "system" fn(isize, isize) -> i32,
            param: isize,
        ) -> i32;
        fn GetWindowTextLengthW(hwnd: isize) -> i32;
        fn GetWindowLongPtrW(hwnd: isize, index: i32) -> isize;
        fn GetClassNameW(hwnd: isize, name: *mut u16, max: i32) -> i32;
        fn GetWindowRect(hwnd: isize, rect: *mut Rect) -> i32;
        fn WindowFromPoint(point: Point) -> isize;
    }

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmGetWindowAttribute(hwnd: isize, attribute: u32, value: *mut c_void, size: u32)
            -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const GA_ROOT: u32 = 2;
    const GA_ROOTOWNER: u32 = 3;
    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x80;
    const DWMWA_CLOAKED: u32 = 14;

    fn pid_of(hwnd: isize) -> u32 {
        let mut pid = 0u32;
        // SAFETY: a valid out pointer; a stale handle leaves it 0.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        pid
    }

    fn class_of(hwnd: isize) -> String {
        let mut buf = [0u16; 256];
        // SAFETY: the buffer is as long as the length given.
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        String::from_utf16_lossy(&buf[..n.clamp(0, 255) as usize])
    }

    /// The top-level window `hwnd` belongs to.
    fn root(hwnd: isize) -> isize {
        if hwnd == 0 {
            return 0;
        }
        // SAFETY: a query on a handle that may be stale.
        let top = unsafe { GetAncestor(hwnd, GA_ROOT) };
        if top == 0 {
            hwnd
        } else {
            top
        }
    }

    /// One of Aevaleron's own windows, or a window one of them owns (a
    /// menu the web view opened).
    fn ours(hwnd: isize) -> bool {
        let me = std::process::id();
        // SAFETY: a query on a handle that may be stale.
        let owner = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
        pid_of(hwnd) == me || (owner != 0 && pid_of(owner) == me)
    }

    fn cloaked(hwnd: isize) -> bool {
        let mut value = 0i32;
        // SAFETY: the out value is four bytes, as said.
        let hr = unsafe {
            DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&mut value as *mut i32).cast(), 4)
        };
        hr == 0 && value != 0
    }

    /// An ordinary app window someone could be working in (host.cs
    /// `Listable`): shown, not minimized, not a tool window, with a title
    /// and a size, and not ours.
    fn listable(hwnd: isize) -> bool {
        // SAFETY: queries on a handle the enumeration just gave.
        unsafe {
            if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
                return false;
            }
            if GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW != 0 {
                return false;
            }
            if GetWindowTextLengthW(hwnd) == 0 {
                return false;
            }
            let mut r = Rect::default();
            if GetWindowRect(hwnd, &mut r) == 0 || r.right <= r.left || r.bottom <= r.top {
                return false;
            }
        }
        !cloaked(hwnd) && !ours(hwnd)
    }

    unsafe extern "system" fn first_listable(hwnd: isize, param: isize) -> i32 {
        if !listable(hwnd) {
            return 1;
        }
        // SAFETY: `param` is the `isize` `front_window` passed, alive for
        // the whole enumeration.
        unsafe {
            *(param as *mut isize) = hwnd;
        }
        0
    }

    /// The window the person is in (host.cs `UserForeground`), or 0.
    fn front_window() -> isize {
        // SAFETY: queries; the callback writes only to `pick`.
        unsafe {
            let fg = root(GetForegroundWindow());
            if fg != 0 && !ours(fg) && IsWindowVisible(fg) != 0 {
                return fg;
            }
            let mut pick = 0isize;
            EnumWindows(first_listable, &mut pick as *mut isize as isize);
            pick
        }
    }

    /// The top-level window a read is about, or 0 when there is none.
    pub fn window_of(what: Read) -> isize {
        match what {
            Read::Screen => front_window(),
            Read::Window(hwnd) => root(hwnd),
            // SAFETY: a query.
            Read::Point(x, y) => root(unsafe { WindowFromPoint(Point { x, y }) }),
        }
    }

    unsafe extern "system" fn core_window(child: isize, param: isize) -> i32 {
        if class_of(child) != "Windows.UI.Core.CoreWindow" {
            return 1;
        }
        // SAFETY: `param` is the `u32` `app_pid` passed, alive for the
        // whole enumeration.
        unsafe {
            *(param as *mut u32) = pid_of(child);
        }
        0
    }

    /// The process an app's window belongs to. Store apps sit inside
    /// ApplicationFrameHost; the process that matters there owns the
    /// CoreWindow child (host.cs `AppPid`).
    fn app_pid(hwnd: isize) -> u32 {
        let pid = pid_of(hwnd);
        if class_of(hwnd) != "ApplicationFrameWindow" {
            return pid;
        }
        let mut found = 0u32;
        // SAFETY: the callback writes only to `found`.
        unsafe {
            EnumChildWindows(hwnd, core_window, &mut found as *mut u32 as isize);
        }
        if found != 0 {
            found
        } else {
            pid
        }
    }

    fn image_name(pid: u32) -> String {
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        // SAFETY: a query-only handle, closed before returning; the buffer
        // is `size` units long.
        let ok = unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process == 0 {
                return String::new();
            }
            let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut size);
            CloseHandle(process);
            ok
        };
        if ok == 0 {
            return String::new();
        }
        let path = String::from_utf16_lossy(&buf[..(size as usize).min(buf.len())]);
        super::file_name(&path).to_string()
    }

    /// The exe name from the process list, for a process that cannot be
    /// opened (one running as administrator, say).
    fn listed_name(pid: u32) -> String {
        use windows::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
            TH32CS_SNAPPROCESS,
        };
        let mut name = String::new();
        // SAFETY: the snapshot is closed below; the entry carries its size.
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return name;
            };
            let mut e = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut e).is_ok() {
                loop {
                    if e.th32ProcessID == pid {
                        let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                        name = String::from_utf16_lossy(&e.szExeFile[..len]);
                        break;
                    }
                    if Process32NextW(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = windows::Win32::Foundation::CloseHandle(snap);
        }
        name
    }

    /// The exe file name of the app that owns `hwnd` ("1Password.exe"), or
    /// empty when there is no such window or its program cannot be told.
    pub fn exe_of(hwnd: isize) -> String {
        if hwnd == 0 {
            return String::new();
        }
        let pid = app_pid(hwnd);
        if pid == 0 {
            return String::new();
        }
        let name = image_name(pid);
        if name.is_empty() {
            listed_name(pid)
        } else {
            name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_name_is_kept_as_a_lower_case_exe() {
        assert_eq!(normalize("Notepad").as_deref(), Some("notepad.exe"));
        assert_eq!(normalize("NOTEPAD.EXE").as_deref(), Some("notepad.exe"));
        assert_eq!(normalize("  notepad.exe  ").as_deref(), Some("notepad.exe"));
        assert_eq!(normalize("1Password").as_deref(), Some("1password.exe"));
        // A name with a dot in it is still a name, not an extension.
        assert_eq!(normalize("my.app").as_deref(), Some("my.app.exe"));
    }

    #[test]
    fn a_path_is_kept_as_its_file_name() {
        let path = r"C:\Program Files\KeePassXC\KeePassXC.exe";
        assert_eq!(normalize(path).as_deref(), Some("keepassxc.exe"));
        let quoted = r#""C:\Windows\System32\notepad.exe""#;
        assert_eq!(normalize(quoted).as_deref(), Some("notepad.exe"));
        let slashes = "C:/Tools/Bitwarden/Bitwarden.exe";
        assert_eq!(normalize(slashes).as_deref(), Some("bitwarden.exe"));
    }

    #[test]
    fn what_is_not_a_file_name_is_refused() {
        for bad in ["", "   ", ".exe", r"C:\Tools\", "a*b", "what?", "\"\""] {
            assert_eq!(normalize(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn matching_ignores_case_extension_and_folder() {
        let list = names(&["notepad.exe", "1password.exe"]);
        // As typed, as the desktop helper names a process, and as Windows
        // gives a running program's path.
        for hit in [
            "notepad.exe",
            "Notepad.exe",
            "NOTEPAD",
            "notepad",
            r"C:\Windows\System32\notepad.exe",
            r"C:\Users\me\AppData\Local\1Password\app\8\1Password.exe",
        ] {
            assert!(is_listed(&list, hit), "{hit}");
        }
        for miss in [
            "notepad2.exe",
            "wordpad",
            "password.exe",
            "",
            r"C:\notepad\calc.exe",
        ] {
            assert!(!is_listed(&list, miss), "{miss}");
        }
    }

    #[test]
    fn a_missing_list_is_the_defaults_and_a_stored_one_is_the_persons() {
        assert_eq!(list_from(None), names(&DEFAULTS));
        assert_eq!(list_from(Some(&Value::Null)), names(&DEFAULTS));
        assert_eq!(list_from(Some(&json!("notepad"))), names(&DEFAULTS));
        // Removing every app leaves an empty list, not the defaults again.
        assert!(list_from(Some(&json!([]))).is_empty());
        // A file edited by hand: names are tidied, repeats and junk dropped.
        let stored = json!(["Notepad", "notepad.exe", 5, "  ", r"C:\x\Calc.EXE"]);
        let want = names(&["notepad.exe", "calc.exe"]);
        assert_eq!(list_from(Some(&stored)), want);
    }

    #[test]
    fn the_defaults_are_kept_names_each_with_a_product_name() {
        for exe in DEFAULTS {
            assert_eq!(normalize(exe).as_deref(), Some(exe));
            assert!(NAMES.iter().any(|(e, _)| *e == exe), "{exe}");
        }
        let mut unique = DEFAULTS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), DEFAULTS.len());
    }

    #[test]
    fn apps_are_named_as_people_know_them() {
        assert_eq!(display_name("1password.exe"), "1Password");
        assert_eq!(display_name("SecHealthUI.exe"), "Windows Security");
        assert_eq!(display_name("notepad.exe"), "Notepad");
        assert_eq!(display_name(r"C:\Tools\MyBank.exe"), "MyBank");
        assert_eq!(display_name("vault"), "Vault");
    }

    #[test]
    fn a_listed_app_is_refused_in_one_plain_line() {
        let list = names(&["notepad.exe"]);
        let no = verdict(&list, "Notepad.exe").unwrap_err();
        let line = "Notepad is on your private apps list, so the screen was not read.";
        assert_eq!(no.message(), line);
        assert_eq!(verdict(&list, "calc.exe"), Ok(()));
        // No window, or a program that could not be named: nothing to refuse.
        assert_eq!(verdict(&list, ""), Ok(()));
        let helper: DesktopError = no.clone().into();
        assert_eq!(
            (helper.code.as_str(), helper.message.as_str()),
            (CODE, line)
        );
        let seam: DeskError = no.into();
        assert_eq!((seam.code.as_str(), seam.message.as_str()), (CODE, line));
    }

    #[test]
    fn with_no_settings_open_nothing_is_refused() {
        if crate::store::try_get().is_none() {
            assert!(list().is_empty());
            assert_eq!(check(Read::Screen), Ok(()));
        }
    }

    #[test]
    fn helper_ops_are_checked_by_what_they_touch() {
        let none: Vec<Read> = Vec::new();
        // A text read of a named window, or of the one in front.
        let named = json!({ "windowHandle": 4660 });
        assert_eq!(op_reads("getText", &named), [Read::Window(4660)]);
        assert_eq!(op_reads("readTree", &json!({})), [Read::Screen]);
        let null = json!({ "windowHandle": null });
        assert_eq!(op_reads("docText", &null), [Read::Screen]);
        assert_eq!(
            op_reads("findText", &json!({ "windowHandle": "w1234" })),
            [Read::Window(0x1234)]
        );
        // Pixels are the screen's, and the named window's too.
        assert_eq!(
            op_reads("capture", &json!({ "rect": [0, 0, 9, 9] })),
            [Read::Screen]
        );
        assert_eq!(op_reads("ocr", &named), [Read::Screen, Read::Window(4660)]);
        // The pointer and the keyboard.
        let at = json!({ "x": 10.4, "y": 20.6 });
        assert_eq!(op_reads("elementAt", &at), [Read::Point(10, 21)]);
        assert_eq!(op_reads("click", &at), [Read::Point(10, 21)]);
        let drag = json!({ "from": { "x": 1, "y": 2 }, "to": { "x": 3, "y": 4 } });
        assert_eq!(
            op_reads("drag", &drag),
            [Read::Point(1, 2), Read::Point(3, 4)]
        );
        assert_eq!(
            op_reads("key", &json!({ "keys": "ctrl+c" })),
            [Read::Screen]
        );
        assert_eq!(op_reads("type", &named), [Read::Window(4660)]);
        // A ref was made by a read that was checked.
        let by_ref = json!({ "ref": "ref_3", "windowHandle": null });
        assert_eq!(op_reads("click", &by_ref), none);
        assert_eq!(op_reads("type", &by_ref), none);
        assert_eq!(op_reads("readTree", &by_ref), none);
        // Nothing of an app's is read by these.
        for op in [
            "listWindows",
            "foregroundWindow",
            "monitors",
            "ping",
            "config",
        ] {
            assert_eq!(op_reads(op, &named), none, "{op}");
        }
    }

    #[test]
    fn only_a_read_ops_answer_is_checked() {
        // No list is open in a unit test, so this is the shape alone: an
        // answer with no window, or from an op that reads nothing, passes.
        assert_eq!(check_reply("getText", &json!({ "text": "x" })), Ok(()));
        assert_eq!(
            check_reply("click", &json!({ "windowHandle": "w1" })),
            Ok(())
        );
        assert!(reads_front("getText") && reads_front("capture"));
        assert!(!reads_front("key") && !reads_front("focused"));
    }
}

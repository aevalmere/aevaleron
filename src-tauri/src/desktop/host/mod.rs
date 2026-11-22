//! The desktop engine, in process: a port of lib/desktop/host.cs (the C#
//! helper that used to run inside a powershell.exe child). Same ops, same
//! request and reply shapes, same error codes, same coordinates (physical
//! virtual-screen pixels), so nothing above `client.rs` changed.
//!
//! Threading is host.cs's: one worker thread takes requests in order, and
//! each request runs on its own MTA thread, so an app that hangs answering
//! UI Automation costs one request (a TIMEOUT), not the engine. A request
//! that timed out or was cancelled never sends input afterwards: every input
//! primitive checks its request's flag first. UI Automation never runs on
//! the UI thread: a UIA call into one of our own windows from there would
//! wait on the thread that has to answer it.

mod input;
mod menus;
mod pixels;
mod uia;
mod watch;
mod wins;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use serde_json::{json, Value};

use super::protocol::{DResult, DesktopError};

pub const PROTOCOL: i64 = 2;

pub(crate) fn err(code: &str, message: impl Into<String>) -> DesktopError {
    DesktopError::new(code, message)
}

/// host.cs's CodeOf for COM failures: a gone element is a stale ref, a
/// disabled one says so, access denied is a process above us, and anything
/// else UI Automation threw is UIA_ERROR.
pub(crate) fn code_of(hr: u32) -> &'static str {
    match hr {
        0x8004_0201 => "STALE_REF",
        0x8004_0200 => "DISABLED",
        0x8007_0005 => "ELEVATED_TARGET",
        0x8007_0057 => "BAD_ARGS",
        _ => "UIA_ERROR",
    }
}

impl From<windows::core::Error> for DesktopError {
    fn from(e: windows::core::Error) -> DesktopError {
        let hr = e.code().0 as u32;
        let msg = e.message();
        let msg = if msg.trim().is_empty() {
            format!("UI Automation call failed (0x{hr:08X})")
        } else {
            msg
        };
        DesktopError::new(code_of(hr), msg)
    }
}

/// UIA_E_ELEMENTNOTAVAILABLE: the element is gone.
pub(crate) fn is_stale(e: &windows::core::Error) -> bool {
    e.code().0 as u32 == 0x8004_0201
}

// ------------------------------------------------------------ cancel

/// A request's cancel flag (B-050). Set when the request times out, and by
/// `cancel` on every request running, so an abandoned or stopped request
/// never sends input afterwards.
#[derive(Default)]
pub struct Req {
    cancelled: AtomicBool,
}

impl Req {
    /// True when this call is the one that cancelled it.
    fn cancel(&self) -> bool {
        !self.cancelled.swap(true, Ordering::SeqCst)
    }
}

static RUNNING: Mutex<Vec<Arc<Req>>> = Mutex::new(Vec::new());

thread_local! {
    static CURRENT: RefCell<Option<Arc<Req>>> = const { RefCell::new(None) };
}

fn current_req() -> Option<Arc<Req>> {
    CURRENT.with(|c| c.borrow().clone())
}

pub(crate) fn check_cancel() -> DResult<()> {
    let stopped = CURRENT.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|r| r.cancelled.load(Ordering::SeqCst))
    });
    if stopped {
        Err(err("CANCELLED", "stopped before it acted"))
    } else {
        Ok(())
    }
}

/// Stops every request running from sending any more input; answers how
/// many were still live.
pub fn cancel_all() -> usize {
    let running = RUNNING.lock();
    let mut n = 0;
    for r in running.iter() {
        if r.cancel() {
            n += 1;
        }
    }
    n
}

// ------------------------------------------------------------ threads

/// Per-monitor v2 for work on this thread, whatever the process chose.
pub(crate) fn thread_dpi() {
    use windows_sys::Win32::UI::HiDpi::{
        SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Joins the MTA. The MTA itself is held open for the life of the process,
/// so the cached IUIAutomation never outlives its apartment.
fn com_mta() {
    use windows_sys::Win32::System::Com::{
        CoIncrementMTAUsage, CoInitializeEx, COINIT_MULTITHREADED,
    };
    static HELD: OnceLock<()> = OnceLock::new();
    HELD.get_or_init(|| unsafe {
        let mut cookie = std::ptr::null_mut();
        let _ = CoIncrementMTAUsage(&mut cookie);
    });
    unsafe {
        let _ = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as _);
    }
}

/// Runs `f` on a fresh MTA thread that carries `req`'s cancel flag. The
/// receiver gets `None` when `f` panicked (logged by crash::guard).
pub(crate) fn spawn_op<T: Send + 'static>(
    req: Option<Arc<Req>>,
    f: impl FnOnce() -> T + Send + 'static,
) -> mpsc::Receiver<Option<T>> {
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("desktop-op".into())
        .spawn(move || {
            CURRENT.with(|c| *c.borrow_mut() = req);
            thread_dpi();
            com_mta();
            let out = crate::crash::guard("desktop op", f);
            let _ = tx.send(out);
        });
    rx
}

// ------------------------------------------------------------ worker

/// One request, as client.rs queues it. Its cancel flag is live from the
/// moment it is queued, so `cancel` stops requests still waiting their turn
/// too, as host.cs's did for every line read so far.
pub struct Job {
    pub id: u64,
    op: String,
    args: Value,
    timeout_ms: u64,
    req: Arc<Req>,
}

impl Job {
    pub fn new(id: u64, op: &str, args: Value, timeout_ms: u64) -> Job {
        let req = Arc::new(Req::default());
        RUNNING.lock().push(req.clone());
        Job {
            id,
            op: op.to_string(),
            args,
            timeout_ms,
            req,
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        let req = self.req.clone();
        RUNNING.lock().retain(|r| !Arc::ptr_eq(r, &req));
    }
}

pub enum Outcome {
    Reply(DResult<Value>),
    /// The op panicked: the worker ends, as the helper process did when it
    /// crashed, and the client starts a fresh one.
    Crashed(String),
}

/// The worker loop: one request at a time, in order, each bounded by its
/// timeout. Ends when the client drops the sender or an op panics.
pub fn serve(jobs: mpsc::Receiver<Job>, reply: impl Fn(u64, Outcome)) {
    thread_dpi();
    com_mta();
    for mut job in jobs {
        let args = std::mem::take(&mut job.args);
        let out = run_one(job.op.clone(), args, job.timeout_ms, job.req.clone());
        let id = job.id;
        drop(job);
        let crashed = matches!(out, Outcome::Crashed(_));
        reply(id, out);
        if crashed {
            return;
        }
    }
}

fn run_one(op: String, args: Value, timeout_ms: u64, req: Arc<Req>) -> Outcome {
    let timeout = Duration::from_millis(timeout_ms.max(100));
    let t0 = Instant::now();
    let name = op.clone();
    let rx = spawn_op(Some(req.clone()), move || {
        let args = if args.is_object() { args } else { json!({}) };
        dispatch(&name, args)
    });
    let out = match rx.recv_timeout(timeout) {
        Ok(Some(r)) => Outcome::Reply(r),
        Ok(None) => Outcome::Crashed(format!("{op} panicked")),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // The thread keeps running, but it may no longer act: the
            // caller is told this request failed.
            req.cancel();
            Outcome::Reply(Err(err(
                "TIMEOUT",
                format!(
                    "{op} did not finish in {}ms (the target app may be hung)",
                    timeout.as_millis()
                ),
            )))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Outcome::Crashed(format!("{op} ended without an answer"))
        }
    };
    note_ms(&op, t0.elapsed().as_millis() as u64);
    out
}

/// The last and slowest time of each op, for ping and the selfcheck.
static OP_MS: Mutex<Option<HashMap<String, (u64, u64, u64)>>> = Mutex::new(None);

fn note_ms(op: &str, ms: u64) {
    let mut g = OP_MS.lock();
    let map = g.get_or_insert_with(HashMap::new);
    let e = map.entry(op.to_string()).or_insert((0, 0, 0));
    e.0 = ms;
    e.1 = e.1.max(ms);
    e.2 += 1;
}

fn op_ms() -> Value {
    let g = OP_MS.lock();
    let mut out = serde_json::Map::new();
    if let Some(map) = g.as_ref() {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for k in keys {
            let (last, max, n) = map[k];
            out.insert(k.clone(), json!({ "last": last, "max": max, "n": n }));
        }
    }
    Value::Object(out)
}

fn dispatch(op: &str, a: Value) -> DResult<Value> {
    match op {
        "ping" => Ok(ping()),
        "config" => config(&a),
        "listWindows" => wins::list_windows(&a),
        "foregroundWindow" => wins::foreground_window(),
        "monitors" => Ok(wins::monitors()),
        "focusWindow" => wins::focus_cmd(&a),
        "closeWindow" => wins::close_cmd(&a),
        "windowStates" => wins::window_states(&a),
        "readTree" => uia::tree(&a),
        "getText" => uia::get_text(a),
        "elementAt" => uia::at(&a),
        "resolve" => uia::resolve(&a),
        "focused" => uia::focused_cmd(&a),
        "findText" => uia::find_text_cmd(a),
        "docText" => uia::doc_text_cmd(a),
        "scrollToText" => uia::scroll_to_text(&a),
        "selectText" => uia::select_text(&a),
        "selection" => uia::selection_cmd(&a),
        "menuItems" => menus::menu_items(&a),
        "menuInvoke" => menus::menu_invoke(&a),
        "ocr" => pixels::ocr_cmd(&a),
        "capture" => pixels::capture_cmd(&a),
        "click" => input::click(&a),
        "hover" => input::hover(&a),
        "drag" => input::drag(&a),
        "type" => input::type_cmd(&a),
        "key" => input::key_cmd(&a),
        "scroll" => input::scroll_cmd(&a),
        "scrollTo" => input::scroll_to(&a),
        "invoke" => input::invoke_cmd(&a),
        "setValue" => input::set_value_cmd(&a),
        "restoreCursor" => Ok(input::restore_cursor()),
        "watchInput" => watch::set(bool_arg(&a, "on", true)),
        "testUserInput" => {
            watch::emit(&str_arg(&a, "kind", "key"), 0, 0, 0.0, true);
            Ok(json!(true))
        }
        "cancel" => Ok(json!({ "ok": true, "cancelled": cancel_all() })),
        _ => Err(err("BAD_ARGS", format!("unknown op: {op}"))),
    }
}

// ------------------------------------------------------------ lifecycle

/// The `ready` event a fresh helper printed.
pub fn ready_event() -> Value {
    json!({
        "event": "ready",
        "protocol": PROTOCOL,
        "pid": std::process::id(),
        "dpi": dpi_mode(),
        "integrity": wins::self_integrity(),
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// What a new helper process started with: no refs, no watched windows, no
/// saved cursor, no input watch. The UI Automation instance is kept.
pub fn reset() {
    watch::stop();
    uia::reset();
    input::forget_cursor();
}

/// Takes the input hooks down, as the helper's exit did.
pub fn stop_watch() {
    watch::stop();
}

/// The input watch's events go here (`user-input` lines).
type Sink = Arc<dyn Fn(Value) + Send + Sync>;
static SINK: RwLock<Option<Sink>> = RwLock::new(None);

pub fn set_sink(f: Sink) {
    *SINK.write() = Some(f);
}

pub(crate) fn emit_event(v: Value) {
    let sink = SINK.read().clone();
    if let Some(f) = sink {
        f(v);
    }
}

fn dpi_mode() -> &'static str {
    use windows_sys::Win32::UI::HiDpi::{
        AreDpiAwarenessContextsEqual, GetDpiAwarenessContextForProcess,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    let v2 = DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2;
    unsafe {
        let me = GetDpiAwarenessContextForProcess(std::ptr::null_mut());
        if AreDpiAwarenessContextsEqual(me, v2) != 0 {
            "per-monitor-v2"
        } else {
            // Every request thread sets it for itself.
            "per-monitor-v2 (thread)"
        }
    }
}

fn ping() -> Value {
    json!({
        "protocol": PROTOCOL,
        "pid": std::process::id(),
        "dpi": dpi_mode(),
        "version": env!("CARGO_PKG_VERSION"),
        "watching": watch::on(),
        "refs": uia::ref_count(),
        "inProcess": true,
        "opMs": op_ms(),
    })
}

// ------------------------------------------------------------ own windows

#[derive(Clone, Copy)]
pub(crate) struct OwnWin {
    pub handle: isize,
    pub block: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Own {
    pub pids: Vec<u32>,
    pub wins: Vec<OwnWin>,
}

static OWN: RwLock<Own> = RwLock::new(Own {
    pids: Vec::new(),
    wins: Vec::new(),
});

pub(crate) fn own() -> Own {
    let mut o = OWN.read().clone();
    let me = std::process::id();
    if !o.pids.contains(&me) {
        o.pids.push(me);
    }
    o
}

fn config(a: &Value) -> DResult<Value> {
    let me = std::process::id();
    let mut pids = vec![me];
    if let Some(list) = a.get("pids").and_then(Value::as_array) {
        for p in list.iter().filter(|p| !p.is_null()) {
            let p = to_int(p)? as u32;
            if !pids.contains(&p) {
                pids.push(p);
            }
        }
    }
    let mut wins = Vec::new();
    if let Some(list) = a.get("windows").and_then(Value::as_array) {
        for w in list {
            let ow = if w.is_object() {
                OwnWin {
                    handle: parse_handle(arg(w, "handle"))?,
                    block: bool_arg(w, "block", true),
                }
            } else {
                OwnWin {
                    handle: parse_handle(Some(w))?,
                    block: true,
                }
            };
            if ow.handle != 0 {
                wins.push(ow);
            }
        }
    }
    let r = json!({ "pids": pids.len(), "windows": wins.len() });
    *OWN.write() = Own { pids, wins };
    Ok(r)
}

// ------------------------------------------------------------ args

/// A present, non-null argument.
pub(crate) fn arg<'a>(a: &'a Value, k: &str) -> Option<&'a Value> {
    a.get(k).filter(|v| !v.is_null())
}

/// Convert.ToString, as host.cs read string arguments.
pub(crate) fn to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn str_arg(a: &Value, k: &str, def: &str) -> String {
    arg(a, k).map(to_text).unwrap_or_else(|| def.to_string())
}

/// Convert.ToInt32: numbers round, text parses, anything else is BAD_ARGS.
pub(crate) fn to_int(v: &Value) -> DResult<i32> {
    let bad = || err("BAD_ARGS", format!("not a number: {v}"));
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i32::try_from(i).map_err(|_| bad())
            } else {
                let f = n.as_f64().ok_or_else(bad)?.round_ties_even();
                if f.is_finite() && f.abs() <= i32::MAX as f64 {
                    Ok(f as i32)
                } else {
                    Err(bad())
                }
            }
        }
        Value::String(s) => s.trim().parse::<i32>().map_err(|_| bad()),
        Value::Bool(b) => Ok(i32::from(*b)),
        _ => Err(bad()),
    }
}

pub(crate) fn int_arg(a: &Value, k: &str, def: i32) -> DResult<i32> {
    match arg(a, k) {
        None => Ok(def),
        Some(v) => to_int(v),
    }
}

pub(crate) fn bool_arg(a: &Value, k: &str, def: bool) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(def)
}

pub(crate) fn require_int(a: &Value, k: &str) -> DResult<i32> {
    match arg(a, k) {
        None => Err(err("BAD_ARGS", format!("missing {k}"))),
        Some(v) => to_int(v),
    }
}

/// Handles travel as 'w' + hex ("w1A2B3C"), the shape listWindows returns;
/// a bare number is accepted too.
pub(crate) fn parse_handle(v: Option<&Value>) -> DResult<isize> {
    let bad = |s: &str| err("BAD_ARGS", format!("not a window handle: {s}"));
    let Some(v) = v else { return Ok(0) };
    match v {
        Value::Null => Ok(0),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(|f| f as i64))
            .map(|i| i as isize)
            .ok_or_else(|| bad(&n.to_string())),
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return Ok(0);
            }
            let parsed = if let Some(hex) = s.strip_prefix(['w', 'W']) {
                i64::from_str_radix(hex, 16).ok()
            } else if let Some(hex) = s.strip_prefix("0x") {
                i64::from_str_radix(hex, 16).ok()
            } else {
                s.parse::<i64>().ok()
            };
            parsed.map(|i| i as isize).ok_or_else(|| bad(s))
        }
        other => Err(bad(&other.to_string())),
    }
}

pub(crate) fn handle_str(h: isize) -> String {
    format!("w{:X}", h as i64)
}

pub(crate) fn ref_str(r: i32) -> String {
    format!("ref_{r}")
}

pub(crate) fn parse_ref(v: Option<&Value>) -> DResult<i32> {
    let Some(v) = v else { return Ok(0) };
    match v {
        Value::String(s) => {
            let s = s.strip_prefix("ref_").unwrap_or(s);
            s.parse::<i32>()
                .map_err(|_| err("BAD_ARGS", "refs look like ref_12"))
        }
        other => to_int(other),
    }
}

pub(crate) type Rect = [i32; 4];

pub(crate) fn rect_arg(a: &Value, k: &str) -> DResult<Option<Rect>> {
    let Some(o) = arg(a, k) else { return Ok(None) };
    if let Some(arr) = o.as_array() {
        if arr.len() == 4 {
            return Ok(Some([
                to_int(&arr[0])?,
                to_int(&arr[1])?,
                to_int(&arr[2])?,
                to_int(&arr[3])?,
            ]));
        }
    }
    if o.is_object() {
        let w = if o.get("width").is_some() {
            int_arg(o, "width", 0)?
        } else {
            int_arg(o, "w", 0)?
        };
        let h = if o.get("height").is_some() {
            int_arg(o, "height", 0)?
        } else {
            int_arg(o, "h", 0)?
        };
        return Ok(Some([int_arg(o, "x", 0)?, int_arg(o, "y", 0)?, w, h]));
    }
    Err(err(
        "BAD_ARGS",
        format!("{k} must be [x, y, w, h] or {{x, y, width, height}}"),
    ))
}

pub(crate) fn rect_json(r: Option<Rect>) -> Value {
    match r {
        Some(r) => json!(r),
        None => Value::Null,
    }
}

pub(crate) fn intersects(a: Option<Rect>, b: Option<Rect>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
        }
        _ => true,
    }
}

/// Length in UTF-16 units, as C# counted.
pub(crate) fn u16len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// The first `n` UTF-16 units of `s`, never splitting a character.
pub(crate) fn take_u16(s: &str, n: usize) -> String {
    let mut used = 0;
    let mut out = String::new();
    for c in s.chars() {
        let l = c.len_utf16();
        if used + l > n {
            break;
        }
        used += l;
        out.push(c);
    }
    out
}

/// At most `max` UTF-16 units, an ellipsis marking a cut.
pub(crate) fn clip(s: &str, max: usize) -> String {
    if u16len(s) <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", take_u16(s, max))
    }
}

pub(crate) fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_parse_every_form() {
        assert_eq!(parse_handle(Some(&json!("w1A2B"))).unwrap(), 0x1a2b);
        assert_eq!(parse_handle(Some(&json!("0x10"))).unwrap(), 16);
        assert_eq!(parse_handle(Some(&json!(42))).unwrap(), 42);
        assert_eq!(parse_handle(Some(&json!(""))).unwrap(), 0);
        assert_eq!(parse_handle(None).unwrap(), 0);
        assert_eq!(parse_handle(Some(&json!("nope"))).unwrap_err().code, "BAD_ARGS");
        assert_eq!(handle_str(0x1a2b), "w1A2B");
    }

    #[test]
    fn ints_convert_as_dotnet() {
        assert_eq!(to_int(&json!(2.5)).unwrap(), 2);
        assert_eq!(to_int(&json!(3.5)).unwrap(), 4);
        assert_eq!(to_int(&json!(-2.5)).unwrap(), -2);
        assert_eq!(to_int(&json!("12")).unwrap(), 12);
        assert_eq!(to_int(&json!(true)).unwrap(), 1);
        assert!(to_int(&json!([1])).is_err());
    }

    #[test]
    fn refs_and_rects() {
        assert_eq!(parse_ref(Some(&json!("ref_12"))).unwrap(), 12);
        assert_eq!(parse_ref(Some(&json!(7))).unwrap(), 7);
        assert_eq!(parse_ref(Some(&json!("x"))).unwrap_err().code, "BAD_ARGS");
        let a = json!({ "r": [1, 2, 3, 4], "o": { "x": 1, "y": 2, "w": 3, "h": 4 }, "bad": 5 });
        assert_eq!(rect_arg(&a, "r").unwrap(), Some([1, 2, 3, 4]));
        assert_eq!(rect_arg(&a, "o").unwrap(), Some([1, 2, 3, 4]));
        assert_eq!(rect_arg(&a, "none").unwrap(), None);
        assert_eq!(rect_arg(&a, "bad").unwrap_err().code, "BAD_ARGS");
    }

    #[test]
    fn clipping_counts_utf16() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 3), "abc\u{2026}");
        assert_eq!(take_u16("a\u{1F600}b", 2), "a");
        assert_eq!(u16len("a\u{1F600}"), 3);
    }

    #[test]
    fn com_errors_map_to_desktop_codes() {
        assert_eq!(code_of(0x8004_0201), "STALE_REF");
        assert_eq!(code_of(0x8007_0005), "ELEVATED_TARGET");
        assert_eq!(code_of(0x8000_4005), "UIA_ERROR");
    }

    #[test]
    fn cancel_marks_running_requests_once() {
        let r = Arc::new(Req::default());
        RUNNING.lock().push(r.clone());
        assert!(cancel_all() >= 1);
        assert!(r.cancelled.load(Ordering::SeqCst));
        RUNNING.lock().retain(|x| !Arc::ptr_eq(x, &r));
    }

    #[test]
    fn unknown_op_is_bad_args() {
        let e = dispatch("nope", json!({})).unwrap_err();
        assert_eq!(e.code, "BAD_ARGS");
    }

    #[test]
    fn worker_answers_in_order_and_times_out() {
        let (tx, rx) = mpsc::channel();
        let (out_tx, out_rx) = mpsc::channel();
        let t = std::thread::spawn(move || {
            serve(rx, move |id, o| {
                let r = match o {
                    Outcome::Reply(r) => r,
                    Outcome::Crashed(m) => Err(err("HELPER_DOWN", m)),
                };
                let _ = out_tx.send((id, r));
            })
        });
        for (id, op) in [(1, "ping"), (2, "nope")] {
            tx.send(Job::new(id, op, json!({}), 4000)).unwrap();
        }
        drop(tx);
        let (a, ra) = out_rx.recv().unwrap();
        let (b, rb) = out_rx.recv().unwrap();
        assert_eq!((a, b), (1, 2));
        assert_eq!(ra.unwrap()["protocol"], json!(PROTOCOL));
        assert_eq!(rb.unwrap_err().code, "BAD_ARGS");
        t.join().unwrap();
    }
}

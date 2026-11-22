//! Physical input watch (host.cs Watch). Low-level hooks see every mouse and
//! keyboard event with a flag saying whether it was injected; anything
//! without it came from the person, and the agent pauses on it. The hook
//! callback only queues: Windows silently unhooks a callback that stalls.
//! Key identity is never reported, only that a key went down.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use serde_json::{json, Value};
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};

use super::{emit_event, err, DResult};

struct Hooks {
    thread: JoinHandle<()>,
    thread_id: u32,
}

static HOOKS: Mutex<Option<Hooks>> = Mutex::new(None);

pub(crate) fn on() -> bool {
    HOOKS.lock().is_some()
}

/// The hook state. Only the hook thread touches it, from its callbacks.
struct Track {
    last_move: u32,
    last_key: u32,
    last_button: u32,
    last: Option<(i32, i32)>,
    travelled: f64,
}

static TRACK: Mutex<Track> = Mutex::new(Track {
    last_move: 0,
    last_key: 0,
    last_button: 0,
    last: None,
    travelled: 0.0,
});

static PENDING: Mutex<VecDeque<Value>> = Mutex::new(VecDeque::new());
static SIGNAL: Condvar = Condvar::new();
static DRAIN: Mutex<bool> = Mutex::new(false);

fn tick() -> u32 {
    unsafe { GetTickCount() }
}

/// One `user-input` event, queued for the drain thread.
pub(crate) fn emit(kind: &str, x: i32, y: i32, dist: f64, test: bool) {
    let mut ev = json!({ "event": "user-input", "kind": kind });
    if kind != "key" {
        ev["x"] = json!(x);
        ev["y"] = json!(y);
    }
    if dist > 0.0 {
        ev["distance"] = json!(dist.round());
    }
    if test {
        ev["test"] = json!(true);
    }
    ev["t"] = json!(tick() as i32);
    let mut q = PENDING.lock();
    if q.len() < 64 {
        q.push_back(ev);
    }
    SIGNAL.notify_one();
    drop(q);
    start_drain();
}

fn start_drain() {
    let mut started = DRAIN.lock();
    if *started {
        return;
    }
    *started = true;
    let _ = std::thread::Builder::new()
        .name("desktop-input-drain".into())
        .spawn(|| loop {
            let ev = {
                let mut q = PENDING.lock();
                while q.is_empty() {
                    SIGNAL.wait(&mut q);
                }
                q.pop_front()
            };
            if let Some(ev) = ev {
                let _ = crate::crash::guard("desktop input event", || emit_event(ev));
            }
        });
}

unsafe extern "system" fn mouse_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && l != 0 {
        let s = &*(l as *const MSLLHOOKSTRUCT);
        // LLMHF_INJECTED and LLMHF_LOWER_IL_INJECTED.
        if s.flags & 0x3 == 0 {
            let msg = w as u32;
            let now = tick();
            let mut t = TRACK.lock();
            if msg == 0x200 {
                if let Some((lx, ly)) = t.last {
                    let dx = f64::from(s.pt.x - lx);
                    let dy = f64::from(s.pt.y - ly);
                    t.travelled += (dx * dx + dy * dy).sqrt();
                }
                t.last = Some((s.pt.x, s.pt.y));
                if now.wrapping_sub(t.last_move) >= 100 {
                    let dist = t.travelled;
                    t.travelled = 0.0;
                    t.last_move = now;
                    drop(t);
                    emit("mouse-move", s.pt.x, s.pt.y, dist, false);
                }
            } else if now.wrapping_sub(t.last_button) >= 50 {
                t.last_button = now;
                drop(t);
                let kind = if msg == 0x20A || msg == 0x20E {
                    "wheel"
                } else {
                    "mouse-button"
                };
                emit(kind, s.pt.x, s.pt.y, 0.0, false);
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, w, l)
}

unsafe extern "system" fn key_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && l != 0 {
        let s = &*(l as *const KBDLLHOOKSTRUCT);
        let msg = w as u32;
        // LLKHF_INJECTED and LLKHF_LOWER_IL_INJECTED; key downs only.
        if s.flags & 0x12 == 0 && (msg == 0x100 || msg == 0x104) {
            let now = tick();
            let mut t = TRACK.lock();
            if now.wrapping_sub(t.last_key) >= 100 {
                t.last_key = now;
                drop(t);
                emit("key", 0, 0, 0.0, false);
            }
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, w, l)
}

/// The hook thread: installs both hooks and pumps messages until WM_QUIT.
fn hook_thread(ready: mpsc::Sender<Result<u32, String>>, id: &AtomicU32) {
    unsafe {
        let tid = GetCurrentThreadId();
        id.store(tid, Ordering::SeqCst);
        super::thread_dpi();
        let module = GetModuleHandleW(std::ptr::null());
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
        let key = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), module, 0);
        if mouse.is_null() || key.is_null() {
            let code = std::io::Error::last_os_error();
            if !mouse.is_null() {
                UnhookWindowsHookEx(mouse);
            }
            if !key.is_null() {
                UnhookWindowsHookEx(key);
            }
            let _ = ready.send(Err(format!("SetWindowsHookEx failed: {code}")));
            return;
        }
        let _ = ready.send(Ok(tid));
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {}
        UnhookWindowsHookEx(mouse);
        UnhookWindowsHookEx(key);
    }
}

pub(crate) fn set(on: bool) -> DResult<Value> {
    if !on {
        stop();
        return Ok(json!({ "watching": false }));
    }
    let mut g = HOOKS.lock();
    if g.is_none() {
        static ID: AtomicU32 = AtomicU32::new(0);
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("desktop-input-hooks".into())
            .spawn(move || hook_thread(tx, &ID))
            .map_err(|e| err("INTERNAL", format!("hook thread: {e}")))?;
        match rx.recv_timeout(Duration::from_millis(3000)) {
            Ok(Ok(thread_id)) => {
                *TRACK.lock() = Track {
                    last_move: 0,
                    last_key: 0,
                    last_button: 0,
                    last: None,
                    travelled: 0.0,
                };
                *g = Some(Hooks { thread, thread_id });
                start_drain();
            }
            Ok(Err(e)) => return Err(err("INTERNAL", e)),
            Err(_) => {
                let tid = ID.load(Ordering::SeqCst);
                if tid != 0 {
                    unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
                }
                return Err(err("INTERNAL", "the input hooks did not start"));
            }
        }
    }
    Ok(json!({ "watching": g.is_some() }))
}

pub(crate) fn stop() {
    let Some(h) = HOOKS.lock().take() else {
        return;
    };
    unsafe { PostThreadMessageW(h.thread_id, WM_QUIT, 0, 0) };
    // Joined with a bound, as host.cs's Join(1000): a hook thread that does
    // not answer is left to end on its own.
    let (tx, rx) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("desktop-input-unhook".into())
        .spawn(move || {
            let _ = h.thread.join();
            let _ = tx.send(());
        });
    let _ = rx.recv_timeout(Duration::from_millis(1000));
}


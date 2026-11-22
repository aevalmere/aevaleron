//! The harness's own labels (`tools/smoke-labels/harness.txt`): the version,
//! the scaffold pages reaching Rust, and the probe API checked end to end, so
//! a FAIL in another lane is about that lane's code and not the harness.
//! Then the desktop selfcheck (main.js:3833), whose labels the capture lane
//! lists. After every lane, [`footprint`]: what the one native process
//! costs at rest (lane native-ui).

use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::CloseHandle;
use ::windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::Foundation::{FILETIME, HANDLE};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

use crate::desktop::{self, protocol, selfcheck};
use crate::selftest::{input, probes, Cx, Input};
use crate::shell::{AppHandle, Manager};
use crate::{capture, ipc, paths, windows};

pub fn run(cx: &mut Cx) {
    let app = cx.app.clone();
    let want: Vec<&str> = windows::SCAFFOLD_WINDOWS.iter().map(|(l, _)| *l).collect();
    let all_seen = || {
        let seen = app.state::<ipc::Seen>().snapshot();
        want.iter().all(|w| seen.contains_key(*w))
    };
    let ready = cx.wait_for(Duration::from_secs(30), all_seen);
    cx.log("version", &app.package_info().version);
    cx.log("userData", paths::user_data_dir().display());
    let seen = app.state::<ipc::Seen>().snapshot();
    let pages: Vec<&str> = seen.keys().map(String::as_str).collect();
    cx.log("scaffold.pages", pages.join(","));
    cx.must("scaffold.bridge", ready, "a page never reached Rust");

    // The overlay's page answers on the UI thread; a window that is not
    // there is an error, not a quiet null.
    let shown = cx.probe(windows::OVERLAY, "visible");
    let missing = cx.probe("no-such-window", "text");
    let ok = shown.as_ref().is_ok_and(|v| v.is_boolean()) && missing.is_err();
    cx.must("selftest.probe", ok, format!("{shown:?} {missing:?}"));
    // SendInput reaches the real pointer.
    let to = (200, 200);
    let sent = cx.input(Input::Move(to.0, to.1));
    let at = input::cursor();
    let why = format!("{sent:?}, pointer at {at:?}");
    cx.must("selftest.input", sent.is_ok() && at == to, why);

    desktop_selfcheck(cx);
}

/// The desktop helper starts, lists windows without ours and refuses a click
/// on the pill; on CI it also works a Notepad of its own (B-009 step 6).
fn desktop_selfcheck(cx: &mut Cx) {
    let own_point = pill_centre(&cx.app);
    cx.log("pillCentre", format!("{own_point:?}"));
    let own_handles: Vec<String> = cx
        .app
        .webview_windows()
        .values()
        .map(|w| protocol::handle_str(capture::hwnd_of(w) as u64))
        .collect();
    let options = selfcheck::Options {
        notepad: probes::notepad_enabled(),
        own_point,
        own_handles,
    };
    // It prints its own [SMOKE] lines; its failed musts count here too.
    let rep = selfcheck::Report::new("SMOKE");
    cx.block_on(selfcheck::run(desktop::client(), &rep, &options));
    cx.rep.absorb(rep.failures());
}

/// The pill's centre in physical pixels, while the overlay shows.
fn pill_centre(app: &AppHandle) -> Option<(i64, i64)> {
    let overlay = app.get_webview_window(windows::OVERLAY)?;
    if !overlay.is_visible().unwrap_or(false) {
        return None;
    }
    let r = windows::screen::dip_to_px(windows::bar_rect(app));
    let x = (r.x + r.width / 2.0).round() as i64;
    let y = (r.y + r.height / 2.0).round() as i64;
    Some((x, y))
}

// ---------------------------------------------------------- footprint

/// `PROCESS_MEMORY_COUNTERS_EX` (psapi.h).
#[repr(C)]
struct MemoryCounters {
    /// cb, then PageFaultCount.
    head: [u32; 2],
    /// PeakWorkingSetSize through PeakPagefileUsage, then PrivateUsage.
    sizes: [usize; 9],
}

// kernel32 exports the psapi call as K32GetProcessMemoryInfo, so it needs
// no windows-sys feature of its own.
#[link(name = "kernel32")]
extern "system" {
    fn K32GetProcessMemoryInfo(process: HANDLE, counters: *mut MemoryCounters, cb: u32) -> i32;
}

/// Bytes of private memory our process holds (Task Manager's "Memory").
fn private_bytes() -> Option<usize> {
    let cb = std::mem::size_of::<MemoryCounters>() as u32;
    let mut m = MemoryCounters {
        head: [cb, 0],
        sizes: [0; 9],
    };
    // SAFETY: our own process's pseudo handle, and a counters struct of the
    // size it says.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, cb) } != 0;
    ok.then_some(m.sizes[8])
}

fn ticks(t: &FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// Kernel plus user CPU time of our process, in 100 ns units.
fn cpu_ticks() -> Option<u64> {
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: our own process's pseudo handle and four live out pointers.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } != 0;
    ok.then(|| ticks(&kernel) + ticks(&user))
}

/// Every process that is us or whose parent is us, by exe name.
fn our_processes() -> Vec<String> {
    let me = std::process::id();
    let mut out = Vec::new();
    // SAFETY: a process snapshot read entry by entry and closed after.
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
                if e.th32ProcessID == me || e.th32ParentProcessID == me {
                    let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                    out.push(String::from_utf16_lossy(&e.szExeFile[..len]));
                }
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// What the app costs at rest, after every lane: its live pages, its
/// processes (no WebView2 browser under it any more), its private memory
/// after 10 s idle, its CPU over 5 s idle, and the hotkey-to-field times the
/// bar lane recorded.
pub fn footprint(cx: &Cx) {
    let pages = cx.on_main(|_| crate::ui::labels().len());
    cx.log("nativeWindows", pages.map_or(-1, |n| n as i64));
    let procs = our_processes();
    cx.log("processCount", procs.len());
    let browsers: Vec<&String> = procs
        .iter()
        .filter(|p| p.eq_ignore_ascii_case("msedgewebview2.exe"))
        .collect();
    cx.must("noWebView2", browsers.is_empty(), procs.join(","));

    cx.sleep(10_000);
    let private = private_bytes().map_or(-1.0, |b| b as f64 / (1024.0 * 1024.0));
    cx.log("idlePrivateMB", format!("{private:.1}"));

    let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
    let (t0, c0) = (Instant::now(), cpu_ticks());
    cx.sleep(5_000);
    let (wall, c1) = (t0.elapsed().as_secs_f64(), cpu_ticks());
    let pct = match (c0, c1) {
        (Some(a), Some(b)) => (b.saturating_sub(a) as f64 / 1e7) / wall / cores * 100.0,
        _ => -1.0,
    };
    cx.log("idleCpuPct", format!("{pct:.2}"));

    let mut samples = windows::bar_open_samples();
    samples.sort_unstable();
    match samples.get(samples.len() / 2) {
        Some(ms) => cx.log("hotkeyToFocusMs", ms),
        None => cx.log("hotkeyToFocusMs", "none"),
    }
}

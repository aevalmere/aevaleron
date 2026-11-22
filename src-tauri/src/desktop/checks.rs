//! CI checks for split (c), run by the built exe on the runner:
//!
//! - `AEVAL_CAPTURE_CHECK=1`: a white window fills the screen, a
//!   content-protected magenta window sits on top, and the production
//!   capture path must show the white through it (excluded, not blanked)
//!   with both backends; then the whole-screen and area shots are checked for
//!   size, encoding and budget (CAP-02..08). The glass backdrop's frame,
//!   cost and fixture checks (GLASS-01/05/06) went with the glass.
//! - `AEVAL_DESKTOP_CHECK=1`: the desktop selfcheck (lib/desktop/selfcheck.js
//!   port) with the Notepad half, against a visible window of ours.
//!
//! Lines are `[CAPTURE] k=v` / `[DESKTOP] k=v`; the run ends with `done` or
//! `failed: <labels>` and exits 0 or 1.

use std::io::Write;
use std::time::{Duration, Instant};

use base64::Engine as _;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::CreateSolidBrush;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage,
    RegisterClassExW, ShowWindow, TranslateMessage, MSG, SW_SHOWNOACTIVATE, WM_CLOSE, WM_DESTROY,
    WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::selfcheck::{self, Report};
use crate::capture::{self, dxgi, encode, screen, Rect, Rgb};
use crate::selftest::input::{self, Input};
use crate::shell::{AppHandle, Manager};
use crate::{desktop, region};

/// COLORREF (0x00BBGGRR) of the desktop stand-in and of the protected probe.
const WHITE: u32 = 0x00FF_FFFF;
const MAGENTA: u32 = 0x00FF_00FF;

pub fn maybe_run(app: &AppHandle) {
    let capture_check = std::env::var_os("AEVAL_CAPTURE_CHECK").is_some();
    let desktop_check = std::env::var_os("AEVAL_DESKTOP_CHECK").is_some();
    if !capture_check && !desktop_check {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        // Let the event loop settle and the scaffold windows load.
        std::thread::sleep(Duration::from_millis(1500));
        let rep = Report::new(if capture_check { "CAPTURE" } else { "DESKTOP" });
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if capture_check {
                capture_checks(&app, &rep);
            } else {
                crate::shell::async_runtime::block_on(desktop_checks(&app, &rep));
            }
        }));
        if result.is_err() {
            rep.must("panic", false, "the check panicked");
        }
        desktop::shutdown();
        let fails = rep.failures();
        if fails.is_empty() {
            println!("[{}] done", rep.prefix);
        } else {
            println!("[{}] failed: {}", rep.prefix, fails.join(","));
        }
        let _ = std::io::stdout().flush();
        app.exit(if fails.is_empty() { 0 } else { 1 });
    });
}

/// A plain, capturable window of one colour on `r` (physical), with its
/// own message loop on a thread of its own: what the fixture page was. It
/// closes when dropped.
struct Solid {
    hwnd: isize,
    pump: Option<std::thread::JoinHandle<()>>,
}

impl Solid {
    fn open(name: &str, r: screen::PxRect, color: u32, protected: bool) -> Option<Solid> {
        let (tx, rx) = std::sync::mpsc::channel::<isize>();
        let name = name.to_string();
        let pump = std::thread::spawn(move || {
            let hwnd = make_solid(&name, r, color);
            let _ = tx.send(hwnd);
            if hwnd != 0 {
                pump_messages();
            }
        });
        let hwnd = rx.recv_timeout(Duration::from_secs(5)).unwrap_or(0);
        if hwnd == 0 {
            return None;
        }
        if protected {
            capture::exclude_window(hwnd);
        }
        // SAFETY: a live window of ours.
        unsafe { ShowWindow(hwnd as HWND, SW_SHOWNOACTIVATE) };
        std::thread::sleep(Duration::from_millis(300));
        Some(Solid {
            hwnd,
            pump: Some(pump),
        })
    }
}

impl Drop for Solid {
    fn drop(&mut self) {
        // SAFETY: posting to a window of ours; its thread ends with it.
        unsafe { PostMessageW(self.hwnd as HWND, WM_CLOSE, 0, 0) };
        if let Some(t) = self.pump.take() {
            let _ = t.join();
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The window, hidden, on the calling thread; 0 when it was not made.
fn make_solid(name: &str, r: screen::PxRect, color: u32) -> isize {
    let class = wide(&format!("AevaleronCheck{color:06X}"));
    let title = wide(name);
    // SAFETY: a class of our own (registering it again fails harmlessly)
    // and a top-level window on this thread, which pumps its messages.
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null());
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        wc.lpfnWndProc = Some(solid_proc);
        wc.hInstance = inst;
        wc.hbrBackground = CreateSolidBrush(color);
        wc.lpszClassName = class.as_ptr();
        RegisterClassExW(&wc);
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            r.x,
            r.y,
            r.width.max(1),
            r.height.max(1),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null(),
        ) as isize
    }
}

unsafe extern "system" fn solid_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn pump_messages() {
    // SAFETY: the standard pump, on the thread that owns the window.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

struct Probe {
    magenta: f64,
    black: f64,
    mean: f64,
}

fn probe(img: &Rgb, r: screen::PxRect, origin: screen::PxRect) -> Probe {
    let (mut m, mut b, mut sum, mut n) = (0u64, 0u64, 0f64, 0u64);
    let x0 = (r.x - origin.x).max(0) as u32;
    let y0 = (r.y - origin.y).max(0) as u32;
    let x1 = ((r.x + r.width - origin.x).max(0) as u32).min(img.width);
    let y1 = ((r.y + r.height - origin.y).max(0) as u32).min(img.height);
    for y in (y0..y1).step_by(2) {
        for x in (x0..x1).step_by(2) {
            let [pr, pg, pb] = img.pixel(x, y);
            if pr > 200 && pg < 60 && pb > 200 {
                m += 1;
            }
            if pr < 16 && pg < 16 && pb < 16 {
                b += 1;
            }
            sum += (pr as f64 + pg as f64 + pb as f64) / 3.0;
            n += 1;
        }
    }
    let n = n.max(1) as f64;
    Probe {
        magenta: m as f64 / n,
        black: b as f64 / n,
        mean: sum / n,
    }
}

/// Horizontal edge energy: how much fine detail an image keeps.
fn edge_energy(img: &Rgb) -> f64 {
    let mut e = 0f64;
    for y in (0..img.height).step_by(3) {
        for x in 1..img.width {
            let a = img.pixel(x, y)[1] as f64;
            let b = img.pixel(x - 1, y)[1] as f64;
            e += (a - b).abs();
        }
    }
    e / (img.width as f64 * (img.height as f64 / 3.0)).max(1.0)
}

/// RMSE (0..255) of Lanczos3 and of `box_resize` against the ideal samples
/// of a 1-D sine (period 8 px, amplitude 100 around 128) shrunk by 0.625.
fn stripe_errors() -> (f64, f64) {
    let (sw, sh, tw, th) = (512u32, 16u32, 320u32, 10u32);
    let wave = |x: f64| 128.0 + 100.0 * (x * std::f64::consts::TAU / 8.0).sin();
    let mut data = Vec::with_capacity((sw * sh * 3) as usize);
    for _ in 0..sh {
        for x in 0..sw {
            let v = wave(x as f64 + 0.5).round() as u8;
            data.extend_from_slice(&[v, v, v]);
        }
    }
    let src = Rgb {
        width: sw,
        height: sh,
        data,
    };
    let rmse = |img: &Rgb| {
        let (mut e, mut n) = (0f64, 0f64);
        let y = th / 2;
        // Skip the borders, where every filter clamps.
        for x in 8..tw - 8 {
            let ideal = wave((x as f64 + 0.5) * sw as f64 / tw as f64);
            let d = img.pixel(x, y)[1] as f64 - ideal;
            e += d * d;
            n += 1.0;
        }
        (e / n).sqrt()
    };
    (rmse(&src.resize(tw, th)), rmse(&box_resize(&src, tw, th)))
}

/// Area-average downscale (the blurry baseline Lanczos must beat).
fn box_resize(img: &Rgb, w: u32, h: u32) -> Rgb {
    let mut data = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        let sy0 = y * img.height / h;
        let sy1 = ((y + 1) * img.height / h).max(sy0 + 1);
        for x in 0..w {
            let sx0 = x * img.width / w;
            let sx1 = ((x + 1) * img.width / w).max(sx0 + 1);
            let mut acc = [0u64; 3];
            let mut n = 0u64;
            for yy in sy0..sy1 {
                for xx in sx0..sx1 {
                    let p = img.pixel(xx, yy);
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += v as u64;
                    }
                    n += 1;
                }
            }
            for a in acc {
                data.push((a / n.max(1)) as u8);
            }
        }
    }
    Rgb {
        width: w,
        height: h,
        data,
    }
}

/// Mean brightness and the share of near-black pixels of a JPEG.
fn decode_look(bytes: &[u8]) -> Option<(f64, f64)> {
    let mut d = jpeg_decoder::Decoder::new(bytes);
    let px = d.decode().ok()?;
    let (mut sum, mut black) = (0f64, 0u64);
    for p in px.chunks_exact(3) {
        let v = (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0;
        sum += v;
        if v < 16.0 {
            black += 1;
        }
    }
    let n = (px.len() / 3).max(1) as f64;
    Some((sum / n, black as f64 / n))
}

/// B-108: with every open failing, a burst of captures tries one open and
/// goes on through GDI. The backoff is dropped afterwards, so DXGI is back.
fn dxgi_open_backoff(rep: &Report, d: &screen::Display) {
    dxgi::force_dxgi_open_failure(true);
    let _ = capture::reset_dxgi(d);
    let before = dxgi::dxgi_opens();
    let mut shots = 0;
    for _ in 0..20 {
        shots += i32::from(capture::grab(d).is_ok());
        std::thread::sleep(Duration::from_millis(50));
    }
    let opens = dxgi::dxgi_opens() - before;
    dxgi::force_dxgi_open_failure(false);
    let _ = capture::reset_dxgi(d);
    rep.log("dxgiOpensPerTick", opens);
    let why = format!("{opens} opens over 1 s, {shots} shots through GDI");
    rep.must("dxgiOpensPerTick", opens <= 1 && shots == 20, &why);
}

/// B-029: a new duplication's first frame shows the desktop, not an empty
/// pointer-only surface. A runner without DXGI says so loudly instead.
fn dxgi_first_frame(rep: &Report, d: &screen::Display, control: screen::PxRect) {
    let _ = capture::reset_dxgi(d);
    std::env::set_var("AEVAL_CAPTURE_BACKEND", "dxgi");
    let t = Instant::now();
    let got = capture::grab(d);
    std::env::remove_var("AEVAL_CAPTURE_BACKEND");
    match got {
        Ok(f) => {
            let c = probe(&f.rgb(), control, d.phys);
            rep.log("dxgiFirstFrameMs", t.elapsed().as_millis());
            rep.must(
                "dxgiFirstFrame",
                c.mean > 150.0 && c.black < 0.05,
                &format!("controlMean={:.0} black={:.3}", c.mean, c.black),
            );
        }
        Err(e) => dxgi_unavailable(rep, &e),
    }
}

fn dxgi_unavailable(rep: &Report, why: &str) {
    rep.log("dxgi", "unavailable");
    rep.log("dxgiError", why);
    eprintln!("WARNING: DXGI capture is unavailable: {why}");
}

fn decode_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    let mut d = jpeg_decoder::Decoder::new(bytes);
    d.decode().ok()?;
    let i = d.info()?;
    Some((i.width as u32, i.height as u32))
}

fn capture_checks(app: &AppHandle, rep: &Report) {
    let list = screen::all_displays();
    let d = screen::primary(&list);
    rep.log("windowsBuild", capture::windows_build());
    rep.must(
        "canHideFromCapture",
        capture::can_hide_from_capture(),
        "Windows older than 2004",
    );
    rep.log("displays", list.len());
    rep.log(
        "display",
        format!(
            "{}x{} phys, {}x{} dip, scale {}",
            d.phys.width, d.phys.height, d.bounds.width, d.bounds.height, d.scale_factor
        ),
    );

    // ---------------------------------------------- fixture + protected probe
    let fixture = Solid::open("capture-fixture", d.phys, WHITE, false);
    rep.must(
        "fixtureShown",
        fixture.is_some(),
        "fixture window did not open",
    );
    let pr = screen::PxRect {
        x: d.phys.x + d.phys.width / 2 - 200,
        y: d.phys.y + d.phys.height / 2 - 120,
        width: 400,
        height: 240,
    };
    let probe_win = Solid::open("capture-probe", pr, MAGENTA, true);
    rep.must(
        "probeShown",
        probe_win.is_some(),
        "protected probe window did not open",
    );
    std::thread::sleep(Duration::from_millis(1200));

    // A control strip of the fixture next to the probe: proves the capture
    // sees the page at all.
    let control = screen::PxRect {
        x: pr.x - 300,
        y: pr.y,
        width: 200,
        height: pr.height,
    };
    dxgi_first_frame(rep, &d, control);
    rep.log("hdr", if dxgi::dxgi_hdr_seen() { "on" } else { "off" });
    dxgi_open_backoff(rep, &d);

    let mut passed_any = false;
    for backend in ["dxgi", "gdi"] {
        std::env::set_var("AEVAL_CAPTURE_BACKEND", backend);
        let t = Instant::now();
        match capture::grab(&d) {
            Ok(f) => {
                let img = f.rgb();
                let p = probe(&img, pr, d.phys);
                let c = probe(&img, control, d.phys);
                rep.log(
                    &format!("{backend}.frame"),
                    format!("{}x{} in {}ms", f.width, f.height, t.elapsed().as_millis()),
                );
                rep.log(
                    &format!("{backend}.probe"),
                    format!(
                        "magenta={:.3} black={:.3} mean={:.0} controlMean={:.0}",
                        p.magenta, p.black, p.mean, c.mean
                    ),
                );
                let excluded = p.magenta < 0.01;
                let not_blanked = p.black < 0.05 && p.mean > 150.0;
                let sees_page = c.mean > 150.0;
                let ok = excluded && not_blanked && sees_page;
                // Whichever backend produced a frame, its result counts
                // (B-029: a working DXGI path is gated, not just logged).
                rep.must(
                    &format!("{backend}ExcludesOwnWithoutBlanking"),
                    ok,
                    &format!("excluded={excluded} notBlanked={not_blanked} seesPage={sees_page}"),
                );
                passed_any |= ok;
            }
            Err(e) => {
                if backend == "gdi" {
                    rep.must("gdiCapture", false, &e);
                } else {
                    dxgi_unavailable(rep, &e);
                }
            }
        }
    }
    std::env::remove_var("AEVAL_CAPTURE_BACKEND");
    rep.must(
        "captureExcludesOwn",
        passed_any,
        "no backend left our window out cleanly",
    );

    // ---------------------------------------------- whole screen (CAP-02/05)
    // The display the whole screen reads (D-09), at its physical size within
    // the 1600 px budget (B-032).
    let t = Instant::now();
    match capture::capture_screen(None) {
        Some(shot) => {
            let active = screen::latched_display(&screen::all_displays());
            let p = active.phys;
            let want = encode::whole_screen_size(p.width as u32, p.height as u32);
            let dims = decode_dims(&shot.jpeg);
            let look = decode_look(&shot.jpeg);
            let b64 = base64::engine::general_purpose::STANDARD
                .decode(&shot.base64)
                .ok();
            rep.log(
                "capture",
                format!(
                    "{}x{} bytes={} q={} backend={} ms={}",
                    shot.width,
                    shot.height,
                    shot.bytes,
                    shot.quality,
                    capture::last_backend(),
                    t.elapsed().as_millis()
                ),
            );
            rep.must(
                "captureSize",
                (shot.width, shot.height) == want && shot.width.max(shot.height) <= 1600,
                &format!(
                    "got {}x{} want {}x{}",
                    shot.width, shot.height, want.0, want.1
                ),
            );
            rep.must(
                "captureJpeg",
                dims == Some((shot.width, shot.height)),
                &format!("decoded {dims:?}"),
            );
            // The fixture fills the display: the shot is bright, not black
            // (B-029).
            rep.must(
                "wholeScreenShowsFixture",
                look.is_some_and(|(mean, black)| mean > 150.0 && black < 0.05),
                &format!("mean, black share: {look:?}"),
            );
            rep.must(
                "captureBudget",
                shot.bytes <= encode::SHOT_BUDGET && shot.quality >= 56,
                &format!("bytes={} q={}", shot.bytes, shot.quality),
            );
            rep.must(
                "captureBase64",
                b64.map(|b| b.len()) == Some(shot.bytes),
                "base64 does not decode to the bytes",
            );
            rep.must(
                "captureDisplay",
                shot.display == active.bounds && shot.region.is_none(),
                "display bounds or region wrong",
            );
            let v = serde_json::to_value(&shot).unwrap_or_default();
            // The field set main.js returned (serde_json's Value orders keys
            // alphabetically; the page reads fields by name).
            let mut keys: Vec<&str> = v
                .as_object()
                .map(|o| o.keys().map(String::as_str).collect())
                .unwrap_or_default();
            keys.sort_unstable();
            rep.must(
                "captureShape",
                keys == ["base64", "bytes", "display", "height", "region", "width"],
                &keys.join(","),
            );
        }
        None => rep.must("capture", false, "capture_screen returned None"),
    }

    // ---------------------------------------------- area at native pixels
    let area = Rect::new(d.bounds.x + 40.0, d.bounds.y + 40.0, 600.0, 300.0);
    match capture::capture_screen(Some(area)) {
        Some(shot) => {
            let (ew, eh) = (
                (600.0 * d.scale_factor).round() as u32,
                (300.0 * d.scale_factor).round() as u32,
            );
            rep.log(
                "regionCapture",
                format!(
                    "{}x{} bytes={} q={}",
                    shot.width, shot.height, shot.bytes, shot.quality
                ),
            );
            rep.must(
                "regionNative",
                (shot.width, shot.height) == (ew, eh),
                &format!("got {}x{} want {ew}x{eh}", shot.width, shot.height),
            );
            rep.must(
                "regionEchoed",
                shot.region == Some(area),
                "region not echoed",
            );
            rep.must(
                "regionJpeg",
                decode_dims(&shot.jpeg) == Some((shot.width, shot.height)),
                "area JPEG does not decode",
            );
        }
        None => rep.must("regionCapture", false, "area capture returned None"),
    }

    // ---------------------------------------------- area selector (CAP-11)
    // A drag on the real selector page comes back as the box, at the
    // display's origin, and the selector closes (B-028).
    region_drag(app, rep, &d);

    // ---------------------------------------------- fidelity (CAP-13 proxy)
    if let Ok(f) = capture::grab(&d) {
        let full = f.rgb();
        // 0.625x, the shrink a 2560-wide desktop gets to reach 1600.
        let (tw, th) = (full.width * 5 / 8, full.height * 5 / 8);
        let lz = full.resize(tw, th);
        let bx = box_resize(&full, tw, th);
        let (el, eb) = (edge_energy(&lz), edge_energy(&bx));
        rep.log(
            "fidelity",
            format!(
                "{}x{} -> {tw}x{th} lanczosEdge={el:.2} boxEdge={eb:.2}",
                full.width, full.height
            ),
        );
    }
    // Edge energy on a real screen rewards aliasing as much as detail, so the
    // gate is a signal with a known answer: fine stripes (8 px period, 4 px
    // at 1600/2560 is still above Nyquist) shrunk by 0.625. Lanczos3 must
    // land close to the ideal samples and beat the crude area average.
    let (lz_err, bx_err) = stripe_errors();
    rep.log(
        "fidelityStripes",
        format!("lanczosRmse={lz_err:.2} boxRmse={bx_err:.2}"),
    );
    rep.must(
        "fidelityLanczos",
        lz_err < 6.0 && lz_err < bx_err,
        &format!("lanczos rmse {lz_err:.2}, box {bx_err:.2}"),
    );
}

fn region_drag(app: &AppHandle, rep: &Report, d: &screen::Display) {
    let (a, on) = (app.clone(), d.clone());
    let pick = std::thread::spawn(move || {
        let select = region::select(&a, Some(on));
        crate::shell::async_runtime::block_on(select)
    });
    let t = Instant::now();
    while !region::ready(app) && t.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(50));
    }
    rep.log("regionLoadMs", t.elapsed().as_millis());
    std::thread::sleep(Duration::from_millis(300));
    // A real left-button drag from (40, 40) to (360, 240) on the display.
    let at = |x: f64, y: f64| {
        (
            d.phys.x + (x * d.scale_factor).round() as i32,
            d.phys.y + (y * d.scale_factor).round() as i32,
        )
    };
    let dragged = input::send(Input::Drag {
        from: at(40.0, 40.0),
        to: at(360.0, 240.0),
    });
    if let Err(e) = &dragged {
        rep.log("regionDragInput", e);
    }
    let t = Instant::now();
    while !pick.is_finished() && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if !pick.is_finished() {
        region::cancel(app);
    }
    let got = pick.join().ok().flatten();
    let want = Rect::new(d.bounds.x + 40.0, d.bounds.y + 40.0, 320.0, 200.0);
    rep.must(
        "regionDragReturnsTheBox",
        got == Some(want),
        &format!("got {got:?} want {want:?}"),
    );
}

async fn desktop_checks(app: &AppHandle, rep: &Report) {
    let list = screen::all_displays();
    let d = screen::primary(&list);
    // A visible window of ours for the BLOCKED_OWN_WINDOW check.
    let r = screen::PxRect {
        x: d.phys.x + 60,
        y: d.phys.y + 60,
        width: 360,
        height: 220,
    };
    let own = Solid::open("desktop-probe", r, WHITE, true);
    rep.must("ownProbeShown", own.is_some(), "probe window did not open");
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut handles: Vec<String> = app
        .webview_windows()
        .values()
        .map(|w| super::protocol::handle_str(capture::hwnd_of(w) as u64))
        .collect();
    if let Some(w) = &own {
        handles.push(super::protocol::handle_str(w.hwnd as u64));
    }
    rep.log("ownHandles", handles.len());
    let ci = std::env::var("GITHUB_ACTIONS").ok();
    let asked = std::env::var("AEVAL_DESKTOP_NOTEPAD").ok();
    let notepad = notepad_wanted(ci.as_deref(), asked.as_deref());
    selfcheck::run(
        desktop::client(),
        rep,
        &selfcheck::Options {
            notepad,
            own_point: own
                .as_ref()
                .map(|_| ((r.x + r.width / 2) as i64, (r.y + r.height / 2) as i64)),
            own_handles: handles,
        },
    )
    .await;
    rep.log("desktopStatus", desktop::client().status());
    drop(own);
}

/// B-115 (LIFE-16): the selfcheck types into Notepad on CI
/// (`GITHUB_ACTIONS`), or when asked with `AEVAL_DESKTOP_NOTEPAD` set to
/// anything but "0"; never by default on someone's desktop.
fn notepad_wanted(ci: Option<&str>, asked: Option<&str>) -> bool {
    ci.is_some() || asked.is_some_and(|v| v != "0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notepad_gating() {
        assert!(!notepad_wanted(None, None));
        assert!(notepad_wanted(Some("true"), None));
        assert!(notepad_wanted(Some("true"), Some("0")));
        assert!(notepad_wanted(None, Some("1")));
        assert!(!notepad_wanted(None, Some("0")));
    }

    #[test]
    fn lanczos_keeps_fine_stripes() {
        let (lz, bx) = stripe_errors();
        assert!(lz < 6.0 && lz < bx, "lanczos {lz:.2} box {bx:.2}");
    }
}

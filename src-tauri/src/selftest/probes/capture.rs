//! The capture lane's smoke labels, listed in `tools/smoke-labels/capture.txt`.
//! `selftest/mod.rs` documents the probe API, with a worked example.
//!
//! Each label keeps the value Electron printed (main.js runSmoke,
//! lib/smoke-v2.js, lib/smoke-glass.js, lib/smoke-v2-desktop.js). Where that
//! value has a pass condition, it prints as is when the condition holds and
//! as `=FAIL:<value>` when not. The desktop selfcheck's labels are printed by
//! the harness probe, which runs it once; they are listed here.

use std::fmt::Display;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::capture::{self, encode, screen, Rect};
use crate::llm::elements::{match_element, parse_elements};
use crate::selftest::{Cx, Input};
use crate::shell::Manager;
use crate::state::shared;
use crate::{backdrop, buddy, desktop, ipc, lens, region, store, windows};

pub fn run(cx: &mut Cx) {
    helper_not_at_boot(cx);
    whole_screen(cx);
    area(cx);
    selector_cancels(cx);
    selector_escape(cx);
    fail_null(cx);
    dpi_round_trip(cx);
    glass_tier(cx);
    lens_first_draw(cx);
    tour(cx);
}

/// smoke-glass.js's tier line. Native pages draw their own solid surface,
/// so the tier is always "solid"; the line stays for the record.
fn glass_tier(cx: &Cx) {
    let tier = backdrop::tier();
    let mut reasons = backdrop::reasons().join("+");
    if reasons.is_empty() {
        reasons = "none".into();
    }
    let can_hide = capture::can_hide_from_capture();
    cx.log("glassTier", format!("{tier} reasons={reasons} canHide={can_hide}"));
}

/// `value` under `label` when `ok`, else `label=FAIL:<value>`.
fn check(cx: &Cx, label: &str, ok: bool, value: impl Display) {
    if ok {
        cx.log(label, value);
    } else {
        cx.must(label, false, value);
    }
}

/// smoke-v2.js B29: the helper starts on first use, never at boot.
fn helper_not_at_boot(cx: &Cx) {
    let at_boot = desktop::helper_at_boot();
    let why = match at_boot {
        Some(true) => "the desktop helper was running before anything used it",
        Some(false) => "",
        None => "the boot windows never reached Rust, so nothing was noted",
    };
    cx.must("helperNotAtBoot", at_boot == Some(false), why);
}

/// `capture` (main.js:3446) at physical pixels within the 1600 px budget
/// (B-032), and `captureActiveDisplay`: the shot is of the display the
/// person is on (D-09: the foreground window's, else the pointer's).
fn whole_screen(cx: &Cx) {
    let list = screen::all_displays();
    let active = screen::latched_display(&list);
    let Some(shot) = cx.block_on(capture::capture_screen_async(None)) else {
        cx.must("capture", false, "failed");
        cx.must("captureActiveDisplay", false, "no shot");
        return;
    };
    let p = active.phys;
    let want = encode::whole_screen_size(p.width as u32, p.height as u32);
    let value = format!("ok:{}bytes:{}x{}", shot.bytes, shot.width, shot.height);
    if (shot.width, shot.height) == want {
        cx.log("capture", value);
    } else {
        let why = format!("{value} want {}x{}", want.0, want.1);
        cx.must("capture", false, why);
    }
    let d = shot.display;
    let at = format!("{},{} {}x{}", d.x, d.y, d.width, d.height);
    check(cx, "captureActiveDisplay", d == active.bounds, at);
}

/// main.js:3672-3697: an area of the primary comes back tagged and at its
/// own pixels, it spends the budget on more pixels per point than the whole
/// screen, and a point in it is a fraction of the area.
fn area(cx: &Cx) {
    let d = screen::primary(&screen::all_displays());
    let bx = Rect::new(d.bounds.x + 40.0, d.bounds.y + 40.0, 320.0, 200.0);
    let area = cx.block_on(capture::capture_screen_async(Some(bx)));
    let full = cx.block_on(capture::capture_screen_async(None));
    let Some(a) = area else {
        cx.must("regionCapture", false, "failed");
        cx.must("regionSharper", false, "no area shot");
        cx.must("regionPointTarget", false, "null");
        return;
    };
    let s = d.scale_factor;
    let native = ((320.0 * s).round() as u32, (200.0 * s).round() as u32);
    let tagged = if a.region.is_some() {
        "tagged"
    } else {
        "untagged"
    };
    let ok = a.region == Some(bx) && (a.width, a.height) == native;
    let value = format!("{}x{}:{tagged}", a.width, a.height);
    check(cx, "regionCapture", ok, value);

    match full {
        Some(f) => {
            let full_ppd = f.width as f64 / d.bounds.width;
            let area_ppd = a.width as f64 / bx.width;
            let value = format!("{full_ppd:.2}x->{area_ppd:.2}x");
            check(cx, "regionSharper", area_ppd >= full_ppd, value);
        }
        None => cx.must("regionSharper", false, "no whole-screen shot"),
    }

    let r = a.region.unwrap_or_default();
    let x = (r.x + 0.5 * r.width).round() as i64;
    let y = (r.y + 0.5 * r.height).round() as i64;
    let want_x = (bx.x + 160.0).round() as i64;
    let want_y = (bx.y + 100.0).round() as i64;
    let centred = (x, y) == (want_x, want_y);
    let value = json!({ "x": x, "y": y });
    check(cx, "regionPointTarget", centred, value);
}

/// The area selector on the primary, picked on a thread of its own.
fn open_selector(cx: &Cx) -> JoinHandle<Option<Rect>> {
    let app = cx.app.clone();
    let d = screen::primary(&screen::all_displays());
    thread::spawn(move || {
        let pick = region::select(&app, Some(d));
        crate::shell::async_runtime::block_on(pick)
    })
}

/// The pick's answer, or `None` while it is still open after `within`.
fn answer(cx: &Cx, pick: JoinHandle<Option<Rect>>, within: Duration) -> Option<Option<Rect>> {
    if !cx.wait_for(within, || pick.is_finished()) {
        region::cancel(&cx.app);
        let _ = cx.wait_for(Duration::from_secs(2), || pick.is_finished());
        return None;
    }
    pick.join().ok()
}

/// main.js:3699-3701: the selector opens and cancels without stranding a
/// full-screen scrim.
fn selector_cancels(cx: &Cx) {
    let pick = open_selector(cx);
    let opened = cx.wait_for(Duration::from_secs(5), || region::is_active(&cx.app));
    region::cancel(&cx.app);
    let got = answer(cx, pick, Duration::from_secs(3));
    let gone = cx.wait_for(Duration::from_secs(2), || !region::is_active(&cx.app));
    let ok = opened && got == Some(None) && gone;
    let why = format!("opened={opened} pick={got:?} closed={gone}");
    let value = if ok { "true".to_string() } else { why };
    check(cx, "regionSelectorCancels", ok, value);
}

/// main.js:3702-3711, with a real Escape key: it dismisses the scrim, not
/// the panel behind it (B-028 claims Escape once the selector exists).
fn selector_escape(cx: &Cx) {
    let pick = open_selector(cx);
    let _ = cx.wait_for(Duration::from_secs(8), || region::ready(&cx.app));
    cx.sleep(300);
    let was = shared(&cx.app).lock().mode;
    let sent = cx.input(Input::Key("Escape"));
    let got = answer(cx, pick, Duration::from_secs(3));
    let now = shared(&cx.app).lock().mode;
    let (cancelled, kept) = (got == Some(None), now == was);
    let state = if cancelled { "cancelled" } else { "stuck" };
    let panel = if kept { "panel-kept" } else { "panel-lost" };
    let value = match sent {
        Ok(()) => format!("{state}:{panel}"),
        Err(e) => format!("{state}:{panel} (Escape not sent: {e})"),
    };
    check(cx, "regionEscape", cancelled && kept, value);
}

/// CAP-06 and B-110: a capture thread that hangs makes the capture come back
/// empty within its limit (the question goes on as text), and the next
/// capture runs on a fresh thread.
fn fail_null(cx: &Cx) {
    capture::stall_worker(Duration::from_secs(8));
    let t = Instant::now();
    let hung = cx.block_on(capture::capture_screen_async(None));
    let ms = t.elapsed().as_millis();
    let next = cx.block_on(capture::capture_screen_async(None));
    let first = if hung.is_some() { "a shot" } else { "null" };
    let then = if next.is_some() { "a shot" } else { "null" };
    let value = format!("{first} in {ms}ms, then {then}");
    let ok = hung.is_none() && ms < 4500 && next.is_some();
    check(cx, "captureFailNull", ok, value);
}

/// DESK-08: physical rects go to DIP and back to the same pixels, on the
/// live displays and on two-display layouts at 125 % and 150 %.
fn dpi_round_trip(cx: &Cx) {
    let px = |x, y, width, height| screen::PxRect {
        x,
        y,
        width,
        height,
    };
    let pair = |dpi: u32| {
        let main = screen::Display::from_phys(
            r"\\.\DISPLAY1",
            true,
            96,
            px(0, 0, 1920, 1080),
            px(0, 0, 1920, 1040),
        );
        let side = screen::Display::from_phys(
            r"\\.\DISPLAY2",
            false,
            dpi,
            px(1920, 0, 2560, 1440),
            px(1920, 0, 2560, 1400),
        );
        screen::with_layout(vec![main, side])
    };
    let layouts = [
        ("live", screen::all_displays()),
        ("125", pair(120)),
        ("150", pair(144)),
    ];
    let (mut n, mut bad) = (0, Vec::new());
    for (name, list) in &layouts {
        for d in list {
            let p = d.phys;
            let r = px(p.x + p.width / 4, p.y + p.height / 3, 301, 167);
            let dip = screen::screen_to_dip_rect(list, r);
            let back = screen::dip_to_screen_rect(list, dip);
            let off = [
                back.x - r.x,
                back.y - r.y,
                back.width - r.width,
                back.height - r.height,
            ];
            n += 1;
            if off.iter().any(|v| v.abs() > 1) {
                bad.push(format!("{name}:{}:{r:?}->{back:?}", d.device));
            }
        }
    }
    let value = if bad.is_empty() {
        format!("ok:{n} rects within 1px")
    } else {
        bad.join(" ")
    };
    check(cx, "dpiRoundTrip", bad.is_empty(), value);
}

/// smoke-v2-desktop.js: a fresh lens told its frame once, with nothing sent
/// after, draws it with its chip (`lensFirstDraw`).
fn lens_first_draw(cx: &Cx) {
    let d = screen::primary(&screen::all_displays());
    let label = lens::label_for(d.id);
    lens::destroy(&cx.app);
    lens::set_frame(&cx.app, Some(&d), "smoke");
    let read = || match (cx.probe(&label, "frame"), cx.probe(&label, "chip")) {
        (Ok(frame), Ok(Value::String(chip))) => format!("{frame}:{chip}"),
        (Ok(frame), Ok(chip)) => format!("{frame}:{chip}"),
        (Err(e), _) | (_, Err(e)) => format!("error:{e}"),
    };
    let drew = cx.wait_for(Duration::from_secs(5), || read() == "true:smoke");
    let last = read();
    cx.log("lensFirstDraw", format!("frame:chip={last}"));
    let why = format!("frame:chip={last} want true:smoke");
    cx.must("lensFirstDraw", drew, why);
    // The page said so itself, once its listeners were up (B-034).
    let seen = cx.app.state::<ipc::Seen>().snapshot();
    let said = seen.get(&label).is_some_and(|c| c.contains("lens:ready"));
    let ready = said && lens::is_ready(&label);
    check(cx, "lensReady", ready, format!("said={said} ready={ready}"));
    lens::set_frame(&cx.app, None, "");
    lens::destroy(&cx.app);
}

fn point_at(x: f64, y: f64, label: &str) -> buddy::Point {
    buddy::Point {
        x,
        y,
        label: label.into(),
    }
}

/// Whether the pointer page is showing `message` within `within`.
fn pointer_shows(cx: &Cx, message: &str, within: Duration) -> bool {
    cx.wait_for(within, || {
        cx.probe(buddy::LABEL, "visible") == Ok(json!(true))
            && cx.probe(buddy::LABEL, "message") == Ok(json!(message))
    })
}

/// main.js:3653-3658: a point the model named resolves through the reader's
/// elements, an unknown name is dropped, and the pointer appears on a window
/// built for it from the one send (B-034).
fn point_resolved(cx: &Cx) {
    buddy::destroy();
    let d = screen::primary(&screen::all_displays());
    let b = d.bounds;
    let report = "ELEMENTS:\n- Terminal tab | 480,905\n- Problems tab | 600,905";
    let elements = parse_elements(Some(report), None);
    let points: Vec<buddy::Point> = ["terminal tab", "nothing here"]
        .iter()
        .filter_map(|name| {
            let e = match_element(name, &elements)?;
            let x = (b.x + e.x / 1000.0 * b.width).round();
            let y = (b.y + e.y / 1000.0 * b.height).round();
            Some(point_at(x, y, name))
        })
        .collect();
    let named = points.len();
    buddy::show_points(points, Some(&d));
    let shown = pointer_shows(cx, "terminal tab", Duration::from_secs(8));
    let ready = buddy::is_ready();
    let ok = named == 1 && shown && ready && buddy::is_active();
    let value = if ok {
        "true".to_string()
    } else {
        format!("named={named} shown={shown} ready={ready}")
    };
    check(cx, "pointResolved", ok, value);
    buddy::cancel();
}

/// smoke-v2.js B23 on the tour's order: the card asking for a key, the
/// first question once there is one, the bar's token (D-06). Runs without a key and then with a fake one; what was
/// stored is put back, and so is the bar, which the last step opens.
fn onboarding(cx: &Cx) {
    buddy::cancel();
    let s = store::get();
    let had = s.get_api_key();
    let mode = shared(&cx.app).lock().mode;
    s.clear_api_key();
    shared(&cx.app).call(&cx.app, |h| h.start_tour.as_ref());
    let _ = cx.wait_for(Duration::from_secs(2), buddy::is_active);
    let first = buddy::step_index();
    s.set_api_key("sk-or-smoke-fake");
    buddy::notify("key-saved");
    let second = buddy::step_index();
    buddy::notify("first-answer");
    let third = buddy::step_index();
    buddy::cancel();
    if had.is_empty() {
        s.clear_api_key();
    } else {
        s.set_api_key(&had);
    }
    cx.sleep(300);
    if shared(&cx.app).lock().mode != mode {
        cx.on_main(windows::collapse_panel);
        let _ = cx.wait_for(Duration::from_secs(2), || {
            shared(&cx.app).lock().mode == mode
        });
    }
    let value = format!("{first}>{second}>{third}");
    check(cx, "onboardingSteps", value == "0>1>2", value);
}

/// B-031: the window is kept, so a new answer's points show at once, and the
/// agent's first cursor after a tour has no page to wait for.
fn reuse(cx: &Cx) {
    buddy::show_points(vec![point_at(120.0, 140.0, "first point")], None);
    let first = pointer_shows(cx, "first point", Duration::from_secs(5));
    let t = Instant::now();
    buddy::show_points(vec![point_at(160.0, 180.0, "second point")], None);
    let second = pointer_shows(cx, "second point", Duration::from_millis(500));
    let shown_ms = t.elapsed().as_millis();
    buddy::cancel();
    shared(&cx.app).call(&cx.app, |h| h.start_tour.as_ref());
    let _ = cx.wait_for(Duration::from_secs(2), buddy::is_active);
    buddy::cancel();
    let d = screen::primary(&screen::all_displays());
    let t = Instant::now();
    cx.block_on(buddy::agent_point((200.0, 200.0), "smoke", &d));
    let glide = Duration::from_millis(buddy::GLIDE_WAIT_MS);
    let waited = t.elapsed().saturating_sub(glide).as_millis();
    buddy::agent_clear();
    let value = format!("points in {shown_ms}ms, agent waited {waited}ms");
    check(cx, "buddyReuse", first && second && waited < 300, value);
}

/// main.js:3921-3926: model points show and finish, and the walkthrough
/// starts.
fn tour(cx: &Cx) {
    buddy::cancel();
    point_resolved(cx);
    let point = buddy::Point {
        x: 80.0,
        y: 80.0,
        label: "smoke point".into(),
    };
    buddy::show_points(vec![point], None);
    let shown = buddy::is_active();
    check(cx, "buddyPoints", shown, shown);
    buddy::advance();
    // Done, and the pointer's window no longer takes clicks (B-114's half
    // that still applies to a native page).
    cx.sleep(200);
    let done = !buddy::is_active();
    let through = buddy::lets_mouse_through();
    let value = format!("{done} clickThrough={through}");
    check(cx, "buddyPointsDone", done && through, value);
    shared(&cx.app).call(&cx.app, |h| h.start_tour.as_ref());
    let started = cx.wait_for(Duration::from_secs(2), buddy::is_active);
    check(cx, "walkthrough", started, started);
    buddy::cancel();
    onboarding(cx);
    reuse(cx);

}

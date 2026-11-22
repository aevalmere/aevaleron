//! The windows lane's smoke labels, listed in `tools/smoke-labels/windows.txt`.
//! `selftest/mod.rs` documents the probe API, with a worked example.
//!
//! - `hoverKeepsAnswer`: the real pointer crosses an open answer window and
//!   leaves it, and the window stays shown, where it was, inside the work
//!   area, with its page saying it shows. tao's click-through hid any window
//!   shown with `show_inactive` the first time the pointer reached it.
//! - `minimizeToPill`: Minimize folds the bar into the pill and hides the
//!   answer window; opening the bar again brings the same conversation back.
//!
//! The native answer window is opaque, so it has no clear margin and
//! Electron's `marginClickThrough` is gone.

use std::time::Duration;

use serde_json::{json, Value};

use crate::selftest::{Cx, Input};
use crate::shell::Manager;
use crate::state::{shared, Rect};
use crate::store;
use crate::windows::{self, flags, outputs, screen, OVERLAY};

const WAIT: Duration = Duration::from_secs(5);
const BOOT: Duration = Duration::from_secs(20);

const LABELS: [&str; 2] = ["hoverKeepsAnswer", "minimizeToPill"];

/// How many messages the answer page shows: its `messages` probe, a list or
/// a count.
fn message_count(v: &Value) -> usize {
    match v {
        Value::Array(a) => a.len(),
        Value::Number(n) => n.as_u64().unwrap_or(0) as usize,
        _ => 0,
    }
}

fn page_shown(cx: &Cx, label: &str) -> bool {
    cx.probe(label, "visible") == Ok(json!(true))
}

fn conversation() -> Vec<Value> {
    vec![
        json!({ "role": "user", "text": "Why does the build fail?" }),
        json!({ "role": "assistant", "text": "Because `vcruntime140.dll` is not on this machine." }),
    ]
}

pub fn run(cx: &mut Cx) {
    let Some(label) = cx.on_main(outputs::target).flatten() else {
        for l in LABELS {
            cx.must(l, false, "no output window opened");
        }
        return;
    };
    let ready = cx.wait_for(BOOT, || cx.probe(&label, "visible").is_ok())
        && cx.wait_for(BOOT, || cx.probe(OVERLAY, "input").is_ok());
    if !ready {
        for l in LABELS {
            cx.must(l, false, "a page never loaded");
        }
        return;
    }

    // What the later probes start from, put back at the end.
    let l = label.clone();
    let before = cx.on_main(move |app| {
        let st = shared(app);
        let s = st.lock();
        let shown = flags::hwnd_for(&l).is_some_and(flags::is_visible);
        s.outputs.get(&l).map(|o| (o.chat.messages.clone(), shown))
    });
    let Some(Some((messages, was_shown))) = before else {
        for l in LABELS {
            cx.must(l, false, "the output has no state");
        }
        return;
    };

    let pos = store::get().get_overlay_pos();
    let l = label.clone();
    cx.on_main(move |app| {
        let payload = outputs::with(app, &l, |o| {
            o.chat.messages = conversation();
            outputs::chat_payload(o)
        });
        if let Some(p) = payload {
            windows::emit(app, &l, "chat:sync", p);
        }
    });

    // Andy lost the answer with the bar at the bottom left: the window
    // grows up from it, so the pointer comes down through its clear part.
    let mut kept = true;
    let mut whys = Vec::new();
    for (h, v) in [
        ("left", "bottom"),
        ("right", "bottom"),
        ("left", "top"),
        ("center", "top"),
    ] {
        place(cx, &label, h, v);
        let (k, why) = hover(cx, &label);
        kept &= k;
        whys.push(format!("[{h}-{v}] {why}"));
    }
    cx.must("hoverKeepsAnswer", kept, whys.join(" "));

    minimize(cx, &label);
    store::get().set_overlay_pos(&pos);

    let l = label.clone();
    cx.on_main(move |app| {
        let payload = outputs::with(app, &l, |o| {
            o.chat.messages = messages;
            outputs::chat_payload(o)
        });
        if let Some(p) = payload {
            windows::emit(app, &l, "chat:sync", p);
        }
        // Only the orb, as the later probes expect: an answer shown or
        // active holds the backdrop's fast pace (capture's backdropIdle).
        // One was up when this probe started; before the fix, tao hid it
        // the first time a probe's pointer crossed it.
        outputs::hide_all(app);
        windows::collapse_panel(app);
    });
    cx.sleep(400);
    // The later probes expect only the orb (capture's backdropIdle).
    let after = cx.on_main(|app| {
        let shown: Vec<String> = app
            .webview_windows()
            .into_iter()
            .filter(|(_, w)| w.is_visible().unwrap_or(false))
            .map(|(l, _)| l)
            .collect();
        let st = shared(app);
        let s = st.lock();
        let active: Vec<String> = s
            .outputs
            .values()
            .filter(|o| o.active)
            .map(|o| o.label.clone())
            .collect();
        format!(
            "shown={} active={} mode={:?}",
            shown.join(","),
            active.join(","),
            s.mode
        )
    });
    cx.log(
        "windowsAfter",
        format!(
            "{} (answer shown at start: {was_shown})",
            after.unwrap_or_default()
        ),
    );
}

/// The bar moved to that corner and the answer placed afresh beside it, as
/// a new answer would be.
fn place(cx: &Cx, label: &str, h: &str, v: &str) {
    cx.on_main(windows::collapse_panel);
    let _ = cx.wait_for(WAIT, || bar_mode(cx) == "pill");
    cx.sleep(300);
    store::get().set_overlay_pos(&json!({ "h": h, "v": v, "offX": 16, "offY": 16 }));
    let l = label.to_string();
    cx.on_main(move |app| {
        if let Some(w) = windows::window(app, &l) {
            flags::hide(&w);
        }
        outputs::with(app, &l, |o| {
            o.active = false;
            o.placed = false;
            o.rect = None;
            o.sheet = None;
            o.user_height = 0.0;
        });
        windows::open_bar(app, None, None);
    });
    let _ = cx.wait_for(WAIT, || bar_mode(cx) == "panel");
    cx.sleep(500);
    // The pointer starts off the windows, away from where they will be.
    let _ = cx.input(Input::Move(4, 4));
    let l = label.to_string();
    cx.on_main(move |app| outputs::show(app, &l));
    // The glob out and the page's first output:glass.
    cx.sleep(1200);
}

/// Inside the work area of the display it is on, to the pixel.
fn inside(r: Rect, wa: Rect) -> bool {
    r.x >= wa.x - 1.0
        && r.y >= wa.y - 1.0
        && r.x + r.width <= wa.x + wa.width + 1.0
        && r.y + r.height <= wa.y + wa.height + 1.0
}

fn same(a: Rect, b: Rect) -> bool {
    (a.x - b.x).abs() < 1.0
        && (a.y - b.y).abs() < 1.0
        && (a.width - b.width).abs() < 1.0
        && (a.height - b.height).abs() < 1.0
}

/// One stop of the pointer: moved there, given time, then looked at:
/// (the window shows, where it is, its page says it shows).
fn stop(cx: &Cx, label: &str, hwnd: isize, at: (i32, i32)) -> (bool, Rect, bool) {
    // Two moves, as a hand makes.
    let moved = cx.input(Input::Move(at.0 + 2, at.1)).is_ok();
    cx.sleep(60);
    let moved = moved && cx.input(Input::Move(at.0, at.1)).is_ok();
    cx.sleep(450);
    (
        moved && flags::is_visible(hwnd),
        flags::rect_px(hwnd),
        page_shown(cx, label),
    )
}

/// The pointer over the answer, near its edges, off it and back: (stayed,
/// the trail).
fn hover(cx: &Cx, label: &str) -> (bool, String) {
    let Some(hwnd) = flags::hwnd_for(label) else {
        return (false, "no window handle".into());
    };
    let start = flags::rect_px(hwnd);
    let wa = screen::matching_px(start).work_area;
    let in_wa = inside(windows::motion::glass(hwnd), wa);
    let start_shown = flags::is_visible(hwnd) && page_shown(cx, label);
    let x = (start.x + start.width / 2.0).round() as i32;
    let middle = (start.y + start.height / 2.0).round() as i32;
    let near_top = (start.y + 12.0).round() as i32;
    let near_bottom = (start.y + start.height - 12.0).round() as i32;
    let outside = ((start.x - 60.0).max(0.0) as i32, middle);
    let steps = [
        ("middle", (x, middle)),
        ("top", (x, near_top)),
        ("bottom", (x, near_bottom)),
        ("middle", (x, middle)),
        ("outside", outside),
    ];

    let mut ok = start_shown && in_wa;
    let mut trail = Vec::new();
    for (name, at) in steps {
        let (visible, rect, shown) = stop(cx, label, hwnd, at);
        ok &= visible && same(rect, start) && shown && inside(windows::motion::glass(hwnd), wa);
        trail.push(format!(
            "{name}:visible={visible},page={shown},rect={},{},{},{}",
            rect.x, rect.y, rect.width, rect.height
        ));
    }
    // Andy saw the answer only while he held Ctrl+Alt+Space: each repeat
    // showed it again. Once let go, the pointer over it must not hide it.
    // The hotkey's own path, not the keys: the real chord is also the voice
    // lane's push-to-talk and would leave its pill up for the later probes.
    cx.on_main(|app| windows::on_hotkey(app, "captureQuery"));
    cx.sleep(600);
    let (visible, rect, shown) = stop(cx, label, hwnd, (x, near_bottom));
    ok &= visible && shown;
    trail.push(format!(
        "after-hotkey:visible={visible},page={shown},rect={},{},{},{}",
        rect.x, rect.y, rect.width, rect.height
    ));
    let why = format!(
        "start_shown={start_shown} in_work_area={in_wa} wa={},{},{},{} {}",
        wa.x,
        wa.y,
        wa.width,
        wa.height,
        trail.join(" ")
    );
    cx.log("hoverTrail", &why);
    (ok, why)
}

fn bar_mode(cx: &Cx) -> String {
    cx.probe(OVERLAY, "mode")
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn minimize(cx: &Cx, label: &str) {
    let visible = || flags::hwnd_for(label).is_some_and(flags::is_visible);
    cx.on_main(|app| windows::open_bar(app, None, None));
    let opened = cx.wait_for(WAIT, || bar_mode(cx) == "panel") && cx.wait_for(WAIT, visible);
    cx.sleep(400);

    cx.on_main(windows::minimize);
    let folded = cx.wait_for(WAIT, || bar_mode(cx) == "pill") && cx.wait_for(WAIT, || !visible());
    cx.sleep(400);

    cx.on_main(|app| windows::open_bar(app, None, None));
    let back = cx.wait_for(WAIT, || bar_mode(cx) == "panel") && cx.wait_for(WAIT, visible);
    cx.sleep(600);
    let messages = cx.probe(label, "messages").unwrap_or(Value::Null);
    let shown = page_shown(cx, label);
    let kept = message_count(&messages) >= 2 && shown;
    cx.must(
        "minimizeToPill",
        opened && folded && back && kept,
        format!("opened={opened} folded={folded} back={back} kept={kept} messages={messages}"),
    );
}

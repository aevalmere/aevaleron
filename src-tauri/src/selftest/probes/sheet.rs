//! The sheet lane's smoke labels, listed in `tools/smoke-labels/sheet.txt`.
//! `selftest/mod.rs` documents the probe API, with a worked example.
//!
//! The markdown checks run on `ui::markdown`, the parser the native answer
//! page draws from. The rest runs in a real output window: the wheel is a
//! real wheel, and the window is read back through its probe keys
//! (`input`, `messages`, `scrollTop`, `atBottom`) at the runner's real scale.

use std::time::Duration;

use serde_json::{json, Value};

use crate::selftest::{Cx, Input};
use crate::ui::markdown::{self, Align, Line};
use crate::windows::{self, flags, outputs, screen};

const LABELS: &[&str] = &[
    "noOutputInput",
    "markdown",
    "markdownStructure",
    "transcriptScroll",
    "transcriptWheel",
    "chatCeiling",
    "moreIndicator",
];

/// Eight questions with answers long enough to overflow any window.
fn long_chat() -> Value {
    let mut messages = Vec::new();
    for i in 1..=8 {
        let answer = format!("Paragraph {i} of the answer. ").repeat(24);
        messages.push(json!({ "role": "user", "text": format!("Question {i}") }));
        messages.push(json!({ "role": "assistant", "text": answer }));
    }
    json!({ "messages": messages })
}

fn fail_all(cx: &Cx, why: &str) {
    for label in LABELS {
        cx.must(label, false, why);
    }
}

pub fn run(cx: &mut Cx) {
    markdown_labels(cx);
    let Some(label) = cx.on_main(outputs::target).flatten() else {
        fail_all(cx, "no output window opened");
        return;
    };
    let ready = cx.wait_for(Duration::from_secs(20), || {
        cx.probe(&label, "visible").is_ok()
    });
    if ready {
        check(cx, &label);
    } else {
        fail_all(cx, "the output page never came up");
    }
    let l = label.clone();
    cx.on_main(move |app| outputs::close(app, &l));
}

/// markdown and markdownStructure: what markdown.js drew, block by block.
fn markdown_labels(cx: &Cx) {
    let simple = markdown::lines("# Title\n\n- **one**\n- two");
    let heading = matches!(simple.first(), Some(Line::Heading { .. }));
    let bold = simple
        .iter()
        .any(|l| matches!(l, Line::Item { text, .. } if text.strong && text.text == "one"));
    cx.must("markdown", heading && bold, format!("{simple:?}"));

    let raw = [
        "# One",
        "## Two",
        "",
        "1. first",
        "2. second",
        "",
        "3. third",
        "   - nested",
        "4. fourth",
        "",
        "| A | B |",
        "|:---|---:|",
        "| x | y |",
        "",
        "See [the book](https://doc.rust-lang.org/book/).",
    ]
    .join("\n");
    let got = markdown::lines(&raw);
    let heads = has_heading(&got, 1) && has_heading(&got, 2);
    let items = got
        .iter()
        .filter(|l| matches!(l, Line::Item { depth: 0, marker, .. } if marker.ends_with('.')))
        .count();
    let nested = got.iter().any(|l| matches!(l, Line::Item { depth: 1, .. }));
    let aligns: Vec<Align> = got
        .iter()
        .find_map(|l| match l {
            Line::Table { aligns, .. } => Some(aligns.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let table = aligns == [Align::Left, Align::Right];
    let host = got.iter().any(|l| match l {
        Line::Para { text, .. } => text.links.iter().any(|k| k.host == "doc.rust-lang.org"),
        _ => false,
    });
    let ok = heads && items == 4 && nested && table && host;
    let why = format!("heads={heads} items={items} nested={nested} table={aligns:?} host={host}");
    cx.must("markdownStructure", ok, why);
}

fn has_heading(lines: &[Line], n: usize) -> bool {
    lines
        .iter()
        .any(|l| matches!(l, Line::Heading { level, .. } if *level == n))
}

fn top(cx: &Cx, label: &str) -> f64 {
    let v = cx.probe(label, "scrollTop").unwrap_or(Value::Null);
    v.as_f64().unwrap_or(-1.0)
}

fn at_bottom(cx: &Cx, label: &str) -> Option<bool> {
    cx.probe(label, "atBottom").ok().and_then(|v| v.as_bool())
}

fn check(cx: &Cx, label: &str) {
    // An answer window has no field to type in.
    let input = cx.probe(label, "input");
    let focus = cx.probe(label, "focused-input");
    let none = input == Ok(Value::Null) && focus != Ok(json!(true));
    cx.must("noOutputInput", none, format!("{input:?} {focus:?}"));

    let l = label.to_string();
    cx.on_main(move |app| windows::emit(app, &l, "chat:sync", long_chat()));
    // The window grows to fit, then stops at its ceiling.
    cx.sleep(1200);
    let messages = cx.probe(label, "messages");
    cx.log("sheetView", format!("{label} messages={messages:?}"));

    transcript(cx, label);
    let Some(hwnd) = flags::hwnd_for(label) else {
        cx.must("transcriptWheel", false, "no window");
        cx.must("chatCeiling", false, "no window");
        cx.must("moreIndicator", false, "no window");
        return;
    };
    wheel(cx, label, hwnd);

    // The window stops inside its display's work area, and the transcript
    // scrolls inside it.
    let r = flags::rect_px(hwnd);
    let work = screen::matching_px(r).work_px;
    let inside = r.height > 0.0 && r.y + r.height <= work.y + work.height + 1.0;
    let why = format!("window {r:?} work area {work:?}");
    cx.must("chatCeiling", inside, why);
}

/// Eight long answers overflow the window: it scrolls, and starts at the
/// newest.
fn transcript(cx: &Cx, label: &str) {
    let settled = cx.wait_for(Duration::from_secs(3), || {
        at_bottom(cx, label) == Some(true)
    });
    let t = top(cx, label);
    cx.must(
        "transcriptScroll",
        settled && t > 0.0,
        format!("scrollTop={t} atBottom={:?}", at_bottom(cx, label)),
    );
}

/// A real wheel over the middle of the window moves the transcript up, and
/// the "more" cue follows: shown off the bottom, gone back at it.
fn wheel(cx: &Cx, label: &str, hwnd: isize) {
    let rect = flags::rect_px(hwnd);
    let at = (
        (rect.x + rect.width / 2.0).round() as i32,
        (rect.y + rect.height / 2.0).round() as i32,
    );
    let before = top(cx, label);
    // The pointer arrives first, so the window stops passing clicks through.
    let moved = cx.input(Input::Move(at.0, at.1));
    cx.sleep(300);
    let sent = cx.input(Input::Wheel { at, notches: 3 });
    let scrolled = cx.wait_for(Duration::from_secs(3), || {
        let t = top(cx, label);
        t >= 0.0 && t < before
    });
    let after = top(cx, label);
    cx.must(
        "transcriptWheel",
        moved.is_ok() && sent.is_ok() && scrolled,
        format!("at {at:?} moved {moved:?} sent {sent:?} top {before}->{after}"),
    );

    let off = at_bottom(cx, label);
    let _ = cx.input(Input::Wheel { at, notches: -40 });
    let back = cx.wait_for(Duration::from_secs(3), || {
        at_bottom(cx, label) == Some(true)
    });
    cx.must(
        "moreIndicator",
        off == Some(false) && back,
        format!("after wheel up atBottom={off:?}, back={back}"),
    );
}

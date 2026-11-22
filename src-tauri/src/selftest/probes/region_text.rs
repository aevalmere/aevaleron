//! The region-text lane's smoke labels, listed in
//! `tools/smoke-labels/region-text.txt`: "Copy text from screen" over the
//! smoke's Notepad (exact text), over a picture of text (only OCR can read
//! it) and over nothing, all without the mouse. The picker is driven as the
//! capture probe drives it: a real Escape, and a real drag (`SendInput`).

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::capture::screen::{self, PxRect};
use crate::desktop;
use crate::desktop::protocol::handle_of;
use crate::region;
use crate::region_text::{self, note};
use crate::selftest::{notepad, Cx, Input};
use crate::voice::insert;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// What the clipboard holds before each copy, so a copy that should change
/// nothing is seen to have changed nothing.
const SENTINEL: &str = "region text probe clipboard";
/// The picker's line for a text pick (renderer/region.js).
const HINT: &str = "Drag over the text to copy it";
/// The two lines the picture holds.
const PICTURE: [&str; 2] = ["Copy this line 7314", "and this one too"];
/// Every gated label, for a run that cannot start.
const LABELS: [&str; 8] = [
    "regionTextHint",
    "regionTextEscape",
    "regionTextNotepad",
    "regionTextExact",
    "regionTextNote",
    "regionTextNoteLeaves",
    "regionTextPicture",
    "regionTextNothing",
];

pub fn run(cx: &mut Cx) {
    let Some(handle) = cx.notepad().map(|n| n.handle.clone()) else {
        for label in LABELS {
            cx.log(label, "skipped (no Notepad)");
        }
        return;
    };
    cx.on_main(crate::windows::collapse_panel);
    cx.sleep(300);
    let kept = insert::clipboard_text();
    let _ = insert::set_clipboard_text(SENTINEL);

    let _ = cx.block_on(desktop::client().focus_window(&json!(handle)));
    cx.sleep(400);
    let front = foreground();
    let in_notepad = handle_of(&json!(handle)) == Some(front);
    cx.log("regionTextFront", in_notepad);

    escape(cx, front);
    notepad_copy(cx, &handle, front);
    picture(cx);

    // The clipboard as it was found.
    let _ = insert::set_clipboard_text(kept.as_deref().unwrap_or(""));
}

fn foreground() -> u64 {
    // SAFETY: a query with no arguments.
    unsafe { GetForegroundWindow() as usize as u64 }
}

fn note_up(cx: &Cx) -> bool {
    cx.on_main(note::visible).unwrap_or(false)
}

/// One of the note page's probe keys, as text ("title", "lead").
fn note_text(cx: &Cx, key: &str) -> String {
    let got = cx.probe(note::LABEL, key).ok();
    got.and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn note_kind(cx: &Cx) -> String {
    let got = cx.probe(note::LABEL, "kind").ok();
    got.and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Starts the action as the hotkey and the tray item do, and waits for the
/// picker's page.
fn open_picker(cx: &Cx) -> bool {
    region_text::clear_outcome();
    region_text::start(&cx.app);
    let open = cx.wait_for(Duration::from_secs(8), || region::ready(&cx.app));
    cx.sleep(300);
    open
}

/// `regionTextHint`: the picker says what the drag will do.
/// `regionTextEscape`: a real Escape closes it; the clipboard is as it was,
/// nothing was read, no note shows and the keyboard is back where it was.
fn escape(cx: &Cx, front: u64) {
    let open = open_picker(cx);
    let hint = cx.probe(region::LABEL, "hint");
    let named = cx.probe(region::LABEL, "label");
    let says = hint == Ok(json!(HINT));
    let labelled = named
        .as_ref()
        .is_ok_and(|v| v.as_str().is_some_and(|s| s.contains("copy its text")));
    cx.must(
        "regionTextHint",
        open && says && labelled,
        format!("open={open} hint={hint:?} label={named:?}"),
    );

    let sent = cx.input(Input::Key("Escape"));
    let gone = cx.wait_for(Duration::from_secs(3), || !region::is_active(&cx.app));
    if !gone {
        region::cancel(&cx.app);
    }
    cx.sleep(500);
    let clip = insert::clipboard_text();
    let outcome = region_text::last_outcome();
    let noted = note_up(cx);
    let now = foreground();
    let same = clip.as_deref() == Some(SENTINEL);
    let ok = sent.is_ok() && gone && same && outcome.is_none() && !noted && now == front;
    cx.must(
        "regionTextEscape",
        ok,
        format!(
            "sent={sent:?} closed={gone} clipboard={clip:?} outcome={outcome:?} \
             note={noted} front={now:#x} was={front:#x}"
        ),
    );
}

fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// The rectangle around the fixture's three lines in Notepad, from the
/// places UI Automation gives for them, with a little room. A Notepad that
/// gives its text as one piece gives that piece's rectangle.
fn fixture_rect(cx: &Cx, handle: &str) -> Result<PxRect, String> {
    let h = json!(handle);
    let got = cx.block_on(desktop::client().get_text(Some(&h), &Value::Null, 50_000, false));
    let got = got.map_err(|e| format!("{}:{}", e.code, e.message))?;
    let spans = got["spans"].as_array().cloned().unwrap_or_default();
    let text_of = |s: &Value| s["text"].as_str().unwrap_or("").to_string();
    let lines: Vec<&str> = notepad::TEXT.split("\r\n").collect();
    let mut held: Vec<&Value> = spans
        .iter()
        .filter(|s| lines.contains(&text_of(s).trim()))
        .collect();
    if held.len() < lines.len() {
        held = spans
            .iter()
            .filter(|s| text_of(s).contains(lines[0]))
            .take(1)
            .collect();
    }
    if held.is_empty() {
        let seen: Vec<String> = spans.iter().map(text_of).take(8).collect();
        return Err(format!("no fixture lines in {seen:?}"));
    }
    // Left, top, right and bottom of each.
    let boxes: Vec<[f64; 4]> = held
        .iter()
        .map(|s| {
            let r = &s["rect"];
            let (x, y) = (number(&r["x"]), number(&r["y"]));
            [x, y, x + number(&r["width"]), y + number(&r["height"])]
        })
        .collect();
    let side = |i: usize| boxes.iter().map(move |b| b[i]);
    let left = side(0).fold(f64::INFINITY, f64::min);
    let top = side(1).fold(f64::INFINITY, f64::min);
    let right = side(2).fold(f64::NEG_INFINITY, f64::max);
    let bottom = side(3).fold(f64::NEG_INFINITY, f64::max);
    const ROOM: f64 = 4.0;
    Ok(PxRect {
        x: (left - ROOM).round() as i32,
        y: (top - ROOM).round() as i32,
        width: (right - left + 2.0 * ROOM).round() as i32,
        height: (bottom - top + 2.0 * ROOM).round() as i32,
    })
}

/// A real left-button drag over `area` (physical pixels), as
/// desktop/checks.rs does it. The picker answers `region:done` and its
/// window closes.
fn drag_over(cx: &Cx, area: PxRect) {
    let from = (area.x, area.y);
    let to = (area.x + area.width, area.y + area.height);
    if let Err(e) = cx.input(Input::Drag { from, to }) {
        cx.log("regionTextDragInput", e);
    }
}

/// `regionTextNotepad`: the whole action over the fixture's lines puts
/// exactly those lines on the clipboard. `regionTextExact`: read through UI
/// Automation, not OCR. `regionTextNote`: the note shows the count and the
/// first words, and Notepad still has the keyboard. `regionTextNoteLeaves`:
/// it goes away by itself.
fn notepad_copy(cx: &Cx, handle: &str, front: u64) {
    let area = match fixture_rect(cx, handle) {
        Ok(area) => area,
        Err(why) => {
            for label in &LABELS[2..6] {
                cx.must(label, false, &why);
            }
            return;
        }
    };
    // Which window is under the middle of the lines, for a failure to say.
    let (mx, my) = (area.x + area.width / 2, area.y + area.height / 2);
    let mid = json!({ "point": { "x": mx, "y": my } });
    let under = cx.block_on(desktop::client().window_states(mid)).ok();
    let under = under.map(|v| v["atPoint"].clone());
    let open = open_picker(cx);
    drag_over(cx, area);
    let done = cx.wait_for(Duration::from_secs(20), || {
        region_text::last_outcome().is_some()
    });
    let outcome = region_text::last_outcome().unwrap_or_default();
    let clip = insert::clipboard_text();
    cx.must(
        "regionTextNotepad",
        open && done && clip.as_deref() == Some(notepad::TEXT),
        format!("open={open} clipboard={clip:?} outcome={outcome:?} area={area:?}"),
    );
    cx.must(
        "regionTextExact",
        outcome.source == "exact",
        format!("{outcome:?} under={under:?} notepad={handle}"),
    );
    cx.log(
        "regionTextReadMs",
        format!("{} {}", outcome.source, outcome.read_ms),
    );

    let shown = cx.wait_for(Duration::from_secs(2), || {
        note_up(cx) && note_kind(cx) == "copied"
    });
    let title = note_text(cx, "title");
    let lead = note_text(cx, "lead");
    let now = foreground();
    let counted = outcome.title == "Copied 11 words" && title == outcome.title;
    let counted = counted && lead.starts_with("Invoice 2291 for March");
    cx.must(
        "regionTextNote",
        shown && counted && now == front,
        format!("shown={shown} title={title:?} lead={lead:?} front={now:#x} was={front:#x}"),
    );
    let left = cx.wait_for(Duration::from_secs(6), || !note_up(cx));
    cx.must(
        "regionTextNoteLeaves",
        left,
        "the note was still up 6 s after it showed",
    );
}

/// A window whose only content is a picture of two lines of text: the
/// window has no text of its own for UI Automation, so only OCR reads it.
/// The lower half of the picture is blank.
fn form_script(tag: &str) -> String {
    let [first, second] = PICTURE;
    [
        "Add-Type -AssemblyName System.Windows.Forms".to_string(),
        "Add-Type -AssemblyName System.Drawing".to_string(),
        "$f = New-Object Windows.Forms.Form".to_string(),
        format!("$f.Text = '{tag}'"),
        "$f.StartPosition = 'Manual'; $f.Left = 40; $f.Top = 300".to_string(),
        "$f.Width = 560; $f.Height = 380; $f.TopMost = $true".to_string(),
        "$b = New-Object Drawing.Bitmap 560, 380".to_string(),
        "$g = [Drawing.Graphics]::FromImage($b)".to_string(),
        "$g.Clear([Drawing.Color]::White)".to_string(),
        "$font = New-Object Drawing.Font 'Segoe UI', 24".to_string(),
        format!("$g.DrawString('{first}', $font, [Drawing.Brushes]::Black, 24, 40)"),
        format!("$g.DrawString('{second}', $font, [Drawing.Brushes]::Black, 24, 90)"),
        "$g.Dispose()".to_string(),
        "$p = New-Object Windows.Forms.PictureBox".to_string(),
        "$p.Dock = 'Fill'; $p.Image = $b".to_string(),
        "$f.Controls.Add($p)".to_string(),
        "[Windows.Forms.Application]::Run($f)".to_string(),
    ]
    .join("\r\n")
}

/// The form needs an STA thread, so this is a script file.
fn open_form(tag: &str) -> Result<(Child, PathBuf), String> {
    let script = std::env::temp_dir().join(format!("{tag}.ps1"));
    let wrote = std::fs::write(&script, form_script(tag));
    wrote.map_err(|e| e.to_string())?;
    let shell = crate::desktop::client::powershell();
    let args = ["-NoProfile", "-NonInteractive", "-STA"];
    let child = Command::new(shell)
        .args(args)
        .args(["-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok((child, script))
}

/// The form's window rectangle once it is listed, in physical pixels.
fn form_rect(cx: &Cx, tag: &str) -> Option<PxRect> {
    let mut found = None;
    cx.wait_for(Duration::from_secs(20), || {
        let listed = cx.block_on(desktop::client().list_windows(false));
        let all = listed.ok().and_then(|v| v.as_array().cloned());
        let win = all
            .unwrap_or_default()
            .into_iter()
            .find(|w| w["title"].as_str() == Some(tag));
        found = win.map(|w| PxRect {
            x: number(&w["rect"]["x"]) as i32,
            y: number(&w["rect"]["y"]) as i32,
            width: number(&w["rect"]["width"]) as i32,
            height: number(&w["rect"]["height"]) as i32,
        });
        found.is_some()
    });
    found
}

/// `regionTextPicture`: the step after the picker, over a picture of text,
/// puts its two lines on the clipboard through OCR. `regionTextNothing`:
/// over the blank part, the clipboard is as it was and the note says that
/// nothing was found. Both print `skipped` on a PC with no OCR language.
fn picture(cx: &Cx) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let tag = format!("aeval-rt-{}-{stamp:x}", std::process::id());
    let (mut child, script) = match open_form(&tag) {
        Ok(opened) => opened,
        Err(why) => {
            for label in &LABELS[6..] {
                cx.must(label, false, &why);
            }
            return;
        }
    };
    match form_rect(cx, &tag) {
        Some(form) => {
            // Painted, and on top.
            cx.sleep(600);
            picture_copies(cx, form);
        }
        None => {
            let exit = child.try_wait().ok().flatten();
            let why = format!("the picture window never opened: exit={exit:?}");
            for label in &LABELS[6..] {
                cx.must(label, false, &why);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_file(&script);
}

fn picture_copies(cx: &Cx, form: PxRect) {
    // Inside the client area: under the title bar, clear of the frame. The
    // lines are drawn in the upper part and nothing in the lower.
    let words = PxRect {
        x: form.x + 16,
        y: form.y + 56,
        width: form.width - 32,
        height: 130,
    };
    let blank = PxRect {
        x: form.x + 16,
        y: form.y + 240,
        width: form.width - 32,
        height: 90,
    };
    let (w, h) = (words.width, words.height);
    let rect = json!({ "x": words.x, "y": words.y, "width": w, "height": h });
    let can_read = cx.block_on(desktop::client().ocr(&rect, None, "auto"));
    if can_read.is_err_and(|e| e.code == "NO_OCR") {
        cx.log("regionTextPicture", "skipped (no OCR language)");
        cx.log("regionTextNothing", "skipped (no OCR language)");
        return;
    }
    let display = screen::active_display(&screen::all_displays());

    let _ = insert::set_clipboard_text(SENTINEL);
    let outcome = cx.block_on(region_text::copy_rect(&cx.app, words, &display));
    let clip = insert::clipboard_text().unwrap_or_default();
    let lines: Vec<String> = clip.lines().map(str::to_lowercase).collect();
    let read = lines.len() == 2
        && lines[0].contains("copy this line")
        && lines[0].contains("7314")
        && lines[1].contains("this one too");
    cx.must(
        "regionTextPicture",
        read && outcome.kind == "copied" && outcome.source == "ocr",
        format!("clipboard={clip:?} outcome={outcome:?} form={form:?}"),
    );
    cx.log("regionTextOcrMs", outcome.read_ms);
    // The note of that copy is gone before the next rectangle is read.
    let _ = cx.wait_for(Duration::from_secs(6), || !note_up(cx));

    let _ = insert::set_clipboard_text(SENTINEL);
    let outcome = cx.block_on(region_text::copy_rect(&cx.app, blank, &display));
    let clip = insert::clipboard_text();
    let said = cx.wait_for(Duration::from_secs(2), || note_kind(cx) == "none");
    let title = note_text(cx, "title");
    let kept = clip.as_deref() == Some(SENTINEL);
    cx.must(
        "regionTextNothing",
        outcome.kind == "none" && kept && said && title == "No text found here",
        format!("clipboard={clip:?} outcome={outcome:?} note={said} title={title:?}"),
    );
    let _ = cx.wait_for(Duration::from_secs(6), || !note_up(cx));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picture_script_draws_both_lines() {
        let script = form_script("aeval-rt-1-abc");
        assert!(script.contains("$f.Text = 'aeval-rt-1-abc'"));
        for line in PICTURE {
            assert!(script.contains(&format!("DrawString('{line}'")), "{line}");
        }
    }
}

//! The private apps lane's smoke labels, listed in
//! `tools/smoke-labels/private-apps.txt`. Notepad goes on the private apps
//! list for the length of the probe and is brought to the front, and the
//! bar is opened over it, as when a person asks. Every reader is then asked
//! for it, as the app asks:
//!
//! - `privateAppsCapture`: no screenshot is taken;
//! - `privateAppsAsk`: Ask's window text is not read, and the desktop
//!   helper is not even asked for it;
//! - `privateAppsBar`: the bar's notice says the one line;
//! - `privateAppsAgain`: off the list, the screenshot and the text read
//!   work again;
//! - `privateAppsRestored`: the stored list is as it was before the probe.
//!
//! `privateAppsMs` logs what the gate costs.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::capture;
use crate::desktop;
use crate::host;
use crate::privacy::{self, Read};
use crate::selftest::{notepad, Cx};
use crate::windows::{self, OVERLAY};

const LINE: &str = "Notepad is on your private apps list, so the screen was not read.";
const WAIT: Duration = Duration::from_secs(5);

const MODE_JS: &str = "document.body.dataset.mode || ''";

// The bar's notice as shown, or empty while it is hidden.
const NOTICE_JS: &str = r#"
(() => {
  const n = document.getElementById('notice');
  return n.hidden ? '' : document.getElementById('notice-text').textContent;
})()
"#;

/// Puts the stored list back however the probe ends, a panic included: the
/// smoke's Notepad cannot be found and closed while it is listed.
struct Restore(Option<Value>);

impl Drop for Restore {
    fn drop(&mut self) {
        let before = self.0.take().unwrap_or(Value::Null);
        crate::store::get().set_raw(privacy::KEY, before);
    }
}

pub fn run(cx: &mut Cx) {
    let Some(handle) = cx.notepad().map(|n| n.handle.clone()) else {
        return;
    };
    let handle = json!(handle);
    let cx: &Cx = cx;
    let listed_before = privacy::list();
    let restore = Restore(crate::store::get().raw(privacy::KEY));

    cx.on_main(windows::collapse_panel);
    let _ = cx.wait_for(WAIT, || mode(cx) == "pill");
    let _ = cx.block_on(desktop::client().focus_window(&handle));
    cx.sleep(300);
    let added = privacy::add("Notepad");
    // The bar opens over Notepad. It may take the keyboard; Notepad is the
    // app in front either way.
    cx.on_main(|app| windows::open_bar(app, None, None));
    let _ = cx.wait_for(WAIT, || mode(cx) == "panel");
    cx.sleep(400);
    let front = cx.wait_for(WAIT, || privacy::check(Read::Screen).is_err());
    let ours = windows::flags::foreground_label();
    cx.log(
        "privateAppsAdded",
        format!("{added:?} front={front} ours={ours:?}"),
    );

    capture_refused(cx);
    ask_refused(cx, &handle);
    bar_says(cx);
    let gate = gate_cost();

    let removed = privacy::remove("NOTEPAD.EXE");
    let again = reads_again(cx, &handle, removed);

    drop(restore);
    let now = privacy::list();
    cx.must(
        "privateAppsRestored",
        now == listed_before,
        format!("{now:?}, was {listed_before:?}"),
    );
    cx.on_main(windows::collapse_panel);
    let _ = cx.wait_for(WAIT, || mode(cx) == "pill");
    cx.log(
        "privateAppsMs",
        format!("gate={gate}us readAgain={again}ms"),
    );
}

fn mode(cx: &Cx) -> String {
    let v = cx.eval(OVERLAY, MODE_JS).unwrap_or(Value::Null);
    v.as_str().unwrap_or_default().to_string()
}

/// A screenshot of the whole screen with Notepad in front: none.
fn capture_refused(cx: &Cx) {
    let began = Instant::now();
    let shot = cx.block_on(capture::capture_screen_async(None));
    let ms = began.elapsed().as_millis();
    let why = privacy::check(Read::Screen).err().map(|r| r.message());
    cx.must(
        "privateAppsCapture",
        shot.is_none() && why.as_deref() == Some(LINE),
        format!("shot={} why={why:?} {ms}ms", shot.is_some()),
    );
}

/// Ask's context: the window's text is not read, and asking the helper for
/// Notepad's text by its handle is refused before anything is sent.
fn ask_refused(cx: &Cx, handle: &Value) {
    let exact = cx.block_on(host::ask::read_exact_text(None));
    let client = desktop::client();
    let direct = cx.block_on(client.get_text(Some(handle), &Value::Null, 16000, false));
    let code = match &direct {
        Ok(_) => "read".to_string(),
        Err(e) => e.code.clone(),
    };
    cx.must(
        "privateAppsAsk",
        exact.is_none() && code == privacy::CODE,
        format!("context={} helper={code}", exact.is_some()),
    );
}

/// The open bar's notice names the app, in the same line as everywhere
/// else (the page draws its notice only while the bar is open).
fn bar_says(cx: &Cx) {
    cx.on_main(windows::send_overlay_state);
    let notice = || cx.eval(OVERLAY, NOTICE_JS).unwrap_or(Value::Null);
    let said = cx.wait_for(WAIT, || notice() == json!(LINE));
    cx.must("privateAppsBar", said, format!("notice={}", notice()));
}

/// What one ask of the gate costs with a list to check, in microseconds.
fn gate_cost() -> u128 {
    const ASKS: u32 = 50;
    let began = Instant::now();
    for _ in 0..ASKS {
        let _ = privacy::check(Read::Screen);
    }
    began.elapsed().as_micros() / u128::from(ASKS)
}

/// Off the list: the screenshot is taken and Notepad's text is read again.
/// The helper was stopped when Notepad was listed, so the first read also
/// starts it. Returns how long the text took to come back, in ms.
fn reads_again(cx: &Cx, handle: &Value, removed: bool) -> u128 {
    let open = privacy::check(Read::Screen).is_ok();
    let shot = cx.block_on(capture::capture_screen_async(None));
    let began = Instant::now();
    let words = notepad::TEXT.lines().next().unwrap_or_default();
    let client = desktop::client();
    let mut last = String::new();
    let read = cx.wait_for(Duration::from_secs(15), || {
        let got = cx.block_on(client.get_text(Some(handle), &Value::Null, 16000, false));
        match got {
            Ok(g) => {
                last = g["text"].as_str().unwrap_or_default().to_string();
                last.contains(words)
            }
            Err(e) => {
                last = format!("{}: {}", e.code, e.message);
                false
            }
        }
    });
    let ms = began.elapsed().as_millis();
    let shown: String = last.chars().take(60).collect();
    cx.must(
        "privateAppsAgain",
        removed && open && shot.is_some() && read,
        format!(
            "removed={removed} open={open} shot={} text={shown:?}",
            shot.is_some()
        ),
    );
    ms
}

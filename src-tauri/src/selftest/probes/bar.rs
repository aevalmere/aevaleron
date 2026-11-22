//! The bar lane's labels (`tools/smoke-labels/bar.txt`), with Electron's
//! semantics from lib/smoke-v2.js, lib/smoke-v21.js and lib/smoke-glass.js
//! on `main`, plus the labels the bar items add: `barMultiline` (B-056),
//! `tokenReply` (B-150) and `earlyKeys` (B-154).
//!
//! The bar is a native page now: the intent checks call `ui::intent`
//! directly, keys go in through `SendInput`, and the field is read back
//! with the bar's probe keys (`input`, `intent`, `locked`, `mode`, `text`,
//! `examples`).

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowLongPtrW, GWL_EXSTYLE, GWL_STYLE,
};

use crate::selftest::{input, Cx, Input};
use crate::shell::Manager;
use crate::ui::intent::{self, Intent};
use crate::windows::{self, flags, OVERLAY};
use crate::{paths, state, store};

const WAIT: Duration = Duration::from_secs(5);
const BOOT: Duration = Duration::from_secs(20);

// setIgnoreMouseEvents owns WS_EX_TRANSPARENT and WS_EX_LAYERED, and
// setFocusable owns WS_EX_NOACTIVATE, which is checked against the mode on
// its own; WS_VISIBLE follows show and hide. None of them is churn.
const NOACTIVATE: u32 = 0x0800_0000;
const EX_MASK: u32 = !(0x20 | 0x8_0000 | NOACTIVATE);
const STYLE_MASK: u32 = !0x1000_0000;

/// One window's styles at one stage: (stage, window, style, extended style).
type Sample = (&'static str, String, u32, u32);

// Three lines a person pastes: each one a question, so Enter asks.
const LINES: &str = "why does the build fail?\nwhat does LNK1104 mean?\nhow do I fix it?";

// lib/smoke-v21.js basics(): the guess against what a person types. Every
// phrase Find used to take is here too, as an ask or a task.
const TABLE: [(&str, Intent); 50] = [
    ("why is the build red", Intent::Ask),
    ("how do I save this as pdf", Intent::Ask),
    ("what does this error mean?", Intent::Ask),
    ("explain this function", Intent::Ask),
    ("summarize this page", Intent::Ask),
    ("is this email a scam?", Intent::Ask),
    ("write me a haiku", Intent::Ask),
    ("translate this paragraph to French", Intent::Ask),
    ("find the bug in this function", Intent::Ask),
    ("what is the total of this column", Intent::Ask),
    ("which button saves the file?", Intent::Ask),
    ("save", Intent::Ask),
    ("/etc/hosts", Intent::Ask),
    ("can you open notepad?", Intent::Ask),
    ("", Intent::Ask),
    ("hello", Intent::Ask),
    ("refund policy", Intent::Ask),
    ("?invoice", Intent::Ask),
    ("?rename these files", Intent::Ask),
    ("invoice", Intent::Ask),
    ("2291", Intent::Ask),
    ("total due", Intent::Ask),
    ("0x1F3A", Intent::Ask),
    ("Please settle by Friday", Intent::Ask),
    ("where is the submit button", Intent::Ask),
    ("/invoice", Intent::Ask),
    ("//total", Intent::Ask),
    ("/ deadline", Intent::Ask),
    ("find invoice", Intent::Ask),
    ("find \"price\"", Intent::Ask),
    ("where is the cancel button", Intent::Ask),
    ("where's the save button", Intent::Ask),
    ("locate the download link", Intent::Ask),
    ("look for the word deadline", Intent::Ask),
    ("show me where the settings are", Intent::Ask),
    ("search for flights to Paris", Intent::Do),
    ("jump to the next chapter", Intent::Do),
    ("highlight the deadline in this document", Intent::Do),
    ("rename these screenshots by date", Intent::Do),
    ("open Notepad and write a haiku", Intent::Do),
    ("send the report to Sam", Intent::Do),
    ("delete the old ones", Intent::Do),
    ("do it", Intent::Do),
    ("can you open notepad", Intent::Do),
    (">save", Intent::Do),
    (">what time is it", Intent::Do),
    ("close all the tabs except this one", Intent::Do),
    ("sort this sheet by date", Intent::Do),
    ("open a new tab and search for the weather", Intent::Do),
    ("move these files into a folder called old", Intent::Do),
];

/// The middle: asked, with "Do this instead" on the answer.
const OFFER: [(&str, bool); 6] = [
    ("save", true),
    ("can you open notepad?", true),
    ("open notepad?", true),
    ("why is the build red", false),
    ("open notepad", false),
    ("?open notepad", false),
];

const PARSE: [(&str, Option<Intent>, &str); 5] = [
    ("?why", Some(Intent::Ask), "why"),
    ("> open notepad", Some(Intent::Do), "open notepad"),
    ("/invoice", None, "/invoice"),
    ("//total", None, "//total"),
    ("/etc/hosts", None, "/etc/hosts"),
];

pub fn run(cx: &mut Cx) {
    probe(cx);
}

fn probe(cx: &Cx) {
    let ready = cx.wait_for(BOOT, || cx.probe(OVERLAY, "input").is_ok());
    if !ready {
        cx.log("barPage", "the overlay page never came up");
    }
    intent_labels(cx);
    key_swallow(cx);
    style(cx);
    bar_keys(cx);
    token_reply(cx);
    early_keys(cx);
    lines_and_suggestions(cx);
    text_entry(cx);
    type_text(cx, "");
    collapse(cx);
}

// ----------------------------------------------------------------- helpers

/// The bar page's answer to `key`, or null when it has none.
fn page(cx: &Cx, key: &str) -> Value {
    cx.probe(OVERLAY, key).unwrap_or(Value::Null)
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn field(cx: &Cx) -> String {
    text(&page(cx, "input"))
}

/// The token as Electron's smoke read it: `ask`, `do`, with `!` when locked.
fn token(cx: &Cx) -> String {
    let locked = page(cx, "locked") == json!(true);
    let mark = if locked { "!" } else { "" };
    format!("{}{mark}", text(&page(cx, "intent")))
}

fn mode(cx: &Cx) -> String {
    text(&page(cx, "mode"))
}

/// The page shows `want`, then time for the morph and the window's resize.
fn settled(cx: &Cx, want: &str) -> bool {
    let ok = cx.wait_for(WAIT, || mode(cx) == want);
    cx.sleep(500);
    ok
}

fn open(cx: &Cx) -> bool {
    cx.on_main(|app| windows::open_bar(app, None, None));
    settled(cx, "panel")
}

fn collapse(cx: &Cx) -> bool {
    cx.on_main(windows::collapse_panel);
    settled(cx, "pill")
}

/// Opens the bar with the real hotkey, so its window takes the foreground
/// the way it does for a person; opens it directly if that fails.
fn open_by_hotkey(cx: &Cx, chord: &str) -> bool {
    if cx.input(Input::Key(chord)).is_ok() && settled(cx, "panel") {
        return true;
    }
    open(cx)
}

/// The middle of the bar, in physical pixels: inside the field.
fn bar_point(cx: &Cx) -> (i32, i32) {
    crate::selftest::bar_field_px(cx)
}

/// Puts the keyboard in the bar's field: a real click on it when it does
/// not have focus already.
fn focus_field(cx: &Cx) -> bool {
    let focused = || page(cx, "focused-input") == json!(true);
    if focused() {
        return true;
    }
    let at = bar_point(cx);
    let _ = cx.input(Input::Click(at.0, at.1));
    cx.wait_for(Duration::from_secs(2), focused)
}

/// Real keys: the field emptied (Ctrl+A, Delete), then `value` typed.
fn type_text(cx: &Cx, value: &str) {
    if mode(cx) != "panel" {
        return;
    }
    focus_field(cx);
    let _ = cx.input(Input::Key("Ctrl+A"));
    let _ = cx.input(Input::Key("Delete"));
    if !value.is_empty() {
        let _ = cx.input(Input::Text(value));
    }
    cx.wait_for(Duration::from_secs(2), || field(cx) == value);
}

/// Whether the key is held as a global hotkey, asked of Windows.
fn held(key: &str) -> bool {
    input::hotkey_taken(key).unwrap_or(true)
}

fn styles(hwnd: isize) -> (u32, u32) {
    let h = hwnd as HWND;
    // SAFETY: reading a style of a live window.
    let style = unsafe { GetWindowLongPtrW(h, GWL_STYLE) };
    // SAFETY: as above.
    let ex = unsafe { GetWindowLongPtrW(h, GWL_EXSTYLE) };
    (style as u32, ex as u32)
}

fn foreground_is_overlay() -> bool {
    // SAFETY: a query with no arguments.
    let fg = unsafe { GetForegroundWindow() } as isize;
    flags::hwnd_for(OVERLAY) == Some(fg)
}

fn last_question() -> String {
    let history = store::get().get_input_history();
    history.last().cloned().unwrap_or_default()
}

/// `hasSentFirstRequest` as the settings file on disk has it: what the next
/// start reads.
fn saved_flag() -> bool {
    let file = paths::user_data_dir().join("aevaleron.json");
    let body = std::fs::read_to_string(file).unwrap_or_default();
    let body = body.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(body).unwrap_or_default();
    v["hasSentFirstRequest"] == true
}

/// Whether the bar shows its starting points: a list or a count from the
/// page's `examples` probe.
fn examples_shown(v: &Value) -> bool {
    match v {
        Value::Array(a) => !a.is_empty(),
        Value::Number(n) => n.as_u64().unwrap_or(0) > 0,
        Value::Bool(b) => *b,
        _ => false,
    }
}

// ------------------------------------------------------------------ labels

/// intentTable, intentOffersDo, intentPrefixes, intentWorstMs, intentFast
/// and intentDoNeedsMargin (lib/smoke-v21.js), on the bar's own rules.
fn intent_labels(cx: &Cx) {
    let mut wrong = Vec::new();
    for (line, want) in TABLE {
        let got = intent::classify(line).intent;
        if got != want {
            wrong.push(format!("\"{line}\" {}!={}", got.key(), want.key()));
        }
    }
    let n = TABLE.len();
    cx.log("intentTable", format!("{}/{n}", n - wrong.len()));
    cx.must(
        "intentTable",
        n >= 40 && wrong.is_empty(),
        wrong.join(" | "),
    );

    let mut offer = Vec::new();
    for (line, want) in OFFER {
        let got = intent::classify(line).offer_do;
        if got != want {
            offer.push(format!("\"{line}\" {got}!={want}"));
        }
    }
    cx.must("intentOffersDo", offer.is_empty(), offer.join(" | "));

    let mut bad = Vec::new();
    for (raw, lock, body) in PARSE {
        let got = intent::parse(raw);
        if got != (lock, body) {
            bad.push(format!("{raw}:{got:?}"));
        }
    }
    cx.must("intentPrefixes", bad.is_empty(), bad.join(" | "));

    // The cost per keystroke, on a long line.
    intent::score("warm up");
    let mut worst = 0.0_f64;
    for _ in 0..3 {
        for (line, _) in TABLE {
            let long = format!("{line} ").repeat(40);
            let t0 = Instant::now();
            intent::score(&long);
            worst = worst.max(t0.elapsed().as_secs_f64() * 1000.0);
        }
    }
    cx.log("intentWorstMs", format!("{worst:.2}"));
    cx.must("intentFast", worst < 5.0, format!("{worst:.2}ms"));

    // Only a clear task runs: the margin rule leaves weak ones as Ask.
    let weak = intent::classify("fix it");
    let margin = weak.run - weak.ask;
    let ok = weak.intent != Intent::Do || margin >= 0.15;
    cx.must("intentDoNeedsMargin", ok, format!("{weak:?}"));
}

/// keySwallow: no global Enter ever, and no global Escape while only the
/// bar is open or nothing is (lib/smoke-v21.js).
fn key_swallow(cx: &Cx) {
    open(cx);
    cx.sleep(300);
    let enter = held("Enter");
    let esc_bar = held("Escape");
    collapse(cx);
    cx.sleep(200);
    let esc_idle = held("Escape");
    let seen = format!("enter={enter} escBar={esc_bar} escIdle={esc_idle}");
    let ok = !enter && !esc_bar && !esc_idle;
    cx.must("keySwallow", ok, seen);
}

/// styleSamples and styleStable: the windows' styles never change across
/// pill, bar and back, except the bits named above, and WS_EX_NOACTIVATE is
/// set on the pill and clear on the bar, exactly (lib/smoke-glass.js).
fn style(cx: &Cx) {
    let mut samples = Vec::new();
    collapse(cx);
    sample(cx, "pill", &mut samples);
    open(cx);
    sample(cx, "bar", &mut samples);
    collapse(cx);
    sample(cx, "pill-again", &mut samples);
    open(cx);
    sample(cx, "bar-again", &mut samples);
    let mut raw = Vec::new();
    for (stage, win, style, ex) in &samples {
        raw.push(format!("{stage}/{win}:{style:#x}/{ex:#x}"));
    }
    cx.log("styleSamples", raw.join(" "));
    let mut drift = Vec::new();
    for s in &samples {
        let (stage, win, style, ex) = s;
        let base = samples.iter().find(|b| b.1 == *win).unwrap_or(s);
        let style_moved = (style & STYLE_MASK) != (base.2 & STYLE_MASK);
        let ex_moved = (ex & EX_MASK) != (base.3 & EX_MASK);
        if style_moved || ex_moved {
            let was = format!("{}/{:#x}/{:#x}", base.0, base.2, base.3);
            let now = format!("{stage}/{style:#x}/{ex:#x}");
            drift.push(format!("{win} {was}->{now}"));
        }
        let pill = stage.starts_with("pill");
        let has = (ex & NOACTIVATE) != 0;
        if win == OVERLAY && has != pill {
            let line = format!("overlay {stage} WS_EX_NOACTIVATE={has} (want {pill})");
            drift.push(line);
        }
    }
    let ok = samples.len() >= 4 && drift.is_empty();
    let why = if drift.is_empty() {
        format!("samples={}", samples.len())
    } else {
        drift.join("; ")
    };
    cx.must("styleStable", ok, why);
}

/// One sample per visible window: the overlay and any output.
fn sample(cx: &Cx, stage: &'static str, out: &mut Vec<Sample>) {
    let st = state::shared(&cx.app);
    let outputs: Vec<String> = st.lock().outputs.keys().cloned().collect();
    let mut labels = vec![OVERLAY.to_string()];
    labels.extend(outputs);
    for win in labels {
        let visible = flags::hwnd_for(&win).filter(|h| flags::is_visible(*h));
        if let Some(h) = visible {
            let (style, ex) = styles(h);
            out.push((stage, win, style, ex));
        }
    }
}

/// barKeys: Tab is never typed into the field; Ctrl+1 and Ctrl+2 lock the
/// token and keep the text; Backspace on an empty locked field unlocks it
/// (lib/smoke-v2.js). Real keys, with the field focused.
fn bar_keys(cx: &Cx) {
    open(cx);
    type_text(cx, "");
    let _ = cx.input(Input::Key("Tab"));
    let tab = field(cx).contains('\t');
    type_text(cx, "invoice");
    let _ = cx.input(Input::Key("Ctrl+2"));
    cx.sleep(150);
    let do_lock = token(cx);
    let kept = field(cx);
    let _ = cx.input(Input::Key("Ctrl+1"));
    cx.sleep(150);
    let back = token(cx);
    type_text(cx, "");
    let _ = cx.input(Input::Key("Backspace"));
    cx.sleep(150);
    let unlocked = token(cx);
    let seen = format!("tab={tab}:{do_lock}:{kept}:{back}:{unlocked}");
    cx.log("barKeys", &seen);
    let locks = do_lock == "do!" && back == "ask!";
    let ok = !tab && kept == "invoice" && locks && !unlocked.ends_with('!');
    cx.must("barKeys", ok, seen);
}

/// Rust's own overlay:state with the agent waiting on the person.
fn send_waiting(cx: &Cx, waiting: &str) {
    let agent = json!({ "running": true, "step": 2, "state": waiting });
    cx.on_main(move |app| {
        let mut st = windows::overlay_state(app, None);
        st["agent"] = agent;
        windows::emit(app, OVERLAY, "overlay:state", st);
    });
}

/// The token's state: its intent and whether the bar's text names `word`.
fn token_says(cx: &Cx, word: &str) -> bool {
    text(&page(cx, "intent")) == "do" && text(&page(cx, "text")).contains(word)
}

/// tokenReply (B-150): while a task asks, the token says Reply and Enter
/// belongs to the task; with a plan waiting and the field empty it says
/// Start; once the task stops waiting it is the guess again.
fn token_reply(cx: &Cx) {
    open(cx);
    type_text(cx, "");
    send_waiting(cx, "asking");
    let asked = cx.wait_for(WAIT, || token_says(cx, "Reply"));
    let got_reply = token(cx);
    send_waiting(cx, "plan");
    let planned = cx.wait_for(WAIT, || token_says(cx, "Start"));
    let got_start = token(cx);
    cx.on_main(windows::send_overlay_state);
    let rest = || {
        let t = text(&page(cx, "text"));
        token(cx) == "ask" && !t.contains("Reply") && !t.contains("Start")
    };
    let back = cx.wait_for(WAIT, rest);
    let got_back = token(cx);
    let why = format!("asking={got_reply}/{asked} plan={got_start}/{planned} after={got_back}");
    cx.must("tokenReply", asked && planned && back, why);
}

/// earlyKeys (B-154): keys typed 5 ms after the hotkey land in the field.
fn early_keys(cx: &Cx) {
    let hotkey = store::get().shortcut("captureQuery");
    // Opened by the hotkey once, so the bar's window has the foreground as
    // it does for a person, then folded with the field empty.
    open_by_hotkey(cx, &hotkey);
    type_text(cx, "");
    collapse(cx);
    let sent = cx.input(Input::Key(&hotkey));
    cx.sleep(5);
    let typed = cx.input(Input::Text("early"));
    let kept = cx.wait_for(WAIT, || field(cx) == "early");
    let value = field(cx);
    let fg = foreground_is_overlay();
    let keys = format!("{hotkey}={sent:?} text={typed:?}");
    let why = format!("{keys} field={value:?} foreground={fg}");
    cx.must("earlyKeys", kept, why);
    // barOpenMs (B-104): hotkey to focused field for every hotkey press this
    // run made, oldest first. Logged, not gated: a shared runner's timing is
    // too noisy to hold to a budget, but the number has to be on record
    // before anyone tunes the open path.
    let samples = windows::bar_open_samples();
    cx.log("barOpenMs", format!("{samples:?}"));
    cx.log("barOpenStages", windows::bar_open_stages().join(" "));
}

/// textEntryDecides: a line handed to the bar as if typed and Entered
/// (`windows::submit_text`, the wake word's way in) is decided like typing:
/// a clear task goes the way of a leading > (`agent:start`, pushed to the
/// history as `>text`), a question and the unclear middle are asked. Run
/// with no key, so the task is turned away before anything happens.
fn text_entry(cx: &Cx) {
    let s = store::get();
    let had = s.get_api_key();
    s.clear_api_key();
    if s.has_api_key() {
        // Another provider's key would let the task start for real.
        if !had.is_empty() {
            s.set_api_key(&had);
        }
        cx.log("textEntryDecides", "skipped: a key is still set");
        return;
    }
    open(cx);
    let mut got = Vec::new();
    let cases = [
        ("open notepad", ">open notepad"),
        ("what does this error mean", "what does this error mean"),
        ("save", "save"),
        ("?open notepad", "?open notepad"),
    ];
    for (line, want) in cases {
        type_text(cx, "");
        let line = line.to_string();
        cx.on_main(move |app| windows::submit_text(app, &line));
        let ok = cx.wait_for(WAIT, || last_question() == want);
        got.push(format!("{want}={ok}"));
    }
    let seen = cx.app.state::<crate::ipc::Seen>().snapshot();
    let started = seen.get(OVERLAY).is_some_and(|c| c.contains("agent:start"));
    if !had.is_empty() {
        s.set_api_key(&had);
    }
    type_text(cx, "");
    let ok = started && got.iter().all(|g| g.ends_with("=true"));
    cx.must(
        "textEntryDecides",
        ok,
        format!("{} agentStart={started}", got.join(" ")),
    );
}

/// barMultiline (B-056): three pasted lines stay three rows, and Enter sends
/// them with both line breaks. suggestionsOnce (NEW-10): that first request
/// sets the flag in the settings file a restart reads, and the reopened bar
/// shows no suggestions.
fn lines_and_suggestions(cx: &Cx) {
    let s = store::get();
    let sent_before = s.get_has_sent_first_request();
    let has_key = s.has_api_key();
    open(cx);
    type_text(cx, "");
    let shown_before = page(cx, "examples");
    let kept = crate::shell::sys::clipboard_read();
    let pasted = crate::shell::sys::clipboard_write(LINES);
    focus_field(cx);
    let paste = cx.input(Input::Key("Ctrl+V"));
    let rows = || field(cx).lines().count();
    let three = cx.wait_for(WAIT, || rows() == 3);
    let got_rows = rows();
    let enter = cx.input(Input::Key("Enter"));
    let sent = cx.wait_for(WAIT, || last_question() == LINES);
    let question = last_question();
    let breaks = question.matches('\n').count();
    let ok = pasted && three && sent && breaks == 2;
    let keys = format!("paste={paste:?} rows={got_rows} enter={enter:?}");
    let why = format!("{keys} sent={question:?} key={has_key}");
    cx.must("barMultiline", ok, why);
    if let Some(t) = kept {
        let _ = crate::shell::sys::clipboard_write(&t);
    }

    let flag = s.get_has_sent_first_request();
    let on_disk = cx.wait_for(WAIT, saved_flag);
    let st = cx.on_main(|app| windows::overlay_state(app, None));
    let in_state = st.is_some_and(|v| v["hasSentFirstRequest"] == true);
    type_text(cx, "");
    collapse(cx);
    open(cx);
    let after = page(cx, "examples");
    let hidden = !examples_shown(&after);
    let ok = flag && on_disk && in_state && hidden;
    let before = format!("before={sent_before}/{shown_before}");
    let why = format!("{before} after={after} disk={on_disk} state={in_state}");
    cx.must("suggestionsOnce", ok, why);
}

//! An answer window (renderer/output.html, output.js and trail.js), native.
//! It hears the same channels the page did (`chat:*`, `query:*`,
//! `agent:*`, `overlay:state`, `output:meta`, `output:away`) and calls the
//! same ones back through `bridge`. The transcript is one flat list of
//! rows (`OutRow`, ui/output.slint) built from the items below and diffed
//! into the model row by row, so a row that did not change keeps its
//! component, its selection and its scroll; rebuilds are throttled to one
//! every 33 ms while an answer streams. Markdown comes from `markdown.rs`.
//!
//! The window is opaque and is the sheet: the page reports what it needs
//! to show everything (`output:glass {top: 0, height}`), and
//! `windows::outputs::set_sheet` sizes a window that follows its
//! transcript; one the person sized keeps its height (B-027).
//!
//! Keys (output.js, frame.js): Esc goes to the bar (panel mode) or back to
//! the app below, F6 to the bar; Ctrl+N, Ctrl+Tab, Ctrl+W, Ctrl+Shift+W,
//! Ctrl+Shift+C, Ctrl+., Ctrl+Shift+R, Ctrl+, Ctrl+H, Ctrl+M as before;
//! Tab and Shift+Tab move a focus ring over the buttons, Enter or Space
//! presses the ringed one; PageUp, PageDown, Up, Down, Home and End scroll;
//! Alt+Space (or a right click on the title bar) opens the window menu,
//! whose Move and Size take the arrows (Shift: 64 px), Enter keeps, Esc
//! puts the window back.
//!
//! Probe keys (`Page::probe`):
//! - `"text"`: the title and every shown text, one per line (string);
//! - `"visible"`: the window is on screen and not away (bool);
//! - `"shown"`: same as visible (the old sheet check);
//! - `"messages"`: transcript entries (questions, answers, error and note
//!   rows, trails, consent), the old `#chat > .msg` count (number);
//! - `"streaming"`: an answer is still coming (bool);
//! - `"title"`: the title bar's text (string); `"kind"`: "answer" or "task";
//! - `"trail"`: the latest trail, `{task, state, status, ended, open, plan:
//!   [{text, state}], steps: [{id, kind, state, text, n}], outcome, banner,
//!   usage}`, or null;
//! - `"card"`: the confirm card, `{id, text, host, rest, value, reason,
//!   similar, seen, ready, answered}`, or null; `"consent"`: the consent
//!   sheet's task (string) or null;
//! - `"scrollTop"`, `"scrollHeight"`, `"clientHeight"`, `"contentHeight"`
//!   (numbers, DIPs); `"atBottom"` and `"more"` (bool: within 8 px of the
//!   end, or not); `"stick"`: follows new words (bool);
//! - `"rows"`: rows in the model (number); `"measure"`: the reading
//!   column's width (number); `"inputs"`: editable fields, always 0;
//! - `"copyAll"`: the title bar's Copy is shown (bool); `"target"` (bool);
//! - `"menu"`: the open menu's rows (strings) or null;
//! - `"md:<markdown>"`: how `<markdown>` parses, `{kinds, heads, items,
//!   nested, table, host, text}` (the old sheet probe's STRUCTURE check).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use regex::Regex;
use serde_json::{json, Value};
use slint::{
    Image, Model, ModelRc, Rgb8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel,
};

use super::markdown::{self, Line};
use super::{bridge, OutButton, OutMenuRow, OutRow, OutputWindow};
use crate::shell::platform::{self, KeyPress};
use crate::state::Rect;
use crate::windows::motion;

macro_rules! re {
    ($src:expr) => {{
        static R: OnceLock<Regex> = OnceLock::new();
        R.get_or_init(|| Regex::new($src).expect("a valid pattern"))
    }};
}

/// At most one rebuild per frame at 30 fps while words stream.
const FLUSH: Duration = Duration::from_millis(33);
/// Free models queue: after this long with no word the label counts.
const WAIT_SAY: Duration = Duration::from_secs(6);
/// "Copied" stays this long.
const FLASH: Duration = Duration::from_millis(1500);
/// A confirm card's buttons wait this long (U45).
const SETTLE: Duration = Duration::from_millis(700);
/// New steps are announced at most this often (U27).
const STEP_SAY_GAP: Duration = Duration::from_secs(2);
/// The frame without the sheet is painted before `output:gone`.
const AWAY_PAINT: Duration = Duration::from_millis(50);
const UNDO_MS: f64 = 10_000.0;
const STEP: f64 = 16.0;
const STEP_BIG: f64 = 64.0;
/// A confirm value longer than the card shows must be seen whole first.
const VALUE_LINES: usize = 8;
const VALUE_CHARS: usize = 560;

const RESET_TEXT: &str = "Started over. The last conversation is in History.";
const RESET_FAILED: &str = "Couldn't bring it back here. It is in History.";
const EMPTY_TEXT: &str = "Your next question goes here.";
const PRIVACY_URL: &str = "https://openrouter.ai/settings/privacy";
const ALLOWED_AS_SIMILAR: &str = "Not asked again: you allowed this same step earlier in this task";

/// A payload field that is not there.
static NULL: Value = Value::Null;

// Segoe MDL2 Assets.
const G_COPY: &str = "\u{E8C8}";
const G_CHECK: &str = "\u{E73E}";
const G_LINK: &str = "\u{E71B}";
const G_REFRESH: &str = "\u{E72C}";
const G_DOWN: &str = "\u{E70D}";
const G_UP: &str = "\u{E70E}";
const G_INFO: &str = "\u{E946}";
const G_CLOCK: &str = "\u{E916}";

mod vk {
    pub const TAB: u16 = 0x09;
    pub const ENTER: u16 = 0x0D;
    pub const ESC: u16 = 0x1B;
    pub const SPACE: u16 = 0x20;
    pub const PGUP: u16 = 0x21;
    pub const PGDN: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const F6: u16 = 0x75;
    pub const COMMA: u16 = 0xBC;
    pub const PERIOD: u16 = 0xBE;
}

// ------------------------------------------------------------------ rows

/// One button as drawn; compared to the last build so unchanged rows stay.
#[derive(Clone, Default, PartialEq)]
struct Btn {
    id: String,
    text: String,
    glyph: String,
    prominent: bool,
    enabled: bool,
    checked: bool,
    focused: bool,
}

fn btn(id: impl Into<String>, text: impl Into<String>) -> Btn {
    Btn {
        id: id.into(),
        text: text.into(),
        enabled: true,
        ..Btn::default()
    }
}

impl Btn {
    fn glyph(mut self, g: &str) -> Self {
        self.glyph = g.to_string();
        self
    }

    fn prominent(mut self) -> Self {
        self.prominent = true;
        self
    }

    fn off(mut self, off: bool) -> Self {
        self.enabled = !off;
        self
    }

    fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
}

/// One drawn row (`OutRow` in Rust terms). `picture` is the id of the
/// answer whose screenshot the row shows, 0 for none.
#[derive(Clone, Default, PartialEq)]
struct Row {
    key: String,
    kind: &'static str,
    text: String,
    sub: String,
    extra: String,
    note: String,
    value: String,
    state: String,
    marker: String,
    level: i32,
    strong: bool,
    mono: bool,
    accent: bool,
    gap: f32,
    lines: Vec<String>,
    cells: Vec<String>,
    aligns: Vec<String>,
    buttons: Vec<Btn>,
    picture: u32,
}

fn row(key: String, kind: &'static str) -> Row {
    Row {
        key,
        kind,
        ..Row::default()
    }
}

fn strings(v: &[String]) -> ModelRc<SharedString> {
    let list: Vec<SharedString> = v.iter().map(|s| SharedString::from(s.as_str())).collect();
    ModelRc::new(VecModel::from(list))
}

fn out_row(r: &Row, picture: Image) -> OutRow {
    let buttons: Vec<OutButton> = r
        .buttons
        .iter()
        .map(|b| OutButton {
            id: b.id.as_str().into(),
            text: b.text.as_str().into(),
            glyph: b.glyph.as_str().into(),
            prominent: b.prominent,
            enabled: b.enabled,
            checked: b.checked,
            focused: b.focused,
        })
        .collect();
    OutRow {
        key: r.key.as_str().into(),
        kind: r.kind.into(),
        text: r.text.as_str().into(),
        sub: r.sub.as_str().into(),
        extra: r.extra.as_str().into(),
        note: r.note.as_str().into(),
        value: r.value.as_str().into(),
        state: r.state.as_str().into(),
        marker: r.marker.as_str().into(),
        level: r.level,
        strong: r.strong,
        mono: r.mono,
        accent: r.accent,
        gap: r.gap,
        lines: strings(&r.lines),
        cells: strings(&r.cells),
        aligns: strings(&r.aligns),
        buttons: ModelRc::new(VecModel::from(buttons)),
        picture,
    }
}

// ----------------------------------------------------------------- items

struct Item {
    id: u32,
    body: Body,
}

enum Body {
    User(String),
    Answer(Box<Answer>),
    Error(Box<Fault>),
    /// Start over's row, with Undo while it can.
    Reset {
        text: String,
        undo: bool,
        busy: bool,
    },
    Empty,
    Trail(Box<Trail>),
    Consent {
        task: String,
        lines: Vec<String>,
    },
    /// A run reopened from history.
    Stored {
        text: String,
        open: bool,
    },
}

#[derive(Default)]
enum Pic {
    #[default]
    Idle,
    Asking,
    Gone,
    Shown(Image),
}

#[derive(Default)]
struct Answer {
    raw: String,
    /// `raw` parsed, and the length it was parsed at.
    md: Vec<Line>,
    md_len: usize,
    stage: String,
    busy: bool,
    stopping: bool,
    question: String,
    details: String,
    usage: Option<Value>,
    sent: Option<Value>,
    msg_id: Value,
    sent_open: bool,
    image: Pic,
    retry: bool,
    /// "Do this instead": the words to run as a task.
    offer: String,
    offer_busy: bool,
}

#[derive(Default)]
struct Fault {
    message: String,
    /// Busy, not broken: a clock and a countdown.
    note: bool,
    /// A neutral row with no glyph (a stopped run's words).
    quiet: bool,
    counting: bool,
    ends: Option<Instant>,
    question: String,
    actions: Vec<Value>,
    provider: String,
    details: String,
    never_ask: bool,
    retry_task: String,
    retry_busy: bool,
}

struct Step {
    id: String,
    r: Value,
    n: usize,
}

struct Card {
    id: Value,
    text: String,
    host: String,
    rest: String,
    value: String,
    reason: String,
    similar: bool,
    seen: bool,
    all: bool,
    settled: bool,
    answered: bool,
    seq: u64,
}

struct Banner {
    text: String,
    plan: bool,
    busy: bool,
}

#[derive(Default)]
struct Trail {
    task: String,
    state: String,
    status: String,
    ended: bool,
    open: bool,
    plan: Option<Vec<String>>,
    steps: Vec<Step>,
    card: Option<Card>,
    banner: Option<Banner>,
    /// "Just answer", until the task has acted.
    answer: bool,
    stopping: bool,
    usage: String,
    outcome: Option<(String, Vec<Line>)>,
    count: usize,
    said: HashSet<String>,
}

enum Pick {
    Header,
    Retry(String, String),
    Win(&'static str),
}

struct Menu {
    /// The error row a pick replaces.
    from: Option<u32>,
    window: bool,
    picks: Vec<Pick>,
    texts: Vec<String>,
    mark: i32,
}

struct Adjust {
    size: bool,
    from: Rect,
}

#[derive(Default)]
struct St {
    items: Vec<Item>,
    next: u32,
    mode: String,
    target: bool,
    away: bool,
    away_seq: u64,
    last_gen: u64,
    live: Option<u32>,
    title: String,
    task_kind: bool,
    titled: bool,
    last_user: String,
    trail: Option<u32>,
    wait: Option<(Instant, bool)>,
    wait_model: String,
    reset_seq: u64,
    card_seq: u64,
    menu: Option<Menu>,
    adjust: Option<Adjust>,
    focus: Option<(String, String)>,
    flashes: Vec<(String, Instant)>,
    names: HashMap<String, String>,
    catalogue: Option<Value>,
    catalogue_failed: bool,
    show_paid: bool,
    names_asked: bool,
    closing: bool,
    step_said: Option<Instant>,
    step_pending: Option<String>,
    step_waiting: bool,
    last_height: f32,
    mirror: Vec<Row>,
}

fn push(st: &mut St, body: Body) -> u32 {
    st.next += 1;
    let id = st.next;
    st.items.push(Item { id, body });
    id
}

fn answer_mut(st: &mut St, id: u32) -> Option<&mut Answer> {
    st.items
        .iter_mut()
        .find(|i| i.id == id)
        .and_then(|i| match &mut i.body {
            Body::Answer(a) => Some(&mut **a),
            _ => None,
        })
}

fn trail_mut(st: &mut St, id: u32) -> Option<&mut Trail> {
    st.items
        .iter_mut()
        .find(|i| i.id == id)
        .and_then(|i| match &mut i.body {
            Body::Trail(t) => Some(&mut **t),
            _ => None,
        })
}

fn clear_placeholders(st: &mut St) {
    st.items
        .retain(|i| !matches!(i.body, Body::Reset { .. } | Body::Empty));
    st.reset_seq += 1;
}

fn set_title(st: &mut St, task: bool, text: &str, force: bool) {
    if st.titled && !force {
        return;
    }
    st.titled = true;
    st.task_kind = task;
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    st.title = if t.is_empty() {
        (if task { "Task" } else { "Answer" }).to_string()
    } else {
        t
    };
}

fn add_user(st: &mut St, text: String) {
    clear_placeholders(st);
    set_title(st, false, &text, false);
    st.last_user.clone_from(&text);
    push(st, Body::User(text));
}

fn new_trail(st: &mut St, task: &str) -> u32 {
    clear_placeholders(st);
    let t = Trail {
        task: task.to_string(),
        state: "planning".into(),
        status: "Planning\u{2026}".into(),
        answer: !task.is_empty(),
        ..Trail::default()
    };
    let id = push(st, Body::Trail(Box::new(t)));
    st.trail = Some(id);
    id
}

fn ensure_trail(st: &mut St) -> u32 {
    if let Some(id) = st.trail {
        if trail_mut(st, id).is_some() {
            return id;
        }
    }
    new_trail(st, "")
}

/// What a probe or Copy reads as the whole conversation.
fn transcript(st: &St) -> String {
    let mut out = Vec::new();
    for i in &st.items {
        match &i.body {
            Body::User(t) => out.push(format!("> {}", t.trim())),
            Body::Answer(a) => out.push(a.raw.trim().to_string()),
            Body::Trail(t) => {
                if let Some((text, _)) = &t.outcome {
                    out.push(text.trim().to_string());
                }
            }
            Body::Error(f) => out.push(f.message.trim().to_string()),
            _ => {}
        }
    }
    out.retain(|s| !s.is_empty());
    out.join("\n\n")
}

fn answer_count(st: &St) -> usize {
    st.items
        .iter()
        .filter(|i| match &i.body {
            Body::Answer(a) => !a.busy,
            Body::Trail(_) => true,
            _ => false,
        })
        .count()
}

// ---------------------------------------------------------------- values

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn num(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}

fn truthy(v: &Value, key: &str) -> bool {
    match v.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(t)) => !t.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x.abs() > 0.0),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

fn or<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() {
        b
    } else {
        a
    }
}

fn id_str(v: &Value) -> String {
    match v {
        Value::String(t) => t.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn gen_of(p: &Value) -> u64 {
    num(p, "gen").unwrap_or(0.0).max(0.0) as u64
}

fn cap(t: &str) -> String {
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    let len = digits.len();
    for (i, c) in digits.bytes().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(char::from(c));
    }
    out
}

fn sec(ms: f64) -> String {
    if ms < 10_000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else {
        format!("{:.0} s", ms / 1000.0)
    }
}

fn ends_in_stop(t: &str) -> bool {
    t.ends_with(['.', '?', '!'])
}

// ----------------------------------------------------------- model names

fn provider_name(id: &str, given: &str) -> String {
    if !given.is_empty() {
        return given.to_string();
    }
    match id {
        "" => String::new(),
        "openrouter" => "OpenRouter".into(),
        "anthropic" => "Anthropic".into(),
        "openai" => "OpenAI".into(),
        "gemini" | "google" => "Gemini".into(),
        other => cap(other),
    }
}

/// "Gemma 3 27B": the catalogue's name, else the id made readable.
fn model_name(id: &str, given: &str, names: &HashMap<String, String>) -> String {
    if !given.is_empty() {
        return given.to_string();
    }
    if id.is_empty() {
        return String::new();
    }
    if let Some(n) = names.get(id) {
        return n.clone();
    }
    let base = id.rsplit('/').next().unwrap_or(id);
    let base = re!(r":[\w-]+$").replace(base, "");
    let base = re!(r"[-_]+").replace_all(&base, " ");
    let base = re!(r"(?i)\b(it|instruct|chat|latest|preview)\b").replace_all(&base, "");
    let base = base.split_whitespace().collect::<Vec<_>>().join(" ");
    let up = re!(r"\b([a-z])").replace_all(&base, |c: &regex::Captures| c[1].to_uppercase());
    re!(r"(?i)\b(\d+(?:\.\d+)?)b\b")
        .replace_all(&up, "${1}B")
        .into_owned()
}

/// Which model answered and who served it: (name, provider name, id).
fn answered_by(
    u: &Value,
    sent: &Value,
    names: &HashMap<String, String>,
) -> (String, String, String) {
    let id = or(s(sent, "model"), s(u, "model")).to_string();
    let pid = or(s(sent, "provider"), s(u, "provider"));
    let pid = if pid.is_empty() && id.contains('/') {
        "openrouter"
    } else {
        pid
    };
    let name = model_name(&id, or(s(sent, "modelName"), s(u, "modelName")), names);
    let given = or(
        or(s(sent, "providerName"), s(u, "providerName")),
        s(u, "via"),
    );
    (name, provider_name(pid, given), id)
}

/// The full telemetry, for Copy details and the context menu.
fn usage_line(u: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(t) = num(u, "ttftMs").filter(|t| *t > 0.0) {
        parts.push(format!("{} to first word", sec(t)));
    }
    if let Some(ms) = num(u, "ms").filter(|t| *t > 0.0) {
        parts.push(sec(ms));
    }
    let tok = num(u, "prompt").unwrap_or(0.0) + num(u, "completion").unwrap_or(0.0);
    if tok > 0.0 {
        parts.push(format!("{} tokens", thousands(tok as u64)));
    }
    if let Some(c) = num(u, "cost").filter(|c| c.is_finite()) {
        if c.abs() < 1e-12 {
            parts.push("free".into());
        } else if c > 0.0 {
            parts.push(if c < 0.01 {
                format!("${c:.4}")
            } else {
                format!("${c:.2}")
            });
        }
    }
    if !s(u, "model").is_empty() {
        parts.push(s(u, "model").to_string());
    }
    if !s(u, "provider").is_empty() {
        parts.push(format!(
            "via {}",
            provider_name(s(u, "provider"), s(u, "via"))
        ));
    }
    if truthy(u, "textOnly") {
        parts.push("read as text".into());
    }
    parts.join(" \u{b7} ")
}

/// The quiet line under an answer (D-29).
fn quiet_usage(u: &Value, sent: &Value, cx: &Cx) -> String {
    let (name, pname, id) = answered_by(u, sent, cx.names);
    let mut parts = Vec::new();
    if !name.is_empty() && !pname.is_empty() {
        parts.push(format!("{name} via {pname}"));
    } else if !name.is_empty() || !pname.is_empty() {
        parts.push(format!("{name}{pname}"));
    }
    if let Some(ms) = num(u, "ms").filter(|t| *t > 0.0) {
        parts.push(sec(ms));
    }
    let hop = or(
        or(s(u, "hopFrom"), s(u, "busyProvider")),
        s(sent, "hopFrom"),
    );
    if !hop.is_empty() {
        parts.push(format!("{} was busy", provider_name(hop, "")));
    }
    let free = num(u, "cost").is_some_and(|c| c.abs() < 1e-12)
        || truthy(sent, "free")
        || id.ends_with(":free");
    if cx.show_paid && free {
        parts.push("free".into());
    }
    parts.join(" \u{b7} ")
}

fn credit_url(provider: &str) -> &'static str {
    match provider {
        "anthropic" => "https://console.anthropic.com/settings/billing",
        "openai" => "https://platform.openai.com/settings/organization/billing",
        "gemini" => "https://aistudio.google.com/",
        _ => "https://openrouter.ai/settings/credits",
    }
}

/// The seconds in a busy note, counted down in place.
fn countdown_text(message: &str, n: u64) -> String {
    match re!(r"(\d+)(\s*s\b)").captures_iter(message).last() {
        Some(c) => match c.get(1) {
            Some(m) => format!("{}{n}{}", &message[..m.start()], &message[m.end()..]),
            None => message.to_string(),
        },
        None => format!("{message} Trying again in {n} s."),
    }
}

fn secs_left(ends: Instant, now: Instant) -> u64 {
    ends.saturating_duration_since(now).as_secs_f64().ceil() as u64
}

fn fault_text(f: &Fault, now: Instant) -> String {
    match f.ends {
        Some(e) => countdown_text(&f.message, secs_left(e, now)),
        None => f.message.clone(),
    }
}

// ---------------------------------------------------------------- trails

fn is_act(kind: &str) -> bool {
    kind == "action" || kind == "act"
}

fn plan_index(r: &Value) -> Option<usize> {
    r.get("planIndex")
        .and_then(Value::as_u64)
        .map(|i| i as usize)
}

fn mark(state: &str) -> &'static str {
    match state {
        "pending" => "\u{25CB}",
        "running" | "paused" => "\u{25CF}",
        "done" => "\u{2713}",
        "skipped" => "\u{2013}",
        "failed" => "\u{2715}",
        _ => "\u{2022}",
    }
}

fn status_text(state: &str, step: u64) -> String {
    if state == "asking" {
        return if step > 0 {
            format!("Step {step}")
        } else {
            String::new()
        };
    }
    let t = match state {
        "planning" => "Planning\u{2026}",
        "thinking" => "Thinking\u{2026}",
        "running" => "Working",
        "confirm" | "plan" => "Waiting for you",
        "paused" => "Paused",
        _ => return String::new(),
    };
    if step > 0 {
        format!("{t} \u{b7} step {step}")
    } else {
        t.to_string()
    }
}

/// Each plan item's state from the acts so far (trail.js syncPlan, F22).
fn plan_states(t: &Trail) -> Vec<&'static str> {
    let Some(items) = &t.plan else {
        return Vec::new();
    };
    let acts: Vec<&Value> = t
        .steps
        .iter()
        .map(|x| &x.r)
        .filter(|r| is_act(s(r, "kind")))
        .collect();
    let indexed = acts.iter().copied().any(|r| plan_index(r).is_some());
    let cur = if indexed {
        acts.iter()
            .copied()
            .filter_map(plan_index)
            .max()
            .unwrap_or(0)
    } else {
        acts.iter()
            .filter(|r| matches!(s(r, "state"), "done" | "skipped"))
            .count()
    };
    let ended = if t.ended { t.state.as_str() } else { "" };
    (0..items.len())
        .map(|i| {
            if ended == "done" || i < cur {
                "done"
            } else if i == cur {
                let last = if indexed {
                    acts.iter()
                        .copied()
                        .rev()
                        .find(|r| plan_index(r) == Some(i))
                } else {
                    None
                };
                if last.is_some_and(|r| s(r, "state") == "failed") {
                    "failed"
                } else if ended == "stopped" {
                    "pending"
                } else if !ended.is_empty() {
                    "failed"
                } else {
                    "running"
                }
            } else {
                "pending"
            }
        })
        .collect()
}

fn note_of(r: &Value) -> String {
    let checked = s(r, "checked");
    let unasked = if !checked.is_empty() {
        format!("Allowed by the safety check: {checked}")
    } else if truthy(r, "similar") {
        ALLOWED_AS_SIMILAR.to_string()
    } else {
        String::new()
    };
    let error = s(r, "error");
    if !error.is_empty() {
        return error.to_string();
    }
    let why = s(r, "why");
    if s(r, "state") == "skipped" && !why.is_empty() {
        return format!("Not done: {why}");
    }
    unasked
}

fn side_of(r: &Value) -> String {
    match s(r, "state") {
        "running" if s(r, "kind") != "thinking" => "\u{2026}".into(),
        "skipped" => "Skipped".into(),
        "failed" => "Failed".into(),
        _ => match num(r, "ms") {
            Some(ms) if ms >= 1500.0 => format!("{:.1}s", ms / 1000.0),
            _ => String::new(),
        },
    }
}

fn recipients(p: &Value) -> Vec<String> {
    let given = p.get("recipients").and_then(Value::as_array);
    let list: Vec<&Value> = match given {
        Some(l) if !l.is_empty() => l.iter().collect(),
        _ => vec![p],
    };
    let mut out: Vec<String> = Vec::new();
    for r in list {
        let who = or(
            or(or(s(r, "providerName"), s(r, "name")), s(r, "provider")),
            s(r, "id"),
        );
        let what = or(s(r, "modelName"), s(r, "model"));
        let (who, what) = (who.trim(), what.trim());
        let one = if !who.is_empty() && !what.is_empty() {
            format!("{who} ({what})")
        } else {
            format!("{who}{what}")
        };
        if !one.is_empty() && !out.contains(&one) {
            out.push(one);
        }
    }
    out
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => "the model set for tasks".into(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// ----------------------------------------------------------------- build

/// What the builders read besides the item itself.
struct Cx<'a> {
    names: &'a HashMap<String, String>,
    show_paid: bool,
    flashes: &'a [(String, Instant)],
    now: Instant,
}

impl Cx<'_> {
    fn flashed(&self, key: &str) -> bool {
        self.flashes.iter().any(|(k, _)| k == key)
    }
}

fn copy_btn(cx: &Cx, row_key: &str, id: &str) -> Btn {
    let done = cx.flashed(&format!("{row_key}/{id}"));
    btn(id, if done { "Copied" } else { "Copy" }).glyph(if done { G_CHECK } else { G_COPY })
}

fn gap_before(line: &Line, prev: &Line) -> f32 {
    match (line, prev) {
        (Line::Item { .. }, Line::Item { .. }) => 4.0,
        (Line::Para { depth, .. }, _) if *depth > 0 => 4.0,
        (Line::Code { depth, .. }, _) if *depth > 0 => 6.0,
        (Line::Heading { .. }, _) => 18.0,
        _ => 10.0,
    }
}

/// A block's links as chips that open them, kept inline in its words too.
fn link_row(key: &str, text: &markdown::Inline, depth: i32, out: &mut Vec<Row>) {
    if text.links.is_empty() {
        return;
    }
    let mut r = row(format!("{key}.links"), "link");
    r.level = depth;
    r.gap = 4.0;
    for l in &text.links {
        let label = if l.host.is_empty() {
            l.label.clone()
        } else {
            format!("{} \u{b7} {}", l.label, l.host)
        };
        r.buttons
            .push(btn(format!("open:{}", l.url), label).glyph(G_LINK));
    }
    out.push(r);
}

fn md_rows(id: u32, tag: char, lines: &[Line], cx: &Cx, out: &mut Vec<Row>) {
    for (n, line) in lines.iter().enumerate() {
        let key = format!("{id}.{tag}{n}");
        let gap = if n == 0 {
            0.0
        } else {
            gap_before(line, &lines[n - 1])
        };
        match line {
            Line::Heading { level, text } => {
                let mut r = row(key.clone(), "h");
                r.level = *level as i32;
                r.text.clone_from(&text.text);
                r.gap = gap;
                out.push(r);
                link_row(&key, text, 0, out);
            }
            Line::Para { depth, text } => {
                let mut r = row(key.clone(), "p");
                r.level = *depth as i32;
                r.text.clone_from(&text.text);
                r.strong = text.strong;
                r.mono = text.mono;
                r.gap = gap;
                out.push(r);
                link_row(&key, text, *depth as i32, out);
            }
            Line::Item {
                depth,
                marker,
                text,
            } => {
                let mut r = row(key.clone(), "li");
                r.level = *depth as i32;
                r.marker.clone_from(marker);
                r.text.clone_from(&text.text);
                r.strong = text.strong;
                r.mono = text.mono;
                r.gap = gap;
                out.push(r);
                link_row(&key, text, *depth as i32 + 1, out);
            }
            Line::Quote(text) => {
                let mut r = row(key.clone(), "quote");
                r.text.clone_from(&text.text);
                r.gap = gap;
                out.push(r);
                link_row(&key, text, 0, out);
            }
            Line::Rule => {
                let mut r = row(key, "hr");
                r.gap = gap.max(12.0);
                out.push(r);
            }
            Line::Code { depth, lang, text } => {
                let mut r = row(key.clone(), "code");
                r.level = *depth as i32;
                r.sub.clone_from(lang);
                r.text.clone_from(text);
                r.gap = gap;
                r.buttons.push(copy_btn(cx, &key, "copy-code"));
                out.push(r);
            }
            Line::Table { head, aligns, rows } => {
                let aligns: Vec<String> = aligns.iter().map(|a| a.as_str().to_string()).collect();
                let mut h = row(key.clone(), "table");
                h.state = "head".into();
                h.cells = head.iter().map(|c| c.text.clone()).collect();
                h.aligns.clone_from(&aligns);
                h.gap = gap;
                out.push(h);
                for (i, cells) in rows.iter().enumerate() {
                    let mut r = row(format!("{key}.{i}"), "table");
                    r.cells = cells.iter().map(|c| c.text.clone()).collect();
                    r.aligns.clone_from(&aligns);
                    r.gap = 4.0;
                    out.push(r);
                }
                for (j, c) in head.iter().chain(rows.iter().flatten()).enumerate() {
                    link_row(&format!("{key}.l{j}"), c, 0, out);
                }
            }
        }
    }
}

fn sent_row(id: u32, a: &Answer, sent: &Value, cx: &Cx) -> Row {
    let mut r = row(format!("{id}.sent"), "sent");
    r.gap = 8.0;
    let (name, pname, _) = answered_by(&NULL, sent, cx.names);
    let to = or(&pname, "the provider").to_string();
    r.text = if name.is_empty() {
        format!("Sent to {to}:")
    } else {
        format!("Sent to {to} ({name}):")
    };
    r.lines.push("Your question".into());
    let image = s(sent, "image");
    if image == "screen" || image == "region" {
        let size = match sent.get("imageSize").and_then(Value::as_array) {
            Some(v) if v.len() == 2 => format!(", {} x {}", v[0], v[1]),
            _ => String::new(),
        };
        r.lines.push(if image == "screen" {
            format!("A screenshot of the whole screen{size}")
        } else {
            format!("A screenshot of the area you picked{size}")
        });
        match &a.image {
            Pic::Idle => r.buttons.push(btn("view", "View")),
            Pic::Asking => r.buttons.push(btn("view", "View").off(true)),
            Pic::Gone => r.buttons.push(btn("view", "Not kept").off(true)),
            Pic::Shown(_) => r.picture = id,
        }
    }
    if let Some(w) = sent
        .get("windowText")
        .filter(|w| num(w, "chars").unwrap_or(0.0) > 0.0)
    {
        let app = s(w, "app");
        let app = if app.is_empty() {
            "the window".to_string()
        } else {
            format!("the {app} window")
        };
        let doc = match s(w, "document") {
            "whole" => " and the whole document in it",
            "part" => " and part of the document in it",
            _ => "",
        };
        let chars = thousands(num(w, "chars").unwrap_or(0.0) as u64);
        r.lines
            .push(format!("The text of {app}{doc}, {chars} characters"));
    }
    if !s(sent, "private").is_empty() {
        r.lines
            .push(format!("Nothing from the screen. {}", s(sent, "private")));
    }
    let history = num(sent, "history").unwrap_or(0.0) as u64;
    if history > 0 {
        let plural = if history == 1 { "" } else { "s" };
        r.lines.push(format!(
            "{history} earlier message{plural} from this conversation"
        ));
    }
    if let Some(reader) = sent.get("reader") {
        if !s(reader, "modelName").is_empty() {
            r.lines
                .push(format!("{} read the screen first", s(reader, "modelName")));
        }
    }
    if truthy(sent, "free") {
        r.note = "Free models' providers may keep and train on what they receive.".into();
    }
    r
}

fn answer_rows(id: u32, a: &mut Answer, cx: &Cx, out: &mut Vec<Row>) {
    if a.busy && a.raw.is_empty() {
        let mut r = row(format!("{id}.think"), "thinking");
        r.text = or(&a.stage, "Thinking").to_string();
        out.push(r);
    } else {
        if a.md_len != a.raw.len() {
            a.md = markdown::lines(&a.raw);
            a.md_len = a.raw.len();
        }
        md_rows(id, 'm', &a.md, cx, out);
    }
    if !a.busy && a.raw.is_empty() {
        return;
    }
    let key = format!("{id}.meta");
    let mut m = row(key.clone(), "meta");
    m.gap = 8.0;
    if a.busy {
        let label = if a.stopping { "Stopping" } else { "Stop" };
        m.buttons.push(btn("stop", label).off(a.stopping));
    }
    if !a.raw.is_empty() {
        m.buttons.push(copy_btn(cx, &key, "copy"));
    }
    let steps = !a.busy && markdown::reads_as_steps(&a.raw);
    if steps {
        m.buttons.push(btn("do-it", "Do it"));
    }
    if a.retry && !a.busy {
        m.buttons.push(btn("retry", "").glyph(G_REFRESH));
        m.buttons.push(btn("retry-more", "").glyph(G_DOWN));
    }
    if !a.offer.is_empty() && !steps && !a.busy {
        m.buttons
            .push(btn("do-instead", "Do this instead").off(a.offer_busy));
    }
    if a.sent.is_some() {
        m.buttons
            .push(btn("sent", "What was sent").checked(a.sent_open));
    }
    if let Some(u) = &a.usage {
        m.sub = quiet_usage(u, a.sent.as_ref().unwrap_or(&NULL), cx);
    }
    out.push(m);
    if a.sent_open {
        if let Some(sent) = &a.sent {
            out.push(sent_row(id, a, sent, cx));
        }
    }
}

fn retry_split(r: &mut Row, text: &str) {
    r.buttons.push(btn("retry", text).glyph(G_REFRESH));
    r.buttons.push(btn("retry-more", "").glyph(G_DOWN));
}

fn fault_row(id: u32, f: &Fault, cx: &Cx) -> Row {
    let neutral = f.note || f.quiet;
    let key = format!("{id}.err");
    let mut r = row(key.clone(), if neutral { "note" } else { "error" });
    if !neutral {
        r.state = "error".into();
    }
    r.marker = if f.quiet {
        String::new()
    } else if f.note {
        G_CLOCK.into()
    } else {
        G_INFO.into()
    };
    r.text = fault_text(f, cx.now);
    for (i, a) in f.actions.iter().enumerate() {
        let aid = or(s(a, "id"), s(a, "action")).to_lowercase();
        let label = s(a, "label");
        let bid = format!("a{i}");
        if aid == "never-ask" {
            let b = btn(bid, or(label, "Don't ask again")).checked(f.never_ask);
            r.buttons
                .push(if f.never_ask { b.glyph(G_CHECK) } else { b });
        } else if aid == "retry" && !f.question.is_empty() {
            retry_split(&mut r, "Retry");
        } else if aid == "copy-details" {
            let done = cx.flashed(&format!("{key}/{bid}"));
            r.buttons.push(btn(
                bid,
                if done {
                    "Copied"
                } else {
                    or(label, "Copy details")
                },
            ));
        } else {
            let b = btn(bid, or(label, &aid));
            r.buttons.push(if aid == "hop" || aid == "fix-key" {
                b.prominent()
            } else {
                b
            });
        }
    }
    if f.actions.is_empty() && !f.question.is_empty() {
        retry_split(&mut r, "Retry");
    }
    if !f.retry_task.is_empty() {
        r.buttons.push(btn("retry-task", "Retry").off(f.retry_busy));
    }
    r
}

fn step_row(id: u32, x: &Step, level: i32) -> Row {
    let r = &x.r;
    let kind = s(r, "kind");
    let state = s(r, "state");
    let key = format!("{id}.s{}", x.id.replace('.', "_"));
    if kind == "thinking" {
        let mut w = row(key, "thinking");
        w.text = s(r, "text").to_string();
        w.level = level;
        w.gap = 4.0;
        return w;
    }
    let mut w = row(key, "step");
    w.text = s(r, "text").to_string();
    w.state = state.to_string();
    w.marker = mark(state).into();
    w.level = level;
    w.gap = 4.0;
    w.note = note_of(r);
    let ask = if kind == "ask" && state == "running" {
        w.accent = true;
        "Answer in the bar".to_string()
    } else if kind == "ask" && !s(r, "answer").is_empty() {
        format!("You: {}", s(r, "answer"))
    } else {
        String::new()
    };
    if !ask.is_empty() {
        w.note = if w.note.is_empty() {
            ask
        } else {
            format!("{} \u{b7} {ask}", w.note)
        };
    }
    w.sub = side_of(r);
    w
}

fn trail_rows(id: u32, t: &Trail, cx: &Cx, out: &mut Vec<Row>) {
    let acts = t.steps.iter().filter(|x| is_act(s(&x.r, "kind"))).count();
    let mut head = row(format!("{id}.head"), "trail-head");
    head.text.clone_from(&t.status);
    if t.ended && acts > 0 {
        let label = if acts == 1 {
            "1 step".to_string()
        } else {
            format!("{acts} steps")
        };
        let g = if t.open { G_UP } else { G_DOWN };
        head.buttons
            .push(btn("toggle", label).glyph(g).checked(t.open));
    }
    out.push(head);
    if let Some((_, md)) = &t.outcome {
        let at = out.len();
        md_rows(id, 'o', md, cx, out);
        if let Some(r) = out.get_mut(at) {
            r.gap = 8.0;
        }
    }
    if !t.ended || t.open {
        let states = plan_states(t);
        let n = t.plan.as_ref().map_or(0, Vec::len);
        if let Some(items) = &t.plan {
            for (i, text) in items.iter().enumerate() {
                let st = states.get(i).copied().unwrap_or("pending");
                let mut r = row(format!("{id}.p{i}"), "step");
                r.strong = true;
                r.text.clone_from(text);
                r.state = st.to_string();
                r.marker = mark(st).into();
                r.gap = 6.0;
                out.push(r);
                for x in t
                    .steps
                    .iter()
                    .filter(|x| is_act(s(&x.r, "kind")) && plan_index(&x.r) == Some(i))
                {
                    out.push(step_row(id, x, 1));
                }
            }
        }
        for x in &t.steps {
            let act = is_act(s(&x.r, "kind"));
            let under = n > 0 && act && plan_index(&x.r).is_some_and(|i| i < n);
            if under {
                continue;
            }
            let state = s(&x.r, "state");
            let behind = t.plan.is_some()
                && act
                && state != "running"
                && state != "failed"
                && !(t.ended && t.open);
            if !behind {
                out.push(step_row(id, x, 0));
            }
        }
    }
    if let Some(c) = &t.card {
        let mut r = row(format!("{id}.card{}", c.seq), "card");
        r.text.clone_from(&c.text);
        r.sub.clone_from(&c.host);
        r.extra.clone_from(&c.rest);
        r.value.clone_from(&c.value);
        r.note.clone_from(&c.reason);
        if c.all {
            r.state = "all".into();
        }
        r.gap = 10.0;
        let ready = c.settled && !c.answered;
        if !c.seen {
            r.buttons.push(btn("show-all", "Show all"));
        }
        r.buttons.push(btn("skip", "Skip").off(!ready));
        if c.similar {
            r.buttons
                .push(btn("similar", "Allow similar").off(!ready || !c.seen));
        }
        r.buttons
            .push(btn("allow", "Allow").prominent().off(!ready || !c.seen));
        out.push(r);
    }
    if let Some(b) = &t.banner {
        let mut r = row(format!("{id}.banner"), "banner");
        r.text.clone_from(&b.text);
        r.gap = 10.0;
        let label = if b.plan { "Start" } else { "Continue" };
        r.buttons.push(btn("go", label).off(b.busy));
        out.push(r);
    }
    let key = format!("{id}.meta");
    let mut m = row(key.clone(), "meta");
    m.gap = 8.0;
    if t.outcome.is_some() {
        m.buttons.push(copy_btn(cx, &key, "copy"));
    }
    if t.answer && !t.ended {
        m.buttons.push(btn("answer", "Just answer"));
    }
    if !t.ended {
        let label = if t.stopping { "Stopping" } else { "Stop" };
        m.buttons.push(btn("stop", label).off(t.stopping));
    }
    m.sub.clone_from(&t.usage);
    if !m.buttons.is_empty() || !m.sub.is_empty() {
        out.push(m);
    }
}

fn build_rows(st: &mut St, now: Instant) -> Vec<Row> {
    let St {
        items,
        names,
        show_paid,
        flashes,
        ..
    } = st;
    let cx = Cx {
        names: &*names,
        show_paid: *show_paid,
        flashes: flashes.as_slice(),
        now,
    };
    let mut out = Vec::new();
    let mut title_seen = false;
    for item in items.iter_mut() {
        let start = out.len();
        let id = item.id;
        match &mut item.body {
            Body::User(text) => {
                // The first question is the title; the transcript skips it.
                if !title_seen {
                    title_seen = true;
                    continue;
                }
                let mut r = row(format!("{id}.user"), "user");
                r.text.clone_from(text);
                out.push(r);
            }
            Body::Answer(a) => answer_rows(id, a, &cx, &mut out),
            Body::Error(f) => out.push(fault_row(id, f, &cx)),
            Body::Reset { text, undo, busy } => {
                let mut r = row(format!("{id}.reset"), "note");
                r.text.clone_from(text);
                if *undo {
                    r.buttons.push(btn("undo", "Undo").off(*busy));
                }
                out.push(r);
            }
            Body::Empty => {
                let mut r = row(format!("{id}.empty"), "empty");
                r.text = EMPTY_TEXT.into();
                r.gap = 24.0;
                out.push(r);
            }
            Body::Trail(t) => trail_rows(id, t, &cx, &mut out),
            Body::Consent { lines, .. } => {
                let mut r = row(format!("{id}.consent"), "card");
                r.text = "Let Aevaleron use your computer?".into();
                r.lines.clone_from(lines);
                r.buttons.push(btn("later", "Not now"));
                r.buttons.push(btn("allow", "Allow").prominent());
                out.push(r);
            }
            Body::Stored { text, open } => {
                let n = text.split('\n').filter(|l| !l.is_empty()).count();
                let mut r = row(format!("{id}.stored"), "stored");
                let g = if *open { G_UP } else { G_DOWN };
                r.buttons.push(
                    btn("toggle", format!("Task steps ({n})"))
                        .glyph(g)
                        .checked(*open),
                );
                if *open {
                    r.value.clone_from(text);
                }
                out.push(r);
            }
        }
        if start > 0 {
            if let Some(r) = out.get_mut(start) {
                r.gap = r.gap.max(16.0);
            }
        }
    }
    out
}

fn picture(st: &St, r: &Row) -> Image {
    if r.picture == 0 {
        return Image::default();
    }
    st.items
        .iter()
        .find(|i| i.id == r.picture)
        .and_then(|i| match &i.body {
            Body::Answer(a) => match &a.image {
                Pic::Shown(img) => Some(img.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_default()
}

// --------------------------------------------------------------- the menu

struct Group<'a> {
    label: String,
    provider: String,
    models: Vec<&'a Value>,
    current: &'a str,
}

fn list(v: Option<&Value>) -> Vec<&Value> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// The catalogue's model ids and names, for the usage line and the menu.
fn collect_names(v: &Value, into: &mut HashMap<String, String>) {
    let mut lists = vec![v.get("models")];
    for g in list(v.get("groups")) {
        lists.push(g.get("models"));
    }
    for p in list(v.get("providers")) {
        lists.push(p.get("models"));
        for g in list(p.get("groups")) {
            lists.push(g.get("models"));
        }
    }
    for m in lists.into_iter().flat_map(list) {
        let id = s(m, "id");
        if !id.is_empty() {
            into.insert(id.to_string(), or(s(m, "name"), id).to_string());
        }
    }
}

/// The providers and their models (output.js menuGroups).
fn menu_groups(r: &Value) -> Vec<Group<'_>> {
    let current = s(r, "current");
    if let Some(ps) = r
        .get("providers")
        .and_then(Value::as_array)
        .filter(|p| !p.is_empty())
    {
        return ps
            .iter()
            .filter(|p| p.get("ready") != Some(&Value::Bool(false)))
            .map(|p| {
                let mut models = list(p.get("models"));
                for g in list(p.get("groups")) {
                    models.extend(list(g.get("models")));
                }
                Group {
                    label: provider_name(s(p, "id"), s(p, "name")),
                    provider: s(p, "id").to_string(),
                    models,
                    current: or(s(p, "current"), current),
                }
            })
            .filter(|g| !g.models.is_empty())
            .collect();
    }
    list(r.get("groups"))
        .into_iter()
        .map(|g| Group {
            label: or(s(g, "label"), "OpenRouter").to_string(),
            provider: "openrouter".into(),
            models: list(g.get("models")),
            current,
        })
        .collect()
}

type MenuFill = (Vec<OutMenuRow>, Vec<Pick>, Vec<String>, i32);

fn retry_rows(cat: &Value, show_paid: bool, names: &HashMap<String, String>) -> MenuFill {
    let (mut rows, mut picks, mut texts) = (Vec::new(), Vec::new(), Vec::new());
    let mut mark = -1;
    for g in menu_groups(cat) {
        rows.push(OutMenuRow {
            header: true,
            text: g.label.as_str().into(),
            ..OutMenuRow::default()
        });
        picks.push(Pick::Header);
        texts.push(g.label.clone());
        for m in g.models {
            let id = s(m, "id");
            let name = model_name(id, s(m, "name"), names);
            let checked = id == g.current;
            if checked && mark < 0 {
                mark = rows.len() as i32;
            }
            let note = if truthy(m, "free") && show_paid {
                "free"
            } else {
                ""
            };
            rows.push(OutMenuRow {
                header: false,
                text: name.as_str().into(),
                note: note.into(),
                checked,
            });
            picks.push(Pick::Retry(g.provider.clone(), id.to_string()));
            texts.push(name);
        }
    }
    if mark < 0 {
        mark = picks
            .iter()
            .position(|p| !matches!(p, Pick::Header))
            .map_or(-1, |i| i as i32);
    }
    (rows, picks, texts, mark)
}

fn is_zoomed(hwnd: isize) -> bool {
    // SAFETY: IsZoomed accepts any value.
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd as _) != 0 }
}

// ------------------------------------------------------------- the image

fn image_url(v: &Value) -> Option<String> {
    let u = match v {
        Value::String(t) => t.as_str(),
        _ => or(or(s(v, "url"), s(v, "src")), s(v, "dataUrl")),
    };
    u.starts_with("data:image/").then(|| u.to_string())
}

/// A sent screenshot's data URL to RGB pixels, off the UI thread.
fn decode_jpeg(url: &str) -> Option<(u32, u32, Vec<u8>)> {
    let (_, b64) = url.split_once(";base64,")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let mut d = jpeg_decoder::Decoder::new(&bytes[..]);
    let px = d.decode().ok()?;
    let info = d.info()?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let n = (w * h) as usize;
    if px.len() == n * 3 {
        Some((w, h, px))
    } else if px.len() == n {
        Some((w, h, px.iter().flat_map(|v| [*v, *v, *v]).collect()))
    } else {
        None
    }
}

fn to_image(w: u32, h: u32, px: &[u8]) -> Image {
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(w, h);
    for (d, p) in buf.make_mut_slice().iter_mut().zip(px.chunks_exact(3)) {
        *d = Rgb8Pixel {
            r: p[0],
            g: p[1],
            b: p[2],
        };
    }
    Image::from_rgb8(buf)
}

// ------------------------------------------------------------------ page

pub struct OutputPage {
    hwnd: isize,
    label: String,
    ui: OutputWindow,
    me: Weak<OutputPage>,
    model: Rc<VecModel<OutRow>>,
    st: RefCell<St>,
    scale: Cell<f32>,
    flush: Timer,
    tick: Timer,
}

impl OutputPage {
    pub fn new(label: &str, spec: platform::Spec) -> Result<Rc<Self>, String> {
        let (ui, hwnd) = super::create(spec, OutputWindow::new)?;
        let model = Rc::new(VecModel::<OutRow>::default());
        ui.set_rows(ModelRc::from(model.clone()));
        let page = Rc::new_cyclic(|me| OutputPage {
            hwnd,
            label: label.to_string(),
            ui,
            me: me.clone(),
            model,
            st: RefCell::new(St {
                title: "Answer".into(),
                ..St::default()
            }),
            scale: Cell::new(1.0),
            flush: Timer::default(),
            tick: Timer::default(),
        });
        page.wire();
        let me = page.me.clone();
        super::set_keys(hwnd, label, move |k| {
            me.upgrade().is_some_and(|p| p.on_key(k))
        });
        page.rebuild();
        Ok(page)
    }

    fn wire(&self) {
        let ui = &self.ui;
        let hwnd = self.hwnd;
        ui.on_drag(move || platform::begin_drag(hwnd));
        ui.on_maximize(move || platform::toggle_maximize(hwnd));
        let me = self.me.clone();
        ui.on_row_button(move |key, id, x, top, bottom| {
            if let Some(p) = me.upgrade() {
                p.on_button(&key, &id, (x, top, bottom));
            }
        });
        let me = self.me.clone();
        ui.on_row_context(move |key| {
            if let Some(p) = me.upgrade() {
                p.on_context(&key);
            }
        });
        let me = self.me.clone();
        ui.on_tool(move |t| {
            if let Some(p) = me.upgrade() {
                p.on_tool(&t);
            }
        });
        let me = self.me.clone();
        ui.on_title_context(move |x, y| {
            if let Some(p) = me.upgrade() {
                p.open_window_menu((x, y, y));
            }
        });
        let me = self.me.clone();
        ui.on_menu_picked(move |i| {
            if let Some(p) = me.upgrade() {
                p.menu_pick(i);
            }
        });
        let me = self.me.clone();
        ui.on_menu_close(move || {
            if let Some(p) = me.upgrade() {
                p.close_menu();
            }
        });
        let me = self.me.clone();
        ui.on_height_changed(move |h| {
            if let Some(p) = me.upgrade() {
                p.report_height(h);
            }
        });
    }

    // ------------------------------------------------------------ plumbing

    fn send(&self, channel: &str, payload: Value) {
        bridge::send(&self.label, channel, payload);
    }

    /// AevalA11y.say: polite for news, assertive for errors and questions.
    fn say(&self, text: &str, assertive: bool) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        let priority = if assertive { "assertive" } else { "polite" };
        self.send("a11y:announce", json!({ "text": t, "priority": priority }));
    }

    fn schedule(&self) {
        if self.flush.running() {
            return;
        }
        let me = self.me.clone();
        self.flush.start(TimerMode::SingleShot, FLUSH, move || {
            if let Some(p) = me.upgrade() {
                p.rebuild();
            }
        });
    }

    fn schedule_in(&self, after: Duration) {
        let me = self.me.clone();
        Timer::single_shot(after, move || {
            if let Some(p) = me.upgrade() {
                p.schedule();
            }
        });
    }

    /// The rows from the items, diffed into the model by key: an unchanged
    /// row is not touched, a new one is inserted where it goes.
    fn rebuild(&self) {
        let mut guard = self.st.borrow_mut();
        let st = &mut *guard;
        let now = Instant::now();
        st.flashes.retain(|(_, t)| now.duration_since(*t) < FLASH);
        let mut rows = build_rows(st, now);
        let mut found = false;
        if let Some((fk, fid)) = &st.focus {
            for r in rows.iter_mut().filter(|r| &r.key == fk) {
                for b in r.buttons.iter_mut().filter(|b| &b.id == fid && b.enabled) {
                    b.focused = true;
                    found = true;
                }
            }
        }
        if !found {
            st.focus = None;
        }
        let mut old = std::mem::take(&mut st.mirror);
        let mut i = 0;
        while i < rows.len() {
            if i < old.len() && old[i].key == rows[i].key {
                if old[i] != rows[i] {
                    self.model
                        .set_row_data(i, out_row(&rows[i], picture(st, &rows[i])));
                }
                i += 1;
                continue;
            }
            if i < old.len() && !rows[i..].iter().any(|r| r.key == old[i].key) {
                self.model.remove(i);
                old.remove(i);
                continue;
            }
            self.model
                .insert(i, out_row(&rows[i], picture(st, &rows[i])));
            old.insert(i, Row::default());
            i += 1;
        }
        while self.model.row_count() > rows.len() {
            self.model.remove(rows.len());
        }
        st.mirror = rows;
        self.ui.set_sheet_title(st.title.as_str().into());
        self.ui.set_task_kind(st.task_kind);
        self.ui.set_copy_all_shown(answer_count(st) > 1);
        self.ui
            .set_copy_all_done(st.flashes.iter().any(|(k, _)| k == "copy-all"));
        self.ui.set_target(st.target && st.mode == "panel");
    }

    /// The wait count and the busy countdown tick once a second.
    fn sync_tick(&self) {
        let on = {
            let st = self.st.borrow();
            let now = Instant::now();
            st.wait.is_some()
                || st
                    .items
                    .iter()
                    .any(|i| matches!(&i.body, Body::Error(f) if f.ends.is_some_and(|e| e > now)))
        };
        if !on {
            self.tick.stop();
            return;
        }
        if self.tick.running() {
            return;
        }
        let me = self.me.clone();
        self.tick
            .start(TimerMode::Repeated, Duration::from_secs(1), move || {
                if let Some(p) = me.upgrade() {
                    p.on_tick();
                }
            });
    }

    fn on_tick(&self) {
        let mut said = None;
        {
            let mut st = self.st.borrow_mut();
            if let Some((t0, was_said)) = st.wait {
                let elapsed = t0.elapsed();
                let who = or(&st.wait_model, "the model").to_string();
                let live = st.live;
                let mut waiting = false;
                if let Some(a) = live.and_then(|id| answer_mut(&mut st, id)) {
                    if a.raw.is_empty() {
                        waiting = true;
                        if elapsed >= WAIT_SAY {
                            let n = elapsed.as_secs_f64().round() as u64;
                            a.stage = format!("Waiting for {who} \u{b7} {n} s");
                        }
                    }
                }
                if !waiting {
                    st.wait = None;
                } else if elapsed >= WAIT_SAY && !was_said {
                    st.wait = Some((t0, true));
                    said = Some(format!("Still waiting for {who}."));
                }
            }
        }
        if let Some(t) = said {
            self.say(&t, false);
        }
        self.sync_tick();
        self.schedule();
    }

    fn report_height(&self, h: f32) {
        {
            let mut st = self.st.borrow_mut();
            if (h - st.last_height).abs() < 0.5 || h <= 0.0 {
                return;
            }
            st.last_height = h;
        }
        self.send("output:glass", json!({ "top": 0, "height": h.round() }));
    }

    fn copy(&self, text: &str, said: &str, flash: String) {
        if text.is_empty() {
            return;
        }
        if !crate::shell::sys::clipboard_write(text) {
            self.say("Couldn't copy. Try again.", true);
            return;
        }
        self.st.borrow_mut().flashes.push((flash, Instant::now()));
        self.say(said, false);
        self.schedule();
        self.schedule_in(FLASH + Duration::from_millis(20));
    }

    fn retry_here(&self, provider: &str, model: &str) {
        let mut p = json!({ "label": self.label });
        if !provider.is_empty() {
            p["provider"] = json!(provider);
        }
        if !model.is_empty() {
            p["model"] = json!(model);
        }
        self.send("query:retry", p);
    }

    fn remove_item(&self, id: u32) {
        self.st.borrow_mut().items.retain(|i| i.id != id);
        self.sync_tick();
        self.schedule();
    }

    fn load_names(&self, force: bool) {
        {
            let mut st = self.st.borrow_mut();
            if st.names_asked && !force {
                return;
            }
            st.names_asked = true;
        }
        let me = self.me.clone();
        bridge::call(&self.label, "models:list", Value::Null, move |r| {
            if let Some(p) = me.upgrade() {
                p.got_models(r.ok());
            }
        });
        let me = self.me.clone();
        bridge::call(&self.label, "settings:get", Value::Null, move |r| {
            if let Some(p) = me.upgrade() {
                let paid = r.ok().is_some_and(|v| truthy(&v, "showPaid"));
                p.st.borrow_mut().show_paid = paid;
                p.schedule();
            }
        });
    }

    fn got_models(&self, v: Option<Value>) {
        {
            let mut st = self.st.borrow_mut();
            match v {
                Some(v) => {
                    collect_names(&v, &mut st.names);
                    st.catalogue = Some(v);
                }
                None => st.catalogue_failed = true,
            }
        }
        self.refill_retry_menu();
        self.schedule();
    }

    // -------------------------------------------------------------- events

    fn on_state(&self, p: &Value) {
        if let Some(m) = p.get("mode").and_then(Value::as_str) {
            self.st.borrow_mut().mode = m.to_string();
        }
        self.schedule();
    }

    fn on_meta(&self, p: &Value) {
        if let Some(t) = p.get("target").and_then(Value::as_bool) {
            self.st.borrow_mut().target = t;
        }
        self.schedule();
    }

    /// A hidden window keeps its last frame: the sheet goes blank first and
    /// only then may the window go (`output:gone`).
    fn on_away(&self, away: bool) {
        if away {
            self.close_menu();
        }
        let seq = {
            let mut st = self.st.borrow_mut();
            st.away = away;
            st.away_seq += 1;
            st.away_seq
        };
        self.ui.set_away(away);
        if !away {
            return;
        }
        let me = self.me.clone();
        Timer::single_shot(AWAY_PAINT, move || {
            if let Some(p) = me.upgrade() {
                if p.st.borrow().away_seq == seq {
                    p.send("output:gone", Value::Null);
                }
            }
        });
    }

    fn on_user(&self, p: &Value) {
        {
            let mut st = self.st.borrow_mut();
            st.last_gen = gen_of(p);
            add_user(&mut st, s(p, "text").to_string());
        }
        self.ui.set_stick(true);
        self.schedule();
    }

    fn on_start(&self, p: &Value) {
        {
            let mut st = self.st.borrow_mut();
            st.last_gen = gen_of(p);
            clear_placeholders(&mut st);
            for i in st.items.iter_mut() {
                if let Body::Error(f) = &mut i.body {
                    f.ends = None;
                }
            }
            // A busy note is replaced by the attempt it counted down to.
            st.items
                .retain(|i| !matches!(&i.body, Body::Error(f) if f.note));
            for i in st.items.iter_mut() {
                if let Body::Answer(a) = &mut i.body {
                    a.retry = false;
                }
            }
            let model = s(p, "model");
            st.wait_model = model_name(model, s(p, "modelName"), &st.names);
            let question = st.last_user.clone();
            let id = push(
                &mut st,
                Body::Answer(Box::new(Answer {
                    busy: true,
                    question,
                    ..Answer::default()
                })),
            );
            st.live = Some(id);
            st.wait = Some((Instant::now(), false));
        }
        self.load_names(false);
        self.ui.set_stick(true);
        self.sync_tick();
        self.schedule();
    }

    fn on_stage(&self, p: &Value) {
        let mut st = self.st.borrow_mut();
        if gen_of(p) < st.last_gen {
            return;
        }
        let Some(id) = st.live else {
            return;
        };
        if let Some(a) = answer_mut(&mut st, id).filter(|a| a.raw.is_empty()) {
            a.stage = s(p, "label").to_string();
        }
        drop(st);
        self.schedule();
    }

    fn on_delta(&self, p: &Value) {
        let mut st = self.st.borrow_mut();
        let gen = gen_of(p);
        if gen < st.last_gen {
            return;
        }
        let Some(id) = st.live else {
            return;
        };
        st.last_gen = gen;
        let text = s(p, "text");
        if text.is_empty() {
            return;
        }
        if let Some(a) = answer_mut(&mut st, id) {
            a.raw.push_str(text);
            a.stage.clear();
        }
        drop(st);
        self.schedule();
    }

    fn on_done(&self, p: &Value) {
        let gen = gen_of(p);
        let mut announce = None;
        {
            let mut st = self.st.borrow_mut();
            if gen < st.last_gen {
                return;
            }
            st.wait = None;
            let now = Instant::now();
            for i in st.items.iter_mut() {
                if let Body::Error(f) = &mut i.body {
                    if let Some(e) = f.ends.take() {
                        f.message = countdown_text(&f.message, secs_left(e, now));
                    }
                }
            }
            let usage = p.get("usage").filter(|u| u.is_object()).cloned();
            if usage.is_none() {
                // Stopped while a busy note counted down.
                st.items
                    .retain(|i| !matches!(&i.body, Body::Error(f) if f.counting));
            }
            if let Some(id) = st.live.take() {
                let empty = answer_mut(&mut st, id).is_some_and(|a| a.raw.is_empty());
                if empty {
                    st.items.retain(|i| i.id != id);
                } else {
                    for i in st.items.iter_mut() {
                        if let Body::Answer(a) = &mut i.body {
                            a.retry = false;
                        }
                    }
                    let sent = p.get("sent").filter(|v| v.is_object()).cloned();
                    let msg_id = p
                        .get("msgId")
                        .filter(|v| !v.is_null())
                        .or_else(|| sent.as_ref().and_then(|v| v.get("msgId")))
                        .cloned()
                        .unwrap_or_else(|| json!(gen));
                    let offer = if truthy(p, "offerDo") {
                        s(p, "question").to_string()
                    } else {
                        String::new()
                    };
                    if let Some(a) = answer_mut(&mut st, id) {
                        a.busy = false;
                        a.stopping = false;
                        a.stage.clear();
                        if let Some(u) = &usage {
                            a.details = usage_line(u);
                        }
                        a.usage = usage;
                        a.sent = sent;
                        a.msg_id = msg_id;
                        a.retry = !a.question.is_empty();
                        a.offer = offer;
                        announce = Some(markdown::first_sentence(&a.raw));
                    }
                }
            }
        }
        if let Some(first) = announce {
            // "Answer ready. <first sentence>. Press F6 to read it." (C-37)
            let t = if first.is_empty() {
                "Answer ready. Press F6 to read it.".to_string()
            } else {
                let stop = if first.ends_with(['.', '!', '?', '\u{2026}']) {
                    ""
                } else {
                    "."
                };
                format!("Answer ready. {first}{stop} Press F6 to read it.")
            };
            self.say(&t, false);
        }
        self.load_names(false);
        self.sync_tick();
        self.schedule();
    }

    fn on_error(&self, p: &Value) {
        let (message, note) = {
            let mut st = self.st.borrow_mut();
            if gen_of(p) < st.last_gen {
                return;
            }
            st.wait = None;
            if let Some(id) = st.live.take() {
                let empty = answer_mut(&mut st, id).is_some_and(|a| a.raw.is_empty());
                if empty {
                    st.items.retain(|i| i.id != id);
                } else if let Some(a) = answer_mut(&mut st, id) {
                    a.busy = false;
                    a.stopping = false;
                    a.stage.clear();
                }
            }
            let message = s(p, "message").to_string();
            let note = s(p, "tone") == "note";
            let retry_ms = num(p, "retryInMs").unwrap_or(0.0);
            let provider = s(p, "provider").to_string();
            let details = if s(p, "details").is_empty() {
                [
                    message.as_str(),
                    s(p, "code"),
                    provider.as_str(),
                    s(p, "model"),
                ]
                .iter()
                .filter(|x| !x.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
            } else {
                s(p, "details").to_string()
            };
            let counting = note && retry_ms > 0.0;
            let ends = counting.then(|| Instant::now() + Duration::from_millis(retry_ms as u64));
            let f = Fault {
                message: message.clone(),
                note,
                counting,
                ends,
                question: s(p, "question").to_string(),
                actions: p
                    .get("actions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                provider,
                details,
                ..Fault::default()
            };
            push(&mut st, Body::Error(Box::new(f)));
            (message, note)
        };
        self.say(&message, !note);
        self.ui.set_stick(true);
        self.sync_tick();
        self.schedule();
    }

    /// Start over keeps the window (U22): a row with Undo, then the empty
    /// sheet.
    fn on_reset(&self, p: &Value) {
        let undo_ms = num(p, "undoMs").unwrap_or(UNDO_MS);
        self.close_menu();
        let seq = {
            let mut st = self.st.borrow_mut();
            st.wait = None;
            st.items.clear();
            st.live = None;
            st.trail = None;
            st.titled = false;
            st.title = "Answer".into();
            st.task_kind = false;
            st.focus = None;
            st.reset_seq += 1;
            if undo_ms <= 0.0 {
                push(&mut st, Body::Empty);
                None
            } else {
                push(
                    &mut st,
                    Body::Reset {
                        text: RESET_TEXT.into(),
                        undo: true,
                        busy: false,
                    },
                );
                Some(st.reset_seq)
            }
        };
        if let Some(seq) = seq {
            self.say(RESET_TEXT, false);
            let me = self.me.clone();
            Timer::single_shot(Duration::from_millis(undo_ms as u64), move || {
                if let Some(p) = me.upgrade() {
                    p.reset_expired(seq);
                }
            });
        }
        self.sync_tick();
        self.schedule();
    }

    fn reset_expired(&self, seq: u64) {
        {
            let mut st = self.st.borrow_mut();
            if st.reset_seq != seq {
                return;
            }
            for i in st.items.iter_mut() {
                if matches!(i.body, Body::Reset { .. }) {
                    i.body = Body::Empty;
                }
            }
        }
        self.schedule();
    }

    /// The transcript again: a reload, a history entry, an undone reset.
    fn on_sync(&self, p: &Value) {
        self.close_menu();
        let any = {
            let mut st = self.st.borrow_mut();
            clear_placeholders(&mut st);
            st.items.clear();
            st.live = None;
            st.trail = None;
            st.titled = false;
            st.wait = None;
            st.focus = None;
            let list = p.get("messages").and_then(Value::as_array);
            let mut last = None;
            for m in list.into_iter().flatten() {
                let text = s(m, "text").to_string();
                if s(m, "role") == "user" {
                    add_user(&mut st, text);
                } else if s(m, "kind") == "trail" {
                    push(&mut st, Body::Stored { text, open: false });
                } else {
                    let question = st.last_user.clone();
                    let a = Answer {
                        raw: text,
                        question,
                        ..Answer::default()
                    };
                    last = Some(push(&mut st, Body::Answer(Box::new(a))));
                }
            }
            if let Some(a) = last.and_then(|id| answer_mut(&mut st, id)) {
                a.retry = !a.question.is_empty();
            }
            list.is_some_and(|l| !l.is_empty())
        };
        if any {
            self.ui.set_stick(true);
        }
        self.sync_tick();
        self.schedule();
    }

    fn on_begin(&self, p: &Value) {
        {
            let mut st = self.st.borrow_mut();
            let task = s(p, "task");
            set_title(&mut st, true, task, false);
            new_trail(&mut st, task);
        }
        self.ui.set_stick(true);
        self.schedule();
    }

    fn on_step(&self, r: &Value) {
        let mut says: Vec<(String, bool)> = Vec::new();
        let mut step_say = None;
        {
            let mut st = self.st.borrow_mut();
            let tid = ensure_trail(&mut st);
            let Some(t) = trail_mut(&mut st, tid) else {
                return;
            };
            let id = id_str(r.get("id").unwrap_or(&NULL));
            let kind = s(r, "kind");
            match kind {
                "remove" => t.steps.retain(|x| x.id != id),
                // The final message is its own answer.
                "message" => return,
                "plan" => {
                    let steps = r
                        .get("steps")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(id_str).collect())
                        .unwrap_or_default();
                    t.plan = Some(steps);
                }
                _ => {
                    let act = is_act(kind);
                    if act {
                        t.answer = false;
                    }
                    let before = t.steps.iter().position(|x| x.id == id);
                    let n = match before {
                        Some(i) => t.steps[i].n,
                        None if act => {
                            t.count += 1;
                            t.count
                        }
                        None => 0,
                    };
                    let state = s(r, "state");
                    let text = s(r, "text");
                    if kind == "ask" && state == "running" {
                        if t.said.insert(format!("ask{id}")) {
                            let stop = if ends_in_stop(text) { "" } else { "." };
                            says.push((
                                format!(
                                    "The task has a question: {text}{stop} Type your answer in the bar."
                                ),
                                true,
                            ));
                        }
                    } else if act {
                        if state == "failed" {
                            if t.said.insert(format!("fail{id}")) {
                                let err = s(r, "error");
                                let err = if err.is_empty() {
                                    String::new()
                                } else {
                                    format!(". {err}")
                                };
                                says.push((format!("Step {n} failed: {text}{err}"), true));
                            }
                        } else if before.is_none() {
                            step_say = Some(format!("Step {n}: {text}"));
                        }
                    }
                    let step = Step {
                        id,
                        r: r.clone(),
                        n,
                    };
                    match before {
                        Some(i) => t.steps[i] = step,
                        None => t.steps.push(step),
                    }
                }
            }
        }
        for (t, assertive) in says {
            self.say(&t, assertive);
        }
        if let Some(t) = step_say {
            self.say_step(t);
        }
        self.schedule();
    }

    /// Each new act is announced as it starts, at most one every 2 s; the
    /// latest waits for the gap (U27).
    fn say_step(&self, text: String) {
        let wait = {
            let mut st = self.st.borrow_mut();
            let now = Instant::now();
            let wait = st.step_said.map_or(Duration::ZERO, |t| {
                (t + STEP_SAY_GAP).saturating_duration_since(now)
            });
            if wait.is_zero() {
                st.step_said = Some(now);
                None
            } else {
                st.step_pending = Some(text.clone());
                if st.step_waiting {
                    return;
                }
                st.step_waiting = true;
                Some(wait)
            }
        };
        let Some(wait) = wait else {
            self.say(&text, false);
            return;
        };
        let me = self.me.clone();
        Timer::single_shot(wait, move || {
            if let Some(p) = me.upgrade() {
                let next = {
                    let mut st = p.st.borrow_mut();
                    st.step_waiting = false;
                    st.step_pending.take()
                };
                if let Some(t) = next {
                    p.say_step(t);
                }
            }
        });
    }

    fn on_agent_state(&self, p: &Value) {
        let state = s(p, "state");
        let message = s(p, "message");
        let step = p.get("step").and_then(Value::as_u64).unwrap_or(0);
        let mut paused = None;
        {
            let mut st = self.st.borrow_mut();
            let Some(tid) = st.trail else {
                return;
            };
            let Some(t) = trail_mut(&mut st, tid) else {
                return;
            };
            t.status = status_text(state, step);
            if t.state != state && state == "paused" {
                paused = Some(format!("Task paused. {message}"));
            }
            t.state = state.to_string();
            if state != "confirm" {
                t.card = None;
            }
            let banner = state == "paused" || state == "plan";
            t.banner = banner.then(|| Banner {
                text: message.to_string(),
                plan: state == "plan",
                busy: false,
            });
            // Enter on this button is the only Enter that continues a run.
            if banner {
                st.focus = Some((format!("{tid}.banner"), "go".into()));
            }
        }
        if let Some(t) = paused {
            self.say(&t, false);
        }
        self.schedule();
    }

    fn on_confirm(&self, p: &Value) {
        let (heard, seq) = {
            let mut st = self.st.borrow_mut();
            let tid = ensure_trail(&mut st);
            st.card_seq += 1;
            let seq = st.card_seq;
            st.focus = None;
            let text = s(p, "text").to_string();
            let reason = s(p, "reason");
            let why = s(p, "why");
            let url = p.get("url").unwrap_or(&NULL);
            let (host, rest) = if s(url, "full").is_empty() {
                (String::new(), String::new())
            } else {
                (s(url, "host").to_string(), s(url, "rest").to_string())
            };
            let value = s(p, "value").to_string();
            let long = value.lines().count() > VALUE_LINES || value.chars().count() > VALUE_CHARS;
            let similar = truthy(p, "similar");
            let mut says = Vec::new();
            if !reason.is_empty() {
                says.push(format!("{}.", cap(reason)));
            }
            if !why.is_empty() {
                says.push(format!("Asked because {why}."));
            }
            let because = if why.is_empty() {
                String::new()
            } else {
                format!(". Asking because {why}")
            };
            let typed = if value.is_empty() {
                ""
            } else {
                " The text that will be typed is in the card."
            };
            let again = if similar { ", Allow similar" } else { "" };
            let heard =
                format!("Asking first: {text}{because}.{typed} Tab to Skip{again} or Allow.");
            let card = Card {
                id: p.get("id").cloned().unwrap_or(Value::Null),
                text,
                host,
                rest,
                seen: !(truthy(p, "truncated") && long),
                value,
                reason: says.join(" "),
                similar,
                all: false,
                settled: false,
                answered: false,
                seq,
            };
            if let Some(t) = trail_mut(&mut st, tid) {
                t.card = Some(card);
            }
            (heard, seq)
        };
        self.say(&heard, true);
        let me = self.me.clone();
        Timer::single_shot(SETTLE, move || {
            if let Some(p) = me.upgrade() {
                p.settle(seq);
            }
        });
        self.ui.set_stick(true);
        self.schedule();
    }

    fn settle(&self, seq: u64) {
        {
            let mut st = self.st.borrow_mut();
            let Some(tid) = st.trail else {
                return;
            };
            if let Some(c) = trail_mut(&mut st, tid)
                .and_then(|t| t.card.as_mut())
                .filter(|c| c.seq == seq)
            {
                c.settled = true;
            }
        }
        self.schedule();
    }

    fn on_agent_done(&self, p: &Value) {
        let state = s(p, "state");
        let text = s(p, "text");
        {
            let mut st = self.st.borrow_mut();
            st.step_pending = None;
            let tid = ensure_trail(&mut st);
            let tone = match state {
                "done" => "done",
                "error" | "limit" | "offline" => "error",
                _ => "quiet",
            };
            let mut fault = None;
            if let Some(t) = trail_mut(&mut st, tid) {
                if let Some(u) = p.get("usage").filter(|u| u.is_object()) {
                    let mut known = u.clone();
                    if !truthy(u, "costKnown") {
                        if let Some(o) = known.as_object_mut() {
                            o.remove("cost");
                        }
                    }
                    if !usage_line(&known).is_empty() {
                        let calls = num(u, "calls").unwrap_or(0.0) as u64;
                        t.usage = format!("{calls} model calls");
                    }
                }
                t.ended = true;
                t.state = state.to_string();
                t.status = match state {
                    "done" => "Done",
                    "stopped" => "Stopped",
                    _ => "Did not finish",
                }
                .into();
                t.answer = false;
                t.card = None;
                t.banner = None;
                t.steps.retain(|x| s(&x.r, "kind") != "thinking");
                // A run that failed keeps its steps open: which one failed
                // is the point.
                if t.steps.iter().any(|x| is_act(s(&x.r, "kind"))) {
                    t.open = state != "done";
                }
                if tone == "done" && !text.is_empty() {
                    t.outcome = Some((text.to_string(), markdown::lines(text)));
                } else if !text.is_empty() {
                    fault = Some(Fault {
                        message: text.to_string(),
                        quiet: tone != "error",
                        retry_task: if tone == "error" {
                            t.task.clone()
                        } else {
                            String::new()
                        },
                        ..Fault::default()
                    });
                }
            }
            if let Some(f) = fault {
                push(&mut st, Body::Error(Box::new(f)));
            }
        }
        match state {
            "done" => self.say("Task done.", false),
            "stopped" => self.say("Task stopped.", false),
            _ => self.say("The task did not finish.", true),
        }
        self.ui.set_stick(true);
        self.schedule();
    }

    /// The first run asks once, here, before anything moves.
    fn on_consent(&self, p: &Value) {
        let sends = format!(
            "Each step sends a screenshot and the window's text to {}.",
            join_names(&recipients(p))
        );
        let free = truthy(p, "free")
            || p.get("recipients")
                .and_then(Value::as_array)
                .is_some_and(|l| l.iter().any(|r| r.get("free") == Some(&Value::Bool(true))));
        let lines = vec![
            "It can see, click and type in any app, the way you would.".to_string(),
            "Anything that sends, deletes, buys, signs or opens a link is checked first, and asks you unless it is clearly what you wanted.".to_string(),
            "Move the mouse to pause it. Press Esc to stop it.".to_string(),
            if free {
                format!("{sends} Free models' providers may keep and train on what they receive.")
            } else {
                sends
            },
        ];
        {
            let mut st = self.st.borrow_mut();
            let task = s(p, "task").to_string();
            push(&mut st, Body::Consent { task, lines });
            st.focus = None;
        }
        self.say(
            "Let Aevaleron use your computer? Tab to Not now or Allow.",
            true,
        );
        self.ui.set_stick(true);
        self.schedule();
    }

    // ------------------------------------------------------------- clicks

    fn on_tool(&self, t: &str) {
        match t {
            "copy-all" => self.copy_conversation(),
            "history" => self.send("overlay:open-history", Value::Null),
            "new" => self.send("chat:new", Value::Null),
            "minimize" => self.send("overlay:minimize", Value::Null),
            "close" => self.close_window(),
            _ => {}
        }
    }

    fn copy_conversation(&self) {
        let text = transcript(&self.st.borrow());
        self.copy(&text, "Copied the conversation", "copy-all".into());
    }

    /// Close: `output:close` takes the window; without it the bar folds.
    fn close_window(&self) {
        {
            let mut st = self.st.borrow_mut();
            if st.closing {
                return;
            }
            st.closing = true;
        }
        self.close_menu();
        let me = self.me.clone();
        let label = self.label.clone();
        bridge::call(
            &self.label,
            "output:close",
            json!({ "label": label }),
            move |r| {
                if let Some(p) = me.upgrade() {
                    if r.is_err() {
                        p.send("overlay:collapse", Value::Null);
                    }
                    p.st.borrow_mut().closing = false;
                }
            },
        );
    }

    /// Stop whatever is still coming here: the answer, or the task.
    fn stop_here(&self) {
        let (live, running) = {
            let mut st = self.st.borrow_mut();
            let live = st.live;
            let running = st
                .trail
                .and_then(|id| trail_mut(&mut st, id))
                .is_some_and(|t| !t.ended);
            (live, running)
        };
        if let Some(id) = live {
            self.answer_button(id, "stop");
        } else if running {
            self.send("agent:stop", Value::Null);
        }
    }

    fn on_button(&self, key: &str, id: &str, at: (f32, f32, f32)) {
        let mut parts = key.split('.');
        let Some(item) = parts.next().and_then(|v| v.parse::<u32>().ok()) else {
            return;
        };
        let tag = parts.next().unwrap_or("");
        if let Some(url) = id.strip_prefix("open:") {
            self.send("open-external", json!({ "url": url }));
            return;
        }
        if id == "copy-code" {
            self.copy_code(item, tag, key);
            return;
        }
        let kind = {
            let st = self.st.borrow();
            st.items
                .iter()
                .find(|i| i.id == item)
                .map(|i| match &i.body {
                    Body::Answer(_) => 1,
                    Body::Error(_) => 2,
                    Body::Trail(_) => 3,
                    Body::Consent { .. } => 4,
                    Body::Reset { .. } => 5,
                    Body::Stored { .. } => 6,
                    Body::User(_) | Body::Empty => 0,
                })
        };
        match kind {
            Some(1) if id == "retry-more" => self.open_retry_menu(None, at),
            Some(1) if id == "copy" => {
                let raw = {
                    let mut st = self.st.borrow_mut();
                    answer_mut(&mut st, item)
                        .map(|a| a.raw.clone())
                        .unwrap_or_default()
                };
                self.copy(&raw, "Copied the answer", format!("{key}/{id}"));
            }
            Some(1) => self.answer_button(item, id),
            Some(2) if id == "retry-more" => self.open_retry_menu(Some(item), at),
            Some(2) => self.fault_button(item, key, id),
            Some(3) => self.trail_button(item, key, id),
            Some(4) => self.consent_button(item, id),
            Some(5) => self.undo_reset(item),
            Some(6) => {
                {
                    let mut st = self.st.borrow_mut();
                    for i in st.items.iter_mut().filter(|i| i.id == item) {
                        if let Body::Stored { open, .. } = &mut i.body {
                            *open = !*open;
                        }
                    }
                }
                self.schedule();
            }
            _ => {}
        }
    }

    fn copy_code(&self, item: u32, tag: &str, key: &str) {
        let n = tag.get(1..).and_then(|v| v.parse::<usize>().ok());
        let text = {
            let st = self.st.borrow();
            let lines = st
                .items
                .iter()
                .find(|i| i.id == item)
                .and_then(|i| match &i.body {
                    Body::Answer(a) if tag.starts_with('m') => Some(&a.md),
                    Body::Trail(t) if tag.starts_with('o') => t.outcome.as_ref().map(|o| &o.1),
                    _ => None,
                });
            match lines.zip(n).and_then(|(l, n)| l.get(n)) {
                Some(Line::Code { text, .. }) => text.clone(),
                _ => String::new(),
            }
        };
        self.copy(&text, "Copied the code", format!("{key}/copy-code"));
    }

    fn answer_button(&self, item: u32, id: &str) {
        let mut st = self.st.borrow_mut();
        let Some(a) = answer_mut(&mut st, item) else {
            return;
        };
        match id {
            "stop" => {
                if a.stopping {
                    return;
                }
                a.stopping = true;
                drop(st);
                self.send("query:cancel", Value::Null);
            }
            "do-it" => {
                let payload = json!({ "question": a.question, "answer": a.raw });
                drop(st);
                self.send("agent:do-it", payload);
            }
            "retry" => {
                drop(st);
                self.retry_here("", "");
            }
            "do-instead" => {
                if a.offer_busy {
                    return;
                }
                a.offer_busy = true;
                let task = a.offer.clone();
                drop(st);
                let me = self.me.clone();
                bridge::call(
                    &self.label,
                    "agent:start",
                    json!({ "task": task }),
                    move |r| {
                        let Some(p) = me.upgrade() else {
                            return;
                        };
                        let status = r
                            .ok()
                            .map(|v| s(&v, "status").to_string())
                            .unwrap_or_default();
                        {
                            let mut st = p.st.borrow_mut();
                            if let Some(a) = answer_mut(&mut st, item) {
                                a.offer_busy = false;
                                if status == "started" || status == "consent" {
                                    a.offer.clear();
                                }
                            }
                        }
                        p.schedule();
                    },
                );
            }
            "sent" => a.sent_open = !a.sent_open,
            "view" => {
                if !matches!(a.image, Pic::Idle) {
                    return;
                }
                a.image = Pic::Asking;
                let msg = a.msg_id.clone();
                drop(st);
                self.view_image(item, msg);
            }
            _ => {}
        }
        self.schedule();
    }

    fn set_pic(&self, item: u32, pic: Pic) {
        if let Some(a) = answer_mut(&mut self.st.borrow_mut(), item) {
            a.image = pic;
        }
        self.schedule();
    }

    fn view_image(&self, item: u32, msg_id: Value) {
        let me = self.me.clone();
        let payload = json!({ "msgId": msg_id });
        bridge::call(&self.label, "output:sent-image", payload, move |r| {
            let Some(p) = me.upgrade() else {
                return;
            };
            let Some(url) = r.ok().as_ref().and_then(image_url) else {
                p.set_pic(item, Pic::Gone);
                return;
            };
            let job = crate::shell::async_runtime::spawn_blocking(move || decode_jpeg(&url));
            let me = p.me.clone();
            let _ = slint::spawn_local(async move {
                let got = job.await.ok().flatten();
                if let Some(p) = me.upgrade() {
                    let pic = got.map_or(Pic::Gone, |(w, h, px)| Pic::Shown(to_image(w, h, &px)));
                    p.set_pic(item, pic);
                }
            });
        });
    }

    fn fault_button(&self, item: u32, key: &str, id: &str) {
        if id == "retry" {
            self.remove_item(item);
            self.retry_here("", "");
            return;
        }
        if id == "retry-task" {
            let task =
                {
                    let mut st = self.st.borrow_mut();
                    let Some(f) = st.items.iter_mut().find(|i| i.id == item).and_then(|i| {
                        match &mut i.body {
                            Body::Error(f) => Some(f),
                            _ => None,
                        }
                    }) else {
                        return;
                    };
                    if f.retry_busy {
                        return;
                    }
                    f.retry_busy = true;
                    f.retry_task.clone()
                };
            self.schedule();
            let me = self.me.clone();
            bridge::call(
                &self.label,
                "agent:start",
                json!({ "task": task }),
                move |_| {
                    if let Some(p) = me.upgrade() {
                        {
                            let mut st = p.st.borrow_mut();
                            for i in st.items.iter_mut().filter(|i| i.id == item) {
                                if let Body::Error(f) = &mut i.body {
                                    f.retry_busy = false;
                                }
                            }
                        }
                        p.schedule();
                    }
                },
            );
            return;
        }
        let Some(n) = id.strip_prefix('a').and_then(|v| v.parse::<usize>().ok()) else {
            return;
        };
        let (a, provider, details, never_ask) = {
            let st = self.st.borrow();
            let Some(f) = st
                .items
                .iter()
                .find(|i| i.id == item)
                .and_then(|i| match &i.body {
                    Body::Error(f) => Some(f),
                    _ => None,
                })
            else {
                return;
            };
            let Some(a) = f.actions.get(n) else {
                return;
            };
            (
                a.clone(),
                f.provider.clone(),
                f.details.clone(),
                f.never_ask,
            )
        };
        let aid = or(s(&a, "id"), s(&a, "action")).to_lowercase();
        let ap = or(s(&a, "provider"), &provider).to_string();
        match aid.as_str() {
            "never-ask" => {
                {
                    let mut st = self.st.borrow_mut();
                    for i in st.items.iter_mut().filter(|i| i.id == item) {
                        if let Body::Error(f) = &mut i.body {
                            f.never_ask = !f.never_ask;
                        }
                    }
                }
                self.schedule();
            }
            "fix-key" => {
                let section = format!("prov-{}", or(&ap, "openrouter"));
                self.send("overlay:open-settings", json!({ "section": section }));
            }
            "choose-model" => {
                let section = or(s(&a, "section"), "pg-models");
                self.send("overlay:open-settings", json!({ "section": section }));
            }
            "add-provider" => {
                self.send("overlay:open-settings", json!({ "section": "pg-account" }));
            }
            "add-credit" => {
                let url = or(s(&a, "url"), credit_url(&ap));
                self.send("open-external", json!({ "url": url }));
            }
            "privacy" | "open-privacy" => {
                let url = or(s(&a, "url"), PRIVACY_URL);
                self.send("open-external", json!({ "url": url }));
            }
            "retry" | "try-now" | "test-again" => {
                self.remove_item(item);
                self.retry_here("", "");
            }
            "hop" => {
                self.remove_item(item);
                let (hp, hm) = (s(&a, "provider").to_string(), s(&a, "model").to_string());
                if never_ask {
                    let me = self.me.clone();
                    let payload = json!({ "askBeforePaid": false });
                    bridge::call(&self.label, "settings:set-failover", payload, move |_| {
                        if let Some(p) = me.upgrade() {
                            p.retry_here(&hp, &hm);
                        }
                    });
                } else {
                    self.retry_here(&hp, &hm);
                }
            }
            "wait" | "keep-waiting" => {
                if !s(&a, "channel").is_empty() {
                    let payload = a.get("payload").cloned().unwrap_or(Value::Null);
                    self.send(s(&a, "channel"), payload);
                }
                self.remove_item(item);
            }
            "copy-details" => self.copy(&details, "", format!("{key}/{id}")),
            _ => {
                if !s(&a, "channel").is_empty() {
                    let payload = a.get("payload").cloned().unwrap_or(Value::Null);
                    self.send(s(&a, "channel"), payload);
                }
            }
        }
    }

    fn trail_button(&self, item: u32, key: &str, id: &str) {
        if id == "copy" {
            let text = {
                let mut st = self.st.borrow_mut();
                trail_mut(&mut st, item)
                    .and_then(|t| t.outcome.as_ref().map(|o| o.0.clone()))
                    .unwrap_or_default()
            };
            self.copy(&text, "", format!("{key}/{id}"));
            return;
        }
        let mut st = self.st.borrow_mut();
        let Some(t) = trail_mut(&mut st, item) else {
            return;
        };
        match id {
            "toggle" => t.open = !t.open,
            "answer" => {
                // Stop the task and answer the same words as a question.
                t.answer = false;
                let task = t.task.clone();
                drop(st);
                let label = self.label.clone();
                bridge::call(&self.label, "agent:stop", Value::Null, move |_| {
                    bridge::send(&label, "query:submit", json!({ "question": task }));
                });
            }
            "stop" => {
                if t.stopping {
                    return;
                }
                t.stopping = true;
                drop(st);
                self.send("agent:stop", Value::Null);
            }
            "show-all" => {
                if let Some(c) = t.card.as_mut() {
                    c.all = true;
                    c.seen = true;
                }
            }
            "skip" | "allow" | "similar" => {
                let Some(c) = t.card.as_mut() else {
                    return;
                };
                let ready = c.settled && !c.answered && (id == "skip" || c.seen);
                if !ready {
                    return;
                }
                c.answered = true;
                let mut payload = json!({ "id": c.id, "allow": id != "skip" });
                if id == "similar" {
                    payload["similar"] = json!(true);
                }
                drop(st);
                self.send("agent:confirm", payload);
            }
            "go" => {
                let Some(b) = t.banner.as_mut().filter(|b| !b.busy) else {
                    return;
                };
                b.busy = true;
                drop(st);
                let me = self.me.clone();
                bridge::call(&self.label, "agent:continue", json!({}), move |_| {
                    if let Some(p) = me.upgrade() {
                        {
                            let mut st = p.st.borrow_mut();
                            if let Some(b) =
                                trail_mut(&mut st, item).and_then(|t| t.banner.as_mut())
                            {
                                b.busy = false;
                            }
                        }
                        p.schedule();
                    }
                });
            }
            _ => {}
        }
        self.schedule();
    }

    fn consent_button(&self, item: u32, id: &str) {
        let task = {
            let mut st = self.st.borrow_mut();
            let task = st
                .items
                .iter()
                .find(|i| i.id == item)
                .and_then(|i| match &i.body {
                    Body::Consent { task, .. } => Some(task.clone()),
                    _ => None,
                });
            st.items.retain(|i| i.id != item);
            task
        };
        if let Some(task) = task {
            self.send(
                "agent:consent",
                json!({ "allow": id == "allow", "task": task }),
            );
        }
        self.schedule();
    }

    fn undo_reset(&self, item: u32) {
        {
            let mut st = self.st.borrow_mut();
            for i in st.items.iter_mut().filter(|i| i.id == item) {
                if let Body::Reset { busy, .. } = &mut i.body {
                    *busy = true;
                }
            }
        }
        self.schedule();
        let me = self.me.clone();
        let payload = json!({ "label": self.label });
        // Rust restores the conversation and replays it (chat:sync).
        bridge::call(&self.label, "output:undo-reset", payload, move |r| {
            let failed = match &r {
                Err(_) => true,
                Ok(v) => v.get("ok") == Some(&Value::Bool(false)),
            };
            let Some(p) = me.upgrade() else {
                return;
            };
            if failed {
                {
                    let mut st = p.st.borrow_mut();
                    for i in st.items.iter_mut().filter(|i| i.id == item) {
                        if let Body::Reset { text, undo, .. } = &mut i.body {
                            *text = RESET_FAILED.into();
                            *undo = false;
                        }
                    }
                }
                p.schedule();
            }
        });
    }

    fn on_context(&self, key: &str) {
        let mut parts = key.split('.');
        let Some(item) = parts.next().and_then(|v| v.parse::<u32>().ok()) else {
            return;
        };
        let tag = parts.next().unwrap_or("");
        let payload = {
            let st = self.st.borrow();
            let Some(i) = st.items.iter().find(|i| i.id == item) else {
                return;
            };
            match &i.body {
                Body::Answer(a) if !a.raw.is_empty() => json!({
                    "kind": "answer",
                    "text": a.raw,
                    "question": a.question,
                    "selection": "",
                    "details": a.details,
                }),
                Body::Trail(t) => {
                    let text = if let Some(sid) = tag.strip_prefix('s') {
                        t.steps
                            .iter()
                            .find(|x| x.id.replace('.', "_") == sid)
                            .map(|x| s(&x.r, "text").to_string())
                    } else if tag.starts_with('p') {
                        t.plan.as_ref().map(|p| p.join("; "))
                    } else {
                        None
                    };
                    let Some(text) = text else {
                        return;
                    };
                    json!({ "kind": "row", "text": text, "ended": t.ended, "task": t.task })
                }
                _ => return,
            }
        };
        self.send("ui:context-menu", payload);
    }

    // -------------------------------------------------------------- menus

    fn show_menu(&self, fill: MenuFill, empty: &str) {
        let (rows, picks, texts, mark) = fill;
        if let Some(m) = self.st.borrow_mut().menu.as_mut() {
            m.picks = picks;
            m.texts = texts;
            m.mark = mark;
        }
        self.ui.set_menu_rows(ModelRc::new(VecModel::from(rows)));
        self.ui.set_menu_mark(mark);
        self.ui.set_menu_empty(empty.into());
    }

    fn place_menu(&self, at: (f32, f32, f32)) {
        self.ui.set_menu_x(at.0);
        self.ui.set_menu_top(at.1);
        self.ui.set_menu_bottom(at.2);
        self.ui.set_menu_open(true);
    }

    /// Retry with another model (U20), under its chevron.
    fn open_retry_menu(&self, from: Option<u32>, at: (f32, f32, f32)) {
        let toggle = self.st.borrow().menu.as_ref().is_some_and(|m| !m.window);
        self.close_menu();
        if toggle {
            return;
        }
        let ask = {
            let mut st = self.st.borrow_mut();
            st.menu = Some(Menu {
                from,
                window: false,
                picks: Vec::new(),
                texts: Vec::new(),
                mark: -1,
            });
            let ask = st.catalogue.is_none();
            if ask {
                st.catalogue_failed = false;
            }
            ask
        };
        self.refill_retry_menu();
        self.place_menu(at);
        if ask {
            self.load_names(true);
        }
    }

    fn refill_retry_menu(&self) {
        let (fill, empty) = {
            let st = self.st.borrow();
            if !st.menu.as_ref().is_some_and(|m| !m.window) {
                return;
            }
            let fill = match &st.catalogue {
                Some(c) => retry_rows(c, st.show_paid, &st.names),
                None => (Vec::new(), Vec::new(), Vec::new(), -1),
            };
            let empty = if st.catalogue.is_none() && !st.catalogue_failed {
                "Loading models\u{2026}"
            } else {
                "No other models are ready."
            };
            (fill, empty)
        };
        self.show_menu(fill, empty);
    }

    /// Move, Size, Maximize or Restore, Minimize, Close (frame.js, U30).
    fn open_window_menu(&self, at: (f32, f32, f32)) {
        self.close_menu();
        let maxed = is_zoomed(self.hwnd);
        let mut names: Vec<(&str, &'static str)> = Vec::new();
        if !maxed {
            names.push(("Move", "move"));
            names.push(("Size", "size"));
        }
        names.push((if maxed { "Restore" } else { "Maximize" }, "max"));
        names.push(("Minimize", "min"));
        names.push(("Close", "close"));
        let rows: Vec<OutMenuRow> = names
            .iter()
            .map(|(t, _)| OutMenuRow {
                text: (*t).into(),
                ..OutMenuRow::default()
            })
            .collect();
        let texts: Vec<String> = names.iter().map(|(t, _)| (*t).to_string()).collect();
        let picks: Vec<Pick> = names.iter().map(|(_, c)| Pick::Win(c)).collect();
        self.st.borrow_mut().menu = Some(Menu {
            from: None,
            window: true,
            picks: Vec::new(),
            texts: Vec::new(),
            mark: 0,
        });
        self.show_menu((rows, picks, texts, 0), "");
        self.place_menu(at);
    }

    fn close_menu(&self) {
        self.st.borrow_mut().menu = None;
        self.ui.set_menu_open(false);
    }

    fn menu_pick(&self, i: i32) {
        let Ok(i) = usize::try_from(i) else {
            return;
        };
        let (pick, from) = {
            let mut st = self.st.borrow_mut();
            let Some(m) = st.menu.take() else {
                return;
            };
            (m.picks.into_iter().nth(i), m.from)
        };
        self.ui.set_menu_open(false);
        match pick {
            Some(Pick::Retry(p, m)) => {
                if let Some(id) = from {
                    self.remove_item(id);
                }
                self.retry_here(&p, &m);
            }
            Some(Pick::Win(cmd)) => match cmd {
                "move" => self.start_adjust(false),
                "size" => self.start_adjust(true),
                "max" => platform::toggle_maximize(self.hwnd),
                "min" => self.send("overlay:minimize", Value::Null),
                "close" => self.close_window(),
                _ => {}
            },
            Some(Pick::Header) | None => {}
        }
    }

    fn menu_key(&self, k: &KeyPress) -> bool {
        let (selectable, window, mark, texts) = {
            let st = self.st.borrow();
            let Some(m) = st.menu.as_ref() else {
                return false;
            };
            let sel: Vec<usize> = m
                .picks
                .iter()
                .enumerate()
                .filter(|(_, p)| !matches!(p, Pick::Header))
                .map(|(i, _)| i)
                .collect();
            (sel, m.window, m.mark, m.texts.clone())
        };
        if matches!(k.vk, vk::ESC | vk::TAB) {
            self.close_menu();
            return true;
        }
        let n = selectable.len();
        if n == 0 {
            return false;
        }
        let cur = usize::try_from(mark)
            .ok()
            .and_then(|m| selectable.iter().position(|i| *i == m));
        let next = match k.vk {
            vk::DOWN => cur.map_or(0, |p| {
                if window {
                    (p + 1) % n
                } else {
                    (p + 1).min(n - 1)
                }
            }),
            vk::UP => cur.map_or(n - 1, |p| {
                if window {
                    (p + n - 1) % n
                } else {
                    p.saturating_sub(1)
                }
            }),
            vk::HOME => 0,
            vk::END => n - 1,
            vk::ENTER | vk::SPACE => {
                if let Some(p) = cur {
                    self.menu_pick(selectable[p] as i32);
                }
                return true;
            }
            _ => {
                let Some(c) = k.ch.filter(|_| window) else {
                    return false;
                };
                let Some(p) = selectable.iter().position(|i| {
                    texts
                        .get(*i)
                        .and_then(|t| t.chars().next())
                        .is_some_and(|f| f.to_ascii_lowercase() == c)
                }) else {
                    return true;
                };
                p
            }
        };
        let to = selectable[next] as i32;
        if let Some(m) = self.st.borrow_mut().menu.as_mut() {
            m.mark = to;
        }
        self.ui.set_menu_mark(to);
        true
    }

    fn start_adjust(&self, size: bool) {
        let from = motion::glass(self.hwnd);
        self.st.borrow_mut().adjust = Some(Adjust { size, from });
        let text = if size {
            "Arrows resize the window. Enter keeps it, Esc puts it back."
        } else {
            "Arrows move the window. Enter keeps it, Esc puts it back."
        };
        self.ui.set_hint(text.into());
        self.say(text, false);
    }

    fn stop_adjust(&self) {
        self.st.borrow_mut().adjust = None;
        self.ui.set_hint(SharedString::new());
    }

    fn adjust_key(&self, k: &KeyPress) -> bool {
        let Some((size, from)) = self.st.borrow().adjust.as_ref().map(|a| (a.size, a.from)) else {
            return false;
        };
        match k.vk {
            vk::ENTER => self.stop_adjust(),
            vk::ESC => {
                // The way back is what really changed: Rust stops a window
                // at the edge of the work area.
                let now = motion::glass(self.hwnd);
                let d = (
                    from.x - now.x,
                    from.y - now.y,
                    from.width - now.width,
                    from.height - now.height,
                );
                if d.0.abs() + d.1.abs() + d.2.abs() + d.3.abs() > 0.5 {
                    let payload = json!({ "dx": d.0, "dy": d.1, "dw": d.2, "dh": d.3 });
                    self.send("window:nudge", payload);
                }
                self.stop_adjust();
            }
            vk::LEFT | vk::RIGHT | vk::UP | vk::DOWN => {
                let by = if k.shift { STEP_BIG } else { STEP };
                let (x, y) = match k.vk {
                    vk::LEFT => (-by, 0.0),
                    vk::RIGHT => (by, 0.0),
                    vk::UP => (0.0, -by),
                    _ => (0.0, by),
                };
                let payload = if size {
                    json!({ "dx": 0, "dy": 0, "dw": x, "dh": y })
                } else {
                    json!({ "dx": x, "dy": y, "dw": 0, "dh": 0 })
                };
                self.send("window:nudge", payload);
            }
            _ => return false,
        }
        true
    }

    // --------------------------------------------------------------- keys

    fn focusables(&self) -> Vec<(String, String)> {
        let st = self.st.borrow();
        st.mirror
            .iter()
            .flat_map(|r| {
                r.buttons
                    .iter()
                    .filter(|b| b.enabled)
                    .map(move |b| (r.key.clone(), b.id.clone()))
            })
            .collect()
    }

    fn move_focus(&self, back: bool) -> bool {
        let mut list = self.focusables();
        if list.is_empty() {
            return false;
        }
        let n = list.len();
        {
            let mut st = self.st.borrow_mut();
            let at = st
                .focus
                .as_ref()
                .and_then(|f| list.iter().position(|x| x == f));
            let i = match (at, back) {
                (None, false) => 0,
                (None, true) => n - 1,
                (Some(i), false) => (i + 1) % n,
                (Some(i), true) => (i + n - 1) % n,
            };
            st.focus = Some(list.swap_remove(i));
        }
        self.schedule();
        true
    }

    fn anchor(&self) -> (f32, f32, f32) {
        let top = 44.0 * self.scale.get();
        (24.0, top, top)
    }

    fn on_key(&self, k: &KeyPress) -> bool {
        if self.st.borrow().adjust.is_some() {
            return self.adjust_key(k);
        }
        if self.st.borrow().menu.is_some() {
            return self.menu_key(k);
        }
        if k.alt && !k.ctrl && !k.meta && !k.shift && k.vk == vk::SPACE {
            self.open_window_menu((8.0, 0.0, 44.0 * self.scale.get()));
            return true;
        }
        if k.vk == vk::ESC && !k.ctrl && !k.alt {
            // Esc moves focus and never closes or stops anything (U3).
            let panel = self.st.borrow().mode == "panel";
            if panel {
                self.send("ui:focus-bar", Value::Null);
            } else {
                self.send("output:blur", json!({ "label": self.label }));
            }
            return true;
        }
        if k.vk == vk::F6 {
            self.send("ui:focus-bar", Value::Null);
            return true;
        }
        if k.ctrl && !k.alt && !k.meta {
            return self.chord(k);
        }
        if k.ctrl || k.alt || k.meta {
            return false;
        }
        match k.vk {
            vk::TAB => self.move_focus(k.shift),
            vk::ENTER | vk::SPACE => {
                let Some((key, id)) = self.st.borrow().focus.clone() else {
                    return false;
                };
                self.on_button(&key, &id, self.anchor());
                true
            }
            vk::PGDN | vk::PGUP | vk::DOWN | vk::UP => {
                let view = self.ui.get_view_height();
                let by = match k.vk {
                    vk::PGDN => view * 0.9,
                    vk::PGUP => -view * 0.9,
                    vk::DOWN => 60.0,
                    _ => -60.0,
                };
                self.ui.invoke_scroll_by(by);
                true
            }
            vk::END => {
                self.ui.invoke_scroll_end();
                true
            }
            vk::HOME => {
                self.ui.invoke_scroll_home();
                true
            }
            _ => false,
        }
    }

    /// Output-window chords (U19).
    fn chord(&self, k: &KeyPress) -> bool {
        match (k.vk, k.shift) {
            (0x4E, false) => self.send("ui:focus-bar", json!({ "newConversation": true })),
            (vk::TAB, back) => {
                let dir = if back { -1 } else { 1 };
                self.send("output:cycle-target", json!({ "dir": dir }));
            }
            (0x57, true) => self.send("output:close-all", Value::Null),
            (0x57, false) => self.close_window(),
            (0x43, true) => self.copy_conversation(),
            (vk::PERIOD, false) => self.stop_here(),
            (0x52, true) => self.retry_here("", ""),
            (vk::COMMA, false) => self.send("overlay:open-settings", json!({})),
            (0x48, false) => self.send("overlay:open-history", Value::Null),
            (0x4D, false) => self.send("overlay:minimize", Value::Null),
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------- probes

    fn texts(&self) -> String {
        let st = self.st.borrow();
        let mut out = vec![st.title.clone()];
        for r in &st.mirror {
            for t in [&r.text, &r.sub, &r.extra, &r.value, &r.note] {
                out.push(t.clone());
            }
            out.extend(r.lines.iter().cloned());
            out.extend(r.cells.iter().cloned());
            out.extend(r.buttons.iter().map(|b| b.text.clone()));
        }
        out.retain(|t| !t.is_empty());
        out.join("\n")
    }

    fn trail_probe(&self) -> Value {
        let st = self.st.borrow();
        let Some(t) = st.trail.and_then(|id| {
            st.items
                .iter()
                .find(|i| i.id == id)
                .and_then(|i| match &i.body {
                    Body::Trail(t) => Some(t),
                    _ => None,
                })
        }) else {
            return Value::Null;
        };
        let states = plan_states(t);
        let plan: Vec<Value> = t
            .plan
            .iter()
            .flatten()
            .enumerate()
            .map(|(i, text)| json!({ "text": text, "state": states.get(i).copied().unwrap_or("pending") }))
            .collect();
        let steps: Vec<Value> = t
            .steps
            .iter()
            .map(|x| {
                json!({
                    "id": x.id,
                    "kind": s(&x.r, "kind"),
                    "state": s(&x.r, "state"),
                    "text": s(&x.r, "text"),
                    "n": x.n,
                })
            })
            .collect();
        json!({
            "task": t.task,
            "state": t.state,
            "status": t.status,
            "ended": t.ended,
            "open": t.open,
            "plan": plan,
            "steps": steps,
            "outcome": t.outcome.as_ref().map(|o| o.0.clone()),
            "banner": t.banner.as_ref().map(|b| b.text.clone()),
            "usage": t.usage,
        })
    }

    fn card_probe(&self) -> Value {
        let st = self.st.borrow();
        let card = st.trail.and_then(|id| {
            st.items
                .iter()
                .find(|i| i.id == id)
                .and_then(|i| match &i.body {
                    Body::Trail(t) => t.card.as_ref(),
                    _ => None,
                })
        });
        card.map_or(Value::Null, |c| {
            json!({
                "id": c.id,
                "text": c.text,
                "host": c.host,
                "rest": c.rest,
                "value": c.value,
                "reason": c.reason,
                "similar": c.similar,
                "seen": c.seen,
                "ready": c.settled && !c.answered,
                "answered": c.answered,
            })
        })
    }

    fn more(&self) -> bool {
        let ui = &self.ui;
        ui.get_scroll_height() - ui.get_scroll_top() - ui.get_view_height() > 8.0
    }
}

fn md_probe(src: &str) -> Value {
    let lines = markdown::lines(src);
    let kinds: Vec<&str> = lines
        .iter()
        .map(|l| match l {
            Line::Heading { .. } => "h",
            Line::Para { .. } => "p",
            Line::Item { .. } => "li",
            Line::Quote(_) => "quote",
            Line::Rule => "hr",
            Line::Code { .. } => "code",
            Line::Table { .. } => "table",
        })
        .collect();
    let heads: Vec<usize> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Heading { level, .. } => Some(*level),
            _ => None,
        })
        .collect();
    let items = lines
        .iter()
        .filter(|l| matches!(l, Line::Item { depth: 0, .. }))
        .count();
    let nested = lines
        .iter()
        .any(|l| matches!(l, Line::Item { depth, .. } if *depth > 0));
    let table = lines
        .iter()
        .find_map(|l| match l {
            Line::Table { aligns, .. } => Some(
                aligns
                    .iter()
                    .map(|a| a.as_str())
                    .collect::<Vec<_>>()
                    .join("|"),
            ),
            _ => None,
        })
        .unwrap_or_default();
    let host = lines
        .iter()
        .find_map(|l| match l {
            Line::Heading { text, .. }
            | Line::Para { text, .. }
            | Line::Item { text, .. }
            | Line::Quote(text) => text.links.first().map(|k| k.host.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let text: Vec<String> = lines.iter().map(markdown::plain).collect();
    json!({
        "kinds": kinds,
        "heads": heads,
        "items": items,
        "nested": nested,
        "table": table,
        "host": host,
        "text": text.join("\n"),
    })
}

impl super::Page for OutputPage {
    fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn on_event(&self, channel: &str, p: &Value) {
        match channel {
            "overlay:state" => self.on_state(p),
            "output:meta" => self.on_meta(p),
            "output:away" => self.on_away(truthy(p, "away")),
            "chat:user" => self.on_user(p),
            "query:start" => self.on_start(p),
            "query:stage" => self.on_stage(p),
            "query:delta" => self.on_delta(p),
            "query:done" => self.on_done(p),
            "query:error" => self.on_error(p),
            "chat:reset" => self.on_reset(p),
            "chat:sync" => self.on_sync(p),
            "agent:begin" => self.on_begin(p),
            "agent:step" => self.on_step(p),
            "agent:state" => self.on_agent_state(p),
            "agent:confirm-request" => self.on_confirm(p),
            "agent:done" => self.on_agent_done(p),
            "agent:consent-request" => self.on_consent(p),
            _ => {}
        }
    }

    fn theme(&self, dark: bool, text_scale: f32) {
        self.scale.set(text_scale);
        super::apply_theme(&self.ui, dark, text_scale);
    }

    fn probe(&self, key: &str) -> Value {
        if let Some(src) = key.strip_prefix("md:") {
            return md_probe(src);
        }
        let ui = &self.ui;
        let shown = || super::history::is_visible(self.hwnd) && !self.st.borrow().away;
        match key {
            "text" => json!(self.texts()),
            "visible" | "shown" => json!(shown()),
            "messages" => json!(self
                .st
                .borrow()
                .items
                .iter()
                .filter(|i| !matches!(i.body, Body::Empty))
                .count()),
            "streaming" => json!(self.st.borrow().live.is_some()),
            "title" => json!(self.st.borrow().title),
            "kind" => json!(if self.st.borrow().task_kind {
                "task"
            } else {
                "answer"
            }),
            "trail" => self.trail_probe(),
            "card" => self.card_probe(),
            "consent" => self
                .st
                .borrow()
                .items
                .iter()
                .find_map(|i| match &i.body {
                    Body::Consent { task, .. } => Some(json!(task)),
                    _ => None,
                })
                .unwrap_or(Value::Null),
            "scrollTop" => json!(ui.get_scroll_top()),
            "scrollHeight" => json!(ui.get_scroll_height()),
            "clientHeight" => json!(ui.get_view_height()),
            "contentHeight" => json!(ui.get_content_height()),
            "atBottom" => json!(!self.more()),
            "more" => json!(self.more()),
            "stick" => json!(ui.get_stick()),
            "rows" => json!(self.model.row_count()),
            "measure" => json!(ui.get_column_width()),
            "inputs" => json!(0),
            "copyAll" => json!(answer_count(&self.st.borrow()) > 1),
            "target" => json!(ui.get_target()),
            "menu" => self
                .st
                .borrow()
                .menu
                .as_ref()
                .map_or(Value::Null, |m| json!(m.texts)),
            _ => Value::Null,
        }
    }

    fn close(&self) {
        self.flush.stop();
        self.tick.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_names_read_like_names() {
        let none = HashMap::new();
        assert_eq!(
            model_name("google/gemma-3-27b-it:free", "", &none),
            "Gemma 3 27B"
        );
        assert_eq!(model_name("x", "Given", &none), "Given");
        assert_eq!(provider_name("openrouter", ""), "OpenRouter");
        assert_eq!(provider_name("groq", ""), "Groq");
    }

    #[test]
    fn countdown_counts_the_last_seconds() {
        assert_eq!(
            countdown_text("Busy. Trying again in 20 s.", 7),
            "Busy. Trying again in 7 s."
        );
        assert_eq!(countdown_text("Busy.", 3), "Busy. Trying again in 3 s.");
    }

    #[test]
    fn usage_line_says_everything() {
        let u = json!({ "ttftMs": 900, "ms": 3200, "prompt": 1000, "completion": 204,
            "cost": 0, "model": "m/x", "provider": "openrouter" });
        assert_eq!(
            usage_line(&u),
            "0.9 s to first word \u{b7} 3.2 s \u{b7} 1,204 tokens \u{b7} free \u{b7} m/x \u{b7} via OpenRouter"
        );
    }

    #[test]
    fn plan_items_follow_their_acts() {
        let mut t = Trail {
            plan: Some(vec!["a".into(), "b".into(), "c".into()]),
            ..Trail::default()
        };
        t.steps.push(Step {
            id: "1".into(),
            r: json!({ "kind": "act", "state": "done", "planIndex": 0 }),
            n: 1,
        });
        t.steps.push(Step {
            id: "2".into(),
            r: json!({ "kind": "act", "state": "running", "planIndex": 1 }),
            n: 2,
        });
        assert_eq!(plan_states(&t), vec!["done", "running", "pending"]);
        t.ended = true;
        t.state = "stopped".into();
        assert_eq!(plan_states(&t), vec!["done", "pending", "pending"]);
    }

    #[test]
    fn names_join_like_a_sentence() {
        assert_eq!(join_names(&[]), "the model set for tasks");
        let two = ["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(join_names(&two), "A, B and C");
        let p = json!({ "recipients": [{ "providerName": "OpenRouter", "modelName": "Gemma" }] });
        assert_eq!(recipients(&p), vec!["OpenRouter (Gemma)".to_string()]);
    }

    #[test]
    fn the_markdown_probe_reads_the_structure() {
        let v = md_probe("# One\n\n1. a\n2. b\n   - c\n\n| A | B |\n|:---|---:|\n| x | y |");
        assert_eq!(v["items"], json!(2));
        assert_eq!(v["nested"], json!(true));
        assert_eq!(v["table"], json!("left|right"));
    }
}

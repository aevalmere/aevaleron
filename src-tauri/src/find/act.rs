//! Acting on a Find hit: what Enter does on the selected hit, the menu
//! commands Find lists after what is on screen, and the one desktop call
//! that carries an action out.
//!
//! Acting is always the person's own Enter on a hit they can see. Nothing
//! here runs without that key press, and none of it goes through the
//! agent's permission system: these are the plain desktop calls Find
//! already reads with.

use serde_json::{json, Value};

use super::Rect;
use crate::agent::seams::{DeskError, DesktopOps};

/// Levels of a menu Find reads: the bar, and three under it.
pub const MENU_DEPTH: usize = 4;
/// Menu commands kept from one window.
pub const MENU_MAX: usize = 400;
/// What the helper may spend reading a window's menu, milliseconds.
pub const MENU_BUDGET_MS: u64 = 300;
/// How long Find waits for that read, queue included, before it goes on
/// without menu commands.
pub const MENU_WAIT_MS: u64 = 2500;
/// The line under a menu command's row.
pub const MENU_WHY: &str = "Menu command";

/// The word a row shows for what Enter will do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    Press,
    Focus,
    Select,
    Click,
}

impl Verb {
    pub fn word(self) -> &'static str {
        match self {
            Verb::Press => "Press",
            Verb::Focus => "Focus",
            Verb::Select => "Select",
            Verb::Click => "Click",
        }
    }
}

/// An element as the helper described it: its role and the patterns it
/// answers to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Target {
    pub r#ref: String,
    pub role: String,
    pub pat: Vec<String>,
    pub enabled: bool,
}

impl Target {
    /// From a span or its `via`: `{ref, role, pat, enabled}`. Nothing
    /// without a ref.
    pub fn from_value(v: &Value) -> Option<Target> {
        let r = v["ref"].as_str().filter(|r| !r.is_empty())?;
        let names = v["pat"].as_array().into_iter().flatten();
        let pat = names.filter_map(Value::as_str).map(str::to_string);
        Some(Target {
            r#ref: r.to_string(),
            role: v["role"].as_str().unwrap_or_default().to_string(),
            pat: pat.collect(),
            enabled: v["enabled"] != false,
        })
    }

    fn has(&self, pattern: &str) -> bool {
        self.pat.iter().any(|p| p == pattern)
    }
}

/// What a span of the snapshot is, as far as acting on it goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanAct {
    /// The span's own element; none for words text recognition found.
    pub own: Option<Target>,
    /// The button, link or item the span sits in.
    pub via: Option<Target>,
    /// A line of a document or editor whose text can be selected.
    pub line: bool,
}

impl SpanAct {
    pub fn from_span(sp: &Value) -> SpanAct {
        SpanAct {
            own: Target::from_value(sp),
            via: Target::from_value(&sp["via"]),
            line: sp["line"] == true,
        }
    }
}

/// Why a hit cannot be acted on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The application has it turned off.
    Disabled,
    /// It has no element and no place on screen.
    NoPlace,
}

/// How one hit is acted on.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// A UI Automation pattern: `invoke`, `toggle`, `select` or `expand`.
    Pattern { r#ref: String, action: &'static str },
    /// The keyboard goes to a text field.
    Focus { r#ref: String },
    /// Words in a document are brought into view and selected: match `n`
    /// as the helper numbered it, or the match nearest a point. Nothing is
    /// clicked.
    SelectText {
        r#ref: String,
        query: String,
        n: Option<usize>,
        near: Option<(f64, f64)>,
    },
    /// A real click at the middle of an element no pattern applies to.
    ClickElement { r#ref: String },
    /// A real click at a point: text recognition found the words and there
    /// is no element behind them.
    ClickPoint { x: f64, y: f64 },
    /// A menu command, by its place among the commands read.
    Menu { index: usize },
    /// Nothing to do.
    Nothing { why: Why },
}

impl Act {
    pub fn verb(&self) -> Option<Verb> {
        match self {
            Act::Pattern { action, .. } if *action == "select" => Some(Verb::Select),
            Act::Pattern { .. } | Act::Menu { .. } => Some(Verb::Press),
            Act::Focus { .. } => Some(Verb::Focus),
            Act::SelectText { .. } => Some(Verb::Select),
            Act::ClickElement { .. } | Act::ClickPoint { .. } => Some(Verb::Click),
            Act::Nothing { .. } => None,
        }
    }

    /// The word for the row; empty when Enter would do nothing.
    pub fn word(&self) -> &'static str {
        self.verb().map_or("", Verb::word)
    }

    /// The element that has to be there still for the action to land.
    pub fn element(&self) -> Option<&str> {
        match self {
            Act::Pattern { r#ref, .. }
            | Act::Focus { r#ref }
            | Act::SelectText { r#ref, .. }
            | Act::ClickElement { r#ref } => Some(r#ref.as_str()),
            _ => None,
        }
    }
}

/// The pattern an element is acted on with, from its control type and the
/// patterns it has. A text field takes the keyboard. A tick box is toggled.
/// An item of a list, a tab strip or a tree, and a radio button, is
/// selected rather than invoked: invoking a file in a list opens it. A menu
/// item that holds a submenu is opened. Anything else goes Invoke, Toggle,
/// SelectionItem, ExpandCollapse. `None` when no pattern applies.
pub fn element_action(t: &Target) -> Option<Act> {
    let r = || t.r#ref.clone();
    let pattern = |action: &'static str| Act::Pattern { r#ref: r(), action };
    match t.role.as_str() {
        "edit" | "document" => return Some(Act::Focus { r#ref: r() }),
        "combobox" if t.has("value") => return Some(Act::Focus { r#ref: r() }),
        "checkbox" if t.has("toggle") => return Some(pattern("toggle")),
        "radiobutton" | "tabitem" | "listitem" | "treeitem" | "dataitem"
            if t.has("selectionitem") =>
        {
            return Some(pattern("select"));
        }
        "menuitem" if t.has("expandcollapse") => return Some(pattern("expand")),
        _ => {}
    }
    const ORDER: [(&str, &str); 4] = [
        ("invoke", "invoke"),
        ("toggle", "toggle"),
        ("selectionitem", "select"),
        ("expandcollapse", "expand"),
    ];
    let first = ORDER.iter().find(|(has, _)| t.has(has));
    first.map(|(_, action)| pattern(*action))
}

/// A pattern on an element the application has turned off does nothing.
fn if_enabled(t: &Target, act: Act) -> Act {
    if t.enabled {
        act
    } else {
        Act::Nothing { why: Why::Disabled }
    }
}

fn centre(rect: Option<Rect>) -> Option<(f64, f64)> {
    rect.map(|r| (r.x + r.width / 2.0, r.y + r.height / 2.0))
}

/// What Enter does on a hit in the snapshot. Words text recognition found
/// get one click at their middle. A line of a document has its words
/// selected. An element is acted on through its own pattern, else through
/// the button, link or item it sits in, and only when neither has one is it
/// clicked.
pub fn plan_span(info: &SpanAct, ocr: bool, rect: Option<Rect>, matched: &str) -> Act {
    let Some(own) = info.own.as_ref().filter(|_| !ocr) else {
        return match centre(rect) {
            Some((x, y)) => Act::ClickPoint { x, y },
            None => Act::Nothing { why: Why::NoPlace },
        };
    };
    if info.line {
        return Act::SelectText {
            r#ref: own.r#ref.clone(),
            query: matched.to_string(),
            n: None,
            near: centre(rect),
        };
    }
    for t in [Some(own), info.via.as_ref()].into_iter().flatten() {
        if let Some(act) = element_action(t) {
            return if_enabled(t, act);
        }
    }
    Act::ClickElement {
        r#ref: own.r#ref.clone(),
    }
}

/// What Enter does on a whole-document match: its words are selected,
/// unless they sit in a link or a button, which is pressed.
pub fn plan_doc(doc: &str, n: usize, query: &str, via: Option<&Target>) -> Act {
    if let Some(t) = via {
        if let Some(act) = element_action(t) {
            return if_enabled(t, act);
        }
    }
    Act::SelectText {
        r#ref: doc.to_string(),
        query: query.to_string(),
        n: Some(n),
        near: None,
    }
}

// ----------------------------------------------------------------- menus

/// One command from a window's menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuCommand {
    /// The names from the menu bar down, as a person reads them.
    pub path: Vec<String>,
    /// What the helper needs to run it: `{source, path, pos}`, the names as
    /// the application wrote them.
    pub item: Value,
    pub window: Value,
}

/// A menu item's name as a person reads it: the shortcut after the tab is
/// dropped, and so is the dots that say a dialog follows. In a classic
/// menu a single `&` marks the access key and `&&` is an ampersand.
pub fn clean_menu_name(raw: &str, classic: bool) -> String {
    let name = raw.split('\t').next().unwrap_or_default();
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if classic && c == '&' {
            if chars.peek() == Some(&'&') {
                out.push('&');
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    let t = out.trim().trim_end_matches("...");
    t.trim_end_matches('\u{2026}').trim_end().to_string()
}

fn known_limit(v: &Value) -> Option<&'static str> {
    match v.as_str() {
        Some("time") => Some("time"),
        Some("count") => Some("count"),
        Some("depth") => Some("depth"),
        _ => None,
    }
}

/// The commands in one menu read, and the bound that cut the read short,
/// if one did. The menu bar itself is left out (it is on screen, and Find
/// has it already), and so is an item that only opens a submenu. Nothing
/// deeper than `depth` levels and no more than `max` commands are kept,
/// whatever the helper sent.
pub fn menu_commands(
    read: &Value,
    window: &Value,
    depth: usize,
    max: usize,
) -> (Vec<MenuCommand>, Option<&'static str>) {
    let source = read["source"].as_str().unwrap_or("win32");
    let classic = source != "uia";
    let mut limit = known_limit(&read["limit"]);
    let mut out: Vec<MenuCommand> = Vec::new();
    for it in read["items"].as_array().into_iter().flatten() {
        let names = it["path"].as_array().into_iter().flatten();
        let raw: Vec<&str> = names.filter_map(Value::as_str).collect();
        if raw.len() < 2 || it["sub"] == true {
            continue;
        }
        if raw.len() > depth {
            limit = limit.or(Some("depth"));
            continue;
        }
        if out.len() >= max {
            limit = limit.or(Some("count"));
            break;
        }
        let clean = raw.iter().map(|n| clean_menu_name(n, classic));
        let path: Vec<String> = clean.collect();
        if path.iter().any(String::is_empty) || out.iter().any(|c| c.path == path) {
            continue;
        }
        let item = json!({ "source": source, "path": it["path"], "pos": it["pos"] });
        out.push(MenuCommand {
            path,
            item,
            window: window.clone(),
        });
    }
    (out, limit)
}

/// The line that says a window's menus were not read in full. `skipped` is
/// Find's own: the read did not come back in time.
pub fn menu_note(limit: Option<&str>) -> Option<&'static str> {
    match limit {
        Some("skipped") => {
            Some("This app's menus took too long to read, so menu commands are not listed.")
        }
        Some("time") => Some("This app's menus were slow to read, so only some are listed."),
        Some("count") => Some("This app has more menu commands than Find lists."),
        Some("depth") => Some("Menus more than four levels down are not listed."),
        _ => None,
    }
}

/// "1 menu command", "3 menu commands".
pub fn menu_count(n: usize) -> String {
    match n {
        1 => "1 menu command".to_string(),
        n => format!("{n} menu commands"),
    }
}

// ------------------------------------------------------------- carrying out

/// One action, ready to carry out.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub act: Act,
    /// The window the hit is in.
    pub window: Value,
    /// A menu command's `{source, path, pos}`.
    pub menu: Option<Value>,
    /// What was typed, to give back if the action fails.
    pub query: String,
}

/// Why Enter did nothing, before anything was touched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    NoHit,
    Changed,
    Gone,
    Disabled,
    NoPlace,
}

impl Refusal {
    pub fn status(self) -> &'static str {
        match self {
            Refusal::NoHit => "no-hit",
            Refusal::Changed => "changed",
            Refusal::Gone => "gone",
            Refusal::Disabled => "disabled",
            Refusal::NoPlace => "no-place",
        }
    }

    /// What the bar says.
    pub fn message(self) -> &'static str {
        match self {
            Refusal::NoHit => "No match is selected. Type what to find first.",
            Refusal::Changed => {
                "The matches changed just now, so nothing was done. Press Enter again."
            }
            Refusal::Gone => GONE,
            Refusal::Disabled => DISABLED,
            Refusal::NoPlace => {
                "That match has no place on screen to act on. Scroll to it and press Enter again."
            }
        }
    }
}

const GONE: &str =
    "That is no longer in the window, so nothing was done. The matches were read again.";
const DISABLED: &str = "That is turned off in the app right now, so nothing was done.";

/// What the bar says when the desktop refused or failed an action: the
/// status for the page and the sentence.
pub fn failure(e: &DeskError) -> (&'static str, &'static str) {
    match e.code.as_str() {
        "BLOCKED_OWN_WINDOW" => (
            "own-window",
            "That is part of Aevaleron itself. Find does not act on its own windows.",
        ),
        "ELEVATED_TARGET" => (
            "elevated",
            "That window runs as administrator, so Aevaleron cannot act in it.",
        ),
        "SECURE_DESKTOP" => (
            "secure",
            "Windows is showing a sign-in or permission prompt. Answer it, then try again.",
        ),
        "OCCLUDED" => (
            "covered",
            "Another window covers that spot, so nothing was clicked. Bring the app forward and try again.",
        ),
        "STALE_REF" | "NOT_FOUND" => ("gone", GONE),
        "DISABLED" => ("disabled", DISABLED),
        _ => (
            "failed",
            "The app did not take that, so it may not have happened. Check the window, then try again.",
        ),
    }
}

/// Carries one action out. An element that said it had a pattern and then
/// refused it gets the real click at its middle instead.
pub async fn perform(desktop: &dyn DesktopOps, plan: &Plan) -> Result<Value, DeskError> {
    let click = |r: &str| json!({ "t": { "ref": r }, "opts": { "physical": true } });
    match &plan.act {
        Act::Pattern { r#ref, action } => {
            let args = json!({ "ref": r#ref, "action": action });
            match desktop.op("invoke", args).await {
                Err(e) if e.code == "NO_PATTERN" => desktop.op("click", click(r#ref)).await,
                other => other,
            }
        }
        Act::Focus { r#ref } => {
            let args = json!({ "ref": r#ref, "action": "focusNear" });
            desktop.op("invoke", args).await
        }
        Act::SelectText {
            r#ref,
            query,
            n,
            near,
        } => {
            let mut args = json!({ "ref": r#ref, "query": query, "n": n.unwrap_or(0) });
            if let (None, Some((x, y))) = (n, near) {
                args["x"] = json!(x.round());
                args["y"] = json!(y.round());
            }
            desktop.op("selectText", args).await
        }
        Act::ClickElement { r#ref } => desktop.op("click", click(r#ref)).await,
        Act::ClickPoint { x, y } => {
            let t = json!({ "x": x.round(), "y": y.round() });
            // The click lands in the hit's own window or not at all.
            let opts = json!({ "inWindow": plan.window });
            desktop.op("click", json!({ "t": t, "opts": opts })).await
        }
        Act::Menu { .. } => {
            let Some(item) = &plan.menu else {
                return Err(DeskError::new("NOT_FOUND", "that menu command is gone"));
            };
            let args = json!({
                "windowHandle": plan.window,
                "source": item["source"],
                "path": item["path"],
                "pos": item["pos"],
            });
            desktop.op("menuInvoke", args).await
        }
        Act::Nothing { .. } => Err(DeskError::new("NO_PATTERN", "nothing to do on that hit")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::seams::testing::FakeDesktop;

    fn target(role: &str, pat: &[&str]) -> Target {
        Target {
            r#ref: "ref_7".into(),
            role: role.into(),
            pat: pat.iter().map(|p| p.to_string()).collect(),
            enabled: true,
        }
    }

    fn action(role: &str, pat: &[&str]) -> Option<(&'static str, &'static str)> {
        let act = element_action(&target(role, pat))?;
        let how = match &act {
            Act::Pattern { action, .. } => *action,
            Act::Focus { .. } => "focus",
            _ => "other",
        };
        Some((how, act.word()))
    }

    #[test]
    fn the_pattern_follows_the_control_type() {
        // Pressed: Invoke first, whatever else is there.
        assert_eq!(action("button", &["invoke"]), Some(("invoke", "Press")));
        let both = ["toggle", "invoke"];
        assert_eq!(action("button", &both), Some(("invoke", "Press")));
        assert_eq!(action("hyperlink", &["invoke"]), Some(("invoke", "Press")));
        // A button that only toggles, a split button that only opens.
        assert_eq!(action("button", &["toggle"]), Some(("toggle", "Press")));
        let open = ["expandcollapse"];
        assert_eq!(action("splitbutton", &open), Some(("expand", "Press")));
        // A tick box is toggled even when it can be invoked.
        let tick = ["invoke", "toggle"];
        assert_eq!(action("checkbox", &tick), Some(("toggle", "Press")));
        // Items are selected, not opened.
        let item = ["invoke", "selectionitem"];
        assert_eq!(action("listitem", &item), Some(("select", "Select")));
        assert_eq!(action("tabitem", &item), Some(("select", "Select")));
        assert_eq!(action("treeitem", &item), Some(("select", "Select")));
        let radio = ["selectionitem"];
        assert_eq!(action("radiobutton", &radio), Some(("select", "Select")));
        // An item with nothing to select falls back to the order.
        assert_eq!(action("listitem", &["invoke"]), Some(("invoke", "Press")));
        // A menu that holds a submenu is opened; a command is invoked.
        let parent = ["invoke", "expandcollapse"];
        assert_eq!(action("menuitem", &parent), Some(("expand", "Press")));
        assert_eq!(action("menuitem", &["invoke"]), Some(("invoke", "Press")));
        // A text field takes the keyboard.
        assert_eq!(action("edit", &["value"]), Some(("focus", "Focus")));
        assert_eq!(action("document", &["text"]), Some(("focus", "Focus")));
        assert_eq!(action("combobox", &["value"]), Some(("focus", "Focus")));
        let list = ["expandcollapse"];
        assert_eq!(action("combobox", &list), Some(("expand", "Press")));
        // Plain text has no pattern.
        assert_eq!(action("text", &[]), None);
        assert_eq!(action("text", &["text"]), None);
    }

    fn rect() -> Option<Rect> {
        Some(Rect {
            x: 100.0,
            y: 40.0,
            width: 60.0,
            height: 20.0,
        })
    }

    #[test]
    fn a_hit_is_planned_from_what_it_is() {
        // Text recognition: one click at the middle, whatever the span says.
        let ocr = SpanAct::default();
        let click = Act::ClickPoint { x: 130.0, y: 50.0 };
        assert_eq!(plan_span(&ocr, true, rect(), "Save"), click);
        assert_eq!(click.word(), "Click");
        let none = plan_span(&ocr, true, None, "Save");
        assert_eq!(none, Act::Nothing { why: Why::NoPlace });
        assert_eq!(none.word(), "");
        // A span that carries an element but came from recognition clicks.
        let odd = SpanAct {
            own: Some(target("button", &["invoke"])),
            ..Default::default()
        };
        assert_eq!(plan_span(&odd, true, rect(), "Save"), click);

        // A line of a document: its words are selected near where they sit.
        let line = SpanAct {
            own: Some(target("document", &["text"])),
            via: None,
            line: true,
        };
        let act = plan_span(&line, false, rect(), "invoice");
        let want = Act::SelectText {
            r#ref: "ref_7".into(),
            query: "invoice".into(),
            n: None,
            near: Some((130.0, 50.0)),
        };
        assert_eq!(act, want);
        assert_eq!(act.word(), "Select");

        // A label inside a button presses the button.
        let mut via = target("button", &["invoke"]);
        via.r#ref = "ref_2".into();
        let label = SpanAct {
            own: Some(target("text", &[])),
            via: Some(via.clone()),
            line: false,
        };
        let pressed = Act::Pattern {
            r#ref: "ref_2".into(),
            action: "invoke",
        };
        assert_eq!(plan_span(&label, false, rect(), "Save"), pressed);
        // Its own pattern wins over the one around it.
        let link = SpanAct {
            own: Some(target("hyperlink", &["invoke"])),
            via: Some(via.clone()),
            line: false,
        };
        let own = Act::Pattern {
            r#ref: "ref_7".into(),
            action: "invoke",
        };
        assert_eq!(plan_span(&link, false, rect(), "Save"), own);

        // No pattern anywhere: the real click, at the element.
        let plain = SpanAct {
            own: Some(target("text", &[])),
            ..Default::default()
        };
        let act = plan_span(&plain, false, rect(), "Total");
        assert_eq!(act.element(), Some("ref_7"));
        assert!(matches!(act, Act::ClickElement { .. }));
        assert_eq!(act.word(), "Click");

        // Turned off: nothing, and no word.
        via.enabled = false;
        let off = SpanAct {
            own: Some(target("text", &[])),
            via: Some(via),
            line: false,
        };
        let act = plan_span(&off, false, rect(), "Save");
        assert_eq!(act, Act::Nothing { why: Why::Disabled });
    }

    #[test]
    fn a_document_match_is_selected_unless_it_is_a_link() {
        let act = plan_doc("ref_3", 4, "total", None);
        let want = Act::SelectText {
            r#ref: "ref_3".into(),
            query: "total".into(),
            n: Some(4),
            near: None,
        };
        assert_eq!(act, want);
        let link = target("hyperlink", &["invoke"]);
        let act = plan_doc("ref_3", 4, "total", Some(&link));
        assert_eq!(act.word(), "Press");
        assert_eq!(act.element(), Some("ref_7"));
        // Something around it with no pattern changes nothing.
        let group = target("group", &[]);
        assert_eq!(plan_doc("ref_3", 0, "a", Some(&group)).word(), "Select");
    }

    #[test]
    fn targets_come_from_the_helper_span() {
        let sp = json!({ "text": "Save", "ref": "ref_9", "role": "text", "pat": [],
            "via": { "ref": "ref_4", "role": "button", "pat": ["invoke"], "enabled": false } });
        let info = SpanAct::from_span(&sp);
        assert_eq!(info.own.as_ref().map(|t| t.role.as_str()), Some("text"));
        let via = info.via.unwrap();
        assert_eq!((via.r#ref.as_str(), via.enabled), ("ref_4", false));
        assert!(!info.line);
        // Recognised words carry no ref, so no element.
        let ocr = json!({ "text": "Save", "ref": null, "source": "ocr" });
        assert_eq!(SpanAct::from_span(&ocr), SpanAct::default());
        let line = json!({ "text": "a", "ref": "ref_1", "role": "document", "line": true });
        assert!(SpanAct::from_span(&line).line);
    }

    #[test]
    fn menu_names_read_as_a_person_reads_them() {
        assert_eq!(clean_menu_name("&Word Wrap", true), "Word Wrap");
        assert_eq!(clean_menu_name("Save &As...\tCtrl+Shift+S", true), "Save As");
        assert_eq!(clean_menu_name("Find && Replace\u{2026}", true), "Find & Replace");
        assert_eq!(clean_menu_name("  ", true), "");
        // A menu read through UI Automation has no access-key marks.
        assert_eq!(clean_menu_name("Save & Close", false), "Save & Close");
    }

    fn item(path: &[&str], sub: bool) -> Value {
        let pos: Vec<usize> = (0..path.len()).collect();
        json!({ "path": path, "pos": pos, "sub": sub, "enabled": true })
    }

    #[test]
    fn menu_commands_keep_to_their_bounds() {
        let read = json!({ "source": "win32", "limit": null, "items": [
            item(&["&File"], true),
            item(&["&File", "&New\tCtrl+N"], false),
            item(&["&File", "Recent"], true),
            item(&["&File", "Recent", "a.txt"], false),
            item(&["F&ormat"], true),
            item(&["F&ormat", "&Word Wrap"], false),
            item(&["F&ormat", "&Word Wrap"], false),
            item(&["&Help"], false),
            item(&["A", "B", "C", "D", "E"], false),
        ] });
        let window = json!(7);
        let (all, limit) = menu_commands(&read, &window, MENU_DEPTH, MENU_MAX);
        let paths: Vec<String> = all.iter().map(|c| c.path.join(" > ")).collect();
        // No bar items, no parents, no repeats, nothing five levels down.
        let want = ["File > New", "File > Recent > a.txt", "Format > Word Wrap"];
        assert_eq!(paths, want);
        assert_eq!(limit, Some("depth"));
        // The helper is handed back the names as the application wrote them.
        assert_eq!(all[2].item["path"], json!(["F&ormat", "&Word Wrap"]));
        assert_eq!(all[2].item["pos"], json!([0, 1]));
        assert_eq!(all[2].item["source"], json!("win32"));
        assert_eq!(all[2].window, window);

        // The count bound holds whatever the helper sent.
        let (two, limit) = menu_commands(&read, &window, MENU_DEPTH, 2);
        assert_eq!(two.len(), 2);
        assert_eq!(limit, Some("count"));
        // So does the depth bound.
        let (flat, limit) = menu_commands(&read, &window, 2, MENU_MAX);
        let paths: Vec<String> = flat.iter().map(|c| c.path.join(" > ")).collect();
        assert_eq!(paths, ["File > New", "Format > Word Wrap"]);
        assert_eq!(limit, Some("depth"));

        // The helper's own bound is passed on, and wins.
        let slow = json!({ "source": "uia", "limit": "time", "items": [
            item(&["Edit", "Save & Close"], false),
        ] });
        let (cmds, limit) = menu_commands(&slow, &window, 1, MENU_MAX);
        assert!(cmds.is_empty());
        assert_eq!(limit, Some("time"));
        let (cmds, _) = menu_commands(&slow, &window, MENU_DEPTH, MENU_MAX);
        assert_eq!(cmds[0].path, ["Edit", "Save & Close"]);
        // Nothing read, nothing listed, nothing said.
        let none = menu_commands(&Value::Null, &window, MENU_DEPTH, MENU_MAX);
        assert_eq!(none, (Vec::new(), None));
    }

    #[test]
    fn a_cut_menu_read_says_so() {
        assert!(menu_note(None).is_none());
        for limit in ["skipped", "time", "count", "depth"] {
            assert!(menu_note(Some(limit)).is_some(), "{limit}");
        }
        assert_eq!(menu_count(1), "1 menu command");
        assert_eq!(menu_count(12), "12 menu commands");
    }

    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn plan(act: Act) -> Plan {
        Plan {
            act,
            window: json!(7),
            menu: None,
            query: "save".into(),
        }
    }

    #[test]
    fn each_action_is_one_desktop_call() {
        run(async {
            let d = FakeDesktop::default();
            let press = Act::Pattern {
                r#ref: "ref_2".into(),
                action: "toggle",
            };
            perform(&d, &plan(press)).await.unwrap();
            let focus = Act::Focus {
                r#ref: "ref_3".into(),
            };
            perform(&d, &plan(focus)).await.unwrap();
            let nth = Act::SelectText {
                r#ref: "ref_4".into(),
                query: "total".into(),
                n: Some(2),
                near: Some((1.0, 1.0)),
            };
            perform(&d, &plan(nth)).await.unwrap();
            let near = Act::SelectText {
                r#ref: "ref_4".into(),
                query: "total".into(),
                n: None,
                near: Some((10.4, 20.6)),
            };
            perform(&d, &plan(near)).await.unwrap();
            let el = Act::ClickElement {
                r#ref: "ref_5".into(),
            };
            perform(&d, &plan(el)).await.unwrap();
            let point = Act::ClickPoint { x: 30.5, y: 40.2 };
            perform(&d, &plan(point)).await.unwrap();
            let mut menu = plan(Act::Menu { index: 0 });
            menu.menu = Some(json!({ "source": "win32", "path": ["F&ormat", "&Word Wrap"],
                "pos": [2, 0] }));
            perform(&d, &menu).await.unwrap();

            let calls = d.calls.lock().clone();
            let want = [
                ("invoke", json!({ "ref": "ref_2", "action": "toggle" })),
                ("invoke", json!({ "ref": "ref_3", "action": "focusNear" })),
                // A numbered match is selected by its number alone.
                ("selectText", json!({ "ref": "ref_4", "query": "total", "n": 2 })),
                (
                    "selectText",
                    json!({ "ref": "ref_4", "query": "total", "n": 0, "x": 10.0, "y": 21.0 }),
                ),
                (
                    "click",
                    json!({ "t": { "ref": "ref_5" }, "opts": { "physical": true } }),
                ),
                (
                    "click",
                    json!({ "t": { "x": 31.0, "y": 40.0 }, "opts": { "inWindow": 7 } }),
                ),
                (
                    "menuInvoke",
                    json!({ "windowHandle": 7, "source": "win32",
                        "path": ["F&ormat", "&Word Wrap"], "pos": [2, 0] }),
                ),
            ];
            assert_eq!(calls.len(), want.len());
            for (got, (method, args)) in calls.iter().zip(want) {
                assert_eq!((got.0.as_str(), &got.1), (method, &args));
            }
        });
    }

    #[test]
    fn a_refused_pattern_falls_back_to_a_click_and_nothing_else_does() {
        run(async {
            let d = FakeDesktop::default();
            d.fail("invoke", "NO_PATTERN");
            let press = Act::Pattern {
                r#ref: "ref_2".into(),
                action: "invoke",
            };
            perform(&d, &plan(press.clone())).await.unwrap();
            assert_eq!(d.methods(), ["invoke", "click"]);

            // Any other failure is reported, not clicked through.
            let d = FakeDesktop::default();
            d.fail("invoke", "BLOCKED_OWN_WINDOW");
            let err = perform(&d, &plan(press)).await.unwrap_err();
            assert_eq!(d.methods(), ["invoke"]);
            assert_eq!(failure(&err).0, "own-window");

            // A hit with nothing to do touches nothing.
            let d = FakeDesktop::default();
            let none = Act::Nothing { why: Why::Disabled };
            assert!(perform(&d, &plan(none)).await.is_err());
            assert!(perform(&d, &plan(Act::Menu { index: 3 })).await.is_err());
            assert!(d.methods().is_empty());
        });
    }

    #[test]
    fn failures_say_what_happened() {
        let say = |code: &str| failure(&DeskError::new(code, code)).0;
        assert_eq!(say("STALE_REF"), "gone");
        assert_eq!(say("NOT_FOUND"), "gone");
        assert_eq!(say("DISABLED"), "disabled");
        assert_eq!(say("OCCLUDED"), "covered");
        assert_eq!(say("ELEVATED_TARGET"), "elevated");
        assert_eq!(say("TIMEOUT"), "failed");
        for r in [
            Refusal::NoHit,
            Refusal::Changed,
            Refusal::Gone,
            Refusal::Disabled,
            Refusal::NoPlace,
        ] {
            assert!(!r.message().is_empty() && !r.status().is_empty());
        }
    }
}

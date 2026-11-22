//! The desktop engine's contract (PARITY DESK-01..03, DESK-10): timeouts,
//! error codes, reply shapes and the read_page text format. Pure functions,
//! unit-tested; the calls are in `client.rs`, the engine in `host`.

use std::fmt;

use serde_json::{json, Map, Value};

pub const OP_TIMEOUT: u64 = 4000;
pub const UIA_COLD_TIMEOUT: u64 = 12000;
pub const OCR_TIMEOUT: u64 = 15000;
pub const CAPTURE_TIMEOUT: u64 = 8000;
pub const DEATH_WINDOW_MS: u64 = 60000;
pub const DEATH_LIMIT: usize = 3;
/// A reply may take this long past its op timeout before the helper is
/// presumed hung and killed.
pub const REPLY_GRACE: u64 = 2000;

/// Every code a call can reject with (DESK-10).
// The DESK-10 list, checked by the tests.
#[allow(dead_code)]
pub const CODES: &[&str] = &[
    "STALE_REF",
    "NOT_FOUND",
    "BLOCKED_OWN_WINDOW",
    "ELEVATED_TARGET",
    "NO_PATTERN",
    "TIMEOUT",
    "HELPER_DOWN",
    "BAD_ARGS",
    "OCCLUDED",
    "NOT_FOREGROUND",
    "FOCUS_LOST",
    "DISABLED",
    "NO_OCR",
    "UIA_ERROR",
    "INTERNAL",
    "UNSUPPORTED",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopError {
    pub code: String,
    pub message: String,
}

impl DesktopError {
    pub fn new(code: &str, message: impl Into<String>) -> DesktopError {
        DesktopError {
            code: code.to_string(),
            message: message.into(),
        }
    }

    /// `{code, message}`, as `DesktopError.toJSON()`.
    pub fn to_json(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}

impl fmt::Display for DesktopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for DesktopError {}

pub type DResult<T> = Result<T, DesktopError>;

/// Typing waits 2 s plus 2 ms a character, never under the op timeout.
pub fn type_timeout(chars: usize) -> u64 {
    OP_TIMEOUT.max(2000 + chars as u64 * 2)
}

/// Keys wait 2 s plus 40 ms a press, never under the op timeout: host.cs
/// sends each chord with a 15 ms pause and a focus check, and caps the
/// repeat at 100 (B-050).
pub fn key_timeout(keys: &str, repeat: u64) -> u64 {
    let chords = keys.split_whitespace().count().max(1) as u64;
    OP_TIMEOUT.max(2000 + repeat.clamp(1, 100) * chords * 40)
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Window handle from any of the forms index.js accepted: a number,
/// `'w1A2B'`, `'0x1a2b'`, or decimal text.
pub fn handle_of(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        Value::String(s) => {
            let s = s.trim();
            if let Some(hex) = s.strip_prefix('w').or_else(|| s.strip_prefix('W')) {
                if !hex.is_empty() && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return u64::from_str_radix(hex, 16).ok();
                }
                return None;
            }
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                if !hex.is_empty() && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return u64::from_str_radix(hex, 16).ok();
                }
                return None;
            }
            if !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()) {
                return s.parse().ok();
            }
            None
        }
        _ => None,
    }
}

pub fn handle_str(n: u64) -> String {
    format!("w{n:X}")
}

/// `[x, y, w, h]` to `{x, y, width, height}`; objects pass through.
pub fn rect_obj(r: &Value) -> Value {
    match r {
        Value::Array(a) if a.len() >= 4 => {
            json!({ "x": a[0], "y": a[1], "width": a[2], "height": a[3] })
        }
        Value::Null => Value::Null,
        other => other.clone(),
    }
}

/// Rect argument as the helper wants it: `[x, y, w, h]`.
pub fn rect_arg(r: &Value) -> Value {
    match r {
        Value::Null => Value::Null,
        Value::Array(_) => r.clone(),
        Value::Object(o) => {
            let pick = |a: &str, b: &str| {
                o.get(a)
                    .or_else(|| o.get(b))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            json!([
                o.get("x").cloned().unwrap_or(Value::Null),
                o.get("y").cloned().unwrap_or(Value::Null),
                pick("width", "w"),
                pick("height", "h")
            ])
        }
        _ => Value::Null,
    }
}

/// A node with its rect as an object.
pub fn node_shape(n: &Value) -> Value {
    let mut n = n.clone();
    if let Some(o) = n.as_object_mut() {
        if let Some(r) = o.get("rect").cloned() {
            o.insert("rect".into(), rect_obj(&r));
        }
    }
    n
}

pub fn point_out(mut r: Value) -> Value {
    if let Some(o) = r.as_object_mut() {
        if let Some(Value::Array(p)) = o.get("point").cloned() {
            if p.len() >= 2 {
                o.insert("point".into(), json!({ "x": p[0], "y": p[1] }));
            }
        }
    }
    r
}

fn quote(s: &str) -> String {
    // JS: replace \ and ", then collapse whitespace runs around newlines.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = String::with_capacity(escaped.len() + 2);
    out.push('"');
    let chars: Vec<char> = escaped.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Find a run of whitespace containing at least one CR/LF.
        if chars[i].is_whitespace() {
            let mut j = i;
            let mut newline = false;
            while j < chars.len() && chars[j].is_whitespace() {
                if chars[j] == '\r' || chars[j] == '\n' {
                    newline = true;
                }
                j += 1;
            }
            if newline {
                // \s*[\r\n]+\s* : the whole run becomes one space.
                out.push(' ');
            } else {
                out.extend(&chars[i..j]);
            }
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out.push('"');
    out
}

fn str_field<'a>(n: &'a Value, k: &str) -> &'a str {
    n.get(k).and_then(Value::as_str).unwrap_or("")
}

/// One node in the browser toolset's read_page line format.
pub fn node_line(n: &Value, indent: usize) -> String {
    let mut line = format!(
        "{}{}",
        "  ".repeat(indent),
        value_text(n.get("role").unwrap_or(&Value::Null))
    );
    let name = str_field(n, "name");
    if !name.is_empty() {
        line.push(' ');
        line.push_str(&quote(name));
    }
    line.push_str(&format!(
        " [{}]",
        value_text(n.get("ref").unwrap_or(&Value::Null))
    ));
    match n.get("value") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) if s.is_empty() => {}
        Some(v) => {
            line.push_str(" value=");
            line.push_str(&quote(&value_text(v)));
        }
    }
    let states: Vec<String> = n
        .get("states")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(value_text)
                .filter(|s| s != "readonly")
                .collect()
        })
        .unwrap_or_default();
    if !states.is_empty() {
        line.push_str(&format!(" ({})", states.join(", ")));
    }
    line
}

fn depth_of(n: &Value) -> usize {
    n.get("depth").and_then(Value::as_i64).unwrap_or(0).max(0) as usize
}

/// The tree as text, cut at `max_chars` (UTF-16 units, as JS counts).
pub fn render_tree(nodes: &[Value], max_chars: usize) -> (String, bool) {
    let mut lines = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;
    for n in nodes {
        let line = node_line(n, depth_of(n));
        let len = line.encode_utf16().count();
        if used + len + 1 > max_chars {
            truncated = true;
            break;
        }
        used += len + 1;
        lines.push(line);
    }
    if truncated {
        lines.push(format!(
            "[output truncated at {max_chars} characters; pass ref to read one subtree, or filter: 'interactive']"
        ));
    }
    (lines.join("\n"), truncated)
}

/// Whether a `key()` chord list sends Escape (main drops its global Esc
/// around it, since RegisterHotKey also fires on injected input).
pub fn sends_escape(keys: &str) -> bool {
    keys.split_whitespace().any(|chord| {
        let last = chord.rsplit('+').next().unwrap_or("");
        last.eq_ignore_ascii_case("esc") || last.eq_ignore_ascii_case("escape")
    })
}

/// A pointer target: `{ref}` or rounded `{x, y}`.
pub fn target(t: &Value) -> DResult<Value> {
    if let Some(r) = t.get("ref").filter(|r| !r.is_null()) {
        return Ok(json!({ "ref": r }));
    }
    let x = t.get("x").and_then(Value::as_f64).filter(|v| v.is_finite());
    let y = t.get("y").and_then(Value::as_f64).filter(|v| v.is_finite());
    match (x, y) {
        (Some(x), Some(y)) => Ok(json!({ "x": x.round() as i64, "y": y.round() as i64 })),
        _ => Err(DesktopError::new(
            "BAD_ARGS",
            "target must be {ref} or {x, y}",
        )),
    }
}

/// Merges two objects (later keys win), as `{...a, ...b}`.
pub fn merge(a: &Value, b: &Value) -> Value {
    let mut out = Map::new();
    for v in [a, b] {
        if let Some(o) = v.as_object() {
            for (k, val) in o {
                out.insert(k.clone(), val.clone());
            }
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeouts() {
        assert_eq!(type_timeout(0), 4000);
        assert_eq!(type_timeout(1000), 4000);
        assert_eq!(type_timeout(5000), 12000);
        assert_eq!(
            (OP_TIMEOUT, UIA_COLD_TIMEOUT, OCR_TIMEOUT, CAPTURE_TIMEOUT),
            (4000, 12000, 15000, 8000)
        );
        assert_eq!(CODES.len(), 16);
    }

    #[test]
    fn handles() {
        assert_eq!(handle_of(&json!("w1A2B")), Some(0x1a2b));
        assert_eq!(handle_of(&json!("0x10")), Some(16));
        assert_eq!(handle_of(&json!("42")), Some(42));
        assert_eq!(handle_of(&json!(42)), Some(42));
        assert_eq!(handle_of(&json!("nope")), None);
        assert_eq!(handle_of(&Value::Null), None);
        assert_eq!(handle_str(0x1a2b), "w1A2B");
    }

    #[test]
    fn rect_shapes() {
        assert_eq!(
            rect_obj(&json!([1, 2, 3, 4])),
            json!({"x":1,"y":2,"width":3,"height":4})
        );
        assert_eq!(
            rect_arg(&json!({"x":1,"y":2,"w":3,"h":4})),
            json!([1, 2, 3, 4])
        );
        assert_eq!(
            rect_arg(&json!({"x":1,"y":2,"width":3,"height":4})),
            json!([1, 2, 3, 4])
        );
        assert_eq!(
            point_out(json!({"point":[5,6]})),
            json!({"point":{"x":5,"y":6}})
        );
    }

    #[test]
    fn read_page_lines() {
        let n = json!({"role":"button","name":"Send","ref":"ref_12","states":["focused","readonly"],"depth":1});
        assert_eq!(node_line(&n, 1), "  button \"Send\" [ref_12] (focused)");
        let e = json!({"role":"edit","name":"Say \"hi\"\n  now","ref":"ref_13","value":"cats"});
        assert_eq!(
            node_line(&e, 0),
            "edit \"Say \\\"hi\\\" now\" [ref_13] value=\"cats\""
        );
        let p = json!({"role":"edit","name":"","ref":"ref_1","value":""});
        assert_eq!(node_line(&p, 0), "edit [ref_1]");
    }

    #[test]
    fn tree_truncates_with_the_hint() {
        let nodes: Vec<Value> = (0..5)
            .map(|i| json!({"role":"text","name":"x","ref":format!("ref_{i}"),"depth":0}))
            .collect();
        let (text, cut) = render_tree(&nodes, 40);
        assert!(cut);
        assert!(text.ends_with("pass ref to read one subtree, or filter: 'interactive']"));
        let (_, cut) = render_tree(&nodes, 50000);
        assert!(!cut);
    }

    #[test]
    fn escape_detection() {
        assert!(sends_escape("Escape"));
        assert!(sends_escape("ctrl+esc"));
        assert!(sends_escape("Backspace ESC"));
        assert!(!sends_escape("ctrl+s"));
    }

    #[test]
    fn targets() {
        assert_eq!(
            target(&json!({"ref":"ref_1","x":1})).unwrap(),
            json!({"ref":"ref_1"})
        );
        assert_eq!(
            target(&json!({"x":1.6,"y":2.4})).unwrap(),
            json!({"x":2,"y":2})
        );
        assert_eq!(target(&json!({"x":1})).unwrap_err().code, "BAD_ARGS");
    }
}

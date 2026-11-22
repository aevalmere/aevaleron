//! Each CLI's JSON lines, read one at a time as they arrive:
//!
//! - Claude Code `-p --output-format stream-json --verbose
//!   --include-partial-messages`: `stream_event` text deltas, the whole
//!   `assistant` message, then one `result` with the usage or the error.
//! - Codex `exec --json`: `item.*` events whose `agent_message` text grows,
//!   `turn.completed` with the usage, `turn.failed` or `error`.
//! - Gemini CLI `--output-format stream-json`: `message` events from the
//!   assistant (`delta: true` for chunks), then a `result`.
//!
//! Lines that are not JSON, and event types this does not know, are
//! skipped: the CLIs add event types between versions.

use std::collections::HashMap;

use serde_json::Value;

use crate::llm::provider::Provider;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The next piece of the answer.
    Text(String),
    /// Tokens in and out, as the CLI reported them.
    Usage(u64, u64),
    /// The answer is complete.
    Done,
    /// The CLI said the request failed, in its words.
    Failed(String),
}

/// One CLI's output, line by line.
pub struct Decoder {
    provider: Provider,
    /// Text arrived as deltas, so a whole message repeats it.
    saw_delta: bool,
    any_text: bool,
    /// Codex: how much of each `agent_message` item was already handed on.
    items: HashMap<String, usize>,
    /// Errors the CLI reported along the way that did not end the run, for
    /// the error text if the run then fails.
    pub notes: Vec<String>,
}

fn count(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
        .unwrap_or(0)
}

fn message_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(_) => {
            let m = v["message"].as_str().or_else(|| v["error"].as_str());
            m.map(str::to_string).unwrap_or_else(|| v.to_string())
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl Decoder {
    pub fn new(provider: Provider) -> Decoder {
        Decoder {
            provider,
            saw_delta: false,
            any_text: false,
            items: HashMap::new(),
            notes: Vec::new(),
        }
    }

    /// The events in one line of the CLI's stdout.
    pub fn line(&mut self, line: &str) -> Vec<Event> {
        let line = line.trim();
        if !line.starts_with('{') {
            return Vec::new();
        }
        let Ok(obj) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let mut out = match self.provider {
            Provider::ClaudeCli => self.claude(&obj),
            Provider::CodexCli => self.codex(&obj),
            _ => self.gemini(&obj),
        };
        out.retain(|e| !matches!(e, Event::Text(t) if t.is_empty()));
        if out.iter().any(|e| matches!(e, Event::Text(_))) {
            self.any_text = true;
        }
        out
    }

    fn claude(&mut self, obj: &Value) -> Vec<Event> {
        match obj["type"].as_str().unwrap_or_default() {
            "stream_event" => {
                let e = &obj["event"];
                let text = e["delta"]["text"].as_str();
                if e["type"] == "content_block_delta" && e["delta"]["type"] == "text_delta" {
                    self.saw_delta = true;
                    return text.map(|t| vec![Event::Text(t.to_string())]).unwrap_or_default();
                }
                Vec::new()
            }
            "assistant" => {
                // A failed call comes back as an assistant message that
                // says why ("Please run /login"); that is the error's text,
                // never the answer.
                let error = obj["error"].as_str();
                if let Some(e) = error.or_else(|| obj["message"]["error"].as_str()) {
                    self.notes.push(e.to_string());
                    return Vec::new();
                }
                if self.saw_delta {
                    return Vec::new();
                }
                let empty = Vec::new();
                let blocks = obj["message"]["content"].as_array().unwrap_or(&empty);
                blocks
                    .iter()
                    .filter(|b| b["type"] == "text")
                    .filter_map(|b| b["text"].as_str())
                    .map(|t| Event::Text(t.to_string()))
                    .collect()
            }
            "result" => {
                let mut out = Vec::new();
                let u = &obj["usage"];
                if u.is_object() {
                    let prompt = count(&u["input_tokens"])
                        + count(&u["cache_creation_input_tokens"])
                        + count(&u["cache_read_input_tokens"]);
                    out.push(Event::Usage(prompt, count(&u["output_tokens"])));
                }
                let failed = obj["is_error"] == true
                    || obj["subtype"].as_str().is_some_and(|s| s != "success");
                if failed {
                    let mut said = message_of(&obj["result"]);
                    if let Some(errors) = obj["errors"].as_array() {
                        for e in errors {
                            said.push('\n');
                            said.push_str(&message_of(e));
                        }
                    }
                    if said.trim().is_empty() {
                        said = obj["subtype"].as_str().unwrap_or("error").to_string();
                    }
                    out.push(Event::Failed(said));
                    return out;
                }
                // A version without partial messages, or one that sent none:
                // the result carries the whole answer.
                if !self.any_text {
                    if let Some(t) = obj["result"].as_str() {
                        out.insert(0, Event::Text(t.to_string()));
                    }
                }
                out.push(Event::Done);
                out
            }
            _ => Vec::new(),
        }
    }

    fn codex(&mut self, obj: &Value) -> Vec<Event> {
        let kind = obj["type"].as_str().unwrap_or_default();
        match kind {
            "item.started" | "item.updated" | "item.completed" => {
                let item = &obj["item"];
                let item_type = item["type"]
                    .as_str()
                    .or_else(|| item["item_type"].as_str())
                    .unwrap_or_default();
                match item_type {
                    "agent_message" | "assistant_message" => {
                        let id = item["id"].as_str().unwrap_or_default().to_string();
                        let text = item["text"].as_str().unwrap_or_default();
                        let new = !self.items.contains_key(&id);
                        let sent = self.items.get(&id).copied().unwrap_or(0);
                        if text.len() <= sent || !text.is_char_boundary(sent) {
                            return Vec::new();
                        }
                        self.items.insert(id, text.len());
                        let mut piece = String::new();
                        if new && self.any_text {
                            piece.push_str("\n\n");
                        }
                        piece.push_str(&text[sent..]);
                        vec![Event::Text(piece)]
                    }
                    "error" => {
                        self.notes.push(message_of(item));
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "turn.completed" => {
                let u = &obj["usage"];
                let usage = Event::Usage(count(&u["input_tokens"]), count(&u["output_tokens"]));
                vec![usage, Event::Done]
            }
            "turn.failed" => vec![Event::Failed(message_of(&obj["error"]))],
            "error" => {
                // Reconnect notices come this way too; the exit code decides.
                self.notes.push(message_of(obj));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn gemini(&mut self, obj: &Value) -> Vec<Event> {
        match obj["type"].as_str().unwrap_or_default() {
            "message" if obj["role"] == "assistant" => {
                let delta = obj["delta"] == true;
                if !delta && self.saw_delta {
                    return Vec::new();
                }
                self.saw_delta |= delta;
                let text = message_of(&obj["content"]);
                vec![Event::Text(text)]
            }
            "result" => {
                let s = &obj["stats"];
                let mut out = Vec::new();
                if s.is_object() {
                    out.push(Event::Usage(
                        count(&s["input_tokens"]),
                        count(&s["output_tokens"]),
                    ));
                }
                if obj["status"] == "error" || obj["error"].is_object() {
                    out.push(Event::Failed(message_of(&obj["error"])));
                } else {
                    out.push(Event::Done);
                }
                out
            }
            "error" => {
                let said = message_of(obj);
                if obj["severity"] == "error" {
                    return vec![Event::Failed(said)];
                }
                self.notes.push(said);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(p: Provider, lines: &[&str]) -> Vec<Event> {
        let mut d = Decoder::new(p);
        lines.iter().flat_map(|l| d.line(l)).collect()
    }

    fn text(events: &[Event]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn claude_stream_json() {
        let lines = [
            r#"{"type":"system","subtype":"init","model":"claude-sonnet-5-5","tools":[]}"#,
            r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hm"}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hello"}]}}"#,
            "not json",
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Hello","usage":{"input_tokens":3,"cache_read_input_tokens":10,"output_tokens":2}}"#,
        ];
        let ev = run(Provider::ClaudeCli, &lines);
        assert_eq!(text(&ev), "Hello");
        assert!(ev.contains(&Event::Usage(13, 2)));
        assert_eq!(ev.last(), Some(&Event::Done));
        // No partial messages: the assistant message, then the result alone.
        let lines = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi"}]}}"#,
            r#"{"type":"result","subtype":"success","result":"Hi"}"#,
        ];
        assert_eq!(text(&run(Provider::ClaudeCli, &lines)), "Hi");
        let only = [r#"{"type":"result","subtype":"success","result":"Just this"}"#];
        assert_eq!(text(&run(Provider::ClaudeCli, &only)), "Just this");
    }

    #[test]
    fn claude_failures() {
        let login = [
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Invalid API key · Please run /login"}]},"error":"authentication_failed"}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Please run /login"}"#,
        ];
        let mut d = Decoder::new(Provider::ClaudeCli);
        let ev: Vec<Event> = login.iter().flat_map(|l| d.line(l)).collect();
        let failed = ev.iter().find_map(|e| match e {
            Event::Failed(s) => Some(s.clone()),
            _ => None,
        });
        assert_eq!(failed.as_deref(), Some("Invalid API key · Please run /login"));
        assert_eq!(d.notes, ["authentication_failed"]);
        let max = [r#"{"type":"result","subtype":"error_max_turns","is_error":true}"#];
        let ev = run(Provider::ClaudeCli, &max);
        assert_eq!(ev, [Event::Failed("error_max_turns".into())]);
    }

    #[test]
    fn codex_json_lines() {
        let lines = [
            r#"{"type":"thread.started","thread_id":"t1"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"item.updated","item":{"id":"item_1","type":"agent_message","text":"The win"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"The window"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"Second."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":20,"cached_input_tokens":4,"output_tokens":7}}"#,
        ];
        let ev = run(Provider::CodexCli, &lines);
        assert_eq!(text(&ev), "The window\n\nSecond.");
        assert!(ev.contains(&Event::Usage(20, 7)));
        assert_eq!(ev.last(), Some(&Event::Done));
        let failed = [
            r#"{"type":"error","message":"Reconnecting... 1/5"}"#,
            r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#,
        ];
        let mut d = Decoder::new(Provider::CodexCli);
        let ev: Vec<Event> = failed.iter().flat_map(|l| d.line(l)).collect();
        assert_eq!(ev, [Event::Failed("You've hit your usage limit.".into())]);
        assert_eq!(d.notes, ["Reconnecting... 1/5"]);
    }

    #[test]
    fn gemini_stream_json() {
        let lines = [
            r#"{"type":"init","session_id":"s","model":"gemini-3.8-flash"}"#,
            r#"{"type":"message","role":"user","content":"q"}"#,
            r#"{"type":"message","role":"assistant","content":"A ","delta":true}"#,
            r#"{"type":"message","role":"assistant","content":"button.","delta":true}"#,
            r#"{"type":"result","status":"success","stats":{"total_tokens":9,"input_tokens":6,"output_tokens":3}}"#,
        ];
        let ev = run(Provider::GeminiCli, &lines);
        assert_eq!(text(&ev), "A button.");
        assert!(ev.contains(&Event::Usage(6, 3)));
        assert_eq!(ev.last(), Some(&Event::Done));
        let failed = [
            r#"{"type":"result","status":"error","error":{"type":"FatalAuthenticationError","message":"Please sign in"}}"#,
        ];
        let ev = run(Provider::GeminiCli, &failed);
        assert_eq!(ev, [Event::Failed("Please sign in".into())]);
    }
}

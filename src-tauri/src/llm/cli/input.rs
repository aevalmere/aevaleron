//! The OpenAI-shaped request body every caller builds, as what a CLI takes:
//! one system prompt, then one message of text and images. Earlier turns
//! of a conversation become a transcript at the top of that message; a CLI
//! run never carries a session from one request to the next.

use serde_json::Value;

/// A screenshot from a `data:` URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub mime: String,
    pub base64: String,
}

impl Image {
    /// The file extension the image is written with.
    pub fn ext(&self) -> &'static str {
        match self.mime.as_str() {
            "image/png" => "png",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "jpg",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Text(String),
    Image(Image),
}

/// One request as a CLI takes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rendered {
    pub system: String,
    pub parts: Vec<Part>,
}

impl Rendered {
    pub fn images(&self) -> impl Iterator<Item = &Image> {
        self.parts.iter().filter_map(|p| match p {
            Part::Image(i) => Some(i),
            Part::Text(_) => None,
        })
    }
}

/// `data:image/jpeg;base64,...` as an image.
pub fn data_image(url: &str) -> Option<Image> {
    let rest = url.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    let mime = if mime.starts_with("image/") {
        mime.to_string()
    } else {
        "image/jpeg".to_string()
    };
    Some(Image {
        mime,
        base64: data.trim().to_string(),
    })
}

/// A message's content as text alone (images say they were there).
fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| match p["type"].as_str() {
                Some("text") => p["text"].as_str().map(str::to_string),
                Some("image_url") => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn speaker(role: &str) -> &'static str {
    match role {
        "assistant" => "Assistant",
        "tool" => "Tool result",
        _ => "User",
    }
}

fn push_text(parts: &mut Vec<Part>, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(Part::Text(last)) = parts.last_mut() {
        last.push_str("\n\n");
        last.push_str(text);
        return;
    }
    parts.push(Part::Text(text.to_string()));
}

/// The request in `body` (`messages`, OpenAI's shape) as one system prompt
/// and one message.
pub fn render(body: &Value) -> Rendered {
    let empty = Vec::new();
    let messages = body["messages"].as_array().unwrap_or(&empty);
    let mut system: Vec<String> = Vec::new();
    let mut turns: Vec<&Value> = Vec::new();
    for m in messages {
        if m["role"] == "system" {
            let t = text_of(&m["content"]);
            if !t.trim().is_empty() {
                system.push(t);
            }
        } else {
            turns.push(m);
        }
    }
    let mut parts: Vec<Part> = Vec::new();
    let Some((last, earlier)) = turns.split_last() else {
        return Rendered {
            system: system.join("\n\n"),
            parts,
        };
    };
    if !earlier.is_empty() {
        let mut lines = vec!["Earlier in this conversation:".to_string()];
        for m in earlier {
            let role = m["role"].as_str().unwrap_or("user");
            let mut text = text_of(&m["content"]);
            if let Some(calls) = m.get("tool_calls").filter(|c| !c.is_null()) {
                text.push_str(&format!("\n(tool calls: {calls})"));
            }
            if !text.trim().is_empty() {
                lines.push(format!("{}: {}", speaker(role), text.trim()));
            }
        }
        lines.push("The newest message:".to_string());
        push_text(&mut parts, &lines.join("\n\n"));
    }
    match &last["content"] {
        Value::Array(list) => {
            for p in list {
                match p["type"].as_str() {
                    Some("text") => push_text(&mut parts, p["text"].as_str().unwrap_or_default()),
                    Some("image_url") => {
                        let url = p["image_url"]["url"].as_str().unwrap_or_default();
                        match data_image(url) {
                            Some(img) => parts.push(Part::Image(img)),
                            None => push_text(&mut parts, &format!("[image: {url}]")),
                        }
                    }
                    _ => {}
                }
            }
        }
        other => push_text(&mut parts, &text_of(other)),
    }
    Rendered {
        system: system.join("\n\n"),
        parts,
    }
}

/// The message as plain text, each image replaced by `name(i)`: how the
/// CLIs that take images as files read it.
pub fn text_with(r: &Rendered, name: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    let mut n = 0;
    for p in &r.parts {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        match p {
            Part::Text(t) => out.push_str(t),
            Part::Image(_) => {
                n += 1;
                out.push_str(&name(n));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_message_with_the_screen() {
        let body = json!({ "messages": [
            { "role": "system", "content": "Be brief." },
            { "role": "user", "content": [
                { "type": "text", "text": "Exact text." },
                { "type": "image_url", "image_url": { "url": "data:image/png;base64,QUJD" } },
                { "type": "text", "text": "what is this?" },
            ] },
        ] });
        let r = render(&body);
        assert_eq!(r.system, "Be brief.");
        assert_eq!(r.parts.len(), 3);
        let img = r.images().next().unwrap();
        assert_eq!(
            (img.mime.as_str(), img.base64.as_str()),
            ("image/png", "QUJD")
        );
        assert_eq!(img.ext(), "png");
        let text = text_with(&r, |i| format!("@shot-{i}.png"));
        assert_eq!(text, "Exact text.\n\n@shot-1.png\n\nwhat is this?");
    }

    #[test]
    fn history_becomes_a_transcript() {
        let body = json!({ "messages": [
            { "role": "system", "content": "S1" },
            { "role": "user", "content": "first" },
            { "role": "assistant", "content": "answer" },
            { "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,AA" } },
                { "type": "text", "text": "again" },
            ] },
        ] });
        let r = render(&body);
        let Part::Text(head) = &r.parts[0] else {
            panic!("text first");
        };
        let want = "Earlier in this conversation:\n\nUser: first\n\nAssistant: answer\n\n\
The newest message:";
        assert_eq!(head, want);
        assert!(matches!(r.parts[1], Part::Image(_)));
        assert_eq!(r.parts[2], Part::Text("again".into()));
        // Plain text content, no system prompt.
        let r = render(&json!({ "messages": [{ "role": "user", "content": "hi" }] }));
        assert_eq!(r.system, "");
        assert_eq!(r.parts, vec![Part::Text("hi".into())]);
        assert_eq!(render(&json!({})), Rendered::default());
        assert!(data_image("https://x/y.png").is_none());
    }
}

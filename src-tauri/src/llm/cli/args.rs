//! Each CLI's command line and stdin for one request, locked down to
//! answering: no tools (Claude Code's are all off; Codex's shell, apps and
//! plugins are off and its sandbox is read-only; Gemini CLI asks before
//! any tool and nobody is there to approve), no MCP servers or plugins
//! from the person's own setup, nothing kept between runs where the CLI
//! allows it, and a scratch folder as the working directory.
//!
//! Images: Claude Code takes them inside its stream-json input, so no file
//! is written; Codex takes `--image <file>`; Gemini CLI reads `@file` from
//! its working directory. Those files are the run's own and are deleted
//! when it ends.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde_json::{json, Value};

use super::input::{text_with, Part, Rendered};
use super::DEFAULT_MODEL;
use crate::llm::provider::Provider;

/// Longer system prompts go at the top of the message instead of on the
/// command line (32 767 characters at most on Windows, every argument
/// included).
const ARG_LIMIT: usize = 16_000;

/// What a run needs besides the program.
pub struct Job<'a> {
    pub provider: Provider,
    pub model: &'a str,
    /// A short job (voice cleanup, the agent's reviewer): least effort.
    pub fast: bool,
    pub dir: &'a Path,
    /// Unique to this run: the names of its files start with it.
    pub tag: &'a str,
}

/// The CLI's arguments, its stdin, the variables it is started with and
/// the files written for it.
#[derive(Debug, Default)]
pub struct Plan {
    pub args: Vec<OsString>,
    pub stdin: Vec<u8>,
    pub env: Vec<(&'static str, OsString)>,
    pub files: Vec<PathBuf>,
}

fn os(items: &[&str]) -> Vec<OsString> {
    items.iter().map(OsString::from).collect()
}

fn picked(model: &str) -> Option<&str> {
    let m = model.trim();
    (!m.is_empty() && m != DEFAULT_MODEL).then_some(m)
}

pub fn plan(job: &Job<'_>, r: &Rendered) -> std::io::Result<Plan> {
    match job.provider {
        Provider::CodexCli => codex(job, r),
        Provider::GeminiCli => gemini(job, r),
        _ => Ok(claude(job, r)),
    }
}

/// `claude -p` with stream-json both ways: the message, images included,
/// goes in on stdin as one `user` line, and the answer streams out.
fn claude(job: &Job<'_>, r: &Rendered) -> Plan {
    let mut args = os(&[
        "-p",
        "--verbose",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--no-session-persistence",
        "--safe-mode",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--permission-mode",
        "dontAsk",
        // `""` is no tools at all. The list takes several values, so the
        // next argument must be an option.
        "--tools",
        "",
    ]);
    if let Some(m) = picked(job.model) {
        args.push(format!("--model={m}").into());
    }
    if job.fast {
        args.extend(os(&["--effort", "low"]));
    }
    let mut blocks: Vec<Value> = Vec::new();
    let system = r.system.trim();
    if system.len() > ARG_LIMIT {
        blocks.push(json!({ "type": "text", "text": system }));
    } else if !system.is_empty() {
        args.push(format!("--system-prompt={system}").into());
    }
    for p in &r.parts {
        blocks.push(match p {
            Part::Text(t) => json!({ "type": "text", "text": t }),
            Part::Image(i) => json!({ "type": "image", "source": {
                "type": "base64", "media_type": i.mime, "data": i.base64,
            } }),
        });
    }
    let line = json!({
        "type": "user",
        "session_id": "",
        "parent_tool_use_id": null,
        "message": { "role": "user", "content": blocks },
    });
    let mut stdin = line.to_string().into_bytes();
    stdin.push(b'\n');
    Plan {
        args,
        stdin,
        env: vec![("DISABLE_AUTOUPDATER", "1".into())],
        files: Vec::new(),
    }
}

/// Each image as `<tag>-<n>.<ext>` in the run's folder.
fn write_images(job: &Job<'_>, r: &Rendered) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for (n, img) in r.images().enumerate() {
        let path = job.dir.join(format!("{}-{}.{}", job.tag, n + 1, img.ext()));
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(img.base64.as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&path, bytes)?;
        files.push(path);
    }
    Ok(files)
}

/// The features `codex exec` would otherwise have on for a turn: shell and
/// exec tools, apps, plugins, sub-agents, image making, the browser and
/// computer use, hooks, and reading images off disk. Names as `codex
/// features list` gave them in 0.166.0.
const CODEX_OFF: [&str; 10] = [
    "shell_tool",
    "unified_exec",
    "apps",
    "plugins",
    "multi_agent",
    "image_generation",
    "computer_use",
    "browser_use",
    "hooks",
    "view_image",
];

/// `codex exec --json`, the prompt on stdin, each image as `--image`.
fn codex(job: &Job<'_>, r: &Rendered) -> std::io::Result<Plan> {
    let files = write_images(job, r)?;
    let mut args = os(&[
        "exec",
        "--json",
        "--color",
        "never",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
    ]);
    args.push("--cd".into());
    args.push(job.dir.as_os_str().to_os_string());
    for f in CODEX_OFF {
        args.extend(os(&["--disable", f]));
    }
    if let Some(m) = picked(job.model) {
        args.extend(os(&["--model", m]));
    }
    if job.fast {
        args.extend(os(&["--config", "model_reasoning_effort=low"]));
    }
    // `--image` takes several values; the `=` form keeps each to one.
    for f in &files {
        let mut a = OsString::from("--image=");
        a.push(f.as_os_str());
        args.push(a);
    }
    let body = text_with(r, |n| format!("(Image {n} is attached.)"));
    let system = r.system.trim();
    let text = if system.is_empty() {
        body
    } else {
        format!("<instructions>\n{system}\n</instructions>\n\n{body}")
    };
    Ok(Plan {
        args,
        stdin: text.into_bytes(),
        env: Vec::new(),
        files,
    })
}

/// `gemini --output-format stream-json`: the system prompt replaces the
/// CLI's own through `GEMINI_SYSTEM_MD`, each image is `@file` in the text.
/// A short message goes as `--prompt`, a long one on stdin.
fn gemini(job: &Job<'_>, r: &Rendered) -> std::io::Result<Plan> {
    let mut files = write_images(job, r)?;
    let names: Vec<String> = files
        .iter()
        .filter_map(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    let text = text_with(r, |n| {
        let name = names.get(n - 1).cloned().unwrap_or_default();
        format!("@{name}")
    });
    let mut env = Vec::new();
    let system = r.system.trim();
    if !system.is_empty() {
        let path = job.dir.join(format!("{}-system.md", job.tag));
        std::fs::write(&path, system)?;
        env.push(("GEMINI_SYSTEM_MD", path.clone().into_os_string()));
        files.push(path);
    }
    let mut args = os(&[
        "--output-format",
        "stream-json",
        "--approval-mode",
        "default",
        "--extensions",
        "none",
    ]);
    if let Some(m) = picked(job.model) {
        args.extend(os(&["--model", m]));
    }
    let (prompt, stdin) = if text.len() <= ARG_LIMIT {
        (text, Vec::new())
    } else {
        ("Answer the message above.".to_string(), text.into_bytes())
    };
    args.push(format!("--prompt={prompt}").into());
    Ok(Plan {
        args,
        stdin,
        env,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::cli::input::Image;

    fn rendered() -> Rendered {
        Rendered {
            system: "Be brief.".into(),
            parts: vec![
                Part::Image(Image {
                    mime: "image/jpeg".into(),
                    base64: "QUJD".into(),
                }),
                Part::Text("what is this?".into()),
            ],
        }
    }

    fn strs(p: &Plan) -> Vec<String> {
        p.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir()
            .join("aevaleron-cli-tests")
            .join(format!("args-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn claude_takes_the_image_on_stdin_with_no_tools() {
        let d = dir("claude");
        let job = Job {
            provider: Provider::ClaudeCli,
            model: "sonnet",
            fast: true,
            dir: &d,
            tag: "t1",
        };
        let p = plan(&job, &rendered()).unwrap();
        let a = strs(&p);
        let tools = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[tools + 1], "");
        assert!(a.get(tools + 2).is_some_and(|x| x.starts_with("--")));
        for want in [
            "-p",
            "--no-session-persistence",
            "--model=sonnet",
            "--safe-mode",
        ] {
            assert!(a.iter().any(|x| x == want), "{want}");
        }
        assert!(a.iter().any(|x| x == "--system-prompt=Be brief."));
        assert!(p.files.is_empty(), "no file for Claude");
        let line: Value = serde_json::from_slice(&p.stdin).unwrap();
        let content = &line["message"]["content"];
        assert_eq!(content[0]["source"]["data"], "QUJD");
        assert_eq!(content[0]["source"]["media_type"], "image/jpeg");
        assert_eq!(content[1]["text"], "what is this?");
        // The CLI's own model: no flag.
        let job = Job {
            model: DEFAULT_MODEL,
            ..job
        };
        let p = plan(&job, &rendered()).unwrap();
        assert!(!strs(&p).iter().any(|x| x.starts_with("--model")));
    }

    #[test]
    fn codex_and_gemini_take_files() {
        let d = dir("files");
        let job = Job {
            provider: Provider::CodexCli,
            model: DEFAULT_MODEL,
            fast: false,
            dir: &d,
            tag: "t2",
        };
        let p = plan(&job, &rendered()).unwrap();
        let a = strs(&p);
        assert_eq!(p.files, vec![d.join("t2-1.jpg")]);
        assert_eq!(std::fs::read(&p.files[0]).unwrap(), b"ABC");
        let image = format!("--image={}", d.join("t2-1.jpg").display());
        assert_eq!(a.last(), Some(&image));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
        assert!(a
            .windows(2)
            .any(|w| w[0] == "--disable" && w[1] == "shell_tool"));
        let text = String::from_utf8(p.stdin).unwrap();
        assert!(text.starts_with("<instructions>\nBe brief.\n</instructions>"));
        assert!(text.ends_with("(Image 1 is attached.)\n\nwhat is this?"));

        let job = Job {
            provider: Provider::GeminiCli,
            tag: "t3",
            ..job
        };
        let p = plan(&job, &rendered()).unwrap();
        let a = strs(&p);
        assert_eq!(a.last().unwrap(), "--prompt=@t3-1.jpg\n\nwhat is this?");
        assert!(p.stdin.is_empty());
        let (name, value) = &p.env[0];
        assert_eq!(*name, "GEMINI_SYSTEM_MD");
        assert_eq!(std::fs::read_to_string(value).unwrap(), "Be brief.");
        assert_eq!(p.files.len(), 2);
        for f in &p.files {
            std::fs::remove_file(f).unwrap();
        }
    }
}

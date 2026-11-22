//! The installed AI command-line tools as providers: Claude Code (`claude`),
//! OpenAI Codex (`codex`) and Gemini CLI (`gemini`) answer on the person's
//! own subscription, with no API key. Every request kind the app sends (an
//! answer about the screen, the reader, the agent's turns and its reviewer,
//! the voice cleanup) goes through [`send`], which runs the CLI headless,
//! locked down, in a scratch folder, and hands its output back as an
//! OpenRouter-shaped reply so the rest of the model layer reads it as it
//! reads any other provider.
//!
//! - [`find`] and [`detect`]: where each CLI is and which version.
//! - `input`: the OpenAI-shaped request as the CLI's prompt and images.
//! - `args`: each CLI's command line and what goes to its stdin.
//! - `parse`: each CLI's JSON lines as text deltas, usage and failures.
//! - `run`: the process, its kill-on-close job, cancel, timeout, and the
//!   failures as the app's own error types.
//!
//! The lane note (.claude/rust-rewrite/lanes/cli-providers.md) lists the
//! flags per CLI and the versions they were checked against.

mod args;
mod find;
mod input;
mod parse;
mod run;

use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

use super::provider::Provider;

pub use find::find;
pub use run::send;

/// The model id that sends no `--model`: the CLI's own choice.
pub const DEFAULT_MODEL: &str = "default";

/// The voice cleanup waits this long for a CLI, which takes a second or
/// more just to start (the HTTP budget is 800 ms).
pub const VOICE_BUDGET: Duration = Duration::from_secs(8);

/// The command a person types for the CLI.
pub fn command_of(p: Provider) -> &'static str {
    match p {
        Provider::ClaudeCli => "claude",
        Provider::CodexCli => "codex",
        Provider::GeminiCli => "gemini",
        _ => "",
    }
}

/// The quickest model for short jobs (the voice cleanup), where the CLI
/// takes one by name.
pub fn fast_model(p: Provider) -> &'static str {
    match p {
        Provider::ClaudeCli => "haiku",
        Provider::GeminiCli => "gemini-3.8-flash",
        _ => DEFAULT_MODEL,
    }
}

/// What Settings shows for one CLI (the UI lane's `settings:cli-detect`).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliInfo {
    /// The provider id: `claude-cli`, `codex-cli` or `gemini-cli`.
    pub id: &'static str,
    pub name: &'static str,
    /// What the person types: `claude`, `codex`, `gemini`.
    pub command: &'static str,
    pub installed: bool,
    pub path: Option<String>,
    /// From `--version`, the number only (`2.1.330`).
    pub version: Option<String>,
    /// From the CLI's own status command, where it has one that spends
    /// nothing (`claude auth status`, `codex login status`); `None` when it
    /// could not say.
    pub signed_in: Option<bool>,
}

/// The last full [`detect`], for `settings:get` without waiting.
fn last() -> &'static Mutex<Vec<CliInfo>> {
    static L: std::sync::OnceLock<Mutex<Vec<CliInfo>>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(Vec::new()))
}

/// Every CLI, installed or not, with its path, version and sign-in state.
/// Runs each one's `--version` and status command (a second or two each,
/// all at once); nothing is sent to a model.
pub async fn detect() -> Vec<CliInfo> {
    let each = Provider::CLI.map(|p| inspect(p, find(p)));
    let [a, b, c] = each;
    let (a, b, c) = tokio::join!(a, b, c);
    let out = vec![a, b, c];
    *last().lock() = out.clone();
    out
}

/// [`detect`] as last run, else only what PATH shows (no version).
pub fn known() -> Vec<CliInfo> {
    let cached = last().lock().clone();
    if !cached.is_empty() {
        return cached;
    }
    Provider::CLI
        .into_iter()
        .map(|p| {
            let path = find(p);
            CliInfo {
                installed: path.is_some(),
                path: path.map(|f| f.to_string_lossy().into_owned()),
                ..blank(p)
            }
        })
        .collect()
}

fn blank(p: Provider) -> CliInfo {
    CliInfo {
        id: p.id(),
        name: p.name(),
        command: command_of(p),
        ..CliInfo::default()
    }
}

/// One CLI at `path`: its version and whether it is signed in.
pub async fn inspect(p: Provider, path: Option<PathBuf>) -> CliInfo {
    let Some(path) = path.filter(|f| f.is_file()) else {
        return blank(p);
    };
    let version = run::version(&path).await;
    let signed_in = run::signed_in(p, &path).await;
    CliInfo {
        installed: true,
        path: Some(path.to_string_lossy().into_owned()),
        version,
        signed_in,
        ..blank(p)
    }
}

/// The first version number in a `--version` line: `2.1.330 (Claude Code)`
/// and `codex-cli 0.166.0` both give the number.
fn version_of(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            w.contains('.')
                && w.starts_with(|c: char| c.is_ascii_digit())
                && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        })
        .map(str::to_string)
}

/// The file name a Settings row shows for a CLI's program.
pub fn hint_of(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_from_each_cli() {
        let v = |t: &str| version_of(t);
        assert_eq!(v("2.1.330 (Claude Code)").as_deref(), Some("2.1.330"));
        assert_eq!(v("codex-cli 0.166.0").as_deref(), Some("0.166.0"));
        assert_eq!(v("0.31.0-nightly\n").as_deref(), Some("0.31.0-nightly"));
        assert_eq!(v("v1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(v("no version here"), None);
    }

    #[test]
    fn names_and_commands() {
        let ids: Vec<&str> = known().iter().map(|c| c.id).collect();
        assert_eq!(ids, ["claude-cli", "codex-cli", "gemini-cli"]);
        assert_eq!(command_of(Provider::CodexCli), "codex");
        assert_eq!(command_of(Provider::OpenRouter), "");
        assert_eq!(hint_of(r"C:\Users\a\.local\bin\claude.exe"), "claude.exe");
        let json = serde_json::to_value(blank(Provider::GeminiCli)).unwrap();
        assert_eq!(json["signedIn"], serde_json::Value::Null);
        assert_eq!(json["command"], "gemini");
    }
}

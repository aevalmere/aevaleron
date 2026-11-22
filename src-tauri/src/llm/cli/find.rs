//! Where each CLI is, and how to start it without a console window or a
//! `cmd.exe` in between.
//!
//! The search covers this process's PATH, the person's and the machine's
//! PATH as stored in the registry (the app may have been started before an
//! install changed it), npm's global folder (`%APPDATA%\npm`, where the
//! `codex` and `gemini` shims land) and `%USERPROFILE%\.local\bin` (Claude
//! Code's native installer). An npm `.cmd` shim is not run through
//! `cmd.exe`, whose quoting cannot carry a prompt safely: the script it
//! names is run with `node` directly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use super::command_of;
use crate::llm::errors::{codes, ApiError};
use crate::llm::provider::Provider;

/// A found program is looked for again after this long, so an install or
/// uninstall shows up without a restart.
const FIND_TTL: Duration = Duration::from_secs(30);

/// How a CLI is started: the program and the arguments that come before
/// the CLI's own (the script, for an npm shim).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: PathBuf,
    pub lead: Vec<OsString>,
}

type FindCache = Vec<(Provider, Instant, Option<PathBuf>)>;

fn cache() -> &'static Mutex<FindCache> {
    static C: std::sync::OnceLock<Mutex<FindCache>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// The CLI's program, or `None` when it is not installed where people
/// install it. Cached for half a minute.
pub fn find(p: Provider) -> Option<PathBuf> {
    if !p.is_cli() {
        return None;
    }
    if let Some((_, _, hit)) = cache()
        .lock()
        .iter()
        .find(|(q, at, _)| *q == p && at.elapsed() < FIND_TTL)
    {
        return hit.clone();
    }
    let hit = search(command_of(p), &search_dirs());
    let mut c = cache().lock();
    c.retain(|(q, _, _)| *q != p);
    c.push((p, Instant::now(), hit.clone()));
    hit
}

/// The first `<command>.exe` or `<command>.cmd` in `dirs`, a native
/// program before a shim in the same folder.
pub fn search(command: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        for ext in ["exe", "cmd"] {
            let path = dir.join(format!("{command}.{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Every folder a CLI may be in, without repeats, in search order.
fn search_dirs() -> Vec<PathBuf> {
    let mut lists: Vec<String> = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        lists.push(path.to_string_lossy().into_owned());
    }
    lists.extend(registry_paths());
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |d: PathBuf| {
        let key = d.to_string_lossy().to_lowercase();
        let known = dirs
            .iter()
            .any(|x| x.to_string_lossy().to_lowercase() == key);
        if !key.is_empty() && !known {
            dirs.push(d);
        }
    };
    for list in &lists {
        for part in list.split(';') {
            let part = expand(part.trim().trim_matches('"'));
            if !part.is_empty() {
                add(PathBuf::from(part));
            }
        }
    }
    if let Some(app) = std::env::var_os("APPDATA") {
        add(PathBuf::from(app).join("npm"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let home = PathBuf::from(home);
        add(home.join(".local").join("bin"));
        add(home.join(".bun").join("bin"));
    }
    dirs
}

/// The person's and the machine's `Path` as the registry holds them.
fn registry_paths() -> Vec<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let read = |root, at: &str| -> Option<String> {
        RegKey::predef(root)
            .open_subkey(at)
            .ok()?
            .get_value("Path")
            .ok()
    };
    let machine = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
    [
        read(HKEY_CURRENT_USER, "Environment"),
        read(HKEY_LOCAL_MACHINE, machine),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// `%NAME%` replaced by the variable's value; unknown names stay as they
/// are.
fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(len) = after.find('%') else {
            break;
        };
        out.push_str(&rest[..start]);
        let name = &after[..len];
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => out.push_str(&value),
            _ => {
                out.push('%');
                out.push_str(name);
                out.push('%');
            }
        }
        rest = &after[len + 1..];
    }
    out.push_str(rest);
    out
}

fn missing(text: String) -> ApiError {
    ApiError::other(text).with_code(codes::CLI_MISSING)
}

/// How to start the program at `path`: an `.exe` as it is, an npm `.cmd`
/// shim as `node <script>`, a `.js` script with `node`.
pub fn launch(path: &Path) -> Result<Launch, ApiError> {
    if !path.is_file() {
        return Err(missing(format!("{} is not there", path.display())));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new("."));
    match ext.as_str() {
        "exe" | "com" => Ok(Launch {
            program: path.to_path_buf(),
            lead: Vec::new(),
        }),
        "cmd" | "bat" => {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let script = shim_target(&text, dir)
                .filter(|s| s.is_file())
                .ok_or_else(|| missing(format!("{} is not an npm shim", path.display())))?;
            Ok(Launch {
                program: node_for(dir)?,
                lead: vec![script.into_os_string()],
            })
        }
        "js" | "mjs" | "cjs" => Ok(Launch {
            program: node_for(dir)?,
            lead: vec![path.as_os_str().to_os_string()],
        }),
        _ => Err(missing(format!("{} can't be started", path.display()))),
    }
}

/// The script an npm (cmd-shim) `.cmd` file runs:
/// `"%dp0%\node_modules\@openai\codex\bin\codex.js" %*`, or the older
/// `"%~dp0\node_modules\...js"`.
pub fn shim_target(text: &str, dir: &Path) -> Option<PathBuf> {
    for marker in ["\"%dp0%\\", "\"%~dp0\\"] {
        for (at, _) in text.match_indices(marker) {
            let rest = &text[at + marker.len()..];
            let Some(end) = rest.find('"') else {
                continue;
            };
            let rel = &rest[..end];
            let lower = rel.to_lowercase();
            if [".js", ".mjs", ".cjs"].iter().any(|e| lower.ends_with(e)) {
                let mut out = dir.to_path_buf();
                for piece in rel.split(['\\', '/']).filter(|s| !s.is_empty()) {
                    out.push(piece);
                }
                return Some(out);
            }
        }
    }
    None
}

/// The `node.exe` beside a shim (what the shim itself prefers), else the
/// first one on the search path.
fn node_for(dir: &Path) -> Result<PathBuf, ApiError> {
    let beside = dir.join("node.exe");
    if beside.is_file() {
        return Ok(beside);
    }
    search("node", &search_dirs()).ok_or_else(|| missing("node.exe was not found".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("aevaleron-cli-tests")
            .join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const CODEX_SHIM: &str = r#"@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0

IF EXIST "%dp0%\node.exe" (
  SET "_prog=%dp0%\node.exe"
) ELSE (
  SET "_prog=node"
)

endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & set PATHEXT=%PATHEXT:;.JS;=;% & "%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %*
"#;

    #[test]
    fn npm_shims_run_their_script_with_node() {
        let dir = temp("shim");
        let got = shim_target(CODEX_SHIM, &dir).unwrap();
        let want = dir
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        assert_eq!(got, want);
        let old = r#"@"%~dp0\node.exe" "%~dp0\node_modules\@google\gemini-cli\dist\index.js" %*"#;
        let got = shim_target(old, &dir).unwrap();
        assert!(got.ends_with("index.js"));
        assert!(shim_target("@echo off\r\nexit /b 1", &dir).is_none());

        // With node.exe beside the shim, that one runs the script.
        std::fs::create_dir_all(want.parent().unwrap()).unwrap();
        std::fs::write(&want, "// codex").unwrap();
        std::fs::write(dir.join("node.exe"), "").unwrap();
        let shim = dir.join("codex.cmd");
        std::fs::write(&shim, CODEX_SHIM).unwrap();
        let l = launch(&shim).unwrap();
        assert_eq!(l.program, dir.join("node.exe"));
        assert_eq!(l.lead, vec![want.into_os_string()]);
        // A .cmd that is not a shim is refused, never run through cmd.exe.
        let other = dir.join("other.cmd");
        std::fs::write(&other, "@echo hi").unwrap();
        let e = launch(&other).unwrap_err();
        assert_eq!(e.code, Some(codes::CLI_MISSING));
        let e = launch(&dir.join("gone.exe")).unwrap_err();
        assert_eq!(e.code, Some(codes::CLI_MISSING));
    }

    #[test]
    fn search_prefers_the_native_program() {
        let a = temp("search-a");
        let b = temp("search-b");
        std::fs::write(a.join("codex.cmd"), CODEX_SHIM).unwrap();
        std::fs::write(b.join("claude.exe"), "").unwrap();
        std::fs::write(b.join("claude.cmd"), "").unwrap();
        let dirs = vec![a.clone(), b.clone()];
        assert_eq!(search("codex", &dirs), Some(a.join("codex.cmd")));
        assert_eq!(search("claude", &dirs), Some(b.join("claude.exe")));
        assert_eq!(search("gemini", &dirs), None);
        let exe = launch(&b.join("claude.exe")).unwrap();
        assert!(exe.lead.is_empty());
    }

    #[test]
    fn path_entries_are_expanded() {
        std::env::set_var("AEVAL_CLI_TEST_DIR", r"C:\x");
        assert_eq!(expand(r"%AEVAL_CLI_TEST_DIR%\bin"), r"C:\x\bin");
        assert_eq!(expand("%NOT_SET_ANYWHERE_42%"), "%NOT_SET_ANYWHERE_42%");
        assert_eq!(expand("50%"), "50%");
        assert!(find(Provider::OpenRouter).is_none());
    }
}

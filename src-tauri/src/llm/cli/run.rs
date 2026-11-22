//! One CLI run: the process with no console window, in a job object that
//! kills it and everything it started when the run ends, is cancelled,
//! times out, or the app exits; its output read line by line into the same
//! reply shape an HTTP provider gives; and its failures as the app's error
//! types.
//!
//! The process starts the moment the request is known, and its input goes
//! in while it boots: the screenshot rides on Claude Code's stdin, so the
//! CLI's start-up and the image transfer overlap.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio_util::sync::CancellationToken;

use super::args::{self, Job as RunJob};
use super::find::{self, Launch};
use super::input;
use super::parse::{Decoder, Event};
use crate::llm::client::{BodyStream, Meta, Reply};
use crate::llm::errors::{codes, ApiError};
use crate::llm::provider::Provider;
use crate::llm::settings::Active;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// No run goes on longer than this, whatever the caller's own timeouts.
const MAX_RUN: Duration = Duration::from_secs(10 * 60);

/// After stdout closes, how long the process gets to exit by itself.
const EXIT_WAIT: Duration = Duration::from_secs(5);

/// `--version` and the status commands.
const QUICK_WAIT: Duration = Duration::from_secs(15);

/// Files a crashed run left behind are swept after this long.
const STALE: Duration = Duration::from_secs(60 * 60);

/// The most stderr kept for the error text.
const STDERR_CAP: usize = 64 * 1024;

/// What the driver hands the reader.
enum Msg {
    Text(String),
    Usage(u64, u64),
    End,
    Fail(ApiError),
}

/// A job object that kills every process in it when it is closed: on drop,
/// and by the system if the app itself dies.
struct Job(isize);

impl Job {
    fn new() -> Job {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        unsafe {
            let Ok(job) = CreateJobObjectW(None, PCWSTR::null()) else {
                return Job(0);
            };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of_val(&info) as u32;
            let ptr: *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION = &info;
            let class = JobObjectExtendedLimitInformation;
            if SetInformationJobObject(job, class, ptr.cast(), size).is_err() {
                let _ = CloseHandle(job);
                return Job(0);
            }
            Job(job.0 as isize)
        }
    }

    fn handle(&self) -> windows::Win32::Foundation::HANDLE {
        windows::Win32::Foundation::HANDLE(self.0 as *mut std::ffi::c_void)
    }

    /// Puts the child in the job. A CLI that forks (Codex's script starts
    /// its native binary) has its children follow it in.
    fn assign(&self, child: &Child) {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        let Some(raw) = child.raw_handle() else {
            return;
        };
        if self.0 != 0 {
            // kill_on_drop still stops the CLI itself if this fails.
            let _ = unsafe { AssignProcessToJobObject(self.handle(), HANDLE(raw)) };
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::JobObjects::TerminateJobObject;
        if self.0 == 0 {
            return;
        }
        unsafe {
            let _ = TerminateJobObject(self.handle(), 1);
            let _ = CloseHandle(self.handle());
        }
    }
}

/// The run's own files, deleted when it ends. A file the dying process
/// still holds is retried off this thread for a few seconds.
struct Files(Vec<PathBuf>);

impl Drop for Files {
    fn drop(&mut self) {
        let left: Vec<PathBuf> = self
            .0
            .drain(..)
            .filter(|f| std::fs::remove_file(f).is_err() && f.exists())
            .collect();
        if left.is_empty() {
            return;
        }
        std::thread::spawn(move || {
            for _ in 0..30 {
                std::thread::sleep(Duration::from_millis(200));
                if left
                    .iter()
                    .all(|f| std::fs::remove_file(f).is_ok() || !f.exists())
                {
                    return;
                }
            }
        });
    }
}

/// Everything one run owns, killed and cleaned up in this order on drop.
struct Running {
    _job: Job,
    child: Child,
    _files: Files,
    stdin: Vec<u8>,
}

/// The empty folder each CLI runs in: one per CLI, so the CLIs that record
/// a project per folder see one project, not one per question. Old files
/// from a run that never cleaned up are swept.
fn work_dir(p: Provider) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("aevaleron-cli").join(p.id());
    std::fs::create_dir_all(&dir)?;
    if let Ok(list) = std::fs::read_dir(&dir) {
        let now = SystemTime::now();
        for e in list.flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > STALE);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(dir)
}

fn tag() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let salt: u32 = rand::random();
    format!("run-{}-{n}-{salt:08x}", std::process::id())
}

/// The command for `launch` plus `args`, with no console window and every
/// pipe the run reads or writes.
fn command(launch: &Launch, args: &[OsString]) -> tokio::process::Command {
    use std::os::windows::process::CommandExt;
    let mut std_cmd = std::process::Command::new(&launch.program);
    std_cmd
        .args(&launch.lead)
        .args(args)
        .env("NO_COLOR", "1")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut cmd = tokio::process::Command::from(std_cmd);
    cmd.kill_on_drop(true);
    cmd
}

fn spawn_error(p: Provider, e: &std::io::Error) -> ApiError {
    let name = p.name();
    ApiError::other(format!("{name} could not be started: {e}")).with_code(codes::CLI_MISSING)
}

/// Sends one OpenAI-shaped request body to the CLI `active` names (its
/// key is the program's path) and returns its answer as OpenRouter would:
/// an SSE stream for `stream: true`, else one JSON body. A stream comes
/// back once the first words do, so a CLI that fails before answering
/// fails here with its own error.
pub async fn send(
    active: &Active,
    body: &Value,
    meta: &Meta,
    cancel: &CancellationToken,
) -> Result<Reply, ApiError> {
    let provider = active.provider;
    let launch = find::launch(Path::new(active.key.trim()))?;
    let rendered = input::render(body);
    let fast = matches!(meta.purpose.as_str(), "voice-cleanup" | "agent-review");
    let dir = work_dir(provider).map_err(|e| spawn_error(provider, &e))?;
    let tag = tag();
    let job = RunJob {
        provider,
        model: body["model"].as_str().unwrap_or_default(),
        fast,
        dir: &dir,
        tag: &tag,
    };
    let plan = args::plan(&job, &rendered).map_err(|e| spawn_error(provider, &e))?;
    let files = Files(plan.files);
    let mut cmd = command(&launch, &plan.args);
    cmd.current_dir(&dir);
    for (k, v) in &plan.env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().map_err(|e| spawn_error(provider, &e))?;
    let job = Job::new();
    job.assign(&child);
    let run = Running {
        _job: job,
        child,
        _files: files,
        stdin: plan.stdin,
    };
    let (tx, rx) = unbounded_channel();
    tokio::spawn(drive(provider, run, tx));
    let stream = body["stream"].as_bool().unwrap_or(false);
    collect(rx, stream, cancel).await
}

fn sse(payload: &Value) -> Vec<u8> {
    format!("data: {payload}\n\n").into_bytes()
}

fn sse_text(text: &str) -> Vec<u8> {
    sse(&json!({ "choices": [{ "delta": { "content": text } }] }))
}

fn usage_json(prompt: u64, completion: u64) -> Value {
    json!({ "prompt_tokens": prompt, "completion_tokens": completion })
}

fn sse_usage(prompt: u64, completion: u64) -> Vec<u8> {
    sse(&json!({ "choices": [], "usage": usage_json(prompt, completion) }))
}

const DONE: &[u8] = b"data: [DONE]\n\n";

/// Waits for the answer to start (a stream) or finish (a whole reply).
async fn collect(
    mut rx: UnboundedReceiver<Msg>,
    stream: bool,
    cancel: &CancellationToken,
) -> Result<Reply, ApiError> {
    let mut text = String::new();
    let mut usage: Option<(u64, u64)> = None;
    loop {
        let msg = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(ApiError::abort()),
            m = rx.recv() => m,
        };
        match msg {
            None => return Err(ApiError::http(500, "the CLI stopped without answering")),
            Some(Msg::Fail(e)) => return Err(e),
            Some(Msg::Usage(p, c)) => usage = Some((p, c)),
            Some(Msg::Text(t)) if stream => {
                let mut first = Vec::new();
                if let Some((p, c)) = usage {
                    first.push(sse_usage(p, c));
                }
                first.push(sse_text(&t));
                return Ok(stream_reply(first, rx));
            }
            Some(Msg::Text(t)) => text.push_str(&t),
            Some(Msg::End) if stream => {
                let mut body = Vec::new();
                if let Some((p, c)) = usage {
                    body.extend(sse_usage(p, c));
                }
                body.extend_from_slice(DONE);
                return Ok(Reply::full(200, body));
            }
            Some(Msg::End) => {
                let mut whole = json!({
                    "choices": [{ "message": { "role": "assistant", "content": text } }],
                });
                if let Some((p, c)) = usage {
                    whole["usage"] = usage_json(p, c);
                }
                return Ok(Reply::full(200, whole.to_string()));
            }
        }
    }
}

/// The rest of a streamed answer, as SSE lines. Dropping it stops the run.
fn stream_reply(first: Vec<Vec<u8>>, rx: UnboundedReceiver<Msg>) -> Reply {
    let head = futures_util::stream::iter(first.into_iter().map(Ok::<Vec<u8>, ApiError>));
    let rest = futures_util::stream::unfold((rx, false), |(rx, ended)| next_sse(rx, ended));
    let body: BodyStream = Box::pin(futures_util::StreamExt::chain(head, rest));
    Reply {
        status: 200,
        retry_after: None,
        body,
    }
}

/// The stream's state between chunks: the channel, and whether it ended.
type SseState = (UnboundedReceiver<Msg>, bool);

/// The next SSE chunk of a streamed answer.
async fn next_sse(
    mut rx: UnboundedReceiver<Msg>,
    ended: bool,
) -> Option<(Result<Vec<u8>, ApiError>, SseState)> {
    if ended {
        return None;
    }
    let item = match rx.recv().await? {
        Msg::Text(t) => Ok(sse_text(&t)),
        Msg::Usage(p, c) => Ok(sse_usage(p, c)),
        Msg::End => return Some((Ok(DONE.to_vec()), (rx, true))),
        Msg::Fail(e) => return Some((Err(e), (rx, true))),
    };
    Some((item, (rx, false)))
}

/// Runs the process to its end, unless the reader goes away first (cancel,
/// or the caller's timeout), and then kills what is left of it.
async fn drive(provider: Provider, mut run: Running, tx: UnboundedSender<Msg>) {
    {
        let work = tokio::time::timeout(MAX_RUN, pump(provider, &mut run, &tx));
        tokio::select! {
            _ = tx.closed() => {}
            done = work => {
                if done.is_err() {
                    let secs = MAX_RUN.as_secs();
                    let e = ApiError::http(408, format!("no answer after {secs} s"));
                    let _ = tx.send(Msg::Fail(e));
                }
            }
        }
    }
    drop(run);
}

#[derive(Default)]
struct Outcome {
    done: bool,
    failed: Option<String>,
}

async fn pump(provider: Provider, run: &mut Running, tx: &UnboundedSender<Msg>) {
    let input = std::mem::take(&mut run.stdin);
    let stdin = run.child.stdin.take();
    let write = async move {
        if let Some(mut w) = stdin {
            let _ = w.write_all(&input).await;
            let _ = w.flush().await;
            // Dropped here: the CLI sees the end of its input.
        }
    };
    let stderr = run.child.stderr.take();
    let read_err = async move {
        let mut said: Vec<u8> = Vec::new();
        if let Some(mut e) = stderr {
            let mut buf = [0u8; 4096];
            while let Ok(n) = e.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                said.extend_from_slice(&buf[..n]);
                if said.len() > STDERR_CAP {
                    let cut = said.len() - STDERR_CAP;
                    said.drain(..cut);
                }
            }
        }
        String::from_utf8_lossy(&said).into_owned()
    };
    let stdout = run.child.stdout.take();
    let mut decoder = Decoder::new(provider);
    let read_out = async {
        let mut state = Outcome::default();
        let Some(out) = stdout else {
            return state;
        };
        let mut reader = BufReader::new(out);
        let mut line: Vec<u8> = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            for ev in decoder.line(&String::from_utf8_lossy(&line)) {
                match ev {
                    Event::Text(t) => {
                        let _ = tx.send(Msg::Text(t));
                    }
                    Event::Usage(p, c) => {
                        let _ = tx.send(Msg::Usage(p, c));
                    }
                    Event::Done if state.failed.is_none() && !state.done => {
                        // The answer is whole; whatever the CLI does next
                        // (flushing, exiting) need not hold it up.
                        state.done = true;
                        let _ = tx.send(Msg::End);
                    }
                    Event::Done => {}
                    Event::Failed(s) => state.failed = Some(s),
                }
            }
        }
        state
    };
    let ((), said, state) = tokio::join!(write, read_err, read_out);
    let exit = match tokio::time::timeout(EXIT_WAIT, run.child.wait()).await {
        Ok(Ok(status)) => status.code(),
        _ => None,
    };
    if state.done {
        return;
    }
    let notes = decoder.notes.join("\n");
    let msg = match state.failed {
        Some(f) => Msg::Fail(classify(
            provider,
            exit,
            &[f.as_str(), notes.as_str(), said.as_str()],
        )),
        None if exit == Some(0) => Msg::End,
        None => Msg::Fail(classify(provider, exit, &[notes.as_str(), said.as_str()])),
    };
    let _ = tx.send(msg);
}

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

const LOGIN_WORDS: [&str; 14] = [
    "not logged in",
    "please run /login",
    "/login",
    "codex login",
    "login required",
    "not authenticated",
    "authentication_failed",
    "authentication failed",
    "unauthorized",
    "invalid api key",
    "auth method",
    "sign in",
    "log in",
    "oauth",
];

const LIMIT_WORDS: [&str; 10] = [
    "usage limit",
    "rate limit",
    "rate_limit",
    "limit reached",
    "hit your limit",
    "quota",
    "too many requests",
    "resource_exhausted",
    "exhausted your",
    "billing_error",
];

const NETWORK_WORDS: [&str; 9] = [
    "enotfound",
    "econnrefused",
    "econnreset",
    "etimedout",
    "getaddrinfo",
    "fetch failed",
    "network error",
    "connection error",
    "failed to connect",
];

/// The first thing the CLI said, as one short line for the error text.
fn detail(parts: &[&str]) -> String {
    let said = parts
        .iter()
        .flat_map(|p| p.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    let mut out: String = said.chars().take(300).collect();
    if out.len() < said.len() {
        out.push('\u{2026}');
    }
    out
}

/// A CLI's failure as the error an HTTP provider would have given: not
/// signed in is a rejected key (401), a plan limit is a rate limit (429),
/// an unknown model is a 404, no connection is offline, anything else a
/// server error the person can retry. Gemini CLI exits 41 when it is not
/// signed in.
pub fn classify(p: Provider, exit: Option<i32>, parts: &[&str]) -> ApiError {
    let name = p.name();
    let all = parts.join("\n").to_lowercase();
    let said = detail(parts);
    let said = if said.is_empty() {
        match exit {
            Some(code) => format!("{name} stopped with exit code {code}"),
            None => format!("{name} stopped"),
        }
    } else {
        said
    };
    if (p == Provider::GeminiCli && exit == Some(41)) || has(&all, &LOGIN_WORDS) {
        return ApiError::http(401, format!("{name} is not signed in: {said}"));
    }
    if has(&all, &LIMIT_WORDS) {
        return ApiError::http(429, said);
    }
    let model_gone = all.contains("model")
        && has(
            &all,
            &[
                "not found",
                "does not exist",
                "not supported",
                "unknown model",
                "invalid model",
            ],
        );
    if model_gone {
        return ApiError::http(404, said);
    }
    if has(&all, &NETWORK_WORDS) {
        return ApiError::network(said);
    }
    ApiError::http(500, said)
}

/// A short command (`--version`, a status check) and what it printed, both
/// streams, or `None` when it would not run in time.
async fn quick(path: &Path, args: &[&str]) -> Option<(Option<i32>, String)> {
    let launch = find::launch(path).ok()?;
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let mut cmd = command(&launch, &args);
    cmd.stdin(Stdio::null()).current_dir(std::env::temp_dir());
    let child = cmd.spawn().ok()?;
    let job = Job::new();
    job.assign(&child);
    let out = tokio::time::timeout(QUICK_WAIT, child.wait_with_output())
        .await
        .ok()?
        .ok()?;
    drop(job);
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Some((out.status.code(), text))
}

/// The CLI's version number, from `--version`.
pub async fn version(path: &Path) -> Option<String> {
    let (_, text) = quick(path, &["--version"]).await?;
    super::version_of(&text)
}

/// Whether the CLI is signed in, from the status command it has that
/// spends nothing: `claude auth status --json`, `codex login status`.
/// Gemini CLI has none; its saved Google sign-in or an API key in the
/// environment count as yes, anything else as not known.
pub async fn signed_in(p: Provider, path: &Path) -> Option<bool> {
    match p {
        Provider::ClaudeCli => {
            let (_, text) = quick(path, &["auth", "status", "--json"]).await?;
            let json = text
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
                .find(|v| v.is_object())
                .or_else(|| serde_json::from_str::<Value>(text.trim()).ok());
            if let Some(v) = json.as_ref().and_then(|v| v["loggedIn"].as_bool()) {
                return Some(v);
            }
            said_signed_in(&text)
        }
        Provider::CodexCli => {
            let (_, text) = quick(path, &["login", "status"]).await?;
            said_signed_in(&text)
        }
        Provider::GeminiCli => {
            let keyed = ["GEMINI_API_KEY", "GOOGLE_API_KEY"]
                .iter()
                .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()));
            let home = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
            let saved = home.join(".gemini").join("oauth_creds.json").is_file();
            (keyed || saved).then_some(true)
        }
        _ => None,
    }
}

fn said_signed_in(text: &str) -> Option<bool> {
    let t = text.to_lowercase();
    if t.contains("not logged in") || t.contains("not signed in") {
        Some(false)
    } else if t.contains("logged in") || t.contains("signed in") {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::catalogue::Kind;
    use crate::llm::client::{ChatTurn, Llm, QueryOpts, Transport};
    use crate::llm::errors::{explain, About};
    use crate::llm::sse::Meter;
    use crate::llm::wire::HttpRequest;
    use async_trait::async_trait;
    use std::sync::Arc;

    const FAKE: &str = include_str!("fake_cli.js");

    struct NoHttp;

    #[async_trait]
    impl Transport for NoHttp {
        async fn send(&self, _: HttpRequest, _: &CancellationToken) -> Result<Reply, ApiError> {
            Err(ApiError::network("no HTTP in these tests"))
        }
    }

    /// The fake CLI script, or `None` (the test is skipped) where node is
    /// not installed. CI has it.
    fn fake() -> Option<String> {
        let dir = std::env::temp_dir()
            .join("aevaleron-cli-tests")
            .join(format!("fake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("fake-cli.js");
        std::fs::write(&script, FAKE).unwrap();
        if find::launch(&script).is_err() {
            eprintln!("node.exe not found: skipping the fake CLI tests");
            return None;
        }
        Some(script.to_string_lossy().into_owned())
    }

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    fn llm() -> Llm {
        let mut llm = Llm::new(Arc::new(NoHttp), None);
        llm.set_offline(false);
        llm
    }

    fn active(p: Provider, script: &str) -> Active {
        Active {
            provider: p,
            key: script.to_string(),
        }
    }

    /// Streams one screen question to the fake playing `p`.
    fn ask(p: Provider, script: &str, model: &str, question: &str) -> Result<String, ApiError> {
        let llm = llm();
        let history = vec![
            ChatTurn {
                role: "user".into(),
                text: "before".into(),
                kind: None,
            },
            ChatTurn {
                role: "assistant".into(),
                text: "seen".into(),
                kind: None,
            },
        ];
        let o = QueryOpts {
            model,
            question,
            image_base64: Some("QUJD"),
            image_width: 1600,
            image_height: 900,
            history: &history,
            ..QueryOpts::default()
        };
        let cancel = CancellationToken::new();
        let mut out = String::new();
        let mut meter = Meter::default();
        let mut push = |d: &str| out.push_str(d);
        let a = active(p, script);
        block(llm.stream_query(&a, o, &cancel, &mut push, &mut meter))?;
        Ok(out)
    }

    #[test]
    fn each_cli_streams_an_answer() {
        let Some(script) = fake() else {
            return;
        };
        for p in Provider::CLI {
            let got = ask(p, &script, "ok", "what now?").unwrap();
            assert_eq!(got, "Hello", "{p:?}");
            // A whole reply, as the reader and the agent take it.
            let llm = llm();
            let cancel = CancellationToken::new();
            let msgs = vec![json!({ "role": "user", "content": "go" })];
            let meta = Meta::purpose("agent");
            let a = active(p, &script);
            let turn = block(llm.chat_turn(&a, "ok", &msgs, None, &cancel, &meta)).unwrap();
            assert_eq!(turn.content, "Hello", "{p:?}");
        }
    }

    fn report(p: Provider, script: &str) -> Value {
        let got = ask(p, script, "echo", "what is this?").unwrap();
        serde_json::from_str(&got).unwrap()
    }

    #[test]
    fn the_screenshot_reaches_each_cli() {
        let Some(script) = fake() else {
            return;
        };
        let has_arg = |r: &Value, a: &str| r["argv"].as_array().unwrap().iter().any(|x| x == a);

        let r = report(Provider::ClaudeCli, &script);
        assert_eq!(r["flavour"], "claude");
        let line: Value = serde_json::from_str(r["input"].as_str().unwrap().trim()).unwrap();
        let content = line["message"]["content"].as_array().unwrap();
        let image = content.iter().find(|b| b["type"] == "image").unwrap();
        assert_eq!(image["source"]["data"], "QUJD");
        let last = content.last().unwrap()["text"].as_str().unwrap();
        assert!(last.ends_with("what is this?"));
        assert!(content[0]["text"]
            .as_str()
            .unwrap()
            .contains("User: before"));
        assert!(has_arg(&r, "--tools") && has_arg(&r, "--no-session-persistence"));
        assert!(r["argv"].as_array().unwrap().iter().any(|a| a
            .as_str()
            .is_some_and(|s| s.starts_with("--system-prompt="))));
        let cwd = r["cwd"].as_str().unwrap();
        assert!(cwd.ends_with("claude-cli"), "{cwd}");

        for p in [Provider::CodexCli, Provider::GeminiCli] {
            let r = report(p, &script);
            let shot = &r["images"][0];
            assert_eq!(shot["exists"], true, "{p:?}");
            assert_eq!(shot["data"], "QUJD", "{p:?}: the file holds the screenshot");
            let path = PathBuf::from(shot["path"].as_str().unwrap());
            // Deleted once the run ends (a moment later, off the thread).
            let gone = (0..50).any(|_| {
                std::thread::sleep(Duration::from_millis(100));
                !path.exists()
            });
            assert!(gone, "{p:?}: {path:?} was left behind");
        }
        let r = report(Provider::CodexCli, &script);
        assert!(has_arg(&r, "read-only") && has_arg(&r, "--ephemeral"));
        assert!(r["input"].as_str().unwrap().starts_with("<instructions>"));
        let r = report(Provider::GeminiCli, &script);
        assert!(r["systemMd"].as_str().is_some_and(|s| !s.is_empty()));
    }

    fn about(p: Provider) -> About<'static> {
        About::new(p, "ok", "Ok", Kind::Vision)
    }

    #[test]
    fn sign_in_and_limits_read_as_the_app_errors() {
        let Some(script) = fake() else {
            return;
        };
        for p in Provider::CLI {
            let e = ask(p, &script, "login", "q").unwrap_err();
            assert_eq!(e.status, Some(401), "{p:?}: {e:?}");
            let x = explain(&e, &about(p)).unwrap();
            assert_eq!(x.code, codes::BAD_KEY);
            let command = super::super::command_of(p);
            assert!(x.text.contains("isn't signed in"), "{}", x.text);
            assert!(x.text.contains(&format!("`{command}`")), "{}", x.text);

            let e = ask(p, &script, "limit", "q").unwrap_err();
            assert_eq!(e.status, Some(429), "{p:?}: {e:?}");
            let x = explain(&e, &about(p)).unwrap();
            assert!(x.text.contains("usage limit"), "{}", x.text);
        }
        let gone = active(Provider::CodexCli, r"C:\nowhere\codex.cmd");
        let cancel = CancellationToken::new();
        let meta = Meta::purpose("agent");
        let e = block(llm().chat_turn(&gone, "ok", &[], None, &cancel, &meta)).unwrap_err();
        assert_eq!(e.code, Some(codes::CLI_MISSING));
        let x = explain(&e, &about(Provider::CodexCli)).unwrap();
        assert!(x.text.contains("can't start Codex"), "{}", x.text);
    }

    #[test]
    fn exit_codes_and_words_are_mapped() {
        let g = Provider::GeminiCli;
        assert_eq!(classify(g, Some(41), &["odd"]).status, Some(401));
        let c = Provider::ClaudeCli;
        let e = classify(c, Some(1), &["", "Error: model claude-x not found"]);
        assert_eq!(e.status, Some(404));
        assert_eq!(e.message, "Error: model claude-x not found");
        assert!(classify(c, Some(1), &["getaddrinfo ENOTFOUND api.anthropic.com"]).is_network());
        let e = classify(c, Some(3), &["", ""]);
        assert_eq!(
            (e.status, e.message.as_str()),
            (Some(500), "Claude Code stopped with exit code 3")
        );
        assert_eq!(said_signed_in("Logged in using ChatGPT"), Some(true));
        assert_eq!(said_signed_in("Not logged in"), Some(false));
        assert_eq!(said_signed_in("?"), None);
    }

    fn beat(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The beat files stop changing: the CLI and its child are gone.
    fn assert_stopped(file: &Path) {
        let child = PathBuf::from(format!("{}.child", file.display()));
        std::thread::sleep(Duration::from_millis(800));
        // Existence, not contents: each beat rewrites (truncates) its file, so
        // a kill in mid-write can leave it empty for good.
        assert!(file.exists() && child.exists(), "both processes ran");
        let (a, b) = (beat(file), beat(&child));
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(beat(file), a, "the CLI was killed");
        assert_eq!(beat(&child), b, "the CLI's child was killed");
    }

    fn beat_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("aevaleron-cli-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join(format!("{name}-{}.beat", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let _ = std::fs::remove_file(format!("{}.child", f.display()));
        f
    }

    #[test]
    fn cancel_kills_the_cli_and_its_children() {
        let Some(script) = fake() else {
            return;
        };
        let file = beat_file("cancel");
        let llm = llm();
        let cancel = CancellationToken::new();
        let question = format!("beat:{}", file.display());
        let o = QueryOpts {
            model: "hang",
            question: &question,
            ..QueryOpts::default()
        };
        let mut got = String::new();
        let stop = cancel.clone();
        let mut on = |d: &str| {
            got.push_str(d);
            stop.cancel();
        };
        let mut meter = Meter::default();
        let a = active(Provider::CodexCli, &script);
        let e = block(llm.stream_query(&a, o, &cancel, &mut on, &mut meter)).unwrap_err();
        assert!(e.is_abort(), "{e:?}");
        assert_eq!(got, "Thinking");
        assert_stopped(&file);
    }

    #[test]
    fn a_silent_cli_times_out_and_is_killed() {
        let Some(script) = fake() else {
            return;
        };
        let file = beat_file("timeout");
        let mut llm = llm();
        llm.set_timeouts(Duration::from_millis(3000), Duration::from_millis(3000));
        let cancel = CancellationToken::new();
        let msgs = vec![json!({ "role": "user", "content": format!("beat:{}", file.display()) })];
        let meta = Meta::purpose("agent");
        let a = active(Provider::ClaudeCli, &script);
        let e = block(llm.chat_turn(&a, "slow", &msgs, None, &cancel, &meta)).unwrap_err();
        assert_eq!(e.status, Some(408), "{e:?}");
        assert_stopped(&file);
    }

    #[test]
    fn version_and_sign_in_are_read_without_a_prompt() {
        let Some(script) = fake() else {
            return;
        };
        let path = PathBuf::from(&script);
        let info = block(super::super::inspect(
            Provider::ClaudeCli,
            Some(path.clone()),
        ));
        assert!(info.installed);
        assert_eq!(info.version.as_deref(), Some("9.8.7"));
        assert_eq!(info.signed_in, Some(true));
        let codex = block(signed_in(Provider::CodexCli, &path));
        assert_eq!(codex, Some(true));
        let none = block(super::super::inspect(Provider::GeminiCli, None));
        assert!(!none.installed && none.version.is_none());
    }
}

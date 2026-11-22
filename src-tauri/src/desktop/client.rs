//! The desktop engine and every call into it: a port of
//! lib/desktop/index.js (PARITY DESK-01..12). The engine itself is
//! `host`, in process (it used to be host.cs inside a powershell.exe child);
//! it runs on a worker thread of its own that takes one request at a time.
//!
//! Lazy: nothing starts until the first call. One op at a time, in order.
//! A worker that panics rejects in-flight calls with HELPER_DOWN and the next
//! call starts a fresh one; three deaths inside 60 s stop restarting until
//! `start()`. Every rect and point is physical virtual-screen pixels.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot};

use super::host;
use super::protocol::{self as p, DResult, DesktopError};
use super::scoring;
use super::win;
use crate::capture::screen;

/// One of our windows, as the engine's own-window registry wants it. A
/// `block: false` layer (lens, pointer) is ours but never in the way of a
/// click that lands under it.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnWindow {
    pub handle: u64,
    pub block: bool,
}

type Hook<T> = Option<Arc<dyn Fn() -> T + Send + Sync>>;
type InputHook = Arc<dyn Fn(&Value) + Send + Sync>;

/// What main.js passed to `desktop.init`. Every field is optional; the
/// defaults read the Tauri app's own windows.
#[derive(Clone, Default)]
pub struct Deps {
    pub own_windows: Hook<Vec<OwnWindow>>,
    pub want_input: Hook<bool>,
    pub on_user_input: Option<InputHook>,
    /// Called before any `key()` that sends Escape, so main can drop its
    /// global Esc (RegisterHotKey fires on injected input too).
    pub before_synthetic_key: Hook<()>,
    /// Called 60 ms after such a key, so the injected key has cleared the
    /// hook before Stop is held again.
    pub after_synthetic_key: Hook<()>,
}

/// The running worker: requests go to it over `tx`.
struct Proc {
    gen: u64,
    tx: std::sync::mpsc::Sender<host::Job>,
}

#[derive(Default)]
struct State {
    proc: Option<Proc>,
    ready: bool,
    ready_info: Option<Value>,
    pending: HashMap<u64, oneshot::Sender<DResult<Value>>>,
    deaths: Vec<Instant>,
    disabled: bool,
    last_stderr: String,
    sent_own: String,
    watching: bool,
    watch_override: Option<bool>,
    uia_warm: bool,
    next_id: u64,
    gen: u64,
    stopping: HashSet<u64>,
}

impl State {
    fn record_death(&mut self) {
        let now = Instant::now();
        self.deaths
            .retain(|t| now.duration_since(*t) < Duration::from_millis(p::DEATH_WINDOW_MS));
        self.deaths.push(now);
        if self.deaths.len() >= p::DEATH_LIMIT {
            self.disabled = true;
        }
    }

    /// A worker that stopped answering: nothing more is sent to it, and its
    /// end, if it ever comes, is not counted as a second death.
    fn abandon(&mut self, gen: u64) {
        if self.proc.as_ref().map(|p| p.gen) == Some(gen) {
            self.proc = None;
            self.ready = false;
        }
        self.record_death();
        self.stopping.insert(gen);
    }

    fn fail_all(&mut self, code: &str, message: &str) {
        for (_, tx) in self.pending.drain() {
            let _ = tx.send(Err(DesktopError::new(code, message)));
        }
    }

    fn stderr_note(&mut self, s: &str) {
        self.last_stderr.push_str(s);
        let len = self.last_stderr.len();
        if len > 4096 {
            let mut cut = len - 4096;
            while !self.last_stderr.is_char_boundary(cut) {
                cut += 1;
            }
            self.last_stderr.drain(..cut);
        }
    }
}

pub struct Desktop {
    state: Mutex<State>,
    queue: tokio::sync::Mutex<()>,
    starting: tokio::sync::Mutex<()>,
    deps: RwLock<Deps>,
    app: RwLock<Option<crate::shell::AppHandle>>,
    events: broadcast::Sender<Value>,
}

fn system32() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    PathBuf::from(root).join("System32")
}

/// A Windows tool by absolute path, so nothing earlier on PATH or in the
/// working directory runs in its place.
pub fn system_tool(name: &str) -> PathBuf {
    system32().join(name)
}

pub fn powershell() -> PathBuf {
    system32()
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe")
}

/// Closes the worker's books when its thread ends, by return or by panic.
struct WorkerExit {
    me: Arc<Desktop>,
    gen: u64,
    why: Option<String>,
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        let why = self.why.take().unwrap_or_else(|| {
            if std::thread::panicking() {
                "the desktop worker panicked".into()
            } else {
                "the desktop worker ended".into()
            }
        });
        self.me.on_exit(self.gen, &why);
    }
}

/// One of our windows for the engine's registry. The handle comes from the
/// cache when the window is registered, because `read` hops to the main
/// thread (B-140).
fn own_window(label: &str, cached: Option<isize>, read: impl FnOnce() -> u64) -> OwnWindow {
    OwnWindow {
        handle: cached.map_or_else(read, |h| h as u64),
        block: !(label.starts_with("lens-") || label == "buddy"),
    }
}

fn arg_window(v: Option<&Value>) -> Value {
    match v {
        None | Some(Value::Null) => Value::Null,
        Some(h) => p::handle_of(h).map(|n| json!(n)).unwrap_or(Value::Null),
    }
}

impl Desktop {
    pub fn new() -> Arc<Desktop> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Desktop {
            state: Mutex::new(State {
                next_id: 1,
                ..Default::default()
            }),
            queue: tokio::sync::Mutex::new(()),
            starting: tokio::sync::Mutex::new(()),
            deps: RwLock::new(Deps::default()),
            app: RwLock::new(None),
            events,
        })
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replaces the hooks main.js passed to `init`.
    pub fn init(&self, deps: Deps) {
        if let Ok(mut d) = self.deps.write() {
            *d = deps;
        }
        self.st().sent_own.clear();
    }

    /// Lets the default own-window list read the app's windows.
    pub fn set_app(&self, app: crate::shell::AppHandle) {
        if let Ok(mut a) = self.app.write() {
            *a = Some(app);
        }
    }

    /// `ready`, `fatal` and `user-input` lines, as index.js's `events`.
    // index.js export; Called by the smoke and shot harnesses (agent e), not merged yet.
    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    pub fn is_ready(&self) -> bool {
        let s = self.st();
        s.ready && s.proc.is_some()
    }

    pub fn status(&self) -> Value {
        let s = self.st();
        let stderr = s.last_stderr.trim();
        let tail: String = {
            let chars: Vec<char> = stderr.chars().collect();
            chars[chars.len().saturating_sub(1000)..].iter().collect()
        };
        json!({
            "ready": s.ready && s.proc.is_some(),
            "disabled": s.disabled,
            "deaths": s.deaths.len(),
            // The engine runs in this process.
            "pid": s.proc.as_ref().map(|_| std::process::id()),
            "inProcess": true,
            "dpi": s.ready_info.as_ref().and_then(|r| r.get("dpi").cloned()),
            "watching": s.watching,
            "stderr": tail,
        })
    }

    // ------------------------------------------------------------ worker

    /// A `user-input` event from the engine's input watch.
    fn on_event(&self, msg: Value) {
        let _ = self.events.send(msg.clone());
        let hook = self.deps.read().ok().and_then(|d| d.on_user_input.clone());
        if let Some(h) = hook {
            // A listener error must not take the watch down.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h(&msg)));
        }
    }

    fn on_reply(&self, id: u64, out: host::Outcome) {
        let mut s = self.st();
        let tx = s.pending.remove(&id);
        let r = match out {
            host::Outcome::Reply(r) => r,
            host::Outcome::Crashed(why) => {
                s.stderr_note(&format!("\n[crash] {why}"));
                Err(DesktopError::new(
                    "HELPER_DOWN",
                    format!("desktop helper crashed: {why}"),
                ))
            }
        };
        drop(s);
        if let Some(tx) = tx {
            let _ = tx.send(r);
        }
    }

    fn on_exit(&self, gen: u64, why: &str) {
        let mut s = self.st();
        let ours = s.proc.as_ref().map(|p| p.gen) == Some(gen);
        let deliberate = s.stopping.remove(&gen);
        if ours {
            s.proc = None;
            s.ready = false;
            if !deliberate {
                s.record_death();
            }
            s.fail_all("HELPER_DOWN", &format!("desktop helper exited ({why})"));
        }
    }

    async fn spawn_helper(self: &Arc<Self>) -> DResult<()> {
        let (tx, rx) = std::sync::mpsc::channel::<host::Job>();
        let gen = {
            let mut s = self.st();
            s.gen += 1;
            let gen = s.gen;
            s.proc = Some(Proc { gen, tx });
            s.ready = false;
            s.uia_warm = false;
            s.sent_own.clear();
            s.watching = false;
            gen
        };
        // What a fresh helper process started with: no refs, no watched
        // windows, no saved cursor, no input watch.
        host::reset();
        let me = self.clone();
        host::set_sink(Arc::new(move |v| me.on_event(v)));
        let me = self.clone();
        let started = std::thread::Builder::new()
            .name("desktop-worker".into())
            .spawn(move || {
                let mut exit = WorkerExit {
                    me: me.clone(),
                    gen,
                    why: None,
                };
                let why = std::cell::RefCell::new(None);
                host::serve(rx, |id, out| {
                    if let host::Outcome::Crashed(w) = &out {
                        *why.borrow_mut() = Some(w.clone());
                    }
                    me.on_reply(id, out);
                });
                exit.why = why.into_inner();
                drop(exit);
            });
        if let Err(e) = started {
            self.st().abandon(gen);
            return Err(DesktopError::new(
                "HELPER_DOWN",
                format!("could not start the desktop worker: {e}"),
            ));
        }
        let ready = host::ready_event();
        {
            let mut s = self.st();
            if s.proc.as_ref().map(|p| p.gen) != Some(gen) {
                return Err(DesktopError::new(
                    "HELPER_DOWN",
                    "desktop helper exited during start",
                ));
            }
            s.ready = true;
            s.ready_info = Some(ready.clone());
        }
        let _ = self.events.send(ready);
        Ok(())
    }

    /// Idempotent; also re-enables after three crashes.
    pub async fn start(self: &Arc<Self>) -> DResult<()> {
        if self.is_ready() {
            return Ok(());
        }
        {
            let mut s = self.st();
            s.disabled = false;
            s.deaths.clear();
        }
        self.ensure().await
    }

    async fn ensure(self: &Arc<Self>) -> DResult<()> {
        if self.is_ready() {
            return Ok(());
        }
        if self.st().disabled {
            return Err(DesktopError::new(
                "HELPER_DOWN",
                "desktop control stopped after three crashes in a minute; call start() to try again",
            ));
        }
        let _g = self.starting.lock().await;
        if self.is_ready() {
            return Ok(());
        }
        self.spawn_helper().await
    }

    /// Ends the worker (its queue closes) and the input watch; call before
    /// quitting.
    pub fn stop(&self) {
        let proc = {
            let mut s = self.st();
            s.ready = false;
            s.watching = false;
            s.fail_all("HELPER_DOWN", "desktop helper stopped");
            let proc = s.proc.take();
            if let Some(p) = &proc {
                s.stopping.insert(p.gen);
            }
            proc
        };
        if proc.is_some() {
            host::cancel_all();
            host::stop_watch();
        }
    }

    // ------------------------------------------------------------ calls

    fn own_config(&self) -> Value {
        let mut pids = win::own_pids();
        let deps = self.deps.read().ok().map(|d| d.clone()).unwrap_or_default();
        let list = match deps.own_windows {
            Some(f) => f(),
            None => self.default_own_windows(),
        };
        pids.sort_unstable();
        pids.dedup();
        let windows: Vec<Value> = list
            .iter()
            .filter(|w| w.handle != 0)
            .map(|w| json!({ "handle": w.handle, "block": w.block }))
            .collect();
        json!({ "pids": pids, "windows": windows })
    }

    /// Every app window; the lens and the pointer as non-blocking layers.
    fn default_own_windows(&self) -> Vec<OwnWindow> {
        use crate::shell::Manager;
        let Some(app) = self.app.read().ok().and_then(|a| a.clone()) else {
            return Vec::new();
        };
        app.webview_windows()
            .into_iter()
            .map(|(label, w)| {
                let cached = crate::windows::flags::hwnd_for(&label);
                own_window(&label, cached, || crate::capture::hwnd_of(&w) as u64)
            })
            .collect()
    }

    fn want_watch(&self) -> bool {
        if let Some(v) = self.st().watch_override {
            return v;
        }
        let deps = self.deps.read().ok().map(|d| d.clone()).unwrap_or_default();
        match deps.want_input {
            Some(f) => f(),
            None => deps.on_user_input.is_some() || self.events.receiver_count() > 0,
        }
    }

    async fn send(&self, op: &str, args: &Value, timeout_ms: u64) -> DResult<Value> {
        let (id, rx, gen) = {
            let mut s = self.st();
            if !s.ready || s.proc.is_none() {
                return Err(DesktopError::new(
                    "HELPER_DOWN",
                    "desktop helper is not running",
                ));
            }
            let id = s.next_id;
            s.next_id += 1;
            let (tx, rx) = oneshot::channel();
            s.pending.insert(id, tx);
            let proc = s.proc.as_ref().expect("checked above");
            let gen = proc.gen;
            let args = if args.is_null() {
                json!({})
            } else {
                args.clone()
            };
            let job = host::Job::new(id, op, args, timeout_ms);
            if proc.tx.send(job).is_err() {
                s.pending.remove(&id);
                return Err(DesktopError::new(
                    "HELPER_DOWN",
                    "desktop helper is not running",
                ));
            }
            (id, rx, gen)
        };
        let wait = Duration::from_millis(timeout_ms + p::REPLY_GRACE);
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => Err(DesktopError::new("HELPER_DOWN", "desktop helper stopped")),
            Err(_) => {
                // A worker that stops answering entirely is left behind: the
                // next call gets a fresh one. Whatever it is still running may
                // no longer act.
                {
                    let mut s = self.st();
                    s.pending.remove(&id);
                    s.abandon(gen);
                    // Nothing else waiting on this worker will hear from it.
                    s.fail_all("HELPER_DOWN", "desktop helper stopped answering");
                }
                host::cancel_all();
                Err(DesktopError::new(
                    "TIMEOUT",
                    format!(
                        "{op} got no answer in {}ms; restarting the helper",
                        timeout_ms + p::REPLY_GRACE
                    ),
                ))
            }
        }
    }

    /// One op at a time, in order: input has to land in the order it was
    /// asked for. `op = None` only syncs the own-window list and the watch.
    ///
    /// Private apps: an op that would read or act on a listed app is not
    /// sent, and a read's answer from one goes no further.
    pub async fn call(
        self: &Arc<Self>,
        op: Option<&str>,
        mut args: Value,
        timeout_ms: u64,
    ) -> DResult<Value> {
        let _q = self.queue.lock().await;
        // Asked first, so a refused op does not even start the helper, and
        // again just before the op is sent.
        if let Some(op) = op {
            crate::privacy::check_op(op, &mut args)?;
        }
        self.ensure().await?;
        let own = self.own_config();
        let key = own.to_string();
        if key != self.st().sent_own {
            self.send("config", &own, p::OP_TIMEOUT).await?;
            self.st().sent_own = key;
        }
        let want = self.want_watch();
        if want != self.st().watching {
            self.send("watchInput", &json!({ "on": want }), p::OP_TIMEOUT)
                .await?;
            self.st().watching = want;
        }
        match op {
            None => Ok(Value::Null),
            Some(op) => {
                crate::privacy::check_op(op, &mut args)?;
                let reply = self.send(op, &args, timeout_ms).await?;
                crate::privacy::check_reply(op, &reply)?;
                Ok(reply)
            }
        }
    }

    async fn op(self: &Arc<Self>, op: &str, args: Value) -> DResult<Value> {
        self.call(Some(op), args, p::OP_TIMEOUT).await
    }

    /// Stops every request the helper is running or has queued from sending
    /// any more input (B-050). It skips the queue, since the request it stops
    /// is the one holding it.
    pub async fn cancel(&self) -> DResult<Value> {
        if !self.is_ready() {
            return Ok(json!({ "ok": true, "cancelled": 0 }));
        }
        Ok(json!({ "ok": true, "cancelled": host::cancel_all() }))
    }

    fn uia_timeout(&self) -> u64 {
        if self.st().uia_warm {
            p::OP_TIMEOUT
        } else {
            p::UIA_COLD_TIMEOUT
        }
    }

    // ------------------------------------------------------------ shapes

    fn window_shape(w: &Value, displays: &[screen::Display]) -> Value {
        if w.is_null() {
            return Value::Null;
        }
        let mut out = w.clone();
        if let Some(o) = out.as_object_mut() {
            let rect = p::rect_obj(o.get("rect").unwrap_or(&Value::Null));
            if let Some(r) = px_rect(&rect) {
                let dip = screen::screen_to_dip_rect(displays, r);
                o.insert("display".into(), json!(screen::matching(displays, &dip).id));
            }
            o.insert("rect".into(), rect);
            // Private apps: a listed app's window is named by its program
            // alone. Its title is text of the app's, and is not passed on.
            let handle = o.get("handle").and_then(p::handle_of).unwrap_or(0);
            let read = crate::privacy::Read::Window(handle as isize);
            if handle != 0 && crate::privacy::check(read).is_err() {
                o.insert("private".into(), json!(true));
                o.insert("title".into(), json!(""));
            }
        }
        out
    }

    // ------------------------------------------------------------ windows

    pub async fn list_windows(self: &Arc<Self>, include_minimized: bool) -> DResult<Value> {
        let list = self
            .op(
                "listWindows",
                json!({ "includeMinimized": include_minimized }),
            )
            .await?;
        let d = screen::all_displays();
        Ok(Value::Array(
            list.as_array()
                .map(|a| a.iter().map(|w| Self::window_shape(w, &d)).collect())
                .unwrap_or_default(),
        ))
    }

    pub async fn foreground_window(self: &Arc<Self>) -> DResult<Value> {
        let w = self.op("foregroundWindow", json!({})).await?;
        Ok(Self::window_shape(&w, &screen::all_displays()))
    }

    pub async fn focus_window(self: &Arc<Self>, handle: &Value) -> DResult<Value> {
        self.op("focusWindow", json!({ "handle": arg_window(Some(handle)) }))
            .await
    }

    pub async fn close_window(self: &Arc<Self>, handle: &Value) -> DResult<Value> {
        self.op("closeWindow", json!({ "handle": arg_window(Some(handle)) }))
            .await
    }

    pub async fn monitors(self: &Arc<Self>) -> DResult<Value> {
        let list = self.op("monitors", json!({})).await?;
        Ok(Value::Array(
            list.as_array()
                .map(|a| {
                    a.iter()
                        .map(|m| {
                            let mut m = m.clone();
                            if let Some(o) = m.as_object_mut() {
                                for k in ["rect", "work"] {
                                    if let Some(r) = o.get(k).cloned() {
                                        o.insert(k.into(), p::rect_obj(&r));
                                    }
                                }
                            }
                            m
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ))
    }

    // ------------------------------------------------------------ perception

    /// `opts`: windowHandle, filter ('visible'), depth (15), ref, maxChars
    /// (50000), maxNodes (3000), strategy.
    pub async fn read_tree(self: &Arc<Self>, opts: &Value) -> DResult<Value> {
        let g = |k: &str| opts.get(k).cloned().filter(|v| !v.is_null());
        let max_chars = opts
            .get("maxChars")
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(50000);
        let args = json!({
            "windowHandle": arg_window(opts.get("windowHandle")),
            "filter": g("filter").unwrap_or(json!("visible")),
            "depth": g("depth").unwrap_or(json!(15)),
            "ref": g("ref").unwrap_or(Value::Null),
            "maxNodes": g("maxNodes").unwrap_or(json!(3000)),
            "strategy": g("strategy").unwrap_or(Value::Null),
        });
        let t = self.uia_timeout();
        let r = self.call(Some("readTree"), args, t).await?;
        self.st().uia_warm = true;
        let nodes: Vec<Value> = r
            .get("nodes")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(p::node_shape).collect())
            .unwrap_or_default();
        let (text, cut) = p::render_tree(&nodes, max_chars);
        let truncated = r.get("truncated").and_then(Value::as_bool).unwrap_or(false) || cut;
        Ok(json!({
            "text": text,
            "nodes": nodes,
            "truncated": truncated,
            "generation": r.get("generation").cloned().unwrap_or(Value::Null),
            "windowHandle": r.get("windowHandle").cloned().unwrap_or(Value::Null),
            "window": Self::window_shape(r.get("window").unwrap_or(&Value::Null), &screen::all_displays()),
            "readMs": r.get("readMs").cloned().unwrap_or(Value::Null),
        }))
    }

    /// Local literal and fuzzy search over one window's tree (minScore 45).
    pub async fn find(
        self: &Arc<Self>,
        query: &str,
        window_handle: Option<&Value>,
        limit: usize,
        min_score: i64,
    ) -> DResult<Value> {
        if query.trim().is_empty() {
            return Err(DesktopError::new("BAD_ARGS", "find needs a query"));
        }
        let mut opts =
            json!({ "filter": "all", "depth": 60, "maxNodes": 4000, "maxChars": u64::MAX });
        if let Some(h) = window_handle {
            opts["windowHandle"] = h.clone();
        }
        let tree = self.read_tree(&opts).await?;
        let mut scored: Vec<(i64, i64, Value)> = Vec::new();
        for n in tree["nodes"].as_array().cloned().unwrap_or_default() {
            let depth = n.get("depth").and_then(Value::as_i64).unwrap_or(0);
            if depth == 0 {
                continue;
            }
            let score = scoring::score_node(&n, query);
            if score >= min_score {
                let mut n = n;
                n["score"] = json!(score);
                scored.push((score, depth, n));
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let keep = limit.clamp(1, 100);
        let nodes: Vec<Value> = scored.into_iter().take(keep).map(|(_, _, n)| n).collect();
        let text = nodes
            .iter()
            .map(|n| p::node_line(n, 0))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(json!({
            "nodes": nodes,
            "text": text,
            "windowHandle": tree["windowHandle"],
            "generation": tree["generation"],
        }))
    }

    pub async fn get_text(
        self: &Arc<Self>,
        window_handle: Option<&Value>,
        rect: &Value,
        max_chars: u64,
        ocr_fallback: bool,
    ) -> DResult<Value> {
        let args = json!({ "windowHandle": arg_window(window_handle), "rect": p::rect_arg(rect), "maxChars": max_chars });
        let t = if ocr_fallback {
            p::OCR_TIMEOUT
        } else {
            p::OP_TIMEOUT
        };
        let r = self.call(Some("getText"), args, t).await?;
        let spans: Vec<Value> = r["spans"]
            .as_array()
            .map(|a| a.iter().map(span_shape).collect())
            .unwrap_or_default();
        let text = r["text"].as_str().unwrap_or("").to_string();
        let mut out = json!({
            "text": text,
            "source": "uia",
            "spans": spans,
            "truncated": r["truncated"].as_bool().unwrap_or(false),
            "windowHandle": r["windowHandle"],
        });
        let dense = text.chars().filter(|c| !c.is_whitespace()).count();
        if !ocr_fallback || dense >= 200 {
            return Ok(out);
        }
        let ocr_rect = if rect.is_null() {
            p::rect_obj(&r["windowRect"])
        } else {
            rect.clone()
        };
        let o = match self.ocr(&ocr_rect, Some(&r["windowHandle"]), "auto").await {
            Ok(o) => o,
            Err(e) => {
                out["ocrError"] = e.to_json();
                return Ok(out);
            }
        };
        let ospans = o["spans"].as_array().cloned().unwrap_or_default();
        let extra: Vec<Value> = ospans
            .iter()
            .filter(|s| !spans.iter().any(|u| overlaps(&u["rect"], &s["rect"])))
            .cloned()
            .collect();
        if spans.is_empty() {
            out["text"] = o["text"].clone();
            out["source"] = json!("ocr");
            out["spans"] = Value::Array(ospans);
            return Ok(out);
        }
        if extra.is_empty() {
            return Ok(out);
        }
        let joined = extra
            .iter()
            .map(|s| s["text"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ");
        let mut all = spans.clone();
        all.extend(extra);
        out["source"] = json!("mixed");
        out["spans"] = Value::Array(all);
        out["text"] = json!(format!("{text}{joined}\n"));
        Ok(out)
    }

    pub async fn ocr(
        self: &Arc<Self>,
        rect: &Value,
        window_handle: Option<&Value>,
        lang: &str,
    ) -> DResult<Value> {
        let args = json!({ "rect": p::rect_arg(rect), "windowHandle": arg_window(window_handle), "lang": lang });
        let r = self.call(Some("ocr"), args, p::OCR_TIMEOUT).await?;
        Ok(json!({
            "spans": r["spans"].as_array().map(|a| a.iter().map(span_shape).collect::<Vec<_>>()).unwrap_or_default(),
            "text": r["text"],
            "lang": r["lang"],
            "rect": p::rect_obj(&r["rect"]),
            "ocrMs": r["ocrMs"],
        }))
    }

    fn capture_shape(r: &Value) -> Value {
        json!({ "dataUrl": r["dataUrl"], "rect": p::rect_obj(&r["rect"]), "scale": r["scale"], "width": r["width"], "height": r["height"] })
    }

    pub async fn screenshot(
        self: &Arc<Self>,
        window_handle: Option<&Value>,
        rect: &Value,
        max_edge: u64,
    ) -> DResult<Value> {
        let args = json!({ "windowHandle": arg_window(window_handle), "rect": p::rect_arg(rect), "maxEdge": max_edge });
        let r = self.call(Some("capture"), args, p::CAPTURE_TIMEOUT).await?;
        Ok(Self::capture_shape(&r))
    }

    pub async fn zoom(self: &Arc<Self>, rect: &Value, max_edge: u64) -> DResult<Value> {
        if rect.is_null() {
            return Err(DesktopError::new("BAD_ARGS", "zoom needs a rect"));
        }
        let args = json!({ "rect": p::rect_arg(rect), "maxEdge": max_edge, "enlarge": true });
        let r = self.call(Some("capture"), args, p::CAPTURE_TIMEOUT).await?;
        Ok(Self::capture_shape(&r))
    }

    pub async fn element_at(self: &Arc<Self>, x: i64, y: i64) -> DResult<Value> {
        let r = self.op("elementAt", json!({ "x": x, "y": y })).await?;
        Ok(json!({ "node": p::node_shape(&r["node"]), "windowHandle": r["windowHandle"] }))
    }

    /// Read-only window facts for the self-check's gates.
    pub async fn window_states(self: &Arc<Self>, args: Value) -> DResult<Value> {
        let base = json!({ "handles": [], "title": "", "point": null, "move": false });
        self.op("windowStates", p::merge(&base, &args)).await
    }

    /// The element with keyboard focus, when `window_handle`'s window (or
    /// any window, with none) is in front: `{node, windowHandle}`, `node`
    /// null otherwise. The agent's gate checks what a Space would press.
    pub async fn focused(self: &Arc<Self>, window_handle: Option<&Value>) -> DResult<Value> {
        let args = json!({ "windowHandle": arg_window(window_handle) });
        let r = self.op("focused", args).await?;
        Ok(json!({ "node": p::node_shape(&r["node"]), "windowHandle": r["windowHandle"] }))
    }

    pub async fn resolve(self: &Arc<Self>, r#ref: &str) -> DResult<Value> {
        let r = self.op("resolve", json!({ "ref": r#ref })).await?;
        Ok(json!({ "node": p::node_shape(&r["node"]), "windowHandle": r["windowHandle"] }))
    }

    // ------------------------------------------------------------ action

    /// `t` is `{ref}` or `{x, y}`, merged with the options as index.js did.
    pub async fn click(self: &Arc<Self>, t: &Value) -> DResult<Value> {
        let mut args = p::target(t)?;
        let count = t["count"]
            .as_u64()
            .unwrap_or(if t["double"].as_bool().unwrap_or(false) {
                2
            } else {
                1
            });
        args["button"] = json!(t["button"].as_str().unwrap_or("left"));
        args["count"] = json!(count);
        args["modifiers"] = json!(t["modifiers"].as_str().unwrap_or(""));
        args["physical"] = json!(t["physical"].as_bool().unwrap_or(false));
        args["force"] = json!(t["force"].as_bool().unwrap_or(false));
        // A click that names its window lands there or not at all.
        if let Some(w) = t.get("inWindow").filter(|w| !w.is_null()) {
            args["inWindow"] = arg_window(Some(w));
        }
        Ok(p::point_out(self.op("click", args).await?))
    }

    pub async fn hover(self: &Arc<Self>, t: &Value) -> DResult<Value> {
        let args = p::target(t)?;
        Ok(p::point_out(self.op("hover", args).await?))
    }

    pub async fn drag(
        self: &Arc<Self>,
        from: &Value,
        to: &Value,
        steps: Option<u64>,
    ) -> DResult<Value> {
        if from.is_null() || to.is_null() {
            return Err(DesktopError::new("BAD_ARGS", "drag needs from and to"));
        }
        let args = json!({ "from": p::target(from)?, "to": p::target(to)?, "steps": steps });
        let r = self.op("drag", args).await?;
        Ok(json!({
            "ok": r["ok"],
            "from": { "x": r["from"][0], "y": r["from"][1] },
            "to": { "x": r["to"][0], "y": r["to"][1] },
        }))
    }

    pub async fn type_text(
        self: &Arc<Self>,
        text: &str,
        r#ref: Option<&str>,
        window_handle: Option<&Value>,
    ) -> DResult<Value> {
        let args = json!({ "text": text, "ref": r#ref, "windowHandle": arg_window(window_handle) });
        let t = p::type_timeout(text.encode_utf16().count());
        self.call(Some("type"), args, t).await
    }

    pub async fn key(
        self: &Arc<Self>,
        keys: &str,
        repeat: u64,
        window_handle: Option<&Value>,
    ) -> DResult<Value> {
        if keys.trim().is_empty() {
            return Err(DesktopError::new("BAD_ARGS", "key needs keys"));
        }
        let guard = p::sends_escape(keys);
        let deps = self.deps.read().ok().map(|d| d.clone()).unwrap_or_default();
        if guard {
            if let Some(f) = &deps.before_synthetic_key {
                f();
            }
        }
        let window = arg_window(window_handle);
        let args = json!({ "keys": keys, "repeat": repeat, "windowHandle": window });
        let r = self
            .call(Some("key"), args, p::key_timeout(keys, repeat))
            .await;
        if guard {
            if let Some(f) = deps.after_synthetic_key.clone() {
                tokio::time::sleep(Duration::from_millis(60)).await;
                f();
            }
        }
        r
    }

    pub async fn scroll(self: &Arc<Self>, t: &Value) -> DResult<Value> {
        let mut args = p::target(t)?;
        args["direction"] = json!(t["direction"].as_str().unwrap_or("down"));
        args["amount"] = json!(t["amount"].as_u64().filter(|v| *v > 0).unwrap_or(3));
        args["physical"] = json!(t["physical"].as_bool().unwrap_or(false));
        args["force"] = json!(t["force"].as_bool().unwrap_or(false));
        Ok(p::point_out(self.op("scroll", args).await?))
    }

    /// The full text of the window's documents and editors, in view or
    /// scrolled away, up to `max_chars` across the window: `{docs: [{ref,
    /// text, seen, truncated}], total, truncated, readMs}`. `seen` is the
    /// start of what the application reports as visible. The helper stops
    /// starting new documents after `budget_ms`.
    pub async fn doc_text(
        self: &Arc<Self>,
        window_handle: Option<&Value>,
        max_chars: u64,
        budget_ms: u64,
    ) -> DResult<Value> {
        let args = json!({
            "windowHandle": arg_window(window_handle),
            "maxChars": max_chars,
            "budgetMs": budget_ms,
        });
        self.op("docText", args).await
    }

    pub async fn scroll_to(self: &Arc<Self>, r#ref: &str) -> DResult<Value> {
        let r = self.op("scrollTo", json!({ "ref": r#ref })).await?;
        Ok(json!({ "ok": r["ok"], "node": p::node_shape(&r["node"]) }))
    }

    pub async fn set_value(self: &Arc<Self>, r#ref: &str, value: Value) -> DResult<Value> {
        let mut r = self
            .op("setValue", json!({ "ref": r#ref, "value": value }))
            .await?;
        if !r["node"].is_null() {
            r["node"] = p::node_shape(&r["node"]);
        }
        Ok(r)
    }

    pub async fn invoke(self: &Arc<Self>, r#ref: &str, action: &str) -> DResult<Value> {
        self.op("invoke", json!({ "ref": r#ref, "action": action }))
            .await
    }

    pub async fn restore_cursor(self: &Arc<Self>) -> DResult<Value> {
        Ok(p::point_out(self.op("restoreCursor", json!({})).await?))
    }

    /// Turns the physical-input watch on or off now, instead of on next call.
    // index.js export; Called by the smoke and shot harnesses (agent e), not merged yet.
    #[allow(dead_code)]
    pub async fn watch_user_input(self: &Arc<Self>, on: bool) -> DResult<Value> {
        self.st().watch_override = Some(on);
        self.call(None, Value::Null, p::OP_TIMEOUT).await?;
        Ok(json!({ "watching": self.st().watching }))
    }

    /// Hands the watch back to the app after [`Desktop::watch_user_input`].
    pub async fn release_watch(self: &Arc<Self>) -> DResult<Value> {
        self.st().watch_override = None;
        self.call(None, Value::Null, p::OP_TIMEOUT).await?;
        Ok(json!({ "watching": self.st().watching }))
    }

    /// Test hook: makes the helper emit one user-input event.
    // index.js export; Called by the smoke and shot harnesses (agent e), not merged yet.
    #[allow(dead_code)]
    pub async fn simulate_user_input(self: &Arc<Self>, kind: &str) -> DResult<Value> {
        self.op("testUserInput", json!({ "kind": kind })).await
    }

    pub async fn ping(self: &Arc<Self>) -> DResult<Value> {
        self.op("ping", json!({})).await
    }

    // ------------------------------------------------------------ DIP

    /// Physical rect (or point, with no width) to DIP.
    // index.js export; Called by the smoke and shot harnesses (agent e), not merged yet.
    #[allow(dead_code)]
    pub fn to_dip(&self, r: &Value) -> Value {
        let d = screen::all_displays();
        let rect = p::rect_obj(r);
        let point = r.get("width").is_none() && !r.is_array();
        let Some(px) = px_rect(&p::merge(&json!({ "width": 0, "height": 0 }), &rect)) else {
            return Value::Null;
        };
        let out = screen::screen_to_dip_rect(&d, px);
        if point {
            json!({ "x": out.x, "y": out.y })
        } else {
            json!(out)
        }
    }

    /// DIP rect (or point) to physical.
    // index.js export; Called by the smoke and shot harnesses (agent e), not merged yet.
    #[allow(dead_code)]
    pub fn to_physical(&self, r: &Value) -> Value {
        let d = screen::all_displays();
        let rect = p::rect_obj(r);
        let point = r.get("width").is_none() && !r.is_array();
        let full = p::merge(&json!({ "width": 0, "height": 0 }), &rect);
        let f = |k: &str| full.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let dip = screen::Rect::new(f("x"), f("y"), f("width"), f("height"));
        let out = screen::dip_to_screen_rect(&d, dip);
        if point {
            json!({ "x": out.x, "y": out.y })
        } else {
            json!(out)
        }
    }
}

fn px_rect(v: &Value) -> Option<screen::PxRect> {
    let f = |k: &str| v.get(k).and_then(Value::as_f64);
    Some(screen::PxRect {
        x: f("x")?.round() as i32,
        y: f("y")?.round() as i32,
        width: f("width").unwrap_or(0.0).round() as i32,
        height: f("height").unwrap_or(0.0).round() as i32,
    })
}

fn span_shape(s: &Value) -> Value {
    let mut out = json!({
        "text": s["text"],
        "rect": p::rect_obj(&s["rect"]),
        "ref": if s["ref"].is_null() { Value::Null } else { s["ref"].clone() },
        "source": s["source"],
    });
    // What the span is: its role and patterns, and the button, link or
    // item it sits in.
    for key in ["role", "pat", "enabled", "line", "via"] {
        if let Some(v) = s.get(key).filter(|v| !v.is_null()) {
            out[key] = v.clone();
        }
    }
    out
}

fn overlaps(a: &Value, b: &Value) -> bool {
    let g = |v: &Value, k: &str| v.get(k).and_then(Value::as_f64);
    let (Some(ax), Some(ay), Some(aw), Some(ah)) =
        (g(a, "x"), g(a, "y"), g(a, "width"), g(a, "height"))
    else {
        return false;
    };
    let (Some(bx), Some(by), Some(bw), Some(bh)) =
        (g(b, "x"), g(b, "y"), g(b, "width"), g(b, "height"))
    else {
        return false;
    };
    ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_rule() {
        let a = json!({"x":0,"y":0,"width":10,"height":10});
        assert!(overlaps(&a, &json!({"x":5,"y":5,"width":10,"height":10})));
        assert!(!overlaps(&a, &json!({"x":10,"y":0,"width":5,"height":5})));
        assert!(!overlaps(&a, &Value::Null));
    }

    #[test]
    fn deaths_disable_after_three_in_a_minute() {
        let mut s = State::default();
        s.record_death();
        s.record_death();
        assert!(!s.disabled);
        s.record_death();
        assert!(s.disabled);
    }

    fn proc_of(gen: u64) -> Proc {
        let (tx, _rx) = std::sync::mpsc::channel();
        Proc { gen, tx }
    }

    /// A helper that never says ready is a death like any other, and no
    /// call may go to it while it is being killed.
    #[test]
    fn start_timeout_counts_as_death() {
        let mut s = State::default();
        for gen in 1..=3 {
            s.proc = Some(proc_of(gen));
            s.ready = true;
            assert!(!s.disabled);
            s.abandon(gen);
            assert!(s.proc.is_none() && !s.ready);
            assert!(s.stopping.contains(&gen));
        }
        assert!(s.disabled);
    }

    /// A late timeout from an old helper must not take the new one away.
    #[test]
    fn abandoning_an_old_helper_leaves_the_new_one() {
        let mut s = State {
            proc: Some(proc_of(9)),
            ready: true,
            ..State::default()
        };
        s.abandon(8);
        assert!(s.proc.is_some() && s.ready);
    }

    #[test]
    fn system_tools_use_absolute_paths() {
        let p = system_tool("cmd.exe");
        assert!(p.is_absolute(), "{p:?}");
        assert!(p.ends_with("System32\\cmd.exe"), "{p:?}");
        assert!(powershell().is_absolute());
    }

    /// Reading a handle off a registered window must not need the main
    /// thread.
    #[test]
    fn own_window_handles_come_from_the_cache() {
        let bar = own_window("bar", Some(42), || panic!("hopped to the main thread"));
        assert_eq!(
            bar,
            OwnWindow {
                handle: 42,
                block: true
            }
        );
        let lens = own_window("lens-0", None, || 7);
        assert_eq!(
            lens,
            OwnWindow {
                handle: 7,
                block: false
            }
        );
    }

    #[test]
    fn fail_all_rejects_every_pending_call() {
        let mut s = State::default();
        let (tx, mut rx) = oneshot::channel();
        s.pending.insert(1, tx);
        s.fail_all("HELPER_DOWN", "gone");
        assert_eq!(rx.try_recv().unwrap().unwrap_err().code, "HELPER_DOWN");
        assert!(s.pending.is_empty());
    }

    /// The engine answers in process: no child, our own pid, and a reply
    /// through the same queue every op takes.
    #[tokio::test]
    async fn the_engine_answers_in_process() {
        let d = Desktop::new();
        d.spawn_helper().await.unwrap();
        assert!(d.is_ready());
        assert_eq!(d.status()["pid"], json!(std::process::id()));
        let pong = d.send("ping", &json!({}), p::OP_TIMEOUT).await.unwrap();
        assert_eq!(pong["inProcess"], json!(true));
        let bad = d.send("nope", &json!({}), p::OP_TIMEOUT).await;
        assert_eq!(bad.unwrap_err().code, "BAD_ARGS");
        d.stop();
        assert!(!d.is_ready());
    }

    /// Attack on timing: `key tab` repeated 100 times outlasts the plain
    /// op timeout, and a request the client gave up on must not still be
    /// pressing keys afterwards. The wait grows with every press instead.
    #[test]
    fn key_timeout_scales_with_repeat() {
        assert_eq!(p::key_timeout("ctrl+s", 1), p::OP_TIMEOUT);
        assert_eq!(p::key_timeout("tab", 100), 6000);
        assert_eq!(p::key_timeout("tab tab space", 100), 14000);
        assert!(p::key_timeout("tab", 100) > p::key_timeout("tab", 75));
        // host.cs caps the repeat at 100, so the wait does too.
        assert_eq!(p::key_timeout("tab", 1000), p::key_timeout("tab", 100));
        assert_eq!(p::key_timeout("", 0), p::OP_TIMEOUT);
    }

    #[test]
    fn helper_is_down_until_started() {
        let d = Desktop::new();
        assert!(!d.is_ready());
        assert_eq!(d.status()["ready"], json!(false));
        assert_eq!(d.status()["pid"], Value::Null);
    }
}

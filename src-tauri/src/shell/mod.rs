//! The app shell that replaced Tauri: one process, one UI thread, native
//! Win32 windows drawn by Slint's software renderer (platform.rs), and the
//! small surface of Tauri's API the core was written against, so the rest
//! of the crate kept its shape (`AppHandle`, `Manager`, `Emitter`, a
//! window handle per label, `async_runtime`).
//!
//! Events still go to a window by label with a JSON payload: the pages are
//! Rust now (src/ui), and `emit_to` hands the payload to the page's
//! `on_event` on the UI thread, in order, with no serialisation to text.

mod builder;
pub mod platform;
pub mod sys;

pub use builder::{webview, BuildSpec, WebviewUrl, WebviewWindowBuilder};

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::num::NonZeroIsize;
use std::sync::OnceLock;

use parking_lot::{Mutex, RwLock};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use serde::Serialize;
use serde_json::Value;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetForegroundWindow, GetWindowRect, IsWindow, IsWindowVisible, PostMessageW,
    SetWindowPos, ShowWindow, HWND_NOTOPMOST, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WM_CLOSE,
};

/// What a shell call can fail with.
#[derive(Debug, Clone)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Error(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Error(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Managed state is process-wide and never dropped, so a reference to it
/// lives as long as anyone asks.
pub type State<'a, T> = &'a T;

// ----------------------------------------------------------- geometry

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalPosition<T> {
    pub x: T,
    pub y: T,
}

impl<T> PhysicalPosition<T> {
    pub fn new(x: T, y: T) -> Self {
        PhysicalPosition { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PhysicalSize<T> {
    pub width: T,
    pub height: T,
}

impl<T> PhysicalSize<T> {
    pub fn new(width: T, height: T) -> Self {
        PhysicalSize { width, height }
    }
}

impl<T: Copy + Into<f64>> PhysicalSize<T> {
    /// In DIPs at `scale`.
    pub fn to_logical<X: From<f64>>(&self, scale: f64) -> LogicalSize<X> {
        let s = if scale > 0.0 { scale } else { 1.0 };
        LogicalSize {
            width: X::from(self.width.into() / s),
            height: X::from(self.height.into() / s),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalSize<T> {
    pub width: T,
    pub height: T,
}

// ------------------------------------------------------------ the app

/// The one app. Every handle names the same process-wide state; cloning
/// one is free.
#[derive(Clone, Default)]
pub struct AppHandle {
    _private: (),
}

/// The version the app reports (Cargo.toml, which package.json mirrors).
pub struct PackageInfo {
    pub version: String,
}

type States = RwLock<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>;

fn states() -> &'static States {
    static S: OnceLock<States> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// label -> HWND of every live window.
fn labels() -> &'static Mutex<HashMap<String, isize>> {
    static L: OnceLock<Mutex<HashMap<String, isize>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

pub(crate) fn register_label(label: &str, hwnd: isize) {
    labels().lock().insert(label.to_string(), hwnd);
}

pub(crate) fn forget_label(label: &str, hwnd: isize) {
    let mut l = labels().lock();
    if l.get(label) == Some(&hwnd) {
        l.remove(label);
    }
}

/// The label a window was created under.
pub fn label_of(hwnd: isize) -> Option<String> {
    labels()
        .lock()
        .iter()
        .find(|(_, h)| **h == hwnd)
        .map(|(l, _)| l.clone())
}

impl AppHandle {
    pub fn get() -> AppHandle {
        AppHandle::default()
    }

    /// Keeps `value` for the life of the process; `state::<T>()` reads it.
    pub fn manage<T: Send + Sync + 'static>(&self, value: T) {
        let leaked: &'static T = Box::leak(Box::new(value));
        states().write().insert(TypeId::of::<T>(), leaked);
    }

    /// Runs `f` on the UI thread: at once when this is it (as Tauri did),
    /// else queued behind what is already waiting there.
    pub fn run_on_main_thread(&self, f: impl FnOnce() + Send + 'static) -> Result<()> {
        if platform::is_ui_thread() {
            f();
            return Ok(());
        }
        platform::post(Box::new(f))
    }

    /// Quits: the exit hooks run, then the process ends with `code`.
    pub fn exit(&self, code: i32) {
        platform::request_exit(code);
    }

    pub fn package_info(&self) -> PackageInfo {
        PackageInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Tauri's Manager trait, as far as the core used it.
pub trait Manager {
    fn app_handle(&self) -> &AppHandle;

    fn state<T: Send + Sync + 'static>(&self) -> State<'static, T> {
        self.try_state::<T>()
            .unwrap_or_else(|| panic!("state {} was never managed", std::any::type_name::<T>()))
    }

    fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'static, T>> {
        let any = *states().read().get(&TypeId::of::<T>())?;
        any.downcast_ref::<T>()
    }

    fn get_webview_window(&self, label: &str) -> Option<WebviewWindow> {
        let hwnd = *labels().lock().get(label)?;
        // SAFETY: IsWindow accepts any value.
        if unsafe { IsWindow(hwnd as HWND) } == 0 {
            return None;
        }
        Some(Window {
            label: label.to_string(),
            hwnd,
            app: AppHandle::default(),
        })
    }

    fn webview_windows(&self) -> HashMap<String, WebviewWindow> {
        let all: Vec<(String, isize)> = labels()
            .lock()
            .iter()
            .map(|(l, h)| (l.clone(), *h))
            .collect();
        all.into_iter()
            // SAFETY: as above.
            .filter(|(_, h)| unsafe { IsWindow(*h as HWND) } != 0)
            .map(|(label, hwnd)| {
                let w = Window {
                    label: label.clone(),
                    hwnd,
                    app: AppHandle::default(),
                };
                (label, w)
            })
            .collect()
    }
}

impl Manager for AppHandle {
    fn app_handle(&self) -> &AppHandle {
        self
    }
}

/// Sends an event to a page, as Tauri's `emit_to` did.
pub trait Emitter {
    fn emit_to<S: Serialize>(&self, label: &str, channel: &str, payload: S) -> Result<()> {
        let v = serde_json::to_value(payload).map_err(|e| Error(e.to_string()))?;
        deliver(label.to_string(), channel.to_string(), v);
        Ok(())
    }

    /// Every window.
    fn emit<S: Serialize>(&self, channel: &str, payload: S) -> Result<()> {
        let v = serde_json::to_value(payload).map_err(|e| Error(e.to_string()))?;
        let all: Vec<String> = labels().lock().keys().cloned().collect();
        for l in all {
            deliver(l, channel.to_string(), v.clone());
        }
        Ok(())
    }
}

impl Emitter for AppHandle {}
impl Emitter for Window {}

/// What receives page events on the UI thread (ui::deliver).
pub type EventSink = fn(&str, &str, &Value);

static SINK: OnceLock<EventSink> = OnceLock::new();

pub fn set_event_sink(f: EventSink) {
    let _ = SINK.set(f);
}

fn deliver(label: String, channel: String, payload: Value) {
    let Some(sink) = SINK.get().copied() else {
        return;
    };
    // Always queued, even on the UI thread: a page never hears an event in
    // the middle of its own call, as with Tauri.
    let _ = platform::post(Box::new(move || sink(&label, &channel, &payload)));
}

// ------------------------------------------------------------- windows

/// What happened to a window (Tauri's WindowEvent, the variants used).
#[derive(Clone, Copy, Debug)]
pub enum WindowEvent {
    Focused(bool),
    Resized(PhysicalSize<u32>),
    Moved(PhysicalPosition<i32>),
    ScaleFactorChanged { scale_factor: f64 },
    Destroyed,
}

pub type WindowEventHandler = fn(&Window, &WindowEvent);

static WINDOW_EVENTS: OnceLock<WindowEventHandler> = OnceLock::new();

pub fn set_window_event_handler(f: WindowEventHandler) {
    let _ = WINDOW_EVENTS.set(f);
}

type OwnHandler = Box<dyn Fn(&WindowEvent) + Send + 'static>;

/// Handlers one window registered for itself (`Window::on_window_event`).
fn own_handlers() -> &'static Mutex<HashMap<isize, Vec<OwnHandler>>> {
    static H: OnceLock<Mutex<HashMap<isize, Vec<OwnHandler>>>> = OnceLock::new();
    H.get_or_init(Default::default)
}

/// Posted, so a handler never runs inside the window procedure.
pub(crate) fn window_event(label: &str, hwnd: isize, e: WindowEvent) {
    let w = Window::from_parts(label, hwnd);
    let _ = platform::post(Box::new(move || {
        // Taken out while they run, so one may register another.
        let own = own_handlers().lock().remove(&w.hwnd);
        if let Some(list) = own {
            for f in &list {
                f(&e);
            }
            if !matches!(e, WindowEvent::Destroyed) {
                let mut all = own_handlers().lock();
                let slot = all.entry(w.hwnd).or_default();
                let added = std::mem::replace(slot, list);
                slot.extend(added);
            }
        }
        if let Some(f) = WINDOW_EVENTS.get().copied() {
            f(&w, &e);
        }
    }));
}

/// Called once for every page built (lib.rs: `windows::on_page_loaded`).
pub type PageLoadedHandler = fn(&Window);

static PAGE_LOADED: OnceLock<PageLoadedHandler> = OnceLock::new();

pub fn set_page_loaded_handler(f: PageLoadedHandler) {
    let _ = PAGE_LOADED.set(f);
}

pub(crate) fn page_loaded(w: &Window) {
    if let Some(f) = PAGE_LOADED.get().copied() {
        f(w);
    }
}

/// One native window, by label. Cheap to clone and usable from any
/// thread: every call is a Win32 call on its HWND.
#[derive(Clone)]
pub struct Window {
    label: String,
    hwnd: isize,
    app: AppHandle,
}

/// The name the core knows windows by.
pub type WebviewWindow = Window;

fn h(x: isize) -> HWND {
    x as HWND
}

impl Window {
    pub(crate) fn from_parts(label: &str, hwnd: isize) -> Window {
        Window {
            label: label.to_string(),
            hwnd,
            app: AppHandle::default(),
        }
    }

    /// Hears what happens to this window, until it is destroyed.
    pub fn on_window_event(&self, f: impl Fn(&WindowEvent) + Send + 'static) {
        own_handlers()
            .lock()
            .entry(self.hwnd)
            .or_default()
            .push(Box::new(f));
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn alive(&self) -> Result<()> {
        // SAFETY: IsWindow accepts any value.
        if unsafe { IsWindow(h(self.hwnd)) } == 0 {
            return Err(Error(format!("window {} is gone", self.label)));
        }
        Ok(())
    }

    pub fn show(&self) -> Result<()> {
        self.alive()?;
        // SAFETY: a live window.
        unsafe { ShowWindow(h(self.hwnd), SW_SHOW) };
        Ok(())
    }

    pub fn hide(&self) -> Result<()> {
        self.alive()?;
        // SAFETY: as above.
        unsafe { ShowWindow(h(self.hwnd), SW_HIDE) };
        Ok(())
    }

    pub fn is_visible(&self) -> Result<bool> {
        self.alive()?;
        // SAFETY: as above.
        Ok(unsafe { IsWindowVisible(h(self.hwnd)) } != 0)
    }

    pub fn is_focused(&self) -> Result<bool> {
        self.alive()?;
        // SAFETY: no arguments.
        Ok(unsafe { GetForegroundWindow() } as isize == self.hwnd)
    }

    pub fn set_focus(&self) -> Result<()> {
        self.alive()?;
        let hwnd = self.hwnd;
        self.app
            .run_on_main_thread(move || platform::focus_window(hwnd))
    }

    pub fn close(&self) -> Result<()> {
        self.alive()?;
        // SAFETY: posting to a live window.
        unsafe { PostMessageW(h(self.hwnd), WM_CLOSE, 0, 0) };
        Ok(())
    }

    pub fn destroy(&self) -> Result<()> {
        self.alive()?;
        let hwnd = self.hwnd;
        self.app
            .run_on_main_thread(move || platform::destroy_window(hwnd))
    }

    pub fn set_always_on_top(&self, on: bool) -> Result<()> {
        self.alive()?;
        let after = if on { HWND_TOPMOST } else { HWND_NOTOPMOST };
        // SAFETY: a live window; nothing moves or activates.
        unsafe {
            SetWindowPos(
                h(self.hwnd),
                after,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
        Ok(())
    }

    pub fn scale_factor(&self) -> Result<f64> {
        self.alive()?;
        // SAFETY: a live window.
        let dpi = unsafe { GetDpiForWindow(h(self.hwnd)) };
        Ok(if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 })
    }

    fn window_rect(&self) -> Result<RECT> {
        self.alive()?;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: a live window and a valid out pointer.
        unsafe { GetWindowRect(h(self.hwnd), &mut r) };
        Ok(r)
    }

    pub fn inner_size(&self) -> Result<PhysicalSize<u32>> {
        self.alive()?;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: as above.
        unsafe { GetClientRect(h(self.hwnd), &mut r) };
        Ok(PhysicalSize::new(
            (r.right - r.left).max(0) as u32,
            (r.bottom - r.top).max(0) as u32,
        ))
    }

    pub fn outer_size(&self) -> Result<PhysicalSize<u32>> {
        let r = self.window_rect()?;
        Ok(PhysicalSize::new(
            (r.right - r.left).max(0) as u32,
            (r.bottom - r.top).max(0) as u32,
        ))
    }

    pub fn outer_position(&self) -> Result<PhysicalPosition<i32>> {
        let r = self.window_rect()?;
        Ok(PhysicalPosition::new(r.left, r.top))
    }

    pub fn set_position(&self, p: PhysicalPosition<i32>) -> Result<()> {
        self.alive()?;
        // SAFETY: a live window.
        unsafe {
            SetWindowPos(
                h(self.hwnd),
                std::ptr::null_mut(),
                p.x,
                p.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        Ok(())
    }

    pub fn set_size(&self, s: PhysicalSize<u32>) -> Result<()> {
        self.alive()?;
        // SAFETY: a live window.
        unsafe {
            SetWindowPos(
                h(self.hwnd),
                std::ptr::null_mut(),
                0,
                0,
                s.width as i32,
                s.height as i32,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        Ok(())
    }
}

impl Manager for Window {
    fn app_handle(&self) -> &AppHandle {
        &self.app
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let Some(nz) = NonZeroIsize::new(self.hwnd) else {
            return Err(HandleError::Unavailable);
        };
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(nz));
        // SAFETY: the HWND is valid while the window lives; callers read it
        // as an integer and check IsWindow before use.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

// -------------------------------------------------------- async runtime

/// Tauri's async_runtime: one multi-threaded tokio runtime for the app.
pub mod async_runtime {
    use std::future::Future;
    use std::sync::OnceLock;

    pub use tokio::task::JoinHandle;

    fn rt() -> &'static tokio::runtime::Runtime {
        static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
        RT.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                // Two workers carry the app: streaming answers, the helper
                // pipe and the timers. Blocking work has its own pool.
                .worker_threads(2)
                .thread_name("aeval-rt")
                .enable_all()
                .build()
                .expect("tokio runtime")
        })
    }

    pub fn spawn<F>(f: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        rt().spawn(f)
    }

    pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        rt().spawn_blocking(f)
    }

    pub fn block_on<F: Future>(f: F) -> F::Output {
        rt().block_on(f)
    }

    pub struct RuntimeHandle(tokio::runtime::Handle);

    impl RuntimeHandle {
        pub fn inner(&self) -> &tokio::runtime::Handle {
            &self.0
        }
    }

    pub fn handle() -> RuntimeHandle {
        RuntimeHandle(rt().handle().clone())
    }
}

//! Tauri's `WebviewWindowBuilder`, as far as the core used it: the same
//! calls build a native window holding the Rust page for that path
//! (`crate::ui::build`). Options every window has anyway (frameless, topmost,
//! off the taskbar) are accepted and change nothing.

use std::path::PathBuf;

use super::{AppHandle, Result, Window};

/// Which page a window shows: the path the HTML page had, which names the
/// native page now (`crate::ui::make`).
pub enum WebviewUrl {
    App(PathBuf),
}

pub mod webview {
    /// A page's life, as Tauri reported it. A native page is built at once,
    /// so both arrive, in order, right after the window is made.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PageLoadEvent {
        Started,
        Finished,
    }

    #[derive(Clone, Copy, Debug)]
    pub struct PageLoadPayload {
        pub(crate) event: PageLoadEvent,
    }

    impl PageLoadPayload {
        pub fn event(&self) -> PageLoadEvent {
            self.event
        }
    }
}

pub type PageLoadHook = Box<dyn Fn(Window, webview::PageLoadPayload) + Send + 'static>;

/// Everything a window is made from.
pub struct BuildSpec {
    pub label: String,
    pub page: String,
    pub title: String,
    pub transparent: bool,
    pub resizable: bool,
    pub maximizable: bool,
    pub focusable: bool,
    pub focused: bool,
    pub visible: bool,
    pub protect: bool,
    /// DIPs.
    pub position: Option<(f64, f64)>,
    /// DIPs.
    pub size: Option<(f64, f64)>,
    pub on_page_load: Option<PageLoadHook>,
}

pub struct WebviewWindowBuilder {
    spec: BuildSpec,
}

impl WebviewWindowBuilder {
    pub fn new(_app: &AppHandle, label: impl Into<String>, url: WebviewUrl) -> Self {
        let WebviewUrl::App(path) = url;
        WebviewWindowBuilder {
            spec: BuildSpec {
                label: label.into(),
                page: path.to_string_lossy().replace('\\', "/"),
                title: "Aevaleron".into(),
                transparent: false,
                resizable: true,
                maximizable: true,
                focusable: true,
                focused: true,
                visible: true,
                protect: false,
                position: None,
                size: None,
                on_page_load: None,
            },
        }
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.spec.title = t.into();
        self
    }

    pub fn transparent(mut self, on: bool) -> Self {
        self.spec.transparent = on;
        self
    }

    pub fn resizable(mut self, on: bool) -> Self {
        self.spec.resizable = on;
        self
    }

    pub fn maximizable(mut self, on: bool) -> Self {
        self.spec.maximizable = on;
        self
    }

    pub fn focusable(mut self, on: bool) -> Self {
        self.spec.focusable = on;
        self
    }

    pub fn focused(mut self, on: bool) -> Self {
        self.spec.focused = on;
        self
    }

    pub fn visible(mut self, on: bool) -> Self {
        self.spec.visible = on;
        self
    }

    pub fn content_protected(mut self, on: bool) -> Self {
        self.spec.protect = on;
        self
    }

    pub fn position(mut self, x: f64, y: f64) -> Self {
        self.spec.position = Some((x, y));
        self
    }

    pub fn inner_size(mut self, w: f64, h: f64) -> Self {
        self.spec.size = Some((w, h));
        self
    }

    pub fn on_page_load<F>(mut self, f: F) -> Self
    where
        F: Fn(Window, webview::PageLoadPayload) + Send + 'static,
    {
        self.spec.on_page_load = Some(Box::new(f));
        self
    }

    // Every window is a frameless, topmost tool window off the taskbar, so
    // these say what is already so.
    pub fn decorations(self, _on: bool) -> Self {
        self
    }

    pub fn always_on_top(self, _on: bool) -> Self {
        self
    }

    pub fn skip_taskbar(self, _on: bool) -> Self {
        self
    }

    pub fn minimizable(self, _on: bool) -> Self {
        self
    }

    pub fn shadow(self, _on: bool) -> Self {
        self
    }

    /// Builds the window and its page on the UI thread (waiting for it when
    /// called from another thread, as Tauri did).
    pub fn build(self) -> Result<Window> {
        crate::ui::build(self.spec)
    }
}
